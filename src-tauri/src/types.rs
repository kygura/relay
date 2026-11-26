//! Shared types. FROZEN: owned by the planner. Other modules import from here.
//! All types serialize with camelCase field names for the TypeScript frontend.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioSource {
    Microphone,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceInfo {
    /// Stable identifier: the device name as cpal reports it.
    pub id: String,
    pub name: String,
    /// Microphone = input device, System = output device usable for loopback.
    pub kind: AudioSource,
    pub is_default: bool,
}

/// One utterance of 16 kHz mono f32 audio, segmented by the VAD.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub source: AudioSource,
    pub samples: Vec<f32>,
    /// Milliseconds since the session started.
    pub start_ms: u64,
    pub end_ms: u64,
}

pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub id: String,
    pub source: AudioSource,
    pub speaker: Option<String>,
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub is_final: bool,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationSegment {
    pub segment_id: String,
    /// BCP 47 tag or plain language name as configured, for example "es" or "Japanese".
    pub target_lang: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptState {
    /// Final segments in time order. Interim segments are only sent as events.
    pub segments: Vec<TranscriptSegment>,
    pub translations: Vec<TranslationSegment>,
}

pub type SharedTranscript = Arc<RwLock<TranscriptState>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Idle,
    /// Microphone only. On stop: polish and insert into the focused app.
    Dictation,
    /// Live captions only, no language model calls. Runs fully offline with local Whisper.
    Transcribe,
    /// Live captions with translation for mic and/or system audio.
    Translate,
    /// Transcript + translation + rolling notes + action item extraction.
    Meeting,
    /// Voice commands become agent tasks.
    Agent,
}

/// Where a session or dictation is in its processing. Dictation walks
/// listening, transcribing, cleaning, then done or error. Other modes only use
/// listening (while recording) and idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    #[default]
    Idle,
    Listening,
    Transcribing,
    Cleaning,
    Done,
    Error,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub mode: Mode,
    pub recording: bool,
    pub session_started_at: Option<String>,
    pub pending_approvals: usize,
    pub active_tasks: usize,
    pub last_error: Option<String>,
    pub phase: Phase,
    /// RFC 3339 time the current phase began.
    pub phase_since: Option<String>,
    /// Done: target app name without ".exe". Error: one-line reason.
    /// Cleaning: "ai" or "basic".
    pub phase_detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionLevel {
    /// Reads state only. Never changes anything.
    ReadOnly = 0,
    /// Local changes that are easy to undo (clipboard, Relay folder files, opening a URL).
    LocalReversible = 1,
    /// Acts on other apps or the shell: typing, clicking, keystrokes, commands.
    External = 2,
    /// Deletes data or writes outside the Relay folder. Always asks.
    Destructive = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    /// Backlog entry. Never runs on its own: the user starts it or the planner rewrites it.
    Draft,
    Pending,
    Running,
    WaitingApproval,
    Escalated,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Draft => "draft",
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::WaitingApproval => "waitingApproval",
            TaskStatus::Escalated => "escalated",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "draft" => TaskStatus::Draft,
            "pending" => TaskStatus::Pending,
            "running" => TaskStatus::Running,
            "waitingApproval" => TaskStatus::WaitingApproval,
            "escalated" => TaskStatus::Escalated,
            "done" => TaskStatus::Done,
            "failed" => TaskStatus::Failed,
            "cancelled" => TaskStatus::Cancelled,
            _ => return None,
        })
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskLogEntry {
    /// RFC 3339 timestamp.
    pub at: String,
    /// "info" | "tool_call" | "tool_result" | "assistant" | "approval" | "escalation" | "error"
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub status: TaskStatus,
    /// "user" | "voice" | "meeting" | "agent"
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    pub result: Option<String>,
    pub log: Vec<TaskLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: String,
    pub task_id: String,
    pub task_title: String,
    pub tool: String,
    pub level: PermissionLevel,
    pub input: serde_json::Value,
    /// One-sentence reason the agent gave, or the tool description.
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Escalation {
    pub task_id: String,
    pub task_title: String,
    pub question: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItem {
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingNotes {
    pub summary: String,
    pub decisions: Vec<String>,
    pub action_items: Vec<ActionItem>,
    pub open_questions: Vec<String>,
    /// Number of final transcript segments covered by these notes.
    pub covered_segments: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveWindow {
    pub title: String,
    /// Executable file name, lower case, for example "outlook.exe".
    pub process_name: String,
    pub hwnd: isize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Txt,
    Markdown,
    Srt,
    Vtt,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum InsertMethod {
    /// Clipboard + Ctrl+V, then restore the previous clipboard.
    #[default]
    Paste,
    /// Synthesized Unicode keystrokes. Slower, works where paste is blocked.
    Type,
}

#[derive(Debug, Clone, Default)]
pub struct PolishContext {
    /// Free-form instructions from the user, for example "formal, British spelling".
    pub instructions: String,
    /// Add a tone hint inferred from `window` (the app the text is going to).
    pub match_app: bool,
    pub window: Option<ActiveWindow>,
    /// Free-form note from settings, for example the user's role or vocabulary.
    pub user_profile: String,
}

/// What happens to text after transcription. The same step serves dictation, edits
/// of typed or pasted text, and finished transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CleanupMode {
    /// Text as transcribed, trimmed.
    Off,
    /// Local rules: fillers, stutters, punctuation, capitals. No network.
    Basic,
    /// Language model with the user's instructions. Falls back to Basic on failure.
    #[default]
    Ai,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryKind {
    /// Microphone speech, transformed and inserted.
    Dictation,
    /// Typed or pasted text run through the transform.
    Edit,
    /// A finished live session (transcribe, translate, meeting or agent mode).
    Transcript,
}

/// One run of the input -> transform -> output primitive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub kind: HistoryKind,
    /// RFC 3339.
    pub created_at: String,
    /// Text before the transform: the transcript, or the text that was edited.
    pub raw: String,
    /// Final text after the transform.
    pub text: String,
    pub cleanup: CleanupMode,
    pub instructions: String,
    /// Process name of the app the text was meant for, when known.
    pub app: Option<String>,
    /// Live mode for transcripts.
    pub mode: Option<Mode>,
    pub duration_ms: Option<u64>,
    /// Set when the transform fell back, for example "AI cleanup failed: ... Used Basic."
    pub note: Option<String>,
    pub inserted: bool,
    /// File name of the recorded audio in the History audio folder, when it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_file: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Screenshot {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Physical pixels per screenshot pixel (>= 1.0).
    pub scale: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}
