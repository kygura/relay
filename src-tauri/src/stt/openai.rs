//! OpenAI-compatible transcription: POST multipart WAV to `{base}/audio/transcriptions`.
//! Works with OpenAI, Groq and local servers that expose the same endpoint.

use super::{encode_wav, SttEngine};
use crate::types::{AudioChunk, TranscriptSegment};
use anyhow::{anyhow, Context};
use futures_util::stream::FuturesOrdered;
use futures_util::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc;

const MAX_IN_FLIGHT: usize = 3;

pub struct OpenAiEngine {
    client: reqwest::Client,
    url: String,
    key: String,
    model: String,
    language: Option<String>,
}

enum Failure {
    /// Stops the run: retrying cannot help until the user changes a setting.
    Fatal(anyhow::Error),
    /// Skips this chunk and continues.
    Transient(anyhow::Error),
}

impl OpenAiEngine {
    pub fn new(base_url: &str, key: String, model: String, language: Option<String>) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("create HTTP client")?;
        Ok(Self {
            client,
            url: format!("{}/audio/transcriptions", base_url.trim_end_matches('/')),
            key,
            model,
            language,
        })
    }

    async fn transcribe(&self, chunk: AudioChunk) -> Result<Option<TranscriptSegment>, Failure> {
        let wav = encode_wav(&chunk.samples).map_err(Failure::Transient)?;
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| Failure::Transient(anyhow!(e)))?;
        let mut form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone());
        if let Some(lang) = &self.language {
            form = form.text("language", lang.clone());
        }
        let mut req = self.client.post(&self.url).multipart(form);
        if !self.key.is_empty() {
            req = req.bearer_auth(&self.key);
        }

        let resp = req.send().await.map_err(|e| {
            Failure::Transient(anyhow!(
                "Could not reach the transcription server at {}: {e}. Check openai_base_url in Settings > Speech to text.",
                self.url
            ))
        })?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(Failure::Fatal(anyhow!(
                "The transcription server rejected the API key (HTTP {}). Check the OpenAI key in Settings > API keys.",
                status.as_u16()
            )));
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(Failure::Fatal(anyhow!(
                "The transcription server returned HTTP 404 for {}. Check openai_base_url and openai_stt_model in Settings > Speech to text.",
                self.url
            )));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let body: String = body.chars().take(300).collect();
            return Err(Failure::Transient(anyhow!("Transcription request failed with HTTP {}: {body}", status.as_u16())));
        }
        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Failure::Transient(anyhow!("Transcription response was not valid JSON: {e}")))?;
        let text = value.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
        if text.is_empty() {
            return Ok(None);
        }
        let language = value
            .get("language")
            .and_then(|l| l.as_str())
            .map(|l| l.to_string())
            .or_else(|| self.language.clone());
        Ok(Some(TranscriptSegment {
            id: uuid::Uuid::new_v4().to_string(),
            source: chunk.source,
            speaker: None,
            text,
            start_ms: chunk.start_ms,
            end_ms: chunk.end_ms,
            is_final: true,
            language,
        }))
    }
}

#[async_trait::async_trait]
impl SttEngine for OpenAiEngine {
    fn name(&self) -> &'static str {
        "openai-compatible"
    }

    async fn run(&self, mut rx: mpsc::Receiver<AudioChunk>, tx: mpsc::Sender<TranscriptSegment>) -> anyhow::Result<()> {
        let mut in_flight = FuturesOrdered::new();
        let mut rx_open = true;
        loop {
            tokio::select! {
                Some(result) = in_flight.next(), if !in_flight.is_empty() => {
                    match result {
                        Ok(Some(seg)) => {
                            if tx.send(seg).await.is_err() {
                                return Ok(());
                            }
                        }
                        Ok(None) => {}
                        Err(Failure::Fatal(e)) => return Err(e),
                        Err(Failure::Transient(e)) => tracing::warn!("transcription chunk skipped: {e:#}"),
                    }
                }
                maybe = rx.recv(), if rx_open && in_flight.len() < MAX_IN_FLIGHT => {
                    match maybe {
                        Some(chunk) => in_flight.push_back(self.transcribe(chunk)),
                        None => rx_open = false,
                    }
                }
                else => break,
            }
        }
        Ok(())
    }
}
