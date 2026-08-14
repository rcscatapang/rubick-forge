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

use crate::fleet::Fleet;
use crate::http::AppState;

/// The keychain service the bot's secrets are filed under, matching the one
/// the desktop app uses for machine tokens.
const KEYCHAIN_SERVICE: &str = "tech.cloverly.rubick-forge";

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

    let token = match secret(&config.token_ref) {
        Ok(token) => token,
        Err(error) => {
            tracing::error!(%error, "the Telegram bot is enabled but has no token; not starting it");
            return;
        }
    };

    // A machine whose token cannot be read is still listed: it reports itself
    // as refusing rather than vanishing from `/status`.
    let tokens: Vec<String> = state
        .config
        .machines
        .iter()
        .map(|machine| {
            secret(&machine.token_ref).unwrap_or_else(|error| {
                tracing::warn!(machine = machine.name, %error, "cannot read a machine's token");
                String::new()
            })
        })
        .collect();

    let telegram = match Telegram::new(BotToken::new(token)) {
        Ok(telegram) => telegram,
        Err(error) => {
            tracing::error!(%error, "cannot build a Telegram client");
            return;
        }
    };

    let fleet = std::sync::Arc::new(Fleet::new(state.clone(), &state.config.machines, &tokens));

    tracing::info!(machines = fleet.len(), "starting the Telegram bot");
    tokio::spawn(Bot::new(telegram, fleet, state, &config).run());
}

/// One secret from the login keychain.
fn secret(account: &str) -> Result<String, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account)
        .map_err(|err| format!("cannot reach the keychain: {err}"))?
        .get_password()
        .map_err(|err| format!("no keychain entry `{account}`: {err}"))
}
