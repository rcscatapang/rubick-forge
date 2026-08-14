//! Working out which GitHub repository a project is, from its git remote.
//!
//! A project that has no GitHub remote simply has no GitHub features. That is
//! a normal state, not an error: plenty of repositories are local, or live
//! somewhere else entirely.

use std::path::Path;

use crate::exec;
use crate::git;

/// One repository on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    /// `owner/name`, the way GitHub's own API paths spell it.
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    pub fn url(&self) -> String {
        format!("https://github.com/{}", self.slug())
    }
}

/// The GitHub repository a remote URL points at, if it points at one.
///
/// Handles the three spellings git uses in practice — `git@github.com:o/n.git`,
/// `https://github.com/o/n.git`, and `ssh://git@github.com/o/n` — and returns
/// `None` for anything else, including other forges.
pub fn parse(url: &str) -> Option<Repo> {
    let url = url.trim();

    // scp-style: `git@github.com:owner/name.git`
    let path = match url.strip_prefix("git@github.com:") {
        Some(rest) => rest,
        None => {
            // `github.com/owner/name`, whatever scheme or userinfo came first.
            let rest = strip_scheme(url)?;
            let host_and_path = rest.split_once('@').map_or(rest, |(_, after)| after);
            host_and_path.strip_prefix("github.com/")?
        }
    };

    let path = path.trim_end_matches('/').trim_end_matches(".git");
    let (owner, name) = path.split_once('/')?;

    // A trailing path segment means this is not a repository root.
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }

    Some(Repo {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

fn strip_scheme(url: &str) -> Option<&str> {
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = url.strip_prefix(scheme) {
            return Some(rest);
        }
    }
    None
}

/// The GitHub repository a checkout's `origin` points at.
pub async fn detect(repo: &Path) -> Option<Repo> {
    let output = exec::run(
        "git",
        &["remote", "get-url", "origin"],
        Some(repo),
        exec::DEFAULT_TIMEOUT,
    )
    .await
    .ok()?;

    if !output.success() {
        return None;
    }

    parse(output.stdout.trim())
}

/// Push a branch to `origin`, setting its upstream.
///
/// Never forced, and never rewriting: a push that would need either is a
/// refusal the user has to resolve, not something to work around.
pub async fn push(tree: &Path, branch: &str) -> Result<(), git::GitError> {
    let output = exec::run(
        "git",
        &["push", "--set-upstream", "origin", branch],
        Some(tree),
        // A push talks to the network, so it gets longer than a local command.
        std::time::Duration::from_secs(120),
    )
    .await
    .map_err(|source| git::GitError::Failed {
        command: format!("push origin {branch}"),
        detail: source.to_string(),
    })?;

    if output.success() {
        return Ok(());
    }

    // git echoes the remote URL in push errors, and a remote can carry a token
    // (`https://ghp_…@github.com/o/n`). Its complaint is scrubbed before it
    // reaches an HTTP body or a log line.
    Err(git::GitError::Failed {
        command: format!("push origin {branch}"),
        detail: redact(output.first_error_line()),
    })
}

/// Replace userinfo in any URL in `message` with an ellipsis.
fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());

    for word in message.split_inclusive(char::is_whitespace) {
        match word.split_once("://") {
            // `scheme://userinfo@host/…` — the userinfo is the secret half.
            Some((scheme, rest)) if rest.contains('@') => {
                let (_, host) = rest.split_once('@').expect("just checked for one");
                out.push_str(scheme);
                out.push_str("://…@");
                out.push_str(host);
            }
            _ => out.push_str(word),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_in_a_remote_url_does_not_survive_an_error_message() {
        let complaint = "fatal: unable to access \
                         'https://ghp_secret@github.com/o/n.git/': 403";

        let cleaned = redact(complaint);

        assert!(!cleaned.contains("ghp_secret"), "{cleaned}");
        assert!(cleaned.contains("github.com/o/n.git"), "{cleaned}");
    }

    #[test]
    fn an_error_with_no_url_in_it_is_left_alone() {
        let complaint = "! [rejected] main -> main (non-fast-forward)";

        assert_eq!(redact(complaint), complaint);
    }

    #[test]
    fn an_ordinary_remote_url_keeps_its_shape() {
        let complaint = "remote: https://github.com/o/n/pull/new/forge/x";

        assert_eq!(redact(complaint), complaint);
    }

    fn repo(owner: &str, name: &str) -> Option<Repo> {
        Some(Repo {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    #[test]
    fn the_scp_spelling_git_gives_you_by_default() {
        assert_eq!(
            parse("git@github.com:rcscatapang/rubick-forge.git"),
            repo("rcscatapang", "rubick-forge")
        );
        assert_eq!(
            parse("git@github.com:rcscatapang/rubick-forge"),
            repo("rcscatapang", "rubick-forge")
        );
    }

    #[test]
    fn the_https_spelling_the_website_offers() {
        assert_eq!(
            parse("https://github.com/rcscatapang/rubick-forge.git"),
            repo("rcscatapang", "rubick-forge")
        );
        assert_eq!(
            parse("https://github.com/rcscatapang/rubick-forge"),
            repo("rcscatapang", "rubick-forge")
        );
        // A token or username in the URL is not part of the repository.
        assert_eq!(
            parse("https://ghp_secret@github.com/rcscatapang/rubick-forge.git"),
            repo("rcscatapang", "rubick-forge")
        );
    }

    #[test]
    fn the_ssh_url_spelling() {
        assert_eq!(
            parse("ssh://git@github.com/rcscatapang/rubick-forge.git"),
            repo("rcscatapang", "rubick-forge")
        );
    }

    #[test]
    fn a_trailing_slash_is_not_part_of_the_name() {
        assert_eq!(
            parse("https://github.com/rcscatapang/rubick-forge/"),
            repo("rcscatapang", "rubick-forge")
        );
    }

    #[test]
    fn a_repository_somewhere_that_is_not_github_is_not_a_github_repository() {
        assert_eq!(parse("git@gitlab.com:owner/name.git"), None);
        assert_eq!(parse("https://bitbucket.org/owner/name"), None);
        assert_eq!(parse("https://git.example.com/owner/name"), None);
        // A self-hosted Enterprise host is a different API, so not this one.
        assert_eq!(parse("https://github.example.com/owner/name"), None);
    }

    #[test]
    fn a_local_or_unparseable_remote_is_not_one_either() {
        assert_eq!(parse("/srv/git/thing.git"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("https://github.com/"), None);
        assert_eq!(parse("https://github.com/owner"), None);
    }

    #[test]
    fn a_url_pointing_inside_a_repository_is_not_the_repository() {
        assert_eq!(parse("https://github.com/owner/name/tree/main"), None);
    }

    #[test]
    fn a_repo_knows_how_to_name_itself() {
        let repo = Repo {
            owner: "rcscatapang".into(),
            name: "rubick-forge".into(),
        };

        assert_eq!(repo.slug(), "rcscatapang/rubick-forge");
        assert_eq!(repo.url(), "https://github.com/rcscatapang/rubick-forge");
    }
}
