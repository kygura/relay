// In-browser mock of the Tauri backend. Used only when the page runs outside Tauri,
// so that `npm run dev` in a normal browser shows a working UI with sample data.

import type {
  ApprovalRequest,
  AppState,
  AudioDeviceInfo,
  CleanupMode,
  HistoryEntry,
  HistoryKind,
  MeetingNotes,
  Mode,
  ModelCatalog,
  ModelChoice,
  Phase,
  Settings,
  Task,
  TaskPlan,
  TranscriptSegment,
  TranscriptState,
  TranslationSegment,
} from './types';

type Handler = (payload: unknown) => void;

const listeners = new Map<string, Set<Handler>>();

export function mockListen(name: string, cb: Handler): () => void {
  let set = listeners.get(name);
  if (!set) {
    set = new Set();
    listeners.set(name, set);
  }
  set.add(cb);
  return () => {
    set?.delete(cb);
  };
}

function emit(name: string, payload: unknown): void {
  listeners.get(name)?.forEach((cb) => cb(payload));
}

const now = () => new Date().toISOString();
let counter = 0;
const uid = () => `mock-${++counter}-${Math.random().toString(36).slice(2, 8)}`;

const defaultSettings: Settings = {
  settingsVersion: 4,
  llmBaseUrl: 'http://localhost:11434/v1',
  dictationLlm: { source: 'anthropic', model: 'claude-haiku-5-5' },
  translationLlm: { source: 'server', model: 'llama3.2' },
  notesLlm: { source: 'anthropic', model: 'claude-sonnet-5-5' },
  plannerLlm: { source: 'anthropic', model: 'claude-sonnet-5-5' },
  agentModel: 'claude-opus-5-5',
  sttEngine: 'localWhisper',
  whisperModel: 'small-q5_1',
  whisperModelPath: '',
  openaiBaseUrl: 'https://api.openai.com/v1',
  openaiSttModel: 'gpt-4o-transcribe',
  deepgramModel: 'nova-3',
  sourceLanguage: null,
  targetLanguages: ['es', 'fr'],
  translateMic: false,
  translateSystem: true,
  captureMic: true,
  micDevice: null,
  captureSystem: true,
  systemDevice: null,
  vadThreshold: 0.012,
  silenceMs: 700,
  maxChunkMs: 12000,
  saveAudio: true,
  cleanup: 'ai',
  instructions: '',
  autoToneFromApp: true,
  userProfile: '',
  insertMethod: 'paste',
  autoInsertDictation: true,
  notesIntervalSecs: 90,
  autoExtractTasks: false,
  agentAutoLevel: 'readOnly',
  agentMaxConcurrent: 2,
  hotkeyToggle: 'CommandOrControl+Shift+Space',
  hotkeyDictate: 'CommandOrControl+Shift+D',
  liveMode: 'transcribe',
};

let settings: Settings = { ...defaultSettings };

function mockEntry(
  kind: HistoryKind,
  raw: string,
  text: string,
  extra: Partial<HistoryEntry> = {},
): HistoryEntry {
  return {
    id: uid(),
    kind,
    createdAt: now(),
    raw,
    text,
    cleanup: 'basic',
    instructions: '',
    app: null,
    mode: kind === 'dictation' ? 'dictation' : null,
    durationMs: null,
    note: null,
    inserted: false,
    ...extra,
  };
}

const minutesAgo = (m: number) => new Date(Date.now() - m * 60_000).toISOString();

let history: HistoryEntry[] = [
  {
    ...mockEntry(
      'dictation',
      'hey so um can you send me the the deck before the call',
      'Hey, can you send me the deck before the call?',
      {
        app: 'slack.exe',
        durationMs: 4100,
        inserted: true,
        cleanup: 'ai',
        instructions: 'Casual, short.',
        audioFile: 'mock-dictation.wav',
        note: 'Used Basic cleanup. Claude has no credit left.',
      },
    ),
    createdAt: minutesAgo(4),
  },
  {
    ...mockEntry(
      'transcript',
      'Speaker: Good morning everyone.\nSpeaker: The launch date moves to March 14 because QA found two blocking bugs.',
      'Speaker: Good morning everyone.\nSpeaker: The launch date moves to March 14 because QA found two blocking bugs.',
      { mode: 'transcribe', durationMs: 64_000, cleanup: 'off', audioFile: 'mock-transcript.wav' },
    ),
    createdAt: minutesAgo(38),
  },
  {
    ...mockEntry(
      'edit',
      'notes from standup api is slow on login fix by friday',
      '- API is slow on login\n- Fix by Friday',
      { cleanup: 'ai', instructions: 'Bullet points' },
    ),
    createdAt: minutesAgo(190),
  },
];
// The mock starts with a Claude key and a local server, and no OpenAI or Deepgram key.
const keys = new Set<string>(['anthropic']);

