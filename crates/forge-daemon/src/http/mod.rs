//! The HTTP/WS surface. Everything but `/health` needs a bearer token.

mod adapters;
pub mod auth;
pub mod error;
mod events;
pub(crate) mod extract;
mod github;
mod health;
mod projects;
mod settings;
pub mod tasks;
mod terminal;

use std::sync::Arc;
use std::time::Instant;

use axum::routing::get;
use axum::Router;

use crate::bus::Bus;
use crate::config::Config;
use crate::runtime::TmuxRuntime;
use crate::sessions::SessionManager;
use crate::store::Store;
use crate::token::Token;
use error::ApiError;
use health::BinaryCache;

/// The GitHub client built from the stored token, if there is one.
///
/// Re-exported so the poller can hold a long-lived one — which is where the
/// ETag cache that makes polling affordable lives.
pub fn github_client() -> Option<crate::github::api::GitHub> {
    github::stored_client()
}

/// tmux is the only runtime, so the app state names it concretely; the
/// manager itself is written against the trait.
pub type Sessions = SessionManager<TmuxRuntime>;

#[derive(Clone)]
pub struct AppState {
    pub bus: Bus,
    pub store: Store,
    pub sessions: Arc<Sessions>,
    pub token: Arc<Token>,
    pub config: Arc<Config>,
    pub machine: Arc<str>,
    pub started_at: Instant,
    pub version: &'static str,
    binaries: BinaryCache,
}

impl AppState {
    pub fn new(
        bus: Bus,
        store: Store,
        sessions: Arc<Sessions>,
        token: Token,
        config: Arc<Config>,
        version: &'static str,
    ) -> Self {
        Self {
            bus,
            store,
            sessions,
            token: Arc::new(token),
            machine: Arc::from(config.machine_name()),
            config,
            started_at: Instant::now(),
            version,
            binaries: BinaryCache::default(),
        }
    }
}

/// The router. Routes registered before `route_layer` are the authenticated
/// ones; `/health` is added after it on purpose.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/adapters", get(adapters::list))
        .route("/settings", get(settings::list).patch(settings::patch))
        .route("/events", get(events::list))
        .route("/ws/events", get(events::stream))
        .route("/ws/sessions/{id}/terminal", get(terminal::stream))
        .route("/projects", get(projects::list).post(projects::register))
        .route(
            "/projects/{id}",
            get(projects::get)
                .patch(projects::patch)
                .delete(projects::delete),
        )
        .route("/projects/{id}/git", get(projects::git_status))
        .route("/tasks", get(tasks::list).post(tasks::create))
        .route(
            "/tasks/{id}",
            get(tasks::get).patch(tasks::patch).delete(tasks::delete),
        )
        .route("/tasks/{id}/git", get(tasks::git_status))
        .route("/tasks/{id}/sessions", get(tasks::sessions))
        .route("/sessions/{id}", get(tasks::session))
        .route("/tasks/{id}/start", axum::routing::post(tasks::start))
        .route("/tasks/{id}/stop", axum::routing::post(tasks::stop))
        .route("/tasks/{id}/restart", axum::routing::post(tasks::restart))
        .route(
            "/tasks/{id}/instruction",
            axum::routing::post(tasks::instruction),
        )
        .route("/github", get(github::status))
        .route(
            "/github/token",
            axum::routing::put(github::set_token).delete(github::forget_token),
        )
        .route("/github/links", get(github::links))
        .route(
            "/github/tasks",
            axum::routing::post(github::task_from_issue),
        )
        .route("/projects/{id}/github", get(github::project_repo))
        .route("/projects/{id}/github/issues", get(github::project_issues))
        .route("/tasks/{id}/github/diff", get(github::task_diff))
        .route(
            "/tasks/{id}/github/commit",
            axum::routing::post(github::commit),
        )
        .route(
            "/tasks/{id}/github/pull",
            axum::routing::post(github::open_pull),
        )
        .route("/tasks/{id}/answer", axum::routing::post(tasks::answer))
        .route("/tasks/{id}/pane", get(tasks::pane))
        .route(
            "/tasks/{id}/worktree/cleanup",
            axum::routing::post(tasks::cleanup_worktree),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        .route("/health", get(health::health))
        .fallback(|| async { ApiError::not_found("this daemon has no such endpoint") })
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                axum::http::StatusCode::METHOD_NOT_ALLOWED,
                "method_not_allowed",
                "that endpoint does not accept this method",
            )
        })
        .with_state(state)
}
