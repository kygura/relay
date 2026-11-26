//! The agent runtime: runs tasks as tool-use loops on the Claude API behind
//! the permission gate, and holds pending approvals and escalations.

use crate::agent::planner::{self, TaskPlan};
use crate::agent::prompt;
use crate::agent::store::{now, TaskStore};
use crate::agent::tools::{self, GateDecision, ToolOutput};
use crate::events;
use crate::llm::{ClaudeClient, Effort, LlmRequest};
use crate::settings::SharedSettings;
use crate::types::{
    ApprovalRequest, Escalation, InsertMethod, PermissionLevel, SharedTranscript, Task,
    TaskLogEntry, TaskStatus,
};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::Emitter;
use tokio::sync::{oneshot, Notify, Semaphore};

const MAX_ITERATIONS: usize = 40;
const MAX_TOKENS: u32 = 16_000;
const MAX_SUBTASKS_PER_TASK: usize = 3;
const KEEP_SCREENSHOTS: usize = 3;
const MAX_NUDGES: usize = 2;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Cancel flag plus a wake-up for anything waiting on the task.
struct TaskCtl {
    cancelled: AtomicBool,
    notify: Notify,
}

impl TaskCtl {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Resolves once the task is cancelled.
    async fn cancelled(&self) {
        loop {
            let waiter = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            waiter.await;
        }
    }
}

/// Per-task state that lives for one run.
struct TaskRun {
    task: Task,
    ctl: Arc<TaskCtl>,
    remembered: HashSet<String>,
    subtasks: usize,
}

enum Outcome {
    Done(String),
    Failed(String),
    Cancelled,
}

enum ApprovalOutcome {
    Approved { remember: bool },
    Denied,
    Cancelled,
}

pub struct AgentRuntime {
    app: tauri::AppHandle,
    store: TaskStore,
    settings: SharedSettings,
    transcript: SharedTranscript,
    semaphore: Arc<Semaphore>,
    approvals: Mutex<HashMap<String, (ApprovalRequest, oneshot::Sender<(bool, bool)>)>>,
    escalations: Mutex<HashMap<String, oneshot::Sender<String>>>,
    ctls: Mutex<HashMap<String, Arc<TaskCtl>>>,
}

impl AgentRuntime {
    /// Marks tasks left live by a previous run as Failed, and sizes the
    /// concurrency limit from `settings.agent_max_concurrent`.
    pub fn new(
        app: tauri::AppHandle,
        store: TaskStore,
        settings: SharedSettings,
        transcript: SharedTranscript,
    ) -> Arc<Self> {
        let max = settings
            .try_read()
            .map(|s| s.agent_max_concurrent)
            .unwrap_or(2)
            .max(1);
        match store.fail_interrupted() {
            Ok(0) => {}
            Ok(n) => tracing::info!("Marked {n} interrupted agent task(s) as failed"),
            Err(e) => tracing::warn!("Could not clean up interrupted tasks: {e:#}"),
        }
        Arc::new(Self {
            app,
            store,
            settings,
            transcript,
            semaphore: Arc::new(Semaphore::new(max)),
            approvals: Mutex::new(HashMap::new()),
            escalations: Mutex::new(HashMap::new()),
            ctls: Mutex::new(HashMap::new()),
        })
    }

    /// Create a task, persist it and start it in the background. Not async:
    /// the work is spawned on the Tauri async runtime.
    pub fn create_task(
        self: &Arc<Self>,
        title: impl Into<String>,
        detail: impl Into<String>,
        source: &str,
    ) -> Result<Task> {
        self.create(title, detail, source, true)
    }

    /// Store a draft. It does not run until `run_draft` is called.
    pub fn create_draft(
        self: &Arc<Self>,
        title: impl Into<String>,
        detail: impl Into<String>,
        source: &str,
    ) -> Result<Task> {
        self.create(title, detail, source, false)
    }

    /// Create a task. With `run` it starts at once, otherwise it is stored as a draft.
    pub fn create(
        self: &Arc<Self>,
        title: impl Into<String>,
        detail: impl Into<String>,
        source: &str,
        run: bool,
    ) -> Result<Task> {
        let status = if run { TaskStatus::Pending } else { TaskStatus::Draft };
        let task = new_task(title.into(), detail.into(), source, status, &now());
        self.store.insert(&task)?;
        self.emit_task(&task);
        if run {
            self.start(&task);
        }
        Ok(task)
    }

