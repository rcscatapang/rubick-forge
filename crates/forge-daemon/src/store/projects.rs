//! The `projects` table: the registry of git repositories tasks hang off.

use forge_core::{AdapterSettings, AgentStatus, Project, Timestamp};
use rusqlite::{OptionalExtension, Row};

use super::{Store, StoreError};

/// A project as it is being written, before the database gives it an id.
#[derive(Debug, Clone, PartialEq)]
pub struct NewProject {
    pub name: String,
    /// Absolute, canonical repository root.
    pub path: String,
    pub default_branch: String,
    pub adapter_settings: AdapterSettings,
}

/// The fields a `PATCH` may change. `None` means "leave alone".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectPatch {
    pub name: Option<String>,
    pub default_branch: Option<String>,
    pub adapter_settings: Option<AdapterSettings>,
}

impl ProjectPatch {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.default_branch.is_none() && self.adapter_settings.is_none()
    }
}

/// Why a project could not be written.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("{0} is already registered")]
    DuplicatePath(String),
    #[error("there is no project with id {0}")]
    NotFound(i64),
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl Store {
    pub fn create_project(&self, new: &NewProject) -> Result<Project, ProjectError> {
        let now = Timestamp::now();

        let id = self.with(|conn| {
            conn.execute(
                "INSERT INTO projects (name, path, default_branch, adapter_settings, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                rusqlite::params![
                    new.name,
                    new.path,
                    new.default_branch,
                    serde_json::to_string(&new.adapter_settings)
                        .expect("AdapterSettings always serialises"),
                    now.to_string(),
                ],
            )
            .map(|_| conn.last_insert_rowid())
            .map_err(StoreError::Query)
        });

        match id {
            Ok(id) => Ok(Project {
                id,
                name: new.name.clone(),
                path: new.path.clone(),
                default_branch: new.default_branch.clone(),
                adapter_settings: new.adapter_settings.clone(),
                created_at: now,
                updated_at: now,
            }),
            Err(err) if is_unique_violation(&err) => {
                Err(ProjectError::DuplicatePath(new.path.clone()))
            }
            Err(err) => Err(err.into()),
        }
    }

    pub fn project(&self, id: i64) -> Result<Option<Project>, StoreError> {
        self.with(|conn| {
            conn.query_row(
                "SELECT id, name, path, default_branch, adapter_settings, created_at, updated_at
                 FROM projects WHERE id = ?1",
                [id],
                decode,
            )
            .optional()
            .map_err(StoreError::Query)?
            .transpose()
        })
    }

    pub fn project_by_path(&self, path: &str) -> Result<Option<Project>, StoreError> {
        self.with(|conn| {
            conn.query_row(
                "SELECT id, name, path, default_branch, adapter_settings, created_at, updated_at
                 FROM projects WHERE path = ?1",
                [path],
                decode,
            )
            .optional()
            .map_err(StoreError::Query)?
            .transpose()
        })
    }

    /// Every project, newest registration last.
    pub fn projects(&self) -> Result<Vec<Project>, StoreError> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, path, default_branch, adapter_settings, created_at, updated_at
                     FROM projects ORDER BY id",
                )
                .map_err(StoreError::Query)?;

            let rows = stmt.query_map([], decode).map_err(StoreError::Query)?;

            let mut projects = Vec::new();
            for row in rows {
                projects.push(row.map_err(StoreError::Query)??);
            }
            Ok(projects)
        })
    }

    /// Apply a patch and return the project as it now stands.
    pub fn update_project(&self, id: i64, patch: &ProjectPatch) -> Result<Project, ProjectError> {
        if patch.is_empty() {
            return self.project(id)?.ok_or(ProjectError::NotFound(id));
        }

        let changed = self.with(|conn| {
            conn.execute(
                "UPDATE projects SET
                     name             = COALESCE(?2, name),
                     default_branch   = COALESCE(?3, default_branch),
                     adapter_settings = COALESCE(?4, adapter_settings),
                     updated_at       = ?5
                 WHERE id = ?1",
                rusqlite::params![
                    id,
                    patch.name,
                    patch.default_branch,
                    patch.adapter_settings.as_ref().map(|settings| {
                        serde_json::to_string(settings).expect("AdapterSettings always serialises")
                    }),
                    Timestamp::now().to_string(),
                ],
            )
            .map_err(StoreError::Query)
        })?;

        if changed == 0 {
            return Err(ProjectError::NotFound(id));
        }

        self.project(id)?.ok_or(ProjectError::NotFound(id))
    }

    /// Remove a project. Its tasks go with it; nothing on disk is touched.
    pub fn delete_project(&self, id: i64) -> Result<(), ProjectError> {
        let deleted = self.with(|conn| {
            conn.execute("DELETE FROM projects WHERE id = ?1", [id])
                .map_err(StoreError::Query)
        })?;

        if deleted == 0 {
            return Err(ProjectError::NotFound(id));
        }
        Ok(())
    }

    /// How many of a project's tasks still expect a running session — the
    /// guard on deleting a project out from under a live agent.
    ///
    /// An errored task is as dead as a stopped one, so it does not block;
    /// counting it would strand the project with nothing able to release it.
    pub fn live_task_count(&self, project_id: i64) -> Result<i64, StoreError> {
        let live: Vec<&str> = AgentStatus::ALL
            .iter()
            .filter(|status| status.is_live())
            .map(|status| status.as_str())
            .collect();
        let placeholders = vec!["?"; live.len()].join(", ");

        self.with(|conn| {
            let mut params: Vec<&dyn rusqlite::ToSql> = vec![&project_id];
            params.extend(live.iter().map(|status| status as &dyn rusqlite::ToSql));

            conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM tasks WHERE project_id = ? AND status IN ({placeholders})"
                ),
                params.as_slice(),
                |row| row.get(0),
            )
            .map_err(StoreError::Query)
        })
    }
}

