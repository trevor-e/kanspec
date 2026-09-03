---
feature: 'Standing rules: what prime injects — accepted decisions, active quirks, ranked and budgeted spec rules'
code: [src/rulesdoc.rs, src/cmd/prime.rs, src/cmd/rules.rs, src/cmd/spec.rs, src/cmd/quirk.rs, src/cmd/features.rs, src/hooks.rs, src/instructions.rs]
---
# rules

## Rules
- [rules.identity] The standing-rules section of `prime` is byte-identical to `kanspec rules` for the same scope: one generator, one renderer, no second path. {pre-kanspec}
- [rules.only-accepted] Only accepted decisions and active quirks steer agents; a proposed decision sits under YOU in `status` until a human acts. {pre-kanspec}
- [rules.ranked] Matched specs rank by glob specificity, then by ownership of the touched paths (a path shared among the specs that match it most specifically counts for its share), then raw count; the budget is spent in that order and every elided spec is named. {pre-kanspec}
- [rules.provenance] Every spec rule bullet carries a `{p-xxxx}`, `{p-xxxx#cN}` or `{pre-kanspec}` token; `rules --audit` flags the rest and `rules --adopt` stamps them.
- [rules.audit-age] The audit asks whether a decision is still wanted only after `[windows] decision_review_secs` with its source proposal closed and no open ticket or shipped rule referencing it. {pre-kanspec}
- [rules.projections] KANSPEC-FEATURES.md and KANSPEC-ARCHITECTURE.md are generated from the specs, decisions and quirks and never hand-edited; a spec's `feature:` and `code:` are what feed them. {pre-kanspec}
