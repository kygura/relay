//! Tool definitions, permission levels, the permission gate and the tools that
//! only need the OS layer. Tools that need runtime state (create_task,
//! add_task_note, list_tasks, get_transcript) are run in `runtime.rs`.

use crate::types::{InsertMethod, PermissionLevel, TranscriptSegment};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const TOOL_ASK_USER: &str = "ask_user";
pub const TOOL_COMPLETE: &str = "complete_task";
pub const TOOL_FAIL: &str = "fail_task";

pub const READ_FILE_CAP: usize = 64 * 1024;
pub const COMMAND_OUTPUT_CAP: usize = 8 * 1024;
pub const COMMAND_TIMEOUT_SECS: u64 = 60;
const LIST_DIR_CAP: usize = 500;

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub level: PermissionLevel,
    pub properties: Value,
    pub required: Vec<&'static str>,
}

impl ToolSpec {
    /// The tool definition sent to the API.
    pub fn to_api(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "input_schema": {
                "type": "object",
                "properties": self.properties,
                "required": self.required,
                "additionalProperties": false
            },
            "strict": true
        })
    }
}

fn string_prop(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn int_prop(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}

fn tool(
    name: &'static str,
    description: &'static str,
    level: PermissionLevel,
    props: Vec<(&'static str, Value)>,
) -> ToolSpec {
    let required: Vec<&'static str> = props.iter().map(|(k, _)| *k).collect();
    let mut map = serde_json::Map::new();
    for (k, v) in props {
        map.insert(k.to_string(), v);
    }
    ToolSpec {
        name,
        description,
        level,
        properties: Value::Object(map),
        required,
    }
}

pub fn all_tools() -> Vec<ToolSpec> {
    use PermissionLevel::*;
    vec![
        // Read only
        tool(
            "get_active_window",
            "Return the title, process name and handle of the window that has focus.",
            ReadOnly,
            vec![],
        ),
        tool(
            "read_clipboard",
            "Return the current clipboard text.",
            ReadOnly,
            vec![],
        ),
        tool(
            "take_screenshot",
            "Capture the primary monitor and return it as an image. Coordinates for mouse_click refer to this image.",
            ReadOnly,
            vec![],
        ),
        tool(
            "read_file",
            "Read a text file. Returns at most 64 KB.",
            ReadOnly,
            vec![("path", string_prop("Absolute file path."))],
        ),
        tool(
            "list_dir",
            "List the entries of a folder. Folders end with a slash.",
            ReadOnly,
            vec![("path", string_prop("Absolute folder path."))],
        ),
        tool(
            "get_transcript",
            "Return the last lines of the live transcript as [mm:ss] Speaker: text.",
            ReadOnly,
            vec![("last_n", int_prop("How many recent segments to return, 1 to 200."))],
        ),
        tool(
            "list_tasks",
            "List recent agent tasks with their status.",
            ReadOnly,
            vec![],
        ),
        // Local reversible
        tool(
            "write_clipboard",
            "Replace the clipboard text.",
            LocalReversible,
            vec![("text", string_prop("Text to place on the clipboard."))],
        ),
        tool(
            "create_task",
            "Create a separate agent task that runs on its own.",
            LocalReversible,
            vec![
                ("title", string_prop("Short task title.")),
                ("detail", string_prop("What the new task should do.")),
            ],
        ),
        tool(
            "add_task_note",
            "Add a note to this task's log.",
            LocalReversible,
            vec![("text", string_prop("Note text."))],
        ),
        tool(
            "write_file",
            "Write a text file, replacing any existing file. Inside Documents/Relay it is local reversible. Anywhere else it is destructive and needs approval.",
            LocalReversible,
            vec![
                ("path", string_prop("Absolute file path.")),
                ("content", string_prop("Full file content.")),
            ],
        ),
        tool(
            "open_url",
            "Open an http or https URL in the default browser.",
            LocalReversible,
            vec![("url", string_prop("The URL to open."))],
        ),
        // External
        tool(
            "type_text",
            "Type text into the focused window with simulated keystrokes.",
            External,
            vec![("text", string_prop("Text to type."))],
        ),
        tool(
            "press_keys",
            "Press a key combination such as ctrl+shift+t, enter or alt+tab.",
            External,
            vec![("combo", string_prop("Key combination joined with plus signs."))],
        ),
        tool(
            "mouse_click",
            "Click at a position in the most recent screenshot.",
            External,
            vec![
                ("x", int_prop("X in screenshot pixels.")),
                ("y", int_prop("Y in screenshot pixels.")),
                (
                    "button",
                    json!({"type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button."}),
                ),
            ],
        ),
        tool(
            "insert_text_in_active_app",
            "Insert text at the cursor of the focused app by pasting it.",
            External,
            vec![("text", string_prop("Text to insert."))],
        ),
        tool(
            "run_command",
            "Run a PowerShell command. Times out after 60 seconds. Output is cut to 8 KB.",
            External,
            vec![("cmd", string_prop("PowerShell command line."))],
        ),
        // Destructive
        tool(
            "delete_file",
            "Delete one file. Always needs approval.",
            Destructive,
            vec![("path", string_prop("Absolute file path."))],
        ),
        // Control
        tool(
            TOOL_ASK_USER,
            "Ask the user a question and wait for the reply. Use it before anything involving money, passwords, credentials, messages sent for the user, public posts or deleting data.",
            ReadOnly,
            vec![("question", string_prop("The question, with the exact action you plan to take."))],
        ),
        tool(
            TOOL_COMPLETE,
            "End the task as done.",
            ReadOnly,
            vec![("summary", string_prop("What was done and anything the user should know."))],
        ),
        tool(
            TOOL_FAIL,
            "End the task as failed.",
            ReadOnly,
            vec![("reason", string_prop("Why the task could not be done."))],
        ),
    ]
}

pub fn find(name: &str) -> Option<ToolSpec> {
    all_tools().into_iter().find(|t| t.name == name)
}

pub fn api_definitions() -> Vec<Value> {
    all_tools().iter().map(ToolSpec::to_api).collect()
}

// ---------------------------------------------------------------------------
// Permission gate

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    Run,
    Ask,
}

/// Decide whether a tool call runs now or needs the user's approval.
/// Destructive calls always ask. A remembered tool name skips the question
/// for the rest of the task.
pub fn gate_decision(level: PermissionLevel, auto_level: PermissionLevel, remembered: bool) -> GateDecision {
    if level == PermissionLevel::Destructive {
        return GateDecision::Ask;
    }
    if remembered || level <= auto_level {
        GateDecision::Run
    } else {
        GateDecision::Ask
    }
}

/// Level for one call. write_file depends on the path.
pub fn effective_level(name: &str, input: &Value) -> PermissionLevel {
    if name == "write_file" {
        let path = input.get("path").and_then(Value::as_str).unwrap_or("");
        return if path_in_relay_folder(path) {
            PermissionLevel::LocalReversible
        } else {
            PermissionLevel::Destructive
        };
    }
    find(name)
        .map(|t| t.level)
        .unwrap_or(PermissionLevel::Destructive)
}

pub fn relay_folder() -> Option<PathBuf> {
    dirs::document_dir().map(|d| d.join("Relay"))
}

/// Canonicalize a path that may not exist yet by resolving its nearest
/// existing ancestor. Paths that need ".." in the missing part are rejected.
fn canonicalize_lenient(p: &Path) -> Option<PathBuf> {
    if let Ok(c) = p.canonicalize() {
        return Some(c);
    }
    let name = p.file_name()?;
    let parent = p.parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    Some(canonicalize_lenient(parent)?.join(name))
}

/// True when `target` is strictly inside `base`. Case-insensitive, as on Windows.
pub fn is_inside(base: &Path, target: &Path) -> bool {
    let b = base.to_string_lossy().to_lowercase();
    let t = target.to_string_lossy().to_lowercase();
    let (b, t) = (PathBuf::from(b), PathBuf::from(t));
    t != b && t.starts_with(&b)
}

/// True when the absolute `path` resolves to a location inside Documents/Relay.
pub fn path_in_relay_folder(path: &str) -> bool {
    let p = Path::new(path);
    if path.trim().is_empty() || !p.is_absolute() {
        return false;
    }
    let Some(base) = relay_folder().and_then(|b| canonicalize_lenient(&b)) else {
        return false;
    };
    let Some(target) = canonicalize_lenient(p) else {
        return false;
    };
    is_inside(&base, &target)
}

// ---------------------------------------------------------------------------
// Text helpers

pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push_str("...");
        out
    }
}

