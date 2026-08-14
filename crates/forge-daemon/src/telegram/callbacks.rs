//! Inline approve/deny, and why a tap sometimes does nothing.
//!
//! A button lives in a chat forever. The question it was asked about does not:
//! the agent may have been answered at the keyboard, restarted, or stopped
//! since. Every button therefore names the exact prompt it belongs to, and a
//! tap that no longer matches is refused rather than guessed at.

use std::collections::HashMap;

/// Which way a tap answers the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Approve,
    Deny,
}

impl Answer {
    fn as_str(self) -> &'static str {
        match self {
            Answer::Approve => "y",
            Answer::Deny => "n",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        match word {
            "y" => Some(Answer::Approve),
            "n" => Some(Answer::Deny),
            _ => None,
        }
    }
}

/// The one prompt a pair of buttons answers.
///
/// `asked` is the id of the `agent_waiting` event that raised them. It is what
/// makes the binding to an *instance* rather than to a session: the next
/// question on the same session is a different event and a different button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prompt {
    pub machine: MachineRef,
    pub task_id: i64,
    pub session_id: i64,
    pub asked: i64,
}

/// Which daemon a button's task lives on. Indexes into the machines config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MachineRef {
    Local,
    /// Position in `config.machines`, which is stable for a daemon's lifetime.
    Remote(usize),
}

impl MachineRef {
    /// Where this machine sits in the fleet, which puts this Mac first.
    ///
    /// The ±1 between a config position and a fleet position lives here and
    /// nowhere else; it was previously open-coded in four places.
    pub fn fleet_index(self) -> usize {
        match self {
            MachineRef::Local => 0,
            MachineRef::Remote(config_index) => config_index + 1,
        }
    }

    /// The machine at `index` in the fleet.
    pub fn from_fleet_index(index: usize) -> Self {
        match index {
            0 => MachineRef::Local,
            other => MachineRef::Remote(other - 1),
        }
    }

    fn as_token(self) -> String {
        match self {
            MachineRef::Local => "l".to_owned(),
            MachineRef::Remote(index) => format!("r{index}"),
        }
    }

    fn parse(token: &str) -> Option<Self> {
        match token.split_at_checked(1)? {
            ("l", "") => Some(MachineRef::Local),
            ("r", index) => index.parse().ok().map(MachineRef::Remote),
            _ => None,
        }
    }
}

/// Telegram gives a callback 64 bytes and hands them back verbatim, so the
/// whole binding travels in the button rather than in a table the daemon would
/// have to keep across restarts.
pub fn encode(prompt: Prompt, answer: Answer) -> String {
    format!(
        "{}:{}:{}:{}:{}",
        answer.as_str(),
        prompt.machine.as_token(),
        prompt.task_id,
        prompt.session_id,
        prompt.asked
    )
}

pub fn decode(data: &str) -> Option<(Prompt, Answer)> {
    let mut parts = data.split(':');
    let answer = Answer::parse(parts.next()?)?;
    let machine = MachineRef::parse(parts.next()?)?;
    let task_id = parts.next()?.parse().ok()?;
    let session_id = parts.next()?.parse().ok()?;
    let asked = parts.next()?.parse().ok()?;

    // A trailing field means this is not a payload this version wrote.
    if parts.next().is_some() {
        return None;
    }

    Some((
        Prompt {
            machine,
            task_id,
            session_id,
            asked,
        },
        answer,
    ))
}

/// What happened to a tap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tap {
    /// Send the adapter's keys for this answer.
    Answer(Answer),
    /// The question is no longer on screen. Nothing is sent.
    Expired(&'static str),
    /// The payload is not one of ours.
    Unreadable,
}

/// The prompt each session is currently sitting on.
///
/// The bot keeps one per session — the last `agent_waiting` it announced — and
/// forgets it as soon as anything else happens to that task.
///
/// Keyed by [`key`] rather than by session id: ids are only unique within one
/// daemon, and two Macs' session 3 are not the same session.
pub type Outstanding = HashMap<SessionKey, i64>;

/// One session anywhere in the fleet.
///
/// A newtype rather than a bare `i64`, so that inserting with the machine and
/// reading without it cannot compile — which is exactly the bug this replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionKey {
    machine: MachineRef,
    session_id: i64,
}

pub fn key(machine: MachineRef, session_id: i64) -> SessionKey {
    SessionKey {
        machine,
        session_id,
    }
}

