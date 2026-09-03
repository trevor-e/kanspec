---
feature: 'The single write path: lock, fresh snapshot, pure planner, atomic files, projections republished'
code: [src/store.rs, src/fm.rs, src/lock.rs, src/plan.rs, src/keys.rs, src/project.rs, src/model.rs, src/logentry.rs, src/doctor.rs]
---
# store

## Rules
- [store.one-write-path] Every mutation is a `Plan` through `Store::transact`; nothing else writes under `.kanspec/` (`init` and the lock file excepted). {pre-kanspec}
- [store.pure-planner] Planners are `fn(&Snapshot, &Facts, &Args, &Minter)`: every subprocess, network call and clock read happens before `transact` and rides in `Facts`. {pre-kanspec}
- [store.one-load] A handler loads the store once before its transaction and uses `Committed.snapshot` after it; a second `ctx.snapshot()` after `transact` is a bug. {pre-kanspec}
- [store.projections] `transact` republishes KANSPEC-FEATURES.md and KANSPEC-ARCHITECTURE.md itself whenever a plan touches a spec, decision, quirk, rule stamp or the git facts; no handler calls a regenerate. {pre-kanspec}
- [store.keys] Every frontmatter key is a `keys.rs` variant; a derived fact such as merge state has no variant and so cannot be written into a file. {pre-kanspec}
- [store.log-proves] Every ticket state is reached by a logged transition; the write path proves the `## Log` before and after staging, so a hand-edited `state:` fails at the next verb. {pre-kanspec}
- [store.pure-derive] `derive.rs` and every doctor check are functions of the `Snapshot` alone: shifting its clock moves every tripwire, and nothing else moves with the wall clock. {pre-kanspec}
