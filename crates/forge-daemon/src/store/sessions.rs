//! The `sessions` table: one row per run of a task.

use forge_core::{AgentStatus, Session, Timestamp};
use rusqlite::{OptionalExtension, Row};

use super::{Store, StoreError};

const COLUMNS: &str = "id, task_id, tmux_name, pid, status, started_at, ended_at";

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("there is no session with id {0}")]
    NotFound(i64),
    #[error("task {0} already has a running session")]
    AlreadyRunning(i64),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl Store {
    /// Open a session for a task.
    ///
    /// The partial unique index on live rows is what actually prevents two,
    /// so a race loses here rather than producing a second tmux session.
    pub fn start_session(&self, task_id: i64, tmux_name: &str) -> Result<Session, SessionError> {
        let started_at = Timestamp::now();

        let id = self.with(|conn| {
            conn.execute(
                "INSERT INTO sessions (task_id, tmux_name, pid, status, started_at, ended_at)
                 VALUES (?1, ?2, NULL, ?3, ?4, NULL)",
                rusqlite::params![
                    task_id,
                    tmux_name,
                    AgentStatus::Idle.as_str(),
                    started_at.to_string(),
                ],
            )
            .map(|_| conn.last_insert_rowid())
            .map_err(StoreError::Query)
        });

        match id {
            Ok(id) => Ok(Session {
                id,
                task_id,
                tmux_name: tmux_name.to_owned(),
                pid: None,
                status: AgentStatus::Idle,
                started_at,
                ended_at: None,
            }),
            Err(err) if is_unique_violation(&err) => Err(SessionError::AlreadyRunning(task_id)),
            Err(err) => Err(err.into()),
        }
    }

    /// Close a session, recording the status it ended in.
    ///
    /// Already-ended sessions are left alone, so a session that vanished and
    /// is then stopped by hand keeps the time it actually ended.
    pub fn end_session(&self, id: i64, status: AgentStatus) -> Result<Session, SessionError> {
        self.close_session(id, status)?;
        self.session(id)?.ok_or(SessionError::NotFound(id))
    }

    /// End a session only if it is still live, saying whether it was.
    ///
    /// Two things can decide a session is over at once — a stop request and a
    /// poll that sees the process gone — and only one of them should announce
    /// it.
    pub fn close_session(&self, id: i64, status: AgentStatus) -> Result<bool, SessionError> {
        let changed = self.with(|conn| {
            conn.execute(
                "UPDATE sessions SET status = ?2, ended_at = ?3
                 WHERE id = ?1 AND ended_at IS NULL",
                rusqlite::params![id, status.as_str(), Timestamp::now().to_string()],
            )
            .map_err(StoreError::Query)
        })?;

        Ok(changed > 0)
    }

    pub fn set_session_pid(&self, id: i64, pid: Option<i64>) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute(
                "UPDATE sessions SET pid = ?2 WHERE id = ?1",
                rusqlite::params![id, pid],
            )
            .map(|_| ())
            .map_err(StoreError::Query)
        })
    }

    pub fn set_session_status(&self, id: i64, status: AgentStatus) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute(
                "UPDATE sessions SET status = ?2 WHERE id = ?1 AND ended_at IS NULL",
                rusqlite::params![id, status.as_str()],
            )
            .map(|_| ())
            .map_err(StoreError::Query)
        })
    }

    pub fn session(&self, id: i64) -> Result<Option<Session>, StoreError> {
        self.with(|conn| {
            conn.query_row(
                &format!("SELECT {COLUMNS} FROM sessions WHERE id = ?1"),
                [id],
                decode,
            )
            .optional()
            .map_err(StoreError::Query)?
            .transpose()
        })
    }

    /// The task's session that has not ended, if it has one.
    pub fn live_session(&self, task_id: i64) -> Result<Option<Session>, StoreError> {
        self.with(|conn| {
            conn.query_row(
                &format!("SELECT {COLUMNS} FROM sessions WHERE task_id = ?1 AND ended_at IS NULL"),
                [task_id],
                decode,
            )
            .optional()
            .map_err(StoreError::Query)?
            .transpose()
        })
    }

    /// Every session that has not ended, across all tasks. This is what boot
    /// reconciliation compares against reality.
    pub fn live_sessions(&self) -> Result<Vec<Session>, StoreError> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM sessions WHERE ended_at IS NULL ORDER BY id"
                ))
                .map_err(StoreError::Query)?;

            let rows = stmt.query_map([], decode).map_err(StoreError::Query)?;

            let mut sessions = Vec::new();
            for row in rows {
                sessions.push(row.map_err(StoreError::Query)??);
            }
            Ok(sessions)
        })
    }

    /// A task's sessions, oldest first.
    pub fn sessions_for_task(&self, task_id: i64) -> Result<Vec<Session>, StoreError> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM sessions WHERE task_id = ?1 ORDER BY id"
                ))
                .map_err(StoreError::Query)?;

            let rows = stmt
                .query_map([task_id], decode)
                .map_err(StoreError::Query)?;

            let mut sessions = Vec::new();
            for row in rows {
                sessions.push(row.map_err(StoreError::Query)??);
            }
            Ok(sessions)
        })
    }
}

