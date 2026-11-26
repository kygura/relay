// TypeScript mirrors of the Rust types in src-tauri/src/types.rs and settings.rs.
// All Rust types serialize with camelCase field names and camelCase enum strings.

export type AudioSource = 'microphone' | 'system';

export interface AudioDeviceInfo {
  id: string;
  name: string;
  kind: AudioSource;
  isDefault: boolean;
}

export interface TranscriptSegment {
  id: string;
  source: AudioSource;
  speaker: string | null;
  text: string;
  startMs: number;
  endMs: number;
  isFinal: boolean;
  language: string | null;
}

export interface TranslationSegment {
  segmentId: string;
  targetLang: string;
  text: string;
}

export interface TranscriptState {
  segments: TranscriptSegment[];
  translations: TranslationSegment[];
}

export type Mode = 'idle' | 'transcribe' | 'dictation' | 'translate' | 'meeting' | 'agent';

/** Processing phase. Dictation walks listening, transcribing, cleaning, then done or error. */
export type Phase = 'idle' | 'listening' | 'transcribing' | 'cleaning' | 'done' | 'error';

export interface AppState {
  mode: Mode;
  recording: boolean;
  sessionStartedAt: string | null;
  pendingApprovals: number;
  activeTasks: number;
  lastError: string | null;
  phase: Phase;
  /** RFC 3339 time the current phase began. */
  phaseSince: string | null;
  /** done: target app name without .exe. error: one-line reason. cleaning: 'ai' or 'basic'. */
  phaseDetail: string | null;
}

export type PermissionLevel = 'readOnly' | 'localReversible' | 'external' | 'destructive';

/** A draft sits in the backlog and never runs on its own. */
export type TaskStatus =
  | 'draft'
  | 'pending'
  | 'running'
  | 'waitingApproval'
  | 'escalated'
  | 'done'
  | 'failed'
  | 'cancelled';

export type TaskLogKind =
  | 'info'
  | 'tool_call'
  | 'tool_result'
  | 'assistant'
  | 'approval'
  | 'escalation'
  | 'error';

export interface TaskLogEntry {
  at: string;
  kind: TaskLogKind | string;
  message: string;
}

export type TaskSource = 'user' | 'voice' | 'meeting' | 'agent';

export interface Task {
  id: string;
  title: string;
  detail: string;
  status: TaskStatus;
  source: TaskSource | string;
  createdAt: string;
  updatedAt: string;
  result: string | null;
  log: TaskLogEntry[];
}

export type PlanChange = 'kept' | 'cleaned' | 'split' | 'merged';

/** One executable goal the planner proposes. `from` holds the ids of the drafts it came from. */
export interface PlanGoal {
  title: string;
  detail: string;
  from: string[];
  change: PlanChange;
}

export interface PlanDrop {
  id: string;
  reason: string;
}

/** A proposal for the backlog. Nothing changes until it is applied. */
export interface TaskPlan {
  summary: string;
  goals: PlanGoal[];
  dropped: PlanDrop[];
}

export interface ApprovalRequest {
  id: string;
  taskId: string;
  taskTitle: string;
  tool: string;
  level: PermissionLevel;
  input: unknown;
  reason: string;
  createdAt: string;
}

export interface Escalation {
  taskId: string;
  taskTitle: string;
  question: string;
  createdAt: string;
}

export interface ActionItem {
  text: string;
  owner: string | null;
  due: string | null;
}

export interface MeetingNotes {
  summary: string;
  decisions: string[];
  actionItems: ActionItem[];
  openQuestions: string[];
  coveredSegments: number;
  updatedAt: string;
}

export type ExportFormat = 'txt' | 'markdown' | 'srt' | 'vtt' | 'json';

export type InsertMethod = 'paste' | 'type';

export type SttEngineKind = 'localWhisper' | 'openAiCompatible' | 'deepgram';

export interface WhisperModelInfo {
  name: string;
  label: string;
  sizeMb: number;
  englishOnly: boolean;
  downloaded: boolean;
}

export interface WhisperStatus {
  built: boolean;
  modelsDir: string;
  models: WhisperModelInfo[];
  downloading: string | null;
}

