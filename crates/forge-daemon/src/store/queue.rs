//! The hub's task queue.
//!
//! Rows here exist only on a daemon with `hub = true`. The queue is not a task
//! list: a row becomes a real task on whichever machine takes it, and this
//! table remembers only where it went.

use forge_core::{AdapterId, QueueState, QueuedTask, Timestamp};
use rusqlite::{params, Row};

use super::{Store, StoreError};

const COLUMNS: &str = "id, project_name, adapter, title, prompt, target, state, reason, \
                       machine, remote_task, considered, created_at, updated_at";

/// What to put on the queue.
#[derive(Debug, Clone)]
pub struct NewQueuedTask {
    pub project_name: String,
    pub adapter: AdapterId,
    pub title: String,
    pub prompt: Option<String>,
    /// A machine name, or `None` for "whichever machine can take it".
    pub target: Option<String>,
}

fn read(row: &Row<'_>) -> rusqlite::Result<QueuedTask> {
    let adapter: String = row.get("adapter")?;
    let state: String = row.get("state")?;
    let considered: Option<String> = row.get("considered")?;
    let created: String = row.get("created_at")?;
    let updated: String = row.get("updated_at")?;

    Ok(QueuedTask {
        id: row.get("id")?,
        project_name: row.get("project_name")?,
        // A row naming an adapter this daemon does not know reads as the
        // default rather than failing the whole queue read. Only a hand-edited
        // database or a downgrade can produce one.
        adapter: adapter.parse().unwrap_or(AdapterId::default()),
        title: row.get("title")?,
        prompt: row.get("prompt")?,
        target: row.get("target")?,
        state: QueueState::parse(&state),
        reason: row.get("reason")?,
        machine: row.get("machine")?,
        remote_task: row.get("remote_task")?,
        // Stored as one line, because the audit trail is for reading rather
        // than querying.
        considered: considered
            .filter(|list| !list.is_empty())
            .map(|list| list.split('\n').map(str::to_owned).collect())
            .unwrap_or_default(),
        created_at: created.parse().unwrap_or_else(|_| Timestamp::now()),
        updated_at: updated.parse().unwrap_or_else(|_| Timestamp::now()),
    })
}

impl Store {
    pub fn enqueue(&self, new: &NewQueuedTask) -> Result<QueuedTask, StoreError> {
        self.with(|conn| {
            let now = Timestamp::now().to_string();

            conn.execute(
                "INSERT INTO queued_tasks
                     (project_name, adapter, title, prompt, target, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    new.project_name,
                    new.adapter.as_str(),
                    new.title,
                    new.prompt,
                    new.target,
                    now
                ],
            )
            .map_err(StoreError::Query)?;

            conn.query_row(
                &format!("SELECT {COLUMNS} FROM queued_tasks WHERE id = ?1"),
                params![conn.last_insert_rowid()],
                read,
            )
            .map_err(StoreError::Query)
        })
    }

