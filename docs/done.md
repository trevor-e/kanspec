# done — the close-out gate

`kanspec done <id>` is the only way a ticket reaches `done`, and it is a gate, not a
formality. It exists because the two things that historically went missing at close-out —
"what is left?" and "what did we learn?" — are exactly the two things nobody remembers to
record.

**Run it on the branch, before the PR merges.** `done` records the *close-out*, not the
landing: it writes `state: done` with the branch tip read from git and a
`closed out <sha> on <branch>` log line, and that record rides in the PR that did the work.
It is legal from `doing` (before the PR even opens) and from `review`. Whether the work is
on main is never stored in the ticket — every later `scan` derives it, and until one does
the card sits in the Review column wearing `closed out · awaiting merge`. So there is no
post-merge `kanspec: sync` commit to land, which a squash-merge team requiring a PR for
every commit could never do.

## 1. Record the head (never assert a merge)

The gate reads the branch tip **from git** — you never type a SHA — and refuses a branch
that carries no commit main lacks (unless a `Kanspec: <id>` trailer on main shows the
work already landed): a close-out of nothing is `--no-code --why`, below, and is recorded
as such.

Landing is detected afterwards, by `kanspec scan`. Detection is a four-rung ladder, and
every rung reports one of merged / not-merged / inconclusive:

| Rung | What it asks | Blind to |
|---|---|---|
| ancestry | `git merge-base --is-ancestor <head> origin/main` | squash merges |
| gh-pr | `gh pr view` / `gh pr list --head`, then re-verified locally as an ancestor | needs `gh` authed |
| trailer | `git log origin/main --grep 'Kanspec: t-9c41'` | reverts |
| patch-id | `git cherry origin/main <head>` — rebase / cherry-pick detection | multi-commit squashes |

An ancestry **miss** is inconclusive, not "no": a squash-merged branch is genuinely not an
ancestor. When every rung declines the badge is an honest `unknown` with its reason, never
a confident wrong answer, and `kanspec scan --explain t-9c41` prints the whole trace:

```
    ancestry   merge-base --is-ancestor a1b9c3d origin/main   exit 1   not an ancestor
    gh-pr      (gh unavailable: HTTP 401 Bad credentials)     exit 1   inconclusive
    trailer    log origin/main --grep 'Kanspec: t-9c41…'      exit 0   0 hits
    patch-id   cherry origin/main a1b9c3d                     exit 0   +2  squash suspected
  unknown (squash suspected, no gh)
```

A close-out git has not seen land for `[windows] landing_dwell_secs` (default seven days)
is a WATCHING line in `kanspec status` — `closed out 8d ago, not on main` — whose fix is
that `--explain`. `scan --confirm` is the recorded human override for the genuinely
ambiguous case (a gc'd branch, a hand-merged fork). It is signed and it lands in the
ticket's `## Log`, not in the disposable cache, so the attestation survives
`rm -rf .kanspec/cache/`. **Agents do not confirm merges** — report the trace and let the
human decide.

Docs and chores that ship no code use `kanspec done <id> --no-code --why "..."`. That is a
different, recorded escape hatch, and it cannot be used to close a ticket already in
review.

## 2. Leftover triage — every unchecked step, one of three answers

There is no fourth option and no silent default:

- `--spawn "title"` → a new ticket linked `followup_of` this one;
- `--drop-step "3:superseded by the new limiter"` → dropped with a logged reason;
- `--actually-done 3` → the step was finished, the box was not ticked.

Non-interactive callers **must** pass `--no-followups` when there is nothing left, so
"nothing left" is always a recorded claim rather than an omission.

## 3. The knowledge checkpoint

`done` maps the branch diff onto every spec's `code:` globs. If your branch touched code a
spec owns and you did not edit that spec, it asks for one of:

- the spec edit (the normal answer — specs are edited on the implementation branch and
  reviewed in the PR like code), or
- `--spec-unchanged "<spec>:<reason>"`, recorded on the ticket and visible on the board.

The gate names every uncovered spec on its own line with the glob that caught it, and takes
one answer per spec. A bare reason is accepted only when exactly one spec is uncovered; over
several it is refused as a blanket, so a real change in one of five specs matched through
broad globs cannot hide behind one answer for all of them.

Then the two one-key captures, while the burn is fresh: quirks discovered, decisions made.
Both are skippable and every skip is auditable.

## 4. What it prints

```
✓ closed out at a1b9c3d on ks/t-9c41-rate-limit-login — landing is detected by git (`kanspec scan`), never asserted here
● t-9c41-rate-limit-login done · p-7de2-login-rate-limiting is settling (last ticket closed out) → kanspec close p-7de2
  → git add -A .kanspec && git commit
  parked 2 discovered tickets → kanspec status
```

The record is dirty in your tree under the default `sync = "batch"`; the `git add` line is
how it gets into the PR — with your code, in the same commit if you like. If this was the
last live ticket of a proposal, the proposal surfaces as *settling* and close-out is
prompted rather than remembered.

## The agent form, in full

```
kanspec done t-9c41 --json \
  --spawn "test concurrent same-key requests" \
  --no-quirks --spec-unchanged "auth:refactor only, no behaviour change"
```

Everything the interactive prompts collect has a flag, and both front doors build the same
typed triage value — so the agent path and the human path record identically.
