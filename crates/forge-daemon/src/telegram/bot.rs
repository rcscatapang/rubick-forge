//! The bot itself: the long poll, who is allowed to talk to it, and what it
//! does about what they say.

use std::sync::Arc;
use std::time::Duration;

use forge_core::{AdapterId, EventRecord, ForgeEvent, QueueState};
use tokio::sync::Mutex;

use super::api::{Button, Telegram, TelegramError, Update};
use super::callbacks::{self, Answer, MachineRef, Outstanding, Prompt, Tap};
use super::commands::{self, Command, Resolution};
use super::format::{status_icon, Reply};
use crate::config::TelegramConfig;
use crate::fleet::{Fleet, FleetError, FleetTask, NewRemoteTask};
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

pub struct Bot {
    telegram: Telegram,
    fleet: Arc<Fleet>,
    state: AppState,
    allowlist: Allowlist,
    /// The last question announced per session, which is what makes a stale
    /// button tap expire rather than answer the wrong thing.
    outstanding: Arc<Mutex<Outstanding>>,
}

impl Bot {
    pub fn new(
        telegram: Telegram,
        fleet: Arc<Fleet>,
        state: AppState,
        config: &TelegramConfig,
    ) -> Self {
        Self {
            telegram,
            fleet,
            state,
            allowlist: Allowlist {
                ids: config.allowed_user_ids.clone(),
            },
            outstanding: Arc::new(Mutex::new(Outstanding::new())),
        }
    }

    /// Poll Telegram until the daemon shuts down.
    pub async fn run(self) {
        let bot = Arc::new(self);

        // Every machine's events, this Mac's included. Each stream reconnects
        // on its own, so a machine that sleeps costs only its own.
        for index in 0..bot.fleet.len() {
            tokio::spawn(Arc::clone(&bot).watch_machine(index));
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
            Command::Queue => self.queue().await,
            Command::Cancel { queued } => self.cancel(&queued).await,
        }
    }

    /// The hub's queue, and what happened to each row.
    ///
    /// Only meaningful on the hub; a worker's queue is empty by construction,
    /// which reads correctly as "nothing waiting".
    async fn queue(&self) -> Reply {
        if !self.state.config.hub {
            return Reply::plain("This Mac is not the hub, so it has no queue.");
        }

        let Ok(rows) = self.state.store.queue() else {
            return Reply::plain("I cannot read the queue.");
        };

        if rows.is_empty() {
            return Reply::plain("The queue is empty.");
        }

        let lines: Vec<String> = rows
            .iter()
            .rev()
            .take(20)
            .map(|row| match row.state {
                QueueState::Dispatched => format!(
                    "#{} {} → {}",
                    row.id,
                    row.title,
                    row.machine.as_deref().unwrap_or("?")
                ),
                QueueState::Cancelled => format!("#{} {} · cancelled", row.id, row.title),
                QueueState::Queued => format!(
                    "#{} {} · waiting{}",
                    row.id,
                    row.title,
                    row.reason
                        .as_deref()
                        .map(|why| format!(" — {why}"))
                        .unwrap_or_default()
                ),
            })
            .collect();

        Reply::plain(&lines.join("\n"))
    }

    async fn cancel(&self, reference: &str) -> Reply {
        if !self.state.config.hub {
            return Reply::plain("This Mac is not the hub, so it has no queue.");
        }

        let Ok(id) = reference.trim().trim_start_matches('#').parse::<i64>() else {
            return Reply::plain("Give me a queued task's number, as /queue lists them.");
        };

        match self.state.store.cancel_queued(id) {
            // No event: a cancellation is not one of the three the spec names,
            // and inventing one so a dashboard refreshes would put a lie on the
            // bus. A dashboard sees it on its next read.
            Ok(true) => Reply::plain(&format!("Cancelled #{id}.")),
            // Either it was never there or it has already gone somewhere; both
            // mean the same thing to whoever typed this.
            Ok(false) => Reply::plain(&format!(
                "#{id} is not waiting. A task that was dispatched is stopped \
                 with /stop instead."
            )),
            Err(_) => Reply::plain("I cannot reach the queue."),
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
        // On the hub, `/start` queues: a phone has no way to say which Mac, and
        // choosing one is exactly what the queue is for. Everywhere else there
        // is no queue, so it runs here.
        if self.state.config.hub {
            return self.enqueue(project, prompt).await;
        }

        self.start_directly(project, prompt).await
    }

    /// Put it on the hub's queue and let placement choose the machine.
    async fn enqueue(&self, project: &str, prompt: String) -> Reply {
        let title = title_from(&prompt);

        let queued = self.state.store.enqueue(&crate::store::NewQueuedTask {
            project_name: project.to_owned(),
            adapter: AdapterId::ClaudeCode,
            title: title.clone(),
            prompt: Some(prompt),
            target: None,
        });

        match queued {
            Ok(row) => {
                let _ = self.state.bus.publish(ForgeEvent::TaskQueued {
                    queued_id: row.id,
                    project_name: row.project_name.clone(),
                    title: row.title.clone(),
                    target: None,
                });

                Reply::plain(&format!(
                    "Queued #{} “{title}” for {project}. I will say where it lands.",
                    row.id
                ))
            }
            Err(error) => Reply::plain(&format!("Could not queue that: {error}")),
        }
    }

    async fn start_directly(&self, project: &str, prompt: String) -> Reply {
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

        let attempt = NewRemoteTask {
            project_id: chosen.id,
            title: title_from(&prompt),
            prompt,
            adapter: AdapterId::ClaudeCode,
            // Typed by a person who will see whether it worked.
            idempotency_key: None,
        };

        match machine.start(attempt).await {
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

    /// Follow one machine's events for as long as the daemon runs.
    ///
    /// Reconnecting and resuming belong to the fleet — "how to reach that Mac"
    /// is its job, not the bot's. All the bot supplies is what to do with each
    /// event that arrives.
    async fn watch_machine(self: Arc<Self>, index: usize) {
        let Some(machine) = self.fleet.at(index) else {
            return;
        };

        let who = MachineRef::from_fleet_index(index);
        // Only a remote machine's name is worth saying; the local one is the
        // Mac the reader is already thinking of.
        let label = (index > 0).then(|| machine.name().to_owned());

        let bot = Arc::clone(&self);
        machine
            .watch(Box::new(move |record| {
                let bot = Arc::clone(&bot);
                let label = label.clone();

                Box::pin(async move {
                    bot.push_from(who, label.as_deref(), record).await;
                })
            }))
            .await;
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
            // The hub's own events, which belong to no session.
            ForgeEvent::TaskQueued {
                queued_id,
                project_name,
                title,
                target,
            } => (
                Reply::plain(&format!(
                    "📥 Queued #{queued_id} {title} ({project_name}){}",
                    target
                        .as_deref()
                        .map(|name| format!(" for {name}"))
                        .unwrap_or_default()
                )),
                Vec::new(),
            ),
            ForgeEvent::TaskDispatched {
                queued_id,
                machine,
                remote_task,
                ..
            } => (
                Reply::plain(&format!(
                    "📤 Queued #{queued_id} went to {machine} as task {remote_task}"
                )),
                Vec::new(),
            ),
            ForgeEvent::DispatchFailed {
                queued_id,
                machine,
                detail,
            } => (
                Reply::plain(&format!(
                    "⚠️ Queued #{queued_id} could not go to {machine}: {detail}"
                )),
                Vec::new(),
            ),
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
/ask <task> <text> — type something into a running agent
/queue — what is on the hub's queue, and where each row went
/cancel <n> — take a waiting row off the queue";

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
