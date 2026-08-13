/**
 * Placeholder dashboard: session states, the activity feed and worktree status
 * are not built yet. This page exists so the scaffold has a route to render and
 * a window to open.
 */
export function DashboardPage() {
  return (
    <main className="mx-auto flex min-h-screen max-w-2xl flex-col justify-center gap-3 p-10">
      <h1 className="text-2xl font-semibold tracking-tight">Rubick Forge</h1>
      <p className="text-sm text-muted-foreground">
        Scaffold only — no daemon connection yet. This window just proves the shell builds and
        opens.
      </p>
    </main>
  );
}
