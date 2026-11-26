// Typed wrappers for every Tauri command and event. Outside Tauri (plain browser),
// calls go to a small in-memory mock so the UI can be developed with `npm run dev`.

import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { mockInvoke, mockListen, seedMock } from './mock';
import type {
  ApiKeyProvider,
  AppErrorPayload,
  AppState,
  ApprovalRequest,
  ApprovalResolvedPayload,
  AudioDeviceInfo,
  AudioLevelPayload,
  CleanupMode,
  DictationResult,
  HistoryEntry,
  Escalation,
  ExportFormat,
  MeetingNotes,
  Mode,
  ModelCatalog,
  ModelChoice,
  Settings,
  Task,
  TaskPlan,
  TaskRemovedPayload,
  TranscriptSegment,
  TranscriptState,
  TranslationSegment,
  WhisperDownloadPayload,
  WhisperStatus,
} from './types';

export const isTauri: boolean = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

if (!isTauri) seedMock();

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri) return invoke<T>(cmd, args);
  return mockInvoke(cmd, args) as Promise<T>;
}

// Event names, from src-tauri/src/events.rs
export const EVENTS = {
  transcriptSegment: 'transcript-segment',
  translationSegment: 'translation-segment',
  dictationResult: 'dictation-result',
  notesUpdated: 'notes-updated',
  agentTask: 'agent-task',
  agentTaskRemoved: 'agent-task-removed',
  agentApproval: 'agent-approval',
  agentApprovalResolved: 'agent-approval-resolved',
  agentEscalation: 'agent-escalation',
  appState: 'app-state',
  audioLevel: 'audio-level',
  appError: 'app-error',
  whisperDownload: 'whisper-download',
  historyChanged: 'history-changed',
} as const;

export type Unlisten = () => void;

async function on<T>(name: string, cb: (payload: T) => void): Promise<Unlisten> {
  if (isTauri) {
    return listen<T>(name, (e) => cb(e.payload));
  }
  return mockListen(name, (p) => cb(p as T));
}

export const onTranscriptSegment = (cb: (p: TranscriptSegment) => void) =>
  on<TranscriptSegment>(EVENTS.transcriptSegment, cb);
export const onTranslationSegment = (cb: (p: TranslationSegment) => void) =>
  on<TranslationSegment>(EVENTS.translationSegment, cb);
export const onDictationResult = (cb: (p: DictationResult) => void) =>
  on<DictationResult>(EVENTS.dictationResult, cb);
export const onNotesUpdated = (cb: (p: MeetingNotes) => void) => on<MeetingNotes>(EVENTS.notesUpdated, cb);
export const onAgentTask = (cb: (p: Task) => void) => on<Task>(EVENTS.agentTask, cb);
export const onAgentTaskRemoved = (cb: (p: TaskRemovedPayload) => void) =>
  on<TaskRemovedPayload>(EVENTS.agentTaskRemoved, cb);
export const onAgentApproval = (cb: (p: ApprovalRequest) => void) =>
  on<ApprovalRequest>(EVENTS.agentApproval, cb);
export const onAgentApprovalResolved = (cb: (p: ApprovalResolvedPayload) => void) =>
  on<ApprovalResolvedPayload>(EVENTS.agentApprovalResolved, cb);
export const onAgentEscalation = (cb: (p: Escalation) => void) => on<Escalation>(EVENTS.agentEscalation, cb);
export const onAppState = (cb: (p: AppState) => void) => on<AppState>(EVENTS.appState, cb);
export const onAudioLevel = (cb: (p: AudioLevelPayload) => void) => on<AudioLevelPayload>(EVENTS.audioLevel, cb);
export const onAppError = (cb: (p: AppErrorPayload) => void) => on<AppErrorPayload>(EVENTS.appError, cb);
export const onWhisperDownload = (cb: (p: WhisperDownloadPayload) => void) =>
  on<WhisperDownloadPayload>(EVENTS.whisperDownload, cb);
export const onHistoryChanged = (cb: () => void) => on<null>(EVENTS.historyChanged, () => cb());

// Commands
export const getSettings = () => call<Settings>('get_settings');
export const saveSettings = (settings: Settings) => call<void>('save_settings', { settings });
export const setApiKey = (provider: ApiKeyProvider, key: string) => call<void>('set_api_key', { provider, key });
export const hasApiKey = (provider: ApiKeyProvider) => call<boolean>('has_api_key', { provider });
/** Send a tiny request to one model. Resolves "Connected (model)" or rejects with the error. */
export const testLlm = (choice: ModelChoice, settings?: Settings) =>
  call<string>('test_llm', { choice, settings: settings ?? null });
