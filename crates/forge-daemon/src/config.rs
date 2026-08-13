//! `daemon.toml` — the handful of knobs that must survive a restart.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Fixed so the desktop app can find the daemon without discovery. Loopback
/// only by default; the tailnet bind is an explicit opt-in.
pub const DEFAULT_PORT: u16 = 8787;

/// What both agent CLIs treat as "stop what you are doing".
const DEFAULT_STOP_KEY: &str = "C-c";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Address to bind. `0.0.0.0` is rejected on load.
    pub bind: IpAddr,
    pub port: u16,
    /// Label for this daemon in a multi-machine UI. Filled in with the Mac's
    /// own name when the file is first written.
    pub machine: Option<String>,
    /// Overrides the `<parent-of-repo>/.forge-worktrees` default.
    pub worktree_root: Option<PathBuf>,
    /// The keys a stop sends before killing the session, so an agent can shut
    /// down on its own terms. tmux key names, in order.
    pub stop_keys: Vec<String>,
    /// How long a stop waits for the agent to take that hint.
    pub stop_grace_secs: u64,
    /// How often live sessions are checked against reality.
    pub poll_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: DEFAULT_PORT,
            machine: None,
            worktree_root: None,
            stop_keys: vec![DEFAULT_STOP_KEY.to_owned()],
            stop_grace_secs: 3,
            poll_secs: 2,
        }
    }
}

impl Config {
    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind, self.port)
    }

    /// The name to show for this daemon, detected if the file did not say.
    pub fn machine_name(&self) -> String {
        self.machine.clone().unwrap_or_else(detect_machine_name)
    }

    /// Read the file, or write the defaults out and use those.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
                    path: path.to_path_buf(),
                    source: Box::new(source),
                })?;
                config.validate()?;
                Ok(config)
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let config = Self {
                    machine: Some(detect_machine_name()),
                    ..Self::default()
                };
                config.write(path)?;
                Ok(config)
            }
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn write(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self).expect("Config always serialises to TOML");
        std::fs::write(path, text).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// The keys a stop sends, borrowed for passing to the runtime.
    pub fn stop_keys(&self) -> Vec<&str> {
        self.stop_keys.iter().map(String::as_str).collect()
    }

    pub fn stop_grace(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.stop_grace_secs)
    }

    pub fn poll_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.poll_secs)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.bind.is_unspecified() {
            return Err(ConfigError::UnspecifiedBind(self.bind));
        }
        if self.port == 0 {
            return Err(ConfigError::ZeroPort);
        }
        if self.poll_secs == 0 {
            return Err(ConfigError::ZeroPoll);
        }
        Ok(())
    }
}

/// The Mac's own name, so a multi-machine UI reads sensibly without anyone
/// having to configure it. Deliberately kept out of `Default`, which must stay
/// a pure value.
fn detect_machine_name() -> String {
    std::process::Command::new("/usr/sbin/scutil")
        .arg("--get")
        .arg("ComputerName")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this Mac".to_owned())
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not valid daemon config: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("bind = \"{0}\" would expose the daemon beyond this machine; Forge only binds a specific interface")]
    UnspecifiedBind(IpAddr),
    #[error("port = 0 would pick a random port the app cannot find")]
    ZeroPort,
    #[error("poll_secs = 0 would spin instead of polling")]
    ZeroPoll,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_loopback_on_the_fixed_port() {
        let config = Config::default();
        assert_eq!(config.socket_addr().to_string(), "127.0.0.1:8787");
        assert!(config.worktree_root.is_none());
        assert!(config.machine.is_none());
    }

    #[test]
    fn a_created_file_records_a_concrete_machine_name() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");

        let config = Config::load_or_create(&path).unwrap();

        assert!(config.machine.is_some());
        assert!(std::fs::read_to_string(&path).unwrap().contains("machine"));
    }

    #[test]
    fn a_file_without_a_machine_name_falls_back_to_detection() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");
        std::fs::write(&path, "port = 9000\n").unwrap();

        let config = Config::load_or_create(&path).unwrap();

        assert!(config.machine.is_none());
        assert!(!config.machine_name().is_empty());
    }

    #[test]
    fn a_missing_file_is_written_with_the_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");

        let created = Config::load_or_create(&path).unwrap();
        assert!(path.exists());
        assert_eq!(Config::load_or_create(&path).unwrap(), created);
    }

    #[test]
    fn partial_files_fill_in_the_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");
        std::fs::write(&path, "port = 9000\n").unwrap();

        let config = Config::load_or_create(&path).unwrap();
        assert_eq!(config.port, 9000);
        assert_eq!(config.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn wildcard_binds_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");
        std::fs::write(&path, "bind = \"0.0.0.0\"\n").unwrap();

        assert!(matches!(
            Config::load_or_create(&path),
            Err(ConfigError::UnspecifiedBind(_))
        ));
    }

    #[test]
    fn unknown_keys_are_a_typo_worth_reporting() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");
        std::fs::write(&path, "prot = 9000\n").unwrap();

        assert!(matches!(
            Config::load_or_create(&path),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn stopping_is_configurable_and_defaults_to_an_interrupt() {
        let config = Config::default();

        assert_eq!(config.stop_keys(), ["C-c"]);
        assert_eq!(config.stop_grace().as_secs(), 3);
        assert_eq!(config.poll_interval().as_secs(), 2);
    }

    #[test]
    fn a_file_can_replace_the_stop_sequence() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");
        std::fs::write(
            &path,
            "stop_keys = [\"Escape\", \"q\"]\nstop_grace_secs = 10\npoll_secs = 5\n",
        )
        .unwrap();

        let config = Config::load_or_create(&path).unwrap();

        assert_eq!(config.stop_keys(), ["Escape", "q"]);
        assert_eq!(config.stop_grace().as_secs(), 10);
        assert_eq!(config.poll_interval().as_secs(), 5);
    }

    #[test]
    fn a_zero_poll_interval_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");
        std::fs::write(&path, "poll_secs = 0\n").unwrap();

        assert!(matches!(
            Config::load_or_create(&path),
            Err(ConfigError::ZeroPoll)
        ));
    }

    #[test]
    fn a_tailnet_bind_is_allowed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("daemon.toml");
        std::fs::write(&path, "bind = \"100.64.0.1\"\n").unwrap();

        assert_eq!(
            Config::load_or_create(&path).unwrap().bind.to_string(),
            "100.64.0.1"
        );
    }
}
