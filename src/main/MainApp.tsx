import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { Kbd, Logo, Toasts, useElapsed } from '../components';
import {
  allShortcuts,
  ariaKeys,
  duplicateKeys,
  inTextField,
  matches,
  registerAction,
  runAction,
  setGlobalHotkeys,
  shortcut,
  type ActionId,
} from '../keymap';
import { Palette } from './Palette';
import { ShortcutSheet } from './ShortcutSheet';
import { IconAgent, IconHistory, IconLive, IconNotes, IconSettings } from '../icons';
import { HistoryTab } from './HistoryTab';
import { isTauri } from '../api';
import { selApp, useRelay } from '../store';
import { AgentTab } from './AgentTab';
import { LiveTab } from './LiveTab';
import { NotesTab } from './NotesTab';
import { SettingsTab } from './SettingsTab';

type TabId = 'live' | 'history' | 'notes' | 'agent' | 'settings';

const TABS: Array<{ id: TabId; label: string; icon: JSX.Element }> = [
  { id: 'live', label: 'Live', icon: <IconLive size={17} /> },
  { id: 'history', label: 'History', icon: <IconHistory size={17} /> },
  { id: 'notes', label: 'Notes', icon: <IconNotes size={17} /> },
  { id: 'agent', label: 'Agent', icon: <IconAgent size={17} /> },
  { id: 'settings', label: 'Settings', icon: <IconSettings size={17} /> },
];

/**
 * Actions the app-level listener never runs. Global hotkeys belong to the OS. History row keys
 * (Enter, Delete, Ctrl+C, Alt+Enter) apply only while a row has focus, so the history list
 * handles them itself.
 */
const SKIP_IN_APP = new Set<ActionId>([
  'dictate',
  'history-open',
  'history-copy',
  'history-toggle-original',
  'history-delete',
]);

function loadTab(): TabId {
  try {
    const v = localStorage.getItem('relay.tab');
    if (v === 'live' || v === 'history' || v === 'notes' || v === 'agent' || v === 'settings') return v;
  } catch {
    // storage unavailable
  }
  return 'live';
}

/** Session state in the status bar: mark, state, and the timer while recording. */
function StatusLine() {
  const app = useRelay(selApp);
  const elapsed = useElapsed(app.sessionStartedAt, app.recording);
  const processing = app.phase === 'transcribing' || app.phase === 'cleaning';
  let label = 'Idle';
  let dot = '';
  if (app.recording && app.mode !== 'dictation') {
    label = `Recording, ${elapsed}`;
    dot = ' live';
  } else if (app.recording || app.phase === 'listening') {
    label = 'Dictating';
    dot = ' live';
  } else if (processing) {
    label = app.phase === 'cleaning' ? 'Cleaning up' : 'Transcribing';
    dot = ' warn';
  }
  return (
    <div className="side-status" role="status" aria-label="Session status">
      <span className={`dot${dot}`} aria-hidden="true" />
      <span>{label}</span>
      {app.activeTasks > 0 ? <span className="muted">{app.activeTasks} running</span> : null}
    </div>
  );
}

