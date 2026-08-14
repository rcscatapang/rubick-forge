import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(), openPath: vi.fn() }));

import { GitHubChip } from "@/components/github-chip";
import type { ChecksState, TaskGitHub } from "@/lib/api-types";
import { renderApp } from "@/test/harness";

function link(overrides: Partial<TaskGitHub> = {}): TaskGitHub {
  return {
    task_id: 1,
    issue_number: null,
    pr_number: 7,
    pr_url: "https://github.com/o/n/pull/7",
    pr_state: "open",
    head_sha: "abc",
    checks: "none",
    polled_at: null,
    ...overrides,
  };
}

describe("a task's GitHub chip", () => {
  it("shows nothing at all for a task with no pull request", () => {
    // Most tasks are this, and a row should not grow an empty slot for it.
    const { container } = renderApp(<GitHubChip link={undefined} />);
    expect(container).toBeEmptyDOMElement();

    const without = renderApp(<GitHubChip link={link({ pr_number: null })} />);
    expect(without.container).toBeEmptyDOMElement();
  });

  it("names the pull request and its state", () => {
    renderApp(<GitHubChip link={link()} />);

    expect(screen.getByRole("button", { name: /PR #7/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /open/ })).toBeInTheDocument();
  });

  it("says when a pull request was merged", () => {
    renderApp(<GitHubChip link={link({ pr_state: "merged" })} />);

    expect(screen.getByRole("button", { name: /merged/ })).toBeInTheDocument();
  });

  it("stays quiet about CI until CI has said something", () => {
    renderApp(<GitHubChip link={link({ checks: "none" })} />);

    expect(screen.queryByText(/CI/)).not.toBeInTheDocument();
  });

  it("reports each check state it is given", () => {
    for (const [checks, label] of [
      ["running", "CI running"],
      ["passed", "CI passed"],
      ["failed", "CI failed"],
    ] as [ChecksState, string][]) {
      const { unmount } = renderApp(<GitHubChip link={link({ checks })} />);
      expect(screen.getByText(label)).toBeInTheDocument();
      unmount();
    }
  });

  it("says how old the reading is, because a stale green is still green", () => {
    renderApp(
      <GitHubChip link={link({ checks: "passed", polled_at: "2026-08-14T10:00:00Z" })} />,
    );

    expect(screen.getByText("CI passed")).toHaveAttribute(
      "title",
      expect.stringContaining("Last checked"),
    );
  });
});
