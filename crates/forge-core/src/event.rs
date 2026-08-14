use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{AgentStatus, Timestamp};

/// The discriminant of a [`ForgeEvent`], stored in the `events.kind` column and
/// used by clients to filter without parsing payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    ProjectRegistered,
    ProjectRemoved,
    TaskCreated,
    TaskDeleted,
    TaskFinished,
    SessionStarted,
    SessionStopped,
    StatusChanged,
    AgentWaiting,
    AgentError,
    WorktreeCreated,
    WorktreeRemoved,
    PrOpened,
    PrMerged,
    PrClosed,
    ChecksPassed,
    ChecksFailed,
}

impl EventKind {
    pub const ALL: [EventKind; 17] = [
        Self::ProjectRegistered,
        Self::ProjectRemoved,
        Self::TaskCreated,
        Self::TaskDeleted,
        Self::TaskFinished,
        Self::SessionStarted,
        Self::SessionStopped,
        Self::StatusChanged,
        Self::AgentWaiting,
        Self::AgentError,
        Self::WorktreeCreated,
        Self::WorktreeRemoved,
        Self::PrOpened,
        Self::PrMerged,
        Self::PrClosed,
        Self::ChecksPassed,
        Self::ChecksFailed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectRegistered => "project_registered",
            Self::ProjectRemoved => "project_removed",
            Self::TaskCreated => "task_created",
            Self::TaskDeleted => "task_deleted",
            Self::TaskFinished => "task_finished",
            Self::SessionStarted => "session_started",
            Self::SessionStopped => "session_stopped",
            Self::StatusChanged => "status_changed",
            Self::AgentWaiting => "agent_waiting",
            Self::AgentError => "agent_error",
            Self::WorktreeCreated => "worktree_created",
            Self::WorktreeRemoved => "worktree_removed",
            Self::PrOpened => "pr_opened",
            Self::PrMerged => "pr_merged",
            Self::PrClosed => "pr_closed",
            Self::ChecksPassed => "checks_passed",
            Self::ChecksFailed => "checks_failed",
        }
    }

    /// Whether this kind is one the desktop app turns into a native
    /// notification.
    ///
    /// A failed check and a merged pull request join the agent triad: both are
    /// the end of something the human was waiting on, and both happen while
    /// they are looking elsewhere. Nothing else GitHub reports is worth an
    /// interruption — a pull request opening is something you just did.
    pub const fn is_notifiable(self) -> bool {
        matches!(
            self,
            Self::AgentWaiting
                | Self::TaskFinished
                | Self::AgentError
                | Self::ChecksFailed
                | Self::PrMerged
        )
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned when a string does not name a known [`EventKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEventKind(pub String);

impl fmt::Display for UnknownEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown event kind: {}", self.0)
    }
}

impl std::error::Error for UnknownEventKind {}

impl FromStr for EventKind {
    type Err = UnknownEventKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| UnknownEventKind(s.to_owned()))
    }
}

/// Why a session ended, carried on [`ForgeEvent::SessionStopped`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// A client asked for it.
    Requested,
    /// The agent process exited on its own.
    Exited,
    /// The tmux session disappeared behind the daemon's back.
    Vanished,
}

/// Everything the daemon announces on the bus.
///
/// The JSON form is flat and tagged — `{"kind":"task_created","task_id":1,…}` —
/// which is exactly what `/ws/events` delivers and what an `events` row
/// reassembles to. Ids that have their own indexed column are repeated in the
/// payload on purpose: the columns are an index, the JSON is the contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForgeEvent {
    ProjectRegistered {
        project_id: i64,
        name: String,
        path: String,
    },
    ProjectRemoved {
        project_id: i64,
    },
    TaskCreated {
        task_id: i64,
        project_id: i64,
        title: String,
    },
    TaskDeleted {
        task_id: i64,
    },
    TaskFinished {
        task_id: i64,
        session_id: i64,
    },
    SessionStarted {
        task_id: i64,
        session_id: i64,
        tmux_name: String,
    },
    SessionStopped {
        task_id: i64,
        session_id: i64,
        reason: StopReason,
    },
    StatusChanged {
        task_id: i64,
        session_id: i64,
        from: AgentStatus,
        to: AgentStatus,
    },
    /// The one event tuned for recall: `tail` is the sanitised pane tail
    /// that tells the human what is being asked of them.
    AgentWaiting {
        task_id: i64,
        session_id: i64,
        tail: String,
    },
    AgentError {
        task_id: i64,
        session_id: i64,
        detail: String,
    },
    WorktreeCreated {
        task_id: i64,
        path: String,
        branch: String,
    },
    WorktreeRemoved {
        task_id: i64,
        path: String,
    },
    PrOpened {
        task_id: i64,
        number: i64,
        url: String,
    },
    PrMerged {
        task_id: i64,
        number: i64,
        url: String,
    },
    /// Closed without being merged, which is a different outcome to report.
    PrClosed {
        task_id: i64,
        number: i64,
        url: String,
    },
    ChecksPassed {
        task_id: i64,
        number: i64,
        url: String,
    },
    /// The one GitHub event worth interrupting someone for.
    ChecksFailed {
        task_id: i64,
        number: i64,
        url: String,
    },
}

impl ForgeEvent {
    pub fn kind(&self) -> EventKind {
        match self {
            Self::ProjectRegistered { .. } => EventKind::ProjectRegistered,
            Self::ProjectRemoved { .. } => EventKind::ProjectRemoved,
            Self::TaskCreated { .. } => EventKind::TaskCreated,
            Self::TaskDeleted { .. } => EventKind::TaskDeleted,
            Self::TaskFinished { .. } => EventKind::TaskFinished,
            Self::SessionStarted { .. } => EventKind::SessionStarted,
            Self::SessionStopped { .. } => EventKind::SessionStopped,
            Self::StatusChanged { .. } => EventKind::StatusChanged,
            Self::AgentWaiting { .. } => EventKind::AgentWaiting,
            Self::AgentError { .. } => EventKind::AgentError,
            Self::WorktreeCreated { .. } => EventKind::WorktreeCreated,
            Self::WorktreeRemoved { .. } => EventKind::WorktreeRemoved,
            Self::PrOpened { .. } => EventKind::PrOpened,
            Self::PrMerged { .. } => EventKind::PrMerged,
            Self::PrClosed { .. } => EventKind::PrClosed,
            Self::ChecksPassed { .. } => EventKind::ChecksPassed,
            Self::ChecksFailed { .. } => EventKind::ChecksFailed,
        }
    }

    /// The `events.task_id` column value.
    pub fn task_id(&self) -> Option<i64> {
        match self {
            Self::ProjectRegistered { .. } | Self::ProjectRemoved { .. } => None,
            Self::TaskCreated { task_id, .. }
            | Self::TaskDeleted { task_id }
            | Self::TaskFinished { task_id, .. }
            | Self::SessionStarted { task_id, .. }
            | Self::SessionStopped { task_id, .. }
            | Self::StatusChanged { task_id, .. }
            | Self::AgentWaiting { task_id, .. }
            | Self::AgentError { task_id, .. }
            | Self::WorktreeCreated { task_id, .. }
            | Self::WorktreeRemoved { task_id, .. }
            | Self::PrOpened { task_id, .. }
            | Self::PrMerged { task_id, .. }
            | Self::PrClosed { task_id, .. }
            | Self::ChecksPassed { task_id, .. }
            | Self::ChecksFailed { task_id, .. } => Some(*task_id),
        }
    }

    /// The `events.session_id` column value.
    pub fn session_id(&self) -> Option<i64> {
        match self {
            Self::TaskFinished { session_id, .. }
            | Self::SessionStarted { session_id, .. }
            | Self::SessionStopped { session_id, .. }
            | Self::StatusChanged { session_id, .. }
            | Self::AgentWaiting { session_id, .. }
            | Self::AgentError { session_id, .. } => Some(*session_id),
            _ => None,
        }
    }

    /// The `events.payload` column value: the tagged JSON minus its tag.
    pub fn payload(&self) -> Map<String, Value> {
        let Ok(Value::Object(mut object)) = serde_json::to_value(self) else {
            // `ForgeEvent` is a tagged enum of plain fields, so it always
            // serialises to an object.
            unreachable!("ForgeEvent serialises to a JSON object")
        };
        object.remove("kind");
        object
    }

    /// Rebuild an event from the two columns it was split into.
    pub fn from_parts(
        kind: EventKind,
        mut payload: Map<String, Value>,
    ) -> Result<Self, EventDecodeError> {
        payload.insert("kind".to_owned(), Value::String(kind.as_str().to_owned()));
        serde_json::from_value(Value::Object(payload))
            .map_err(|source| EventDecodeError { kind, source })
    }
}

/// A stored event whose payload no longer matches the current [`ForgeEvent`]
/// shape — a schema drift, not a client error.
#[derive(Debug)]
pub struct EventDecodeError {
    pub kind: EventKind,
    pub source: serde_json::Error,
}

impl fmt::Display for EventDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot decode a {} event: {}", self.kind, self.source)
    }
}

impl std::error::Error for EventDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// A persisted event: an id and a timestamp wrapped around a [`ForgeEvent`].
///
/// This is the row shape of `GET /events` and the frame shape of
/// `WS /ws/events`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: i64,
    pub ts: Timestamp,
    #[serde(flatten)]
    pub event: ForgeEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_of(kind: EventKind) -> ForgeEvent {
        match kind {
            EventKind::ProjectRegistered => ForgeEvent::ProjectRegistered {
                project_id: 1,
                name: "forge".into(),
                path: "/repos/forge".into(),
            },
            EventKind::ProjectRemoved => ForgeEvent::ProjectRemoved { project_id: 1 },
            EventKind::TaskCreated => ForgeEvent::TaskCreated {
                task_id: 2,
                project_id: 1,
                title: "Add adapters".into(),
            },
            EventKind::TaskDeleted => ForgeEvent::TaskDeleted { task_id: 2 },
            EventKind::TaskFinished => ForgeEvent::TaskFinished {
                task_id: 2,
                session_id: 3,
            },
            EventKind::SessionStarted => ForgeEvent::SessionStarted {
                task_id: 2,
                session_id: 3,
                tmux_name: "forge-2".into(),
            },
            EventKind::SessionStopped => ForgeEvent::SessionStopped {
                task_id: 2,
                session_id: 3,
                reason: StopReason::Requested,
            },
            EventKind::StatusChanged => ForgeEvent::StatusChanged {
                task_id: 2,
                session_id: 3,
                from: AgentStatus::Working,
                to: AgentStatus::Waiting,
            },
            EventKind::AgentWaiting => ForgeEvent::AgentWaiting {
                task_id: 2,
                session_id: 3,
                tail: "Allow edit to src/main.rs? (y/n)".into(),
            },
            EventKind::AgentError => ForgeEvent::AgentError {
                task_id: 2,
                session_id: 3,
                detail: "claude exited with 1".into(),
            },
            EventKind::WorktreeCreated => ForgeEvent::WorktreeCreated {
                task_id: 2,
                path: "/repos/.forge-worktrees/forge/add-adapters".into(),
                branch: "forge/add-adapters".into(),
            },
            EventKind::WorktreeRemoved => ForgeEvent::WorktreeRemoved {
                task_id: 2,
                path: "/repos/.forge-worktrees/forge/add-adapters".into(),
            },
            EventKind::PrOpened => ForgeEvent::PrOpened {
                task_id: 2,
                number: 41,
                url: "https://github.com/o/n/pull/41".into(),
            },
            EventKind::PrMerged => ForgeEvent::PrMerged {
                task_id: 2,
                number: 41,
                url: "https://github.com/o/n/pull/41".into(),
            },
            EventKind::PrClosed => ForgeEvent::PrClosed {
                task_id: 2,
                number: 41,
                url: "https://github.com/o/n/pull/41".into(),
            },
            EventKind::ChecksPassed => ForgeEvent::ChecksPassed {
                task_id: 2,
                number: 41,
                url: "https://github.com/o/n/pull/41".into(),
            },
            EventKind::ChecksFailed => ForgeEvent::ChecksFailed {
                task_id: 2,
                number: 41,
                url: "https://github.com/o/n/pull/41".into(),
            },
        }
    }

    #[test]
    fn every_kind_survives_the_split_into_columns() {
        for kind in EventKind::ALL {
            let event = sample_of(kind);
            assert_eq!(event.kind(), kind);
            let restored = ForgeEvent::from_parts(kind, event.payload()).unwrap();
            assert_eq!(restored, event);
        }
    }

    #[test]
    fn payload_drops_the_tag_but_keeps_the_ids() {
        let event = sample_of(EventKind::SessionStarted);
        let payload = event.payload();
        assert!(!payload.contains_key("kind"));
        assert_eq!(payload["task_id"], 2);
        assert_eq!(payload["tmux_name"], "forge-2");
    }

    #[test]
    fn column_ids_match_the_payload() {
        for kind in EventKind::ALL {
            let event = sample_of(kind);
            let payload = event.payload();
            assert_eq!(
                event.task_id(),
                payload.get("task_id").and_then(Value::as_i64)
            );
            assert_eq!(
                event.session_id(),
                payload.get("session_id").and_then(Value::as_i64)
            );
        }
    }

    #[test]
    fn kind_strings_round_trip() {
        for kind in EventKind::ALL {
            assert_eq!(kind.as_str().parse::<EventKind>().unwrap(), kind);
        }
        assert!("exploded".parse::<EventKind>().is_err());
    }

    #[test]
    fn a_mismatched_payload_is_a_decode_error() {
        let mut payload = Map::new();
        payload.insert("task_id".into(), Value::String("two".into()));
        assert!(ForgeEvent::from_parts(EventKind::TaskDeleted, payload).is_err());
    }

    #[test]
    fn records_flatten_the_event_into_the_row() {
        let record = EventRecord {
            id: 7,
            ts: "2026-08-13T09:30:00Z".parse().unwrap(),
            event: sample_of(EventKind::TaskDeleted),
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["id"], 7);
        assert_eq!(json["kind"], "task_deleted");
        assert_eq!(json["task_id"], 2);
        assert_eq!(serde_json::from_value::<EventRecord>(json).unwrap(), record);
    }

    #[test]
    fn only_the_kinds_worth_interrupting_someone_for_are_notifiable() {
        let notifiable: Vec<_> = EventKind::ALL
            .into_iter()
            .filter(|kind| kind.is_notifiable())
            .collect();

        assert_eq!(
            notifiable,
            [
                EventKind::TaskFinished,
                EventKind::AgentWaiting,
                EventKind::AgentError,
                // Both are the end of something the human was waiting on.
                EventKind::PrMerged,
                EventKind::ChecksFailed,
            ]
        );
    }

    #[test]
    fn opening_a_pull_request_is_not_news_to_whoever_opened_it() {
        assert!(!EventKind::PrOpened.is_notifiable());
        assert!(!EventKind::ChecksPassed.is_notifiable());
    }
}
