//! Client for OpenAI-compatible chat servers (Ollama, LM Studio, OpenAI, Groq, OpenRouter).
//!
//! Sends `POST {base}/chat/completions` and reads `choices[0].message.content`.
//! Tool use is not supported here: the agent stays on Claude. The optional API
//! key is read from the `openai-llm` credential at call time and is never put
//! in an error message.

use super::{shared_http, LlmRequest, LlmResponse};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::time::Duration;

/// Credential Manager entry for the key of a custom or local server (optional).
pub const KEY_PROVIDER: &str = "openai-llm";
/// Credential Manager entry for the OpenAI account (also used for speech to text).
pub const OPENAI_KEY_PROVIDER: &str = "openai";
pub const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";

const TEMPERATURE: f64 = 0.2;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_RETRIES: u32 = 2;

#[derive(Clone)]
pub struct OpenAiClient {
    http: reqwest::Client,
    url: String,
    model: String,
    /// Credential Manager entry the key is read from at call time.
    key_provider: &'static str,
}

/// The key may go over https, or over http to the local machine. Never over plain
/// http to a remote host.
pub fn key_transport_ok(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    let host = url.host_str().unwrap_or("");
    match url.scheme() {
        "https" => true,
        "http" => {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        _ => false,
    }
}

/// Full chat completions URL for a base URL, or an error for an unusable base.
pub fn chat_url(base: &str) -> Result<String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        bail!("No server address set. Enter a base URL in Settings > Models.");
    }
    let parsed = reqwest::Url::parse(base)
        .map_err(|_| anyhow!("The base URL is not a valid address. Use a form like http://localhost:11434/v1."))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("The base URL must start with http:// or https://.");
    }
    Ok(format!("{base}/chat/completions"))
}

/// Text of a message `content` value, which is a string or an array of blocks.
fn flatten_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                Value::String(s) => Some(s.as_str()),
                _ => b.get("text").and_then(Value::as_str),
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The JSON schema inside an `output_format` value, which may be a bare schema or a
/// `{"type":"json_schema","schema":{...}}` wrapper.
fn schema_of(fmt: &Value) -> &Value {
    if fmt.get("type").and_then(Value::as_str) == Some("json_schema") {
        fmt.get("schema").unwrap_or(fmt)
    } else {
        fmt
    }
}

/// Build the request body. Pure, so it can be unit tested. With `json_mode` the body
/// carries `response_format: {type: json_object}`. The schema instruction is added to
/// the system prompt whenever `output_format` is set, so the model knows the shape even
/// when the server ignores `response_format`.
pub fn build_body(model: &str, req: &LlmRequest, json_mode: bool) -> Value {
    let mut system = req.system.join("\n\n");
    if let Some(fmt) = &req.output_format {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(&format!(
            "Reply with one JSON object only, with no other text and no code fence. It must match this JSON schema:\n{}",
            schema_of(fmt)
        ));
    }

    let mut messages: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    for m in &req.messages {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = flatten_content(m.get("content").unwrap_or(&Value::Null));
        messages.push(json!({"role": role, "content": content}));
    }

    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("max_tokens".into(), json!(req.max_tokens));
    body.insert("temperature".into(), json!(TEMPERATURE));
    body.insert("stream".into(), json!(false));
    if json_mode {
        body.insert("response_format".into(), json!({"type": "json_object"}));
    }
    Value::Object(body)
}

/// Map an OpenAI `finish_reason` to the stop reasons the pipelines understand.
pub fn map_finish_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("length") => "max_tokens",
        Some("content_filter") => "refusal",
        Some("tool_calls") | Some("function_call") => "tool_use",
        _ => "end_turn",
    }
}

/// Some local reasoning models put their thoughts in a leading `<think>` block.
fn strip_think(text: &str) -> &str {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            return rest[end + "</think>".len()..].trim_start();
        }
    }
    text
}

/// Parse a chat completions response into an `LlmResponse` with one text block.
pub fn parse_response(v: &Value) -> Result<LlmResponse> {
    let choice = v
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("The server reply had no choices."))?;
    let message = choice.get("message").unwrap_or(&Value::Null);
    let text = flatten_content(message.get("content").unwrap_or(&Value::Null));
    let text = strip_think(&text).to_string();
    let stop_reason = map_finish_reason(choice.get("finish_reason").and_then(Value::as_str));
    if text.trim().is_empty() && stop_reason == "end_turn" {
        bail!("The model returned no text.");
    }
    Ok(LlmResponse {
        content: vec![json!({"type": "text", "text": text})],
        stop_reason: stop_reason.to_string(),
        usage: v.get("usage").cloned().unwrap_or(Value::Null),
        stop_details: None,
        incomplete_tool_ids: Vec::new(),
    })
}

