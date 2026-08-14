/**
 * Whether the app and a daemon are close enough to understand each other.
 *
 * With several machines, one of them will be behind. That has to read as a
 * warning on that machine's chip rather than as unexplained failures halfway
 * through the dashboard.
 */

export type Skew = "ok" | "daemon-older" | "daemon-newer" | "unknown";

interface Version {
  major: number;
  minor: number;
}

/** `"0.2.1"`, or `"0.2.1-rc1"` — everything after the minor is ignored. */
function parse(version: string): Version | null {
  const match = /^(\d+)\.(\d+)/.exec(version.trim());
  if (!match) return null;

  return { major: Number(match[1]), minor: Number(match[2]) };
}

/**
 * How a daemon's version compares with the app's.
 *
 * The patch level is deliberately not compared: within a minor version the API
 * is the same, and a chip that complains about every point release is a chip
 * people learn to ignore.
 *
 * Before 1.0 the minor version is where breaking changes land, so it is
 * compared like a major one.
 */
export function skew(app: string, daemon: string): Skew {
  const ours = parse(app);
  const theirs = parse(daemon);
  if (!ours || !theirs) return "unknown";

  if (ours.major !== theirs.major) {
    return theirs.major < ours.major ? "daemon-older" : "daemon-newer";
  }
  if (ours.minor !== theirs.minor) {
    return theirs.minor < ours.minor ? "daemon-older" : "daemon-newer";
  }

  return "ok";
}

/** What to tell someone about a mismatch, or `null` when there is nothing to say. */
export function describeSkew(state: Skew, daemon: string): string | null {
  switch (state) {
    case "ok":
      return null;
    case "daemon-older":
      return `Daemon ${daemon} is older than this app. Some things may be missing.`;
    case "daemon-newer":
      return `Daemon ${daemon} is newer than this app. Update the app.`;
    case "unknown":
      return `Cannot tell what version daemon "${daemon}" is.`;
  }
}
