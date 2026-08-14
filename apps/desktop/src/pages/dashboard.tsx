import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";

import { ActivityFeed } from "@/components/activity-feed";
import { NewTaskForm } from "@/components/new-task";
import { TaskRow } from "@/components/task-row";
import { useEvents } from "@/hooks/use-events";
import { needsAttention, type Project, type Task } from "@/lib/api-types";
import { describe } from "@/lib/errors";
import { useHealth, useProjectActions, useProjects, useTasks } from "@/lib/queries";

/**
 * Everything at a glance: what needs you, what is running, and what happened.
 *
 * Nothing here polls. The daemon's event stream invalidates exactly what each
 * change touched, and these queries refetch themselves.
 */
export function DashboardPage() {
  useEvents();

  const health = useHealth();
  const projects = useProjects();
  const tasks = useTasks();
  const projectActions = useProjectActions();

  const all = tasks.data?.tasks ?? [];
  const attention = all.filter((task) => needsAttention(task.status));
  const byProject = new Map<number, Task[]>();
  for (const task of all) {
    byProject.set(task.project_id, [...(byProject.get(task.project_id) ?? []), task]);
  }

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
    <main className="mx-auto flex max-w-5xl flex-col gap-6 p-6">
      <header className="flex items-baseline justify-between gap-4">
        <h1 className="text-lg font-semibold tracking-tight">Rubick Forge</h1>
        <p className="text-xs text-muted-foreground">
          daemon {health.data?.version} on {health.data?.machine}
          {health.data?.binaries
            .filter((binary) => !binary.ok)
            .map((binary) => ` · ${binary.name} unavailable`)
            .join("")}
        </p>
      </header>

      {attention.length > 0 && (
        <section className="flex flex-col gap-2">
          <h2 className="text-sm font-semibold text-[var(--status-waiting)]">
            Needs you ({attention.length})
          </h2>
          <ul className="flex flex-col gap-2">
            {attention.map((task) => (
              <TaskRow key={task.id} task={task} />
            ))}
          </ul>
        </section>
      )}

      <NewTaskForm projects={projects.data?.projects ?? []} />

      <section className="flex flex-col gap-4">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold">Projects</h2>
          <button
            type="button"
            className="rounded-md border border-border px-2 py-1 text-xs"
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
            />
          ))
        )}
      </section>

      <ActivityFeed />
    </main>
  );
}

function ProjectGroup({ project, tasks }: { project: Project; tasks: Task[] }) {
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-baseline gap-2">
        <h3 className="text-sm font-medium">{project.name}</h3>
        <span className="truncate text-xs text-muted-foreground">
          {project.default_branch} · {project.path}
        </span>
      </div>

      {tasks.length === 0 ? (
        <p className="text-xs text-muted-foreground">No tasks yet.</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {tasks.map((task) => (
            <TaskRow key={task.id} task={task} />
          ))}
        </ul>
      )}
    </section>
  );
}
