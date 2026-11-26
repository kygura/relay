//! Audio kept with History entries. While a session runs, each source is written to its own
//! 16 kHz mono WAV (`<prefix>.mic.wav`, `<prefix>.system.wav`). When the entry is saved the
//! sources are mixed into one file named after the entry, and the source files are removed.

use crate::types::{AudioSource, SAMPLE_RATE};
use anyhow::Context;
use std::path::{Path, PathBuf};

/// Name prefix of the per-source files of a running session.
pub const SESSION_PREFIX: &str = "rec-";

/// File one source of a running session is recorded to.
pub fn source_file(prefix: &Path, source: AudioSource) -> PathBuf {
    let tag = match source {
        AudioSource::Microphone => "mic",
        AudioSource::System => "system",
    };
    let mut name = prefix.as_os_str().to_owned();
    name.push(format!(".{tag}.wav"));
    PathBuf::from(name)
}

/// The source files of a session that exist on disk.
pub fn existing_sources(prefix: &Path) -> Vec<PathBuf> {
    [AudioSource::Microphone, AudioSource::System]
        .into_iter()
        .map(|s| source_file(prefix, s))
        .filter(|p| p.is_file())
        .collect()
}

/// Remove the source files of a session. Missing files are fine.
pub fn discard(prefix: &Path) {
    for s in [AudioSource::Microphone, AudioSource::System] {
        let _ = std::fs::remove_file(source_file(prefix, s));
    }
}

/// Mix 16 kHz mono 16-bit WAV files into one, sample by sample, clamped to the 16-bit range.
/// The sources start together, so sample n of each is the same moment. Returns the length in
/// milliseconds; nothing is written when there is no audio.
pub fn mix(inputs: &[PathBuf], out: &Path) -> anyhow::Result<u64> {
    let mut readers = Vec::new();
    for p in inputs {
        let r = hound::WavReader::open(p).with_context(|| format!("open {}", p.display()))?;
        let spec = r.spec();
        if spec.channels != 1 || spec.sample_rate != SAMPLE_RATE || spec.bits_per_sample != 16 {
            anyhow::bail!("{} is not a 16 kHz mono 16-bit WAV", p.display());
        }
        if r.len() > 0 {
            readers.push(r);
        }
    }
    if readers.is_empty() {
        return Ok(0);
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let tmp = out.with_extension("wav.tmp");
    let mut w = hound::WavWriter::create(&tmp, spec).with_context(|| format!("create {}", tmp.display()))?;
    let mut iters: Vec<_> = readers.iter_mut().map(|r| r.samples::<i16>()).collect();
    let mut n: u64 = 0;
    loop {
        let mut any = false;
        let mut sum: i32 = 0;
        for it in iters.iter_mut() {
            if let Some(s) = it.next() {
                sum += s.context("read audio sample")? as i32;
                any = true;
            }
        }
        if !any {
            break;
        }
        w.write_sample(sum.clamp(i16::MIN as i32, i16::MAX as i32) as i16)?;
        n += 1;
    }
    w.finalize().context("finish the audio file")?;
    std::fs::rename(&tmp, out).with_context(|| format!("save {}", out.display()))?;
    Ok(n * 1000 / SAMPLE_RATE as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, samples: &[i16]) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
    }

    fn read(path: &Path) -> Vec<i16> {
        hound::WavReader::open(path).unwrap().samples::<i16>().map(|s| s.unwrap()).collect()
    }

    #[test]
    fn mixes_sums_clamps_and_keeps_the_longer_tail() {
        let dir = std::env::temp_dir().join(format!("relay-clip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let prefix = dir.join("rec-test");
        let mic = source_file(&prefix, AudioSource::Microphone);
        let sys = source_file(&prefix, AudioSource::System);
        write(&mic, &[100, -200, 30_000, 5]);
        write(&sys, &[1, 2, 10_000]);
        assert_eq!(existing_sources(&prefix).len(), 2);

        let out = dir.join("entry.wav");
        let ms = mix(&existing_sources(&prefix), &out).unwrap();
        assert_eq!(read(&out), vec![101, -198, i16::MAX, 5]);
        assert_eq!(ms, 0); // 4 samples is well under a millisecond at 16 kHz

        discard(&prefix);
        assert!(existing_sources(&prefix).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn empty_sources_write_nothing() {
        let dir = std::env::temp_dir().join(format!("relay-clip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mic = dir.join("rec-x.mic.wav");
        write(&mic, &[]);
        let out = dir.join("entry.wav");
        assert_eq!(mix(&[mic], &out).unwrap(), 0);
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn source_file_names() {
        let p = Path::new("C:/data/audio/rec-1");
        assert!(source_file(p, AudioSource::Microphone).ends_with("rec-1.mic.wav"));
        assert!(source_file(p, AudioSource::System).ends_with("rec-1.system.wav"));
    }
}
