import { useState } from "react";
import { toast } from "sonner";

import type { Project, QueuedTask } from "@/lib/api-types";
import { describe } from "@/lib/errors";
import { useHub, useHubActions, useQueue } from "@/lib/queries";

/**
 * The hub's queue: what is waiting, and where each row went.
 *
 * Only rendered on the machine that is the hub. Every other machine's queue is
 * empty by construction, and a section saying so on each of them would be
 * noise.
 */
export function HubQueue({ projects }: { projects: Project[] }) {
  const hub = useHub();
  const queue = useQueue(hub.data?.hub ?? false);
  const actions = useHubActions();

  const [project, setProject] = useState("");
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [target, setTarget] = useState("");

  if (!hub.data?.hub) return null;

  // A hub whose own daemon is unreachable is a real state: the queue features
  // pause and everything else on the dashboard carries on.
  if (queue.isError) {
    return (
      <section className="flex flex-col gap-2 text-xs">
        <h2 className="text-sm font-semibold">Queue</h2>
        <p className="text-[var(--status-error)]">
          The hub is not answering, so the queue is unavailable. Machines can
          still be driven directly.
        </p>
      </section>
    );
  }

  const enqueue = async () => {
    try {
      await actions.enqueue.mutateAsync({
        project_name: project.trim(),
        title: title.trim(),
        prompt: prompt.trim() || undefined,
        target: target || undefined,
      });
      setTitle("");
      setPrompt("");
      toast.success("Queued.");
    } catch (error) {
      toast.error(describe(error));
    }
  };

  const cancel = async (id: number) => {
    try {
      await actions.cancel.mutateAsync(id);
    } catch (error) {
      toast.error(describe(error));
    }
  };

  const rows = [...(queue.data?.queue ?? [])].reverse();
  const names = [...new Set(projects.map((each) => each.name))];

  return (
    <section className="flex flex-col gap-3 text-xs">
      <h2 className="text-sm font-semibold">Queue</h2>
      <p className="text-muted-foreground">
        Queued work goes to whichever Mac has that project registered. Nothing
        is cloned anywhere — a project has to exist on a machine before work can
        land there.
      </p>

      <div className="flex flex-col gap-2 rounded-lg border border-border p-3">
        <div className="flex flex-wrap gap-1.5">
          <input
            className="min-w-32 flex-1 rounded border border-border bg-transparent px-2 py-1"
            placeholder="Project name"
            list="hub-projects"
            value={project}
            onChange={(event) => setProject(event.target.value)}
          />
          <datalist id="hub-projects">
            {names.map((name) => (
              <option key={name} value={name} />
            ))}
          </datalist>

          <select
            className="rounded border border-border bg-transparent px-2 py-1"
            value={target}
            onChange={(event) => setTarget(event.target.value)}
          >
            <option value="">Any machine</option>
            {(hub.data?.machines ?? []).map((machine) => (
              <option key={machine} value={machine}>
                {machine}
              </option>
            ))}
          </select>
        </div>

        <input
          className="w-full rounded border border-border bg-transparent px-2 py-1"
          placeholder="Title"
          value={title}
          onChange={(event) => setTitle(event.target.value)}
        />
        <textarea
          className="min-h-14 w-full rounded border border-border bg-transparent px-2 py-1"
          placeholder="What should the agent do?"
          value={prompt}
          onChange={(event) => setPrompt(event.target.value)}
        />

        <button
          type="button"
          className="self-start rounded bg-primary px-2 py-1 text-primary-foreground disabled:opacity-60"
          disabled={!project.trim() || !title.trim() || actions.enqueue.isPending}
          onClick={() => void enqueue()}
        >
          {actions.enqueue.isPending ? "Queueing…" : "Add to queue"}
        </button>
      </div>

      {rows.length === 0 ? (
        <p className="text-muted-foreground">Nothing on the queue.</p>
      ) : (
        <ul className="flex flex-col gap-1">
          {rows.map((row) => (
            <QueueRow key={row.id} row={row} onCancel={() => void cancel(row.id)} />
          ))}
        </ul>
      )}
    </section>
  );
}

function QueueRow({ row, onCancel }: { row: QueuedTask; onCancel: () => void }) {
  return (
    <li className="flex items-baseline justify-between gap-2 rounded border border-border px-2 py-1">
      <span className="min-w-0">
        <span className="text-muted-foreground">#{row.id}</span> {row.title}
        <span className="text-muted-foreground"> · {row.project_name}</span>
        {row.state === "dispatched" && (
          <span className="text-muted-foreground">
            {" "}
            → {row.machine} (task {row.remote_task})
          </span>
        )}
        {row.state === "cancelled" && <span className="text-muted-foreground"> · cancelled</span>}
        {row.state === "queued" && row.reason && (
          <span className="block text-[var(--status-waiting)]">{row.reason}</span>
        )}
        {/* The audit trail: which machines could have taken it. Shown even for
            one, because "only this Mac could" is the answer to the question. */}
        {row.considered.length > 0 && (
          <span className="block text-muted-foreground">
            considered {row.considered.join(", ")}
          </span>
        )}
      </span>

      {row.state === "queued" && (
        <button
          type="button"
          className="shrink-0 rounded border border-border px-2 py-0.5"
          onClick={onCancel}
        >
          Cancel
        </button>
      )}
    </li>
  );
}
