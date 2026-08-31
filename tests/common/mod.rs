//! `TestRepo` — the real-half harness. FROZEN, foundation-owned.
//!
//! Builds a real temp git repo with a real bare `origin`, real commits, real worktrees and
//! the six real merge shapes. Determinism comes from exactly three env overrides read in
//! `Ctx::open` — `KANSPEC_NOW`, `KANSPEC_ACTOR` (+ `KANSPEC_ACTOR_KIND`),
//! `KANSPEC_ID_SEED` — plus the one and only mock seam in the crate,
//! `KANSPEC_GH_FIXTURES`. **Git itself is never mocked.**
//!
//! Owner: **F**.

// Each integration test binary pulls in this module and uses a different subset of it;
// `dead_code` here would fire on whatever that binary happens not to call.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

pub mod merges;

/// Frozen clock. Every merge shape and every `## Log` line in the fixtures is dated before
/// it, so `replay`'s monotonicity check has a stable "now" to live under.
pub const NOW: &str = "2026-08-31T12:00:00Z";
pub const ACTOR: &str = "trevor";
pub const ID_SEED: &str = "20260831";

pub struct TestRepo {
    pub root: PathBuf,
    pub origin: PathBuf,
    _tmp: tempfile::TempDir,
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    #[track_caller]
    pub fn ok(self) -> Run {
        assert_eq!(
            self.code, 0,
            "expected success, got {}:\n{}\n{}",
            self.code, self.stdout, self.stderr
        );
        self
    }
}

/// The path of the binary under test. `cargo` sets this for every `[[bin]]`.
fn kanspec_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kanspec")
}

/// Built ONCE per test process, then copied per test.
fn template() -> &'static Path {
    static TEMPLATE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    let (_tmp, path) = TEMPLATE.get_or_init(|| {
        let tmp = tempfile::tempdir().expect("a temp dir");
        build_template(tmp.path());
        let p = tmp.path().to_path_buf();
        (tmp, p)
    });
    path
}

