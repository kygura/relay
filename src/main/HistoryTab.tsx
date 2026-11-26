// Every run of the text primitive: dictations, edits and finished live transcripts.
// Each entry keeps the original text and the result, and can be reopened in the editor.
//
// Keyboard model: the list is one tab stop (roving tabindex). With a row focused:
// Up, Down, Home, End move; Enter opens in the editor; Alt+Enter shows the original or the result;
// Ctrl+C copies the result (when no text is selected); Delete removes the row with a 6 s undo.

import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import * as api from '../api';
import { EmptyState, Kbd, PageHeader } from '../components';
import { IconAlert, IconCopy, IconEdit, IconMic, IconSearch, IconSpeaker, IconTrash } from '../icons';
import { inTextField, matches, registerAction } from '../keymap';
import { openInEditor, pushToast, reportError, selHistory, useRelay } from '../store';
import type { HistoryEntry, HistoryKind } from '../types';
import { copyText } from '../util';
import { AudioClip } from './AudioClip';
import { CLEANUP_LABEL } from './Editor';

type Filter = 'all' | HistoryKind;

const FILTERS: Array<{ id: Filter; label: string }> = [
  { id: 'all', label: 'All' },
  { id: 'dictation', label: 'Dictation' },
  { id: 'edit', label: 'Edits' },
  { id: 'transcript', label: 'Transcripts' },
];

const KIND_LABEL: Record<HistoryKind, string> = {
  dictation: 'Dictation',
  edit: 'Edit',
  transcript: 'Transcript',
};

const UNDO_MS = 6000;

function relativeTime(iso: string): string {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return iso;
  const s = Math.round((Date.now() - t) / 1000);
  if (s < 45) return 'just now';
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  return new Date(iso).toLocaleDateString([], { month: 'short', day: 'numeric' });
}

