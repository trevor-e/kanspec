---
id: p-97d6
title: 'Close-out rides in the PR: done is recorded on the branch, landed is derived'
status: review
specs: [verbs, store, ladder]
approved: null
ledger: []
created: 2026-09-03
---
## Why
`done` refuses until it can prove the merge, so the record it writes can only exist after the merge, as a separate `kanspec: sync` commit on main. A team that squash-merges everything and requires a PR for every commit can never land that commit, and closing seventeen tickets this week took three PRs to do it. The close-out has to be written on the branch, inside the PR, before the merge — and that is only possible if `done` stops asserting a merge and the board derives "landed" from git, the way it already derives the In-main overlay. t-f8a0 holds the requirement.

## Changes
- [c1] verbs: `done` records the close-out, not the landing. It keeps every gate except the ladder — leftover triage, one spec waiver per uncovered spec, the quirk and decision captures, the `--no-code` waiver — writes `state: done` with `head:` read from git, and its log line says `closed out` rather than `merged verified`. Legal from `doing` and from `review`, so a branch closes itself before the PR opens or while it is under review.
- [c2] ladder: "landed" is derived and never stored. The Done column is `state: done` and the ladder says Landed; a `done` ticket the ladder has not detected stays in the Review column wearing a `closed out · awaiting merge` chip, the mirror of today's In-main overlay for `review` tickets already on main. Wiping the cache moves such cards back to the chip, never to a wrong column.
- [c3] verbs: a new tripwire. `done` and not detected on main after `[windows] landing_dwell_secs` (default seven days) is a WATCHING line — `closed out 8d ago, not on main` — with `kanspec scan --explain <id>` as its fix. `in_main_dwell_secs` keeps its meaning for the opposite shape, landed but never closed out.
- [c4] verbs: `scan --confirm` stays the human override for a close-out git can no longer see (a gc'd branch, a hand-merged fork), and `done --no-code` is unchanged; neither touches the branch flow.
- [c5] store: a close-out regenerates the projections, so two PRs each closing a ticket both rewrite KANSPEC-FEATURES.md. Register `kanspec` as the git merge driver for the two generated files: it regenerates from the merged store instead of asking a human to resolve a table, and the driver is installed by `init` beside the hooks.
- [c6] docs: `instructions done`, `instructions start` and DESIGN.md's state machine say it plainly — `done` means closed out on the branch, in the PR; landed is detected by git. The post-merge `kanspec: sync` commit leaves the sanctioned flow, and the default `sync = "batch"` reminder stops suggesting one after a merge.

## Testing and verification
- lifecycle: `start` → `ship --pr` → `done` on the branch records the close-out and the card sits in Review with the chip → a real squash merge on main → `scan` moves it to Done by the trailer rung, and `git status` on main is clean: no tracker commit after the merge.
- The tripwire: a `done` ticket eight days old with no detection is a WATCHING line naming `scan --explain`; the same ticket detected is not.
- doctor: a `state: done` whose `## Log` carries no `closed out` line fails the log-trail check, exactly as a hand-edited `done` does today.
- cache_wipe: every Done card degrades to `closed out · awaiting merge`, never to Review or to a wrong column, and comes back on the next `scan`.
- The merge driver: two branches that each close a ticket merge without a conflict in either generated file, and the merged file equals a fresh regeneration.

## Prescriptions
- [p1] (promote: decision) `state: done` means closed out, recorded on the branch that did the work; landed is derived from git and is never stored in a ticket. This supersedes DESIGN.md's reading of Done as "closed out and verified landed".
- [p2] (temp until t-f8a0) Until c1 ships, run `ship --pr` on the branch before opening the PR, so at least the ship record rides in it and only the done record trails the merge.

## Tickets
- [t1] (spec: ladder) The derived Done column, the awaiting-merge chip and the cache-wipe behaviour · M · implements: c2
- [t2] (spec: store) The merge driver for the two generated projections, installed by init · M · implements: c5
- [t3] (spec: verbs) The docs and DESIGN.md state machine · S · implements: c6 · deps: t-f8a0

c1, c3 and c4 are t-f8a0, already claimed; t1 and t3 depend on it landing.
