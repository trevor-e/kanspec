//! t-1e66 — `start` and `init` on a branch the configured main has never seen.
//!
//! Found on a trial that integrated through a migration branch: `start` cut ticket
//! branches from `origin/main`, which had no `.kanspec/`, and nothing said so until the
//! next verb could not find a store. The fix was one config line nobody was told about.

mod common;

use std::path::Path;

use common::{write_at, TestRepo};
use serde_json::Value;

fn json(repo: &TestRepo, args: &[&str]) -> Value {
    repo.json(args)
}

fn refusal(repo: &TestRepo, args: &[&str]) -> Value {
    let mut all: Vec<&str> = args.to_vec();
    all.push("--json");
    let r = repo.ks(&all);
    assert_ne!(
        r.code,
        0,
        "`kanspec {}` was supposed to refuse:\n{}",
        all.join(" "),
        r.stdout
    );
    let v: Value = serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
        panic!(
            "a refusal must still be JSON ({e}):\n{}\n{}",
            r.stdout, r.stderr
        )
    });
    assert_eq!(v["ok"], Value::Bool(false));
    v["error"].clone()
}

fn fixes(err: &Value) -> Vec<String> {
    err["fix"]
        .as_array()
        .expect("a fix list")
        .iter()
        .map(|f| f.as_str().unwrap().to_string())
        .collect()
}

fn new_ticket(repo: &TestRepo, title: &str) -> String {
    json(repo, &["new", title])["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A pushed branch `other` that carries the code but NOT `.kanspec/` — what `origin/main`
/// looked like on the trial. Leaves the repo back on `main`, where the store lives.
fn push_storeless_branch(repo: &TestRepo, name: &str) {
    repo.git(&["checkout", "-q", "-b", name]);
    repo.git(&["rm", "-r", "-q", ".kanspec"]);
    repo.git(&["commit", "-q", "-m", "a branch that never had the store"]);
    repo.push(name);
    repo.git(&["checkout", "-q", "main"]);
}

fn set_main(repo: &TestRepo, main: &str) {
    write_at(
        &repo.root,
        ".kanspec/config.toml",
        &format!("main = \"{main}\"\nid_width = 4\nsync = \"batch\"\n"),
    );
}

#[test]
fn start_refuses_to_cut_from_a_main_that_has_never_carried_the_store() {
    let repo = TestRepo::new();
    push_storeless_branch(&repo, "other");
    set_main(&repo, "origin/other");
    let id = new_ticket(&repo, "Rate-limit login endpoint");

    let err = refusal(&repo, &["start", &id]);
    assert_eq!(err["code"], "main_blind", "{err}");
    let msg = err["message"].as_str().unwrap();
    assert!(
        msg.contains("origin/other")
            && msg.contains(".kanspec/config.toml")
            && msg.contains("main"),
        "the refusal names the blind base, the store, and where the store lives: {msg}"
    );
    // The one fix the trial had to discover by hand. `origin/main` exists, so it is the
    // remote spelling that gets suggested — the same value the default config carries.
    let f = fixes(&err);
    assert!(
        f[0].contains("main = \"origin/main\"") && f[0].contains("config.toml"),
        "the first fix is the config line: {f:?}"
    );
    // No branch was left behind by the refusal.
    let branches = repo.git(&["branch", "--list", "ks/*"]);
    assert!(
        branches.trim().is_empty(),
        "a refused claim creates nothing: {branches}"
    );
    let t = json(&repo, &["show", &id]);
    assert_eq!(t["state"], "todo", "{t}");

    // `status` sees the same thing and gives the same fix, not the tracker transplant.
    let s = json(&repo, &["status"]);
    let d = &s["tracker_drift"];
    assert_eq!(d["main_blind"], Value::Bool(true), "{s}");
    assert!(
        d["fix"]
            .as_str()
            .unwrap()
            .contains("main = \"origin/main\""),
        "the drift fix is the config line, not a checkout of .gitkeep files: {d}"
    );
    let human = repo.ks(["status"]).stdout;
    assert!(human.contains("has never carried .kanspec/"), "{human}");

    // The config line is the whole fix.
    set_main(&repo, "origin/main");
    let started = json(&repo, &["start", &id]);
    assert_eq!(started["state"], "doing", "{started}");
    assert!(
        started["base_note"].is_null(),
        "main is on main's history: {started}"
    );
    let s = json(&repo, &["status"]);
    assert!(s["tracker_drift"].is_null(), "{s}");
}

#[test]
fn start_refuses_when_main_is_merely_unpushed_and_says_push() {
    let repo = TestRepo::new();
    push_storeless_branch(&repo, "trial");
    // The store gets committed onto the local `trial`, which is also the configured main —
    // it just has not been pushed. A `switch` onto a branch cut from origin/trial would
    // delete the tracked store from the working tree.
    repo.git(&["checkout", "-q", "trial"]);
    repo.git(&["checkout", "main", "--", ".kanspec"]);
    set_main(&repo, "origin/trial");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "kanspec: the store, on trial"]);
    let id = new_ticket(&repo, "Rate-limit login endpoint");

    let err = refusal(&repo, &["start", &id]);
    assert_eq!(err["code"], "main_behind", "{err}");
    assert_eq!(fixes(&err)[0], "git push origin trial", "{err}");

    repo.push("trial");
    let started = json(&repo, &["start", &id]);
    assert_eq!(started["state"], "doing", "{started}");
    assert!(started["base_note"].is_null(), "{started}");
    assert!(
        repo.root.join(".kanspec/config.toml").exists(),
        "the switch onto the ticket branch kept the store"
    );
}