/** Models each source offers right now, from the stored keys and the servers that answer. */
export const listModels = (settings?: Settings) => call<ModelCatalog>('list_models', { settings: settings ?? null });
/** Remember the mode the Live hotkey starts, without starting a session. */
export const setLiveMode = (mode: Mode) => call<void>('set_live_mode', { mode });
export const listAudioDevices = () => call<AudioDeviceInfo[]>('list_audio_devices');
export const startSession = (mode: Mode) => call<void>('start_session', { mode });
export const stopSession = () => call<void>('stop_session');
/** Escape while listening: stop capture and discard the dictation. */
export const cancelDictation = () => call<void>('cancel_dictation');
/** Escape while AI cleanup runs: use the Basic result now. */
export const skipCleanup = () => call<void>('skip_cleanup');
/** Clear a dictation error from the overlay. */
export const dismissDictationError = () => call<void>('dismiss_dictation_error');
export const getState = () => call<AppState>('get_state');
export const getTranscript = () => call<TranscriptState>('get_transcript');
export const clearTranscript = () => call<void>('clear_transcript');
export const runEdit = (text: string, cleanup: CleanupMode, instructions: string) =>
  call<HistoryEntry>('run_edit', { text, cleanup, instructions });
export const insertText = (text: string, historyId: string | null = null) =>
  call<void>('insert_text', { text, historyId });
export const historyList = () => call<HistoryEntry[]>('history_list');
export const historyDelete = (id: string) => call<void>('history_delete', { id });

let audioDir: Promise<string> | null = null;
/** URL the History player loads a kept recording from. */
export async function historyAudioUrl(file: string): Promise<string> {
  if (!isTauri) return mockInvoke('history_audio_url', { file }) as Promise<string>;
  audioDir ??= invoke<string>('history_audio_dir');
  const dir = await audioDir;
  return convertFileSrc(`${dir.replace(/[\\/]+$/, '')}\\${file}`);
}
export const historyClear = () => call<void>('history_clear');
export const exportTranscript = (format: ExportFormat, includeTranslations: boolean) =>
  call<string>('export_transcript', { format, includeTranslations });
export const saveExport = (format: ExportFormat, includeTranslations: boolean) =>
  call<string>('save_export', { format, includeTranslations });
export const generateNotes = () => call<MeetingNotes>('generate_notes');
/** `run: false` stores a backlog draft. `run: true` starts the task now. */
export const agentCreateTask = (title: string, detail: string, run: boolean) =>
  call<Task>('agent_create_task', { title, detail, run });
/** Start a backlog draft. */
export const agentRunTask = (id: string) => call<void>('agent_run_task', { id });
/** Edit a backlog draft. */
export const agentUpdateTask = (id: string, title: string, detail: string) =>
  call<Task>('agent_update_task', { id, title, detail });
/** Delete a draft or a finished task. */
export const agentDeleteTask = (id: string) => call<void>('agent_delete_task', { id });
/** Ask the planner model for a proposal over these drafts. Changes nothing. */
export const agentPlanTasks = (ids: string[], instruction: string) =>
  call<TaskPlan>('agent_plan_tasks', { ids, instruction });
/** Replace these drafts with the goals of a plan. Resolves with the new drafts. */
export const agentApplyPlan = (ids: string[], plan: TaskPlan) => call<Task[]>('agent_apply_plan', { ids, plan });
export const agentListTasks = () => call<Task[]>('agent_list_tasks');
export const agentPendingApprovals = () => call<ApprovalRequest[]>('agent_pending_approvals');
export const agentCancelTask = (id: string) => call<void>('agent_cancel_task', { id });
export const agentResolveApproval = (id: string, approve: boolean, remember: boolean) =>
  call<void>('agent_resolve_approval', { id, approve, remember });
export const agentReplyEscalation = (taskId: string, text: string) =>
  call<void>('agent_reply_escalation', { taskId, text });
export const whisperStatus = () => call<WhisperStatus>('whisper_status');
export const downloadWhisperModel = (name: string) => call<void>('download_whisper_model', { name });
export const cancelWhisperDownload = () => call<void>('cancel_whisper_download');
export const showMain = () => call<void>('show_main');
export const toggleOverlay = () => call<void>('toggle_overlay');

