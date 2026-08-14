//! `/adapters` — what this daemon can drive, and how it can be configured.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use forge_core::{AdapterId, BinaryStatus};
use serde::Serialize;

use super::error::ApiResult;
use super::AppState;
use crate::adapters::{self, LoadError, SettingDef};

#[derive(Debug, Serialize)]
pub struct AdapterInfo {
    pub id: AdapterId,
    pub name: String,
    /// Whether the CLI is installed and answering.
    pub binary: BinaryStatus,
    /// The settings a project may set for this adapter.
    pub settings: Vec<SettingDef>,
}

#[derive(Debug, Serialize)]
pub struct AdapterList {
    pub adapters: Vec<AdapterInfo>,
    /// Manifests that would not load. Reported rather than hidden: an adapter
    /// missing from the list above has a reason, and this is it.
    pub errors: Vec<LoadError>,
}

/// Everything a client needs to offer a choice of agent and render its
/// settings form, without hard-coding either.
pub async fn list(State(_state): State<AppState>) -> ApiResult<Json<AdapterList>> {
    let mut listed = Vec::new();

    for adapter in adapters::all() {
        listed.push(AdapterInfo {
            id: adapter.id().clone(),
            name: adapter.name().to_owned(),
            binary: adapter.binary_check().await,
            settings: adapter.settings_schema(),
        });
    }

    Ok(Json(AdapterList {
        adapters: listed,
        errors: adapters::load_errors(),
    }))
}

#[derive(Debug, Serialize)]
pub struct ReloadResult {
    pub loaded: usize,
    pub errors: Vec<LoadError>,
}

/// Read the adapters directory again.
///
/// Running sessions keep the adapter they started with — their argv is already
/// fixed — so this changes what the next task gets, not what is running.
pub async fn reload(State(_state): State<AppState>) -> ApiResult<(StatusCode, Json<ReloadResult>)> {
    let (loaded, errors) = adapters::reload();

    tracing::info!(loaded, failed = errors.len(), "reloaded adapter manifests");

    Ok((StatusCode::OK, Json(ReloadResult { loaded, errors })))
}
