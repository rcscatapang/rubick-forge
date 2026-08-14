//! The bot itself: the long poll, who is allowed to talk to it, and what it
//! does about what they say.

use std::sync::Arc;
use std::time::Duration;

use forge_core::{EventRecord, ForgeEvent};
use tokio::sync::Mutex;

use super::api::{Button, Telegram, TelegramError, Update};
use super::callbacks::{self, Answer, MachineRef, Outstanding, Prompt, Tap};
use super::commands::{self, Command, Resolution};
use super::format::{status_icon, Reply};
use crate::config::TelegramConfig;
use crate::fleet::{Fleet, FleetError, FleetTask};
use crate::http::AppState;

/// Where the long-poll offset is kept, so a restart does not replay what the
/// bot already answered.
const OFFSET_KEY: &str = "telegram.offset";

/// How long to wait after a failed poll before trying again.
const RETRY_DELAYS: [u64; 5] = [1, 2, 5, 15, 30];

/// Who the bot is willing to talk to, and where it talks back.
struct Allowlist {
    ids: Vec<i64>,
}

impl Allowlist {
    fn permits(&self, user_id: i64) -> bool {
        self.ids.contains(&user_id)
    }
}

/// One remote machine's event stream, and where it sits in the fleet.
#[derive(Clone)]
pub struct RemoteStream {
    /// Position in the fleet, which is what a button payload records.
    pub index: usize,
    pub name: String,
    pub url: String,
    pub token: String,
}

pub struct Bot {
    telegram: Telegram,
    fleet: Fleet,
    state: AppState,
    remotes: Vec<RemoteStream>,
    allowlist: Allowlist,
    /// The last question announced per session, which is what makes a stale
    /// button tap expire rather than answer the wrong thing.
    outstanding: Arc<Mutex<Outstanding>>,
}

impl Bot {
    pub fn new(
        telegram: Telegram,
        fleet: Fleet,
        state: AppState,
        config: &TelegramConfig,
        remotes: Vec<RemoteStream>,
    ) -> Self {
        Self {
            telegram,
            fleet,
            state,
            remotes,
            allowlist: Allowlist {
                ids: config.allowed_user_ids.clone(),
            },
            outstanding: Arc::new(Mutex::new(Outstanding::new())),
        }
    }