export function MainApp() {
  const [tab, setTab] = useState<TabId>(loadTab);
  const app = useRelay(selApp);
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});
  const mainRef = useRef<HTMLElement>(null);
  const [overlay, setOverlay] = useState<'palette' | 'sheet' | null>(null);
  const overlayRef = useRef(overlay);
  overlayRef.current = overlay;
  const [focusReq, setFocusReq] = useState(0);

  useEffect(() => {
    try {
      localStorage.setItem('relay.tab', tab);
    } catch {
      // storage unavailable
    }
  }, [tab]);

  // Hotkeys follow settings, so hints show the real keys on every tab.
  const hotkeyToggle = useRelay((s) => s.settings?.hotkeyToggle);
  const hotkeyDictate = useRelay((s) => s.settings?.hotkeyDictate);
  useEffect(() => {
    if (hotkeyToggle && hotkeyDictate) setGlobalHotkeys(hotkeyToggle, hotkeyDictate);
  }, [hotkeyToggle, hotkeyDictate]);

  // Dev-only check that the shortcut table has no clashing keys.
  useEffect(() => {
    if ((import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV) {
      const dups = duplicateKeys();
      if (dups.length > 0) console.error(`Duplicate shortcut keys: ${dups.join(', ')}`);
    }
  }, []);

  // Move focus to the panel after a keyboard-driven tab switch.
  useEffect(() => {
    if (focusReq > 0) mainRef.current?.focus();
  }, [focusReq]);

  // Actions the palette and the key listener can run.
  useEffect(() => {
    const offs = TABS.map((t) =>
      registerAction(`go-${t.id}` as ActionId, () => {
        setTab(t.id);
        setFocusReq((n) => n + 1);
      }),
    );
    offs.push(registerAction('shortcuts', () => setOverlay('sheet')));
    offs.push(registerAction('palette', () => setOverlay('palette')));
    return () => offs.forEach((off) => off());
  }, []);

  // One window-level key listener. It reads the keymap, so rebinds apply without changes here.
  useEffect(() => {
    const cycleRegion = (dir: 1 | -1) => {
      const regions = Array.from(document.querySelectorAll<HTMLElement>('[data-region]')).filter(
        (el) => el.getClientRects().length > 0,
      );
      if (regions.length === 0) return;
      const active = document.activeElement;
      let cur = -1;
      regions.forEach((el, i) => {
        if (active && el.contains(active)) cur = i;
      });
      const next = regions[(cur + dir + regions.length) % regions.length];
      const target = next.querySelector<HTMLElement>('[aria-selected="true"]') ?? next;
      if (target === next && next.tabIndex < 0 && !next.hasAttribute('tabindex')) next.tabIndex = -1;
      target.focus();
    };

    const onWindowKey = (e: globalThis.KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const open = overlayRef.current;
      const text = inTextField(e.target);
      const modified = e.ctrlKey || e.metaKey || e.altKey;
      const plain = !modified && !/^F\d+$/.test(e.key) && e.key !== 'Escape';
      if (text && plain) return;

      if (open) {
        // Dialogs own their keys. Ctrl+K and Ctrl+/ still toggle them.
        if (matches(e, 'palette')) {
          e.preventDefault();
          setOverlay(open === 'palette' ? null : 'palette');
        } else if (matches(e, 'shortcuts')) {
          e.preventDefault();
          setOverlay(open === 'sheet' ? null : 'sheet');
        }
        return;
      }

      if (matches(e, 'region-next') || matches(e, 'region-prev')) {
        e.preventDefault();
        cycleRegion(matches(e, 'region-prev') ? -1 : 1);
        return;
      }

      for (const s of allShortcuts()) {
        if (SKIP_IN_APP.has(s.id) || !matches(e, s.id)) continue;
        if (s.id === 'live-toggle') {
          // In the app the global shortcut already fires and toggles. Only the browser mock needs this.
          if (isTauri) return;
        }
        if (runAction(s.id)) {
          e.preventDefault();
          e.stopPropagation();
          return;
        }
      }
    };
    window.addEventListener('keydown', onWindowKey);
    return () => window.removeEventListener('keydown', onWindowKey);
  }, []);

  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    const i = TABS.findIndex((t) => t.id === tab);
    let n = i;
    if (e.key === 'ArrowDown') n = (i + 1) % TABS.length;
    else if (e.key === 'ArrowUp') n = (i - 1 + TABS.length) % TABS.length;
    else if (e.key === 'Home') n = 0;
    else if (e.key === 'End') n = TABS.length - 1;
    else return;
    e.preventDefault();
    setTab(TABS[n].id);
    refs.current[TABS[n].id]?.focus();
  };

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <Logo size={24} />
          <div className="brand-text">
            <div className="brand-name">Relay</div>
          </div>
        </div>
        <nav
          className="nav"
          role="tablist"
          aria-label="Sections"
          aria-orientation="vertical"
          data-region="nav"
          onKeyDown={onKey}
        >
          {TABS.map((t) => (
            <button
              key={t.id}
              ref={(el) => {
                refs.current[t.id] = el;
              }}
              type="button"
              role="tab"
              id={`tab-${t.id}`}
              aria-selected={tab === t.id}
              aria-controls={`panel-${t.id}`}
              tabIndex={tab === t.id ? 0 : -1}
              className={`nav-item${tab === t.id ? ' on' : ''}`}
              onClick={() => setTab(t.id)}
              title={`${t.label} (${shortcut(`go-${t.id}` as ActionId).keys[0]})`}
              aria-keyshortcuts={ariaKeys(`go-${t.id}` as ActionId)}
            >
              {t.icon}
              <span className="nav-label">{t.label}</span>
              <span className="nav-key" aria-hidden="true">
                <Kbd action={`go-${t.id}` as ActionId} />
              </span>
              {t.id === 'agent' && app.pendingApprovals > 0 ? (
                <span className="count-badge" aria-label={`${app.pendingApprovals} pending approvals`}>
                  {app.pendingApprovals}
                </span>
              ) : null}
            </button>
          ))}
        </nav>
      </aside>
      <main
        ref={mainRef}
        className="canvas"
        role="tabpanel"
        id={`panel-${tab}`}
        aria-labelledby={`tab-${tab}`}
        tabIndex={-1}
        data-region="main"
      >
        {tab === 'live' && <LiveTab />}
        {tab === 'history' && <HistoryTab onOpenEditor={() => setTab('live')} />}
        {tab === 'notes' && <NotesTab />}
        {tab === 'agent' && <AgentTab />}
        {tab === 'settings' && <SettingsTab />}
      </main>
      <footer className="statusbar">
        <StatusLine />
        <span className="spacer" />
        <span className="status-hint">
          <span>Commands</span>
          <Kbd action="palette" />
        </span>
        <span className="status-hint">
          <span>Shortcuts</span>
          <Kbd action="shortcuts" />
        </span>
      </footer>
      <Toasts />
      {overlay === 'palette' ? <Palette onClose={() => setOverlay(null)} /> : null}
      {overlay === 'sheet' ? <ShortcutSheet onClose={() => setOverlay(null)} /> : null}
    </div>
  );
}
