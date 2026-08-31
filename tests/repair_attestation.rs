//! `tests/repair_attestation.rs`
//!
//! Proves: `repair` cannot launder an unmerged ticket into `done`; it still rescues
//! everything D-12 built it for; and what it DOES attest is badged everywhere afterwards.
//!
//! Owner: **S3**.
//!
//! `repair` is the one verb whose logged state is authoritative, which is exactly why it
//! needs a limit. Vouching is not evidence: `done` is the one state this tool computes from
//! git rather than accepting on anybody's word, and an attestation must not be able to buy
//! it. Everything below runs the real binary against a real repo, the way the two-command
//! route that opened this hole was found.

mod common;

use common::TestRepo;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct DoctorJson {
    findings: Vec<FindingJson>,
}

#[derive(Debug, Deserialize)]
struct FindingJson {
    check: String,
    severity: String,
    subject: String,
    message: String,
    fix: String,
}

impl DoctorJson {
    fn of(&self, check: &str) -> Vec<&FindingJson> {
        self.findings.iter().filter(|f| f.check == check).collect()
    }
}

/// The refusal envelope every gate prints under `--json`.
#[derive(Debug, Deserialize)]
struct ErrJson {
    ok: bool,
    error: ErrBody,
}

#[derive(Debug, Deserialize)]
struct ErrBody {
    kind: String,
    code: Option<String>,
    message: String,
    fix: Vec<String>,
}

/// `new -> start -> ship`: a ticket that was shipped for review and never landed.
const SHIPPED: &[&str] = &[
    "- 2026-08-30T14:02Z  todo     trevor                new",
    "- 2026-08-30T14:20Z  doing    trevor                start (branch + worktree created)",
    "- 2026-08-30T16:41Z  review   trevor                ship (pr 142)",
];

