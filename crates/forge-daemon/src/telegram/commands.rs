//! Turning a chat message into something the daemon can do.
//!
//! Parsing and resolution are kept apart from doing: what a message means is
//! decided here, against a list of names, with no daemon and no network in
//! sight.

/// What an allowlisted sender asked for (SPEC D22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// One line per machine: how many tasks, and how many need a human.
    Status,
    /// Every live session, with its state and its machine.
    Agents,
    Start {
        project: String,
        prompt: String,
    },
    Stop {
        task: String,
    },
    Ask {
        task: String,
        text: String,
    },
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("I only understand /status, /agents, /start, /stop, /ask and /help.")]
    Unknown,
    #[error("/start needs a project and something to do: /start forge fix the flaky test")]
    StartNeedsPrompt,
    #[error("/stop needs a task: /stop 12, or /stop flaky")]
    StopNeedsTask,
    #[error("/ask needs a task and something to say: /ask 12 yes, go ahead")]
    AskNeedsText,
}

/// Parse one message.
///
/// Telegram appends `@thisbot` to commands in groups, which is not part of the
/// command. Anything that is not a command at all is `None` rather than an
/// error — the bot does not lecture people for talking in a chat it is in.
pub fn parse(message: &str) -> Option<Result<Command, ParseError>> {
    let message = message.trim();
    let rest = message.strip_prefix('/')?;

    let (word, arguments) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let word = word.split('@').next().unwrap_or(word);
    let arguments = arguments.trim();

    Some(match word {
        "status" => Ok(Command::Status),
        "agents" => Ok(Command::Agents),
        "help" | "start_help" => Ok(Command::Help),
        "start" => match arguments.split_once(char::is_whitespace) {
            Some((project, prompt)) if !prompt.trim().is_empty() => Ok(Command::Start {
                project: project.to_owned(),
                prompt: prompt.trim().to_owned(),
            }),
            _ => Err(ParseError::StartNeedsPrompt),
        },
        "stop" if !arguments.is_empty() => Ok(Command::Stop {
            task: arguments.to_owned(),
        }),
        "stop" => Err(ParseError::StopNeedsTask),
        "ask" => match arguments.split_once(char::is_whitespace) {
            Some((task, text)) if !text.trim().is_empty() => Ok(Command::Ask {
                task: task.to_owned(),
                text: text.trim().to_owned(),
            }),
            _ => Err(ParseError::AskNeedsText),
        },
        _ => Err(ParseError::Unknown),
    })
}

/// One thing a reference could have meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: i64,
    pub name: String,
    /// Which machine it is on, for a reply that has to say.
    pub machine: String,
}

/// What a `/stop 12` or `/start forge …` turned out to mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    One(Candidate),
    /// More than one thing matched. The bot lists them; it never picks.
    Ambiguous(Vec<Candidate>),
    None,
}

/// Find the one thing `reference` names, or say why not.
///
/// An exact numeric id wins outright, then an exact case-insensitive name, then
/// a unique case-insensitive prefix. Ambiguity is always reported, never
/// guessed: `/stop api` with two projects starting `api` must not stop one of
/// them at random.
///
/// Ids are only unambiguous within a machine, so a bare number that matches on
/// two machines is ambiguous like any other duplicate.
pub fn resolve(reference: &str, candidates: &[Candidate]) -> Resolution {
    let reference = reference.trim();
    if reference.is_empty() {
        return Resolution::None;
    }

    if let Ok(id) = reference.parse::<i64>() {
        let matched: Vec<_> = candidates.iter().filter(|c| c.id == id).cloned().collect();
        return pick(matched);
    }

    let lowered = reference.to_lowercase();

    let exact: Vec<_> = candidates
        .iter()
        .filter(|c| c.name.to_lowercase() == lowered)
        .cloned()
        .collect();
    if !exact.is_empty() {
        return pick(exact);
    }

    pick(
        candidates
            .iter()
            .filter(|c| c.name.to_lowercase().starts_with(&lowered))
            .cloned()
            .collect(),
    )
}

