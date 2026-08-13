import { useState } from "react";
import { useParams } from "react-router";

import { SessionTerminal, type ConnectionState } from "@/components/terminal";
import { useDaemon } from "@/lib/connection";

/**
 * One session's live terminal, filling the window.
 *
 * This is the "watch and steer" surface: what you see is what a `tmux attach`
 * in your own terminal would show, and typing here goes to the same place.
 */
export function SessionPage() {
  const { id } = useParams();
  const connection = useDaemon();
  const [readOnly, setReadOnly] = useState(false);
  const [state, setState] = useState<ConnectionState>("connecting");

  const sessionId = Number(id);
  if (!Number.isInteger(sessionId)) {
    return <p className="p-6 text-sm text-muted-foreground">No such session.</p>;
  }

  return (
    <main className="flex h-screen flex-col">
      <header className="flex items-center justify-between border-b border-border px-4 py-2">
        <h1 className="text-sm font-medium">Session {sessionId}</h1>
        <div className="flex items-center gap-3 text-xs text-muted-foreground">
          <span>{state === "connected" ? "Attached" : state}</span>
          <label className="flex items-center gap-1.5">
            <input
              type="checkbox"
              checked={readOnly}
              onChange={(event) => setReadOnly(event.target.checked)}
            />
            Read only
          </label>
        </div>
      </header>

      <SessionTerminal
        className="flex-1 p-2"
        connection={connection}
        sessionId={sessionId}
        readOnly={readOnly}
        onStateChange={setState}
      />
    </main>
  );
}
