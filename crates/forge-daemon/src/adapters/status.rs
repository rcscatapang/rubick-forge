//! Turning one poll of a session into a status, and deciding what to announce.
//!
//! The whole thing is heuristic and says so. What it protects is the one
//! transition people rely on: entering `waiting`, because that is when a human
//! has to be told.

use forge_core::AgentStatus;

use super::markers::StatusPatterns;
use crate::runtime::PaneState;

/// Polls a state may survive with nothing recognised before it decays to
/// `idle`. Without this, an agent that finished into a screen the markers do
/// not know would claim to be working forever.
pub const STALENESS_CAP: u32 = 15;

/// Polls a new `waiting` reading must survive before it is announced.
///
/// A CLI redrawing its screen can show a confirmation dialog's remains for a
/// single frame; announcing on the first sighting turns that into a
/// notification the user cannot act on.
pub const WAITING_DEBOUNCE: u32 = 1;

/// What the poller knows about a session between polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracker {
    /// The status currently believed, and already announced.
    pub status: AgentStatus,
    /// Polls since anything was recognised.
    stale_polls: u32,
    /// A reading seen but not yet trusted, and how many polls it has held.
    pending: Option<(AgentStatus, u32)>,
    /// The runtime's activity marker as of the last poll that read the pane.
    last_activity: Option<u64>,
}

/// What the caller should do about a poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The status after this poll.
    pub status: AgentStatus,
    /// Set when the status changed, so a `status_changed` event is due.
    pub changed_from: Option<AgentStatus>,
    /// Set when the session has just entered `waiting` and held it.
    pub entered_waiting: bool,
    /// The agent's process has exited, so the session is over.
    ///
    /// Only ever set from the process itself. An `error` read off the screen
    /// says the agent is unhappy, not that it is gone — ending its session on
    /// that would destroy a recoverable agent over a line of output.
    pub process_ended: bool,
}

impl Outcome {
    pub fn changed(&self) -> bool {
        self.changed_from.is_some()
    }
}

impl Tracker {
    pub fn new(status: AgentStatus) -> Self {
        Self {
            status,
            stale_polls: 0,
            pending: None,
            last_activity: None,
        }
    }

    /// Whether the pane has produced nothing since the last poll that read it.
    ///
    /// Capturing is the expensive part; a session sitting at a prompt costs
    /// one cheap question instead.
    ///
    /// A reading still waiting to be confirmed is never skipped: a question on
    /// a motionless screen is exactly the case the debounce exists for, and
    /// skipping would leave it pending forever.
    pub fn is_unchanged_since(&self, activity: Option<u64>) -> bool {
        if self.pending.is_some() {
            return false;
        }

        match (self.last_activity, activity) {
            (Some(last), Some(now)) => last == now,
            // Without a marker to compare, there is nothing to skip on.
            _ => false,
        }
    }

    pub fn saw_activity(&mut self, activity: Option<u64>) {
        self.last_activity = activity;
    }

    /// Apply one poll's evidence, in the documented order.
    ///
    /// A dead process wins over anything on screen: whatever the pane still
    /// shows, an agent that has exited is not waiting for anyone.
    pub fn poll(
        &mut self,
        pane: &PaneState,
        output: Option<&str>,
        patterns: &StatusPatterns,
    ) -> Outcome {
        if !pane.alive {
            let status = match pane.exit_code {
                Some(0) | None => AgentStatus::Stopped,
                Some(_) => AgentStatus::Error,
            };
            let mut outcome = self.settle(status);
            outcome.process_ended = true;
            return outcome;
        }

        match output.and_then(|screen| patterns.classify(screen)) {
            Some(seen) => {
                self.stale_polls = 0;
                self.consider(seen)
            }
            None => self.nothing_recognised(),
        }
    }

    /// Nothing was read this time, and that is not evidence of anything.
    ///
    /// Used when the poll was skipped because the pane had produced no output
    /// since the last one: the belief holds, and the staleness clock does not
    /// advance, because nothing failed to be recognised.
    pub fn skipped(&self) -> Outcome {
        self.unchanged()
    }

    /// A reading has to hold before it is believed — but only entering
    /// `waiting` is worth the delay, since only it raises a notification.
    fn consider(&mut self, seen: AgentStatus) -> Outcome {
        if seen == self.status {
            self.pending = None;
            return self.unchanged();
        }

        if seen != AgentStatus::Waiting {
            return self.settle(seen);
        }

        let held = match self.pending {
            Some((pending, held)) if pending == seen => held + 1,
            _ => 0,
        };

        if held >= WAITING_DEBOUNCE {
            return self.settle(seen);
        }

        self.pending = Some((seen, held));
        self.unchanged()
    }

    /// Nothing matched. Keep the current belief, up to a point.
    fn nothing_recognised(&mut self) -> Outcome {
        self.pending = None;
        self.stale_polls += 1;

        if self.stale_polls > STALENESS_CAP && self.status != AgentStatus::Idle {
            self.stale_polls = 0;
            return self.settle(AgentStatus::Idle);
        }

        self.unchanged()
    }

