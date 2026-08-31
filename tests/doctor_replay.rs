//! `tests/doctor_replay.rs`
//!
//! Proves: a hand-edited state fails; a legal trail passes; repair recovers
//!
//! Owner: **S4**.
//!
//! R-2 stated plainly: the seals bind the TOOL, the `## Log` binds the HUMAN.
//! `sed -i 's/state: review/state: done/'` still works on a plain file, so the answer is
//! detection — at the very next verb and in CI. Every case below is a real file on a real
//! disk, edited the way a human or a bad merge would edit it, then proved by the real
//! binary through its real exit code.

mod common;

use common::TestRepo;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct DoctorJson {
    findings: Vec<FindingJson>,
    fixed: Vec<String>,
    checks_run: usize,
    next: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct FindingJson {
    check: String,
    severity: String,
    subject: String,
    message: String,
    fix: String,
    fixable: bool,
}

impl DoctorJson {
    fn of(&self, check: &str) -> Vec<&FindingJson> {
        self.findings.iter().filter(|f| f.check == check).collect()
    }
}

/// A ticket file with the frontmatter and the `## Log` given separately, so a test can
/// doctor exactly one of them — which is the whole point.
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

/// `new -> start -> ship`, spelled exactly as `LogEntry::format` writes it.
const LEGAL: &[&str] = &[
    "- 2026-08-30T14:02Z  todo     trevor                new",
    "- 2026-08-30T14:20Z  doing    claude/sess-a91       start (branch + worktree created)",
    "- 2026-08-30T16:41Z  review   claude/sess-a91       ship (pr 142)",
];

fn doctor(repo: &TestRepo) -> DoctorJson {
    repo.json(&["doctor"])
}

#[test]
fn a_legally_reached_state_replays_clean_and_exits_zero() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-9c41", "review", LEGAL);

    let run = repo.ks(["doctor"]);
    assert_eq!(
        run.code, 0,
        "stdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout.contains("checks passed"),
        "a clean repo says so: {}",
        run.stdout
    );

    let j = doctor(&repo);
    assert!(j.findings.is_empty(), "{:#?}", j.findings);
    assert!(j.fixed.is_empty());
    assert!(j.checks_run >= 12, "every registered check ran");
    assert!(j.next.is_empty(), "nothing to do next");
}

#[test]
fn a_hand_edited_state_is_caught_by_the_very_next_run() {
    let repo = TestRepo::new();
    // `sed -i 's/state: review/state: done/'` — the log is untouched and legal; it simply
    // never reached `done`.
    write_ticket(&repo, "t-9c41", "done", LEGAL);

    let run = repo.ks(["doctor"]);
    assert_eq!(run.code, 1, "a violation is exit 1, so `doctor` gates CI");

    let j = doctor(&repo);
    let f = j.of("log_trail");
    assert_eq!(f.len(), 1, "{:#?}", j.findings);
    assert_eq!(f[0].severity, "error");
    assert_eq!(f[0].subject, "t-9c41");
    assert!(
        f[0].message.contains("frontmatter says `done`")
            && f[0].message.contains("replays to `review`"),
        "the message must say what disagrees with what: {}",
        f[0].message
    );
    assert!(
        f[0].fix.starts_with("kanspec repair t-9c41"),
        "invariant 9: {}",
        f[0].fix
    );
    assert!(
        !f[0].fixable,
        "recovery is a HUMAN attestation, never a mechanical rewrite (D-12)"
    );
}

#[test]
fn a_doctored_log_line_is_caught_even_though_the_frontmatter_agrees_with_it() {
    let repo = TestRepo::new();
    // The subtler forgery: edit the LOG so it claims `ship` reached `done`, and set the
    // frontmatter to match. Only replaying each entry against the table catches this.
    write_ticket(
        &repo,
        "t-9c41",
        "done",
        &[
            "- 2026-08-30T14:02Z  todo     trevor                new",
            "- 2026-08-30T14:20Z  doing    trevor                start",
            "- 2026-08-30T16:41Z  done     trevor                ship",
        ],
    );

    assert_eq!(repo.ks(["doctor"]).code, 1);
    let j = doctor(&repo);
    let f = j.of("log_trail");
    assert_eq!(f.len(), 1);
    assert!(
        f[0].message.contains("records `done`") && f[0].message.contains("`review`"),
        "{}",
        f[0].message
    );
}