#[test]
fn start_says_when_the_ticket_branch_starts_without_heads_commits() {
    let repo = TestRepo::new();
    let id = new_ticket(&repo, "Rate-limit login endpoint");
    let other = new_ticket(&repo, "Charge idempotency");
    // Source only: the tracker stays uncommitted, as it is between syncs.
    write_at(
        &repo.root,
        "src/auth/limit.ts",
        "export const limit = () => {};\n",
    );
    repo.git(&["add", "src"]);
    repo.git(&["commit", "-q", "-m", "an unpushed commit on main"]);

    // main is 1 ahead of origin/main: the claim goes through, and says what it leaves out.
    let started = json(&repo, &["start", &id]);
    assert_eq!(started["state"], "doing", "{started}");
    let note = started["base_note"]
        .as_str()
        .unwrap_or_else(|| panic!("a base note: {started}"));
    assert!(
        note.contains("1 commit ahead of origin/main") && note.contains("starts without"),
        "{note}"
    );
    // From a ticket branch that carries a commit, the next claim is expected to be ahead:
    // no line, because every second claim would otherwise print one.
    let branch = started["branch"].as_str().unwrap().to_string();
    assert_eq!(started["checked_out"], Value::Bool(true), "{started}");
    assert_eq!(repo.git(&["branch", "--show-current"]).trim(), branch);
    write_at(
        &repo.root,
        "src/auth/limit.ts",
        "export const limit = () => 1;\n",
    );
    repo.git(&["add", "src"]);
    repo.git(&["commit", "-q", "-m", "work on the first ticket"]);
    let second = json(&repo, &["start", &other]);
    assert_eq!(second["state"], "doing", "{second}");
    assert!(second["base_note"].is_null(), "{second}");
}

#[test]
fn init_names_the_integration_branch_when_head_is_not_on_mains_history() {
    let repo = TestRepo::new();
    // A repo whose work lives on a branch main never sees, store not yet scaffolded there.
    repo.git(&["checkout", "-q", "-b", "kanspec-migration"]);
    repo.git(&["rm", "-r", "-q", ".kanspec"]);
    repo.git(&[
        "commit",
        "-q",
        "-m",
        "the migration branch, without a store",
    ]);

    // Plain `init` scaffolds against the default and says the default is wrong here.
    let r = json(&repo, &["init"]);
    let note = r["main_note"]
        .as_str()
        .unwrap_or_else(|| panic!("a main note: {r}"));
    assert!(
        note.contains("kanspec-migration") && note.contains("origin/main"),
        "{note}"
    );
    let next: Vec<&str> = r["next"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // Not pushed, so the local spelling is what gets suggested.
    assert!(
        next[0].contains("main = \"kanspec-migration\"") && next[0].contains("config.toml"),
        "{next:?}"
    );
    let human = repo.ks(["init"]).ok().stdout;
    assert!(human.contains("⚠ main"), "{human}");

    // `--main` over an existing config is not written — the config is the user's file —
    // and says so, with the manual edit as the fix.
    let r = json(&repo, &["init", "--main", "kanspec-migration"]);
    assert!(
        r["main_note"].as_str().unwrap().contains("was not written"),
        "{r}"
    );
    assert!(
        repo.read(".kanspec/config.toml")
            .contains("main             = \"origin/main\""),
        "init never rewrites a config"
    );

    // Start over: `--main` on the first init writes the branch it was told.
    std::fs::remove_dir_all(repo.root.join(".kanspec")).unwrap();
    repo.push("kanspec-migration");
    let r = json(&repo, &["init", "--main", "origin/kanspec-migration"]);
    assert!(r["main_note"].is_null(), "{r}");
    let cfg = repo.read(".kanspec/config.toml");
    assert!(
        cfg.contains("main             = \"origin/kanspec-migration\""),
        "{cfg}"
    );
    // ... and `start` cuts from it.
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-q", "-m", "kanspec: init"]);
    repo.push("kanspec-migration");
    let id = new_ticket(&repo, "Rate-limit login endpoint");
    let started = json(&repo, &["start", &id]);
    assert_eq!(started["state"], "doing", "{started}");
    assert!(started["base_note"].is_null(), "{started}");
    let fork = repo.git(&[
        "merge-base",
        started["branch"].as_str().unwrap(),
        "origin/kanspec-migration",
    ]);
    assert_eq!(fork.trim(), repo.sha("origin/kanspec-migration"));
}

#[test]
fn init_refuses_a_main_that_does_not_resolve() {
    let repo = TestRepo::new();
    std::fs::remove_dir_all(repo.root.join(".kanspec")).unwrap();
    let err = refusal(&repo, &["init", "--main", "origin/nowhere"]);
    assert!(
        err["message"].as_str().unwrap().contains("origin/nowhere"),
        "{err}"
    );
    assert!(
        !Path::new(&repo.root).join(".kanspec").exists(),
        "nothing scaffolded"
    );
}
