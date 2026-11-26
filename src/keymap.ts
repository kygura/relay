// Single source of truth for keyboard shortcuts. Every hint in the UI, the command palette,
// the shortcut sheet and every keydown handler reads from this table, so a rebind or a new
// action shows up everywhere at once.
//
// Key strings use "+" between parts, modifiers first in the order Ctrl, Alt, Shift:
// "Ctrl+Shift+Enter", "Alt+1", "F6", "/". Global hotkeys come from settings in Tauri's
// accelerator format ("CommandOrControl+Shift+D") and are converted with fromAccelerator.

import { useSyncExternalStore, type KeyboardEvent as ReactKeyboardEvent } from 'react';

type AnyKeyEvent = KeyboardEvent | ReactKeyboardEvent;

export type Scope = 'global' | 'app' | 'live' | 'editor' | 'history' | 'palette';

export type ActionId =
  | 'live-toggle'
  | 'dictate'
  | 'go-live'
  | 'go-history'
  | 'go-notes'
  | 'go-agent'
  | 'go-settings'
  | 'palette'
  | 'shortcuts'
  | 'region-next'
  | 'region-prev'
  | 'mode-transcribe'
  | 'mode-dictation'
  | 'mode-translate'
  | 'mode-meeting'
  | 'mode-agent'
  | 'send-to-editor'
  | 'run-edit'
  | 'insert-output'
  | 'copy-output'
  | 'clear-editor'
  | 'cleanup-off'
  | 'cleanup-basic'
  | 'cleanup-ai'
  | 'focus-input'
  | 'focus-instructions'
  | 'focus-output'
  | 'cancel'
  | 'history-search'
  | 'history-open'
  | 'history-copy'
  | 'history-toggle-original'
  | 'history-delete'
  | 'history-undo';

export interface Shortcut {
  id: ActionId;
  /** One or more key strings; the first is shown in hints. */
  keys: string[];
  label: string;
  scope: Scope;
  /** Listed in the command palette. */
  palette: boolean;
}

const TABLE: Shortcut[] = [
  // Global (system-wide, from settings; listed so hints and the sheet can show them)
  { id: 'live-toggle', keys: ['Ctrl+Shift+Space'], label: 'Start or stop Live', scope: 'global', palette: true },
  { id: 'dictate', keys: ['Ctrl+Shift+D'], label: 'Dictate into any app', scope: 'global', palette: false },
  // App-wide
  { id: 'go-live', keys: ['Ctrl+1'], label: 'Go to Live', scope: 'app', palette: true },
  { id: 'go-history', keys: ['Ctrl+2'], label: 'Go to History', scope: 'app', palette: true },
  { id: 'go-notes', keys: ['Ctrl+3'], label: 'Go to Notes', scope: 'app', palette: true },
  { id: 'go-agent', keys: ['Ctrl+4'], label: 'Go to Agent', scope: 'app', palette: true },
  { id: 'go-settings', keys: ['Ctrl+5', 'Ctrl+,'], label: 'Go to Settings', scope: 'app', palette: true },
  { id: 'palette', keys: ['Ctrl+K', 'Ctrl+Shift+P'], label: 'Command palette', scope: 'app', palette: false },
  { id: 'shortcuts', keys: ['Ctrl+/', 'F1'], label: 'Keyboard shortcuts', scope: 'app', palette: true },
  { id: 'region-next', keys: ['F6'], label: 'Next region', scope: 'app', palette: false },
  { id: 'region-prev', keys: ['Shift+F6'], label: 'Previous region', scope: 'app', palette: false },
  { id: 'cancel', keys: ['Escape'], label: 'Cancel or close', scope: 'app', palette: false },
  // Live
  { id: 'mode-transcribe', keys: ['Alt+1'], label: 'Mode: Transcribe', scope: 'live', palette: true },
  { id: 'mode-dictation', keys: ['Alt+2'], label: 'Mode: Dictate', scope: 'live', palette: true },
  { id: 'mode-translate', keys: ['Alt+3'], label: 'Mode: Translate', scope: 'live', palette: true },
  { id: 'mode-meeting', keys: ['Alt+4'], label: 'Mode: Meeting', scope: 'live', palette: true },
  { id: 'mode-agent', keys: ['Alt+5'], label: 'Mode: Agent', scope: 'live', palette: true },
  { id: 'send-to-editor', keys: ['Ctrl+E'], label: 'Send transcript to editor', scope: 'live', palette: true },
  // Editor
  { id: 'run-edit', keys: ['Ctrl+Enter'], label: 'Run', scope: 'editor', palette: true },
  { id: 'insert-output', keys: ['Ctrl+Shift+Enter'], label: 'Insert output', scope: 'editor', palette: true },
  { id: 'copy-output', keys: ['Ctrl+Shift+C'], label: 'Copy output', scope: 'editor', palette: true },
  { id: 'clear-editor', keys: ['Ctrl+Shift+Backspace'], label: 'Clear editor', scope: 'editor', palette: true },
  { id: 'cleanup-off', keys: ['Alt+O'], label: 'Cleanup: Off', scope: 'editor', palette: true },
  { id: 'cleanup-basic', keys: ['Alt+B'], label: 'Cleanup: Basic', scope: 'editor', palette: true },
  { id: 'cleanup-ai', keys: ['Alt+A'], label: 'Cleanup: AI', scope: 'editor', palette: true },
  { id: 'focus-input', keys: ['Ctrl+Alt+1'], label: 'Focus input', scope: 'editor', palette: true },
  { id: 'focus-instructions', keys: ['Ctrl+Alt+2'], label: 'Focus instructions', scope: 'editor', palette: true },
  { id: 'focus-output', keys: ['Ctrl+Alt+3'], label: 'Focus output', scope: 'editor', palette: true },
  // History (row-scoped keys apply when a history row has focus)
  { id: 'history-search', keys: ['Ctrl+F', '/'], label: 'Search history', scope: 'history', palette: true },
  { id: 'history-open', keys: ['Enter'], label: 'Open in editor', scope: 'history', palette: false },
  { id: 'history-copy', keys: ['Ctrl+C'], label: 'Copy result', scope: 'history', palette: false },
  { id: 'history-toggle-original', keys: ['Alt+Enter'], label: 'Show original or result', scope: 'history', palette: false },
  { id: 'history-delete', keys: ['Delete'], label: 'Delete entry', scope: 'history', palette: false },
  { id: 'history-undo', keys: ['Ctrl+Z'], label: 'Undo delete', scope: 'history', palette: false },
];

