//! `/tasks` — the unit of work, and the git worktree it runs in.

use std::path::Path;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use forge_core::{AdapterId, AgentStatus, ForgeEvent, GitStatus, Project, Session, Task};
use serde::{Deserialize, Serialize};

use super::error::{ApiError, ApiResult};
use super::extract::Query;
use super::AppState;
use crate::git;
use crate::sessions::SessionManagerError;
use crate::store::{NewTask, TaskError, TaskPatch, TaskQuery};
use crate::worktree::{self, BranchDisposal};

#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    /// Names this attempt so it is safe to repeat. A second create with the
    /// same key returns the first task rather than making another.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    pub project_id: i64,
    pub title: String,
    pub adapter: AdapterId,
    /// Defaults to the project's default branch.
    pub base_branch: Option<String>,
    pub initial_prompt: Option<String>,
    /// A task without a worktree runs in the repository root.
    #[serde(default = "yes")]
    pub use_worktree: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
pub struct PatchRequest {
    pub title: Option<String>,
    pub initial_prompt: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub project: Option<i64>,
    pub status: Option<AgentStatus>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DeleteQuery {
    /// Clean the worktree up as part of deleting, discarding its changes.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct CleanupRequest {
    /// Remove the worktree even with uncommitted changes, and the branch even
    /// if it was never merged.
    #[serde(default)]
    pub force: bool,
    /// Delete `forge/<slug>` along with the worktree.
    #[serde(default)]
    pub delete_branch: bool,
}

#[derive(Debug, Serialize)]
pub struct TaskList {
    pub tasks: Vec<Task>,
}

#[derive(Debug, Serialize)]
pub struct SessionList {
    pub sessions: Vec<Session>,
}

/// The argv for a task's agent, built from its adapter and the project's
/// settings for that adapter.
fn launch_command(task: &Task, project: &Project) -> ApiResult<Vec<String>> {
    let adapter = require_adapter(task)?;
    let settings = project
        .adapter_settings
        .get(&task.adapter)
        .cloned()
        .unwrap_or_default();

    Ok(adapter.launch_command(task, &settings))
}

/// Long enough for a paragraph of direction, short enough that a client
/// cannot paste a file into someone's terminal.
const MAX_INSTRUCTION_CHARS: usize = 8_000;

#[derive(Debug, Deserialize)]
pub struct InstructionRequest {
    pub text: String,
}

/// Type something into a running agent.
pub async fn instruction(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
    Json(request): Json<InstructionRequest>,
) -> ApiResult<StatusCode> {
    load(&state, id)?;

    if request.text.trim().is_empty() {
        return Err(ApiError::bad_request("an instruction needs some text"));
    }
    if request.text.chars().count() > MAX_INSTRUCTION_CHARS {
        return Err(ApiError::bad_request(format!(
            "an instruction may be at most {MAX_INSTRUCTION_CHARS} characters"
        )));
    }

    state.sessions.send_instruction(id, &request.text).await?;

    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, serde::Deserialize)]
pub struct AnswerRequest {
    /// True takes the safe option, false cancels.
    pub approve: bool,
}

/// Answer a permission dialog a running agent is showing.
///
/// Separate from `/instruction` because the two are different acts: an
/// instruction is text to type, an answer is a keystroke on a dialog. The keys
/// come from the task's adapter, so what "yes" means belongs to whoever knows
/// that CLI's interface.
pub async fn answer(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
    Json(request): Json<AnswerRequest>,
) -> ApiResult<StatusCode> {
    let task = load(&state, id)?;
    let keys = require_adapter(&task)?.answer_keys(request.approve);
    let keys: Vec<&str> = keys.iter().map(String::as_str).collect();

    state.sessions.send_answer(id, &keys).await?;

    Ok(StatusCode::ACCEPTED)
}

#[derive(Debug, serde::Serialize)]
pub struct PaneResponse {
    pub tail: String,
}

/// What is on a task's screen right now.
///
/// The terminal WebSocket is the full-fidelity view; this is the one-shot
/// readable version, for a client that cannot hold a socket open — a phone,
/// via the bot.
pub async fn pane(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<PaneResponse>> {
    load(&state, id)?;

    Ok(Json(PaneResponse {
        tail: state.sessions.pane_tail(id).await?,
    }))
}

/// One session, whichever task it belongs to.
///
/// A terminal view knows a session id and nothing else; without this it cannot
/// find out whose session it is showing.
pub async fn session(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<Session>> {
    state
        .store
        .session(id)?
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("there is no session with id {id}")))
}

