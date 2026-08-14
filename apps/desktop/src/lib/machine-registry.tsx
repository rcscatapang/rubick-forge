import { invoke } from "@tauri-apps/api/core";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { client } from "@/lib/api";
import { storedConnection } from "@/lib/connection";
import type { DaemonConnection } from "@/lib/daemon";
import {
  isLocal,
  LOCAL_MACHINE_ID,
  newMachineId,
  rememberMachines,
  storedMachines,
  type Machine,
} from "@/lib/machines";

/** A machine and the credentials to reach it. */
export interface MachineConnection {
  machine: Machine;
  connection: DaemonConnection;
}

interface Registry {
  machines: MachineConnection[];
  /** False until every machine's token has been looked up. */
  ready: boolean;
  add: (draft: MachineDraft) => Promise<Machine>;
  update: (id: string, draft: MachineDraft) => Promise<void>;
  remove: (id: string) => Promise<void>;
}

/** What the add/edit form collects. The token goes straight to the keychain. */
export interface MachineDraft {
  name: string;
  url: string;
  token: string;
  sshHost: string | null;
}

const RegistryContext = createContext<Registry | null>(null);

/**
 * The tokens obtainable without asking anyone, or `null` if any must be
 * looked up.
 *
 * The local token is usually still in browser storage from last time. Starting
 * from what is already there keeps a normal launch from blanking the window
 * while it re-reads something it had all along.
 */
function alreadyKnown(machines: Machine[]): Record<string, string> | null {
  const local = storedConnection().token;
  if (!local) return null;
  if (machines.some((machine) => !isLocal(machine))) return null;

  return { [LOCAL_MACHINE_ID]: local };
}

/**
 * A remote machine's token, from the macOS keychain.
 *
 * The local machine's comes from the file its own daemon wrote, which is the
 * one token this app has never been told.
 */
async function tokenFor(machine: Machine): Promise<string> {
  try {
    if (isLocal(machine)) {
      return storedConnection().token || (await invoke<string>("daemon_token"));
    }
    return (await invoke<string | null>("machine_token", { machine: machine.id })) ?? "";
  } catch {
    // A machine with no token still belongs on the list; it shows as
    // unauthorised rather than disappearing.
    return "";
  }
}

/**
 * Every daemon this app talks to.
 *
 * The list is the app's own: no daemon knows about any other (SPEC D17), and
 * nothing here is sent anywhere. Tokens are read once on load and whenever the
 * list changes, so a keychain entry added by hand needs an app restart and a
 * token changed through the UI does not.
 */
export function MachineRegistry({ children }: { children: ReactNode }) {
  const [machines, setMachines] = useState<Machine[]>(storedMachines);
  const [tokens, setTokens] = useState(() => alreadyKnown(machines));

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      const pairs = await Promise.all(
        machines.map(async (machine) => [machine.id, await tokenFor(machine)] as const),
      );
      if (!cancelled) setTokens(Object.fromEntries(pairs));
    })();

    return () => {
      cancelled = true;
    };
  }, [machines]);

  const persist = useCallback((next: Machine[]) => {
    rememberMachines(next);
    setMachines(next);
  }, []);

  const add = useCallback(
    async (draft: MachineDraft) => {
      const machine: Machine = {
        id: newMachineId(),
        name: draft.name,
        url: draft.url,
        sshHost: draft.sshHost,
      };

      // The keychain first: a machine on the list whose token never got saved
      // would look broken for no visible reason.
      await invoke("set_machine_token", { machine: machine.id, token: draft.token });
      persist([...machines, machine]);
      return machine;
    },
    [machines, persist],
  );

  const update = useCallback(
    async (id: string, draft: MachineDraft) => {
      // An empty token in an edit means "leave the stored one alone", so that
      // renaming a machine does not require retyping its secret.
      if (draft.token) {
        await invoke("set_machine_token", { machine: id, token: draft.token });
      }

      persist(
        machines.map((machine) =>
          machine.id === id
            ? { ...machine, name: draft.name, url: draft.url, sshHost: draft.sshHost }
            : machine,
        ),
      );
    },
    [machines, persist],
  );

  const remove = useCallback(
    async (id: string) => {
      if (id === LOCAL_MACHINE_ID) return;

      persist(machines.filter((machine) => machine.id !== id));
      // Last, and tolerated if it fails: a token left behind is tidier than a
      // machine that will not go away.
      await invoke("forget_machine_token", { machine: id }).catch(() => undefined);
    },
    [machines, persist],
  );

  const value = useMemo<Registry>(
    () => ({
      machines: machines.map((machine) => ({
        machine,
        connection: { url: machine.url, token: tokens?.[machine.id] ?? "" },
      })),
      ready: tokens !== null,
      add,
      update,
      remove,
    }),
    [machines, tokens, add, update, remove],
  );

  // Nothing renders against a machine whose token is still being looked up.
  // An empty token is not "no token yet", it is a request the daemon answers
  // 401 — and a query that has errored will not retry merely because the
  // connection it closed over changed underneath it.
  return (
    <RegistryContext.Provider value={value}>
      {tokens === null ? null : children}
    </RegistryContext.Provider>
  );
}

export function useMachines(): Registry {
  const value = useContext(RegistryContext);
  if (!value) {
    throw new Error("useMachines must be used inside a MachineRegistry");
  }
  return value;
}

/**
 * Ask a daemon who it is, before committing it to the list.
 *
 * `/health` is the one unauthenticated route, so this separates "nothing
 * there" from "wrong token" — which are different problems with different
 * fixes, and worth telling apart before the machine is saved.
 */
export async function testConnection(
  url: string,
  token: string,
): Promise<{ ok: true; name: string; version: string } | { ok: false; problem: string }> {
  const anonymous = client({ url, token: "" });

  let health;
  try {
    health = await anonymous.health();
  } catch {
    return { ok: false, problem: "No daemon answered. Check the address, and that it is up." };
  }

  try {
    // Any authenticated route will do; this one is cheap and always there.
    await client({ url, token }).settings();
  } catch {
    return { ok: false, problem: `${health.machine} is there, but rejected that token.` };
  }

  return { ok: true, name: health.machine, version: health.version };
}
