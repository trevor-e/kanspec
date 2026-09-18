#!/bin/bash
# SessionStart hook for Claude Code on the web (and any fresh clone): put the `kanspec`
# binary this repo IS onto the PATH and install its git hooks, so the agent can drive the
# tracker through kanspec verbs instead of reading `.kanspec/` files by hand.
#
# Only runs remotely: a local checkout already has `cargo install --path .` in its loop
# (CLAUDE.md), and rebuilding on every session start would be two minutes nobody asked for.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

root="${CLAUDE_PROJECT_DIR:-$(pwd)}"
export PATH="$HOME/.cargo/bin:$PATH"

if ! command -v kanspec >/dev/null 2>&1; then
  # `--locked` so a session cannot drift the lockfile; the release profile is what the
  # hooks and the merge driver will call for the rest of the session.
  cargo install --path "$root" --locked --quiet
fi

# Persist the PATH for the rest of the session — the hook's own environment dies with it.
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> "$CLAUDE_ENV_FILE"
fi

# The git hooks (trailer stamping, the post-merge scan, the projections' merge driver) are
# per-clone and a fresh clone has none. `init` over an existing store only installs them.
kanspec --repo "$root" init --refresh-hooks >/dev/null
