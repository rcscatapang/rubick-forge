import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";

import { client } from "@/lib/api";
import type { EventKind } from "@/lib/api-types";
import { useDaemon } from "@/lib/connection";

/**
 * Query keys, in one place.
 *
 * Every key starts with `forge` so a daemon's data can be dropped wholesale
 * when the connection changes.
 */
export const keys = {
  all: ["forge"] as const,
  health: () => [...keys.all, "health"] as const,
  adapters: () => [...keys.all, "adapters"] as const,
  projects: () => [...keys.all, "projects"] as const,
  tasks: () => [...keys.all, "tasks"] as const,
  taskGit: (id: number) => [...keys.all, "tasks", id, "git"] as const,
  sessions: (taskId: number) => [...keys.all, "tasks", taskId, "sessions"] as const,
  events: () => [...keys.all, "events"] as const,
};

function useClient() {
  return client(useDaemon());
}

export function useHealth() {
  const api = useClient();
  return useQuery({
    queryKey: keys.health(),
    queryFn: () => api.health(),
    // The one thing worth asking about without being told: it is how the app
    // notices the daemon went away.
    refetchInterval: 5_000,
    retry: false,
  });
}

export function useAdapters() {
  const api = useClient();
  return useQuery({ queryKey: keys.adapters(), queryFn: () => api.adapters() });
}

export function useProjects() {
  const api = useClient();
  return useQuery({ queryKey: keys.projects(), queryFn: () => api.projects() });
}

export function useTasks() {
  const api = useClient();
  return useQuery({ queryKey: keys.tasks(), queryFn: () => api.tasks() });
}

export function useTaskGit(id: number) {
  const api = useClient();
  return useQuery({ queryKey: keys.taskGit(id), queryFn: () => api.taskGit(id) });
}

export function useSessions(taskId: number) {
  const api = useClient();
  return useQuery({ queryKey: keys.sessions(taskId), queryFn: () => api.sessions(taskId) });
}

export function useActivity() {
  const api = useClient();
  return useQuery({
    queryKey: keys.events(),
    // The end of history, not its beginning: a feed that paged forward from
    // the first event ever recorded would freeze once there were more than a
    // page of them.
    queryFn: () => api.events({ limit: 50, newest: true }),
  });
}

/**
 * Refetch what a given event actually changed, and nothing else.
 *
 * The task list is invalidated exactly, not by prefix: `["forge","tasks"]` is
 * a prefix of every per-task key, so a prefix match would refetch every task's
 * git status and session list on every status change.
 */
export function invalidateFor(queries: QueryClient, kind: EventKind, taskId?: number | null) {
  void queries.invalidateQueries({ queryKey: keys.events() });

  // Removing a project takes its tasks with it.
  if (kind === "project_registered" || kind === "project_removed") {
    void queries.invalidateQueries({ queryKey: keys.projects() });
  }

  void queries.invalidateQueries({ queryKey: keys.tasks(), exact: true });

  // A task's git facts and session list both hang off its id.
  if (taskId != null) {
    void queries.invalidateQueries({ queryKey: [...keys.all, "tasks", taskId] });
  }
}

/** The actions the dashboard offers, each refreshing what it changed. */
export function useTaskActions() {
  const api = useClient();
  const queries = useQueryClient();

  // A mutation's own result is not waited for by the event stream, so the
  // screen updates as soon as the daemon answers.
  const refresh = () => {
    void queries.invalidateQueries({ queryKey: keys.tasks() });
    void queries.invalidateQueries({ queryKey: keys.events() });
  };

  return {
    start: useMutation({ mutationFn: (id: number) => api.start(id), onSuccess: refresh }),
    stop: useMutation({ mutationFn: (id: number) => api.stop(id), onSuccess: refresh }),
    restart: useMutation({ mutationFn: (id: number) => api.restart(id), onSuccess: refresh }),
    cleanup: useMutation({
      mutationFn: ({ id, force }: { id: number; force?: boolean }) =>
        api.cleanupWorktree(id, { force }),
      onSuccess: refresh,
    }),
    remove: useMutation({
      mutationFn: ({ id, force }: { id: number; force?: boolean }) =>
        api.deleteTask(id, force),
      onSuccess: refresh,
    }),
    create: useMutation({
      mutationFn: (body: Parameters<ReturnType<typeof client>["createTask"]>[0]) =>
        api.createTask(body),
      onSuccess: refresh,
    }),
  };
}

export function useProjectActions() {
  const api = useClient();
  const queries = useQueryClient();

  const refresh = () => {
    void queries.invalidateQueries({ queryKey: keys.projects() });
    void queries.invalidateQueries({ queryKey: keys.events() });
  };

  return {
    register: useMutation({
      mutationFn: (path: string) => api.registerProject({ path }),
      onSuccess: refresh,
    }),
    remove: useMutation({
      mutationFn: (id: number) => api.removeProject(id),
      onSuccess: refresh,
    }),
  };
}
