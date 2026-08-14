//! The slice of GitHub's REST API the daemon uses.
//!
//! Polling only (SPEC D23), so the two things that matter most here are not
//! features: conditional requests, so a poll that finds nothing new costs no
//! rate-limit budget, and a rate-limit state the caller can see rather than
//! discover by failing.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use super::remote::Repo;

const BASE: &str = "https://api.github.com";

/// GitHub asks for one, and rejects requests without it.
const USER_AGENT: &str = concat!("rubick-forge/", env!("CARGO_PKG_VERSION"));

const TIMEOUT: Duration = Duration::from_secs(20);

/// A personal access token, kept out of every Debug line and every log.
#[derive(Clone)]
pub struct Pat(String);

impl Pat {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }
}

impl std::fmt::Debug for Pat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pat(…)")
    }
}

/// What GitHub said about how much budget is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct RateLimit {
    pub remaining: Option<u32>,
    /// Unix seconds at which the window resets.
    pub resets_at: Option<i64>,
}

impl RateLimit {
    /// Whether there is budget left to spend on a poll.
    ///
    /// A small reserve is kept back so that a user-initiated action — opening a
    /// PR — is not blocked by background polling having spent everything.
    pub fn allows_polling(&self) -> bool {
        self.remaining.is_none_or(|left| left > POLL_RESERVE)
    }
}

const POLL_RESERVE: u32 = 50;

/// A pull request, as much of one as the dashboard shows.
#[derive(Debug, Clone, Deserialize)]
pub struct PullRequest {
    pub number: i64,
    pub html_url: String,
    pub state: String,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub draft: bool,
    pub head: Ref,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Ref {
    #[serde(rename = "ref")]
    pub name: String,
    pub sha: String,
}

/// One issue. GitHub returns pull requests from the issues endpoint too, and
/// `pull_request` being present is the only way to tell them apart.
#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: i64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

impl Issue {
    pub fn is_pull_request(&self) -> bool {
        self.pull_request.is_some()
    }
}

/// The rolled-up state of a commit's check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Checks {
    /// Nothing has reported yet, or there is no CI.
    None,
    Running,
    Passed,
    Failed,
}

#[derive(Debug, Deserialize)]
struct CheckRuns {
    check_runs: Vec<CheckRun>,
}

#[derive(Debug, Deserialize)]
struct CheckRun {
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
}

/// Roll a commit's check runs into one state.
///
/// A single failure decides the whole thing: a green run alongside a red one is
/// not a passing commit, and showing it as one would be the most misleading
/// chip on the dashboard.
pub fn roll_up(runs: &[(String, Option<String>)]) -> Checks {
    if runs.is_empty() {
        return Checks::None;
    }

    let mut running = false;

    for (status, conclusion) in runs {
        match (status.as_str(), conclusion.as_deref()) {
            (_, Some("failure" | "timed_out" | "startup_failure")) => return Checks::Failed,
            // Cancelled and skipped are not failures and not successes; they
            // do not hold the roll-up open either.
            (_, Some("success" | "neutral" | "skipped" | "cancelled")) => {}
            (_, Some(_)) => {}
            ("completed", None) => {}
            _ => running = true,
        }
    }

    if running {
        Checks::Running
    } else {
        Checks::Passed
    }
}

/// What a conditional GET returned.
enum Conditional<T> {
    Fresh(T),
    /// GitHub said nothing changed, which costs no rate-limit budget.
    Unchanged,
}

pub struct GitHub {
    http: reqwest::Client,
    token: Pat,
    base: String,
    /// The last ETag seen per URL, so a repeat poll can be conditional.
    etags: Mutex<HashMap<String, String>>,
    rate: Mutex<RateLimit>,
}

impl GitHub {
    pub fn new(token: Pat) -> Result<Self, GitHubError> {
        Self::with_base(token, BASE)
    }

