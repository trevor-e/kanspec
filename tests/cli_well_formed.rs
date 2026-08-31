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