    /// Poll Telegram until the daemon shuts down.
    pub async fn run(self) {
        let bot = Arc::new(self);

        // This Mac's own bus, and one socket per other Mac. Each reconnects on
        // its own, so a machine that sleeps costs only its own stream.
        tokio::spawn(Arc::clone(&bot).watch_events());
        for remote in bot.remotes.clone() {
            tokio::spawn(Arc::clone(&bot).watch_remote(remote));
        }

        let mut offset = bot.stored_offset();
        let mut attempt = 0usize;

        loop {
            match bot.telegram.get_updates(offset).await {
                Ok(updates) => {
                    attempt = 0;
                    for update in updates {
                        // The offset advances past every update, handled or
                        // not: one the bot cannot deal with must not be
                        // redelivered forever.
                        offset = update.update_id + 1;
                        bot.handle(update).await;
                        bot.remember_offset(offset);
                    }
                }
                Err(error) => {
                    let wait = RETRY_DELAYS[attempt.min(RETRY_DELAYS.len() - 1)];
                    attempt += 1;
                    tracing::warn!(%error, wait, "the Telegram poll failed; retrying");
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
            }
        }
    }

    fn stored_offset(&self) -> i64 {
        self.state
            .store
            .setting(OFFSET_KEY)
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    }

    fn remember_offset(&self, offset: i64) {
        if let Err(error) = self
            .state
            .store
            .set_setting(OFFSET_KEY, &offset.to_string())
        {
            tracing::warn!(%error, "cannot remember the Telegram offset");
        }
    }

    async fn handle(&self, update: Update) {
        if let Some(query) = update.callback_query {
            self.handle_tap(query).await;
            return;
        }

        let Some(message) = update.message else {
            return;
        };
        let Some(sender) = message.from.as_ref().map(|user| user.id) else {
            return;
        };

        // Not allowlisted: nothing is sent back at all. A reply would confirm
        // the bot exists to whoever found it.
        if !self.allowlist.permits(sender) {
            tracing::warn!(
                sender,
                "dropped a message from a sender not on the allowlist"
            );
            return;
        }

        let Some(text) = message.text.as_deref() else {
            return;
        };
        let Some(parsed) = commands::parse(text) else {
            return;
        };

        let reply = match parsed {
            Ok(command) => self.run_command(command).await,
            Err(problem) => Reply::plain(&problem.to_string()),
        };

        self.say(message.chat.id, &reply, Vec::new()).await;
    }

    async fn run_command(&self, command: Command) -> Reply {
        match command {
            Command::Help => Reply::plain(HELP),
            Command::Status => self.status().await,
            Command::Agents => self.agents().await,
            Command::Start { project, prompt } => self.start(&project, prompt).await,
            Command::Stop { task } => self.stop(&task).await,
            Command::Ask { task, text } => self.ask(&task, text).await,
        }
    }

    async fn status(&self) -> Reply {
        let (tasks, failed) = self.fleet.tasks().await;
        let mut lines = Vec::new();

        for name in self.fleet.names() {
            if failed.iter().any(|error| error.machine() == Some(name)) {
                lines.push(format!("{name}: not answering"));
                continue;
            }

            let mine: Vec<&FleetTask> = tasks.iter().filter(|t| t.machine == name).collect();
            let live = mine.iter().filter(|t| t.task.status.is_live()).count();
            let waiting = mine
                .iter()
                .filter(|t| t.task.status.needs_attention())
                .count();

            lines.push(if mine.is_empty() {
                format!("{name}: nothing running")
            } else if waiting > 0 {
                format!(
                    "{name}: {live} of {} running, {waiting} need you",
                    mine.len()
                )
            } else {
                format!("{name}: {live} of {} running", mine.len())
            });
        }

        Reply::plain(&lines.join("\n"))
    }

    async fn agents(&self) -> Reply {
        let (tasks, failed) = self.fleet.tasks().await;

        let mut lines: Vec<String> = tasks
            .iter()
            .filter(|found| found.task.status.is_live())
            .map(|found| {
                format!(
                    "{} {} · {} · {}",
                    status_icon(found.task.status),
                    found.task.title,
                    found.project.as_deref().unwrap_or("?"),
                    found.machine
                )
            })
            .collect();

        if lines.is_empty() {
            lines.push("Nothing is running.".to_owned());
        }
        lines.extend(unreachable_lines(&failed));

        Reply::plain(&lines.join("\n"))
    }

    async fn start(&self, project: &str, prompt: String) -> Reply {
        let (candidates, failed) = self.fleet.project_candidates().await;
        let names: Vec<_> = candidates.iter().map(|(_, c)| c.clone()).collect();

        let chosen = match commands::resolve(project, &names) {
            Resolution::One(one) => one,
            Resolution::Ambiguous(choices) => return ambiguous("project", &choices),
            Resolution::None => {
                return with_failures(&format!("No project matches “{project}”."), &failed)
            }
        };

        let Some((index, _)) = candidates
            .iter()
            .find(|(_, c)| c.id == chosen.id && c.machine == chosen.machine)
        else {
            return Reply::plain("That project went away while I was looking at it.");
        };
        let Some(machine) = self.fleet.at(*index) else {
            return Reply::plain("That machine is no longer in my configuration.");
        };

        match machine.start(chosen.id, title_from(&prompt), prompt).await {
            Ok(task) => Reply::plain(&format!(
                "Started “{}” on {} ({}).",
                task.title, chosen.machine, chosen.name
            )),
            Err(error) => Reply::plain(&format!("Could not start it: {error}")),
        }
    }

    async fn stop(&self, reference: &str) -> Reply {
        match self.find_task(reference).await {
            Err(reply) => reply,
            Ok((index, found)) => {
                let Some(machine) = self.fleet.at(index) else {
                    return Reply::plain("That machine is no longer in my configuration.");
                };

                match machine.stop(found.task.id).await {
                    Ok(()) => Reply::plain(&format!(
                        "Stopped “{}” on {}.",
                        found.task.title, found.machine
                    )),
                    Err(error) => Reply::plain(&format!("Could not stop it: {error}")),
                }
            }
        }
    }

    async fn ask(&self, reference: &str, text: String) -> Reply {
        match self.find_task(reference).await {
            Err(reply) => reply,
            Ok((index, found)) => {
                let Some(machine) = self.fleet.at(index) else {
                    return Reply::plain("That machine is no longer in my configuration.");
                };

                if let Err(error) = machine.instruct(found.task.id, text).await {
                    return Reply::plain(&format!("Could not send that: {error}"));
                }

                // Give the agent a moment to redraw before reading the screen,
                // or the reply shows the question rather than the answer.
                tokio::time::sleep(Duration::from_millis(700)).await;

                match machine.pane_tail(found.task.id).await {
                    Ok(tail) => {
                        Reply::with_pane(&format!("Sent to “{}”.", found.task.title), &tail)
                    }
                    Err(_) => Reply::plain(&format!("Sent to “{}”.", found.task.title)),
                }
            }
        }
    }

    /// The one task a reference names, or the reply explaining why not.
    async fn find_task(&self, reference: &str) -> Result<(usize, FleetTask), Reply> {
        let (candidates, failed) = self.fleet.task_candidates().await;
        let names: Vec<_> = candidates.iter().map(|(_, c, _)| c.clone()).collect();

        match commands::resolve(reference, &names) {
            Resolution::One(one) => candidates
                .into_iter()
                .find(|(_, c, _)| c.id == one.id && c.machine == one.machine)
                .map(|(index, _, found)| (index, found))
                .ok_or_else(|| Reply::plain("That task went away while I was looking at it.")),
            Resolution::Ambiguous(choices) => Err(ambiguous("task", &choices)),
            Resolution::None => Err(with_failures(
                &format!("No task matches “{reference}”."),
                &failed,
            )),
        }
    }

    async fn handle_tap(&self, query: super::api::CallbackQuery) {
        if !self.allowlist.permits(query.from.id) {
            tracing::warn!(
                sender = query.from.id,
                "dropped a tap from a sender not on the allowlist"
            );
            return;
        }

        let data = query.data.unwrap_or_default();

        // Deciding and claiming happen under one lock. Otherwise two quick taps
        // both read a live prompt and both send keystrokes, which is the thing
        // the staleness rule exists to prevent.
        let decision = {
            let mut outstanding = self.outstanding.lock().await;
            let decision = callbacks::tap(&data, &outstanding);

            if let (Tap::Answer(_), Some((prompt, _))) = (&decision, callbacks::decode(&data)) {
                outstanding.remove(&callbacks::key(prompt.machine, prompt.session_id));
            }

            decision
        };

        let acknowledgement = match decision {
            Tap::Unreadable => "I do not recognise that button.".to_owned(),
            Tap::Expired(why) => why.to_owned(),
            Tap::Answer(answer) => self.answer(&data, answer).await,
        };

        if let Err(error) = self
            .telegram
            .answer_callback(&query.id, &acknowledgement)
            .await
        {
            tracing::warn!(%error, "cannot acknowledge a button tap");
        }

        // An answered question's buttons come off, so the scrollback cannot be
        // tapped again. A failure here is cosmetic: the prompt is already
        // claimed, so a second tap expires anyway.
        if let Some(message) = query.message {
            if let Err(error) = self
                .telegram
                .clear_buttons(message.chat.id, message.message_id)
                .await
            {
                tracing::debug!(%error, "cannot take the buttons off an answered prompt");
            }
        }
    }

    /// Follow one remote machine's event stream, reconnecting for as long as
    /// the daemon runs.
    async fn watch_remote(self: Arc<Self>, remote: RemoteStream) {
        let mut attempt = 0usize;
        let mut after: Option<i64> = None;

        loop {
            match self.stream_once(&remote, &mut after).await {
                Ok(()) => attempt = 0,
                Err(error) => {
                    tracing::debug!(machine = remote.name, %error, "a remote event stream ended");
                }
            }

            let wait = RETRY_DELAYS[attempt.min(RETRY_DELAYS.len() - 1)];
            attempt = attempt.saturating_add(1);
            tokio::time::sleep(Duration::from_secs(wait)).await;
        }
    }

    /// One connection's worth of a remote machine's events.
    ///
    /// `after` is carried across reconnects, so a machine that drops off the
    /// tailnet and comes back replays what was missed instead of losing it.
    async fn stream_once(
        &self,
        remote: &RemoteStream,
        after: &mut Option<i64>,
    ) -> Result<(), String> {
        use futures_util::StreamExt;

        // Browsers cannot set headers on a handshake, so the daemon accepts the
        // token in the query string on `/ws` routes; this client does the same.
        let mut url = format!(
            "{}/ws/events?token={}",
            websocket_base(&remote.url),
            remote.token
        );
        if let Some(seen) = after {
            url.push_str(&format!("&after={seen}"));
        }

        // The URL has a bearer token in it, and tungstenite puts the URL in its
        // errors. Nothing derived from it reaches a log.
        let (mut socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|_| format!("cannot open {}'s event stream", remote.name))?;

        while let Some(frame) = socket.next().await {
            let frame = frame.map_err(|_| format!("{}'s event stream ended", remote.name))?;
            let Ok(text) = frame.into_text() else {
                continue;
            };

            let Ok(record) = serde_json::from_str::<EventRecord>(&text) else {
                continue;
            };

            *after = Some(record.id);
            self.push_from(
                MachineRef::Remote(remote.index - 1),
                Some(&remote.name),
                record,
            )
            .await;
        }

        Ok(())
    }

    async fn answer(&self, data: &str, answer: Answer) -> String {
        let Some((prompt, _)) = callbacks::decode(data) else {
            return "I do not recognise that button.".to_owned();
        };

        let index = match prompt.machine {
            MachineRef::Local => 0,
            MachineRef::Remote(at) => at + 1,
        };
        let Some(machine) = self.fleet.at(index) else {
            return "That machine is no longer in my configuration.".to_owned();
        };

        let approve = answer == Answer::Approve;
        if let Err(error) = machine.answer(prompt.task_id, approve).await {
            return format!("Could not answer: {error}");
        }

        if approve {
            "Approved.".to_owned()
        } else {
            "Denied.".to_owned()
        }
    }

    /// Push the three signal kinds from this Mac's own bus.
    async fn watch_events(self: Arc<Self>) {
        let mut events = self.state.bus.subscribe();

        while let Ok(record) = events.recv().await {
            self.push(record).await;
        }
    }

    async fn push(&self, record: EventRecord) {
        self.push_from(MachineRef::Local, None, record).await;
    }

    /// Announce one event, from whichever machine it happened on.
    ///
    /// A remote event's session id is only unique on its own daemon, so the
    /// outstanding-prompt key is namespaced by machine — otherwise two Macs'
    /// session 3 would expire each other's buttons.
    async fn push_from(&self, machine: MachineRef, label: Option<&str>, record: EventRecord) {
        let where_ = label.map(|name| format!(" on {name}")).unwrap_or_default();

        let (reply, buttons) = match &record.event {
            ForgeEvent::AgentWaiting {
                task_id,
                session_id,
                tail,
            } => {
                self.outstanding
                    .lock()
                    .await
                    .insert(callbacks::key(machine, *session_id), record.id);

                let heading = format!(
                    "🙋 {}{where_} needs you",
                    self.task_name(machine, *task_id).await
                );

                // Buttons only where there is something to answer. A "waiting"
                // read off a screen that is not a permission dialog would send
                // `1` and Enter into whatever is actually there.
                let buttons = if crate::adapters::looks_like_permission_prompt(tail) {
                    let prompt = Prompt {
                        machine,
                        task_id: *task_id,
                        session_id: *session_id,
                        asked: record.id,
                    };

                    vec![vec![
                        Button {
                            text: "Approve".to_owned(),
                            callback_data: callbacks::encode(prompt, Answer::Approve),
                        },
                        Button {
                            text: "Deny".to_owned(),
                            callback_data: callbacks::encode(prompt, Answer::Deny),
                        },
                    ]]
                } else {
                    Vec::new()
                };

                (Reply::with_pane(&heading, tail), buttons)
            }
            ForgeEvent::AgentError {
                task_id, detail, ..
            } => {
                self.forget(machine, &record).await;
                let heading = format!(
                    "🔥 {}{where_} errored",
                    self.task_name(machine, *task_id).await
                );

                (Reply::with_pane(&heading, detail), Vec::new())
            }
            ForgeEvent::TaskFinished { task_id, .. } => {
                self.forget(machine, &record).await;

                (
                    Reply::plain(&format!(
                        "✅ {}{where_} finished",
                        self.task_name(machine, *task_id).await
                    )),
                    Vec::new(),
                )
            }
            // Anything else means the question on that session is no longer the
            // one a button in the chat refers to.
            _ => {
                self.forget(machine, &record).await;
                return;
            }
        };

        // Pushes go to the allowlisted users directly, never to whatever chat
        // the bot happens to be in: a group would put pane output in front of
        // everyone in it, allowlisted or not.
        for user in &self.allowlist.ids {
            self.say(*user, &reply, buttons.clone()).await;
        }
    }

    /// Any event other than the question itself clears the outstanding prompt.
    async fn forget(&self, machine: MachineRef, record: &EventRecord) {
        if let Some(session_id) = session_of(&record.event) {
            self.outstanding
                .lock()
                .await
                .remove(&callbacks::key(machine, session_id));
        }
    }

    /// A task's title, which only the local store can be asked for cheaply.
    async fn task_name(&self, machine: MachineRef, task_id: i64) -> String {
        if machine == MachineRef::Local {
            if let Ok(Some(task)) = self.state.store.task(task_id) {
                return task.title;
            }
        }
        format!("Task {task_id}")
    }

    async fn say(&self, chat_id: i64, reply: &Reply, buttons: Vec<Vec<Button>>) {
        if let Err(error) = self
            .telegram
            .send_message(chat_id, reply.as_str(), buttons)
            .await
        {
            match error {
                TelegramError::Api(detail) => {
                    tracing::warn!(chat_id, detail, "Telegram would not deliver a message")
                }
                TelegramError::Transport(detail) => {
                    tracing::warn!(chat_id, detail, "cannot reach Telegram")
                }
            }
        }
    }
}

/// An `http(s)` base as its WebSocket equivalent.
///
/// Only the scheme is rewritten: a blanket replace turns a host that happens to
/// contain "http" into nonsense.
fn websocket_base(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');

    match trimmed.split_once("://") {
        Some(("http", rest)) => format!("ws://{rest}"),
        Some(("https", rest)) => format!("wss://{rest}"),
        _ => trimmed.to_owned(),
    }
}

fn session_of(event: &ForgeEvent) -> Option<i64> {
    match event {
        ForgeEvent::AgentWaiting { session_id, .. }
        | ForgeEvent::AgentError { session_id, .. }
        | ForgeEvent::TaskFinished { session_id, .. }
        | ForgeEvent::SessionStarted { session_id, .. }
        | ForgeEvent::SessionStopped { session_id, .. }
        | ForgeEvent::StatusChanged { session_id, .. } => Some(*session_id),
        _ => None,
    }
}

/// A task title from what someone asked for, since the bot has no title field.
pub fn title_from(prompt: &str) -> String {
    const MAX_WORDS: usize = 8;
    const MAX_CHARS: usize = 60;

    let words: Vec<&str> = prompt.split_whitespace().take(MAX_WORDS).collect();
    let title = words.join(" ");

    let title = if title.chars().count() > MAX_CHARS {
        title.chars().take(MAX_CHARS).collect::<String>()
    } else {
        title
    };

    let title = title
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .trim();

    if title.is_empty() {
        "Task from Telegram".to_owned()
    } else {
        title.to_owned()
    }
}

/// The choices, so the user picks rather than the bot guessing.
fn ambiguous(kind: &str, choices: &[commands::Candidate]) -> Reply {
    let mut lines = vec![format!("More than one {kind} matches. Which one?")];
    lines.extend(
        choices
            .iter()
            .map(|choice| format!("· {} ({}) — {}", choice.name, choice.id, choice.machine)),
    );

    Reply::plain(&lines.join("\n"))
}

fn with_failures(message: &str, failed: &[FleetError]) -> Reply {
    let mut lines = vec![message.to_owned()];
    lines.extend(unreachable_lines(failed));
    Reply::plain(&lines.join("\n"))
}

/// Which machines could not be asked, so a partial answer says it is partial.
fn unreachable_lines(failed: &[FleetError]) -> Vec<String> {
    failed
        .iter()
        .filter_map(|error| error.machine())
        .map(|machine| format!("({machine} is not answering, so it is not counted.)"))
        .collect()
}

const HELP: &str = "\
/status — how every machine is doing
/agents — what is running, and where
/start <project> <what to do> — create a task and launch its agent
/stop <task> — stop a task's agent
/ask <task> <text> — type something into a running agent";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_first_few_words_of_the_prompt() {
        assert_eq!(title_from("fix the flaky test"), "fix the flaky test");
    }

    #[test]
    fn a_long_prompt_becomes_a_short_title() {
        let title = title_from(
            "please go and fix the flaky integration test that keeps failing in continuous \
             integration on tuesdays",
        );

        assert!(title.chars().count() <= 60, "{title}");
        assert!(title.starts_with("please go and fix"));
    }

    #[test]
    fn a_title_does_not_end_in_dangling_punctuation() {
        assert_eq!(title_from("fix the build,"), "fix the build");
        assert_eq!(title_from("why is it broken?"), "why is it broken");
    }

    #[test]
    fn a_prompt_with_no_words_still_produces_a_title() {
        assert_eq!(title_from("   "), "Task from Telegram");
        assert_eq!(title_from("..."), "Task from Telegram");
    }

    #[test]
    fn only_the_scheme_becomes_a_websocket_one() {
        assert_eq!(
            websocket_base("http://100.64.0.1:8787"),
            "ws://100.64.0.1:8787"
        );
        assert_eq!(websocket_base("https://mini:8787/"), "wss://mini:8787");
        // A blanket replace would have made this "ws://ws-mini:8787".
        assert_eq!(
            websocket_base("http://http-mini:8787"),
            "ws://http-mini:8787"
        );
    }

    #[test]
    fn the_allowlist_is_the_whole_of_who_may_talk() {
        let allowlist = Allowlist { ids: vec![7, 9] };

        assert!(allowlist.permits(7));
        assert!(allowlist.permits(9));
        assert!(!allowlist.permits(8));
        // An empty allowlist permits nobody. The config refuses to load in that
        // state, so this is the second line of defence.
        assert!(!Allowlist { ids: vec![] }.permits(7));
    }

    #[test]
    fn ambiguity_lists_every_choice_with_its_machine() {
        let choices = vec![
            commands::Candidate {
                id: 1,
                name: "api-server".into(),
                machine: "mini".into(),
            },
            commands::Candidate {
                id: 2,
                name: "api-client".into(),
                machine: "laptop".into(),
            },
        ];

        let reply = ambiguous("task", &choices);

        assert!(reply.as_str().contains("api"), "{reply:?}");
        assert!(reply.as_str().contains("server"), "{reply:?}");
        assert!(reply.as_str().contains("client"), "{reply:?}");
        assert!(reply.as_str().contains("laptop"), "{reply:?}");
    }

    #[test]
    fn a_partial_answer_names_the_machine_it_could_not_ask() {
        let failed = [FleetError::Unreachable {
            machine: "Mac mini".into(),
        }];

        let reply = with_failures("Nothing matches.", &failed);

        assert!(
            reply.as_str().contains("Mac mini is not answering"),
            "{reply:?}"
        );
    }

    #[test]
    fn a_refusal_is_not_reported_as_an_unreachable_machine() {
        let failed = [FleetError::Refused("no".into())];

        assert!(unreachable_lines(&failed).is_empty());
    }

    #[test]
    fn every_event_with_a_session_is_one_that_clears_a_prompt() {
        let waiting = ForgeEvent::AgentWaiting {
            task_id: 1,
            session_id: 3,
            tail: String::new(),
        };
        let changed = ForgeEvent::StatusChanged {
            task_id: 1,
            session_id: 3,
            from: forge_core::AgentStatus::Waiting,
            to: forge_core::AgentStatus::Working,
        };

        assert_eq!(session_of(&waiting), Some(3));
        assert_eq!(session_of(&changed), Some(3));
        assert_eq!(session_of(&ForgeEvent::TaskDeleted { task_id: 1 }), None);
    }
}
