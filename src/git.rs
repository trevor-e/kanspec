//! `git -C <primary_root>` shell-out. No libgit2, no gitoxide, no `trait GitBackend` —
//! exact behavioural parity with the user's git, and the second implementation does not
//! exist.
//!
//! Universal invocation rules, each verified by recon:
//! always `git -C <primary_root>`; always scrub `GIT_DIR`/`GIT_WORK_TREE`/`GIT_INDEX_FILE`
//! from the child env (git sets `GIT_DIR` for hooks, and env beats `-C`); always
//! `--path-format=absolute`; every pathspec through [`Pathspec`]; never trust exit code
//! alone for `log`/`cherry` (both exit 0 on no matches); **exit 128 is always
//! [`Tri::Unknown`], never [`Tri::No`]**.
//!
//! Owner: **S2**.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{EnvCode, KsError, Result};
use crate::ids::TicketId;
use crate::{fix, fixes};

// ─────────────────────────────────────────────────────────────────────────────
// The SHA seal
// ─────────────────────────────────────────────────────────────────────────────

/// Private field; the ONLY constructor is [`Sha::mint`], private to THIS file, and every
/// call site parses real git stdout. `head:` is therefore provably read from git and can
/// never be agent-typed. `Serialize` yes; **`Deserialize` never**, so a cache entry cannot
/// mint one.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha(String);

