import type { EventRecord } from "@/lib/api-types";
import { useActivity } from "@/lib/queries";

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
  }
}

export function ActivityFeed() {
  const activity = useActivity();
  // Newest first, though the daemon pages oldest-first.
  const events = [...(activity.data?.events ?? [])].reverse();

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-sm font-semibold">Activity</h2>

      {events.length === 0 ? (
        <p className="text-xs text-muted-foreground">Nothing has happened yet.</p>
      ) : (
        <ol className="flex flex-col gap-1 text-xs">
          {events.map((event) => (
            <li key={event.id} className="flex gap-2 text-muted-foreground">
              <time dateTime={event.ts} className="tabular-nums">
                {new Date(event.ts).toLocaleTimeString()}
              </time>
              <span className="text-foreground">{describe(event)}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
