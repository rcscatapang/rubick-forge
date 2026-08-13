//! The `settings` table: a small key/value store for daemon-wide preferences
//! that every client should share (worktree root, notification toggles).

use std::collections::BTreeMap;

use rusqlite::OptionalExtension;

use super::{Store, StoreError};

impl Store {
    pub fn setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.with(|conn| {
            conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(StoreError::Query)
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .map(|_| ())
            .map_err(StoreError::Query)
        })
    }

    pub fn clear_setting(&self, key: &str) -> Result<(), StoreError> {
        self.with(|conn| {
            conn.execute("DELETE FROM settings WHERE key = ?1", [key])
                .map(|_| ())
                .map_err(StoreError::Query)
        })
    }

    /// Apply several changes at once, or none of them.
    ///
    /// A half-applied set of preferences is worse than a rejected one: the
    /// client would show a state nothing agreed to.
    pub fn apply_settings(
        &self,
        changes: &BTreeMap<String, Option<String>>,
    ) -> Result<(), StoreError> {
        self.with(|conn| {
            let tx = conn.unchecked_transaction().map_err(StoreError::Query)?;

            for (key, value) in changes {
                match value {
                    Some(value) => tx.execute(
                        "INSERT INTO settings (key, value) VALUES (?1, ?2)
                         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                        [key.as_str(), value.as_str()],
                    ),
                    None => tx.execute("DELETE FROM settings WHERE key = ?1", [key.as_str()]),
                }
                .map_err(StoreError::Query)?;
            }

            tx.commit().map_err(StoreError::Query)
        })
    }

    pub fn settings(&self) -> Result<BTreeMap<String, String>, StoreError> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare("SELECT key, value FROM settings ORDER BY key")
                .map_err(StoreError::Query)?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(StoreError::Query)?;

            rows.collect::<rusqlite::Result<_>>()
                .map_err(StoreError::Query)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_a_key_twice_overwrites_it() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.setting("worktree_root").unwrap(), None);

        store.set_setting("worktree_root", "/a").unwrap();
        store.set_setting("worktree_root", "/b").unwrap();

        assert_eq!(
            store.setting("worktree_root").unwrap().as_deref(),
            Some("/b")
        );
    }

    #[test]
    fn clearing_removes_the_key_entirely() {
        let store = Store::open_in_memory().unwrap();
        store.set_setting("notify.waiting", "false").unwrap();
        store.clear_setting("notify.waiting").unwrap();

        assert_eq!(store.setting("notify.waiting").unwrap(), None);
        assert!(store.settings().unwrap().is_empty());
    }

    #[test]
    fn a_set_of_changes_applies_together() {
        let store = Store::open_in_memory().unwrap();
        store.set_setting("keep", "yes").unwrap();
        store.set_setting("drop", "no").unwrap();

        store
            .apply_settings(&BTreeMap::from([
                ("added".to_owned(), Some("new".to_owned())),
                ("drop".to_owned(), None),
            ]))
            .unwrap();

        let all = store.settings().unwrap();
        assert_eq!(all.get("added").map(String::as_str), Some("new"));
        assert_eq!(all.get("keep").map(String::as_str), Some("yes"));
        assert!(!all.contains_key("drop"));
    }

    #[test]
    fn settings_list_in_key_order() {
        let store = Store::open_in_memory().unwrap();
        store.set_setting("b", "2").unwrap();
        store.set_setting("a", "1").unwrap();

        let all = store.settings().unwrap();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["a", "b"]);
    }
}
