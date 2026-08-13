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

/** Mirrors `forge_core::AdapterId`. */
export type AdapterId = "claude-code" | "codex";

export const ADAPTER_IDS: readonly AdapterId[] = ["claude-code", "codex"] as const;

export const ADAPTER_LABELS: Record<AdapterId, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
};

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
  | "worktree_removed";

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
  | { kind: "worktree_removed"; task_id: number; path: string };

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
