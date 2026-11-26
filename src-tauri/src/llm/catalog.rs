//! The model catalog: which models Relay can offer right now, derived from the stored API
//! keys and from the servers that answer. One source failing never fails the catalog.
//!
//! Every fetch has a short timeout and returns a reason that is safe to show. A key is
//! only ever sent in a header and never appears in a note.

use super::openai::{key_transport_ok, KEY_PROVIDER, OPENAI_KEY_PROVIDER};
use crate::settings::{get_api_key, LlmSource, Settings, SttEngineKind};
use crate::stt::models::WhisperModelInfo;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

const FETCH_TIMEOUT: Duration = Duration::from_secs(4);

/// Claude models offered when the list cannot be fetched.
const BUILT_IN_CLAUDE: [(&str, &str); 4] = [
    ("claude-haiku-5-5", "Haiku 5.5"),
    ("claude-sonnet-5-5", "Sonnet 5.5"),
    ("claude-opus-5-5", "Opus 5.5"),
    ("claude-fable-5-2", "Fable 5.2"),
];

/// Ids containing any of these are not chat models.
const NOT_CHAT: [&str; 12] = [
    "embedding", "tts", "whisper", "transcribe", "dall-e", "image", "moderation", "audio", "realtime", "davinci",
    "babbage", "search",
];

const OPENAI_STT_FALLBACK: [&str; 3] = ["gpt-4o-transcribe", "gpt-4o-mini-transcribe", "whisper-1"];
const DEEPGRAM_MODELS: [&str; 2] = ["nova-3", "nova-2"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
}

impl ModelInfo {
    fn same(id: &str) -> Self {
        Self { id: id.to_string(), label: id.to_string() }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSource {
    pub source: LlmSource,
    pub label: String,
    pub local: bool,
    pub has_key: bool,
    pub available: bool,
    pub note: Option<String>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechOption {
    pub engine: SttEngineKind,
    pub label: String,
    pub local: bool,
    pub available: bool,
    pub note: Option<String>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
    /// Always three entries, in the order Anthropic, OpenAi, Server.
    pub sources: Vec<ModelSource>,
    /// Always three entries, in the order localWhisper, openAiCompatible, deepgram.
    pub speech: Vec<SpeechOption>,
}

// ---------------------------------------------------------------------------
// Fetching

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The address cannot be used.
    BadUrl,
    /// Nothing answered.
    Unreachable,
    Timeout,
    /// The server answered with an error status.
    Http(u16),
    /// The reply was not a model list.
    Invalid,
    /// A key was set but the address is plain http to another computer.
    PlainHttp,
}

/// GET a JSON document with a 4 second limit.
async fn get_json(url: &str, headers: &[(&str, &str)], bearer: Option<&str>) -> Result<Value, FetchError> {
    let http = super::shared_http().map_err(|_| FetchError::Unreachable)?;
    let mut rb = http.get(url);
    for (k, v) in headers {
        rb = rb.header(*k, *v);
    }
    if let Some(k) = bearer {
        rb = rb.bearer_auth(k);
    }
    let work = async {
        let resp = rb.send().await.map_err(|e| {
            if e.is_builder() {
                FetchError::BadUrl
            } else {
                FetchError::Unreachable
            }
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(FetchError::Http(status.as_u16()));
        }
        let text = resp.text().await.map_err(|_| FetchError::Unreachable)?;
        serde_json::from_str::<Value>(&text).map_err(|_| FetchError::Invalid)
    };
    tokio::time::timeout(FETCH_TIMEOUT, work).await.unwrap_or(Err(FetchError::Timeout))
}

fn models_url(base: &str) -> Result<String, FetchError> {
    let base = base.trim().trim_end_matches('/');
    match reqwest::Url::parse(base) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => Ok(format!("{base}/models")),
        _ => Err(FetchError::BadUrl),
    }
}

/// Ids from an OpenAI style `{"data":[{"id":...}]}` reply.
fn parse_ids(v: &Value) -> Result<Vec<String>, FetchError> {
    let data = v.get("data").and_then(Value::as_array).ok_or(FetchError::Invalid)?;
    Ok(data
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .map(|s| s.to_string())
        .collect())
}

/// Models from the Anthropic list reply, with `display_name` as the label when present.
fn parse_anthropic(v: &Value) -> Result<Vec<ModelInfo>, FetchError> {
    let data = v.get("data").and_then(Value::as_array).ok_or(FetchError::Invalid)?;
    Ok(data
        .iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(Value::as_str)?;
            let label = m
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(id);
            Some(ModelInfo { id: id.to_string(), label: label.to_string() })
        })
        .collect())
}

