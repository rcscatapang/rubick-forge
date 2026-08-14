import { useState } from "react";
import { toast } from "sonner";

import { isLive, type Task } from "@/lib/api-types";
import { describe } from "@/lib/errors";
import { useGitHubActions, useTaskDiff } from "@/lib/queries";

/**
 * Commit a task's worktree, and offer to open a pull request for it.
 *
 * Shows what would be committed before committing it: this stages everything
 * in the worktree, and "everything" is only a safe default if you can see it.
 */
export function CommitDialog({ task, onClose }: { task: Task; onClose: () => void }) {
  const diff = useTaskDiff(task.id, true);
  const actions = useGitHubActions();
  const [message, setMessage] = useState(task.title);
  const [committed, setCommitted] = useState(false);

  const stat = diff.data;
  const nothingToCommit = stat !== undefined && stat.files === 0;
  const busy = actions.commit.isPending || actions.openPull.isPending;

  const commit = async () => {
    try {
      await actions.commit.mutateAsync({ id: task.id, message });
      setCommitted(true);
      toast.success("Committed.");
    } catch (error) {
      toast.error(describe(error));
    }
  };

  const openPull = async () => {
    try {
      const pull = await actions.openPull.mutateAsync(task.id);
      toast.success(`Opened PR #${pull.number}.`);
      onClose();
    } catch (error) {
      toast.error(describe(error));
    }
  };

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-border p-3 text-xs">
      <div className="flex items-baseline justify-between">
        <h4 className="text-sm font-medium">Commit “{task.title}”</h4>
        <button type="button" className="text-muted-foreground" onClick={onClose}>
          Close
        </button>
      </div>

      {/* An agent mid-edit may be part-way through a change; committing under
          it captures whatever half of it is on disk. */}
      {isLive(task.status) && task.status === "working" && (
        <p className="rounded border border-[var(--status-waiting)] p-2 text-[var(--status-waiting)]">
          This agent is still working. Committing now captures whatever it has
          written so far.
        </p>
      )}

      {diff.isLoading && <p className="text-muted-foreground">Looking at the worktree…</p>}
      {diff.isError && <p className="text-[var(--status-error)]">{describe(diff.error)}</p>}

      {stat && (
        <div className="flex flex-col gap-1">
          <p className="text-muted-foreground">
            {stat.files === 0
              ? "Nothing has changed in this worktree."
              : `${stat.files} file${stat.files === 1 ? "" : "s"}, +${stat.insertions} −${stat.deletions}`}
          </p>
          {stat.paths.length > 0 && (
            <ul className="max-h-40 overflow-y-auto font-mono text-[11px] text-muted-foreground">
              {stat.paths.map((path) => (
                <li key={path} className="truncate">
                  {path}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <label className="flex flex-col gap-1">
        <span className="text-muted-foreground">Message</span>
        <textarea
          className="min-h-16 w-full rounded border border-border bg-transparent px-2 py-1"
          value={message}
          onChange={(event) => setMessage(event.target.value)}
        />
      </label>

      <div className="flex gap-1.5">
        <button
          type="button"
          className="rounded bg-primary px-2 py-1 text-primary-foreground disabled:opacity-60"
          disabled={busy || nothingToCommit || committed || !message.trim()}
          onClick={() => void commit()}
        >
          {actions.commit.isPending ? "Committing…" : "Commit"}
        </button>

        <button
          type="button"
          className="rounded border border-border px-2 py-1 disabled:opacity-60"
          disabled={busy}
          title="Pushes the branch and opens a pull request against the base branch"
          onClick={() => void openPull()}
        >
          {actions.openPull.isPending ? "Opening…" : "Push and open PR"}
        </button>
      </div>
    </div>
  );
}