#[test]
fn an_inserted_verb_that_the_table_never_allows_is_caught() {
    let repo = TestRepo::new();
    // Two claims with nothing between them — what a cross-machine double-claim leaves in a
    // union-merged `## Log` (R-8).
    write_ticket(
        &repo,
        "t-9c41",
        "doing",
        &[
            "- 2026-08-30T14:02Z  todo     trevor                new",
            "- 2026-08-30T14:20Z  doing    trevor                start",
            "- 2026-08-30T14:25Z  doing    claude/sess-a91       start",
        ],
    );

    assert_eq!(repo.ks(["doctor"]).code, 1);
    let j = doctor(&repo);
    assert!(
        j.of("log_trail")[0]
            .message
            .contains("not legal from doing"),
        "{:#?}",
        j.findings
    );

    // ...and `status` names the same break as a YOU line with the same fix, because the
    // anti-stuck query shows doctor's errors rather than hiding them behind a second
    // command.
    let out = repo.ks(["status"]).stdout;
    assert!(out.contains("t-9c41"), "{out}");
    assert!(out.contains("double claim"), "{out}");
}

#[test]
fn a_deleted_log_is_a_break_not_a_clean_slate() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-9c41", "doing", &[]);
    assert_eq!(repo.ks(["doctor"]).code, 1);
    let j = doctor(&repo);
    assert!(
        j.of("log_trail")[0]
            .message
            .contains("no state was ever reached"),
        "{:#?}",
        j.findings
    );
}

#[test]
fn a_backdated_entry_is_caught_because_a_merge_can_interleave_two_machines() {
    let repo = TestRepo::new();
    write_ticket(
        &repo,
        "t-9c41",
        "doing",
        &[
            "- 2026-08-30T14:20Z  todo     trevor                new",
            "- 2026-08-30T14:02Z  doing    trevor                start",
        ],
    );
    assert_eq!(repo.ks(["doctor"]).code, 1);
    assert!(doctor(&repo).of("log_trail")[0]
        .message
        .contains("goes backwards in time"));
}

#[test]
fn an_attested_repair_line_makes_a_diverged_ticket_replay_clean_again() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-9c41", "done", LEGAL);
    assert_eq!(repo.ks(["doctor"]).code, 1, "diverged");

    // What `kanspec repair t-9c41 --why "..."` records: an attributed, timestamped,
    // human-signed `repair` line whose state is authoritative (D-12). The VERB is S3's
    // (`cmd/repair.rs`); what is proved here is the semantics the verb has to produce.
    let mut log: Vec<&str> = LEGAL.to_vec();
    log.push(
        "- 2026-08-30T17:10Z  done     trevor                repair (imported from the old tracker)",
    );
    write_ticket(&repo, "t-9c41", "done", &log);

    let run = repo.ks(["doctor"]);
    assert_eq!(
        run.code, 0,
        "the attested reset must make the repo writable again:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(doctor(&repo).findings.is_empty());
}

#[test]
fn a_reserved_key_in_a_hand_edited_file_is_the_last_unguarded_hole_in_invariant_1() {
    let repo = TestRepo::new();
    // `keys.rs` makes `merged` unwritable BY TYPE, so no command can produce this. A
    // hand-edit, an import or a bad merge can — and this is the only thing that catches it.
    repo.write(
        ".kanspec/tickets/t-9c41.md",
        "---\n\
         id: t-9c41\n\
         title: Rate-limit login endpoint\n\
         state: review\n\
         merged: true\n\
         checked_at: 2026-08-30T16:00:00Z\n\
         future_knob: 7\n\
         created: 2026-08-30T14:02:11Z\n\
         ---\n\
         Body.\n\
         \n\
         ## Log\n\
         - 2026-08-30T14:02Z  todo     trevor                new\n\
         - 2026-08-30T14:20Z  doing    trevor                start\n\
         - 2026-08-30T16:41Z  review   trevor                ship\n",
    );

    assert_eq!(repo.ks(["doctor"]).code, 1);
    let j = doctor(&repo);
    let f = j.of("reserved_keys");
    assert_eq!(f.len(), 1, "one finding per entity: {:#?}", j.findings);
    assert_eq!(f[0].severity, "error");
    assert!(
        f[0].message.contains("`merged`") && f[0].message.contains("`checked_at`"),
        "{}",
        f[0].message
    );
    assert!(
        !f[0].message.contains("future_knob"),
        "a key a NEWER kanspec wrote is carried forward, not condemned: {}",
        f[0].message
    );
    assert!(
        f[0].fix.contains("t-9c41.md"),
        "the fix names the file and the line: {}",
        f[0].fix
    );

    // The forged key must change no derived answer: `status` reads git, not the file.
    let out = repo.ks(["status"]).stdout;
    assert!(
        !out.contains("in main"),
        "a file that CLAIMS to be merged is not in main: {out}"
    );
}

