//! cpal stream ownership and the per-source processing thread.
//!
//! For each source there are two std threads:
//! - the stream thread builds the cpal stream, plays it and parks until stopped.
//!   cpal streams are not Send, so the stream never leaves this thread.
//! - the processing thread receives raw buffers from the cpal callback over a
//!   std channel, downmixes, resamples to 16 kHz, runs the VAD, reports levels
//!   and forwards finished chunks.

use super::resample::{downmix, Resampler};
use super::vad::{Vad, VadChunk, VadConfig};
use crate::types::{AudioChunk, AudioSource, SAMPLE_RATE};
use anyhow::{anyhow, bail, Context};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;
use tokio::sync::mpsc::error::TrySendError;

pub type LevelCb = Arc<dyn Fn(AudioSource, f32) + Send + Sync>;
type WavFile = hound::WavWriter<BufWriter<File>>;

/// Open a 16 kHz mono 16-bit WAV for the recording of one source.
fn open_recording(path: &PathBuf) -> anyhow::Result<WavFile> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).context("create the audio folder")?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    hound::WavWriter::create(path, spec).with_context(|| format!("create {}", path.display()))
}

/// Report a level after this many 16 kHz samples (50 ms, about 20 per second).
const LEVEL_WINDOW: usize = 800;

pub struct SourceHandle {
    stop: Arc<AtomicBool>,
    stream_thread: Thread,
    threads: Vec<JoinHandle<()>>,
}

impl SourceHandle {
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::Release);
        self.stream_thread.unpark();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Downmix, resample, level metering and VAD for one source.
pub struct Processor {
    source: AudioSource,
    channels: usize,
    resampler: Resampler,
    vad: Vad,
    level_cb: LevelCb,
    sum_sq: f64,
    level_n: usize,
    /// The whole 16 kHz stream, silence included, so playback lines up with the session.
    rec: Option<WavFile>,
}

impl Processor {
    pub fn new(
        source: AudioSource,
        in_rate: u32,
        channels: u16,
        vad: VadConfig,
        level_cb: LevelCb,
        rec: Option<WavFile>,
    ) -> Self {
        Self {
            source,
            channels: channels.max(1) as usize,
            resampler: Resampler::new(in_rate, SAMPLE_RATE),
            vad: Vad::new(vad),
            level_cb,
            sum_sq: 0.0,
            level_n: 0,
            rec,
        }
    }

    pub fn process(&mut self, interleaved: &[f32]) -> Vec<VadChunk> {
        let mono = downmix(interleaved, self.channels);
        let pcm = self.resampler.process(&mono);
        if let Some(w) = self.rec.as_mut() {
            let mut failed = false;
            for s in crate::stt::f32_to_i16(&pcm) {
                if w.write_sample(s).is_err() {
                    failed = true;
                    break;
                }
            }
            if failed {
                tracing::warn!("{:?} recording stopped: could not write the audio file", self.source);
                self.rec = None;
            }
        }
        for s in &pcm {
            self.sum_sq += (*s as f64) * (*s as f64);
        }
        self.level_n += pcm.len();
        if self.level_n >= LEVEL_WINDOW {
            let rms = (self.sum_sq / self.level_n as f64).sqrt() as f32;
            (self.level_cb)(self.source, rms);
            self.sum_sq = 0.0;
            self.level_n = 0;
        }
        self.vad.push(&pcm)
    }

    pub fn flush(&mut self) -> Vec<VadChunk> {
        self.vad.flush()
    }

    /// Write the WAV header sizes and close the recording file.
    pub fn finish_recording(&mut self) {
        if let Some(w) = self.rec.take() {
            if let Err(e) = w.finalize() {
                tracing::warn!("{:?} recording could not be closed: {e}", self.source);
            }
        }
    }
}

pub fn spawn_source(
    source: AudioSource,
    device_name: Option<String>,
    vad: VadConfig,
    tx: tokio::sync::mpsc::Sender<AudioChunk>,
    level_cb: LevelCb,
    record_to: Option<PathBuf>,
) -> anyhow::Result<SourceHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let (audio_tx, audio_rx) = std_mpsc::channel::<Vec<f32>>();
    let (ready_tx, ready_rx) = std_mpsc::channel::<Result<(u32, u16), String>>();

    let label = match source {
        AudioSource::Microphone => "mic",
        AudioSource::System => "system",
    };

    let stream_stop = stop.clone();
    let stream_join = thread::Builder::new()
        .name(format!("relay-audio-stream-{label}"))
        .spawn(move || run_stream_thread(source, device_name, stream_stop, audio_tx, ready_tx))
        .context("start audio stream thread")?;
    let stream_thread = stream_join.thread().clone();

    let (rate, channels) = match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(v)) => v,
        Ok(Err(msg)) => {
            let _ = stream_join.join();
            bail!("{msg}");
        }
        Err(_) => {
            stop.store(true, Ordering::Release);
            stream_thread.unpark();
            bail!("The {label} audio device did not start within 10 seconds. Pick another device in Settings > Audio.");
        }
    };
    tracing::info!("{label} capture started at {rate} Hz, {channels} channel(s)");

    // A recording that cannot be opened is logged; capture and transcription go on without it.
    let rec = record_to.and_then(|p| match open_recording(&p) {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::warn!("{label} audio will not be kept: {e:#}");
            None
        }
    });

    let proc_stop = stop.clone();
    let proc_join = thread::Builder::new()
        .name(format!("relay-audio-proc-{label}"))
        .spawn(move || {
            let mut proc = Processor::new(source, rate, channels, vad, level_cb, rec);
            loop {
                match audio_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(buf) => {
                        for c in proc.process(&buf) {
                            if !forward(&tx, &proc_stop, source, c) {
                                return;
                            }
                        }
                    }
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                }
                if proc_stop.load(Ordering::Acquire) {
                    break;
                }
            }
            proc.finish_recording();
            for c in proc.flush() {
                if !forward(&tx, &proc_stop, source, c) {
                    return;
                }
            }
        })
        .context("start audio processing thread")?;

    Ok(SourceHandle { stop, stream_thread, threads: vec![stream_join, proc_join] })
}

