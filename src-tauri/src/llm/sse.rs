//! Server-sent event parsing and reassembly of streamed Messages API responses.
//!
//! `SseParser` turns raw bytes into events. It buffers bytes, not text, so a
//! multi-byte character split across two network chunks is decoded correctly.
//! `StreamAssembler` consumes the JSON of each event and rebuilds the full
//! content blocks, so the caller can append `content` to the conversation
//! unchanged (thinking blocks keep their signature, unknown block types are
//! kept as raw JSON).

use super::LlmResponse;
use anyhow::{anyhow, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add bytes and return every event that is now complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some((end, sep_len)) = find_boundary(&self.buf) {
            let rest = self.buf.split_off(end + sep_len);
            let mut block = std::mem::replace(&mut self.buf, rest);
            block.truncate(end);
            if let Some(ev) = parse_block(&block) {
                out.push(ev);
            }
        }
        out
    }

    /// Parse whatever is left when the connection closes without a final blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if self.buf.is_empty() {
            return None;
        }
        let block = std::mem::take(&mut self.buf);
        parse_block(&block)
    }
}

/// Returns (index of the boundary, length of the separator).
fn find_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let len = buf.len();
    let mut i = 0;
    while i < len {
        if buf[i] == b'\n' && i + 1 < len && buf[i + 1] == b'\n' {
            return Some((i, 2));
        }
        if buf[i] == b'\r' && buf[i..].starts_with(b"\r\n\r\n") {
            return Some((i, 4));
        }
        i += 1;
    }
    None
}

