//! `tests/spec_staleness.rs`
//!
//! Proves: the staleness tripwire fires on merges **kanspec never tracked**.
//!
//! Owner: **S4**.
//!
//! DESIGN.md's anti-rot gear 1 exists to catch a spec drifting away from the code it
//! describes. The drift that matters most is the drift nobody logged: a teammate's PR, a
//! hotfix pushed straight to main, a dependabot bump, everything that predates adoption.
//! None of those has a kanspec ticket, so none of them appears in `GitState.tickets` — and
//! a tripwire that counts only kanspec's own merged tickets is silent for exactly the repos
//! that most need it. `scan` already asks git the right question and records the answer in
//! `SpecAnchor::merges_since`; this file is the end-to-end proof that the answer reaches
//! the surfaces a human reads.
//!
//! Every commit below is dated explicitly. The harness clock is frozen at
//! `common::NOW`, while real `git commit` stamps wall-clock time — so a fixture that let
//! git pick the dates would decide "is the attestation newer than the spec's last edit?"
//! by whatever second the suite happened to run in.

mod common;

use std::path::Path;
use std::process::Command;

use common::TestRepo;

/// The spec's last edit. Every merge below lands after it, and the frozen `NOW` is after
/// all of them.
const SPEC_EDIT: &str = "2026-08-01T00:00:00Z";

