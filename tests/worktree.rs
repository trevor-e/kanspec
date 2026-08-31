//! `tests/worktree.rs`
//!
//! Proves: every mutating verb run from a linked worktree lands in the primary `.kanspec/`
//!
//! Every mutating verb goes through exactly one function — `Store::transact` — so this
//! tests that function directly rather than waiting on the verbs that will wrap it. If a
//! write from a linked worktree could land anywhere but the primary `.kanspec/`, every
//! parallel agent would get its own board, its own ready queue and its own claim ledger,
//! and `start` would stop being atomic across worktrees.
//!
//! Owner: **S2**.

mod common;

use chrono::{TimeZone, Utc};
use common::TestRepo;
use kanspec::ctx::Actor;
use kanspec::error::Result;
use kanspec::fm::Yv;
use kanspec::ids::{Minter, TicketId};
use kanspec::keys::{Key, TicketKey};
use kanspec::model::Snapshot;
use kanspec::plan::{EntityRef, Op, Plan};
use kanspec::store::Store;
use kanspec::transitions::{State, Verb};

const TICKET: &str = "\
---
id: t-9c41
title: Rate-limit login endpoint
state: todo               # todo | doing | review | done | dropped
spec: auth
deps: []
branch: null
claimed_by: null
pr: null
head: null
created: 2026-08-30T14:02:11Z
---
Implement [auth.lockout].

## Steps
- [x] lockout counter
- [ ] 429 + Retry-After

## Log
- 2026-08-30T14:02Z  todo     trevor                new
";

fn seed(repo: &TestRepo) {
    repo.write(".kanspec/tickets/t-9c41.md", TICKET);
}

/// `Committed` and `Snapshot` deliberately do not derive `Debug` (a `Snapshot` is the
/// whole store), so `unwrap_err` is unavailable and the refusal is unwrapped by hand.
#[track_caller]
fn refusal<T>(r: Result<T>) -> kanspec::error::KsError {
    match r {
        Ok(_) => panic!("expected a refusal, got a successful commit"),
        Err(e) => e,
    }
}

fn tid() -> TicketId {
    TicketId::parse("t-9c41").unwrap()
}

fn actor() -> Actor {
    Actor::Human {
        name: "trevor".into(),
    }
}

/// The exact `Op` shape `kanspec start` produces.
fn start_plan(
    verb: Verb,
    also: Vec<(TicketKey, Yv)>,
) -> impl Fn(&Snapshot, &Minter) -> Result<Plan> {
    move |_s: &Snapshot, _m: &Minter| {
        Ok(Plan::of(vec![Op::Transition {
            id: tid(),
            verb,
            actor: actor(),
            at: Utc.with_ymd_and_hms(2026, 8, 31, 10, 14, 0).unwrap(),
            detail: "branch + worktree created".into(),
            also: also.clone(),
        }]))
    }
}

#[test]
fn a_transaction_run_from_a_linked_worktree_writes_the_primary_kanspec() {
    let repo = TestRepo::new();
    seed(&repo);
    let wt = repo.worktree("t-9c41");
    // The linked worktree is checked out from a commit that predates the ticket, so it has
    // no `.kanspec/tickets/` of its own. If the write landed locally we would see it.
    assert!(!wt.join(".kanspec/tickets/t-9c41.md").exists());

    let ctx = common::ctx_at(&wt);
    assert!(ctx.repo.linked());

    let done = Store::open(&ctx)
        .transact(
            Some(Verb::Start),
            "kanspec start t-9c41",
            start_plan(
                Verb::Start,
                vec![
                    (TicketKey::Branch, Yv::s("ks/t-9c41-rate-limit-login")),
                    (TicketKey::ClaimedBy, Yv::s("trevor")),
                ],
            ),
        )
        .expect("the transaction must commit");

    // The write landed in the PRIMARY worktree, and nowhere else.
    assert_eq!(done.touched.len(), 1);
    assert_eq!(
        done.touched[0],
        repo.root
            .canonicalize()
            .unwrap()
            .join(".kanspec/tickets/t-9c41.md")
    );
    assert!(
        !wt.join(".kanspec/tickets/t-9c41.md").exists(),
        "a linked worktree must never grow its own board"
    );

    let after = repo.read(".kanspec/tickets/t-9c41.md");
    assert!(after.contains("state: doing               # todo | doing | review | done | dropped"));
    assert!(after.contains("branch: ks/t-9c41-rate-limit-login"));
    assert!(after.contains("claimed_by: trevor"));
    assert!(after.ends_with(
        "- 2026-08-31T10:14Z  doing    trevor                start (branch + worktree created)\n"
    ));

    // The frontmatter delta and the `## Log` line are ONE write to ONE file, so exactly
    // three source lines changed and exactly one was appended.
    let before: Vec<&str> = TICKET.lines().collect();
    let now: Vec<&str> = after.lines().collect();
    assert_eq!(now.len(), before.len() + 1);
    let changed = before.iter().zip(&now).filter(|(a, b)| a != b).count();
    assert_eq!(changed, 3, "state + branch + claimed_by");

    // And the returned snapshot is the POST-write view, so a caller (or the server's memo)
    // can never observe the pre-write state.
    assert_eq!(done.snapshot.ticket(&tid()).unwrap().fm.state, State::Doing);
    assert_eq!(done.snapshot.rev, done.rev);
    assert!(done.rev >= 1);
}

