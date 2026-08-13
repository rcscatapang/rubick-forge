import { describe, expect, it } from "vitest";

import {
  ADAPTER_IDS,
  ADAPTER_LABELS,
  AGENT_STATUSES,
  AGENT_STATUS_LABELS,
  type EventRecord,
  type Health,
  isHealthy,
  isLive,
  isNotifiable,
  needsAttention,
} from "@/lib/api-types";

describe("api-types", () => {
  it("labels every status and adapter", () => {
    for (const status of AGENT_STATUSES) {
      expect(AGENT_STATUS_LABELS[status]).toBeTruthy();
    }
    for (const adapter of ADAPTER_IDS) {
      expect(ADAPTER_LABELS[adapter]).toBeTruthy();
    }
  });

  it("flags waiting and error as needing a human", () => {
    expect(needsAttention("waiting")).toBe(true);
    expect(needsAttention("error")).toBe(true);
    expect(needsAttention("working")).toBe(false);
  });

  it("treats only live statuses as session-backed", () => {
    expect(isLive("working")).toBe(true);
    expect(isLive("stopped")).toBe(false);
    expect(isLive("error")).toBe(false);
  });

  it("notifies on exactly the three signal kinds", () => {
    expect(isNotifiable("agent_waiting")).toBe(true);
    expect(isNotifiable("task_finished")).toBe(true);
    expect(isNotifiable("agent_error")).toBe(true);
    expect(isNotifiable("status_changed")).toBe(false);
  });

  it("narrows an event record by its kind", () => {
    const record: EventRecord = {
      id: 7,
      ts: "2026-08-13T09:30:00Z",
      kind: "agent_waiting",
      task_id: 2,
      session_id: 3,
      tail: "Allow edit to src/main.rs?",
    };

    // The discriminant is what gives access to the kind-specific fields.
    expect(record.kind === "agent_waiting" && record.tail).toBe("Allow edit to src/main.rs?");
  });

  it("is unhealthy when any dependency is missing", () => {
    const health: Health = {
      version: "0.1.0",
      uptime_secs: 12,
      machine: "mac-mini",
      binaries: [{ name: "git", path: "/usr/bin/git", version: "2.50.1", ok: true, detail: null }],
    };

    expect(isHealthy(health)).toBe(true);

    health.binaries.push({
      name: "tmux",
      path: null,
      version: null,
      ok: false,
      detail: "`tmux` was not found on PATH",
    });
    expect(isHealthy(health)).toBe(false);
  });
});