/// The key is sent only over https, or over http to this computer.
fn bearer_for<'a>(url: &str, key: Option<&'a str>) -> Option<&'a str> {
    key.filter(|_| key_transport_ok(url))
}

async fn fetch_anthropic(key: &str) -> Result<Vec<ModelInfo>, FetchError> {
    let v = get_json(
        "https://api.anthropic.com/v1/models?limit=100",
        &[("x-api-key", key), ("anthropic-version", super::API_VERSION)],
        None,
    )
    .await?;
    parse_anthropic(&v)
}

/// Model ids of an OpenAI compatible endpoint. Without `key` the request is anonymous.
async fn fetch_ids(base: &str, key: Option<&str>) -> Result<Vec<String>, FetchError> {
    let url = models_url(base)?;
    let v = get_json(&url, &[], bearer_for(&url, key)).await?;
    parse_ids(&v)
}

/// Model ids of the OpenAI account. The key is only sent over https or to this computer.
async fn fetch_openai(base: &str, key: &str) -> Result<Vec<String>, FetchError> {
    let url = models_url(base)?;
    if !key_transport_ok(&url) {
        return Err(FetchError::PlainHttp);
    }
    parse_ids(&get_json(&url, &[], Some(key)).await?)
}

// ---------------------------------------------------------------------------
// Pure helpers

/// True for ids that can answer a chat request.
pub fn is_chat_model(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    !NOT_CHAT.iter().any(|bad| id.contains(bad))
}

/// Chat-capable ids, sorted and without duplicates.
pub fn chat_models(ids: &[String]) -> Vec<ModelInfo> {
    let mut v: Vec<&String> = ids.iter().filter(|id| is_chat_model(id)).collect();
    v.sort();
    v.dedup();
    v.into_iter().map(|id| ModelInfo::same(id)).collect()
}

/// Speech to text ids from a model list, or the usual three when none match.
pub fn speech_models(ids: &[String]) -> Vec<ModelInfo> {
    let mut v: Vec<&String> = ids
        .iter()
        .filter(|id| {
            let l = id.to_ascii_lowercase();
            l.contains("whisper") || l.contains("transcribe")
        })
        .collect();
    v.sort();
    v.dedup();
    if v.is_empty() {
        return OPENAI_STT_FALLBACK.iter().map(|id| ModelInfo::same(id)).collect();
    }
    v.into_iter().map(|id| ModelInfo::same(id)).collect()
}

fn host_and_port(base: &str) -> Option<(String, Option<u16>)> {
    let u = reqwest::Url::parse(base.trim()).ok()?;
    let host = u.host_str()?.trim_matches(['[', ']']).to_string();
    Some((host, u.port()))
}

/// True when the address points at this computer.
pub fn is_local_address(base: &str) -> bool {
    match host_and_port(base) {
        Some((host, _)) => {
            host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
        }
        None => false,
    }
}

/// "OpenAI" for api.openai.com, otherwise the host name.
pub fn openai_label(base: &str) -> String {
    match host_and_port(base) {
        Some((host, _)) if !host.eq_ignore_ascii_case("api.openai.com") => host,
        _ => "OpenAI".to_string(),
    }
}

/// "Ollama" for port 11434, "LM Studio" for port 1234, otherwise the host name.
pub fn server_label(base: &str) -> String {
    match host_and_port(base) {
        Some((_, Some(11434))) => "Ollama".to_string(),
        Some((_, Some(1234))) => "LM Studio".to_string(),
        Some((host, _)) => host,
        None => "Custom server".to_string(),
    }
}

fn built_in_claude() -> Vec<ModelInfo> {
    BUILT_IN_CLAUDE
        .iter()
        .map(|(id, label)| ModelInfo { id: id.to_string(), label: label.to_string() })
        .collect()
}

