//! Local Whisper through whisper-rs. Compiled only with the `local-whisper` feature.

use super::SttEngine;
use crate::types::{AudioChunk, TranscriptSegment};
use anyhow::{anyhow, bail, Context};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Whisper needs at least one second of input. Shorter chunks are padded with silence.
const MIN_SAMPLES: usize = 16_000;

/// The last loaded model, kept between sessions so starting a recording is instant.
static LOADED: Mutex<Option<(PathBuf, Arc<WhisperContext>)>> = Mutex::new(None);

pub struct WhisperEngine {
    ctx: Arc<WhisperContext>,
    language: Option<String>,
    /// Set when the session stops: skip queued chunks and abort the running inference.
    cancel: Arc<AtomicBool>,
}

impl WhisperEngine {
    pub fn new(model_path: &Path, language: Option<String>, cancel: Arc<AtomicBool>) -> anyhow::Result<Self> {
        if !model_path.is_file() {
            bail!(
                "The Whisper model is not downloaded yet ({}). Download it in Settings > Speech to text.",
                model_path.display()
            );
        }
        let mut cache = LOADED.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((p, ctx)) = cache.as_ref() {
            if p == model_path {
                return Ok(Self { ctx: ctx.clone(), language, cancel });
            }
        }
        // Route whisper.cpp's stderr logging into the `log` facade, where it is dropped.
        static HOOKS: std::sync::Once = std::sync::Once::new();
        HOOKS.call_once(whisper_rs::install_logging_hooks);
        *cache = None; // free the old model before loading the new one
        let path_str = model_path.to_string_lossy();
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| anyhow!("Could not load the Whisper model at '{path_str}': {e:?}. The file must be a ggml model."))?;
        let ctx = Arc::new(ctx);
        *cache = Some((model_path.to_path_buf(), ctx.clone()));
        Ok(Self { ctx, language, cancel })
    }
}

/// Encoder context sized to the chunk: 50 frames per second plus margin, capped at the
/// full 30 s window (1500). Chunks are at most a few seconds, so this cuts encode time
/// roughly in half compared with always encoding 30 s.
fn audio_ctx_for(samples: usize) -> i32 {
    let frames = samples / 320; // 16 kHz / 50 frames per second
    (frames as i32 + 128).clamp(512, 1500)
}

fn transcribe(
    ctx: &WhisperContext,
    samples: &[f32],
    language: Option<&str>,
    cancel: Arc<AtomicBool>,
) -> anyhow::Result<String> {
    let mut state = ctx.create_state().map_err(|e| anyhow!("create Whisper state: {e:?}"))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    // whisper.cpp polls this during encoding and decoding, so a stop ends the chunk at once.
    let abort: Box<dyn FnMut() -> bool> = Box::new(move || cancel.load(Ordering::Relaxed));
    params.set_abort_callback_safe::<Box<dyn FnMut() -> bool>, Box<dyn FnMut() -> bool>>(abort);
    params.set_language(Some(language.unwrap_or("auto")));
    params.set_translate(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_no_context(true);
    params.set_suppress_blank(true);
    params.set_audio_ctx(audio_ctx_for(samples.len()));
    let threads = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4).min(12);
    params.set_n_threads(threads);

    let mut padded;
    let input: &[f32] = if samples.len() < MIN_SAMPLES {
        padded = samples.to_vec();
        padded.resize(MIN_SAMPLES, 0.0);
        &padded
    } else {
        samples
    };

    state.full(params, input).map_err(|e| anyhow!("Whisper inference failed: {e:?}"))?;
    let mut text = String::new();
    for i in 0..state.full_n_segments() {
        if let Some(seg) = state.get_segment(i) {
            let part = seg
                .to_str_lossy()
                .map_err(|e| anyhow!("read Whisper segment text: {e:?}"))?;
            text.push_str(&part);
        }
    }
    Ok(text.trim().to_string())
}

/// Whisper emits markers such as [BLANK_AUDIO] or (music) for non-speech.
fn is_marker(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')'))
}

#[async_trait::async_trait]
impl SttEngine for WhisperEngine {
    fn name(&self) -> &'static str {
        "local-whisper"
    }

    async fn run(&self, mut rx: mpsc::Receiver<AudioChunk>, tx: mpsc::Sender<TranscriptSegment>) -> anyhow::Result<()> {
        while let Some(chunk) = rx.recv().await {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            let ctx = self.ctx.clone();
            let language = self.language.clone();
            let samples = chunk.samples.clone();
            let cancel = self.cancel.clone();
            let audio_ms = samples.len() as u64 * 1000 / 16_000;
            let started = Instant::now();
            let result = tokio::task::spawn_blocking(move || transcribe(&ctx, &samples, language.as_deref(), cancel))
                .await
                .context("Whisper worker stopped")?;
            let took_ms = started.elapsed().as_millis() as u64;
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            if took_ms > audio_ms.max(1000) {
                tracing::warn!(audio_ms, took_ms, "Whisper is slower than real time; audio is queueing up");
            } else {
                tracing::debug!(audio_ms, took_ms, "Whisper chunk");
            }
            let text = match result {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("Whisper chunk skipped: {e:#}");
                    continue;
                }
            };
            if text.is_empty() || is_marker(&text) {
                continue;
            }
            let seg = TranscriptSegment {
                id: uuid::Uuid::new_v4().to_string(),
                source: chunk.source,
                speaker: None,
                text,
                start_ms: chunk.start_ms,
                end_ms: chunk.end_ms,
                is_final: true,
                language: self.language.clone(),
            };
            if tx.send(seg).await.is_err() {
                break;
            }
        }
        Ok(())
    }
}
