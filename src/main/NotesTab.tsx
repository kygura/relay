import { useState } from 'react';
import * as api from '../api';
import { EmptyState, PageHeader } from '../components';
import { IconCopy, IconDownload, IconRefresh, IconSend } from '../icons';
import { pushToast, reportError, selNotes, setNotes, upsertTaskLocal, useRelay } from '../store';
import { NotesSetup } from './LiveSetup';
import type { ActionItem, ExportFormat } from '../types';
import { copyText, formatDateTime } from '../util';

const FORMATS: Array<{ value: ExportFormat; label: string }> = [
  { value: 'txt', label: 'TXT' },
  { value: 'markdown', label: 'MD' },
  { value: 'srt', label: 'SRT' },
  { value: 'vtt', label: 'VTT' },
  { value: 'json', label: 'JSON' },
];

function ExportControls() {
  const [format, setFormat] = useState<ExportFormat>('markdown');
  const [withTr, setWithTr] = useState(true);
  const [busy, setBusy] = useState(false);

  const save = async () => {
    setBusy(true);
    try {
      const path = await api.saveExport(format, withTr);
      pushToast('info', 'Export saved.', path);
    } catch (e) {
      reportError('save_export', e);
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    setBusy(true);
    try {
      const text = await api.exportTranscript(format, withTr);
      await copyText(text);
      pushToast('info', 'Export copied to the clipboard.', '');
    } catch (e) {
      reportError('export_transcript', e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="export" role="group" aria-label="Export transcript">
      <div className="seg-control" role="radiogroup" aria-label="Export format">
        {FORMATS.map((f) => (
          <label key={f.value} className={`seg-opt${format === f.value ? ' on' : ''}`}>
            <input
              type="radio"
              name="export-format"
              value={f.value}
              checked={format === f.value}
              onChange={() => setFormat(f.value)}
            />
            <span>{f.label}</span>
          </label>
        ))}
      </div>
      <label className="check">
        <input type="checkbox" checked={withTr} onChange={(e) => setWithTr(e.target.checked)} />
        <span>Include translations</span>
      </label>
      <button type="button" className="btn sm" onClick={save} disabled={busy}>
        <IconDownload size={14} /> Save to Documents/Relay
      </button>
      <button type="button" className="btn sm ghost" onClick={copy} disabled={busy}>
        <IconCopy size={14} /> Copy text
      </button>
    </div>
  );
}

function ActionRow({ item }: { item: ActionItem }) {
  const [state, setState] = useState<'idle' | 'sending' | 'sent'>('idle');

  const send = async () => {
    setState('sending');
    const parts = [
      item.owner ? `Owner: ${item.owner}.` : '',
      item.due ? `Due: ${item.due}.` : '',
      'Created from meeting notes.',
    ].filter(Boolean);
    try {
      // A draft: it waits in the Agent backlog until it is started or organized.
      upsertTaskLocal(await api.agentCreateTask(item.text, parts.join(' '), false));
      setState('sent');
    } catch (e) {
      setState('idle');
      reportError('agent_create_task', e);
    }
  };

  return (
    <li className="action">
      <span className="action-check" aria-hidden="true" />
      <div className="action-main">
        <div className="action-text">{item.text}</div>
        <div className="action-meta">
          <span className="chip subtle">{item.owner ?? 'Unassigned'}</span>
          <span className="chip subtle">{item.due ? `Due ${item.due}` : 'No date'}</span>
        </div>
      </div>
      <button type="button" className={`btn sm${state === 'sent' ? ' ghost' : ''}`} onClick={send} disabled={state !== 'idle'}>
        <IconSend size={14} /> {state === 'sent' ? 'In the backlog' : state === 'sending' ? 'Adding' : 'Add to backlog'}
      </button>
    </li>
  );
}

export function NotesTab() {
  const notes = useRelay(selNotes);
  const [busy, setBusy] = useState(false);

  const generate = async () => {
    setBusy(true);
    try {
      setNotes(await api.generateNotes());
    } catch (e) {
      reportError('generate_notes', e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page notes">
      <PageHeader
        title="Notes"
        subtitle={
          notes
            ? `Covers ${notes.coveredSegments} segments. Updated ${formatDateTime(notes.updatedAt)}.`
            : 'Summary, decisions, action items and open questions from the transcript.'
        }
        actions={
          <button type="button" className="btn primary" onClick={generate} disabled={busy}>
            <IconRefresh size={14} /> {busy ? 'Working' : notes ? 'Refresh notes' : 'Generate notes'}
          </button>
        }
      />
      <section className="export-bar" aria-label="Export">
        <h2>Export transcript</h2>
        <ExportControls />
      </section>
      <details className="setup-fold wide">
        <summary>Notes setup</summary>
        <div className="setup-fold-body cols">
          <NotesSetup idPrefix="nt" />
        </div>
      </details>
      {!notes ? (
        <EmptyState title="No notes yet">
          <span>Notes refresh on their own in Meeting mode. You can also build them now from the current transcript.</span>
        </EmptyState>
      ) : (
        <div className="notes-grid">
          <section className="notes-sec wide notes-summary" aria-labelledby="n-sum">
            <h2 id="n-sum">Summary</h2>
            <p className="prose">{notes.summary || 'No summary yet.'}</p>
          </section>
          <section className="notes-sec" aria-labelledby="n-dec">
            <h2 id="n-dec">Decisions</h2>
            {notes.decisions.length === 0 ? (
              <p className="muted">None recorded.</p>
            ) : (
              <ul className="plain-list">
                {notes.decisions.map((d, i) => (
                  <li key={i}>{d}</li>
                ))}
              </ul>
            )}
          </section>
          <section className="notes-sec q" aria-labelledby="n-q">
            <h2 id="n-q">Open questions</h2>
            {notes.openQuestions.length === 0 ? (
              <p className="muted">None recorded.</p>
            ) : (
              <ul className="plain-list">
                {notes.openQuestions.map((d, i) => (
                  <li key={i}>{d}</li>
                ))}
              </ul>
            )}
          </section>
          <section className="notes-sec wide" aria-labelledby="n-act">
            <h2 id="n-act">Action items</h2>
            {notes.actionItems.length === 0 ? (
              <p className="muted">None recorded.</p>
            ) : (
              <ul className="actions">
                {notes.actionItems.map((a, i) => (
                  <ActionRow key={`${i}-${a.text}`} item={a} />
                ))}
              </ul>
            )}
          </section>
        </div>
      )}
    </div>
  );
}
