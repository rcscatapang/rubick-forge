/**
 * Hand-mirrored from `crates/forge-core`. The daemon's JSON is the contract;
 * when a serde representation changes there, change it here in the same commit.
 */

/** Mirrors `forge_core::AgentStatus`. */
export type AgentStatus = "idle" | "working" | "waiting" | "error" | "stopped";

export const AGENT_STATUSES: readonly AgentStatus[] = [
  "idle",
  "working",
  "waiting",
  "error",
  "stopped",
] as const;

/**
 * Mirrors `forge_core::AdapterId`.
 *
 * Open, not a union: adapters are declared by TOML manifests on the daemon, so
 * the app learns what exists from `GET /adapters` rather than from this file.
 */
export type AdapterId = string;

/** The two Forge ships with, for a sensible default before adapters load. */
export const DEFAULT_ADAPTER: AdapterId = "claude-code";

export const AGENT_STATUS_LABELS: Record<AgentStatus, string> = {
  idle: "Idle",
  working: "Working",
  waiting: "Waiting for you",
  error: "Error",
  stopped: "Stopped",
};

/** Mirrors `AgentStatus::needs_attention`. */
export function needsAttention(status: AgentStatus): boolean {
  return status === "waiting" || status === "error";
}

/** Mirrors `AgentStatus::is_live`: a tmux session is expected to exist. */
export function isLive(status: AgentStatus): boolean {
  return status === "idle" || status === "working" || status === "waiting";
}

/** RFC 3339, UTC. Mirrors `forge_core::Timestamp`. */
export type Timestamp = string;

/** Per-adapter configuration on a project. The daemon does not interpret it. */
export type AdapterSettings = Partial<Record<AdapterId, Record<string, unknown>>>;

/** Mirrors `forge_core::Project`. */
export interface Project {
  id: number;
  name: string;
  path: string;
  default_branch: string;
  adapter_settings: AdapterSettings;
  created_at: Timestamp;
  updated_at: Timestamp;
}

/** Mirrors `forge_core::Task`. `worktree_path: null` means "runs in the repo root". */
export interface Task {
  id: number;
  project_id: number;
  title: string;
  adapter: AdapterId;
  base_branch: string;
  branch: string;
  worktree_path: string | null;
  initial_prompt: string | null;
  status: AgentStatus;
  created_at: Timestamp;
  updated_at: Timestamp;
}

/** Mirrors `forge_core::Session`. */
export interface Session {
  id: number;
  task_id: number;
  tmux_name: string;
  pid: number | null;
  status: AgentStatus;
  started_at: Timestamp;
  ended_at: Timestamp | null;
}

/** Mirrors `forge_core::GitStatus`. Every field degrades rather than fails. */
export interface GitStatus {
  /** `null` when HEAD is detached. */
  branch: string | null;
  /** `null` in a repository with no commits yet. */
  head: string | null;
  dirty: boolean;
  upstream: string | null;
  ahead: number | null;
  behind: number | null;
}

/** The body of `GET /projects`. */
export interface ProjectList {
  projects: Project[];
}

/** The body of `GET /tasks`. */
export interface TaskList {
  tasks: Task[];
}

/** The body of `GET /tasks/:id/sessions`. */
export interface SessionList {
  sessions: Session[];
}

/** What kind of value an adapter setting takes. */
export type SettingKind = "text" | "number" | "flag";

/** One setting an adapter understands. */
export interface SettingDef {
  key: string;
  kind: SettingKind;
  description: string;
}

/** One entry of `GET /adapters`. */
export interface AdapterInfo {
  id: AdapterId;
  name: string;
  binary: BinaryStatus;
  settings: SettingDef[];
}

/** One adapter manifest that would not load. */
export interface AdapterLoadError {
  /** The file it came from, or `built-in`. */
  source: string;
  detail: string;
}

/** The body of `GET /adapters`. */
export interface AdapterList {
  adapters: AdapterInfo[];
  /** Manifests that would not load; an adapter missing above has a reason. */
  errors: AdapterLoadError[];
}

/** The body of `POST /adapters/reload`. */
export interface ReloadResult {
  loaded: number;
  errors: AdapterLoadError[];
}