    /// Register the task and run it in the background.
    fn start(self: &Arc<Self>, task: &Task) {
        let ctl = TaskCtl::new();
        lock(&self.ctls).insert(task.id.clone(), ctl.clone());
        let rt = Arc::clone(self);
        let id = task.id.clone();
        tauri::async_runtime::spawn(async move {
            rt.run_task(id, ctl).await;
        });
    }

    fn require_draft(&self, id: &str) -> Result<Task> {
        match self.store.get(id)? {
            Some(t) if t.status == TaskStatus::Draft => Ok(t),
            Some(_) => Err(anyhow!("Only a draft can do that.")),
            None => Err(anyhow!("That task no longer exists.")),
        }
    }

    /// Start a draft the same way a new task starts.
    pub fn run_draft(self: &Arc<Self>, id: &str) -> Result<Task> {
        let mut task = self.require_draft(id)?;
        if lock(&self.ctls).contains_key(id) {
            return Err(anyhow!("That task is already starting."));
        }
        task.status = TaskStatus::Pending;
        task.updated_at = now();
        self.store.update(&task)?;
        self.emit_task(&task);
        self.start(&task);
        Ok(task)
    }

    /// Change the title and detail of a draft.
    pub fn update_draft(&self, id: &str, title: &str, detail: &str) -> Result<Task> {
        let title = title.trim();
        if title.is_empty() {
            return Err(anyhow!("A task needs a title."));
        }
        let mut task = self.require_draft(id)?;
        task.title = title.to_string();
        task.detail = detail.trim().to_string();
        task.updated_at = now();
        self.store.update(&task)?;
        self.emit_task(&task);
        Ok(task)
    }

    /// Delete a draft or a finished task.
    pub fn delete_task(&self, id: &str) -> Result<()> {
        let removable = [TaskStatus::Draft, TaskStatus::Done, TaskStatus::Failed, TaskStatus::Cancelled];
        if self.store.delete(id, &removable)? {
            self.emit_removed(id);
            return Ok(());
        }
        match self.store.get(id)? {
            Some(_) => Err(anyhow!("A task that is running cannot be deleted. Cancel it first.")),
            None => Err(anyhow!("That task no longer exists.")),
        }
    }

    /// Title keys of every draft and every task that has not finished.
    pub fn open_title_keys(&self) -> Result<HashSet<String>> {
        Ok(self
            .store
            .list()?
            .iter()
            .filter(|t| !t.status.is_terminal())
            .map(|t| title_key(&t.title))
            .collect())
    }

