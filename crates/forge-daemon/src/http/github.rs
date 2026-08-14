//! The GitHub half of the API.
//!
//! Every route here answers `github_unconfigured` rather than failing when no
//! token has been stored, so a client can ask without knowing first — and a
//! project with no GitHub remote answers "not a GitHub project" rather than an
//! error, because that is a normal thing for a project to be.

use std::path::Path;

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use forge_core::{Project, Task, TaskGitHub};
use serde::{Deserialize, Serialize};

use super::error::{ApiError, ApiResult};
use super::AppState;
use crate::git;
use crate::github::api::{GitHub, GitHubError, Pat, RateLimit};
use crate::github::{pull, remote};

/// The keychain account holding the GitHub personal access token.
const PAT_ACCOUNT: &str = "github-pat";

/// The scope a classic token needs in order to open a pull request.
const REQUIRED_SCOPE: &str = "repo";

/// The stored token, as a client.
///
/// Built per request rather than held: it is cheap, and a token replaced
/// through the API then takes effect without a restart. The polling loop keeps
/// its own long-lived client, which is where the ETag cache lives.
pub fn client() -> Result<GitHub, ApiError> {
    let token = crate::secret(PAT_ACCOUNT).map_err(|_| {
        ApiError::new(
            StatusCode::PRECONDITION_FAILED,
            "github_unconfigured",
            "No GitHub token has been saved. Add one in settings.",
        )
    })?;

    GitHub::new(Pat::new(token)).map_err(from_github)
}

/// The stored token, or `None` when there is none.
pub fn stored_client() -> Option<GitHub> {
    let token = crate::secret(PAT_ACCOUNT).ok()?;

    GitHub::new(Pat::new(token)).ok()
}

fn from_github(error: GitHubError) -> ApiError {
    let (status, code) = match &error {
        GitHubError::Unauthorised => (StatusCode::PRECONDITION_FAILED, "github_unauthorised"),
        GitHubError::NotFound => (StatusCode::NOT_FOUND, "github_not_found"),
        GitHubError::RateLimited { .. } => (StatusCode::TOO_MANY_REQUESTS, "github_rate_limited"),
        GitHubError::Forbidden(_) => (StatusCode::FORBIDDEN, "github_forbidden"),
        GitHubError::Transport(_) => (StatusCode::BAD_GATEWAY, "github_unreachable"),
        GitHubError::Refused(_) => (StatusCode::BAD_GATEWAY, "github_refused"),
    };

    ApiError::new(status, code, error.to_string())
}

#[derive(Debug, Serialize)]
pub struct GitHubStatus {
    /// Whether a token is stored at all.
    pub configured: bool,
    pub rate_limit: RateLimit,
}

/// Whether GitHub is usable, without spending a request to find out.
pub async fn status(State(_): State<AppState>) -> ApiResult<Json<GitHubStatus>> {
    Ok(Json(GitHubStatus {
        configured: stored_client().is_some(),
        rate_limit: RateLimit::default(),
    }))
}

/// Deliberately not `Debug`: the whole point of [`Pat`] is that a token is not
/// printable, and a request body that carries one must not be either.
#[derive(Deserialize)]
pub struct TokenRequest {
    pub token: String,
}

