//! The hub's own routes: the queue, and one merged view of the fleet.
//!
//! Present on every daemon and answering `not_a_hub` unless `hub = true`, so a
//! client can ask without knowing which Mac it is talking to.

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use forge_core::{AdapterId, ForgeEvent, QueuedTask};
use serde::{Deserialize, Serialize};

use super::error::{ApiError, ApiResult};
use super::AppState;
use crate::hub::placement::Inventory;
use crate::store::NewQueuedTask;

fn require_hub(state: &AppState) -> ApiResult<()> {
    if state.config.hub {
        return Ok(());
    }

    Err(ApiError::new(
        StatusCode::PRECONDITION_FAILED,
        "not_a_hub",
        "This daemon is not a hub. Set hub = true in its daemon.toml.",
    ))
}

#[derive(Debug, Serialize)]
pub struct HubStatus {
    pub hub: bool,
    /// Machine names this hub dispatches to, this Mac first.
    pub machines: Vec<String>,
}

/// Whether this daemon is a hub, which every client wants to know first.
pub async fn status(State(state): State<AppState>) -> ApiResult<Json<HubStatus>> {
    let mut machines = vec![state.machine.to_string()];
    machines.extend(state.config.machines.iter().map(|entry| entry.name.clone()));

    Ok(Json(HubStatus {
        hub: state.config.hub,
        machines: if state.config.hub {
            machines
        } else {
            Vec::new()
        },
    }))
}

#[derive(Debug, Deserialize)]
pub struct EnqueueRequest {
    /// Matched against machines' projects by name, not path.
    pub project_name: String,
    pub title: String,
    #[serde(default)]
    pub adapter: Option<AdapterId>,
    #[serde(default)]
    pub prompt: Option<String>,
    /// A machine name, or omitted for "whichever machine can take it".
    #[serde(default)]
    pub target: Option<String>,
}

/// Put something on the queue. No task exists until it is dispatched.
pub async fn enqueue(
    State(state): State<AppState>,
    Json(request): Json<EnqueueRequest>,
) -> ApiResult<(StatusCode, Json<QueuedTask>)> {
    require_hub(&state)?;

    let project_name = request.project_name.trim();
    let title = request.title.trim();

    if project_name.is_empty() {
        return Err(ApiError::bad_request("a queued task needs a project name"));
    }
    if title.is_empty() {
        return Err(ApiError::bad_request("a queued task needs a title"));
    }

    // A target that is not configured is refused here rather than left to sit
    // on the queue: it is a typo, and it will never become placeable.
    if let Some(target) = request
        .target
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        let known = std::iter::once(state.machine.to_string())
            .chain(state.config.machines.iter().map(|entry| entry.name.clone()))
            .any(|name| name.eq_ignore_ascii_case(target));

        if !known {
            return Err(ApiError::bad_request(format!(
                "there is no machine called {target} in this hub's configuration"
            )));
        }
    }

    let row = state.store.enqueue(&NewQueuedTask {
        project_name: project_name.to_owned(),
        adapter: request.adapter.unwrap_or(AdapterId::ClaudeCode),
        title: title.to_owned(),
        prompt: request
            .prompt
            .map(|prompt| prompt.trim().to_owned())
            .filter(|prompt| !prompt.is_empty()),
        target: request.target.map(|target| target.trim().to_owned()),
    })?;

    state.bus.publish(ForgeEvent::TaskQueued {
        queued_id: row.id,
        project_name: row.project_name.clone(),
        title: row.title.clone(),
        target: row.target.clone(),
    })?;

    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Debug, Serialize)]
pub struct QueueList {
    pub queue: Vec<QueuedTask>,
}

/// The whole queue, including what has already been dispatched — that is the
/// dispatch history the audit trail lives in.
pub async fn queue(State(state): State<AppState>) -> ApiResult<Json<QueueList>> {
    require_hub(&state)?;

    Ok(Json(QueueList {
        queue: state.store.queue()?,
    }))
}

/// Cancel something that has not been dispatched yet.
pub async fn cancel(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<StatusCode> {
    require_hub(&state)?;

    let Some(row) = state.store.queued_task(id)? else {
        return Err(ApiError::not_found(format!("there is no queued task {id}")));
    };

    if !state.store.cancel_queued(id)? {
        return Err(ApiError::bad_request(format!(
            "that was already {}. A dispatched task is stopped on the machine \
             running it.",
            row.state.as_str()
        )));
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct FleetView {
    pub machines: Vec<MachineView>,
}

#[derive(Debug, Serialize)]
pub struct MachineView {
    pub name: String,
    pub reachable: bool,
    pub projects: Vec<String>,
    /// How many sessions it is running, which is what placement looks at.
    pub active: usize,
}

/// One merged view of the fleet, so a phone-class client can be hub-only.
///
/// The desktop app keeps its own per-machine connections; this exists for a
/// client that cannot hold several.
pub async fn fleet(State(state): State<AppState>) -> ApiResult<Json<FleetView>> {
    require_hub(&state)?;

    let fleet = crate::hub::shared_fleet(&state);
    let inventory = crate::hub::dispatch::take_inventory(&fleet).await;

    Ok(Json(FleetView {
        machines: inventory.into_iter().map(as_view).collect(),
    }))
}

fn as_view(machine: Inventory) -> MachineView {
    MachineView {
        name: machine.name,
        reachable: machine.reachable,
        projects: machine.projects.into_iter().map(|(_, name)| name).collect(),
        active: machine.active,
    }
}
