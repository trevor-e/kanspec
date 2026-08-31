//! `tests/scan_ladder.rs`
//!
//! Proves: all six merge shapes -> exact `MergeStatus` AND `Method`, including the two that
//! MUST be `unknown`.
//!
//! Every repo here is a REAL git repo with a REAL bare origin and six REAL merges (
//! `tests/common/merges.rs`). Nothing about git is mocked; the one seam is
//! `$KANSPEC_GH_FIXTURES`, because a test cannot create a GitHub PR — and rung 2 is the
//! only rung that can see a title-only squash.
//!
//! **A ladder that answers all six confidently is a ladder that guesses.** Two of these
//! shapes are genuinely undecidable from local git alone, and the assertions below pin
//! *which two* and *why*, so an "improvement" that turns an `unknown` into a confident
//! answer has to argue with a named test.
//!
//! Owner: **S3**.

mod common;

use common::merges::Shape;
use common::{ctx_at, TestRepo};

use kanspec::cache::{GitState, MergeFact, MergeStatus};
use kanspec::error::GateDetail;
use kanspec::git::Method;
use kanspec::ids::TicketId;
use kanspec::scan;

// ─────────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────────

fn gitstate(repo: &TestRepo) -> GitState {
    let text = repo.read(".kanspec/cache/gitstate.json");
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("cache/gitstate.json is not a GitState ({e}):\n{text}"))
}

#[track_caller]
fn fact(s: &GitState, id: &str) -> MergeFact {
    let key = TicketId::parse(id).expect("a fixture id");
    s.tickets
        .get(&key)
        .unwrap_or_else(|| panic!("no merge fact for {id}; cache holds {:?}", s.tickets.keys()))
        .clone()
}

