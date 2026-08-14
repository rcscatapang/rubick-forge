//! The Telegram bot (SPEC D21-D22).
//!
//! Hosted by the always-on Mac's daemon, which is the only one that needs the
//! machines list. Outbound only: the bot long-polls Telegram and reaches other
//! daemons over their ordinary HTTP API. Nothing here opens a port.

pub mod api;
pub mod callbacks;
pub mod commands;
pub mod format;
