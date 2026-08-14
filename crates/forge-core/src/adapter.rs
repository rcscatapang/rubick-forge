use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The id of an agent CLI Forge can drive.
///
/// Open rather than an enum: adapters are declared by TOML manifests, and a
/// third-party CLI is a file rather than a variant. The two built-ins are
/// manifests too — the same code path, so there is nothing for them to be
/// special about.
///
/// Backed by `Arc<str>` because ids are cloned constantly and never mutated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AdapterId(Arc<str>);

/// The two Forge ships with. Not privileged — just the ones always present.
pub const CLAUDE_CODE: &str = "claude-code";
pub const CODEX: &str = "codex";

impl AdapterId {
    /// The wire form, identical to the serde representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The id Forge uses when a caller does not say which agent.
    pub fn default_id() -> Self {
        Self(Arc::from(CLAUDE_CODE))
    }

    /// Whether `candidate` is usable as an id.
    ///
    /// Lowercase, digits and dashes only. An id reaches a filename, a database
    /// column and a URL path, so it is kept to what all three agree on rather
    /// than escaped differently in each.
    pub fn is_valid(candidate: &str) -> bool {
        !candidate.is_empty()
            && candidate.len() <= 64
            && candidate.starts_with(|c: char| c.is_ascii_lowercase())
            && candidate
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    }
}

impl Default for AdapterId {
    fn default() -> Self {
        Self::default_id()
    }
}

impl fmt::Display for AdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Returned when a string cannot be an [`AdapterId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownAdapterId(pub String);

impl fmt::Display for UnknownAdapterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a usable adapter id: use lowercase letters, digits and dashes",
            self.0
        )
    }
}

impl std::error::Error for UnknownAdapterId {}

impl FromStr for AdapterId {
    type Err = UnknownAdapterId;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if Self::is_valid(s) {
            Ok(Self(Arc::from(s)))
        } else {
            Err(UnknownAdapterId(s.to_owned()))
        }
    }
}

impl TryFrom<String> for AdapterId {
    type Error = UnknownAdapterId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<AdapterId> for String {
    fn from(id: AdapterId) -> Self {
        id.0.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_round_trips_through_json() {
        let id: AdapterId = CLAUDE_CODE.parse().unwrap();

        assert_eq!(serde_json::to_string(&id).unwrap(), "\"claude-code\"");
        assert_eq!(
            serde_json::from_str::<AdapterId>("\"claude-code\"").unwrap(),
            id
        );
    }

    #[test]
    fn an_id_forge_has_never_heard_of_is_still_an_id() {
        // The whole point: a third CLI is a manifest, not a code change.
        let id: AdapterId = "aider".parse().unwrap();

        assert_eq!(id.as_str(), "aider");
        assert_eq!(serde_json::from_str::<AdapterId>("\"aider\"").unwrap(), id);
    }

    #[test]
    fn an_id_reaches_a_filename_a_column_and_a_url_so_it_is_kept_plain() {
        assert!(AdapterId::is_valid("claude-code"));
        assert!(AdapterId::is_valid("agent2"));

        assert!(!AdapterId::is_valid(""));
        assert!(!AdapterId::is_valid("Claude"), "no capitals");
        assert!(!AdapterId::is_valid("2fast"), "must start with a letter");
        assert!(!AdapterId::is_valid("../etc/passwd"), "no path separators");
        assert!(!AdapterId::is_valid("with space"));
        assert!(!AdapterId::is_valid("under_score"));
        assert!(!AdapterId::is_valid(&"x".repeat(65)));
    }

    #[test]
    fn an_unusable_id_is_refused_rather_than_sanitised() {
        assert!("../evil".parse::<AdapterId>().is_err());
        assert!(serde_json::from_str::<AdapterId>("\"../evil\"").is_err());
    }

    #[test]
    fn the_default_is_the_one_most_people_have() {
        assert_eq!(AdapterId::default().as_str(), CLAUDE_CODE);
    }
}
