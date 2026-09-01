//! `tests/lifecycle.rs`
//!
//! Proves: **the walking skeleton** — `new → ready → start --worktree → ship --pr →
//! a REAL squash merge → scan → done`, run from **inside a linked worktree**, asserting
//! every write landed in the primary `.kanspec/` and the `## Log` replays clean.
//!
//! This is the product's proof of life, so it is deliberately not a happy path. It also
//! proves the two refusals the whole design is built around:
//!
//! * `done` cannot be talked into closing an unmerged ticket — not by asking twice, and not
//!   by reaching for `--no-code` on a ticket that was shipped for review;
//! * the leftover-triage gate has **no fourth option**, and in non-interactive mode
//!   "nothing left" must be said out loud rather than inferred from an omitted flag.
//!
//! Git is never mocked here. The merge is a real `git merge --squash` into a real bare
//! origin, which is why the ladder answers by TRAILER rather than by ancestry — a squash
//! rewrites the commits, and rung 1 is genuinely negative.
//!
//! Owner: **S5**.

mod common;

use std::path::{Path, PathBuf};

use common::{git_at, write_at, TestRepo};
use serde_json::Value;

/// `kanspec … --json`, run from an arbitrary worktree. `TestRepo::json` always runs at the
/// primary root, and the point of this file is to run everywhere else.
#[track_caller]
fn json_in(repo: &TestRepo, cwd: &Path, args: &[&str]) -> Value {
    let mut all: Vec<&str> = args.to_vec();
    all.push("--json");
    let r = repo.ks_in(cwd, &all);
    assert_eq!(
        r.code,
        0,
        "`kanspec {}` exited {}\nstdout:\n{}\nstderr:\n{}",
        all.join(" "),
        r.code,
        r.stdout,
        r.stderr
    );
    serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
        panic!(
            "`kanspec {}` did not print JSON ({e}):\n{}",
            all.join(" "),
            r.stdout
        )
    })
}

/// The refusal envelope: `{"ok":false,"error":{…}}` on STDOUT, so an agent reading only
/// stdout still gets the reason and its fix list.
#[track_caller]
fn refusal(repo: &TestRepo, cwd: &Path, args: &[&str]) -> Value {
    let mut all: Vec<&str> = args.to_vec();
    all.push("--json");
    let r = repo.ks_in(cwd, &all);
    assert_ne!(
        r.code,
        0,
        "`kanspec {}` was supposed to refuse:\n{}",
        all.join(" "),
        r.stdout
    );
    let v: Value = serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
        panic!(
            "a refusal must still be JSON under --json ({e}):\n{}\n{}",
            r.stdout, r.stderr
        )
    });
    assert_eq!(v["ok"], Value::Bool(false));
    assert!(
        !v["error"]["fix"].as_array().expect("a fix list").is_empty(),
        "invariant 9: every refusal names its one-command fix — {v}"
    );
    v["error"].clone()
}

fn str_at<'v>(v: &'v Value, key: &str) -> &'v str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is not a string in {v}"))
}