/// Short reason for an OpenAI request that failed. Never contains a key.
fn openai_reason(e: &FetchError, host: &str) -> String {
    match e {
        FetchError::BadUrl => "The OpenAI address is not valid.".to_string(),
        FetchError::Unreachable => format!("Could not reach {host}."),
        FetchError::Timeout => format!("{host} did not answer in time."),
        FetchError::Http(c @ (401 | 403)) => format!("The key was refused (HTTP {c})."),
        FetchError::Http(c) => format!("{host} returned HTTP {c}."),
        FetchError::Invalid => format!("{host} did not return a model list."),
        FetchError::PlainHttp => "The key is not sent over plain http to another computer.".to_string(),
    }
}

pub fn anthropic_source(has_key: bool, fetched: Option<Result<Vec<ModelInfo>, FetchError>>) -> ModelSource {
    let (available, note, models) = match (has_key, fetched) {
        (false, _) => (false, Some("Add an Anthropic key.".to_string()), built_in_claude()),
        (true, Some(Ok(m))) if !m.is_empty() => (true, None, m),
        (true, _) => (
            true,
            Some("Could not list models. Showing the built-in list.".to_string()),
            built_in_claude(),
        ),
    };
    ModelSource {
        source: LlmSource::Anthropic,
        label: "Claude".into(),
        local: false,
        has_key,
        available,
        note,
        models,
    }
}

pub fn openai_source(base: &str, has_key: bool, fetched: Option<Result<Vec<String>, FetchError>>) -> ModelSource {
    let label = openai_label(base);
    let (available, note, models) = match (has_key, fetched) {
        (false, _) | (true, None) => (false, Some("Add an OpenAI key.".to_string()), Vec::new()),
        (true, Some(Ok(ids))) => (true, None, chat_models(&ids)),
        (true, Some(Err(e))) => (false, Some(openai_reason(&e, &label)), Vec::new()),
    };
    ModelSource {
        source: LlmSource::OpenAi,
        local: is_local_address(base),
        label,
        has_key,
        available,
        note,
        models,
    }
}

pub fn server_source(base: &str, has_key: bool, fetched: Option<Result<Vec<String>, FetchError>>) -> ModelSource {
    let base = base.trim();
    let (available, note, models) = match fetched {
        _ if base.is_empty() => (false, Some("No server address set.".to_string()), Vec::new()),
        Some(Ok(ids)) => (true, None, ids.iter().map(|id| ModelInfo::same(id)).collect()),
        Some(Err(FetchError::Http(code))) => (
            false,
            Some(format!("The server at {base} returned HTTP {code}. Check the address and the key.")),
            Vec::new(),
        ),
        Some(Err(FetchError::Invalid)) => (
            false,
            Some(format!("The server at {base} did not return a model list. Check that the address ends with /v1.")),
            Vec::new(),
        ),
        _ => (false, Some(format!("No server answered at {base}.")), Vec::new()),
    };
    ModelSource {
        source: LlmSource::Server,
        label: server_label(base),
        local: is_local_address(base),
        has_key,
        available,
        note,
        models,
    }
}

/// The local Whisper entry from the catalog of models and what is on disk.
pub fn local_whisper_option(built: bool, models: &[WhisperModelInfo]) -> SpeechOption {
    let downloaded: Vec<ModelInfo> = models
        .iter()
        .filter(|m| m.downloaded)
        .map(|m| ModelInfo { id: m.name.to_string(), label: m.label.to_string() })
        .collect();
    let (available, note, models) = if !built {
        (false, Some("This build has no local Whisper.".to_string()), Vec::new())
    } else if downloaded.is_empty() {
        (false, Some("No model downloaded.".to_string()), downloaded)
    } else {
        (true, None, downloaded)
    };
    SpeechOption {
        engine: SttEngineKind::LocalWhisper,
        label: "Local Whisper".into(),
        local: true,
        available,
        note,
        models,
    }
}

pub fn openai_speech_option(base: &str, has_key: bool, fetched: Option<&Vec<String>>) -> SpeechOption {
    let models = match fetched {
        Some(ids) if has_key => speech_models(ids),
        _ => speech_models(&[]),
    };
    SpeechOption {
        engine: SttEngineKind::OpenAiCompatible,
        label: openai_label(base),
        local: is_local_address(base),
        available: has_key,
        note: if has_key { None } else { Some("Add an OpenAI key.".to_string()) },
        models,
    }
}

pub fn deepgram_option(has_key: bool) -> SpeechOption {
    SpeechOption {
        engine: SttEngineKind::Deepgram,
        label: "Deepgram".into(),
        local: false,
        available: has_key,
        note: if has_key { None } else { Some("Add a Deepgram key.".to_string()) },
        models: DEEPGRAM_MODELS.iter().map(|id| ModelInfo::same(id)).collect(),
    }
}

