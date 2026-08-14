import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { openPath } from "@tauri-apps/plugin-opener";
import { Link } from "react-router";
import { toast } from "sonner";

import { CommitDialog } from "@/components/commit-dialog";
import { GitHubChip } from "@/components/github-chip";
import { StatusBadge } from "@/components/status-badge";
import { isLive, type Task, type TaskGitHub } from "@/lib/api-types";
import { useMachineId } from "@/lib/connection";
import { describe } from "@/lib/errors";
import { isLocal, type Machine } from "@/lib/machines";
import { useSessions, useTaskActions, useTaskGit } from "@/lib/queries";

/**
 * One task, with everything you can do to it from here.
 *
 * Actions are disabled by state rather than hidden, so the row does not
 * reshuffle itself every time an agent changes what it is doing.
 */
export function TaskRow({
  task,
  machine,
  github,
}: {
  task: Task;
  machine?: Machine;
  /** This task's GitHub link, when the project is on GitHub. */
  github?: TaskGitHub;
}) {
  const machineId = useMachineId();
  const actions = useTaskActions();
  const [committing, setCommitting] = useState(false);
  const git = useTaskGit(task.id);
  const sessions = useSessions(task.id);

  const live = isLive(task.status);
  const session = sessions.data?.sessions.find((candidate) => candidate.ended_at === null);

  const run = async (what: string, action: Promise<unknown>) => {
    try {
      await action;
    } catch (error) {
      toast.error(`${what} failed: ${describe(error)}`);
    }
  };

  return (
    <li className="flex flex-col gap-2 rounded-lg border border-border p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="truncate text-sm font-medium">{task.title}</p>
          <p className="truncate text-xs text-muted-foreground">
            {task.adapter} · {task.branch}
            {git.data?.dirty && " · uncommitted changes"}
            {git.data?.ahead ? ` · ${git.data.ahead} to push` : ""}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <GitHubChip link={github} />
          <StatusBadge status={task.status} />
        </div>
      </div>

      <div className="flex flex-wrap gap-1.5 text-xs">
        {live ? (
          <>
            <button
              type="button"
              className="rounded border border-border px-2 py-1"
              onClick={() => void run("Stopping", actions.stop.mutateAsync(task.id))}
            >
              Stop
            </button>
            <button
              type="button"
              className="rounded border border-border px-2 py-1"
              onClick={() => void run("Restarting", actions.restart.mutateAsync(task.id))}
            >
              Restart
            </button>
          </>
        ) : (
          <button
            type="button"
            className="rounded border border-border px-2 py-1"
            onClick={() => void run("Starting", actions.start.mutateAsync(task.id))}
          >
            Start
          </button>
        )}

        {session && (
          <Link
            className="rounded border border-border px-2 py-1"
            to={`/machines/${machineId}/sessions/${session.id}`}
          >
            Terminal
          </Link>
        )}

        {/* SPEC D18: ssh is not a transport, only an escape hatch to a real
            terminal on the Mac the session is actually running on. */}
        {session && machine?.sshHost && (
          <button
            type="button"
            className="rounded border border-border px-2 py-1"
            onClick={() =>
              void run(
                "Opening a terminal",
                invoke("open_ssh_session", {
                  host: machine.sshHost,
                  tmuxName: session.tmux_name,
                }),
              )
            }
          >
            Terminal over ssh
          </button>
        )}

        {task.worktree_path && (
          <>
            <button
              type="button"
              className="rounded border border-border px-2 py-1 disabled:opacity-50"
              // A path on another Mac means nothing to this one's Finder.
              disabled={machine !== undefined && !isLocal(machine)}
              title={
                machine !== undefined && !isLocal(machine)
                  ? "The worktree is on another Mac"
                  : undefined
              }
              onClick={() => void openPath(task.worktree_path as string)}
            >
              Open folder
            </button>
            <button
              type="button"
              className="rounded border border-border px-2 py-1 disabled:opacity-50"
              disabled={live}
              title={live ? "Stop the task first" : undefined}
              onClick={() =>
                void run("Cleanup", actions.cleanup.mutateAsync({ id: task.id }))
              }
            >
              Clean up worktree
            </button>
          </>
        )}

        {/* Committing needs a worktree of its own to stage; a task in the
            repository root shares it with everything else. */}
        {task.worktree_path && isLocalMachine(machine) && (
          <button
            type="button"
            className="rounded border border-border px-2 py-1"
            onClick={() => setCommitting((open) => !open)}
          >
            {committing ? "Hide commit" : "Commit…"}
          </button>
        )}

        <button
          type="button"
          className="ml-auto rounded border border-border px-2 py-1 text-[var(--status-error)] disabled:opacity-50"
          disabled={live}
          title={live ? "Stop the task first" : undefined}
          onClick={() =>
            void run("Deleting", actions.remove.mutateAsync({ id: task.id, force: true }))
          }
        >
          Delete
        </button>
      </div>

      {committing && <CommitDialog task={task} onClose={() => setCommitting(false)} />}
    </li>
  );
}

/** Committing runs git in a worktree, which only this Mac's daemon can reach. */
function isLocalMachine(machine?: Machine): boolean {
  return machine === undefined || isLocal(machine);
}
