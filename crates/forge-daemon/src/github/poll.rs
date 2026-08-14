//! Watching open pull requests, and saying when something changed.
//!
//! Polling, never webhooks (SPEC D23). The decisions about *what changed* are
//! pure and tested here; the loop that applies them is at the bottom and does
//! nothing else.

use std::sync::Arc;
use std::time::Duration;

use forge_core::{ChecksState, ForgeEvent, TaskGitHub};

use super::api::{Checks, GitHub, GitHubError, PullRequest};
use super::remote;
use crate::http::AppState;

/// How often open pull requests are checked.
pub const INTERVAL: Duration = Duration::from_secs(60);

/// How long to wait when GitHub has nothing left to give.
///
/// A used-up budget resets on the hour, so trying again sooner only spends the
/// request that says so.
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(300);

/// What a poll found that is worth telling anyone about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Merged,
    Closed,
    ChecksPassed,
    ChecksFailed,
    /// Something moved, but nothing anyone needs to hear about.
    Quiet,
}

/// The API client's reading, as the domain type the store and the app share.
fn as_state(checks: Checks) -> ChecksState {
    match checks {
        Checks::None => ChecksState::None,
        Checks::Running => ChecksState::Running,
        Checks::Passed => ChecksState::Passed,
        Checks::Failed => ChecksState::Failed,
    }
}

/// Decide what a fresh reading means, given what was known before.
///
/// Only *transitions* are reported. A pull request that was already merged, or
/// checks that were already red, are not news — polling every minute would
/// otherwise announce the same failure sixty times an hour.
pub fn compare(before: &TaskGitHub, state: &str, checks: ChecksState) -> Change {
    let was = before.pr_state.as_deref();

    if state == "merged" && was != Some("merged") {
        return Change::Merged;
    }
    if state == "closed" && was != Some("closed") {
        return Change::Closed;
    }

    if checks != before.checks {
        return match checks {
            ChecksState::Failed => Change::ChecksFailed,
            ChecksState::Passed => Change::ChecksPassed,
            // Going back to running is movement, not news.
            _ => Change::Quiet,
        };
    }

    Change::Quiet
}

/// GitHub reports a merge as a closed pull request with a merge timestamp.
pub fn state_of(pull: &PullRequest) -> &'static str {
    if pull.merged_at.is_some() {
        "merged"
    } else if pull.state == "closed" {
        "closed"
    } else {
        "open"
    }
}

/// The event a change becomes, if it becomes one.
fn event_for(change: &Change, task_id: i64, number: i64, url: &str) -> Option<ForgeEvent> {
    let url = url.to_owned();

    Some(match change {
        Change::Merged => ForgeEvent::PrMerged {
            task_id,
            number,
            url,
        },
        Change::Closed => ForgeEvent::PrClosed {
            task_id,
            number,
            url,
        },
        Change::ChecksPassed => ForgeEvent::ChecksPassed {
            task_id,
            number,
            url,
        },
        Change::ChecksFailed => ForgeEvent::ChecksFailed {
            task_id,
            number,
            url,
        },
        Change::Quiet => return None,
    })
}

/// Watch every open pull request until the daemon stops.
///
/// One task rather than one per pull request: the rate limit is shared, so the
/// decision to stop polling has to be made in one place.
///
/// The client is built lazily and dropped when GitHub rejects it, so a token
/// saved after start-up begins polling on the next tick and a revoked one
/// stops rather than retrying forever. It is otherwise kept, because the ETag
/// cache inside it is what makes polling affordable.
pub async fn watch(state: AppState) {
    let mut github: Option<Arc<GitHub>> = None;

    loop {
        if github.is_none() {
            github = crate::http::github_client().map(Arc::new);
        }

        let wait = match &github {
            // No token: nothing to poll and nothing to complain about.
            None => INTERVAL,
            Some(client) => match poll_once(&state, client).await {
                Ok(()) => INTERVAL,
                Err(error) if error.is_rate_limit() => {
                    tracing::warn!(%error, "pausing GitHub polling until the rate limit resets");
                    RATE_LIMIT_BACKOFF
                }
                Err(GitHubError::Unauthorised) => {
                    tracing::warn!("GitHub rejected the stored token; will re-read it");
                    github = None;
                    INTERVAL
                }
                Err(error) => {
                    tracing::warn!(%error, "a GitHub poll failed");
                    INTERVAL
                }
            },
        };

        tokio::time::sleep(wait).await;
    }
}

/// One pass over every task with an open pull request.
///
/// A task whose repository or project has gone is skipped rather than failing
/// the pass: one broken link must not stop the others being polled.
async fn poll_once(state: &AppState, github: &GitHub) -> Result<(), GitHubError> {
    if !github.rate_limit().allows_polling() {
        return Err(GitHubError::RateLimited {
            resets_at: github.rate_limit().resets_at,
        });
    }

    let open = match state.store.tasks_with_open_pulls() {
        Ok(open) => open,
        Err(error) => {
            tracing::warn!(%error, "cannot list the pull requests to poll");
            return Ok(());
        }
    };

    for link in open {
        if let Err(error) = poll_task(state, github, &link).await {
            // A rate limit ends the whole pass; anything else is this task's
            // problem and the next task may be fine.
            if error.is_rate_limit() {
                return Err(error);
            }
            tracing::warn!(task = link.task_id, %error, "cannot poll a pull request");
        }
    }

    Ok(())
}

