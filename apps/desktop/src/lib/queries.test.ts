import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";

import { invalidateFor, keysFor } from "@/lib/queries";

const MACHINE = "local";
const keys = keysFor(MACHINE);

function watched() {
  const queries = new QueryClient();
  const invalidate = vi.spyOn(queries, "invalidateQueries");
  return { queries, invalidate };
}

interface Asked {
  queryKey: unknown[];
  exact?: boolean;
}

/** The query keys a call asked to refetch. */
function invalidated(invalidate: { mock: { calls: unknown[][] } }): Asked[] {
  return invalidate.mock.calls.map(([filter]) => filter as Asked);
}

describe("what an event makes stale", () => {
  it("always refetches the activity feed", () => {
    const { queries, invalidate } = watched();

    invalidateFor(queries, MACHINE, "status_changed", 7);

    expect(invalidated(invalidate).some((asked) => asked.queryKey.includes("events"))).toBe(true);
  });

  it("refetches a project change's tasks too, since removing one takes them with it", () => {
    const { queries, invalidate } = watched();

    invalidateFor(queries, MACHINE, "project_removed", null);

    const filters = invalidated(invalidate);
    expect(filters.some((asked) => asked.queryKey.includes("projects"))).toBe(true);
    expect(filters.some((asked) => asked.queryKey.includes("tasks"))).toBe(true);
  });

  it("does not refetch every task's git and sessions on one task's change", () => {
    const { queries, invalidate } = watched();

    invalidateFor(queries, MACHINE, "status_changed", 7);

    const list = invalidated(invalidate).find(
      (asked) => JSON.stringify(asked.queryKey) === JSON.stringify(keys.tasks()),
    );

    expect(list?.exact).toBe(true);
  });

  it("refetches the changed task's own detail", () => {
    const { queries, invalidate } = watched();

    invalidateFor(queries, MACHINE, "agent_waiting", 7);

    expect(
      invalidated(invalidate).some(
        (asked) => JSON.stringify(asked.queryKey) === JSON.stringify([...keys.all, "tasks", 7]),
      ),
    ).toBe(true);
  });

  it("has nothing task-shaped to refetch for a project event", () => {
    const { queries, invalidate } = watched();

    invalidateFor(queries, MACHINE, "project_registered", null);

    expect(
      invalidated(invalidate).some(
        (asked) => asked.queryKey.length > 3 && asked.queryKey[2] === "tasks",
      ),
    ).toBe(false);
  });
});
