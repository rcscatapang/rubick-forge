import { useState } from "react";
import { toast } from "sonner";

import { describe } from "@/lib/errors";
import {
  testConnection,
  useMachines,
  type MachineDraft,
} from "@/lib/machine-registry";
import { isLocal, normaliseUrl, type Machine } from "@/lib/machines";

const EMPTY: MachineDraft = { name: "", url: "", token: "", sshHost: null };

/**
 * The machines this app talks to.
 *
 * Adding one is the whole of "going multi-machine": every Mac already runs the
 * same daemon, and none of them needs to know about the others.
 */
export function MachineSettings() {
  const { machines, add, update, remove } = useMachines();
  const [editing, setEditing] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  return (
    <section className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold">Machines</h2>
        {!adding && (
          <button
            type="button"
            className="rounded-md border border-border px-2 py-1 text-xs"
            onClick={() => setAdding(true)}
          >
            Add a machine…
          </button>
        )}
      </div>

      <p className="text-xs text-muted-foreground">
        Every Mac runs its own daemon. Reach one over your tailnet by its
        Tailscale address — never a public one — and its token is kept in this
        Mac&rsquo;s keychain.
      </p>

      <ul className="flex flex-col gap-2">
        {machines.map(({ machine }) => (
          <li key={machine.id} className="rounded-lg border border-border p-3">
            {editing === machine.id ? (
              <MachineForm
                machine={machine}
                onCancel={() => setEditing(null)}
                onSave={async (draft) => {
                  await update(machine.id, draft);
                  setEditing(null);
                }}
              />
            ) : (
              <div className="flex items-baseline justify-between gap-3">
                <div className="min-w-0">
                  <p className="truncate text-sm font-medium">{machine.name}</p>
                  <p className="truncate text-xs text-muted-foreground">
                    {machine.url}
                    {machine.sshHost && ` · ssh ${machine.sshHost}`}
                  </p>
                </div>

                {isLocal(machine) ? (
                  <span className="shrink-0 text-xs text-muted-foreground">this Mac</span>
                ) : (
                  <div className="flex shrink-0 gap-1.5 text-xs">
                    <button
                      type="button"
                      className="rounded border border-border px-2 py-1"
                      onClick={() => setEditing(machine.id)}
                    >
                      Edit
                    </button>
                    <button
                      type="button"
                      className="rounded border border-border px-2 py-1 text-[var(--status-error)]"
                      onClick={() => void remove(machine.id).catch((e) => toast.error(describe(e)))}
                    >
                      Remove
                    </button>
                  </div>
                )}
              </div>
            )}
          </li>
        ))}
      </ul>

      {adding && (
        <div className="rounded-lg border border-border p-3">
          <MachineForm
            onCancel={() => setAdding(false)}
            onSave={async (draft) => {
              await add(draft);
              setAdding(false);
            }}
          />
        </div>
      )}
    </section>
  );
}

function MachineForm({
  machine,
  onSave,
  onCancel,
}: {
  machine?: Machine;
  onSave: (draft: MachineDraft) => Promise<void>;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState<MachineDraft>(
    machine
      ? { name: machine.name, url: machine.url, token: "", sshHost: machine.sshHost }
      : EMPTY,
  );
  const [problem, setProblem] = useState<string | null>(null);
  const [tested, setTested] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const url = normaliseUrl(draft.url);

  const check = async () => {
    setBusy(true);
    setProblem(null);
    setTested(null);

    try {
      if (!url) throw new Error("That is not an http address.");
      const result = await testConnection(url, draft.token);

      if (result.ok) setTested(`${result.name}, daemon ${result.version}`);
      else setProblem(result.problem);
    } catch (error) {
      setProblem(describe(error));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!url) return setProblem("That is not an http address.");
    if (!draft.name.trim()) return setProblem("Give it a name you will recognise.");
    // Editing with the token box left blank keeps the stored one.
    if (!machine && !draft.token) return setProblem("A daemon needs its bearer token.");

    setBusy(true);
    try {
      await onSave({ ...draft, url, name: draft.name.trim() });
    } catch (error) {
      setProblem(describe(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-2 text-xs">
      <Field label="Name">
        <input
          className="w-full rounded border border-border bg-transparent px-2 py-1"
          placeholder="Mac mini"
          value={draft.name}
          onChange={(e) => setDraft({ ...draft, name: e.target.value })}
        />
      </Field>

      <Field label="Address">
        <input
          className="w-full rounded border border-border bg-transparent px-2 py-1"
          placeholder="http://100.101.102.103:8787"
          value={draft.url}
          onChange={(e) => setDraft({ ...draft, url: e.target.value })}
        />
      </Field>

      <Field label="Token">
        <input
          type="password"
          className="w-full rounded border border-border bg-transparent px-2 py-1"
          placeholder={machine ? "unchanged" : "the daemon's bearer token"}
          value={draft.token}
          onChange={(e) => setDraft({ ...draft, token: e.target.value })}
        />
      </Field>

      <Field label="ssh host">
        <input
          className="w-full rounded border border-border bg-transparent px-2 py-1"
          placeholder="optional — for “Terminal over ssh”"
          value={draft.sshHost ?? ""}
          onChange={(e) => setDraft({ ...draft, sshHost: e.target.value || null })}
        />
      </Field>

      {problem && <p className="text-[var(--status-error)]">{problem}</p>}
      {tested && <p className="text-[var(--status-idle,inherit)]">Reached {tested}.</p>}

      <div className="flex gap-1.5">
        <button
          type="button"
          className="rounded bg-primary px-2 py-1 text-primary-foreground disabled:opacity-60"
          disabled={busy}
          onClick={() => void save()}
        >
          Save
        </button>
        <button
          type="button"
          className="rounded border border-border px-2 py-1 disabled:opacity-60"
          disabled={busy}
          onClick={() => void check()}
        >
          Test
        </button>
        <button
          type="button"
          className="rounded border border-border px-2 py-1"
          onClick={onCancel}
        >
          Cancel
        </button>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}
