---
id: t-e3a7
title: 'One store load per command: use Committed.snapshot, build RulesDoc once'
state: doing
spec: null
proposal: null
item: null
deps: []
followup_of: null
discovered_in: null
branch: ks/t-e3a7-one-store-load-per-command-use-committed
worktree: null
claimed_by: noreply
pr: null
head: null
spec_unchanged: null
created: 2026-09-03T02:15:33Z
---
One store load per command: use Committed.snapshot, build RulesDoc once

## Steps

Store::transact returns the post-write Snapshot in Committed, yet several verbs re-ran ctx.snapshot() afterwards, and prime built the rules document and resolved the claimed ticket twice (a second git subprocess). The review pass fixed the instances it found; add the rule to ARCHITECTURE §2.16 and audit the remaining handlers (repair, features, up/server reads) so a command parses the store exactly once before and once after its transaction.

## Log
- 2026-09-03T02:15Z  todo     noreply               new
- 2026-09-03T03:36Z  doing    noreply               start (branch created)
