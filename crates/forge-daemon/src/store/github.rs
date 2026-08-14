//! A task's GitHub link: the issue it came from, the pull request it produced.

use forge_core::{ChecksState, TaskGitHub, Timestamp};
use rusqlite::{params, Row};

use super::{Store, StoreError};

const COLUMNS: &str = "task_id, issue_number, pr_number, pr_url, pr_state, \
                       head_sha, checks, polled_at";

fn read(row: &Row<'_>) -> rusqlite::Result<TaskGitHub> {
    let checks: String = row.get("checks")?;
    let polled: Option<String> = row.get("polled_at")?;

    Ok(TaskGitHub {
        task_id: row.get("task_id")?,
        issue_number: row.get("issue_number")?,
        pr_number: row.get("pr_number")?,
        pr_url: row.get("pr_url")?,
        pr_state: row.get("pr_state")?,
        head_sha: row.get("head_sha")?,
        checks: ChecksState::parse(&checks),
        // A timestamp the daemon wrote itself; an unreadable one is treated as
        // never having polled rather than failing the read.
        polled_at: polled.and_then(|value| value.parse().ok()),
    })
}

impl Store {
    pub fn task_github(&self, task_id: i64) -> Result<Option<TaskGitHub>, StoreError> {
        self.with(|conn| {
            conn.query_row(
                &format!("SELECT {COLUMNS} FROM task_github WHERE task_id = ?1"),
                params![task_id],
                read,
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Query(other)),
            })
        })
    }

    /// Every task's GitHub link, for a dashboard that shows them all at once.
    pub fn all_task_github(&self) -> Result<Vec<TaskGitHub>, StoreError> {
        self.with(|conn| {
            let mut statement = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM task_github ORDER BY task_id"
                ))
                .map_err(StoreError::Query)?;

            let rows = statement.query_map([], read).map_err(StoreError::Query)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(StoreError::Query)
        })
    }

    /// The tasks the polling loop has to ask GitHub about.
    ///
    /// Only tasks with a pull request still open: a merged or closed one has
    /// nothing left to say, and polling it would spend budget on an answer
    /// that cannot change.
    pub fn tasks_with_open_pulls(&self) -> Result<Vec<TaskGitHub>, StoreError> {
        self.with(|conn| {
            let mut statement = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM task_github WHERE pr_state = 'open' ORDER BY task_id"
                ))
                .map_err(StoreError::Query)?;

            let rows = statement.query_map([], read).map_err(StoreError::Query)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(StoreError::Query)
        })
    }

    /// Record which issue a task was started from.
    pub fn set_task_issue(&self, task_id: i64, issue_number: i64) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO task_github (task_id, issue_number) VALUES (?1, ?2)
                 ON CONFLICT(task_id) DO UPDATE SET issue_number = excluded.issue_number",
                params![task_id, issue_number],
            )
            .map_err(StoreError::Query)?;
            Ok(())
        })
    }

    /// Record what GitHub says about a task's pull request.
    ///
    /// Upserted rather than inserted: a task started from an issue already has
    /// a row, and overwriting its issue number would lose the link that a
    /// `Closes #n` depends on.
    pub fn set_task_pull(
        &self,
        task_id: i64,
        number: i64,
        url: &str,
        state: &str,
        head_sha: Option<&str>,
    ) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO task_github (task_id, pr_number, pr_url, pr_state, head_sha, polled_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(task_id) DO UPDATE SET
                     pr_number = excluded.pr_number,
                     pr_url    = excluded.pr_url,
                     pr_state  = excluded.pr_state,
                     head_sha  = excluded.head_sha,
                     polled_at = excluded.polled_at",
                params![
                    task_id,
                    number,
                    url,
                    state,
                    head_sha,
                    Timestamp::now().to_string()
                ],
            )
            .map_err(StoreError::Query)?;
            Ok(())
        })
    }

    pub fn set_task_checks(&self, task_id: i64, checks: ChecksState) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute(
                "UPDATE task_github SET checks = ?2, polled_at = ?3 WHERE task_id = ?1",
                params![task_id, checks.as_str(), Timestamp::now().to_string()],
            )
            .map_err(StoreError::Query)?;
            Ok(())
        })
    }

    /// Note that a poll happened even though nothing changed, so a chip can
    /// say how old what it is showing is.
    pub fn touch_task_github(&self, task_id: i64) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute(
                "UPDATE task_github SET polled_at = ?2 WHERE task_id = ?1",
                params![task_id, Timestamp::now().to_string()],
            )
            .map_err(StoreError::Query)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NewProject, NewTask};
    use forge_core::{AdapterId, AdapterSettings};

    /// A store with one project and one task in it.
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
                title: "Fix the flaky test".into(),
                adapter: AdapterId::ClaudeCode,
                base_branch: "main".into(),
                initial_prompt: None,
            })
            .unwrap();

        (store, task.id)
    }

    #[test]
    fn a_task_with_no_github_link_has_none() {
        let (store, task) = store_with_task();

        assert_eq!(store.task_github(task).unwrap(), None);
    }

    #[test]
    fn an_issue_number_survives_a_pull_request_arriving_later() {
        // The `Closes #n` in a PR body depends on this not being overwritten.
        let (store, task) = store_with_task();

        store.set_task_issue(task, 41).unwrap();
        store
            .set_task_pull(task, 7, "https://example/pull/7", "open", Some("abc"))
            .unwrap();

        let link = store.task_github(task).unwrap().expect("a link");
        assert_eq!(link.issue_number, Some(41));
        assert_eq!(link.pr_number, Some(7));
    }

    #[test]
    fn a_pull_request_reads_back_as_it_was_written() {
        let (store, task) = store_with_task();

        store
            .set_task_pull(task, 7, "https://example/pull/7", "open", Some("abc123"))
            .unwrap();

        let link = store.task_github(task).unwrap().expect("a link");
        assert_eq!(link.pr_url.as_deref(), Some("https://example/pull/7"));
        assert_eq!(link.pr_state.as_deref(), Some("open"));
        assert_eq!(link.head_sha.as_deref(), Some("abc123"));
        assert_eq!(link.checks, ChecksState::None);
        assert!(link.polled_at.is_some());
    }

    #[test]
    fn only_open_pull_requests_are_worth_polling() {
        let (store, task) = store_with_task();

        store
            .set_task_pull(task, 7, "https://example/pull/7", "open", None)
            .unwrap();
        assert_eq!(store.tasks_with_open_pulls().unwrap().len(), 1);

        // A merged pull request cannot change again.
        store
            .set_task_pull(task, 7, "https://example/pull/7", "merged", None)
            .unwrap();
        assert!(store.tasks_with_open_pulls().unwrap().is_empty());
    }

    #[test]
    fn a_check_state_round_trips_through_its_stored_word() {
        let (store, task) = store_with_task();
        store
            .set_task_pull(task, 7, "https://example/pull/7", "open", None)
            .unwrap();

        for state in [
            ChecksState::Running,
            ChecksState::Passed,
            ChecksState::Failed,
            ChecksState::None,
        ] {
            store.set_task_checks(task, state).unwrap();
            assert_eq!(store.task_github(task).unwrap().unwrap().checks, state);
        }
    }

    #[test]
    fn a_link_goes_when_its_task_does() {
        let (store, task) = store_with_task();
        store.set_task_issue(task, 41).unwrap();

        store.delete_task(task).unwrap();

        assert_eq!(store.task_github(task).unwrap(), None);
    }
}
