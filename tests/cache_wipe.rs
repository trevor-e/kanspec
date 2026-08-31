//! `tests/cache_wipe.rs`
//!
//! Proves: rm -rf .kanspec/cache changes nothing but freshness stamps
//!
//! Owner: **S4**.
//!
//! This is the **behavioural** proof of invariant 1, and the reason it is worth more than
//! the grep and the seals put together: it holds regardless of HOW a derived fact got
//! where it is. If any command had ever written "merged" into a file, or if any tripwire
//! kept a running count, the wipe below would leave that residue behind and the assertions
//! would catch it — which no type and no source scan can do.
//!
//! Three properties, in order of how much they buy:
//!
//! 1. **Nothing git-tracked moves.** Every byte under `.kanspec/` except `cache/` is
//!    identical before and after, so no derived fact was ever stored in an entity file.
//! 2. **A wipe subtracts knowledge, never inverts it.** Every answer that came from git
//!    degrades to *never scanned* — never to a confident opposite. A counter in a
//!    disposable cache would instead read a confident **zero**, which under-fires the
//!    staleness tripwire: the dangerous direction, and exactly what D-10 forbids.
//! 3. **The cache carries no state beyond its own bytes.** Putting it back reproduces the
//!    original output byte for byte, which is what makes re-deriving it (one `scan`) a
//!    complete recovery.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use common::TestRepo;
use kanspec::derive::{self, Badge, Staleness};
use kanspec::ids::{SpecName, TicketId};
use kanspec::model::Snapshot;

/// The frozen clock every fixture below is dated against — `common::NOW`.
fn now() -> DateTime<Utc> {
    common::NOW.parse().expect("the harness clock is RFC3339")
}

fn ticket(repo: &TestRepo, id: &str, state: &str, fields: &str, log: &str) {
    repo.write(
        &format!(".kanspec/tickets/{id}.md"),
        &format!(
            "---\nid: {id}\ntitle: {id} work\nstate: {state}\n{fields}\
             created: 2026-08-29T10:00:00Z\n---\nBody.\n\n## Log\n{log}"
        ),
    );
}

