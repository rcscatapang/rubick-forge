import { isHealthy } from "@/lib/api-types";
import { ApiError } from "@/lib/api";
import { useHealth } from "@/lib/queries";
import { describeSkew, skew } from "@/lib/version";

/** The app's own version, stamped in at build time by Vite. */
const APP_VERSION = __APP_VERSION__;

/**
 * What one machine is doing, in a line.
 *
 * A machine that is down says so here and nowhere else: the rest of the
 * dashboard keeps showing every other machine's live state, because one Mac
 * being asleep is not an app-wide failure.
 */
export function MachineChip() {
  const health = useHealth();

  if (health.isLoading) {
    return <Chip tone="muted">checking…</Chip>;
  }

  if (health.isError) {
    const reason =
      health.error instanceof ApiError && !health.error.unreachable
        ? "not answering properly"
        : "unreachable";
    return <Chip tone="error">{reason}</Chip>;
  }

  const daemon = health.data;
  if (!daemon) return null;

  const mismatch = describeSkew(skew(APP_VERSION, daemon.version), daemon.version);
  const broken = daemon.binaries.filter((binary) => !binary.ok);

  return (
    <span className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
      <Chip tone={isHealthy(daemon) ? "ok" : "warn"}>daemon {daemon.version}</Chip>
      <span className="truncate">{daemon.machine}</span>
      {broken.map((binary) => (
        <Chip key={binary.name} tone="warn" title={binary.detail ?? undefined}>
          {binary.name} unavailable
        </Chip>
      ))}
      {mismatch && (
        <Chip tone="warn" title={mismatch}>
          version mismatch
        </Chip>
      )}
    </span>
  );
}

const TONES = {
  ok: "border-border text-muted-foreground",
  warn: "border-[var(--status-waiting)] text-[var(--status-waiting)]",
  error: "border-[var(--status-error)] text-[var(--status-error)]",
  muted: "border-border text-muted-foreground opacity-70",
} as const;

function Chip({
  tone,
  title,
  children,
}: {
  tone: keyof typeof TONES;
  title?: string;
  children: React.ReactNode;
}) {
  return (
    <span className={`rounded border px-1.5 py-0.5 ${TONES[tone]}`} title={title}>
      {children}
    </span>
  );
}
