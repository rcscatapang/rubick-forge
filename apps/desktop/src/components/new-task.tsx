import { useState } from "react";
import { toast } from "sonner";

import { ADAPTER_LABELS, type AdapterId, type Project } from "@/lib/api-types";
import { describe } from "@/lib/errors";
import { useAdapters, useTaskActions } from "@/lib/queries";

/**
 * The product's front door: a title, a prompt, and where to run it.
 *
 * Defaults are the answer most of the time — the project's own branch, its own
 * worktree, started immediately — so the only thing that has to be typed is
 * what the agent should do.
 */
export function NewTaskForm({
  projects,
  onCreated,
}: {
  projects: Project[];
  onCreated?: (taskId: number) => void;
}) {
  const adapters = useAdapters();
  const actions = useTaskActions();

  const [projectId, setProjectId] = useState<number | "">(projects[0]?.id ?? "");
  const [adapter, setAdapter] = useState<AdapterId>("claude-code");
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [baseBranch, setBaseBranch] = useState("");
  const [useWorktree, setUseWorktree] = useState(true);
  const [startNow, setStartNow] = useState(true);

  const project = projects.find((candidate) => candidate.id === projectId);
  const chosen = adapters.data?.adapters.find((entry) => entry.id === adapter);

  // What the daemon would refuse anyway, said before the round trip.
  const problem =
    projects.length === 0
      ? "Register a project first."
      : !project
        ? "Choose a project."
        : title.trim() === ""
          ? "Give the task a title."
          : chosen && !chosen.binary.ok
            ? (chosen.binary.detail ?? `${ADAPTER_LABELS[adapter]} is not installed.`)
            : null;

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (problem || !project) return;

    try {
      const task = await actions.create.mutateAsync({
        project_id: project.id,
        title: title.trim(),
        adapter,
        base_branch: baseBranch.trim() || undefined,
        initial_prompt: prompt.trim() || undefined,
        use_worktree: useWorktree,
      });

      if (startNow) {
        await actions.start.mutateAsync(task.id);
      }

      setTitle("");
      setPrompt("");
      toast.success(`Created “${task.title}”.`);
      onCreated?.(task.id);
    } catch (error) {
      toast.error(describe(error));
    }
  };

  const busy = actions.create.isPending || actions.start.isPending;

  return (
    <form onSubmit={submit} className="flex flex-col gap-3 rounded-lg border border-border p-4">
      <h2 className="text-sm font-semibold">New task</h2>

      <div className="flex flex-wrap gap-2">
        <label className="flex flex-col gap-1 text-xs text-muted-foreground">
          Project
          <select
            className="rounded-md border border-border bg-card px-2 py-1 text-sm text-foreground"
            value={projectId}
            onChange={(event) => setProjectId(Number(event.target.value))}
          >
            {projects.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                {candidate.name}
              </option>
            ))}
          </select>
        </label>

        <label className="flex flex-col gap-1 text-xs text-muted-foreground">
          Agent
          <select
            className="rounded-md border border-border bg-card px-2 py-1 text-sm text-foreground"
            value={adapter}
            onChange={(event) => setAdapter(event.target.value as AdapterId)}
          >
            {(adapters.data?.adapters ?? []).map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.name}
                {entry.binary.ok ? "" : " (not installed)"}
              </option>
            ))}
          </select>
        </label>

        <label className="flex flex-col gap-1 text-xs text-muted-foreground">
          Base branch
          <input
            className="rounded-md border border-border bg-card px-2 py-1 text-sm text-foreground"
            placeholder={project?.default_branch ?? "main"}
            value={baseBranch}
            onChange={(event) => setBaseBranch(event.target.value)}
          />
        </label>
      </div>

      <input
        className="rounded-md border border-border bg-card px-2 py-1.5 text-sm text-foreground"
        placeholder="What should the agent do?"
        aria-label="Title"
        value={title}
        onChange={(event) => setTitle(event.target.value)}
      />

      <textarea
        className="min-h-16 rounded-md border border-border bg-card px-2 py-1.5 text-sm text-foreground"
        placeholder="Opening instructions (optional)"
        aria-label="Initial prompt"
        value={prompt}
        onChange={(event) => setPrompt(event.target.value)}
      />

      <div className="flex flex-wrap items-center gap-4 text-xs text-muted-foreground">
        <label className="flex items-center gap-1.5">
          <input
            type="checkbox"
            checked={useWorktree}
            onChange={(event) => setUseWorktree(event.target.checked)}
          />
          Its own worktree
        </label>
        <label className="flex items-center gap-1.5">
          <input
            type="checkbox"
            checked={startNow}
            onChange={(event) => setStartNow(event.target.checked)}
          />
          Start straight away
        </label>
      </div>

      {problem && <p className="text-xs text-[var(--status-waiting)]">{problem}</p>}

      <button
        type="submit"
        disabled={busy || problem !== null}
        className="self-start rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground disabled:opacity-50"
      >
        {busy ? "Creating…" : "Create task"}
      </button>
    </form>
  );
}
