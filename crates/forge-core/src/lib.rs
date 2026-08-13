//! Domain types shared by the daemon and (via serde JSON) the desktop app.
//!
//! Everything here is wire-visible: the JSON produced by these types is the
//! daemon's HTTP/WS contract, mirrored by hand in
//! `apps/desktop/src/lib/api-types.ts` until we generate it.

mod adapter;
mod status;

pub use adapter::AdapterId;
pub use status::AgentStatus;
