# done — the close-out gate

`kanspec done <id>` is the only way a ticket reaches `done`, and it is a gate, not a
formality. It exists because the two things that historically went missing at close-out —
"did this actually land?" and "what did we learn?" — are exactly the two things nobody
remembers to record.

## 1. Verify it merged (never assert it)

The gate re-runs merge detection against git **at close time**; a cached "merged" from a
minute ago is not proof. Detection is a four-rung ladder, and every rung reports one of
merged / not-merged / inconclusive:

| Rung | What it asks | Blind to |
|---|---|---|
| ancestry | `git merge-base --is-ancestor <head> origin/main` | squash merges |
| gh-pr | `gh pr view` / `gh pr list --head`, then re-verified locally as an ancestor | needs `gh` authed |
| trailer | `git log origin/main --grep 'Kanspec: t-9c41'` | reverts |
| patch-id | `git cherry origin/main <head>` — rebase / cherry-pick detection | multi-commit squashes |

An ancestry **miss** is inconclusive, not "no": a squash-merged branch is genuinely not an
ancestor. When every rung declines you get an honest `unknown` with its reason and the
whole trace, never a confident wrong answer:

```
✗ t-9c41 is not on origin/main — cannot close it
    ancestry   merge-base --is-ancestor a1b9c3d origin/main   exit 1   not an ancestor
    gh-pr      (gh unavailable: HTTP 401 Bad credentials)     exit 1   inconclusive
    trailer    log origin/main --grep 'Kanspec: t-9c41…'      exit 0   0 hits
    patch-id   cherry origin/main a1b9c3d                     exit 0   +2  squash suspected
  unknown (squash suspected, no gh)
  → kanspec scan --explain t-9c41
  → kanspec scan --confirm t-9c41 --why "..."
```

`scan --confirm` is the recorded human override for the genuinely ambiguous case. It is
signed and it lands in the ticket's `## Log`, not in the disposable cache, so the
attestation survives `rm -rf .kanspec/cache/`. **Agents do not confirm merges** — report
the trace and let the human decide.

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
● t-9c41 done · p-7de2 is settling (last ticket landed) → kanspec close p-7de2
  parked 2 discovered tickets → kanspec status
```

If this was the last live ticket of a proposal, the proposal surfaces as *settling* and
close-out is prompted rather than remembered.

## The agent form, in full

```
kanspec done t-9c41 --json \
  --spawn "test concurrent same-key requests" \
  --no-quirks --spec-unchanged "auth:refactor only, no behaviour change"
```

Everything the interactive prompts collect has a flag, and both front doors build the same
typed triage value — so the agent path and the human path record identically.
