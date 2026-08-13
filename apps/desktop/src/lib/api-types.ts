/**
 * Hand-mirrored from `crates/forge-core` (SPEC §2). The daemon's JSON is the
 * contract; when a serde representation changes there, change it here in the
 * same commit.
 */

/** Mirrors `forge_core::AgentStatus` (SPEC §5.2, D12). */
export type AgentStatus = "idle" | "working" | "waiting" | "error" | "stopped";

export const AGENT_STATUSES: readonly AgentStatus[] = [
  "idle",
  "working",
  "waiting",
  "error",
  "stopped",
] as const;

/** Mirrors `forge_core::AdapterId` (SPEC §6, D16). */
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