/// A `UNIQUE` breach is a duplicate registration, not a daemon fault. The
/// extended code matters: a NOT NULL or CHECK failure is also a constraint
/// violation, and calling one of those "already registered" sends the user
/// somewhere useless.
fn is_unique_violation(err: &StoreError) -> bool {
    let StoreError::Query(rusqlite::Error::SqliteFailure(error, _)) = err else {
        return false;
    };
    error.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
}

/// The outer `Result` is rusqlite's; the inner one is ours, for a row that
/// parses as SQL but not as a `Project`.
fn decode(row: &Row<'_>) -> rusqlite::Result<Result<Project, StoreError>> {
    let id: i64 = row.get(0)?;
    let name: String = row.get(1)?;
    let path: String = row.get(2)?;
    let default_branch: String = row.get(3)?;
    let settings: String = row.get(4)?;
    let created_at: String = row.get(5)?;
    let updated_at: String = row.get(6)?;

    let corrupt = |field: &str, err: &dyn std::fmt::Display| {
        StoreError::Corrupt(format!("project {id} {field}: {err}"))
    };

    Ok(Ok(Project {
        id,
        name,
        path,
        default_branch,
        adapter_settings: match serde_json::from_str(&settings) {
            Ok(settings) => settings,
            Err(err) => return Ok(Err(corrupt("adapter_settings", &err))),
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
    use forge_core::AdapterId;
    use serde_json::{Map, Value};

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    fn new_project(path: &str) -> NewProject {
        NewProject {
            name: "forge".into(),
            path: path.into(),
            default_branch: "main".into(),
            adapter_settings: AdapterSettings::default(),
        }
    }

    fn with_model(model: &str) -> AdapterSettings {
        let mut settings = AdapterSettings::default();
        let mut claude = Map::new();
        claude.insert("model".into(), Value::String(model.into()));
        settings.set(AdapterId::default(), claude);
        settings
    }

    #[test]
    fn a_created_project_reads_back_identically() {
        let store = store();
        let mut new = new_project("/repos/forge");
        new.adapter_settings = with_model("opus");

        let created = store.create_project(&new).unwrap();
        let read = store.project(created.id).unwrap().unwrap();

        assert_eq!(read, created);
        assert_eq!(read.adapter_settings, new.adapter_settings);
        assert_eq!(read.created_at, read.updated_at);
    }

    #[test]
    fn registering_the_same_path_twice_is_refused() {
        let store = store();
        store.create_project(&new_project("/repos/forge")).unwrap();

        let again = store.create_project(&new_project("/repos/forge"));

        assert!(matches!(again, Err(ProjectError::DuplicatePath(path)) if path == "/repos/forge"));
        assert_eq!(store.projects().unwrap().len(), 1);
    }

    #[test]
    fn a_project_can_be_found_by_its_path() {
        let store = store();
        let created = store.create_project(&new_project("/repos/forge")).unwrap();

        assert_eq!(
            store.project_by_path("/repos/forge").unwrap(),
            Some(created)
        );
        assert_eq!(store.project_by_path("/repos/other").unwrap(), None);
    }

    #[test]
    fn a_patch_touches_only_what_it_names() {
        let store = store();
        let created = store.create_project(&new_project("/repos/forge")).unwrap();

        let patched = store
            .update_project(
                created.id,
                &ProjectPatch {
                    default_branch: Some("develop".into()),
                    ..ProjectPatch::default()
                },
            )
            .unwrap();

        assert_eq!(patched.default_branch, "develop");
        assert_eq!(patched.name, created.name);
        assert_eq!(patched.path, created.path);
        assert_eq!(patched.created_at, created.created_at);
    }

    #[test]
    fn adapter_settings_are_replaced_wholesale() {
        let store = store();
        let mut new = new_project("/repos/forge");
        new.adapter_settings = with_model("opus");
        let created = store.create_project(&new).unwrap();

        let patched = store
            .update_project(
                created.id,
                &ProjectPatch {
                    adapter_settings: Some(with_model("sonnet")),
                    ..ProjectPatch::default()
                },
            )
            .unwrap();

        assert_eq!(
            patched.adapter_settings.get(&AdapterId::default()).unwrap()["model"],
            "sonnet"
        );
    }

    #[test]
    fn an_empty_patch_is_a_read() {
        let store = store();
        let created = store.create_project(&new_project("/repos/forge")).unwrap();

        let patched = store
            .update_project(created.id, &ProjectPatch::default())
            .unwrap();

        assert_eq!(patched, created);
    }

    #[test]
    fn patching_or_deleting_an_unknown_project_says_so() {
        let store = store();

        assert!(matches!(
            store.update_project(404, &ProjectPatch::default()),
            Err(ProjectError::NotFound(404))
        ));
        assert!(matches!(
            store.update_project(
                404,
                &ProjectPatch {
                    name: Some("x".into()),
                    ..ProjectPatch::default()
                }
            ),
            Err(ProjectError::NotFound(404))
        ));
        assert!(matches!(
            store.delete_project(404),
            Err(ProjectError::NotFound(404))
        ));
    }

    #[test]
    fn deleting_removes_it_from_the_list() {
        let store = store();
        let a = store.create_project(&new_project("/repos/a")).unwrap();
        store.create_project(&new_project("/repos/b")).unwrap();

        store.delete_project(a.id).unwrap();

        let remaining = store.projects().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, "/repos/b");
    }

    #[test]
    fn projects_list_in_registration_order() {
        let store = store();
        for path in ["/repos/c", "/repos/a", "/repos/b"] {
            store.create_project(&new_project(path)).unwrap();
        }

        let paths: Vec<String> = store
            .projects()
            .unwrap()
            .into_iter()
            .map(|project| project.path)
            .collect();

        assert_eq!(paths, ["/repos/c", "/repos/a", "/repos/b"]);
    }

    #[test]
    fn only_session_backed_tasks_count_as_live() {
        let store = store();
        let project = store.create_project(&new_project("/repos/forge")).unwrap();
        assert_eq!(store.live_task_count(project.id).unwrap(), 0);

        for status in AgentStatus::ALL {
            store
                .with(|conn| {
                    conn.execute(
                        "INSERT INTO tasks (project_id, title, adapter, base_branch, branch, status, created_at, updated_at)
                         VALUES (?1, 't', 'claude-code', 'main', 'main', ?2, '', '')",
                        rusqlite::params![project.id, status.as_str()],
                    )
                    .map_err(StoreError::Query)
                })
                .unwrap();
        }

        // Idle, working and waiting block a delete; stopped and errored do not.
        assert_eq!(store.live_task_count(project.id).unwrap(), 3);
    }

    #[test]
    fn a_corrupt_settings_blob_is_reported_not_guessed() {
        let store = store();
        let project = store.create_project(&new_project("/repos/forge")).unwrap();
        store
            .with(|conn| {
                conn.execute(
                    "UPDATE projects SET adapter_settings = 'not json' WHERE id = ?1",
                    [project.id],
                )
                .map_err(StoreError::Query)
            })
            .unwrap();

        assert!(matches!(
            store.project(project.id),
            Err(StoreError::Corrupt(_))
        ));
    }
}