fn write_ticket(repo: &TestRepo, id: &str, state: &str, log: &[&str]) {
    let body = log
        .iter()
        .map(|l| format!("{l}\n"))
        .collect::<Vec<_>>()
        .concat();
    repo.write(
        &format!(".kanspec/tickets/{id}.md"),
        &format!(
            "---\n\
             id: {id}\n\
             title: Rate-limit login endpoint\n\
             state: {state}\n\
             created: 2026-08-30T14:02:11Z\n\
             ---\n\
             Implement [auth.lockout].\n\
             \n\
             ## Log\n{body}"
        ),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// the hole
// ─────────────────────────────────────────────────────────────────────────────

/// THE repro, in the two documented commands it took. `done` refuses because nothing is on
/// main; a `sed` writes `done` into the frontmatter anyway; `repair` used to bless it, and
/// `doctor` then reported a clean bill of health over an unmerged, closed ticket.
///
/// `--no-code` was already guarded against exactly this ("cannot close work that has a
/// branch"). `repair` shipped strictly more permissive than the escape that was guarded.
#[test]
fn the_two_command_route_that_closed_an_unmerged_ticket_is_refused() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-5443", "review", SHIPPED);

    // step 1 — the honest refusal
    let done = repo.ks(["done", "t-5443", "--no-followups", "--no-quirks"]);
    assert_eq!(done.code, 1, "nothing is on main:\n{}", done.stderr);

    // step 2 — `sed -i '' 's/state: review/state: done/'`
    write_ticket(&repo, "t-5443", "done", SHIPPED);

    // step 3 — the laundering command, now refused BY NAME
    let run = repo.ks(["repair", "t-5443", "--why", "trust me"]);
    assert_eq!(
        run.code, 1,
        "stdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    let e: ErrJson = repo.json(&["repair", "t-5443", "--why", "trust me"]);
    assert!(!e.ok);
    assert_eq!(e.error.kind, "gate");
    assert_eq!(e.error.code.as_deref(), Some("repair_cannot_close"));
    assert!(
        e.error
            .message
            .contains("nothing in the ## Log says this work landed"),
        "{}",
        e.error.message
    );

    // Invariant 9: the refusal points at real, runnable commands — the recorded override
    // for a genuine squash, and the lossless undo that names the state the log reached.
    assert!(
        e.error
            .fix
            .iter()
            .any(|f| f.starts_with("kanspec scan --confirm t-5443 --why")),
        "{:?}",
        e.error.fix
    );
    assert!(
        e.error
            .fix
            .iter()
            .any(|f| f.contains("set `state:` back to review")),
        "{:?}",
        e.error.fix
    );

    // …and the file is untouched, so the hand-edit is still on the record rather than
    // quietly blessed.
    let j: DoctorJson = repo.json(&["doctor"]);
    assert_eq!(j.of("log_trail").len(), 1, "{:#?}", j.findings);
    assert_eq!(repo.ks(["doctor"]).code, 1);
    assert!(
        !repo.read(".kanspec/tickets/t-5443.md").contains("repair"),
        "a refused transaction writes nothing"
    );
}

/// The worse half of the same hole: a brand-new `todo` ticket with no branch, no commit and
/// no PR, hand-edited straight to `done` — a jump the transition table forbids for every
/// real verb, and which `repair` was the only route to.
#[test]
fn a_todo_ticket_hand_edited_to_done_cannot_be_attested_either() {
    let repo = TestRepo::new();
    write_ticket(
        &repo,
        "t-5443",
        "done",
        &["- 2026-08-30T14:02Z  todo     trevor                new"],
    );
    let e: ErrJson = repo.json(&["repair", "t-5443", "--why", "it's fine"]);
    assert_eq!(e.error.code.as_deref(), Some("repair_cannot_close"));
}

// ─────────────────────────────────────────────────────────────────────────────
// what must keep working — D-12's whole reason to exist
// ─────────────────────────────────────────────────────────────────────────────

/// The rescue. An imported ticket has no `## Log` at all, so `replay` reaches no state and
/// the write path refuses every verb; without `repair` that repo is permanently unwritable.
/// Nothing here claims work shipped, so nothing here is guarded.
#[test]
fn an_imported_ticket_with_no_log_is_still_rescued_by_an_attestation() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-5443", "review", &[]);
    assert_eq!(repo.ks(["doctor"]).code, 1, "unwritable until attested");

    let run = repo.ks(["repair", "t-5443", "--why", "imported from the old tracker"]);
    assert_eq!(
        run.code, 0,
        "stdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert!(
        repo.read(".kanspec/tickets/t-5443.md")
            .contains("attested review — imported from the old tracker"),
        "the attestation lives in the ## Log, signed"
    );

    let run = repo.ks(["doctor"]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    let j: DoctorJson = repo.json(&["doctor"]);
    assert!(
        j.findings.is_empty(),
        "a non-terminal attestation claims nothing about shipped work: {:#?}",
        j.findings
    );
}

/// A trail a union merge broke on a ticket that WAS legitimately closed. Every `done` line
/// in a `## Log` was written by the gate against a sealed proof or a recorded `--no-code`
/// waiver, so the close is already in the record and re-attesting it asserts nothing new.
/// Refusing here would leave the ticket permanently unwritable — the failure D-12 prevents.
#[test]
fn a_close_the_log_already_carries_can_still_be_re_attested() {
    let repo = TestRepo::new();
    write_ticket(
        &repo,
        "t-5443",
        "done",
        &[
            "- 2026-08-30T14:02Z  todo     trevor                new",
            "- 2026-08-30T14:20Z  doing    trevor                start",
            // what a union-merged `## Log` actually produces
            "- 2026-08-30T14:25Z  doing    claude/sess-a91       start",
            "- 2026-08-30T16:41Z  review   trevor                ship",
            "- 2026-08-30T17:02Z  done     trevor                done (in main a1b9c3d via gh-pr #142)",
        ],
    );
    assert_eq!(repo.ks(["doctor"]).code, 1, "the duplicate claim breaks it");

    let run = repo.ks([
        "repair",
        "t-5443",
        "--why",
        "union merge duplicated the claim",
    ]);
    assert_eq!(
        run.code, 0,
        "stdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );

    // The trail is repaired, and the close is not condemned — but it is on the record as
    // attested rather than replayed, at warning grade, which does not fail CI.
    let j: DoctorJson = repo.json(&["doctor"]);
    assert!(j.of("log_trail").is_empty(), "{:#?}", j.findings);
    let f = j.of("attested_state");
    assert_eq!(f.len(), 1, "{:#?}", j.findings);
    assert_eq!(f[0].severity, "warning");
    assert_eq!(f[0].subject, "t-5443");
    assert!(f[0].message.contains("by attestation"), "{}", f[0].message);
    assert_eq!(f[0].fix, "kanspec log t-5443");
    assert_eq!(repo.ks(["doctor"]).code, 0, "a warning is not a CI failure");
}

/// `dropped` is terminal too, and deliberately unguarded: a drop is an act, not a merge,
/// and `--why` is already the whole of its evidence.
#[test]
fn an_imported_drop_needs_no_merge_evidence() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-5443", "dropped", &[]);
    let run = repo.ks(["repair", "t-5443", "--why", "abandoned in the old tracker"]);
    assert_eq!(
        run.code, 0,
        "stdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert_eq!(
        repo.ks(["doctor"]).code,
        0,
        "recorded, not condemned — and it is a warning, not an error"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// the badge — an attested close cannot hide among the proven ones
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LsJson {
    rows: Vec<LsRow>,
}

#[derive(Debug, Deserialize)]
struct LsRow {
    id: String,
    badge_text: String,
}

/// The `scan --confirm` precedent, applied: a result a human vouched for is labelled as
/// such wherever it is shown, so it cannot pass for one git proved. `ls`, `show` and the
/// board all read the same `derive::badge`, and `doctor` carries the standing check.
#[test]
fn an_attested_close_is_badged_wherever_a_ticket_is_shown() {
    let repo = TestRepo::new();
    write_ticket(
        &repo,
        "t-5443",
        "done",
        &[
            "- 2026-08-30T14:02Z  todo     trevor                new",
            "- 2026-08-30T14:20Z  doing    trevor                start",
            "- 2026-08-30T14:25Z  doing    claude/sess-a91       start",
            "- 2026-08-30T16:41Z  review   trevor                ship",
            "- 2026-08-30T17:02Z  done     trevor                done (in main a1b9c3d via gh-pr #142)",
        ],
    );
    repo.ks([
        "repair",
        "t-5443",
        "--why",
        "union merge duplicated the claim",
    ])
    .ok();

    let ls: LsJson = repo.json(&["ls", "--all"]);
    let row = ls
        .rows
        .iter()
        .find(|r| r.id == "t-5443")
        .expect("the closed ticket is listed");
    assert!(
        row.badge_text.contains("attested done by trevor"),
        "the merge column must not read like proof: {}",
        row.badge_text
    );

    for argv in [
        vec!["show", "t-5443"],
        vec!["ls", "--all"],
        vec!["board"],
        vec!["doctor"],
    ] {
        let out = repo.ks(&argv).stdout;
        assert!(
            out.contains("attested"),
            "`kanspec {}` hides the attestation:\n{out}",
            argv.join(" ")
        );
    }
}

/// The attestation is spent the moment an ordinary verb moves the ticket on: a ticket
/// rescued back to a working state and then driven forward stands on its own trail again
/// and wears no badge.
#[test]
fn an_attestation_a_later_verb_moved_past_leaves_no_badge_behind() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-5443", "todo", &[]);
    repo.ks(["repair", "t-5443", "--why", "imported"]).ok();
    repo.ks(["start", "t-5443"]).ok();

    let j: DoctorJson = repo.json(&["doctor"]);
    assert!(j.of("attested_state").is_empty(), "{:#?}", j.findings);
    let ls: LsJson = repo.json(&["ls"]);
    let row = ls.rows.iter().find(|r| r.id == "t-5443").expect("listed");
    assert!(
        !row.badge_text.contains("attested"),
        "the badge belongs to the attested state, not to the ticket forever: {}",
        row.badge_text
    );
}