#[test]
fn the_lock_is_the_primary_worktrees_lock_however_many_worktrees_there_are() {
    let repo = TestRepo::new();
    seed(&repo);
    let a = repo.worktree("t-aaaa");
    let b = repo.worktree("t-bbbb");
    let primary = common::ctx_at(&repo.root);
    for cwd in [repo.root.clone(), a, b] {
        let ctx = common::ctx_at(&cwd);
        assert_eq!(
            ctx.layout.lock(),
            primary.layout.lock(),
            "one lock per machine, or `start` stops being atomic across worktrees"
        );
        assert_eq!(ctx.layout.gitstate(), primary.layout.gitstate());
        assert_eq!(ctx.layout.tickets_dir(), primary.layout.tickets_dir());
    }
}

/// A linked worktree has a `.kanspec/config.toml` of its own — it is a tracked file, so
/// `git worktree add` checks one out — and it is not the one that governs. If config
/// resolution ever slipped from the primary to the cwd, an agent in a worktree would
/// auto-commit while the human batched (or push to a different `main`, or mint IDs at a
/// different width) and the two halves of one repo would silently disagree.
#[test]
fn the_config_a_linked_worktree_reads_is_the_primarys() {
    let repo = TestRepo::new();
    seed(&repo);
    let wt = repo.worktree("t-9c41");
    assert!(
        wt.join(".kanspec/config.toml").exists(),
        "the fixture is only honest if the worktree really has a config of its own"
    );
    // Two configs that disagree about something with a visible consequence.
    std::fs::write(
        wt.join(".kanspec/config.toml"),
        "main = \"origin/main\"\nsync = \"commit\"\nid_width = 8\n",
    )
    .unwrap();
    let head = repo.sha("HEAD");

    let ctx = common::ctx_at(&wt);
    assert!(ctx.repo.linked());
    assert_eq!(
        ctx.cfg.sync,
        kanspec::config::SyncMode::Batch,
        "the primary's config governs, not the one under the cwd"
    );
    assert_eq!(ctx.cfg.id_width, 4);

    // And the consequence is real, not just a field read: a `sync = "commit"` config in
    // the worktree would have turned this transaction into a git commit.
    Store::open(&ctx)
        .transact(
            Some(Verb::Start),
            "kanspec start t-9c41",
            start_plan(Verb::Start, vec![]),
        )
        .expect("the transaction must commit");
    assert_eq!(
        repo.sha("HEAD"),
        head,
        "batch sync leaves the tracker dirty; the worktree's `commit` must not have won"
    );
}

