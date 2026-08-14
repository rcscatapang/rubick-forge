//! Which adapters exist, and what went wrong loading the rest.
//!
//! Built-ins are compiled in; everything else is a `.toml` file in the state
//! directory's `adapters/`. A file that will not load costs its own adapter and
//! nothing else — the error is kept and reported on `/health` rather than
//! stopping the daemon.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use forge_core::AdapterId;
use serde::Serialize;

use super::manifest;
use super::Adapter;

/// The manifests Forge ships with, loaded exactly like external ones.
const BUILT_IN: [&str; 2] = [
    include_str!("builtin/claude-code.toml"),
    include_str!("builtin/codex.toml"),
];

/// One manifest that could not be used, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoadError {
    /// The file it came from, or the built-in's id.
    pub source: String,
    pub detail: String,
}

#[derive(Default)]
struct Loaded {
    adapters: Vec<Arc<Adapter>>,
    errors: Vec<LoadError>,
}

fn state() -> &'static RwLock<Loaded> {
    static STATE: OnceLock<RwLock<Loaded>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(load(adapters_dir().as_deref())))
}

/// Where external manifests live, if the state directory can be found.
fn adapters_dir() -> Option<PathBuf> {
    crate::paths::StateDir::locate()
        .ok()
        .map(|dir| dir.adapters_dir())
}

/// Every adapter, built-ins first and then external ones by id.
pub fn all() -> Vec<Arc<Adapter>> {
    state()
        .read()
        .expect("the adapter registry lock is never poisoned")
        .adapters
        .clone()
}

/// The adapter with this id, if one is loaded.
///
/// Returns `None` rather than a default: a task naming an adapter that has gone
/// away must say so, not quietly run something else.
pub fn adapter(id: &AdapterId) -> Option<Arc<Adapter>> {
    all().into_iter().find(|adapter| adapter.id() == id)
}

/// Every manifest that would not load, for `/health`.
pub fn load_errors() -> Vec<LoadError> {
    state()
        .read()
        .expect("the adapter registry lock is never poisoned")
        .errors
        .clone()
}

/// Read the adapters directory again, and report what is there now.
///
/// Running sessions keep the adapter they started with — their argv is already
/// fixed — so a reload changes what the *next* task gets.
pub fn reload() -> (usize, Vec<LoadError>) {
    let fresh = load(adapters_dir().as_deref());
    let count = fresh.adapters.len();
    let errors = fresh.errors.clone();

    *state()
        .write()
        .expect("the adapter registry lock is never poisoned") = fresh;

    (count, errors)
}

/// Load the built-ins, then whatever is in `dir`.
fn load(dir: Option<&Path>) -> Loaded {
    let mut loaded = Loaded::default();

    for text in BUILT_IN {
        match manifest::parse(text) {
            Ok(manifest) => loaded.adapters.push(Arc::new(Adapter::new(manifest))),
            // A built-in that will not parse is this daemon's own bug, and a
            // test catches it. It is still not fatal at runtime.
            Err(error) => loaded.errors.push(LoadError {
                source: "built-in".to_owned(),
                detail: error.to_string(),
            }),
        }
    }

    if let Some(dir) = dir {
        load_dir(dir, &mut loaded);
    }

    loaded
}

fn load_dir(dir: &Path, loaded: &mut Loaded) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        // No adapters directory is the ordinary case, not an error.
        return;
    };

    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();

    // Read in a stable order so two machines with the same files agree on
    // which duplicate wins.
    paths.sort();

    for path in paths {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                loaded.errors.push(LoadError {
                    source: name,
                    detail: format!("cannot read it: {err}"),
                });
                continue;
            }
        };

        match manifest::parse(&text) {
            Ok(manifest) => {
                // A built-in's id cannot be taken over by a file. Someone who
                // shadowed `claude-code` by accident would get a daemon that
                // behaves differently for reasons nothing on screen explains.
                if loaded.adapters.iter().any(|had| had.id() == &manifest.id) {
                    loaded.errors.push(LoadError {
                        source: name,
                        detail: format!(
                            "`{}` is already loaded, so this file is ignored. \
                             Give it an id of its own.",
                            manifest.id
                        ),
                    });
                    continue;
                }

                loaded.adapters.push(Arc::new(Adapter::new(manifest)));
            }
            Err(error) => loaded.errors.push(LoadError {
                source: name,
                detail: error.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AIDER: &str = r#"
manifest_version = 1
id = "aider"
name = "Aider"
binary = "aider"

[[status]]
status = "waiting"
markers = ["Do you want to"]
"#;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    #[test]
    fn the_built_ins_are_manifests_and_they_parse() {
        // If this fails, the shipped files are wrong, not the user's.
        let loaded = load(None);

        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        assert_eq!(loaded.adapters.len(), 2);
    }

    #[test]
    fn a_third_cli_is_a_file_and_nothing_else() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "aider.toml", AIDER);

        let loaded = load(Some(temp.path()));

        assert_eq!(loaded.adapters.len(), 3);
        assert!(loaded.errors.is_empty());
        assert!(loaded.adapters.iter().any(|a| a.id().as_str() == "aider"));
    }

    #[test]
    fn a_broken_manifest_costs_its_own_adapter_and_no_other() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "aider.toml", AIDER);
        write(temp.path(), "broken.toml", "this is not toml {{{");

        let loaded = load(Some(temp.path()));

        assert_eq!(
            loaded.adapters.len(),
            3,
            "the built-ins and aider still load"
        );
        assert_eq!(loaded.errors.len(), 1);
        assert_eq!(loaded.errors[0].source, "broken.toml");
    }

    #[test]
    fn an_error_names_the_file_so_it_can_be_found() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "later.toml",
            &AIDER.replace("_version = 1", "_version = 9"),
        );

        let loaded = load(Some(temp.path()));

        assert_eq!(loaded.errors[0].source, "later.toml");
        assert!(loaded.errors[0].detail.contains("manifest_version"));
    }

    #[test]
    fn a_file_cannot_take_over_a_built_ins_id() {
        // Shadowing would give a daemon that behaves differently for reasons
        // nothing on screen explains.
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "mine.toml",
            &AIDER.replace(r#"id = "aider""#, r#"id = "claude-code""#),
        );

        let loaded = load(Some(temp.path()));

        assert_eq!(loaded.adapters.len(), 2, "the built-in is untouched");
        assert!(loaded.errors[0].detail.contains("already loaded"));
    }

    #[test]
    fn anything_that_is_not_a_toml_file_is_not_a_manifest() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "notes.md", "# just notes");
        write(temp.path(), "aider.toml", AIDER);

        let loaded = load(Some(temp.path()));

        assert_eq!(loaded.adapters.len(), 3);
        assert!(loaded.errors.is_empty());
    }

    #[test]
    fn no_adapters_directory_is_the_ordinary_case() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("nothing-here");

        let loaded = load(Some(&missing));

        assert_eq!(loaded.adapters.len(), 2);
        assert!(loaded.errors.is_empty());
    }

    #[test]
    fn files_load_in_a_stable_order() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "zebra.toml", &AIDER.replace("aider", "zebra"));
        write(temp.path(), "alpha.toml", &AIDER.replace("aider", "alpha"));

        let ids: Vec<String> = load(Some(temp.path()))
            .adapters
            .iter()
            .map(|adapter| adapter.id().to_string())
            .collect();

        assert_eq!(ids, ["claude-code", "codex", "alpha", "zebra"]);
    }
}