/// `git init` + a bare `origin` + one commit + the `.kanspec/` scaffold.
fn build_template(dir: &Path) {
    let root = dir.join("repo");
    let origin = dir.join("origin.git");
    std::fs::create_dir_all(&root).unwrap();

    git_at(
        dir,
        &[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch=main",
            "origin.git",
        ],
    );
    git_at(dir, &["init", "--quiet", "--initial-branch=main", "repo"]);
    for (k, v) in [
        ("user.email", "test@kanspec.invalid"),
        ("user.name", "kanspec test"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("core.hooksPath", ""),
    ] {
        if v.is_empty() {
            continue;
        }
        git_at(&root, &["config", k, v]);
    }
    git_at(
        &root,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    );

    // A repo with real code in it, so spec `code:` globs have something to match.
    write_at(&root, "README.md", "# fixture\n");
    write_at(
        &root,
        "src/auth/login.ts",
        "export const login = () => {};\n",
    );
    write_at(
        &root,
        "src/billing/charge.ts",
        "export const charge = () => {};\n",
    );

    // `.kanspec/` scaffold. `kanspec init` is S7's; the harness must not wait on it.
    write_at(
        &root,
        ".kanspec/config.toml",
        "main = \"origin/main\"\nid_width = 4\nsync = \"batch\"\n",
    );
    write_at(&root, ".kanspec/.gitignore", "cache/\n");
    for d in ["tickets", "specs", "decisions", "quirks", "proposals"] {
        std::fs::create_dir_all(root.join(".kanspec").join(d)).unwrap();
        write_at(&root, &format!(".kanspec/{d}/.gitkeep"), "");
    }
    std::fs::create_dir_all(root.join(".kanspec/proposals/closed")).unwrap();
    write_at(&root, ".kanspec/proposals/closed/.gitkeep", "");

    git_at(&root, &["add", "-A"]);
    git_at(&root, &["commit", "--quiet", "-m", "genesis"]);
    git_at(&root, &["push", "--quiet", "-u", "origin", "main"]);
    // git >= 2.28 populates origin/HEAD on fetch; do it explicitly so `resolve_main`'s
    // symbolic-ref fallback is exercised on every git the suite might run on.
    git_at(&root, &["remote", "set-head", "origin", "--auto"]);
}

impl Default for TestRepo {
    fn default() -> Self {
        TestRepo::new()
    }
}

impl TestRepo {
    /// Builds the repo ONCE into a process-wide template dir, then copies it per test
    /// (~4ms vs ~30ms for `git init` per test). With nine agents each running the suite on
    /// every save, that is the difference between a fast inner loop and a suite nobody runs.
    pub fn new() -> TestRepo {
        let tmp = tempfile::tempdir().expect("a temp dir");
        copy_dir(template(), tmp.path());
        let root = tmp.path().join("repo");
        let origin = tmp.path().join("origin.git");
        // The copied `.git/config` still names the TEMPLATE's origin by absolute path.
        git_at(
            &root,
            &["remote", "set-url", "origin", &origin.to_string_lossy()],
        );
        TestRepo {
            root,
            origin,
            _tmp: tmp,
        }
    }

    pub fn with_merges() -> TestRepo {
        let repo = TestRepo::new();
        merges::all(&repo);
        repo
    }

    /// A real linked worktree — the fixture that proves worktree unification.
    pub fn worktree(&self, name: &str) -> PathBuf {
        let path = self._tmp.path().join("wt").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // `--no-track` is mandatory: without it a later `git push` from the ticket branch
        // targets main (D-21).
        self.git(&[
            "worktree",
            "add",
            "--no-track",
            "-b",
            &format!("wt/{name}"),
            &path.to_string_lossy(),
            "main",
        ]);
        path
    }

    /// Runs the REAL binary as a child process.
    ///
    /// NOTE (deviation from ARCHITECTURE.md §9, reported): the contract says "in-process
    /// for speed". Every determinism knob kanspec has — `KANSPEC_NOW`, `KANSPEC_ACTOR`,
    /// `KANSPEC_ID_SEED`, `KANSPEC_GH_FIXTURES` — and the cwd itself are *process*-global,
    /// so an in-process harness would make two `cargo test` threads share one clock and one
    /// working directory. A subprocess is the only version of this that is not racy.
    pub fn ks<I, S>(&self, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.ks_in(&self.root.clone(), args)
    }

    pub fn ks_in<I, S>(&self, cwd: &Path, args: I) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.ks_in_env(cwd, args, &[])
    }

    /// `ks_in` with the harness's fixed environment OVERRIDDEN per call.
    ///
    /// ROUND-C ADDITION (integration), reported as a gap by S6. The defaults below pin
    /// `KANSPEC_ACTOR_KIND=human` and strip every agent session variable, which is right for
    /// almost every test — but it also made the AGENT half of **invariant 8** unreachable
    /// from an integration test: there was no way to run the real binary as an agent and
    /// watch `accept` / `revoke` / `supersede` refuse. `HumanActor` is the mechanism the
    /// whole invariant rests on (D-18), so it deserves an end-to-end proof and not only
    /// `ctx.rs`'s unit test. Overrides are applied last, so a caller can replace any default
    /// (pass an empty value to unset one).
    pub fn ks_in_env<I, S>(&self, cwd: &Path, args: I, env: &[(&str, &str)]) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args: Vec<String> = args.into_iter().map(|a| a.as_ref().to_string()).collect();
        let mut cmd = Command::new(kanspec_bin());
        cmd.current_dir(cwd)
            .args(&args)
            .env("KANSPEC_NOW", NOW)
            .env("KANSPEC_ACTOR", ACTOR)
            .env("KANSPEC_ACTOR_KIND", "human")
            .env("KANSPEC_ID_SEED", ID_SEED)
            .env("NO_COLOR", "1")
            .env_remove("CLAUDE_SESSION_ID")
            .env_remove("CURSOR_SESSION_ID")
            .env_remove("CODEX_SESSION_ID")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        if self.gh_fixtures().is_dir() {
            cmd.env("KANSPEC_GH_FIXTURES", self.gh_fixtures());
        }
        for (k, v) in env {
            if v.is_empty() {
                cmd.env_remove(k);
            } else {
                cmd.env(k, v);
            }
        }
        let out = cmd.output().expect("the binary must be runnable");
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    /// `ks` at the primary root, with environment overrides. See [`Self::ks_in_env`].
    pub fn ks_env<I, S>(&self, args: I, env: &[(&str, &str)]) -> Run
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.ks_in_env(&self.root.clone(), args, env)
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> T {
        let mut all: Vec<&str> = args.to_vec();
        all.push("--json");
        let r = self.ks(&all);
        serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
            panic!(
                "`kanspec {}` did not print JSON ({e})\nexit {}\nstdout:\n{}\nstderr:\n{}",
                all.join(" "),
                r.code,
                r.stdout,
                r.stderr
            )
        })
    }

    pub fn write(&self, rel: &str, body: &str) {
        write_at(&self.root, rel, body);
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel))
            .unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }

    pub fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "--quiet", "--allow-empty", "-m", msg]);
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    pub fn push(&self, branch: &str) {
        self.git(&["push", "--quiet", "origin", branch]);
    }

    /// The recorded `gh` JSON that rung 2 replays — the ONE mock seam in the crate.
    pub fn gh_fixture(&self, name: &str, json: &str) {
        let dir = self.gh_fixtures();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.json")), json).unwrap();
    }

    pub fn gh_fixtures(&self) -> PathBuf {
        self._tmp.path().join("gh-fixtures")
    }

    /// `git -C <root> …`, asserting success. Returns stdout.
    #[track_caller]
    pub fn git(&self, args: &[&str]) -> String {
        git_at(&self.root, args)
    }

    /// `git -C <root> …` WITHOUT asserting success — for probing.
    pub fn git_try(&self, args: &[&str]) -> (i32, String, String) {
        let out = base_git(&self.root, args).output().expect("git runs");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    pub fn sha(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev]).trim().to_string()
    }
}

