# start — claiming work

Claiming is the one thing you must do *before* you write code. It is atomic across every
worktree and every agent on this machine, so a claim is also a lock: nobody else picks up
what you are holding.

## The loop

```
kanspec ready --json           # what is claimable right now (todo, every dep satisfied)
kanspec start t-9c41           # claim it: todo -> doing, branch created, claim logged
kanspec start t-9c41 --worktree  # ... and a linked worktree, so parallel agents don't collide
```

`start` does five things in one transaction:

1. checks the transition is legal (`todo -> doing`, or `review -> doing` for rework) and
   that nobody else holds the claim;
2. creates the branch `ks/<id>-<slug>` (`branch_prefix` is configurable) from your
   configured main, `--no-track` so a later `git push` cannot target main by accident;
3. with `--worktree`, adds a linked worktree under `worktree_dir` (`../kanspec-wt/<id>`);
4. records `branch.<name>.kanspec-ticket` in git config — this is what the
   `prepare-commit-msg` hook reads to stamp the `Kanspec: <id>` trailer on every commit,
   which is what keeps a squash merge detectable months later;
5. appends an actor-stamped line to the ticket's `## Log`. Every state in kanspec has a
   log line proving it was reached legally; a hand-edited `state:` has none and fails at
   the next verb.

Step 2 is checked before anything is created. If the configured `main` has never carried
`.kanspec/` — the store lives on a branch main does not contain — `start` refuses with the
config line to set (`main = "origin/<branch>"`), because a ticket branch that cannot see
the board is not a claim. If `main` is merely behind (the store is committed locally, not
pushed), the fix it names is `git push`. When the cut is fine but HEAD still carries commits
main lacks, the claim goes through with one `⚠ base` line saying the ticket branch starts
without them.

## What you get back

```
  claimed  t-9c41  Rate-limit login endpoint          (logged: doing · claude/sess-a91)
  branch   ks/t-9c41-rate-limit-login    worktree ../kanspec-wt/t-9c41
  context  spec auth (3 rules) · 1 decision in scope (D-8c1a) · 2 quirks match paths
  board    http://127.0.0.1:5757/t/t-9c41
```

The `context` line is not decoration: those are the standing rules that steer this ticket.
Read them before the first edit — `kanspec rules --path <file>` prints the full text for
any path you are about to touch.

## While you work

- **Out-of-scope work you uncover** (anything over about five minutes) is a new ticket, not
  scope creep: `kanspec new "..."` stamps `discovered_in` to your claimed ticket
  automatically. Discovered tickets never block your `done`, so capturing a rabbit hole
  costs nothing.
- **A landmine you learned the hard way** is a quirk:
  `kanspec quirk add "..." --paths 'src/billing/**'`. The `PostToolUse` hook re-warns the
  next agent at the moment it writes a matching file.
- **A real architectural call** made mid-implementation is a decision:
  `kanspec decide "..." --from t-9c41`. It lands as *proposed*; only a human accepts it.
- **Stopping for now?** `kanspec park t-9c41 --why "..."` puts it back in todo and releases
  the claim, so it does not rot as a stalled card. Abandoning it for good is
  `kanspec drop t-9c41 --why "..."`.

If you go quiet on a claimed ticket for longer than the `stall_secs` window (default 2h)
with no commits and no updates, `kanspec status` raises it as **STALLED**. That is a
prompt, not a punishment: park it or push something.

## When the diff is ready

`kanspec ship t-9c41 --pr 142` moves `doing -> review` and records the branch tip SHA
**read from git**. You never type a SHA and you never claim a merge; see
`kanspec instructions done`.
