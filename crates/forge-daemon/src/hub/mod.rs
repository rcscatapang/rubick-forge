//! Hub mode: a task queue that dispatches to whichever Mac can take the work.
//!
//! A config flag on the always-on daemon (SPEC D25), not a separate program.
//! Workers stay entirely unaware: the hub reaches them over the same HTTP API
//! the desktop app uses, and nothing ever connects inbound to the hub.

pub mod dispatch;
pub mod placement;

use std::sync::{Arc, OnceLock};

use crate::fleet::Fleet;
use crate::http::AppState;

/// The fleet this daemon speaks for, built once.
///
/// Reading the keychain is a synchronous, permission-checked call; doing it per
/// request would put one in the middle of every `/fleet`. A token changed on
/// disk needs a restart, which is the same as every other secret here.
static FLEET: OnceLock<Arc<Fleet>> = OnceLock::new();

/// The shared fleet, building it the first time it is asked for.
pub fn shared_fleet(state: &AppState) -> Arc<Fleet> {
    Arc::clone(FLEET.get_or_init(|| Arc::new(fleet_for(state))))
}

/// The fleet this daemon speaks for.
///
/// Built here rather than in the bot or the hub separately, so both agree on
/// what the fleet is and a machine's position means the same thing to each —
/// which matters, because a Telegram button records one.
pub fn fleet_for(state: &AppState) -> Fleet {
    let tokens: Vec<String> = state
        .config
        .machines
        .iter()
        .map(|machine| {
            crate::secret(&machine.token_ref).unwrap_or_else(|error| {
                tracing::warn!(machine = machine.name, %error, "cannot read a machine's token");
                String::new()
            })
        })
        .collect();

    Fleet::new(state.clone(), &state.config.machines, &tokens)
}

/// Start the dispatch loop, if this daemon is a hub.
pub fn spawn(state: AppState) {
    if !state.config.hub {
        return;
    }

    let fleet = shared_fleet(&state);
    tracing::info!(machines = fleet.len(), "starting the hub dispatch loop");

    tokio::spawn(dispatch::run(state, fleet));
}
