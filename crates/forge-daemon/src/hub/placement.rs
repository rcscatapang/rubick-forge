//! Choosing which machine takes a queued task.
//!
//! Pure, and deliberately unambitious (SPEC D25): match by project name, prefer
//! the machine doing least, and never clone a repository onto a machine that
//! does not have it. There is no scheduler here and there is not meant to be.

use forge_core::QueuedTask;

/// What one machine can currently offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    /// Position in the fleet, for dispatching to it afterwards.
    pub index: usize,
    pub name: String,
    /// The projects it has, as (id, name). Matched by name — the same
    /// repository is checked out somewhere different on every Mac — but the id
    /// is what creating a task there needs, and it differs per machine.
    pub projects: Vec<(i64, String)>,
    /// How many sessions it is running, which is the whole placement heuristic.
    pub active: usize,
    /// False when the machine could not be asked. It is not a candidate, and
    /// that is worth saying rather than silently skipping.
    pub reachable: bool,
}

/// Where a queued row should go, or why it cannot go anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// Dispatch to this machine. `considered` is every machine that could have
    /// taken it, recorded so the choice is auditable afterwards.
    Send {
        index: usize,
        machine: String,
        /// The project's id *on that machine*.
        project_id: i64,
        considered: Vec<String>,
    },
    /// Leave it queued, and say why.
    Wait {
        reason: String,
        considered: Vec<String>,
    },
}

/// Decide where `row` goes, given what every machine currently has.
///
/// A named target is honoured or refused — never quietly redirected. Someone
/// who said "run this on the Mac mini" did not mean "run it anywhere".
pub fn place(row: &QueuedTask, fleet: &[Inventory]) -> Placement {
    if let Some(target) = row.target.as_deref() {
        return place_named(row, target, fleet);
    }

    let eligible: Vec<&Inventory> = fleet
        .iter()
        .filter(|machine| machine.reachable && machine.has(&row.project_name))
        .collect();

    let considered: Vec<String> = eligible.iter().map(|m| m.name.clone()).collect();

    // Fewest active sessions wins; ties go to the earlier machine so that a
    // fleet doing nothing at all places predictably rather than arbitrarily.
    match eligible.iter().min_by_key(|machine| machine.active) {
        Some(chosen) => Placement::Send {
            index: chosen.index,
            machine: chosen.name.clone(),
            project_id: chosen.project_id(&row.project_name).unwrap_or_default(),
            considered,
        },
        None => Placement::Wait {
            reason: unsatisfiable(&row.project_name, fleet),
            considered,
        },
    }
}

fn place_named(row: &QueuedTask, target: &str, fleet: &[Inventory]) -> Placement {
    let Some(machine) = fleet
        .iter()
        .find(|machine| machine.name.eq_ignore_ascii_case(target))
    else {
        return Placement::Wait {
            reason: format!("no machine called {target} is configured"),
            considered: Vec::new(),
        };
    };

    let considered = vec![machine.name.clone()];

    if !machine.reachable {
        return Placement::Wait {
            reason: format!("{} is not answering", machine.name),
            considered,
        };
    }
    let Some(project_id) = machine.project_id(&row.project_name) else {
        return Placement::Wait {
            reason: format!(
                "{} does not have a project called {}. Register it there first.",
                machine.name, row.project_name
            ),
            considered,
        };
    };

    Placement::Send {
        index: machine.index,
        machine: machine.name.clone(),
        project_id,
        considered,
    }
}

/// Why nothing could take this, in words that say what to do about it.
fn unsatisfiable(project: &str, fleet: &[Inventory]) -> String {
    let unreachable: Vec<&str> = fleet
        .iter()
        .filter(|machine| !machine.reachable)
        .map(|machine| machine.name.as_str())
        .collect();

    // A machine that is merely asleep may well have the project, so an "asleep"
    // answer is different from "nobody has it" and leads somewhere different.
    if !unreachable.is_empty() {
        return format!(
            "no machine that answered has a project called {project} \
             ({} not answering)",
            unreachable.join(", ")
        );
    }

    format!("no machine has a project called {project}. Register it on one.")
}

impl Inventory {
    fn has(&self, project: &str) -> bool {
        self.project_id(project).is_some()
    }