/// Every run of a task, oldest first.
pub async fn sessions(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<SessionList>> {
    load(&state, id)?;

    Ok(Json(SessionList {
        sessions: state.store.sessions_for_task(id)?,
    }))
}

pub async fn start(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<(StatusCode, Json<Session>)> {
    let task = load(&state, id)?;
    let project = project_of(&state, &task)?;

    // The runtime is checked first, so a machine with no tmux says so however
    // many agent CLIs it is missing as well.
    state.sessions.preflight().await?;
    require_agent(&task).await?;

    let session = state
        .sessions
        .start(&task, &project, &launch_command(&task, &project)?)
        .await?;

    Ok((StatusCode::CREATED, Json(session)))
}

/// Refuse to start when the agent's CLI is not there, and say which one.
async fn require_agent(task: &Task) -> ApiResult<()> {
    let adapter = require_adapter(task)?;
    let check = adapter.binary_check().await;

    match unusable_agent(&adapter, &check) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// The adapter a task names, or an error saying it is gone.
///
/// A manifest can be deleted while a task that used it still exists. Saying so
/// is better than falling back to another agent, which would run something the
/// task never asked for.
pub fn require_adapter(task: &Task) -> ApiResult<std::sync::Arc<crate::adapters::Adapter>> {
    crate::adapters::adapter(&task.adapter).ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "adapter_unavailable",
            format!(
                "no adapter called `{}` is loaded. Its manifest may have been \
                 removed or failed to load; see /adapters.",
                task.adapter
            ),
        )
    })
}

/// The error for an agent CLI that cannot be used, or `None` when it can.
fn unusable_agent(
    adapter: &crate::adapters::Adapter,
    check: &forge_core::BinaryStatus,
) -> Option<ApiError> {
    if check.ok {
        return None;
    }

    Some(ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "adapter_unavailable",
        check
            .detail
            .clone()
            .unwrap_or_else(|| format!("`{}` is not usable", adapter.binary_name())),
    ))
}

