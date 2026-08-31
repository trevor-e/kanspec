//! `tests/purity.rs`
//!
//! Proves: source grep: derive.rs imports no std::fs, no std::process, calls no Utc::now()
//!
//! Owner: **S4**.
//!
//! Two halves, because a grep alone is a deterrent rather than a guarantee (R-3):
//!
//! 1. **The grep**, over `src/derive.rs` with comments stripped — plus an allowlist of the
//!    modules its `use` lines may name, which closes the `use std::fs as f;` hole a bare
//!    substring search walks straight past. The scanner is itself tested against synthetic
//!    violating sources, so a purity test that has quietly stopped looking is a failure.
//! 2. **The behavioural half**: shifting `Snapshot::now` must change the answers. A
//!    `Utc::now()` hiding anywhere in the projection would make the tripwires fire (or not)
//!    independently of the snapshot's clock, which is exactly what the assertions below
//!    rule out — and no grep can.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, TimeZone, Utc};

use kanspec::cache::{BranchFact, MergeFact, MergeStatus, SpecAnchor};
use kanspec::config::Config;
use kanspec::derive;
use kanspec::git::Method;
use kanspec::ids::{SpecName, TicketId};
use kanspec::logentry::LogEntry;
use kanspec::model::{Snapshot, Spec, SpecFm, Ticket, TicketFm};
use kanspec::transitions::{State, Verb};

/// The file under test, read at COMPILE time so this cannot silently pass by looking in
/// the wrong directory.
const DERIVE_RS: &str = include_str!("../src/derive.rs");
/// The checks are `fn(&Snapshot) -> Vec<Finding>` by contract; hold them to it too.
const DOCTOR_RS: &str = include_str!("../src/doctor.rs");

/// Every token that would mean the projection reached outside its `Snapshot`.
const BANNED: &[(&str, &str)] = &[
    ("std::fs", "the filesystem"),
    ("std::process", "a subprocess"),
    ("std::env", "the environment"),
    ("Command::new", "a subprocess"),
    ("Utc::now", "the wall clock"),
    ("Local::now", "the wall clock"),
    ("SystemTime::now", "the wall clock"),
    ("Instant::now", "the wall clock"),
    ("unsafe", "unchecked memory"),
];

/// Which modules `derive.rs`'s `use` lines may name. This is the half that survives an
/// alias: `use std::fs as f;` is invisible to a search for `fs::write`, but it cannot
/// hide from a list of what may be imported at all.
const ALLOWED_USE_ROOTS: &[&str] = &[
    "std::collections",
    "std::time",
    "chrono",
    "globset",
    "serde",
    "crate::cache",
    "crate::config",
    "crate::git",
    "crate::ids",
    "crate::logentry",
    "crate::model",
    "crate::out",
    "crate::transitions",
    // the inline unit tests import their own fixtures
    "super",
];

/// Source with comments removed — the module header of `derive.rs` NAMES the banned
/// tokens in prose, and a scanner that trips over the documentation of its own rule is
/// useless.
fn code_only(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        let code = match line.find("//") {
            // Not a string-aware split, deliberately: a `//` inside a literal would only
            // ever make this scanner look at LESS, so it is reported below as a mismatch
            // rather than trusted.
            Some(i) if !line[..i].contains('"') => &line[..i],
            _ => line,
        };
        out.push_str(code);
        out.push('\n');
    }
    out
}

fn offences(src: &str) -> Vec<String> {
    let code = code_only(src);
    let mut out = Vec::new();
    for (i, line) in code.lines().enumerate() {
        for (token, what) in BANNED {
            if line.contains(token) {
                out.push(format!("line {}: `{token}` — {what}", i + 1));
            }
        }
    }
    out
}

/// Everything above the inline `#[cfg(test)] mod tests` — the half that ships. The test
/// module legitimately imports `std::path::PathBuf` to build `Ticket` literals, and a
/// fixture builder is not the projection.
fn production(src: &str) -> &str {
    match src.find("#[cfg(test)]") {
        Some(i) => &src[..i],
        Option::None => src,
    }
}

