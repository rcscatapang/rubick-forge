import { describe, expect, it } from "vitest";

import { describeSkew, skew } from "@/lib/version";

describe("comparing the app with a daemon", () => {
  it("is happy within a minor version", () => {
    expect(skew("0.2.0", "0.2.0")).toBe("ok");
    // The patch level is not compared: within a minor version the API is the
    // same, and a warning on every point release is one people learn to ignore.
    expect(skew("0.2.0", "0.2.7")).toBe("ok");
    expect(skew("0.2.3", "0.2.0")).toBe("ok");
  });

  it("says which side is behind", () => {
    expect(skew("0.2.0", "0.1.9")).toBe("daemon-older");
    expect(skew("0.2.0", "0.3.0")).toBe("daemon-newer");
    expect(skew("1.0.0", "0.9.0")).toBe("daemon-older");
    expect(skew("0.9.0", "1.0.0")).toBe("daemon-newer");
  });

  it("admits when it cannot tell", () => {
    expect(skew("0.2.0", "")).toBe("unknown");
    expect(skew("0.2.0", "dev")).toBe("unknown");
    expect(skew("", "0.2.0")).toBe("unknown");
  });

  it("ignores a pre-release suffix", () => {
    expect(skew("0.2.0", "0.2.0-rc1")).toBe("ok");
  });
});

describe("what to say about a mismatch", () => {
  it("says nothing when there is nothing to say", () => {
    expect(describeSkew("ok", "0.2.0")).toBeNull();
  });

  it("names the version and what to do about it", () => {
    expect(describeSkew("daemon-newer", "0.3.0")).toContain("Update the app");
    expect(describeSkew("daemon-older", "0.1.0")).toContain("0.1.0");
    expect(describeSkew("unknown", "dev")).toContain("dev");
  });
});
