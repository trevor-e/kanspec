//! `tests/json_matrix.rs`
//!
//! Proves: **every command supports `--json`** — reads included.
//!
//! DESIGN.md's CLI reference opens with "Every command: `--json`". A prior plan's matrix
//! covered the mutating verbs only, which is exactly backwards: an agent's day is `ready`,
//! `ls`, `show`, `status`, `rules`, `prime` and `board`, and those are the surfaces whose
//! JSON nobody notices is missing until a hook parses a table.
//!
//! The command list is **walked off the clap tree**, not typed here, and the argv table is
//! asserted to cover it exactly — so a new subcommand fails this file rather than slipping
//! past it.
//!
//! Two levels, because they prove different things:
//!
//! 1. **parse** — every leaf command, hidden v0.2 arms included, accepts `--json`;
//! 2. **run** — every v0.1 command actually prints parseable JSON on stdout, whether it
//!    succeeds or refuses. A refusal is `{"ok":false,…}`; an agent that asked for JSON gets
//!    JSON either way, or the flag is a lie.
//!
//! Owner: **S8**.

mod common;

use std::collections::BTreeSet;

use clap::{CommandFactory, Parser};
use common::TestRepo;
use kanspec::cli::Cli;

// ─────────────────────────────────────────────────────────────────────────────
// the tree walk
// ─────────────────────────────────────────────────────────────────────────────

/// Every command a user can actually invoke, as a space-joined path
/// (`"spec new"`, `"quirk add"`). A parent whose subcommand is required is not itself
/// invocable, so it is not a case.
fn leaf_commands() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk(&Cli::command(), "", &mut out);
    out
}

fn walk(cmd: &clap::Command, prefix: &str, out: &mut BTreeSet<String>) {
    for sub in cmd.get_subcommands() {
        let path = if prefix.is_empty() {
            sub.get_name().to_string()
        } else {
            format!("{prefix} {}", sub.get_name())
        };
        let has_subs = sub.get_subcommands().next().is_some();
        if !has_subs || !sub.is_subcommand_required_set() {
            out.insert(path.clone());
        }
        walk(sub, &path, out);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// the argv table — one runnable invocation per leaf command
// ─────────────────────────────────────────────────────────────────────────────

/// What a case needs in the repo before it can run. `{id}` in the argv is replaced with
/// the ticket this level prepared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Needs {
    /// the seeded spec / decision / quirk, and nothing else
    Bare,
    /// a `todo` ticket
    Todo,
    /// a claimed ticket
    Doing,
    /// a ticket in `review`, with a real commit behind it
    Review,
}

/// Why a v0.1 command cannot be *run* here. Every entry is a deliberate exclusion, and the
/// list's own contents are asserted below so it cannot grow quietly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Run {
    Yes,
    /// v0.2: the handler is `todo!()` and would panic, not print JSON
    V02,
    /// blocks in the foreground until Ctrl-C
    Server,
}

struct Case {
    path: &'static str,
    argv: &'static [&'static str],
    needs: Needs,
    run: Run,
}