#[test]
fn the_findings_json_is_stable_enough_for_an_agent_to_branch_on() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-9c41", "done", LEGAL);
    let j = doctor(&repo);
    for f in &j.findings {
        assert!(!f.check.is_empty() && f.check.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        assert!(matches!(f.severity.as_str(), "error" | "warning"));
        assert!(!f.subject.is_empty() && !f.message.is_empty() && !f.fix.is_empty());
    }
    assert!(
        !j.next.is_empty(),
        "a broken repo always names a next command"
    );
}

#[test]
fn doctor_on_a_repo_with_nothing_in_it_is_clean_and_silent() {
    let repo = TestRepo::new();
    let run = repo.ks(["doctor"]);
    assert_eq!(run.code, 0);
    assert!(doctor(&repo).findings.is_empty());
}

#[test]
fn doctor_outside_a_kanspec_repo_refuses_with_the_environment_code() {
    let repo = TestRepo::new();
    std::fs::remove_dir_all(repo.root.join(".kanspec")).unwrap();
    let run = repo.ks(["doctor"]);
    assert_eq!(
        run.code, 69,
        "no `.kanspec/` is an ENVIRONMENT failure (J-7)"
    );
    assert!(run.stderr.contains("init"), "{}", run.stderr);
}

// ─────────────────────────────────────────────────────────────────────────────
// --fix goes through the ONE write path, or it does not go at all
// ─────────────────────────────────────────────────────────────────────────────

/// A proposal that says `status: closed` while still living where an agent can read it is
/// invariant 4 half-applied — the status flip landed, the directory move did not (R-1).
/// It is the one finding with a mechanical repair, and the repair is an ordinary
/// `Store::transact`, not a second writer.
#[test]
fn doctor_fix_repairs_a_half_applied_close_through_store_transact() {
    let repo = TestRepo::new();
    repo.write(
        ".kanspec/proposals/p-7de2-login-rate-limiting/proposal.md",
        "---\n\
         id: p-7de2\n\
         title: Login rate limiting\n\
         status: closed\n\
         created: 2026-08-30\n\
         ---\n\
         ## Why\nBrute force.\n",
    );

    let before = repo.json::<DoctorJson>(&["doctor"]);
    let f = before.of("closed_agree");
    assert_eq!(f.len(), 1, "{:#?}", before.findings);
    assert!(f[0].fixable, "this one CAN be repaired mechanically");
    assert!(
        before.next.iter().any(|n| n.contains("doctor --fix")),
        "and the report says so: {:?}",
        before.next
    );

    let run = repo.ks(["doctor", "--fix"]);
    assert_eq!(
        run.code, 0,
        "stdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert!(
        repo.exists(".kanspec/proposals/closed/p-7de2-login-rate-limiting/proposal.md"),
        "closed prose must become unreachable, not merely labelled"
    );
    assert!(!repo.exists(".kanspec/proposals/p-7de2-login-rate-limiting"));

    let after = repo.json::<DoctorJson>(&["doctor"]);
    assert!(after.findings.is_empty(), "{:#?}", after.findings);
    assert_eq!(repo.ks(["doctor"]).code, 0);
}

#[test]
fn doctor_fix_is_a_no_op_when_nothing_is_mechanically_repairable() {
    let repo = TestRepo::new();
    write_ticket(&repo, "t-9c41", "done", LEGAL); // diverged: a HUMAN attestation, not a rewrite
    let run = repo.ks(["doctor", "--fix"]);
    assert_eq!(run.code, 1, "an unfixable violation still fails CI");
    let j = repo.json::<DoctorJson>(&["doctor", "--fix"]);
    assert!(j.fixed.is_empty(), "nothing was touched: {:?}", j.fixed);
    assert_eq!(j.of("log_trail").len(), 1, "and the finding is still there");
}