/// What the ladder must conclude for each shape from LOCAL GIT ALONE, rung by rung.
///
/// `Shape::expected()` (foundation-owned) predicts `NotMerged` for `Never`. The ladder
/// cannot honestly say that: `git cherry origin/main <head>` reports `+2`, and a `+` line
/// cannot distinguish an unmerged branch from a multi-commit squash (D-3) — so the answer
/// is `unknown`, never "no". That is why ARCHITECTURE.md §10's round-2 gate reads "all six
/// shapes with exact `Method`, **two** landing on `unknown`" while the helper predicts one.
fn expected(shape: Shape) -> (MergeStatus, Method, &'static str) {
    match shape {
        // The branch tip is literally reachable from main. Rung 1 is exact, and cheap.
        Shape::TrueMerge => (MergeStatus::Merged, Method::Ancestry, ""),
        // git's own squash message quotes every original message, indented four spaces —
        // which is exactly why rung 3's grep is unanchored.
        Shape::SquashGitNative => (MergeStatus::Merged, Method::Trailer, ""),
        // The trailer is destroyed and the patch-ids do not match. All four rungs decline.
        Shape::SquashGhTitleOnly => (
            MergeStatus::Unknown,
            Method::None,
            "unknown (squash suspected, no gh)",
        ),
        // Cherry-picking copies the message verbatim, so the trailer is on main and rung 3
        // answers BEFORE rung 4 gets to see `git cherry`'s all-`-` output. Rung 4 is not
        // dead: `SquashOneCommit` below is decided by it and nothing else.
        Shape::Rebase => (MergeStatus::Merged, Method::Trailer, ""),
        // One commit in, one commit out: the patch-id survives the squash.
        Shape::SquashOneCommit => (MergeStatus::Merged, Method::PatchId, ""),
        // Not merged, and not provably so — the honest answer, and the reason
        // `scan --confirm` exists.
        Shape::Never => (
            MergeStatus::Unknown,
            Method::None,
            "unknown (squash suspected, no gh)",
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// the ladder, against the six real shapes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_six_real_merge_shapes_land_on_exact_methods_with_two_honest_unknowns() {
    let repo = TestRepo::with_merges();
    repo.ks(["scan"]).ok();
    let state = gitstate(&repo);

    assert_eq!(state.main, "origin/main");
    assert!(state.scanned_at.is_some(), "a scan stamps its own clock");

    let mut unknowns = 0;
    for shape in Shape::all() {
        let (status, method, why) = expected(shape);
        let f = fact(&state, shape.ticket());
        assert_eq!(
            f.status,
            status,
            "{shape:?} ({}): expected {status:?}, got {:?} ({:?})",
            shape.ticket(),
            f.status,
            f.why
        );
        assert_eq!(
            f.method,
            method,
            "{shape:?} ({}) resolved by the wrong rung",
            shape.ticket()
        );
        if status == MergeStatus::Unknown {
            unknowns += 1;
            assert_eq!(f.why.as_deref(), Some(why), "{shape:?} must explain itself");
            assert!(f.sha.is_none(), "an unknown never names a SHA");
        } else {
            assert!(f.sha.is_some(), "{shape:?} landed, so it names what landed");
        }
    }
    assert_eq!(
        unknowns, 2,
        "exactly two of the six are undecidable without gh (ARCHITECTURE.md §10, round 2); \
         a ladder that answers all six confidently is a ladder that guesses"
    );

    // Five of the six agree with the foundation's own prediction table; `Never` is the
    // documented divergence above.
    for shape in Shape::all() {
        if shape == Shape::Never {
            continue;
        }
        let predicted = match shape.expected() {
            common::merges::ExpectedStatus::Merged => MergeStatus::Merged,
            common::merges::ExpectedStatus::NotMerged => MergeStatus::NotMerged,
            common::merges::ExpectedStatus::Unknown => MergeStatus::Unknown,
        };
        assert_eq!(fact(&state, shape.ticket()).status, predicted, "{shape:?}");
    }
}

/// The headline promise: a ladder that cannot prove a merge says so, with the reason, and
/// **never** with a confident negative. Only rungs that can prove absence may say no — and
/// none of the four can.
#[test]
fn an_unmerged_branch_is_unknown_with_a_reason_never_a_confident_no() {
    let repo = TestRepo::with_merges();
    repo.ks(["scan"]).ok();
    let f = fact(&gitstate(&repo), Shape::Never.ticket());
    assert_eq!(f.status, MergeStatus::Unknown);
    assert_ne!(
        f.status,
        MergeStatus::NotMerged,
        "`git cherry` reporting `+` cannot tell an unmerged branch from a multi-commit \
         squash, so it must not be read as proof of absence (D-3)"
    );
    assert_eq!(f.why.as_deref(), Some("unknown (squash suspected, no gh)"));
}

/// Every ticket carries the paths its branch changed, so spec staleness is recomputable at
/// READ time instead of accumulated in a disposable cache (D-10).
#[test]
fn every_scanned_ticket_records_the_paths_its_branch_changed() {
    let repo = TestRepo::with_merges();
    repo.ks(["scan"]).ok();
    let state = gitstate(&repo);
    for shape in Shape::all() {
        let f = fact(&state, shape.ticket());
        let want = format!("src/auth/{}-0.ts", shape.ticket());
        assert!(
            f.changed.contains(&want),
            // The TrueMerge shape is the interesting one: three-dot `main...head` is EMPTY
            // once the branch's commits are on main, so this only passes because the
            // trailer fallback asks each commit directly.
            "{shape:?} recorded {:?}, which is missing {want}",
            f.changed
        );
    }
}

/// A branch that has never carried a commit is trivially an ancestor of main. Rung 1 would
/// call that MERGED — a verified-looking answer for work that never happened — so guard 0b
/// takes it first. It must NOT take the true merge with it (see `ladder`'s doc comment).
#[test]
fn a_fresh_branch_with_no_commits_is_unknown_not_merged() {
    let repo = TestRepo::with_merges();
    repo.git(&["branch", "ks/t-aaaa-fresh", "main"]);
    repo.write(
        ".kanspec/tickets/t-aaaa.md",
        "---\n\
         id: t-aaaa\n\
         title: freshly started, nothing committed\n\
         state: doing\n\
         branch: ks/t-aaaa-fresh\n\
         head: null\n\
         created: 2026-08-30T09:00:00Z\n\
         ---\n\
         \n\
         ## Log\n\
         - 2026-08-30T09:00Z  todo     trevor                new\n\
         - 2026-08-30T10:00Z  doing    trevor                start\n",
    );
    repo.ks(["scan"]).ok();

    let state = gitstate(&repo);
    let f = fact(&state, "t-aaaa");
    assert_eq!(f.status, MergeStatus::Unknown, "{:?}", f.why);
    assert_eq!(
        f.why.as_deref(),
        Some("unknown (branch has no commits of its own)")
    );
    // …and the true merge, whose head is ALSO zero commits ahead of main, is untouched.
    assert_eq!(
        fact(&state, Shape::TrueMerge.ticket()).method,
        Method::Ancestry
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// --explain
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn explain_shows_the_rungs_of_the_run_that_produced_the_verdict() {
    let repo = TestRepo::with_merges();
    let out = repo
        .ks(["scan", Shape::SquashGhTitleOnly.ticket(), "--explain"])
        .ok()
        .stdout;
    for expect in [
        "ancestry",
        "merge-base --is-ancestor",
        "not an ancestor",
        "inconclusive",
        "trailer",
        "0 hits",
        "patch-id",
        "git cherry",
        "+2",
        "unknown (squash suspected, no gh)",
    ] {
        assert!(
            out.contains(expect),
            "`scan --explain` omitted {expect:?}:\n{out}"
        );
    }
    // The order is the order the rungs ran in — that is what makes the trace readable as
    // reasoning rather than as a list of facts.
    let pos = |s: &str| out.find(s).unwrap_or_else(|| panic!("missing {s}"));
    assert!(pos("ancestry") < pos("trailer") && pos("trailer") < pos("patch-id"));

    // Without --explain the same verdict prints without the reasoning.
    let quiet = repo
        .ks(["scan", Shape::SquashGhTitleOnly.ticket()])
        .ok()
        .stdout;
    assert!(quiet.contains("unknown (squash suspected, no gh)"));
    assert!(!quiet.contains("merge-base --is-ancestor"), "{quiet}");
}

#[test]
fn quiet_prints_nothing_at_all_for_the_git_hooks() {
    let repo = TestRepo::with_merges();
    let run = repo.ks(["scan", "--quiet"]).ok();
    assert!(
        run.stdout.is_empty(),
        "post-merge hook noise: {}",
        run.stdout
    );
    // …and it still wrote the cache.
    assert_eq!(
        fact(&gitstate(&repo), Shape::TrueMerge.ticket()).status,
        MergeStatus::Merged
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// rung 2 — the only rung that sees a title-only squash
// ─────────────────────────────────────────────────────────────────────────────

/// `gh` JSON in the shape `gh pr list --head <branch> --json …` really prints (recorded
/// corpus: `tests/fixtures/gh/pr-list-head-squash-merged.json`).
fn pr_list(number: u64, merge_commit: &str, head: &str) -> String {
    format!(
        r#"[{{"headRefName":"x","headRefOid":"{head}","mergeCommit":{{"oid":"{merge_commit}"}},
             "mergedAt":"2026-08-30T16:31:55Z","number":{number},"state":"MERGED",
             "url":"https://github.com/kanspec/fixture/pull/{number}"}}]"#
    )
}

/// The shape that defeats local git entirely, answered — and re-verified against local git
/// before it is believed.
#[test]
fn gh_turns_the_title_only_squash_from_unknown_into_a_verified_merge() {
    let repo = TestRepo::with_merges();
    repo.ks(["scan"]).ok();
    assert_eq!(
        fact(&gitstate(&repo), Shape::SquashGhTitleOnly.ticket()).status,
        MergeStatus::Unknown,
        "without gh this shape is undecidable"
    );

    // The commit GitHub would report as the squash merge — a real commit, on real main.
    let squash = repo
        .git(&[
            "log",
            "origin/main",
            "--format=%H",
            "-1",
            "--grep",
            "squashed work",
        ])
        .trim()
        .to_string();
    assert!(!squash.is_empty(), "the fixture's squash commit must exist");
    let head = repo.sha(Shape::SquashGhTitleOnly.branch());
    repo.gh_fixture(
        &kanspec::gh::fixture_name_head(Shape::SquashGhTitleOnly.branch()),
        &pr_list(145, &squash, &head),
    );

    repo.ks(["scan", Shape::SquashGhTitleOnly.ticket()]).ok();
    let state = gitstate(&repo);
    let f = fact(&state, Shape::SquashGhTitleOnly.ticket());
    assert_eq!(f.status, MergeStatus::Merged);
    assert_eq!(f.method, Method::GhPr);
    assert_eq!(f.pr, Some(145));
    assert_eq!(
        f.sha.as_deref(),
        Some(squash.as_str()),
        "the recorded SHA is the merge commit git re-verified, not gh's word"
    );
    // A TARGETED scan must not discard the facts it did not recompute.
    assert_eq!(
        fact(&state, Shape::TrueMerge.ticket()).method,
        Method::Ancestry
    );
}

/// gh is authoritative about GitHub, not about this checkout. A `MERGED` PR whose merge
/// commit is not on our main is a stale fetch or a different base branch — and that is an
/// `unknown` naming the SHA, never a merge.
#[test]
fn a_gh_merge_that_is_not_on_our_main_is_unknown_not_merged() {
    let repo = TestRepo::with_merges();
    // A commit that exists locally and is provably NOT on main: the never-merged branch.
    let off_main = repo.sha(Shape::Never.branch());
    repo.gh_fixture(
        &kanspec::gh::fixture_name_head(Shape::SquashGhTitleOnly.branch()),
        &pr_list(146, &off_main, &off_main),
    );
    repo.ks(["scan"]).ok();

    let f = fact(&gitstate(&repo), Shape::SquashGhTitleOnly.ticket());
    assert_eq!(f.status, MergeStatus::Unknown);
    assert!(
        f.why
            .as_deref()
            .is_some_and(|w| w.contains("is not on main")),
        "the badge must say what gh claimed and why we did not believe it: {:?}",
        f.why
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// the gate, and the recorded human override
// ─────────────────────────────────────────────────────────────────────────────

/// The gate re-runs the ladder rather than trusting the cache, and refuses with the trace
/// that justified the refusal.
#[test]
fn the_done_gate_refuses_an_undetectable_merge_and_hands_back_the_ladder() {
    let repo = TestRepo::with_merges();
    let ctx = ctx_at(&repo.root);
    let snap = ctx.snapshot().expect("a snapshot");

    let t = snap
        .ticket(&TicketId::parse(Shape::SquashGhTitleOnly.ticket()).unwrap())
        .unwrap();
    let err = scan::proof_for_done(&ctx, t).expect_err("an undecidable merge cannot close");
    assert_eq!(err.kind(), "gate");
    assert_eq!(err.code(), Some("not_landed"));
    match err.detail() {
        Some(GateDetail::NotLanded { trace }) => {
            assert!(
                trace.len() >= 3,
                "the refusal carries the rungs that produced it: {trace:?}"
            );
            assert!(trace.iter().any(|r| r.method == Method::Ancestry));
            assert!(trace.iter().any(|r| r.method == Method::PatchId));
        }
        other => panic!("the gate must carry its trace, got {other:?}"),
    }
    // Invariant 9: the refusal names the two commands that resolve it.
    let fixes: Vec<String> = err.fixes().iter().map(|f| f.to_string()).collect();
    assert!(fixes.iter().any(|f| f.contains("--explain")), "{fixes:?}");
    assert!(fixes.iter().any(|f| f.contains("--confirm")), "{fixes:?}");

    // …and a shape git CAN prove mints a real proof, with the rung that proved it.
    let t = snap
        .ticket(&TicketId::parse(Shape::TrueMerge.ticket()).unwrap())
        .unwrap();
    let proof = scan::proof_for_done(&ctx, t).expect("ancestry proves this one");
    assert_eq!(proof.method(), Method::Ancestry);
    assert_eq!(proof.ticket().as_str(), Shape::TrueMerge.ticket());
    assert!(
        proof.badge().starts_with("IN MAIN (ancestry"),
        "{}",
        proof.badge()
    );
}

/// D-11: the attestation goes in the TICKET'S `## Log`, not the cache — so it is signed,
/// diffable, and survives `rm -rf .kanspec/cache`.
#[test]
fn a_confirmation_is_recorded_on_the_ticket_and_outlives_a_cache_wipe() {
    let repo = TestRepo::with_merges();
    let id = Shape::SquashGhTitleOnly.ticket();
    repo.ks(["scan"]).ok();
    assert_eq!(fact(&gitstate(&repo), id).status, MergeStatus::Unknown);

    repo.ks([
        "scan",
        "--confirm",
        id,
        "--why",
        "github squash, verified by hand",
    ])
    .ok();

    // 1. it is on the ticket, attributed, with the SHA it vouches for.
    let ticket = repo.read(&format!(".kanspec/tickets/{id}.md"));
    let line = ticket
        .lines()
        .find(|l| l.contains("confirm"))
        .unwrap_or_else(|| panic!("no confirm line in:\n{ticket}"));
    assert!(line.contains("trevor"), "{line}");
    assert!(line.contains("in main"), "{line}");
    assert!(line.contains("github squash, verified by hand"), "{line}");
    // The ticket's state is unchanged: `confirm` records, it does not transition.
    assert!(ticket.contains("state: review"));

    // 2. the cache is disposable, and the attestation is not.
    std::fs::remove_dir_all(repo.root.join(".kanspec/cache")).unwrap();
    repo.ks(["scan"]).ok();
    let f = fact(&gitstate(&repo), id);
    assert_eq!(f.status, MergeStatus::Merged);
    assert_eq!(f.method, Method::HumanConfirm);
    assert!(f.why.as_deref().is_some_and(|w| w.contains("confirmed")));

    // 3. and the gate accepts it — the ONLY non-ladder route to a proof.
    let ctx = ctx_at(&repo.root);
    let snap = ctx.snapshot().unwrap();
    let t = snap.ticket(&TicketId::parse(id).unwrap()).unwrap();
    let proof = scan::proof_for_done(&ctx, t).expect("the recorded override unblocks `done`");
    assert_eq!(proof.method(), Method::HumanConfirm);
    assert_eq!(
        proof.sha().as_str(),
        repo.sha(Shape::SquashGhTitleOnly.branch())
    );
}

/// A human override can only speak where git is silent. It never overrules a fresh answer,
/// because the ladder runs first and the confirmation is consulted only when it declines.
#[test]
fn a_confirmation_never_overrules_what_git_can_still_prove() {
    let repo = TestRepo::with_merges();
    let id = Shape::TrueMerge.ticket();
    repo.ks(["scan", "--confirm", id, "--why", "belt and braces"])
        .ok();
    repo.ks(["scan"]).ok();
    assert_eq!(
        fact(&gitstate(&repo), id).method,
        Method::Ancestry,
        "git proved it; the attestation must not relabel the method"
    );
}

#[test]
fn a_confirmation_without_a_reason_is_refused() {
    let repo = TestRepo::with_merges();
    // clap enforces the pairing before the handler ever runs.
    let run = repo.ks(["scan", "--confirm", Shape::Never.ticket()]);
    assert_ne!(run.code, 0);
    assert!(
        format!("{}{}", run.stdout, run.stderr).contains("--why"),
        "{run:?} must name the missing flag",
        run = run.stderr
    );
    // And an unknown ticket is a not_found, not a silently recorded attestation.
    let run = repo.ks(["scan", "--confirm", "t-0000", "--why", "x"]);
    assert_ne!(run.code, 0);
    assert!(run.stderr.contains("t-0000"), "{}", run.stderr);
}

// ─────────────────────────────────────────────────────────────────────────────
// the cache is a cache
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_gitstate_cache_is_gitignored_and_rebuilt_from_nothing() {
    let repo = TestRepo::with_merges();
    repo.ks(["scan"]).ok();
    let before = gitstate(&repo);

    let (code, out, _) =
        repo.git_try(&["check-ignore", "-q", "--", ".kanspec/cache/gitstate.json"]);
    assert_eq!(code, 0, "the cache must be gitignored: {out}");

    std::fs::remove_dir_all(repo.root.join(".kanspec/cache")).unwrap();
    repo.ks(["scan"]).ok();
    let after = gitstate(&repo);
    for shape in Shape::all() {
        let (a, b) = (fact(&before, shape.ticket()), fact(&after, shape.ticket()));
        assert_eq!(
            (a.status, a.method, a.sha),
            (b.status, b.method, b.sha),
            "{shape:?}"
        );
    }
}

/// The transcript is the product: a badge that does not say how it knows, or a line that
/// does not name the next command, is the failure this whole file exists to prevent.
#[test]
fn every_line_of_the_transcript_says_how_it_knows_and_what_to_do_next() {
    let repo = TestRepo::with_merges();
    let out = repo.ks(["scan"]).ok().stdout;
    let sha = &repo.sha(Shape::TrueMerge.branch())[..7];

    let landed = out
        .lines()
        .find(|l| l.contains(Shape::TrueMerge.ticket()))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        landed.starts_with(" ⇂ "),
        "the in-main overlay glyph: {landed}"
    );
    assert!(
        landed.contains(&format!("in main (ancestry · {sha})")),
        "a badge names its method AND what landed: {landed}"
    );
    assert!(
        landed.contains(&format!("→ kanspec done {}", Shape::TrueMerge.ticket())),
        "invariant 9: {landed}"
    );

    let unsure = out
        .lines()
        .find(|l| l.contains(Shape::Never.ticket()))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        unsure.contains("unknown (squash suspected, no gh)"),
        "{unsure}"
    );

    assert!(
        out.contains("6 scanned · 4 in main · 2 unknown · 0 not in main"),
        "the summary counts every shape:\n{out}"
    );
    // `--json` is the same values, not a second rendering of them.
    let json: serde_json::Value =
        serde_json::from_str(&repo.ks(["scan", "--json"]).ok().stdout).expect("--json prints JSON");
    assert_eq!(json["scanned"], 6);
    assert_eq!(json["landed"].as_array().unwrap().len(), 4);
    assert_eq!(json["unknown"].as_array().unwrap().len(), 2);
    let row = json["landed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == Shape::TrueMerge.ticket())
        .expect("the true merge is in the landed group");
    assert_eq!(row["method"], "ancestry");
    assert_eq!(row["sha"], sha);
    assert_eq!(
        row["fix"],
        format!("kanspec done {}", Shape::TrueMerge.ticket())
    );
    assert!(
        json["unknown"][0]["trace"].as_array().unwrap().is_empty(),
        "the rung table costs four subprocess lines per ticket — only --explain pays for it"
    );
}

#[test]
fn scanning_an_unknown_ticket_is_a_not_found_before_any_fetch() {
    let repo = TestRepo::with_merges();
    let run = repo.ks(["scan", "t-0000"]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(
        run.stderr.contains("ticket t-0000 not found"),
        "{}",
        run.stderr
    );
}
