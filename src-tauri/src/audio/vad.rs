//! Energy based voice activity detection and utterance segmentation.
//!
//! Input is 16 kHz mono f32. The detector works on 30 ms frames. A frame is
//! voiced when its RMS is at or above the threshold. A chunk opens on the first
//! voiced frame (with 200 ms of preceding audio) and closes after `silence_ms`
//! of unvoiced frames or when it reaches `max_chunk_ms`. Chunks with less than
//! 300 ms of voiced audio are dropped.

use std::collections::VecDeque;

pub const FRAME_MS: u64 = 30;
pub const FRAME_SAMPLES: usize = 480;
const RATE: u64 = 16_000;
const PREROLL_SAMPLES: usize = 3_200;
const MIN_SPEECH_MS: u64 = 300;

#[derive(Debug, Clone)]
pub struct VadConfig {
    pub threshold: f32,
    pub silence_ms: u64,
    pub max_chunk_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VadChunk {
    pub samples: Vec<f32>,
    pub start_ms: u64,
    pub end_ms: u64,
}

pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f64 = frame.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / frame.len() as f64).sqrt() as f32
}

pub struct Vad {
    cfg: VadConfig,
    /// Samples that do not yet fill a whole frame.
    partial: Vec<f32>,
    preroll: VecDeque<f32>,
    chunk: Vec<f32>,
    in_speech: bool,
    /// Sample index where the open chunk starts.
    chunk_start: u64,
    voiced_frames: u64,
    silence_frames: u64,
    /// Samples in frames processed so far.
    consumed: u64,
}

impl Vad {
    pub fn new(mut cfg: VadConfig) -> Self {
        cfg.silence_ms = cfg.silence_ms.max(FRAME_MS * 2);
        cfg.max_chunk_ms = cfg.max_chunk_ms.max(1_000);
        Self {
            cfg,
            partial: Vec::new(),
            preroll: VecDeque::with_capacity(PREROLL_SAMPLES + FRAME_SAMPLES),
            chunk: Vec::new(),
            in_speech: false,
            chunk_start: 0,
            voiced_frames: 0,
            silence_frames: 0,
            consumed: 0,
        }
    }

    /// Feed samples. Returns any chunks that closed during this call.
    pub fn push(&mut self, samples: &[f32]) -> Vec<VadChunk> {
        let mut out = Vec::new();
        self.partial.extend_from_slice(samples);
        let buf = std::mem::take(&mut self.partial);
        let mut frames = buf.chunks_exact(FRAME_SAMPLES);
        for frame in &mut frames {
            self.process_frame(frame, &mut out);
        }
        self.partial = frames.remainder().to_vec();
        out
    }

    /// Close the open chunk, if any. Call when the stream ends.
    pub fn flush(&mut self) -> Vec<VadChunk> {
        let mut out = Vec::new();
        if self.in_speech {
            self.finish(&mut out, false);
        }
        out
    }

    fn process_frame(&mut self, frame: &[f32], out: &mut Vec<VadChunk>) {
        let voiced = rms(frame) >= self.cfg.threshold;
        let frame_start = self.consumed;
        self.consumed += frame.len() as u64;

        if !self.in_speech {
            if !voiced {
                self.preroll.extend(frame.iter().copied());
                while self.preroll.len() > PREROLL_SAMPLES {
                    self.preroll.pop_front();
                }
                return;
            }
            self.in_speech = true;
            self.chunk.clear();
            self.chunk.extend(self.preroll.drain(..));
            self.chunk_start = frame_start - self.chunk.len() as u64;
            self.chunk.extend_from_slice(frame);
            self.voiced_frames = 1;
            self.silence_frames = 0;
        } else {
            self.chunk.extend_from_slice(frame);
            if voiced {
                self.voiced_frames += 1;
                self.silence_frames = 0;
            } else {
                self.silence_frames += 1;
            }
        }

        let chunk_ms = self.chunk.len() as u64 * 1000 / RATE;
        if self.silence_frames * FRAME_MS >= self.cfg.silence_ms {
            self.finish(out, false);
        } else if chunk_ms + FRAME_MS > self.cfg.max_chunk_ms {
            // Close now if the next frame would push the chunk past the limit.
            self.finish(out, true);
        }
    }