    /// The id `project` has here, which is not its id anywhere else.
    fn project_id(&self, project: &str) -> Option<i64> {
        self.projects
            .iter()
            .find(|(_, held)| held.eq_ignore_ascii_case(project))
            .map(|(id, _)| *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::{AdapterId, QueueState, Timestamp};

    fn row(project: &str, target: Option<&str>) -> QueuedTask {
        QueuedTask {
            id: 1,
            project_name: project.to_owned(),
            adapter: AdapterId::default(),
            title: "Fix the flaky test".into(),
            prompt: None,
            target: target.map(str::to_owned),
            state: QueueState::Queued,
            reason: None,
            machine: None,
            remote_task: None,
            considered: Vec::new(),
            created_at: Timestamp::now(),
            updated_at: Timestamp::now(),
        }
    }

    fn machine(index: usize, name: &str, projects: &[&str], active: usize) -> Inventory {
        Inventory {
            index,
            name: name.to_owned(),
            projects: projects
                .iter()
                .enumerate()
                .map(|(at, p)| (at as i64 + 1, (*p).to_owned()))
                .collect(),
            active,
            reachable: true,
        }
    }

    #[test]
    fn the_only_machine_with_the_project_takes_it() {
        let fleet = [
            machine(0, "mini", &["forge"], 0),
            machine(1, "laptop", &["other"], 0),
        ];

        assert_eq!(
            place(&row("forge", None), &fleet),
            Placement::Send {
                index: 0,
                machine: "mini".into(),
                project_id: 1,
                considered: vec!["mini".into()],
            }
        );
    }

    #[test]
    fn the_machine_doing_least_wins() {
        let fleet = [
            machine(0, "mini", &["forge"], 3),
            machine(1, "laptop", &["forge"], 1),
        ];

        let Placement::Send {
            machine,
            considered,
            ..
        } = place(&row("forge", None), &fleet)
        else {
            panic!("both have it");
        };

        assert_eq!(machine, "laptop");
        // Both are recorded, so the decision can be audited afterwards.
        assert_eq!(considered, ["mini", "laptop"]);
    }

    #[test]
    fn a_tie_goes_to_the_first_machine_rather_than_an_arbitrary_one() {
        let fleet = [
            machine(0, "mini", &["forge"], 2),
            machine(1, "laptop", &["forge"], 2),
        ];

        let Placement::Send { machine, .. } = place(&row("forge", None), &fleet) else {
            panic!("both have it");
        };

        assert_eq!(machine, "mini");
    }

    #[test]
    fn nothing_takes_a_project_nobody_has() {
        let fleet = [machine(0, "mini", &["other"], 0)];

        let Placement::Wait { reason, considered } = place(&row("forge", None), &fleet) else {
            panic!("nobody has forge");
        };

        assert!(
            reason.contains("no machine has a project called forge"),
            "{reason}"
        );
        assert!(reason.contains("Register it"), "{reason}");
        assert!(considered.is_empty());
    }

    #[test]
    fn a_machine_that_is_asleep_is_not_a_candidate_but_is_worth_mentioning() {
        // "Asleep" and "nobody has it" lead somewhere different.
        let fleet = [Inventory {
            reachable: false,
            ..machine(0, "mini", &["forge"], 0)
        }];

        let Placement::Wait { reason, .. } = place(&row("forge", None), &fleet) else {
            panic!("the only machine is unreachable");
        };

        assert!(reason.contains("mini not answering"), "{reason}");
        assert!(!reason.contains("Register it"), "{reason}");
    }

    #[test]
    fn an_unreachable_machine_does_not_take_work_from_a_reachable_one() {
        let fleet = [
            Inventory {
                reachable: false,
                ..machine(0, "mini", &["forge"], 0)
            },
            machine(1, "laptop", &["forge"], 5),
        ];

        let Placement::Send { machine, .. } = place(&row("forge", None), &fleet) else {
            panic!("laptop can take it");
        };

        assert_eq!(machine, "laptop", "even though it is busier");
    }

    #[test]
    fn a_named_machine_is_honoured_rather_than_optimised() {
        // Someone who said "on the Mac mini" did not mean "anywhere".
        let fleet = [
            machine(0, "mini", &["forge"], 9),
            machine(1, "laptop", &["forge"], 0),
        ];

        let Placement::Send { machine, .. } = place(&row("forge", Some("mini")), &fleet) else {
            panic!("mini has it");
        };

        assert_eq!(machine, "mini");
    }

    #[test]
    fn a_named_machine_without_the_project_is_refused_not_redirected() {
        let fleet = [
            machine(0, "mini", &["other"], 0),
            machine(1, "laptop", &["forge"], 0),
        ];

        let Placement::Wait { reason, .. } = place(&row("forge", Some("mini")), &fleet) else {
            panic!("mini does not have forge");
        };

        assert!(reason.contains("mini does not have"), "{reason}");
        assert!(reason.contains("Register it there"), "{reason}");
    }

    #[test]
    fn a_named_machine_that_is_not_configured_says_so() {
        let fleet = [machine(0, "mini", &["forge"], 0)];

        let Placement::Wait { reason, .. } = place(&row("forge", Some("desktop")), &fleet) else {
            panic!("there is no desktop");
        };

        assert!(reason.contains("no machine called desktop"), "{reason}");
    }

    #[test]
    fn a_named_machine_that_is_asleep_waits_for_it() {
        let fleet = [Inventory {
            reachable: false,
            ..machine(0, "mini", &["forge"], 0)
        }];

        let Placement::Wait { reason, .. } = place(&row("forge", Some("mini")), &fleet) else {
            panic!("mini is unreachable");
        };

        assert!(reason.contains("mini is not answering"), "{reason}");
    }

    #[test]
    fn project_and_machine_names_match_without_regard_to_case() {
        let fleet = [machine(0, "Mac mini", &["Forge"], 0)];

        assert!(matches!(
            place(&row("forge", Some("mac MINI")), &fleet),
            Placement::Send { .. }
        ));
    }

    #[test]
    fn an_empty_fleet_places_nothing() {
        let Placement::Wait { reason, .. } = place(&row("forge", None), &[]) else {
            panic!("there is nowhere to send it");
        };

        assert!(reason.contains("no machine has"), "{reason}");
    }
}
