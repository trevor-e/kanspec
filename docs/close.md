# close — expire or promote, and why closed proposals bind nothing

The failure this gate exists to prevent has a name: **stale consent**. An old approved
proposal, half-implemented, quietly steering work a year later because nobody ever said out
loud which of its promises still hold. kanspec makes that impossible in three moves.

## Rule 1 — closed proposals bind nothing

`kanspec rules` and `kanspec prime` draw *only* from standing records: specs, accepted
decisions, active quirks — plus live work items. `kanspec close` sets `status: closed` and
moves the directory into `proposals/closed/`. Closed bodies are never loaded, so there is
physically no value through which closed prose can reach a future session.

If you are an agent: never read `proposals/closed/`. Nothing in there binds anything.

## Rule 2 — expire or promote, mechanically gated

`close` enumerates every `[cN]` and `[pN]` item and refuses until each has a disposition,
printing the exact command for each unmet one:

```
$ kanspec close p-7de2
✗ cannot close p-7de2 — 2 items undispositioned:
  [c3] "ops: alert when lockouts > 100/hr" — not found in any spec
       → kanspec close p-7de2 --followup c3   |   --dropped c3 --note "..."
  [p1] (promote: decision) "rate-limit state in Redis only"
       → kanspec promote p-7de2#p1 --as decision --scope "src/auth/**"
  [p2] (temp until t-31aa) — t-31aa landed ✓ → will auto-expire at close
```

The five dispositions are `shipped → spec#rule`, `promoted → D-x | q-x`,
`followup → t-x`, `dropped (note)`, and `expired (reason)`. Most of them fill themselves
in: a Change whose rule now carries the `{p-7de2}` token is auto-recognised as shipped, and
a `(temp until t-x)` prescription becomes auto-expirable the moment merge detection sees
its guard ticket land. A typical close is one or two keys.

The ledger is stamped into the closed proposal's frontmatter:

```
✓ p-7de2 closed · ledger: c1 shipped→auth#lockout · c2 shipped→auth#lockout-event ·
  c3 followup→t-9d02 · p1 promoted→D-8c1a (awaiting accept) · p2 expired (t-31aa landed)
```

Leftover work became visible board tickets. Standing rules became auditable records.
Everything else demonstrably died — and "demonstrably" is the point: the ledger converts
*silently lost* into *explicitly waved through*.

## Rule 3 — every standing record points home

Decisions carry `source: p-x#pN`, spec rules carry `{p-x}` tokens, quirks carry
`source: t-x`. Links run from the standing record back to its origin, never the other way,
so auditing never requires reading an old proposal and revoking is a one-file diff.

```
kanspec rules            # everything steering agents right now, with provenance
kanspec rules --audit    # what looks abandoned: closed sources, zero references, no token
kanspec why auth#lockout # the whole chain: rule -> proposal item -> tickets -> PR
```

`kanspec rules` output is **byte-identical** to the standing-rules section of
`kanspec prime`. The audit surface *is* the injection surface; there is no second channel
by which an old sign-off can steer an agent.

## Promotion never self-accepts

`kanspec promote p-7de2#p1 --as decision` mints a **proposed** decision with its `source:`
pre-filled. Only a human runs `kanspec accept D-8c1a`, and an agent session literally
cannot — the accept path takes a human-only actor. A proposed decision is not injected as a
standing rule; it sits in the YOU section of `kanspec status` until accepted or dropped:
visible, never silently binding.

Abandoning a proposal outright is `kanspec abandon p-7de2 --why "..."` — no gate, because
nothing shipped and nothing needs disposing.
