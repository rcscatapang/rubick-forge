//! Domain types shared by the daemon and (via serde JSON) the desktop app.
//!
//! Everything here is wire-visible: the JSON produced by these types is the
//! daemon's HTTP/WS contract, mirrored by hand in
//! `apps/desktop/src/lib/api-types.ts`.

mod adapter;
mod entity;
mod event;
mod git;
mod health;
mod status;
mod timestamp;

pub use adapter::{AdapterId, UnknownAdapterId, CLAUDE_CODE, CODEX};
pub use entity::{
    task_branch_name, tmux_session_name, AdapterSettings, ChecksState, Project, QueueState,
    QueuedTask, Session, Task, TaskGitHub,
};
pub use event::{
    EventDecodeError, EventKind, EventRecord, ForgeEvent, StopReason, UnknownEventKind,
};
pub use git::GitStatus;
pub use health::{AdapterLoadError, BinaryStatus, Health};
pub use status::{AgentStatus, UnknownAgentStatus};
pub use timestamp::{InvalidTimestamp, Timestamp};
