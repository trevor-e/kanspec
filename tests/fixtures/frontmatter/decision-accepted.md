---
id: D-8c1a
title: Rate-limit state lives in Redis only
status: accepted          # proposed | accepted | superseded | revoked
date: 2026-09-02
source: p-7de2#p1
scope: [src/auth/**]
supersedes: null
superseded_by: null
---
## Decision
Sliding window in Redis; keys `rl:login:<user>`, TTL 10m.
