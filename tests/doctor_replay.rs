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

/// The same, for a ticket that was really STARTED and SHIPPED: `start` records `branch:`
/// and `ship` records `head:` and `pr:`, so the file names a branch git can be asked
/// about. That is the shape every close-gate forgery has to wear, and the shape
/// `unproven_close` speaks about.
fn write_shipped_ticket(repo: &TestRepo, id: &str, state: &str, log: &[&str]) {
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
             branch: ks/{id}-rate-limit\n\
             head: 3f2a19c7d4b6e8a0c1f5920b7e6d4a3c8b1f0e29\n\
             pr: 142\n\
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
    // Invariant 9 — and the prescription is the HONEST one. `doctor` used to print
    // `kanspec repair t-9c41 --why "..."` here, which meant the two commands it takes to
    // launder an unmerged ticket into `done` were `sed` and *the command doctor itself
    // handed you*. The lossless remedy leads instead: the `## Log` already says which state
    // this ticket legally reached, so putting the field back loses nothing.
    assert!(
        f[0].fix.starts_with("edit ")
            && f[0].fix.contains("t-9c41.md")
            && f[0].fix.ends_with("and set `state: review`"),
        "the first fix must restore the truth, not overwrite the question: {}",
        f[0].fix
    );
    assert!(
        !f[0].fix.contains("repair"),
        "the attestation is the last resort, never the prescription: {}",
        f[0].fix
    );
    // It stays discoverable — described as what it is, with its consequence attached.
    assert!(
        f[0].message.contains("kanspec repair t-9c41 --why")
            && f[0].message.contains("badged attested"),
        "{}",
        f[0].message
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
    // The rescue D-12 exists for: the frontmatter says `review`, the `## Log` only ever
    // reached `doing`, and nothing else can write this ticket until a human answers for it.
    let broken: Vec<&str> = LEGAL[..2].to_vec();
    write_ticket(&repo, "t-9c41", "review", &broken);
    assert_eq!(repo.ks(["doctor"]).code, 1, "diverged");

    // What `kanspec repair t-9c41 --why "..."` records: an attributed, timestamped,
    // human-signed `repair` line whose state is authoritative (D-12). The VERB is S3's
    // (`cmd/repair.rs`); what is proved here is the semantics the verb has to produce.
    let mut log = broken.clone();
    log.push(
        "- 2026-08-30T17:10Z  review   trevor                repair (imported from the old tracker)",
    );
    write_ticket(&repo, "t-9c41", "review", &log);

    let run = repo.ks(["doctor"]);
    assert_eq!(
        run.code, 0,
        "the attested reset must make the repo writable again:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(doctor(&repo).findings.is_empty());
}

/// The same line, attesting a TERMINAL state, is a different animal — and it used to be
/// invisible. The reset replays perfectly, so `log_trail` is silent, and before
/// `attested_state` existed this repo answered `12 checks passed` with an unmerged ticket
/// closed inside it. The trail is genuinely repaired; the CLAIM is not evidence, and
/// `doctor` now says which is which.
#[test]
fn an_attested_close_replays_clean_and_is_reported_anyway() {
    let repo = TestRepo::new();
    let mut log: Vec<&str> = LEGAL.to_vec();
    log.push(
        "- 2026-08-30T17:10Z  done     trevor                repair (attested done — trust me)",
    );
    write_ticket(&repo, "t-9c41", "done", &log);

    let j = doctor(&repo);
    assert!(
        j.of("log_trail").is_empty(),
        "the attested reset DOES repair the trail: {:#?}",
        j.findings
    );
    let f = j.of("attested_state");
    assert_eq!(f.len(), 1, "{:#?}", j.findings);
    assert_eq!(f[0].severity, "error", "an unproven close fails CI");
    assert_eq!(f[0].subject, "t-9c41");
    assert!(
        f[0].message.contains("vouched for, never proven"),
        "{}",
        f[0].message
    );
    assert!(
        f[0].fix.starts_with("kanspec scan --confirm t-9c41"),
        "invariant 9: {}",
        f[0].fix
    );
    assert_eq!(repo.ks(["doctor"]).code, 1);

    // ONE check owns the attested close. `unproven_close` deliberately stands down on a
    // ticket carrying an attestation: saying the same thing in two voices is not two
    // proofs, and D-12's whole point is that the escape stays visible rather than doubled.
    assert!(
        j.of("unproven_close").is_empty(),
        "the attestation is `attested_state`'s finding, not a second one: {:#?}",
        j.findings
    );

    // …and `status` names it, because the anti-stuck query promotes doctor's errors to YOU
    // lines rather than hiding them behind a second command.
    let out = repo.ks(["status"]).stdout;
    assert!(
        out.contains("t-9c41") && out.contains("never proven"),
        "{out}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// a close is a CLAIM until something outside the ## Log corroborates it
// ─────────────────────────────────────────────────────────────────────────────

/// THE forgery this check exists for, and the one this repo used to answer
/// `13 checks passed` to.
///
/// `sed -i 's/state: review/state: done/'` on its own is caught by `log_trail`. Append ONE
/// well-formed line and the trail replays *perfectly* to the state the frontmatter claims;
/// no `repair` verb appears, so `attested_state` is silent too. Two edits to a plain text
/// file, and a provably unmerged ticket was closed with exit 0.
///
/// The log cannot fabricate its way out of this one: every close the gate grants records
/// the commit it was granted against (`in main <sha> via <method>`) or the durable
/// `--no-code` waiver, and neither of those can be minted without git or a signed reason.
/// A close carrying neither, on a ticket that names a branch, was not written by the gate.
#[test]
fn a_forged_close_with_nothing_behind_it_is_caught() {
    let repo = TestRepo::new();
    let mut log: Vec<&str> = LEGAL.to_vec();
    log.push("- 2026-08-30T17:10Z  done     trevor                done");
    write_shipped_ticket(&repo, "t-9c41", "done", &log);

    let j = doctor(&repo);
    assert!(
        j.of("log_trail").is_empty(),
        "the forged trail replays clean — which is exactly why it needed its own check: {:#?}",
        j.findings
    );
    assert!(
        j.of("attested_state").is_empty(),
        "no `repair` verb, so the attestation check never sees it: {:#?}",
        j.findings
    );

    let f = j.of("unproven_close");
    assert_eq!(f.len(), 1, "{:#?}", j.findings);
    assert_eq!(f[0].severity, "error", "an uncorroborated close fails CI");
    assert_eq!(f[0].subject, "t-9c41");
    assert!(
        f[0].message.contains("nothing outside its own ## Log"),
        "the message must say what is missing: {}",
        f[0].message
    );
    // Invariant 9, and the honest one: ask GIT what happened to this ticket. A targeted
    // scan re-runs the ladder on a terminal ticket, so if the work really did land this
    // command is also the repair — and if it did not, nothing here launders anything.
    assert_eq!(f[0].fix, "kanspec scan --explain t-9c41");
    assert!(
        !f[0].fixable,
        "there is nothing to rewrite: the remedy is evidence, or a truer state"
    );
    // The other two remedies stay discoverable, described as what they are.
    assert!(
        f[0].message.contains("kanspec scan --confirm t-9c41 --why")
            && f[0].message.contains("--no-code"),
        "{}",
        f[0].message
    );
    assert_eq!(repo.ks(["doctor"]).code, 1, "so `doctor` gates CI");

    // …and `status` names it, because the anti-stuck query promotes doctor's errors to YOU
    // lines rather than hiding them behind a second command.
    let out = repo.ks(["status"]).stdout;
    assert!(
        out.contains("t-9c41") && out.contains("nothing outside its own ## Log"),
        "{out}"
    );

    // The forged close changes no DERIVED answer either: merge state is computed from git
    // and never read out of the log, so the badge stays honest whatever the file says.
    let ls = repo.ks(["ls", "--all"]).stdout;
    assert!(
        !ls.contains("in main"),
        "a ## Log that CLAIMS a close is not a merge: {ls}"
    );
}

/// The gate-granted close, end to end through the REAL binary against a REAL merge — the
/// one path that must never be reported. `plan_done` records the commit it was granted
/// against, and that record travels through git, so the corroboration survives a cache
/// wipe: it is not the disposable badge doing the work.
#[test]
fn a_gate_granted_close_is_never_reported_even_with_the_cache_wiped() {
    let repo = TestRepo::with_merges();
    // The fixture tickets carry `spec: auth`; give the spec a file so `half_applied` is
    // not what this test ends up measuring.
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login\ncode: []\n---\n# auth\n",
    );

    // t-9c41 is the TRUE merge shape: ancestry alone proves it, with no `gh` in sight.
    let run = repo.ks([
        "done",
        "t-9c41",
        "--actually-done",
        "2",
        "--no-followups",
        "--no-quirks",
    ]);
    assert_eq!(
        run.code, 0,
        "stdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );

    let file = repo.read(".kanspec/tickets/t-9c41.md");
    assert!(
        file.contains("done (in main "),
        "the gate records the commit it was granted against:\n{file}"
    );

    // The cache is gitignored and disposable. The corroboration must not be.
    std::fs::remove_dir_all(repo.root.join(".kanspec/cache")).ok();
    let j = doctor(&repo);
    assert!(
        j.of("unproven_close").is_empty(),
        "a close the gate granted is corroborated by its own record: {:#?}",
        j.findings
    );
}

/// The recorded `--no-code` escape, end to end through the REAL binary. The waiver is
/// prose under `## Log` — signed, dated, git-tracked — and it is what stands behind a
/// close that never had a merge to prove.
#[test]
fn a_recorded_no_code_close_is_never_reported() {
    let repo = TestRepo::new();
    let created: serde_json::Value = repo.json(&["new", "Tidy the changelog"]);
    let id = created["id"].as_str().expect("a minted id").to_string();
    repo.ks(["start", &id]).ok();
    repo.ks([
        "done",
        &id,
        "--no-code",
        "--why",
        "changelog only, no code changed",
        "--no-followups",
        "--no-quirks",
    ])
    .ok();

    let file = repo.read(&format!(".kanspec/tickets/{id}.md"));
    assert!(file.contains("no-code waiver by trevor"), "{file}");

    let run = repo.ks(["doctor"]);
    assert_eq!(
        run.code, 0,
        "the recorded escape is evidence, not an anomaly:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(doctor(&repo).findings.is_empty());
}

/// The recorded human override (D-11). `scan --confirm` writes `in main <sha> — <why>`
/// into the `## Log`, and `plan_confirm` refuses without both a commit and a reason —
/// so the close that follows stands on a commit somebody named, not on an assertion.
#[test]
fn a_close_the_recorded_human_override_stands_behind_is_never_reported() {
    let repo = TestRepo::new();
    let mut log: Vec<&str> = LEGAL.to_vec();
    log.push(
        "- 2026-08-30T17:05Z  review   trevor                confirm (in main 3f2a19c7 — squash \
         merged by hand, verified)",
    );
    log.push(
        "- 2026-08-30T17:10Z  done     trevor                done (in main 3f2a19c7 via \
         human-confirm)",
    );
    write_shipped_ticket(&repo, "t-9c41", "done", &log);

    let j = doctor(&repo);
    assert!(j.of("unproven_close").is_empty(), "{:#?}", j.findings);
    assert_eq!(repo.ks(["doctor"]).code, 0);
}

/// `dropped` is terminal too, and deliberately unguarded here: a drop is an act, not a
/// merge. `--why` is the whole of its evidence, and demanding a commit for one would fire
/// on every legitimately abandoned ticket.
#[test]
fn a_dropped_ticket_is_never_asked_to_prove_a_merge() {
    let repo = TestRepo::new();
    let mut log: Vec<&str> = LEGAL.to_vec();
    log.push("- 2026-08-30T17:10Z  dropped  trevor                drop (superseded)");
    write_shipped_ticket(&repo, "t-9c41", "dropped", &log);

    let j = doctor(&repo);
    assert!(j.of("unproven_close").is_empty(), "{:#?}", j.findings);
    assert_eq!(repo.ks(["doctor"]).code, 0);
}

/// A ticket that never had a branch, a commit or a PR leaves git nothing to be asked
/// about, so there is no corroboration to demand and the check says nothing. This is the
/// honest boundary of the detection, and DESIGN.md's invariant 10 now names it: a
/// hand-edit that also strips `branch:` and `head:` is past what a corroboration check can
/// see.
#[test]
fn a_close_with_no_branch_to_corroborate_against_is_outside_what_this_can_see() {
    let repo = TestRepo::new();
    let mut log: Vec<&str> = LEGAL.to_vec();
    log.push("- 2026-08-30T17:10Z  done     trevor                done");
    write_ticket(&repo, "t-9c41", "done", &log); // no branch:, no head:, no pr:

    let j = doctor(&repo);
    assert!(
        j.of("unproven_close").is_empty(),
        "nothing for git to place: {:#?}",
        j.findings
    );
    assert_eq!(repo.ks(["doctor"]).code, 0);
}

/// A broken trail is `log_trail`'s finding. The forgery this check exists for replays
/// PERFECTLY, so the two never speak about the same ticket and CI names one break at a
/// time.
#[test]
fn a_close_whose_trail_is_already_broken_is_left_to_the_replay_check() {
    let repo = TestRepo::new();
    write_shipped_ticket(&repo, "t-9c41", "done", LEGAL); // the log only ever reached `review`

    let j = doctor(&repo);
    assert_eq!(j.of("log_trail").len(), 1, "{:#?}", j.findings);
    assert!(
        j.of("unproven_close").is_empty(),
        "one break, one finding: {:#?}",
        j.findings
    );
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
