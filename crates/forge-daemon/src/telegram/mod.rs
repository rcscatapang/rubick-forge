//! The Telegram bot (SPEC D21-D22).
//!
//! Hosted by the always-on Mac's daemon, which is the only one that needs the
//! machines list. Outbound only: the bot long-polls Telegram and reaches other
//! daemons over their ordinary HTTP API. Nothing here opens a port.

pub mod api;
pub mod bot;
pub mod callbacks;
pub mod commands;
pub mod format;

use api::{BotToken, Telegram};
use bot::Bot;

use crate::http::AppState;

/// Start the bot if the config asks for one.
///
/// A bot that cannot start is logged and skipped, never fatal: a daemon that
/// refuses to run its agents because Telegram is unreachable would have the
/// priorities backwards.
pub fn spawn(state: AppState) {
    let config = state.config.telegram.clone();
    if !config.enabled {
        return;
    }

    let token = match crate::secret(&config.token_ref) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "the Telegram bot is enabled but has no token; not starting it");
            return;
        }
    };

    let telegram = match Telegram::new(BotToken::new(token)) {
        Ok(telegram) => telegram,
        Err(error) => {
            tracing::error!(%error, "cannot build a Telegram client");
            return;
        }
    };

    // The same fleet the hub builds, so a machine's position means the same
    // thing to both — which matters, because a button records one.
    let fleet = crate::hub::shared_fleet(&state);

    tracing::info!(machines = fleet.len(), "starting the Telegram bot");
    tokio::spawn(Bot::new(telegram, fleet, state, &config).run());
}