    /// The tasks with these ids, in the order given. Every one must be a draft, and
    /// there must be between 1 and `MAX_PLAN_TASKS` of them.
    pub fn drafts(&self, ids: &[String]) -> Result<Vec<Task>> {
        if ids.is_empty() {
            return Err(anyhow!("Select at least one draft."));
        }
        if ids.len() > planner::MAX_PLAN_TASKS {
            return Err(anyhow!("Select at most {} drafts.", planner::MAX_PLAN_TASKS));
        }
        let mut seen = HashSet::new();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if !seen.insert(id.as_str()) {
                continue;
            }
            out.push(self.require_draft(id)?);
        }
        Ok(out)
    }

    /// Replace the drafts `ids` with one new draft per goal, all or nothing.
    pub fn apply_plan(&self, ids: &[String], plan: &TaskPlan) -> Result<Vec<Task>> {
        let old = self.drafts(ids)?;
        let at = now();
        let mut tasks: Vec<Task> = Vec::new();
        for g in &plan.goals {
            let title = g.title.trim();
            if title.is_empty() {
                continue;
            }
            let source = g
                .from
                .iter()
                .find_map(|id| old.iter().find(|t| &t.id == id))
                .map(|t| t.source.as_str())
                .unwrap_or("agent");
            tasks.push(new_task(title.to_string(), g.detail.trim().to_string(), source, TaskStatus::Draft, &at));
        }
        if tasks.is_empty() {
            return Err(anyhow!("The plan has no goals."));
        }
        let old_ids: Vec<String> = old.iter().map(|t| t.id.clone()).collect();
        // Inserted last goal first so the list, which shows the newest first, reads in plan order.
        let reversed: Vec<Task> = tasks.iter().rev().cloned().collect();
        self.store.replace_drafts(&old_ids, &reversed)?;
        for id in &old_ids {
            self.emit_removed(id);
        }
        for t in &tasks {
            self.emit_task(t);
        }
        Ok(tasks)
    }

    /// Request cancellation. A running task stops at its next wait point.
    pub fn cancel(&self, id: &str) {
        let ctl = lock(&self.ctls).get(id).cloned();
        if let Some(ctl) = ctl {
            ctl.cancel();
            return;
        }
        // Not running in this process: close it in the store if it is still open.
        if let Ok(Some(mut task)) = self.store.get(id) {
            if !task.status.is_terminal() {
                task.status = TaskStatus::Cancelled;
                task.updated_at = now();
                let entry = TaskLogEntry {
                    at: task.updated_at.clone(),
                    kind: "info".into(),
                    message: "Task cancelled.".into(),
                };
                let _ = self.store.update(&task);
                let _ = self.store.append_log(&task.id, &entry);
                task.log.push(entry);
                self.emit_task(&task);
            }
        }
    }

    pub fn resolve_approval(&self, approval_id: &str, approve: bool, remember: bool) -> Result<()> {
        let entry = lock(&self.approvals).remove(approval_id);
        match entry {
            Some((_, tx)) => tx
                .send((approve, remember))
                .map_err(|_| anyhow!("The task is no longer waiting for this approval.")),
            None => Err(anyhow!("No pending approval with id {approval_id}.")),
        }
    }

    pub fn reply_escalation(&self, task_id: &str, text: &str) -> Result<()> {
        let tx = lock(&self.escalations).remove(task_id);
        match tx {
            Some(tx) => tx
                .send(text.to_string())
                .map_err(|_| anyhow!("The task is no longer waiting for a reply.")),
            None => Err(anyhow!("That task is not waiting for a reply.")),
        }
    }

    pub fn list(&self) -> Result<Vec<Task>> {
        self.store.list()
    }

    /// Approvals waiting for a decision, oldest first.
    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        let mut v: Vec<ApprovalRequest> = lock(&self.approvals)
            .values()
            .map(|(req, _)| req.clone())
            .collect();
        v.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        v
    }

    /// Tasks that have not reached a final status in this process.
    pub fn active_count(&self) -> usize {
        lock(&self.ctls).len()
    }

    // -----------------------------------------------------------------------
    // Persistence and events

    fn emit_task(&self, task: &Task) {
        let _ = self.app.emit(events::AGENT_TASK, task);
    }

    fn emit_removed(&self, id: &str) {
        let _ = self.app.emit(events::AGENT_TASK_REMOVED, json!({ "id": id }));
    }

    fn log(&self, run: &mut TaskRun, kind: &str, message: impl Into<String>) {
        let entry = TaskLogEntry {
            at: now(),
            kind: kind.to_string(),
            message: message.into(),
        };
        if let Err(e) = self.store.append_log(&run.task.id, &entry) {
            tracing::warn!("Could not save a task log entry: {e:#}");
        }
        run.task.updated_at = entry.at.clone();
        run.task.log.push(entry);
        self.emit_task(&run.task);
    }

    fn set_status(&self, run: &mut TaskRun, status: TaskStatus) {
        run.task.status = status;
        run.task.updated_at = now();
        if let Err(e) = self.store.update(&run.task) {
            tracing::warn!("Could not save task status: {e:#}");
        }
        self.emit_task(&run.task);
    }

    // -----------------------------------------------------------------------
    // Task lifecycle

    async fn run_task(self: Arc<Self>, id: String, ctl: Arc<TaskCtl>) {
        let task = match self.store.get(&id) {
            Ok(Some(t)) => t,
            _ => {
                lock(&self.ctls).remove(&id);
                return;
            }
        };
        let mut run = TaskRun {
            task,
            ctl,
            remembered: HashSet::new(),
            subtasks: 0,
        };
        let outcome = match self.run_inner(&mut run).await {
            Ok(o) => o,
            Err(e) => Outcome::Failed(format!("{e:#}")),
        };
        match outcome {
            Outcome::Done(summary) => {
                run.task.result = Some(summary.clone());
                self.log(&mut run, "info", format!("Task completed: {}", tools::truncate_chars(&summary, 300)));
                self.set_status(&mut run, TaskStatus::Done);
            }
            Outcome::Failed(reason) => {
                run.task.result = Some(reason.clone());
                self.log(&mut run, "error", format!("Task failed: {}", tools::truncate_chars(&reason, 300)));
                self.set_status(&mut run, TaskStatus::Failed);
            }
            Outcome::Cancelled => {
                self.log(&mut run, "info", "Task cancelled.");
                self.set_status(&mut run, TaskStatus::Cancelled);
            }
        }
        // Drop anything still waiting on this task.
        let dropped: Vec<String> = {
            let mut map = lock(&self.approvals);
            let ids: Vec<String> = map
                .iter()
                .filter(|(_, (req, _))| req.task_id == id)
                .map(|(k, _)| k.clone())
                .collect();
            for k in &ids {
                map.remove(k);
            }
            ids
        };
        for approval_id in dropped {
            let _ = self.app.emit(events::AGENT_APPROVAL_RESOLVED, json!({ "id": approval_id }));
        }
        lock(&self.escalations).remove(&id);
        lock(&self.ctls).remove(&id);
    }

    async fn run_inner(self: &Arc<Self>, run: &mut TaskRun) -> Result<Outcome> {
        let ctl = run.ctl.clone();

        // Wait for a free slot.
        let _permit = tokio::select! {
            p = self.semaphore.clone().acquire_owned() => {
                p.map_err(|_| anyhow!("The agent scheduler is closed."))?
            }
            _ = ctl.cancelled() => return Ok(Outcome::Cancelled),
        };

        self.set_status(run, TaskStatus::Running);
        self.log(run, "info", "Task started.");

        // The agent needs tool use, so it always runs on Claude.
        let model = self.settings.read().await.choice_for(crate::settings::LlmTask::Agent).model;
        if crate::settings::get_api_key("anthropic").is_none() {
            let msg = "The agent needs a Claude API key.".to_string();
            self.log(run, "error", &msg);
            return Ok(Outcome::Failed(msg));
        }
        let client = ClaudeClient::new(model)?;
        let system = prompt::system_blocks();
        let tool_defs = tools::api_definitions();
        let mut messages: Vec<Value> = vec![json!({
            "role": "user",
            "content": prompt::first_user_message(&run.task.title, &run.task.detail)
        })];

        let mut iterations = 0usize;
        let mut consecutive_errors = 0usize;
        let mut max_token_hits = 0usize;
        let mut nudges = 0usize;

        loop {
            if ctl.is_cancelled() {
                return Ok(Outcome::Cancelled);
            }

            if iterations >= MAX_ITERATIONS {
                let q = format!(
                    "This task has used {MAX_ITERATIONS} steps without finishing. Reply to let it continue for another {MAX_ITERATIONS} steps, or cancel the task."
                );
                match self.escalate(run, &q).await {
                    Some(reply) => {
                        push_user_blocks(&mut messages, vec![text_block(&format!("The user replied: {reply}"))]);
                        iterations = 0;
                    }
                    None => return Ok(Outcome::Cancelled),
                }
            }
            iterations += 1;

            let req = LlmRequest {
                system: system.clone(),
                messages: messages.clone(),
                tools: tool_defs.clone(),
                max_tokens: MAX_TOKENS,
                effort: Effort::High,
                output_format: None,
            };
            let mut sink = |_: &str| {};
            let result = tokio::select! {
                r = client.stream(req, &mut sink) => r,
                _ = ctl.cancelled() => return Ok(Outcome::Cancelled),
            };
            let resp = match result {
                Ok(r) => r,
                Err(e) => {
                    let msg = format!("{e:#}");
                    self.log(run, "error", tools::truncate_chars(&msg, 500));
                    return Ok(Outcome::Failed(msg));
                }
            };

            let spoken = resp.text();
            let spoken = spoken.trim().to_string();
            if !spoken.is_empty() {
                self.log(run, "assistant", tools::truncate_chars(&spoken, 500));
            }
            let uses = resp.tool_uses();

            match resp.stop_reason.as_str() {
                "refusal" => {
                    let details = resp
                        .stop_details
                        .as_ref()
                        .map(|d| tools::truncate_chars(&d.to_string(), 300))
                        .unwrap_or_default();
                    self.log(run, "error", format!("The model declined to continue. {details}"));
                    let q = "The model declined to continue this task. Reply with a rephrased request or more context, or cancel the task.";
                    match self.escalate(run, q).await {
                        Some(reply) => push_user_blocks(
                            &mut messages,
                            vec![text_block(&format!("The user replied: {reply}"))],
                        ),
                        None => return Ok(Outcome::Cancelled),
                    }
                    continue;
                }
                "pause_turn" => {
                    push_assistant(&mut messages, &resp.content);
                    continue;
                }
                "max_tokens" => {
                    max_token_hits += 1;
                    push_assistant(&mut messages, &resp.content);
                    let mut blocks: Vec<Value> = uses
                        .iter()
                        .map(|(id, _, _)| {
                            tools::error_result_block(
                                id,
                                "The reply hit the output limit, so this tool input was incomplete and the tool was not run. Retry with a shorter input.",
                            )
                        })
                        .collect();
                    if blocks.is_empty() {
                        blocks.push(text_block(
                            "Your last reply hit the output limit and was cut off. Continue with shorter output.",
                        ));
                    }
                    push_user_blocks(&mut messages, blocks);
                    if max_token_hits >= 2 {
                        let q = "The model ran out of output space twice in a row. Reply with guidance, for example a smaller scope, or cancel the task.";
                        match self.escalate(run, q).await {
                            Some(reply) => {
                                push_user_blocks(
                                    &mut messages,
                                    vec![text_block(&format!("The user replied: {reply}"))],
                                );
                                max_token_hits = 0;
                            }
                            None => return Ok(Outcome::Cancelled),
                        }
                    }
                    continue;
                }
                _ => {}
            }

            if uses.is_empty() {
                // The model stopped without calling a control tool.
                push_assistant(&mut messages, &resp.content);
                nudges += 1;
                if nudges > MAX_NUDGES {
                    let summary = if spoken.is_empty() {
                        "The agent stopped without a summary.".to_string()
                    } else {
                        spoken
                    };
                    return Ok(Outcome::Done(summary));
                }
                push_user_blocks(
                    &mut messages,
                    vec![text_block(
                        "Call complete_task with a summary if the task is done, fail_task with a reason if it cannot be done, or ask_user if you need an answer from the user.",
                    )],
                );
                continue;
            }

            // Tool turn.
            push_assistant(&mut messages, &resp.content);
            let mut results: Vec<Value> = Vec::new();

            for (id, name, input) in uses {
                if ctl.is_cancelled() {
                    return Ok(Outcome::Cancelled);
                }

                if resp.incomplete_tool_ids.contains(&id) {
                    self.log(run, "error", format!("{name}: the tool input was incomplete, so it was not run."));
                    results.push(tools::error_result_block(
                        &id,
                        "The tool input was incomplete or not valid JSON, so the tool was not run. Retry with a complete input.",
                    ));
                    consecutive_errors += 1;
                    continue;
                }

                match name.as_str() {
                    tools::TOOL_COMPLETE => {
                        let summary = input
                            .get("summary")
                            .and_then(Value::as_str)
                            .unwrap_or("Done.")
                            .to_string();
                        return Ok(Outcome::Done(summary));
                    }
                    tools::TOOL_FAIL => {
                        let reason = input
                            .get("reason")
                            .and_then(Value::as_str)
                            .unwrap_or("The agent gave no reason.")
                            .to_string();
                        return Ok(Outcome::Failed(reason));
                    }
                    tools::TOOL_ASK_USER => {
                        let question = input
                            .get("question")
                            .and_then(Value::as_str)
                            .unwrap_or("The agent needs an answer.")
                            .to_string();
                        match self.escalate(run, &question).await {
                            Some(reply) => {
                                results.push(
                                    ToolOutput::text(format!("The user replied: {reply}")).to_result_block(&id),
                                );
                                consecutive_errors = 0;
                            }
                            None => return Ok(Outcome::Cancelled),
                        }
                        continue;
                    }
                    _ => {}
                }

                let Some(spec) = tools::find(&name) else {
                    self.log(run, "error", format!("Unknown tool: {name}"));
                    results.push(tools::error_result_block(&id, &format!("Unknown tool: {name}")));
                    consecutive_errors += 1;
                    continue;
                };

                self.log(
                    run,
                    "tool_call",
                    format!("{} {}", name, tools::truncate_chars(&input.to_string(), 300)),
                );

                let level = tools::effective_level(&name, &input);
                let (auto_level, insert_method) = {
                    let s = self.settings.read().await;
                    (s.agent_auto_level, s.insert_method)
                };
                let remembered = run.remembered.contains(&name);
                if tools::gate_decision(level, auto_level, remembered) == GateDecision::Ask {
                    let reason = if spoken.is_empty() {
                        spec.description.to_string()
                    } else {
                        tools::truncate_chars(&spoken, 200)
                    };
                    match self.request_approval(run, &name, level, &input, &reason).await {
                        ApprovalOutcome::Cancelled => return Ok(Outcome::Cancelled),
                        ApprovalOutcome::Denied => {
                            self.log(run, "approval", format!("Denied: {name}"));
                            results.push(tools::error_result_block(
                                &id,
                                "The user denied this action. Do not repeat it. Choose another approach or call ask_user.",
                            ));
                            continue;
                        }
                        ApprovalOutcome::Approved { remember } => {
                            self.log(run, "approval", format!("Approved: {name}"));
                            if remember && level != PermissionLevel::Destructive {
                                run.remembered.insert(name.clone());
                            }
                        }
                    }
                }

                let out = match self.run_tool(run, &name, &input, insert_method).await {
                    Some(o) => o,
                    None => return Ok(Outcome::Cancelled),
                };
                let shown = tools::truncate_chars(&out.text, 300);
                if out.is_error {
                    self.log(run, "tool_result", format!("error: {shown}"));
                    consecutive_errors += 1;
                } else {
                    self.log(run, "tool_result", shown);
                    consecutive_errors = 0;
                }
                results.push(out.to_result_block(&id));
            }

            push_user_blocks(&mut messages, results);
            prune_old_images(&mut messages, KEEP_SCREENSHOTS);

            if consecutive_errors >= 3 {
                let q = "Three tool calls in a row failed. Reply with guidance, or cancel the task.";
                match self.escalate(run, q).await {
                    Some(reply) => {
                        push_user_blocks(
                            &mut messages,
                            vec![text_block(&format!("The user replied: {reply}"))],
                        );
                        consecutive_errors = 0;
                    }
                    None => return Ok(Outcome::Cancelled),
                }
            }
        }
    }

    /// Run one non-control tool. Returns None when the task was cancelled meanwhile.
    async fn run_tool(
        self: &Arc<Self>,
        run: &mut TaskRun,
        name: &str,
        input: &Value,
        insert_method: InsertMethod,
    ) -> Option<ToolOutput> {
        let text_arg = |key: &str| -> Option<String> {
            input.get(key).and_then(Value::as_str).map(String::from)
        };
        match name {
            "add_task_note" => {
                let Some(text) = text_arg("text") else {
                    return Some(ToolOutput::error("Missing argument: text."));
                };
                self.log(run, "info", format!("Note: {}", tools::truncate_chars(&text, 500)));
                Some(ToolOutput::text("Note added."))
            }
            "create_task" => {
                let (Some(title), Some(detail)) = (text_arg("title"), text_arg("detail")) else {
                    return Some(ToolOutput::error("Missing argument: title or detail."));
                };
                if run.subtasks >= MAX_SUBTASKS_PER_TASK {
                    return Some(ToolOutput::error(format!(
                        "A task can create at most {MAX_SUBTASKS_PER_TASK} other tasks."
                    )));
                }
                match self.create_task(title, detail, "agent") {
                    Ok(t) => {
                        run.subtasks += 1;
                        Some(ToolOutput::text(format!("Created task {} ({}).", t.id, t.title)))
                    }
                    Err(e) => Some(ToolOutput::error(format!("Could not create the task: {e:#}"))),
                }
            }
            "list_tasks" => match self.store.list() {
                Ok(tasks) => {
                    if tasks.is_empty() {
                        return Some(ToolOutput::text("There are no tasks."));
                    }
                    let lines: Vec<String> = tasks
                        .iter()
                        .take(30)
                        .map(|t| format!("{} | {} | {}", t.id, t.status.as_str(), t.title))
                        .collect();
                    Some(ToolOutput::text(lines.join("\n")))
                }
                Err(e) => Some(ToolOutput::error(format!("Could not list tasks: {e:#}"))),
            },
            "get_transcript" => {
                let n = input
                    .get("last_n")
                    .and_then(Value::as_i64)
                    .unwrap_or(20)
                    .clamp(1, 200) as usize;
                let t = self.transcript.read().await;
                Some(ToolOutput::text(tools::format_transcript(&t.segments, n)))
            }
            _ => {
                let ctl = run.ctl.clone();
                tokio::select! {
                    r = tools::run_os_tool(name, input, insert_method) => Some(match r {
                        Ok(o) => o,
                        Err(e) => ToolOutput::error(format!("{e:#}")),
                    }),
                    _ = ctl.cancelled() => None,
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Waiting on the user

    async fn request_approval(
        &self,
        run: &mut TaskRun,
        tool: &str,
        level: PermissionLevel,
        input: &Value,
        reason: &str,
    ) -> ApprovalOutcome {
        let id = uuid::Uuid::new_v4().to_string();
        let req = ApprovalRequest {
            id: id.clone(),
            task_id: run.task.id.clone(),
            task_title: run.task.title.clone(),
            tool: tool.to_string(),
            level,
            input: input.clone(),
            reason: reason.to_string(),
            created_at: now(),
        };
        let (tx, rx) = oneshot::channel::<(bool, bool)>();
        lock(&self.approvals).insert(id.clone(), (req.clone(), tx));
        self.set_status(run, TaskStatus::WaitingApproval);
        self.log(run, "approval", format!("Waiting for approval: {tool} ({level:?})"));
        let _ = self.app.emit(events::AGENT_APPROVAL, &req);

        let ctl = run.ctl.clone();
        let decision = tokio::select! {
            r = rx => r.ok(),
            _ = ctl.cancelled() => None,
        };
        lock(&self.approvals).remove(&id);
        let _ = self.app.emit(events::AGENT_APPROVAL_RESOLVED, json!({ "id": id }));

        if ctl.is_cancelled() {
            return ApprovalOutcome::Cancelled;
        }
        self.set_status(run, TaskStatus::Running);
        match decision {
            Some((true, remember)) => ApprovalOutcome::Approved { remember },
            _ => ApprovalOutcome::Denied,
        }
    }

    /// Ask the user a question. Returns None when the task is cancelled.
    async fn escalate(&self, run: &mut TaskRun, question: &str) -> Option<String> {
        let (tx, rx) = oneshot::channel::<String>();
        lock(&self.escalations).insert(run.task.id.clone(), tx);
        self.set_status(run, TaskStatus::Escalated);
        self.log(run, "escalation", tools::truncate_chars(question, 500));
        let esc = Escalation {
            task_id: run.task.id.clone(),
            task_title: run.task.title.clone(),
            question: question.to_string(),
            created_at: now(),
        };
        let _ = self.app.emit(events::AGENT_ESCALATION, &esc);

        let ctl = run.ctl.clone();
        let reply = tokio::select! {
            r = rx => r.ok(),
            _ = ctl.cancelled() => None,
        };
        lock(&self.escalations).remove(&run.task.id);
        match reply {
            Some(text) if !ctl.is_cancelled() => {
                self.set_status(run, TaskStatus::Running);
                self.log(run, "info", format!("User replied: {}", tools::truncate_chars(&text, 300)));
                Some(text)
            }
            _ => None,
        }
    }
}

/// Titles are compared trimmed and without regard to case.
pub fn title_key(title: &str) -> String {
    title.trim().to_lowercase()
}

fn new_task(title: String, detail: String, source: &str, status: TaskStatus, at: &str) -> Task {
    Task {
        id: uuid::Uuid::new_v4().to_string(),
        title,
        detail,
        status,
        source: source.to_string(),
        created_at: at.to_string(),
        updated_at: at.to_string(),
        result: None,
        log: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Message helpers

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn push_assistant(messages: &mut Vec<Value>, content: &[Value]) {
    if !content.is_empty() {
        messages.push(json!({"role": "assistant", "content": content}));
    }
}

/// Add blocks to the last user message, or start a new one. Keeps roles alternating.
fn push_user_blocks(messages: &mut Vec<Value>, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut() {
        if last.get("role").and_then(Value::as_str) == Some("user") {
            let old = last["content"].take();
            let mut arr: Vec<Value> = match old {
                Value::Array(a) => a,
                Value::String(s) => vec![text_block(&s)],
                Value::Null => Vec::new(),
                other => vec![other],
            };
            arr.extend(blocks);
            last["content"] = Value::Array(arr);
            return;
        }
    }
    messages.push(json!({"role": "user", "content": blocks}));
}

/// Replace all but the newest `keep` screenshots with a short text note.
fn prune_old_images(messages: &mut [Value], keep: usize) {
    let mut seen = 0usize;
    for msg in messages.iter_mut().rev() {
        let Some(content) = msg.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in content.iter_mut().rev() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(inner) = block.get_mut("content").and_then(Value::as_array_mut) else {
                continue;
            };
            for sub in inner.iter_mut().rev() {
                if sub.get("type").and_then(Value::as_str) == Some("image") {
                    seen += 1;
                    if seen > keep {
                        *sub = text_block("[older screenshot removed to save space]");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot_message(tag: &str) -> Value {
        json!({"role": "user", "content": [
            ToolOutput {
                text: tag.to_string(),
                image_b64: Some("AAAA".into()),
                is_error: false,
            }.to_result_block(tag)
        ]})
    }

    #[test]
    fn titles_compare_trimmed_and_case_insensitive() {
        assert_eq!(title_key("  Send the Report "), title_key("send the report"));
        assert_ne!(title_key("send the report"), title_key("send the reports"));
    }

    #[test]
    fn new_tasks_carry_status_and_source() {
        let t = new_task("T".into(), "D".into(), "meeting", TaskStatus::Draft, "2026-01-01T00:00:00Z");
        assert_eq!(t.status, TaskStatus::Draft);
        assert_eq!(t.source, "meeting");
        assert_eq!(t.created_at, t.updated_at);
        assert!(t.result.is_none() && t.log.is_empty());
    }

    #[test]
    fn old_screenshots_are_pruned() {
        let mut msgs = vec![
            shot_message("a"),
            json!({"role": "assistant", "content": [{"type": "text", "text": "x"}]}),
            shot_message("b"),
            shot_message("c"),
        ];
        prune_old_images(&mut msgs, 2);
        assert_eq!(msgs[0]["content"][0]["content"][1]["type"], "text");
        assert_eq!(msgs[2]["content"][0]["content"][1]["type"], "image");
        assert_eq!(msgs[3]["content"][0]["content"][1]["type"], "image");
    }

    #[test]
    fn user_blocks_merge_into_last_user_message() {
        let mut msgs = vec![json!({"role": "user", "content": "first"})];
        push_user_blocks(&mut msgs, vec![text_block("second")]);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["content"][0]["text"], "first");
        assert_eq!(msgs[0]["content"][1]["text"], "second");

        msgs.push(json!({"role": "assistant", "content": [{"type": "text", "text": "ok"}]}));
        push_user_blocks(&mut msgs, vec![text_block("third")]);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[2]["role"], "user");
    }

    #[test]
    fn empty_assistant_content_is_not_pushed() {
        let mut msgs: Vec<Value> = vec![];
        push_assistant(&mut msgs, &[]);
        assert!(msgs.is_empty());
    }
}