/// An in-process `Ctx` for the layers that have no CLI verb yet — the git wrapper and the
/// store. It goes through the real `Repo::discover` ladder, so worktree unification is
/// exercised rather than bypassed.
///
/// Deliberately does NOT set the determinism env vars: they are process-global, and
/// `cargo test` runs these in parallel threads. Anything needing a frozen clock or a
/// recorded actor goes through [`TestRepo::ks`], which gets its own process.
pub fn ctx_at(cwd: &Path) -> kanspec::ctx::Ctx {
    use clap::Parser as _;
    let cli = kanspec::cli::Cli::try_parse_from(["kanspec", "status"]).expect("a valid argv");
    kanspec::ctx::Ctx::open(&cli, cwd).unwrap_or_else(|e| {
        panic!("Ctx::open({}) failed: {e}", cwd.display());
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────────

fn base_git(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "kanspec test")
        .env("GIT_AUTHOR_EMAIL", "test@kanspec.invalid")
        .env("GIT_COMMITTER_NAME", "kanspec test")
        .env("GIT_COMMITTER_EMAIL", "test@kanspec.invalid")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    c
}

#[track_caller]
pub fn git_at(dir: &Path, args: &[&str]) -> String {
    let out = base_git(dir, args).output().expect("git must be runnable");
    assert!(
        out.status.success(),
        "git {} failed in {} ({}):\n{}",
        args.join(" "),
        dir.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn write_at(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(&p, body).unwrap_or_else(|e| panic!("cannot write {}: {e}", p.display()));
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let ty = entry.file_type().unwrap();
        if ty.is_dir() {
            copy_dir(&src, &dst);
        } else if ty.is_symlink() {
            // git never puts a symlink in `.git`, but a fixture might grow one.
            let target = std::fs::read_link(&src).unwrap();
            #[cfg(unix)]
            let _ = std::os::unix::fs::symlink(target, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}
