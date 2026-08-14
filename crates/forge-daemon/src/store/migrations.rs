//! Forward-only schema migrations, versioned by SQLite's `user_version`.
//!
//! Migrations are append-only: never edit one that has shipped, add the next.

use rusqlite::Connection;

/// Each entry is one migration; its index + 1 is the schema version it
/// produces.
const MIGRATIONS: &[&str] = &[
    include_str!("migrations/0001_initial.sql"),
    include_str!("migrations/0002_github.sql"),
];

/// The version a fully migrated database reports.
pub fn latest_version() -> i64 {
    MIGRATIONS.len() as i64
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(
        "the database is at schema version {found}, but this daemon only knows {known}; \
         downgrading is not supported, so run a newer forge-daemon"
    )]
    FromTheFuture { found: i64, known: i64 },
}

/// Bring `conn` up to [`latest_version`]. Safe to call on every start.
pub fn run(conn: &Connection) -> Result<i64, MigrationError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if current > latest_version() {
        return Err(MigrationError::FromTheFuture {
            found: current,
            known: latest_version(),
        });
    }

    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        // PRAGMA does not accept bound parameters.
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }

    Ok(latest_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
        rows.map(Result::unwrap).collect()
    }

    #[test]
    fn a_fresh_database_reaches_the_latest_version() {
        let conn = Connection::open_in_memory().unwrap();
        assert_eq!(run(&conn).unwrap(), latest_version());
        assert_eq!(
            tables(&conn),
            [
                "events",
                "projects",
                "sessions",
                "settings",
                "task_github",
                "tasks"
            ]
        );
    }

    #[test]
    fn running_twice_changes_nothing() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        let before = tables(&conn);

        run(&conn).unwrap();
        assert_eq!(tables(&conn), before);
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, latest_version());
    }

    #[test]
    fn a_database_from_the_future_is_refused_rather_than_mangled() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", latest_version() + 1)
            .unwrap();
        assert!(run(&conn).is_err());
    }
}
