/**
 * The machines this app talks to.
 *
 * Every Mac runs the same daemon and no daemon knows about any other, so the
 * list of them is the app's alone (SPEC D17). It holds no secrets: a machine's
 * bearer token lives in the macOS keychain, keyed by the machine's id.
 */

import { storedConnection } from "@/lib/daemon";

/** The daemon on this Mac, which is always present and cannot be removed. */
export const LOCAL_MACHINE_ID = "local";

export interface Machine {
  id: string;
  /** What to call it on screen until `/health` says its real name. */
  name: string;
  /** Base URL, without a trailing slash. */
  url: string;
  /** Destination for "open terminal over ssh", or `null` to leave it out. */
  sshHost: string | null;
}

export function isLocal(machine: Machine): boolean {
  return machine.id === LOCAL_MACHINE_ID;
}

/**
 * This Mac.
 *
 * Kept implicit so an install that never adds a second machine behaves exactly
 * as it did before there were machines at all.
 */
export function localMachine(): Machine {
  // Whatever the app was pointed at, so the gate and the dashboard cannot end
  // up talking to two different daemons.
  return {
    id: LOCAL_MACHINE_ID,
    name: "This Mac",
    url: storedConnection().url,
    sshHost: null,
  };
}

const MACHINES_KEY = "forge.machines";

/** Ids are opaque and permanent: renaming a machine must not orphan its token. */
export function newMachineId(): string {
  return `machine-${crypto.randomUUID()}`;
}

/**
 * A base URL with no trailing slash and no credentials in it.
 *
 * Returns `null` for anything that is not an http(s) URL, which is what a
 * mistyped entry looks like.
 */
export function normaliseUrl(raw: string): string | null {
  let url: URL;
  try {
    url = new URL(raw.trim());
  } catch {
    return null;
  }

  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  // A token belongs in the keychain, not in a URL that gets logged.
  if (url.username || url.password) return null;

  return url.toString().replace(/\/+$/, "");
}

function parse(value: unknown): Machine | null {
  if (typeof value !== "object" || value === null) return null;
  const { id, name, url, sshHost } = value as Record<string, unknown>;

  if (typeof id !== "string" || !id || id === LOCAL_MACHINE_ID) return null;
  if (typeof name !== "string" || !name) return null;
  if (typeof url !== "string" || normaliseUrl(url) === null) return null;

  return {
    id,
    name,
    url,
    sshHost: typeof sshHost === "string" && sshHost ? sshHost : null,
  };
}

/**
 * The machines this app knows about, local first.
 *
 * An entry that does not parse is dropped rather than thrown: a machines list
 * mangled by a hand edit should cost that machine, not the whole app.
 */
export function storedMachines(): Machine[] {
  let remote: Machine[] = [];

  try {
    const raw: unknown = JSON.parse(localStorage.getItem(MACHINES_KEY) ?? "[]");
    if (Array.isArray(raw)) {
      remote = raw.map(parse).filter((machine): machine is Machine => machine !== null);
    }
  } catch {
    // Unparseable storage is the same as none.
  }

  return [localMachine(), ...remote];
}

/** Persist everything but the local machine, which is not configuration. */
export function rememberMachines(machines: Machine[]): void {
  localStorage.setItem(
    MACHINES_KEY,
    JSON.stringify(machines.filter((machine) => !isLocal(machine))),
  );
}
