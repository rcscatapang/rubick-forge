import { useMutation, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { type ReactNode } from "react";
import { toast } from "sonner";

import { useConnect, useDaemon } from "@/lib/connection";
import { describe } from "@/lib/errors";
import { useHealth } from "@/lib/queries";

/**
 * Nothing renders until the daemon is reachable.
 *
 * The app is a viewer: with no daemon there is nothing to view, and pretending
 * otherwise would show stale or empty state as if it were the truth.
 */
export function ConnectionGate({ children }: { children: ReactNode }) {
  const connection = useDaemon();
  const connect = useConnect();
  const health = useHealth();
  const queries = useQueryClient();

  const install = useMutation({
    mutationFn: async () => {
      const binary = await open({
        title: "Where is forge-daemon?",
        multiple: false,
        directory: false,
      });
      if (typeof binary !== "string") return null;

      const message = await invoke<string>("install_daemon", { binary });
      const token = await invoke<string>("daemon_token");
      connect({ ...connection, token });
      return message;
    },
    onSuccess: (message) => {
      if (message === null) return;
      toast.success("The daemon is installed and running.");
      // The token changed, so everything read with the old one is worthless.
      void queries.invalidateQueries();
    },
    onError: (thrown) => toast.error(describe(thrown)),
  });

  const retry = useMutation({
    mutationFn: async () => {
      const token = await invoke<string>("daemon_token");
      connect({ ...connection, token });
    },
    onSuccess: () => void queries.invalidateQueries(),
    onError: (thrown) => toast.error(describe(thrown)),
  });

  if (health.isSuccess) {
    return <>{children}</>;
  }

  return (
    <main className="mx-auto flex min-h-screen max-w-lg flex-col justify-center gap-4 p-10">
      <h1 className="text-xl font-semibold tracking-tight">
        {health.isLoading ? "Looking for the daemon…" : "The daemon is not answering"}
      </h1>

      {!health.isLoading && (
        <>
          <p className="text-sm text-muted-foreground">
            Forge keeps your agents alive in a background daemon at{" "}
            <code className="rounded bg-muted px-1 py-0.5">{connection.url}</code>. Nothing
            is running there yet — or this app has not been given its token.
          </p>

          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="rounded-md bg-primary px-3 py-1.5 text-sm text-primary-foreground disabled:opacity-60"
              disabled={install.isPending}
              onClick={() => install.mutate()}
            >
              {install.isPending ? "Installing…" : "Install the daemon…"}
            </button>
            <button
              type="button"
              className="rounded-md border border-border px-3 py-1.5 text-sm disabled:opacity-60"
              disabled={retry.isPending}
              onClick={() => retry.mutate()}
            >
              Try again
            </button>
          </div>

          <p className="text-xs text-muted-foreground">
            You can also start it yourself with{" "}
            <code className="rounded bg-muted px-1 py-0.5">forge-daemon --install-launchd</code>
            , then choose “Try again”.
          </p>
        </>
      )}
    </main>
  );
}
