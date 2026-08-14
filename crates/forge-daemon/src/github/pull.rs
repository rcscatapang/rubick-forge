//! What a pull request opened from a task says.
//!
//! Pure: given a task, produce the title and body. Kept apart from the API
//! client so the wording can be tested without a network or a token.

use forge_core::Task;

/// How much of an initial prompt goes into a PR body.
const MAX_PROMPT: usize = 2_000;

/// The default commit message for a task's work.
///
/// The task title is what the human wrote when they described the job, so it is
/// already the sentence they would have typed.
pub fn commit_message(task: &Task) -> String {
    task.title.trim().to_owned()
}

/// The pull request's title.
pub fn title(task: &Task) -> String {
    task.title.trim().to_owned()
}

/// The pull request's body.
///
/// The prompt is quoted rather than presented as the author's own words: it is
/// what the agent was asked to do, and a reviewer should be able to tell the
/// brief from the result.
pub fn body(task: &Task, issue: Option<i64>) -> String {
    let mut body = String::new();

    if let Some(prompt) = task.initial_prompt.as_deref().map(str::trim) {
        if !prompt.is_empty() {
            body.push_str("### What this agent was asked to do\n\n");
            for line in truncate(prompt).lines() {
                body.push_str("> ");
                body.push_str(line);
                body.push('\n');
            }
            body.push('\n');
        }
    }

    if let Some(number) = issue {
        body.push_str(&format!("Closes #{number}\n\n"));
    }

    body.push_str(&format!(
        "---\nOpened by [Rubick Forge](https://github.com/rcscatapang/rubick-forge) \
         from task {} on branch `{}`.\n",
        task.id, task.branch
    ));

    body
}

fn truncate(prompt: &str) -> String {
    if prompt.chars().count() <= MAX_PROMPT {
        return prompt.to_owned();
    }

    let kept: String = prompt.chars().take(MAX_PROMPT).collect();
    format!("{}\n…", kept.trim_end())
}

/// The branch name for a task started from an issue.
///
/// Numbered so the issue it came from is legible in `git branch`, which is
/// where someone looks when they have forgotten what a branch was for.
pub fn issue_branch(number: i64, title: &str) -> String {
    format!("forge/issue-{number}-{}", crate::slug::slugify(title))
}

/// The task title for one started from an issue.
pub fn issue_task_title(number: i64, title: &str) -> String {
    format!("#{number} {}", title.trim())
}

/// The initial prompt for a task started from an issue.
///
/// The issue body is included because it is usually where the actual
/// requirement is; the title alone is a label.
pub fn issue_prompt(number: i64, title: &str, body: Option<&str>) -> String {
    let mut prompt = format!("Work on GitHub issue #{number}: {}\n", title.trim());

    if let Some(body) = body.map(str::trim).filter(|body| !body.is_empty()) {
        prompt.push_str("\nThe issue says:\n\n");
        prompt.push_str(&truncate(body));
        prompt.push('\n');
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{AdapterId, AgentStatus, Timestamp};

    fn task(prompt: Option<&str>) -> Task {
        Task {
            id: 12,
            project_id: 1,
            title: "Fix the flaky test".into(),
            adapter: AdapterId::default(),
            base_branch: "main".into(),
            branch: "forge/fix-the-flaky-test-12".into(),
            worktree_path: None,
            initial_prompt: prompt.map(str::to_owned),
            status: AgentStatus::Idle,
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    #[test]
    fn the_title_is_what_the_human_called_the_job() {
        assert_eq!(title(&task(None)), "Fix the flaky test");
        assert_eq!(commit_message(&task(None)), "Fix the flaky test");
    }

    #[test]
    fn the_body_quotes_the_brief_rather_than_claiming_it() {
        let body = body(&task(Some("make CI stop failing on tuesdays")), None);

        assert!(
            body.contains("> make CI stop failing on tuesdays"),
            "{body}"
        );
        assert!(body.contains("asked to do"), "{body}");
    }

    #[test]
    fn a_multi_line_prompt_is_quoted_on_every_line() {
        let body = body(&task(Some("first line\nsecond line")), None);

        assert!(body.contains("> first line\n> second line"), "{body}");
    }

    #[test]
    fn a_task_with_no_prompt_has_a_body_without_an_empty_quote() {
        let body = body(&task(None), None);

        assert!(!body.contains("asked to do"), "{body}");
        assert!(!body.contains('>'), "{body}");
        assert!(body.contains("task 12"), "{body}");
    }

    #[test]
    fn a_prompt_that_is_only_whitespace_is_no_prompt() {
        assert!(!body(&task(Some("   \n  ")), None).contains("asked to do"));
    }

    #[test]
    fn an_issue_sourced_task_closes_its_issue() {
        let body = body(&task(None), Some(41));

        assert!(body.contains("Closes #41"), "{body}");
    }

    #[test]
    fn a_task_from_nowhere_closes_nothing() {
        assert!(!body(&task(None), None).contains("Closes"));
    }

    #[test]
    fn the_body_says_where_the_work_is() {
        let body = body(&task(None), None);

        assert!(body.contains("forge/fix-the-flaky-test-12"), "{body}");
    }

    #[test]
    fn an_enormous_prompt_is_cut_rather_than_posted_whole() {
        let body = body(&task(Some(&"word ".repeat(1_000))), None);

        assert!(body.chars().count() < 2_400, "{}", body.chars().count());
        assert!(body.contains('…'), "{body}");
    }

    #[test]
    fn an_issue_branch_says_which_issue() {
        assert_eq!(
            issue_branch(41, "Flaky test on Tuesdays"),
            "forge/issue-41-flaky-test-on-tuesdays"
        );
    }

    #[test]
    fn an_issue_task_is_titled_with_its_number() {
        assert_eq!(issue_task_title(41, "Flaky test"), "#41 Flaky test");
    }

    #[test]
    fn an_issue_prompt_carries_the_body_where_the_requirement_usually_is() {
        let prompt = issue_prompt(41, "Flaky test", Some("It fails one run in ten."));

        assert!(prompt.contains("issue #41"), "{prompt}");
        assert!(prompt.contains("It fails one run in ten."), "{prompt}");
    }

    #[test]
    fn an_issue_with_no_body_still_makes_a_prompt() {
        let prompt = issue_prompt(41, "Flaky test", None);

        assert!(prompt.contains("Flaky test"), "{prompt}");
        assert!(!prompt.contains("The issue says"), "{prompt}");
        assert!(!issue_prompt(41, "Flaky test", Some("  ")).contains("The issue says"));
    }
}
