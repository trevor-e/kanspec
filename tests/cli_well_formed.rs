//! `Cli::command().debug_assert()` — **not optional**: `global + required` is a
//! debug-only assert that is compiled out of release, so without this test a malformed
//! clap tree ships silently.
//!
//! Owner: **F**.

use clap::CommandFactory;
use kanspec::cli::{Cli, Command};

#[test]
fn cli_tree_is_well_formed() {
    Cli::command().debug_assert();
}

#[test]
fn every_command_has_an_about_line() {
    for sub in Cli::command().get_subcommands() {
        assert!(
            sub.get_about().is_some(),
            "`{}` has no about line — `--help` must read well",
            sub.get_name()
        );
    }
}

#[test]
fn every_positional_and_flag_has_help() {
    fn check(cmd: &clap::Command) {
        for arg in cmd.get_arguments() {
            assert!(
                arg.get_help().is_some() || arg.get_long_help().is_some(),
                "`{} {}` has no help text — `--help` must read well",
                cmd.get_name(),
                arg.get_id()
            );
        }
        for sub in cmd.get_subcommands() {
            check(sub);
        }
    }
    // The auto-generated --help/--version args carry their own text.
    check(&Cli::command());
}

/// Global args reach subcommands only once the tree is built.
fn built() -> clap::Command {
    let mut cmd = Cli::command();
    cmd.build();
    cmd
}

#[test]
fn the_v01_surface_is_visible_and_the_v02_surface_is_hidden() {
    let cmd = Cli::command();
    let hidden: Vec<&str> = cmd
        .get_subcommands()
        .filter(|s| s.is_hide_set())
        .map(|s| s.get_name())
        .collect();
    for v02 in [
        "propose",
        "review",
        "comments",
        "comment",
        "approve",
        "close",
        "abandon",
        "promote",
        "expire",
        "landcheck",
        "ci",
    ] {
        assert!(hidden.contains(&v02), "`{v02}` is v0.2 and must be hidden");
    }
    for v01 in [
        "init",
        "setup",
        "doctor",
        "new",
        "ready",
        "start",
        "ship",
        "done",
        "park",
        "drop",
        "show",
        "log",
        "where",
        "ls",
        "repair",
        "status",
        "scan",
        "board",
        "up",
        "open",
        "rules",
        "why",
        "decide",
        "accept",
        "supersede",
        "revoke",
        "features",
        "spec",
        "quirk",
        "quirks",
        "prime",
        "instructions",
        "completions",
    ] {
        assert!(
            !hidden.contains(&v01),
            "`{v01}` is v0.1 and must be visible"
        );
        assert!(
            cmd.get_subcommands().any(|s| s.get_name() == v01),
            "`{v01}` is missing from the CLI tree"
        );
    }
}

#[test]
fn every_command_accepts_the_global_json_flag() {
    // Walked mechanically off the finished clap tree, so a command added without --json
    // fails here rather than in an agent's session.
    fn leaves(cmd: &clap::Command, path: Vec<String>, out: &mut Vec<Vec<String>>) {
        let mut subs = cmd.get_subcommands().peekable();
        if subs.peek().is_none() {
            out.push(path);
            return;
        }
        for sub in cmd.get_subcommands() {
            let mut p = path.clone();
            p.push(sub.get_name().to_string());
            leaves(sub, p, out);
        }
    }
    let cmd = built();
    let mut paths = Vec::new();
    leaves(&cmd, Vec::new(), &mut paths);
    assert!(
        paths.len() > 30,
        "expected the full surface, got {}",
        paths.len()
    );

    for p in paths {
        let sub = p.iter().fold(&cmd, |c, name| {
            c.get_subcommands()
                .find(|s| s.get_name() == name)
                .unwrap_or_else(|| panic!("missing subcommand {name}"))
        });
        for global in ["json", "color", "repo"] {
            assert!(
                sub.get_arguments().any(|a| a.get_id() == global),
                "`kanspec {}` does not accept --{global}",
                p.join(" ")
            );
        }
    }
}

#[test]
fn the_dispatch_arms_and_the_clap_tree_agree() {
    // `Command` is matched exhaustively in `dispatch`, so this only has to pin the arity;
    // the compiler proves every arm exists. Bumping it is a deliberate act, batched
    // between waves as a request to F.
    let n = Cli::command().get_subcommands().count();
    assert_eq!(
        n, 44,
        "the CLI surface changed — update `dispatch` and this count together"
    );
    let _ = std::mem::size_of::<Command>();
}

