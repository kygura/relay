import { useEffect, useMemo, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
import * as api from '../api';
import { Editor } from './Editor';
import { LiveSetup, setupSummary } from './LiveSetup';
import { langName, SHOW_EDITOR_EVENT, useSettings } from './setup';
import { Kbd, ModePills, useElapsed, useSessionControl } from '../components';
import { Waveform, useLevelRef } from '../overlay/Waveform';
import { IconEdit, IconSettings, IconTrash } from '../icons';
import { ariaKeys, registerAction, shortcut } from '../keymap';
import {
  clearTranscriptLocal,
  reportError,
  selDictation,
  selFinals,
  selInterim,
  selTranslations,
  setEditor,
  useCatalog,
  useRelay,
} from '../store';
import type { Mode, TranscriptSegment } from '../types';
import { formatMs } from '../util';

const MODE_HINT: Record<Exclude<Mode, 'idle'>, string> = {
  transcribe: 'Live captions from your microphone and system audio.',
  dictation: 'Microphone only. When you stop, the text runs through the editor steps and is pasted.',
  translate: 'Live captions with translation for the sources you capture.',
  meeting: 'Transcript, translation and notes that refresh on their own.',
  agent: 'Each spoken request becomes an agent task.',
};

type Side = 'editor' | 'setup';
type View = 'both' | 'reader';
type ReadSize = 's' | 'm' | 'l';

function loadPref<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const v = localStorage.getItem(key);
    if (v && (allowed as readonly string[]).includes(v)) return v as T;
  } catch {
    // storage unavailable
  }
  return fallback;
}

function savePref(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // storage unavailable
  }
}

function speakerName(seg: TranscriptSegment): string {
  if (seg.speaker) return seg.speaker;
  return seg.source === 'microphone' ? 'You' : 'Speaker';
}

