import { describe, expect, it } from "vitest";

import {
  ADAPTER_IDS,
  ADAPTER_LABELS,
  AGENT_STATUSES,
  AGENT_STATUS_LABELS,
  isLive,
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
});
