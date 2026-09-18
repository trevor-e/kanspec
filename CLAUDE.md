# kanspec

The binding docs are `DESIGN.md` (the product) and `ARCHITECTURE.md` (the code contract; §10 says where code lives now that the parallel build is over). `docs/*.md` are the embedded `kanspec instructions` topics. Tests: `cargo test` (27 suites), `cargo clippy --all-targets`, `cargo fmt`. A rebuild reaches the hooks only after `cargo install --path .`.

<!-- kanspec:begin -->
## kanspec
This repo tracks work, specs, and standing rules with kanspec. `kanspec prime` is auto-injected
at session start; run it yourself if context feels missing.
- Find work: `kanspec ready --json`. Claim before coding: `kanspec start <id>` (creates branch/worktree).
- Diff ready: `kanspec ship <id> --pr <n>`. Finish ON THE BRANCH, before the PR merges: `kanspec done <id>`
  records the close-out (answer its flags) and rides in the PR; landed is detected by git afterwards.
- Never state whether something is merged: git detects it; report `kanspec show <id>` output. Name every id to a
  human by its LABEL (`t-9c41-rate-limit-login`, never bare `t-9c41`); `kanspec show <id>` prints one for t-/p-/D-/q-.
- Unsure what you owe, or whether you are stuck: `kanspec status` — every line names its own fix.
- Review feedback is work, not scrollback: `kanspec comments --unresolved --json` gives
  {target, quote, body}; answer with `kanspec comment reply <cm-id> --body "..."` and close it
  with `kanspec comment resolve <cm-id> --note "what changed"`.
- Standing rules are `kanspec rules` output ONLY. Closed proposals bind nothing — never read
  .kanspec/proposals/closed/. Never edit an accepted decision; propose one with `kanspec decide`.
- Out-of-scope work you uncover (>5 min): `kanspec new "..."` — one command; it auto-links
  discovered_in to your claimed ticket. Park it and keep going; do NOT expand your current ticket.
  Gotcha learned the hard way: `kanspec quirk add "..." --paths <glob>`.
- Change state ONLY via kanspec verbs — never hand-edit frontmatter. Workflow details: `kanspec instructions`.
<!-- kanspec:end -->