/** The body of `GET /settings` and `PATCH /settings`. */
export interface SettingsBody {
  settings: Record<string, string>;
}

/** The body of `POST /tasks/:id/instruction`. */
export interface InstructionRequest {
  text: string;
}

/** The body of `POST /tasks`. */
export interface CreateTaskRequest {
  project_id: number;
  title: string;
  adapter: AdapterId;
  /** Defaults to the project's default branch. */
  base_branch?: string;
  initial_prompt?: string;
  /** Defaults to true; false runs the task in the repository root. */
  use_worktree?: boolean;
}

/** The body of `POST /tasks/:id/worktree/cleanup`. Both default to false. */
export interface CleanupWorktreeRequest {
  /** Remove a dirty worktree, and an unmerged branch. */
  force?: boolean;
  /** Drop `forge/<slug>` as well as the directory. */
  delete_branch?: boolean;
}

/** Mirrors `forge_core::EventKind`. */
export type EventKind =
  | "project_registered"
  | "project_removed"
  | "task_created"
  | "task_deleted"
  | "task_finished"
  | "session_started"
  | "session_stopped"
  | "status_changed"
  | "agent_waiting"
  | "agent_error"
  | "worktree_created"
  | "worktree_removed"
  | "pr_opened"
  | "pr_merged"
  | "pr_closed"
  | "checks_passed"
  | "checks_failed"
  | "task_queued"
  | "task_dispatched"
  | "dispatch_failed";

/** Mirrors `forge_core::StopReason`. */
export type StopReason = "requested" | "exited" | "vanished";

/**
 * Mirrors `forge_core::ForgeEvent`: a flat, `kind`-tagged union. This is what
 * `/ws/events` frames carry and what `/events` rows contain.
 */
export type ForgeEvent =
  | { kind: "project_registered"; project_id: number; name: string; path: string }
  | { kind: "project_removed"; project_id: number }
  | { kind: "task_created"; task_id: number; project_id: number; title: string }
  | { kind: "task_deleted"; task_id: number }
  | { kind: "task_finished"; task_id: number; session_id: number }
  | { kind: "session_started"; task_id: number; session_id: number; tmux_name: string }
  | { kind: "session_stopped"; task_id: number; session_id: number; reason: StopReason }
  | {
      kind: "status_changed";
      task_id: number;
      session_id: number;
      from: AgentStatus;
      to: AgentStatus;
    }
  | { kind: "agent_waiting"; task_id: number; session_id: number; tail: string }
  | { kind: "agent_error"; task_id: number; session_id: number; detail: string }
  | { kind: "worktree_created"; task_id: number; path: string; branch: string }
  | { kind: "worktree_removed"; task_id: number; path: string }
  | { kind: "pr_opened"; task_id: number; number: number; url: string }
  | { kind: "pr_merged"; task_id: number; number: number; url: string }
  | { kind: "pr_closed"; task_id: number; number: number; url: string }
  | { kind: "checks_passed"; task_id: number; number: number; url: string }
  | { kind: "checks_failed"; task_id: number; number: number; url: string }
  | {
      kind: "task_queued";
      queued_id: number;
      project_name: string;
      title: string;
      target: string | null;
    }
  | {
      kind: "task_dispatched";
      queued_id: number;
      machine: string;
      /** The task's id *on that machine*, which is not this daemon's. */
      remote_task: number;
      considered: string[];
    }
  | { kind: "dispatch_failed"; queued_id: number; machine: string; detail: string };

/** Mirrors `forge_core::EventRecord`: a stored event, flattened. */
export type EventRecord = { id: number; ts: Timestamp } & ForgeEvent;

/** Mirrors the daemon's `EventPage`. */
export interface EventPage {
  events: EventRecord[];
  next_after: number | null;
}

/** Mirrors `EventKind::is_notifiable`: the kinds that raise a notification. */
export const NOTIFIABLE_KINDS: readonly EventKind[] = [
  "agent_waiting",
  "task_finished",
  "agent_error",
  // Both are the end of something the human was waiting on, and both happen
  // while they are looking elsewhere.
  "pr_merged",
  "checks_failed",
] as const;