async fn poll_task(
    state: &AppState,
    github: &GitHub,
    link: &TaskGitHub,
) -> Result<(), GitHubError> {
    let Some((task, repo)) = task_and_repo(state, link.task_id).await else {
        return Ok(());
    };

    let Some(pull) = github.pull_for_branch(&repo, &task.branch).await? else {
        // Unchanged since the last poll, which cost no budget.
        if let Err(error) = state.store.touch_task_github(link.task_id) {
            tracing::warn!(task = link.task_id, %error, "cannot record a poll");
        }
        return Ok(());
    };

    // An unchanged check-runs answer keeps what was stored. Treating it as
    // "no checks" would blank the chip and make the next poll read the same
    // result back as fresh and announce it a second time.
    let checks = match github.checks(&repo, &pull.head.sha).await? {
        Some(fresh) => as_state(fresh),
        None => link.checks,
    };

    let pr_state = state_of(&pull);
    let change = compare(link, pr_state, checks);

    if let Err(error) = state.store.set_task_pull(
        link.task_id,
        pull.number,
        &pull.html_url,
        pr_state,
        Some(&pull.head.sha),
    ) {
        tracing::warn!(task = link.task_id, %error, "cannot record a pull request");
        return Ok(());
    }

    if let Err(error) = state.store.set_task_checks(link.task_id, checks) {
        tracing::warn!(task = link.task_id, %error, "cannot record a check state");
    }

    if let Some(event) = event_for(&change, link.task_id, pull.number, &pull.html_url) {
        if let Err(error) = state.bus.publish(event) {
            tracing::warn!(%error, "cannot publish a GitHub event");
        }
    }

    Ok(())
}

/// A task and the repository it belongs to, when both are still there.
async fn task_and_repo(state: &AppState, task_id: i64) -> Option<(forge_core::Task, remote::Repo)> {
    let task = state.store.task(task_id).ok().flatten()?;
    let project = state.store.project(task.project_id).ok().flatten()?;
    let repo = remote::detect(std::path::Path::new(&project.path)).await?;

    Some((task, repo))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(state: Option<&str>, checks: ChecksState) -> TaskGitHub {
        TaskGitHub {
            task_id: 1,
            pr_state: state.map(str::to_owned),
            checks,
            ..TaskGitHub::default()
        }
    }

    #[test]
    fn a_pull_request_that_just_merged_is_news() {
        assert_eq!(
            compare(
                &known(Some("open"), ChecksState::Passed),
                "merged",
                ChecksState::Passed
            ),
            Change::Merged
        );
    }

    #[test]
    fn a_pull_request_that_merged_an_hour_ago_is_not_news_again() {
        // Polling every minute would otherwise announce it sixty times.
        assert_eq!(
            compare(
                &known(Some("merged"), ChecksState::Passed),
                "merged",
                ChecksState::Passed
            ),
            Change::Quiet
        );
    }

    #[test]
    fn closing_without_merging_is_its_own_outcome() {
        assert_eq!(
            compare(
                &known(Some("open"), ChecksState::None),
                "closed",
                ChecksState::None
            ),
            Change::Closed
        );
    }

    #[test]
    fn checks_turning_red_is_news_and_staying_red_is_not() {
        assert_eq!(
            compare(
                &known(Some("open"), ChecksState::Running),
                "open",
                ChecksState::Failed
            ),
            Change::ChecksFailed
        );
        assert_eq!(
            compare(
                &known(Some("open"), ChecksState::Failed),
                "open",
                ChecksState::Failed
            ),
            Change::Quiet
        );
    }

    #[test]
    fn checks_turning_green_is_news_once() {
        assert_eq!(
            compare(
                &known(Some("open"), ChecksState::Running),
                "open",
                ChecksState::Passed
            ),
            Change::ChecksPassed
        );
        assert_eq!(
            compare(
                &known(Some("open"), ChecksState::Passed),
                "open",
                ChecksState::Passed
            ),
            Change::Quiet
        );
    }

    #[test]
    fn a_rerun_starting_is_movement_rather_than_news() {
        assert_eq!(
            compare(
                &known(Some("open"), ChecksState::Passed),
                "open",
                ChecksState::Running
            ),
            Change::Quiet
        );
    }

    #[test]
    fn a_merge_is_reported_even_if_the_checks_changed_in_the_same_poll() {
        // The merge is the bigger fact, and both would be one notification too
        // many for a single minute.
        assert_eq!(
            compare(
                &known(Some("open"), ChecksState::Running),
                "merged",
                ChecksState::Passed
            ),
            Change::Merged
        );
    }

    #[test]
    fn a_pull_request_seen_for_the_first_time_reports_its_checks() {
        assert_eq!(
            compare(&known(None, ChecksState::None), "open", ChecksState::Failed),
            Change::ChecksFailed
        );
    }

    #[test]
    fn github_calls_a_merge_a_closed_pull_request_with_a_timestamp() {
        let merged = PullRequest {
            number: 1,
            html_url: "u".into(),
            state: "closed".into(),
            merged_at: Some("2026-08-14T10:00:00Z".into()),
            draft: false,
            head: super::super::api::Ref {
                name: "forge/x".into(),
                sha: "abc".into(),
            },
        };
        let closed = PullRequest {
            merged_at: None,
            ..merged.clone()
        };
        let open = PullRequest {
            state: "open".into(),
            ..closed.clone()
        };

        assert_eq!(state_of(&merged), "merged");
        assert_eq!(state_of(&closed), "closed");
        assert_eq!(state_of(&open), "open");
    }

    #[test]
    fn only_a_change_worth_hearing_becomes_an_event() {
        assert!(event_for(&Change::Quiet, 1, 2, "u").is_none());
        assert!(matches!(
            event_for(&Change::ChecksFailed, 1, 2, "u"),
            Some(ForgeEvent::ChecksFailed { task_id: 1, .. })
        ));
        assert!(matches!(
            event_for(&Change::Merged, 1, 2, "u"),
            Some(ForgeEvent::PrMerged { number: 2, .. })
        ));
    }
}
