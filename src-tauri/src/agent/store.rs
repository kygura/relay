//! SQLite persistence for agent tasks and their logs.

use crate::types::{Task, TaskLogEntry, TaskStatus};
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

/// Current time as an RFC 3339 string.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[derive(Clone)]
pub struct TaskStore {
    conn: Arc<Mutex<Connection>>,
}

impl TaskStore {
    /// Open or create the database at `path` (normally `{app_data_dir}/relay.db`).
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).context("Could not create the data folder")?;
            }
        }
        let conn = Connection::open(path).context("Could not open the task database")?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS tasks (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL,
                 detail TEXT NOT NULL,
                 status TEXT NOT NULL,
                 source TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 result TEXT
             );
             CREATE TABLE IF NOT EXISTS task_log (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                 at TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 message TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_task_log_task ON task_log(task_id);",
        )
        .context("Could not create the task tables")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow!("The task database lock is poisoned."))
    }

    /// Insert a new task and any log entries it already has.
    pub fn insert(&self, task: &Task) -> Result<()> {
        let conn = self.lock()?;
        insert_on(&conn, task)
    }

    /// Delete a task and its log, but only while its status is one of `allowed`.
    /// The check and the delete are one statement. Returns whether a row was deleted.
    pub fn delete(&self, id: &str, allowed: &[TaskStatus]) -> Result<bool> {
        let conn = self.lock()?;
        let list = allowed
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let n = conn.execute(
            &format!("DELETE FROM tasks WHERE id = ?1 AND status IN ({list})"),
            params![id],
        )?;
        Ok(n > 0)
    }

    /// Replace drafts with new tasks in one transaction. Fails, and changes nothing, when
    /// any of `old_ids` is missing or is no longer a draft.
    pub fn replace_drafts(&self, old_ids: &[String], new_tasks: &[Task]) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        for id in old_ids {
            let n = tx.execute(
                "DELETE FROM tasks WHERE id = ?1 AND status = 'draft'",
                params![id],
            )?;
            if n == 0 {
                return Err(anyhow!("A draft was changed or removed. Reload the drafts and try again."));
            }
        }
        for t in new_tasks {
            insert_on(&tx, t)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Update the mutable fields of a task (not its log).
    pub fn update(&self, task: &Task) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE tasks SET title = ?2, detail = ?3, status = ?4, updated_at = ?5, result = ?6
             WHERE id = ?1",
            params![
                task.id,
                task.title,
                task.detail,
                task.status.as_str(),
                task.updated_at,
                task.result
            ],
        )?;
        Ok(())
    }

    pub fn append_log(&self, task_id: &str, entry: &TaskLogEntry) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO task_log (task_id, at, kind, message) VALUES (?1, ?2, ?3, ?4)",
            params![task_id, entry.at, entry.kind, entry.message],
        )?;
        conn.execute(
            "UPDATE tasks SET updated_at = ?2 WHERE id = ?1",
            params![task_id, entry.at],
        )?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<Task>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, detail, status, source, created_at, updated_at, result
             FROM tasks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_task)?;
        match rows.next() {
            Some(row) => {
                let mut task = row?;
                task.log = load_log(&conn, &task.id)?;
                Ok(Some(task))
            }
            None => Ok(None),
        }
    }

    /// All tasks, newest first, each with its log.
    pub fn list(&self) -> Result<Vec<Task>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, title, detail, status, source, created_at, updated_at, result
             FROM tasks ORDER BY created_at DESC, rowid DESC",
        )?;
        let mut tasks = stmt
            .query_map([], row_to_task)?
            .collect::<rusqlite::Result<Vec<Task>>>()?;
        for t in tasks.iter_mut() {
            t.log = load_log(&conn, &t.id)?;
        }
        Ok(tasks)
    }

    /// Mark tasks that were left in a live state by a previous run as Failed.
    /// Returns how many were changed.
    pub fn fail_interrupted(&self) -> Result<usize> {
        let conn = self.lock()?;
        let ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM tasks WHERE status IN ('pending', 'running', 'waitingApproval', 'escalated')",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()?
        };
        let at = now();
        for id in &ids {
            conn.execute(
                "INSERT INTO task_log (task_id, at, kind, message) VALUES (?1, ?2, 'error', ?3)",
                params![
                    id,
                    at,
                    "Relay was closed while this task was in progress. Marked as failed."
                ],
            )?;
            conn.execute(
                "UPDATE tasks SET status = ?2, updated_at = ?3,
                     result = COALESCE(result, 'Interrupted when Relay closed.')
                 WHERE id = ?1",
                params![id, TaskStatus::Failed.as_str(), at],
            )?;
        }
        Ok(ids.len())
    }
}

