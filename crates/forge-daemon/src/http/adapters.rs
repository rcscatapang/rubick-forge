//! `/adapters` — what this daemon can drive, and how it can be configured.

use axum::extract::State;
use axum::Json;
use forge_core::{AdapterId, BinaryStatus};
use serde::Serialize;

use super::error::ApiResult;
use super::AppState;
use crate::adapters::{self, SettingDef};

#[derive(Debug, Serialize)]
pub struct AdapterInfo {
    pub id: AdapterId,
    pub name: &'static str,
    /// Whether the CLI is installed and answering.
    pub binary: BinaryStatus,
    /// The settings a project may set for this adapter.
    pub settings: &'static [SettingDef],
}

#[derive(Debug, Serialize)]
pub struct AdapterList {
    pub adapters: Vec<AdapterInfo>,
}

/// Everything a client needs to offer a choice of agent and render its
/// settings form, without hard-coding either.
pub async fn list(State(_state): State<AppState>) -> ApiResult<Json<AdapterList>> {
    let mut listed = Vec::new();

    for adapter in adapters::all() {
        listed.push(AdapterInfo {
            id: adapter.id(),
            name: adapter.id().display_name(),
            binary: adapter.binary_check().await,
            settings: adapter.settings_schema(),
        });
    }

    Ok(Json(AdapterList { adapters: listed }))
}
