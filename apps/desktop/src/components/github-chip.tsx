import { openUrl } from "@tauri-apps/plugin-opener";

import type { TaskGitHub } from "@/lib/api-types";

/** How each check state reads, and how loud it is. */
const CHECKS = {
  none: null,
  running: { label: "CI running", tone: "text-muted-foreground" },
  passed: { label: "CI passed", tone: "text-[var(--status-idle,inherit)]" },
  failed: { label: "CI failed", tone: "text-[var(--status-error)]" },
} as const;

const PR_STATE = {
  open: { label: "open", tone: "text-muted-foreground" },
  merged: { label: "merged", tone: "text-[var(--status-idle,inherit)]" },
  closed: { label: "closed", tone: "text-muted-foreground" },
} as const;

/**
 * A task's pull request and what CI said about it.
 *
 * Renders nothing for a task with no pull request, which is most of them —
 * this is the deferred test/build status from D13, and it only exists once
 * there is something on GitHub to report.
 */
export function GitHubChip({ link }: { link: TaskGitHub | undefined }) {
  if (!link?.pr_number || !link.pr_url) return null;

  const url = link.pr_url;
  const state = PR_STATE[link.pr_state as keyof typeof PR_STATE] ?? PR_STATE.open;
  const checks = CHECKS[link.checks];

  return (
    <span className="flex items-center gap-1.5 text-xs">
      <button
        type="button"
        className={`rounded border border-border px-1.5 py-0.5 ${state.tone}`}
        title={`Open pull request #${link.pr_number} on GitHub`}
        onClick={() => void openUrl(url)}
      >
        PR #{link.pr_number} · {state.label}
      </button>

      {checks && (
        <span className={checks.tone} title={staleness(link)}>
          {checks.label}
        </span>
      )}
    </span>
  );
}

/**
 * How old this reading is.
 *
 * Shown rather than hidden: when the rate limit is exhausted the chip stops
 * updating, and a stale green is only honest if it says when it was green.
 */
function staleness(link: TaskGitHub): string | undefined {
  if (!link.polled_at) return undefined;

  return `Last checked ${new Date(link.polled_at).toLocaleTimeString()}`;
}
