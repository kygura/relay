import { useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import * as api from './api';
import { IconAlert, IconClose } from './icons';
import { dismissToast, reportError, selApp, selToasts, useRelay } from './store';
import type { Mode } from './types';
import { loadLiveMode, MODE_LABEL, saveLiveMode, SESSION_MODES } from './util';
import {
  allShortcuts,
  ariaKeys,
  eventKeys,
  fromAccelerator,
  keyParts,
  registerAction,
  toAccelerator,
  useKeymapVersion,
  type ActionId,
} from './keymap';

type SessionMode = Exclude<Mode, 'idle'>;

/** Selected mode plus start and stop handlers shared by the overlay and the Live tab. */
export function useSessionControl() {
  const app = useRelay(selApp);
  const [picked, setPicked] = useState<SessionMode>(loadLiveMode);
  const [busy, setBusy] = useState(false);
  const active: SessionMode = app.mode !== 'idle' ? app.mode : picked;

  const run = useCallback(async (fn: () => Promise<void>, scope: string) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      reportError(scope, e);
    } finally {
      setBusy(false);
    }
  }, []);

  const pick = useCallback(
    (mode: SessionMode) => {
      setPicked(mode);
      saveLiveMode(mode);
      // The Live hotkey starts this mode, also after a restart.
      if (mode !== 'dictation') api.setLiveMode(mode).catch((e) => reportError('set_live_mode', e));
      if (app.recording && mode !== app.mode) {
        void run(async () => {
          await api.stopSession();
          await api.startSession(mode);
        }, 'start_session');
      }
    },
    [app.recording, app.mode, run],
  );

  const toggle = useCallback(() => {
    if (app.recording) void run(() => api.stopSession(), 'stop_session');
    else void run(() => api.startSession(active), 'start_session');
  }, [app.recording, active, run]);

  // Expose the mode and toggle actions to the app-level key listener and the command palette
  // while a surface that owns the session controls is mounted (the Live tab or the overlay).
  useEffect(() => {
    const offs = SESSION_MODES.map((m) => registerAction(`mode-${m}` as ActionId, () => pick(m)));
    offs.push(registerAction('live-toggle', toggle));
    return () => offs.forEach((off) => off());
  }, [pick, toggle]);

  return { app, active, pick, toggle, busy };
}

