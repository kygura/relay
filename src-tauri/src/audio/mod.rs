//! Audio capture: microphone and system loopback through cpal (WASAPI),
//! downmix, resample to 16 kHz and VAD segmentation into utterance chunks.

mod capture;
pub mod clip;
pub mod resample;
pub mod vad;

use crate::types::{AudioChunk, AudioDeviceInfo, AudioSource};
use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub capture_mic: bool,
    pub mic_device: Option<String>,
    pub capture_system: bool,
    pub system_device: Option<String>,
    pub vad_threshold: f32,
    pub max_chunk_ms: u64,
    pub silence_ms: u64,
    /// When set, each source's 16 kHz stream is also written to a WAV file, see `clip::source_file`.
    pub record_prefix: Option<PathBuf>,
}

/// Owns the capture threads. Capture stops when this is dropped.
pub struct CaptureHandle {
    sources: Vec<capture::SourceHandle>,
}

impl CaptureHandle {
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        for s in self.sources.drain(..) {
            s.shutdown();
        }
    }
}

/// Microphones are input devices. System devices are output devices that can be
/// captured as loopback.
pub fn list_devices() -> anyhow::Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let default_in = host.default_input_device().and_then(|d| d.name().ok());
    let default_out = host.default_output_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();

    for d in host.input_devices().context("list microphones")? {
        if let Ok(name) = d.name() {
            let is_default = default_in.as_deref() == Some(name.as_str());
            out.push(AudioDeviceInfo { id: name.clone(), name, kind: AudioSource::Microphone, is_default });
        }
    }
    for d in host.output_devices().context("list output devices")? {
        if let Ok(name) = d.name() {
            let is_default = default_out.as_deref() == Some(name.as_str());
            out.push(AudioDeviceInfo { id: name.clone(), name, kind: AudioSource::System, is_default });
        }
    }
    Ok(out)
}

/// Start capturing. Finished utterances are sent on `tx`. `level_cb` is called
/// about 20 times per second per source with the RMS level of the audio.
/// Fails only when no requested source could be started.
pub fn start_capture(
    cfg: CaptureConfig,
    tx: tokio::sync::mpsc::Sender<AudioChunk>,
    level_cb: Box<dyn Fn(AudioSource, f32) + Send + Sync>,
) -> anyhow::Result<CaptureHandle> {
    if !cfg.capture_mic && !cfg.capture_system {
        anyhow::bail!("Nothing to capture. Turn on the microphone or system audio in Settings > Audio.");
    }
    let level_cb: capture::LevelCb = Arc::from(level_cb);
    let vad_cfg = vad::VadConfig {
        threshold: cfg.vad_threshold,
        silence_ms: cfg.silence_ms,
        max_chunk_ms: cfg.max_chunk_ms,
    };

    let mut sources = Vec::new();
    let mut first_error: Option<anyhow::Error> = None;
    let requested = [
        (cfg.capture_mic, AudioSource::Microphone, cfg.mic_device.clone()),
        (cfg.capture_system, AudioSource::System, cfg.system_device.clone()),
    ];
    for (enabled, source, device) in requested {
        if !enabled {
            continue;
        }
        let record_to = cfg.record_prefix.as_deref().map(|p| clip::source_file(p, source));
        match capture::spawn_source(source, device, vad_cfg.clone(), tx.clone(), level_cb.clone(), record_to) {
            Ok(h) => sources.push(h),
            Err(e) => {
                tracing::warn!("{source:?} capture failed: {e:#}");
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }
    if sources.is_empty() {
        return Err(first_error.unwrap_or_else(|| anyhow::anyhow!("No audio source could be started.")));
    }
    Ok(CaptureHandle { sources })
}