    pub fn with_base(token: Pat, base: &str) -> Result<Self, GitHubError> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|err| GitHubError::Transport(err.to_string()))?;

        Ok(Self {
            http,
            token,
            base: base.trim_end_matches('/').to_owned(),
            etags: Mutex::new(HashMap::new()),
            rate: Mutex::new(RateLimit::default()),
        })
    }

    pub fn rate_limit(&self) -> RateLimit {
        *self
            .rate
            .lock()
            .expect("the rate limit lock is never poisoned")
    }

    /// Check a token before it is stored, and say what it can do.
    ///
    /// GitHub reports a classic token's scopes in a response header. A
    /// fine-grained token reports none, which is not the same as having none —
    /// so an empty list is accepted, and the first real call is what proves it.
    pub async fn check_token(&self) -> Result<Vec<String>, GitHubError> {
        let response = self
            .request(reqwest::Method::GET, "user")
            .send()
            .await
            .map_err(|err| GitHubError::Transport(err.to_string()))?;

        self.record_rate(&response);

        let scopes = response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|scope| !scope.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(GitHubError::Unauthorised);
        }
        if !response.status().is_success() {
            return Err(GitHubError::Refused(describe(response).await));
        }

        Ok(scopes)
    }

    /// The pull request for `branch`, in whatever state it is now.
    ///
    /// `state=all` on purpose: a merged or closed one is exactly what the
    /// poller needs to see in order to notice that it merged or closed.
    ///
    /// `Ok(None)` means GitHub said nothing has changed since the last ask.
    pub async fn pull_for_branch(
        &self,
        repo: &Repo,
        branch: &str,
    ) -> Result<Option<PullRequest>, GitHubError> {
        let path = format!(
            "repos/{}/pulls?head={}:{branch}&state=all",
            repo.slug(),
            repo.owner
        );

        match self.conditional::<Vec<PullRequest>>(&path).await? {
            Conditional::Unchanged => Ok(None),
            Conditional::Fresh(pulls) => Ok(pulls.into_iter().next()),
        }
    }

    pub async fn open_pull(
        &self,
        repo: &Repo,
        head: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest, GitHubError> {
        let response = self
            .request(
                reqwest::Method::POST,
                &format!("repos/{}/pulls", repo.slug()),
            )
            .json(&serde_json::json!({
                "title": title,
                "body": body,
                "head": head,
                "base": base,
            }))
            .send()
            .await
            .map_err(|err| GitHubError::Transport(err.to_string()))?;

        self.record_rate(&response);

        if response.status().is_success() {
            return response
                .json()
                .await
                .map_err(|err| GitHubError::Transport(err.to_string()));
        }

        Err(self.classify(response).await)
    }

    /// Open issues, newest first, excluding pull requests.
    pub async fn issues(&self, repo: &Repo) -> Result<Vec<Issue>, GitHubError> {
        let issues: Vec<Issue> = self
            .fresh(&format!(
                "repos/{}/issues?state=open&per_page=50",
                repo.slug()
            ))
            .await?;

        Ok(issues
            .into_iter()
            .filter(|issue| !issue.is_pull_request())
            .collect())
    }

    /// One issue by number.
    ///
    /// Asked for directly rather than found in the list: the list is one page,
    /// and a repository's fifty-first open issue is still an issue.
    pub async fn issue(&self, repo: &Repo, number: i64) -> Result<Issue, GitHubError> {
        let issue: Issue = self
            .fresh(&format!("repos/{}/issues/{number}", repo.slug()))
            .await?;

        // GitHub serves pull requests from the issues endpoint too.
        if issue.is_pull_request() {
            return Err(GitHubError::NotFound);
        }

        Ok(issue)
    }

    /// The rolled-up check state for a commit, or `None` if it has not changed.
    ///
    /// "Unchanged" and "no checks" are deliberately different answers. Folding
    /// a 304 into `Checks::None` would overwrite a stored pass or failure, and
    /// the next poll would read it back as fresh and announce it again.
    pub async fn checks(&self, repo: &Repo, sha: &str) -> Result<Option<Checks>, GitHubError> {
        let path = format!("repos/{}/commits/{sha}/check-runs", repo.slug());

        match self.conditional::<CheckRuns>(&path).await? {
            Conditional::Unchanged => Ok(None),
            Conditional::Fresh(body) => Ok(Some(roll_up(
                &body
                    .check_runs
                    .into_iter()
                    .map(|run| (run.status, run.conclusion))
                    .collect::<Vec<_>>(),
            ))),
        }
    }

    /// A GET that always fetches, for a caller that needs a body every time.
    async fn fresh<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, GitHubError> {
        let response = self
            .request(reqwest::Method::GET, path)
            .send()
            .await
            .map_err(|err| GitHubError::Transport(err.to_string()))?;

        self.record_rate(&response);

        if !response.status().is_success() {
            return Err(self.classify(response).await);
        }

        response
            .json()
            .await
            .map_err(|err| GitHubError::Transport(err.to_string()))
    }

    /// A GET that sends the last ETag, so an unchanged answer is free.
    async fn conditional<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<Conditional<T>, GitHubError> {
        let mut request = self.request(reqwest::Method::GET, path);

        if let Some(etag) = self.etag_for(path) {
            request = request.header("if-none-match", etag);
        }

        let response = request
            .send()
            .await
            .map_err(|err| GitHubError::Transport(err.to_string()))?;

        self.record_rate(&response);

        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(Conditional::Unchanged);
        }
        if !response.status().is_success() {
            return Err(self.classify(response).await);
        }

        if let Some(etag) = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
        {
            self.etags
                .lock()
                .expect("the etag lock is never poisoned")
                .insert(path.to_owned(), etag);
        }

        response
            .json()
            .await
            .map(Conditional::Fresh)
            .map_err(|err| GitHubError::Transport(err.to_string()))
    }

    fn etag_for(&self, path: &str) -> Option<String> {
        self.etags
            .lock()
            .expect("the etag lock is never poisoned")
            .get(path)
            .cloned()
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}/{path}", self.base))
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .bearer_auth(&self.token.0)
    }

    fn record_rate(&self, response: &reqwest::Response) {
        let header = |name: &str| -> Option<&str> {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
        };

        let mut rate = self
            .rate
            .lock()
            .expect("the rate limit lock is never poisoned");

        *rate = RateLimit {
            remaining: header("x-ratelimit-remaining").and_then(|v| v.parse().ok()),
            resets_at: header("x-ratelimit-reset").and_then(|v| v.parse().ok()),
        };
    }

    /// Tell the failures apart that a caller has to act on differently.
    async fn classify(&self, response: reqwest::Response) -> GitHubError {
        let status = response.status();
        let exhausted = self
            .rate_limit()
            .remaining
            .is_some_and(|remaining| remaining == 0);

        match status {
            reqwest::StatusCode::UNAUTHORIZED => GitHubError::Unauthorised,
            reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
                if exhausted =>
            {
                GitHubError::RateLimited {
                    resets_at: self.rate_limit().resets_at,
                }
            }
            reqwest::StatusCode::FORBIDDEN => GitHubError::Forbidden(describe(response).await),
            reqwest::StatusCode::NOT_FOUND => GitHubError::NotFound,
            _ => GitHubError::Refused(describe(response).await),
        }
    }
}