/// Cut to at most `max` bytes on a character boundary.
pub fn truncate_bytes(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// "[mm:ss] Speaker: text" for the last `last_n` segments.
pub fn format_transcript(segments: &[TranscriptSegment], last_n: usize) -> String {
    if segments.is_empty() {
        return "The transcript is empty.".to_string();
    }
    let n = last_n.clamp(1, 200);
    let start = segments.len().saturating_sub(n);
    segments[start..]
        .iter()
        .map(|s| {
            let secs = s.start_ms / 1000;
            let speaker = match (&s.speaker, s.source) {
                (Some(name), _) if !name.trim().is_empty() => name.clone(),
                (_, crate::types::AudioSource::Microphone) => "Microphone".to_string(),
                (_, crate::types::AudioSource::System) => "System".to_string(),
            };
            format!("[{:02}:{:02}] {}: {}", secs / 60, secs % 60, speaker, s.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Tool output

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub text: String,
    pub image_b64: Option<String>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            image_b64: None,
            is_error: false,
        }
    }

    pub fn error(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            image_b64: None,
            is_error: true,
        }
    }

    /// The `tool_result` block for the next user message.
    pub fn to_result_block(&self, tool_use_id: &str) -> Value {
        let content = match &self.image_b64 {
            Some(b64) => json!([
                {"type": "text", "text": self.text},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": b64}}
            ]),
            None => Value::String(self.text.clone()),
        };
        json!({
            "type": "tool_result",
            "tool_use_id": tool_use_id,
            "content": content,
            "is_error": self.is_error
        })
    }
}

pub fn error_result_block(tool_use_id: &str, message: &str) -> Value {
    ToolOutput::error(message).to_result_block(tool_use_id)
}

// ---------------------------------------------------------------------------
// OS and file tools

fn req_str<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing text argument \"{key}\"."))
}

