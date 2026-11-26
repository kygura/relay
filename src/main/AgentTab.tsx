import { useEffect, useMemo, useState, type FormEvent } from 'react';
import * as api from '../api';
import { EmptyState, PageHeader } from '../components';
import { IconAlert, IconCheck, IconClose, IconPlay, IconSend, IconTrash } from '../icons';
import {
  patchSettings,
  pushToast,
  removeApproval,
  removeEscalation,
  removeTaskLocal,
  reportError,
  selApprovals,
  selEscalations,
  selTasks,
  upsertTaskLocal,
  useCatalog,
  useRelay,
} from '../store';
import type { ApprovalRequest, Escalation, PermissionLevel, PlanChange, Task, TaskPlan, TaskStatus } from '../types';
import { formatDateTime, formatTime, LEVEL_HINT, LEVEL_LABEL, prettyJson, STATUS_LABEL, TERMINAL } from '../util';
import { FeatureModel, modelName, ModelPicker, NumField, SetupRow, useSettings } from './setup';

export function StatusChip({ status }: { status: TaskStatus }) {
  return <span className={`chip status-${status}`}>{STATUS_LABEL[status]}</span>;
}

export function LevelChip({ level }: { level: PermissionLevel }) {
  return (
    <span className={`chip level-${level}`} title={LEVEL_HINT[level]}>
      {level === 'destructive' ? <IconAlert size={12} /> : null}
      {LEVEL_LABEL[level]}
    </span>
  );
}

function ApprovalCard({ a }: { a: ApprovalRequest }) {
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const destructive = a.level === 'destructive';

  const resolve = async (approve: boolean, remember: boolean) => {
    setBusy(true);
    try {
      await api.agentResolveApproval(a.id, approve, remember);
      removeApproval(a.id);
    } catch (e) {
      reportError('agent_resolve_approval', e);
      setBusy(false);
      setConfirming(false);
    }
  };

  return (
    <article className={`card approval lvl-${a.level}`} aria-label={`Approval request for ${a.tool}`}>
      <div className="approval-head">
        <LevelChip level={a.level} />
        <code className="tool-name">{a.tool}</code>
        <span className="muted">for {a.taskTitle}</span>
        <span className="spacer" />
        <span className="muted">{formatTime(a.createdAt)}</span>
      </div>
      {a.reason ? <p className="approval-reason">{a.reason}</p> : null}
      <div className="eyebrow">Input</div>
      <pre className="json" tabIndex={0}>
        {prettyJson(a.input)}
      </pre>
      {confirming ? (
        <div className="confirm-box" role="alertdialog" aria-label="Confirm destructive approval">
          <p>
            This action is destructive: {LEVEL_HINT.destructive.toLowerCase()} Approving runs it once with the input
            shown above.
          </p>
          <div className="row gap">
            <button type="button" className="btn danger-outline" disabled={busy} onClick={() => resolve(true, false)}>
              <IconCheck size={14} /> Confirm approve
            </button>
            <button type="button" className="btn" disabled={busy} onClick={() => setConfirming(false)}>
              Go back
            </button>
          </div>
        </div>
      ) : (
        <div className="row gap">
          {destructive ? (
            <>
              <button type="button" className="btn" disabled={busy} onClick={() => resolve(false, false)}>
                <IconClose size={14} /> Deny
              </button>
              <button type="button" className="btn danger-outline" disabled={busy} onClick={() => setConfirming(true)}>
                <IconCheck size={14} /> Approve
              </button>
            </>
          ) : (
            <>
              <button type="button" className="btn primary" disabled={busy} onClick={() => resolve(true, false)}>
                <IconCheck size={14} /> Approve
              </button>
              <button type="button" className="btn" disabled={busy} onClick={() => resolve(true, true)}>
                Approve and remember
              </button>
              <button type="button" className="btn" disabled={busy} onClick={() => resolve(false, false)}>
                <IconClose size={14} /> Deny
              </button>
            </>
          )}
        </div>
      )}
      {destructive ? (
        <p className="muted small">Destructive actions always ask. Approvals for them cannot be remembered.</p>
      ) : null}
    </article>
  );
}

