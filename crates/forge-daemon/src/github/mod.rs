//! GitHub, by polling and nothing else (SPEC D23).
//!
//! No webhooks, ever: an inbound endpoint is exactly the thing this project
//! does not have. Everything here is the daemon asking GitHub a question.

pub mod api;
pub mod pull;
pub mod remote;