export function isNotifiable(kind: EventKind): boolean {
  return NOTIFIABLE_KINDS.includes(kind);
}

/** Mirrors `forge_core::BinaryStatus`. */
export interface BinaryStatus {
  name: string;
  path: string | null;
  version: string | null;
  ok: boolean;
  detail: string | null;
}

/** Mirrors `forge_core::Health`: the body of `GET /health`. */
export interface Health {
  version: string;
  uptime_secs: number;
  machine: string;
  binaries: BinaryStatus[];
  /** Adapter manifests that would not load. A warning, never a failure. */
  adapter_errors: AdapterLoadError[];
}

/** Mirrors `Health::is_healthy`. */
export function isHealthy(health: Health): boolean {
  return health.binaries.every((binary) => binary.ok);
}

/** The daemon's error envelope. Every non-2xx response has this shape. */
export interface ApiErrorBody {
  error: {
    code: string;
    message: string;
  };
}

/** Mirrors `forge_core::ChecksState`. */
export type ChecksState = "none" | "running" | "passed" | "failed";

/** Mirrors `forge_core::TaskGitHub`. Every field fills in over a task's life. */
export interface TaskGitHub {
  task_id: number;
  /** The issue this task was started from. */
  issue_number: number | null;
  pr_number: number | null;
  pr_url: string | null;
  /** GitHub's own word: `open`, `merged` or `closed`. */
  pr_state: string | null;
  head_sha: string | null;
  checks: ChecksState;
  /** When the daemon last heard from GitHub. */
  polled_at: Timestamp | null;
}

/** The body of `GET /github/links`. */
export interface TaskGitHubList {
  links: TaskGitHub[];
}

/** Mirrors the daemon's `RateLimit`. */
export interface RateLimit {
  remaining: number | null;
  resets_at: number | null;
}

/** The body of `GET /github`. */
export interface GitHubStatus {
  configured: boolean;
  rate_limit: RateLimit;
}

/** The body of `GET /projects/:id/github`. `repo: null` means not on GitHub. */
export interface ProjectRepo {
  repo: string | null;
  url: string | null;
}

/** One open issue, from `GET /projects/:id/github/issues`. */
export interface IssueSummary {
  number: number;
  title: string;
  url: string;
}

export interface IssueList {
  issues: IssueSummary[];
}

/** Mirrors `forge_daemon::git::DiffStat`: what a commit would include. */
export interface DiffStat {
  files: number;
  insertions: number;
  deletions: number;
  paths: string[];
}

/** The body of `POST /tasks/:id/github/pull`. */
export interface PullResponse {
  number: number;
  url: string;
}

/** The body of `POST /tasks/:id/github/commit`. */
export interface CommitRequest {
  /** Defaults to the task's title. */
  message?: string;
}

/** Mirrors `forge_core::QueueState`. */
export type QueueState = "queued" | "dispatched" | "cancelled";

/**
 * Mirrors `forge_core::QueuedTask`.
 *
 * Not a `Task`: it has no branch, worktree or session. It becomes a task on
 * whichever machine takes it, and `remote_task` is that task's id *there*.
 */
export interface QueuedTask {
  id: number;
  project_name: string;
  adapter: AdapterId;
  title: string;
  prompt: string | null;
  /** A machine name, or `null` for "whichever machine can take it". */
  target: string | null;
  state: QueueState;
  /** Why a queued row is still queued. */
  reason: string | null;
  machine: string | null;
  remote_task: number | null;
  /** Every machine that could have taken it, recorded before the choice. */
  considered: string[];
  created_at: Timestamp;
  updated_at: Timestamp;
}

/** The body of `GET /hub/queue`. */
export interface QueueList {
  queue: QueuedTask[];
}

/** The body of `GET /hub`. */
export interface HubStatus {
  hub: boolean;
  /** Machine names this hub dispatches to, empty when it is not a hub. */
  machines: string[];
}

/** The body of `POST /hub/queue`. */
export interface EnqueueRequest {
  project_name: string;
  title: string;
  adapter?: AdapterId;
  prompt?: string;
  /** Omitted for "whichever machine can take it". */
  target?: string;
}
