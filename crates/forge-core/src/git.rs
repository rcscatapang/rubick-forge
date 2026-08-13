use serde::{Deserialize, Serialize};

/// What git says about a working tree right now.
///
/// Every field degrades rather than fails: a detached HEAD has no branch, a
/// branch with no upstream has no ahead/behind, and both are normal states the
/// dashboard has to render.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Abbreviated commit id, or `None` in a repository with no commits yet.
    pub head: Option<String>,
    /// Whether there are uncommitted changes, tracked or not.
    pub dirty: bool,
    /// The tracked remote branch, when the current branch has one.
    pub upstream: Option<String>,
    /// Commits on this branch that the upstream lacks.
    pub ahead: Option<u32>,
    /// Commits on the upstream that this branch lacks.
    pub behind: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> GitStatus {
        GitStatus {
            branch: Some("main".into()),
            head: Some("abc1234".into()),
            dirty: false,
            upstream: Some("origin/main".into()),
            ahead: Some(0),
            behind: Some(0),
        }
    }

    #[test]
    fn a_detached_head_serialises_with_a_null_branch() {
        let status = GitStatus {
            branch: None,
            ..status()
        };
        let json = serde_json::to_value(&status).unwrap();

        assert!(json["branch"].is_null());
        assert_eq!(json["head"], "abc1234");
    }
}
