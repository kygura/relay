//! Downmix and sample rate conversion to 16 kHz.
//!
//! The resampler is streaming: feed it consecutive buffers and it keeps the
//! unconsumed tail between calls. When downsampling it averages the input
//! samples that fall inside each output sample (a box filter), which removes
//! most of the aliasing a plain linear interpolation would let through.

/// Average interleaved channels into one mono channel.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let inv = 1.0 / channels as f32;
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() * inv)
        .collect()
}

pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    /// Read position inside `carry`, in input samples.
    pos: f64,
    carry: Vec<f32>,
    passthrough: bool,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let in_rate = in_rate.max(1);
        let out_rate = out_rate.max(1);
        Self {
            step: in_rate as f64 / out_rate as f64,
            pos: 0.0,
            carry: Vec::new(),
            passthrough: in_rate == out_rate,
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if self.passthrough {
            return input.to_vec();
        }
        self.carry.extend_from_slice(input);
        let len = self.carry.len() as f64;
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 2);

        if self.step >= 1.0 {
            // Box filter over the window [pos, pos + step).
            while self.pos + self.step <= len {
                let a = self.pos;
                let b = a + self.step;
                let mut acc = 0.0f64;
                let mut i = a.floor() as usize;
                while (i as f64) < b {
                    let lo = (i as f64).max(a);
                    let hi = ((i + 1) as f64).min(b);
                    acc += self.carry[i] as f64 * (hi - lo);
                    i += 1;
                }
                out.push((acc / self.step) as f32);
                self.pos = b;
            }
        } else {
            // Upsampling: linear interpolation between neighbours.
            while self.pos + 1.0 < len {
                let i = self.pos.floor() as usize;
                let frac = (self.pos - i as f64) as f32;
                out.push(self.carry[i] * (1.0 - frac) + self.carry[i + 1] * frac);
                self.pos += self.step;
            }
        }

        let consumed = (self.pos.floor() as usize).min(self.carry.len());
        self.carry.drain(..consumed);
        self.pos -= consumed as f64;
        out
    }
}

/// Convert one-shot audio. Used by tests and by callers that have a whole buffer.
pub fn resample_once(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    Resampler::new(in_rate, out_rate).process(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, secs: f32, amp: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    fn zero_crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
    }

    #[test]
    fn downmix_averages_channels() {
        let out = downmix(&[1.0, 0.0, 0.5, 0.5], 2);
        assert_eq!(out, vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }

    #[test]
    fn length_48k_to_16k() {
        let out = resample_once(&sine(440.0, 48_000, 1.0, 0.5), 48_000, 16_000);
        assert!((out.len() as i64 - 16_000).abs() <= 2, "got {}", out.len());
    }

    #[test]
    fn length_44k1_to_16k() {
        let out = resample_once(&sine(440.0, 44_100, 1.0, 0.5), 44_100, 16_000);
        assert!((out.len() as i64 - 16_000).abs() <= 3, "got {}", out.len());
    }

    #[test]
    fn length_8k_to_16k() {
        let out = resample_once(&sine(440.0, 8_000, 1.0, 0.5), 8_000, 16_000);
        assert!((out.len() as i64 - 16_000).abs() <= 3, "got {}", out.len());
    }

    #[test]
    fn tone_frequency_is_preserved() {
        let out = resample_once(&sine(1000.0, 48_000, 1.0, 0.5), 48_000, 16_000);
        // 1 kHz over one second has about 2000 sign changes.
        let zc = zero_crossings(&out);
        assert!((zc as i64 - 2000).abs() <= 8, "zero crossings {zc}");
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.05, "rms {rms}");
    }

    #[test]
    fn chunked_processing_matches_length_of_one_shot() {
        let input = sine(300.0, 44_100, 1.0, 0.4);
        let whole = resample_once(&input, 44_100, 16_000);
        let mut r = Resampler::new(44_100, 16_000);
        let mut chunked = Vec::new();
        for piece in input.chunks(441) {
            chunked.extend(r.process(piece));
        }
        assert!((whole.len() as i64 - chunked.len() as i64).abs() <= 1);
        let n = whole.len().min(chunked.len());
        for i in 0..n {
            assert!((whole[i] - chunked[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn same_rate_is_passthrough() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_once(&input, 16_000, 16_000), input);
    }
}
