//! The GitHub client against a GitHub that is not GitHub.
//!
//! A stub API on a loopback port, so conditional requests, rate-limit accounting
//! and error classification can be driven without a token or a repository.

use std::sync::{Arc, Mutex};

use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use forge_daemon::github::api::{Checks, GitHub, GitHubError, Pat};
use forge_daemon::github::remote::Repo;
use serde_json::{json, Value};

/// What the stub will answer with, and what it was asked.
#[derive(Default)]
struct Fake {
    /// Bodies by path suffix, e.g. `pulls` or `check-runs`.
    bodies: Vec<(String, Value)>,
    /// The ETag handed out, and required for a 304 next time.
    etag: Option<String>,
    /// Requests seen, as (path, if-none-match).
    seen: Vec<(String, Option<String>)>,
    /// Sent as `x-ratelimit-remaining` when set.
    remaining: Option<u32>,
    /// Answered instead of a body when set.
    status: Option<u16>,
}

type Shared = Arc<Mutex<Fake>>;

async fn any(
    State(shared): State<Shared>,
    Path(path): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let mut fake = shared.lock().unwrap();

    // Recorded with its query string, since that is where `state=all` lives.
    let path = match query {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };

    let conditional = headers
        .get("if-none-match")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    fake.seen.push((path.clone(), conditional.clone()));

    let mut response_headers = HeaderMap::new();
    if let Some(remaining) = fake.remaining {
        response_headers.insert(
            "x-ratelimit-remaining",
            remaining.to_string().parse().unwrap(),
        );
        response_headers.insert("x-ratelimit-reset", "1760000000".parse().unwrap());
    }

    if let Some(status) = fake.status {
        return (
            StatusCode::from_u16(status).unwrap(),
            response_headers,
            Json(json!({ "message": "the stub refused" })),
        )
            .into_response();
    }

    // A repeat ask with the matching ETag is answered "nothing changed".
    if let (Some(etag), Some(sent)) = (&fake.etag, &conditional) {
        if etag == sent {
            return (StatusCode::NOT_MODIFIED, response_headers).into_response();
        }
    }

    if let Some(etag) = &fake.etag {
        response_headers.insert("etag", etag.parse().unwrap());
    }

    let body = fake
        .bodies
        .iter()
        .find(|(suffix, _)| path.split('?').next().is_some_and(|p| p.ends_with(suffix)))
        .map(|(_, body)| body.clone())
        .unwrap_or_else(|| json!([]));

    (StatusCode::OK, response_headers, Json(body)).into_response()
}

async fn stub() -> (String, Shared) {
    let shared: Shared = Arc::default();

    let app = Router::new()
        .route("/{*path}", get(any).post(any))
        .with_state(Arc::clone(&shared));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{addr}"), shared)
}

fn client(base: &str) -> GitHub {
    GitHub::with_base(Pat::new("stub-token"), base).unwrap()
}

fn repo() -> Repo {
    Repo {
        owner: "o".into(),
        name: "n".into(),
    }
}

fn check_runs(conclusion: &str) -> Value {
    json!({ "check_runs": [{ "status": "completed", "conclusion": conclusion }] })
}

#[tokio::test]
async fn a_second_ask_sends_the_etag_it_was_given() {
    let (base, shared) = stub().await;
    {
        let mut fake = shared.lock().unwrap();
        fake.etag = Some("\"abc\"".into());
        fake.bodies = vec![("check-runs".into(), check_runs("success"))];
    }
    let github = client(&base);

    assert_eq!(
        github.checks(&repo(), "sha").await.unwrap(),
        Some(Checks::Passed)
    );
    // Unchanged, and free.
    assert_eq!(github.checks(&repo(), "sha").await.unwrap(), None);

    let seen = shared.lock().unwrap().seen.clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].1, None, "the first ask has no etag to send");
    assert_eq!(seen[1].1.as_deref(), Some("\"abc\""));
}

#[tokio::test]
async fn an_unchanged_answer_is_not_mistaken_for_having_no_checks() {
    // The bug this guards: folding a 304 into `Checks::None` blanked a stored
    // green and made the next poll announce it all over again.
    let (base, shared) = stub().await;
    {
        let mut fake = shared.lock().unwrap();
        fake.etag = Some("\"abc\"".into());
        fake.bodies = vec![("check-runs".into(), check_runs("failure"))];
    }
    let github = client(&base);

    assert_eq!(
        github.checks(&repo(), "sha").await.unwrap(),
        Some(Checks::Failed)
    );
    assert_eq!(
        github.checks(&repo(), "sha").await.unwrap(),
        None,
        "unchanged is its own answer, not `no checks`"
    );
}

#[tokio::test]
async fn a_repository_with_no_ci_reports_no_checks() {
    let (base, shared) = stub().await;
    shared.lock().unwrap().bodies = vec![("check-runs".into(), json!({ "check_runs": [] }))];

    assert_eq!(
        client(&base).checks(&repo(), "sha").await.unwrap(),
        Some(Checks::None)
    );
}

#[tokio::test]
async fn the_rate_limit_is_read_off_every_answer() {
    let (base, shared) = stub().await;
    {
        let mut fake = shared.lock().unwrap();
        fake.remaining = Some(4_900);
        fake.bodies = vec![("check-runs".into(), check_runs("success"))];
    }
    let github = client(&base);

    assert!(github.rate_limit().remaining.is_none(), "nothing asked yet");
    github.checks(&repo(), "sha").await.unwrap();

    assert_eq!(github.rate_limit().remaining, Some(4_900));
    assert!(github.rate_limit().allows_polling());
}