/// Hermeticity, and the tri-state, at the `Ctx` level.
///
/// Every `TestRepo` has a bare local path for an `origin`, so `gh` has nothing it could
/// answer about it — and must say so *without spawning anything*. This is what keeps the
/// whole suite deterministic on a developer machine where `gh auth status` succeeds: a
/// `Gh` that probed first and asked questions later would put a live network call inside
/// every scan test.
///
/// The refusal shape matters as much as the refusal. `pr_for_head` declines with a reason
/// rather than answering `Ok(vec![])`, because an empty list is a real observation
/// ("GitHub knows of no PR for this branch") and reading "gh is not usable here" as one
/// would be the first step down the road to a confident wrong "not merged".
#[test]
fn gh_is_unavailable_over_a_non_github_origin_and_says_why_rather_than_answering() {
    assert!(
        std::env::var_os("KANSPEC_GH_FIXTURES").is_none(),
        "this test asserts the un-mocked behaviour; unset KANSPEC_GH_FIXTURES to run it"
    );
    let repo = TestRepo::new();
    let wt = repo.worktree("reader");
    let ctx = common::ctx_at(&wt);

    assert!(
        !ctx.gh.available(),
        "`origin` is a bare local path — `gh` cannot have an opinion about it"
    );

    let view = ctx.gh.pr_view(142).unwrap_err().to_string();
    assert!(
        view.contains("not a GitHub remote"),
        "the badge needs the reason, not an empty answer: {view}"
    );

    match ctx.gh.pr_for_head("ks/t-9c41-rate-limit-login") {
        Ok(list) => panic!(
            "an unusable `gh` must decline, not report {} PRs — an empty list is an \
             observation about GitHub, and this is an observation about `gh`",
            list.len()
        ),
        Err(e) => assert!(e.to_string().contains("not a GitHub remote"), "{e}"),
    }
}

#[test]
fn an_illegal_transition_is_refused_before_a_byte_moves() {
    let repo = TestRepo::new();
    seed(&repo);
    let before = repo.read(".kanspec/tickets/t-9c41.md");
    let ctx = common::ctx_at(&repo.root);

    // `ship` is legal only from `doing`; this ticket is `todo`.
    let e = refusal(Store::open(&ctx).transact(
        Some(Verb::Ship),
        "kanspec ship t-9c41",
        start_plan(Verb::Ship, vec![]),
    ));
    assert_eq!(e.kind(), "illegal_transition");
    assert!(e.to_string().contains("t-9c41 is todo, not doing"), "{e}");
    assert!(e.fixes().iter().next().is_some(), "invariant 9");
    assert_eq!(
        repo.read(".kanspec/tickets/t-9c41.md"),
        before,
        "a refused transaction must leave the file byte-identical"
    );
}

#[test]
fn a_hand_edited_state_is_caught_by_the_very_next_verb() {
    let repo = TestRepo::new();
    seed(&repo);
    // `sed -i 's/state: todo/state: review/'` — the exact edit no type can prevent (R-2).
    // The answer is detection, not prevention: the `## Log` still says the ticket only
    // ever reached `todo`.
    repo.write(
        ".kanspec/tickets/t-9c41.md",
        &TICKET.replace("state: todo ", "state: review "),
    );
    let poisoned = repo.read(".kanspec/tickets/t-9c41.md");
    let ctx = common::ctx_at(&repo.root);

    // `start` IS legal from `review` (it is the rework path), so without the pre-write
    // proof the forged state would be laundered into a legal one and the edit would
    // disappear.
    let e = refusal(Store::open(&ctx).transact(
        Some(Verb::Start),
        "kanspec start t-9c41",
        start_plan(Verb::Start, vec![]),
    ));
    assert_eq!(e.kind(), "gate");
    assert_eq!(e.code(), Some("log_violation"));
    assert!(e.to_string().contains("t-9c41"), "{e}");
    assert!(
        e.fixes().iter().any(|f| f.as_str().contains("repair")),
        "a ticket nobody can write is worse than one somebody has to re-attest: {e}"
    );
    assert_eq!(repo.read(".kanspec/tickets/t-9c41.md"), poisoned);

    // …and `repair` is the way out, precisely because its logged state is authoritative.
    let done = Store::open(&ctx)
        .transact(
            Some(Verb::Repair),
            "kanspec repair t-9c41 --why \"imported\"",
            move |_s: &Snapshot, _m: &Minter| {
                Ok(Plan::of(vec![Op::Transition {
                    id: tid(),
                    verb: Verb::Repair,
                    actor: actor(),
                    at: Utc.with_ymd_and_hms(2026, 8, 31, 10, 14, 0).unwrap(),
                    detail: "imported from another tracker".into(),
                    also: vec![],
                }]))
            },
        )
        .expect("repair must be able to run on a ticket nothing else can write");
    assert_eq!(
        done.snapshot.ticket(&tid()).unwrap().fm.state,
        State::Review,
        "repair records the state a human attested to"
    );

    // And the very next ordinary verb now works.
    Store::open(&ctx)
        .transact(
            Some(Verb::Start),
            "kanspec start t-9c41",
            start_plan(Verb::Start, vec![]),
        )
        .expect("the ticket is writable again");
}

