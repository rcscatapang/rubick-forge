import type { EventRecord, Project, Task } from "@/lib/api-types";

/** The three kinds worth interrupting someone for. */
export const NOTIFIED_KINDS = ["agent_waiting", "task_finished", "agent_error"] as const;
export type NotifiedKind = (typeof NOTIFIED_KINDS)[number];

/** Whether each kind is on. Settings live in the daemon so future clients share them. */
export type NotificationPrefs = Record<NotifiedKind, boolean>;

export const DEFAULT_PREFS: NotificationPrefs = {
  agent_waiting: true,
  task_finished: true,
  agent_error: true,
};

/** The daemon settings key for one kind. */
export function prefKey(kind: NotifiedKind): string {
  return `notify.${kind}`;
}

export interface Notification {
  kind: NotifiedKind;
  title: string;
  body: string;
  /** Where clicking it should land. */
  taskId: number;
}

/**
 * How much of a pane tail a notification carries.
 *
 * macOS truncates well before this; the cap is about not handing the system a
 * screenful, not about what it displays.
 */
const MAX_BODY = 240;

/**
 * Everything that draws a terminal rather than saying something.
 *
 * CSI parameters cover the whole range a terminal may send, not just the
 * digits and semicolons of a colour code: `\x1b[>4;2m` and `\x1b[=…` are real,
 * and half-stripping one leaves its tail as literal garbage in a
 * notification.
 */
const ANSI = /\x1b\[[\x30-\x3f]*[\x20-\x2f]*[\x40-\x7e]|\x1b\][^\u0007]*(?:\u0007|\x1b\\)|\x1b[@-Z\\-_]/g;

/**
 * Terminal output as a line a person can read at a glance.
 *
 * A pane tail is a rectangle of a redrawing TUI: escape sequences, box
 * drawing, and whatever blank space was left over.
 */
export function sanitise(tail: string): string {
  const text = tail
    .replace(ANSI, "")
    .split("\n")
    .map((line) => line.replace(/[\u2502\u2503\u2506\u250a\u2551\u254e\u254f]/g, " ").trimEnd())
    .filter((line) => line.trim() !== "" && !/^[\s\u2500\u2501\u2504\u2508\u254c\u2550\u254d-]+$/.test(line))
    .join(" \u00b7 ")
    .replace(/\s+/g, " ")
    .trim();

  return text.length > MAX_BODY ? `${text.slice(0, MAX_BODY).trimEnd()}\u2026` : text;
}

interface Context {
  tasks: Task[];
  projects: Project[];
}

/** "Add adapters - forge", or as much of it as is known. */
function name(taskId: number, { tasks, projects }: Context): string {
  const task = tasks.find((candidate) => candidate.id === taskId);
  if (!task) return `Task ${taskId}`;

  const project = projects.find((candidate) => candidate.id === task.project_id);
  return project ? `${task.title} \u2014 ${project.name}` : task.title;
}

/**
 * What, if anything, to show a person about this event.
 *
 * Returns `null` for everything that is not one of the three signal kinds, for
 * a kind the user turned off, and for the task they are already looking at in
 * a focused window - telling someone what is on their screen is noise.
 */
export function notificationFor(
  event: EventRecord,
  context: Context & {
    prefs: NotificationPrefs;
    /** The task whose terminal is on screen, when the window has focus. */
    watching?: number | null;
  },
): Notification | null {
  if (!isNotified(event.kind)) return null;
  if (!context.prefs[event.kind]) return null;

  const taskId = "task_id" in event ? event.task_id : null;
  if (taskId === null) return null;
  if (context.watching === taskId) return null;

  const title = name(taskId, context);

  switch (event.kind) {
    case "agent_waiting":
      return { kind: event.kind, taskId, title, body: sanitise(event.tail) || "Waiting for you." };
    case "agent_error":
      return {
        kind: event.kind,
        taskId,
        title,
        body: sanitise(event.detail) || "The agent errored.",
      };
    case "task_finished":
      return { kind: event.kind, taskId, title, body: "Finished." };
  }
}

function isNotified(kind: string): kind is NotifiedKind {
  return (NOTIFIED_KINDS as readonly string[]).includes(kind);
}

/**
 * Whether this is the same interruption as the last one.
 *
 * An agent that keeps asking while nobody answers should not restack. The
 * daemon already fires `agent_waiting` only on *entering* the state, so this
 * is the second line of defence - a restart puts a task back into waiting, and
 * that is not news either until something else has happened to it.
 */
export function isRepeat(notification: Notification, last: Map<number, NotifiedKind>): boolean {
  return last.get(notification.taskId) === notification.kind;
}
