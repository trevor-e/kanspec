---
feature: 'Ticket verbs: new, start, ship, done, park, drop, status and the close-out gate'
code: [src/cmd/ticket.rs, src/cmd/flow.rs, src/cmd/done.rs, src/triage.rs, src/transitions.rs, src/cmd/status.rs, src/derive.rs, src/error.rs]
---
# verbs

## Rules
- [verbs.claim-first] `start` is the claim and the lock: the branch is cut from the configured main with `--no-track`, the claim is logged, and the verb refuses when main cannot see the store (`main_blind`, `main_behind`). {pre-kanspec}
- [verbs.done-gate] `done` re-runs the ladder at close time, triages every unchecked step (spawn, drop with a reason, or actually done, no fourth option) and takes one spec waiver per uncovered spec, never a blanket. {pre-kanspec}
- [verbs.no-confirm] Agents never run `scan --confirm`; the human override is signed into the `## Log` and is the only merge attestation a person makes. {pre-kanspec}
- [verbs.fix-named] Every refusal is a `KsError` naming its one fix; gate codes are the closed `GateCode` enum, serialised snake_case, which hooks and agents branch on and which never change once shipped. {pre-kanspec}
- [verbs.status-owner] `status` lists what only a human can discharge under YOU (in main not closed, proposals in review, proposed decisions), agent work under AGENT and tripwires under WATCHING; every line names its fix. {pre-kanspec}
- [verbs.discovered] Out-of-scope work is `kanspec new` with `discovered_in` stamped to the claimed ticket; it never blocks that ticket's `done`. {pre-kanspec}
