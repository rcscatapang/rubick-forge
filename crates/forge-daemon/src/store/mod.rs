//! SQLite persistence: one connection, guarded, migrated on open.
//!
//! A single connection behind a mutex is enough here — the workload is a
//! handful of tiny local queries per second, and it removes a whole class of
//! "database is locked" races. WAL mode means an external `sqlite3` reader can
//! still read while the daemon writes.

mod events;
mod migrations;
mod settings;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use rusqlite::Connection;

pub use events::{EventPage, Query};

pub(crate) use events::append as append_event_on;

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Open (creating if needed) and migrate the database at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(StoreError::Open)?;
        Self::prepare(conn)
    }

    /// An anonymous database that lives as long as the returned store.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(StoreError::Open)?;
        Self::prepare(conn)
    }

    fn prepare(conn: Connection) -> Result<Self, StoreError> {
        // WAL survives `kill -9` without corruption and lets readers in while
        // the daemon writes. NORMAL is the matching durability level: a crash
        // can lose the last transaction, never the file.
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(StoreError::Pragma)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(StoreError::Pragma)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(StoreError::Pragma)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(StoreError::Pragma)?;

        migrations::run(&conn).map_err(StoreError::Migrate)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run `body` with exclusive use of the connection.
    ///
    /// Poisoning is ignored on purpose: a panic mid-query leaves SQLite itself
    /// consistent (the transaction rolls back), so refusing every later query
    /// would turn one bug into an outage.
    pub(crate) fn with<T>(
        &self,
        body: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        body(&conn)
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        self.with(|conn| {
            conn.query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(StoreError::Query)
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("cannot open the database: {0}")]
    Open(#[source] rusqlite::Error),
    #[error("cannot configure the database: {0}")]
    Pragma(#[source] rusqlite::Error),
    #[error("cannot migrate the database: {0}")]
    Migrate(#[source] migrations::MigrationError),
    #[error("database query failed: {0}")]
    Query(#[source] rusqlite::Error),
    #[error("a stored row no longer matches the code that reads it: {0}")]
    Corrupt(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_migrates_and_reopening_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("forge.db");

        let first = Store::open(&path).unwrap();
        assert_eq!(
            first.schema_version().unwrap(),
            migrations::latest_version()
        );
        drop(first);

        let second = Store::open(&path).unwrap();
        assert_eq!(
            second.schema_version().unwrap(),
            migrations::latest_version()
        );
    }

    #[test]
    fn the_database_runs_in_wal_mode() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("forge.db")).unwrap();

        let mode: String = store
            .with(|conn| {
                conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))
                    .map_err(StoreError::Query)
            })
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = Store::open_in_memory().unwrap();
        let orphaned = store.with(|conn| {
            Ok(conn.execute(
                "INSERT INTO tasks (project_id, title, adapter, base_branch, branch, status, created_at, updated_at)
                 VALUES (404, 't', 'claude-code', 'main', 'main', 'stopped', '', '')",
                [],
            ))
        });
        assert!(orphaned.unwrap().is_err());
    }
}