/// Short error text from a server error body. Hides the key if the server echoes it.
fn error_message(body: &str, key: Option<&str>) -> String {
    let mut msg = String::new();
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(m) = v["error"]["message"].as_str() {
            msg = m.to_string();
        } else if let Some(m) = v["error"].as_str() {
            msg = m.to_string();
        } else if let Some(m) = v["message"].as_str() {
            msg = m.to_string();
        }
    }
    if msg.is_empty() {
        msg = body.trim().to_string();
    }
    if msg.is_empty() {
        msg = "empty response body".to_string();
    }
    if let Some(k) = key.filter(|k| !k.is_empty()) {
        msg = msg.replace(k, "***");
    }
    if msg.chars().count() > 300 {
        let cut: String = msg.chars().take(300).collect();
        msg = format!("{cut}...");
    }
    msg
}

fn status_hint(status: u16) -> &'static str {
    match status {
        401 | 403 => " Check the API key.",
        404 => " Check the base URL and the model name.",
        _ => "",
    }
}

impl OpenAiClient {
    /// `key_provider` names the credential to read: `OPENAI_KEY_PROVIDER` (required) or
    /// `KEY_PROVIDER` (optional).
    pub fn new(base_url: &str, model: String, key_provider: &'static str) -> Result<Self> {
        let url = chat_url(base_url)?;
        let model = model.trim().to_string();
        if model.is_empty() {
            bail!("No model name set. Enter a model in Settings > Models.");
        }
        Ok(Self {
            http: shared_http()?,
            url,
            model,
            key_provider,
        })
    }