fn pick(matched: Vec<Candidate>) -> Resolution {
    match matched.len() {
        0 => Resolution::None,
        1 => Resolution::One(matched.into_iter().next().expect("just checked the length")),
        _ => Resolution::Ambiguous(matched),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: i64, name: &str, machine: &str) -> Candidate {
        Candidate {
            id,
            name: name.to_owned(),
            machine: machine.to_owned(),
        }
    }

    #[test]
    fn the_commands_the_spec_lists_all_parse() {
        assert_eq!(parse("/status"), Some(Ok(Command::Status)));
        assert_eq!(parse("/agents"), Some(Ok(Command::Agents)));
        assert_eq!(
            parse("/start forge fix the flaky test"),
            Some(Ok(Command::Start {
                project: "forge".into(),
                prompt: "fix the flaky test".into(),
            }))
        );
        assert_eq!(
            parse("/stop 12"),
            Some(Ok(Command::Stop { task: "12".into() }))
        );
        assert_eq!(
            parse("/ask 12 yes, go ahead"),
            Some(Ok(Command::Ask {
                task: "12".into(),
                text: "yes, go ahead".into(),
            }))
        );
    }

    #[test]
    fn a_command_addressed_to_the_bot_in_a_group_is_the_same_command() {
        assert_eq!(parse("/status@rubickforgebot"), Some(Ok(Command::Status)));
        assert_eq!(
            parse("/stop@rubickforgebot 12"),
            Some(Ok(Command::Stop { task: "12".into() }))
        );
    }

    #[test]
    fn ordinary_conversation_is_not_a_command_and_not_an_error() {
        assert_eq!(parse("hello"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("what is forge doing"), None);
    }

    #[test]
    fn a_command_missing_its_arguments_says_what_it_wanted() {
        assert_eq!(parse("/start"), Some(Err(ParseError::StartNeedsPrompt)));
        // A project with no prompt is not a task worth starting.
        assert_eq!(
            parse("/start forge"),
            Some(Err(ParseError::StartNeedsPrompt))
        );
        assert_eq!(parse("/stop"), Some(Err(ParseError::StopNeedsTask)));
        assert_eq!(parse("/ask 12"), Some(Err(ParseError::AskNeedsText)));
        assert_eq!(parse("/wat"), Some(Err(ParseError::Unknown)));
    }

    #[test]
    fn the_prompt_keeps_its_spacing_and_punctuation() {
        let Some(Ok(Command::Start { prompt, .. })) = parse("/start forge  fix /tmp: it's broken")
        else {
            panic!("should parse");
        };

        assert_eq!(prompt, "fix /tmp: it's broken");
    }

    #[test]
    fn an_id_resolves_to_the_task_with_it() {
        let all = [
            candidate(11, "flaky", "mini"),
            candidate(12, "docs", "mini"),
        ];

        assert_eq!(resolve("12", &all), Resolution::One(all[1].clone()));
    }

    #[test]
    fn a_name_resolves_whatever_its_case() {
        let all = [candidate(11, "Flaky test", "mini")];

        assert_eq!(resolve("flaky test", &all), Resolution::One(all[0].clone()));
    }

    #[test]
    fn a_unique_prefix_is_enough() {
        let all = [
            candidate(11, "flaky", "mini"),
            candidate(12, "docs", "mini"),
        ];

        assert_eq!(resolve("fl", &all), Resolution::One(all[0].clone()));
    }

    #[test]
    fn an_ambiguous_prefix_returns_the_choices_rather_than_guessing() {
        let all = [
            candidate(11, "api-server", "mini"),
            candidate(12, "api-client", "mini"),
        ];

        let Resolution::Ambiguous(choices) = resolve("api", &all) else {
            panic!("two things start with api");
        };
        assert_eq!(choices.len(), 2);
    }

    #[test]
    fn an_exact_name_beats_a_prefix_of_a_longer_one() {
        let all = [
            candidate(11, "api", "mini"),
            candidate(12, "api-client", "mini"),
        ];

        assert_eq!(resolve("api", &all), Resolution::One(all[0].clone()));
    }

    #[test]
    fn the_same_id_on_two_machines_is_ambiguous() {
        // Ids are per-daemon, so this is the normal case in a fleet.
        let all = [
            candidate(1, "flaky", "mini"),
            candidate(1, "docs", "laptop"),
        ];

        assert!(matches!(resolve("1", &all), Resolution::Ambiguous(_)));
    }

    #[test]
    fn nothing_matching_is_nothing_matching() {
        let all = [candidate(11, "flaky", "mini")];

        assert_eq!(resolve("nope", &all), Resolution::None);
        assert_eq!(resolve("99", &all), Resolution::None);
        assert_eq!(resolve("", &all), Resolution::None);
    }
}