    pub fn queued_task(&self, id: i64) -> Result<Option<QueuedTask>, StoreError> {
        self.with(|conn| {
            conn.query_row(
                &format!("SELECT {COLUMNS} FROM queued_tasks WHERE id = ?1"),
                params![id],
                read,
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(StoreError::Query(other)),
            })
        })
    }

    /// The whole queue, oldest first, however each row ended up.
    pub fn queue(&self) -> Result<Vec<QueuedTask>, StoreError> {
        self.select("SELECT {COLUMNS} FROM queued_tasks ORDER BY id")
    }

    /// The rows the dispatch loop has to place, oldest first.
    ///
    /// Order matters: a queue that placed the newest first would starve
    /// whatever could not be placed on its first pass.
    pub fn pending_queue(&self) -> Result<Vec<QueuedTask>, StoreError> {
        self.select("SELECT {COLUMNS} FROM queued_tasks WHERE state = 'queued' ORDER BY id")
    }

    fn select(&self, sql: &str) -> Result<Vec<QueuedTask>, StoreError> {
        let sql = sql.replace("{COLUMNS}", COLUMNS);

        self.with(|conn| {
            let mut statement = conn.prepare(&sql).map_err(StoreError::Query)?;
            let rows = statement.query_map([], read).map_err(StoreError::Query)?;

            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(StoreError::Query)
        })
    }

    /// Record where a row went.
    ///
    /// Conditional on the row still being `queued`: this is the second half of
    /// the idempotency guarantee, so that a dispatch racing a retry cannot
    /// overwrite an already-recorded placement. Returns whether it won.
    pub fn mark_dispatched(
        &self,
        id: i64,
        machine: &str,
        remote_task: i64,
        considered: &[String],
    ) -> Result<bool, StoreError> {
        self.with(|conn| {
            let changed = conn
                .execute(
                    "UPDATE queued_tasks
                        SET state = 'dispatched', machine = ?2, remote_task = ?3,
                            considered = ?4, reason = NULL, updated_at = ?5
                      WHERE id = ?1 AND state = 'queued'",
                    params![
                        id,
                        machine,
                        remote_task,
                        considered.join("\n"),
                        Timestamp::now().to_string()
                    ],
                )
                .map_err(StoreError::Query)?;

            Ok(changed == 1)
        })
    }

    /// Say why a row is still queued, without changing its state.
    pub fn set_queue_reason(
        &self,
        id: i64,
        reason: &str,
        considered: &[String],
    ) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute(
                "UPDATE queued_tasks SET reason = ?2, considered = ?3, updated_at = ?4
                  WHERE id = ?1 AND state = 'queued'",
                params![
                    id,
                    reason,
                    considered.join("\n"),
                    Timestamp::now().to_string()
                ],
            )
            .map_err(StoreError::Query)?;
            Ok(())
        })
    }

    /// Cancel a row that has not been dispatched. Returns whether it was one.
    ///
    /// A dispatched row is not cancellable here: the task is real and lives on
    /// another machine, so stopping it is that machine's business.
    pub fn cancel_queued(&self, id: i64) -> Result<bool, StoreError> {
        self.with(|conn| {
            let changed = conn
                .execute(
                    "UPDATE queued_tasks SET state = 'cancelled', updated_at = ?2
                      WHERE id = ?1 AND state = 'queued'",
                    params![id, Timestamp::now().to_string()],
                )
                .map_err(StoreError::Query)?;

            Ok(changed == 1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(target: Option<&str>) -> NewQueuedTask {
        NewQueuedTask {
            project_name: "forge".into(),
            adapter: AdapterId::default(),
            title: "Fix the flaky test".into(),
            prompt: Some("it fails one run in ten".into()),
            target: target.map(str::to_owned),
        }
    }

    #[test]
    fn a_row_reads_back_as_it_was_written() {
        let store = Store::open_in_memory().unwrap();

        let row = store.enqueue(&queued(None)).unwrap();

        assert_eq!(row.project_name, "forge");
        assert_eq!(row.adapter, AdapterId::default());
        assert_eq!(row.prompt.as_deref(), Some("it fails one run in ten"));
        assert_eq!(row.state, QueueState::Queued);
        assert_eq!(row.target, None);
        assert!(row.considered.is_empty());
    }

    #[test]
    fn a_row_aimed_at_one_machine_remembers_which() {
        let store = Store::open_in_memory().unwrap();

        let row = store.enqueue(&queued(Some("Mac mini"))).unwrap();

        assert_eq!(row.target.as_deref(), Some("Mac mini"));
    }

    #[test]
    fn the_dispatch_loop_sees_only_what_is_still_queued() {
        let store = Store::open_in_memory().unwrap();
        let first = store.enqueue(&queued(None)).unwrap();
        let second = store.enqueue(&queued(None)).unwrap();

        store.mark_dispatched(first.id, "mini", 7, &[]).unwrap();

        let pending = store.pending_queue().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, second.id);
        // And the whole queue still has both.
        assert_eq!(store.queue().unwrap().len(), 2);
    }

    #[test]
    fn the_oldest_row_is_placed_first_so_nothing_starves() {
        let store = Store::open_in_memory().unwrap();
        let first = store.enqueue(&queued(None)).unwrap();
        let second = store.enqueue(&queued(None)).unwrap();

        let ids: Vec<i64> = store
            .pending_queue()
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect();

        assert_eq!(ids, [first.id, second.id]);
    }

    #[test]
    fn dispatching_records_where_it_went_and_who_was_considered() {
        let store = Store::open_in_memory().unwrap();
        let row = store.enqueue(&queued(None)).unwrap();

        let won = store
            .mark_dispatched(
                row.id,
                "Mac mini",
                12,
                &["Mac mini".into(), "Laptop".into()],
            )
            .unwrap();

        assert!(won);
        let after = store.queued_task(row.id).unwrap().unwrap();
        assert_eq!(after.state, QueueState::Dispatched);
        assert_eq!(after.machine.as_deref(), Some("Mac mini"));
        assert_eq!(after.remote_task, Some(12));
        assert_eq!(after.considered, ["Mac mini", "Laptop"]);
    }

    #[test]
    fn a_second_dispatch_of_the_same_row_loses() {
        // This is half of "exactly once": a retry after a crash must not be
        // able to overwrite a placement that was already recorded.
        let store = Store::open_in_memory().unwrap();
        let row = store.enqueue(&queued(None)).unwrap();

        assert!(store.mark_dispatched(row.id, "Mac mini", 12, &[]).unwrap());
        assert!(!store.mark_dispatched(row.id, "Laptop", 99, &[]).unwrap());

        let after = store.queued_task(row.id).unwrap().unwrap();
        assert_eq!(after.machine.as_deref(), Some("Mac mini"));
        assert_eq!(after.remote_task, Some(12));
    }

    #[test]
    fn a_reason_explains_a_row_without_moving_it_on() {
        let store = Store::open_in_memory().unwrap();
        let row = store.enqueue(&queued(None)).unwrap();

        store
            .set_queue_reason(row.id, "no machine has forge", &[])
            .unwrap();

        let after = store.queued_task(row.id).unwrap().unwrap();
        assert_eq!(after.state, QueueState::Queued);
        assert_eq!(after.reason.as_deref(), Some("no machine has forge"));
    }

    #[test]
    fn dispatching_clears_the_reason_it_was_waiting_for() {
        let store = Store::open_in_memory().unwrap();
        let row = store.enqueue(&queued(None)).unwrap();
        store
            .set_queue_reason(row.id, "no machine yet", &[])
            .unwrap();

        store.mark_dispatched(row.id, "mini", 1, &[]).unwrap();

        assert_eq!(store.queued_task(row.id).unwrap().unwrap().reason, None);
    }

    #[test]
    fn only_a_queued_row_can_be_cancelled() {
        let store = Store::open_in_memory().unwrap();
        let row = store.enqueue(&queued(None)).unwrap();

        assert!(store.cancel_queued(row.id).unwrap());
        assert_eq!(
            store.queued_task(row.id).unwrap().unwrap().state,
            QueueState::Cancelled
        );
        // Cancelling twice is not a second cancellation.
        assert!(!store.cancel_queued(row.id).unwrap());
    }

    #[test]
    fn a_dispatched_row_cannot_be_cancelled_because_its_task_is_real() {
        let store = Store::open_in_memory().unwrap();
        let row = store.enqueue(&queued(None)).unwrap();
        store.mark_dispatched(row.id, "mini", 1, &[]).unwrap();

        assert!(!store.cancel_queued(row.id).unwrap());
    }

    #[test]
    fn a_row_that_was_never_there_is_not_there() {
        let store = Store::open_in_memory().unwrap();

        assert_eq!(store.queued_task(404).unwrap(), None);
        assert!(!store.cancel_queued(404).unwrap());
    }
}