    fn settle(&mut self, status: AgentStatus) -> Outcome {
        let previous = self.status;
        self.status = status;
        self.pending = None;

        Outcome {
            status,
            changed_from: (previous != status).then_some(previous),
            entered_waiting: previous != status && status == AgentStatus::Waiting,
            process_ended: false,
        }
    }

    fn unchanged(&self) -> Outcome {
        Outcome {
            status: self.status,
            changed_from: None,
            entered_waiting: false,
            process_ended: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::markers::MarkerSet;

    const PATTERNS: StatusPatterns = StatusPatterns {
        sets: &[
            MarkerSet {
                status: AgentStatus::Waiting,
                markers: &["ASKING"],
            },
            MarkerSet {
                status: AgentStatus::Working,
                markers: &["BUSY"],
            },
            MarkerSet {
                status: AgentStatus::Idle,
                markers: &["PROMPT"],
            },
        ],
    };

    fn alive() -> PaneState {
        PaneState {
            pid: Some(1),
            alive: true,
            exit_code: None,
        }
    }

    fn dead(code: Option<i32>) -> PaneState {
        PaneState {
            pid: Some(1),
            alive: false,
            exit_code: code,
        }
    }

    fn poll(tracker: &mut Tracker, screen: &str) -> Outcome {
        tracker.poll(&alive(), Some(screen), &PATTERNS)
    }

    #[test]
    fn a_recognised_change_is_announced_once() {
        let mut tracker = Tracker::new(AgentStatus::Idle);

        let first = poll(&mut tracker, "BUSY");
        assert_eq!(first.status, AgentStatus::Working);
        assert_eq!(first.changed_from, Some(AgentStatus::Idle));

        let second = poll(&mut tracker, "BUSY");
        assert_eq!(second.status, AgentStatus::Working);
        assert!(!second.changed());
    }

    #[test]
    fn entering_waiting_takes_one_extra_poll() {
        let mut tracker = Tracker::new(AgentStatus::Working);

        let first = poll(&mut tracker, "ASKING");
        assert_eq!(first.status, AgentStatus::Working, "not believed yet");
        assert!(!first.entered_waiting);

        let second = poll(&mut tracker, "ASKING");
        assert_eq!(second.status, AgentStatus::Waiting);
        assert!(second.entered_waiting);
        assert_eq!(second.changed_from, Some(AgentStatus::Working));
    }

    #[test]
    fn a_single_frame_of_waiting_never_becomes_an_event() {
        let mut tracker = Tracker::new(AgentStatus::Working);

        poll(&mut tracker, "ASKING");
        let recovered = poll(&mut tracker, "BUSY");

        assert_eq!(recovered.status, AgentStatus::Working);
        assert!(!recovered.entered_waiting);
        assert!(!recovered.changed(), "the flap changed nothing");
    }

    #[test]
    fn waiting_is_announced_only_on_entering_it() {
        let mut tracker = Tracker::new(AgentStatus::Working);
        poll(&mut tracker, "ASKING");
        assert!(poll(&mut tracker, "ASKING").entered_waiting);

        for _ in 0..5 {
            let outcome = poll(&mut tracker, "ASKING");
            assert!(!outcome.entered_waiting, "no restacking while it holds");
            assert!(!outcome.changed());
        }
    }

    #[test]
    fn leaving_and_re_entering_waiting_announces_again() {
        let mut tracker = Tracker::new(AgentStatus::Working);
        poll(&mut tracker, "ASKING");
        poll(&mut tracker, "ASKING");
        poll(&mut tracker, "BUSY");

        poll(&mut tracker, "ASKING");
        assert!(poll(&mut tracker, "ASKING").entered_waiting);
    }

    #[test]
    fn other_transitions_are_believed_immediately() {
        let mut tracker = Tracker::new(AgentStatus::Idle);

        assert!(poll(&mut tracker, "BUSY").changed());
        assert!(poll(&mut tracker, "PROMPT").changed());
    }

    #[test]
    fn an_unrecognised_screen_keeps_the_current_belief() {
        let mut tracker = Tracker::new(AgentStatus::Working);

        for _ in 0..STALENESS_CAP {
            let outcome = poll(&mut tracker, "something new");
            assert_eq!(outcome.status, AgentStatus::Working);
            assert!(!outcome.changed());
        }
    }

    #[test]
    fn a_belief_that_goes_unconfirmed_for_long_enough_decays_to_idle() {
        let mut tracker = Tracker::new(AgentStatus::Working);

        let mut outcome = None;
        for _ in 0..=STALENESS_CAP {
            outcome = Some(poll(&mut tracker, "unrecognised"));
        }

        let outcome = outcome.unwrap();
        assert_eq!(outcome.status, AgentStatus::Idle);
        assert_eq!(outcome.changed_from, Some(AgentStatus::Working));
    }

    #[test]
    fn idle_does_not_decay_into_itself_over_and_over() {
        let mut tracker = Tracker::new(AgentStatus::Idle);

        for _ in 0..STALENESS_CAP * 3 {
            assert!(!poll(&mut tracker, "unrecognised").changed());
        }
    }

    #[test]
    fn a_recognised_screen_resets_the_staleness_clock() {
        let mut tracker = Tracker::new(AgentStatus::Working);

        for _ in 0..STALENESS_CAP {
            poll(&mut tracker, "unrecognised");
        }
        poll(&mut tracker, "BUSY");
        for _ in 0..STALENESS_CAP {
            assert!(!poll(&mut tracker, "unrecognised").changed());
        }
    }

    #[test]
    fn an_error_read_off_the_screen_does_not_end_the_session() {
        const WITH_ERROR: StatusPatterns = StatusPatterns {
            sets: &[MarkerSet {
                status: AgentStatus::Error,
                markers: &["API Error"],
            }],
        };
        let mut tracker = Tracker::new(AgentStatus::Working);

        let outcome = tracker.poll(&alive(), Some("API Error: overloaded"), &WITH_ERROR);

        assert_eq!(outcome.status, AgentStatus::Error);
        assert!(
            !outcome.process_ended,
            "an unhappy agent is still a running agent"
        );
    }

    #[test]
    fn only_the_process_dying_ends_a_session() {
        let mut tracker = Tracker::new(AgentStatus::Working);
        assert!(!poll(&mut tracker, "BUSY").process_ended);

        let mut tracker = Tracker::new(AgentStatus::Working);
        assert!(tracker.poll(&dead(Some(0)), None, &PATTERNS).process_ended);
    }

    #[test]
    fn a_dead_process_is_reported_even_when_the_status_does_not_change() {
        // A session already believed to be in error, whose process then dies:
        // nothing changes status, but the session is over and must be closed.
        let mut tracker = Tracker::new(AgentStatus::Error);

        let outcome = tracker.poll(&dead(Some(1)), None, &PATTERNS);

        assert!(!outcome.changed());
        assert!(outcome.process_ended);
    }

    #[test]
    fn a_pane_that_has_produced_nothing_is_not_re_read() {
        let mut tracker = Tracker::new(AgentStatus::Working);
        assert!(
            !tracker.is_unchanged_since(Some(10)),
            "nothing to compare to"
        );

        tracker.saw_activity(Some(10));
        assert!(tracker.is_unchanged_since(Some(10)));
        assert!(!tracker.is_unchanged_since(Some(11)), "it produced output");
        assert!(!tracker.is_unchanged_since(None), "no marker, no skipping");
    }

    #[test]
    fn a_reading_awaiting_confirmation_is_never_skipped() {
        let mut tracker = Tracker::new(AgentStatus::Working);
        tracker.saw_activity(Some(10));
        assert!(tracker.is_unchanged_since(Some(10)));

        // A question appears, and the screen then stops moving.
        poll(&mut tracker, "ASKING");

        assert!(
            !tracker.is_unchanged_since(Some(10)),
            "the pending reading still needs its confirming poll"
        );
    }

    #[test]
    fn a_skipped_poll_neither_changes_nor_ages_anything() {
        let mut tracker = Tracker::new(AgentStatus::Working);

        for _ in 0..STALENESS_CAP * 2 {
            let outcome = tracker.skipped();
            assert_eq!(outcome.status, AgentStatus::Working);
            assert!(!outcome.changed());
        }
        // The staleness clock never advanced, so one unrecognised poll is not
        // suddenly the last straw.
        assert!(!poll(&mut tracker, "unrecognised").changed());
    }

    #[test]
    fn a_dead_process_beats_whatever_is_on_screen() {
        let mut tracker = Tracker::new(AgentStatus::Waiting);

        // The confirmation dialog is still on screen, but nobody is asking.
        let outcome = tracker.poll(&dead(Some(0)), Some("ASKING"), &PATTERNS);

        assert_eq!(outcome.status, AgentStatus::Stopped);
        assert_eq!(outcome.changed_from, Some(AgentStatus::Waiting));
    }

    #[test]
    fn how_a_process_died_decides_between_stopped_and_error() {
        for (code, expected) in [
            (Some(0), AgentStatus::Stopped),
            (None, AgentStatus::Stopped),
            (Some(1), AgentStatus::Error),
            (Some(130), AgentStatus::Error),
        ] {
            let mut tracker = Tracker::new(AgentStatus::Working);
            assert_eq!(
                tracker.poll(&dead(code), None, &PATTERNS).status,
                expected,
                "exit {code:?}"
            );
        }
    }

    #[test]
    fn a_poll_with_no_capture_is_treated_as_nothing_recognised() {
        let mut tracker = Tracker::new(AgentStatus::Working);

        let outcome = tracker.poll(&alive(), None, &PATTERNS);

        assert_eq!(outcome.status, AgentStatus::Working);
        assert!(!outcome.changed());
    }
}
