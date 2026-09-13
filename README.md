# kanspec

A kanban board and spec/decision/quirk tracker for coding agents, over plain git-tracked files.

`.kanspec/` holds one markdown file per entity — tickets, living specs, decisions, quirks. One
static Rust binary turns them into a terminal board, a localhost review surface, and a validated CLI
that agents drive. The lifecycle lives in the binary as typed transitions, not in prose procedures an
agent has to remember.

Derived facts — merged-to-main, staleness, stuck-ness — are **computed from git, never asserted**.
No command, UI action, or file field can claim a ticket landed.

## Install

```sh
cargo install --path .        # installs both `kanspec` and its `ks` alias
```

Needs `git`. `gh` is optional and only improves squash-merge detection.

### Prebuilt binaries

Every [release](https://github.com/trevor-e/kanspec/releases) ships an archive per platform
holding both `kanspec` and `ks`, plus a `SHA256SUMS` file. The Linux archives are static musl
builds, so they run on any distro or container image. To put the latest Linux build on the PATH
of a machine without a Rust toolchain (a CI job, a Claude Code cloud session):

```sh
mkdir -p ~/.local/bin
curl -fsSL https://github.com/trevor-e/kanspec/releases/latest/download/kanspec-x86_64-unknown-linux-musl.tar.gz \
  | tar -xz -C ~/.local/bin kanspec ks
```

Targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `aarch64-apple-darwin`,
`x86_64-apple-darwin` (`.tar.gz`) and `x86_64-pc-windows-msvc` (`.zip`). Releases are cut by
the `Release` workflow: push a `v*` tag, or run it by hand with a tag name.

### Windows

Native Windows builds use `LockFileEx`; no WSL or background service is needed.
Install [Rust](https://rust-lang.org/tools/install/) and Git for Windows, then run
from this checkout in PowerShell:

```powershell
.\scripts\install-windows.ps1
kanspec --version
kanspec prime
```

The default Rust MSVC toolchain needs the Visual Studio C++ build tools. With a
Rust GNU toolchain, pass the `bin` directory of a complete GCC/MinGW toolchain:

```powershell
.\scripts\install-windows.ps1 -GnuBin C:\tools\mingw64\bin
```

The script installs `kanspec.exe`, `ks.exe`, and `kanspec-update.cmd` in
`%USERPROFILE%\.local\bin` and adds that directory to your **user PATH** without
replacing its existing entries. Restart an existing terminal or app only if its
PATH does not already contain that directory.

Run `kanspec-update` to fast-forward the installed source checkout's current
tracked branch and rebuild into the same location. Local changes or divergent
history stop the update; a build failure leaves the previous executables usable.
To select another existing source branch, use `kanspec-update -Branch main`.
This is an explicit update command, not an automatic background updater. Keep the
source checkout in a stable location because the update command points to it.

Windows and Linux CI run the tests, formatting, and lint checks; Windows CI also
produces an artifact containing both release executables.

## The daily loop

```sh
kanspec init                      # scaffold .kanspec/, .gitattributes, git hooks
kanspec setup claude              # install the agent snippet + session hooks

kanspec new "Rate-limit login" --spec auth
kanspec ready                     # todo tickets with every dep satisfied
kanspec start t-9c41              # atomic claim; creates ks/t-9c41-rate-limit-login
kanspec ship  t-9c41 --pr 142     # records the head SHA from git, never typed
kanspec scan                      # merge detection: ancestry -> gh -> trailer -> patch-id
kanspec done  t-9c41              # the close-out gate; refuses without a git-detected merge

kanspec status                    # everything non-terminal, grouped by who owes the next verb
kanspec up                        # live board on 127.0.0.1:5757
```

Every command takes `--json`. Errors name the exact next command to run. Long-form workflow docs
ship inside the binary: `kanspec instructions [start|done|review|close|config]`.

## What it does

- **Tickets on a board**, linked to specs and proposals, showing branch, worktree and claiming agent.
- **Merge detection** as a four-rung ladder, where a rung can answer merged / not-merged /
  *inconclusive*. Ancestry can prove a merge but not disprove one, so a squash falls through to `gh`,
  a commit trailer, then patch-id. When every rung is inconclusive the answer is `unknown` **with the
  reason**, never a guess. `kanspec scan --explain <id>` shows the whole ladder.
- **Standing rules** — accepted decisions, active quirks, spec rules — injected into agent sessions
  path-scoped by `kanspec prime`. `kanspec rules` is byte-identical to prime's rules section, so what
  you audit is exactly what steers agents. Agents mint *proposed* decisions; only a human accepts.
- **Anti-rot**: a staleness tripwire fires when merges touch a spec's `code:` globs without the spec
  being edited, and `KANSPEC-FEATURES.md` / `KANSPEC-ARCHITECTURE.md` regenerate at the repo root so
  a teammate reads current truth without knowing kanspec exists.
- **Nothing gets stuck**: every non-terminal item has one owed verb and one owner, and `kanspec
  status` prints them all with the fix command.

## Configuration

`.kanspec/config.toml`, written commented by `init`. Notable knobs: `sync` (`batch` default — leaves
tracker changes for you to commit), `worktree` (whether `start` makes a linked worktree unasked; off
by default), `[paths]` to rename the generated projections, `[git] gh` (whether the ladder consults
`gh`), `[ci] provider`.

## Status

The package still carries the `0.1.0` version while the next slices land. The daily loop, merge
detection, knowledge layer, board and server, proposals, review pages, comment round-trip, and
`landcheck` are implemented and tested. The planned import/backtest workflow and CI-provider reader
are not implemented yet. Path installs include the source revision in `kanspec --version`, because
the package version alone cannot distinguish an older local build from the current checkout.

Sharp edges worth knowing:

- **Prefer `start --worktree`** (or set `worktree = true`). Claiming in place leaves the tracker
  dirty on your ticket branch, so a reflexive `git add -A` commits `.kanspec/` where main never sees
  it. kanspec detects and recovers that, but escaping with `git stash` or `checkout -f` leaves it
  blind.
- **Put `doctor` in CI, not `status`** — only `doctor` exits non-zero. Glob liveness is checked
  against tracked files on every doctor run, so a brand-new uncommitted file does not make its glob
  live yet.
- `doctor` **detects** forged state, it does not prevent it. The log is a plain text file. A careless
  forgery is caught by replay and by corroboration against git; a careful one — a copied close note
  on a ticket git has never definitively answered "no" about — is not. Merge state is unaffected
  either way, because it is never read out of a file.
- The `gh` rung has not yet run against a real GitHub remote.
- There is no tagged release yet. Pinning a Git revision is currently the reproducible way to put
  the same binary in CI; an unpinned path install follows whatever checkout happens to be present.

## Docs

- [`DESIGN.md`](DESIGN.md) — the product design: philosophy, data model, lifecycle, invariants, and
  an honest risks section.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — the code contract: module tree, the shared Rust types, the
  single write path, and 24 resolved design ambiguities.