fn req_int(input: &Value, key: &str) -> Result<i64> {
    input
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("Missing integer argument \"{key}\"."))
}

async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| anyhow!("A background task failed: {e}"))?
}

/// Run a tool that needs only the OS layer or the file system.
pub async fn run_os_tool(name: &str, input: &Value, insert_method: InsertMethod) -> Result<ToolOutput> {
    match name {
        "get_active_window" => {
            let w = blocking(crate::os::active_window).await?;
            Ok(ToolOutput::text(serde_json::to_string(&w)?))
        }
        "read_clipboard" => {
            let t = blocking(crate::os::clipboard_get).await?;
            Ok(ToolOutput::text(if t.is_empty() {
                "The clipboard is empty.".to_string()
            } else {
                t
            }))
        }
        "take_screenshot" => {
            let shot = blocking(crate::os::screenshot_png).await?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&shot.png);
            Ok(ToolOutput {
                text: format!(
                    "Screenshot {}x{} pixels. Use positions in this image for mouse_click.",
                    shot.width, shot.height
                ),
                image_b64: Some(b64),
                is_error: false,
            })
        }
        "read_file" => {
            let path = req_str(input, "path")?.to_string();
            blocking(move || {
                use std::io::Read;
                let f = std::fs::File::open(&path)
                    .with_context(|| format!("Could not open {path}"))?;
                let mut buf = Vec::new();
                f.take(READ_FILE_CAP as u64 + 1)
                    .read_to_end(&mut buf)
                    .with_context(|| format!("Could not read {path}"))?;
                let truncated = buf.len() > READ_FILE_CAP;
                buf.truncate(READ_FILE_CAP);
                let mut text = String::from_utf8_lossy(&buf).into_owned();
                if truncated {
                    text.push_str("\n[truncated at 64 KB]");
                }
                Ok(ToolOutput::text(text))
            })
            .await
        }
        "list_dir" => {
            let path = req_str(input, "path")?.to_string();
            blocking(move || {
                let mut names: Vec<String> = Vec::new();
                for entry in std::fs::read_dir(&path)
                    .with_context(|| format!("Could not list {path}"))?
                {
                    let entry = entry?;
                    let mut name = entry.file_name().to_string_lossy().into_owned();
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        name.push('/');
                    }
                    names.push(name);
                }
                names.sort_by_key(|n| n.to_lowercase());
                let total = names.len();
                names.truncate(LIST_DIR_CAP);
                let mut text = names.join("\n");
                if total > LIST_DIR_CAP {
                    text.push_str(&format!("\n[{} more entries not shown]", total - LIST_DIR_CAP));
                }
                if text.is_empty() {
                    text = "The folder is empty.".to_string();
                }
                Ok(ToolOutput::text(text))
            })
            .await
        }
        "write_clipboard" => {
            let text = req_str(input, "text")?.to_string();
            let n = text.chars().count();
            blocking(move || crate::os::clipboard_set(&text)).await?;
            Ok(ToolOutput::text(format!("Clipboard set ({n} characters).")))
        }
        "write_file" => {
            let path = req_str(input, "path")?.to_string();
            let content = req_str(input, "content")?.to_string();
            blocking(move || {
                if let Some(parent) = Path::new(&path).parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)
                            .with_context(|| format!("Could not create the folder for {path}"))?;
                    }
                }
                std::fs::write(&path, content.as_bytes())
                    .with_context(|| format!("Could not write {path}"))?;
                Ok(ToolOutput::text(format!("Wrote {} bytes to {path}.", content.len())))
            })
            .await
        }
        "open_url" => {
            let url = req_str(input, "url")?.to_string();
            let lower = url.trim().to_lowercase();
            if !(lower.starts_with("http://") || lower.starts_with("https://")) {
                bail!("Only http and https URLs can be opened.");
            }
            blocking(move || crate::os::open_url(url.trim())).await?;
            Ok(ToolOutput::text("Opened the URL in the default browser."))
        }
        "type_text" => {
            let text = req_str(input, "text")?.to_string();
            let n = text.chars().count();
            blocking(move || crate::os::type_text(&text)).await?;
            Ok(ToolOutput::text(format!("Typed {n} characters.")))
        }
        "press_keys" => {
            let combo = req_str(input, "combo")?.to_string();
            let shown = combo.clone();
            blocking(move || crate::os::press_keys(&combo)).await?;
            Ok(ToolOutput::text(format!("Pressed {shown}.")))
        }
        "mouse_click" => {
            let x = i32::try_from(req_int(input, "x")?).map_err(|_| anyhow!("x is out of range."))?;
            let y = i32::try_from(req_int(input, "y")?).map_err(|_| anyhow!("y is out of range."))?;
            let button = req_str(input, "button")?.to_string();
            if !matches!(button.as_str(), "left" | "right" | "middle") {
                bail!("button must be left, right or middle.");
            }
            let shown = button.clone();
            blocking(move || crate::os::mouse_click(x, y, &button)).await?;
            Ok(ToolOutput::text(format!("Clicked {shown} at {x}, {y}.")))
        }
        "insert_text_in_active_app" => {
            let text = req_str(input, "text")?.to_string();
            let n = text.chars().count();
            blocking(move || crate::os::insert_text(&text, insert_method)).await?;
            Ok(ToolOutput::text(format!("Inserted {n} characters.")))
        }
        "run_command" => {
            let cmd = req_str(input, "cmd")?.to_string();
            let out = blocking(move || crate::os::run_powershell(&cmd, COMMAND_TIMEOUT_SECS)).await?;
            Ok(format_command_output(&out))
        }
        "delete_file" => {
            let path = req_str(input, "path")?.to_string();
            blocking(move || {
                let meta = std::fs::metadata(&path).with_context(|| format!("Could not find {path}"))?;
                if !meta.is_file() {
                    bail!("delete_file only removes single files, and {path} is not a file.");
                }
                std::fs::remove_file(&path).with_context(|| format!("Could not delete {path}"))?;
                Ok(ToolOutput::text(format!("Deleted {path}.")))
            })
            .await
        }
        other => bail!("Unknown tool: {other}"),
    }
}

