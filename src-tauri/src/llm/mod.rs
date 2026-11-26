//! Claude Messages API client over raw HTTP.
//!
//! Model rules for this client (the API returns 400 otherwise): no `thinking`,
//! `temperature`, `top_p` or `top_k` field, no assistant prefill, and
//! `tool_choice` is only ever `auto`. Thinking is adaptive by default, so
//! responses can contain `thinking` blocks that must be passed back unchanged.

pub mod catalog;
pub mod openai;
pub mod sse;

use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde_json::{json, Map, Value};
use sse::{ApiStreamError, SseParser, StreamAssembler};
use std::sync::OnceLock;
use std::time::Duration;

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2026-10-21";
pub const BETA_HEADER: &str = "server-side-fallback-2026-07-01";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

const MAX_RETRIES: u32 = 3;
const HEADER_TIMEOUT: Duration = Duration::from_secs(120);
const BODY_TIMEOUT: Duration = Duration::from_secs(300);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effort {
    Low,
    #[default]
    Medium,
    High,
    XHigh,
}

impl Effort {
    pub fn as_str(&self) -> &'static str {
        match self {
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::XHigh => "xhigh",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LlmRequest {
    /// Text blocks of the system prompt. The last entry gets `cache_control`.
    pub system: Vec<String>,
    pub messages: Vec<Value>,
    pub tools: Vec<Value>,
    pub max_tokens: u32,
    pub effort: Effort,
    /// Either a full `{"type":"json_schema","schema":{...}}` object or a bare
    /// JSON schema, which is wrapped automatically.
    pub output_format: Option<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct LlmResponse {
    /// Content blocks as raw JSON, ready to append to the conversation unchanged.
    pub content: Vec<Value>,
    pub stop_reason: String,
    pub usage: Value,
    /// Present on `refusal`.
    pub stop_details: Option<Value>,
    /// Ids of tool_use blocks whose streamed input was not valid JSON. Do not run these.
    pub incomplete_tool_ids: Vec<String>,
}

impl LlmResponse {
    /// All text blocks joined in order.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("")
    }

    /// (id, name, input) for every tool_use block.
    pub fn tool_uses(&self) -> Vec<(String, String, Value)> {
        self.content
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .map(|b| {
                (
                    b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    b.get("input").cloned().unwrap_or_else(|| json!({})),
                )
            })
            .collect()
    }

    fn from_message(msg: &Value) -> Result<Self> {
        let content = msg
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| anyhow!("The response had no content array."))?;
        let stop_reason = msg
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let stop_details = msg.get("stop_details").filter(|v| !v.is_null()).cloned();
        let mut incomplete_tool_ids = Vec::new();
        if stop_reason == "max_tokens" {
            for b in &content {
                if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                    if let Some(id) = b.get("id").and_then(Value::as_str) {
                        incomplete_tool_ids.push(id.to_string());
                    }
                }
            }
        }
        Ok(Self {
            content,
            stop_reason,
            usage: msg.get("usage").cloned().unwrap_or(Value::Null),
            stop_details,
            incomplete_tool_ids,
        })
    }
}

/// Build the JSON body for one request. Pure, so it can be unit tested.
pub fn build_body(model: &str, req: &LlmRequest, stream: bool) -> Value {
    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("max_tokens".into(), json!(req.max_tokens));

    if !req.system.is_empty() {
        let last = req.system.len() - 1;
        let blocks: Vec<Value> = req
            .system
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let mut b = json!({"type": "text", "text": text});
                if i == last {
                    b["cache_control"] = json!({"type": "ephemeral"});
                }
                b
            })
            .collect();
        body.insert("system".into(), Value::Array(blocks));
    }

    body.insert("messages".into(), Value::Array(req.messages.clone()));

    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                let mut t = t.clone();
                if stream {
                    if let Some(o) = t.as_object_mut() {
                        o.insert("eager_input_streaming".into(), json!(true));
                    }
                }
                t
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
        body.insert("tool_choice".into(), json!({"type": "auto"}));
    }

    let mut output_config = Map::new();
    output_config.insert("effort".into(), json!(req.effort.as_str()));
    if let Some(fmt) = &req.output_format {
        let wrapped = if fmt.get("type").and_then(Value::as_str) == Some("json_schema") {
            fmt.clone()
        } else {
            json!({"type": "json_schema", "schema": fmt})
        };
        output_config.insert("format".into(), wrapped);
    }
    body.insert("output_config".into(), Value::Object(output_config));

    body.insert("fallbacks".into(), json!("default"));
    if stream {
        body.insert("stream".into(), json!(true));
    }
    Value::Object(body)
}

