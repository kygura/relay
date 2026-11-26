// App state store. Subscribes to backend events once per window and exposes
// selector hooks built on useSyncExternalStore.

import { useEffect, useSyncExternalStore } from 'react';
import * as api from './api';
import type {
  AppState,
  ApprovalRequest,
  DictationResult,
  Escalation,
  HistoryEntry,
  HistoryKind,
  MeetingNotes,
  ModelCatalog,
  Settings,
  Task,
  TranscriptSegment,
  TranslationSegment,
  CleanupMode,
} from './types';

export interface Toast {
  id: number;
  kind: 'error' | 'info';
  scope: string;
  message: string;
}

export interface RelayState {
  ready: boolean;
  /** Final segments in time order. */
  finals: TranscriptSegment[];
  /** Interim segments by id. */
  interim: Record<string, TranscriptSegment>;
  /** Translations by segment id. */
  translations: Record<string, TranslationSegment[]>;
  app: AppState;
  /** Tasks, newest first. */
  tasks: Task[];
  approvals: ApprovalRequest[];
  /** Open escalations by task id. */
  escalations: Record<string, Escalation>;
  notes: MeetingNotes | null;
  levels: { microphone: number; system: number };
  dictation: DictationResult | null;
  /** Dictations, edits and transcripts, newest first. Null until loaded. */
  history: HistoryEntry[] | null;
  editor: EditorState;
  toasts: Toast[];
  /** Saved settings with any edits not written yet. Null until loaded. */
  settings: Settings | null;
  /** 'saving' from an edit until the write returns. */
  settingsSave: 'saved' | 'saving' | 'error';
  /** Models each source offers. Null until the first listing returns. */
  catalog: ModelCatalog | null;
  catalogBusy: boolean;
}

/** The editor is the visible form of the shared primitive: input, instructions, output. */
export interface EditorState {
  input: string;
  output: string;
  /** History entry the output was saved as. */
  entryId: string | null;
  /** Where the input came from. */
  source: HistoryKind | null;
  app: string | null;
  note: string | null;
  inserted: boolean;
  /** Transform settings of an opened entry, for the editor to apply once and then clear. */
  load: { instructions: string; cleanup: CleanupMode } | null;
}

const emptyEditor: EditorState = {
  input: '',
  output: '',
  entryId: null,
  source: null,
  app: null,
  note: null,
  inserted: false,
  load: null,
};

const emptyApp: AppState = {
  mode: 'idle',
  recording: false,
  sessionStartedAt: null,
  pendingApprovals: 0,
  activeTasks: 0,
  lastError: null,
  phase: 'idle',
  phaseSince: null,
  phaseDetail: null,
};

let state: RelayState = {
  ready: false,
  finals: [],
  interim: {},
  translations: {},
  app: emptyApp,
  tasks: [],
  approvals: [],
  escalations: {},
  notes: null,
  levels: { microphone: 0, system: 0 },
  dictation: null,
  history: null,
  editor: emptyEditor,
  toasts: [],
  settings: null,
  settingsSave: 'saved',
  catalog: null,
  catalogBusy: false,
};

const subs = new Set<() => void>();

function set(patch: Partial<RelayState> | ((s: RelayState) => Partial<RelayState>)): void {
  const p = typeof patch === 'function' ? patch(state) : patch;
  state = { ...state, ...p };
  subs.forEach((f) => f());
}

export function getRelayState(): RelayState {
  return state;
}

export function useRelay<T>(selector: (s: RelayState) => T): T {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => {
        subs.delete(cb);
      };
    },
    () => selector(state),
    () => selector(state),
  );
}

// Selectors

export const selApp = (s: RelayState) => s.app;
export const selPhase = (s: RelayState) => s.app.phase;
export const selLevels = (s: RelayState) => s.levels;
export const selFinals = (s: RelayState) => s.finals;
export const selInterim = (s: RelayState) => s.interim;
export const selTranslations = (s: RelayState) => s.translations;
export const selTasks = (s: RelayState) => s.tasks;
export const selApprovals = (s: RelayState) => s.approvals;
export const selEscalations = (s: RelayState) => s.escalations;
export const selNotes = (s: RelayState) => s.notes;
export const selDictation = (s: RelayState) => s.dictation;
export const selHistory = (s: RelayState) => s.history;
export const selEditor = (s: RelayState) => s.editor;

export function setEditor(patch: Partial<EditorState>): void {
  set((s) => ({ editor: { ...s.editor, ...patch } }));
}

export function clearEditor(): void {
  set({ editor: emptyEditor });
}

/** Load a history entry into the editor: its original text as input, its result as output. */
export function openInEditor(e: HistoryEntry): void {
  set({
    editor: {
      input: e.raw,
      output: e.text,
      entryId: e.id,
      source: e.kind,
      app: e.app,
      note: e.note,
      inserted: e.inserted,
      // Transcripts were never transformed, so keep the editor's current settings for them.
      load: e.kind === 'transcript' ? null : { instructions: e.instructions, cleanup: e.cleanup },
    },
  });
}