function SegmentRow({ seg, interim, readLang }: { seg: TranscriptSegment; interim?: boolean; readLang: string | null }) {
  const trs = useRelay((s) => selTranslations(s)[seg.id]);
  const name = speakerName(seg);
  const read = readLang ? trs?.find((t) => t.targetLang === readLang) : undefined;
  return (
    <li className={`seg${interim ? ' interim' : ''}`}>
      <div className="seg-gutter">
        <span className="seg-speaker" title={`${name}, ${seg.source === 'microphone' ? 'microphone' : 'system audio'}`}>
          {name}
        </span>
        <span className="seg-time">{formatMs(seg.startMs)}</span>
        {seg.language ? <span className="seg-lang">{seg.language}</span> : null}
      </div>
      {readLang ? (
        // Reader: the translation is the text you read. The original sits under it, small.
        <div className="seg-main">
          {read ? (
            <div className="seg-read" lang={readLang}>
              {read.text}
            </div>
          ) : null}
          <div className={`seg-orig${read ? '' : ' waiting'}`}>{seg.text}</div>
        </div>
      ) : (
        <div className="seg-main">
          <div className="seg-text">{seg.text}</div>
          {trs && trs.length > 0 ? (
            <ul className="seg-trs">
              {trs.map((t) => (
                <li key={t.targetLang} lang={t.targetLang}>
                  <span className="tr-lang">{t.targetLang}</span>
                  <span>{t.text}</span>
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      )}
    </li>
  );
}

const SIZES: Array<{ id: ReadSize; label: string; name: string }> = [
  { id: 's', label: 'A', name: 'Normal text' },
  { id: 'm', label: 'A+', name: 'Large text' },
  { id: 'l', label: 'A++', name: 'Largest text' },
];

function Transcript({ onClear, onShowEditor }: { onClear: () => void; onShowEditor: () => void }) {
  const finals = useRelay(selFinals);
  const interimMap = useRelay(selInterim);
  const hasTranslations = useRelay((s) => Object.keys(selTranslations(s)).length > 0);
  const settings = useSettings();
  const interim = useMemo(
    () => Object.values(interimMap).sort((a, b) => a.startMs - b.startMs),
    [interimMap],
  );
  const endRef = useRef<HTMLDivElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  const targets = settings?.targetLanguages ?? [];
  const canRead = targets.length > 0 || hasTranslations;
  const [view, setView] = useState<View>(() => loadPref('relay.liveView', ['both', 'reader'] as const, 'both'));
  const [size, setSize] = useState<ReadSize>(() => loadPref('relay.readSize', ['s', 'm', 'l'] as const, 'm'));
  const [lang, setLang] = useState<string>(() => loadPref('relay.readLang', targets, targets[0] ?? ''));
  const reading = view === 'reader' && canRead;
  const readLang = reading ? (targets.includes(lang) ? lang : (targets[0] ?? lang)) : null;

  const onScroll = () => {
    const el = boxRef.current;
    if (!el) return;
    stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };

  useEffect(() => {
    if (stick.current) endRef.current?.scrollIntoView({ block: 'end' });
  }, [finals, interim, reading, size]);

  const empty = finals.length === 0 && interim.length === 0;

  const sendToEditor = () => {
    if (finals.length === 0) return;
    setEditor({
      input: finals.map((s) => `${speakerName(s)}: ${s.text.trim()}`).join('\n'),
      output: '',
      entryId: null,
      source: 'transcript',
      app: null,
      note: null,
      inserted: false,
    });
    onShowEditor();
    document.getElementById('ed-in')?.focus();
  };
  const sendRef = useRef(sendToEditor);
  sendRef.current = sendToEditor;
  useEffect(() => registerAction('send-to-editor', () => sendRef.current()), []);

  const pickView = (v: View) => {
    setView(v);
    savePref('relay.liveView', v);
  };

  return (
    <section className="pane transcript-pane" aria-labelledby="tr-h">
      <div className="pane-head">
        <h2 id="tr-h">Transcript</h2>
        <span className="pane-count">{finals.length}</span>
        {canRead ? (
          <div className="seg-control" role="radiogroup" aria-label="Transcript view">
            <button
              type="button"
              role="radio"
              aria-checked={!reading}
              className={`seg-opt${reading ? '' : ' on'}`}
              title="Original text with the translations under it"
              onClick={() => pickView('both')}
            >
              Both
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={reading}
              className={`seg-opt${reading ? ' on' : ''}`}
              title="The translation in large text, for reading along in a meeting"
              onClick={() => pickView('reader')}
            >
              Reader
            </button>
          </div>
        ) : null}
        {reading ? (
          <>
            {targets.length > 1 ? (
              <select
                className="read-lang"
                aria-label="Language to read"
                value={readLang ?? ''}
                onChange={(e) => {
                  setLang(e.target.value);
                  savePref('relay.readLang', e.target.value);
                }}
              >
                {targets.map((l) => (
                  <option key={l} value={l}>
                    {langName(l)}
                  </option>
                ))}
              </select>
            ) : null}
            <div className="seg-control" role="radiogroup" aria-label="Text size">
              {SIZES.map((z) => (
                <button
                  key={z.id}
                  type="button"
                  role="radio"
                  aria-checked={size === z.id}
                  aria-label={z.name}
                  title={z.name}
                  className={`seg-opt${size === z.id ? ' on' : ''}`}
                  onClick={() => {
                    setSize(z.id);
                    savePref('relay.readSize', z.id);
                  }}
                >
                  {z.label}
                </button>
              ))}
            </div>
          </>
        ) : null}
        <span className="spacer" />
        <button
          type="button"
          className="btn ghost sm"
          disabled={finals.length === 0}
          title={`Send the transcript to the editor (${shortcut('send-to-editor').keys[0]})`}
          aria-keyshortcuts={ariaKeys('send-to-editor')}
          onClick={sendToEditor}
        >
          <IconEdit size={13} /> <span className="btn-label">Send to editor</span>
        </button>
        <button type="button" className="btn ghost sm" onClick={onClear} disabled={empty} title="Clear the transcript">
          <IconTrash size={13} /> <span className="btn-label">Clear</span>
        </button>
      </div>
      <div
        className={`transcript${reading ? ` reader size-${size}` : ''}`}
        ref={boxRef}
        onScroll={onScroll}
        tabIndex={0}
        aria-label="Transcript"
        aria-live="polite"
      >
        {empty ? (
          <div className="tr-empty">
            <div className="tr-empty-title">No transcript yet</div>
            <p>
              <Kbd action="live-toggle" /> starts live captions of your microphone and system audio.
            </p>
            <p>
              <Kbd action="dictate" /> dictates into any app and lands in the editor.
            </p>
          </div>
        ) : (
          <ul className="seg-list">
            {finals.map((s) => (
              <SegmentRow key={s.id} seg={s} readLang={readLang} />
            ))}
            {interim.map((s) => (
              <SegmentRow key={`i-${s.id}`} seg={s} interim readLang={readLang} />
            ))}
          </ul>
        )}
        <div ref={endRef} />
      </div>
    </section>
  );
}

function SessionControl({
  recording,
  elapsed,
  busy,
  onToggle,
}: {
  recording: boolean;
  elapsed: string;
  busy: boolean;
  onToggle: () => void;
}) {
  const levelRef = useLevelRef(true);
  const reduced = useMemo(
    () => typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches,
    [],
  );
  const label = recording ? 'Stop recording' : 'Start recording';
  // The record control is a round button like the one on a recorder: a red disc that becomes
  // a red square while recording. Its label and shortcut sit next to it, with the counter and
  // the level wave.
  return (
    <>
      <button
        type="button"
        className={`rec-btn${recording ? ' on' : ''}`}
        onClick={onToggle}
        disabled={busy}
        aria-label={label}
        title={`${label} (${shortcut('live-toggle').keys[0]})`}
        aria-keyshortcuts={ariaKeys('live-toggle')}
      >
        <span className="rec-mark" aria-hidden="true" />
      </button>
      <div className="session-info">
        <div className="session-read">
          <span className="session-time" aria-label="Elapsed time">
            {elapsed}
          </span>
          {recording ? (
            <span className="live-wave">
              <Waveform levelRef={levelRef} frozen={false} reducedMotion={reduced} />
            </span>
          ) : null}
        </div>
        <div className="rec-hint" aria-hidden="true">
          <span>{recording ? 'Stop' : 'Start'}</span>
          <Kbd action="live-toggle" />
        </div>
      </div>
    </>
  );
}

export function LiveTab() {
  const { app, active, pick, toggle, busy } = useSessionControl();
  const elapsed = useElapsed(app.sessionStartedAt, app.recording);
  const settings = useSettings();
  const catalog = useCatalog();
  const dictation = useRelay(selDictation);
  const [side, setSide] = useState<Side>(() => loadPref('relay.liveSide', ['editor', 'setup'] as const, 'editor'));

  const show = (s: Side) => {
    setSide(s);
    savePref('relay.liveSide', s);
  };

  // A finished dictation lands in the editor, so show it.
  const firstDictation = useRef(dictation);
  useEffect(() => {
    if (dictation && dictation !== firstDictation.current) setSide('editor');
  }, [dictation]);

  // The editor sizes its text areas from their content, which it cannot measure while hidden.
  useEffect(() => {
    if (side === 'editor') window.dispatchEvent(new Event('resize'));
  }, [side]);

  // Editor shortcuts (focus input, run, insert) bring the editor back before they act.
  useEffect(() => {
    const onShow = () => flushSync(() => setSide('editor'));
    window.addEventListener(SHOW_EDITOR_EVENT, onShow);
    return () => window.removeEventListener(SHOW_EDITOR_EVENT, onShow);
  }, []);

  const clear = async () => {
    try {
      await api.clearTranscript();
      clearTranscriptLocal();
    } catch (e) {
      reportError('clear_transcript', e);
    }
  };

  const summary = settings ? setupSummary(settings, catalog, active) : [];

  return (
    <div className="page fill live">
      <div className={`session${app.recording ? ' rec' : ''}`}>
        <SessionControl recording={app.recording} elapsed={elapsed} busy={busy} onToggle={toggle} />
        <div className="session-modes">
          <ModePills active={active} onPick={pick} />
          <p className="mode-line">{MODE_HINT[active]}</p>
        </div>
        {summary.length > 0 ? (
          <button
            type="button"
            className={`setup-summary${side === 'setup' ? ' on' : ''}`}
            aria-pressed={side === 'setup'}
            title={side === 'setup' ? 'Back to the editor' : 'Change the setup for this mode'}
            onClick={() => show(side === 'setup' ? 'editor' : 'setup')}
          >
            <IconSettings size={14} />
            <span className="sr-only">Setup: </span>
            {summary.map((part) => (
              <span key={part} className="setup-part">
                {part}
              </span>
            ))}
          </button>
        ) : null}
      </div>
      <div className="live-body">
        <Transcript onClear={clear} onShowEditor={() => flushSync(() => setSide('editor'))} />
        <div className="side-col">
          <div className="side-tabs seg-control" role="tablist" aria-label="Side panel">
            <button
              type="button"
              role="tab"
              id="side-tab-editor"
              aria-selected={side === 'editor'}
              aria-controls="side-editor"
              className={`seg-opt${side === 'editor' ? ' on' : ''}`}
              onClick={() => show('editor')}
            >
              Editor
            </button>
            <button
              type="button"
              role="tab"
              id="side-tab-setup"
              aria-selected={side === 'setup'}
              aria-controls="side-setup"
              className={`seg-opt${side === 'setup' ? ' on' : ''}`}
              onClick={() => show('setup')}
            >
              Setup
            </button>
          </div>
          {/* The editor stays mounted so its text and its shortcuts survive a look at the setup. */}
          <div id="side-editor" role="tabpanel" aria-labelledby="side-tab-editor" className="side-panel" hidden={side !== 'editor'}>
            <Editor />
          </div>
          {side === 'setup' ? (
            <div id="side-setup" role="tabpanel" aria-labelledby="side-tab-setup" className="side-panel pane setup-pane">
              <LiveSetup mode={active} recording={app.recording} />
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