function fmtDuration(ms: number | null): string | null {
  if (!ms) return null;
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s} s` : `${Math.floor(s / 60)} min ${s % 60} s`;
}

function KindIcon({ kind }: { kind: HistoryKind }) {
  if (kind === 'dictation') return <IconMic size={14} />;
  if (kind === 'transcript') return <IconSpeaker size={14} />;
  return <IconEdit size={14} />;
}

function Row({
  entry,
  active,
  onActive,
  onOpen,
  onMove,
  onDelete,
}: {
  entry: HistoryEntry;
  active: boolean;
  onActive: () => void;
  onOpen: () => void;
  onMove: (to: 'prev' | 'next' | 'first' | 'last') => void;
  onDelete: () => void;
}) {
  const [showRaw, setShowRaw] = useState(false);
  const [open, setOpen] = useState(false);
  const changed = entry.raw.trim() !== entry.text.trim();
  const meta = [
    entry.app ? entry.app.replace(/\.exe$/i, '') : null,
    fmtDuration(entry.durationMs),
    entry.kind !== 'transcript' ? `Cleanup: ${CLEANUP_LABEL[entry.cleanup]}` : null,
  ].filter(Boolean);
  // Controls inside a row are tab stops only for the active row.
  const tab = active ? 0 : -1;

  const copy = async () => {
    try {
      await copyText(entry.text);
      pushToast('info', 'Copied to the clipboard.', '');
    } catch (e) {
      reportError('clipboard', e);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLLIElement>) => {
    if (e.target !== e.currentTarget) return;
    const stop = () => {
      e.preventDefault();
      e.stopPropagation();
    };
    if (e.key === 'ArrowDown' && !e.altKey && !e.ctrlKey) {
      stop();
      onMove('next');
    } else if (e.key === 'ArrowUp' && !e.altKey && !e.ctrlKey) {
      stop();
      onMove('prev');
    } else if (e.key === 'Home' && !e.ctrlKey) {
      stop();
      onMove('first');
    } else if (e.key === 'End' && !e.ctrlKey) {
      stop();
      onMove('last');
    } else if (matches(e, 'history-toggle-original')) {
      stop();
      if (changed) setShowRaw((r) => !r);
    } else if (matches(e, 'history-open')) {
      stop();
      onOpen();
    } else if (matches(e, 'history-delete')) {
      stop();
      onDelete();
    } else if (matches(e, 'history-copy')) {
      if (window.getSelection()?.toString()) return;
      stop();
      void copy();
    } else if (e.key === ' ' && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      stop();
      setOpen((o) => !o);
    }
  };

  return (
    <li
      className="h-row"
      data-hid={entry.id}
      tabIndex={active ? 0 : -1}
      aria-label={`${KIND_LABEL[entry.kind]}, ${relativeTime(entry.createdAt)}`}
      onFocus={onActive}
      onKeyDown={onKeyDown}
    >
      <div className="h-side">
        <div className="h-kind">
          <KindIcon kind={entry.kind} />
          <span>{KIND_LABEL[entry.kind]}</span>
        </div>
        <time className="h-time" dateTime={entry.createdAt} title={new Date(entry.createdAt).toLocaleString()}>
          {relativeTime(entry.createdAt)}
        </time>
      </div>
      <div className="h-main">
        {meta.length > 0 || entry.inserted ? (
          <div className="h-top">
            {meta.map((m) => (
              <span key={m} className="h-meta">
                {m}
              </span>
            ))}
            {entry.inserted ? <span className="chip ok">Inserted</span> : null}
          </div>
        ) : null}
        <button
          type="button"
          className={`h-text${open ? ' open' : ''}`}
          onClick={() => setOpen((o) => !o)}
          aria-expanded={open}
          tabIndex={tab}
        >
          {showRaw ? entry.raw : entry.text}
        </button>
        {entry.audioFile ? <AudioClip file={entry.audioFile} durationMs={entry.durationMs} tabIndex={tab} /> : null}
        {entry.instructions.trim() !== '' && entry.cleanup === 'ai' ? (
          <div className="h-instr">
            <span className="eyebrow">Instructions</span> {entry.instructions}
          </div>
        ) : null}
        {entry.note ? (
          <div className="pipe-note h-note" title={entry.note}>
            <IconAlert size={13} />
            <span>{entry.note}</span>
          </div>
        ) : null}
        <div className="h-actions">
          {changed ? (
            <button
              type="button"
              className={`btn ghost sm${showRaw ? ' pressed' : ''}`}
              aria-pressed={showRaw}
              aria-keyshortcuts="Alt+Enter"
              tabIndex={tab}
              onClick={() => setShowRaw((r) => !r)}
            >
              {showRaw ? 'Show result' : 'Show original'}
            </button>
          ) : null}
          <span className="spacer" />
          <button type="button" className="btn ghost sm" tabIndex={tab} onClick={onOpen}>
            <IconEdit size={13} /> Open in editor
          </button>
          <button type="button" className="btn ghost sm" tabIndex={tab} onClick={() => void copy()}>
            <IconCopy size={13} /> Copy
          </button>
          <button
            type="button"
            className="icon-btn"
            aria-label="Delete entry"
            title="Delete"
            tabIndex={tab}
            onClick={onDelete}
          >
            <IconTrash size={14} />
          </button>
        </div>
      </div>
    </li>
  );
}

export function HistoryTab({ onOpenEditor }: { onOpenEditor: () => void }) {
  const history = useRelay(selHistory);
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  const [confirmClear, setConfirmClear] = useState(false);
  const [activeId, setActiveId] = useState<string | null>(null);
  // Entries hidden while the undo window is open (latest last), and entries already sent for deletion.
  const [pending, setPending] = useState<string[]>([]);
  const [gone, setGone] = useState<string[]>([]);

  const listRef = useRef<HTMLUListElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const filterRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const timers = useRef(new Map<string, number>());
  const focusAfter = useRef<string | null>(null);

  const visible = useMemo(() => {
    if (!history) return [];
    const hide = new Set([...pending, ...gone]);
    return history.filter((e) => !hide.has(e.id));
  }, [history, pending, gone]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return visible.filter(
      (e) =>
        (filter === 'all' || e.kind === filter) &&
        (q === '' || e.text.toLowerCase().includes(q) || e.raw.toLowerCase().includes(q)),
    );
  }, [visible, filter, query]);

  const counts = useMemo(() => {
    const c: Record<Filter, number> = { all: 0, dictation: 0, edit: 0, transcript: 0 };
    for (const e of visible) {
      c.all++;
      c[e.kind]++;
    }
    return c;
  }, [visible]);

  const activeEff = shown.some((e) => e.id === activeId) ? activeId : (shown[0]?.id ?? null);

  const focusRow = useCallback((id: string | null) => {
    if (!id) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-hid="${CSS.escape(id)}"]`);
    el?.focus();
  }, []);

  // Move focus after a delete or an undo, once the list has re-rendered.
  useEffect(() => {
    if (focusAfter.current && shown.some((e) => e.id === focusAfter.current)) {
      const id = focusAfter.current;
      focusAfter.current = null;
      setActiveId(id);
      focusRow(id);
    }
  }, [shown, focusRow]);

  const move = (from: string, to: 'prev' | 'next' | 'first' | 'last') => {
    const i = shown.findIndex((e) => e.id === from);
    if (i < 0) return;
    const j = to === 'first' ? 0 : to === 'last' ? shown.length - 1 : Math.min(shown.length - 1, Math.max(0, i + (to === 'next' ? 1 : -1)));
    const id = shown[j].id;
    setActiveId(id);
    focusRow(id);
  };

  // ----- Delete with undo: hide now, call the backend after 6 s -----

  const commit = useCallback((id: string) => {
    timers.current.delete(id);
    setPending((p) => p.filter((x) => x !== id));
    setGone((g) => [...g, id]);
    api.historyDelete(id).catch((e) => {
      setGone((g) => g.filter((x) => x !== id));
      reportError('history_delete', e);
    });
  }, []);

  const softDelete = (entry: HistoryEntry) => {
    const i = shown.findIndex((e) => e.id === entry.id);
    const next = shown[i + 1] ?? shown[i - 1] ?? null;
    focusAfter.current = next ? next.id : null;
    setPending((p) => [...p, entry.id]);
    timers.current.set(entry.id, window.setTimeout(() => commit(entry.id), UNDO_MS));
    if (!next) searchRef.current?.focus();
  };

  const undo = useCallback(() => {
    setPending((p) => {
      const id = p[p.length - 1];
      if (id === undefined) return p;
      const t = timers.current.get(id);
      if (t !== undefined) window.clearTimeout(t);
      timers.current.delete(id);
      focusAfter.current = id;
      return p.slice(0, -1);
    });
  }, []);

  // Ctrl+Z restores the last deleted entry while the bar is up.
  useEffect(() => {
    if (pending.length === 0) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key.toLowerCase() === 'z' && !inTextField(e.target)) {
        e.preventDefault();
        undo();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [pending.length, undo]);

  // Leaving the tab finishes any delete still waiting.
  useEffect(() => {
    const t = timers.current;
    return () => {
      for (const [id, h] of t) {
        window.clearTimeout(h);
        api.historyDelete(id).catch((e) => reportError('history_delete', e));
      }
      t.clear();
    };
  }, []);

  // ----- Search -----

  const focusSearch = useCallback(() => {
    const el = searchRef.current;
    if (!el) return;
    el.focus();
    el.select();
  }, []);

  useEffect(() => registerAction('history-search', focusSearch), [focusSearch]);

  // '/' and Ctrl+F also work from here; focusing twice is harmless.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (!matches(e, 'history-search')) return;
      if (e.key === '/' && (inTextField(e.target) || e.ctrlKey || e.altKey || e.metaKey)) return;
      e.preventDefault();
      focusSearch();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [focusSearch]);

  const onSearchKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== 'Escape') return;
    e.preventDefault();
    e.stopPropagation();
    if (query !== '') setQuery('');
    else if (activeEff) focusRow(activeEff);
    else (e.target as HTMLElement).blur();
  };

  // ----- Filter radios -----

  const onFilterKey = (e: KeyboardEvent<HTMLButtonElement>, i: number) => {
    let j = i;
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') j = (i + 1) % FILTERS.length;
    else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') j = (i - 1 + FILTERS.length) % FILTERS.length;
    else if (e.key === 'Home') j = 0;
    else if (e.key === 'End') j = FILTERS.length - 1;
    else return;
    e.preventDefault();
    setFilter(FILTERS[j].id);
    filterRefs.current[j]?.focus();
  };

  const clearAll = async () => {
    try {
      await api.historyClear();
      setConfirmClear(false);
    } catch (e) {
      reportError('history_clear', e);
    }
  };

  const total = history ? history.length : 0;

  return (
    <div className="page fill history">
      <PageHeader
        title="History"
        subtitle="Dictations, edits and finished transcripts. Stored on this PC."
        actions={
          confirmClear ? (
            <div className="row gap">
              <span className="muted small">Delete all {counts.all} entries?</span>
              <button type="button" className="btn danger-outline sm" onClick={clearAll}>
                Delete all
              </button>
              <button type="button" className="btn ghost sm" onClick={() => setConfirmClear(false)}>
                Cancel
              </button>
            </div>
          ) : (
            <button type="button" className="btn ghost sm" disabled={!counts.all} onClick={() => setConfirmClear(true)}>
              <IconTrash size={13} /> Clear all
            </button>
          )
        }
      />
      <div className="h-toolbar">
        <div className="seg-control" role="radiogroup" aria-label="Filter by kind">
          {FILTERS.map((f, i) => (
            <button
              key={f.id}
              ref={(el) => {
                filterRefs.current[i] = el;
              }}
              type="button"
              role="radio"
              aria-checked={filter === f.id}
              tabIndex={filter === f.id ? 0 : -1}
              className={`seg-opt${filter === f.id ? ' on' : ''}`}
              onClick={() => setFilter(f.id)}
              onKeyDown={(e) => onFilterKey(e, i)}
            >
              {f.label}
              <span className="seg-count">{counts[f.id]}</span>
            </button>
          ))}
        </div>
        <label className="h-search">
          <IconSearch size={14} />
          <input
            ref={searchRef}
            type="text"
            placeholder="Search"
            aria-label="Search history"
            aria-keyshortcuts="Control+F /"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onSearchKey}
          />
        </label>
      </div>
      {pending.length > 0 ? (
        <div className="undo-bar" role="status">
          <span>{pending.length === 1 ? 'Entry deleted.' : `${pending.length} entries deleted.`}</span>
          <button type="button" className="link-btn" onClick={undo}>
            Undo <Kbd action="history-undo" />
          </button>
        </div>
      ) : null}
      <div className="h-scroll">
      {history === null ? (
        <div className="muted">Loading history.</div>
      ) : total === 0 || visible.length === 0 ? (
        <EmptyState title="Nothing here yet">
          <span>Dictations, edits and live transcripts are saved here when they finish.</span>
          <span className="empty-keys">
            <span>
              Dictate <Kbd action="dictate" />
            </span>
            <span>
              Live <Kbd action="live-toggle" />
            </span>
          </span>
        </EmptyState>
      ) : shown.length === 0 ? (
        <EmptyState title="No matches">
          <span>Try another word or filter.</span>
        </EmptyState>
      ) : (
        <ul className="h-list" ref={listRef} aria-label="History entries">
          {shown.map((e) => (
            <Row
              key={e.id}
              entry={e}
              active={e.id === activeEff}
              onActive={() => setActiveId(e.id)}
              onMove={(to) => move(e.id, to)}
              onDelete={() => softDelete(e)}
              onOpen={() => {
                openInEditor(e);
                onOpenEditor();
              }}
            />
          ))}
        </ul>
      )}
      </div>
      <div className="h-hint">
        <Kbd keys={['Up']} /> and <Kbd keys={['Down']} /> to move, <Kbd action="history-open" /> to open,{' '}
        <Kbd action="history-delete" /> to remove
      </div>
    </div>
  );
}
