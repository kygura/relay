// The editor is the visible form of Relay's one text primitive:
//   input (dictation, transcript, typed or pasted text) -> transform (cleanup + instructions) -> output.
// Dictation fills it automatically, History can reopen any entry in it, and typed text runs through
// the same backend step (run_edit) that dictation uses.
//
// Keyboard path from typed text to inserted output: type, Ctrl+Enter (run, focus lands in the
// output), Ctrl+Shift+Enter (insert). Every action is registered with the keymap so the
// command palette and the app-level key listener can call it.

import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type RefObject } from 'react';
import { flushSync } from 'react-dom';
import * as api from '../api';
import { Kbd } from '../components';
import { IconAlert, IconMic, IconPlay } from '../icons';
import { ariaKeys, matches, registerAction, shortcut, type ActionId } from '../keymap';
import { clearEditor, patchSettings, pushToast, reportError, selEditor, setEditor, useRelay } from '../store';
import type { CleanupMode, HistoryKind } from '../types';
import { copyText } from '../util';
import { FeatureModel, SHOW_EDITOR_EVENT, useSettings } from './setup';

/** Bring the editor into view when it sits behind the setup panel. */
function reveal(): void {
  window.dispatchEvent(new Event(SHOW_EDITOR_EVENT));
}

export const CLEANUP_LABEL: Record<CleanupMode, string> = { off: 'Off', basic: 'Basic', ai: 'AI' };

export const CLEANUP_HINT: Record<CleanupMode, string> = {
  off: 'Text as transcribed, trimmed.',
  basic: 'Removes fillers and stutters, fixes punctuation and capitals. Runs on this PC.',
  ai: 'A language model follows your instructions. Falls back to Basic if it fails.',
};

export const INSTRUCTIONS_PLACEHOLDER = [
  'Describe what you want, for example:',
  'formal email with British spelling, bullet points,',
  'or translate to Spanish',
].join('\n');

const SOURCE_LABEL: Record<HistoryKind, string> = {
  dictation: 'Dictation',
  edit: 'Typed',
  transcript: 'Transcript',
};

const CLEANUP_ACTION: Record<CleanupMode, ActionId> = {
  off: 'cleanup-off',
  basic: 'cleanup-basic',
  ai: 'cleanup-ai',
};

/** Off / Basic / AI radio group. With accessKeys, each option shows its Alt key (Alt+O, Alt+B, Alt+A). */
export function CleanupSwitch({
  value,
  onChange,
  name,
  accessKeys,
}: {
  value: CleanupMode;
  onChange: (m: CleanupMode) => void;
  name: string;
  accessKeys?: boolean;
}) {
  return (
    <div className="seg-control" role="radiogroup" aria-label="Cleanup">
      {(Object.keys(CLEANUP_LABEL) as CleanupMode[]).map((m) => {
        const label = CLEANUP_LABEL[m];
        return (
          <label
            key={m}
            className={`seg-opt${value === m ? ' on' : ''}`}
            title={accessKeys ? `Alt+${label[0]}` : undefined}
          >
            <input
              type="radio"
              name={name}
              value={m}
              checked={value === m}
              onChange={() => onChange(m)}
              aria-keyshortcuts={accessKeys ? ariaKeys(CLEANUP_ACTION[m]) : undefined}
            />
            {accessKeys ? (
              <>
                <span className="ak" aria-hidden="true">
                  {label[0]}
                </span>
                <span aria-hidden="true">{label.slice(1)}</span>
                <span className="sr-only">{label}</span>
              </>
            ) : (
              label
            )}
          </label>
        );
      })}
    </div>
  );
}

function appName(app: string): string {
  return app.replace(/\.exe$/i, '');
}