/** Elapsed time since `startedAt` (RFC 3339), ticking once per second while running. */
export function useElapsed(startedAt: string | null | undefined, running: boolean): string {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [running]);
  if (!running || !startedAt) return '0:00';
  const start = new Date(startedAt).getTime();
  if (Number.isNaN(start)) return '0:00';
  const total = Math.max(0, Math.floor((now - start) / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = String(total % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

export function ModePills({ active, onPick }: { active: SessionMode; onPick: (m: SessionMode) => void }) {
  const index = SESSION_MODES.indexOf(active);
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    const last = SESSION_MODES.length - 1;
    let n = index;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') n = index >= last ? 0 : index + 1;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') n = index <= 0 ? last : index - 1;
    else if (e.key === 'Home') n = 0;
    else if (e.key === 'End') n = last;
    else return;
    e.preventDefault();
    onPick(SESSION_MODES[n]);
    refs.current[n]?.focus();
  };
  return (
    <div
      className="segmented"
      role="radiogroup"
      aria-label="Mode"
      onKeyDown={onKeyDown}
      style={{ ['--seg-count' as string]: SESSION_MODES.length, ['--seg-index' as string]: index }}
    >
      <span className="segmented-thumb" aria-hidden="true" />
      {SESSION_MODES.map((m, i) => (
        <button
          key={m}
          ref={(el) => {
            refs.current[i] = el;
          }}
          type="button"
          role="radio"
          className={`segmented-opt${active === m ? ' on' : ''}`}
          aria-checked={active === m}
          aria-keyshortcuts={ariaKeys(`mode-${m}` as ActionId)}
          tabIndex={active === m ? 0 : -1}
          onClick={() => onPick(m)}
        >
          {MODE_LABEL[m]}
        </button>
      ))}
    </div>
  );
}

/** Relay mark: five rounded bars in a tile, drawn in the inverse colours of the theme. */
export function Logo({ size = 28 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true" focusable="false" className="logo">
      <rect x="0" y="0" width="32" height="32" rx="9" fill="var(--inverse)" />
      <g stroke="var(--on-inverse)" strokeWidth="2.4" strokeLinecap="round">
        <path d="M9 13.5v5" />
        <path d="M12.5 10.5v11" />
        <path d="M16 8v16" />
        <path d="M19.5 10.5v11" />
        <path d="M23 13.5v5" />
      </g>
    </svg>
  );
}

/** Key hint. Pass `action` to read the keys from the keymap (follows rebinds), or `keys` directly. */
export function Kbd({ keys, action }: { keys?: string[]; action?: ActionId }) {
  useKeymapVersion();
  const parts = action ? keyParts(action) : (keys ?? []);
  return (
    <span className="kbd-group">
      {parts.map((k) => (
        <kbd key={k}>{k}</kbd>
      ))}
    </span>
  );
}

const ACCEL_KEY: Record<string, string> = {
  ArrowUp: 'Up',
  ArrowDown: 'Down',
  ArrowLeft: 'Left',
  ArrowRight: 'Right',
  Space: 'Space',
  Enter: 'Enter',
  Delete: 'Delete',
  Insert: 'Insert',
  Home: 'Home',
  End: 'End',
  PageUp: 'PageUp',
  PageDown: 'PageDown',
};

/** Convert a captured key string ("Ctrl+Shift+D") to a Tauri accelerator, or an error message. */
function captureToAccelerator(keys: string): { acc: string } | { error: string } {
  const parts = keys.split('+');
  const main = parts[parts.length - 1];
  const mods = parts.slice(0, -1);
  if (!mods.includes('Ctrl') && !mods.includes('Alt')) return { error: 'Use Ctrl or Alt in the combination.' };
  let key = main;
  if (ACCEL_KEY[main]) key = ACCEL_KEY[main];
  else if (!(main.length === 1 || /^F([1-9]|1\d|2[0-4])$/.test(main))) return { error: 'That key cannot be used.' };
  return { acc: toAccelerator([...mods, key].join('+')) };
}

/** Capture field for a global hotkey. Value is in Tauri accelerator format. */
export function HotkeyCapture({
  id,
  value,
  defaultValue,
  otherValue,
  onChange,
  describedBy,
}: {
  id: string;
  value: string;
  defaultValue: string;
  otherValue: string;
  onChange: (accelerator: string) => void;
  describedBy?: string;
}) {
  useKeymapVersion();
  const [capturing, setCapturing] = useState(false);
  const [error, setError] = useState('');
  const current = fromAccelerator(value);
  const ci = current.toUpperCase();
  let warning = '';
  if (ci === fromAccelerator(otherValue).toUpperCase()) {
    warning = 'The other global hotkey uses the same keys.';
  } else {
    const clash = allShortcuts().find((s) => s.scope !== 'global' && s.keys.some((k) => k.toUpperCase() === ci));
    if (clash) warning = `Also used inside Relay for "${clash.label}".`;
  }

  const onKeyDown = (e: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (e.key === 'Tab' && !e.ctrlKey && !e.altKey) return;
    if (e.key === 'Control' || e.key === 'Alt' || e.key === 'Shift' || e.key === 'Meta') {
      e.preventDefault();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') {
      setError('');
      setCapturing(false);
      return;
    }
    if (e.key === 'Backspace' && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      setError('');
      onChange(defaultValue);
      return;
    }
    const res = captureToAccelerator(eventKeys(e));
    if ('error' in res) {
      setError(res.error);
      return;
    }
    setError('');
    setCapturing(false);
    onChange(res.acc);
  };

  return (
    <div className="hotkey-field">
      <button
        id={id}
        type="button"
        className={`hotkey-capture${capturing ? ' capturing' : ''}${error ? ' invalid' : ''}`}
        aria-describedby={`${id}-help${describedBy ? ` ${describedBy}` : ''}`}
        aria-invalid={error ? true : undefined}
        onFocus={() => setCapturing(true)}
        onBlur={() => {
          setCapturing(false);
          setError('');
        }}
        onClick={() => setCapturing(true)}
        onKeyDown={onKeyDown}
      >
        {capturing ? <span className="hotkey-prompt">Press a new combination</span> : null}
        <Kbd keys={current.split('+')} />
      </button>
      <div id={`${id}-help`} className="hint">
        {capturing ? 'Escape cancels. Backspace restores the default.' : 'Focus the field, then press the new combination.'}
      </div>
      {error ? (
        <div className="hotkey-msg error" role="alert">
          {error}
        </div>
      ) : null}
      {!error && warning ? (
        <div className="hotkey-msg warn" role="status">
          {warning}
        </div>
      ) : null}
    </div>
  );
}

export function PageHeader({
  title,
  subtitle,
  actions,
}: {
  title: string;
  subtitle?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="page-head">
      <div className="page-head-text">
        <h1>{title}</h1>
        {subtitle ? <p className="page-sub">{subtitle}</p> : null}
      </div>
      {actions ? <div className="page-actions">{actions}</div> : null}
    </header>
  );
}

export function EmptyState({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="empty-state">
      <div className="empty-title">{title}</div>
      {children ? <div className="empty-body">{children}</div> : null}
    </div>
  );
}

function toastText(t: { kind: string; scope: string; message: string }): string {
  const head = t.kind === 'error' ? `Error in ${t.scope}` : t.scope;
  return t.message ? `: ${t.message}` : head;
}

/** One line by default, two at most. The full text is in the title. Errors stay until dismissed. */
export function Toasts() {
  const toasts = useRelay(selToasts);
  if (toasts.length === 0) return null;
  return (
    <div className="toasts" role="region" aria-label="Notifications">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.kind}`} role={t.kind === 'error' ? 'alert' : 'status'}>
          <span className="toast-icon">{t.kind === 'error' ? <IconAlert size={16} /> : <span className="toast-dot" />}</span>
          <div className="toast-body" title={toastText(t)}>
            <span className="toast-scope">{t.kind === 'error' ? `Error in ${t.scope}` : t.scope}</span>
            {t.message ? <span className="toast-msg"> {t.message}</span> : null}
          </div>
          <button type="button" className="icon-btn" aria-label="Dismiss notification" onClick={() => dismissToast(t.id)}>
            <IconClose size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