// ---------- Global hotkeys from settings ----------

let globalKeys: Partial<Record<ActionId, string>> = {};
let version = 0;
const subs = new Set<() => void>();

/** Convert Tauri's "CommandOrControl+Shift+D" to "Ctrl+Shift+D". */
export function fromAccelerator(acc: string): string {
  return acc
    .split('+')
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => {
      const l = p.toLowerCase();
      if (l === 'commandorcontrol' || l === 'cmdorctrl' || l === 'control' || l === 'ctrl') return 'Ctrl';
      if (l === 'option') return 'Alt';
      if (l.length === 1) return p.toUpperCase();
      return p[0].toUpperCase() + p.slice(1);
    })
    .join('+');
}

/** Convert "Ctrl+Shift+D" to Tauri's "CommandOrControl+Shift+D". */
export function toAccelerator(keys: string): string {
  return keys.replace(/^Ctrl(?=\+|$)/, 'CommandOrControl');
}

/** Call after loading or saving settings so hints follow rebinds. */
export function setGlobalHotkeys(toggle: string, dictate: string): void {
  globalKeys = { 'live-toggle': fromAccelerator(toggle), dictate: fromAccelerator(dictate) };
  version++;
  subs.forEach((f) => f());
}

/** Re-render when global hotkeys change. */
export function useKeymapVersion(): number {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => version,
  );
}

// ---------- Lookups ----------

export function shortcut(id: ActionId): Shortcut {
  const s = TABLE.find((x) => x.id === id);
  if (!s) throw new Error(`unknown shortcut ${id}`);
  const g = globalKeys[id];
  return g ? { ...s, keys: [g] } : s;
}

export function allShortcuts(): Shortcut[] {
  return TABLE.map((s) => shortcut(s.id));
}

/** Parts of the primary key string, for <Kbd keys={...}>: ["Ctrl", "Shift", "Enter"]. */
export function keyParts(id: ActionId): string[] {
  return shortcut(id).keys[0].split('+');
}

/** Value for aria-keyshortcuts ("Control+Shift+Enter"). */
export function ariaKeys(id: ActionId): string {
  return shortcut(id)
    .keys.map((k) => k.replace(/\bCtrl\b/g, 'Control').replace(/\bEsc\b/g, 'Escape'))
    .join(' ');
}

/** Duplicate key strings within overlapping scopes; used by tests. */
export function duplicateKeys(): string[] {
  const seen = new Map<string, Scope>();
  const dups: string[] = [];
  const overlaps = (a: Scope, b: Scope) => a === b || a === 'app' || b === 'app' || a === 'global' || b === 'global';
  for (const s of TABLE) {
    for (const k of s.keys) {
      const prev = seen.get(k);
      if (prev && overlaps(prev, s.scope)) dups.push(k);
      seen.set(k, s.scope);
    }
  }
  return dups;
}

// ---------- Matching ----------

/** Normalize a keyboard event to a key string such as "Ctrl+Shift+Enter". */
export function eventKeys(e: AnyKeyEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push('Ctrl');
  if (e.altKey) parts.push('Alt');
  if (e.shiftKey) parts.push('Shift');
  let k = e.key;
  // Use the physical key for digits and letters so Shift and Alt do not change them.
  if (/^Digit\d$/.test(e.code)) k = e.code.slice(5);
  else if (/^Key[A-Z]$/.test(e.code)) k = e.code.slice(3);
  else if (k === ' ') k = 'Space';
  else if (k === 'Esc') k = 'Escape';
  else if (k.length === 1) k = k.toUpperCase();
  if (!['Control', 'Alt', 'Shift', 'Meta'].includes(k)) parts.push(k);
  return parts.join('+');
}

export function matches(e: AnyKeyEvent, id: ActionId): boolean {
  const k = eventKeys(e);
  return shortcut(id).keys.some((s) => s.toUpperCase() === k.toUpperCase());
}

/** True when focus is in a text field, where single-key shortcuts must not fire. */
export function inTextField(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  const tag = el.tagName;
  return tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable || (tag === 'INPUT' && !['checkbox', 'radio', 'button'].includes((el as HTMLInputElement).type));
}

// ---------- Runnable actions ----------
// Components register handlers for the actions they own while mounted (for example the
// editor registers run-edit). The app-level key listener and the command palette call run().

type Handler = () => void;
const handlers = new Map<ActionId, Handler>();

/** Register a handler; returns an unregister function for useEffect cleanup. */
export function registerAction(id: ActionId, run: Handler): () => void {
  handlers.set(id, run);
  return () => {
    if (handlers.get(id) === run) handlers.delete(id);
  };
}

export function canRun(id: ActionId): boolean {
  return handlers.has(id);
}

export function runAction(id: ActionId): boolean {
  const h = handlers.get(id);
  if (!h) return false;
  h();
  return true;
}