// ---------------------------------------------------------------------------
// Assembly

/// Build the catalog. The three sources are queried at the same time.
pub async fn build(settings: &Settings, models_dir: &Path) -> ModelCatalog {
    let anthropic_key = get_api_key("anthropic");
    let openai_key = get_api_key(OPENAI_KEY_PROVIDER);
    let server_key = get_api_key(KEY_PROVIDER);
    let deepgram_key = get_api_key("deepgram");

    let server_base = settings.llm_base_url.trim().to_string();
    let openai_base = settings.openai_base_url.trim().to_string();

    let (claude, openai, server) = tokio::join!(
        async {
            match anthropic_key.as_deref() {
                Some(k) => Some(fetch_anthropic(k).await),
                None => None,
            }
        },
        async {
            match openai_key.as_deref() {
                Some(k) => Some(fetch_openai(&openai_base, k).await),
                None => None,
            }
        },
        async {
            if server_base.is_empty() {
                None
            } else {
                Some(fetch_ids(&server_base, server_key.as_deref()).await)
            }
        },
    );

    let openai_ids = openai.as_ref().and_then(|r| r.as_ref().ok()).cloned();
    let sources = vec![
        anthropic_source(anthropic_key.is_some(), claude),
        openai_source(&openai_base, openai_key.is_some(), openai),
        server_source(&server_base, server_key.is_some(), server),
    ];
    let speech = vec![
        local_whisper_option(crate::stt::LOCAL_WHISPER_BUILT, &crate::stt::models::list(models_dir)),
        openai_speech_option(&openai_base, openai_key.is_some(), openai_ids.as_ref()),
        deepgram_option(deepgram_key.is_some()),
    ];
    ModelCatalog { sources, speech }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn model_ids(m: &[ModelInfo]) -> Vec<&str> {
        m.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn chat_filter_drops_non_chat_models_and_sorts() {
        let all = ids(&[
            "gpt-4o-mini",
            "text-embedding-3-small",
            "whisper-1",
            "gpt-4o-transcribe",
            "tts-1",
            "dall-e-3",
            "gpt-image-1",
            "omni-moderation-latest",
            "gpt-4o-audio-preview",
            "gpt-4o-realtime-preview",
            "davinci-002",
            "babbage-002",
            "gpt-4o-search-preview",
            "gpt-4.1",
            "gpt-4o-mini",
        ]);
        assert_eq!(model_ids(&chat_models(&all)), ["gpt-4.1", "gpt-4o-mini"]);
        assert!(is_chat_model("llama3.2:latest"));
        assert!(!is_chat_model("TEXT-EMBEDDING-3-LARGE"));
    }

    #[test]
    fn speech_models_pick_whisper_and_transcribe_or_fall_back() {
        let all = ids(&["gpt-4o", "whisper-1", "gpt-4o-transcribe", "tts-1"]);
        assert_eq!(model_ids(&speech_models(&all)), ["gpt-4o-transcribe", "whisper-1"]);
        assert_eq!(
            model_ids(&speech_models(&ids(&["gpt-4o"]))),
            ["gpt-4o-transcribe", "gpt-4o-mini-transcribe", "whisper-1"]
        );
    }

    #[test]
    fn labels_follow_host_and_port() {
        assert_eq!(openai_label("https://api.openai.com/v1"), "OpenAI");
        assert_eq!(openai_label("https://api.groq.com/openai/v1"), "api.groq.com");
        assert_eq!(openai_label("not a url"), "OpenAI");
        assert_eq!(server_label("http://localhost:11434/v1"), "Ollama");
        assert_eq!(server_label("http://127.0.0.1:1234/v1"), "LM Studio");
        assert_eq!(server_label("http://192.168.1.20:8080/v1"), "192.168.1.20");
        assert_eq!(server_label("https://models.example.com/v1"), "models.example.com");
        assert_eq!(server_label(""), "Custom server");
    }

    #[test]
    fn local_means_this_computer() {
        assert!(is_local_address("http://localhost:11434/v1"));
        assert!(is_local_address("http://127.0.0.1:1234/v1"));
        assert!(is_local_address("http://[::1]:1234/v1"));
        assert!(!is_local_address("http://192.168.1.20:11434/v1"));
        assert!(!is_local_address("https://api.openai.com/v1"));
        assert!(!is_local_address(""));
    }

    #[test]
    fn parses_openai_and_anthropic_replies() {
        let v = json!({"object": "list", "data": [{"id": "a"}, {"id": "b"}, {"nope": 1}]});
        assert_eq!(parse_ids(&v).unwrap(), ["a", "b"]);
        assert_eq!(parse_ids(&json!({})), Err(FetchError::Invalid));
        let v = json!({"data": [
            {"id": "claude-x", "display_name": "Claude X", "type": "model"},
            {"id": "claude-y"}
        ], "has_more": false});
        let m = parse_anthropic(&v).unwrap();
        assert_eq!(m[0].label, "Claude X");
        assert_eq!(m[1].label, "claude-y");
    }

    #[test]
    fn anthropic_source_states() {
        let s = anthropic_source(false, None);
        assert!(!s.available && !s.has_key);
        assert_eq!(s.note.as_deref(), Some("Add an Anthropic key."));
        assert_eq!(s.models.len(), 4);
        assert_eq!(s.models[3].id, "claude-fable-5-2");

        let live = vec![ModelInfo { id: "claude-z".into(), label: "Z".into() }];
        let s = anthropic_source(true, Some(Ok(live)));
        assert!(s.available && s.has_key);
        assert!(s.note.is_none());
        assert_eq!(model_ids(&s.models), ["claude-z"]);

        for failed in [Some(Err(FetchError::Timeout)), None, Some(Ok(vec![]))] {
            let s = anthropic_source(true, failed);
            assert!(s.available);
            assert_eq!(s.note.as_deref(), Some("Could not list models. Showing the built-in list."));
            assert_eq!(s.models.len(), 4);
        }
    }

    #[test]
    fn openai_source_states() {
        let base = "https://api.openai.com/v1";
        let s = openai_source(base, false, None);
        assert_eq!(s.label, "OpenAI");
        assert!(!s.available && s.models.is_empty());
        assert_eq!(s.note.as_deref(), Some("Add an OpenAI key."));

        let s = openai_source(base, true, Some(Ok(ids(&["gpt-4o", "whisper-1"]))));
        assert!(s.available && s.has_key);
        assert_eq!(model_ids(&s.models), ["gpt-4o"]);

        let s = openai_source(base, true, Some(Err(FetchError::Http(401))));
        assert!(!s.available);
        assert_eq!(s.note.as_deref(), Some("The key was refused (HTTP 401)."));
        let s = openai_source(base, true, Some(Err(FetchError::Unreachable)));
        assert_eq!(s.note.as_deref(), Some("Could not reach OpenAI."));
        let s = openai_source(base, true, Some(Err(FetchError::Http(500))));
        assert_eq!(s.note.as_deref(), Some("OpenAI returned HTTP 500."));
    }

    #[test]
    fn server_source_states() {
        let base = "http://localhost:11434/v1";
        let s = server_source(base, false, Some(Ok(ids(&["llama3.2", "qwen2.5"]))));
        assert!(s.available && s.local);
        assert_eq!(s.label, "Ollama");
        assert_eq!(model_ids(&s.models), ["llama3.2", "qwen2.5"]);

        let s = server_source(base, false, Some(Err(FetchError::Unreachable)));
        assert!(!s.available);
        assert_eq!(s.note.as_deref(), Some("No server answered at http://localhost:11434/v1."));
        let s = server_source(base, false, Some(Err(FetchError::Timeout)));
        assert_eq!(s.note.as_deref(), Some("No server answered at http://localhost:11434/v1."));

        let s = server_source("  ", false, None);
        assert!(!s.available && !s.local);
        assert_eq!(s.note.as_deref(), Some("No server address set."));
        assert!(s.models.is_empty());
    }

    #[test]
    fn local_whisper_states() {
        let mut models: Vec<WhisperModelInfo> = crate::stt::models::CATALOG.to_vec();
        let o = local_whisper_option(true, &models);
        assert!(!o.available && o.local && o.models.is_empty());
        assert_eq!(o.note.as_deref(), Some("No model downloaded."));

        models[0].downloaded = true;
        let o = local_whisper_option(true, &models);
        assert!(o.available);
        assert!(o.note.is_none());
        assert_eq!(o.models[0].id, models[0].name);
        assert_eq!(o.models[0].label, models[0].label);

        let o = local_whisper_option(false, &models);
        assert!(!o.available && o.models.is_empty());
        assert_eq!(o.note.as_deref(), Some("This build has no local Whisper."));
    }

    #[test]
    fn cloud_speech_options() {
        let base = "https://api.openai.com/v1";
        let o = openai_speech_option(base, false, None);
        assert!(!o.available);
        assert_eq!(o.note.as_deref(), Some("Add an OpenAI key."));
        assert_eq!(o.models.len(), 3);
        let listed = ids(&["whisper-1", "gpt-4o"]);
        let o = openai_speech_option(base, true, Some(&listed));
        assert!(o.available);
        assert_eq!(model_ids(&o.models), ["whisper-1"]);
        let o = openai_speech_option(base, true, None);
        assert_eq!(o.models.len(), 3);

        let d = deepgram_option(false);
        assert!(!d.available);
        assert_eq!(d.note.as_deref(), Some("Add a Deepgram key."));
        assert_eq!(model_ids(&d.models), ["nova-3", "nova-2"]);
        assert!(deepgram_option(true).available);
    }

    #[test]
    fn catalog_serializes_in_camel_case() {
        let c = ModelCatalog {
            sources: vec![openai_source("https://api.openai.com/v1", false, None)],
            speech: vec![deepgram_option(true), local_whisper_option(true, &[])],
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["sources"][0]["source"], "openAi");
        assert_eq!(v["sources"][0]["hasKey"], false);
        assert_eq!(v["speech"][0]["engine"], "deepgram");
        assert_eq!(v["speech"][1]["engine"], "localWhisper");
    }

    #[test]
    fn key_is_not_sent_over_plain_http_to_remote_hosts() {
        assert_eq!(bearer_for("https://api.example.com/v1/models", Some("k")), Some("k"));
        assert_eq!(bearer_for("http://localhost:11434/v1/models", Some("k")), Some("k"));
        assert_eq!(bearer_for("http://192.168.1.20/v1/models", Some("k")), None);
        assert_eq!(bearer_for("https://api.example.com/v1/models", None), None);
    }

    #[test]
    fn models_url_needs_a_web_address() {
        assert_eq!(models_url(" http://localhost:11434/v1/ ").unwrap(), "http://localhost:11434/v1/models");
        assert_eq!(models_url(""), Err(FetchError::BadUrl));
        assert_eq!(models_url("localhost:11434"), Err(FetchError::BadUrl));
        assert_eq!(models_url("file:///x"), Err(FetchError::BadUrl));
    }

    /// Serve one canned response and return the raw request head that was received.
    async fn one_shot(response: String) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut tmp).await.unwrap();
                assert!(n > 0, "connection closed early");
                buf.extend_from_slice(&tmp[..n]);
            }
            sock.write_all(response.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
            String::from_utf8_lossy(&buf).to_lowercase()
        });
        (format!("http://127.0.0.1:{port}/v1"), handle)
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn fetches_ids_with_bearer_over_loopback() {
        let (base, head) = one_shot(http("200 OK", r#"{"data":[{"id":"llama3.2"},{"id":"qwen2.5"}]}"#)).await;
        let got = fetch_ids(&base, Some("sk-test-key")).await.unwrap();
        assert_eq!(got, ["llama3.2", "qwen2.5"]);
        let head = head.await.unwrap();
        assert!(head.starts_with("get /v1/models "));
        assert!(head.contains("authorization: bearer sk-test-key"));
    }

    #[tokio::test]
    async fn http_errors_and_junk_are_reported_without_the_key() {
        let (base, _h) = one_shot(http("401 Unauthorized", r#"{"error":{"message":"bad key sk-test-key"}}"#)).await;
        assert_eq!(fetch_ids(&base, Some("sk-test-key")).await, Err(FetchError::Http(401)));
        let (base, _h) = one_shot(http("200 OK", "<html>hello</html>")).await;
        assert_eq!(fetch_ids(&base, None).await, Err(FetchError::Invalid));
        // Nothing listens on this port once the listener is dropped.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        assert_eq!(fetch_ids(&format!("http://127.0.0.1:{port}/v1"), None).await, Err(FetchError::Unreachable));
        assert_eq!(fetch_ids("", None).await, Err(FetchError::BadUrl));
    }
}