const CLAUDE_MODELS = [
  { id: 'claude-haiku-5-5', label: 'Haiku 5.5' },
  { id: 'claude-sonnet-5-5', label: 'Sonnet 5.5' },
  { id: 'claude-opus-5-5', label: 'Opus 5.5' },
  { id: 'claude-fable-5-2', label: 'Fable 5.2' },
];

function mockCatalog(s: Settings): ModelCatalog {
  const ids = (list: string[]) => list.map((id) => ({ id, label: id }));
  const url = s.llmBaseUrl.trim();
  const local = /^https?:\/\/(localhost|127\.0\.0\.1)/.test(url);
  const serverLabel = url.includes(':11434') ? 'Ollama' : url.includes(':1234') ? 'LM Studio' : url.replace(/^https?:\/\//, '').split('/')[0] || 'Local server';
  return {
    sources: [
      {
        source: 'anthropic',
        label: 'Claude',
        local: false,
        hasKey: keys.has('anthropic'),
        available: keys.has('anthropic'),
        note: keys.has('anthropic') ? null : 'Add an Anthropic key.',
        models: CLAUDE_MODELS,
      },
      {
        source: 'openAi',
        label: 'OpenAI',
        local: false,
        hasKey: keys.has('openai'),
        available: keys.has('openai'),
        note: keys.has('openai') ? null : 'Add an OpenAI key.',
        models: keys.has('openai') ? ids(['gpt-4o', 'gpt-4o-mini', 'o4-mini']) : [],
      },
      {
        source: 'server',
        label: serverLabel,
        local,
        hasKey: keys.has('openai-llm'),
        available: local,
        note: url === '' ? 'No server address set.' : local ? null : `No server answered at ${url}.`,
        models: local ? ids(['llama3.2', 'qwen2.5:7b', 'mistral-nemo']) : [],
      },
    ],
    speech: [
      {
        engine: 'localWhisper',
        label: 'Local Whisper',
        local: true,
        available: true,
        note: null,
        models: [{ id: 'small-q5_1', label: 'Small, quantized (default)' }],
      },
      {
        engine: 'openAiCompatible',
        label: 'OpenAI',
        local: false,
        available: keys.has('openai'),
        note: keys.has('openai') ? null : 'Add an OpenAI key.',
        models: ids(['gpt-4o-transcribe', 'gpt-4o-mini-transcribe', 'whisper-1']),
      },
      {
        engine: 'deepgram',
        label: 'Deepgram',
        local: false,
        available: keys.has('deepgram'),
        note: keys.has('deepgram') ? null : 'Add a Deepgram key.',
        models: ids(['nova-3', 'nova-2']),
      },
    ],
  };
}

/** A canned proposal: clean every draft, split one that joins two outcomes with "and", merge exact repeats. */
function mockPlan(ids: string[]): TaskPlan {
  const picked = tasks.filter((t) => ids.includes(t.id));
  const plan: TaskPlan = { summary: '', goals: [], dropped: [] };
  const seen = new Map<string, number>();
  for (const t of picked) {
    const key = t.title.trim().toLowerCase().replace(/[.!]+$/, '');
    const dup = seen.get(key);
    if (dup !== undefined) {
      plan.goals[dup].from.push(t.id);
      plan.goals[dup].change = 'merged';
      continue;
    }
    const parts = t.title.split(/\s+and\s+/i).filter((p) => p.trim().length > 8);
    if (parts.length > 1) {
      for (const p of parts) {
        const title = p.trim().replace(/^./, (c) => c.toUpperCase());
        plan.goals.push({ title, detail: `Done when: ${title.toLowerCase()} is finished and the result is written down.`, from: [t.id], change: 'split' });
      }
      continue;
    }
    seen.set(key, plan.goals.length);
    plan.goals.push({
      title: t.title.trim().replace(/^./, (c) => c.toUpperCase()),
      detail: t.detail || `Done when: ${t.title.trim().toLowerCase()} is finished and the result is written down.`,
      from: [t.id],
      change: t.detail ? 'kept' : 'cleaned',
    });
  }
  plan.summary = `${picked.length} drafts became ${plan.goals.length} goals.`;
  return plan;
}

let state: AppState = {
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

const transcript: TranscriptState = { segments: [], translations: [] };
const tasks: Task[] = [];
const approvals: ApprovalRequest[] = [];
let timers: number[] = [];
let sessionStart = 0;

const devices: AudioDeviceInfo[] = [
  { id: 'Microphone (USB headset)', name: 'Microphone (USB headset)', kind: 'microphone', isDefault: true },
  { id: 'Microphone Array (Realtek)', name: 'Microphone Array (Realtek)', kind: 'microphone', isDefault: false },
  { id: 'Speakers (Realtek)', name: 'Speakers (Realtek)', kind: 'system', isDefault: true },
  { id: 'Headphones (USB headset)', name: 'Headphones (USB headset)', kind: 'system', isDefault: false },
];

function setState(patch: Partial<AppState>): void {
  state = { ...state, ...patch };
  emit('app-state', state);
}

const script: Array<{ source: 'microphone' | 'system'; speaker: string; text: string; es: string }> = [
  {
    source: 'system',
    speaker: 'Speaker 1',
    text: 'Thanks everyone for joining. We have three items on the agenda today.',
    es: 'Gracias a todos por unirse. Hoy tenemos tres puntos en la agenda.',
  },
  {
    source: 'system',
    speaker: 'Speaker 2',
    text: 'The release moves to the 14th because the installer tests are still failing.',
    es: 'El lanzamiento pasa al 14 porque las pruebas del instalador siguen fallando.',
  },
  {
    source: 'microphone',
    speaker: 'You',
    text: 'I can fix the installer tests by Friday. I will send the build to QA after that.',
    es: 'Puedo arreglar las pruebas del instalador para el viernes. Enviare la version a QA despues.',
  },
  {
    source: 'system',
    speaker: 'Speaker 1',
    text: 'Good. Who is updating the changelog?',
    es: 'Bien. Quien actualiza el registro de cambios?',
  },
];

function startMockSession(): void {
  stopTimers();
  sessionStart = Date.now();
  let i = 0;
  const levelTimer = window.setInterval(() => {
    emit('audio-level', { source: 'microphone', rms: Math.random() * 0.08 });
    emit('audio-level', { source: 'system', rms: Math.random() * 0.15 });
  }, 120);
  const segTimer = window.setInterval(() => {
    const item = script[i % script.length];
    const id = uid();
    const startMs = Date.now() - sessionStart;
    const base: TranscriptSegment = {
      id,
      source: item.source,
      speaker: item.speaker,
      text: item.text,
      startMs,
      endMs: startMs + 3000,
      isFinal: false,
      language: 'en',
    };
    emit('transcript-segment', { ...base, text: item.text.split(' ').slice(0, 4).join(' ') });
    window.setTimeout(() => {
      const fin = { ...base, isFinal: true };
      transcript.segments.push(fin);
      emit('transcript-segment', fin);
      // Spanish is scripted. Any other target gets a marked stand-in so the reader has text.
      settings.targetLanguages.forEach((lang, n) => {
        const tr: TranslationSegment = {
          segmentId: id,
          targetLang: lang,
          text: lang === 'es' ? item.es : `[${lang}] ${item.text}`,
        };
        transcript.translations.push(tr);
        window.setTimeout(() => emit('translation-segment', tr), 400 + n * 250);
      });
    }, 900);
    i += 1;
  }, 4500);
  timers = [levelTimer, segTimer];
}

function stopTimers(): void {
  timers.forEach((t) => window.clearInterval(t));
  timers = [];
}

function setPhase(phase: Phase, detail: string | null = null): void {
  setState({ phase, phaseSince: now(), phaseDetail: phase === 'idle' ? null : detail });
}

// A mock dictation walks listening, transcribing (400 ms), cleaning (900 ms), done.
let dictationTimers: number[] = [];

function clearDictationTimers(): void {
  dictationTimers.forEach((t) => window.clearTimeout(t));
  dictationTimers = [];
}

function finishMockDictation(note: string | null): void {
  clearDictationTimers();
  const entry = mockEntry(
    'dictation',
    'um so please move the meeting to tuesday no wait make that wednesday at three',
    'Please move the meeting to Wednesday at 3 pm.',
    { app: 'slack.exe', durationMs: 6200, cleanup: settings.cleanup, note, inserted: settings.autoInsertDictation },
  );
  history.unshift(entry);
  emit('dictation-result', entry);
  emit('history-changed', null);
  setPhase('done', 'slack');
}

function walkMockDictation(): void {
  clearDictationTimers();
  setPhase('transcribing');
  dictationTimers.push(
    window.setTimeout(() => {
      if (settings.cleanup === 'off') {
        finishMockDictation(null);
        return;
      }
      setPhase('cleaning', settings.cleanup === 'ai' ? 'ai' : 'basic');
      dictationTimers.push(window.setTimeout(() => finishMockDictation(null), 900));
    }, 400),
  );
}

function endMockCapture(): void {
  stopTimers();
  emit('audio-level', { source: 'microphone', rms: 0 });
  emit('audio-level', { source: 'system', rms: 0 });
}

function seedApproval(task: Task): void {
  const a: ApprovalRequest = {
    id: uid(),
    taskId: task.id,
    taskTitle: task.title,
    tool: 'delete_file',
    level: 'destructive',
    input: { path: 'C:\\Users\\demo\\Downloads\\old-installer.exe' },
    reason: 'The task asks to remove the old installer after the new build is saved.',
    createdAt: now(),
  };
  approvals.push(a);
  setState({ pendingApprovals: approvals.length });
  emit('agent-approval', a);
}

function makeDraft(title: string, detail: string, source = 'user'): Task {
  const t: Task = {
    id: uid(),
    title,
    detail,
    status: 'draft',
    source,
    createdAt: now(),
    updatedAt: now(),
    result: null,
    log: [],
  };
  tasks.unshift(t);
  emit('agent-task', t);
  return t;
}

function makeTask(title: string, detail: string, source = 'user'): Task {
  const t: Task = {
    id: uid(),
    title,
    detail,
    status: 'running',
    source,
    createdAt: now(),
    updatedAt: now(),
    result: null,
    log: [
      { at: now(), kind: 'info', message: 'Task created.' },
      { at: now(), kind: 'tool_call', message: 'get_active_window {}' },
      { at: now(), kind: 'tool_result', message: 'Relay (relay.exe)' },
    ],
  };
  tasks.unshift(t);
  setState({ activeTasks: tasks.filter((x) => x.status === 'running' || x.status === 'waitingApproval').length });
  emit('agent-task', t);
  window.setTimeout(() => {
    t.status = 'waitingApproval';
    t.updatedAt = now();
    t.log.push({ at: now(), kind: 'approval', message: 'Waiting for approval: delete_file' });
    emit('agent-task', { ...t });
    seedApproval(t);
  }, 1500);
  return t;
}

const sampleNotes: MeetingNotes = {
  summary:
    'The team moved the release to the 14th because installer tests fail. The user will fix the tests by Friday and send the build to QA.',
  decisions: ['Release date moves to the 14th.', 'QA receives the build after the installer tests pass.'],
  actionItems: [
    { text: 'Fix the installer tests', owner: 'You', due: 'Friday' },
    { text: 'Update the changelog', owner: null, due: null },
  ],
  openQuestions: ['Who owns the changelog update?'],
  coveredSegments: 4,
  updatedAt: now(),
};

const toneUrls = new Map<number, string>();
/** Object URL of a quiet 16 kHz WAV tone that rises and falls, `seconds` long. */
function mockToneUrl(seconds: number): string {
  const cached = toneUrls.get(seconds);
  if (cached) return cached;
  const rate = 16000;
  const n = rate * seconds;
  const buf = new ArrayBuffer(44 + n * 2);
  const v = new DataView(buf);
  const str = (o: number, s: string) => [...s].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
  str(0, 'RIFF');
  v.setUint32(4, 36 + n * 2, true);
  str(8, 'WAVE');
  str(12, 'fmt ');
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, 1, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  str(36, 'data');
  v.setUint32(40, n * 2, true);
  for (let i = 0; i < n; i++) {
    const t = i / rate;
    const env = Math.sin((Math.PI * t) / seconds) * (0.5 + 0.5 * Math.sin(t * 9));
    v.setInt16(44 + i * 2, Math.round(Math.sin(2 * Math.PI * 220 * t) * env * 2500), true);
  }
  const url = URL.createObjectURL(new Blob([buf], { type: 'audio/wav' }));
  toneUrls.set(seconds, url);
  return url;
}

export async function mockInvoke(cmd: string, args: Record<string, unknown> = {}): Promise<unknown> {
  switch (cmd) {
    case 'whisper_status':
      return {
        built: true,
        modelsDir: 'C:\\Users\\you\\AppData\\Roaming\\dev.relay.assistant\\models',
        downloading: null,
        models: [
          { name: 'tiny', label: 'Tiny (fastest, rough)', sizeMb: 78, englishOnly: false, downloaded: false },
          { name: 'base', label: 'Base (fast)', sizeMb: 148, englishOnly: false, downloaded: false },
          { name: 'small-q5_1', label: 'Small, quantized (default)', sizeMb: 190, englishOnly: false, downloaded: true },
          { name: 'large-v3-turbo-q5_0', label: 'Large v3 turbo, quantized (best quality)', sizeMb: 574, englishOnly: false, downloaded: false },
        ],
      };
    case 'download_whisper_model':
    case 'cancel_whisper_download':
      return undefined;
    case 'get_settings':
      return settings;
    case 'save_settings':
      // The backend keeps its own live mode.
      settings = { ...(args.settings as Settings), liveMode: settings.liveMode };
      return null;
    case 'set_live_mode':
      settings = { ...settings, liveMode: args.mode as Mode };
      return null;
    case 'test_llm': {
      const choice = args.choice as ModelChoice;
      await new Promise((r) => setTimeout(r, 400));
      const src = mockCatalog(settings).sources.find((x) => x.source === choice.source);
      if (!src?.available) throw src?.note ?? 'This source is not available.';
      return `Connected (${choice.model})`;
    }
    case 'list_models':
      await new Promise((r) => setTimeout(r, 250));
      return mockCatalog((args.settings as Settings | null) ?? settings);
    case 'set_api_key': {
      const p = String(args.provider);
      if (String(args.key)) keys.add(p);
      else keys.delete(p);
      return null;
    }
    case 'has_api_key':
      return keys.has(String(args.provider));
    case 'list_audio_devices':
      return devices;
    case 'start_session':
      clearDictationTimers();
      setState({ mode: args.mode as Mode, recording: true, sessionStartedAt: now() });
      setPhase('listening');
      startMockSession();
      return null;
    case 'stop_session': {
      const wasDictation = state.mode === 'dictation';
      endMockCapture();
      setState({ mode: 'idle', recording: false, sessionStartedAt: null });
      if (wasDictation) walkMockDictation();
      else setPhase('idle');
      return null;
    }
    case 'cancel_dictation':
      if (state.mode === 'dictation') {
        endMockCapture();
        clearDictationTimers();
        setState({ mode: 'idle', recording: false, sessionStartedAt: null });
        setPhase('idle');
      }
      return null;
    case 'skip_cleanup':
      if (state.phase === 'cleaning' && state.phaseDetail === 'ai') {
        finishMockDictation('AI cleanup was skipped, so Basic was used.');
      }
      return null;
    case 'dismiss_dictation_error':
      if (state.phase === 'error') setPhase('idle');
      return null;
    case 'get_state':
      return state;
    case 'get_transcript':
      return transcript;
    case 'clear_transcript':
      transcript.segments = [];
      transcript.translations = [];
      return null;
    case 'run_edit': {
      const raw = String(args.text);
      const cleaned = raw.replace(/\b(um|uh)\b[,]?\s*/gi, '').trim().replace(/^./, (c) => c.toUpperCase());
      const entry = mockEntry('edit', raw, cleaned, {
        cleanup: args.cleanup as CleanupMode,
        instructions: String(args.instructions ?? ''),
        note: args.cleanup === 'ai' ? 'AI cleanup failed, so Basic was used. Mock backend has no model.' : null,
      });
      history.unshift(entry);
      emit('history-changed', null);
      return entry;
    }
    case 'history_audio_url':
      // A short tone stands in for the recording, so the player can be tried in the browser.
      return mockToneUrl(String(args.file).includes('transcript') ? 6 : 3);
    case 'history_list':
      return [...history];
    case 'history_delete':
      history = history.filter((e) => e.id !== args.id);
      emit('history-changed', null);
      return null;
    case 'history_clear':
      history = [];
      emit('history-changed', null);
      return null;
    case 'insert_text':
      if (args.historyId) {
        history = history.map((e) => (e.id === args.historyId ? { ...e, inserted: true } : e));
        emit('history-changed', null);
      }
      return null;
    case 'export_transcript':
      return transcript.segments.map((s) => `${s.speaker ?? ''}: ${s.text}`).join('\n');
    case 'save_export':
      return `C:\\Users\\demo\\Documents\\Relay\\transcript.${String(args.format) === 'markdown' ? 'md' : String(args.format)}`;
    case 'generate_notes': {
      const notes = { ...sampleNotes, updatedAt: now() };
      emit('notes-updated', notes);
      return notes;
    }
    case 'agent_create_task':
      if (args.run === false) return makeDraft(String(args.title), String(args.detail));
      return makeTask(String(args.title), String(args.detail));
    case 'agent_run_task': {
      const i = tasks.findIndex((x) => x.id === args.id && x.status === 'draft');
      if (i < 0) throw 'Only a draft can be started.';
      const [d] = tasks.splice(i, 1);
      emit('agent-task-removed', { id: d.id });
      makeTask(d.title, d.detail, d.source);
      return null;
    }
    case 'agent_update_task': {
      const t = tasks.find((x) => x.id === args.id && x.status === 'draft');
      if (!t) throw 'Only a draft can be edited.';
      t.title = String(args.title);
      t.detail = String(args.detail);
      t.updatedAt = now();
      emit('agent-task', { ...t });
      return { ...t };
    }
    case 'agent_delete_task': {
      const i = tasks.findIndex((x) => x.id === args.id);
      if (i >= 0) tasks.splice(i, 1);
      emit('agent-task-removed', { id: args.id });
      return null;
    }
    case 'agent_plan_tasks':
      await new Promise((r) => setTimeout(r, 900));
      return mockPlan(args.ids as string[]);
    case 'agent_apply_plan': {
      const ids = args.ids as string[];
      const plan = args.plan as TaskPlan;
      for (const id of ids) {
        const i = tasks.findIndex((x) => x.id === id);
        if (i >= 0) tasks.splice(i, 1);
        emit('agent-task-removed', { id });
      }
      return plan.goals.map((g) => makeDraft(g.title, g.detail, 'agent'));
    }
    case 'agent_list_tasks':
      return tasks;
    case 'agent_pending_approvals':
      return approvals;
    case 'agent_cancel_task': {
      const t = tasks.find((x) => x.id === args.id);
      if (t) {
        t.status = 'cancelled';
        t.updatedAt = now();
        emit('agent-task', { ...t });
      }
      return null;
    }
    case 'agent_resolve_approval': {
      const idx = approvals.findIndex((a) => a.id === args.id);
      if (idx >= 0) {
        const [a] = approvals.splice(idx, 1);
        emit('agent-approval-resolved', { id: a.id });
        const t = tasks.find((x) => x.id === a.taskId);
        if (t) {
          t.status = args.approve ? 'done' : 'failed';
          t.result = args.approve ? 'Removed the old installer.' : 'Denied by the user.';
          t.updatedAt = now();
          t.log.push({ at: now(), kind: 'approval', message: args.approve ? 'Approved.' : 'Denied.' });
          emit('agent-task', { ...t });
        }
        setState({ pendingApprovals: approvals.length });
      }
      return null;
    }
    case 'agent_reply_escalation':
      return null;
    case 'show_main':
    case 'toggle_overlay':
      return null;
    default:
      throw new Error(`Mock backend has no command named ${cmd}`);
  }
}

// Seed some sample data so the Agent and Notes tabs have content in a plain browser.
export function seedMock(): void {
  const t: Task = {
    id: 'seed-1',
    title: 'Summarize the open window',
    detail: 'Read the active window title and the clipboard, then write a two line summary.',
    status: 'done',
    source: 'user',
    createdAt: now(),
    updatedAt: now(),
    result: 'The active window is a code editor. The clipboard is empty.',
    log: [
      { at: now(), kind: 'info', message: 'Task created.' },
      { at: now(), kind: 'tool_call', message: 'get_active_window {}' },
      { at: now(), kind: 'tool_result', message: 'Visual Studio Code (code.exe)' },
      { at: now(), kind: 'assistant', message: 'Done.' },
    ],
  };
  tasks.push(t);
  const drafts: Array<[string, string, string]> = [
    ['fix the installer tests and send the build to QA', '', 'meeting'],
    ['Update the changelog', 'Owner: unassigned. Created from meeting notes.', 'meeting'],
    ['update the changelog.', '', 'voice'],
    ['look into why login is slow', '', 'user'],
  ];
  drafts.forEach(([title, detail, source], i) => {
    const at = new Date(Date.now() - (i + 1) * 60_000).toISOString();
    tasks.push({ id: `seed-d${i}`, title, detail, status: 'draft', source, createdAt: at, updatedAt: at, result: null, log: [] });
  });
}