pub fn format_command_output(out: &crate::types::CommandOutput) -> ToolOutput {
    let mut text = String::new();
    if out.timed_out {
        text.push_str(&format!("The command timed out after {COMMAND_TIMEOUT_SECS} seconds and was stopped.\n"));
    }
    match out.exit_code {
        Some(code) => text.push_str(&format!("exit code: {code}\n")),
        None => text.push_str("exit code: none\n"),
    }
    if !out.stdout.trim().is_empty() {
        text.push_str("stdout:\n");
        text.push_str(out.stdout.trim_end());
        text.push('\n');
    }
    if !out.stderr.trim().is_empty() {
        text.push_str("stderr:\n");
        text.push_str(out.stderr.trim_end());
        text.push('\n');
    }
    let (mut text, cut) = truncate_bytes(&text, COMMAND_OUTPUT_CAP);
    if cut {
        text.push_str("\n[output truncated at 8 KB]");
    }
    let failed = out.timed_out || out.exit_code.map(|c| c != 0).unwrap_or(true);
    ToolOutput {
        text,
        image_b64: None,
        is_error: failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AudioSource, CommandOutput};
    use PermissionLevel::*;

    #[test]
    fn gate_decision_table() {
        // (level, auto_level, remembered, expected)
        let cases = [
            (ReadOnly, ReadOnly, false, GateDecision::Run),
            (LocalReversible, ReadOnly, false, GateDecision::Ask),
            (LocalReversible, LocalReversible, false, GateDecision::Run),
            (External, LocalReversible, false, GateDecision::Ask),
            (External, LocalReversible, true, GateDecision::Run),
            (External, External, false, GateDecision::Run),
            (Destructive, External, false, GateDecision::Ask),
            (Destructive, Destructive, false, GateDecision::Ask),
            (Destructive, Destructive, true, GateDecision::Ask),
        ];
        for (level, auto, remembered, want) in cases {
            assert_eq!(
                gate_decision(level, auto, remembered),
                want,
                "level {level:?}, auto {auto:?}, remembered {remembered}"
            );
        }
    }

    #[test]
    fn every_tool_schema_is_strict() {
        let defs = api_definitions();
        assert!(defs.len() >= 20);
        for d in defs {
            assert_eq!(d["strict"], true);
            assert_eq!(d["input_schema"]["additionalProperties"], false);
            let props = d["input_schema"]["properties"].as_object().unwrap();
            let required = d["input_schema"]["required"].as_array().unwrap();
            assert_eq!(props.len(), required.len(), "{}", d["name"]);
        }
    }

    #[test]
    fn tool_levels_match_the_spec() {
        assert_eq!(find("read_file").unwrap().level, ReadOnly);
        assert_eq!(find("open_url").unwrap().level, LocalReversible);
        assert_eq!(find("run_command").unwrap().level, External);
        assert_eq!(find("delete_file").unwrap().level, Destructive);
        assert!(find("no_such_tool").is_none());
    }

    #[test]
    fn write_file_outside_relay_is_destructive() {
        let outside = std::env::temp_dir().join("relay-outside-test.txt");
        let input = json!({"path": outside.to_string_lossy(), "content": "x"});
        assert_eq!(effective_level("write_file", &input), Destructive);
        assert_eq!(effective_level("write_file", &json!({"path": "relative.txt", "content": ""})), Destructive);
        assert_eq!(effective_level("read_file", &json!({"path": "C:/x"})), ReadOnly);
    }

    #[test]
    fn inside_check_is_strict_and_case_insensitive() {
        let base = Path::new("C:\\Users\\a\\Documents\\Relay");
        assert!(is_inside(base, Path::new("C:\\Users\\a\\Documents\\Relay\\notes.txt")));
        assert!(is_inside(base, Path::new("c:\\users\\A\\documents\\relay\\sub\\x.md")));
        assert!(!is_inside(base, base));
        assert!(!is_inside(base, Path::new("C:\\Users\\a\\Documents\\Relay2\\x.txt")));
        assert!(!is_inside(base, Path::new("C:\\Users\\a\\Documents\\x.txt")));
    }

    #[test]
    fn transcript_lines_are_formatted() {
        let seg = |ms: u64, speaker: Option<&str>, source: AudioSource, text: &str| TranscriptSegment {
            id: "i".into(),
            source,
            speaker: speaker.map(String::from),
            text: text.into(),
            start_ms: ms,
            end_ms: ms + 1000,
            is_final: true,
            language: None,
        };
        let segs = vec![
            seg(5_000, None, AudioSource::Microphone, "first"),
            seg(65_000, Some("Ana"), AudioSource::System, "second"),
            seg(3_725_000, None, AudioSource::System, "third"),
        ];
        let all = format_transcript(&segs, 10);
        assert_eq!(
            all,
            "[00:05] Microphone: first\n[01:05] Ana: second\n[62:05] System: third"
        );
        let last = format_transcript(&segs, 1);
        assert_eq!(last, "[62:05] System: third");
        assert_eq!(format_transcript(&[], 5), "The transcript is empty.");
    }

    #[test]
    fn tool_result_block_shapes() {
        let text = ToolOutput::text("ok").to_result_block("t1");
        assert_eq!(text["type"], "tool_result");
        assert_eq!(text["tool_use_id"], "t1");
        assert_eq!(text["content"], "ok");
        assert_eq!(text["is_error"], false);

        let img = ToolOutput {
            text: "shot".into(),
            image_b64: Some("AAAA".into()),
            is_error: false,
        }
        .to_result_block("t2");
        assert_eq!(img["content"][1]["type"], "image");
        assert_eq!(img["content"][1]["source"]["media_type"], "image/png");
    }

    #[test]
    fn command_output_is_truncated() {
        let out = CommandOutput {
            stdout: "x".repeat(20_000),
            stderr: String::new(),
            exit_code: Some(0),
            timed_out: false,
        };
        let r = format_command_output(&out);
        assert!(r.text.len() < COMMAND_OUTPUT_CAP + 100);
        assert!(r.text.contains("truncated"));
        assert!(!r.is_error);
    }
}
