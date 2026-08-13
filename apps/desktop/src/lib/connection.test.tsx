import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { DaemonProvider, storedConnection, useConnect, useDaemon } from "@/lib/connection";

function Probe() {
  const connection = useDaemon();
  const connect = useConnect();

  return (
    <button type="button" onClick={() => connect({ ...connection, token: "fresh" })}>
      token:{connection.token || "none"}
    </button>
  );
}

beforeEach(() => localStorage.clear());

describe("the daemon connection", () => {
  it("starts from what the browser remembers", () => {
    localStorage.setItem("forge.daemon.token", "remembered");

    render(
      <DaemonProvider>
        <Probe />
      </DaemonProvider>,
    );

    expect(screen.getByText("token:remembered")).toBeTruthy();
  });

  it("takes effect immediately, not on the next launch", () => {
    render(
      <DaemonProvider>
        <Probe />
      </DaemonProvider>,
    );
    expect(screen.getByText("token:none")).toBeTruthy();

    act(() => screen.getByRole("button").click());

    // The install flow writes a token; everything after it must use that one,
    // or the app keeps authenticating with nothing until it restarts.
    expect(screen.getByText("token:fresh")).toBeTruthy();
    expect(storedConnection().token).toBe("fresh");
  });
});
