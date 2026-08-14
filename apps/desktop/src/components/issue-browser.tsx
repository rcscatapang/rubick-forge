import { useState } from "react";
import { toast } from "sonner";

import type { Project } from "@/lib/api-types";
import { describe } from "@/lib/errors";
import { useGitHubActions, useProjectIssues, useProjectRepo } from "@/lib/queries";

/**
 * Start a task from an open issue.
 *
 * Issues are fetched only when the list is opened: they cost a request each
 * time, and most sessions never look at them.
 */
export function IssueBrowser({ project }: { project: Project }) {
  const repo = useProjectRepo(project.id);
  const [open, setOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const issues = useProjectIssues(project.id, open);
  const actions = useGitHubActions();

  // A project that is not on GitHub shows no GitHub affordances at all.
  if (!repo.data?.repo) return null;

  const matching = (issues.data?.issues ?? []).filter((issue) => {
    const needle = filter.trim().toLowerCase();
    return (
      needle === "" ||
      issue.title.toLowerCase().includes(needle) ||
      String(issue.number) === needle
    );
  });

  const start = async (number: number) => {
    try {
      const task = await actions.fromIssue.mutateAsync({ project_id: project.id, number });
      toast.success(`Started “${task.title}”.`);
      setOpen(false);
    } catch (error) {
      toast.error(describe(error));
    }
  };

  return (
    <div className="flex flex-col gap-2 text-xs">
      <button
        type="button"
        className="self-start rounded border border-border px-2 py-1"
        onClick={() => setOpen((shown) => !shown)}
      >
        {open ? "Hide issues" : `Issues on ${repo.data.repo}…`}
      </button>

      {open && (
        <div className="flex flex-col gap-2 rounded-lg border border-border p-3">
          <input
            className="w-full rounded border border-border bg-transparent px-2 py-1"
            placeholder="Filter by title or number"
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />

          {issues.isLoading && <p className="text-muted-foreground">Asking GitHub…</p>}
          {issues.isError && (
            <p className="text-[var(--status-error)]">{describe(issues.error)}</p>
          )}
          {issues.isSuccess && matching.length === 0 && (
            <p className="text-muted-foreground">No open issues match.</p>
          )}

          <ul className="flex max-h-64 flex-col gap-1 overflow-y-auto">
            {matching.map((issue) => (
              <li key={issue.number} className="flex items-center justify-between gap-2">
                <span className="truncate">
                  <span className="text-muted-foreground">#{issue.number}</span>{" "}
                  {issue.title}
                </span>
                <button
                  type="button"
                  className="shrink-0 rounded border border-border px-2 py-1 disabled:opacity-60"
                  disabled={actions.fromIssue.isPending}
                  onClick={() => void start(issue.number)}
                >
                  Start task
                </button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
