//! Turning queued rows into real tasks on other Macs.
//!
//! The loop is deliberately small: read the queue, ask every machine what it
//! has, place each row, create it there. Nothing here retries cleverly — a row
//! that cannot be placed stays queued with a reason and is tried again next
//! time round.

use std::sync::Arc;
use std::time::Duration;

use forge_core::{ForgeEvent, QueuedTask};

use super::placement::{place, Inventory, Placement};
use crate::fleet::{Fleet, NewRemoteTask};
use crate::http::AppState;

/// How often the queue is looked at.
///
/// Slow on purpose: a queue that could not be placed a second ago will not be
/// placeable now, and each pass asks every machine two questions.
pub const INTERVAL: Duration = Duration::from_secs(15);

/// Ask every machine what it has and what it is doing.
///
/// A machine that cannot be reached comes back unreachable rather than absent,
/// so a row waiting on it can say so instead of reading as unplaceable.
pub async fn take_inventory(fleet: &Fleet) -> Vec<Inventory> {
    let mut inventory = Vec::with_capacity(fleet.len());

    for (index, machine) in fleet.each() {
        let projects = machine.projects().await;
        let tasks = machine.tasks().await;

        inventory.push(match (projects, tasks) {
            (Ok(projects), Ok(tasks)) => Inventory {
                index,
                name: machine.name().to_owned(),
                projects: projects
                    .into_iter()
                    .map(|project| (project.id, project.name))
                    .collect(),
                active: tasks
                    .iter()
                    .filter(|found| found.task.status.is_live())
                    .count(),
                reachable: true,
            },
            _ => Inventory {
                index,
                name: machine.name().to_owned(),
                projects: Vec::new(),
                active: 0,
                reachable: false,
            },
        });
    }

    inventory
}

/// Place and dispatch the queue until the daemon stops.
pub async fn run(state: AppState, fleet: Arc<Fleet>) {
    loop {
        if let Err(error) = pass(&state, &fleet).await {
            tracing::warn!(%error, "a dispatch pass failed");
        }

        tokio::time::sleep(INTERVAL).await;
    }
}

/// One pass over everything still queued.
///
/// Public so a test can drive a whole pass rather than its pieces, which is
/// where the interesting behaviour — placing, creating, claiming — actually is.
pub async fn pass(state: &AppState, fleet: &Fleet) -> Result<(), crate::store::StoreError> {
    let pending = state.store.pending_queue()?;
    if pending.is_empty() {
        return Ok(());
    }

    // Taken once per pass, not per row: it is two requests per machine, and a
    // fleet's inventory does not change between two rows of the same queue.
    let inventory = take_inventory(fleet).await;

    for row in pending {
        dispatch_one(state, fleet, &row, &inventory).await;
    }

    Ok(())
}

async fn dispatch_one(state: &AppState, fleet: &Fleet, row: &QueuedTask, inventory: &[Inventory]) {
    let (index, machine, project_id, considered) = match place(row, inventory) {
        Placement::Send {
            index,
            machine,
            project_id,
            considered,
        } => (index, machine, project_id, considered),
        Placement::Wait { reason, considered } => {
            // Recorded so the queue can explain itself, but not published: a
            // row that stays unplaceable would otherwise emit an event every
            // fifteen seconds forever. Written every pass rather than only on
            // a changed reason, so the audit list keeps up with a fleet whose
            // machines come and go.
            if let Err(error) = state.store.set_queue_reason(row.id, &reason, &considered) {
                tracing::warn!(queued = row.id, %error, "cannot record why a row is waiting");
            }
            return;
        }
    };

    let Some(target) = fleet.at(index) else {
        // The fleet changed under the pass. Recorded rather than silent, or the
        // row would sit there with no explanation at all.
        let _ = state.store.set_queue_reason(
            row.id,
            "the chosen machine is no longer configured",
            &considered,
        );
        return;
    };

    // The queue row names the attempt, so a retry after a crash between
    // creating the task and recording it is answered with the first task
    // rather than making a second one.
    let attempt = NewRemoteTask {
        project_id,
        title: row.title.clone(),
        prompt: row.prompt.clone().unwrap_or_default(),
        adapter: row.adapter,
        idempotency_key: Some(format!("forge-queue-{}", row.id)),
    };

    match target.start(attempt).await {
        Ok(task) => {
            // The row is claimed conditionally, so a duplicate created by a
            // crash between these two steps is detectable rather than silent.
            match state
                .store
                .mark_dispatched(row.id, &machine, task.id, &considered)
            {
                Ok(true) => {
                    let _ = state.bus.publish(ForgeEvent::TaskDispatched {
                        queued_id: row.id,
                        machine,
                        remote_task: task.id,
                        considered,
                    });
                }
                // Something else claimed it first. The task now exists twice,
                // which is worth a loud log: the hub cannot un-create it.
                Ok(false) => tracing::error!(
                    queued = row.id,
                    machine,
                    remote_task = task.id,
                    "dispatched a row that was already dispatched; \
                     this task needs deleting by hand"
                ),
                Err(error) => tracing::error!(
                    queued = row.id,
                    %error,
                    "cannot record a dispatch that already happened"
                ),
            }
        }
        Err(error) => {
            let detail = error.to_string();
            let _ = state.store.set_queue_reason(row.id, &detail, &considered);

            let _ = state.bus.publish(ForgeEvent::DispatchFailed {
                queued_id: row.id,
                machine,
                detail,
            });
        }
    }
}
