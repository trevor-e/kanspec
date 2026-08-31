# review — proposals, threads, and answering feedback

A proposal is **one page, four sections**: Why (2–4 sentences), Changes (`[cN]` bullets),
Prescriptions (`[pN]`, typed), Tickets (`[tN]`, minted at approve). There is no design.md,
no tasks.md, no delta-spec files and no SHALL grammar. A one-line tweak is a five-line
proposal.

```
kanspec propose "Login rate limiting" --spec auth   # scaffolds proposals/p-7de2-.../proposal.md
kanspec review p-7de2                               # draft -> review, prints the review URL
```

The bracket ids are **visible text on purpose**. They are the comment anchors, the
disposition keys at close, and the merge keys when two people edit the same page — an
invisible HTML comment an LLM has to remember to preserve is not any of those things.

## Prescriptions are typed

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
