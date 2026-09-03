# config — every knob in `.kanspec/config.toml`

`kanspec init` writes this file with every default present and commented, so the knobs are
discoverable without reading source. A **missing** config.toml is legal and means "all
defaults"; a **malformed** one is a typed refusal that names the line.

```toml
main              = "origin/main"   # the branch merge detection resolves against
id_width          = 4               # hex digits in a minted id; widens on collision
sync              = "batch"         # batch | commit | branch(v0.4)
port              = 5757            # `kanspec up` binds 127.0.0.1:<port>
branch_prefix     = "ks/"           # `start` creates <prefix><id>-<slug>
worktree_dir      = "../kanspec-wt" # `start --worktree` puts worktrees here
lock_timeout_secs = 5               # how long a verb waits for the advisory lock
```

**`main`** is what "did it land?" means here. It is a git revision, so `origin/main`,
`upstream/trunk` and `origin/develop` all work. Detection compares against this and nothing
else, and `start` cuts every ticket branch from it. A repo that integrates through some
other branch says so at scaffold time — `kanspec init --main origin/develop` — or by editing
this line; `init` warns when the branch it runs on is not on `main`'s history, and `start`
refuses to cut a ticket branch from a `main` that has never carried `.kanspec/`.

**`sync`** decides what happens to the files kanspec writes:

- `batch` (default, solo work) — mutations dirty the working tree and you commit them when
  you commit; `status` reminds you when N tracker changes are pending.
- `commit` — every verb auto-commits `kanspec: <verb> <id>` to the current branch. Full
  audit trail in history, noisier log, clean multi-machine sync.
- `branch` — team mode, v0.4.

## `[paths]` — where the generated projections land

```toml
[paths]
features     = "KANSPEC-FEATURES.md"
architecture = "KANSPEC-ARCHITECTURE.md"
```

Both are **generated to the repo root and committed**, where a teammate browsing GitHub
will find them without knowing kanspec exists. The `KANSPEC-` prefix keeps the names
collision-free and groups the pair in a directory listing; rename them freely — plain
`FEATURES.md` is yours to take if it is free. `kanspec init` **refuses to claim a path that
already exists un-generated**, so it can never eat a file you wrote by hand. Both files
carry a `GENERATED` header; edit the sources (`specs/`, `decisions/`), never the
projection.

## `[windows]` — the tripwires

```toml
[windows]
stall_secs            = 7200     # doing, no commit or update -> STALLED
review_dwell_secs     = 604800   # in review this long -> a WATCHING line
in_main_dwell_secs    = 86400    # landed but not closed
settling_dwell_secs   = 259200   # proposal's last ticket landed, not closed
discovered_dwell_secs = 604800   # a discovered_in ticket sitting untriaged
stale_merges          = 3        # merges touching a spec's globs before it is stale
fetch_max_age_secs    = 300      # older than this and a scan says so on the badge
```

These are the whole anti-stuck mechanism: each one turns "nobody noticed" into a line in
`kanspec status` carrying its own fix command. Widen them if a window nags; do not widen
them to make a real stall quiet.

`stale_merges` drives the spec staleness tripwire — N merges touching a spec's `code:`
globs with zero spec edits raises "payments: 4 merges since last spec edit". Staleness is
**recomputed from git at read time**, never accumulated in the cache, so wiping the cache
cannot silently reset it. Resolve it by editing the spec, or with
`kanspec features --confirm <spec> --why "..."`, which records a signed `stale_ack` in the
spec's own frontmatter — git-tracked, so it survives a cache wipe.

## `[git]`

```toml
[git]
fetch = true      # `git fetch origin` before a scan
gh    = "auto"    # auto | always | never — rung 2 of the merge ladder
```

`gh = "never"` is the honest setting for non-GitHub remotes: rung 2 is the only rung that
sees a title-only squash, so without it you will see more `unknown` badges — which is the
point. kanspec renders `unknown` with its reason rather than guessing.

## `[ci]` — read-only, advisory, never a state driver

```toml
[ci]
provider = "auto"    # auto | homerunner | gh | none

[ci.homerunner]
bin = "~/dev/homerunner/target/release/homerunner"
db  = "~/.local/share/homerunner/homerunner.db"
api = "http://127.0.0.1:4123"
```

`auto` resolves to homerunner when this repo's origin appears in homerunner's `[[repos]]`,
else `gh` when `gh` is authed, else none. A red build never moves a card. The reader
itself lands in v0.2; v0.1 parses the table and detects the provider so the config you
write today keeps working.

## `[prime]` — the injection budget

```toml
[prime]
spec_budget_tokens = 2000   # spec rules `prime` injects, in tokens; 0 = no budget
```

`prime` (and `rules --path`, which is byte-identical to it) injects the rules of every spec
whose `code:` globs touch the scoped paths. On a real corpus that is several specs per file
and twenty on a wide branch — measured at ~14k tokens in the worst case, into every
SessionStart and PreCompact. So matched specs are **ranked** — the spec whose glob names
the file first, then the one covering the most touched paths, then name — and shown in that
order while the budget is unspent. The budget is soft: the spec that crosses it still shows
whole, so the first-ranked spec is never cut and the payload overshoots by at most one spec.
Every spec past it is **named**, with its rule count and the command that shows it:

```
  assets — 19 rules not shown, over the prime budget → kanspec spec show assets
```

Tokens are estimated at four bytes each. `0` lifts the budget; so does `kanspec rules
--full`, for a human checking what was named but not shown. `prime` has no such flag.

## `[hooks]`

```toml
[hooks]
landcheck = false    # the Stop hook. Opt-in; v0.2.
```

`landcheck` blocks an agent session from ending while the tracker disagrees with the
working tree. It is **off by default and installed only when this is true** — a Stop hook
that blocks is the single most disruptive thing kanspec can do to a session, so it is a
deliberate opt-in rather than a surprise.

## Git hooks, and re-installing them

`init` installs four git hooks, resolved through `git rev-parse --git-path hooks` so
`core.hooksPath` (husky, lefthook) is honoured rather than silently bypassed:

| Hook | What it does |
|---|---|
| `post-merge` | `kanspec scan --quiet` — merge badges stay fresh with zero agent involvement |
| `post-checkout` | the same, on branch checkouts |
| `prepare-commit-msg` | appends the `Kanspec: t-9c41` trailer that makes a squash merge detectable |
| `commit-msg` | the same trailer, for a message typed in the editor |

Two hooks for one trailer is deliberate. `prepare-commit-msg` runs *before* the editor, so
on an interactive commit there is no message yet — and stamping an empty one would quietly
destroy git's "an empty message aborts the commit". So `prepare-commit-msg` stamps the
messages that already have content (`-m`, `-F`, `-t`) and `commit-msg`, which runs after
the editor, stamps the rest. Neither ever stamps twice, and neither ever stamps a merge or
a squash message.

Both dispatch on `branch.<name>.kanspec-ticket`, which `kanspec start` records — git has no
per-branch hooks, so that config key *is* the per-branch dispatch. On a branch without one,
the hook costs a single `git config` read.

A hook you already had is **not overwritten**: it moves to `<hook>.d/10-<hook>` and the
kanspec entrypoint runs it first, propagating its exit code. `kanspec setup <agent>
--remove` puts it back exactly.

If your repo was initialised before a hook existed, `kanspec init --refresh-hooks`
re-installs them without touching your scaffold or your config.

The hooks invoke `kanspec` from `PATH`. Set `KANSPEC_BIN` to an absolute path if the binary
is not on the PATH that git sees.