/// GitHub's own sentence about what went wrong, which is usually the useful one.
async fn describe(response: reqwest::Response) -> String {
    let status = response.status();

    response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| body["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("GitHub answered {status}"))
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    #[error("cannot reach GitHub: {0}")]
    Transport(String),
    #[error("GitHub rejected the token. Check it has not expired or been revoked.")]
    Unauthorised,
    #[error("GitHub refused: {0}")]
    Forbidden(String),
    #[error("GitHub has nothing there. Check the repository exists and the token can see it.")]
    NotFound,
    #[error("GitHub's rate limit is used up. Polling will resume when it resets.")]
    RateLimited { resets_at: Option<i64> },
    #[error("GitHub refused: {0}")]
    Refused(String),
}

impl GitHubError {
    /// Whether waiting is the only thing that will help.
    pub fn is_rate_limit(&self) -> bool {
        matches!(self, GitHubError::RateLimited { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(pairs: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        pairs
            .iter()
            .map(|(status, conclusion)| (status.to_string(), conclusion.map(str::to_owned)))
            .collect()
    }

    #[test]
    fn a_token_never_appears_in_debug_output() {
        let token = Pat::new("ghp_thisissecret");

        assert_eq!(format!("{token:?}"), "Pat(…)");
    }

    #[test]
    fn no_checks_at_all_is_not_a_pass() {
        // A repository with no CI must not show green; it has said nothing.
        assert_eq!(roll_up(&[]), Checks::None);
    }

    #[test]
    fn every_run_green_is_a_pass() {
        assert_eq!(
            roll_up(&runs(&[
                ("completed", Some("success")),
                ("completed", Some("success"))
            ])),
            Checks::Passed
        );
    }

    #[test]
    fn one_failure_decides_the_whole_commit() {
        // Green alongside red is not a passing commit, whichever came first.
        assert_eq!(
            roll_up(&runs(&[
                ("completed", Some("success")),
                ("completed", Some("failure"))
            ])),
            Checks::Failed
        );
        assert_eq!(
            roll_up(&runs(&[
                ("completed", Some("failure")),
                ("completed", Some("success"))
            ])),
            Checks::Failed
        );
    }

    #[test]
    fn a_failure_beats_a_run_still_going() {
        assert_eq!(
            roll_up(&runs(&[
                ("in_progress", None),
                ("completed", Some("failure"))
            ])),
            Checks::Failed
        );
    }

    #[test]
    fn anything_still_going_holds_the_answer_open() {
        assert_eq!(
            roll_up(&runs(&[
                ("completed", Some("success")),
                ("in_progress", None)
            ])),
            Checks::Running
        );
        assert_eq!(roll_up(&runs(&[("queued", None)])), Checks::Running);
    }

    #[test]
    fn a_timeout_is_a_failure_and_a_skip_is_not() {
        assert_eq!(
            roll_up(&runs(&[("completed", Some("timed_out"))])),
            Checks::Failed
        );
        assert_eq!(
            roll_up(&runs(&[("completed", Some("skipped"))])),
            Checks::Passed
        );
        assert_eq!(
            roll_up(&runs(&[("completed", Some("cancelled"))])),
            Checks::Passed
        );
    }

    #[test]
    fn polling_stops_before_it_spends_the_last_of_the_budget() {
        // The reserve is what leaves room for someone to actually open a PR.
        assert!(RateLimit {
            remaining: Some(500),
            resets_at: None
        }
        .allows_polling());
        assert!(!RateLimit {
            remaining: Some(10),
            resets_at: None
        }
        .allows_polling());
        assert!(!RateLimit {
            remaining: Some(0),
            resets_at: None
        }
        .allows_polling());
    }

    #[test]
    fn a_budget_nobody_has_reported_yet_is_not_a_reason_to_stop() {
        assert!(RateLimit::default().allows_polling());
    }

    #[test]
    fn an_issues_response_can_tell_a_pull_request_apart() {
        let issue: Issue = serde_json::from_str(
            r#"{"number":1,"title":"a","html_url":"u","pull_request":{"url":"x"}}"#,
        )
        .unwrap();
        let plain: Issue =
            serde_json::from_str(r#"{"number":2,"title":"b","html_url":"u"}"#).unwrap();

        assert!(issue.is_pull_request());
        assert!(!plain.is_pull_request());
    }

    #[test]
    fn only_a_used_up_budget_reads_as_rate_limited() {
        assert!(GitHubError::RateLimited { resets_at: None }.is_rate_limit());
        assert!(!GitHubError::Forbidden("no".into()).is_rate_limit());
        assert!(!GitHubError::Unauthorised.is_rate_limit());
    }
}
