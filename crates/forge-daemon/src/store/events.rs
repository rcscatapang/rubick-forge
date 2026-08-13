//! The `events` table: append-only history behind `GET /events`.

use forge_core::{EventKind, EventRecord, ForgeEvent, Timestamp};
use rusqlite::Connection;
use serde_json::{Map, Value};

use super::{Store, StoreError};

/// Clients ask for a page size; this is what they get at most.
pub const MAX_PAGE: usize = 500;
const DEFAULT_PAGE: usize = 100;

/// One page of history, plus the cursor to ask for the next one.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EventPage {
    pub events: Vec<EventRecord>,
    /// Highest id in this page, or `None` when it is empty.
    pub next_after: Option<i64>,
}

/// Which slice of history to return.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Query {
    /// Only events with a greater id.
    pub after: Option<i64>,
    pub limit: Option<usize>,
    /// Only events about this task.
    pub task_id: Option<i64>,
}

impl Store {
    /// History in id order.
    pub fn events(&self, query: Query) -> Result<EventPage, StoreError> {
        let limit = query.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);

        self.with(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, ts, kind, payload FROM events
                     WHERE id > ?1 AND (?2 IS NULL OR task_id = ?2)
                     ORDER BY id LIMIT ?3",
                )
                .map_err(StoreError::Query)?;

            let rows = stmt
                .query_map(
                    rusqlite::params![query.after.unwrap_or(0), query.task_id, limit as i64],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .map_err(StoreError::Query)?;

            let mut events = Vec::new();
            for row in rows {
                let (id, ts, kind, payload) = row.map_err(StoreError::Query)?;
                events.push(decode(id, &ts, &kind, &payload)?);
            }

            Ok(EventPage {
                next_after: events.last().map(|record| record.id),
                events,
            })
        })
    }

    /// Write history without announcing it. Tests only: every real write goes
    /// through [`crate::bus::Bus::publish`], because an event that reaches
    /// history without reaching subscribers is a stall the UI cannot recover
    /// from.
    #[cfg(test)]
    pub(crate) fn append_event(&self, event: &ForgeEvent) -> Result<EventRecord, StoreError> {
        self.with(|conn| append(conn, event))
    }
}

/// Insert on a caller-held connection, so the bus can persist and broadcast
/// under one lock and keep ids in send order.
pub(crate) fn append(conn: &Connection, event: &ForgeEvent) -> Result<EventRecord, StoreError> {
    let ts = Timestamp::now();
    let payload = Value::Object(event.payload()).to_string();

    conn.execute(
        "INSERT INTO events (ts, kind, task_id, session_id, payload)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            ts.to_string(),
            event.kind().as_str(),
            event.task_id(),
            event.session_id(),
            payload,
        ],
    )
    .map_err(StoreError::Query)?;

    Ok(EventRecord {
        id: conn.last_insert_rowid(),
        ts,
        event: event.clone(),
    })
}