/// The knowledge the claim transcript reads: one spec with rules and `code:` globs, one
/// accepted decision and one active quirk scoped to the same ground.
fn seed_knowledge(repo: &TestRepo) {
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login (JWT 24h), lockout after 5 failures\ncode: [src/auth/**]\n---\n\
         # auth\n\n## Rules\n- [auth.jwt] Login issues a JWT valid 24h. {p-02cc}\n",
    );
    repo.write(
        ".kanspec/decisions/D-8c1a.md",
        "---\nid: D-8c1a\ntitle: Rate-limit state lives in Redis only\nstatus: accepted\n\
         date: 2026-08-20\nsource: null\nscope: [src/auth/**]\nsupersedes: null\n\
         superseded_by: null\n---\n## Decision\nSliding window in Redis.\n",
    );
    repo.write(
        ".kanspec/quirks/q-11ba.md",
        "---\nid: q-11ba\ntitle: Sessions rotate on every redeploy\npaths: [src/auth/**]\n\
         severity: landmine\nstatus: active\nsource: null\nfixed_by: null\n---\nBody.\n",
    );
    // A decision and a quirk on ground this ticket does NOT touch: the context line must
    // path-scope, or it is a list rather than a briefing.
    repo.write(
        ".kanspec/decisions/D-2c77.md",
        "---\nid: D-2c77\ntitle: Money amounts are integer cents\nstatus: accepted\n\
         date: 2026-06-11\nsource: null\nscope: [src/billing/**]\nsupersedes: null\n\
         superseded_by: null\n---\n## Decision\nCents.\n",
    );
    repo.git(&["add", "-A", "--", ".kanspec"]);
    repo.git(&["commit", "--quiet", "-m", "knowledge"]);
    repo.push("main");
}

/// git's own body for `merge --squash`: every original message, indented — which is exactly
/// why rung 3 must not anchor its pattern to the start of a line.
fn squash_msg(repo: &TestRepo) -> Option<String> {
    let p = repo.git(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "SQUASH_MSG",
    ]);
    std::fs::read_to_string(p.trim()).ok()
}

/// One commit on a ticket branch, carrying the trailer the `prepare-commit-msg` hook writes
/// in a real repo — the squash-surviving merge-detection signal.
fn commit_in(wt: &Path, id: &str, subject: &str) {
    git_at(wt, &["add", "-A", "--", "src", ".kanspec/specs"]);
    git_at(
        wt,
        &[
            "commit",
            "--quiet",
            "-m",
            &format!("{id}: {subject}\n\nKanspec: {id}\n"),
        ],
    );
}

#[test]
fn the_walking_skeleton_runs_end_to_end_from_a_linked_worktree() {
    let repo = TestRepo::new();
    seed_knowledge(&repo);

    // The agent never stands in the primary worktree. Every write below still has to land
    // in the primary `.kanspec/` — that is worktree unification, and it is the reason
    // `start` is atomic across agents on one machine.
    let agent = repo.worktree("agent");

    // ── new ──────────────────────────────────────────────────────────────────
    let created = json_in(
        &repo,
        &agent,
        &["new", "Rate-limit login endpoint", "--spec", "auth"],
    );
    let id = str_at(&created, "id").to_string();
    assert_eq!(created["state"], "todo");
    assert!(created["discovered_in"].is_null(), "no claim, no link");

    let ticket_rel = format!(".kanspec/tickets/{id}.md");
    assert!(
        repo.exists(&ticket_rel),
        "the write landed in the PRIMARY .kanspec/"
    );
    assert!(
        !agent.join(&ticket_rel).exists(),
        "…and nowhere else: a linked worktree has no store of its own"
    );

    // ── ready ────────────────────────────────────────────────────────────────
    let ready = json_in(&repo, &agent, &["ready"]);
    let queued: Vec<&str> = ready["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| str_at(r, "id"))
        .collect();
    assert_eq!(queued, [id.as_str()], "todo with no open deps is claimable");

    // ── start --worktree ─────────────────────────────────────────────────────
    let start = repo.ks_in(&agent, ["start", &id, "--worktree"]).ok();
    let claim = json_in(&repo, &agent, &["show", &id]);
    assert_eq!(claim["state"], "doing");

    // DESIGN.md's claim transcript, INCLUDING the context line — path-scoped, and read
    // from the real knowledge entities rather than stubbed.
    assert!(start.stdout.contains("claimed"), "{}", start.stdout);
    assert!(
        start
            .stdout
            .contains(&format!("branch   ks/{id}-rate-limit-login-endpoint")),
        "{}",
        start.stdout
    );
    assert!(
        start.stdout.contains(
            "context  spec auth (1 rule) · 1 decision in scope (D-8c1a) · 1 quirk matches paths (q-11ba)"
        ),
        "the claim transcript's context line must be real, not a stub:\n{}",
        start.stdout
    );
    assert!(
        !start.stdout.contains("D-2c77"),
        "the context line is path-scoped, or it is a list rather than a briefing:\n{}",
        start.stdout
    );

    // The FULL transcript, pinned. Every prior plan stubbed the context line and no plan
    // ever removed the stub, so DESIGN.md's claim block was never actually delivered.
    let transcript: Vec<&str> = start.stdout.lines().collect();
    assert_eq!(
        transcript,
        vec![
            format!(
                "  claimed  {id}  Rate-limit login endpoint          (logged: doing · trevor)"
            ),
            format!(
                "  branch   ks/{id}-rate-limit-login-endpoint    worktree ../kanspec-wt/{id}"
            ),
            "  context  spec auth (1 rule) · 1 decision in scope (D-8c1a) · 1 quirk matches paths (q-11ba)".to_string(),
            format!("  board    http://127.0.0.1:5757/t/{id}"),
            format!("  → cd ../kanspec-wt/{id}"),
            format!("  → kanspec ship {id} --pr <n>"),
        ],
        "the claim transcript drifted from DESIGN.md"
    );

    let branch = format!("ks/{id}-rate-limit-login-endpoint");
    let ticket_wt: PathBuf = repo.root.join("..").join("kanspec-wt").join(&id);
    assert!(ticket_wt.is_dir(), "start --worktree creates the worktree");
    // D-21: without `--no-track` a later `git push` from the ticket branch targets main.
    let upstream = repo.git_try(&["config", "--get", &format!("branch.{branch}.merge")]);
    assert_ne!(
        upstream.0, 0,
        "the ticket branch must not track main (D-21)"
    );
    // The per-branch dispatch key the commit hooks read back (D-5).
    assert_eq!(
        repo.git(&[
            "config",
            "--get",
            &format!("branch.{branch}.kanspec-ticket")
        ])
        .trim(),
        id
    );

    // An agent fills in the ticket's steps: the BODY is prose an agent may edit, and only
    // the frontmatter is verb-only.
    let body = repo.read(&ticket_rel).replace(
        "## Steps\n",
        "## Steps\n- [x] lockout counter in Redis, sliding window\n\
         - [ ] test: two concurrent requests, same key\n",
    );
    repo.write(&ticket_rel, &body);

    // ── the rabbit-hole valve, mid-ticket ────────────────────────────────────
    //
    // One command from inside the ticket's own worktree, and the provenance is already
    // stamped — no flag, no ceremony, and it does not expand the current ticket.
    let aside = json_in(
        &repo,
        &ticket_wt,
        &["new", "Session middleware leaks a listener"],
    );
    let aside_id = str_at(&aside, "id").to_string();
    assert_eq!(
        aside["discovered_in"], id,
        "a discovery made while claiming {id} links back to it automatically"
    );

    // ── the work ─────────────────────────────────────────────────────────────
    write_at(
        &ticket_wt,
        "src/auth/lockout.ts",
        "export const lockout = () => {};\n",
    );
    commit_in(&ticket_wt, &id, "lockout counter");
    // The spec moves WITH the code, on the same branch, reviewed in the same diff — the
    // knowledge checkpoint's happy path.
    let spec = std::fs::read_to_string(ticket_wt.join(".kanspec/specs/auth.md")).unwrap();
    write_at(
        &ticket_wt,
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.lockout] 5 failed logins within 10m locks the account 15m.\n"),
    );
    write_at(
        &ticket_wt,
        "src/auth/window.ts",
        "export const window = () => {};\n",
    );
    // TWO commits, so the squash below is genuinely multi-commit and `git cherry` cannot
    // rescue it: only the trailer survives, which is the shape rung 3 exists for.
    commit_in(&ticket_wt, &id, "sliding window + spec rule");
    git_at(&ticket_wt, &["push", "--quiet", "origin", &branch]);

    // ── ship ─────────────────────────────────────────────────────────────────
    let branch_head = git_at(&ticket_wt, &["rev-parse", "HEAD"])
        .trim()
        .to_string();
    let shipped = json_in(&repo, &ticket_wt, &["ship", &id, "--pr", "145"]);
    assert_eq!(shipped["state"], "review");
    assert_eq!(
        str_at(&shipped, "head"),
        branch_head,
        "the head SHA is read FROM GIT, never typed by an agent"
    );
    assert_eq!(shipped["pr"], 145);

    // ── a REAL squash merge into the real bare origin ────────────────────────
    repo.git(&["merge", "--quiet", "--squash", &branch]);
    let msg = squash_msg(&repo).expect("git wrote a SQUASH_MSG");
    assert!(
        msg.contains(&format!("Kanspec: {id}")),
        "git indents the original trailers into the squash body:\n{msg}"
    );
    repo.git(&["commit", "--quiet", "-m", &msg]);
    repo.push("main");
    assert_ne!(
        repo.git(&["rev-parse", "origin/main"]).trim(),
        branch_head,
        "a squash rewrites the commits — this is not a fast-forward"
    );

    // ── scan ─────────────────────────────────────────────────────────────────
    let scan = json_in(&repo, &ticket_wt, &["scan"]);
    let landed: Vec<&str> = scan["landed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| str_at(r, "id"))
        .collect();
    assert!(landed.contains(&id.as_str()), "scan says it landed: {scan}");
    assert!(
        repo.exists(".kanspec/cache/gitstate.json"),
        "derived git facts live in the gitignored cache and nowhere else"
    );
    assert!(
        !ticket_wt.join(".kanspec/cache/gitstate.json").exists(),
        "…in the PRIMARY's cache"
    );

    // ── done: the gate, before the close ─────────────────────────────────────
    //
    // An omission is not a claim. Saying nothing about leftovers is refused by name.
    let e = refusal(&repo, &ticket_wt, &["done", &id]);
    assert_eq!(e["code"], "followups_unanswered", "{e}");

    // NO FOURTH OPTION: the unchecked step must be spawned, dropped, or marked done, and
    // `--no-followups` is a claim about followups, not an escape from disposition.
    let e = refusal(
        &repo,
        &ticket_wt,
        &["done", &id, "--no-followups", "--no-quirks"],
    );
    assert_eq!(e["code"], "steps_undispositioned", "{e}");
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("test: two concurrent requests, same key"),
        "the refusal names the step it is about: {e}"
    );

    // ── done ─────────────────────────────────────────────────────────────────
    let done = json_in(
        &repo,
        &ticket_wt,
        &[
            "done",
            &id,
            "--spawn",
            "test concurrent same-key requests",
            "--no-quirks",
        ],
    );
    assert_eq!(done["state"], "done");
    assert_eq!(
        done["method"], "trailer",
        "a multi-commit squash is invisible to ancestry and to patch-id: {done}"
    );
    assert_eq!(
        done["spec_check"]["spec_check"], "edited_on_branch",
        "the branch touched src/auth/** AND moved the spec with it: {done}"
    );
    let spawned = done["spawned"].as_array().unwrap();
    assert_eq!(spawned.len(), 1, "{done}");
    assert_eq!(
        str_at(&spawned[0], "title"),
        "test concurrent same-key requests"
    );
    assert_eq!(
        spawned[0]["from_step"], 2,
        "the followup names the step it came out of: {done}"
    );
    let followup = str_at(&spawned[0], "id").to_string();

    // DESIGN.md's mechanism for keeping rabbit-hole captures visible.
    let parked: Vec<&str> = done["discovered"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        parked,
        [aside_id.as_str()],
        "the close-out summary reports what was captured along the way: {done}"
    );

    // Every write landed in the PRIMARY, whichever worktree ran the verb.
    for rel in [
        ticket_rel.clone(),
        format!(".kanspec/tickets/{followup}.md"),
        format!(".kanspec/tickets/{aside_id}.md"),
    ] {
        assert!(repo.exists(&rel), "{rel} is missing from the primary store");
        assert!(
            !ticket_wt.join(&rel).exists(),
            "{rel} must not exist in the linked worktree"
        );
    }

    // The followup carries its parent's story; the discovery deliberately does not.
    let f = json_in(&repo, &ticket_wt, &["show", &followup]);
    let f_fm = repo.read(&format!(".kanspec/tickets/{followup}.md"));
    assert!(f_fm.contains(&format!("followup_of: {id}")), "{f_fm}");
    assert_eq!(f["spec"], "auth", "a followup inherits the parent's spec");
    let a_fm = repo.read(&format!(".kanspec/tickets/{aside_id}.md"));
    assert!(a_fm.contains(&format!("discovered_in: {id}")), "{a_fm}");
    assert!(a_fm.contains("followup_of: null"), "{a_fm}");

    // ── the log replays clean ────────────────────────────────────────────────
    let log = json_in(&repo, &ticket_wt, &["log", &id]);
    assert!(
        log["violation"].is_null(),
        "the ## Log must replay after a full lifecycle: {log}"
    );
    assert_eq!(log["replays_to"], "done");
    let verbs: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| str_at(e, "verb"))
        .collect();
    assert_eq!(verbs, ["new", "start", "ship", "done"]);

    // …and `doctor` agrees, which is the same proof one layer out.
    let d = repo.ks(["doctor"]);
    assert_eq!(
        d.code, 0,
        "doctor found something after a clean lifecycle:\n{}\n{}",
        d.stdout, d.stderr
    );
}

/// The slice's other half: `done` cannot be talked into closing an unmerged ticket. Not by
/// asking again, and not by reaching for the chore/docs escape on work that has a branch.
#[test]
fn done_cannot_be_talked_into_closing_an_unmerged_ticket() {
    let repo = TestRepo::new();
    seed_knowledge(&repo);
    let agent = repo.worktree("agent");

    let created = json_in(&repo, &agent, &["new", "Never lands", "--spec", "auth"]);
    let id = str_at(&created, "id").to_string();
    repo.ks_in(&agent, ["start", &id, "--worktree"]).ok();
    let wt = repo.root.join("..").join("kanspec-wt").join(&id);

    // A branch with no commits has nothing to review — and shipping it would record MAIN'S
    // OWN SHA as `head:`, which ancestry then answers MERGED. The verified false positive
    // is refused at the door.
    let e = refusal(&repo, &wt, &["ship", &id]);
    assert_eq!(e["code"], "nothing_to_ship", "{e}");

    write_at(&wt, "src/auth/never.ts", "export const never = 1;\n");
    commit_in(&wt, &id, "work that never lands");
    git_at(
        &wt,
        &["push", "--quiet", "origin", &format!("ks/{id}-never-lands")],
    );
    json_in(&repo, &wt, &["ship", &id, "--pr", "999"]);

    // 1. The gate refuses, and the refusal carries the whole ladder trace — the
    //    `--explain`-grade output that makes it arguable rather than arbitrary.
    let e = refusal(&repo, &wt, &["done", &id, "--no-followups", "--no-quirks"]);
    assert_eq!(e["code"], "not_landed", "{e}");
    let trace = e["detail"]["trace"].as_array().expect("a ladder trace");
    assert!(!trace.is_empty(), "{e}");
    let methods: Vec<&str> = trace.iter().map(|t| str_at(t, "method")).collect();
    assert!(
        methods.contains(&"ancestry"),
        "rung 1 must have actually run: {e}"
    );

    // 2. Asking twice does not help.
    let again = refusal(&repo, &wt, &["done", &id, "--no-followups", "--no-quirks"]);
    assert_eq!(again["code"], "not_landed");

    // 3. And the chore/docs escape provably cannot launder a ticket that was shipped for
    //    review: `--no-code` is legal from `doing`, and only from `doing`.
    let e = refusal(
        &repo,
        &wt,
        &[
            "done",
            &id,
            "--no-code",
            "--why",
            "it is basically docs",
            "--no-followups",
            "--no-quirks",
        ],
    );
    assert_eq!(e["code"], "no_code_from_review", "{e}");

    // The ticket did not move.
    let show = json_in(&repo, &wt, &["show", &id]);
    assert_eq!(show["state"], "review");
}

/// The recorded escape, on the path it is actually for: a chore with no code, closed from
/// `doing`, with a reason that lands in the file rather than in the terminal.
#[test]
fn the_no_code_escape_is_recorded_on_the_ticket_not_just_claimed() {
    let repo = TestRepo::new();
    let agent = repo.worktree("agent");
    let created = json_in(&repo, &agent, &["new", "Tidy the changelog"]);
    let id = str_at(&created, "id").to_string();
    repo.ks_in(&agent, ["start", &id]).ok();

    let done = json_in(
        &repo,
        &agent,
        &[
            "done",
            &id,
            "--no-code",
            "--why",
            "changelog only, no code changed",
            "--no-followups",
            "--no-quirks",
        ],
    );
    assert_eq!(done["state"], "done");
    assert!(done["sha"].is_null(), "there is no SHA to claim: {done}");

    let file = repo.read(&format!(".kanspec/tickets/{id}.md"));
    assert!(
        file.contains("no-code waiver by trevor"),
        "the waiver is durable, signed, and in the repo:\n{file}"
    );
    assert!(file.contains("changelog only, no code changed"), "{file}");

    // …and it is PROSE under `## Log`: a second parseable entry for one act would break the
    // very proof the log exists for.
    let log = json_in(&repo, &agent, &["log", &id]);
    assert!(log["violation"].is_null(), "{log}");
    let verbs: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| str_at(e, "verb"))
        .collect();
    assert_eq!(verbs, ["new", "start", "done"]);
}

/// `park` and `drop` are the other two ways work leaves the board, and both are recorded.
#[test]
fn parking_releases_the_claim_and_dropping_unblocks_what_waited_on_it() {
    let repo = TestRepo::new();
    let agent = repo.worktree("agent");

    let a = json_in(&repo, &agent, &["new", "Lockout table"]);
    let a_id = str_at(&a, "id").to_string();
    let b = json_in(&repo, &agent, &["new", "Alert wiring", "--dep", &a_id]);
    let b_id = str_at(&b, "id").to_string();

    let ready = json_in(&repo, &agent, &["ready"]);
    let queued: Vec<&str> = ready["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| str_at(r, "id"))
        .collect();
    assert_eq!(queued, [a_id.as_str()], "the blocked one is not claimable");
    let blocked: Vec<&str> = ready["blocked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| str_at(r, "id"))
        .collect();
    assert_eq!(blocked, [b_id.as_str()], "…and it says what holds it");

    repo.ks_in(&agent, ["start", &a_id]).ok();
    let parked = json_in(
        &repo,
        &agent,
        &["park", &a_id, "--why", "waiting on the redis cluster"],
    );
    assert_eq!(parked["state"], "todo");
    let show = json_in(&repo, &agent, &["show", &a_id]);
    assert!(show["claimed_by"].is_null(), "park unclaims: {show}");
    assert_eq!(
        show["branch"],
        format!("ks/{a_id}-lockout-table"),
        "…but keeps the branch, so a later start picks the work back up"
    );

    let dropped = json_in(
        &repo,
        &agent,
        &["drop", &a_id, "--why", "the table shipped elsewhere"],
    );
    assert_eq!(dropped["state"], "dropped");
    let unblocked: Vec<&str> = dropped["unblocked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        unblocked,
        [b_id.as_str()],
        "a dropped dep counts as satisfied (D-17): {dropped}"
    );

    let log = json_in(&repo, &agent, &["log", &a_id]);
    assert!(log["violation"].is_null(), "{log}");
    let notes: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["note"].as_str())
        .collect();
    assert!(
        notes.contains(&"waiting on the redis cluster"),
        "the reason is in the file, not in someone's memory: {log}"
    );
    assert!(notes.contains(&"the table shipped elsewhere"), "{log}");
}

/// `where` is the question an agent asks after a compaction: which ticket owns this branch,
/// and where do writes actually land?
#[test]
fn where_answers_from_inside_a_linked_worktree() {
    let repo = TestRepo::new();
    let agent = repo.worktree("agent");
    let created = json_in(&repo, &agent, &["new", "Rate-limit login"]);
    let id = str_at(&created, "id").to_string();
    // `start` under `--json`: the claim transcript's every field, typed, for the agent that
    // is about to act on it.
    let claim = json_in(&repo, &agent, &["start", &id, "--worktree"]);
    assert_eq!(claim["state"], "doing");
    assert_eq!(claim["branch"], format!("ks/{id}-rate-limit-login"));
    assert_eq!(claim["worktree"], format!("../kanspec-wt/{id}"));
    assert_eq!(claim["claimed_by"], "trevor");
    assert_eq!(claim["url"], format!("http://127.0.0.1:5757/t/{id}"));
    assert_eq!(
        claim["checked_out"], false,
        "a linked worktree never moves the primary's HEAD"
    );
    let wt = repo.root.join("..").join("kanspec-wt").join(&id);

    let here = json_in(&repo, &wt, &["where"]);
    assert_eq!(here["ticket"], id);
    assert_eq!(here["branch"], format!("ks/{id}-rate-limit-login"));
    assert_eq!(here["linked_worktree"], true);
    assert!(
        str_at(&here, "primary_kanspec").ends_with(".kanspec"),
        "worktree unification: the store is the primary's: {here}"
    );

    // A worktree that owns no ticket says so rather than guessing.
    let elsewhere = json_in(&repo, &agent, &["where"]);
    assert!(elsewhere["ticket"].is_null(), "{elsewhere}");

    // And `--branch` answers for a branch you are not standing on.
    let named = json_in(
        &repo,
        &agent,
        &["where", "--branch", &format!("ks/{id}-rate-limit-login")],
    );
    assert_eq!(named["ticket"], id);
}

/// `ls` is the working list, and its filters have to mean something.
#[test]
fn ls_filters_by_spec_by_claim_and_by_merge_state() {
    let repo = TestRepo::new();
    seed_knowledge(&repo);
    let agent = repo.worktree("agent");

    let a = json_in(&repo, &agent, &["new", "Lockout", "--spec", "auth"]);
    let a_id = str_at(&a, "id").to_string();
    let b = json_in(&repo, &agent, &["new", "Unspecced chore"]);
    let b_id = str_at(&b, "id").to_string();
    repo.ks_in(&agent, ["start", &a_id]).ok();

    let all = json_in(&repo, &agent, &["ls"]);
    let ids: Vec<&str> = all["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| str_at(r, "id"))
        .collect();
    assert_eq!(ids.len(), 2, "{all}");

    let by_spec = json_in(&repo, &agent, &["ls", "--spec", "auth"]);
    let ids: Vec<&str> = by_spec["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| str_at(r, "id"))
        .collect();
    assert_eq!(ids, [a_id.as_str()]);

    let mine = json_in(&repo, &agent, &["ls", "--mine"]);
    let ids: Vec<&str> = mine["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| str_at(r, "id"))
        .collect();
    assert_eq!(ids, [a_id.as_str()], "claimed by this actor only");

    // Nothing has landed, so everything is unmerged.
    let unmerged = json_in(&repo, &agent, &["ls", "--unmerged"]);
    assert_eq!(unmerged["rows"].as_array().unwrap().len(), 2);

    // A dropped ticket leaves the working list and comes back with --all.
    json_in(&repo, &agent, &["drop", &b_id, "--why", "not needed"]);
    let open = json_in(&repo, &agent, &["ls"]);
    assert_eq!(open["rows"].as_array().unwrap().len(), 1, "{open}");
    let every = json_in(&repo, &agent, &["ls", "--all"]);
    assert_eq!(every["rows"].as_array().unwrap().len(), 2, "{every}");
}

/// new -> start --worktree -> two real commits (spec moved with the code) -> push -> ship ->
/// a REAL squash merge into the real bare origin.
///
/// Shared by the skeleton and by the close-out transcript pin, so the two cannot exercise
/// two different merge shapes and quietly disagree about which rung answered.
fn land_a_ticket(repo: &TestRepo, from: &Path, args: &[&str]) -> (String, PathBuf) {
    let created = json_in(repo, from, args);
    let id = str_at(&created, "id").to_string();
    repo.ks_in(from, ["start", &id, "--worktree"]).ok();
    let wt = repo.root.join("..").join("kanspec-wt").join(&id);
    let branch = json_in(repo, &wt, &["where"])["branch"]
        .as_str()
        .expect("start recorded a branch")
        .to_string();

    write_at(&wt, &format!("src/auth/{id}.ts"), "export const a = 1;\n");
    commit_in(&wt, &id, "the work");
    let spec = std::fs::read_to_string(wt.join(".kanspec/specs/auth.md")).unwrap();
    write_at(
        &wt,
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.{id}] the rule this ticket shipped.\n"),
    );
    write_at(&wt, &format!("src/auth/{id}-2.ts"), "export const b = 2;\n");
    // TWO commits, so the squash below is genuinely multi-commit and `git cherry` cannot
    // rescue it: only the trailer survives, which is the shape rung 3 exists for.
    commit_in(&wt, &id, "the spec rule");
    git_at(&wt, &["push", "--quiet", "origin", &branch]);
    json_in(repo, &wt, &["ship", &id, "--pr", "145"]);

    repo.git(&["merge", "--quiet", "--squash", &branch]);
    let msg = squash_msg(repo).expect("git wrote a SQUASH_MSG");
    repo.git(&["commit", "--quiet", "-m", &msg]);
    repo.push("main");
    (id, wt)
}

/// DESIGN.md's close-out transcript, pinned — and the settling flag, which is the only part
/// of the `done` ritual whose subject is a PROPOSAL rather than a ticket.
#[test]
fn the_close_out_transcript_reads_like_design_md_and_flags_the_settling_proposal() {
    let repo = TestRepo::new();
    seed_knowledge(&repo);
    repo.write(
        ".kanspec/proposals/p-7de2-login-rate-limiting/proposal.md",
        "---\nid: p-7de2\ntitle: Login rate limiting\nstatus: approved\nspecs: [auth]\n\
         approved: null\nledger: []\ncreated: 2026-08-30\n---\n## Why\nCredential stuffing.\n\
         \n## Tickets\n- [t1] Rate-limit login endpoint\n",
    );
    repo.git(&["add", "-A", "--", ".kanspec"]);
    repo.git(&["commit", "--quiet", "-m", "proposal"]);

    let agent = repo.worktree("agent");
    let (id, wt) = land_a_ticket(
        &repo,
        &agent,
        &[
            "new",
            "Rate-limit login endpoint",
            "--spec",
            "auth",
            "--proposal",
            "p-7de2",
        ],
    );
    let body = repo.read(&format!(".kanspec/tickets/{id}.md")).replace(
        "## Steps\n",
        "## Steps\n- [x] lockout counter\n- [ ] test: two concurrent requests, same key\n",
    );
    repo.write(&format!(".kanspec/tickets/{id}.md"), &body);

    // HUMAN mode: this is the face of the product, and the one surface a transcript in
    // DESIGN.md is a promise about.
    let out = repo
        .ks_in(
            &wt,
            [
                "done",
                &id,
                "--spawn",
                "test concurrent same-key requests",
                "--no-quirks",
            ],
        )
        .ok()
        .stdout;
    let lines: Vec<&str> = out.lines().collect();

    assert!(
        lines[0].starts_with("\u{2713} merged verified: ")
            && lines[0]
                .contains("reachable from origin/main (method: trailer #145 \u{b7} checked "),
        "line 1 is DESIGN.md's merge line:\n{out}"
    );
    assert_eq!(
        lines[1], "Leftover triage \u{2014} 1 unchecked step:",
        "{out}"
    );
    assert!(
        lines[2].starts_with("  \u{2713} spawned t-")
            && lines[2].ends_with(&format!(
                "\"test concurrent same-key requests\" (followup_of {id})"
            )),
        "line 3 names the followup AND its title:\n{out}"
    );
    assert_eq!(
        lines[3],
        "Knowledge check \u{2014} branch touched src/auth/** (spec: auth): spec edited on this \
         branch \u{2713}",
        "{out}"
    );
    // NOT settling yet, and that is the point: the followup this close-out just spawned
    // inherits the proposal, so the proposal's story is demonstrably still open. A
    // "settling" flag that fired here would be prompting a close nobody can honour.
    assert!(
        lines[4].contains(&id) && !lines[4].contains("settling"),
        "a live followup keeps its proposal open:\n{out}"
    );

    // Close the followup, and the proposal's last live ticket is gone.
    let followup = json_in(&repo, &wt, &["ls"])["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| str_at(r, "id").to_string())
        .find(|f| *f != id)
        .expect("the followup is on the board");
    repo.ks_in(&wt, ["start", &followup]).ok();
    let out = repo
        .ks_in(
            &wt,
            [
                "done",
                &followup,
                "--no-code",
                "--why",
                "covered by the integration suite",
                "--no-followups",
                "--no-quirks",
            ],
        )
        .ok()
        .stdout;
    let last = out.lines().last().unwrap_or_default().to_string();
    assert!(
        last.contains(&followup)
            && last.contains("done \u{b7} p-7de2 is settling (last ticket landed)")
            && last.contains("\u{2192} kanspec close p-7de2"),
        "the settling proposal is prompted, never remembered:\n{out}"
    );

    // ...and the same fact, typed, for the agent surface.
    for t in [&id, &followup] {
        let log = json_in(&repo, &wt, &["log", t]);
        assert_eq!(log["replays_to"], "done", "{log}");
        assert!(log["violation"].is_null(), "{log}");
    }
}

/// ROUND-C REGRESSION (found by the integrator's dogfood run, fixed at integration).
///
/// `start` moves the primary worktree onto the ticket branch when — and only when — no
/// separate worktree was asked for and the user's own source tree carries no uncommitted
/// work. As shipped, that guard ran `git status` AFTER `transact`, so the only dirty file
/// was the ticket `start` had itself just written: the branch was never checked out, on any
/// repo, ever, and `checked_out` was unreachable. An agent following the CLAUDE.md snippet
/// therefore kept committing to main, which is the exact failure the feature exists to stop.
///
/// Both directions are pinned, because a guard that always says yes is as wrong as one that
/// always says no.
#[test]
fn start_moves_the_primary_onto_the_branch_only_when_the_source_tree_is_clean() {
    let repo = TestRepo::new();
    let root = repo.root.clone();

    // ── clean source tree: the claim checks the branch out ───────────────────
    let a = json_in(&repo, &root, &["new", "first claim"]);

    // COMMIT THE TICKET BEFORE CLAIMING IT. This is the ordinary rhythm of a real repo —
    // DESIGN.md commits `.kanspec/`, and `sync = "batch"` means a human or a hook sweeps it
    // up between verbs — and it is exactly what the original guard could not survive: once
    // the ticket file is TRACKED, the transition `start` writes into it makes
    // `git status -uno` non-empty, so a guard running after `transact` reads the verb's own
    // write as the user's uncommitted work and refuses every time. While the file is still
    // untracked (a fresh `TestRepo` that never commits) `-uno` hides it and the bug cannot
    // be seen — which is how it shipped green.
    // Pushed as well as committed: `start` forks the ticket branch from `origin/main`, so
    // a local `main` that is ahead of the remote genuinely cannot be switched away from
    // while the tracker carries uncommitted edits — git itself refuses, and `start`
    // correctly reports `checked_out: false` and names `git switch`. That is a THIRD,
    // legitimate no-switch case; this test is about the two the guard decides.
    repo.commit("kanspec: sync the tracker, as a real repo does");
    repo.push("main");
    let a_id = a["id"].as_str().expect("an id").to_string();
    let started = json_in(&repo, &root, &["start", &a_id]);
    let a_branch = started["branch"].as_str().expect("a branch").to_string();

    assert_eq!(
        started["checked_out"],
        Value::Bool(true),
        "a clean source tree must be moved onto the ticket branch: {started}"
    );
    assert_eq!(
        repo.git(&["branch", "--show-current"]).trim(),
        a_branch,
        "the primary worktree is standing on the ticket branch"
    );
    // Having actually switched, the report must not also tell the user to switch.
    let next = started["next"].as_array().expect("a next list");
    assert!(
        !next
            .iter()
            .any(|n| n.as_str().unwrap_or("").starts_with("git switch")),
        "nothing left to switch — {next:?}"
    );

    // The tracker's own pending edits are the normal resting state under `sync = "batch"`
    // and must NOT count as dirt; the switch above already proved that, since `start`
    // itself wrote a ticket file before this point.

    // ── real uncommitted source work: nothing moves ──────────────────────────
    repo.git(&["switch", "main"]);
    repo.write("src/half-written.rs", "fn t() { /* mid-thought */ }\n");
    repo.git(&["add", "src/half-written.rs"]);

    let b = json_in(&repo, &root, &["new", "second claim"]);
    let b_id = b["id"].as_str().expect("an id").to_string();
    let refused = json_in(&repo, &root, &["start", &b_id]);

    assert_eq!(
        refused["checked_out"],
        Value::Bool(false),
        "uncommitted work in the user's OWN source must keep HEAD where it is: {refused}"
    );
    assert_eq!(
        repo.git(&["branch", "--show-current"]).trim(),
        "main",
        "the primary must not be moved out from under uncommitted work"
    );
    let next = refused["next"].as_array().expect("a next list");
    assert!(
        next.iter()
            .any(|n| n.as_str().unwrap_or("").starts_with("git switch")),
        "when it does not switch, it must say how — {next:?}"
    );
}