/// A board with one of everything the projection can say.
fn fixture(repo: &TestRepo) {
    // landed but not closed -> a YOU line, and the dep that unblocks t-66d1
    ticket(
        repo,
        "t-31aa",
        "review",
        "spec: auth\npr: 142\n",
        "- 2026-08-31T09:00Z  todo     trevor                new\n\
         - 2026-08-31T09:30Z  doing    trevor                start\n\
         - 2026-08-31T10:00Z  review   trevor                ship\n",
    );
    // claimable only BECAUSE t-31aa landed (D-17)
    ticket(
        repo,
        "t-66d1",
        "todo",
        "spec: auth\ndeps: [t-31aa]\n",
        "- 2026-08-31T09:00Z  todo     trevor                new\n",
    );
    // stalled: doing, last touched 3h before the frozen NOW
    ticket(
        repo,
        "t-88fe",
        "doing",
        "spec: auth\n",
        "- 2026-08-31T08:00Z  todo     trevor                new\n\
         - 2026-08-31T09:00Z  doing    claude/sess-a91       start\n",
    );
    // three merged tickets whose paths land inside the payments spec's globs
    for id in ["t-0aa1", "t-0aa2", "t-0aa3"] {
        ticket(
            repo,
            id,
            "done",
            "spec: payments\n",
            &format!(
                "- 2026-08-30T09:00Z  todo     trevor                new\n\
                 - 2026-08-30T09:30Z  doing    trevor                start\n\
                 - 2026-08-30T10:00Z  review   trevor                ship\n\
                 - 2026-08-30T11:00Z  done     trevor                done ({id})\n"
            ),
        );
    }
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login (JWT 24h)\ncode: [src/auth/**]\n---\n# auth\n",
    );
    repo.write(
        ".kanspec/specs/payments.md",
        "---\nfeature: Payments\ncode: [src/payments/**]\n---\n# payments\n",
    );
    write_cache(repo);
}

/// Exactly the bytes a `scan` would leave behind — hand-written here because `scan` is
/// S3's, and because a hand-written cache is the harsher test: nothing in the projection
/// may depend on having produced it.
const CACHE: &str = r#"{
  "version": 1,
  "scanned_at": "2026-08-31T11:56:00Z",
  "main": "origin/main",
  "fetch_age_secs": 10,
  "tickets": {
    "t-31aa": {"status":"merged","sha":"a1b9c3d","method":"gh_pr","pr":142,"why":null,
               "checked_at":"2026-08-31T11:56:00Z","changed":["src/auth/lockout.ts"]},
    "t-0aa1": {"status":"merged","sha":"b2c3d4e","method":"ancestry","pr":null,"why":null,
               "checked_at":"2026-08-31T11:56:00Z","changed":["src/payments/charge.ts"]},
    "t-0aa2": {"status":"merged","sha":"c3d4e5f","method":"ancestry","pr":null,"why":null,
               "checked_at":"2026-08-31T11:56:00Z","changed":["src/payments/refund.ts"]},
    "t-0aa3": {"status":"merged","sha":"d4e5f6a","method":"ancestry","pr":null,"why":null,
               "checked_at":"2026-08-31T11:56:00Z","changed":["src/payments/fee.ts"]}
  },
  "branches": {
    "t-88fe": {"branch":"ks/t-88fe","head":"e5f6a7b","ahead":2,"behind":0,
               "last_commit_at":"2026-08-31T09:00:00Z","pushed":true}
  },
  "specs": {
    "auth":     {"last_edit_sha":"aaa1111","last_edit_at":"2026-08-31T00:00:00Z",
                 "merges_since":0,"dead_globs":[]},
    "payments": {"last_edit_sha":"bbb2222","last_edit_at":"2026-07-01T00:00:00Z",
                 "merges_since":0,"dead_globs":[]}
  }
}
"#;

fn write_cache(repo: &TestRepo) {
    std::fs::create_dir_all(repo.root.join(".kanspec/cache")).unwrap();
    repo.write(".kanspec/cache/gitstate.json", CACHE);
}

fn wipe_cache(repo: &TestRepo) {
    std::fs::remove_dir_all(repo.root.join(".kanspec/cache")).expect("the cache is disposable");
    assert!(!repo.exists(".kanspec/cache"));
}

/// Every byte under `.kanspec/` EXCEPT the disposable cache.
fn tracked_bytes(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            if rel.starts_with("cache") {
                continue;
            }
            if p.is_dir() {
                walk(&p, base, out);
            } else if let Ok(b) = std::fs::read(&p) {
                out.insert(rel, b);
            }
        }
    }
    let ks = root.join(".kanspec");
    let mut out = BTreeMap::new();
    walk(&ks, &ks, &mut out);
    assert!(!out.is_empty(), "the fixture wrote nothing");
    out
}

/// The snapshot the CLI would load, with the harness clock pinned onto it so the
/// assertions do not drift with the wall clock.
fn snapshot(repo: &TestRepo) -> Snapshot {
    let ctx = common::ctx_at(&repo.root);
    let mut s = kanspec::store::load_snapshot(&ctx).expect("the store loads the fixture");
    s.now = now();
    s
}

fn tid(s: &str) -> TicketId {
    TicketId::parse(s).unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. nothing git-tracked moves
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reading_the_board_never_writes_a_derived_fact_into_a_file() {
    let repo = TestRepo::new();
    fixture(&repo);
    let before = tracked_bytes(&repo.root);

    // Every read surface that exists today, run against a cache full of merge facts.
    repo.ks(["status"]);
    repo.ks(["status", "--json"]);
    repo.ks(["doctor"]);
    repo.ks(["status", "--owner", "you"]);

    assert_eq!(
        tracked_bytes(&repo.root),
        before,
        "a read must not write, and `merged` must never reach an entity file"
    );
    // The `merged: true` a badge is computed FROM lives in exactly one place.
    for (name, bytes) in &before {
        let text = String::from_utf8_lossy(bytes);
        for forbidden in ["merged:", "in_main:", "checked_at:", "stalled:", "ready:"] {
            assert!(
                !text.contains(forbidden),
                "{name} carries the derived key `{forbidden}`"
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. a wipe subtracts knowledge, never inverts it
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_wipe_leaves_every_git_tracked_byte_untouched() {
    let repo = TestRepo::new();
    fixture(&repo);
    let before = tracked_bytes(&repo.root);
    wipe_cache(&repo);
    repo.ks(["status"]);
    repo.ks(["doctor"]);
    assert_eq!(tracked_bytes(&repo.root), before);
}

#[test]
fn a_wiped_cache_degrades_every_git_answer_to_never_scanned() {
    let repo = TestRepo::new();
    fixture(&repo);

    let s = snapshot(&repo);
    let landed = s.tickets[&tid("t-31aa")].clone();
    assert!(matches!(derive::badge(&s, &landed), Badge::InMain { .. }));
    assert!(derive::in_main(&s, &landed).is_some());
    assert!(matches!(
        derive::staleness(&s, &s.specs[&SpecName::parse("payments").unwrap()]),
        Staleness::Stale { merges: 3, .. }
    ));

    wipe_cache(&repo);
    let s = snapshot(&repo);
    let landed = s.tickets[&tid("t-31aa")].clone();

    // The dangerous direction, closed: NOT `Unpushed`, NOT `NotMerged`, NOT "0 merges".
    assert!(
        matches!(derive::badge(&s, &landed), Badge::NeverScanned),
        "a wiped cache must say it does not know — never that the answer is no"
    );
    assert!(derive::in_main(&s, &landed).is_none());
    assert!(
        matches!(
            derive::staleness(&s, &s.specs[&SpecName::parse("payments").unwrap()]),
            Staleness::NeverScanned
        ),
        "an accumulated counter would read a confident ZERO here and under-fire (D-10)"
    );
    assert!(s.git.is_empty() && s.git.age(s.now).is_none());
}

#[test]
fn every_answer_that_does_not_come_from_git_survives_the_wipe_unchanged() {
    let repo = TestRepo::new();
    fixture(&repo);

    let before = snapshot(&repo);
    let stalled_before: Vec<String> = before
        .tickets
        .values()
        .filter(|t| derive::stalled(&before, t).is_some())
        .map(|t| t.fm.id.to_string())
        .collect();
    let doctor_before = serde_json::to_string(&kanspec::doctor::run_all(&before)).unwrap();
    let claims_before = derive::double_claims(&before);
    let cycles_before = derive::dep_cycles(&before);

    wipe_cache(&repo);
    let after = snapshot(&repo);

    assert_eq!(
        stalled_before,
        after
            .tickets
            .values()
            .filter(|t| derive::stalled(&after, t).is_some())
            .map(|t| t.fm.id.to_string())
            .collect::<Vec<_>>(),
        "STALLED is anchored on the ticket's own `## Log`, which is git-TRACKED"
    );
    assert_eq!(
        doctor_before,
        serde_json::to_string(&kanspec::doctor::run_all(&after)).unwrap(),
        "the invariant proofs read entity files, never the cache"
    );
    assert_eq!(claims_before, derive::double_claims(&after));
    assert_eq!(cycles_before, derive::dep_cycles(&after));
}

/// The same property for the half of staleness that has NO ticket behind it.
///
/// `SpecAnchor::merges_since` is what `scan` got back from
/// `rev-list --count --first-parent <last edit>..<main> -- <globs>`: every merge that
/// touched the spec's code, including the teammate PRs, hotfixes and dependabot bumps
/// kanspec never tracked. It is a RESULT recomputed whole on every scan, so a wipe must
/// erase it to *never scanned* — the moment it degraded to a confident **zero** it would
/// be an accumulator, and the tripwire would go quiet on exactly the repos that need it.
#[test]
fn a_recorded_merge_count_with_no_ticket_behind_it_degrades_to_never_scanned() {
    let repo = TestRepo::new();
    fixture(&repo);
    // Four merges against `src/auth/**` that no kanspec ticket accounts for: `tickets`
    // holds exactly one auth fact (t-31aa), which is 1 < the window of 3.
    let untracked = CACHE.replace(
        r#""auth":     {"last_edit_sha":"aaa1111","last_edit_at":"2026-08-31T00:00:00Z",
                 "merges_since":0"#,
        r#""auth":     {"last_edit_sha":"aaa1111","last_edit_at":"2026-08-31T00:00:00Z",
                 "merges_since":4"#,
    );
    assert!(
        untracked.contains(r#""merges_since":4"#),
        "the edit applied"
    );
    repo.write(".kanspec/cache/gitstate.json", &untracked);

    let auth = SpecName::parse("auth").unwrap();
    let s = snapshot(&repo);
    assert!(
        matches!(
            derive::staleness(&s, &s.specs[&auth]),
            Staleness::Stale { merges: 4, .. }
        ),
        "work kanspec never tracked still drifts the spec: {:?}",
        derive::staleness(&s, &s.specs[&auth])
    );

    wipe_cache(&repo);
    let s = snapshot(&repo);
    assert!(
        matches!(
            derive::staleness(&s, &s.specs[&auth]),
            Staleness::NeverScanned
        ),
        "a wipe must SUBTRACT the count, never resolve it to a clean zero (D-10)"
    );

    // Putting the same bytes back reproduces the same verdict: nothing was accumulated.
    std::fs::create_dir_all(repo.root.join(".kanspec/cache")).unwrap();
    repo.write(".kanspec/cache/gitstate.json", &untracked);
    let s = snapshot(&repo);
    assert!(matches!(
        derive::staleness(&s, &s.specs[&auth]),
        Staleness::Stale { merges: 4, .. }
    ));
}

#[test]
fn a_human_attestation_outlives_the_cache_because_it_lives_in_the_spec() {
    let repo = TestRepo::new();
    fixture(&repo);
    let payments = SpecName::parse("payments").unwrap();
    assert!(matches!(
        derive::staleness(&snapshot(&repo), &snapshot(&repo).specs[&payments]),
        Staleness::Stale { .. }
    ));

    // `kanspec features --confirm payments` (S6's verb) records this in the spec's
    // FRONTMATTER, git-tracked, signed — not in the cache and not as a counter (D-10).
    repo.write(
        ".kanspec/specs/payments.md",
        "---\nfeature: Payments\ncode: [src/payments/**]\n\
         stale_ack: {sha: bbb2222, at: 2026-08-31T11:00:00Z, by: trevor, why: refactor only}\n\
         ---\n# payments\n",
    );
    let s = snapshot(&repo);
    assert!(
        matches!(derive::staleness(&s, &s.specs[&payments]), Staleness::Ok),
        "the attestation is the new anchor"
    );

    // ...and it is still the anchor after the cache is gone and put back.
    wipe_cache(&repo);
    write_cache(&repo);
    let s = snapshot(&repo);
    assert!(matches!(
        derive::staleness(&s, &s.specs[&payments]),
        Staleness::Ok
    ));
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. the cache carries no state beyond its own bytes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn putting_the_cache_back_reproduces_the_original_output_byte_for_byte() {
    let repo = TestRepo::new();
    fixture(&repo);

    let before = repo.ks(["status"]).stdout;
    let before_json = repo.ks(["status", "--json"]).stdout;
    assert!(
        before.contains("in main") && before.contains("STALLED"),
        "the fixture must actually exercise the git overlay:\n{before}"
    );

    wipe_cache(&repo);
    let wiped = repo.ks(["status"]).stdout;
    assert_ne!(before, wiped, "a wipe is visible — it is not a no-op");
    assert!(
        wiped.contains("never scanned"),
        "and it says so out loud:\n{wiped}"
    );

    // One `scan` would rewrite exactly these bytes. Restoring them is the same thing
    // without depending on S3.
    write_cache(&repo);
    assert_eq!(
        repo.ks(["status"]).stdout,
        before,
        "if any answer were accumulated rather than derived, it could not come back"
    );
    assert_eq!(repo.ks(["status", "--json"]).stdout, before_json);
}

#[test]
fn a_corrupt_cache_is_the_same_as_no_cache_and_never_a_wrong_answer() {
    let repo = TestRepo::new();
    fixture(&repo);
    let wiped = {
        wipe_cache(&repo);
        repo.ks(["status"]).stdout
    };

    for broken in [
        "",
        "{",
        "not json at all",
        r#"{"version":999,"scanned_at":"2026-08-31T11:56:00Z"}"#,
        r#"{"version":1,"tickets":{"t-31aa":{"status":"merged"}}}"#,
    ] {
        std::fs::create_dir_all(repo.root.join(".kanspec/cache")).unwrap();
        repo.write(".kanspec/cache/gitstate.json", broken);
        let run = repo.ks(["status"]);
        assert_eq!(run.code, 0, "a disposable cache must never fail a command");
        assert_eq!(
            run.stdout, wiped,
            "a cache that cannot be read is NO facts, not wrong facts: {broken:?}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// and what the wipe DOES change, stated exactly
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn the_only_answers_a_wipe_changes_are_the_ones_git_owns() {
    let repo = TestRepo::new();
    fixture(&repo);

    let before: Vec<String> = derive::attention(&snapshot(&repo))
        .iter()
        .map(|a| format!("{:?} {} {}", a.owner, a.subject, a.line))
        .collect();
    wipe_cache(&repo);
    let after: Vec<String> = derive::attention(&snapshot(&repo))
        .iter()
        .map(|a| format!("{:?} {} {}", a.owner, a.subject, a.line))
        .collect();

    let gone: Vec<&String> = before.iter().filter(|l| !after.contains(l)).collect();
    let new: Vec<&String> = after.iter().filter(|l| !before.contains(l)).collect();

    // Exactly three lines depend on the cache, and every one of them is a git fact:
    // t-31aa landed, t-66d1 is claimable because it landed, and payments is stale.
    assert!(
        gone.iter().any(|l| l.contains("in main")),
        "the in-main line is a git fact: {gone:?}"
    );
    assert!(
        gone.iter().any(|l| l.contains("merges touched")),
        "the staleness line is a git fact: {gone:?}"
    );
    assert!(
        gone.iter().any(|l| l.contains("ready · auth")),
        "readiness through an in-main dep is a git fact (D-17): {gone:?}"
    );
    assert!(
        new.is_empty(),
        "a wipe may only SUBTRACT knowledge; it invented {new:?}"
    );
    // ...and the tripwire that reads only the log is untouched.
    assert!(before.iter().any(|l| l.contains("STALLED")));
    assert!(after.iter().any(|l| l.contains("STALLED")));
}

// ─────────────────────────────────────────────────────────────────────────────
// and what the whole thing renders as — the product's face
// ─────────────────────────────────────────────────────────────────────────────

/// DESIGN.md's anti-stuck transcript, against real files on a real disk. This is the most
///-used command in the product, so its shape is pinned line by line rather than sampled.
#[test]
fn status_renders_design_mds_transcript() {
    let repo = TestRepo::new();
    fixture(&repo);
    let out = repo.ks(["status"]).stdout;
    let lines: Vec<&str> = out.lines().collect();

    // grouped by WHO OWES THE NEXT VERB, with a count per group
    assert_eq!(lines[0], " YOU (1)", "{out}");
    assert!(lines[1].starts_with(" \u{21c2} t-31aa"), "{out}");
    assert!(
        lines[1].contains("in main 2h (gh-pr #142 \u{b7} checked 4m ago), not closed"),
        "the badge names its method, its PR and its own age: {out}"
    );
    assert!(
        lines[1]
            .trim_end()
            .ends_with("\u{2192} kanspec done t-31aa"),
        "{out}"
    );

    assert_eq!(lines[2], " AGENT (1)", "{out}");
    assert!(
        lines[3].starts_with(" \u{25cb} t-66d1")
            && lines[3].contains("ready \u{b7} auth \u{b7} unblocked when t-31aa closed"),
        "{out}"
    );

    assert_eq!(lines[4], " WATCHING (2)", "{out}");
    assert!(
        lines[5].starts_with(" \u{25d0} t-88fe")
            && lines[5].contains("STALLED: doing, no commits or updates for 3h")
            && lines[5].contains("kanspec park t-88fe --why"),
        "{out}"
    );
    assert!(
        lines[6].starts_with(" \u{26a0} payments")
            && lines[6].contains("3 merges touched src/payments/** since spec last edited")
            && lines[6].contains("kanspec features --stale"),
        "{out}"
    );

    // the `sync = "batch"` reminder (D-13), and the cache's own age
    assert!(
        lines[7].contains("tracker changes pending"),
        "batch mode reminds when tracker changes are uncommitted: {out}"
    );
    assert!(lines[8].contains("merge state checked 4m ago"), "{out}");

    // every line that names an owed verb names exactly one command to run
    for l in lines
        .iter()
        .filter(|l| l.starts_with(' ') && !l.ends_with(')'))
    {
        assert!(
            l.contains('\u{2192}') || l.contains("merge state"),
            "a line with no fix is a line nobody can act on: {l:?}"
        );
    }
}

#[test]
fn status_json_is_the_same_answer_grouped_for_an_agent() {
    let repo = TestRepo::new();
    fixture(&repo);
    let j: serde_json::Value = repo.json(&["status"]);

    assert_eq!(j["you"].as_array().unwrap().len(), 1);
    assert_eq!(j["agent"].as_array().unwrap().len(), 1);
    assert_eq!(j["watching"].as_array().unwrap().len(), 2);
    assert_eq!(j["you"][0]["owner"], "you");
    assert_eq!(j["you"][0]["subject"], "t-31aa");
    assert_eq!(j["you"][0]["fix"], "kanspec done t-31aa");
    assert_eq!(j["watching"][0]["owner"], "watching");
    assert!(j["cache_age_secs"].as_u64().unwrap() > 0);
    assert!(j["pending_changes"].as_u64().unwrap() > 0);
    assert!(!j["next"].as_array().unwrap().is_empty());

    // --owner narrows the answer without changing any line in it
    let you: serde_json::Value = repo.json(&["status", "--owner", "you"]);
    assert_eq!(you["you"], j["you"]);
    assert!(you["agent"].as_array().unwrap().is_empty());
    assert!(you["watching"].as_array().unwrap().is_empty());
}

#[test]
fn a_board_with_nothing_owed_says_so_rather_than_printing_nothing() {
    let repo = TestRepo::new();
    ticket(
        &repo,
        "t-0001",
        "done",
        "",
        "- 2026-08-31T09:00Z  todo     trevor                new\n\
         - 2026-08-31T09:30Z  doing    trevor                start\n\
         - 2026-08-31T10:00Z  review   trevor                ship\n\
         - 2026-08-31T10:30Z  done     trevor                done\n",
    );
    let out = repo.ks(["status"]).stdout;
    assert!(out.contains("nothing owed"), "{out}");
    assert!(
        out.contains("never scanned"),
        "no cache yet, and it says so: {out}"
    );
}
