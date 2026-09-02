---
id: t-2ba7
title: 'prime ranking: a broad-glob spec must not outrank a specific match on coverage'
state: todo
spec: null
proposal: null
item: null
deps: []
followup_of: null
discovered_in: null
branch: null
worktree: null
claimed_by: null
pr: null
head: null
spec_unchanged: null
created: 2026-09-02T03:50:18Z
---
prime ranking: a broad-glob spec must not outrank a specific match on coverage

## Steps

In the trial the 52-rule `tasks` spec outranked the relevant 16-rule `materializer` spec because it covered two touched files via `models.py`/`schemas.py`. `Scope::rank` counts covered paths as the tiebreak; count only globs at least as specific as the best match, or weight coverage below specificity. Unit test with the trial shape (D-59 is the design record).

## Log
- 2026-09-02T03:50Z  todo     trevor                new
