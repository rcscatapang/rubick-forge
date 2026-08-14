import { useQueries } from "@tanstack/react-query";

import type { EventRecord } from "@/lib/api-types";
import { useMachines } from "@/lib/machine-registry";
import { machineEventsQuery } from "@/lib/queries";

/** What each event kind reads as in the feed. */
function describe(event: EventRecord): string {
  switch (event.kind) {
    case "project_registered":
      return `Registered ${event.name}`;
    case "project_removed":
      return `Removed project ${event.project_id}`;
    case "task_created":
      return `Created “${event.title}”`;
    case "task_deleted":
      return `Deleted task ${event.task_id}`;
    case "task_finished":
      return `Task ${event.task_id} finished`;
    case "session_started":
      return `Started ${event.tmux_name}`;
    case "session_stopped":
      return `Session ${event.session_id} ${event.reason}`;
    case "status_changed":
      return `Task ${event.task_id}: ${event.from} → ${event.to}`;
    case "agent_waiting":
      return `Task ${event.task_id} needs you`;
    case "agent_error":
      return `Task ${event.task_id}: ${event.detail}`;
    case "worktree_created":
      return `Worktree on ${event.branch}`;
    case "worktree_removed":
      return `Removed a worktree for task ${event.task_id}`;
    case "pr_opened":
      return `Opened PR #${event.number}`;
    case "pr_merged":
      return `PR #${event.number} merged`;
    case "pr_closed":
      return `PR #${event.number} closed without merging`;
    case "checks_passed":
      return `Checks passed on PR #${event.number}`;
    case "checks_failed":
      return `Checks failed on PR #${event.number}`;
  }
}

/** How many entries the merged feed shows, however many machines fed it. */
const FEED_LENGTH = 50;

/** One machine's event, kept together with where it came from. */
interface Entry {
  machine: string;
  /** Only shown when there is more than one machine to tell apart. */
  badge: string | null;
  event: EventRecord;
}

/**
 * What happened, across every machine, newest first.
 *
 * This is the one view that genuinely has to merge: everything else on the
 * dashboard belongs to one machine and is rendered under it. Ordering is by
 * the daemons' own timestamps, which are UTC — two Macs whose clocks disagree
 * will interleave by however much they disagree, and no ordering the app
 * invents would be more honest than that.
 */
export function ActivityFeed() {
  const { machines } = useMachines();
  const named = machines.length > 1;

  const feeds = useQueries({ queries: machines.map(machineEventsQuery) });

  const entries: Entry[] = feeds.flatMap((feed, index) => {
    const { machine } = machines[index];
    return (feed.data?.events ?? []).map((event) => ({
      machine: machine.id,
      badge: named ? machine.name : null,
      event,
    }));
  });

  entries.sort((a, b) => b.event.ts.localeCompare(a.event.ts) || b.event.id - a.event.id);

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-sm font-semibold">Activity</h2>

      {entries.length === 0 ? (
        <p className="text-xs text-muted-foreground">Nothing has happened yet.</p>
      ) : (
        <ol className="flex flex-col gap-1 text-xs">
          {entries.slice(0, FEED_LENGTH).map(({ machine, badge, event }) => (
            <li key={`${machine}:${event.id}`} className="flex gap-2 text-muted-foreground">
              <time dateTime={event.ts} className="tabular-nums">
                {new Date(event.ts).toLocaleTimeString()}
              </time>
              {badge && <span className="shrink-0 rounded bg-muted px-1">{badge}</span>}
              <span className="text-foreground">{describe(event)}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