const fn c(path: &'static str, argv: &'static [&'static str], needs: Needs) -> Case {
    Case {
        path,
        argv,
        needs,
        run: Run::Yes,
    }
}

const fn skip(path: &'static str, argv: &'static [&'static str], run: Run) -> Case {
    Case {
        path,
        argv,
        needs: Needs::Bare,
        run,
    }
}

fn cases() -> Vec<Case> {
    vec![
        // ── SETUP ────────────────────────────────────────────────────────────
        c("init", &["init", "--refresh-hooks"], Needs::Bare),
        c("setup", &["setup", "claude"], Needs::Bare),
        c("doctor", &["doctor"], Needs::Bare),
        c("instructions", &["instructions"], Needs::Bare),
        c("completions", &["completions", "bash"], Needs::Bare),
        // ── TICKETS ──────────────────────────────────────────────────────────
        c("new", &["new", "A fresh ticket", "--spec", "auth"], Needs::Bare),
        c("ready", &["ready"], Needs::Todo),
        c("start", &["start", "{id}"], Needs::Todo),
        c("ship", &["ship", "{id}"], Needs::Doing),
        // The gate refuses (nothing is merged) — and a refusal under `--json` is still
        // JSON, which is the property this file is about.
        c("done", &["done", "{id}", "--no-followups"], Needs::Review),
        c("park", &["park", "{id}", "--why", "matrix"], Needs::Doing),
        c("drop", &["drop", "{id}", "--why", "matrix"], Needs::Todo),
        c("show", &["show", "{id}"], Needs::Todo),
        c("log", &["log", "{id}"], Needs::Todo),
        c("where", &["where"], Needs::Doing),
        c("ls", &["ls", "--all"], Needs::Todo),
        c("repair", &["repair", "{id}", "--why", "matrix"], Needs::Todo),
        // ── STATUS & GIT TRUTH ───────────────────────────────────────────────
        c("status", &["status"], Needs::Todo),
        c("scan", &["scan", "--no-fetch"], Needs::Review),
        c("board", &["board"], Needs::Todo),
        skip("up", &["up"], Run::Server),
        c("open", &["open"], Needs::Bare),
        skip("ci", &["ci"], Run::V02),
        skip("ci why", &["ci", "why", "t-9c41"], Run::V02),
        // ── PROVENANCE & DECISIONS ───────────────────────────────────────────
        c("rules", &["rules"], Needs::Bare),
        c("why", &["why", "auth#jwt"], Needs::Bare),
        c("decide", &["decide", "A matrix decision"], Needs::Bare),
        c("accept", &["accept", "D-8c1a"], Needs::Bare),
        c("supersede", &["supersede", "D-8c1a", "--with", "a newer call"], Needs::Bare),
        c("revoke", &["revoke", "D-8c1a", "--why", "matrix"], Needs::Bare),
        // ── KNOWLEDGE ────────────────────────────────────────────────────────
        c("features", &["features"], Needs::Bare),
        c("spec new", &["spec", "new", "billing", "--code", "src/billing/**"], Needs::Bare),
        c("spec show", &["spec", "show", "auth"], Needs::Bare),
        c("spec grep", &["spec", "grep", "token"], Needs::Bare),
        c("quirk add", &["quirk", "add", "A matrix quirk", "--paths", "src/auth/**"], Needs::Bare),
        c("quirk fix", &["quirk", "fix", "q-11ba", "--by", "{id}"], Needs::Todo),
        c("quirks", &["quirks"], Needs::Bare),
        c("prime", &["prime"], Needs::Todo),
        // ── PROPOSALS & REVIEW (v0.2) ────────────────────────────────────────
        skip("propose", &["propose", "A proposal"], Run::V02),
        skip("review", &["review", "p-7de2"], Run::V02),
        skip("comments", &["comments"], Run::V02),
        skip("comment add", &["comment", "add", "p-7de2#c1", "--body", "x"], Run::V02),
        skip("comment reply", &["comment", "reply", "cm-1111", "--body", "x"], Run::V02),
        skip("comment resolve", &["comment", "resolve", "cm-1111", "--note", "x"], Run::V02),
        skip("approve", &["approve", "p-7de2"], Run::V02),
        skip("close", &["close", "p-7de2"], Run::V02),
        skip("abandon", &["abandon", "p-7de2", "--why", "x"], Run::V02),
        skip("promote", &["promote", "p-7de2#p1", "--as", "decision"], Run::V02),
        skip("expire", &["expire", "p-7de2#p2", "--reason", "x"], Run::V02),
        skip("landcheck", &["landcheck"], Run::V02),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// 0. the table and the tree agree
// ─────────────────────────────────────────────────────────────────────────────

/// The point of walking the tree: a command added to `cli.rs` without a case here fails
/// **this** test, by name, instead of quietly never being checked for `--json`.
#[test]
fn the_matrix_covers_every_command_in_the_clap_tree() {
    let tree = leaf_commands();
    let table: BTreeSet<String> = cases().iter().map(|c| c.path.to_string()).collect();

    let missing: Vec<&String> = tree.difference(&table).collect();
    let extra: Vec<&String> = table.difference(&tree).collect();
    assert!(
        missing.is_empty(),
        "these commands exist in the CLI but have no --json case:\n  {}",
        missing
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    assert!(
        extra.is_empty(),
        "these cases name a command the CLI does not have:\n  {}",
        extra
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    assert!(tree.len() > 40, "the command tree shrank: {}", tree.len());
}

/// The only commands excluded from the RUN matrix are the v0.2 arms (whose handlers are
/// `todo!()`) and `up` (which blocks until Ctrl-C). Asserting the list's shape is what
/// stops a v0.1 command being parked here to make a failure go away.
#[test]
fn nothing_is_excluded_from_the_run_matrix_without_a_reason() {
    let cli = Cli::command();
    let hidden: BTreeSet<&str> = cli
        .get_subcommands()
        .filter(|s| s.is_hide_set())
        .map(|s| s.get_name())
        .collect();

    for case in cases() {
        let top = case.path.split(' ').next().unwrap();
        match case.run {
            Run::Yes => assert!(
                !hidden.contains(top),
                "{} is a hidden v0.2 arm but the matrix tries to run it",
                case.path
            ),
            Run::V02 => assert!(
                hidden.contains(top),
                "{} is excluded as v0.2 but the CLI ships it as v0.1",
                case.path
            ),
            Run::Server => assert_eq!(
                case.path, "up",
                "`up` is the only command that blocks the terminal"
            ),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. parse — the flag reaches every arm
// ─────────────────────────────────────────────────────────────────────────────

/// `--json` is a global, but "global" is a clap setting somebody could drop. This asserts
/// the observable property: the parser accepts it after every command, hidden ones too,
/// and it lands on `Cli::json`.
#[test]
fn every_command_accepts_the_json_flag() {
    for case in cases() {
        let mut argv: Vec<&str> = vec!["kanspec"];
        argv.extend_from_slice(case.argv);
        argv.push("--json");
        let cli = Cli::try_parse_from(&argv)
            .unwrap_or_else(|e| panic!("`{}` does not parse with --json:\n{e}", argv.join(" ")));
        assert!(cli.json, "--json did not reach Cli::json for {}", case.path);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. run — every v0.1 command prints parseable JSON, success or refusal
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn every_v01_command_prints_json_on_stdout() {
    for case in cases() {
        if case.run != Run::Yes {
            continue;
        }
        let repo = TestRepo::new();
        seed(&repo);
        let id = prepare(&repo, case.needs);

        let argv: Vec<String> = case
            .argv
            .iter()
            .map(|a| a.replace("{id}", &id))
            .chain(std::iter::once("--json".to_string()))
            .collect();
        let r = repo.ks_env(&argv, &[("KANSPEC_NO_BROWSER", "1")]);

        // Exit code is not the assertion — a refusal is a legitimate outcome for `done`
        // against an unmerged ticket. The assertion is that stdout is machine-readable.
        assert!(
            !r.stdout.trim().is_empty(),
            "`kanspec {}` printed nothing on stdout (exit {})\nstderr:\n{}",
            argv.join(" "),
            r.code,
            r.stderr
        );
        let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
            panic!(
                "`kanspec {}` did not print JSON under --json ({e})\nexit {}\nstdout:\n{}\nstderr:\n{}",
                argv.join(" "),
                r.code,
                r.stdout,
                r.stderr
            )
        });
        assert!(
            v.is_object() || v.is_array(),
            "`kanspec {}` printed a bare scalar: {v}",
            argv.join(" ")
        );
        // Invariant 9, on the JSON surface: a refusal carries its one-command fix.
        if v.get("ok") == Some(&serde_json::Value::Bool(false)) {
            let fixes = v["error"]["fix"].as_array().unwrap_or_else(|| {
                panic!("`kanspec {}` refused without a fix list: {v}", argv.join(" "))
            });
            assert!(
                !fixes.is_empty(),
                "`kanspec {}` refused with an empty fix list",
                argv.join(" ")
            );
        }
    }
}

/// The read commands, spelled out by name because they are the ones a prior plan's matrix
/// left out — and the ones an agent runs every session.
#[test]
fn every_read_command_prints_json_including_the_ones_a_mutation_matrix_would_miss() {
    let repo = TestRepo::new();
    seed(&repo);
    let id = prepare(&repo, Needs::Todo);

    for argv in [
        vec!["ready"],
        vec!["ls", "--all"],
        vec!["ls", "--spec", "auth"],
        vec!["show", id.as_str()],
        vec!["log", id.as_str()],
        vec!["status"],
        vec!["rules"],
        vec!["prime"],
        vec!["board"],
        vec!["features"],
        vec!["quirks"],
        vec!["spec", "show", "auth"],
        vec!["instructions"],
    ] {
        let v: serde_json::Value = {
            let mut all = argv.clone();
            all.push("--json");
            let r = repo.ks(&all);
            assert_eq!(
                r.code,
                0,
                "`kanspec {}` is a READ and must succeed:\n{}",
                argv.join(" "),
                r.stderr
            );
            serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
                panic!(
                    "`kanspec {}` did not print JSON ({e}):\n{}",
                    argv.join(" "),
                    r.stdout
                )
            })
        };
        assert!(v.is_object(), "`kanspec {}` -> {v}", argv.join(" "));
    }
}

/// `--json` must not leak the human rendering onto stdout beside the object — an agent
/// pipes stdout straight into a parser.
#[test]
fn json_mode_prints_exactly_one_object_and_nothing_else() {
    let repo = TestRepo::new();
    seed(&repo);
    let id = prepare(&repo, Needs::Todo);

    for argv in [vec!["show", id.as_str()], vec!["board"], vec!["status"]] {
        let mut all = argv.clone();
        all.push("--json");
        let out = repo.ks(&all).ok().stdout;
        assert!(out.trim_start().starts_with('{'), "{out}");
        assert!(out.trim_end().ends_with('}'), "{out}");
        // One value, not a value plus a banner: a second parse of the tail must find
        // nothing left over.
        let mut de = serde_json::Deserializer::from_str(&out).into_iter::<serde_json::Value>();
        assert!(de.next().is_some(), "{out}");
        assert!(
            de.next().is_none(),
            "`kanspec {} --json` printed more than one value:\n{out}",
            argv.join(" ")
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// fixture
// ─────────────────────────────────────────────────────────────────────────────

fn seed(repo: &TestRepo) {
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login sessions and lockout\ncode: [src/auth/**]\n---\n\
         # auth\n\n## Rules\n- [auth.jwt] Login issues a signed token. {p-02cc}\n",
    );
    repo.write(
        ".kanspec/decisions/D-8c1a.md",
        "---\nid: D-8c1a\ntitle: Rate-limit state lives in Redis only\nstatus: proposed\n\
         date: 2026-08-20\nsource: null\nscope: [src/auth/**]\nsupersedes: null\n\
         superseded_by: null\n---\n## Decision\nSliding window in Redis.\n",
    );
    repo.write(
        ".kanspec/quirks/q-11ba.md",
        "---\nid: q-11ba\ntitle: Sessions rotate on every redeploy\npaths: [src/auth/**]\n\
         severity: landmine\nstatus: active\nsource: null\nfixed_by: null\n---\nBody.\n",
    );
    repo.git(&["add", "-A", "--", ".kanspec"]);
    repo.git(&["commit", "--quiet", "-m", "knowledge"]);
    repo.push("main");
}

/// Returns the ticket id the case's `{id}` placeholder refers to.
fn prepare(repo: &TestRepo, needs: Needs) -> String {
    if needs == Needs::Bare {
        return "t-0000".to_string();
    }
    let created = repo.json::<serde_json::Value>(&["new", "Rate-limit login", "--spec", "auth"]);
    let id = created["id"].as_str().expect("`new` reports its id").to_string();
    if needs == Needs::Todo {
        return id;
    }

    repo.ks(["start", &id]).ok();
    if needs == Needs::Doing {
        return id;
    }

    // `ship` refuses a branch that is zero commits ahead of main (D-35): without a real
    // commit, `head:` would be main's own SHA and rung 1 would answer MERGED for work that
    // never happened.
    repo.write(&format!("src/auth/{id}.ts"), "export const a = 1;\n");
    repo.git(&["add", "-A", "--", "src"]);
    repo.git(&[
        "commit",
        "--quiet",
        "-m",
        &format!("{id}: the work\n\nKanspec: {id}\n"),
    ]);
    repo.ks(["ship", &id, "--pr", "7"]).ok();
    id
}