/// Store a personal access token, after checking GitHub accepts it.
///
/// Checked before it is saved, and the scope named when it is missing: a token
/// that cannot open a pull request should fail here rather than at the moment
/// someone tries to open one.
pub async fn set_token(
    State(_): State<AppState>,
    Json(request): Json<TokenRequest>,
) -> ApiResult<StatusCode> {
    let token = request.token.trim();
    if token.is_empty() {
        return Err(ApiError::bad_request("a token cannot be empty"));
    }

    let github = GitHub::new(Pat::new(token)).map_err(from_github)?;
    let scopes = github.check_token().await.map_err(from_github)?;

    // A fine-grained token reports no scopes at all, which is not the same as
    // reporting an insufficient one. Only a classic token's explicit list can
    // be judged here; a fine-grained one is proved by its first real call.
    if !scopes.is_empty() && !scopes.iter().any(|scope| scope == REQUIRED_SCOPE) {
        return Err(ApiError::bad_request(format!(
            "that token has {}, but opening a pull request needs `{REQUIRED_SCOPE}`",
            scopes.join(", ")
        )));
    }

    keyring::Entry::new(crate::KEYCHAIN_SERVICE, PAT_ACCOUNT)
        .and_then(|entry| entry.set_password(token))
        .map_err(|err| ApiError::internal(format!("cannot save the token: {err}")))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn forget_token(State(_): State<AppState>) -> ApiResult<StatusCode> {
    if let Ok(entry) = keyring::Entry::new(crate::KEYCHAIN_SERVICE, PAT_ACCOUNT) {
        // Deleting one that was never there is not a failure.
        let _ = entry.delete_credential();
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct RepoResponse {
    /// `null` for a project that is not on GitHub, which is not an error.
    pub repo: Option<String>,
    pub url: Option<String>,
}

/// Which GitHub repository a project is, if it is one.
pub async fn project_repo(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<RepoResponse>> {
    let project = load_project(&state, id)?;

    Ok(Json(match remote::detect(Path::new(&project.path)).await {
        Some(repo) => RepoResponse {
            repo: Some(repo.slug()),
            url: Some(repo.url()),
        },
        None => RepoResponse {
            repo: None,
            url: None,
        },
    }))
}

/// Open issues on a project's repository.
pub async fn project_issues(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<IssueList>> {
    let project = load_project(&state, id)?;
    let repo = require_repo(&project).await?;

    let issues = client()?.issues(&repo).await.map_err(from_github)?;

    Ok(Json(IssueList {
        issues: issues
            .into_iter()
            .map(|issue| IssueSummary {
                number: issue.number,
                title: issue.title,
                url: issue.html_url,
            })
            .collect(),
    }))
}

#[derive(Debug, Serialize)]
pub struct IssueList {
    pub issues: Vec<IssueSummary>,
}

#[derive(Debug, Serialize)]
pub struct IssueSummary {
    pub number: i64,
    pub title: String,
    pub url: String,
}

/// What committing a task's worktree would commit.
pub async fn task_diff(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<git::DiffStat>> {
    let task = load_task(&state, id)?;
    let tree = require_worktree(&task)?;

    Ok(Json(git::diff_stat(Path::new(&tree)).await?))
}

#[derive(Debug, Deserialize)]
pub struct CommitRequest {
    /// Defaults to the task's title.
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CommitResponse {
    pub sha: String,
}

/// Stage everything in a task's worktree and commit it.
pub async fn commit(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
    Json(request): Json<CommitRequest>,
) -> ApiResult<Json<CommitResponse>> {
    let task = load_task(&state, id)?;
    let tree = require_worktree(&task)?;
    require_own_branch(&task, &tree).await?;

    let message = request
        .message
        .map(|message| message.trim().to_owned())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| pull::commit_message(&task));

    let stat = git::diff_stat(Path::new(&tree)).await?;
    if stat.is_empty() {
        return Err(ApiError::bad_request(
            "there is nothing to commit in this worktree",
        ));
    }

    Ok(Json(CommitResponse {
        sha: git::commit_all(Path::new(&tree), &message).await?,
    }))
}

#[derive(Debug, Serialize)]
pub struct PullResponse {
    pub number: i64,
    pub url: String,
}

/// Push a task's branch and open a pull request for it.
///
/// One route rather than two: pushing without opening leaves a branch nobody
/// asked for, and opening without pushing cannot work.
pub async fn open_pull(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<PullResponse>> {
    let task = load_task(&state, id)?;
    let tree = require_worktree(&task)?;
    require_own_branch(&task, &tree).await?;

    let project = load_project(&state, task.project_id)?;
    let repo = require_repo(&project).await?;

    remote::push(Path::new(&tree), &task.branch).await?;

    let link = state.store.task_github(task.id)?;
    let github = client()?;

    let opened = github
        .open_pull(
            &repo,
            &task.branch,
            &task.base_branch,
            &pull::title(&task),
            &pull::body(&task, link.and_then(|link| link.issue_number)),
        )
        .await
        .map_err(from_github)?;

    state.store.set_task_pull(
        task.id,
        opened.number,
        &opened.html_url,
        "open",
        Some(&opened.head.sha),
    )?;

    state.bus.publish(forge_core::ForgeEvent::PrOpened {
        task_id: task.id,
        number: opened.number,
        url: opened.html_url.clone(),
    })?;

    Ok(Json(PullResponse {
        number: opened.number,
        url: opened.html_url,
    }))
}

#[derive(Debug, Deserialize)]
pub struct FromIssueRequest {
    pub project_id: i64,
    pub number: i64,
}

/// Create a task from an issue, and start its agent.
pub async fn task_from_issue(
    State(state): State<AppState>,
    Json(request): Json<FromIssueRequest>,
) -> ApiResult<(StatusCode, Json<Task>)> {
    let project = load_project(&state, request.project_id)?;
    let repo = require_repo(&project).await?;

    let issue = client()?
        .issue(&repo, request.number)
        .await
        .map_err(from_github)?;

    let task = super::tasks::create_and_start(
        &state,
        project.id,
        pull::issue_task_title(issue.number, &issue.title),
        pull::issue_prompt(issue.number, &issue.title, issue.body.as_deref()),
        forge_core::AdapterId::default(),
        // Started by hand from a browser; nobody is retrying this.
        None,
    )
    .await?;

    // Recorded after the task exists, so the `Closes #n` in its pull request
    // body has something to point at.
    state.store.set_task_issue(task.id, issue.number)?;

    Ok((StatusCode::CREATED, Json(task)))
}

/// Every task's GitHub link, for a dashboard that shows them together.
pub async fn links(State(state): State<AppState>) -> ApiResult<Json<LinkList>> {
    Ok(Json(LinkList {
        links: state.store.all_task_github()?,
    }))
}

#[derive(Debug, Serialize)]
pub struct LinkList {
    pub links: Vec<TaskGitHub>,
}

fn load_project(state: &AppState, id: i64) -> ApiResult<Project> {
    state
        .store
        .project(id)?
        .ok_or_else(|| ApiError::not_found(format!("there is no project with id {id}")))
}

fn load_task(state: &AppState, id: i64) -> ApiResult<Task> {
    state
        .store
        .task(id)?
        .ok_or_else(|| ApiError::not_found(format!("there is no task with id {id}")))
}

fn require_worktree(task: &Task) -> ApiResult<String> {
    task.worktree_path.clone().ok_or_else(|| {
        ApiError::bad_request(
            "this task runs in the repository root, so Forge will not commit for it",
        )
    })
}

/// Refuse to act unless the worktree is still on the task's own branch.
///
/// An agent can `git checkout` inside its worktree. Without this, committing
/// would land on whatever it moved to and the push would fast-forward that
/// branch on the remote — which for a checkout of `main` means Forge pushing
/// straight to the base branch nobody asked it to touch.
async fn require_own_branch(task: &Task, tree: &str) -> ApiResult<()> {
    let status = git::status(Path::new(tree)).await?;

    match status.branch.as_deref() {
        Some(branch) if branch == task.branch => Ok(()),
        Some(branch) => Err(ApiError::bad_request(format!(
            "this worktree is on `{branch}`, not the task's `{}`. \
             Check the task's branch back out first.",
            task.branch
        ))),
        None => Err(ApiError::bad_request(
            "this worktree has a detached HEAD, so there is no branch to push",
        )),
    }
}

async fn require_repo(project: &Project) -> ApiResult<remote::Repo> {
    remote::detect(Path::new(&project.path))
        .await
        .ok_or_else(|| ApiError::bad_request(format!("{} has no GitHub remote", project.name)))
}
