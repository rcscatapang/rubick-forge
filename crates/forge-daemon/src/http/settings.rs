//! `/settings` — daemon-wide preferences every client shares.
//!
//! Deliberately untyped: what a setting means is the business of whoever reads
//! it, and a daemon that validated every client's preferences would need
//! changing every time a client grew one.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::error::{ApiError, ApiResult};
use super::AppState;

/// Long enough for a path or a JSON blob, short enough that the settings table
/// cannot become a file store.
const MAX_VALUE: usize = 4_096;
const MAX_KEY: usize = 128;

#[derive(Debug, Serialize)]
pub struct Settings {
    pub settings: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct SettingsPatch {
    /// A `null` value clears the key.
    #[serde(flatten)]
    pub settings: BTreeMap<String, Option<String>>,
}

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<Settings>> {
    Ok(Json(Settings {
        settings: state.store.settings()?,
    }))
}

/// Set or clear the keys named, leaving the rest alone.
pub async fn patch(
    State(state): State<AppState>,
    Json(request): Json<SettingsPatch>,
) -> ApiResult<Json<Settings>> {
    // Everything is checked before anything is written, and the write itself
    // is one transaction: a half-applied set of preferences would leave a
    // client showing a state nobody asked for.
    for (key, value) in &request.settings {
        if key.trim().is_empty() {
            return Err(ApiError::bad_request("a setting needs a key"));
        }
        if key.len() > MAX_KEY {
            return Err(ApiError::bad_request(format!(
                "a setting key may be at most {MAX_KEY} characters"
            )));
        }
        if value.as_ref().is_some_and(|value| value.len() > MAX_VALUE) {
            return Err(ApiError::bad_request(format!(
                "`{key}` is longer than {MAX_VALUE} characters"
            )));
        }
    }

    state.store.apply_settings(&request.settings)?;

    Ok(Json(Settings {
        settings: state.store.settings()?,
    }))
}
