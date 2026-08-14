//! Turning a task title into something safe for a branch name and a directory.

/// Long enough to stay recognisable, short enough to keep paths sane.
const MAX_SLUG_BYTES: usize = 48;

/// A filesystem- and git-safe identifier for a task.
///
/// The id is appended rather than hoped-for: two tasks may share a title, and
/// the slug becomes both a branch name and a directory, neither of which may
/// collide.
pub fn task_slug(title: &str, id: i64) -> String {
    let words = slugify(title);
    if words.is_empty() {
        format!("task-{id}")
    } else {
        format!("{words}-{id}")
    }
}

/// A safe single path component for `name`, falling back to `fallback`.
///
/// Anything a user can type ends up in a path somewhere; nothing that reaches
/// one may contain a separator, a `..`, or be absolute.
pub fn path_component(name: &str, fallback: &str) -> String {
    let slug = slugify(name);
    if slug.is_empty() {
        fallback.to_owned()
    } else {
        slug
    }
}

/// Lowercase ASCII words joined by hyphens. Everything else is a separator.
///
/// Deliberately strict rather than clever: git refuses names with `..`, `~`,
/// `^`, `:`, a leading `-`, a trailing `.lock`, and more, and transliterating
/// is not worth the surprise. A title with nothing usable falls back to the id.
pub fn slugify(title: &str) -> String {
    let mut slug = String::new();

    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.extend(ch.to_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= MAX_SLUG_BYTES {
            break;
        }
    }

    slug.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_component_cannot_contain_a_separator_or_climb() {
        for hostile in [
            "../../etc",
            "/absolute/path",
            "..",
            ".",
            "a/b",
            "C:\\windows",
        ] {
            let component = path_component(hostile, "project-1");
            assert!(!component.contains('/'), "{component}");
            assert!(!component.contains('\\'), "{component}");
            assert_ne!(component, "..");
            assert_ne!(component, ".");
            assert!(!component.starts_with('.'), "{component}");
        }
    }

    #[test]
    fn a_path_component_falls_back_when_nothing_is_usable() {
        assert_eq!(path_component("///", "project-1"), "project-1");
        assert_eq!(path_component("", "project-1"), "project-1");
        assert_eq!(path_component("Forge", "project-1"), "forge");
    }

    #[test]
    fn a_plain_title_becomes_lowercase_words() {
        assert_eq!(task_slug("Add agent adapters", 7), "add-agent-adapters-7");
    }

    #[test]
    fn the_id_makes_identical_titles_distinct() {
        assert_ne!(task_slug("Fix it", 1), task_slug("Fix it", 2));
    }

    #[test]
    fn punctuation_and_spacing_collapse_to_single_hyphens() {
        assert_eq!(task_slug("Fix   the  cart!!!", 3), "fix-the-cart-3");
        assert_eq!(
            task_slug("  leading and trailing  ", 4),
            "leading-and-trailing-4"
        );
    }

    #[test]
    fn characters_git_refuses_never_survive() {
        // `..`, `~`, `^`, `:`, `?`, `*`, `[`, `\`, and a leading `-` are all
        // rejected by git check-ref-format.
        let slug = task_slug("a..b~c^d:e?f*g[h\\i", 5);

        assert_eq!(slug, "a-b-c-d-e-f-g-h-i-5");
        for forbidden in ["..", "~", "^", ":", "?", "*", "[", "\\", " "] {
            assert!(!slug.contains(forbidden), "{slug} contains {forbidden}");
        }
        assert!(!slug.starts_with('-'));
    }

    #[test]
    fn path_separators_cannot_escape_the_worktree_directory() {
        assert_eq!(task_slug("../../etc/passwd", 6), "etc-passwd-6");
        assert!(!task_slug("a/b", 7).contains('/'));
    }

    #[test]
    fn a_title_with_nothing_usable_falls_back_to_the_id() {
        assert_eq!(task_slug("！！！", 8), "task-8");
        assert_eq!(task_slug("", 9), "task-9");
        assert_eq!(task_slug("   ", 10), "task-10");
    }

    #[test]
    fn non_ascii_is_dropped_rather_than_transliterated() {
        assert_eq!(task_slug("Café résumé", 11), "caf-r-sum-11");
    }

    #[test]
    fn a_long_title_is_truncated_but_still_ends_with_the_id() {
        let slug = task_slug(&"very long title ".repeat(20), 12);

        assert!(slug.len() < 70, "{slug}");
        assert!(slug.ends_with("-12"));
        assert!(!slug.contains("--"));
    }

    #[test]
    fn a_slug_never_ends_in_a_hyphen_before_its_id() {
        assert_eq!(
            task_slug("trailing punctuation!", 13),
            "trailing-punctuation-13"
        );
    }
}
