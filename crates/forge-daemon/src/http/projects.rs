//! `/projects` — register, list, edit and remove local repositories.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::Json;
use forge_core::{ForgeEvent, GitStatus, Project};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::{ApiError, ApiResult};
use super::AppState;
use crate::adapters::settings::{self, SettingsError};
use crate::git::{self, GitError};
use crate::store::{NewProject, ProjectError, ProjectPatch};

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    /// Any path inside the repository; it is resolved to the root.
    pub path: String,
    /// Defaults to the repository directory's own name.
    pub name: Option<String>,
    pub adapter_settings: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PatchRequest {
    pub name: Option<String>,
    pub default_branch: Option<String>,
    pub adapter_settings: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct ProjectList {
    pub projects: Vec<Project>,
}

pub async fn list(State(state): State<AppState>) -> ApiResult<Json<ProjectList>> {
    Ok(Json(ProjectList {
        projects: state.store.projects()?,
    }))
}

pub async fn get(State(state): State<AppState>, Path(id): Path<i64>) -> ApiResult<Json<Project>> {
    Ok(Json(load(&state, id)?))
}

/// Git facts for the repository root. Its own endpoint because it shells out,
/// and the list must stay cheap however many projects are registered.
pub async fn git_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<GitStatus>> {
    let project = load(&state, id)?;

    git::status(root_of(&project))
        .await
        .map(Json)
        .map_err(ApiError::from)
}

pub async fn register(
    State(state): State<AppState>,
    Json(request): Json<RegisterRequest>,
) -> ApiResult<(axum::http::StatusCode, Json<Project>)> {
    let root = git::repo_root(&PathBuf::from(&request.path)).await?;
    let path = root.display().to_string();

    // Checked before writing so the caller gets "already registered" rather
    // than a constraint error; the insert re-checks, which is what actually
    // makes it safe.
    if let Some(existing) = state.store.project_by_path(&path)? {
        return Err(ApiError::conflict(format!(
            "{path} is already registered as project {}",
            existing.id
        )));
    }

    let new = NewProject {
        name: request.name.unwrap_or_else(|| directory_name(&root)),
        path,
        default_branch: git::default_branch(&root).await?,
        adapter_settings: match request.adapter_settings {
            Some(raw) => settings::parse(raw)?,
            None => Default::default(),
        },
    };

    let project = state.store.create_project(&new)?;

    state.bus.publish(ForgeEvent::ProjectRegistered {
        project_id: project.id,
        name: project.name.clone(),
        path: project.path.clone(),
    })?;

    Ok((axum::http::StatusCode::CREATED, Json(project)))
}

pub async fn patch(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(request): Json<PatchRequest>,
) -> ApiResult<Json<Project>> {
    let project = load(&state, id)?;

    if let Some(branch) = &request.default_branch {
        require_branch(root_of(&project), branch).await?;
    }

    let patch = ProjectPatch {
        name: request.name,
        default_branch: request.default_branch,
        adapter_settings: request.adapter_settings.map(settings::parse).transpose()?,
    };

    Ok(Json(state.store.update_project(id, &patch)?))
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<axum::http::StatusCode> {
    let project = load(&state, id)?;

    let live = state.store.live_task_count(id)?;
    if live > 0 {
        return Err(ApiError::conflict(format!(
            "{} still has {live} task(s) that are not stopped; stop them first",
            project.name
        )));
    }

    state.store.delete_project(id)?;
    state
        .bus
        .publish(ForgeEvent::ProjectRemoved { project_id: id })?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// The repository root as a path. `Project.path` is a `String` because that
/// is what it is on the wire.
fn root_of(project: &Project) -> &std::path::Path {
    std::path::Path::new(&project.path)
}

fn load(state: &AppState, id: i64) -> ApiResult<Project> {
    state
        .store
        .project(id)?
        .ok_or_else(|| ApiError::not_found(format!("there is no project with id {id}")))
}

async fn require_branch(repo: &std::path::Path, branch: &str) -> ApiResult<()> {
    if git::branch_exists(repo, branch).await? {
        return Ok(());
    }
    Err(ApiError::bad_request(format!(
        "`{branch}` is not a branch in this repository"
    )))
}

/// The repository directory's own name, which is what a person calls it.
fn directory_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

impl From<GitError> for ApiError {
    fn from(err: GitError) -> Self {
        match err {
            GitError::NotADirectory(_) | GitError::NotARepository(_) => {
                Self::bad_request(err.to_string())
            }
            GitError::Refused { detail } => Self::conflict(format!("git refused: {detail}")),
            GitError::Missing => Self::new(
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "git_missing",
                err.to_string(),
            ),
            // git's own words go to the log, not to the client: they are
            // full of paths and jargon a UI cannot act on.
            GitError::Unreadable { .. } | GitError::Failed { .. } => {
                tracing::warn!(error = %err, "git command failed");
                Self::internal("the daemon could not read this repository; check the daemon logs")
            }
        }
    }
}

impl From<SettingsError> for ApiError {
    fn from(err: SettingsError) -> Self {
        Self::bad_request(err.to_string())
    }
}

impl From<ProjectError> for ApiError {
    fn from(err: ProjectError) -> Self {
        match err {
            ProjectError::DuplicatePath(_) => Self::conflict(err.to_string()),
            ProjectError::NotFound(_) => Self::not_found(err.to_string()),
            ProjectError::Store(store) => store.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_defaults_to_the_directory() {
        assert_eq!(
            directory_name(std::path::Path::new("/repos/forge")),
            "forge"
        );
    }

    #[test]
    fn git_problems_map_to_the_status_the_caller_deserves() {
        assert_eq!(
            ApiError::from(GitError::NotARepository("/tmp".into())).status(),
            axum::http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiError::from(GitError::Missing).status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            ApiError::from(GitError::Failed {
                command: "status".into(),
                detail: "boom".into()
            })
            .status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn a_duplicate_registration_is_a_conflict_not_a_server_error() {
        assert_eq!(
            ApiError::from(ProjectError::DuplicatePath("/repos/forge".into())).status(),
            axum::http::StatusCode::CONFLICT
        );
        assert_eq!(
            ApiError::from(ProjectError::NotFound(7)).status(),
            axum::http::StatusCode::NOT_FOUND
        );
    }
}
