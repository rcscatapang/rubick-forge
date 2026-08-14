import { beforeEach, describe, expect, it } from "vitest";

import {
  isLocal,
  LOCAL_MACHINE_ID,
  localMachine,
  newMachineId,
  normaliseUrl,
  rememberMachines,
  storedMachines,
  type Machine,
} from "@/lib/machines";

const remote: Machine = {
  id: "machine-1",
  name: "Mac mini",
  url: "http://100.101.102.103:8787",
  sshHost: "mac-mini",
};

beforeEach(() => {
  localStorage.clear();
});

describe("the machines list", () => {
  it("always has this Mac on it, first", () => {
    const machines = storedMachines();

    expect(machines).toHaveLength(1);
    expect(machines[0].id).toBe(LOCAL_MACHINE_ID);
    expect(isLocal(machines[0])).toBe(true);
  });

  it("round-trips a machine someone added", () => {
    rememberMachines([localMachine(), remote]);

    expect(storedMachines()).toEqual([localMachine(), remote]);
  });

  it("never writes this Mac down, since it is not configuration", () => {
    rememberMachines([localMachine(), remote]);

    expect(localStorage.getItem("forge.machines")).not.toContain(LOCAL_MACHINE_ID);
  });

  it("drops an entry it cannot make sense of, and keeps the rest", () => {
    localStorage.setItem(
      "forge.machines",
      JSON.stringify([
        remote,
        { id: "machine-2" }, // no name or url
        { id: "machine-3", name: "Nope", url: "not a url" },
        "a string",
        null,
      ]),
    );

    expect(storedMachines().map((machine) => machine.id)).toEqual([LOCAL_MACHINE_ID, "machine-1"]);
  });

  it("refuses a stored entry claiming to be this Mac", () => {
    localStorage.setItem(
      "forge.machines",
      JSON.stringify([{ ...remote, id: LOCAL_MACHINE_ID, name: "Impostor" }]),
    );

    const machines = storedMachines();
    expect(machines).toHaveLength(1);
    expect(machines[0].name).toBe(localMachine().name);
  });

  it("treats unreadable storage as an empty list rather than a crash", () => {
    localStorage.setItem("forge.machines", "{{{");

    expect(storedMachines()).toEqual([localMachine()]);
  });

  it("gives every machine an id of its own", () => {
    expect(newMachineId()).not.toBe(newMachineId());
  });
});

describe("a machine's address", () => {
  it("loses its trailing slashes, so URLs are built the same way every time", () => {
    expect(normaliseUrl("http://100.64.0.1:8787/")).toBe("http://100.64.0.1:8787");
    expect(normaliseUrl("  http://100.64.0.1:8787  ")).toBe("http://100.64.0.1:8787");
  });

  it("is http or nothing", () => {
    expect(normaliseUrl("100.64.0.1:8787")).toBeNull();
    expect(normaliseUrl("ws://100.64.0.1:8787")).toBeNull();
    expect(normaliseUrl("file:///etc/passwd")).toBeNull();
    expect(normaliseUrl("")).toBeNull();
  });

  it("refuses credentials in the URL, which is not where a token goes", () => {
    expect(normaliseUrl("http://user:secret@100.64.0.1:8787")).toBeNull();
  });
});
