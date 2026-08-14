//! What an agent's terminal output says about what it is doing.
//!
//! Matching is substring-based rather than regular expressions: the markers are
//! literal fragments of a CLI's own interface, they change when that interface
//! changes, and a plain list of strings is something a person can read and
//! correct without reasoning about escaping.

use forge_core::AgentStatus;

/// How much of the pane's tail is examined.
///
/// The states worth detecting are all at the bottom of the screen — a prompt,
/// a spinner, a confirmation — and the further up the pane you look, the more
/// likely you are to match something the agent merely *printed* about a state
/// rather than being in it.
pub const TAIL_LINES: usize = 30;

/// The literal fragments that identify one state.
///
/// Owned rather than `&'static`: markers come from a TOML manifest read at
/// start-up, not from this file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerSet {
    pub status: AgentStatus,
    pub markers: Vec<String>,
}

impl MarkerSet {
    fn matches(&self, tail: &str) -> bool {
        self.markers.iter().any(|marker| tail.contains(marker))
    }
}

/// One adapter's markers, in the order they are tried.
///
/// Order is the whole design: `waiting` is checked before `working` because a
/// CLI usually keeps its spinner or status line on screen while it asks a
/// question, and being wrong about `waiting` is the expensive mistake — that
/// is the state a human has to be told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPatterns {
    pub sets: Vec<MarkerSet>,
}

impl StatusPatterns {
    /// The first status whose markers appear in the tail of `pane`.
    pub fn classify(&self, pane: &str) -> Option<AgentStatus> {
        let tail = tail_of(pane, TAIL_LINES);
        self.sets
            .iter()
            .find(|set| set.matches(&tail))
            .map(|set| set.status)
    }

    /// Every status this adapter can recognise, for documentation and tests.
    pub fn statuses(&self) -> impl Iterator<Item = AgentStatus> + '_ {
        self.sets.iter().map(|set| set.status)
    }
}

/// The last `lines` lines that have anything on them.
///
/// Trailing blanks are dropped first. A terminal pane is a fixed rectangle, so
/// output that stops halfway leaves the rest blank — counting those as "the
/// tail" would look past everything the agent actually said.
pub fn tail_of(pane: &str, lines: usize) -> String {
    let mut all: Vec<&str> = pane.lines().map(str::trim_end).collect();
    while all.last().is_some_and(|line| line.is_empty()) {
        all.pop();
    }

    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patterns() -> StatusPatterns {
        StatusPatterns {
            sets: vec![
                MarkerSet {
                    status: AgentStatus::Waiting,
                    markers: vec!["Do you want to".into(), "(y/n)".into()],
                },
                MarkerSet {
                    status: AgentStatus::Working,
                    markers: vec!["Thinking…".into()],
                },
                MarkerSet {
                    status: AgentStatus::Idle,
                    markers: vec!["❯ ".into()],
                },
            ],
        }
    }

    #[test]
    fn the_first_matching_set_wins() {
        // Both a question and a prompt on screen: the question is what matters.
        let pane = "❯ \nDo you want to proceed?";

        assert_eq!(patterns().classify(pane), Some(AgentStatus::Waiting));
    }

    #[test]
    fn any_marker_in_a_set_is_enough() {
        assert_eq!(
            patterns().classify("Continue? (y/n)"),
            Some(AgentStatus::Waiting)
        );
        assert_eq!(
            patterns().classify("Do you want to edit?"),
            Some(AgentStatus::Waiting)
        );
    }

    #[test]
    fn nothing_recognised_is_not_a_guess() {
        assert_eq!(patterns().classify("some unrelated output"), None);
        assert_eq!(patterns().classify(""), None);
    }

    #[test]
    fn only_the_tail_is_examined() {
        let mut pane = String::from("Do you want to proceed?\n");
        // Push the question far above the visible tail.
        for index in 0..TAIL_LINES + 10 {
            pane.push_str(&format!("line {index}\n"));
        }

        assert_eq!(
            patterns().classify(&pane),
            None,
            "a question scrolled away is not a question being asked"
        );
    }

    #[test]
    fn the_tail_keeps_the_last_lines_in_order() {
        let pane = "a\nb\nc\nd";

        assert_eq!(tail_of(pane, 2), "c\nd");
        assert_eq!(tail_of(pane, 99), "a\nb\nc\nd");
    }

    #[test]
    fn the_blank_bottom_of_a_pane_is_not_the_tail() {
        // A question, then the rest of a 30-row terminal.
        let mut pane = String::from("Do you want to proceed?");
        pane.push_str(&"\n".repeat(25));

        assert_eq!(tail_of(&pane, 5), "Do you want to proceed?");
        assert_eq!(patterns().classify(&pane), Some(AgentStatus::Waiting));
    }

    #[test]
    fn a_pane_of_nothing_but_blanks_is_empty() {
        assert_eq!(tail_of("\n\n   \n", 5), "");
    }
}