/// Send a chunk to the consumer. Returns false when the consumer is gone.
/// When the channel is full the call retries every 20 ms until the capture is
/// stopped, then drops the chunk with a warning.
fn forward(tx: &tokio::sync::mpsc::Sender<AudioChunk>, stop: &AtomicBool, source: AudioSource, c: VadChunk) -> bool {
    let mut chunk = AudioChunk { source, samples: c.samples, start_ms: c.start_ms, end_ms: c.end_ms };
    loop {
        match tx.try_send(chunk) {
            Ok(()) => return true,
            Err(TrySendError::Closed(_)) => return false,
            Err(TrySendError::Full(back)) => {
                if stop.load(Ordering::Acquire) {
                    tracing::warn!("dropped a {:?} audio chunk because the transcription queue is full", source);
                    return true;
                }
                chunk = back;
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn run_stream_thread(
    source: AudioSource,
    device_name: Option<String>,
    stop: Arc<AtomicBool>,
    audio_tx: std_mpsc::Sender<Vec<f32>>,
    ready: std_mpsc::Sender<Result<(u32, u16), String>>,
) {
    match build_stream(source, device_name.as_deref(), audio_tx) {
        Ok((stream, rate, channels)) => {
            if let Err(e) = stream.play() {
                let _ = ready.send(Err(format!("Could not start the audio stream: {e}. Pick another device in Settings > Audio.")));
                return;
            }
            let _ = ready.send(Ok((rate, channels)));
            while !stop.load(Ordering::Acquire) {
                thread::park_timeout(Duration::from_millis(200));
            }
            drop(stream);
        }
        Err(e) => {
            let _ = ready.send(Err(format!("{e:#}")));
        }
    }
}

fn find_device(host: &cpal::Host, name: Option<&str>, output: bool) -> anyhow::Result<cpal::Device> {
    if let Some(wanted) = name.filter(|n| !n.is_empty()) {
        let devices: Vec<cpal::Device> = if output {
            host.output_devices().context("list output devices")?.collect()
        } else {
            host.input_devices().context("list input devices")?.collect()
        };
        for d in devices {
            if d.name().map(|n| n == wanted).unwrap_or(false) {
                return Ok(d);
            }
        }
        if output {
            bail!("System audio device '{wanted}' was not found. Pick another one in Settings > Audio, or clear it to use the default output.");
        } else {
            bail!("Microphone '{wanted}' was not found. Pick another one in Settings > Audio, or clear it to use the default microphone.");
        }
    }
    if output {
        host.default_output_device()
            .ok_or_else(|| anyhow!("No default output device for system audio. Check Windows sound settings."))
    } else {
        host.default_input_device()
            .ok_or_else(|| anyhow!("No default microphone found. Check Windows sound settings and microphone privacy permissions."))
    }
}

fn build_stream(
    source: AudioSource,
    name: Option<&str>,
    tx: std_mpsc::Sender<Vec<f32>>,
) -> anyhow::Result<(cpal::Stream, u32, u16)> {
    let host = cpal::default_host();
    let (device, supported) = match source {
        AudioSource::Microphone => {
            let d = find_device(&host, name, false)?;
            let c = d
                .default_input_config()
                .context("read microphone format. Check Settings > Audio")?;
            (d, c)
        }
        AudioSource::System => {
            // Loopback: WASAPI treats an input stream on a render device as a capture of its output.
            let d = find_device(&host, name, true)?;
            let c = d
                .default_output_config()
                .context("read system audio format. Check Settings > Audio")?;
            (d, c)
        }
    };
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.config();
    let rate = config.sample_rate.0;
    let channels = config.channels;

    let err_label = match source {
        AudioSource::Microphone => "microphone",
        AudioSource::System => "system audio",
    };
    let err_cb = move |e: cpal::StreamError| tracing::warn!("{err_label} stream error: {e}");

    let built = match format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let _ = tx.send(data.to_vec());
            },
            err_cb,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                let _ = tx.send(data.iter().map(|s| *s as f32 / 32768.0).collect());
            },
            err_cb,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                let _ = tx.send(data.iter().map(|s| (*s as f32 - 32768.0) / 32768.0).collect());
            },
            err_cb,
            None,
        ),
        other => bail!("The {err_label} device uses the sample format {other:?}, which Relay does not support."),
    };
    let stream = built.map_err(|e| anyhow!("Could not open the {err_label} device: {e}. Pick another device in Settings > Audio."))?;
    Ok((stream, rate, channels))
}
