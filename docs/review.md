# review — proposals, threads, and answering feedback

A proposal is **one page, five sections**: Why (2–4 sentences), Changes (`[cN]` bullets),
Testing and verification (how anyone will know it works — the tests each ticket carries, a
hand check, a metric if one is warranted), Prescriptions (`[pN]`, typed — the rules this
proposal leaves behind), Tickets (`[tN]`, minted at approve). There is no design.md, no
tasks.md, no delta-spec files and no SHALL grammar. A one-line tweak is a five-line
proposal. Any other `## ` section you add (a repo's `Security impact`, say) is shown on the
review page in file order; nothing an author writes is dropped.

**Write the first sentence of every bullet as its headline.** The review page shows only
that sentence and folds the rest behind a tap, so a reviewer reads the whole plan in a
dozen lines and opens only what they doubt. "Record the move-in date on the household
profile." then the column, the wire field, the validation.

```
kanspec propose "Login rate limiting" --spec auth   # scaffolds proposals/p-7de2-.../proposal.md
kanspec review p-7de2                               # draft -> review, prints the review URL
kanspec review p-7de2 --export page.html            # ...and a static, comment-less copy of the page
```

The bracket ids are **visible text on purpose**. They are the comment anchors, the
disposition keys at close, and the merge keys when two people edit the same page — an
invisible HTML comment an LLM has to remember to preserve is not any of those things.

## Cutting the tickets

**One ticket is one PR is one session:** one capability, a handful of steps, sized S or M.
An L is a ticket to cut again. The scaffold already cuts along the capability — one
`[tN] (spec: x)` placeholder per `--spec` — because the capability is the unit whose rules
`prime` injects and whose `code:` globs the diff maps to, so a one-capability ticket pays
for one spec's rules and no other's.

```
- [t1] (spec: auth) Rate-limit login endpoint · S · implements: c1
  - lockout counter in Redis, sliding window
  - 429 + Retry-After on lock
  - test: two concurrent requests, same key
```

The indented sub-bullets are the ticket's steps: `approve` mints them as its unchecked
`## Steps`, the list `done` later triages. The `· S ·` estimate is read for the review
line and never stored. A bare `[tN] title` is still a valid item.

**kanspec annotates the cut; you write nothing extra.** `review` and `approve` print one
line under every `[tN]`, and the page shows the same line on the card:

```
[t1] auth · reads 12 rules (~0.9k tokens) · 1 decision · surface 9 files, 1.8k lines under src/auth/lockout.rs · 3 steps
```

That is what the ticket will have to **read** before writing a line — the standing rules
in scope for its capability, from the same generator `prime` spends, and the code under
the spec's globs, narrowed to the paths its `[cN]` bullets name in backticks. It is derived
from the store and the tree at read time and never written into the proposal. It is **not**
an estimate of the work: kanspec never guesses how many tokens a ticket will take, and a
spec that names no `code:` gets `surface unknown` rather than a number.

A cut worth looking at again is **flagged, with the cut named**, and nothing blocks:

```
[t1] auth · reads 6 rules (~455 tokens) · surface 8 files, 8.9k lines · size L
     ⚠ t1 implements c1, c2 (auth) and c3 (billing) → one ticket per capability: auth (c1, c2), billing (c3)
     ⚠ surface 8.9k lines, over surface_lines_max 4000 → one ticket per subtree: src/auth/lockout/**, src/auth/session/**
     ⚠ sized L → cut again — an L is more than one session
⚠ c4: no ticket implements it → add a [tN] · implements: c4, or say in the bullet why no ticket is needed
```

The thresholds are `[review] rules_tokens_max`, `surface_lines_max` and `changes_max` in
`config.toml`; a change spanning capabilities, an `L`, and a `[cN]` no ticket implements
are flagged regardless. The human approving over a flag is the recorded waiver, exactly as
resolving a thread themselves is — the flag is advice with evidence, not a gate. To make a
surface flag go quiet, name the paths the change touches in backticks: the surface narrows
to them, and so does the flag.

## Prescriptions are the rules this proposal leaves behind

A closed proposal binds nothing (Rule 1). So anything that should keep steering agents
after close has to be named here and typed — the page labels the section "Rules this
leaves behind" and says so.

```
- [p1] (promote: decision) Rate-limit state lives in Redis only — never Postgres.
- [p2] (temp until t-31aa) Keep the legacy captcha path until the lockout table ships.
```

`(temp until t-x)` dies automatically the moment merge detection sees its guard ticket land.
`(promote: decision|spec|quirk)` must become a standing record before the proposal can
close. An untyped prescription is a `doctor` warning and a close blocker — not because
ceremony is good, but because an untyped "we should probably…" is exactly the prose that
silently expires and then silently steers someone a year later.

## The review round trip

1. The human clicks `[c3]` on the review page and types a comment. The browser POSTs to the
   local server, which appends one line to `comments.jsonl` through the same write library
   the CLI uses. Truth is the repo; the page is a pure view; there is nothing to sync.
2. The unresolved count rides in `kanspec prime` and in `kanspec status`, so feedback is
   work you can see rather than scrollback you have to remember.
3. **Your side of it, as an agent:**

   ```
   kanspec comments p-7de2 --unresolved --json
   ```

   Each thread is a self-locating `{target, quote, body}` triple — you need no page
   context. The `quote` is the item's text *at comment time*, so if the item has been
   edited since you can see the drift; if the item was deleted, its thread lands in a
   visible orphan tray instead of disappearing.
4. Address the feedback by editing `proposal.md` (keep the bracket ids — dropping one
   orphans its thread and `doctor` will say so), then:

   ```
   kanspec comment reply cm-88f1 --body "Agreed — moved to p-8a10; c3 removed."
   kanspec comment resolve cm-88f1 --note "c3 -> p-8a10"
   ```

   `resolve` requires a note. A resolution with no note is not a resolution.
5. `kanspec approve p-7de2` ends it. It **refuses while unresolved threads exist** (a human
   resolving a thread themselves is the recorded waiver), stamps who and when, and mints
   the `[tN]` items into board tickets. Approval is an explicit recorded act, never
   implicit.

## When specs change

As part of implementation, never as an archive step. The ticket that implements `[c1]`
edits `specs/auth.md` on its own branch — adding or changing the rule bullet with its
`{p-7de2}` provenance token — and that edit is reviewed in the PR like any other diff. No
deferred delta debt, no bot commits to trunk, and the spec can never say something main
does not do for longer than one review.

> Reviewing on the page requires `kanspec up`. The CLI half works cold: `kanspec comments
> --unresolved --json` is the whole agent-side surface.
