//! `GET /health` — the one endpoint that answers without a token, so the app
//! can tell "daemon down" from "wrong token".

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::Json;
use forge_core::{BinaryStatus, Health};

use super::AppState;
use crate::binaries;

/// How long a binary probe stays good for. `/health` is unauthenticated, and
/// each probe spawns processes; without a cache a caller could make the daemon
/// fork as fast as it can ask.
const PROBE_TTL: Duration = Duration::from_secs(30);

/// A probe result and the moment it was taken.
type Probe = Option<(Instant, Vec<BinaryStatus>)>;

/// The most recent probe, shared by every request.
#[derive(Clone, Default)]
pub struct BinaryCache(Arc<Mutex<Probe>>);

impl BinaryCache {
    async fn get(&self) -> Vec<BinaryStatus> {
        if let Some(fresh) = self.fresh() {
            return fresh;
        }

        // Two racing requests may both probe; both write the same answer, so
        // the only cost is one redundant pair of processes.
        let probed = binaries::probe_required().await;
        *self.lock() = Some((Instant::now(), probed.clone()));
        probed
    }

    fn fresh(&self) -> Option<Vec<BinaryStatus>> {
        self.lock()
            .as_ref()
            .filter(|(taken_at, _)| taken_at.elapsed() < PROBE_TTL)
            .map(|(_, statuses)| statuses.clone())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Probe> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        version: state.version.to_owned(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        machine: state.machine.to_string(),
        binaries: state.binaries.get().await,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_second_call_is_served_from_the_cache() {
        let cache = BinaryCache::default();

        let first = cache.get().await;
        let taken_at = cache.lock().as_ref().unwrap().0;
        let second = cache.get().await;

        assert_eq!(first, second);
        assert_eq!(cache.lock().as_ref().unwrap().0, taken_at, "not re-probed");
    }

    #[tokio::test]
    async fn a_stale_entry_is_probed_again() {
        let cache = BinaryCache::default();
        cache.get().await;

        let stale = Instant::now() - PROBE_TTL - Duration::from_secs(1);
        cache.lock().as_mut().unwrap().0 = stale;
        cache.get().await;

        assert!(cache.lock().as_ref().unwrap().0 > stale);
    }
}
