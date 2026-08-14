import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";

import { ActivityFeed } from "@/components/activity-feed";
import { MachineChip } from "@/components/machine-chip";
import { MachineSettings } from "@/components/machine-settings";
import { NotificationSettings } from "@/components/notification-settings";
import { NewTaskForm } from "@/components/new-task";
import { TaskRow } from "@/components/task-row";
import { ApiError } from "@/lib/api";
import { needsAttention, type Project, type Task } from "@/lib/api-types";
import { DaemonProvider } from "@/lib/connection";
import { describe } from "@/lib/errors";
import { useMachines } from "@/lib/machine-registry";
import { isLocal, type Machine } from "@/lib/machines";
import { useProjectActions, useProjects, useTasks } from "@/lib/queries";

/**
 * Everything at a glance: what needs you, what is running, and what happened.
 *
 * Each machine gets its own subtree with its own daemon, so a Mac that is
 * asleep costs its own section and nothing else. Nothing here polls except the
 * health check: the daemons say what changed and the affected queries refetch.
 */
export function DashboardPage() {
  const { machines } = useMachines();

  return (
    <main className="mx-auto flex max-w-5xl flex-col gap-8 p-6">
      <header className="flex items-baseline justify-between gap-4">
        <h1 className="text-lg font-semibold tracking-tight">Rubick Forge</h1>
      </header>

      {machines.map(({ machine, connection }) => (
        <DaemonProvider key={machine.id} machineId={machine.id} connection={connection}>
          <MachineBoard machine={machine} alone={machines.length === 1} />
        </DaemonProvider>
      ))}

      <ActivityFeed />
      <MachineSettings />
      <NotificationSettings />
    </main>
  );
}

/**
 * One machine's projects and tasks.
 *
 * With a single machine the heading would be a label on the only thing there
 * is, so it is left off — a v0.1 install looks exactly as it did.
 */
function MachineBoard({ machine, alone }: { machine: Machine; alone: boolean }) {
  const projects = useProjects();
  const tasks = useTasks();
  const projectActions = useProjectActions();

  const all = tasks.data?.tasks ?? [];
  const attention = all.filter((task) => needsAttention(task.status));
  const byProject = new Map<number, Task[]>();
  for (const task of all) {
    byProject.set(task.project_id, [...(byProject.get(task.project_id) ?? []), task]);
  }

  // `/health` is unauthenticated, so a machine with the wrong token answers
  // the chip happily and then refuses everything else. Say so once, here,
  // rather than rendering an empty section that looks like an idle Mac.
  const refused =
    tasks.error instanceof ApiError && tasks.error.status === 401 ? tasks.error : null;

  const register = async () => {
    const path = await open({ title: "Which repository?", directory: true, multiple: false });
    if (typeof path !== "string") return;

    try {
      const project = await projectActions.register.mutateAsync(path);
      toast.success(`Registered ${project.name}.`);
    } catch (error) {
      toast.error(describe(error));
    }
  };

  return (
    <section className="flex flex-col gap-6">
      <div className="flex flex-wrap items-baseline justify-between gap-2 border-b border-border pb-2">
        {alone ? <span /> : <h2 className="text-sm font-semibold">{machine.name}</h2>}
        <MachineChip />
      </div>

      {refused && (
        <p className="rounded-lg border border-[var(--status-error)] p-3 text-xs text-[var(--status-error)]">
          This machine rejected the app&rsquo;s token. Edit it under Machines and paste the
          token from its own <code>~/Library/Application Support/rubick-forge/token</code>.
        </p>
      )}

      {attention.length > 0 && (
        <section className="flex flex-col gap-2">
          <h3 className="text-sm font-semibold text-[var(--status-waiting)]">
            Needs you ({attention.length})
          </h3>
          <ul className="flex flex-col gap-2">
            {attention.map((task) => (
              <TaskRow key={task.id} task={task} machine={machine} />
            ))}
          </ul>
        </section>
      )}

      <NewTaskForm projects={projects.data?.projects ?? []} />

      <section className="flex flex-col gap-4">
        <div className="flex items-center justify-between">
          <h3 className="text-sm font-semibold">Projects</h3>
          <button
            type="button"
            className="rounded-md border border-border px-2 py-1 text-xs disabled:opacity-50"
            // A folder picker on this Mac cannot choose a path on another one.
            disabled={!isLocal(machine)}
            title={isLocal(machine) ? undefined : "Register repositories on that Mac itself"}
            onClick={() => void register()}
          >
            Register a repository…
          </button>
        </div>

        {(projects.data?.projects ?? []).length === 0 ? (
          <p className="text-xs text-muted-foreground">
            No repositories yet. Register one to give an agent somewhere to work.
          </p>
        ) : (
          (projects.data?.projects ?? []).map((project) => (
            <ProjectGroup
              key={project.id}
              project={project}
              tasks={byProject.get(project.id) ?? []}
              machine={machine}
            />
          ))
        )}
      </section>
    </section>
  );
}

function ProjectGroup({
  project,
  tasks,
  machine,
}: {
  project: Project;
  tasks: Task[];
  machine: Machine;
}) {
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-baseline gap-2">
        <h4 className="text-sm font-medium">{project.name}</h4>
        <span className="truncate text-xs text-muted-foreground">
          {project.default_branch} · {project.path}
        </span>
      </div>

      {tasks.length === 0 ? (
        <p className="text-xs text-muted-foreground">No tasks yet.</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {tasks.map((task) => (
            <TaskRow key={task.id} task={task} machine={machine} />
          ))}
        </ul>
      )}
    </section>
  );
}