fn parse_block(block: &[u8]) -> Option<SseEvent> {
    let text = String::from_utf8_lossy(block);
    let mut event: Option<String> = None;
    let mut data: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with(':') {
            continue;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => event = Some(value.to_string()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(SseEvent {
        event,
        data: data.join("\n"),
    })
}

/// An `error` event sent inside the stream.
#[derive(Debug, thiserror::Error)]
#[error("Claude API error ({kind}): {message}")]
pub struct ApiStreamError {
    pub kind: String,
    pub message: String,
}

impl ApiStreamError {
    /// Errors that are worth a retry when nothing has been delivered yet.
    pub fn is_transient(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "overloaded_error" | "api_error" | "rate_limit_error"
        )
    }
}

struct Slot {
    block: Value,
    partial: String,
    finalized: bool,
}

#[derive(Default)]
pub struct StreamAssembler {
    slots: BTreeMap<usize, Slot>,
    stop_reason: Option<String>,
    stop_details: Option<Value>,
    usage: Map<String, Value>,
    stopped: bool,
    incomplete: Vec<String>,
}

fn append_str(block: &mut Value, key: &str, add: &str) {
    if let Some(obj) = block.as_object_mut() {
        let entry = obj
            .entry(key.to_string())
            .or_insert_with(|| Value::String(String::new()));
        if let Value::String(s) = entry {
            s.push_str(add);
        }
    }
}

fn merge_usage(into: &mut Map<String, Value>, from: &Value) {
    if let Some(obj) = from.as_object() {
        for (k, v) in obj {
            into.insert(k.clone(), v.clone());
        }
    }
}

fn finalize_slot(slot: &mut Slot, incomplete: &mut Vec<String>) {
    if slot.finalized {
        return;
    }
    slot.finalized = true;
    let kind = slot
        .block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if kind != "tool_use" && kind != "server_tool_use" && slot.partial.is_empty() {
        return;
    }
    let parsed: Option<Value> = if slot.partial.trim().is_empty() {
        Some(json!({}))
    } else {
        serde_json::from_str::<Value>(&slot.partial)
            .ok()
            .filter(|v| v.is_object())
    };
    match parsed {
        Some(v) => {
            slot.block["input"] = v;
        }
        None => {
            let id = slot
                .block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            incomplete.push(id);
            slot.block["input"] = json!({});
        }
    }
}

impl StreamAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// True once any content block has started. Used to decide whether a failed
    /// stream can be retried without repeating text the caller already saw.
    pub fn has_content(&self) -> bool {
        !self.slots.is_empty()
    }

    /// Apply one event. `data` is the JSON text of the event.
    pub fn handle(&mut self, data: &str, on_text: &mut (dyn FnMut(&str) + Send)) -> Result<()> {
        let ev: Value = serde_json::from_str(data)
            .map_err(|e| anyhow!("Could not parse a stream event: {e}"))?;
        match ev["type"].as_str().unwrap_or("") {
            "message_start" => {
                merge_usage(&mut self.usage, &ev["message"]["usage"]);
            }
            "content_block_start" => {
                let idx = block_index(&ev)?;
                let block = ev["content_block"].clone();
                if !block.is_object() {
                    return Err(anyhow!("A content_block_start event had no content block."));
                }
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(t) = block.get("text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            on_text(t);
                        }
                    }
                }
                self.slots.insert(
                    idx,
                    Slot {
                        block,
                        partial: String::new(),
                        finalized: false,
                    },
                );
            }
            "content_block_delta" => {
                let idx = block_index(&ev)?;
                let slot = self
                    .slots
                    .get_mut(&idx)
                    .ok_or_else(|| anyhow!("A delta arrived for unknown block index {idx}."))?;
                let delta = &ev["delta"];
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let t = delta["text"].as_str().unwrap_or("");
                        append_str(&mut slot.block, "text", t);
                        if !t.is_empty() {
                            on_text(t);
                        }
                    }
                    "thinking_delta" => {
                        append_str(
                            &mut slot.block,
                            "thinking",
                            delta["thinking"].as_str().unwrap_or(""),
                        );
                    }
                    "signature_delta" => {
                        append_str(
                            &mut slot.block,
                            "signature",
                            delta["signature"].as_str().unwrap_or(""),
                        );
                    }
                    "input_json_delta" => {
                        slot.partial
                            .push_str(delta["partial_json"].as_str().unwrap_or(""));
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let idx = block_index(&ev)?;
                if let Some(slot) = self.slots.get_mut(&idx) {
                    finalize_slot(slot, &mut self.incomplete);
                }
            }
            "message_delta" => {
                if let Some(r) = ev["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(r.to_string());
                }
                let details = &ev["delta"]["stop_details"];
                if !details.is_null() {
                    self.stop_details = Some(details.clone());
                }
                merge_usage(&mut self.usage, &ev["usage"]);
            }
            "message_stop" => {
                self.stopped = true;
            }
            "ping" => {}
            "error" => {
                let kind = ev["error"]["type"].as_str().unwrap_or("unknown").to_string();
                let message = ev["error"]["message"]
                    .as_str()
                    .unwrap_or("no message")
                    .to_string();
                return Err(ApiStreamError { kind, message }.into());
            }
            _ => {}
        }
        Ok(())
    }

    /// Build the final response. Fails when the stream ended early.
    pub fn finish(mut self) -> Result<LlmResponse> {
        if !self.stopped && self.stop_reason.is_none() {
            return Err(anyhow!(
                "The response stream ended before the message was complete."
            ));
        }
        for slot in self.slots.values_mut() {
            finalize_slot(slot, &mut self.incomplete);
        }
        let stop_reason = self
            .stop_reason
            .ok_or_else(|| anyhow!("The response did not include a stop_reason."))?;
        let content: Vec<Value> = self.slots.into_values().map(|s| s.block).collect();
        Ok(LlmResponse {
            content,
            stop_reason,
            usage: Value::Object(self.usage),
            stop_details: self.stop_details,
            incomplete_tool_ids: self.incomplete,
        })
    }
}