fn decode(id: i64, ts: &str, kind: &str, payload: &str) -> Result<EventRecord, StoreError> {
    let kind: EventKind = kind
        .parse()
        .map_err(|err| StoreError::Corrupt(format!("event {id}: {err}")))?;

    let payload: Map<String, Value> = serde_json::from_str(payload)
        .map_err(|err| StoreError::Corrupt(format!("event {id} payload: {err}")))?;

    Ok(EventRecord {
        id,
        ts: ts
            .parse()
            .map_err(|err| StoreError::Corrupt(format!("event {id}: {err}")))?,
        event: ForgeEvent::from_parts(kind, payload)
            .map_err(|err| StoreError::Corrupt(format!("event {id}: {err}")))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::StopReason;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn project_event(id: i64) -> ForgeEvent {
        ForgeEvent::ProjectRegistered {
            project_id: id,
            name: format!("p{id}"),
            path: format!("/repos/p{id}"),
        }
    }

    #[test]
    fn appended_events_come_back_intact() {
        let store = store();
        let event = ForgeEvent::SessionStopped {
            task_id: 1,
            session_id: 2,
            reason: StopReason::Vanished,
        };

        let written = store.append_event(&event).unwrap();
        let page = store.events(Query::default()).unwrap();

        assert_eq!(page.events, std::slice::from_ref(&written));
        assert_eq!(page.events[0].event, event);
        assert_eq!(page.next_after, Some(written.id));
    }

    #[test]
    fn ids_increase_and_the_cursor_pages_forward() {
        let store = store();
        for id in 1..=5 {
            store.append_event(&project_event(id)).unwrap();
        }

        let first = store
            .events(Query {
                limit: Some(2),
                ..Query::default()
            })
            .unwrap();
        assert_eq!(first.events.len(), 2);

        let second = store
            .events(Query {
                after: first.next_after,
                limit: Some(2),
                ..Query::default()
            })
            .unwrap();
        assert_eq!(second.events.len(), 2);
        assert!(second.events[0].id > first.events[1].id);

        let rest = store
            .events(Query {
                after: second.next_after,
                limit: Some(2),
                ..Query::default()
            })
            .unwrap();
        assert_eq!(rest.events.len(), 1);

        let end = store
            .events(Query {
                after: rest.next_after,
                limit: Some(2),
                ..Query::default()
            })
            .unwrap();
        assert!(end.events.is_empty());
        assert_eq!(end.next_after, None);
    }

    #[test]
    fn the_page_size_is_clamped_rather_than_trusted() {
        let store = store();
        for id in 1..=3 {
            store.append_event(&project_event(id)).unwrap();
        }

        assert_eq!(
            store
                .events(Query {
                    limit: Some(0),
                    ..Query::default()
                })
                .unwrap()
                .events
                .len(),
            1
        );
        assert_eq!(
            store
                .events(Query {
                    limit: Some(usize::MAX),
                    ..Query::default()
                })
                .unwrap()
                .events
                .len(),
            3
        );
    }

    #[test]
    fn history_outlives_the_task_it_describes() {
        let store = store();
        store
            .append_event(&ForgeEvent::TaskDeleted { task_id: 99 })
            .unwrap();

        let page = store.events(Query::default()).unwrap();
        assert_eq!(page.events.len(), 1);
    }

    #[test]
    fn a_task_filter_returns_only_that_tasks_events() {
        let store = store();
        store.append_event(&project_event(1)).unwrap();
        store
            .append_event(&ForgeEvent::TaskDeleted { task_id: 7 })
            .unwrap();
        store
            .append_event(&ForgeEvent::TaskDeleted { task_id: 8 })
            .unwrap();

        let page = store
            .events(Query {
                task_id: Some(7),
                ..Query::default()
            })
            .unwrap();

        assert_eq!(page.events.len(), 1);
        assert_eq!(page.events[0].event, ForgeEvent::TaskDeleted { task_id: 7 });
    }

    #[test]
    fn a_task_filter_composes_with_the_cursor() {
        let store = store();
        for _ in 0..3 {
            store
                .append_event(&ForgeEvent::TaskDeleted { task_id: 7 })
                .unwrap();
        }

        let first = store
            .events(Query {
                task_id: Some(7),
                limit: Some(2),
                ..Query::default()
            })
            .unwrap();
        let rest = store
            .events(Query {
                task_id: Some(7),
                after: first.next_after,
                ..Query::default()
            })
            .unwrap();

        assert_eq!(first.events.len(), 2);
        assert_eq!(rest.events.len(), 1);
    }

    #[test]
    fn an_unreadable_row_is_reported_not_skipped() {
        let store = store();
        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO events (ts, kind, payload) VALUES ('2026-01-01T00:00:00Z', 'nonsense', '{}')",
                    [],
                )
                .map_err(StoreError::Query)
            })
            .unwrap();

        assert!(matches!(
            store.events(Query::default()),
            Err(StoreError::Corrupt(_))
        ));
    }
}
