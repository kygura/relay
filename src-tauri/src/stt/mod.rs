//! Speech to text engines. `build_engine` picks one from the settings.

pub mod deepgram;
pub mod models;
pub mod openai;
#[cfg(feature = "local-whisper")]
pub mod whisper;

use crate::settings::{get_api_key, Settings, SttEngineKind};
use crate::types::{AudioChunk, TranscriptSegment, SAMPLE_RATE};
use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::mpsc;

/// True when this build can run Whisper in-process.
pub const LOCAL_WHISPER_BUILT: bool = cfg!(feature = "local-whisper");

/// Model file for the local engine: the custom path when set, else the catalog model.
pub fn whisper_model_file(settings: &Settings, models_dir: &Path) -> anyhow::Result<PathBuf> {
    let custom = settings.whisper_model_path.trim();
    if !custom.is_empty() {
        return Ok(PathBuf::from(custom));
    }
    let name = settings.whisper_model.trim();
    let name = if name.is_empty() { models::DEFAULT_MODEL } else { name };
    models::model_path(models_dir, name)
}

#[async_trait::async_trait]
pub trait SttEngine: Send + Sync {
    fn name(&self) -> &'static str;
    /// Consume chunks until rx closes; send segments on tx. Interim (is_final=false) segments optional.
    async fn run(&self, rx: mpsc::Receiver<AudioChunk>, tx: mpsc::Sender<TranscriptSegment>) -> anyhow::Result<()>;
}

/// `cancel` is set when the session stops and the queued audio should be dropped. The local
/// engine checks it between chunks and during inference; the network engines end when their
/// input closes.
pub fn build_engine(
    settings: &Settings,
    models_dir: &Path,
    cancel: Arc<AtomicBool>,
) -> anyhow::Result<Box<dyn SttEngine>> {
    let language = settings
        .source_language
        .as_deref()
        .map(base_language)
        .filter(|l| !l.is_empty());
    match settings.stt_engine {
        SttEngineKind::LocalWhisper => build_whisper(&whisper_model_file(settings, models_dir)?, language, cancel),
        SttEngineKind::OpenAiCompatible => {
            let base = settings.openai_base_url.trim();
            if base.is_empty() {
                bail!("openai_base_url is empty. Set it in Settings > Speech to text, for example https://api.openai.com/v1.");
            }
            let key = get_api_key("openai");
            if key.is_none() && base.contains("api.openai.com") {
                bail!("No OpenAI key set. Add it in Settings > API keys, or set openai_base_url to a local server.");
            }
            if settings.openai_stt_model.trim().is_empty() {
                bail!("openai_stt_model is empty. Set a model name in Settings > Speech to text.");
            }
            Ok(Box::new(openai::OpenAiEngine::new(
                base,
                key.unwrap_or_default(),
                settings.openai_stt_model.trim().to_string(),
                language,
            )?))
        }
        SttEngineKind::Deepgram => {
            let key = get_api_key("deepgram").context(
                "No Deepgram key set. Add it in Settings > API keys, or choose another engine in Settings > Speech to text.",
            )?;
            if settings.deepgram_model.trim().is_empty() {
                bail!("deepgram_model is empty. Set a model such as nova-3 in Settings > Speech to text.");
            }
            Ok(Box::new(deepgram::DeepgramEngine::new(
                key,
                settings.deepgram_model.trim().to_string(),
                language,
            )))
        }
    }
}

#[cfg(feature = "local-whisper")]
fn build_whisper(model: &Path, language: Option<String>, cancel: Arc<AtomicBool>) -> anyhow::Result<Box<dyn SttEngine>> {
    Ok(Box::new(whisper::WhisperEngine::new(model, language, cancel)?))
}

#[cfg(not(feature = "local-whisper"))]
fn build_whisper(_model: &Path, _language: Option<String>, _cancel: Arc<AtomicBool>) -> anyhow::Result<Box<dyn SttEngine>> {
    bail!("Local Whisper is not part of this build. Rebuild with the cargo feature local-whisper, or choose another engine in Settings > Speech to text.")
}

/// "en-US" and "EN" both become "en".
pub fn base_language(tag: &str) -> String {
    tag.trim().split(['-', '_']).next().unwrap_or("").to_lowercase()
}

/// Convert f32 samples in [-1, 1] to 16-bit PCM. Values outside the range are clamped.
pub fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
        .collect()
}

/// Encode 16 kHz mono samples as a 16-bit PCM WAV file in memory.
pub fn encode_wav(samples: &[f32]) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::with_capacity(44 + samples.len() * 2));
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).context("create WAV writer")?;
        for s in f32_to_i16(samples) {
            writer.write_sample(s).context("write WAV sample")?;
        }
        writer.finalize().context("finalize WAV")?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_to_i16_known_values() {
        assert_eq!(f32_to_i16(&[0.0, 1.0, -1.0, 0.5]), vec![0, 32767, -32767, 16384]);
    }

    #[test]
    fn f32_to_i16_clamps() {
        assert_eq!(f32_to_i16(&[2.0, -3.5]), vec![32767, -32767]);
    }

    #[test]
    fn wav_has_header_and_expected_size() {
        let wav = encode_wav(&vec![0.1; 1600]).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 1600 * 2);
    }

    #[test]
    fn base_language_strips_region() {
        assert_eq!(base_language("en-US"), "en");
        assert_eq!(base_language(" ES "), "es");
        assert_eq!(base_language("pt_BR"), "pt");
    }
}
