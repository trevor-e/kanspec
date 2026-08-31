---
id: t-9c41
title: Rate-limit login endpoint
state: doing              # todo | doing | review | done | dropped
spec: auth
proposal: p-7de2
item: t1
deps: [t-31aa, t-8812]
followup_of: null
discovered_in: null

# set by kanspec start
branch: ks/t-9c41-rate-limit-login
worktree: ../kanspec-wt/t-9c41
claimed_by: claude/sess-a91
estimate: "S"
pr: null
head: null
severity_hint_v2: high    # a key a NEWER kanspec wrote; never drop it
created: 2026-08-30T14:02:11Z
---
Implement [auth.lockout]. Read q-83d0 before touching session middleware.

## Steps
- [x] lockout counter in Redis, sliding window
- [ ] 429 + Retry-After on lock
- [ ] test: two concurrent requests, same key

## Log
- 2026-08-30T14:02Z  todo     trevor                new
- 2026-08-30T14:20Z  doing    claude/sess-a91       start (branch + worktree created)
