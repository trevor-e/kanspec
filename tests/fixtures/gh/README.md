# Recorded `gh` JSON — the one and only mock seam in the crate

Every file here is **real `gh` output**, captured verbatim from a public repository with a
read-only command. Nothing is hand-written and nothing is edited after capture: guessing
the JSON shape defeats the entire point of the seam, and `gh` does not print what a naive
struct would expect (`mergeCommit` is a nested `{"oid": …}` object, not a string; an
ancient merged PR really does come back with `mergeCommit: null`).

`src/gh.rs` parses these through *exactly* the same code that parses live `gh`, and its
unit tests `include_str!` them, so a fixture that stops matching the parser fails the
build's own tests.

Owner: **S2** (this subdirectory). `tests/fixtures/**` otherwise belongs to **F**.

## How the seam consumes them

`$KANSPEC_GH_FIXTURES` names a directory of `<stem>.json` files. `TestRepo::gh_fixture(stem,
json)` writes one; `gh.rs` reads it. The stems are not a convention to be remembered — they
are produced by `kanspec::gh::fixture_name_pr(n)` and `kanspec::gh::fixture_name_head(branch)`:

| call | stem |
|---|---|
| `Gh::pr_view(142)` | `pr-142` |
| `Gh::pr_for_head("ks/t-9c41-rate-limit")` | `head-ks-t-9c41-rate-limit` |

A **missing** fixture is `GhUnavailable` (inconclusive), never an empty answer — a test that
forgets to record one gets `unknown`, not a false "not merged".

## The corpus

| file | recorded with | what it pins |
|---|---|---|
| `pr-view-squash-merged.json` | `gh pr view 36723 --repo denoland/deno --json number,state,mergedAt,mergeCommit,headRefOid,url` | **The case rung 2 exists for.** A real GitHub *squash* merge: merge commit `baca93a…` has one parent and the title-only subject `fix(cli): … (#36723)`. Ancestry, trailer and patch-id all decline on this shape; only `gh` sees it. |
| `pr-view-merge-commit.json` | `gh pr view 9000 --repo cli/cli --json …` | A merge-commit ("Create a merge commit") merge — two parents, `Merge pull request #9000 from …`. Same JSON shape, different git shape. |
| `pr-view-open.json` | `gh pr view 36742 --repo denoland/deno --json …` | `state: OPEN`, `mergedAt: null`, `mergeCommit: null`. A successful answer that carries no merge. |
| `pr-view-closed-unmerged.json` | `gh pr view 36734 --repo denoland/deno --json …` | `state: CLOSED`, never merged. The answer that *looks* like proof of "not merged" and is not — its commits can still have been cherry-picked, so it stays evidence, never a verdict. |
| `pr-view-merged-no-merge-commit.json` | `gh pr view 13 --repo rails/rails --json …` | `state: MERGED` with `mergeCommit: null` — GitHub genuinely has no merge commit for PRs this old (2010). gh said "merged" and handed the ladder no SHA to re-verify, so rung 2 must fall through rather than conclude. |
| `pr-list-head-squash-merged.json` | `gh pr list --repo denoland/deno --head fix/bundle-sourcemap-optional-value --state all --json number,state,mergedAt,mergeCommit,headRefOid,url,headRefName` | The `pr_for_head` rung: a JSON **array**, with the extra `headRefName` field `pr view` does not emit (which is why the wire struct tolerates unknown fields). `--state all` matters: the default `--state open` cannot see the merge. |
| `pr-list-head-none.json` | the same, with `--head kanspec/no-such-branch-xyz` | `[]`, exit 0. "GitHub knows of no PR for this branch" — inconclusive, never a negative. |

## Re-recording

The PR numbers above are permanent, but a PR that is open today will be merged later, so
`pr-view-open.json` is the one file that can go stale in meaning. Re-record with the exact
command in the table; keep `--json` field order alone (it is `gh`'s, alphabetical) and do
not reformat.