#[test]
fn a_transaction_that_cannot_be_edited_in_place_is_refused_by_type() {
    let repo = TestRepo::new();
    // A quoted key: the YAML parser sees it, the line indexer does not. Without the
    // `fm::writable` guard, `set` would APPEND a duplicate key and the file would grow a
    // second `state:` on every verb.
    repo.write(
        ".kanspec/tickets/t-9c41.md",
        &TICKET.replace("spec: auth", "\"my key\": 1\nspec: auth"),
    );
    let before = repo.read(".kanspec/tickets/t-9c41.md");
    let ctx = common::ctx_at(&repo.root);
    let e = refusal(Store::open(&ctx).transact(
        Some(Verb::Start),
        "kanspec start t-9c41",
        start_plan(Verb::Start, vec![]),
    ));
    assert_eq!(e.kind(), "invalid");
    assert!(e.to_string().contains("cannot edit in place"), "{e}");
    assert_eq!(repo.read(".kanspec/tickets/t-9c41.md"), before);
}

#[test]
fn a_missing_kanspec_directory_is_an_environment_refusal_not_a_panic() {
    let repo = TestRepo::new();
    std::fs::remove_dir_all(repo.root.join(".kanspec")).unwrap();
    let ctx = common::ctx_at(&repo.root);
    let e = refusal(Store::open(&ctx).transact(
        Some(Verb::Start),
        "kanspec start t-9c41",
        start_plan(Verb::Start, vec![]),
    ));
    assert_eq!(e.kind(), "environment");
    assert_eq!(e.code(), Some("not_initialized"));
    assert_eq!(e.exit_code(), 69);
}