export async function refreshHistory(): Promise<void> {
  try {
    set({ history: await api.historyList() });
  } catch (e) {
    reportError('history_list', e);
  }
}
export const selToasts = (s: RelayState) => s.toasts;
export const selSettings = (s: RelayState) => s.settings;
export const selSettingsSave = (s: RelayState) => s.settingsSave;
export const selCatalog = (s: RelayState) => s.catalog;
export const selCatalogBusy = (s: RelayState) => s.catalogBusy;

// Settings. Every surface edits the same copy. An edit shows at once and is written a moment
// later, so a run of keystrokes becomes one write.

const SAVE_DELAY_MS = 350;
let saveTimer: number | null = null;
let saveSeq = 0;
/** Keys that change which models a source lists. */
const CATALOG_KEYS: Array<keyof Settings> = ['llmBaseUrl', 'openaiBaseUrl'];
let catalogTimer: number | null = null;

async function writeSettings(): Promise<void> {
  saveTimer = null;
  const s = state.settings;
  if (!s) return;
  const seq = ++saveSeq;
  try {
    await api.saveSettings(s);
    if (seq === saveSeq && saveTimer === null) set({ settingsSave: 'saved' });
  } catch (e) {
    if (seq === saveSeq) set({ settingsSave: 'error' });
    reportError('save_settings', e);
  }
}

/** Change settings from any surface. */
export function patchSettings(patch: Partial<Settings>): void {
  const cur = state.settings;
  if (!cur) return;
  set({ settings: { ...cur, ...patch }, settingsSave: 'saving' });
  if (saveTimer !== null) window.clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => void writeSettings(), SAVE_DELAY_MS);
  if (CATALOG_KEYS.some((k) => k in patch)) {
    if (catalogTimer !== null) window.clearTimeout(catalogTimer);
    catalogTimer = window.setTimeout(() => void refreshCatalog(), 900);
  }
}

let catalogSeq = 0;

/** List the models again. Call after a key or a server address changes. */
export async function refreshCatalog(): Promise<void> {
  const seq = ++catalogSeq;
  set({ catalogBusy: true });
  try {
    const catalog = await api.listModels(state.settings ?? undefined);
    if (seq === catalogSeq) set({ catalog });
  } catch (e) {
    reportError('list_models', e);
  } finally {
    if (seq === catalogSeq) set({ catalogBusy: false });
  }
}

async function loadSettings(): Promise<void> {
  try {
    set({ settings: await api.getSettings() });
  } catch (e) {
    reportError('get_settings', e);
  }
}

let catalogAsked = false;

/** Models each source offers. The first caller starts the listing. */
export function useCatalog(): ModelCatalog | null {
  const catalog = useRelay(selCatalog);
  const ready = useRelay((s) => s.settings !== null);
  useEffect(() => {
    if (ready && !catalogAsked) {
      catalogAsked = true;
      void refreshCatalog();
    }
  }, [ready]);
  return catalog;
}

// Reducers

function upsertTask(tasks: Task[], t: Task): Task[] {
  const i = tasks.findIndex((x) => x.id === t.id);
  const next = i >= 0 ? tasks.map((x, j) => (j === i ? t : x)) : [t, ...tasks];
  return next.sort((a, b) => b.createdAt.localeCompare(a.createdAt));
}

function upsertFinal(list: TranscriptSegment[], seg: TranscriptSegment): TranscriptSegment[] {
  const i = list.findIndex((x) => x.id === seg.id);
  if (i >= 0) return list.map((x, j) => (j === i ? seg : x));
  const next = [...list, seg];
  if (next.length > 1 && next[next.length - 2].startMs > seg.startMs) {
    next.sort((a, b) => a.startMs - b.startMs);
  }
  return next;
}

function groupTranslations(list: TranslationSegment[]): Record<string, TranslationSegment[]> {
  const out: Record<string, TranslationSegment[]> = {};
  for (const t of list) addTo(out, t);
  return out;
}

function addTo(map: Record<string, TranslationSegment[]>, t: TranslationSegment): void {
  const cur = map[t.segmentId] ?? [];
  const i = cur.findIndex((x) => x.targetLang === t.targetLang);
  map[t.segmentId] = i >= 0 ? cur.map((x, j) => (j === i ? t : x)) : [...cur, t];
}

let toastSeq = 0;

export function pushToast(kind: Toast['kind'], scope: string, message: string): void {
  const id = ++toastSeq;
  set((s) => ({ toasts: [...s.toasts.slice(-4), { id, kind, scope, message }] }));
  if (kind === 'info') {
    window.setTimeout(() => dismissToast(id), 6000);
  }
}

export function dismissToast(id: number): void {
  set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
}

