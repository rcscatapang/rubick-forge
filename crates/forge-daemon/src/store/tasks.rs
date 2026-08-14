//! The `tasks` table: the unit of work agents run against.

use forge_core::{AdapterId, AgentStatus, Task, Timestamp};
use rusqlite::{OptionalExtension, Row};

use super::{Store, StoreError};

const COLUMNS: &str = "id, project_id, title, adapter, base_branch, branch, worktree_path, \
                       initial_prompt, status, created_at, updated_at";

/// A task as it is being written, before the database gives it an id.
///
/// `branch` and `worktree_path` are absent because both derive from the slug,
/// which needs the id — the row is created first, then completed.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTask {
    /// Names this attempt, so a retry returns the first task rather than a
    /// second one. `None` for a create nobody will repeat.
    pub idempotency_key: Option<String>,
    pub project_id: i64,
    pub title: String,
    pub adapter: AdapterId,
    pub base_branch: String,
    pub initial_prompt: Option<String>,
}

/// The fields a `PATCH` may change. `None` means "leave alone".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub initial_prompt: Option<String>,
    pub status: Option<AgentStatus>,
}

impl TaskPatch {
    fn is_empty(&self) -> bool {
        self.title.is_none() && self.initial_prompt.is_none() && self.status.is_none()
    }
}

