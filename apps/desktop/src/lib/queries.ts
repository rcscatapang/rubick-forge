import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";

import { client } from "@/lib/api";
import type { EventKind } from "@/lib/api-types";
import { useDaemon, useMachineId } from "@/lib/connection";
import type { MachineConnection } from "@/lib/machine-registry";

/**
 * Query keys for one machine, in one place.
 *
 * Every key starts with `forge` and that machine's id: ids are what keep two
 * daemons' task 1 apart, and what lets one machine's data be dropped wholesale
 * without touching another's.
 */
export function keysFor(machine: string) {
  const all = ["forge", machine] as const;

  return {
    all,
    health: () => [...all, "health"] as const,
    adapters: () => [...all, "adapters"] as const,
    projects: () => [...all, "projects"] as const,
    tasks: () => [...all, "tasks"] as const,
    taskGit: (id: number) => [...all, "tasks", id, "git"] as const,
    sessions: (taskId: number) => [...all, "tasks", taskId, "sessions"] as const,
    session: (id: number) => [...all, "sessions", id] as const,
    events: () => [...all, "events"] as const,
    settings: () => [...all, "settings"] as const,
  };
}

export type Keys = ReturnType<typeof keysFor>;

/** The keys for the machine the surrounding subtree belongs to. */
export function useKeys(): Keys {
  return keysFor(useMachineId());
}

function useClient() {
  return client(useDaemon());
}

export function useHealth() {
  const api = useClient();
  const keys = useKeys();
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
  const keys = useKeys();
  return useQuery({ queryKey: keys.adapters(), queryFn: () => api.adapters() });
}

export function useProjects() {
  const api = useClient();
  const keys = useKeys();
  return useQuery({ queryKey: keys.projects(), queryFn: () => api.projects() });
}

export function useTasks() {
  const api = useClient();
  const keys = useKeys();
  return useQuery({ queryKey: keys.tasks(), queryFn: () => api.tasks() });
}

export function useTaskGit(id: number) {
  const api = useClient();
  const keys = useKeys();
  return useQuery({ queryKey: keys.taskGit(id), queryFn: () => api.taskGit(id) });
}

export function useSessions(taskId: number) {
  const api = useClient();
  const keys = useKeys();
  return useQuery({ queryKey: keys.sessions(taskId), queryFn: () => api.sessions(taskId) });
}

export function useSettings() {
  const api = useClient();
  const keys = useKeys();
  return useQuery({ queryKey: keys.settings(), queryFn: () => api.settings() });
}

export function useUpdateSettings() {
  const api = useClient();
  const queries = useQueryClient();
  const keys = useKeys();

  return useMutation({
    mutationFn: (changes: Record<string, string | null>) => api.updateSettings(changes),
    onSuccess: (updated) => queries.setQueryData(keys.settings(), updated),
  });
}

/** One session, for a view that knows only its id. */
export function useSession(id: number) {
  const api = useClient();
  const keys = useKeys();
  return useQuery({ queryKey: keys.session(id), queryFn: () => api.session(id) });
}

/** How many events the feed asks each machine for. */
const FEED_PAGE = 50;

/**
 * One machine's slice of the activity feed.
 *
 * Not a hook: the feed asks every machine at once through `useQueries`, so the
 * query has to be describable outside a component.
 *
 * It reads the end of history, not its beginning — a feed that paged forward
 * from the first event ever recorded would freeze once there were more than a
 * page of them.
 */
export function machineEventsQuery({ machine, connection }: MachineConnection) {
  return {
    queryKey: keysFor(machine.id).events(),
    queryFn: () => client(connection).events({ limit: FEED_PAGE, newest: true }),
  };
}

/**
 * Refetch what a given event actually changed, on the machine it happened on.
 *
 * The task list is invalidated exactly, not by prefix: `keys.tasks()` is a
 * prefix of every per-task key, so a prefix match would refetch every task's
 * git status and session list on every status change.
 */
export function invalidateFor(
  queries: QueryClient,
  machine: string,
  kind: EventKind,
  taskId?: number | null,
) {
  const keys = keysFor(machine);

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
  const keys = useKeys();

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
  const keys = useKeys();

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
