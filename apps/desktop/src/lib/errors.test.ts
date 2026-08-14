import { describe as suite, expect, it } from "vitest";

import { ApiError } from "@/lib/api";
import { describe } from "@/lib/errors";

suite("describing a failure", () => {
  it("keeps what the daemon said", () => {
    expect(describe(new ApiError("that branch does not exist", "bad_request", 400))).toBe(
      "that branch does not exist",
    );
  });

  it("handles a tauri command, which rejects with a plain string", () => {
    expect(describe("there is no daemon binary at /nope")).toBe(
      "there is no daemon binary at /nope",
    );
  });

  it("never shows the user undefined", () => {
    for (const thrown of [undefined, null, {}, new Error(""), 42]) {
      expect(describe(thrown)).toBe("Something went wrong.");
    }
  });
});