/** Grow a textarea with its content between minRows and maxRows (rows of the computed line height). */
function useAutosize(ref: RefObject<HTMLTextAreaElement>, value: string, minRows: number, maxRows: number) {
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const fit = () => {
      const cs = getComputedStyle(el);
      const lh = parseFloat(cs.lineHeight) || 20;
      const pad = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom);
      const border = parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth);
      el.style.height = 'auto';
      const content = el.scrollHeight;
      const max = maxRows * lh + pad;
      const h = Math.min(Math.max(content, minRows * lh + pad), max);
      el.style.height = `${Math.ceil(h + border)}px`;
      el.style.overflowY = content > max ? 'auto' : 'hidden';
    };
    fit();
    window.addEventListener('resize', fit);
    return () => window.removeEventListener('resize', fit);
  }, [ref, value, minRows, maxRows]);
}

export function Editor() {
  const ed = useRelay(selEditor);
  const settings = useSettings();
  const [instructions, setInstructions] = useState('');
  const [cleanup, setCleanup] = useState<CleanupMode>('ai');
  const [busy, setBusy] = useState<'run' | 'insert' | 'save' | null>(null);
  const [secs, setSecs] = useState(0);
  const [justInserted, setJustInserted] = useState(false);

  const inRef = useRef<HTMLTextAreaElement>(null);
  const insRef = useRef<HTMLTextAreaElement>(null);
  const outRef = useRef<HTMLTextAreaElement>(null);
  const runningRef = useRef(false);
  const insertingRef = useRef(false);
  const focusOutRef = useRef(false);

  // True once an opened entry has set instructions and cleanup, so the settings
  // defaults that load afterwards do not overwrite them.
  const loadedFromEntry = useRef(false);

  // Start from the saved defaults, once, when the settings arrive.
  const seeded = useRef(false);
  useEffect(() => {
    if (!settings || seeded.current) return;
    seeded.current = true;
    if (!loadedFromEntry.current) {
      setInstructions(settings.instructions);
      setCleanup(settings.cleanup);
    }
  }, [settings]);

  // Apply the transform settings of an entry opened from History or a finished dictation.
  useEffect(() => {
    if (!ed.load) return;
    loadedFromEntry.current = true;
    setInstructions(ed.load.instructions);
    setCleanup(ed.load.cleanup);
    setEditor({ load: null });
  }, [ed.load]);

  // Elapsed seconds while a run is in flight.
  useEffect(() => {
    if (busy !== 'run') {
      setSecs(0);
      return;
    }
    const start = Date.now();
    const t = window.setInterval(() => setSecs(Math.floor((Date.now() - start) / 1000)), 500);
    return () => window.clearInterval(t);
  }, [busy]);

  // After a run, the caret goes to the start of the output with nothing selected.
  useEffect(() => {
    if (!focusOutRef.current) return;
    focusOutRef.current = false;
    const el = outRef.current;
    if (!el) return;
    el.focus();
    el.setSelectionRange(0, 0);
    el.scrollTop = 0;
  }, [ed.output, ed.entryId]);

  // "Inserted" stays visible for 1.5 s.
  useEffect(() => {
    if (!justInserted) return;
    const t = window.setTimeout(() => setJustInserted(false), 1500);
    return () => window.clearTimeout(t);
  }, [justInserted]);

  const differsFromDefault =
    settings !== null && (instructions !== settings.instructions || cleanup !== settings.cleanup);

  const run = async () => {
    // Idempotent: the key can arrive from the textarea and from the app-level listener.
    if (runningRef.current || ed.input.trim() === '') return;
    reveal();
    runningRef.current = true;
    setBusy('run');
    try {
      const entry = await api.runEdit(ed.input, cleanup, instructions);
      focusOutRef.current = true;
      setEditor({
        output: entry.text,
        entryId: entry.id,
        note: entry.note,
        inserted: false,
        source: ed.source ?? 'edit',
      });
      setJustInserted(false);
    } catch (e) {
      reportError('run_edit', e);
    } finally {
      runningRef.current = false;
      setBusy(null);
      // Same text twice produces no state change; still move focus.
      if (focusOutRef.current) {
        window.setTimeout(() => {
          if (!focusOutRef.current) return;
          focusOutRef.current = false;
          const el = outRef.current;
          if (el) {
            el.focus();
            el.setSelectionRange(0, 0);
          }
        }, 80);
      }
    }
  };

  const saveDefault = () => {
    if (!settings) return;
    patchSettings({ instructions, cleanup });
    pushToast('info', 'Saved as the default for dictation and edits.', '');
  };

  const insert = async () => {
    if (insertingRef.current || runningRef.current || ed.output.trim() === '') return;
    insertingRef.current = true;
    setBusy('insert');
    try {
      await api.insertText(ed.output, ed.entryId);
      setEditor({ inserted: true });
      setJustInserted(true);
    } catch (e) {
      reportError('insert_text', e);
    } finally {
      insertingRef.current = false;
      setBusy(null);
    }
  };

  const copy = async () => {
    if (ed.output.trim() === '') return;
    try {
      await copyText(ed.output);
      pushToast('info', 'Copied to the clipboard.', '');
    } catch (e) {
      reportError('clipboard', e);
    }
  };

  const focusInstructions = () => {
    reveal();
    if (cleanup !== 'ai') flushSync(() => setCleanup('ai'));
    insRef.current?.focus();
  };

  // Registered handlers always call the latest closures.
  const latest = useRef({ run, insert, copy, focusInstructions });
  latest.current = { run, insert, copy, focusInstructions };

  useEffect(() => {
    const L = latest;
    const offs = [
      registerAction('run-edit', () => void L.current.run()),
      registerAction('insert-output', () => void L.current.insert()),
      registerAction('copy-output', () => void L.current.copy()),
      registerAction('clear-editor', () => clearEditor()),
      registerAction('cleanup-off', () => setCleanup('off')),
      registerAction('cleanup-basic', () => setCleanup('basic')),
      registerAction('cleanup-ai', () => setCleanup('ai')),
      registerAction('focus-input', () => {
        reveal();
        inRef.current?.focus();
      }),
      registerAction('focus-instructions', () => L.current.focusInstructions()),
      registerAction('focus-output', () => {
        reveal();
        outRef.current?.focus();
      }),
    ];
    return () => offs.forEach((off) => off());
  }, []);

  // Ctrl+Enter runs from any of the three text areas.
  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (matches(e, 'run-edit')) {
      e.preventDefault();
      e.stopPropagation();
      void run();
    }
  };

  const hasInput = ed.input.trim() !== '';
  const hasOutput = ed.output.trim() !== '';
  const running = busy === 'run';
  const runLabel = cleanup === 'ai' ? 'Cleaning up (AI)' : cleanup === 'basic' ? 'Cleaning up (Basic)' : 'Running';

  // The instructions field is one line until it is focused or holds more than one line.
  const [insFocus, setInsFocus] = useState(false);
  const insOpen = insFocus || instructions.includes('\n');
  useAutosize(inRef, ed.input, 3, 8);
  useAutosize(outRef, ed.output, hasOutput ? 3 : 1, 14);

  return (
    <section className="pane editor" aria-labelledby="ed-h">
      <div className="pane-head">
        <h2 id="ed-h">Editor</h2>
        <span className="spacer" />
        <button
          type="button"
          className="btn ghost sm"
          onClick={clearEditor}
          disabled={!hasInput && !hasOutput}
          aria-keyshortcuts={ariaKeys('clear-editor')}
          title="Clear the editor"
        >
          Clear
        </button>
      </div>

      <div className="pipe">
        <div className="stage">
          <div className="stage-head">
            <label htmlFor="ed-in">Input</label>
            {ed.source ? (
              <span className="chip subtle">
                {ed.source === 'dictation' ? <IconMic size={12} /> : null}
                {SOURCE_LABEL[ed.source]}
                {ed.app ? `, ${appName(ed.app)}` : ''}
              </span>
            ) : null}
          </div>
          <textarea
            id="ed-in"
            ref={inRef}
            rows={1}
            spellCheck
            placeholder="Dictate, or type or paste text here."
            value={ed.input}
            aria-keyshortcuts={ariaKeys('run-edit')}
            onKeyDown={onKey}
            onChange={(e) => setEditor({ input: e.target.value, source: ed.source ?? 'edit' })}
          />
        </div>

        <div className="stage">
          <div className="stage-head">
            <span className="stage-title" id="ed-tf">Transform</span>
            <span className="spacer" />
            <CleanupSwitch name="ed-cleanup" value={cleanup} onChange={setCleanup} accessKeys />
          </div>
          {cleanup === 'ai' ? (
            <textarea
              id="ed-ins"
              ref={insRef}
              className={insOpen ? undefined : 'one'}
              rows={insOpen ? 3 : 1}
              wrap={insOpen ? 'soft' : 'off'}
              aria-label="Instructions"
              placeholder={insOpen ? INSTRUCTIONS_PLACEHOLDER : 'Instructions, for example: formal email'}
              value={instructions}
              aria-keyshortcuts={ariaKeys('run-edit')}
              onKeyDown={onKey}
              onFocus={() => setInsFocus(true)}
              onBlur={() => setInsFocus(false)}
              onChange={(e) => setInstructions(e.target.value)}
            />
          ) : null}
          {cleanup === 'ai' ? (
            <div className="stage-model">
              <label htmlFor="ed-model">Model</label>
              <FeatureModel id="ed-model" feature="dictationLlm" />
            </div>
          ) : null}
          <div className="stage-foot">
            <span className="hint">{CLEANUP_HINT[cleanup]}</span>
            {differsFromDefault ? (
              <button type="button" className="link-btn" onClick={saveDefault} disabled={busy !== null}>
                Use as default
              </button>
            ) : null}
          </div>
          <div className="run-row">
            <button
              type="button"
              className="btn primary"
              onClick={() => void run()}
              disabled={!hasInput || busy !== null}
              aria-keyshortcuts={ariaKeys('run-edit')}
            >
              <IconPlay size={12} /> Run <Kbd action="run-edit" />
            </button>
            <span role="status">
              {running ? (
                <span className="run-status">
                  {runLabel}
                  {secs >= 1 ? ` ${secs} s` : ''}
                </span>
              ) : null}
            </span>
          </div>
        </div>

        <div className="out-panel">
          <div className="stage-head">
            <label htmlFor="ed-out">Output</label>
            <span role="status">{justInserted ? <span className="chip ok">Inserted</span> : null}</span>
          </div>
          {ed.note ? (
            <div className="pipe-note" role="status">
              <IconAlert size={13} />
              <span>{ed.note}</span>
            </div>
          ) : null}
          <textarea
            id="ed-out"
            ref={outRef}
            className={hasOutput ? 'filled' : undefined}
            rows={1}
            placeholder="The result appears here. You can edit it before inserting."
            value={ed.output}
            aria-keyshortcuts={ariaKeys('run-edit')}
            onKeyDown={onKey}
            onChange={(e) => setEditor({ output: e.target.value })}
          />
          <div className="out-foot">
            <span className="insert-target" id="ed-target">
              {ed.app ? `Inserts into ${appName(ed.app)}` : 'Inserts into the last focused app'}
            </span>
            <button
              type="button"
              className="btn sm"
              onClick={() => void copy()}
              disabled={!hasOutput}
              title={`Copy (${shortcut('copy-output').keys[0]})`}
              aria-keyshortcuts={ariaKeys('copy-output')}
            >
              Copy
            </button>
            <button
              type="button"
              className="btn sm"
              onClick={() => void insert()}
              disabled={!hasOutput || busy !== null}
              aria-keyshortcuts={ariaKeys('insert-output')}
              aria-describedby="ed-target"
            >
              Insert <Kbd action="insert-output" />
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}