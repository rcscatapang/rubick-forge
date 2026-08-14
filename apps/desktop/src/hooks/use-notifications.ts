import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useRef, useState } from "react";

import type { EventRecord } from "@/lib/api-types";
import {
  DEFAULT_PREFS,
  isRepeat,
  notificationFor,
  prefKey,
  NOTIFIED_KINDS,
  type NotificationPrefs,
  type NotifiedKind,
} from "@/lib/notifications";
import { useProjects, useSettings, useTasks } from "@/lib/queries";

/** What the daemon's settings say, filled in with the defaults. */
export function prefsFrom(settings: Record<string, string> | undefined): NotificationPrefs {
  const prefs = { ...DEFAULT_PREFS };

  for (const kind of NOTIFIED_KINDS) {
    const stored = settings?.[prefKey(kind)];
    if (stored !== undefined) {
      prefs[kind] = stored !== "false";
    }
  }

  return prefs;
}

/** `unknown` covers both "not asked yet" and "asked and dismissed". */
export type PermissionState = "unknown" | "granted" | "denied";

/**
 * Ask macOS for permission to notify, once.
 *
 * Kept apart from raising them: with several machines there are several event
 * streams, and one permission prompt between them.
 */
export function useAskNotificationPermission(): PermissionState {
  const [permission, setPermission] = useState<PermissionState>("unknown");

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      if (await isPermissionGranted()) {
        if (!cancelled) setPermission("granted");
        return;
      }

      // Only an explicit refusal is a refusal: a prompt the user dismissed
      // will be asked again, and complaining about it would be wrong.
      const answer = await requestPermission();
      if (cancelled) return;
      setPermission(answer === "granted" ? "granted" : answer === "denied" ? "denied" : "unknown");
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  return permission;
}

/**
 * Turn one daemon's events into native notifications.
 *
 * This is the whole promise of walking away: the agent asks for something,
 * finishes, or breaks, and the Mac says so. Anything else it said would be
 * noise, and noise gets notifications turned off.
 */
export function useNotifications(
  /** The task whose terminal is on screen, when this window has focus. */
  watching?: number | null,
) {
  const tasks = useTasks();
  const projects = useProjects();
  const settings = useSettings();

  // The last kind announced per task, so an agent that keeps asking while
  // nobody answers does not restack.
  const announced = useRef(new Map<number, NotifiedKind>());

  const prefs = prefsFrom(settings.data?.settings);

  // Read through a ref: this callback is handed to a socket that outlives the
  // render it was created in.
  const current = useRef({ prefs, watching, tasks: tasks.data, projects: projects.data });
  useEffect(() => {
    current.current = { prefs, watching, tasks: tasks.data, projects: projects.data };
  });

  const notify = useCallback((event: EventRecord) => {
    const { prefs, watching, tasks, projects } = current.current;
    const taskId = "task_id" in event ? event.task_id : null;

    const notification = notificationFor(event, {
      prefs,
      watching,
      tasks: tasks?.tasks ?? [],
      projects: projects?.projects ?? [],
    });

    // Anything else happening to a task means the last thing announced about
    // it is no longer the current state, so the next one is news again.
    if (!notification) {
      if (taskId !== null) announced.current.delete(taskId);
      return;
    }

    if (isRepeat(notification, announced.current)) return;
    announced.current.set(notification.taskId, notification.kind);

    sendNotification({ title: notification.title, body: notification.body });
  }, []);

  return notify;
}
