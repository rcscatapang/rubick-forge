#!/usr/bin/env python3
"""Enforce the D3 boundary: the desktop shell is an API client, nothing more.

Reads Cargo's own view of the manifest rather than grepping it, so renamed
(`db = { package = "rusqlite" }`) and table-form (`[dependencies.rusqlite]`)
declarations are caught too. The list is an allowlist on purpose: adding a
dependency to the shell is a spec decision, so it should require editing this
file and saying why.
"""

import json
import subprocess
import sys

SHELL_CRATE = "rubick-forge-desktop"

# Anything the window shell may legitimately need. Storage (rusqlite, …),
# process spawning, and git crates belong to forge-daemon and must never
# appear here.
ALLOWED = {
    "tauri",
    "tauri-build",
    "tauri-plugin-notification",  # native notifications (D20)
    "keyring",  # remote-machine tokens in the macOS keychain (SPEC §8)
    "serde",
    "serde_json",
}


def main() -> int:
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )

    shell = next((p for p in meta["packages"] if p["name"] == SHELL_CRATE), None)
    if shell is None:
        print(f"error: {SHELL_CRATE} is not a workspace member", file=sys.stderr)
        return 1

    declared = {dep["name"] for dep in shell["dependencies"]}
    unexpected = sorted(declared - ALLOWED)

    if unexpected:
        print(
            f"D3 violation: {SHELL_CRATE} declares {', '.join(unexpected)}.\n"
            "The desktop shell talks to forge-daemon over HTTP/WS; it never opens "
            "storage, spawns processes, or shells out to git. If this dependency is "
            "genuinely shell-side, add it to ALLOWED in scripts/check-d3-boundary.py "
            "with a comment saying why.",
            file=sys.stderr,
        )
        return 1

    print(f"D3 boundary ok: {SHELL_CRATE} declares {', '.join(sorted(declared))}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
