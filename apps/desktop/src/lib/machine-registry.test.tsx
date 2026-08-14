import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import type { ReactNode } from "react";

import {
  MachineRegistry,
  testConnection,
  useMachines,
} from "@/lib/machine-registry";
import { LOCAL_MACHINE_ID, rememberMachines, localMachine } from "@/lib/machines";
import { daemonReturning } from "@/test/harness";

function wrapper({ children }: { children: ReactNode }) {
  return <MachineRegistry>{children}</MachineRegistry>;
}

const remote = {
  id: "machine-1",
  name: "Mac mini",
  url: "http://100.101.102.103:8787",
  sshHost: null,
};

beforeEach(() => {
  localStorage.clear();
  invoke.mockReset();
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("resolving machines to connections", () => {
  it("reads the local token from the daemon's own file, not the keychain", async () => {
    invoke.mockImplementation((command: string) =>
      command === "daemon_token" ? Promise.resolve("local-token") : Promise.reject(new Error()),
    );

    const { result } = renderHook(() => useMachines(), { wrapper });
    await waitFor(() => expect(result.current.ready).toBe(true));

    expect(result.current.machines).toHaveLength(1);
    expect(result.current.machines[0].connection.token).toBe("local-token");
    expect(invoke).not.toHaveBeenCalledWith("machine_token", expect.anything());
  });

  it("reads a remote token from the keychain, keyed by machine id", async () => {
    rememberMachines([localMachine(), remote]);
    invoke.mockImplementation((command: string, args: { machine?: string }) => {
      if (command === "daemon_token") return Promise.resolve("local-token");
      if (command === "machine_token" && args.machine === remote.id) {
        return Promise.resolve("remote-token");
      }
      return Promise.resolve(null);
    });

    const { result } = renderHook(() => useMachines(), { wrapper });
    await waitFor(() => expect(result.current.machines).toHaveLength(2));
    await waitFor(() => expect(result.current.ready).toBe(true));

    expect(result.current.machines[1].connection).toEqual({
      url: remote.url,
      token: "remote-token",
    });
  });

  it("keeps a machine whose token cannot be read, so it can say why", async () => {
    rememberMachines([localMachine(), remote]);
    invoke.mockRejectedValue(new Error("the keychain said no"));

    const { result } = renderHook(() => useMachines(), { wrapper });
    await waitFor(() => expect(result.current.ready).toBe(true));

    expect(result.current.machines).toHaveLength(2);
    expect(result.current.machines[1].connection.token).toBe("");
  });

  it("saves a token before putting the machine on the list", async () => {
    invoke.mockResolvedValue(undefined);
    const { result } = renderHook(() => useMachines(), { wrapper });
    await waitFor(() => expect(result.current.ready).toBe(true));

    await result.current.add({
      name: "Mac mini",
      url: remote.url,
      token: "secret",
      sshHost: null,
    });

    expect(invoke).toHaveBeenCalledWith("set_machine_token", {
      machine: expect.stringContaining("machine-"),
      token: "secret",
    });
    // And the secret is not what got written to storage.
    expect(localStorage.getItem("forge.machines")).not.toContain("secret");
  });

  it("cannot remove this Mac", async () => {
    invoke.mockResolvedValue(undefined);
    const { result } = renderHook(() => useMachines(), { wrapper });
    await waitFor(() => expect(result.current.ready).toBe(true));

    await result.current.remove(LOCAL_MACHINE_ID);

    await waitFor(() => expect(result.current.machines).toHaveLength(1));
    expect(invoke).not.toHaveBeenCalledWith("forget_machine_token", expect.anything());
  });
});

describe("testing a machine before saving it", () => {
  it("tells a daemon that is not there from one that refused the token", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new Error("no route"))));

    const result = await testConnection("http://daemon.test", "t");

    expect(result).toEqual({ ok: false, problem: expect.stringContaining("No daemon answered") });
  });

  it("names the machine when the address is right and the token is not", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string) => {
        if (new URL(input).pathname === "/health") {
          return new Response(
            JSON.stringify({ version: "0.2.0", uptime_secs: 1, machine: "Mac mini", binaries: [] }),
            { status: 200, headers: { "content-type": "application/json" } },
          );
        }
        return new Response(
          JSON.stringify({ error: { code: "unauthorized", message: "no" } }),
          { status: 401, headers: { "content-type": "application/json" } },
        );
      }),
    );

    const result = await testConnection("http://daemon.test", "wrong");

    expect(result).toEqual({ ok: false, problem: expect.stringContaining("rejected that token") });
  });

  it("reports who answered when everything is right", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        daemonReturning({
          health: { version: "0.2.0", uptime_secs: 1, machine: "Mac mini", binaries: [] },
          settings: { settings: {} },
        }),
      ),
    );

    const result = await testConnection("http://daemon.test", "right");

    expect(result).toEqual({ ok: true, name: "Mac mini", version: "0.2.0" });
  });
});