/// Which tasks to list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TaskQuery {
    pub project_id: Option<i64>,
    pub status: Option<AgentStatus>,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("there is no task with id {0}")]
    NotFound(i64),
    #[error("there is no project with id {0}")]
    NoSuchProject(i64),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl Store {
    /// Insert a task with no branch of its own yet.
    ///
    /// It starts `stopped` — nothing is running until something starts it —
    /// and on the base branch, which is where a task without a worktree runs.
    /// The task already created under `key`, if there is one.
    ///
    /// What makes `POST /tasks` safe to retry: a client whose request succeeded
    /// but whose answer was lost asks again with the same key and is given the
    /// task it already made.
    pub fn task_by_key(&self, key: &str) -> Result<Option<Task>, StoreError> {
        self.with(|conn| {
            conn.query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE idempotency_key = ?1"),
                rusqlite::params![key],
                decode,
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Query(other)),
            })
        })?
        .transpose()
    }

    pub fn create_task(&self, new: &NewTask) -> Result<Task, TaskError> {
        let now = Timestamp::now();

        let id = self.with(|conn| {
            conn.execute(
                "INSERT INTO tasks
                     (project_id, title, adapter, base_branch, branch, worktree_path,
                      initial_prompt, status, idempotency_key, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, NULL, ?5, ?6, ?7, ?8, ?8)",
                rusqlite::params![
                    new.project_id,
                    new.title,
                    new.adapter.as_str(),
                    new.base_branch,
                    new.initial_prompt,
                    AgentStatus::Stopped.as_str(),
                    new.idempotency_key,
                    now.to_string(),
                ],
            )
            .map(|_| conn.last_insert_rowid())
            .map_err(StoreError::Query)
        });

        match id {
            Ok(id) => Ok(Task {
                id,
                project_id: new.project_id,
                title: new.title.clone(),
                adapter: new.adapter.clone(),
                base_branch: new.base_branch.clone(),
                branch: new.base_branch.clone(),
                worktree_path: None,
                initial_prompt: new.initial_prompt.clone(),
                status: AgentStatus::Stopped,
                created_at: now,
                updated_at: now,
            }),
            Err(err) if is_foreign_key_violation(&err) => {
                Err(TaskError::NoSuchProject(new.project_id))
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Record the branch and directory a freshly provisioned worktree got.
    pub fn attach_worktree(
        &self,
        id: i64,
        branch: &str,
        worktree_path: &str,
    ) -> Result<Task, TaskError> {
        self.set_worktree(id, Some(branch), Some(worktree_path))
    }

    /// Forget a worktree.
    ///
    /// `kept_branch` says whether `forge/<slug>` still exists. When it does the
    /// task keeps pointing at it, because that is where its work is; only a
    /// deleted branch sends the task back to its base.
    pub fn detach_worktree(&self, id: i64, kept_branch: bool) -> Result<Task, TaskError> {
        let task = self.task(id)?.ok_or(TaskError::NotFound(id))?;
        let branch = (!kept_branch).then_some(task.base_branch.as_str());
        self.set_worktree(id, branch, None)
    }

    /// `branch: None` leaves the branch column alone.
    fn set_worktree(
        &self,
        id: i64,
        branch: Option<&str>,
        worktree_path: Option<&str>,
    ) -> Result<Task, TaskError> {
        let changed = self.with(|conn| {
            conn.execute(
                "UPDATE tasks SET branch = COALESCE(?2, branch), worktree_path = ?3, updated_at = ?4
                 WHERE id = ?1",
                rusqlite::params![id, branch, worktree_path, Timestamp::now().to_string()],
            )
            .map_err(StoreError::Query)
        })?;

        if changed == 0 {
            return Err(TaskError::NotFound(id));
        }
        self.task(id)?.ok_or(TaskError::NotFound(id))
    }

    pub fn task(&self, id: i64) -> Result<Option<Task>, StoreError> {
        self.with(|conn| {
            conn.query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"),
                [id],
                decode,
            )
            .optional()
            .map_err(StoreError::Query)?
            .transpose()
        })
    }

    pub fn tasks(&self, query: TaskQuery) -> Result<Vec<Task>, StoreError> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM tasks
                     WHERE (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR status = ?2)
                     ORDER BY id"
                ))
                .map_err(StoreError::Query)?;

            let rows = stmt
                .query_map(
                    rusqlite::params![query.project_id, query.status.map(AgentStatus::as_str)],
                    decode,
                )
                .map_err(StoreError::Query)?;

            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(row.map_err(StoreError::Query)??);
            }
            Ok(tasks)
        })
    }

    pub fn update_task(&self, id: i64, patch: &TaskPatch) -> Result<Task, TaskError> {
        if patch.is_empty() {
            return self.task(id)?.ok_or(TaskError::NotFound(id));
        }

        let changed = self.with(|conn| {
            conn.execute(
                "UPDATE tasks SET
                     title          = COALESCE(?2, title),
                     initial_prompt = COALESCE(?3, initial_prompt),
                     status         = COALESCE(?4, status),
                     updated_at     = ?5
                 WHERE id = ?1",
                rusqlite::params![
                    id,
                    patch.title,
                    patch.initial_prompt,
                    patch.status.map(AgentStatus::as_str),
                    Timestamp::now().to_string(),
                ],
            )
            .map_err(StoreError::Query)
        })?;

        if changed == 0 {
            return Err(TaskError::NotFound(id));
        }
        self.task(id)?.ok_or(TaskError::NotFound(id))
    }

    pub fn delete_task(&self, id: i64) -> Result<(), TaskError> {
        let deleted = self.with(|conn| {
            conn.execute("DELETE FROM tasks WHERE id = ?1", [id])
                .map_err(StoreError::Query)
        })?;

        if deleted == 0 {
            return Err(TaskError::NotFound(id));
        }
        Ok(())
    }

    /// Whether the task has a session that has not ended.
    pub fn has_live_session(&self, task_id: i64) -> Result<bool, StoreError> {
        self.with(|conn| {
            conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM sessions WHERE task_id = ?1 AND ended_at IS NULL)",
                [task_id],
                |row| row.get(0),
            )
            .map_err(StoreError::Query)
        })
    }
}

/// A missing project is the caller naming one that does not exist.
fn is_foreign_key_violation(err: &StoreError) -> bool {
    let StoreError::Query(rusqlite::Error::SqliteFailure(error, _)) = err else {
        return false;
    };
    error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
}

