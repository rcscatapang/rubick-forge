/**
 * Whatever was thrown, as a sentence.
 *
 * Failures arrive in three shapes: an `Error` from the API client, a plain
 * string from a Tauri command, and whatever a rejected promise felt like
 * carrying. None of them may reach the user as "undefined".
 */
export function describe(thrown: unknown): string {
  if (typeof thrown === "string") return thrown;
  if (thrown instanceof Error && thrown.message) return thrown.message;

  if (thrown && typeof thrown === "object" && "message" in thrown) {
    const message = (thrown as { message: unknown }).message;
    if (typeof message === "string" && message) return message;
  }

  return "Something went wrong.";
}