pub async fn stop(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<Session>> {
    load(&state, id)?;

    Ok(Json(state.sessions.stop(id).await?))
}

pub async fn restart(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<(StatusCode, Json<Session>)> {
    let task = load(&state, id)?;
    let project = project_of(&state, &task)?;

    state.sessions.preflight().await?;
    require_agent(&task).await?;

    let session = state
        .sessions
        .restart(&task, &project, &launch_command(&task, &project)?)
        .await?;

    Ok((StatusCode::CREATED, Json(session)))
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<TaskList>> {
    Ok(Json(TaskList {
        tasks: state.store.tasks(TaskQuery {
            project_id: query.project,
            status: query.status,
        })?,
    }))
}

pub async fn get(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<Task>> {
    Ok(Json(load(&state, id)?))
}

/// Git facts for the task's own working tree — its worktree, or the repository
/// root when it has none.
pub async fn git_status(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<GitStatus>> {
    let task = load(&state, id)?;
    let project = project_of(&state, &task)?;

    Ok(Json(
        git::status(Path::new(task.working_dir(&project))).await?,
    ))
}

/// Create the task, then provision its worktree.
///
/// The row has to exist first, because the slug — and so the branch and the
/// directory — is derived from the task's id. If provisioning fails the row is
/// deleted again, so a failed create leaves nothing behind.
pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateRequest>,
) -> ApiResult<(StatusCode, Json<Task>)> {
    if request.title.trim().is_empty() {
        return Err(ApiError::bad_request("a task needs a title"));
    }

    // Answered before anything is created, so a retry whose first answer was
    // lost gets the task it already made rather than a second one.
    if let Some(key) = request.idempotency_key.as_deref() {
        if let Some(existing) = state.store.task_by_key(key)? {
            return Ok((StatusCode::OK, Json(existing)));
        }
    }

    let project = state.store.project(request.project_id)?.ok_or_else(|| {
        ApiError::not_found(format!(
            "there is no project with id {}",
            request.project_id
        ))
    })?;

    let base_branch = request
        .base_branch
        .unwrap_or_else(|| project.default_branch.clone());

    if !git::branch_exists(Path::new(&project.path), &base_branch).await? {
        return Err(ApiError::bad_request(format!(
            "`{base_branch}` is not a branch in {}",
            project.name
        )));
    }

    let task = state.store.create_task(&NewTask {
        idempotency_key: request.idempotency_key.clone(),
        project_id: project.id,
        title: request.title.trim().to_owned(),
        adapter: request.adapter,
        base_branch: base_branch.clone(),
        initial_prompt: request.initial_prompt,
    })?;

    let task = if request.use_worktree {
        match provision(&state, &project, &task, &base_branch).await {
            Ok(task) => task,
            Err(err) => {
                roll_back(&state, &project, &task).await;
                return Err(err);
            }
        }
    } else {
        task
    };

    state.bus.publish(ForgeEvent::TaskCreated {
        task_id: task.id,
        project_id: project.id,
        title: task.title.clone(),
    })?;

    Ok((StatusCode::CREATED, Json(task)))
}

/// Create a task and start its agent, the way `POST /tasks` then
/// `POST /tasks/:id/start` would.
///
/// Shared with the Telegram bot, which has to do both in one command and must
/// not grow a second, subtly different idea of what starting a task means.
/// A task whose agent will not start is deleted again rather than left sitting
/// there: nobody asked for a task, they asked for an agent.
pub async fn create_and_start(
    state: &AppState,
    project_id: i64,
    title: String,
    prompt: String,
    adapter: AdapterId,
    idempotency_key: Option<String>,
) -> ApiResult<Task> {
    let (status, Json(task)) = create(
        State(state.clone()),
        Json(CreateRequest {
            idempotency_key,
            project_id,
            title,
            adapter,
            base_branch: None,
            initial_prompt: Some(prompt),
            use_worktree: true,
        }),
    )
    .await?;

    // The task already existed, so it was already started too. Starting it
    // again would be an error about a session that is not this caller's fault.
    if status == StatusCode::OK {
        return Ok(task);
    }

    match start(State(state.clone()), UrlPath(task.id)).await {
        Ok(_) => Ok(task),
        Err(err) => {
            if let Err(undo) = delete(
                State(state.clone()),
                UrlPath(task.id),
                crate::http::extract::Query(DeleteQuery { force: true }),
            )
            .await
            {
                tracing::warn!(
                    task = task.id,
                    error = undo.message(),
                    "cannot undo a task that would not start"
                );
            }
            Err(err)
        }
    }
}

async fn provision(
    state: &AppState,
    project: &Project,
    task: &Task,
    base: &str,
) -> ApiResult<Task> {
    let placement = placement_for(state, project, task)?;

    worktree::create(project, &placement, base).await?;

    let path = placement.path.display().to_string();
    let attached = state
        .store
        .attach_worktree(task.id, &placement.branch, &path)?;

    state.bus.publish(ForgeEvent::WorktreeCreated {
        task_id: task.id,
        path,
        branch: placement.branch,
    })?;

    Ok(attached)
}

fn placement_for(
    state: &AppState,
    project: &Project,
    task: &Task,
) -> ApiResult<worktree::Placement> {
    let root = worktree::root_for(&state.store, project)?;
    Ok(worktree::placement(&root, project, &task.title, task.id))
}

/// Undo a half-finished create.
///
/// Provisioning can fail after git has already made the worktree — recording
/// it in the database is a separate step — so the directory and branch are
/// removed too, not just the row. Failures here are logged rather than
/// returned: the caller is already receiving the error that caused this.
async fn roll_back(state: &AppState, project: &Project, task: &Task) {
    if let Ok(placement) = placement_for(state, project, task) {
        if placement.path.exists() {
            if let Err(error) = worktree::remove(
                project,
                &placement.path,
                &placement.branch,
                BranchDisposal::Delete,
                true,
            )
            .await
            {
                tracing::error!(task = task.id, %error, "cannot roll back a worktree");
            }
        }
    }

    if let Err(error) = state.store.delete_task(task.id) {
        tracing::error!(task = task.id, %error, "cannot roll back a task row");
    }
}

pub async fn patch(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
    Json(request): Json<PatchRequest>,
) -> ApiResult<Json<Task>> {
    load(&state, id)?;

    if request.title.as_ref().is_some_and(|t| t.trim().is_empty()) {
        return Err(ApiError::bad_request("a task needs a title"));
    }

    // Renaming does not move the worktree: the slug is fixed at creation, and
    // moving a checked-out directory under a running agent is not worth it.
    Ok(Json(state.store.update_task(
        id,
        &TaskPatch {
            title: request.title.map(|t| t.trim().to_owned()),
            initial_prompt: request.initial_prompt,
            status: None,
        },
    )?))
}

pub async fn delete(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
    Query(query): Query<DeleteQuery>,
) -> ApiResult<StatusCode> {
    let task = load(&state, id)?;

    if state.store.has_live_session(id)? {
        return Err(ApiError::conflict(
            "this task still has a running session; stop it first",
        ));
    }

    if let Some(path) = task.worktree_path.clone() {
        if !query.force {
            return Err(ApiError::conflict(
                "this task still has a worktree; clean it up first, or delete with force=true",
            ));
        }
        // The branch survives: deleting a task should not be able to destroy
        // commits that were never merged anywhere.
        let project = project_of(&state, &task)?;
        tear_down(&state, &project, &task, &path, BranchDisposal::Keep, true).await?;
    }

    state.store.delete_task(id)?;
    state.bus.publish(ForgeEvent::TaskDeleted { task_id: id })?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn cleanup_worktree(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
    body: Option<Json<CleanupRequest>>,
) -> ApiResult<Json<Task>> {
    let task = load(&state, id)?;
    let Json(request) = body.unwrap_or_default();

    let Some(path) = task.worktree_path.clone() else {
        return Err(ApiError::conflict("this task has no worktree"));
    };

    if state.store.has_live_session(id)? {
        return Err(ApiError::conflict(
            "this task still has a running session; stop it before removing its worktree",
        ));
    }

    if !request.force && worktree::is_dirty(Path::new(&path)).await? {
        return Err(ApiError::conflict(
            "this worktree has uncommitted changes; commit them, or clean up with force=true",
        ));
    }

    let project = project_of(&state, &task)?;
    let disposal = if request.delete_branch {
        BranchDisposal::Delete
    } else {
        BranchDisposal::Keep
    };

    tear_down(&state, &project, &task, &path, disposal, request.force).await?;

    Ok(Json(
        state
            .store
            .detach_worktree(id, disposal == BranchDisposal::Keep)?,
    ))
}

/// Remove the worktree and announce it. The task row is the caller's problem.
async fn tear_down(
    state: &AppState,
    project: &Project,
    task: &Task,
    path: &str,
    disposal: BranchDisposal,
    force: bool,
) -> ApiResult<()> {
    worktree::remove(project, Path::new(path), &task.branch, disposal, force).await?;

    state.bus.publish(ForgeEvent::WorktreeRemoved {
        task_id: task.id,
        path: path.to_owned(),
    })?;

    Ok(())
}

fn load(state: &AppState, id: i64) -> ApiResult<Task> {
    state
        .store
        .task(id)?
        .ok_or_else(|| ApiError::not_found(format!("there is no task with id {id}")))
}

/// A task always has a project: the foreign key cascades, so a missing one is
/// the daemon's own inconsistency rather than a bad request.
fn project_of(state: &AppState, task: &Task) -> ApiResult<Project> {
    state.store.project(task.project_id)?.ok_or_else(|| {
        tracing::error!(
            task = task.id,
            project = task.project_id,
            "a task outlived its project"
        );
        ApiError::internal("this task's project is missing; check the daemon logs")
    })
}

impl From<SessionManagerError> for ApiError {
    fn from(err: SessionManagerError) -> Self {
        match err {
            SessionManagerError::AlreadyRunning(_) | SessionManagerError::NotRunning(_) => {
                Self::conflict(err.to_string())
            }
            // A runtime that cannot start sessions is a dependency problem the
            // user can fix, and `/health` says which one.
            SessionManagerError::Runtime(detail) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime_unavailable",
                detail,
            ),
            SessionManagerError::Store(store) => store.into(),
        }
    }
}

impl From<TaskError> for ApiError {
    fn from(err: TaskError) -> Self {
        match err {
            TaskError::NotFound(_) | TaskError::NoSuchProject(_) => {
                Self::not_found(err.to_string())
            }
            TaskError::Store(store) => store.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> std::sync::Arc<crate::adapters::Adapter> {
        crate::adapters::adapter(&AdapterId::default()).expect("a built-in")
    }

    #[test]
    fn a_usable_agent_is_no_obstacle() {
        let check = forge_core::BinaryStatus {
            name: "claude".into(),
            path: Some("/usr/local/bin/claude".into()),
            version: Some("2.1.0".into()),
            ok: true,
            detail: None,
        };

        assert!(unusable_agent(&claude(), &check).is_none());
    }

    #[test]
    fn a_missing_agent_is_a_503_that_names_the_cli() {
        let check = forge_core::BinaryStatus::missing("claude");

        let err = unusable_agent(&claude(), &check).unwrap();

        assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(err.message().contains("claude"), "{}", err.message());
    }

    #[test]
    fn a_missing_task_or_project_is_a_404() {
        assert_eq!(
            ApiError::from(TaskError::NotFound(7)).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::from(TaskError::NoSuchProject(7)).status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn a_worktree_is_the_default_for_a_new_task() {
        let request: CreateRequest =
            serde_json::from_str(r#"{"project_id":1,"title":"t","adapter":"claude-code"}"#)
                .unwrap();

        assert!(request.use_worktree);
        assert!(request.base_branch.is_none());
    }

    #[test]
    fn opting_out_of_a_worktree_is_explicit() {
        let request: CreateRequest = serde_json::from_str(
            r#"{"project_id":1,"title":"t","adapter":"codex","use_worktree":false}"#,
        )
        .unwrap();

        assert!(!request.use_worktree);
    }

    #[test]
    fn cleanup_keeps_the_branch_and_refuses_dirt_unless_told_otherwise() {
        let request: CleanupRequest = serde_json::from_str("{}").unwrap();

        assert!(!request.force);
        assert!(!request.delete_branch);
    }
}