/// ROUND-D REGRESSION — **the `git add -A` wedge**, hit unprompted by an adversarial
/// auditor on a first scripted run.
///
/// `start` without `--worktree` leaves the PRIMARY worktree — the tree that owns
/// `.kanspec/` — standing on the ticket branch. Under `sync = "batch"` (the default, D-13)
/// the tracker is *meant* to be dirty there, so the near-universal
/// `git add -A && git commit` sweeps `.kanspec/tickets/t-xxxx.md` onto the feature branch.
/// The next verb re-dirties that file, and from then on `git checkout main` refuses:
///
/// ```text
/// error: Your local changes to the following files would be overwritten by checkout:
///         .kanspec/tickets/t-c7ec.md
/// ```
///
/// As shipped, every kanspec surface said all was well — `doctor` 13/13, `status` "nothing
/// owed" — while the agent could not move. Worse, the one remediation `status` DID print
/// (`git add -A .kanspec && git commit`) commits the ship record onto the feature branch,
/// where main never sees it: following kanspec's own advice deepened the hole.
///
/// kanspec does not own the user's git commands, so this cannot be prevented outright. The
/// contract this test pins is the three things it CAN do: say so at claim time, see the
/// wedge afterwards, and name a recovery that genuinely undoes it — which the test proves by
/// running the printed command verbatim rather than by reading it.
#[test]
fn the_git_add_dash_a_wedge_is_visible_and_the_printed_recovery_undoes_it() {
    let repo = TestRepo::new();
    let root = repo.root.clone();

    // ── claim WITHOUT --worktree: the primary itself moves onto the branch ────
    let t = json_in(&repo, &root, &["new", "Rate-limit login endpoint"]);
    let id = t["id"].as_str().expect("an id").to_string();
    let ticket = format!(".kanspec/tickets/{id}.md");
    // The ordinary rhythm: the tracker is committed to main between verbs, so main's board
    // holds `todo` and the branch is what drifts away from it.
    repo.commit("kanspec: sync the tracker, as a real repo does");
    repo.push("main");

    let started = json_in(&repo, &root, &["start", &id]);
    let branch = started["branch"].as_str().expect("a branch").to_string();
    assert_eq!(started["checked_out"], Value::Bool(true), "{started}");

    // PREVENT: one line, at claim time, naming the hazard and the alternative.
    let note = started["tracker_note"]
        .as_str()
        .unwrap_or_else(|| panic!("a --worktree-less claim must name the hazard: {started}"));
    assert!(
        note.contains("git add -A") && note.contains("--worktree"),
        "the notice has to name the command that causes it and the flag that avoids it: {note}"
    );

    // NO FALSE POSITIVE. A merely-dirty tracker is the resting state of `sync = "batch"`:
    // the ticket file is still byte-identical to main's, `git switch` carries it across, and
    // a warning here would be noise on every single claim.
    let quiet = json_in(&repo, &root, &["status"]);
    assert!(
        quiet["tracker_drift"].is_null(),
        "a freshly claimed branch has drifted nowhere yet: {quiet}"
    );
    assert!(
        repo.git_try(&["checkout", "main"]).0 == 0,
        "and it proves it"
    );
    repo.git(&["checkout", &branch]);

    // ── the agent runs the near-universal commit, sweeping the tracker along ──
    write_at(
        &root,
        "src/auth/limit.ts",
        "export const limit = () => {};\n",
    );
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "--quiet", "-m", "feat: rate limit login"]);

    // ── any further verb re-dirties the swept file ───────────────────────────
    json_in(&repo, &root, &["ship", &id, "--pr", "7"]);

    // ── THE WEDGE, with real git and no mocking ──────────────────────────────
    let (code, _, err) = repo.git_try(&["checkout", "main"]);
    assert_ne!(
        code, 0,
        "the wedge must actually reproduce, or this proves nothing"
    );
    assert!(
        err.contains("would be overwritten by checkout") && err.contains(&ticket),
        "{err}"
    );

    // ── DETECT ───────────────────────────────────────────────────────────────
    let st = json_in(&repo, &root, &["status"]);
    let drift = &st["tracker_drift"];
    assert!(
        !drift.is_null(),
        "status was cheerfully silent while the working tree was wedged: {st}"
    );
    assert_eq!(drift["branch"], Value::String(branch.clone()));
    assert_eq!(
        drift["main"], "main",
        "`main = \"origin/main\"` is a REMOTE ref — a fix that pasted it into `git switch` \
         would detach HEAD and hand the user a second wedge: {drift}"
    );
    assert_eq!(drift["blocking"][0], Value::String(ticket.clone()));
    assert_eq!(drift["committed"][0], Value::String(ticket.clone()));

    let fix = str_at(drift, "fix").to_string();
    assert_eq!(
        st["next"][0],
        Value::String(fix.clone()),
        "no ticket verb can run until the checkout works, so the recovery leads: {st}"
    );

    let human = repo.ks(["status"]).stdout;
    assert!(
        human.contains("is blocked") && human.contains(&branch),
        "the human surface has to say it too: {human}"
    );
    assert!(
        !human.contains("nothing owed"),
        "\"nothing owed\" printed above a wedged tree is the exact silence this ends: {human}"
    );
    assert!(
        !human.contains("tracker changes pending"),
        "the batch reminder's advice commits the ship record onto the feature branch — under \
         drift it must be REPLACED, not printed alongside: {human}"
    );

    // ── RECOVER: run what was printed, verbatim, and require that it works ────
    let ran = std::process::Command::new("sh")
        .arg("-c")
        .arg(&fix)
        .current_dir(&root)
        .output()
        .expect("sh runs");
    assert!(
        ran.status.success(),
        "the named recovery must actually run:\n{}\n{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );

    assert_eq!(
        repo.git(&["branch", "--show-current"]).trim(),
        "main",
        "the checkout that was refused is the thing the fix had to deliver"
    );
    assert!(
        repo.git(&["status", "--porcelain"]).trim().is_empty(),
        "and it leaves no rubble behind"
    );
    // The whole point of getting to main: main's board can now see the ship record.
    let on_main = repo.read(&ticket);
    assert!(
        on_main.contains("state: review") && on_main.contains("pr: 7"),
        "the ship record has to survive the move, not be traded for a clean checkout:\n{on_main}"
    );
    let after = json_in(&repo, &root, &["status"]);
    assert!(after["tracker_drift"].is_null(), "{after}");
    assert_eq!(after["pending_changes"], 0, "{after}");
    // And the work is not stranded: the ticket branch is checkoutable again.
    assert_eq!(repo.git_try(&["checkout", &branch]).0, 0);
    repo.git(&["checkout", "main"]);

    // ── the arrangement that never wedges says nothing at all ────────────────
    let w = json_in(&repo, &root, &["new", "Second ticket"]);
    let w_id = w["id"].as_str().expect("an id").to_string();
    let claimed = json_in(&repo, &root, &["start", &w_id, "--worktree"]);
    assert!(
        claimed["tracker_note"].is_null(),
        "`--worktree` leaves the primary on main; there is no hazard to warn about: {claimed}"
    );
    let still = json_in(&repo, &root, &["status"]);
    assert!(
        still["tracker_drift"].is_null(),
        "the primary never left main, so nothing drifted: {still}"
    );
}

