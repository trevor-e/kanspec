//! Shells the REAL binary: argv, exit codes, and the `ks` alias — the wiring the
//! in-process `TestRepo::ks` path skips.
//!
//! Owner: **F**.

use std::process::Command;

fn bin(name: &str) -> std::path::PathBuf {
    // `CARGO_BIN_EXE_<name>` is set by cargo for every `[[bin]]` in the package.
    match name {
        "kanspec" => env!("CARGO_BIN_EXE_kanspec").into(),
        "ks" => env!("CARGO_BIN_EXE_ks").into(),
        other => panic!("no such bin: {other}"),
    }
}

fn run(name: &str, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin(name))
        .args(args)
        .output()
        .expect("the binary must be runnable");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn help_prints_the_full_command_surface_and_exits_zero() {
    let (code, stdout, _) = run("kanspec", &["--help"]);
    assert_eq!(code, 0);
    for verb in [
        "init", "new", "ready", "start", "ship", "done", "status", "scan", "board", "up", "rules",
        "prime", "doctor", "quirk", "spec", "features", "repair",
    ] {
        assert!(
            stdout.contains(verb),
            "`--help` never mentions `{verb}`:\n{stdout}"
        );
    }
    // v0.2 is hidden until its bodies land.
    assert!(
        !stdout.contains("landcheck"),
        "v0.2 verbs must be hidden:\n{stdout}"
    );
}

#[test]
fn every_subcommand_help_subtree_works() {
    for verb in [
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
        // hidden, but `--help` on a named v0.2 verb still has to work
        "propose",
        "comment",
        "close",
        "landcheck",
    ] {
        let (code, stdout, stderr) = run("kanspec", &[verb, "--help"]);
        assert_eq!(code, 0, "`kanspec {verb} --help` exited {code}: {stderr}");
        assert!(
            stdout.contains("Usage:"),
            "`kanspec {verb} --help` printed no usage block"
        );
    }
}

#[test]
fn the_ks_alias_is_the_same_binary_under_its_own_name() {
    let (code, stdout, _) = run("ks", &["--help"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("Usage: ks"),
        "`ks --help` must name itself `ks`:\n{stdout}"
    );
}

#[test]
fn version_exits_zero() {
    for name in ["kanspec", "ks"] {
        let (code, stdout, _) = run(name, &["--version"]);
        assert_eq!(code, 0);
        assert!(stdout.contains(env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn a_bare_invocation_exits_64_not_0() {
    // D-16: an agent that runs an incomplete command must not think it succeeded.
    for args in [&[][..], &["comment"][..], &["spec"][..], &["quirk"][..]] {
        let (code, _, _) = run("kanspec", args);
        assert_eq!(
            code,
            64,
            "`kanspec {}` should exit 64, got {code}",
            args.join(" ")
        );
    }
}

#[test]
fn a_typoed_flag_is_remapped_from_claps_2_to_64() {
    // Clap's native exit code 2 is REMAPPED, because 2 is reachable from exactly one
    // module in this crate — `cmd::landcheck`, sealed by `BlockToken`.
    let (code, _, stderr) = run("kanspec", &["status", "--nonsuch"]);
    assert_eq!(code, 64, "got {code}: {stderr}");
    let (code, _, _) = run("kanspec", &["nonsuch-command"]);
    assert_eq!(code, 64);
}