fn block_index(ev: &Value) -> Result<usize> {
    ev["index"]
        .as_u64()
        .map(|i| i as usize)
        .ok_or_else(|| anyhow!("A stream event had no block index."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(name: &str, data: &str) -> String {
        format!("event: {name}\ndata: {data}\n\n")
    }

    fn recorded_stream() -> Vec<u8> {
        let mut s = String::new();
        s.push_str(&ev(
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-opus-5-5","stop_reason":null,"usage":{"input_tokens":25,"output_tokens":1}}}"#,
        ));
        s.push_str(&ev(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
        ));
        s.push_str(&ev("ping", r#"{"type":"ping"}"#));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think"}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" about it."}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"EqQBCk"}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"YXY="}}"#,
        ));
        s.push_str(&ev(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ));
        s.push_str(&ev(
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Caf"}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"é open"}}"#,
        ));
        s.push_str(&ev(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":1}"#,
        ));
        s.push_str(&ev(
            "content_block_start",
            r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"th\":\"C:/a.txt\""}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"}"}}"#,
        ));
        s.push_str(&ev(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":2}"#,
        ));
        // A second tool call that is cut off mid-JSON.
        s.push_str(&ev(
            "content_block_start",
            r#"{"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"toolu_2","name":"read_file","input":{}}}"#,
        ));
        s.push_str(&ev(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"C:/b"}}"#,
        ));
        s.push_str(&ev(
            "content_block_stop",
            r#"{"type":"content_block_stop","index":3}"#,
        ));
        s.push_str(&ev(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":42}}"#,
        ));
        s.push_str(&ev("message_stop", r#"{"type":"message_stop"}"#));
        s.into_bytes()
    }

    fn assemble(chunk_size: usize) -> (LlmResponse, String) {
        let bytes = recorded_stream();
        let mut parser = SseParser::new();
        let mut asm = StreamAssembler::new();
        let mut streamed = String::new();
        for chunk in bytes.chunks(chunk_size) {
            for e in parser.push(chunk) {
                asm.handle(&e.data, &mut |t: &str| streamed.push_str(t))
                    .expect("event should apply");
            }
        }
        assert!(parser.finish().is_none());
        (asm.finish().expect("stream should finish"), streamed)
    }

    #[test]
    fn reassembles_at_awkward_boundaries() {
        for size in [1usize, 2, 3, 5, 7, 13, 64, 1000] {
            let (resp, streamed) = assemble(size);
            assert_eq!(resp.stop_reason, "tool_use", "chunk size {size}");
            assert_eq!(resp.content.len(), 4, "chunk size {size}");
            assert_eq!(resp.content[0]["type"], "thinking");
            assert_eq!(resp.content[0]["thinking"], "Let me think about it.");
            assert_eq!(resp.content[0]["signature"], "EqQBCkYXY=");
            assert_eq!(resp.content[1]["text"], "Café open");
            assert_eq!(streamed, "Café open");
            assert_eq!(resp.text(), "Café open");
            assert_eq!(resp.content[2]["input"], json!({"path": "C:/a.txt"}));
            assert_eq!(resp.incomplete_tool_ids, vec!["toolu_2".to_string()]);
            assert_eq!(resp.usage["input_tokens"], 25);
            assert_eq!(resp.usage["output_tokens"], 42);
            let uses = resp.tool_uses();
            assert_eq!(uses.len(), 2);
            assert_eq!(uses[0].0, "toolu_1");
            assert_eq!(uses[0].1, "read_file");
        }
    }

    #[test]
    fn handles_crlf_and_comments() {
        let mut p = SseParser::new();
        let evs = p.push(b": keepalive\r\nevent: ping\r\ndata: {\"type\":\"ping\"}\r\n\r\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("ping"));
        assert_eq!(evs[0].data, "{\"type\":\"ping\"}");
    }

    #[test]
    fn error_event_returns_err() {
        let mut asm = StreamAssembler::new();
        let err = asm
            .handle(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                &mut |_: &str| {},
            )
            .unwrap_err();
        let api = err.downcast_ref::<ApiStreamError>().expect("typed error");
        assert!(api.is_transient());
        assert_eq!(api.message, "Overloaded");
    }

    #[test]
    fn truncated_stream_is_an_error() {
        let mut asm = StreamAssembler::new();
        asm.handle(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            &mut |_: &str| {},
        )
        .unwrap();
        assert!(asm.finish().is_err());
    }

    #[test]
    fn unknown_block_types_are_kept() {
        let mut asm = StreamAssembler::new();
        let mut sink = |_: &str| {};
        asm.handle(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"fallback","from":"a","to":"b"}}"#,
            &mut sink,
        )
        .unwrap();
        asm.handle(r#"{"type":"content_block_stop","index":0}"#, &mut sink)
            .unwrap();
        asm.handle(
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
            &mut sink,
        )
        .unwrap();
        asm.handle(r#"{"type":"message_stop"}"#, &mut sink).unwrap();
        let resp = asm.finish().unwrap();
        assert_eq!(resp.content[0], json!({"type":"fallback","from":"a","to":"b"}));
    }
}
