import { useRef } from 'react';
import { Kbd } from '../components';
import { allShortcuts, type Scope } from '../keymap';
import { SCOPE_LABEL, useModal } from './Palette';

const ORDER: Scope[] = ['global', 'app', 'live', 'editor', 'history'];

export function ShortcutSheet({ onClose }: { onClose: () => void }) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const { close, onKeyDown } = useModal(onClose, dialogRef);
  const all = allShortcuts();

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div
        ref={dialogRef}
        className="dialog sheet"
        role="dialog"
        aria-modal="true"
        aria-labelledby="sheet-title"
        onKeyDown={onKeyDown}
      >
        <div className="sheet-head">
          <h2 id="sheet-title">Keyboard shortcuts</h2>
          <button type="button" className="btn ghost sm" onClick={close} autoFocus>
            Close
          </button>
        </div>
        <div className="sheet-body">
          {ORDER.map((scope) => {
            const rows = all.filter((s) => s.scope === scope);
            if (rows.length === 0) return null;
            return (
              <section key={scope} aria-labelledby={`sheet-${scope}`}>
                <h3 id={`sheet-${scope}`}>{SCOPE_LABEL[scope]}</h3>
                <ul>
                  {rows.map((s) => (
                    <li key={s.id}>
                      <span>{s.label}</span>
                      <span className="sheet-keys">
                        {s.keys.map((k, i) => (
                          <span key={k} className="sheet-alt">
                            {i > 0 ? <span className="sheet-or">or</span> : null}
                            <Kbd keys={k === '+' ? ['+'] : k.split('+')} />
                          </span>
                        ))}
                      </span>
                    </li>
                  ))}
                </ul>
              </section>
            );
          })}
        </div>
      </div>
    </div>
  );
}