export function reportError(scope: string, e: unknown): void {
  const message = e instanceof Error ? e.message : typeof e === 'string' ? e : JSON.stringify(e);
  pushToast('error', scope, message);
}

export function setNotes(notes: MeetingNotes): void {
  set({ notes });
}

export function setDictation(d: DictationResult | null): void {
  set({ dictation: d });
}

export function removeApproval(id: string): void {
  set((s) => ({ approvals: s.approvals.filter((a) => a.id !== id) }));
}

export function removeEscalation(taskId: string): void {
  set((s) => {
    const next = { ...s.escalations };
    delete next[taskId];
    return { escalations: next };
  });
}

export function upsertTaskLocal(t: Task): void {
  set((s) => ({ tasks: upsertTask(s.tasks, t) }));
}

export function removeTaskLocal(id: string): void {
  set((s) => ({ tasks: s.tasks.filter((t) => t.id !== id) }));
}

export function clearTranscriptLocal(): void {
  set({ finals: [], interim: {}, translations: {} });
}

// Level meters are fed about 20 times per second. Keep the last two values so
// the bar does not jump on single quiet frames.
let levelSmoothing = { microphone: 0, system: 0 };

let initPromise: Promise<void> | null = null;

export function initStore(): Promise<void> {
  if (initPromise) return initPromise;
  initPromise = (async () => {
    await Promise.all([
      api.onTranscriptSegment((seg) => {
        if (seg.isFinal) {
          set((s) => {
            const interim = { ...s.interim };
            delete interim[seg.id];
            return { finals: upsertFinal(s.finals, seg), interim };
          });
        } else {
          set((s) => ({ interim: { ...s.interim, [seg.id]: seg } }));
        }
      }),
      api.onTranslationSegment((t) => {
        set((s) => {
          const map = { ...s.translations };
          addTo(map, t);
          return { translations: map };
        });
      }),
      api.onDictationResult((d) => {
        set({ dictation: d });
        openInEditor(d);
      }),
      api.onHistoryChanged(() => void refreshHistory()),
      api.onNotesUpdated((n) => set({ notes: n })),
      api.onAgentTask((t) => {
        set((s) => {
          const escalations = { ...s.escalations };
          if (t.status !== 'escalated') delete escalations[t.id];
          return { tasks: upsertTask(s.tasks, t), escalations };
        });
      }),
      api.onAgentTaskRemoved((p) => removeTaskLocal(p.id)),
      api.onAgentApproval((a) => {
        set((s) => ({
          approvals: s.approvals.some((x) => x.id === a.id) ? s.approvals : [...s.approvals, a],
        }));
      }),
      api.onAgentApprovalResolved((p) => removeApproval(p.id)),
      api.onAgentEscalation((e) => set((s) => ({ escalations: { ...s.escalations, [e.taskId]: e } }))),
      api.onAppState((a) => {
        set((s) => {
          // A new session starts with an empty interim map.
          const interim = a.recording ? s.interim : {};
          return { app: a, interim };
        });
      }),
      api.onAudioLevel((p) => {
        const prev = levelSmoothing[p.source];
        const level = meterLevel(p.rms);
        const next = level > prev ? level : prev * 0.7 + level * 0.3;
        levelSmoothing = { ...levelSmoothing, [p.source]: next };
        set({ levels: { ...levelSmoothing } });
      }),
      api.onAppError((e) => {
        pushToast('error', e.scope, e.message);
      }),
    ]);

    // Initial snapshots. A failed snapshot is reported but does not stop the UI.
    const [app, transcript, tasks, pending] = await Promise.allSettled([
      api.getState(),
      api.getTranscript(),
      api.agentListTasks(),
      api.agentPendingApprovals(),
    ]);
    if (pending.status === 'fulfilled') {
      set((s) => ({
        approvals: [
          ...s.approvals,
          ...pending.value.filter((a) => !s.approvals.some((x) => x.id === a.id)),
        ],
      }));
    } else reportError('agent_pending_approvals', pending.reason);
    if (app.status === 'fulfilled') set({ app: app.value });
    else reportError('get_state', app.reason);
    if (transcript.status === 'fulfilled') {
      set({
        finals: [...transcript.value.segments].sort((a, b) => a.startMs - b.startMs),
        translations: groupTranslations(transcript.value.translations),
      });
    } else reportError('get_transcript', transcript.reason);
    if (tasks.status === 'fulfilled') {
      set({ tasks: [...tasks.value].sort((a, b) => b.createdAt.localeCompare(a.createdAt)) });
    } else reportError('agent_list_tasks', tasks.reason);
    await Promise.all([refreshHistory(), loadSettings()]);
    set({ ready: true });
  })();
  return initPromise;
}

/** Map RMS (0 to 1) to a 0 to 1 meter position on a 60 dB scale. */
export function meterLevel(rms: number): number {
  if (rms <= 0.000001) return 0;
  const db = 20 * Math.log10(rms);
  return Math.min(1, Math.max(0, (db + 60) / 60));
}