#[test]
fn create_entity_never_clobbers_and_set_fields_reaches_every_entity_kind() {
    let repo = TestRepo::new();
    seed(&repo);
    let ctx = common::ctx_at(&repo.root);
    let quirk = kanspec::ids::QuirkId::parse("q-11ba").unwrap();
    let entity = EntityRef::Quirk(quirk.clone());
    let contents = "\
---
id: q-11ba
title: Stripe webhooks replay in staging
paths: [src/billing/**]
severity: landmine
status: active
source: t-9c41
fixed_by: null
---
Retries are not idempotent before the ledger write.
";

    let e = entity.clone();
    let c = contents.to_string();
    let done = Store::open(&ctx)
        .transact(Some(Verb::New), "kanspec quirk add", move |_s, _m| {
            Ok(Plan::of(vec![Op::CreateEntity {
                entity: e.clone(),
                contents: c.clone(),
            }]))
        })
        .expect("a new quirk");
    assert_eq!(done.touched.len(), 1);
    assert_eq!(repo.read(".kanspec/quirks/q-11ba.md"), contents);
    assert!(done.snapshot.quirks.contains_key(&quirk));

    // The same plan a second time must be refused rather than overwrite the file.
    let e = entity.clone();
    let c = contents.to_string();
    let err =
        refusal(
            Store::open(&ctx).transact(Some(Verb::New), "kanspec quirk add", move |_s, _m| {
                Ok(Plan::of(vec![Op::CreateEntity {
                    entity: e.clone(),
                    contents: c.clone(),
                }]))
            }),
        );
    assert_eq!(err.kind(), "conflict");

    // `SetFields` edits it in place, surgically.
    let e = entity.clone();
    Store::open(&ctx)
        .transact(
            Some(Verb::Confirm),
            "kanspec quirk fix q-11ba",
            move |_s, _m| {
                Ok(Plan::of(vec![Op::SetFields {
                    entity: e.clone(),
                    sets: vec![
                        (Key::Quirk(kanspec::keys::QuirkKey::Status), Yv::s("fixed")),
                        (
                            Key::Quirk(kanspec::keys::QuirkKey::FixedBy),
                            Yv::s("t-9c41"),
                        ),
                    ],
                }]))
            },
        )
        .expect("a quirk edit");
    let after = repo.read(".kanspec/quirks/q-11ba.md");
    assert!(after.contains("status: fixed"));
    assert!(after.contains("fixed_by: t-9c41"));
    assert!(
        after.contains("paths: [src/billing/**]"),
        "unquoted globs stay unquoted"
    );
    assert_eq!(after.lines().count(), contents.lines().count());
}

#[test]
fn an_empty_plan_commits_nothing_and_is_still_a_success() {
    let repo = TestRepo::new();
    seed(&repo);
    let before = repo.read(".kanspec/tickets/t-9c41.md");
    let ctx = common::ctx_at(&repo.root);
    let done = Store::open(&ctx)
        .transact(Some(Verb::Confirm), "kanspec scan", |_s, _m| {
            Ok(Plan::empty())
        })
        .expect("nothing to do is not a failure");
    assert!(done.touched.is_empty());
    assert_eq!(repo.read(".kanspec/tickets/t-9c41.md"), before);
}

#[test]
fn sync_commit_makes_the_transaction_a_git_commit_of_the_tracker_alone() {
    let repo = TestRepo::new();
    seed(&repo);
    repo.write(
        ".kanspec/config.toml",
        "main = \"origin/main\"\nsync = \"commit\"\n",
    );
    repo.commit("seed the ticket");
    let head = repo.sha("HEAD");
    repo.write("src/auth/wip.ts", "// a human's uncommitted work\n");

    let ctx = common::ctx_at(&repo.root);
    Store::open(&ctx)
        .transact(
            Some(Verb::Start),
            "kanspec start t-9c41",
            start_plan(Verb::Start, vec![]),
        )
        .expect("commit-sync");

    assert_ne!(repo.sha("HEAD"), head);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s"]).trim(),
        "kanspec: start t-9c41"
    );
    assert_eq!(
        repo.git(&["show", "--name-only", "--format=", "HEAD"])
            .trim(),
        ".kanspec/tickets/t-9c41.md",
        "the tracker commit must not sweep up the human's working tree"
    );
    assert_eq!(
        repo.git(&["status", "--porcelain", "--", "src"]).trim(),
        "?? src/auth/wip.ts"
    );
}

#[test]
fn the_snapshot_load_path_reads_every_entity_kind_from_the_primary() {
    let repo = TestRepo::with_merges();
    let wt = repo.worktree("reader");
    let ctx = common::ctx_at(&wt);

    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login and lockout\ncode: [src/auth/**]\n---\n# auth\n\n## Rules\n\
         - [auth.lockout] 5 failed logins lock the account. {p-7de2}\n",
    );
    repo.write(
        ".kanspec/decisions/D-8c1a.md",
        "---\nid: D-8c1a\ntitle: Redis only\nstatus: accepted\ndate: 2026-09-02\n\
         source: p-7de2#p1\nscope: [src/auth/**]\n---\n## Decision\nRedis.\n",
    );
    repo.write(
        ".kanspec/quirks/q-11ba.md",
        "---\nid: q-11ba\ntitle: Webhook replay\npaths: [src/billing/**]\nseverity: landmine\n\
         status: active\n---\nRetries are not idempotent.\n",
    );
    repo.write(
        ".kanspec/proposals/p-7de2-login-rate-limiting/proposal.md",
        "---\nid: p-7de2\ntitle: Login rate limiting\nstatus: review\nspecs: [auth]\n\
         created: 2026-08-30\n---\n## Changes\n- [c1] auth: lockout\n\n## Prescriptions\n\
         - [p1] (promote: decision) Redis only.\n",
    );
    std::fs::create_dir_all(
        repo.root
            .join(".kanspec/proposals/closed/p-19f0-money-as-cents"),
    )
    .unwrap();
    repo.write(
        ".kanspec/proposals/p-7de2-login-rate-limiting/comments.jsonl",
        "{\"id\":\"cm-88f1\",\"op\":\"comment\",\"target\":\"p-7de2#c1\",\"body\":\"x\",\
         \"at\":\"2026-08-30T16:02:00Z\"}\n",
    );

    let snap = ctx.snapshot().expect("the snapshot loads");
    assert_eq!(snap.tickets.len(), 6, "the six merge-shape fixtures");
    assert_eq!(snap.specs.len(), 1);
    assert_eq!(snap.specs.values().next().unwrap().rules.len(), 1);
    assert_eq!(snap.decisions.len(), 1);
    assert_eq!(snap.quirks.len(), 1);
    assert_eq!(snap.proposals.len(), 1, "OPEN proposals only");
    assert_eq!(snap.proposals.values().next().unwrap().items.len(), 2);
    assert!(
        snap.closed_ids.contains("p-19f0"),
        "a closed proposal contributes its ID — and nothing else — so the minter cannot \
         collide with it"
    );
    assert_eq!(snap.comments.values().flatten().count(), 1);
    assert!(snap.taken_ids().contains("p-19f0"));
    // The disposable cache is part of the snapshot, and an absent one is `NeverScanned`.
    assert!(snap.git.is_empty());
}

#[test]
fn a_ticket_whose_filename_disagrees_with_its_frontmatter_is_refused_loudly() {
    let repo = TestRepo::new();
    repo.write(".kanspec/tickets/t-0001.md", TICKET);
    let ctx = common::ctx_at(&repo.root);
    let e = refusal(ctx.snapshot());
    assert_eq!(e.kind(), "invalid");
    assert!(e.to_string().contains("t-0001"), "{e}");
    assert!(e.to_string().contains("t-9c41"), "{e}");
    assert!(e.fixes().iter().next().is_some());
}

/// `status`'s sync remediation has to be runnable *where it is printed*, and has to name
/// every file it counts. It failed both ways: it counted only `.kanspec/**`, so a
/// regenerated `KANSPEC-FEATURES.md` stayed uncommitted while `status` reported nothing
/// pending — leaving the committed feature map stale, the exact rot the projections exist
/// to prevent; and it emitted a bare `git`, which from the linked worktree that
/// `start --worktree` sends the agent into finds a clean tree, exits 1, and leaves the
/// primary dirty.
#[test]
fn the_sync_fix_is_runnable_from_a_linked_worktree_and_names_the_projections() {
    let repo = TestRepo::new();
    seed(&repo);
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "seed"]);
    let wt = repo.worktree("t-9c41");

    // A dirty projection at the repo ROOT — outside `.kanspec/`, which is the blind spot.
    repo.write("KANSPEC-FEATURES.md", "<!-- GENERATED -->\n# Feature map\n");

    let out = repo.ks_in(&wt, ["status", "--json"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    let fix = v["sync_fix"].as_str().unwrap();

    assert!(
        v["pending_changes"].as_u64().unwrap() >= 1,
        "a dirty root projection must count as a pending tracker change: {v}"
    );
    assert!(
        fix.contains("KANSPEC-FEATURES.md"),
        "the fix must name the projections it counted: {fix}"
    );
    // Canonicalized: on macOS the temp root resolves through /private.
    let primary = repo.root.canonicalize().unwrap();
    assert!(
        fix.contains(&format!("-C {}", primary.display())),
        "printed inside a linked worktree, the fix must target the primary: {fix}"
    );

    // The real test of advice: follow it verbatim and the thing it promised must happen.
    let ran = std::process::Command::new("sh")
        .arg("-c")
        .arg(fix)
        .current_dir(&wt)
        .output()
        .unwrap();
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let after = repo.ks_in(&wt, ["status", "--json"]);
    let v2: serde_json::Value = serde_json::from_str(&after.stdout).unwrap();
    assert_eq!(
        v2["pending_changes"].as_u64().unwrap(),
        0,
        "after following the fix, nothing may still be pending: {v2}"
    );
}
