//! The six real merge shapes from recon — each a REAL merge into a REAL origin.
//!
//! Two of them MUST land on `unknown`: a multi-commit squash with a GitHub title-only
//! merge message and no `gh` defeats all four rungs (R-4). `tests/scan_ladder.rs` asserts
//! the exact `MergeStatus` **and** `Method` for every shape, including those two — a
//! ladder that confidently answers all six is a ladder that guesses.
//!
//! Owner: **F**. FROZEN.

#![allow(dead_code)]

use super::TestRepo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// a real merge commit — rung 1 answers exactly
    TrueMerge,
    /// `git merge --squash` — rung 3's trailer survives it
    SquashGitNative,
    /// GitHub's title-only squash — **only rung 2 can see this one**
    SquashGhTitleOnly,
    /// rebased onto main — rung 4 (`cherry`, all `-`)
    Rebase,
    /// a one-commit squash — rung 4 still sees it
    SquashOneCommit,
    /// never merged at all — and STILL `unknown`, not `not merged`, because a `+` cherry
    /// line cannot tell an unmerged branch from a multi-commit squash (D-3)
    Never,
}

impl Shape {
    /// The ticket the shape owns. Fixed rather than minted, so a failure message names a
    /// shape rather than a hash.
    pub const fn ticket(self) -> &'static str {
        match self {
            Shape::TrueMerge => "t-9c41",
            Shape::SquashGitNative => "t-88fe",
            Shape::SquashGhTitleOnly => "t-dddd",
            Shape::Rebase => "t-bbbb",
            Shape::SquashOneCommit => "t-eeee",
            Shape::Never => "t-cccc",
        }
    }
    pub const fn branch(self) -> &'static str {
        match self {
            Shape::TrueMerge => "ks/t-9c41-true-merge",
            Shape::SquashGitNative => "ks/t-88fe-squash-native",
            Shape::SquashGhTitleOnly => "ks/t-dddd-squash-title-only",
            Shape::Rebase => "ks/t-bbbb-rebase",
            Shape::SquashOneCommit => "ks/t-eeee-squash-one",
            Shape::Never => "ks/t-cccc-never",
        }
    }
    /// How many commits the branch carries. The single-commit squash is a DIFFERENT shape
    /// from the multi-commit one precisely because `git cherry` can only see the former.
    pub const fn commits(self) -> usize {
        match self {
            Shape::SquashOneCommit => 1,
            _ => 2,
        }
    }
    /// How many of the branch's own commits are NOT on `origin/main` after the merge.
    ///
    /// Only a TRUE merge puts the branch's actual commit objects onto main, so only it
    /// reads 0 here. **This is a live hazard for the ladder** (ARCHITECTURE.md §2.15
    /// guard 0b): `rev-list --count <main>..<head> == 0` is specified as
    /// `Unknown::ZeroCommitBranch`, but it is ALSO exactly what a true merge looks like —
    /// so, run before rung 1, that guard would answer `unknown` for the one shape ancestry
    /// can prove. The guard has to run AFTER ancestry, or be conditioned on ancestry being
    /// negative.
    pub const fn commits_ahead_of_main(self) -> usize {
        match self {
            Shape::TrueMerge => 0,
            other => other.commits(),
        }
    }

    /// What a correct ladder must conclude WITHOUT `gh`.
    ///
    /// **Two shapes land on `unknown`**, which is ARCHITECTURE.md §10's round-2 gate and
    /// invariant 2 at its sharpest. `SquashGhTitleOnly` is the one recon named: all four
    /// rungs decline (R-4). `Never` joins it — corrected in round B, where this helper said
    /// `NotMerged` and the ladder observably disagreed — because rung 4's only non-merged
    /// output is a `+` cherry line, and a `+` cannot distinguish a branch that never merged
    /// from a multi-commit squash that did (D-3). Saying `not merged` there would be a
    /// confident wrong answer for the squash, which is the one thing this ladder may never
    /// produce. `Verdict::NotLanded` is consequently unreachable in v0.1 — see §11 D-25.
    pub const fn expected(self) -> ExpectedStatus {
        match self {
            Shape::TrueMerge | Shape::SquashGitNative | Shape::Rebase | Shape::SquashOneCommit => {
                ExpectedStatus::Merged
            }
            Shape::SquashGhTitleOnly | Shape::Never => ExpectedStatus::Unknown,
        }
    }
    pub const fn all() -> [Shape; 6] {
        [
            Shape::TrueMerge,
            Shape::SquashGitNative,
            Shape::SquashGhTitleOnly,
            Shape::Rebase,
            Shape::SquashOneCommit,
            Shape::Never,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedStatus {
    Merged,
    NotMerged,
    Unknown,
}

/// The expectation table on its own, for a repo built by [`TestRepo::with_merges`].
pub fn all_expectations() -> Vec<(Shape, String, ExpectedStatus)> {
    Shape::all()
        .into_iter()
        .map(|s| (s, s.ticket().to_string(), s.expected()))
        .collect()
}

/// Builds all six shapes against the real bare origin and writes one ticket per shape.
/// Returns `(shape, ticket id, expected verdict)`.
pub fn all(repo: &TestRepo) -> Vec<(Shape, String, ExpectedStatus)> {
    let mut out = Vec::new();
    for shape in Shape::all() {
        build(repo, shape);
        out.push((shape, shape.ticket().to_string(), shape.expected()));
    }
    // The tickets are part of the fixture's git history, not stray working-tree files.
    repo.git(&["add", "-A", "--", ".kanspec"]);
    repo.git(&["commit", "--quiet", "-m", "fixture tickets"]);
    repo.git(&["push", "--quiet", "origin", "main"]);
    repo.git(&["fetch", "--quiet", "origin"]);
    out
}

fn build(repo: &TestRepo, shape: Shape) {
    let id = shape.ticket();
    let branch = shape.branch();

    repo.git(&["checkout", "--quiet", "-B", branch, "main"]);
    for n in 0..shape.commits() {
        let file = format!("src/auth/{id}-{n}.ts");
        repo.write(
            &file,
            &format!("export const {id_ident}_{n} = {n};\n", id_ident = ident(id)),
        );
        // `add -A` would sweep up the ticket files earlier shapes left untracked in the
        // working tree, commit them onto THIS branch, and then have them vanish on the
        // next `checkout main`. Stage exactly the file this commit is about.
        repo.git(&["add", "--", &file]);
        // The trailer the `prepare-commit-msg` hook writes in a real repo. Rung 3 greps
        // for exactly this, UNANCHORED, because git indents it four spaces inside a
        // squash body.
        repo.git(&[
            "commit",
            "--quiet",
            "-m",
            &format!("{id}: step {n}\n\nKanspec: {id}\n"),
        ]);
    }
    let head = repo.sha("HEAD");
    repo.push(branch);
    repo.git(&["checkout", "--quiet", "main"]);

    match shape {
        Shape::TrueMerge => {
            repo.git(&[
                "merge",
                "--quiet",
                "--no-ff",
                "-m",
                &format!("Merge {branch}"),
                branch,
            ]);
        }
        Shape::SquashGitNative => {
            // git's own squash message keeps every original message, indented — which is
            // exactly why rung 3 must not anchor its pattern to the start of a line.
            repo.git(&["merge", "--quiet", "--squash", branch]);
            let msg = squash_msg(repo).unwrap_or_else(|| format!("squash {id}\n"));
            repo.git(&["commit", "--quiet", "-m", &msg]);
        }
        Shape::SquashGhTitleOnly => {
            // GitHub with "PR title only" as the squash message. The trailer is DESTROYED
            // and the patch-ids do not match, so all four rungs decline (R-4).
            repo.git(&["merge", "--quiet", "--squash", branch]);
            repo.git(&["commit", "--quiet", "-m", "squashed work (#145)"]);
        }
        Shape::Rebase => {
            // Main must move first. Cherry-picking onto the branch's own fork point
            // reproduces byte-identical commits — same tree, same parent, same message —
            // so git hands back the SAME SHAs and the "rebase" shape silently becomes a
            // fast-forward. Advancing main is what makes the new commits genuinely new.
            repo.write("src/rebase-base.txt", "main moved first\n");
            repo.git(&["add", "--", "src/rebase-base.txt"]);
            repo.git(&["commit", "--quiet", "-m", "main moves before the rebase"]);
            let range = format!("main..{branch}");
            let shas: Vec<String> = repo
                .git(&["rev-list", "--reverse", &range])
                .lines()
                .map(str::to_string)
                .collect();
            for sha in shas {
                repo.git(&["cherry-pick", "--allow-empty", &sha]);
            }
        }
        Shape::SquashOneCommit => {
            // One commit in, one commit out: the patch-id survives, so `git cherry` sees
            // `-1` and this is the ONE squash shape rung 4 can answer.
            repo.git(&["merge", "--quiet", "--squash", branch]);
            repo.git(&["commit", "--quiet", "-m", "one-commit squash (#7)"]);
        }
        Shape::Never => {}
    }
    if shape != Shape::Never {
        repo.git(&["push", "--quiet", "origin", "main"]);
    }

    write_ticket(repo, shape, &head);
}

/// `.git/SQUASH_MSG` — git's default body for `merge --squash`.
fn squash_msg(repo: &TestRepo) -> Option<String> {
    let dir = repo.git(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "SQUASH_MSG",
    ]);
    std::fs::read_to_string(dir.trim()).ok()
}

/// A ticket in `review` whose `## Log` replays legally — so the fixture is usable by the
/// `done` gate and by `doctor`, not just by the ladder.
fn write_ticket(repo: &TestRepo, shape: Shape, head: &str) {
    let id = shape.ticket();
    let body = format!(
        "---\n\
         id: {id}\n\
         title: fixture {shape:?}\n\
         state: review\n\
         spec: auth\n\
         deps: []\n\
         branch: {branch}\n\
         claimed_by: trevor\n\
         pr: null\n\
         head: {head}\n\
         created: 2026-08-30T09:00:00Z\n\
         ---\n\
         Fixture ticket for the {shape:?} merge shape.\n\
         \n\
         ## Steps\n\
         - [x] make the commits\n\
         - [ ] land them\n\
         \n\
         ## Log\n\
         - 2026-08-30T09:00Z  todo     trevor                new\n\
         - 2026-08-30T10:00Z  doing    trevor                start\n\
         - 2026-08-30T11:00Z  review   trevor                ship\n",
        branch = shape.branch(),
    );
    repo.write(&format!(".kanspec/tickets/{id}.md"), &body);
}

fn ident(id: &str) -> String {
    id.replace('-', "_")
}
