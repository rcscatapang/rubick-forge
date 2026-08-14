import { useEffect, useState } from "react";
import { Link, useParams } from "react-router";

import { useWatching } from "@/components/live-daemon";
import { SessionTerminal, type ConnectionState } from "@/components/terminal";
import { DaemonProvider, useDaemon, useMachineId } from "@/lib/connection";
import { useMachines } from "@/lib/machine-registry";
import { useSession } from "@/lib/queries";

/**
 * One session's live terminal, filling the window.
 *
 * The route names the machine as well as the session: session 3 exists on
 * every daemon, and they are not the same session.
 */
export function SessionPage() {
  const { machineId, id } = useParams();
  const { machines } = useMachines();

  const found = machines.find((candidate) => candidate.machine.id === machineId);

  if (!found) {
    return (
      <Gone>
        That machine is not on this app&rsquo;s list any more.
      </Gone>
    );
  }

  return (
    <DaemonProvider machineId={found.machine.id} connection={found.connection}>
      <SessionTerminalPage sessionId={Number(id)} />
    </DaemonProvider>
  );
}

function SessionTerminalPage({ sessionId }: { sessionId: number }) {
  const connection = useDaemon();
  const machineId = useMachineId();
  const { watch } = useWatching();
  const [readOnly, setReadOnly] = useState(false);
  const [state, setState] = useState<ConnectionState>("connecting");

  // Which task this session belongs to; the route knows only the session.
  const session = useSession(sessionId);
  const taskId = session.data?.task_id ?? null;

  // While this is on screen, the app has no reason to tell you about it.
  useEffect(() => {
    watch(taskId === null ? null : { machineId, taskId });
    return () => watch(null);
  }, [machineId, taskId, watch]);

  if (!Number.isInteger(sessionId)) {
    return <Gone>No such session.</Gone>;
  }

  // A notification outlives the session it came from, so landing here on one
  // that has been cleaned up is ordinary, not an error worth a stack trace.
  if (session.isError) {
    return <Gone>That session is gone. The agent may have been stopped since.</Gone>;
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

function Gone({ children }: { children: React.ReactNode }) {
  return (
    <main className="flex flex-col gap-3 p-6 text-sm text-muted-foreground">
      <p>{children}</p>
      <Link className="text-xs underline" to="/">
        Back to the dashboard
      </Link>
    </main>
  );
}