function EscalationCard({ e }: { e: Escalation }) {
  const [text, setText] = useState('');
  const [busy, setBusy] = useState(false);

  const submit = async (ev: FormEvent) => {
    ev.preventDefault();
    if (!text.trim()) return;
    setBusy(true);
    try {
      await api.agentReplyEscalation(e.taskId, text.trim());
      removeEscalation(e.taskId);
    } catch (err) {
      reportError('agent_reply_escalation', err);
      setBusy(false);
    }
  };

  return (
    <form className="card escalation" onSubmit={submit} aria-label={`Question from task ${e.taskTitle}`}>
      <div className="approval-head">
        <span className="chip status-escalated">Needs your answer</span>
        <span className="muted">from {e.taskTitle}</span>
        <span className="spacer" />
        <span className="muted">{formatTime(e.createdAt)}</span>
      </div>
      <p className="approval-reason">{e.question}</p>
      <label className="field-label" htmlFor={`reply-${e.taskId}`}>
        Your reply
      </label>
      <textarea id={`reply-${e.taskId}`} rows={2} value={text} onChange={(ev) => setText(ev.target.value)} />
      <div className="row gap">
        <button type="submit" className="btn primary" disabled={busy || !text.trim()}>
          <IconSend size={14} /> Send reply
        </button>
      </div>
    </form>
  );
}

