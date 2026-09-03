---
feature: 'Proposals and the review loop: propose, review, comments, approve, close, promote and expire'
code: [src/cmd/proposal.rs, src/cmd/comment.rs, src/cmd/decision.rs, docs/review.md, docs/close.md]
---
# review

## Rules
- [review.one-page] A proposal is one page: Why, Changes `[cN]`, Prescriptions `[pN]` (typed), Tickets `[tN]`. Item anchors are visible text and are the comment and disposition keys. {pre-kanspec}
- [review.approve-gate] `approve` refuses over unresolved threads, stamps who and when, and mints `[tN]` into tickets; a `[tN]` names its spec with `(spec: x)`, inherits it from the `[cN]` it implements, or takes the proposal's first spec. {pre-kanspec}
- [review.close-gate] `close` refuses until every `[cN]` and `[pN]` is dispositioned (shipped, expired, promoted, followup) and stamps the ledger; a closed proposal binds nothing and is never read for rules. {pre-kanspec}
- [review.human-accepts] A promoted or captured decision lands proposed; only a human accepts, revokes or supersedes it. {pre-kanspec}
- [review.via-agent] A comment row carries `via: agent` when an agent process wrote it; the label is the git identity, so an agent relaying a human's words is still marked. {pre-kanspec}
- [review.owed] A proposal in review is owed a human decision from the moment `review` runs: a YOU line in `status` and a row in the Review-queue tab, until `approve` or a comment. {pre-kanspec}
