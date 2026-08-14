import { AGENT_STATUS_LABELS, needsAttention, type AgentStatus } from "@/lib/api-types";
import { cn } from "@/lib/utils";

/**
 * One hue per state, everywhere.
 *
 * `waiting` is the loudest on purpose: it is the state that needs a human, and
 * a dashboard full of quiet badges would bury it.
 */
const TONE: Record<AgentStatus, string> = {
  idle: "bg-[var(--status-idle)]/12 text-[var(--status-idle)]",
  working: "bg-[var(--status-working)]/15 text-[var(--status-working)]",
  waiting:
    "bg-[var(--status-waiting)]/25 text-[var(--status-waiting)] ring-1 ring-[var(--status-waiting)]/60",
  error: "bg-[var(--status-error)]/15 text-[var(--status-error)]",
  stopped: "bg-[var(--status-stopped)]/12 text-[var(--status-stopped)]",
};

export function StatusBadge({
  status,
  className,
}: {
  status: AgentStatus;
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium",
        TONE[status],
        className,
      )}
    >
      <span
        aria-hidden
        className={cn(
          "size-1.5 rounded-full bg-current",
          needsAttention(status) && "animate-pulse",
        )}
      />
      {AGENT_STATUS_LABELS[status]}
    </span>
  );
}