fn shared_http() -> Result<reqwest::Client> {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = HTTP.get() {
        return Ok(c.clone());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent("relay-assistant/0.1")
        .build()
        .context("Could not create the HTTP client")?;
    let _ = HTTP.set(client.clone());
    Ok(client)
}

fn api_error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        let msg = v["error"]["message"].as_str();
        let kind = v["error"]["type"].as_str();
        match (kind, msg) {
            (Some(k), Some(m)) => return format!("{k}: {m}"),
            (None, Some(m)) => return m.to_string(),
            _ => {}
        }
    }
    let trimmed = body.trim();
    if trimmed.chars().count() > 300 {
        let cut: String = trimmed.chars().take(300).collect();
        format!("{cut}...")
    } else if trimmed.is_empty() {
        "empty response body".to_string()
    } else {
        trimmed.to_string()
    }
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let raw = resp.headers().get("retry-after")?.to_str().ok()?;
    let secs: f64 = raw.trim().parse().ok()?;
    if secs.is_finite() && secs >= 0.0 {
        Some(Duration::from_secs_f64(secs.min(60.0)))
    } else {
        None
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1u64 << attempt.min(5))
}

fn status_is_retryable(status: reqwest::StatusCode) -> bool {
    status.as_u16() == 429 || (500..=599).contains(&status.as_u16())
}

#[derive(Clone)]
pub struct ClaudeClient {
    http: reqwest::Client,
    model: String,
}

struct StreamFail {
    err: anyhow::Error,
    retryable: bool,
}