fn decode(row: &Row<'_>) -> rusqlite::Result<Result<Task, StoreError>> {
    let id: i64 = row.get(0)?;
    let project_id: i64 = row.get(1)?;
    let title: String = row.get(2)?;
    let adapter: String = row.get(3)?;
    let base_branch: String = row.get(4)?;
    let branch: String = row.get(5)?;
    let worktree_path: Option<String> = row.get(6)?;
    let initial_prompt: Option<String> = row.get(7)?;
    let status: String = row.get(8)?;
    let created_at: String = row.get(9)?;
    let updated_at: String = row.get(10)?;

    let corrupt = |field: &str, err: &dyn std::fmt::Display| {
        StoreError::Corrupt(format!("task {id} {field}: {err}"))
    };

    Ok(Ok(Task {
        id,
        project_id,
        title,
        adapter: match adapter.parse() {
            Ok(adapter) => adapter,
            Err(err) => return Ok(Err(corrupt("adapter", &err))),
        },
        base_branch,
        branch,
        worktree_path,
        initial_prompt,
        status: match status.parse() {
            Ok(status) => status,
            Err(err) => return Ok(Err(corrupt("status", &err))),
        },
        created_at: match created_at.parse() {
            Ok(ts) => ts,
            Err(err) => return Ok(Err(corrupt("created_at", &err))),
        },
        updated_at: match updated_at.parse() {
            Ok(ts) => ts,
            Err(err) => return Ok(Err(corrupt("updated_at", &err))),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::NewProject;
    use forge_core::AdapterSettings;

    fn store_with_project() -> (Store, i64) {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .create_project(&NewProject {
                name: "forge".into(),
                path: "/repos/forge".into(),
                default_branch: "main".into(),
                adapter_settings: AdapterSettings::default(),
            })
            .unwrap();
        (store, project.id)
    }

    fn new_task(project_id: i64) -> NewTask {
        NewTask {
            idempotency_key: None,
            project_id,
            title: "Add adapters".into(),
            adapter: AdapterId::default(),
            base_branch: "main".into(),
            initial_prompt: Some("go".into()),
        }
    }

    #[test]
    fn a_new_task_starts_stopped_on_its_base_branch_with_no_worktree() {
        let (store, project_id) = store_with_project();

        let task = store.create_task(&new_task(project_id)).unwrap();

        assert_eq!(task.status, AgentStatus::Stopped);
        assert_eq!(task.branch, "main");
        assert_eq!(task.worktree_path, None);
        assert_eq!(store.task(task.id).unwrap().unwrap(), task);
    }

    #[test]
    fn a_task_for_an_unknown_project_is_refused() {
        let (store, _) = store_with_project();

        assert!(matches!(
            store.create_task(&new_task(404)),
            Err(TaskError::NoSuchProject(404))
        ));
    }

    #[test]
    fn attaching_a_worktree_moves_the_task_onto_its_own_branch() {
        let (store, project_id) = store_with_project();
        let task = store.create_task(&new_task(project_id)).unwrap();

        let attached = store
            .attach_worktree(task.id, "forge/add-adapters-1", "/wt/add-adapters-1")
            .unwrap();

        assert_eq!(attached.branch, "forge/add-adapters-1");
        assert_eq!(
            attached.worktree_path.as_deref(),
            Some("/wt/add-adapters-1")
        );
        assert_eq!(attached.base_branch, "main");
    }

    #[test]
    fn detaching_puts_the_task_back_on_its_base_branch() {
        let (store, project_id) = store_with_project();
        let task = store.create_task(&new_task(project_id)).unwrap();
        store
            .attach_worktree(task.id, "forge/add-adapters-1", "/wt/add-adapters-1")
            .unwrap();

        let dropped = store.detach_worktree(task.id, false).unwrap();
        assert_eq!(dropped.worktree_path, None);
        assert_eq!(
            dropped.branch, "main",
            "a deleted branch sends it back to base"
        );
    }

    #[test]
    fn detaching_keeps_the_branch_the_work_is_on() {
        let (store, project_id) = store_with_project();
        let task = store.create_task(&new_task(project_id)).unwrap();
        store
            .attach_worktree(task.id, "forge/add-adapters-1", "/wt/add-adapters-1")
            .unwrap();

        let kept = store.detach_worktree(task.id, true).unwrap();

        assert_eq!(kept.worktree_path, None);
        assert_eq!(kept.branch, "forge/add-adapters-1");
    }

    #[test]
    fn tasks_can_be_filtered_by_project_and_status() {
        let (store, project_id) = store_with_project();
        let other = store
            .create_project(&NewProject {
                name: "other".into(),
                path: "/repos/other".into(),
                default_branch: "main".into(),
                adapter_settings: AdapterSettings::default(),
            })
            .unwrap();

        let mine = store.create_task(&new_task(project_id)).unwrap();
        store.create_task(&new_task(other.id)).unwrap();
        store
            .update_task(
                mine.id,
                &TaskPatch {
                    status: Some(AgentStatus::Working),
                    ..TaskPatch::default()
                },
            )
            .unwrap();

        assert_eq!(store.tasks(TaskQuery::default()).unwrap().len(), 2);
        assert_eq!(
            store
                .tasks(TaskQuery {
                    project_id: Some(project_id),
                    ..TaskQuery::default()
                })
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .tasks(TaskQuery {
                    status: Some(AgentStatus::Working),
                    ..TaskQuery::default()
                })
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .tasks(TaskQuery {
                project_id: Some(other.id),
                status: Some(AgentStatus::Working),
            })
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_patch_touches_only_what_it_names() {
        let (store, project_id) = store_with_project();
        let task = store.create_task(&new_task(project_id)).unwrap();

        let patched = store
            .update_task(
                task.id,
                &TaskPatch {
                    title: Some("Renamed".into()),
                    ..TaskPatch::default()
                },
            )
            .unwrap();

        assert_eq!(patched.title, "Renamed");
        assert_eq!(patched.initial_prompt, task.initial_prompt);
        assert_eq!(patched.status, task.status);
    }

    #[test]
    fn an_unknown_task_is_reported_rather_than_silently_ignored() {
        let (store, _) = store_with_project();

        assert!(store.task(404).unwrap().is_none());
        assert!(matches!(
            store.update_task(404, &TaskPatch::default()),
            Err(TaskError::NotFound(404))
        ));
        assert!(matches!(
            store.delete_task(404),
            Err(TaskError::NotFound(404))
        ));
        assert!(matches!(
            store.detach_worktree(404, true),
            Err(TaskError::NotFound(404))
        ));
    }

    #[test]
    fn deleting_a_project_takes_its_tasks_with_it() {
        let (store, project_id) = store_with_project();
        store.create_task(&new_task(project_id)).unwrap();

        store.delete_project(project_id).unwrap();

        assert!(store.tasks(TaskQuery::default()).unwrap().is_empty());
    }

    #[test]
    fn a_session_is_live_until_it_ends() {
        let (store, project_id) = store_with_project();
        let task = store.create_task(&new_task(project_id)).unwrap();
        assert!(!store.has_live_session(task.id).unwrap());

        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO sessions (task_id, tmux_name, status, started_at)
                     VALUES (?1, 'forge-1', 'working', '2026-01-01T00:00:00Z')",
                    [task.id],
                )
                .map_err(StoreError::Query)
            })
            .unwrap();
        assert!(store.has_live_session(task.id).unwrap());

        store
            .with(|conn| {
                conn.execute(
                    "UPDATE sessions SET ended_at = '2026-01-01T01:00:00Z' WHERE task_id = ?1",
                    [task.id],
                )
                .map_err(StoreError::Query)
            })
            .unwrap();
        assert!(!store.has_live_session(task.id).unwrap());
    }

    #[test]
    fn a_corrupt_status_is_reported_not_guessed() {
        let (store, project_id) = store_with_project();
        let task = store.create_task(&new_task(project_id)).unwrap();
        store
            .with(|conn| {
                conn.execute(
                    "UPDATE tasks SET status = 'napping' WHERE id = ?1",
                    [task.id],
                )
                .map_err(StoreError::Query)
            })
            .unwrap();

        assert!(matches!(store.task(task.id), Err(StoreError::Corrupt(_))));
    }
}
