import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { useEffect, useRef, useState } from "react";
import "@xterm/xterm/css/xterm.css";

import { type DaemonConnection, socketUrl } from "@/lib/daemon";
import { cn } from "@/lib/utils";

export type ConnectionState = "connecting" | "connected" | "closed";

/** How long to wait before reattaching, per attempt. */
const RETRY_DELAYS_MS = [500, 1_000, 2_000, 5_000];

interface Props {
  connection: DaemonConnection;
  sessionId: number;
  /** Watch without being able to type. The daemon enforces it too. */
  readOnly?: boolean;
  onStateChange?: (state: ConnectionState) => void;
  className?: string;
}

/**
 * A live terminal for one session.
 *
 * The daemon runs `tmux attach` in a pty and proxies raw bytes; this renders
 * them and sends keystrokes back. Closing it detaches — the agent carries on.
 */
export function SessionTerminal({
  connection,
  sessionId,
  readOnly = false,
  onStateChange,
  className,
}: Props) {
  const host = useRef<HTMLDivElement>(null);
  const socketRef = useRef<WebSocket | null>(null);
  const [state, setState] = useState<ConnectionState>("connecting");

  // Read inside handlers that outlive the render they were created in.
  const notify = useRef(onStateChange);
  const readOnlyRef = useRef(readOnly);

  useEffect(() => {
    notify.current = onStateChange;
  }, [onStateChange]);

  // Toggling read-only tells the daemon rather than reconnecting: retaking the
  // terminal would lose everything on screen.
  useEffect(() => {
    readOnlyRef.current = readOnly;
    const socket = socketRef.current;
    if (socket?.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify({ type: "read_only", value: readOnly }));
    }
  }, [readOnly]);

  useEffect(() => {
    if (!host.current) return;

    const terminal = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      fontFamily: "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, monospace",
      fontSize: 12,
      // The agent CLIs draw their own interface; leave the colours to them.
      theme: { background: "#00000000" },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host.current);
    fit.fit();

    let disposed = false;
    let attempt = 0;
    let retry: ReturnType<typeof setTimeout> | undefined;

    const move = (next: ConnectionState) => {
      if (disposed) return;
      setState(next);
      notify.current?.(next);
    };

    const sendSize = () => {
      const socket = socketRef.current;
      if (socket?.readyState !== WebSocket.OPEN) return;
      socket.send(
        JSON.stringify({ type: "resize", cols: terminal.cols, rows: terminal.rows }),
      );
    };

    const connect = () => {
      if (disposed) return;
      move("connecting");

      const socket = new WebSocket(
        socketUrl(connection, `ws/sessions/${sessionId}/terminal`, {
          cols: terminal.cols,
          rows: terminal.rows,
          read_only: readOnlyRef.current,
        }),
      );
      socket.binaryType = "arraybuffer";
      socketRef.current = socket;

      socket.onopen = () => {
        attempt = 0;
        move("connected");
      };

      socket.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer) {
          terminal.write(new Uint8Array(event.data));
        }
      };

      socket.onclose = () => {
        if (disposed || socketRef.current !== socket) return;
        move("closed");

        // The session may simply have ended, so give up rather than retry
        // forever; reopening the view starts again.
        const delay = RETRY_DELAYS_MS[attempt];
        if (delay !== undefined) {
          attempt += 1;
          retry = setTimeout(connect, delay);
        }
      };
    };

    connect();

    const typed = terminal.onData((data) => {
      const socket = socketRef.current;
      if (readOnlyRef.current || socket?.readyState !== WebSocket.OPEN) return;
      socket.send(new TextEncoder().encode(data));
    });

    // tmux sizes a window to its smallest client, so telling the daemon the
    // real size is what stops the pane being cropped to 80×24.
    const observer = new ResizeObserver(() => {
      fit.fit();
      sendSize();
    });
    observer.observe(host.current);

    return () => {
      disposed = true;
      clearTimeout(retry);
      observer.disconnect();
      typed.dispose();
      socketRef.current?.close();
      socketRef.current = null;
      terminal.dispose();
    };
  }, [connection, sessionId]);

  return (
    <div className={cn("relative h-full w-full overflow-hidden", className)}>
      <div ref={host} className="h-full w-full" />
      {state !== "connected" && (
        <div className="absolute inset-x-0 top-0 bg-muted px-3 py-1 text-xs text-muted-foreground">
          {state === "connecting"
            ? "Attaching to the session…"
            : "Detached. The agent is still running."}
        </div>
      )}
    </div>
  );
}
