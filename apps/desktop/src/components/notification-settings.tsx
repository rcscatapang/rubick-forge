import { toast } from "sonner";

import { useNotificationPermission } from "@/components/live-daemon";
import { describe } from "@/lib/errors";
import { prefKey, NOTIFIED_KINDS, type NotifiedKind } from "@/lib/notifications";
import { prefsFrom } from "@/hooks/use-notifications";
import { useSettings, useUpdateSettings } from "@/lib/queries";

const LABELS: Record<NotifiedKind, string> = {
  agent_waiting: "An agent needs you",
  task_finished: "A task finishes",
  agent_error: "An agent errors",
};

/**
 * Which pings to raise. Stored in the daemon, so a phone or a second Mac
 * eventually shares the same answer.
 */
export function NotificationSettings() {
  const settings = useSettings();
  const update = useUpdateSettings();
  const permission = useNotificationPermission();

  const prefs = prefsFrom(settings.data?.settings);

  const toggle = (kind: NotifiedKind, on: boolean) => {
    update.mutate(
      { [prefKey(kind)]: on ? "true" : "false" },
      { onError: (thrown) => toast.error(describe(thrown)) },
    );
  };

  return (
    <section className="flex flex-col gap-2">
      <h2 className="text-sm font-semibold">Notify me when</h2>

      {permission === "denied" && (
        <p className="rounded-md bg-muted px-3 py-2 text-xs text-muted-foreground">
          macOS is not letting Rubick Forge send notifications. Turn them on in System
          Settings › Notifications, then reopen the app. Everything else still works — the
          dashboard shows the same states.
        </p>
      )}

      <div className="flex flex-col gap-1.5 text-xs">
        {NOTIFIED_KINDS.map((kind) => (
          <label key={kind} className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={prefs[kind]}
              disabled={settings.isLoading}
              onChange={(event) => toggle(kind, event.target.checked)}
            />
            {LABELS[kind]}
          </label>
        ))}
      </div>

      <p className="text-xs text-muted-foreground">
        Notifications need this app to be open. Away from your Mac, the Telegram bot is the
        answer — it lives in the daemon.
      </p>
    </section>
  );
}