fn insert_on(conn: &Connection, task: &Task) -> Result<()> {
    conn.execute(
        "INSERT INTO tasks (id, title, detail, status, source, created_at, updated_at, result)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            task.id,
            task.title,
            task.detail,
            task.status.as_str(),
            task.source,
            task.created_at,
            task.updated_at,
            task.result
        ],
    )?;
    for e in &task.log {
        conn.execute(
            "INSERT INTO task_log (task_id, at, kind, message) VALUES (?1, ?2, ?3, ?4)",
            params![task.id, e.at, e.kind, e.message],
        )?;
    }
    Ok(())
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let status: String = row.get(3)?;
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        detail: row.get(2)?,
        status: TaskStatus::parse(&status).unwrap_or(TaskStatus::Failed),
        source: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        result: row.get(7)?,
        log: Vec::new(),
    })
}

fn load_log(conn: &Connection, task_id: &str) -> Result<Vec<TaskLogEntry>> {
    let mut stmt =
        conn.prepare("SELECT at, kind, message FROM task_log WHERE task_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![task_id], |r| {
        Ok(TaskLogEntry {
            at: r.get(0)?,
            kind: r.get(1)?,
            message: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, status: TaskStatus) -> Task {
        Task {
            id: id.into(),
            title: format!("Task {id}"),
            detail: "Detail text".into(),
            status,
            source: "user".into(),
            created_at: now(),
            updated_at: now(),
            result: None,
            log: vec![],
        }
    }

    fn temp_db() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("relay-test-{}.db", uuid::Uuid::new_v4()))
    }

    #[test]
    fn round_trip_in_temp_file() {
        let path = temp_db();
        {
            let store = TaskStore::open(&path).unwrap();
            let mut t = sample("a", TaskStatus::Pending);
            store.insert(&t).unwrap();
            assert!(store.get("missing").unwrap().is_none());

            t.status = TaskStatus::Done;
            t.result = Some("All finished".into());
            t.updated_at = now();
            store.update(&t).unwrap();
            store
                .append_log(
                    "a",
                    &TaskLogEntry {
                        at: now(),
                        kind: "info".into(),
                        message: "first".into(),
                    },
                )
                .unwrap();
            store
                .append_log(
                    "a",
                    &TaskLogEntry {
                        at: now(),
                        kind: "tool_call".into(),
                        message: "read_file {}".into(),
                    },
                )
                .unwrap();

            let got = store.get("a").unwrap().expect("task exists");
            assert_eq!(got.status, TaskStatus::Done);
            assert_eq!(got.result.as_deref(), Some("All finished"));
            assert_eq!(got.log.len(), 2);
            assert_eq!(got.log[0].message, "first");
            assert_eq!(got.log[1].kind, "tool_call");
        }
        // Reopen the same file to confirm the data was written to disk.
        let store = TaskStore::open(&path).unwrap();
        let all = store.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "a");
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn drafts_survive_a_restart_and_status_round_trips() {
        let path = temp_db();
        let store = TaskStore::open(&path).unwrap();
        store.insert(&sample("d", TaskStatus::Draft)).unwrap();
        store.insert(&sample("p", TaskStatus::Pending)).unwrap();
        assert_eq!(store.fail_interrupted().unwrap(), 1);
        assert_eq!(store.get("d").unwrap().unwrap().status, TaskStatus::Draft);
        assert_eq!(store.get("p").unwrap().unwrap().status, TaskStatus::Failed);
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn delete_only_removes_allowed_statuses_and_the_log() {
        let path = temp_db();
        let store = TaskStore::open(&path).unwrap();
        store.insert(&sample("draft", TaskStatus::Draft)).unwrap();
        store.insert(&sample("run", TaskStatus::Running)).unwrap();
        store.insert(&sample("done", TaskStatus::Done)).unwrap();
        store
            .append_log("done", &TaskLogEntry { at: now(), kind: "info".into(), message: "x".into() })
            .unwrap();
        let allowed = [TaskStatus::Draft, TaskStatus::Done, TaskStatus::Failed, TaskStatus::Cancelled];
        assert!(!store.delete("run", &allowed).unwrap());
        assert!(!store.delete("missing", &allowed).unwrap());
        assert!(store.delete("draft", &allowed).unwrap());
        assert!(store.delete("done", &allowed).unwrap());
        assert!(store.get("draft").unwrap().is_none());
        assert!(store.get("done").unwrap().is_none());
        assert!(store.get("run").unwrap().is_some());
        let logs: i64 = store
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM task_log WHERE task_id = 'done'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(logs, 0);
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replace_drafts_is_all_or_nothing() {
        let path = temp_db();
        let store = TaskStore::open(&path).unwrap();
        store.insert(&sample("a", TaskStatus::Draft)).unwrap();
        store.insert(&sample("b", TaskStatus::Draft)).unwrap();
        store.insert(&sample("c", TaskStatus::Pending)).unwrap();

        // "c" is not a draft: nothing changes, even though "a" was valid.
        let new = [sample("n1", TaskStatus::Draft)];
        let ids = vec!["a".to_string(), "c".to_string()];
        assert!(store.replace_drafts(&ids, &new).is_err());
        assert!(store.get("a").unwrap().is_some());
        assert!(store.get("n1").unwrap().is_none());

        // A missing id fails the same way.
        let ids = vec!["a".to_string(), "zzz".to_string()];
        assert!(store.replace_drafts(&ids, &new).is_err());
        assert!(store.get("a").unwrap().is_some());

        let ids = vec!["a".to_string(), "b".to_string()];
        let new = [sample("n1", TaskStatus::Draft), sample("n2", TaskStatus::Draft)];
        store.replace_drafts(&ids, &new).unwrap();
        assert!(store.get("a").unwrap().is_none());
        assert!(store.get("b").unwrap().is_none());
        assert!(store.get("n1").unwrap().is_some());
        assert!(store.get("n2").unwrap().is_some());
        assert_eq!(store.get("c").unwrap().unwrap().status, TaskStatus::Pending);
        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn restart_marks_live_tasks_failed() {
        let path = temp_db();
        let store = TaskStore::open(&path).unwrap();
        store.insert(&sample("run", TaskStatus::Running)).unwrap();
        store.insert(&sample("wait", TaskStatus::WaitingApproval)).unwrap();
        store.insert(&sample("esc", TaskStatus::Escalated)).unwrap();
        store.insert(&sample("done", TaskStatus::Done)).unwrap();
        assert_eq!(store.fail_interrupted().unwrap(), 3);
        for id in ["run", "wait", "esc"] {
            let t = store.get(id).unwrap().unwrap();
            assert_eq!(t.status, TaskStatus::Failed);
            assert_eq!(t.log.len(), 1);
            assert!(t.result.is_some());
        }
        assert_eq!(store.get("done").unwrap().unwrap().status, TaskStatus::Done);
        assert_eq!(store.fail_interrupted().unwrap(), 0);
        drop(store);
        let _ = std::fs::remove_file(&path);
    }
}
