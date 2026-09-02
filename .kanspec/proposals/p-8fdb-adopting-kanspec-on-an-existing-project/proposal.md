---
id: p-8fdb
title: 'Adopting kanspec on an existing project: import wide, verify by sampling, then go incremental'
status: draft
specs: []
approved: null
ledger: []
created: 2026-09-02
---
## Why
Every second user arrives with an existing project, and the adulting migration showed what a naive conversion does: the first pass kept about half the binding decisions, and reaching ~95% survival took three passes and two verification methods. Incremental-only adoption is no better — path-scoped injection over three specs is confident silence everywhere else. The answer has to be a documented path with tooling, not advice.

## Changes
- [c1] `kanspec import` drafts one spec per capability from whatever the project has. Sources: an OpenSpec tree, ADRs, a CLAUDE.md rules section, README conventions; one rule per binding decision, stamped `{pre-kanspec}`; `code:` globs guessed from paths named in the source and flagged when dead.
- [c2] `kanspec backtest` samples the source and reports survival before the corpus is trusted. A seeded random sample of source assertions, each classified survived / lost / non-binding by an agent or a human against the drafted specs; the report is the gate, and `doctor` warns while an import has no backtest on record.
- [c3] `rules --adopt` stays the stamping half. A human vouches for imported text; nothing else changes.
- [c4] The docs say the one path plainly. Greenfield: `init` then `propose` on day one. Existing project: `import` → `backtest` → fix → `adopt` → every change through a proposal from then on.

## Testing and verification
- The adulting corpus is the fixture: `import` over `openspec/specs` must reproduce at least the rule count of the hand migration, and `backtest` must reproduce the 95% figure within sampling error.
- A fresh repo with only a CLAUDE.md rules section imports to one spec with those rules and a dead-glob warning per unmatched path.

## Prescriptions
- [p1] (promote: decision) An imported corpus is untrusted until a recorded backtest says otherwise. `doctor` says so; `prime` does not withhold rules, because silence is worse than a lossy rule, but the board shows the gap.

## Tickets
- [t1] `import` for an OpenSpec tree, on the adulting fixture.
- [t2] `backtest` with the sampled-survival report and the doctor warning.
- [t3] The adoption doc, both paths.
