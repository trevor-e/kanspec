//! `tests/purity.rs`
//!
//! Proves: the derived-state projection is a function of its `Snapshot` and nothing else.
//!
//! Owner: **S4**.
//!
//! Purity is STRUCTURAL: `derive::compute` and every `doctor` check take `&Snapshot` and
//! nothing else — no `Ctx`, no `Git`, no path — so there is no handle through which they
//! could reach the filesystem, a subprocess or the clock. What a type signature cannot rule
//! out is a bare `Utc::now()`; this file rules that out behaviourally: shifting
//! `Snapshot::now` must change every tripwire answer, and only the freshness stamp may
//! move with the wall clock. (A source grep used to stand guard here too; it tripped on a
//! comment during the review refactor and guarded nothing the signatures do not — t-0769.)

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

// ─────────────────────────────────────────────────────────────────────────────
// the clock is an INPUT
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
        "in main (gh-pr #142 · checked 1h ago)",
        "the badge names its own age, so it can never lie about freshness"
    );
    assert_eq!(badge.text(at(-1)), "in main (gh-pr #142 · checked 2h ago)");
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
