use serde::{Deserialize, Serialize};

/// What the daemon found when it looked for an external binary it depends on.
///
/// Covers the hard dependencies (`tmux`, `git`) and, each
/// adapter's CLI. A missing binary is a warning on `/health`, never a crash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinaryStatus {
    /// The name looked up on `PATH`, e.g. `tmux` or `claude`.
    pub name: String,
    /// Resolved absolute path, when found.
    pub path: Option<String>,
    /// Version string as the binary reported it.
    pub version: Option<String>,
    /// Whether the binary is present *and* usable for its purpose (a too-old
    /// tmux is found but not ok).
    pub ok: bool,
    /// Human-readable reason when `ok` is false.
    pub detail: Option<String>,
}

impl BinaryStatus {
    pub fn missing(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            detail: Some(format!("`{name}` was not found on PATH")),
            name,
            path: None,
            version: None,
            ok: false,
        }
    }
}

/// The body of `GET /health` — the one unauthenticated endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub version: String,
    pub uptime_secs: u64,
    /// This daemon's machine name, so a multi-daemon UI can label it.
    pub machine: String,
    pub binaries: Vec<BinaryStatus>,
}

impl Health {
    /// Whether every dependency the daemon needs to run agents is usable.
    pub fn is_healthy(&self) -> bool {
        self.binaries.iter().all(|binary| binary.ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_binary_explains_itself() {
        let status = BinaryStatus::missing("tmux");
        assert!(!status.ok);
        assert_eq!(status.detail.unwrap(), "`tmux` was not found on PATH");
    }

    #[test]
    fn health_is_only_green_when_every_binary_is() {
        let mut health = Health {
            version: "0.1.0".into(),
            uptime_secs: 12,
            machine: "mac-mini".into(),
            binaries: vec![],
        };
        assert!(health.is_healthy());
        health.binaries.push(BinaryStatus::missing("tmux"));
        assert!(!health.is_healthy());
    }
}
