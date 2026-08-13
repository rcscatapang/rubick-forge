use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The heuristic state of a running agent session.
///
/// Accuracy is "good enough to glance at" by design; `Waiting` is the one
/// variant tuned for high recall, because notifications hang off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Session alive, agent sitting at an empty prompt.
    Idle,
    /// Session alive, agent producing output.
    Working,
    /// Session alive, agent blocked on a human (permission prompt, confirmation).
    Waiting,
    /// Process died with a nonzero exit.
    Error,
    /// Process died cleanly, or the session was never started.
    Stopped,
}

impl AgentStatus {
    /// Every status, in the order the UI ranks them.
    pub const ALL: [AgentStatus; 5] = [
        Self::Idle,
        Self::Working,
        Self::Waiting,
        Self::Error,
        Self::Stopped,
    ];

    /// The wire form, identical to the serde representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Error => "error",
            Self::Stopped => "stopped",
        }
    }

    /// Whether the agent needs a human before it can make progress.
    pub const fn needs_attention(self) -> bool {
        matches!(self, Self::Waiting | Self::Error)
    }

    /// Whether a tmux session is expected to exist for this status.
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Idle | Self::Working | Self::Waiting)
    }
}

impl fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned when a string does not name a known [`AgentStatus`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownAgentStatus(pub String);

impl fmt::Display for UnknownAgentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown agent status: {}", self.0)
    }
}

impl std::error::Error for UnknownAgentStatus {}

impl FromStr for AgentStatus {
    type Err = UnknownAgentStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "idle" => Ok(Self::Idle),
            "working" => Ok(Self::Working),
            "waiting" => Ok(Self::Waiting),
            "error" => Ok(Self::Error),
            "stopped" => Ok(Self::Stopped),
            other => Err(UnknownAgentStatus(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_round_trips_through_json() {
        for status in AgentStatus::ALL {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, format!("\"{status}\""));
            assert_eq!(serde_json::from_str::<AgentStatus>(&json).unwrap(), status);
        }
    }

    #[test]
    fn from_str_matches_the_wire_form() {
        for status in AgentStatus::ALL {
            assert_eq!(status.as_str().parse::<AgentStatus>().unwrap(), status);
        }
        assert!("busy".parse::<AgentStatus>().is_err());
    }

    #[test]
    fn only_live_statuses_expect_a_session() {
        assert!(AgentStatus::Working.is_live());
        assert!(!AgentStatus::Stopped.is_live());
        assert!(!AgentStatus::Error.is_live());
    }

    #[test]
    fn waiting_and_error_need_a_human() {
        assert!(AgentStatus::Waiting.needs_attention());
        assert!(AgentStatus::Error.needs_attention());
        assert!(!AgentStatus::Working.needs_attention());
    }
}
