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
