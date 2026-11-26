import type { Mode, PermissionLevel, TaskStatus } from './types';

export function formatMs(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const mm = h > 0 ? String(m).padStart(2, '0') : String(m);
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

export function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

export function formatDateTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
}

export const MODE_LABEL: Record<Exclude<Mode, 'idle'>, string> = {
  transcribe: 'Transcribe',
  dictation: 'Dictate',
  translate: 'Translate',
  meeting: 'Meeting',
  agent: 'Agent',
};

export const SESSION_MODES: Array<Exclude<Mode, 'idle'>> = ['transcribe', 'dictation', 'translate', 'meeting', 'agent'];

/** Modes the Live hotkey can start (dictation has its own hotkey). */
export type LiveMode = Exclude<Mode, 'idle' | 'dictation'>;

export function loadLiveMode(): LiveMode {
  try {
    const v = localStorage.getItem('relay.liveMode');
    if (v === 'transcribe' || v === 'translate' || v === 'meeting' || v === 'agent') return v;
  } catch {
    // storage unavailable
  }
  return 'transcribe';
}

export function saveLiveMode(m: Exclude<Mode, 'idle'>) {
  if (m === 'dictation') return;
  try {
    localStorage.setItem('relay.liveMode', m);
  } catch {
    // storage unavailable
  }
}

export const LEVEL_LABEL: Record<PermissionLevel, string> = {
  readOnly: 'Read only',
  localReversible: 'Local, reversible',
  external: 'External',
  destructive: 'Destructive',
};

export const LEVEL_HINT: Record<PermissionLevel, string> = {
  readOnly: 'Reads state only. Changes nothing.',
  localReversible: 'Local change that is easy to undo.',
  external: 'Acts on other apps or the shell.',
  destructive: 'Deletes data or writes outside the Relay folder.',
};

export const STATUS_LABEL: Record<TaskStatus, string> = {
  draft: 'Draft',
  pending: 'Pending',
  running: 'Running',
  waitingApproval: 'Waiting for approval',
  escalated: 'Needs your answer',
  done: 'Done',
  failed: 'Failed',
  cancelled: 'Cancelled',
};

export const TERMINAL: TaskStatus[] = ['done', 'failed', 'cancelled'];

export function prettyJson(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2) ?? 'null';
  } catch {
    return String(v);
  }
}

export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}