export interface WhisperDownloadPayload {
  name: string;
  done: number;
  total: number;
}

/**
 * Where a text model runs. `anthropic` is Claude. `openAi` is the account at `openaiBaseUrl`.
 * `server` is the OpenAI-compatible server at `llmBaseUrl` (Ollama, LM Studio or another).
 */
export type LlmSource = 'anthropic' | 'openAi' | 'server';

export interface ModelChoice {
  source: LlmSource;
  model: string;
}

export interface ModelInfo {
  id: string;
  label: string;
}

/** The models one source offers right now. `available` is false without a key or a reachable server. */
export interface ModelSource {
  source: LlmSource;
  label: string;
  local: boolean;
  hasKey: boolean;
  available: boolean;
  note: string | null;
  models: ModelInfo[];
}

export interface SpeechOption {
  engine: SttEngineKind;
  label: string;
  local: boolean;
  available: boolean;
  note: string | null;
  models: ModelInfo[];
}

export interface ModelCatalog {
  sources: ModelSource[];
  speech: SpeechOption[];
}

/** The settings keys that hold a model choice, one per feature. */
export type ModelFeature = 'dictationLlm' | 'translationLlm' | 'notesLlm' | 'plannerLlm';

export interface Settings {
  settingsVersion: number;
  /** Address of the `server` source. */
  llmBaseUrl: string;
  /** Dictation polish and the editor. */
  dictationLlm: ModelChoice;
  /** Live translation, once per caption. */
  translationLlm: ModelChoice;
  /** Meeting notes. */
  notesLlm: ModelChoice;
  /** Task planner: cleans up, splits and merges backlog drafts. */
  plannerLlm: ModelChoice;
  /** The agent runs on Claude, because it needs tool use. */
  agentModel: string;
  sttEngine: SttEngineKind;
  whisperModel: string;
  whisperModelPath: string;
  openaiBaseUrl: string;
  openaiSttModel: string;
  deepgramModel: string;
  sourceLanguage: string | null;
  targetLanguages: string[];
  translateMic: boolean;
  translateSystem: boolean;
  captureMic: boolean;
  micDevice: string | null;
  captureSystem: boolean;
  systemDevice: string | null;
  vadThreshold: number;
  silenceMs: number;
  maxChunkMs: number;
  /** Keep the audio of each dictation and live session with its History entry. */
  saveAudio: boolean;
  cleanup: CleanupMode;
  instructions: string;
  autoToneFromApp: boolean;
  userProfile: string;
  insertMethod: InsertMethod;
  autoInsertDictation: boolean;
  notesIntervalSecs: number;
  autoExtractTasks: boolean;
  agentAutoLevel: PermissionLevel;
  agentMaxConcurrent: number;
  hotkeyToggle: string;
  hotkeyDictate: string;
  /** Mode the Live hotkey starts. Owned by the backend: save_settings keeps the stored value. */
  liveMode: Mode;
}

export type ApiKeyProvider = 'anthropic' | 'openai' | 'deepgram' | 'openai-llm';

/** What happens to text after transcription. Shared by dictation, edits and transcripts. */
export type CleanupMode = 'off' | 'basic' | 'ai';

export type HistoryKind = 'dictation' | 'edit' | 'transcript';

/** One run of input -> transform -> output. */
export interface HistoryEntry {
  id: string;
  kind: HistoryKind;
  createdAt: string;
  raw: string;
  text: string;
  cleanup: CleanupMode;
  instructions: string;
  app: string | null;
  mode: Mode | null;
  durationMs: number | null;
  note: string | null;
  inserted: boolean;
  /** File name of the kept recording in the History audio folder. Absent when none was kept. */
  audioFile?: string;
}

// Event payloads (see src-tauri/src/events.rs)

export type DictationResult = HistoryEntry;

export interface AudioLevelPayload {
  source: AudioSource;
  rms: number;
}

export interface AppErrorPayload {
  scope: string;
  message: string;
}

export interface ApprovalResolvedPayload {
  id: string;
}

export interface TaskRemovedPayload {
  id: string;
}