impl ClaudeClient {
    /// Cheap: the HTTP client is shared, and the API key is read at call time.
    pub fn new(model: String) -> Result<Self> {
        let model = if model.trim().is_empty() {
            DEFAULT_MODEL.to_string()
        } else {
            model
        };
        Ok(Self {
            http: shared_http()?,
            model,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Send the request with retries and return the successful response.
    async fn send(&self, body: &Value) -> Result<reqwest::Response> {
        let key = crate::settings::get_api_key("anthropic").ok_or_else(|| {
            anyhow!("No Anthropic API key set. Add it in Settings > API keys.")
        })?;
        let bytes = serde_json::to_vec(body).context("Could not encode the request")?;
        let mut attempt: u32 = 0;
        loop {
            let rb = self
                .http
                .post(API_URL)
                .header("x-api-key", key.as_str())
                .header("anthropic-version", API_VERSION)
                .header("anthropic-beta", BETA_HEADER)
                .header("content-type", "application/json")
                .body(bytes.clone());
            let sent = tokio::time::timeout(HEADER_TIMEOUT, rb.send()).await;
            let resp = match sent {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    if !e.is_builder() && attempt < MAX_RETRIES {
                        tracing::warn!("Claude request failed ({e}), retrying");
                        tokio::time::sleep(backoff(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    bail!("Could not reach the Claude API: {e}");
                }
                Err(_) => {
                    if attempt < MAX_RETRIES {
                        tracing::warn!("Claude request timed out, retrying");
                        tokio::time::sleep(backoff(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    bail!("The Claude API did not answer within {} seconds.", HEADER_TIMEOUT.as_secs());
                }
            };
            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            let wait = retry_after(&resp);
            let text = resp.text().await.unwrap_or_default();
            if status_is_retryable(status) && attempt < MAX_RETRIES {
                let delay = wait.unwrap_or_else(|| backoff(attempt));
                tracing::warn!("Claude API returned {status}, retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                attempt += 1;
                continue;
            }
            bail!("Claude API returned HTTP {}: {}", status.as_u16(), api_error_message(&text));
        }
    }

    /// One request, one JSON response.
    pub async fn complete(&self, req: LlmRequest) -> Result<LlmResponse> {
        let body = build_body(&self.model, &req, false);
        let resp = self.send(&body).await?;
        let text = tokio::time::timeout(BODY_TIMEOUT, resp.text())
            .await
            .map_err(|_| anyhow!("The Claude API response timed out."))?
            .context("Could not read the Claude API response")?;
        let msg: Value = serde_json::from_str(&text)
            .map_err(|e| anyhow!("The Claude API response was not valid JSON: {e}"))?;
        LlmResponse::from_message(&msg)
    }

    /// Streaming request. `on_text` receives each text delta as it arrives.
    pub async fn stream(
        &self,
        req: LlmRequest,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> Result<LlmResponse> {
        let body = build_body(&self.model, &req, true);
        let mut attempt: u32 = 0;
        loop {
            let resp = self.send(&body).await?;
            match Self::read_stream(resp, &mut *on_text).await {
                Ok(r) => return Ok(r),
                Err(fail) => {
                    if fail.retryable && attempt < MAX_RETRIES {
                        tracing::warn!("Claude stream failed ({}), retrying", fail.err);
                        tokio::time::sleep(backoff(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(fail.err);
                }
            }
        }
    }

    async fn read_stream(
        resp: reqwest::Response,
        on_text: &mut (dyn FnMut(&str) + Send),
    ) -> std::result::Result<LlmResponse, StreamFail> {
        let mut bytes = Box::pin(resp.bytes_stream());
        let mut parser = SseParser::new();
        let mut asm = StreamAssembler::new();

        fn fail(err: anyhow::Error, asm: &StreamAssembler, transient: bool) -> StreamFail {
            StreamFail {
                retryable: transient && !asm.has_content(),
                err,
            }
        }

        loop {
            let next = tokio::time::timeout(STREAM_IDLE_TIMEOUT, bytes.next()).await;
            let chunk = match next {
                Err(_) => {
                    return Err(fail(
                        anyhow!("The Claude stream stalled for {} seconds.", STREAM_IDLE_TIMEOUT.as_secs()),
                        &asm,
                        true,
                    ))
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    return Err(fail(anyhow!("The Claude stream was interrupted: {e}"), &asm, true))
                }
                Ok(Some(Ok(c))) => c,
            };
            for ev in parser.push(&chunk) {
                if let Err(e) = asm.handle(&ev.data, &mut *on_text) {
                    let transient = e
                        .downcast_ref::<ApiStreamError>()
                        .map(|a| a.is_transient())
                        .unwrap_or(false);
                    return Err(fail(e, &asm, transient));
                }
            }
        }
        if let Some(ev) = parser.finish() {
            if let Err(e) = asm.handle(&ev.data, &mut *on_text) {
                return Err(fail(e, &asm, false));
            }
        }
        match asm.finish() {
            Ok(r) => Ok(r),
            Err(e) => Err(StreamFail {
                err: e,
                retryable: false,
            }),
        }
    }
}

/// The model client used by the text pipelines. Which variant runs depends on the source
/// chosen for the feature. Pipelines only call `complete`. Tool use and streaming
/// (the agent) go through `ClaudeClient` directly.
#[derive(Clone)]
pub enum LlmClient {
    Claude(ClaudeClient),
    OpenAi(openai::OpenAiClient),
}

impl LlmClient {
    /// Client for a task, using the source and model stored in settings.
    pub fn for_task(settings: &crate::settings::Settings, task: crate::settings::LlmTask) -> Result<Self> {
        Self::for_choice(settings, &settings.choice_for(task))
    }

    /// Client for an explicit source and model. Addresses come from `settings`.
    pub fn for_choice(settings: &crate::settings::Settings, choice: &crate::settings::ModelChoice) -> Result<Self> {
        use crate::settings::LlmSource;
        let model = choice.model.trim().to_string();
        match choice.source {
            LlmSource::Anthropic => Ok(Self::Claude(ClaudeClient::new(model)?)),
            LlmSource::OpenAi => Ok(Self::OpenAi(openai::OpenAiClient::new(
                &settings.openai_base_url,
                model,
                openai::OPENAI_KEY_PROVIDER,
            )?)),
            LlmSource::Server => Ok(Self::OpenAi(openai::OpenAiClient::new(
                &settings.llm_base_url,
                model,
                openai::KEY_PROVIDER,
            )?)),
        }
    }

    pub fn model(&self) -> &str {
        match self {
            Self::Claude(c) => c.model(),
            Self::OpenAi(c) => c.model(),
        }
    }

    pub async fn complete(&self, req: LlmRequest) -> Result<LlmResponse> {
        match self {
            Self::Claude(c) => c.complete(req).await,
            Self::OpenAi(c) => c.complete(req).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_follows_the_source_of_each_task() {
        use crate::settings::{LlmSource, LlmTask, ModelChoice, Settings};
        let mut s = Settings::default();
        assert!(matches!(LlmClient::for_task(&s, LlmTask::Dictation).unwrap(), LlmClient::Claude(_)));
        s.dictation_llm = ModelChoice { source: LlmSource::Server, model: "llama3.2".into() };
        let c = LlmClient::for_task(&s, LlmTask::Dictation).unwrap();
        assert!(matches!(c, LlmClient::OpenAi(_)));
        assert_eq!(c.model(), "llama3.2");
        // Other tasks keep their own source.
        assert!(matches!(LlmClient::for_task(&s, LlmTask::Notes).unwrap(), LlmClient::Claude(_)));
        s.llm_base_url = String::new();
        assert!(LlmClient::for_task(&s, LlmTask::Dictation).is_err());

        s.planner_llm = ModelChoice { source: LlmSource::OpenAi, model: "gpt-x".into() };
        let c = LlmClient::for_task(&s, LlmTask::Planner).unwrap();
        assert!(matches!(c, LlmClient::OpenAi(_)));
        assert_eq!(c.model(), "gpt-x");
        s.openai_base_url = String::new();
        assert!(LlmClient::for_choice(&s, &s.planner_llm).is_err());
    }

    fn sample_request() -> LlmRequest {
        LlmRequest {
            system: vec!["First block.".into(), "Second block.".into()],
            messages: vec![json!({"role": "user", "content": "hello"})],
            tools: vec![json!({
                "name": "read_file",
                "description": "Read a file.",
                "input_schema": {"type": "object", "properties": {}, "required": [], "additionalProperties": false},
                "strict": true
            })],
            max_tokens: 1000,
            effort: Effort::High,
            output_format: None,
        }
    }

    #[test]
    fn body_has_no_forbidden_keys() {
        let body = build_body("claude-opus-5-5", &sample_request(), true);
        let obj = body.as_object().unwrap();
        for key in ["thinking", "temperature", "top_p", "top_k"] {
            assert!(!obj.contains_key(key), "{key} must not be sent");
        }
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["stream"], true);
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["tool_choice"], json!({"type": "auto"}));
        assert_eq!(body["tools"][0]["eager_input_streaming"], true);
        assert_eq!(body["tools"][0]["strict"], true);
    }

    #[test]
    fn cache_control_only_on_last_system_block() {
        let body = build_body("m", &sample_request(), false);
        let system = body["system"].as_array().unwrap();
        assert_eq!(system.len(), 2);
        assert!(system[0].get("cache_control").is_none());
        assert_eq!(system[1]["cache_control"], json!({"type": "ephemeral"}));
        assert_eq!(system[1]["text"], "Second block.");
    }

    #[test]
    fn non_streaming_body_differs_only_where_expected() {
        let body = build_body("m", &sample_request(), false);
        assert!(body.get("stream").is_none());
        assert!(body["tools"][0].get("eager_input_streaming").is_none());
    }

    #[test]
    fn no_tools_means_no_tool_choice() {
        let mut req = sample_request();
        req.tools.clear();
        let body = build_body("m", &req, false);
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn output_format_goes_inside_output_config() {
        let mut req = sample_request();
        req.output_format = Some(json!({"type": "object", "properties": {}}));
        let body = build_body("m", &req, false);
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(body["output_config"]["format"]["schema"]["type"], "object");
        assert_eq!(body["output_config"]["effort"], "high");

        req.output_format = Some(json!({"type": "json_schema", "schema": {"type": "object"}}));
        let body = build_body("m", &req, false);
        assert_eq!(body["output_config"]["format"]["schema"]["type"], "object");
    }

    #[test]
    fn response_helpers() {
        let resp = LlmResponse {
            content: vec![
                json!({"type": "thinking", "thinking": "x", "signature": "s"}),
                json!({"type": "text", "text": "Hello "}),
                json!({"type": "text", "text": "there"}),
                json!({"type": "tool_use", "id": "t1", "name": "list_dir", "input": {"path": "C:/"}}),
            ],
            stop_reason: "tool_use".into(),
            ..Default::default()
        };
        assert_eq!(resp.text(), "Hello there");
        let uses = resp.tool_uses();
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].2["path"], "C:/");
    }

    #[test]
    fn error_message_parsing() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad field"}}"#;
        assert_eq!(api_error_message(body), "invalid_request_error: bad field");
        assert_eq!(api_error_message(""), "empty response body");
    }
}