#[tokio::test]
async fn a_nearly_spent_budget_stops_polling_before_it_runs_out() {
    let (base, shared) = stub().await;
    {
        let mut fake = shared.lock().unwrap();
        fake.remaining = Some(3);
        fake.bodies = vec![("check-runs".into(), check_runs("success"))];
    }
    let github = client(&base);
    github.checks(&repo(), "sha").await.unwrap();

    // The reserve is what leaves room for someone to open a pull request.
    assert!(!github.rate_limit().allows_polling());
}

#[tokio::test]
async fn a_spent_budget_reads_as_rate_limited_rather_than_forbidden() {
    let (base, shared) = stub().await;
    {
        let mut fake = shared.lock().unwrap();
        fake.remaining = Some(0);
        fake.status = Some(403);
    }

    let error = client(&base).checks(&repo(), "sha").await.unwrap_err();

    assert!(error.is_rate_limit(), "{error}");
}

#[tokio::test]
async fn a_forbidden_answer_with_budget_left_is_just_forbidden() {
    let (base, shared) = stub().await;
    {
        let mut fake = shared.lock().unwrap();
        fake.remaining = Some(4_000);
        fake.status = Some(403);
    }

    let error = client(&base).checks(&repo(), "sha").await.unwrap_err();

    assert!(!error.is_rate_limit(), "{error}");
    assert!(matches!(error, GitHubError::Forbidden(_)), "{error}");
}

#[tokio::test]
async fn a_rejected_token_says_so_rather_than_something_generic() {
    let (base, shared) = stub().await;
    shared.lock().unwrap().status = Some(401);

    let error = client(&base).checks(&repo(), "sha").await.unwrap_err();

    assert!(matches!(error, GitHubError::Unauthorised), "{error}");
}

#[tokio::test]
async fn a_missing_repository_is_told_apart_from_a_refusal() {
    let (base, shared) = stub().await;
    shared.lock().unwrap().status = Some(404);

    let error = client(&base).checks(&repo(), "sha").await.unwrap_err();

    assert!(matches!(error, GitHubError::NotFound), "{error}");
}

#[tokio::test]
async fn the_issue_list_leaves_out_the_pull_requests_github_puts_in_it() {
    let (base, shared) = stub().await;
    shared.lock().unwrap().bodies = vec![(
        "issues".into(),
        json!([
            { "number": 1, "title": "a real issue", "html_url": "u" },
            { "number": 2, "title": "a pull request", "html_url": "u",
              "pull_request": { "url": "x" } },
        ]),
    )];

    let issues = client(&base).issues(&repo()).await.unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].number, 1);
}

#[tokio::test]
async fn the_issue_list_is_asked_fresh_every_time() {
    // A 304 here would render an empty browser rather than the cached list.
    let (base, shared) = stub().await;
    {
        let mut fake = shared.lock().unwrap();
        fake.etag = Some("\"abc\"".into());
        fake.bodies = vec![(
            "issues".into(),
            json!([{ "number": 1, "title": "a", "html_url": "u" }]),
        )];
    }
    let github = client(&base);

    assert_eq!(github.issues(&repo()).await.unwrap().len(), 1);
    assert_eq!(github.issues(&repo()).await.unwrap().len(), 1);

    let seen = shared.lock().unwrap().seen.clone();
    assert!(seen.iter().all(|(_, etag)| etag.is_none()), "{seen:?}");
}

#[tokio::test]
async fn one_issue_is_asked_for_by_number_rather_than_found_in_a_page() {
    // A repository's fifty-first open issue is still an issue.
    let (base, shared) = stub().await;
    shared.lock().unwrap().bodies = vec![(
        "issues/51".into(),
        json!({ "number": 51, "title": "the fifty-first", "html_url": "u" }),
    )];

    let issue = client(&base).issue(&repo(), 51).await.unwrap();

    assert_eq!(issue.number, 51);
    assert!(shared
        .lock()
        .unwrap()
        .seen
        .iter()
        .any(|(path, _)| path.ends_with("issues/51")));
}

#[tokio::test]
async fn asking_for_an_issue_that_is_really_a_pull_request_finds_nothing() {
    let (base, shared) = stub().await;
    shared.lock().unwrap().bodies = vec![(
        "issues/7".into(),
        json!({ "number": 7, "title": "a pull request", "html_url": "u",
                "pull_request": { "url": "x" } }),
    )];

    let error = client(&base).issue(&repo(), 7).await.unwrap_err();

    assert!(matches!(error, GitHubError::NotFound), "{error}");
}

#[tokio::test]
async fn a_pull_request_is_asked_for_in_every_state_so_a_merge_is_visible() {
    let (base, shared) = stub().await;
    shared.lock().unwrap().bodies = vec![(
        "pulls".into(),
        json!([{
            "number": 41, "html_url": "u", "state": "closed",
            "merged_at": "2026-08-14T10:00:00Z",
            "head": { "ref": "forge/x", "sha": "abc" },
        }]),
    )];

    let pull = client(&base)
        .pull_for_branch(&repo(), "forge/x")
        .await
        .unwrap()
        .expect("a pull request");

    assert_eq!(pull.number, 41);
    assert!(pull.merged_at.is_some());
    assert!(shared
        .lock()
        .unwrap()
        .seen
        .iter()
        .any(|(path, _)| path.contains("state=all")));
}
