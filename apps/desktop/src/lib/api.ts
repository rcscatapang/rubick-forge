import type {
  AdapterList,
  CommitRequest,
  DiffStat,
  EnqueueRequest,
  HubStatus,
  QueuedTask,
  QueueList,
  GitHubStatus,
  IssueList,
  ProjectRepo,
  PullResponse,
  TaskGitHubList,
  ApiErrorBody,
  CleanupWorktreeRequest,
  CreateTaskRequest,
  EventPage,
  GitStatus,
  Health,
  Project,
  ProjectList,
  Session,
  SessionList,
  SettingsBody,
  Task,
  TaskList,
} from "@/lib/api-types";
import { apiUrl, type DaemonConnection } from "@/lib/daemon";

/**
 * A failure the daemon described, or the fact that it could not be reached.
 *
 * `message` always reads as a sentence, because the daemon writes it that way
 * — the UI shows it as-is rather than inventing its own wording.
 */
export class ApiError extends Error {
  constructor(
    message: string,
    readonly code: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "ApiError";
  }

  /** The daemon could not be reached at all, as opposed to refusing. */
  get unreachable(): boolean {
    return this.status === 0;
  }
}

interface RequestOptions {
  method?: string;
  body?: unknown;
  params?: Record<string, string | number | boolean | undefined>;
}

async function request<T>(
  connection: DaemonConnection,
  path: string,
  { method = "GET", body, params }: RequestOptions = {},
): Promise<T> {
  let response: Response;

  try {
    response = await fetch(apiUrl(connection, path, params), {
      method,
      headers: {
        ...(connection.token ? { authorization: `Bearer ${connection.token}` } : {}),
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (cause) {
    throw new ApiError(
      "The daemon is not answering. It may not be running.",
      "unreachable",
      0,
    );
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const payload = await response.json().catch(() => null);

  if (!response.ok) {
    const described = (payload as ApiErrorBody | null)?.error;
    throw new ApiError(
      described?.message ?? `The daemon returned ${response.status}.`,
      described?.code ?? "unknown",
      response.status,
    );
  }

  return payload as T;
}

/** Everything the app can ask of one daemon. */
export function client(connection: DaemonConnection) {
  return {
    health: () => request<Health>(connection, "health"),
    adapters: () => request<AdapterList>(connection, "adapters"),

    projects: () => request<ProjectList>(connection, "projects"),
    registerProject: (body: { path: string; name?: string }) =>
      request<Project>(connection, "projects", { method: "POST", body }),
    removeProject: (id: number) =>
      request<void>(connection, `projects/${id}`, { method: "DELETE" }),

    tasks: (params: { project?: number; status?: string } = {}) =>
      request<TaskList>(connection, "tasks", { params }),
    taskGit: (id: number) => request<GitStatus>(connection, `tasks/${id}/git`),
    createTask: (body: CreateTaskRequest) =>
      request<Task>(connection, "tasks", { method: "POST", body }),
    deleteTask: (id: number, force = false) =>
      request<void>(connection, `tasks/${id}`, { method: "DELETE", params: { force } }),
    cleanupWorktree: (id: number, body: CleanupWorktreeRequest = {}) =>
      request<Task>(connection, `tasks/${id}/worktree/cleanup`, { method: "POST", body }),

    sessions: (taskId: number) => request<SessionList>(connection, `tasks/${taskId}/sessions`),
    session: (id: number) => request<Session>(connection, `sessions/${id}`),
    start: (taskId: number) =>
      request<Session>(connection, `tasks/${taskId}/start`, { method: "POST" }),
    stop: (taskId: number) =>
      request<Session>(connection, `tasks/${taskId}/stop`, { method: "POST" }),
    restart: (taskId: number) =>
      request<Session>(connection, `tasks/${taskId}/restart`, { method: "POST" }),

    settings: () => request<SettingsBody>(connection, "settings"),
    updateSettings: (changes: Record<string, string | null>) =>
      request<SettingsBody>(connection, "settings", { method: "PATCH", body: changes }),

    events: (params: { after?: number; limit?: number; task?: number; newest?: boolean } = {}) =>
      request<EventPage>(connection, "events", { params }),

    hub: () => request<HubStatus>(connection, "hub"),
    queue: () => request<QueueList>(connection, "hub/queue"),
    enqueue: (body: EnqueueRequest) =>
      request<QueuedTask>(connection, "hub/queue", { method: "POST", body }),
    cancelQueued: (id: number) =>
      request<void>(connection, `hub/queue/${id}`, { method: "DELETE" }),

    github: () => request<GitHubStatus>(connection, "github"),
    setGitHubToken: (token: string) =>
      request<void>(connection, "github/token", { method: "PUT", body: { token } }),
    forgetGitHubToken: () =>
      request<void>(connection, "github/token", { method: "DELETE" }),
    githubLinks: () => request<TaskGitHubList>(connection, "github/links"),
    projectRepo: (id: number) => request<ProjectRepo>(connection, `projects/${id}/github`),
    projectIssues: (id: number) =>
      request<IssueList>(connection, `projects/${id}/github/issues`),
    taskFromIssue: (body: { project_id: number; number: number }) =>
      request<Task>(connection, "github/tasks", { method: "POST", body }),
    taskDiff: (id: number) => request<DiffStat>(connection, `tasks/${id}/github/diff`),
    commitTask: (id: number, body: CommitRequest = {}) =>
      request<{ sha: string }>(connection, `tasks/${id}/github/commit`, {
        method: "POST",
        body,
      }),
    openPull: (id: number) =>
      request<PullResponse>(connection, `tasks/${id}/github/pull`, { method: "POST" }),
  };
}

export type DaemonClient = ReturnType<typeof client>;
