import { useEffect, useRef, useState } from 'react';
import * as api from '../api';
import { Kbd, Toasts, useElapsed } from '../components';
import { IconAlert, IconCheck, IconClose, IconEdit, IconHide, IconMic, IconOpen, IconShield } from '../icons';
import { setGlobalHotkeys } from '../keymap';
import { reportError, selDictation, selFinals, selInterim, selTranslations, useRelay } from '../store';
import type { AppState, DictationResult, Mode, TranscriptSegment } from '../types';
import './pill.css';
import { useLevelRef, Waveform } from './Waveform';

/** How long the done state stays on screen. The backend keeps `done` until the next start. */
const DONE_MS = 1500;
/** Transcribing shows elapsed seconds after this long. */
const SHOW_SECONDS_MS = 3000;
/** Cleaning mentions Escape after this long. */
const SHOW_SKIP_MS = 8000;

type View = 'idle' | 'listening' | 'transcribing' | 'cleaning' | 'done' | 'error';

const LIVE_LABEL: Partial<Record<Mode, string>> = {
  transcribe: 'Live captions',
  translate: 'Translate',
  meeting: 'Meeting',
  agent: 'Agent',
};

function safe(fn: () => Promise<void>, scope: string) {
  fn().catch((e) => reportError(scope, e));
}

/** Which state the card shows. */
function viewOf(app: AppState, now: number): View {
  switch (app.phase) {
    case 'transcribing':
    case 'cleaning':
    case 'error':
    case 'listening':
      return app.phase;
    case 'done': {
      const since = app.phaseSince ? Date.parse(app.phaseSince) : NaN;
      if (!Number.isNaN(since) && now - since < DONE_MS) return 'done';
      return app.recording ? 'listening' : 'idle';
    }
    default:
      return app.recording ? 'listening' : 'idle';
  }
}

/** Milliseconds since the current phase began. */
function phaseAge(app: AppState, now: number): number {
  const since = app.phaseSince ? Date.parse(app.phaseSince) : NaN;
  return Number.isNaN(since) ? 0 : Math.max(0, now - since);
}

/** Current time, refreshed every 250 ms while `active`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const t = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(t);
  }, [active]);
  return now;
}

function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  useEffect(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const on = () => setReduced(mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, []);
  return reduced;
}

function titleCase(s: string): string {
  return s ? s[0].toUpperCase() + s.slice(1) : s;
}

function doneText(app: AppState, dictation: DictationResult | null): string {
  if (!dictation?.inserted) return 'Saved to History';
  return app.phaseDetail ? `Inserted into ${titleCase(app.phaseDetail)}` : 'Inserted';
}

/**
 * Ids of transcript segments that existed when the current session began. Segments with
 * other ids belong to this session. Ids are used instead of a length so a re-sorted
 * transcript cannot leak older text into the card.
 */
function useSessionBaseline(app: AppState, finals: TranscriptSegment[]): Set<string> {
  const ref = useRef<{ started: string | null; listen: string | null; ids: Set<string> }>({
    started: null,
    listen: null,
    ids: new Set(finals.map((s) => s.id)),
  });
  const r = ref.current;
  const listen = app.phase === 'listening' ? app.phaseSince : r.listen;
  const started = app.sessionStartedAt ?? r.started;
  if (started !== r.started || listen !== r.listen) {
    r.started = started;
    r.listen = listen;
    r.ids = new Set(finals.map((s) => s.id));
  }
  return r.ids;
}

interface Line {
  seg: TranscriptSegment;
  interim: boolean;
}

function EndText({ text }: { text: string }) {
  // direction rtl puts the ellipsis at the start; the inner ltr span keeps punctuation in place.
  return (
    <span className="ov-end">
      <bdi className="ov-end-in">{text}</bdi>
    </span>
  );
}

function Caption({ line, old, withTranslation }: { line: Line; old: boolean; withTranslation: boolean }) {
  const tr = useRelay((s) => selTranslations(s)[line.seg.id]);
  const first = withTranslation && tr && tr.length > 0 ? tr[0] : null;
  return (
    <div
      className={`lv-line${line.interim ? ' interim' : ''}${old ? ' old' : ''}`}
      aria-hidden={line.interim ? true : undefined}
    >
      <EndText text={line.seg.text} />
      {first ? (
        <div className="lv-tr" lang={first.targetLang}>
          <span className="lv-lang">{first.targetLang}</span>
          <EndText text={first.text} />
        </div>
      ) : null}
    </div>
  );
}