/// `worktree =` in the repo's config sets what `start` does when nobody says otherwise.
///
/// It is a REPO setting on purpose: whether worktrees suit a project (untracked `.env`,
/// build output, `node_modules`) is a fact about the project, equally true for every
/// teammate and every agent in it — so one person works it out and the committed config
/// settles it. The flags stay available for the one-off in either direction.
#[test]
fn the_repo_can_default_start_to_a_worktree_and_either_flag_still_wins() {
    // ── default is OFF: the historical behaviour, unchanged ──────────────────
    let repo = TestRepo::new();
    let id = new_ticket(&repo, "claims in place by default");
    repo.ks(["start", &id]).ok();
    assert!(
        !worktree_of(&repo, &id).is_dir(),
        "with no setting and no flag, `start` must claim in place"
    );
    // ── worktree = true: `start` makes one unasked ───────────────────────────
    let repo = TestRepo::new();
    set_worktree_default(&repo, true);
    let id = new_ticket(&repo, "claims into a worktree");
    let r = repo.ks(["start", &id]).ok();
    assert!(
        worktree_of(&repo, &id).is_dir(),
        "the repo asked for a worktree by default: {}",
        r.stdout
    );

    // ── --no-worktree beats the repo default ─────────────────────────────────
    let repo = TestRepo::new();
    set_worktree_default(&repo, true);
    let id = new_ticket(&repo, "opts out for this one claim");
    repo.ks(["start", &id, "--no-worktree"]).ok();
    assert!(
        !worktree_of(&repo, &id).is_dir(),
        "--no-worktree must beat `worktree = true`"
    );

    // ── --worktree beats an explicit false ───────────────────────────────────
    let repo = TestRepo::new();
    set_worktree_default(&repo, false);
    let id = new_ticket(&repo, "opts in for this one claim");
    repo.ks(["start", &id, "--worktree"]).ok();
    assert!(
        worktree_of(&repo, &id).is_dir(),
        "--worktree must beat `worktree = false`"
    );

    // ── the two flags are mutually exclusive, and say so ─────────────────────
    let repo = TestRepo::new();
    let id = new_ticket(&repo, "cannot ask for both");
    let both = repo.ks(["start", &id, "--worktree", "--no-worktree"]);
    assert_eq!(both.code, 64, "{}{}", both.stdout, both.stderr);
}

