import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { StatusBadge } from "@/components/status-badge";
import { AGENT_STATUSES } from "@/lib/api-types";

describe("StatusBadge", () => {
  it("names every status a task can be in", () => {
    for (const status of AGENT_STATUSES) {
      const { unmount } = render(<StatusBadge status={status} />);
      expect(screen.getByText(/idle|working|waiting for you|error|stopped/i)).toBeTruthy();
      unmount();
    }
  });

  it("makes waiting the loudest, because it is the one that needs a human", () => {
    const { container: waiting } = render(<StatusBadge status="waiting" />);
    const { container: idle } = render(<StatusBadge status="idle" />);

    expect(waiting.querySelector(".animate-pulse")).not.toBeNull();
    expect(idle.querySelector(".animate-pulse")).toBeNull();
    expect(waiting.firstElementChild?.className).toContain("ring-1");
  });
});
