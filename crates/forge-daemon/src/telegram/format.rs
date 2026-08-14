//! What the bot says, and how it is made safe to say it.
//!
//! Everything outbound goes through here: pane text is somebody else's TUI, and
//! it reaches a phone as readable lines or not at all.

use forge_core::AgentStatus;

/// Telegram rejects a message over 4096 characters. The cap is well under it,
/// because a phone notification shows a fraction of that anyway.
const MAX_MESSAGE: usize = 1_500;

/// How much of a pane tail a notification carries.
const MAX_TAIL_LINES: usize = 20;

/// One escape sequence: CSI, OSC, or a two-character escape.
///
/// Written out rather than pulled from a regex crate — the daemon has no regex
/// dependency, and this is the only place that needs one.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }

        match chars.next() {
            // CSI: parameters and intermediates, then one final byte.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: runs until BEL or ST.
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Anything else is a two-character escape, already consumed.
            _ => {}
        }
    }

    out
}

/// Terminal output as lines a person can read on a phone.
///
/// A pane tail is a rectangle of a redrawing TUI: escape sequences, box
/// drawing, and whatever blank space was left over.
pub fn sanitise_tail(tail: &str) -> String {
    let trim = |c: char| c.is_whitespace() || is_box_drawing(c);

    let stripped = strip_ansi(tail);
    let lines: Vec<&str> = stripped
        .lines()
        .map(|line| line.trim_matches(trim))
        .filter(|line| !line.is_empty())
        .collect();

    // The end of the pane, not its beginning: what the agent just said is what
    // someone reading this on a phone needs.
    let start = lines.len().saturating_sub(MAX_TAIL_LINES);
    cap(&lines[start..].join("\n"))
}

fn is_box_drawing(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{257f}')
}

/// Truncate on a character boundary, saying that it was truncated.
pub fn cap(text: &str) -> String {
    if text.chars().count() <= MAX_MESSAGE {
        return text.to_owned();
    }

    let kept: String = text.chars().take(MAX_MESSAGE).collect();
    format!("{}\n…", kept.trim_end())
}

/// Escape the four characters Telegram's MarkdownV2 treats as structure inside
/// a code block.
///
/// Only `\` and `` ` `` matter inside one, but a stray backtick in pane output
/// would end the block and let the rest be parsed as markup.
pub fn escape_code(text: &str) -> String {
    text.replace('\\', "\\\\").replace('`', "\\`")
}

/// Pane output as a fenced block, which is how terminal text stays legible.
pub fn code_block(text: &str) -> String {
    format!("```\n{}\n```", escape_code(text))
}

/// The emoji that carries a status at a glance in a chat list.
pub fn status_icon(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "💤",
        AgentStatus::Working => "⚙️",
        AgentStatus::Waiting => "🙋",
        AgentStatus::Error => "🔥",
        AgentStatus::Stopped => "⏹",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_do_not_reach_the_phone() {
        let raw = "\u{1b}[38;2;255;106;193mDo you want to edit src/main.rs?\u{1b}[39m";

        assert_eq!(sanitise_tail(raw), "Do you want to edit src/main.rs?");
    }

    #[test]
    fn the_escapes_a_colour_code_is_not_are_stripped_too() {
        let raw = "\u{1b}[>4;2m\u{1b}[=7h\u{1b}[?2004hReady\u{1b}[0m";

        assert_eq!(sanitise_tail(raw), "Ready");
    }

    #[test]
    fn an_operating_system_command_is_stripped_whole() {
        let raw = "\u{1b}]0;a title\u{7}Ready";

        assert_eq!(sanitise_tail(raw), "Ready");
    }

    #[test]
    fn the_box_a_dialog_is_drawn_in_is_not_the_question() {
        let raw = "\u{2502} Do you want to proceed? \u{2502}\n\u{2500}\u{2500}\u{2500}\u{2500}\n\n\u{2502} 1. Yes \u{2502}";

        assert_eq!(sanitise_tail(raw), "Do you want to proceed?\n1. Yes");
    }

    #[test]
    fn a_long_pane_keeps_its_end_not_its_beginning() {
        // What the agent just said matters; what it said a screen ago does not.
        let pane = (1..=40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");

        let tail = sanitise_tail(&pane);
        let lines: Vec<&str> = tail.lines().collect();

        assert_eq!(lines.len(), MAX_TAIL_LINES);
        assert_eq!(lines[0], "line 21");
        assert_eq!(lines[MAX_TAIL_LINES - 1], "line 40");
    }

    #[test]
    fn a_tail_with_nothing_readable_says_nothing() {
        assert_eq!(sanitise_tail("\u{1b}[2J\u{1b}[H"), "");
        assert_eq!(sanitise_tail(""), "");
        assert_eq!(sanitise_tail("   \n\u{2500}\u{2500}\n  "), "");
    }

    #[test]
    fn a_message_too_long_for_telegram_is_cut_and_says_so() {
        let long = "x".repeat(MAX_MESSAGE + 500);

        let capped = cap(&long);

        assert!(capped.chars().count() <= MAX_MESSAGE + 2);
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn capping_counts_characters_not_bytes() {
        // Splitting a multi-byte character would produce invalid output.
        let long = "é".repeat(MAX_MESSAGE + 10);

        let capped = cap(&long);

        assert!(capped.starts_with('é'));
        assert!(capped.ends_with('…'));
    }

    #[test]
    fn a_backtick_in_pane_output_cannot_end_the_code_block() {
        // Unescaped, this would close the fence and let the rest be markup.
        assert_eq!(escape_code("run `rm -rf /` now"), "run \\`rm -rf /\\` now");
        assert_eq!(escape_code(r"a \ backslash"), r"a \\ backslash");

        let block = code_block("run `rm -rf /` now");
        assert!(block.starts_with("```\n"));
        assert!(block.ends_with("\n```"));
        // Every backtick between the fences is escaped.
        let inner = &block[4..block.len() - 4];
        assert!(inner
            .match_indices('`')
            .all(|(at, _)| inner[..at].ends_with('\\')));
    }

    #[test]
    fn every_status_has_an_icon() {
        for status in [
            AgentStatus::Idle,
            AgentStatus::Working,
            AgentStatus::Waiting,
            AgentStatus::Error,
            AgentStatus::Stopped,
        ] {
            assert!(!status_icon(status).is_empty());
        }
    }
}