    /// The OpenAI account needs a key. A custom server may run without one.
    fn resolve_key(&self, found: Option<String>) -> Result<Option<String>> {
        if found.is_none() && self.key_provider == OPENAI_KEY_PROVIDER {
            bail!("No OpenAI key set. Add it in Settings.");
        }
        Ok(found)
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    async fn post(&self, key: Option<&str>, body: &Value) -> Result<(u16, String)> {
        let bytes = serde_json::to_vec(body).context("Could not encode the request")?;
        let mut attempt: u32 = 0;
        loop {
            let mut rb = self
                .http
                .post(&self.url)
                .header("content-type", "application/json")
                .body(bytes.clone());
            if let Some(k) = key {
                rb = rb.bearer_auth(k);
            }
            let sent = tokio::time::timeout(REQUEST_TIMEOUT, async {
                let resp = rb.send().await?;
                let status = resp.status().as_u16();
                let text = resp.text().await?;
                Ok::<_, reqwest::Error>((status, text))
            })
            .await;
            match sent {
                Err(_) => bail!("The model server did not answer within {} seconds.", REQUEST_TIMEOUT.as_secs()),
                Ok(Err(e)) => {
                    let e = e.without_url();
                    bail!("Could not reach the model server: {e}");
                }
                Ok(Ok((status, text))) => {
                    if (status == 429 || (500..=599).contains(&status)) && attempt < MAX_RETRIES {
                        tracing::warn!("Model server returned {status}, retrying");
                        tokio::time::sleep(Duration::from_secs(1u64 << attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Ok((status, text));
                }
            }
        }
    }

    pub async fn complete(&self, req: LlmRequest) -> Result<LlmResponse> {
        let key = self.resolve_key(crate::settings::get_api_key(self.key_provider))?;
        self.complete_with_key(key.as_deref(), req).await
    }

    async fn complete_with_key(&self, key: Option<&str>, req: LlmRequest) -> Result<LlmResponse> {
        if key.is_some() && !key_transport_ok(&self.url) {
            bail!("The API key is not sent over plain http to another computer. Use an https address or a local server.");
        }
        let json_mode = req.output_format.is_some();
        let body = build_body(&self.model, &req, json_mode);
        let (mut status, mut text) = self.post(key, &body).await?;
        if status == 400 && json_mode {
            tracing::warn!("Model server rejected response_format, retrying without it");
            let body = build_body(&self.model, &req, false);
            (status, text) = self.post(key, &body).await?;
        }
        if !(200..300).contains(&status) {
            bail!(
                "The model server returned HTTP {status}: {}.{}",
                error_message(&text, key).trim_end_matches('.'),
                status_hint(status)
            );
        }
        let v: Value = serde_json::from_str(&text)
            .map_err(|_| anyhow!("The model server reply was not valid JSON. Check that the base URL ends with /v1."))?;
        parse_response(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Effort;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn req() -> LlmRequest {
        LlmRequest {
            system: vec!["Be brief.".into(), "Use plain words.".into()],
            messages: vec![
                json!({"role": "user", "content": "hello"}),
                json!({"role": "assistant", "content": [{"type": "text", "text": "hi "}, {"type": "text", "text": "there"}]}),
                json!({"role": "user", "content": "again"}),
            ],
            tools: vec![],
            max_tokens: 512,
            effort: Effort::Low,
            output_format: None,
        }
    }

    #[test]
    fn body_puts_system_first_and_sets_sampling() {
        let b = build_body("llama3.2", &req(), false);
        assert_eq!(b["model"], "llama3.2");
        assert_eq!(b["max_tokens"], 512);
        assert_eq!(b["temperature"], 0.2);
        assert_eq!(b["stream"], false);
        assert!(b.get("response_format").is_none());
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m.len(), 4);
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[0]["content"], "Be brief.\n\nUse plain words.");
        assert_eq!(m[1]["content"], "hello");
        assert_eq!(m[2]["role"], "assistant");
        assert_eq!(m[2]["content"], "hi there");
        assert!(b.get("tools").is_none());
        assert!(b.get("output_config").is_none());
    }

    #[test]
    fn output_format_adds_schema_and_json_mode() {
        let mut r = req();
        r.output_format = Some(json!({"type": "json_schema", "schema": {"type": "object", "title": "Notes"}}));
        let b = build_body("m", &r, true);
        assert_eq!(b["response_format"], json!({"type": "json_object"}));
        let sys = b["messages"][0]["content"].as_str().unwrap();
        assert!(sys.contains("JSON"));
        assert!(sys.contains("\"title\":\"Notes\""));

        // Without json mode the instruction stays, the field goes.
        let b = build_body("m", &r, false);
        assert!(b.get("response_format").is_none());
        assert!(b["messages"][0]["content"].as_str().unwrap().contains("JSON"));

        // A bare schema works too.
        r.output_format = Some(json!({"type": "object", "title": "Bare"}));
        let b = build_body("m", &r, true);
        assert!(b["messages"][0]["content"].as_str().unwrap().contains("\"title\":\"Bare\""));
    }

    #[test]
    fn no_system_means_no_system_message() {
        let mut r = req();
        r.system.clear();
        let b = build_body("m", &r, false);
        assert_eq!(b["messages"][0]["role"], "user");
    }

    #[test]
    fn parses_text_and_usage() {
        let v = json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello there"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2}
        });
        let r = parse_response(&v).unwrap();
        assert_eq!(r.text(), "Hello there");
        assert_eq!(r.stop_reason, "end_turn");
        assert_eq!(r.usage["completion_tokens"], 2);
        assert!(r.tool_uses().is_empty());
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(map_finish_reason(Some("length")), "max_tokens");
        assert_eq!(map_finish_reason(Some("stop")), "end_turn");
        assert_eq!(map_finish_reason(Some("content_filter")), "refusal");
        assert_eq!(map_finish_reason(None), "end_turn");
        let v = json!({"choices": [{"message": {"content": "cut off"}, "finish_reason": "length"}]});
        assert_eq!(parse_response(&v).unwrap().stop_reason, "max_tokens");
    }

    #[test]
    fn rejects_empty_or_missing_choices() {
        assert!(parse_response(&json!({"choices": []})).is_err());
        assert!(parse_response(&json!({})).is_err());
        let v = json!({"choices": [{"message": {"content": "  "}, "finish_reason": "stop"}]});
        assert!(parse_response(&v).is_err());
    }

    #[test]
    fn strips_leading_think_block() {
        let v = json!({"choices": [{"message": {"content": "<think>hmm</think>\n\nAnswer"}, "finish_reason": "stop"}]});
        assert_eq!(parse_response(&v).unwrap().text(), "Answer");
        assert_eq!(strip_think("plain"), "plain");
    }

    #[test]
    fn key_transport_rule() {
        assert!(key_transport_ok("https://api.openai.com/v1/chat/completions"));
        assert!(key_transport_ok("http://localhost:11434/v1"));
        assert!(key_transport_ok("http://LOCALHOST:1234/v1"));
        assert!(key_transport_ok("http://127.0.0.1:1234/v1"));
        assert!(key_transport_ok("http://[::1]:1234/v1"));
        assert!(!key_transport_ok("http://192.168.1.20:11434/v1"));
        assert!(!key_transport_ok("http://example.com/v1"));
        assert!(!key_transport_ok("ftp://localhost/v1"));
        assert!(!key_transport_ok("not a url"));
    }

    #[test]
    fn chat_url_building() {
        assert_eq!(chat_url("http://localhost:11434/v1/").unwrap(), "http://localhost:11434/v1/chat/completions");
        assert_eq!(chat_url(" https://api.groq.com/openai/v1 ").unwrap(), "https://api.groq.com/openai/v1/chat/completions");
        assert!(chat_url("").is_err());
        assert!(chat_url("localhost:11434").is_err());
        assert!(chat_url("file:///x").is_err());
    }

    #[test]
    fn new_needs_a_model() {
        assert!(OpenAiClient::new("http://localhost:11434/v1", "  ".into(), KEY_PROVIDER).is_err());
        assert!(OpenAiClient::new("http://localhost:11434/v1", "llama3.2".into(), KEY_PROVIDER).is_ok());
    }

    #[test]
    fn openai_source_requires_a_key_and_server_does_not() {
        let openai = OpenAiClient::new("https://api.openai.com/v1", "gpt-x".into(), OPENAI_KEY_PROVIDER).unwrap();
        let err = openai.resolve_key(None).unwrap_err().to_string();
        assert_eq!(err, "No OpenAI key set. Add it in Settings.");
        assert_eq!(openai.resolve_key(Some("k".into())).unwrap().as_deref(), Some("k"));
        let server = OpenAiClient::new("http://localhost:11434/v1", "m".into(), KEY_PROVIDER).unwrap();
        assert_eq!(server.resolve_key(None).unwrap(), None);
    }

    #[test]
    fn error_text_hides_the_key() {
        let body = r#"{"error":{"message":"Incorrect API key provided: sk-secret-123"}}"#;
        let m = error_message(body, Some("sk-secret-123"));
        assert!(!m.contains("sk-secret-123"));
        assert!(m.contains("***"));
        assert_eq!(error_message("", None), "empty response body");
    }

    #[tokio::test]
    async fn remote_http_with_key_is_refused_before_any_request() {
        let c = OpenAiClient::new("http://example.invalid/v1", "m".into(), KEY_PROVIDER).unwrap();
        let err = c.complete_with_key(Some("sk-secret"), req()).await.unwrap_err().to_string();
        assert!(err.contains("plain http"));
        assert!(!err.contains("sk-secret"));
    }

    /// Serve the given raw HTTP responses, one per connection, and return the request
    /// bodies that were received.
    async fn mock_server(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut bodies = Vec::new();
            for resp in responses {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head_end, content_len) = loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                            .unwrap_or(0);
                        break (pos + 4, len);
                    }
                    assert!(n > 0, "connection closed early");
                };
                while buf.len() < head_end + content_len {
                    let n = sock.read(&mut tmp).await.unwrap();
                    buf.extend_from_slice(&tmp[..n]);
                }
                bodies.push(String::from_utf8_lossy(&buf[head_end..head_end + content_len]).into_owned());
                sock.write_all(resp.as_bytes()).await.unwrap();
                let _ = sock.shutdown().await;
            }
            bodies
        });
        (format!("http://127.0.0.1:{}/v1", addr.port()), handle)
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn retries_once_without_response_format_on_400() {
        let ok = r#"{"choices":[{"message":{"content":"{\"a\":1}"},"finish_reason":"stop"}]}"#;
        let (base, handle) = mock_server(vec![
            http("400 Bad Request", r#"{"error":{"message":"response_format is not supported"}}"#),
            http("200 OK", ok),
        ])
        .await;
        let c = OpenAiClient::new(&base, "m".into(), KEY_PROVIDER).unwrap();
        let mut r = req();
        r.output_format = Some(json!({"type": "object"}));
        let resp = c.complete_with_key(None, r).await.unwrap();
        assert_eq!(resp.text(), "{\"a\":1}");
        let bodies = handle.await.unwrap();
        assert_eq!(bodies.len(), 2);
        let first: Value = serde_json::from_str(&bodies[0]).unwrap();
        let second: Value = serde_json::from_str(&bodies[1]).unwrap();
        assert!(first.get("response_format").is_some());
        assert!(second.get("response_format").is_none());
    }

    #[tokio::test]
    async fn plain_400_without_json_mode_is_an_error() {
        let (base, handle) = mock_server(vec![http("400 Bad Request", r#"{"error":{"message":"model not found"}}"#)]).await;
        let c = OpenAiClient::new(&base, "m".into(), KEY_PROVIDER).unwrap();
        let err = c.complete_with_key(None, req()).await.unwrap_err().to_string();
        assert!(err.contains("HTTP 400"));
        assert!(err.contains("model not found"));
        assert_eq!(handle.await.unwrap().len(), 1);
    }

    /// Needs a server at localhost:11434 with the model `RELAY_TEST_MODEL` (default llama3.2).
    #[tokio::test]
    #[ignore]
    async fn live_ollama() {
        let model = std::env::var("RELAY_TEST_MODEL").unwrap_or_else(|_| "llama3.2".into());
        let c = OpenAiClient::new(DEFAULT_BASE_URL, model, KEY_PROVIDER).unwrap();
        let r = c.complete_with_key(None, req()).await.unwrap();
        assert!(!r.text().trim().is_empty());
    }
}