/// Turning the default on must also silence the `git add -A` wedge warning, which exists
/// only for the claim-in-place arrangement.
#[test]
fn the_wedge_warning_follows_the_effective_worktree_choice_not_the_flag() {
    let repo = TestRepo::new();
    set_worktree_default(&repo, true);
    let id = new_ticket(&repo, "no wedge to warn about");
    let r = repo.ks(["start", &id]).ok();
    assert!(
        !r.stdout.contains("git add -A"),
        "a worktree claim has no tracker wedge to warn about: {}",
        r.stdout
    );

    let repo = TestRepo::new();
    let id = new_ticket(&repo, "wedge is real here");
    let r = repo.ks(["start", &id]).ok();
    assert!(
        r.stdout.contains("git add -A"),
        "claiming in place must still name the hazard: {}",
        r.stdout
    );
}

fn set_worktree_default(repo: &TestRepo, on: bool) {
    let p = ".kanspec/config.toml";
    let cfg = repo.read(p);
    // Replace the key if it is there, append if not — a duplicate key is a TOML error,
    // and the fixture writes a minimal config rather than the full `init` scaffold.
    let next = match cfg.lines().find(|l| l.trim_start().starts_with("worktree ")) {
        Some(line) => cfg.replace(line, &format!("worktree = {on}")),
        None => format!("{cfg}\nworktree = {on}\n"),
    };
    repo.write(p, &next);
}

fn new_ticket(repo: &TestRepo, title: &str) -> String {
    str_at(&json_in(repo, &repo.root, &["new", title]), "id").to_string()
}

fn worktree_of(repo: &TestRepo, id: &str) -> std::path::PathBuf {
    repo.root.join("..").join("kanspec-wt").join(id)
}
