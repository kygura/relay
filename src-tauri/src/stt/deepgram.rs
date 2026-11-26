//! Deepgram streaming transcription over one websocket per run.
//!
//! The engine receives a single rx that may carry chunks from both sources.
//! Audio is sent in arrival order as one stream. Deepgram reports times
//! relative to the start of that stream, so each sent chunk is recorded with
//! its stream offset and its session start time. Results are mapped back to
//! session time and to the source of the chunk that contains the result, so
//! the source label follows the audio that was actually sent. Diarization runs
//! across the merged stream, so speaker numbers can mix microphone and system
//! voices.

use super::{f32_to_i16, SttEngine};
use crate::types::{AudioChunk, AudioSource, TranscriptSegment};
use anyhow::{anyhow, Context};
use futures_util::{SinkExt, Stream, StreamExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

const KEEPALIVE: Duration = Duration::from_secs(8);
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// Samples per websocket binary frame (256 ms).
const FRAME_SAMPLES: usize = 4096;
const MAX_MAPPINGS: usize = 512;

pub struct DeepgramEngine {
    key: String,
    model: String,
    language: Option<String>,
}

#[derive(Debug, Clone)]
struct Mapping {
    stream_start_ms: u64,
    #[allow(dead_code)]
    stream_end_ms: u64,
    session_start_ms: u64,
    source: AudioSource,
}

impl DeepgramEngine {
    pub fn new(key: String, model: String, language: Option<String>) -> Self {
        Self { key, model, language }
    }

    fn url(&self) -> String {
        let mut url = format!(
            "wss://api.deepgram.com/v1/listen?model={}&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true&diarize=true",
            self.model
        );
        if let Some(lang) = &self.language {
            url.push_str("&language=");
            url.push_str(lang);
        }
        url
    }
}

fn explain_connect_error(e: WsError) -> anyhow::Error {
    if let WsError::Http(resp) = &e {
        let code = resp.status().as_u16();
        if code == 401 || code == 403 {
            return anyhow!("Deepgram rejected the key (HTTP {code}). Check the Deepgram key in Settings > API keys.");
        }
        return anyhow!("Deepgram refused the connection (HTTP {code}). Check deepgram_model in Settings > Speech to text.");
    }
    anyhow!("Could not connect to Deepgram: {e}. Check your network connection.")
}

#[async_trait::async_trait]
impl SttEngine for DeepgramEngine {
    fn name(&self) -> &'static str {
        "deepgram"
    }

    async fn run(&self, mut rx: mpsc::Receiver<AudioChunk>, tx: mpsc::Sender<TranscriptSegment>) -> anyhow::Result<()> {
        let mut request = self.url().into_client_request().context("build Deepgram request")?;
        let auth = HeaderValue::from_str(&format!("Token {}", self.key))
            .map_err(|_| anyhow!("The Deepgram key contains characters that cannot be sent in a header. Enter it again in Settings > API keys."))?;
        request.headers_mut().insert("Authorization", auth);

        let (ws, _) = tokio_tungstenite::connect_async(request).await.map_err(explain_connect_error)?;
        let (mut sink, stream) = ws.split();

        let mappings: Arc<Mutex<Vec<Mapping>>> = Arc::new(Mutex::new(Vec::new()));
        let mut reader = tokio::spawn(read_loop(stream, tx, mappings.clone(), self.language.clone()));

        let mut sent_ms: u64 = 0;
        let mut keepalive = tokio::time::interval_at(Instant::now() + KEEPALIVE, KEEPALIVE);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut send_error: Option<WsError> = None;

        let reader_result = loop {
            tokio::select! {
                maybe = rx.recv() => {
                    let Some(chunk) = maybe else {
                        // Input closed: ask Deepgram to flush and wait for the final results.
                        if let Err(e) = sink.send(Message::Text(r#"{"type":"CloseStream"}"#.to_string())).await {
                            tracing::warn!("could not send CloseStream: {e}");
                        }
                        match tokio::time::timeout(CLOSE_WAIT, &mut reader).await {
                            Ok(joined) => break Some(joined),
                            Err(_) => {
                                tracing::warn!("Deepgram did not finish within {} s after CloseStream", CLOSE_WAIT.as_secs());
                                reader.abort();
                                return Ok(());
                            }
                        }
                    };
                    let duration_ms = chunk.samples.len() as u64 * 1000 / 16_000;
                    {
                        let mut maps = mappings.lock().unwrap_or_else(|p| p.into_inner());
                        maps.push(Mapping {
                            stream_start_ms: sent_ms,
                            stream_end_ms: sent_ms + duration_ms,
                            session_start_ms: chunk.start_ms,
                            source: chunk.source,
                        });
                        if maps.len() > MAX_MAPPINGS {
                            let excess = maps.len() - MAX_MAPPINGS;
                            maps.drain(..excess);
                        }
                    }
                    sent_ms += duration_ms;

                    let pcm = f32_to_i16(&chunk.samples);
                    for frame in pcm.chunks(FRAME_SAMPLES) {
                        let mut bytes = Vec::with_capacity(frame.len() * 2);
                        for s in frame {
                            bytes.extend_from_slice(&s.to_le_bytes());
                        }
                        if let Err(e) = sink.send(Message::Binary(bytes)).await {
                            send_error = Some(e);
                            break;
                        }
                    }
                    if send_error.is_some() {
                        break None;
                    }
                    keepalive.reset();
                }
                _ = keepalive.tick() => {
                    if let Err(e) = sink.send(Message::Text(r#"{"type":"KeepAlive"}"#.to_string())).await {
                        send_error = Some(e);
                        break None;
                    }
                }
                joined = &mut reader => {
                    // The server closed the connection while audio was still coming.
                    let inner = joined.map_err(|e| anyhow!("Deepgram reader stopped: {e}"))?;
                    inner?;
                    return Err(anyhow!("Deepgram closed the connection before the session ended. Check deepgram_model and the Deepgram key in Settings."));
                }
            }
        };

        if let Some(e) = send_error {
            reader.abort();
            return Err(anyhow!("Lost the connection to Deepgram: {e}"));
        }
        match reader_result {
            Some(joined) => joined.map_err(|e| anyhow!("Deepgram reader stopped: {e}"))?,
            None => Ok(()),
        }
    }
}

async fn read_loop<S>(
    mut stream: S,
    tx: mpsc::Sender<TranscriptSegment>,
    mappings: Arc<Mutex<Vec<Mapping>>>,
    language: Option<String>,
) -> anyhow::Result<()>
where
    S: Stream<Item = Result<Message, WsError>> + Unpin + Send + 'static,
{
    let mut current_id: Option<String> = None;
    while let Some(msg) = stream.next().await {
        let msg = msg.map_err(|e| anyhow!("Deepgram connection error: {e}"))?;
        match msg {
            Message::Text(text) => {
                let seg = {
                    let maps = mappings.lock().unwrap_or_else(|p| p.into_inner());
                    parse_results(&text, &mut current_id, &maps, language.as_deref())
                };
                if let Some(seg) = seg {
                    if tx.send(seg).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}

/// Parse one Deepgram websocket message. Returns a segment for non-empty `Results` messages.
fn parse_results(
    text: &str,
    current_id: &mut Option<String>,
    maps: &[Mapping],
    language: Option<&str>,
) -> Option<TranscriptSegment> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    match v.get("type").and_then(|t| t.as_str()) {
        Some("Results") => {}
        Some("Error") | Some("Warning") => {
            tracing::warn!("Deepgram message: {text}");
            return None;
        }
        _ => return None,
    }
    let alt = v.pointer("/channel/alternatives/0")?;
    let transcript = alt.get("transcript")?.as_str()?.trim();
    if transcript.is_empty() {
        return None;
    }
    let is_final = v.get("is_final").and_then(|b| b.as_bool()).unwrap_or(false);
    let start_s = v.get("start").and_then(|n| n.as_f64()).unwrap_or(0.0);
    let dur_s = v.get("duration").and_then(|n| n.as_f64()).unwrap_or(0.0);
    let (start_ms, end_ms, source) = map_time(maps, (start_s * 1000.0) as u64, (dur_s * 1000.0) as u64);
    let id = current_id.get_or_insert_with(|| uuid::Uuid::new_v4().to_string()).clone();
    if is_final {
        *current_id = None;
    }
    Some(TranscriptSegment {
        id,
        source,
        speaker: dominant_speaker(alt.get("words")),
        text: transcript.to_string(),
        start_ms,
        end_ms,
        is_final,
        language: language.map(|l| l.to_string()),
    })
}

/// Convert a stream time to session time and find the source of the audio at that time.
fn map_time(maps: &[Mapping], stream_ms: u64, dur_ms: u64) -> (u64, u64, AudioSource) {
    let found = maps
        .iter()
        .rev()
        .find(|m| m.stream_start_ms <= stream_ms)
        .or_else(|| maps.first());
    match found {
        Some(m) => {
            let offset = stream_ms.saturating_sub(m.stream_start_ms);
            let start = m.session_start_ms + offset;
            (start, start + dur_ms, m.source)
        }
        None => (stream_ms, stream_ms + dur_ms, AudioSource::Microphone),
    }
}

/// "Speaker N" for the speaker with the most words. Numbers are 1-based.
fn dominant_speaker(words: Option<&serde_json::Value>) -> Option<String> {
    let words = words?.as_array()?;
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for w in words {
        if let Some(s) = w.get("speaker").and_then(|s| s.as_u64()) {
            *counts.entry(s).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(speaker, _)| format!("Speaker {}", speaker + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maps() -> Vec<Mapping> {
        vec![
            Mapping { stream_start_ms: 0, stream_end_ms: 2_000, session_start_ms: 10_000, source: AudioSource::Microphone },
            Mapping { stream_start_ms: 2_000, stream_end_ms: 5_000, session_start_ms: 30_000, source: AudioSource::System },
        ]
    }

    #[test]
    fn speaker_is_most_common_one_based() {
        let words = serde_json::json!([
            {"word": "a", "speaker": 1}, {"word": "b", "speaker": 0}, {"word": "c", "speaker": 1}
        ]);
        assert_eq!(dominant_speaker(Some(&words)).as_deref(), Some("Speaker 2"));
        assert_eq!(dominant_speaker(None), None);
    }

    #[test]
    fn stream_time_maps_to_session_time_and_source() {
        let (s, e, src) = map_time(&maps(), 2_500, 400);
        assert_eq!((s, e), (30_500, 30_900));
        assert_eq!(src, AudioSource::System);
        let (s, _, src) = map_time(&maps(), 500, 100);
        assert_eq!(s, 10_500);
        assert_eq!(src, AudioSource::Microphone);
    }

    #[test]
    fn interim_results_share_one_id_until_final() {
        let interim = r#"{"type":"Results","start":0.5,"duration":1.0,"is_final":false,
            "channel":{"alternatives":[{"transcript":"hello wor","words":[{"word":"hello","speaker":0}]}]}}"#;
        let fin = r#"{"type":"Results","start":0.5,"duration":1.2,"is_final":true,
            "channel":{"alternatives":[{"transcript":"hello world","words":[{"word":"hello","speaker":0}]}]}}"#;
        let mut id = None;
        let a = parse_results(interim, &mut id, &maps(), Some("en")).unwrap();
        let b = parse_results(fin, &mut id, &maps(), Some("en")).unwrap();
        assert!(!a.is_final && b.is_final);
        assert_eq!(a.id, b.id);
        assert_eq!(b.text, "hello world");
        assert_eq!(b.speaker.as_deref(), Some("Speaker 1"));
        let c = parse_results(interim, &mut id, &maps(), None).unwrap();
        assert_ne!(c.id, b.id);
    }

    #[test]
    fn empty_and_non_result_messages_are_ignored() {
        let mut id = None;
        let empty = r#"{"type":"Results","is_final":true,"channel":{"alternatives":[{"transcript":""}]}}"#;
        assert!(parse_results(empty, &mut id, &[], None).is_none());
        assert!(parse_results(r#"{"type":"Metadata"}"#, &mut id, &[], None).is_none());
        assert!(parse_results("not json", &mut id, &[], None).is_none());
    }
}