fn bad_imports(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in code_only(src).lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("use ") else {
            continue;
        };
        let path = rest.trim_end_matches(';');
        if !ALLOWED_USE_ROOTS.iter().any(|r| path.starts_with(r)) {
            out.push(t.to_string());
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. the grep
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn derive_rs_reaches_outside_its_snapshot_nowhere() {
    let found = offences(DERIVE_RS);
    assert!(
        found.is_empty(),
        "src/derive.rs is the PURE projection — every fact it needs arrives in the \
         Snapshot:\n  {}",
        found.join("\n  ")
    );
}

#[test]
fn derive_rs_imports_only_what_a_pure_projection_can_need() {
    let bad = bad_imports(production(DERIVE_RS));
    assert!(
        bad.is_empty(),
        "an import outside the allowlist is how `use std::fs as f;` would sneak past the \
         substring scan:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn the_doctor_checks_are_pure_over_the_snapshot_too() {
    // `RunCheck = fn(&Snapshot) -> Vec<Finding>`: a check that could shell out would be a
    // second, slower, un-unit-testable copy of `scan`.
    let found = offences(DOCTOR_RS);
    assert!(found.is_empty(), "src/doctor.rs:\n  {}", found.join("\n  "));
}

#[test]
fn the_scanner_actually_fires_on_a_violation() {
    // A purity test that has stopped looking passes forever. These are the exact shapes a
    // future edit would take.
    for src in [
        "use std::fs;\nfn f() {}\n",
        "fn f() { let x = std::process::Command::new(\"git\"); }\n",
        "fn f() -> DateTime<Utc> { Utc::now() }\n",
        "fn f() { let t = SystemTime::now(); }\n",
        "fn f() { unsafe { } }\n",
    ] {
        assert!(
            !offences(src).is_empty(),
            "the scanner missed a violation: {src:?}"
        );
    }
    assert!(
        !bad_imports("use std::fs as f;\n").is_empty(),
        "the import allowlist missed an alias"
    );
    assert!(
        !bad_imports("use crate::store::load_snapshot;\n").is_empty(),
        "the import allowlist missed an IO module"
    );
    // ...and does not fire on the documentation of its own rule
    assert!(offences("//! no `use std::fs`, no `Utc::now()`\n").is_empty());
    assert!(offences("// std::process is banned here\n").is_empty());
    assert!(bad_imports("use crate::model::Snapshot;\n").is_empty());
}

#[test]
fn the_production_half_is_where_the_import_rule_applies() {
    let src = "use crate::model::Snapshot;\n#[cfg(test)]\nmod tests { use std::fs; }\n";
    assert!(!production(src).contains("mod tests"));
    assert!(bad_imports(production(src)).is_empty());
    // ...and the banned-token scan still covers the WHOLE file, test module included.
    assert!(!offences(src).is_empty());
    assert!(
        production(DERIVE_RS).len() > 1000 && production(DERIVE_RS).len() < DERIVE_RS.len(),
        "the split must find the real test module"
    );
}

#[test]
fn the_comment_stripper_never_hides_more_than_a_comment() {
    // If `code_only` ever removed real code, every assertion above would weaken silently.
    let src = "let a = 1; // trailing\nlet b = \"http://x\";\n";
    let out = code_only(src);
    assert!(out.contains("let a = 1;") && !out.contains("trailing"));
    assert!(
        out.contains("http://x"),
        "a `//` inside a string literal is not a comment: {out:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. the behavioural half — the clock is an INPUT
// ─────────────────────────────────────────────────────────────────────────────

fn at(h: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap() - chrono::Duration::hours(h)
}

fn ticket(id: &str, state: State) -> Ticket {
    Ticket {
        fm: TicketFm {
            id: TicketId::parse(id).unwrap(),
            title: "t".into(),
            state,
            spec: None,
            proposal: None,
            item: None,
            deps: Vec::new(),
            followup_of: None,
            discovered_in: None,
            branch: None,
            worktree: None,
            claimed_by: None,
            pr: None,
            head: None,
            spec_unchanged: None,
            created: at(500),
            extra: BTreeMap::new(),
        },
        path: PathBuf::from(format!(".kanspec/tickets/{id}.md")),
        body: String::new(),
        steps: Vec::new(),
        log: Vec::new(),
        mtime: SystemTime::UNIX_EPOCH,
    }
}

fn snapshot_at(now: DateTime<Utc>) -> Snapshot {
    let mut s = Snapshot::empty(Config::default(), now);
    let mut doing = ticket("t-0001", State::Doing);
    doing.log = vec![LogEntry {
        at: at(3),
        state: State::Doing,
        actor: "claude/sess-a91".into(),
        verb: Verb::Start,
        note: None,
    }];
    s.tickets.insert(doing.fm.id.clone(), doing);

    let mut review = ticket("t-0002", State::Review);
    review.log = vec![LogEntry {
        at: at(24 * 9),
        state: State::Review,
        actor: "trevor".into(),
        verb: Verb::Ship,
        note: None,
    }];
    s.tickets.insert(review.fm.id.clone(), review);
    s
}

#[test]
fn every_tripwire_answer_moves_when_the_snapshots_clock_moves() {
    let doing = TicketId::parse("t-0001").unwrap();
    let review = TicketId::parse("t-0002").unwrap();

    // "now" == the fixture's now: 3h idle (> the 2h window) and 9d in review (> 7d).
    let s = snapshot_at(at(0));
    assert_eq!(
        derive::stalled(&s, &s.tickets[&doing]).map(|d| d.as_secs()),
        Some(3 * 3600)
    );
    assert!(derive::dwell(&s, &s.tickets[&review]).is_some());

    // Wind the SNAPSHOT's clock back an hour: the same ticket has been idle 2h, which is
    // not MORE than the 2h window, and the review is a day younger.
    let s = snapshot_at(at(1));
    assert!(
        derive::stalled(&s, &s.tickets[&doing]).is_none(),
        "a wall-clock read anywhere in `stalled` would ignore Snapshot::now"
    );

    // Wind it forward: everything fires harder.
    let s = snapshot_at(at(-24));
    assert_eq!(
        derive::stalled(&s, &s.tickets[&doing]).map(|d| d.as_secs()),
        Some(27 * 3600)
    );
}

#[test]
fn the_same_snapshot_computes_the_same_answer_twice() {
    // No interior mutability, no hidden clock, no map iteration order: `compute` is a
    // function of its argument, and the server memoizes on exactly that assumption (D-22).
    let s = snapshot_at(at(0));
    let a = serde_json::to_string(&derive::compute(&s)).unwrap();
    let b = serde_json::to_string(&derive::compute(&s)).unwrap();
    assert_eq!(a, b);
}

#[test]
fn a_freshness_stamp_is_the_only_thing_that_moves_with_the_wall_clock() {
    // The badge quotes `checked_at` from the cache and renders it against `Snapshot::now`,
    // so two snapshots that differ ONLY in `now` differ only in that rendering.
    let mut a = snapshot_at(at(0));
    a.git.tickets.insert(
        TicketId::parse("t-0002").unwrap(),
        MergeFact {
            status: MergeStatus::Merged,
            sha: Some("a1b9c3d".into()),
            method: Method::GhPr,
            pr: Some(142),
            why: None,
            checked_at: at(1),
            changed: vec![],
        },
    );
    let t = a.tickets[&TicketId::parse("t-0002").unwrap()].clone();
    let badge = derive::badge(&a, &t);
    assert_eq!(
        badge.text(a.now),
        "in main (gh-pr · checked 1h ago)",
        "the badge names its own age, so it can never lie about freshness"
    );
    assert_eq!(badge.text(at(-1)), "in main (gh-pr · checked 2h ago)");
}

#[test]
fn staleness_reads_the_cache_and_the_spec_and_nothing_else() {
    // The staleness answer must be a function of (cached facts, spec frontmatter, now) —
    // in particular it must NOT walk the working tree to see whether the globs match.
    let mut s = Snapshot::empty(Config::default(), at(0));
    let name = SpecName::parse("payments").unwrap();
    let spec = Spec {
        name: name.clone(),
        fm: SpecFm {
            feature: "Payments".into(),
            code: vec!["src/payments/**".into()],
            stale_ack: None,
            extra: BTreeMap::new(),
        },
        path: PathBuf::from("/nonexistent/specs/payments.md"),
        body: String::new(),
        rules: vec![],
    };
    s.git.specs.insert(
        name.clone(),
        SpecAnchor {
            last_edit_sha: Some("deadbee".into()),
            last_edit_at: Some(at(24 * 30)),
            merges_since: 0,
            dead_globs: vec![],
        },
    );
    for id in ["t-0aa1", "t-0aa2", "t-0aa3"] {
        s.git.tickets.insert(
            TicketId::parse(id).unwrap(),
            MergeFact {
                status: MergeStatus::Merged,
                sha: None,
                method: Method::Ancestry,
                pr: None,
                why: None,
                checked_at: at(1),
                changed: vec!["src/payments/charge.ts".into()],
            },
        );
    }
    s.specs.insert(name, spec.clone());

    // Three merges, none of which has a ticket file behind it and none of whose paths
    // exist on disk — and the tripwire still fires, because it never looked.
    assert!(matches!(
        derive::staleness(&s, &spec),
        kanspec::derive::Staleness::Stale { merges: 3, .. }
    ));
}

#[test]
fn a_branch_fact_is_read_from_the_cache_never_from_git() {
    let mut s = snapshot_at(at(0));
    let id = TicketId::parse("t-0001").unwrap();
    s.git.branches.insert(
        id.clone(),
        BranchFact {
            // a branch that does not exist in any repository anywhere
            branch: Some("ks/t-0001-imaginary".into()),
            head: None,
            ahead: Some(4),
            behind: Some(0),
            last_commit_at: Some(at(0)),
            pushed: true,
        },
    );
    let t = s.tickets[&id].clone();
    assert!(matches!(
        derive::badge(&s, &t),
        kanspec::derive::Badge::Pushed
    ));
    // ...and the commit it claims counts as activity, so the STALLED tripwire clears.
    assert!(derive::stalled(&s, &t).is_none());
    assert_eq!(derive::short(Duration::from_secs(3 * 3600)), "3h");
}
