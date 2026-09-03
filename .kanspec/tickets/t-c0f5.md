---
id: t-c0f5
title: Validate GitState on load or stop calling the cache hand-editable
state: doing
spec: null
proposal: null
item: null
deps: []
followup_of: null
discovered_in: null
branch: ks/t-c0f5-validate-gitstate-on-load-or-stop-callin
worktree: null
claimed_by: noreply
pr: null
head: null
spec_unchanged: null
created: 2026-09-03T02:15:33Z
---
Validate GitState on load or stop calling the cache hand-editable

## Steps

cache.rs says a text editor can write .kanspec/cache, and cache::load promises a disposable cache can never fail a command, but consumers trust its shape: doctor panicked on `dead[0]` for an empty quirk glob row until the review pass guarded it. Either validate GitState after deserialising (drop rows that violate invariants, log why) or document the file as tool-owned and drop the hand-edit promise.

## Log
- 2026-09-03T02:15Z  todo     noreply               new
- 2026-09-03T03:27Z  doing    noreply               start (branch created)
