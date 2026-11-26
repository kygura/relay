import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from 'react';
import { Kbd } from '../components';
import { allShortcuts, canRun, runAction, type ActionId, type Scope } from '../keymap';
import { openInEditor, selHistory, useRelay } from '../store';
import type { HistoryEntry } from '../types';

export const SCOPE_LABEL: Record<Scope, string> = {
  global: 'Global',
  app: 'App',
  live: 'Live',
  editor: 'Editor',
  history: 'History',
  palette: 'Palette',
};

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * Shared modal behaviour: remembers the element that had focus, closes on Escape (and stops
 * the event so nothing underneath sees it), and keeps Tab inside the dialog.
 * `restore()` returns focus to the remembered element; call it before running an action
 * so the action can move focus elsewhere.
 */
export function useModal(onClose: () => void, dialogRef: RefObject<HTMLElement>) {
  const [prev] = useState(() => document.activeElement as HTMLElement | null);
  const restore = () => {
    if (prev && prev.isConnected) prev.focus();
  };
  const close = () => {
    restore();
    onClose();
  };
  const onKeyDown = (e: KeyboardEvent<HTMLElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      e.nativeEvent.stopImmediatePropagation();
      close();
      return;
    }
    if (e.key === 'Tab') {
      const items = Array.from(dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
      if (items.length === 0) {
        e.preventDefault();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === first || !dialogRef.current?.contains(active))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (active === last || !dialogRef.current?.contains(active))) {
        e.preventDefault();
        first.focus();
      }
    }
  };
  return { restore, close, onKeyDown };
}

/** Score a candidate against a query: 0 prefix, 1 substring, 2 subsequence, null no match. */
function score(text: string, q: string): number | null {
  if (!q) return 0;
  const t = text.toLowerCase();
  if (t.startsWith(q)) return 0;
  if (t.includes(q)) return 1;
  let i = 0;
  for (const ch of t) {
    if (ch === q[i]) i++;
    if (i === q.length) return 2;
  }
  return null;
}

type Item =
  | { key: string; kind: 'command'; id: ActionId; label: string; scope: Scope }
  | { key: string; kind: 'history'; entry: HistoryEntry; label: string };

function oneLine(s: string, n: number): string {
  const t = s.replace(/\s+/g, ' ').trim();
  return t.length > n ? `${t.slice(0, n).trimEnd()}...` : t;
}

export function Palette({ onClose }: { onClose: () => void }) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const { restore, close, onKeyDown } = useModal(onClose, dialogRef);
  const history = useRelay(selHistory);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);

  const commands = useMemo<Item[]>(
    () =>
      allShortcuts()
        .filter((s) => s.palette && canRun(s.id))
        .map((s) => ({ key: `c-${s.id}`, kind: 'command', id: s.id, label: s.label, scope: s.scope })),
    [],
  );
  const recents = useMemo<Item[]>(
    () =>
      (history ?? []).slice(0, 8).map((h) => ({
        key: `h-${h.id}`,
        kind: 'history',
        entry: h,
        label: `Open in editor: ${oneLine(h.text || h.raw, 60)}`,
      })),
    [history],
  );

  const items = useMemo(() => {
    const q = query.trim().toLowerCase();
    const scored: Array<{ item: Item; s: number; n: number }> = [];
    [...commands, ...recents].forEach((item, n) => {
      const hay = item.kind === 'command' ? `${item.label} ${SCOPE_LABEL[item.scope]}` : item.label;
      const s = score(item.label, q) ?? (item.kind === 'command' ? score(hay, q) : null);
      if (s !== null) scored.push({ item, s, n });
    });
    scored.sort((a, b) => a.s - b.s || a.n - b.n);
    return scored.map((x) => x.item);
  }, [commands, recents, query]);

  const safeActive = Math.min(active, Math.max(0, items.length - 1));

  useEffect(() => {
    document.getElementById(`pal-opt-${safeActive}`)?.scrollIntoView({ block: 'nearest' });
  }, [safeActive]);

  const run = (item: Item | undefined) => {
    if (!item) return;
    restore();
    onClose();
    if (item.kind === 'command') {
      runAction(item.id);
    } else {
      openInEditor(item.entry);
      runAction('go-live');
    }
  };

  const onInputKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setActive(items.length ? (safeActive + 1) % items.length : 0);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setActive(items.length ? (safeActive - 1 + items.length) % items.length : 0);
    } else if (e.key === 'Home' && items.length) {
      e.preventDefault();
      setActive(0);
    } else if (e.key === 'End' && items.length) {
      e.preventDefault();
      setActive(items.length - 1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      run(items[safeActive]);
    }
  };

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div
        ref={dialogRef}
        className="dialog palette"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onKeyDown={onKeyDown}
      >
        <input
          type="text"
          className="palette-input"
          role="combobox"
          aria-label="Search commands"
          aria-expanded="true"
          aria-controls="pal-list"
          aria-autocomplete="list"
          aria-activedescendant={items.length ? `pal-opt-${safeActive}` : undefined}
          placeholder="Type a command or search history"
          autoComplete="off"
          spellCheck={false}
          autoFocus
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
          }}
          onKeyDown={onInputKey}
        />
        <ul ref={listRef} id="pal-list" className="palette-list" role="listbox" aria-label="Commands">
          {items.map((item, i) => (
            <li
              key={item.key}
              id={`pal-opt-${i}`}
              role="option"
              aria-selected={i === safeActive}
              className={`palette-opt${i === safeActive ? ' on' : ''}`}
              onMouseMove={() => i !== safeActive && setActive(i)}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => run(item)}
            >
              <span className="palette-label">{item.label}</span>
              {item.kind === 'command' ? (
                <>
                  <span className="palette-scope">{SCOPE_LABEL[item.scope]}</span>
                  <Kbd action={item.id} />
                </>
              ) : (
                <span className="palette-scope">History</span>
              )}
            </li>
          ))}
        </ul>
        {items.length === 0 ? <div className="palette-empty">No matching commands.</div> : null}
        <div className="sr-only" role="status" aria-live="polite">
          {items.length === 1 ? '1 result' : `${items.length} results`}
        </div>
      </div>
    </div>
  );
}
