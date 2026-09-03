---
feature: 'Merge detection: the four-rung ladder that answers ''did this land?'' and never guesses'
code: [src/scan.rs, src/git.rs, src/gh.rs, src/cache.rs, src/cmd/scan.rs, src/cmd/repair.rs]
---
# ladder

## Rules
- [ladder.never-no] No rung proves absence: a `+` cherry line cannot tell an unmerged branch from a multi-commit squash, so the ladder has two outcomes, Landed and Unknown with a reason. There is no NotLanded, and nothing adds one. {pre-kanspec}
- [ladder.zero-commit] A branch tip with zero commits main lacks is Unknown (ZeroCommitBranch) unless a `Kanspec: <id>` trailer on main shows every commit already landed; then the ladder falls through to ancestry. {pre-kanspec}
- [ladder.proof-grade] `MergedProof` is minted only by `scan::ladder` from a live run or a recorded human confirmation. The cache is badge-grade and has no inverse into a proof. {pre-kanspec}
- [ladder.cache-disposable] `cache::load` is total and validating: a missing, stale-versioned or invariant-breaking file yields defaults or dropped rows named in `GitState::dropped`, never a failed command. {pre-kanspec}
- [ladder.badge] `derive::Badge` is the one merge-state vocabulary; `Badge::text(now)` renders every card, scan row and proof, always with the caller's clock, never `Utc::now()`. {pre-kanspec}
- [ladder.head] `head:` is written only by `ship` and `done` from real git output; `plan_start` never writes it, or every fresh branch reads as merged by ancestry. {pre-kanspec}
