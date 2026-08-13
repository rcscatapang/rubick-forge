//! The daemon's state directory.

use std::io;
use std::path::{Path, PathBuf};

/// Set this to relocate the whole state directory. Tests use it for isolation;
/// a second daemon instance can use it to run side by side.
pub const STATE_DIR_ENV: &str = "FORGE_STATE_DIR";

const APP_DIR: &str = "rubick-forge";

/// `~/Library/Application Support/rubick-forge/` and the files inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDir(PathBuf);

impl StateDir {
    /// The configured location, without touching the filesystem.
    pub fn locate() -> Result<Self, StateDirError> {
        if let Some(overridden) = std::env::var_os(STATE_DIR_ENV) {
            let path = PathBuf::from(overridden);
            if path.as_os_str().is_empty() {
                return Err(StateDirError::EmptyOverride);
            }
            return Ok(Self(path));
        }

        let home = std::env::var_os("HOME").ok_or(StateDirError::NoHome)?;
        Ok(Self(
            Path::new(&home)
                .join("Library")
                .join("Application Support")
                .join(APP_DIR),
        ))
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// Create the directory tree if it is not there yet. Idempotent.
    pub fn ensure(&self) -> Result<(), StateDirError> {
        std::fs::create_dir_all(self.logs_dir()).map_err(|source| StateDirError::Create {
            path: self.0.clone(),
            source,
        })
    }

    pub fn root(&self) -> &Path {
        &self.0
    }

    pub fn db_path(&self) -> PathBuf {
        self.0.join("forge.db")
    }

    pub fn config_path(&self) -> PathBuf {
        self.0.join("daemon.toml")
    }

    pub fn token_path(&self) -> PathBuf {
        self.0.join("token")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.0.join("logs")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StateDirError {
    #[error("HOME is not set, so the state directory cannot be located")]
    NoHome,
    #[error("{STATE_DIR_ENV} is set but empty")]
    EmptyOverride,
    #[error("cannot create the state directory at {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_hang_off_the_root() {
        let dir = StateDir::at("/state");
        assert_eq!(dir.db_path(), Path::new("/state/forge.db"));
        assert_eq!(dir.config_path(), Path::new("/state/daemon.toml"));
        assert_eq!(dir.token_path(), Path::new("/state/token"));
        assert_eq!(dir.logs_dir(), Path::new("/state/logs"));
    }

    #[test]
    fn ensure_creates_the_tree_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let dir = StateDir::at(temp.path().join("nested").join("state"));
        dir.ensure().unwrap();
        dir.ensure().unwrap();
        assert!(dir.logs_dir().is_dir());
    }
}
