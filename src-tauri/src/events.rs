//! Event names emitted to the frontend with `AppHandle::emit`. FROZEN: owned by the planner.

/// Payload: TranscriptSegment (interim and final).
pub const TRANSCRIPT_SEGMENT: &str = "transcript-segment";
/// Payload: TranslationSegment.
pub const TRANSLATION_SEGMENT: &str = "translation-segment";
/// Payload: HistoryEntry for the finished dictation.
pub const DICTATION_RESULT: &str = "dictation-result";
/// Payload: none. The history list changed; reload it.
pub const HISTORY_CHANGED: &str = "history-changed";
/// Payload: MeetingNotes.
pub const NOTES_UPDATED: &str = "notes-updated";
/// Payload: Task (full, after every change).
pub const AGENT_TASK: &str = "agent-task";
/// Payload: { id: String } when a task was deleted or replaced by a plan.
pub const AGENT_TASK_REMOVED: &str = "agent-task-removed";
/// Payload: ApprovalRequest.
pub const AGENT_APPROVAL: &str = "agent-approval";
/// Payload: { id: String } when an approval is resolved or dropped.
pub const AGENT_APPROVAL_RESOLVED: &str = "agent-approval-resolved";
/// Payload: Escalation.
pub const AGENT_ESCALATION: &str = "agent-escalation";
/// Payload: AppState.
pub const APP_STATE: &str = "app-state";
/// Payload: { source: AudioSource, rms: f32 }.
pub const AUDIO_LEVEL: &str = "audio-level";
/// Payload: { scope: String, message: String }.
pub const APP_ERROR: &str = "app-error";
/// Payload: { name: String, done: u64, total: u64 } while a Whisper model downloads.
pub const WHISPER_DOWNLOAD: &str = "whisper-download";