impl Sha {
    /// PRIVATE — accepts >= 7 lowercase hex and nothing else.
    fn mint(s: &str) -> Option<Sha> {
        let s = s.trim();
        let ok = s.len() >= 7
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        ok.then(|| Sha(s.to_string()))
    }
    /// 7 chars — what a badge shows.
    pub fn short(&self) -> &str {
        &self.0[..7.min(self.0.len())]
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Sha {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A branch tip, distinguished from an arbitrary commit so `ship`/`done` cannot record
/// something that was never a HEAD.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct HeadSha(Sha);

impl HeadSha {
    pub fn sha(&self) -> &Sha {
        &self.0
    }
}

/// Always emits `:(glob,top)`. `Git` will not accept a bare `&str` where a `Pathspec` is
/// expected, so recon finding 6 — pathspecs are cwd-relative, and `glob` makes `**` cross
/// separators exactly like globset — cannot be forgotten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pathspec(String);

impl Pathspec {
    pub fn glob(g: &str) -> Pathspec {
        Pathspec(format!(":(glob,top){g}"))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tri / Unknown — "unknown" is a VALUE, not an error path
// ─────────────────────────────────────────────────────────────────────────────

/// Every consumer must destructure it, so no code path can quietly fold a git failure
/// into "not merged" (invariant 2).
#[derive(Clone, Debug)]
pub enum Tri<T> {
    Yes(T),
    No,
    Unknown(Unknown),
}

/// One variant per decline reason, exhaustively matched, each carrying the fields its
/// badge text needs.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Unknown {
    NoHead,
    ZeroCommitBranch,
    HeadNotInObjectStore {
        sha: String,
    },
    GitFailed {
        cmd: String,
        code: i32,
        stderr: String,
    },
    GhUnavailable {
        why: String,
    },
    SquashSuspectedNoGh {
        plus_lines: usize,
    },
    GhMergedButNotAncestor {
        merge_sha: String,
    },
    FetchStale {
        age_secs: u64,
    },
    ConflictingSignals {
        rungs: Vec<RungTrace>,
    },
}

impl Unknown {
    /// `"unknown (squash suspected, no gh)"` — what the card actually shows.
    pub fn badge(&self) -> String {
        let why = match self {
            Unknown::NoHead => "no branch or head SHA recorded".to_string(),
            Unknown::ZeroCommitBranch => "branch has no commits of its own".to_string(),
            Unknown::HeadNotInObjectStore { sha } => format!("{sha} is not in the object store"),
            Unknown::GitFailed { code, .. } => format!("git exited {code}"),
            Unknown::GhUnavailable { why } => format!("gh unavailable: {why}"),
            Unknown::SquashSuspectedNoGh { .. } => "squash suspected, no gh".to_string(),
            Unknown::GhMergedButNotAncestor { merge_sha } => {
                format!("gh says merged, but {merge_sha} is not on main")
            }
            Unknown::FetchStale { age_secs } => format!("fetch is {age_secs}s stale"),
            Unknown::ConflictingSignals { .. } => "signals conflict".to_string(),
        };
        format!("unknown ({why})")
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct RungTrace {
    pub method: Method,
    pub cmd: String,
    pub exit: i32,
    pub saw: String,
    pub verdict: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Ancestry,
    GhPr,
    Trailer,
    PatchId,
    HumanConfirm,
    None,
}

impl Method {
    pub const fn as_str(self) -> &'static str {
        match self {
            Method::Ancestry => "ancestry",
            Method::GhPr => "gh-pr",
            Method::Trailer => "trailer",
            Method::PatchId => "patch-id",
            Method::HumanConfirm => "confirmed",
            Method::None => "-",
        }
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The wrapper
// ─────────────────────────────────────────────────────────────────────────────

/// ALWAYS `git -C <primary_root>`.
pub struct Git {
    root: PathBuf,
}

pub struct GitOut {
    pub code: i32,
    pub out: String,
    pub err: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChangedPath {
    pub status: char,
    pub path: String,
    pub renamed_from: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CherryLine {
    /// `-` in `git cherry` output: this patch is already upstream.
    pub upstream: bool,
    pub sha: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WorktreeRow {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}

impl Git {
    /// Bound once by `Ctx::open`; there is no other way to get one.
    pub(crate) fn bind(root: &Path) -> Git {
        Git {
            root: root.to_path_buf(),
        }
    }

    /// The primary worktree every invocation is anchored to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// ALWAYS scrubs `GIT_DIR` / `GIT_WORK_TREE` / `GIT_INDEX_FILE` from the child env.
    /// `Err` only when git is not runnable at all; a non-zero exit is a normal `GitOut`.
    pub fn run(&self, args: &[&str]) -> Result<GitOut> {
        self.run_env(args, &[])
    }

    /// `run`, plus environment the caller needs. Private: the scrub list is not optional.
    fn run_env(&self, args: &[&str], extra: &[(&str, &str)]) -> Result<GitOut> {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.root).args(args);
        // git sets GIT_DIR when it runs a hook, and the environment beats `-C`: without
        // this scrub a hook-invoked `kanspec scan` reads the LINKED worktree's git dir
        // while writing the PRIMARY worktree's files (recon finding 0).
        cmd.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = cmd.output().map_err(|e| {
            KsError::environment(
                EnvCode::GitMissing,
                format!("cannot run `git`: {e}"),
                fixes![fix!("install git and re-run")],
            )
        })?;
        Ok(GitOut {
            code: out.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&out.stdout).into_owned(),
            err: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    pub fn run_ps(&self, args: &[&str], ps: &[Pathspec]) -> Result<GitOut> {
        let mut all: Vec<&str> = args.to_vec();
        all.push("--");
        all.extend(ps.iter().map(Pathspec::as_str));
        self.run(&all)
    }

    pub fn head_sha(&self, rev: &str) -> Result<HeadSha> {
        let spec = format!("{rev}^{{commit}}");
        let o = self.run(&["rev-parse", "--verify", &spec])?;
        Sha::mint(o.out.trim()).map(HeadSha).ok_or_else(|| {
            self.git_err(
                format!("cannot resolve `{rev}` to a commit"),
                format!("git rev-parse --verify {spec}"),
                o.code,
                &o.err,
            )
        })
    }

    /// `symbolic-ref`; `None` means detached HEAD.
    pub fn current_branch(&self) -> Option<String> {
        // NOT `rev-parse --abbrev-ref HEAD`: that prints the literal string `HEAD` when
        // detached, which is ambiguous with a branch actually named `HEAD`.
        let o = self
            .run(&["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()?;
        (o.code == 0)
            .then(|| o.out.trim().to_string())
            .filter(|b| !b.is_empty())
    }

    pub fn object_exists(&self, s: &Sha) -> bool {
        let spec = format!("{}^{{commit}}", s.as_str());
        self.run(&["rev-parse", "--verify", "--quiet", &spec])
            .map(|o| o.code == 0)
            .unwrap_or(false)
    }

    /// The configured `main`, or `symbolic-ref refs/remotes/origin/HEAD` when it does not
    /// resolve. Never guesses silently: the fallbacks are ordered and the failure names
    /// the config key.
    pub fn resolve_main(&self, cfg: &str) -> Result<String> {
        if !cfg.trim().is_empty() && self.rev_resolves(cfg) {
            return Ok(cfg.trim().to_string());
        }
        // git >= 2.28 populates origin/HEAD on the first fetch; older git and a manually
        // deleted ref both land on the candidate list below.
        if let Ok(o) = self.run(&[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ]) {
            if o.code == 0 {
                let r = o.out.trim().to_string();
                if !r.is_empty() && self.rev_resolves(&r) {
                    return Ok(r);
                }
            }
        }
        for cand in ["origin/main", "origin/master", "main", "master"] {
            if self.rev_resolves(cand) {
                return Ok(cand.to_string());
            }
        }
        Err(KsError::environment(
            EnvCode::NotARepo,
            format!("cannot resolve a main branch (`{cfg}` does not exist, and neither does origin/HEAD)"),
            fixes![
                fix!("git fetch origin"),
                fix!("set `main = \"origin/<branch>\"` in .kanspec/config.toml"),
            ],
        ))
    }

    fn rev_resolves(&self, rev: &str) -> bool {
        let spec = format!("{rev}^{{commit}}");
        self.run(&["rev-parse", "--verify", "--quiet", &spec])
            .map(|o| o.code == 0)
            .unwrap_or(false)
    }

    /// 0=Yes 1=No 128=Unknown. Ancestry-NEGATIVE is `No` HERE; it is the *ladder* that
    /// downgrades it to inconclusive, because a squash-merged branch is genuinely not an
    /// ancestor.
    pub fn is_ancestor(&self, s: &Sha, base: &str) -> Tri<()> {
        let cmd = format!("git merge-base --is-ancestor {} {base}", s.as_str());
        match self.run(&["merge-base", "--is-ancestor", s.as_str(), base]) {
            Err(e) => Tri::Unknown(Unknown::GitFailed {
                cmd,
                code: -1,
                stderr: e.to_string(),
            }),
            Ok(o) => match o.code {
                0 => Tri::Yes(()),
                1 => Tri::No,
                // 128 = "git could not answer": a gc'd head, a missing base ref, a tree
                // where a commit was expected. NEVER "not merged" (invariant 2).
                code => Tri::Unknown(Unknown::GitFailed {
                    cmd,
                    code,
                    stderr: o.err.trim().to_string(),
                }),
            },
        }
    }

    /// The zero-commit guard: a fresh `start` branch is trivially an ancestor of main, so
    /// without this rung 1 reports MERGED for work that never happened.
    pub fn commits_ahead(&self, base: &str, head: &Sha) -> Tri<u32> {
        let range = format!("{base}..{}", head.as_str());
        let cmd = format!("git rev-list --count {range}");
        match self.run(&["rev-list", "--count", &range]) {
            Err(e) => Tri::Unknown(git_failed(cmd, -1, &e.to_string())),
            Ok(o) if o.code == 0 => match o.out.trim().parse::<u32>() {
                Ok(n) => Tri::Yes(n),
                Err(_) => Tri::Unknown(git_failed(cmd, 0, &o.out)),
            },
            Ok(o) => Tri::Unknown(git_failed(cmd, o.code, &o.err)),
        }
    }

    /// UNANCHORED (git indents squash-body trailers 4 spaces) and boundary-terminated
    /// (ids are 4 hex; `t-9c4` would match `t-9c41`).
    pub fn grep_trailer(&self, base: &str, id: &TicketId) -> Tri<Vec<Sha>> {
        let pat = format!("Kanspec: {}([^0-9a-f]|$)", id.as_str());
        let cmd = format!("git log {base} -E --grep '{pat}' --format=%H");
        // `log` exits 0 with empty stdout when nothing matches, so the exit code alone is
        // never the answer; scope to `base` and never `--all`, which would match the
        // branch's own commits.
        match self.run(&["log", base, "-E", "--grep", &pat, "--format=%H"]) {
            Err(e) => Tri::Unknown(git_failed(cmd, -1, &e.to_string())),
            Ok(o) if o.code == 0 => Tri::Yes(o.out.lines().filter_map(Sha::mint).collect()),
            Ok(o) => Tri::Unknown(git_failed(cmd, o.code, &o.err)),
        }
    }

    /// `-` means an equivalent patch is already upstream; `+` means it is not — and a `+`
    /// cannot tell an unmerged branch apart from a multi-commit squash, which is why the
    /// ladder maps it to `Unknown` rather than `NotMerged` (D-3).
    pub fn cherry(&self, base: &str, head: &Sha) -> Tri<Vec<CherryLine>> {
        let cmd = format!("git cherry {base} {}", head.as_str());
        match self.run(&["cherry", base, head.as_str()]) {
            Err(e) => Tri::Unknown(git_failed(cmd, -1, &e.to_string())),
            Ok(o) if o.code == 0 => Tri::Yes(
                o.out
                    .lines()
                    .filter_map(|l| {
                        let mut it = l.split_whitespace();
                        let sign = it.next()?;
                        let sha = it.next()?.to_string();
                        Some(CherryLine {
                            upstream: sign == "-",
                            sha,
                        })
                    })
                    .collect(),
            ),
            Ok(o) => Tri::Unknown(git_failed(cmd, o.code, &o.err)),
        }
    }

    /// 3-dot, `-M`, `-z`. Two dots would leak `base`'s own changes (recon measured 6 files
    /// against 1); `-M` keeps a rename distinguishable from a delete plus an add, which
    /// matters because a spec glob matching a DELETED path must not count as "touched".
    pub fn changed_paths(&self, base: &str, head: &str) -> Tri<Vec<ChangedPath>> {
        let range = format!("{base}...{head}");
        let cmd = format!("git diff --name-status -M -z {range}");
        let o = match self.run(&["diff", "--name-status", "-M", "-z", &range]) {
            Err(e) => return Tri::Unknown(git_failed(cmd, -1, &e.to_string())),
            Ok(o) => o,
        };
        if o.code != 0 {
            // An unborn branch and a gc'd ref both land here as 128 — "cannot answer".
            return Tri::Unknown(git_failed(cmd, o.code, &o.err));
        }
        let mut out = Vec::new();
        let mut it = o.out.split('\0').filter(|f| !f.is_empty());
        while let Some(status) = it.next() {
            let letter = status.chars().next().unwrap_or('?');
            if matches!(letter, 'R' | 'C') {
                // With -z a rename is three fields: `R100`, source, destination.
                let (Some(from), Some(to)) = (it.next(), it.next()) else {
                    break;
                };
                out.push(ChangedPath {
                    status: letter,
                    path: to.to_string(),
                    renamed_from: Some(from.to_string()),
                });
            } else {
                let Some(path) = it.next() else { break };
                out.push(ChangedPath {
                    status: letter,
                    path: path.to_string(),
                    renamed_from: None,
                });
            }
        }
        Tri::Yes(out)
    }

    /// `--first-parent`: the tripwire counts MERGES, not commits — recon measured 2
    /// commits against 1 first-parent merge for the same path, so without it the tripwire
    /// over-fires by the size of every PR (D-9).
    ///
    /// NOTE (deviation from ARCHITECTURE.md §2.11, reported): the contract's signature
    /// omits the base ref while its own body names `<since>..<main>`. `base` is passed
    /// explicitly rather than defaulting to `HEAD`, which in the primary worktree is
    /// whatever branch the human happens to be standing on.
    pub fn merges_touching(&self, since: &Sha, base: &str, globs: &[Pathspec]) -> Tri<u32> {
        let range = format!("{}..{base}", since.as_str());
        let cmd = format!(
            "git rev-list --count --first-parent {range} -- <{} globs>",
            globs.len()
        );
        match self.run_ps(&["rev-list", "--count", "--first-parent", &range], globs) {
            Err(e) => Tri::Unknown(git_failed(cmd, -1, &e.to_string())),
            Ok(o) if o.code == 0 => match o.out.trim().parse::<u32>() {
                Ok(n) => Tri::Yes(n),
                Err(_) => Tri::Unknown(git_failed(cmd, 0, &o.out)),
            },
            Ok(o) => Tri::Unknown(git_failed(cmd, o.code, &o.err)),
        }
    }

    /// The spec's last-edit anchor. `None` means "never committed on `rev`" — which is
    /// "no anchor", not "0 merges since".
    ///
    /// NOTE (deviation from ARCHITECTURE.md §2.11, reported): `rev` is explicit for the
    /// same reason as [`Git::merges_touching`]'s `base`.
    pub fn last_touch(&self, rev: &str, p: &Pathspec) -> Option<(Sha, DateTime<Utc>)> {
        let o = self
            .run_ps(
                &["log", "-1", "--format=%H%x00%cI", rev],
                std::slice::from_ref(p),
            )
            .ok()?;
        if o.code != 0 {
            return None;
        }
        let line = o.out.trim();
        let (sha, at) = line.split_once('\0')?;
        Some((Sha::mint(sha)?, parse_iso(at)?))
    }

    /// `(ahead, behind)` for `head` relative to `base`.
    pub fn ahead_behind(&self, base: &str, head: &str) -> Option<(u32, u32)> {
        let range = format!("{base}...{head}");
        let o = self
            .run(&["rev-list", "--left-right", "--count", &range])
            .ok()?;
        if o.code != 0 {
            return None;
        }
        let mut it = o.out.split_whitespace();
        // `--left-right` prints LEFT then RIGHT: left is `base`-only (behind), right is
        // `head`-only (ahead).
        let behind: u32 = it.next()?.parse().ok()?;
        let ahead: u32 = it.next()?.parse().ok()?;
        Some((ahead, behind))
    }

    pub fn last_commit_at(&self, rev: &str) -> Option<DateTime<Utc>> {
        let o = self.run(&["log", "-1", "--format=%cI", rev]).ok()?;
        (o.code == 0).then(|| parse_iso(o.out.trim())).flatten()
    }

    pub fn fetch(&self) -> Result<()> {
        // `GIT_TERMINAL_PROMPT=0` turns a credential prompt into a fast failure. An
        // agent-driven CLI that blocks on an invisible prompt is worse than one that
        // reports `unknown (fetch failed)` and moves on.
        let o = self.run_env(
            &["fetch", "--prune", "--quiet", "origin"],
            &[("GIT_TERMINAL_PROMPT", "0")],
        )?;
        if o.code != 0 {
            return Err(self.git_err(
                format!("git fetch failed: {}", o.err.trim()),
                "git fetch --prune --quiet origin".to_string(),
                o.code,
                &o.err,
            ));
        }
        Ok(())
    }

    /// `mtime(common_dir/FETCH_HEAD)` — updated on every fetch even when nothing new
    /// arrived, which makes it a valid freshness clock. A remote-tracking ref's mtime is
    /// not: refs get packed into `packed-refs`.
    pub fn fetch_age(&self) -> Option<Duration> {
        let o = self
            .run(&[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "FETCH_HEAD",
            ])
            .ok()?;
        if o.code != 0 {
            return None;
        }
        let path = PathBuf::from(o.out.trim());
        std::fs::metadata(path)
            .ok()?
            .modified()
            .ok()?
            .elapsed()
            .ok()
    }

    /// `-z`; the FIRST stanza is the primary worktree, wherever this runs from.
    pub fn worktrees(&self) -> Result<Vec<WorktreeRow>> {
        let o = self.run(&["worktree", "list", "--porcelain", "-z"])?;
        if o.code != 0 {
            return Err(self.git_err(
                "cannot list worktrees".to_string(),
                "git worktree list --porcelain -z".to_string(),
                o.code,
                &o.err,
            ));
        }
        let mut rows = Vec::new();
        let mut cur: Option<WorktreeRow> = None;
        for field in o.out.split('\0') {
            if field.is_empty() {
                // The blank record between stanzas.
                if let Some(r) = cur.take() {
                    rows.push(r);
                }
                continue;
            }
            let (key, val) = match field.split_once(' ') {
                Some((k, v)) => (k, Some(v)),
                None => (field, None),
            };
            match key {
                "worktree" => {
                    if let Some(r) = cur.take() {
                        rows.push(r);
                    }
                    cur = Some(WorktreeRow {
                        path: PathBuf::from(val.unwrap_or_default()),
                        branch: None,
                        head: None,
                        bare: false,
                        detached: false,
                        locked: None,
                        prunable: None,
                    });
                }
                _ => {
                    let Some(r) = cur.as_mut() else { continue };
                    match key {
                        "HEAD" => r.head = val.map(str::to_string),
                        "branch" => {
                            r.branch =
                                val.map(|b| b.strip_prefix("refs/heads/").unwrap_or(b).to_string())
                        }
                        "bare" => r.bare = true,
                        "detached" => r.detached = true,
                        "locked" => r.locked = Some(val.unwrap_or_default().to_string()),
                        "prunable" => r.prunable = Some(val.unwrap_or_default().to_string()),
                        _ => {}
                    }
                }
            }
        }
        if let Some(r) = cur.take() {
            rows.push(r);
        }
        Ok(rows)
    }

    /// `--no-track` is mandatory: without it a later `git push` from the ticket branch
    /// targets **main** (D-21).
    ///
    /// Exit codes here are inconsistent — recon measured **255** for "branch exists" and
    /// **128** for everything else — so the failure is classified by stderr, never by the
    /// code.
    pub fn worktree_add(&self, path: &Path, branch: &str, base: &str) -> Result<()> {
        let p = path.to_string_lossy().into_owned();
        let o = self.run(&["worktree", "add", "--no-track", "-b", branch, &p, base])?;
        if o.code == 0 {
            return Ok(());
        }
        let e = o.err.trim().to_string();
        let cmd = format!("git worktree add --no-track -b {branch} {p} {base}");
        if e.contains("already used by worktree at") {
            return Err(KsError::conflict(
                format!("branch `{branch}` is checked out in another worktree"),
                // `where` takes `--branch <BRANCH>`, never a positional — the spelling the
                // sibling refusal in `cmd/flow.rs::create_branch` already gets right.
                fixes![
                    fix!("git worktree list"),
                    fix!("kanspec where --branch {branch}"),
                ],
            ));
        }
        if e.contains("a branch named") && e.contains("already exists") {
            return Err(KsError::conflict(
                format!("branch `{branch}` already exists"),
                // There is no `--no-worktree` flag: a worktree is opt-in via `--worktree`
                // (`cmd/flow.rs` branches on `a.worktree` alone), so claiming WITHOUT the
                // flag is the "no worktree" path and reuses the branch that already exists.
                fixes![fix!("git branch -D {branch}"), fix!("kanspec start <id>"),],
            ));
        }
        if e.contains("already exists") {
            return Err(KsError::conflict(
                format!("{p} already exists and is not empty"),
                // Same phantom flag as above: plain `start` is the no-worktree claim.
                fixes![fix!("rm -rf {p}"), fix!("kanspec start <id>")],
            ));
        }
        Err(self.git_err(format!("cannot create worktree: {e}"), cmd, o.code, &o.err))
    }

    /// Does NOT delete the branch — `git worktree remove` never does, so `kanspec` calls
    /// [`Git::branch_delete`] separately.
    pub fn worktree_remove(&self, path: &Path, force: bool) -> Result<()> {
        let p = path.to_string_lossy().into_owned();
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(&p);
        let o = self.run(&args)?;
        if o.code == 0 {
            return Ok(());
        }
        Err(self.git_err(
            format!("cannot remove worktree {p}: {}", o.err.trim()),
            format!("git worktree remove {p}"),
            o.code,
            &o.err,
        ))
    }

    pub fn branch_delete(&self, branch: &str, force: bool) -> Result<()> {
        let flag = if force { "-D" } else { "-d" };
        let o = self.run(&["branch", flag, branch])?;
        if o.code == 0 {
            return Ok(());
        }
        Err(self.git_err(
            format!("cannot delete branch {branch}: {}", o.err.trim()),
            format!("git branch {flag} {branch}"),
            o.code,
            &o.err,
        ))
    }

    /// How many `.kanspec/` changes are pending — what `sync = "batch"` reminds about.
    /// `status` respects gitignore, so the disposable `cache/` never inflates the count.
    /// `extra` carries the generated projections, which live at the REPO ROOT rather than
    /// under `.kanspec/` and are renameable via `[paths]`. Counting only `.kanspec/**` made
    /// `status` report "nothing pending" while a regenerated `KANSPEC-FEATURES.md` sat
    /// uncommitted — so following kanspec's own sync advice left the committed feature map
    /// stale, which is precisely the rot the projections exist to prevent.
    pub fn dirty_kanspec(&self, extra: &[&Path]) -> Result<u32> {
        let mut ps = vec![Pathspec::glob(".kanspec/**")];
        ps.extend(extra.iter().map(|p| Pathspec::glob(&p.to_string_lossy())));
        let o = self.run_ps(&["status", "--porcelain=v2", "-z"], &ps)?;
        if o.code != 0 {
            return Err(self.git_err(
                "cannot read the working tree status".to_string(),
                "git status --porcelain=v2 -z -- .kanspec/**".to_string(),
                o.code,
                &o.err,
            ));
        }
        // porcelain=v2 -z puts a rename's ORIGINAL path in its own NUL-terminated field,
        // so counting records would overcount renames; count only entry headers.
        Ok(o.out
            .split('\0')
            .filter(|r| {
                r.starts_with("1 ")
                    || r.starts_with("2 ")
                    || r.starts_with("u ")
                    || r.starts_with("? ")
                    || r.starts_with("! ")
            })
            .count() as u32)
    }

    /// Proves an ignore RULE exists. It says nothing about tracking state — a file
    /// committed before the rule was added stays tracked forever — which is why `doctor`
    /// pairs it with [`Git::is_tracked`].
    pub fn is_ignored(&self, p: &Path) -> bool {
        let s = p.to_string_lossy().into_owned();
        self.run(&["check-ignore", "-q", "--", &s])
            .map(|o| o.code == 0)
            .unwrap_or(false)
    }

    pub fn is_tracked(&self, p: &Path) -> bool {
        let s = p.to_string_lossy().into_owned();
        self.run(&["ls-files", "--error-unmatch", "--", &s])
            .map(|o| o.code == 0)
            .unwrap_or(false)
    }

    /// `--git-path hooks`, because `core.hooksPath` (husky/lefthook) makes `.git/hooks`
    /// completely inert — a hook installed there would be silently dead (D-7).
    pub fn hooks_dir(&self) -> Result<PathBuf> {
        let o = self.run(&["rev-parse", "--path-format=absolute", "--git-path", "hooks"])?;
        if o.code != 0 || o.out.trim().is_empty() {
            return Err(self.git_err(
                "cannot resolve the hooks directory".to_string(),
                "git rev-parse --git-path hooks".to_string(),
                o.code,
                &o.err,
            ));
        }
        Ok(PathBuf::from(o.out.trim()))
    }

    /// `sync = "commit"`. A no-op when nothing under `.kanspec/` is staged, so a verb that
    /// changed nothing does not manufacture an empty commit.
    pub fn commit_kanspec(&self, msg: &str) -> Result<()> {
        let ps = [Pathspec::glob(".kanspec/**")];
        let add = self.run_ps(&["add", "-A"], &ps)?;
        if add.code != 0 {
            return Err(self.git_err(
                format!("cannot stage .kanspec/: {}", add.err.trim()),
                "git add -A -- .kanspec/**".to_string(),
                add.code,
                &add.err,
            ));
        }
        let staged = self.run_ps(&["diff", "--cached", "--quiet"], &ps)?;
        if staged.code == 0 {
            return Ok(()); // nothing to commit
        }
        let commit = self.run_ps(&["commit", "-m", msg], &ps)?;
        if commit.code != 0 {
            return Err(self.git_err(
                format!("cannot commit .kanspec/: {}", commit.err.trim()),
                format!("git commit -m {msg:?} -- .kanspec/**"),
                commit.code,
                &commit.err,
            ));
        }
        Ok(())
    }

    fn git_err(&self, message: String, cmd: String, exit: i32, stderr: &str) -> KsError {
        let detail = stderr.trim();
        KsError::Git {
            message: if detail.is_empty() {
                message
            } else {
                format!("{message} ({detail})")
            },
            cmd,
            exit,
            fix: fixes![fix!("kanspec doctor"), fix!("kanspec scan --explain")],
        }
    }
}

fn git_failed(cmd: String, code: i32, stderr: &str) -> Unknown {
    Unknown::GitFailed {
        cmd,
        code,
        stderr: stderr.trim().to_string(),
    }
}

fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_mint_rejects_anything_that_is_not_git_output() {
        assert!(Sha::mint("a1b9c3d").is_some());
        assert!(Sha::mint("a1b9c3").is_none(), "fewer than 7 hex");
        assert!(Sha::mint("A1B9C3D").is_none(), "git prints lowercase");
        assert!(Sha::mint("not-a-sha").is_none());
        assert_eq!(Sha::mint("a1b9c3d5f00").unwrap().short(), "a1b9c3d");
    }

    #[test]
    fn pathspecs_always_carry_the_glob_top_magic() {
        assert_eq!(
            Pathspec::glob("src/auth/**").as_str(),
            ":(glob,top)src/auth/**"
        );
    }

    #[test]
    fn every_unknown_reason_renders_a_badge() {
        for u in [
            Unknown::NoHead,
            Unknown::ZeroCommitBranch,
            Unknown::HeadNotInObjectStore {
                sha: "a1b9c3d".into(),
            },
            Unknown::GhUnavailable {
                why: "HTTP 401".into(),
            },
            Unknown::SquashSuspectedNoGh { plus_lines: 2 },
            Unknown::FetchStale { age_secs: 900 },
        ] {
            assert!(u.badge().starts_with("unknown ("), "{}", u.badge());
        }
        assert_eq!(
            Unknown::SquashSuspectedNoGh { plus_lines: 2 }.badge(),
            "unknown (squash suspected, no gh)"
        );
    }
}