/// Decide what a tap does, without doing it.
///
/// A tap answers only if the session is still waiting on the *same* question.
/// Anything else expires: the agent has moved on, and injecting a "yes" into
/// whatever it is doing now would answer a question nobody asked.
pub fn tap(data: &str, outstanding: &Outstanding) -> Tap {
    let Some((prompt, answer)) = decode(data) else {
        return Tap::Unreadable;
    };

    match outstanding.get(&key(prompt.machine, prompt.session_id)) {
        Some(&asked) if asked == prompt.asked => Tap::Answer(answer),
        Some(_) => Tap::Expired("That was a different question — the agent has asked again since."),
        None => Tap::Expired("That question has been answered already."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt() -> Prompt {
        Prompt {
            machine: MachineRef::Local,
            task_id: 12,
            session_id: 3,
            asked: 941,
        }
    }

    #[test]
    fn a_button_round_trips_through_telegrams_64_bytes() {
        for machine in [
            MachineRef::Local,
            MachineRef::Remote(0),
            MachineRef::Remote(7),
        ] {
            let original = Prompt {
                machine,
                ..prompt()
            };

            for answer in [Answer::Approve, Answer::Deny] {
                let data = encode(original, answer);

                assert!(data.len() <= 64, "{data} is {} bytes", data.len());
                assert_eq!(decode(&data), Some((original, answer)));
            }
        }
    }

    #[test]
    fn approve_and_deny_are_different_buttons() {
        assert_ne!(
            encode(prompt(), Answer::Approve),
            encode(prompt(), Answer::Deny)
        );
    }

    #[test]
    fn a_payload_from_somewhere_else_is_not_decoded() {
        assert_eq!(decode(""), None);
        assert_eq!(decode("y"), None);
        assert_eq!(decode("y:l:12:3"), None);
        assert_eq!(decode("y:l:12:3:941:extra"), None);
        assert_eq!(decode("maybe:l:12:3:941"), None);
        assert_eq!(decode("y:x:12:3:941"), None);
        assert_eq!(decode("y:l:twelve:3:941"), None);
    }

    #[test]
    fn a_tap_on_the_question_still_on_screen_answers_it() {
        let outstanding = Outstanding::from([(key(MachineRef::Local, 3), 941)]);

        assert_eq!(
            tap(&encode(prompt(), Answer::Approve), &outstanding),
            Tap::Answer(Answer::Approve)
        );
        assert_eq!(
            tap(&encode(prompt(), Answer::Deny), &outstanding),
            Tap::Answer(Answer::Deny)
        );
    }

    #[test]
    fn a_tap_after_the_question_was_answered_at_the_keyboard_sends_nothing() {
        // Answering at the keyboard moves the session on, and the bot forgets
        // the prompt — so the button in the chat is now historical.
        let outstanding = Outstanding::new();

        assert!(matches!(
            tap(&encode(prompt(), Answer::Approve), &outstanding),
            Tap::Expired(_)
        ));
    }

    #[test]
    fn a_tap_on_last_weeks_question_does_not_answer_this_weeks() {
        // Same session, a different question: the dangerous case, because the
        // agent *is* waiting and would accept the keys.
        let outstanding = Outstanding::from([(key(MachineRef::Local, 3), 1_200)]);

        assert!(matches!(
            tap(&encode(prompt(), Answer::Approve), &outstanding),
            Tap::Expired(_)
        ));
    }

    #[test]
    fn another_sessions_question_is_not_this_ones() {
        let outstanding = Outstanding::from([(key(MachineRef::Local, 99), 941)]);

        assert!(matches!(
            tap(&encode(prompt(), Answer::Approve), &outstanding),
            Tap::Expired(_)
        ));
    }

    #[test]
    fn fleet_positions_round_trip_with_this_mac_first() {
        assert_eq!(MachineRef::Local.fleet_index(), 0);
        assert_eq!(MachineRef::Remote(0).fleet_index(), 1);
        assert_eq!(MachineRef::Remote(3).fleet_index(), 4);

        for machine in [
            MachineRef::Local,
            MachineRef::Remote(0),
            MachineRef::Remote(3),
        ] {
            assert_eq!(MachineRef::from_fleet_index(machine.fleet_index()), machine);
        }
    }

    #[test]
    fn a_remote_machines_question_is_answerable_too() {
        // The bug this guards: keying the map by session id alone made every
        // remote tap miss, so the buttons were dead on the fleet case the
        // feature exists for.
        let remote = Prompt {
            machine: MachineRef::Remote(1),
            ..prompt()
        };
        let outstanding = Outstanding::from([(key(MachineRef::Remote(1), 3), 941)]);

        assert_eq!(
            tap(&encode(remote, Answer::Approve), &outstanding),
            Tap::Answer(Answer::Approve)
        );
    }

    #[test]
    fn two_machines_session_three_are_not_the_same_question() {
        // Only the local machine's session 3 is waiting.
        let outstanding = Outstanding::from([(key(MachineRef::Local, 3), 941)]);
        let remote = Prompt {
            machine: MachineRef::Remote(0),
            ..prompt()
        };

        assert!(matches!(
            tap(&encode(remote, Answer::Approve), &outstanding),
            Tap::Expired(_)
        ));
    }

    #[test]
    fn a_payload_the_bot_did_not_write_is_refused_not_guessed() {
        assert_eq!(
            tap(
                "nonsense",
                &Outstanding::from([(key(MachineRef::Local, 3), 941)])
            ),
            Tap::Unreadable
        );
    }
}
