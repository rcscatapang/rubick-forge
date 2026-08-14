import { useState } from "react";
import { toast } from "sonner";

import { describe } from "@/lib/errors";
import { useGitHub, useGitHubActions } from "@/lib/queries";

/**
 * The GitHub token, and nothing else.
 *
 * The token is checked with GitHub before it is stored, so a token that cannot
 * open a pull request fails here rather than at the moment someone tries to.
 * It goes straight to the daemon's keychain; this app never keeps a copy.
 */
export function GitHubSettings() {
  const github = useGitHub();
  const actions = useGitHubActions();
  const [token, setToken] = useState("");

  const save = async () => {
    try {
      await actions.saveToken.mutateAsync(token);
      setToken("");
      toast.success("GitHub is connected.");
    } catch (error) {
      toast.error(describe(error));
    }
  };

  const forget = async () => {
    try {
      await actions.forgetToken.mutateAsync();
      toast.success("Forgotten.");
    } catch (error) {
      toast.error(describe(error));
    }
  };

  const configured = github.data?.configured ?? false;

  return (
    <section className="flex flex-col gap-2 text-xs">
      <h2 className="text-sm font-semibold">GitHub</h2>

      {configured ? (
        <div className="flex items-center justify-between gap-3">
          <p className="text-muted-foreground">
            Connected. Pull requests and CI status are polled every minute — no
            webhooks, so nothing is exposed.
          </p>
          <button
            type="button"
            className="shrink-0 rounded border border-border px-2 py-1 text-[var(--status-error)]"
            onClick={() => void forget()}
          >
            Forget token
          </button>
        </div>
      ) : (
        <>
          <p className="text-muted-foreground">
            Paste a personal access token with the <code>repo</code> scope. It
            is checked with GitHub, then kept in this Mac&rsquo;s keychain.
          </p>
          <div className="flex gap-1.5">
            <input
              type="password"
              className="flex-1 rounded border border-border bg-transparent px-2 py-1"
              placeholder="ghp_…"
              value={token}
              onChange={(event) => setToken(event.target.value)}
            />
            <button
              type="button"
              className="rounded bg-primary px-2 py-1 text-primary-foreground disabled:opacity-60"
              disabled={!token.trim() || actions.saveToken.isPending}
              onClick={() => void save()}
            >
              {actions.saveToken.isPending ? "Checking…" : "Save"}
            </button>
          </div>
        </>
      )}
    </section>
  );
}