fn is_unique_violation(err: &StoreError) -> bool {
    let StoreError::Query(rusqlite::Error::SqliteFailure(error, _)) = err else {
        return false;
    };
    error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
}

fn decode(row: &Row<'_>) -> rusqlite::Result<Result<Session, StoreError>> {
    let id: i64 = row.get(0)?;
    let task_id: i64 = row.get(1)?;
    let tmux_name: String = row.get(2)?;
    let pid: Option<i64> = row.get(3)?;
    let status: String = row.get(4)?;
    let started_at: String = row.get(5)?;
    let ended_at: Option<String> = row.get(6)?;

    let corrupt = |field: &str, err: &dyn std::fmt::Display| {
        StoreError::Corrupt(format!("session {id} {field}: {err}"))
    };

    Ok(Ok(Session {
        id,
        task_id,
        tmux_name,
        pid,
        status: match status.parse() {
            Ok(status) => status,
            Err(err) => return Ok(Err(corrupt("status", &err))),
        },
        started_at: match started_at.parse() {
            Ok(ts) => ts,
            Err(err) => return Ok(Err(corrupt("started_at", &err))),
        },
        ended_at: match ended_at.map(|ts| ts.parse()).transpose() {
            Ok(ts) => ts,
            Err(err) => return Ok(Err(corrupt("ended_at", &err))),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewProject, NewTask};
    use forge_core::{tmux_session_name, AdapterId, AdapterSettings};

    fn store_with_task() -> (Store, i64) {
        let store = Store::open_in_memory().unwrap();
        let project = store
            .create_project(&NewProject {
                name: "forge".into(),
                path: "/repos/forge".into(),
                default_branch: "main".into(),
                adapter_settings: AdapterSettings::default(),
            })
            .unwrap();
        let task = store
            .create_task(&NewTask {
                idempotency_key: None,
                project_id: project.id,
                title: "Add adapters".into(),
                adapter: AdapterId::default(),
                base_branch: "main".into(),
                initial_prompt: None,
            })
            .unwrap();
        (store, task.id)
    }

    #[test]
    fn a_started_session_is_live_and_idle() {
        let (store, task_id) = store_with_task();

        let session = store
            .start_session(task_id, &tmux_session_name(task_id))
            .unwrap();

        assert_eq!(session.status, AgentStatus::Idle);
        assert!(session.is_live());
        assert_eq!(session.pid, None);
        assert_eq!(store.live_session(task_id).unwrap(), Some(session));
    }

    #[test]
    fn a_task_cannot_have_two_live_sessions() {
        let (store, task_id) = store_with_task();
        let name = tmux_session_name(task_id);
        store.start_session(task_id, &name).unwrap();

        assert!(matches!(
            store.start_session(task_id, &name),
            Err(SessionError::AlreadyRunning(_))
        ));
    }

    #[test]
    fn a_task_keeps_its_history_across_restarts() {
        let (store, task_id) = store_with_task();
        let name = tmux_session_name(task_id);

        let first = store.start_session(task_id, &name).unwrap();
        store.end_session(first.id, AgentStatus::Stopped).unwrap();
        let second = store.start_session(task_id, &name).unwrap();

        assert_ne!(first.id, second.id);
        assert_eq!(store.sessions_for_task(task_id).unwrap().len(), 2);
        assert_eq!(store.live_session(task_id).unwrap().unwrap().id, second.id);
    }

    #[test]
    fn ending_a_session_records_when_and_how() {
        let (store, task_id) = store_with_task();
        let session = store
            .start_session(task_id, &tmux_session_name(task_id))
            .unwrap();

        let ended = store.end_session(session.id, AgentStatus::Error).unwrap();

        assert_eq!(ended.status, AgentStatus::Error);
        assert!(ended.ended_at.is_some());
        assert!(!ended.is_live());
        assert_eq!(store.live_session(task_id).unwrap(), None);
    }

    #[test]
    fn only_the_first_close_reports_having_closed_it() {
        let (store, task_id) = store_with_task();
        let session = store
            .start_session(task_id, &tmux_session_name(task_id))
            .unwrap();

        assert!(store
            .close_session(session.id, AgentStatus::Stopped)
            .unwrap());
        assert!(
            !store.close_session(session.id, AgentStatus::Error).unwrap(),
            "the second caller must not announce it too"
        );
    }

    #[test]
    fn ending_twice_keeps_the_first_ending() {
        let (store, task_id) = store_with_task();
        let session = store
            .start_session(task_id, &tmux_session_name(task_id))
            .unwrap();

        let first = store.end_session(session.id, AgentStatus::Error).unwrap();
        let second = store.end_session(session.id, AgentStatus::Stopped).unwrap();

        assert_eq!(second.status, AgentStatus::Error);
        assert_eq!(second.ended_at, first.ended_at);
    }

    #[test]
    fn pid_and_status_are_recorded_while_live() {
        let (store, task_id) = store_with_task();
        let session = store
            .start_session(task_id, &tmux_session_name(task_id))
            .unwrap();

        store.set_session_pid(session.id, Some(4242)).unwrap();
        store
            .set_session_status(session.id, AgentStatus::Working)
            .unwrap();

        let read = store.session(session.id).unwrap().unwrap();
        assert_eq!(read.pid, Some(4242));
        assert_eq!(read.status, AgentStatus::Working);
    }

    #[test]
    fn an_ended_sessions_status_is_not_reopened() {
        let (store, task_id) = store_with_task();
        let session = store
            .start_session(task_id, &tmux_session_name(task_id))
            .unwrap();
        store.end_session(session.id, AgentStatus::Stopped).unwrap();

        store
            .set_session_status(session.id, AgentStatus::Working)
            .unwrap();

        assert_eq!(
            store.session(session.id).unwrap().unwrap().status,
            AgentStatus::Stopped
        );
    }

    #[test]
    fn live_sessions_are_what_reconciliation_reads() {
        let (store, task_id) = store_with_task();
        let session = store
            .start_session(task_id, &tmux_session_name(task_id))
            .unwrap();

        assert_eq!(store.live_sessions().unwrap().len(), 1);

        store.end_session(session.id, AgentStatus::Stopped).unwrap();
        assert!(store.live_sessions().unwrap().is_empty());
    }

    #[test]
    fn deleting_a_task_takes_its_sessions_with_it() {
        let (store, task_id) = store_with_task();
        store
            .start_session(task_id, &tmux_session_name(task_id))
            .unwrap();

        store.delete_task(task_id).unwrap();

        assert!(store.live_sessions().unwrap().is_empty());
    }
}