    fn finish(&mut self, out: &mut Vec<VadChunk>, keep_going: bool) {
        let samples = std::mem::take(&mut self.chunk);
        let start = self.chunk_start;
        let end = start + samples.len() as u64;
        if self.voiced_frames * FRAME_MS >= MIN_SPEECH_MS {
            out.push(VadChunk {
                samples,
                start_ms: start * 1000 / RATE,
                end_ms: end * 1000 / RATE,
            });
        }
        self.voiced_frames = 0;
        self.silence_frames = 0;
        if keep_going {
            self.chunk_start = end;
        } else {
            self.in_speech = false;
            self.preroll.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> VadConfig {
        VadConfig { threshold: 0.012, silence_ms: 700, max_chunk_ms: 12_000 }
    }

    fn silence(ms: usize) -> Vec<f32> {
        vec![0.0; ms * 16]
    }

    fn tone(ms: usize) -> Vec<f32> {
        (0..ms * 16)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
            .collect()
    }

    fn run(vad: &mut Vad, signal: &[f32]) -> Vec<VadChunk> {
        let mut out = Vec::new();
        // Feed in odd sized pieces to exercise the partial frame buffer.
        for piece in signal.chunks(1_000) {
            out.extend(vad.push(piece));
        }
        out.extend(vad.flush());
        out
    }

    #[test]
    fn silence_produces_no_chunk() {
        let mut vad = Vad::new(cfg());
        assert!(run(&mut vad, &silence(5_000)).is_empty());
    }

    #[test]
    fn short_burst_is_discarded() {
        let mut vad = Vad::new(cfg());
        let mut s = silence(1_000);
        s.extend(tone(150));
        s.extend(silence(2_000));
        assert!(run(&mut vad, &s).is_empty());
    }

    #[test]
    fn tone_burst_gives_one_chunk_with_preroll() {
        let mut vad = Vad::new(cfg());
        let mut s = silence(1_000);
        s.extend(tone(1_000));
        s.extend(silence(1_500));
        let chunks = run(&mut vad, &s);
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        let c = &chunks[0];
        // The tone starts at 1000 ms. With 200 ms of pre-roll the chunk starts near 800 ms.
        assert!(c.start_ms >= 750 && c.start_ms <= 850, "start {}", c.start_ms);
        // Pre-roll + tone + about 700 ms of hangover.
        assert!(c.end_ms >= 2_600 && c.end_ms <= 2_100 + 700 + 60, "end {}", c.end_ms);
        assert_eq!(c.samples.len() as u64 * 1000 / 16_000, c.end_ms - c.start_ms);
    }

    #[test]
    fn long_tone_is_split_at_max_chunk() {
        let mut vad = Vad::new(cfg());
        let mut s = silence(300);
        s.extend(tone(30_000));
        s.extend(silence(1_500));
        let chunks = run(&mut vad, &s);
        assert!(chunks.len() >= 3, "got {} chunks", chunks.len());
        for c in &chunks {
            assert!(c.end_ms - c.start_ms <= 12_000, "chunk {} ms", c.end_ms - c.start_ms);
        }
        // Chunks are contiguous in time.
        for w in chunks.windows(2) {
            assert!(w[1].start_ms >= w[0].end_ms.saturating_sub(1));
        }
    }

    #[test]
    fn two_bursts_give_two_chunks() {
        let mut vad = Vad::new(cfg());
        let mut s = tone(600);
        s.extend(silence(2_000));
        s.extend(tone(600));
        s.extend(silence(2_000));
        assert_eq!(run(&mut vad, &s).len(), 2);
    }

    #[test]
    fn rms_of_constant_signal() {
        assert!((rms(&[0.5; 100]) - 0.5).abs() < 1e-6);
        assert_eq!(rms(&[]), 0.0);
    }
}