// ─────────────────────────────────────────────────────────────────────────────
// Every advertised command is a command the CLI accepts
// ─────────────────────────────────────────────────────────────────────────────
//
// NON-NEGOTIABLE: every `KsError` names its one-command fix, and that fix must be a REAL
// RUNNABLE command. `tests/transition_table.rs` proves it for the fixes the TRANSITION
// layer mints — which is where the rule was first broken — but a fix raised anywhere else
// in the crate was never parsed by anything, and three had rotted:
//
//     src/git.rs  `kanspec where {branch}`       — `where` takes `--branch`, never a positional
//     src/git.rs  `kanspec start --no-worktree`  — no such flag; a worktree is opt-in via `--worktree`
//     src/git.rs  `kanspec start --no-worktree`  — again, in the sibling refusal
//
// All three sat a dozen lines from a `cmd/flow.rs` refusal that spells the same advice
// correctly. That is how a hand-written command string rots: nothing ever reads it.

/// The Rust string literal of every `fix!("…")` under `src/`, un-escaped, with its site.
/// Comment lines are skipped: the doc comments on `Fix::cmd` and `out::spoken` quote
/// `fix!("kanspec …")` as PROSE, and an ellipsis is not a command.
fn fix_literals() -> Vec<(String, usize, String)> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, usize, String)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
                continue;
            }
            if p.extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            for (i, line) in std::fs::read_to_string(&p).unwrap().lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let mut at = 0usize;
                while let Some(k) = line[at..].find("fix!(\"") {
                    let mut s = String::new();
                    let mut it = line[at + k + 6..].chars();
                    loop {
                        match it.next() {
                            Some('\\') => match it.next() {
                                Some('"') => s.push('"'),
                                Some('\\') => s.push('\\'),
                                Some(c) => {
                                    s.push('\\');
                                    s.push(c);
                                }
                                None => break,
                            },
                            Some('"') | None => break,
                            Some(c) => s.push(c),
                        }
                    }
                    out.push((rel.clone(), i + 1, s));
                    at += k + 6;
                }
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort();
    assert!(out.len() > 100, "the fix corpus shrank to {}", out.len());
    out
}

/// `"a b"` is one argument.
fn argv(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in cmd.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            ' ' if !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            _ => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What each `{…}`/`<…>` hole stands for, decided by the flag it follows. A fix string is
/// a TEMPLATE: this guard is about the shape clap sees, so the substitution only has to be
/// well-typed, never realistic.
fn concrete(cmd: &str) -> Vec<String> {
    let mut v = argv(cmd);
    for i in 0..v.len() {
        let hole = (v[i].starts_with('{') && v[i].ends_with('}'))
            || (v[i].starts_with('<') && v[i].ends_with('>'));
        if !hole {
            continue;
        }
        let prev = if i == 0 { "" } else { v[i - 1].as_str() };
        v[i] = match prev {
            "--port" | "--pr" | "--actually-done" => "1",
            "--branch" => "ks/t-9c41-slug",
            "--repo" => ".",
            // the subcommand slot itself must be a VERB, and one that takes the `--why`
            // that follows it in `kanspec {verb} {id} --why "…"`
            _ if i == 1 => "park",
            _ => "t-9c41",
        }
        .to_string();
    }
    v
}

#[test]
fn the_placeholder_substitution_this_guard_leans_on_actually_works() {
    assert_eq!(
        argv("kanspec park t-1 --why \"a b\""),
        ["kanspec", "park", "t-1", "--why", "a b"]
    );
    assert_eq!(argv("kanspec new \"\""), ["kanspec", "new", ""]);
    assert_eq!(
        concrete("kanspec up --port {}"),
        ["kanspec", "up", "--port", "1"]
    );
    assert_eq!(concrete("kanspec show {id}"), ["kanspec", "show", "t-9c41"]);
    assert_eq!(
        concrete("kanspec where --branch {branch}"),
        ["kanspec", "where", "--branch", "ks/t-9c41-slug"]
    );
    assert_eq!(concrete("kanspec {verb} {id} --why \"x\"")[1], "park");
}

#[test]
fn every_fix_that_names_kanspec_is_a_command_the_real_cli_accepts() {
    use clap::Parser;
    let mut broken: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for (file, line, lit) in fix_literals() {
        // Only the fixes that invoke THIS binary. `git …`, `open …`, `rm -rf …` and the
        // prose remediations are advice clap has no opinion about.
        if !lit.starts_with("kanspec ") {
            continue;
        }
        checked += 1;
        if let Err(e) = Cli::try_parse_from(concrete(&lit)) {
            let first = e.to_string().lines().next().unwrap_or("").to_string();
            broken.push(format!("{file}:{line}: {lit}\n      clap: {first}"));
        }
    }
    assert!(
        checked > 40,
        "only {checked} kanspec fixes were parsed — the extractor broke"
    );
    assert!(
        broken.is_empty(),
        "a refusal offered a command the CLI does not accept:\n  {}",
        broken.join("\n  ")
    );
}