/// `git -C <root> …` at a FIXED date, asserting success.
#[track_caller]
fn git_dated(root: &Path, when: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "teammate")
        .env("GIT_AUTHOR_EMAIL", "teammate@kanspec.invalid")
        .env("GIT_COMMITTER_NAME", "teammate")
        .env("GIT_COMMITTER_EMAIL", "teammate@kanspec.invalid")
        .env("GIT_AUTHOR_DATE", when)
        .env("GIT_COMMITTER_DATE", when)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git must be runnable");
    assert!(
        out.status.success(),
        "git {} failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A spec over `src/auth/**`, committed and pushed — so it HAS a last-edit anchor.
fn commit_spec(repo: &TestRepo) {
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login (JWT 24h)\ncode: [src/auth/**]\n---\n# auth\n\n\
         - [a1] Sessions expire after 24h.\n",
    );
    git_dated(&repo.root, SPEC_EDIT, &["add", "-A"]);
    git_dated(
        &repo.root,
        SPEC_EDIT,
        &["commit", "--quiet", "-m", "spec: auth"],
    );
}

/// `n` merges into main that touch the spec's globs and that kanspec has never heard of —
/// a teammate's PR, a hotfix, a dependabot bump. No ticket file, no `Kanspec:` trailer, no
/// row in `GitState.tickets`.
fn merges_nobody_tracked(repo: &TestRepo, n: usize) {
    for i in 1..=n {
        let when = format!("2026-08-{:02}T00:00:00Z", 9 + i);
        let branch = format!("teammate/change-{i}");
        git_dated(&repo.root, &when, &["checkout", "--quiet", "-b", &branch]);
        common::write_at(
            &repo.root,
            "src/auth/login.ts",
            &format!("export const login = () => {{}}; // rev {i}\n"),
        );
        git_dated(&repo.root, &when, &["add", "-A"]);
        git_dated(
            &repo.root,
            &when,
            &["commit", "--quiet", "-m", &format!("harden login ({i})")],
        );
        git_dated(&repo.root, &when, &["checkout", "--quiet", "main"]);
        git_dated(
            &repo.root,
            &when,
            &[
                "merge",
                "--quiet",
                "--no-ff",
                "-m",
                &format!("Merge pull request #{i} from teammate"),
                &branch,
            ],
        );
    }
    repo.push("main");
}

/// What `scan` recorded for one spec — the premise every assertion below rests on.
fn recorded_merges_since(repo: &TestRepo, spec: &str) -> u64 {
    let cache: serde_json::Value = serde_json::from_str(&repo.read(".kanspec/cache/gitstate.json"))
        .expect("the cache is JSON");
    cache["specs"][spec]["merges_since"]
        .as_u64()
        .unwrap_or_else(|| panic!("no anchor recorded for {spec}: {cache}"))
}

/// The `staleness` verdict `features --json` prints for one spec — the same value the
/// terminal table, the board's dot and the `status` line are all rendered from.
fn verdict(repo: &TestRepo, spec: &str) -> serde_json::Value {
    let j: serde_json::Value = repo.json(&["features"]);
    let rows = j["rows"].as_array().expect("features --json has rows");
    rows.iter()
        .find(|r| r["spec"] == spec)
        .unwrap_or_else(|| panic!("no row for {spec}: {j}"))["staleness"]
        .clone()
}

/// `Staleness::Stale` over `n` merges counted from the spec's own last edit, with no
/// kanspec ticket to name.
fn stale_untracked(n: u64) -> serde_json::Value {
    serde_json::json!({
        "staleness": "stale",
        "merges": n,
        "since": SPEC_EDIT,
        "examples": [],
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// the headline: merges kanspec never tracked still trip the wire
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn merges_with_no_kanspec_ticket_behind_them_still_go_stale() {
    let repo = TestRepo::new();
    commit_spec(&repo);
    merges_nobody_tracked(&repo, 4);
    repo.ks(["scan"]).ok();

    // The premise: `scan` asked git and git answered 4. Nothing below is meaningful if
    // this fails — the bug would be S3's, not the projection's.
    assert_eq!(
        recorded_merges_since(&repo, "auth"),
        4,
        "scan must record the merges touching the spec's globs since its last edit"
    );
    // ...and kanspec has no tickets at all, so the old ticket-only count is 0.
    let board: serde_json::Value = repo.json(&["ls", "--all"]);
    assert!(
        board["rows"].as_array().is_none_or(|r| r.is_empty()),
        "the fixture must have NO kanspec tickets: {board}"
    );

    let stale = repo.ks(["features", "--stale"]).ok().stdout;
    assert!(
        stale.contains("auth"),
        "4 untracked merges against a threshold of 3 must trip the wire:\n{stale}"
    );
    assert!(
        stale.contains("STALE: 4 merges since"),
        "and it must name the count git gave:\n{stale}"
    );
    assert!(
        stale.contains("kanspec features --confirm auth"),
        "every stale row names its one-command fix (invariant 9):\n{stale}"
    );

    // the same fact on the status board, as a WATCHING line
    let status = repo.ks(["status"]).ok().stdout;
    assert!(
        status.contains("4 merges touched src/auth/** since spec last edited"),
        "the tripwire must reach `status`:\n{status}"
    );
    assert!(status.contains("kanspec features --stale"), "{status}");
}

#[test]
fn confirming_no_behaviour_change_resets_a_tripwire_nothing_ever_counted() {
    let repo = TestRepo::new();
    commit_spec(&repo);
    merges_nobody_tracked(&repo, 4);
    repo.ks(["scan"]).ok();
    assert_eq!(verdict(&repo, "auth"), stale_untracked(4));

    repo.ks([
        "features",
        "--confirm",
        "auth",
        "--why",
        "renamed a helper, no behaviour change",
    ])
    .ok();

    // The attestation is NEWER than the spec's last edit, so it is the anchor — and the
    // recorded count, which is measured from the older one, no longer applies. Nobody
    // decremented anything: the cache still says 4.
    assert_eq!(recorded_merges_since(&repo, "auth"), 4);
    assert_eq!(
        verdict(&repo, "auth"),
        serde_json::json!({"staleness": "ok"})
    );
    let stale = repo.ks(["features", "--stale"]).ok().stdout;
    assert!(stale.contains("no specs are stale"), "{stale}");

    // ...and it survives the cache going away and coming back, because it lives in the
    // spec's own frontmatter (D-10).
    assert!(repo.read(".kanspec/specs/auth.md").contains("stale_ack: {"));
}

#[test]
fn a_confirmation_older_than_the_last_spec_edit_does_not_hold() {
    let repo = TestRepo::new();
    commit_spec(&repo);
    merges_nobody_tracked(&repo, 4);
    // An attestation from BEFORE the spec was last edited says nothing about the merges
    // counted from that edit. Written by hand: `features --confirm` can only ever stamp
    // `now`.
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login (JWT 24h)\ncode: [src/auth/**]\n\
         stale_ack: {sha: deadbee, at: 2026-07-01T00:00:00Z, by: trevor, why: stale ack}\n\
         ---\n# auth\n",
    );
    repo.ks(["scan"]).ok();
    assert_eq!(
        verdict(&repo, "auth"),
        stale_untracked(4),
        "the LATER of the two anchors wins, and here that is the spec's own last edit"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// "I have never looked" is never a green tick
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_spec_with_no_last_edit_anchor_reads_never_scanned_not_ok() {
    let repo = TestRepo::new();
    // Written but never committed: `git log origin/main -- <spec>` finds nothing, so the
    // anchor has no `last_edit_at` and there is no point to count merges from. The globs
    // match real tracked files, so this is not glob rot either.
    repo.write(
        ".kanspec/specs/billing.md",
        "---\nfeature: Billing\ncode: [src/billing/**]\n---\n# billing\n",
    );
    repo.ks(["scan"]).ok();

    assert_eq!(
        verdict(&repo, "billing"),
        serde_json::json!({"staleness": "never_scanned"}),
        "an anchor with nothing to count from must say so — never a confident `ok`"
    );
    let stale = repo.ks(["features", "--stale"]).ok().stdout;
    assert!(
        stale.contains("billing") && stale.contains("never scanned"),
        "and it is listed as unresolved, with `kanspec scan` as its fix:\n{stale}"
    );
    assert!(stale.contains("kanspec scan"), "{stale}");
}
