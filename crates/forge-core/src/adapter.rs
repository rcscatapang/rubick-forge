use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The agent CLIs Forge knows how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterId {
    ClaudeCode,
    Codex,
}

impl AdapterId {
    /// Every adapter, in the order the UI lists them.
    pub const ALL: [AdapterId; 2] = [Self::ClaudeCode, Self::Codex];

    /// The wire form, identical to the serde representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }

    /// The executable expected on `PATH` for this adapter.
    pub const fn binary_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
        }
    }

    /// Human-facing name for the UI.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

impl fmt::Display for AdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned when a string does not name a known [`AdapterId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownAdapterId(pub String);

impl fmt::Display for UnknownAdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown adapter id: {}", self.0)
    }
}

impl std::error::Error for UnknownAdapterId {}

impl FromStr for AdapterId {
    type Err = UnknownAdapterId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude-code" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            other => Err(UnknownAdapterId(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_round_trips_through_json() {
        for adapter in AdapterId::ALL {
            let json = serde_json::to_string(&adapter).unwrap();
            assert_eq!(json, format!("\"{adapter}\""));
            assert_eq!(serde_json::from_str::<AdapterId>(&json).unwrap(), adapter);
        }
    }

    #[test]
    fn from_str_matches_the_wire_form() {
        for adapter in AdapterId::ALL {
            assert_eq!(adapter.as_str().parse::<AdapterId>().unwrap(), adapter);
        }
        assert!("aider".parse::<AdapterId>().is_err());
    }

    #[test]
    fn binaries_are_the_bare_cli_names() {
        assert_eq!(AdapterId::ClaudeCode.binary_name(), "claude");
        assert_eq!(AdapterId::Codex.binary_name(), "codex");
    }
}