/** New work either starts now or waits in the backlog as a draft. */
function NewTaskForm({ onCreated }: { onCreated: (t: Task) => void }) {
  const [title, setTitle] = useState('');
  const [detail, setDetail] = useState('');
  const [busy, setBusy] = useState(false);

  const create = async (run: boolean) => {
    if (!title.trim()) return;
    setBusy(true);
    try {
      const t = await api.agentCreateTask(title.trim(), detail.trim(), run);
      upsertTaskLocal(t);
      onCreated(t);
      setTitle('');
      setDetail('');
    } catch (e) {
      reportError('agent_create_task', e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="new-task"
      onSubmit={(ev) => {
        ev.preventDefault();
        void create(true);
      }}
      aria-labelledby="nt-h"
    >
      <h2 id="nt-h" className="side-h">
        New task
      </h2>
      <label className="sr-only" htmlFor="nt-title">
        Title
      </label>
      <input
        id="nt-title"
        type="text"
        placeholder="What should the agent do?"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
      />
      <label className="sr-only" htmlFor="nt-detail">
        Details
      </label>
      <textarea
        id="nt-detail"
        rows={3}
        placeholder="Details, constraints, where to put the result"
        value={detail}
        onChange={(e) => setDetail(e.target.value)}
      />
      <div className="row gap wrap">
        <button type="submit" className="btn primary" disabled={busy || !title.trim()}>
          Start now
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy || !title.trim()}
          title="Keep it as a draft. Drafts do not run until you start them."
          onClick={() => void create(false)}
        >
          Add to backlog
        </button>
      </div>
    </form>
  );
}

/** A backlog draft: edit it, start it or delete it. */
function DraftDetail({ task }: { task: Task }) {
  const [title, setTitle] = useState(task.title);
  const [detail, setDetail] = useState(task.detail);
  const [busy, setBusy] = useState(false);

  // Follow the task when the planner or another window rewrites it.
  useEffect(() => {
    setTitle(task.title);
    setDetail(task.detail);
  }, [task.id, task.title, task.detail]);

  const dirty = title.trim() !== task.title || detail.trim() !== task.detail;

  const act = async (scope: string, fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      reportError(scope, e);
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    act('agent_update_task', async () => {
      upsertTaskLocal(await api.agentUpdateTask(task.id, title.trim(), detail.trim()));
    });

  const run = () =>
    act('agent_run_task', async () => {
      if (dirty) upsertTaskLocal(await api.agentUpdateTask(task.id, title.trim(), detail.trim()));
      await api.agentRunTask(task.id);
    });

  const remove = () =>
    act('agent_delete_task', async () => {
      await api.agentDeleteTask(task.id);
      removeTaskLocal(task.id);
    });

  return (
    <section className="task-detail draft" aria-label="Draft">
      <div className="approval-head">
        <StatusChip status="draft" />
        <span className="muted">From {task.source}</span>
        <span className="muted">Created {formatDateTime(task.createdAt)}</span>
      </div>
      <label className="field-label" htmlFor="dr-title">
        Goal
      </label>
      <input id="dr-title" type="text" value={title} onChange={(e) => setTitle(e.target.value)} />
      <label className="field-label" htmlFor="dr-detail">
        Outcome and done condition
      </label>
      <textarea
        id="dr-detail"
        rows={5}
        placeholder="What should exist when this is done, and how to tell it is done"
        value={detail}
        onChange={(e) => setDetail(e.target.value)}
      />
      <div className="row gap wrap">
        <button type="button" className="btn primary" onClick={run} disabled={busy || !title.trim()}>
          <IconPlay size={12} /> Start
        </button>
        <button type="button" className="btn" onClick={save} disabled={busy || !dirty || !title.trim()}>
          Save changes
        </button>
        <span className="spacer" />
        <button type="button" className="btn ghost" onClick={remove} disabled={busy}>
          <IconTrash size={13} /> Delete draft
        </button>
      </div>
      <p className="hint">A draft does not run until you start it. The agent asks before any action above your approval level.</p>
    </section>
  );
}

function TaskDetail({ task }: { task: Task }) {
  const [busy, setBusy] = useState(false);
  const done = TERMINAL.includes(task.status);
  const cancel = async () => {
    setBusy(true);
    try {
      await api.agentCancelTask(task.id);
    } catch (e) {
      reportError('agent_cancel_task', e);
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    setBusy(true);
    try {
      await api.agentDeleteTask(task.id);
      removeTaskLocal(task.id);
    } catch (e) {
      reportError('agent_delete_task', e);
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="task-detail" aria-label="Task detail">
      <div className="approval-head">
        <h2 className="grow">{task.title}</h2>
        <StatusChip status={task.status} />
        {done ? (
          <button type="button" className="btn ghost" onClick={remove} disabled={busy}>
            <IconTrash size={13} /> Delete
          </button>
        ) : (
          <button type="button" className="btn" onClick={cancel} disabled={busy}>
            Cancel task
          </button>
        )}
      </div>
      <div className="meta-line">
        <span>From {task.source}</span>
        <span>Created {formatDateTime(task.createdAt)}</span>
        <span>Updated {formatDateTime(task.updatedAt)}</span>
      </div>
      {task.detail ? <p className="prose">{task.detail}</p> : null}
      {task.result ? (
        <div className="result">
          <div className="field-label">Result</div>
          <p className="prose">{task.result}</p>
        </div>
      ) : null}
      <div className="eyebrow">Activity</div>
      {task.log.length === 0 ? (
        <p className="muted">No activity yet.</p>
      ) : (
        <ol className="timeline">
          {task.log.map((l, i) => (
            <li key={i} className={`tl kind-${l.kind}`}>
              <span className="tl-time">{formatTime(l.at)}</span>
              <span className="tl-dot" aria-hidden="true" />
              <span className="tl-body">
                <span className="tl-kind">{l.kind.replace('_', ' ')}</span>
                <span className="tl-msg">{l.message}</span>
              </span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

const CHANGE_LABEL: Record<PlanChange, string> = {
  kept: 'Kept',
  cleaned: 'Cleaned up',
  split: 'Split',
  merged: 'Merged',
};

/**
 * Organize the backlog. The planner model reads the picked drafts and proposes goals the agent
 * can run. The proposal is shown first; the backlog changes only when it is applied.
 */
function Organize({ drafts, onClose, onApplied }: { drafts: Task[]; onClose: () => void; onApplied: (t: Task[]) => void }) {
  const settings = useSettings();
  const catalog = useCatalog();
  const [picked, setPicked] = useState<Set<string>>(() => new Set(drafts.map((d) => d.id)));
  const [instruction, setInstruction] = useState('');
  const [busy, setBusy] = useState<'plan' | 'apply' | null>(null);
  const [plan, setPlan] = useState<TaskPlan | null>(null);
  /** The ids the shown plan was made from. */
  const [planIds, setPlanIds] = useState<string[]>([]);

  const ids = drafts.filter((d) => picked.has(d.id)).map((d) => d.id);
  const titleOf = (id: string) => drafts.find((d) => d.id === id)?.title ?? 'a removed draft';
  // A plan is stale once one of its drafts was edited away or started.
  const stale = plan !== null && planIds.some((id) => !drafts.some((d) => d.id === id));

  const toggle = (id: string) =>
    setPicked((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const propose = async () => {
    setBusy('plan');
    try {
      const p = await api.agentPlanTasks(ids, instruction.trim());
      setPlan(p);
      setPlanIds(ids);
    } catch (e) {
      reportError('agent_plan_tasks', e);
    } finally {
      setBusy(null);
    }
  };

  const apply = async () => {
    if (!plan) return;
    setBusy('apply');
    try {
      const created = await api.agentApplyPlan(planIds, plan);
      planIds.forEach(removeTaskLocal);
      created.forEach(upsertTaskLocal);
      pushToast('info', 'Backlog updated.', `${created.length} goal${created.length === 1 ? '' : 's'} ready to start.`);
      onApplied(created);
    } catch (e) {
      reportError('agent_apply_plan', e);
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="organize" aria-labelledby="org-h">
      <div className="approval-head">
        <h2 id="org-h" className="grow">
          Organize the backlog
        </h2>
        <button type="button" className="btn ghost" onClick={onClose} disabled={busy !== null}>
          <IconClose size={13} /> Close
        </button>
      </div>
      <p className="prose muted">
        The planner rewrites the drafts you pick into goals the agent can run. It cleans up the wording, splits a draft
        that mixes several outcomes and merges drafts that are the same work. Nothing changes until you apply the plan.
      </p>

      {plan === null ? (
        <>
          <div className="org-pick" role="group" aria-label="Drafts to organize">
            <div className="org-pick-head">
              <span className="eyebrow">
                Drafts <span className="muted tabular">{ids.length} of {drafts.length}</span>
              </span>
              <span className="spacer" />
              <button
                type="button"
                className="link-btn"
                onClick={() => setPicked(ids.length === drafts.length ? new Set() : new Set(drafts.map((d) => d.id)))}
              >
                {ids.length === drafts.length ? 'Pick none' : 'Pick all'}
              </button>
            </div>
            <ul className="org-list">
              {drafts.map((d) => (
                <li key={d.id}>
                  <label className="org-item">
                    <input type="checkbox" checked={picked.has(d.id)} onChange={() => toggle(d.id)} />
                    <span className="org-item-text">
                      <span className="org-item-title">{d.title}</span>
                      {d.detail ? <span className="org-item-detail">{d.detail}</span> : null}
                    </span>
                    <span className="chip subtle">{d.source}</span>
                  </label>
                </li>
              ))}
            </ul>
          </div>
          <label className="field-label" htmlFor="org-ins">
            Guidance (optional)
          </label>
          <textarea
            id="org-ins"
            rows={2}
            placeholder="For example: group by project, keep each goal under an hour of work"
            value={instruction}
            onChange={(e) => setInstruction(e.target.value)}
          />
          <SetupRow label="Planner model" htmlFor="org-model">
            <FeatureModel id="org-model" feature="plannerLlm" />
          </SetupRow>
          <div className="row gap wrap">
            <button type="button" className="btn primary" onClick={propose} disabled={busy !== null || ids.length === 0}>
              {busy === 'plan' ? 'Planning' : 'Propose a plan'}
            </button>
            {busy === 'plan' && settings ? (
              <span className="run-status" role="status">
                Asking {modelName(catalog, settings.plannerLlm)}
              </span>
            ) : null}
          </div>
        </>
      ) : (
        <>
          {plan.summary ? <p className="prose">{plan.summary}</p> : null}
          <div className="eyebrow">
            Goals <span className="muted tabular">{plan.goals.length}</span>
          </div>
          <ol className="plan-goals">
            {plan.goals.map((g, i) => (
              <li key={i} className="plan-goal">
                <div className="plan-goal-head">
                  <span className="plan-goal-title">{g.title}</span>
                  <span className={`chip change-${g.change}`}>{CHANGE_LABEL[g.change] ?? g.change}</span>
                </div>
                {g.detail ? <p className="plan-goal-detail">{g.detail}</p> : null}
                {g.from.length > 0 && g.change !== 'kept' ? (
                  <p className="plan-from">From: {g.from.map(titleOf).join('; ')}</p>
                ) : null}
              </li>
            ))}
          </ol>
          {plan.dropped.length > 0 ? (
            <>
              <div className="eyebrow">
                Dropped <span className="muted tabular">{plan.dropped.length}</span>
              </div>
              <ul className="plan-dropped">
                {plan.dropped.map((d) => (
                  <li key={d.id}>
                    <span className="plan-drop-title">{titleOf(d.id)}</span>
                    <span className="muted">{d.reason}</span>
                  </li>
                ))}
              </ul>
            </>
          ) : null}
          {stale ? (
            <p className="hint warn" role="status">
              The backlog changed since this plan was made. Make a new plan.
            </p>
          ) : null}
          <div className="row gap wrap">
            <button type="button" className="btn primary" onClick={apply} disabled={busy !== null || stale || plan.goals.length === 0}>
              <IconCheck size={14} /> {busy === 'apply' ? 'Applying' : `Replace ${planIds.length} draft${planIds.length === 1 ? '' : 's'} with ${plan.goals.length} goal${plan.goals.length === 1 ? '' : 's'}`}
            </button>
            <button type="button" className="btn" onClick={() => setPlan(null)} disabled={busy !== null}>
              Change the request
            </button>
          </div>
        </>
      )}
    </section>
  );
}

const LEVELS: PermissionLevel[] = ['readOnly', 'localReversible', 'external', 'destructive'];

/** Agent setup, in the place it is used. */
export function AgentSetup({ idPrefix }: { idPrefix: string }) {
  const s = useSettings();
  if (!s) return null;
  return (
    <>
      <SetupRow label="Agent model" htmlFor={`${idPrefix}-agent-model`} hint="Runs tasks with tools. The agent needs Claude.">
        <ModelPicker
          id={`${idPrefix}-agent-model`}
          only="anthropic"
          value={{ source: 'anthropic', model: s.agentModel }}
          onChange={(c) => patchSettings({ agentModel: c.model })}
        />
      </SetupRow>
      <SetupRow label="Planner model" htmlFor={`${idPrefix}-planner-model`} hint="Organizes the backlog. Any source works.">
        <FeatureModel id={`${idPrefix}-planner-model`} feature="plannerLlm" />
      </SetupRow>
      <SetupRow
        label="Run without asking up to"
        htmlFor={`${idPrefix}-level`}
        hint={`${LEVEL_HINT[s.agentAutoLevel]} Destructive actions always ask.`}
      >
        <select id={`${idPrefix}-level`} value={s.agentAutoLevel} onChange={(e) => patchSettings({ agentAutoLevel: e.target.value as PermissionLevel })}>
          {LEVELS.map((l) => (
            <option key={l} value={l}>
              {LEVEL_LABEL[l]}
            </option>
          ))}
        </select>
      </SetupRow>
      <SetupRow label="Tasks at the same time" htmlFor={`${idPrefix}-conc`}>
        <NumField
          id={`${idPrefix}-conc`}
          value={s.agentMaxConcurrent}
          min={1}
          max={8}
          step={1}
          integer
          onChange={(n) => patchSettings({ agentMaxConcurrent: n })}
        />
      </SetupRow>
    </>
  );
}

function TaskButton({ t, on, onPick }: { t: Task; on: boolean; onPick: () => void }) {
  return (
    <li>
      <button type="button" className={`task-item${on ? ' on' : ''}`} aria-current={on} onClick={onPick}>
        <span className="task-title">{t.title}</span>
        <span className="task-item-meta">
          {t.status === 'draft' ? <span>{t.source}</span> : <StatusChip status={t.status} />}
          <span>{formatTime(t.updatedAt)}</span>
        </span>
      </button>
    </li>
  );
}

export function AgentTab() {
  const tasks = useRelay(selTasks);
  const approvals = useRelay(selApprovals);
  const escalations = useRelay(selEscalations);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [organizing, setOrganizing] = useState(false);

  const drafts = useMemo(() => tasks.filter((t) => t.status === 'draft'), [tasks]);
  const runs = useMemo(() => tasks.filter((t) => t.status !== 'draft'), [tasks]);
  const selected = useMemo(
    () => tasks.find((t) => t.id === selectedId) ?? runs[0] ?? drafts[0] ?? null,
    [tasks, runs, drafts, selectedId],
  );
  const escList = Object.values(escalations);

  const open = runs.filter((t) => !TERMINAL.includes(t.status)).length;
  const waiting = approvals.length + escList.length;
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;
  const counts = `${plural(open, 'task')} open, ${plural(drafts.length, 'draft')} in the backlog.`;

  const pick = (id: string) => {
    setSelectedId(id);
    setOrganizing(false);
  };

  return (
    <div className="page fill agent-page">
      <PageHeader
        title="Agent"
        subtitle={
          waiting > 0
            ? `${plural(waiting, 'item')} waiting for you. ${counts}`
            : `${counts} Actions above your approval level pause and ask.`
        }
      />
      <div className="agent">
        <aside className="agent-side">
          <NewTaskForm onCreated={(t) => pick(t.id)} />
          <section aria-labelledby="bl-h" className="task-list-wrap">
            <div className="side-h">
              <h2 id="bl-h" className="side-h">
                Backlog <span className="muted tabular">{drafts.length}</span>
              </h2>
              <span className="spacer" />
              <button
                type="button"
                className={`btn sm${organizing ? ' pressed' : ''}`}
                aria-pressed={organizing}
                disabled={drafts.length === 0}
                title="Clean up, split and merge drafts into goals the agent can run"
                onClick={() => setOrganizing((o) => !o)}
              >
                Organize
              </button>
            </div>
            {drafts.length === 0 ? (
              <p className="empty">
                No drafts. Add one above, or turn on action items in Notes so a meeting fills the backlog.
              </p>
            ) : (
              <ul className="task-list">
                {drafts.map((t) => (
                  <TaskButton key={t.id} t={t} on={!organizing && selected?.id === t.id} onPick={() => pick(t.id)} />
                ))}
              </ul>
            )}
          </section>
          <section aria-labelledby="tl-h" className="task-list-wrap">
            <h2 id="tl-h" className="side-h">
              Tasks <span className="muted tabular">{runs.length}</span>
            </h2>
            {runs.length === 0 ? (
              <p className="empty">No tasks yet.</p>
            ) : (
              <ul className="task-list">
                {runs.map((t) => (
                  <TaskButton key={t.id} t={t} on={!organizing && selected?.id === t.id} onPick={() => pick(t.id)} />
                ))}
              </ul>
            )}
          </section>
          <details className="setup-fold">
            <summary>Agent setup</summary>
            <div className="setup-fold-body">
              <AgentSetup idPrefix="ag" />
            </div>
          </details>
        </aside>
        <div className="agent-main">
          {waiting > 0 ? (
            <section aria-label="Waiting for you" className="stack">
              <h2 className="side-h">Waiting for you</h2>
              {approvals.map((a) => (
                <ApprovalCard key={a.id} a={a} />
              ))}
              {escList.map((e) => (
                <EscalationCard key={e.taskId} e={e} />
              ))}
            </section>
          ) : null}
          {organizing && drafts.length > 0 ? (
            <Organize
              drafts={drafts}
              onClose={() => setOrganizing(false)}
              onApplied={(created) => {
                setOrganizing(false);
                if (created[0]) setSelectedId(created[0].id);
              }}
            />
          ) : selected ? (
            selected.status === 'draft' ? (
              <DraftDetail task={selected} />
            ) : (
              <TaskDetail task={selected} />
            )
          ) : (
            <EmptyState title="No tasks yet">
              <span>
                Describe a task on the left and start it, or add it to the backlog. In Agent mode each spoken request
                starts a task.
              </span>
            </EmptyState>
          )}
        </div>
      </div>
    </div>
  );
}