function Hint({ action, label }: { action: 'dictate' | 'live-toggle' | 'cancel'; label: string }) {
  return (
    <span className="ov-hint">
      <Kbd action={action} />
      <span>{label}</span>
    </span>
  );
}

export function Overlay() {
  const app = useRelay((s) => s.app);
  const finals = useRelay(selFinals);
  const interim = useRelay(selInterim);
  const dictation = useRelay(selDictation);
  const elapsed = useElapsed(app.sessionStartedAt, app.recording);
  const reducedMotion = useReducedMotion();
  const ticking = ['listening', 'transcribing', 'cleaning', 'done'].includes(app.phase);
  const now = useNow(ticking);
  const view = viewOf(app, now);
  // Live modes keep their own mode while they run. Dictation clears the mode when the
  // recording stops, so the finishing phases belong to the pill.
  const isLive = app.mode !== 'dictation' && app.mode !== 'idle';
  const isDictation = !isLive;
  const levelRef = useLevelRef(isLive);
  const baseline = useSessionBaseline(app, finals);

  // Hint keys follow the saved settings. Read them at start and whenever a dictation begins.
  const listenKey = app.phase === 'listening' ? app.phaseSince : null;
  useEffect(() => {
    api
      .getSettings()
      .then((s) => setGlobalHotkeys(s.hotkeyToggle, s.hotkeyDictate))
      .catch(() => {});
  }, [listenKey]);

  // The target app has the keyboard, so the backend registers Escape globally while a
  // dictation needs it. This covers the case where the overlay itself has focus.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      if (app.phase === 'listening' && app.mode === 'dictation') safe(() => api.cancelDictation(), 'cancel_dictation');
      else if (app.phase === 'cleaning' && app.phaseDetail === 'ai') safe(() => api.skipCleanup(), 'skip_cleanup');
      else if (app.phase === 'error') safe(() => api.dismissDictationError(), 'dismiss_dictation_error');
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [app.phase, app.mode, app.phaseDetail]);

  // The last two lines of this session, newest at the bottom.
  const sessionFinals = finals.filter((s) => !baseline.has(s.id));
  const interimList = Object.values(interim)
    .filter((s) => !baseline.has(s.id))
    .sort((a, b) => a.startMs - b.startMs);
  const currentInterim = interimList.length > 0 ? interimList[interimList.length - 1] : null;
  const lines: Line[] = [
    ...sessionFinals.map((seg) => ({ seg, interim: false })),
    ...(currentInterim ? [{ seg: currentInterim, interim: true }] : []),
  ].slice(-2);
  const showTranslation = app.mode === 'translate' || app.mode === 'meeting';

  const age = phaseAge(app, now);
  const seconds = Math.floor(age / 1000);
  const frozen = view === 'transcribing' || view === 'cleaning';
  const waveOff = view === 'idle' || view === 'done' || view === 'error';
  const pending = app.pendingApprovals;
  const errorText = app.phaseDetail || app.lastError || 'Something went wrong.';
  const canSkip = view === 'cleaning' && app.phaseDetail === 'ai' && age >= SHOW_SKIP_MS;

  const wave = (
    <div className={`ov-wavebox${frozen ? ' frozen' : ''}${waveOff ? ' off' : ''}`}>
      <Waveform levelRef={levelRef} frozen={frozen || waveOff} reducedMotion={reducedMotion} />
    </div>
  );

  const badge =
    pending > 0 ? (
      <button
        type="button"
        className="ov-badge"
        onClick={() => safe(() => api.showMain(), 'show_main')}
        aria-label={`${pending} pending approval${pending === 1 ? '' : 's'}. Open the main window.`}
      >
        <IconShield size={13} />
        <span>{pending}</span>
      </button>
    ) : null;

  const hideBtn =
    view === 'error' ? (
      <button
        type="button"
        className="icon-btn"
        onClick={() => safe(() => api.dismissDictationError(), 'dismiss_dictation_error')}
        aria-label="Dismiss"
        title="Dismiss (Esc)"
      >
        <IconClose size={14} />
      </button>
    ) : (
      <button
        type="button"
        className="icon-btn"
        onClick={() => safe(() => api.toggleOverlay(), 'toggle_overlay')}
        aria-label="Hide overlay"
        title="Hide overlay"
      >
        <IconHide size={15} />
      </button>
    );

  const openBtn =
    view === 'idle' ? (
      <button
        type="button"
        className="icon-btn"
        onClick={() => safe(() => api.showMain(), 'show_main')}
        aria-label="Open main window"
        title="Open main window"
      >
        <IconOpen size={15} />
      </button>
    ) : null;

  // ---- One pill for both hotkeys ----
  // Dictation and Live share the surface. The label says which one runs, the hint names the
  // key that stops it, and the bubble above shows the words: the latest phrase for dictation,
  // the last two captions (with translation) for Live.
  const liveLabel = LIVE_LABEL[app.mode] ?? 'Live';
  const target = isDictation && view === 'listening' && app.phaseDetail ? titleCase(app.phaseDetail) : null;
  let glyph: JSX.Element;
  let word: string;
  let sub: string | null = null;
  let subTitle: string | undefined;
  switch (view) {
    case 'listening':
      glyph = <IconMic size={16} />;
      word = isDictation ? 'Dictating' : liveLabel;
      sub = target ? `into ${target}` : null;
      break;
    case 'transcribing':
      glyph = <IconMic size={16} />;
      word = 'Transcribing';
      break;
    case 'cleaning':
      glyph = <IconEdit size={16} />;
      word = app.phaseDetail === 'ai' ? 'Cleaning up (AI)' : 'Cleaning up (Basic)';
      break;
    case 'done':
      glyph = <IconCheck size={17} />;
      word = doneText(app, dictation);
      sub = dictation?.note ?? null;
      subTitle = dictation?.note ?? undefined;
      break;
    case 'error':
      glyph = <IconAlert size={16} />;
      word = isDictation ? 'Dictation failed' : 'Recording failed';
      sub = errorText;
      subTitle = errorText;
      break;
    default:
      glyph = <IconMic size={16} />;
      word = 'Ready';
  }

  let time = '';
  if (view === 'listening') time = elapsed;
  else if (view === 'transcribing' && age >= SHOW_SECONDS_MS) time = `${seconds}s`;

  let hint: JSX.Element | null = null;
  if (view === 'listening') hint = isDictation ? <Hint action="dictate" label="to finish" /> : <Hint action="live-toggle" label="to stop" />;
  else if (view === 'transcribing') hint = <Hint action="cancel" label="to cancel" />;
  else if (canSkip) hint = <Hint action="cancel" label="to skip" />;
  else if (view === 'error') hint = <Hint action="cancel" label="to dismiss" />;
  else if (view === 'idle') hint = <Hint action="dictate" label="to dictate" />;

  // Bubble: dictation shows its latest phrase, then the inserted text; Live shows the
  // last two captions, the translation under the newest in Translate and Meeting.
  const lastLine = lines.length > 0 ? lines[lines.length - 1] : null;
  let bubble: JSX.Element | null = null;
  if (isLive && view === 'listening' && lines.length > 0) {
    bubble = (
      <div className="dp-bubble two" aria-live="polite">
        {lines.map((l, i) => (
          <Caption
            key={l.seg.id + (l.interim ? ':i' : '')}
            line={l}
            old={i < lines.length - 1}
            withTranslation={showTranslation && i === lines.length - 1}
          />
        ))}
      </div>
    );
  } else {
    const bubbleText =
      view === 'done' && dictation?.text ? dictation.text : view === 'listening' || frozen ? (lastLine?.seg.text ?? '') : '';
    if (bubbleText) {
      bubble = (
        <div className="dp-bubble" aria-hidden="true" title={bubbleText}>
          <EndText text={bubbleText} />
        </div>
      );
    }
  }
  const showWave = view === 'listening' || frozen;

  const surface = (
    <div className="dp-wrap">
      {bubble}
      <div className={`dp-pill ov-theme ov-v-${view}`} data-tauri-drag-region>
        <span className="dp-glyph" aria-hidden="true">
          {glyph}
        </span>
        <div className="dp-text" role={view === 'error' ? 'alert' : 'status'} data-tauri-drag-region>
          <span className="dp-word" title={word}>
            {word}
          </span>
          {sub ? (
            <span className={`dp-sub${view === 'error' ? ' err' : ''}`} title={subTitle}>
              {sub}
            </span>
          ) : null}
        </div>
        {showWave ? wave : <span className="dp-spacer" data-tauri-drag-region />}
        {time ? <span className="ov-time">{time}</span> : null}
        {hint ? <div className="ov-hints">{hint}</div> : null}
        {badge}
        {openBtn}
        {hideBtn}
        {frozen ? <span className="ov-bar" aria-hidden="true" /> : null}
      </div>
    </div>
  );

  return (
    <div className="ov-root">
      {surface}
      <Toasts />
    </div>
  );
}
