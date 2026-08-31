//! `gh` JSON, and **the one and only mock seam in the crate**.
//!
//! `$KANSPEC_GH_FIXTURES` points at recorded `gh` JSON. You cannot create a real GitHub PR
//! in a test, and rung 2 is the only rung that catches a title-only squash — the case that
//! defeats all four. **Git itself is never mocked**, and there is no `trait GhBackend`.
//!
//! # The tri-state, and why nothing here ever says "no"
//!
//! Rung 2's three answers are *merged* / *not-merged* / *inconclusive*. Of those, this
//! module can produce exactly two — **it has no way to say "not merged"**:
//!
//! * **merged** — `Ok(pr)` whose [`PrInfo::state`] is [`PrState::Merged`]. The caller still
//!   re-verifies `mergeCommit` against local git before trusting it (ARCHITECTURE.md §7).
//! * **inconclusive** — `Err(`[`GhUnavailable`]`)`. Absent `gh`, unauthenticated `gh`,
//!   `[git] gh = "never"`, a network failure, a `gh` that never answers, an unparseable
//!   response, and a missing fixture are **all** this. Never a negative.
//!
//! The third shape — `Ok(pr)` in `OPEN`/`CLOSED`, or `Ok(vec![])` — is *evidence GitHub
//! offered*, and it is still not a negative: a closed PR's commits can have been
//! cherry-picked onto main, and a branch with no PR at all can have been pushed straight
//! there. The ladder falls through to rung 3 rather than concluding anything.
//! [`merged_pr`] is the one place that reads a list, and it can only ever return "here is
//! a merged PR" or "nothing to say".
//!
//! Every outcome carries its evidence — the PR's own fields on the way up, the `gh`
//! command and what it printed on the way down — so a badge can render method + reason.
//!
//! # The fixture seam
//!
//! When `$KANSPEC_GH_FIXTURES` names a directory, every query reads a recorded JSON file
//! from it instead of shelling out, and [`Gh::available`] answers `true` without probing.
//! File names come from [`fixture_name_pr`] / [`fixture_name_head`] (+`.json`), which is
//! exactly what `TestRepo::gh_fixture` writes. A *missing* fixture is inconclusive, not
//! empty — a test that forgets to record one gets `unknown`, never a false "not merged".
//! The recorded corpus lives in `tests/fixtures/gh/`; it is real `gh` output, captured
//! from public repositories, and it is parsed here by the same code that parses live `gh`.
//!
//! Owner: **S2**.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// NOTE the deviation from §2.11's literal `authed: OnceCell<bool>`: `std::cell::OnceCell`
// is `!Sync`, which breaks `Arc<Ctx>: Send + Sync` — the assertion §2.6 makes a hard
// compile-time fact, and the property that lets the server's POST handlers call the very
// same `cmd::*` functions inside `spawn_blocking`. `OnceLock` is the `Sync` equivalent and
// memoizes identically.
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::git::Git;

/// The `[git] gh` policy. Aliased here so `Gh::detect(git, cfg: &GhCfg)` reads exactly as
/// the contract writes it while there is still only one definition of the enum.
pub use crate::config::GhMode as GhCfg;

/// The `--json` field set. One constant so the live query and the recorded fixtures can
/// never drift apart.
const PR_FIELDS: &str = "number,state,mergedAt,mergeCommit,headRefOid,url";

/// `gh auth status` hits the API to validate the token; a dead network must not wedge
/// `kanspec ls`.
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// A PR query is a network round trip. Past this it is inconclusive, not slow.
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the deadline is checked while `gh` runs. Small next to any network call.
const POLL: Duration = Duration::from_millis(5);
/// A branch can carry several PRs (reused branch names); more than this is noise.
const LIST_LIMIT: &str = "20";
/// Evidence goes on a one-line badge. Longer than this is a wall, not a reason.
const DETAIL_MAX: usize = 240;

pub struct Gh {
    /// The primary worktree, so a lazily-resolved slug and every `gh` invocation are
    /// anchored exactly where `Git` is.
    ///
    /// NOTE (deviation from ARCHITECTURE.md §2.11, reported): the contract's `Gh` holds a
    /// plain `slug: Option<String>`, which can only be filled eagerly in `detect` — but
    /// `detect`'s own contract says the repo slug is deferred, because `Ctx::open` runs
    /// before *every* command and most of them never ask `gh` anything. Holding the root
    /// plus a `OnceLock` keeps the deferral the contract asks for; the fields are private,
    /// so nothing outside this file can tell the difference.
    root: PathBuf,
    slug: OnceLock<Option<String>>,
    mode: GhCfg,
    fixtures: Option<PathBuf>,
    authed: OnceLock<bool>,
}

/// Never an error type: a `gh` failure is inconclusive, never "not merged" (invariant 2).
#[derive(Debug, Clone, Serialize)]
pub struct GhUnavailable(pub String);

impl std::fmt::Display for GhUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub state: PrState,
    pub merged_at: Option<DateTime<Utc>>,
    pub merge_commit: Option<String>,
    pub head_ref_oid: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

// ─────────────────────────────────────────────────────────────────────────────
// The wire shapes — what `gh --json` actually prints
// ─────────────────────────────────────────────────────────────────────────────

/// `gh` speaks camelCase and wraps the merge commit in an object (`"mergeCommit":
/// {"oid": "…"}` or `null`) — verified against real `gh pr view`/`gh pr list` output,
/// recorded in `tests/fixtures/gh/`. Unknown fields are tolerated on purpose: `gh pr list`
/// adds `headRefName`, and a future `gh` may add more.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrWire {
    number: u64,
    state: PrState,
    #[serde(default)]
    merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    merge_commit: Option<OidWire>,
    #[serde(default)]
    head_ref_oid: String,
    #[serde(default)]
    url: String,
}

#[derive(Debug, Deserialize)]
struct OidWire {
    #[serde(default)]
    oid: String,
}

impl PrWire {
    fn into_info(self) -> PrInfo {
        PrInfo {
            number: self.number,
            state: self.state,
            merged_at: self.merged_at,
            // An empty `oid` is not a SHA. Old PRs really do come back with one, and a
            // blank string would sail straight into `Sha::mint` as a `None` nobody
            // expected — better to say "gh knows of no merge commit" here.
            merge_commit: self
                .merge_commit
                .map(|c| c.oid)
                .filter(|s| !s.trim().is_empty()),
            head_ref_oid: self.head_ref_oid,
            url: self.url,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixture naming — the seam's one convention, as code rather than a comment
// ─────────────────────────────────────────────────────────────────────────────

/// The fixture stem [`Gh::pr_view`] replays: `pr-<n>`. Pass it to
/// `TestRepo::gh_fixture(name, json)`, which appends `.json`.
pub fn fixture_name_pr(n: u64) -> String {
    format!("pr-{n}")
}

/// The fixture stem [`Gh::pr_for_head`] replays: `head-<branch>`, with every character a
/// branch may legally contain but a flat filename may not (`/` above all) folded to `-`.
pub fn fixture_name_head(branch: &str) -> String {
    let slug: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("head-{slug}")
}

// ─────────────────────────────────────────────────────────────────────────────
// Reading a list without inventing a negative
// ─────────────────────────────────────────────────────────────────────────────

/// The one place a list of PRs becomes rung 2's evidence.
///
/// `Some(pr)` **only** for a PR GitHub itself reports as `MERGED`; when several merged PRs
/// share a head branch (a reused branch name), the most recently merged wins. `None` means
/// "`gh` answered and showed nothing that landed" — which is *not* proof the work did not
/// land, because a closed PR's commits can still have been cherry-picked onto main. The
/// caller falls through to the next rung; it never concludes "no" from this.
pub fn merged_pr(list: &[PrInfo]) -> Option<&PrInfo> {
    list.iter()
        .filter(|p| p.state == PrState::Merged)
        .max_by_key(|p| (p.merged_at, p.number))
}

impl Gh {
    /// Cheap and total: reads the fixture env var and the config policy. Everything that
    /// can fail (auth, the repo slug) is deferred to [`Gh::available`], because `Ctx::open`
    /// runs before every command including `--help`-adjacent ones.
    pub(crate) fn detect(git: &Git, cfg: &GhCfg) -> Gh {
        Gh {
            root: git.root().to_path_buf(),
            slug: OnceLock::new(),
            mode: *cfg,
            fixtures: std::env::var_os("KANSPEC_GH_FIXTURES").map(PathBuf::from),
            authed: OnceLock::new(),
        }
    }

    /// `gh auth status`, cached for the process. Always `false` under
    /// `[git] gh = "never"`; always `true` when fixtures are recorded.
    ///
    /// Order matters, and `never` wins: an explicit "do not talk to GitHub" is a policy,
    /// while the fixture directory is a test seam, so a suite can set both and still
    /// exercise the no-`gh` path of the ladder.
    ///
    /// Under `auto` a remote that is not GitHub answers `false` without spawning anything:
    /// `gh` has nothing to say about a GitLab or a bare-path origin, and probing it would
    /// cost every `kanspec ls` in such a repo a subprocess to learn so. `always` skips that
    /// check — the user asked.
    pub fn available(&self) -> bool {
        if self.mode == GhCfg::Never {
            return false;
        }
        if self.fixtures.is_some() {
            return true;
        }
        if self.mode == GhCfg::Auto && self.slug().is_none() {
            return false;
        }
        *self.authed.get_or_init(|| self.probe_auth())
    }

    pub fn pr_view(&self, n: u64) -> std::result::Result<PrInfo, GhUnavailable> {
        // The gate comes first even on the fixture path, so `gh = "never"` means never —
        // including in a suite that has recordings on disk for other tickets.
        self.gate()?;
        let wire: PrWire = match &self.fixtures {
            Some(dir) => read_fixture(dir, &fixture_name_pr(n))?,
            None => {
                let num = n.to_string();
                self.json(&["pr", "view", &num, "--json", PR_FIELDS])?
            }
        };
        Ok(wire.into_info())
    }

    /// REQUIRED, not optional: a squash-merged ticket with `pr: null` would otherwise skip
    /// the only rung that can see a title-only squash.
    ///
    /// `--state all`, because the PR that landed a squash is `MERGED` and therefore
    /// invisible to `gh pr list`'s default `--state open`. The result is sorted so the most
    /// recently merged PR comes first: the caller may take the first `MERGED` entry (or use
    /// [`merged_pr`]) and get the right one when a branch name has been reused.
    ///
    /// An empty `Vec` is a real, successful answer meaning "GitHub knows of no PR for this
    /// branch" — inconclusive at the ladder, never a negative.
    pub fn pr_for_head(&self, branch: &str) -> std::result::Result<Vec<PrInfo>, GhUnavailable> {
        self.gate()?;
        let mut list: Vec<PrInfo> = match &self.fixtures {
            Some(dir) => read_fixture::<Vec<PrWire>>(dir, &fixture_name_head(branch))?,
            None => self.json::<Vec<PrWire>>(&[
                "pr", "list", "--head", branch, "--state", "all", "--limit", LIST_LIMIT, "--json",
                PR_FIELDS,
            ])?,
        }
        .into_iter()
        .map(PrWire::into_info)
        .collect();
        // Merged first, newest merge first; everything GitHub never merged sorts last.
        list.sort_by(|a, b| {
            let key = |p: &PrInfo| (p.state == PrState::Merged, p.merged_at, p.number);
            key(b).cmp(&key(a))
        });
        Ok(list)
    }

    // ── internals ────────────────────────────────────────────────────────────

    /// The refusal every live query starts with, so a disabled or absent `gh` can never
    /// reach the network — and so its reason reaches the badge instead of an empty result.
    fn gate(&self) -> std::result::Result<(), GhUnavailable> {
        if self.mode == GhCfg::Never {
            return Err(GhUnavailable(
                "`gh` is disabled by `[git] gh = \"never\"`".into(),
            ));
        }
        if !self.available() {
            return Err(GhUnavailable(match self.slug() {
                None if self.mode == GhCfg::Auto => {
                    "`origin` is not a GitHub remote, so `gh` has nothing to answer".into()
                }
                _ => "`gh auth status` failed: not installed, or not authenticated".to_string(),
            }));
        }
        Ok(())
    }

    /// `owner/name` for the `origin` remote, resolved at most once. `None` for a remote
    /// `gh` cannot speak for (a local path, a non-GitHub host) or no `origin` at all.
    fn slug(&self) -> Option<&str> {
        self.slug
            .get_or_init(|| {
                let git = Git::bind(&self.root);
                let o = git.run(&["config", "--get", "remote.origin.url"]).ok()?;
                (o.code == 0).then_some(())?;
                parse_slug(o.out.trim())
            })
            .as_deref()
    }

    fn probe_auth(&self) -> bool {
        matches!(
            run_capped(self.cmd(&["auth", "status"]), AUTH_TIMEOUT),
            Ok(c) if c.code == 0
        )
    }

    /// Every `gh` invocation, built one way — environment and working directory only.
    ///
    /// **No `--repo` here.** `gh auth status` does not accept the flag and exits 1 on it,
    /// so a base command that carried it would report every authenticated machine as
    /// unauthenticated, and rung 2 would silently never run against a real GitHub repo.
    /// The flag rides with the subcommands that take it: [`Gh::pr_cmd`].
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new("gh");
        c.current_dir(&self.root).args(args);
        // `gh` shells out to git, and git exports GIT_DIR when it runs a hook: without the
        // scrub a hook-invoked `kanspec scan` would have `gh` inspect the LINKED worktree.
        c.env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            // Nothing here is interactive, and nothing here may block on a prompt or on
            // gh's release check.
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1");
        c
    }

    /// A `gh pr …` query, pinned to the repository we resolved rather than to whatever
    /// `gh` would infer from the working directory — the same reason `git.rs` is always
    /// `git -C <primary_root>`.
    fn pr_cmd(&self, args: &[&str]) -> Command {
        let mut c = self.cmd(args);
        if let Some(slug) = self.slug() {
            c.arg("--repo").arg(slug);
        }
        c
    }

    /// Run `gh pr …` and parse its stdout, turning every failure into evidence.
    fn json<T: DeserializeOwned>(&self, args: &[&str]) -> std::result::Result<T, GhUnavailable> {
        let shown = format!("gh {}", args.join(" "));
        let cap = run_capped(self.pr_cmd(args), QUERY_TIMEOUT)
            .map_err(|e| GhUnavailable(format!("`{shown}` {e}")))?;
        if cap.code != 0 {
            return Err(GhUnavailable(format!(
                "`{shown}` exited {}: {}",
                cap.code,
                detail(if cap.err.trim().is_empty() {
                    &cap.out
                } else {
                    &cap.err
                })
            )));
        }
        serde_json::from_str(&cap.out).map_err(|e| {
            GhUnavailable(format!(
                "`{shown}` printed JSON kanspec could not read: {e}"
            ))
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixtures
// ─────────────────────────────────────────────────────────────────────────────

fn read_fixture<T: DeserializeOwned>(
    dir: &Path,
    stem: &str,
) -> std::result::Result<T, GhUnavailable> {
    let path = dir.join(format!("{stem}.json"));
    let body = std::fs::read_to_string(&path).map_err(|e| {
        // A forgotten recording is inconclusive, NOT an empty answer: the difference is
        // the whole point of the seam.
        GhUnavailable(format!(
            "no recorded `gh` fixture at {}: {e}",
            path.display()
        ))
    })?;
    // Same clause as the live path's parse failure ("could not read"), so a badge reads
    // the same whether the JSON came off a socket or off disk.
    serde_json::from_str(&body).map_err(|e| {
        GhUnavailable(format!(
            "recorded `gh` fixture {} is JSON kanspec could not read: {e}",
            path.display()
        ))
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Subprocess plumbing — a `gh` that never answers must not become a `kanspec` that
// never answers
// ─────────────────────────────────────────────────────────────────────────────

struct Captured {
    code: i32,
    out: String,
    err: String,
}

/// Spawn, drain both pipes on their own threads (so a large answer cannot deadlock on a
/// full pipe buffer), and kill the child if it outlives `limit`. `Err` is a one-clause
/// reason, ready to be appended to the command that produced it.
fn run_capped(mut cmd: Command, limit: Duration) -> std::result::Result<Captured, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not run: {e}"))?;
    let out_t = drain(child.stdout.take());
    let err_t = drain(child.stderr.take());

    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Err(e) => return Err(format!("could not be waited on: {e}")),
            Ok(Some(s)) => break s,
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("did not answer within {}s", limit.as_secs()));
        }
        std::thread::sleep(POLL);
    };
    Ok(Captured {
        code: status.code().unwrap_or(-1),
        // The child has exited, so both pipes are closed and both reads have finished.
        out: out_t.join().unwrap_or_default(),
        err: err_t.join().unwrap_or_default(),
    })
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).into_owned()
    })
}

/// One line, bounded: evidence renders on a badge, not in a pager.
fn detail(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= DETAIL_MAX {
        return flat;
    }
    let head: String = flat.chars().take(DETAIL_MAX).collect();
    format!("{head}…")
}

/// `git@github.com:owner/repo.git` · `https://github.com/owner/repo(.git)` ·
/// `ssh://git@github.com/owner/repo.git` · `github.com/owner/repo`.
///
/// `None` for anything `gh` cannot speak for — a bare local path (every `TestRepo`), a
/// non-GitHub host, or a URL with no `owner/name` in it.
fn parse_slug(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let rest = match url.split_once("://") {
        // scheme://[user@]host/owner/repo
        Some((_scheme, after)) => after,
        None => url,
    };
    // scp-like `git@host:owner/repo` — the only form where the separator is a colon.
    let (host, path) = match rest.split_once('@') {
        Some((_user, after)) => match after.split_once(':') {
            Some((h, p)) => (h, p),
            None => after.split_once('/')?,
        },
        None => rest.split_once('/')?,
    };
    // A port, if the URL carried one, is not part of the host name.
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    let github = host.eq_ignore_ascii_case("github.com")
        || host.to_ascii_lowercase().ends_with(".github.com")
        || host.to_ascii_lowercase().starts_with("github.");
    if !github {
        return None;
    }
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let owner = segs.next()?;
    let name = segs.next()?;
    // `owner/repo` and nothing after it: a deeper path is a URL to a file, not a repo.
    if segs.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The recorded corpus, compiled in so a fixture that stops parsing fails the build's
    // own tests rather than only S3's. Every one of these is real `gh` output — see
    // `tests/fixtures/gh/README.md` for the command that produced each.
    const VIEW_SQUASH: &str = include_str!("../tests/fixtures/gh/pr-view-squash-merged.json");
    const VIEW_MERGE_COMMIT: &str = include_str!("../tests/fixtures/gh/pr-view-merge-commit.json");
    const VIEW_OPEN: &str = include_str!("../tests/fixtures/gh/pr-view-open.json");
    const VIEW_CLOSED: &str = include_str!("../tests/fixtures/gh/pr-view-closed-unmerged.json");
    const VIEW_MERGED_NO_COMMIT: &str =
        include_str!("../tests/fixtures/gh/pr-view-merged-no-merge-commit.json");
    const LIST_MERGED: &str = include_str!("../tests/fixtures/gh/pr-list-head-squash-merged.json");
    const LIST_NONE: &str = include_str!("../tests/fixtures/gh/pr-list-head-none.json");

    /// The branch the recorded `pr list --head` fixture was captured for.
    const RECORDED_BRANCH: &str = "fix/bundle-sourcemap-optional-value";

    /// A `Gh` wired to a fixture directory, built by hand: `detect` reads a process-global
    /// env var, and `cargo test` runs these in parallel threads.
    fn gh_with(fixtures: &Path, mode: GhCfg) -> Gh {
        Gh {
            root: fixtures.to_path_buf(),
            slug: OnceLock::from(Some("kanspec/fixture".to_string())),
            mode,
            fixtures: Some(fixtures.to_path_buf()),
            authed: OnceLock::new(),
        }
    }

    /// A `Gh` with no fixtures and no way to reach a network: `never` guarantees nothing
    /// is spawned, which is what makes this usable in a unit test.
    fn gh_never() -> Gh {
        Gh {
            root: PathBuf::from("/nonexistent"),
            slug: OnceLock::from(None),
            mode: GhCfg::Never,
            fixtures: None,
            authed: OnceLock::new(),
        }
    }

    fn fixture_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("a temp dir");
        for (stem, body) in files {
            std::fs::write(d.path().join(format!("{stem}.json")), body).unwrap();
        }
        d
    }

    #[track_caller]
    fn why<T>(r: std::result::Result<T, GhUnavailable>) -> String {
        match r {
            Ok(_) => panic!("expected `gh` to decline, and it answered"),
            Err(GhUnavailable(w)) => w,
        }
    }

    #[test]
    fn pr_json_from_gh_deserializes() {
        let p: PrInfo = serde_json::from_str(
            r#"{"number":142,"state":"MERGED","merged_at":"2026-08-30T16:02:00Z",
                "merge_commit":"a1b9c3d","head_ref_oid":"deadbee","url":"https://x/pull/142"}"#,
        )
        .unwrap();
        assert_eq!(p.state, PrState::Merged);
        assert_eq!(p.number, 142);
    }

    // ── the recorded shapes ──────────────────────────────────────────────────

    /// The shape that matters: a real GitHub squash merge, whose `mergeCommit` is a nested
    /// object and not the string the naive struct would expect.
    #[test]
    fn recorded_squash_merge_parses_to_merged_with_a_merge_commit() {
        let d = fixture_dir(&[(&fixture_name_pr(36723), VIEW_SQUASH)]);
        let pr = gh_with(d.path(), GhCfg::Auto).pr_view(36723).unwrap();
        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(pr.number, 36723);
        assert_eq!(
            pr.merge_commit.as_deref(),
            Some("baca93a73975d805dd6c6975487d496a53377c5c"),
            "gh nests the merge commit as {{\"oid\": …}}; rung 2 needs the oid itself"
        );
        assert_eq!(pr.head_ref_oid, "d3c760380be778c086280a1952e545efd429b1f0");
        assert_eq!(
            pr.merged_at.map(|t| t.to_rfc3339()),
            Some("2026-08-28T16:31:55+00:00".to_string())
        );
        assert!(pr.url.ends_with("/pull/36723"));
    }

    #[test]
    fn recorded_true_merge_parses_to_merged() {
        let d = fixture_dir(&[(&fixture_name_pr(9000), VIEW_MERGE_COMMIT)]);
        let pr = gh_with(d.path(), GhCfg::Auto).pr_view(9000).unwrap();
        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(
            pr.merge_commit.as_deref(),
            Some("68dfd87f47e3757e92953c3a0eaa42cf4c7d0d4f")
        );
    }

    /// An open PR is a successful answer that says nothing landed — `Ok`, not `Err`, so
    /// the badge can distinguish "GitHub says it is still open" from "gh could not answer".
    #[test]
    fn recorded_open_pr_answers_ok_with_no_merge() {
        let d = fixture_dir(&[(&fixture_name_pr(36742), VIEW_OPEN)]);
        let pr = gh_with(d.path(), GhCfg::Auto).pr_view(36742).unwrap();
        assert_eq!(pr.state, PrState::Open);
        assert_eq!(pr.merged_at, None);
        assert_eq!(pr.merge_commit, None);
    }

    /// The trap: a CLOSED, never-merged PR is the one answer that *looks* like proof of
    /// "not merged" — and is not, because its commits can still have been cherry-picked.
    /// It stays an `Ok` observation; only the ladder decides, and only from other rungs.
    #[test]
    fn recorded_closed_unmerged_pr_is_evidence_not_a_negative() {
        let d = fixture_dir(&[(&fixture_name_pr(36734), VIEW_CLOSED)]);
        let pr = gh_with(d.path(), GhCfg::Auto).pr_view(36734).unwrap();
        assert_eq!(pr.state, PrState::Closed);
        assert_eq!(pr.merged_at, None);
        assert_eq!(pr.merge_commit, None);
        assert!(
            merged_pr(&[pr]).is_none(),
            "a closed PR must never be read as a merge"
        );
    }

    /// `state: MERGED` with no merge commit: gh answered, and rung 2 still cannot hand
    /// local git a SHA to re-verify. It must not become a confident "merged".
    #[test]
    fn merged_without_a_merge_commit_yields_no_sha_to_verify() {
        let d = fixture_dir(&[(&fixture_name_pr(13), VIEW_MERGED_NO_COMMIT)]);
        let pr = gh_with(d.path(), GhCfg::Auto).pr_view(13).unwrap();
        assert_eq!(pr.state, PrState::Merged);
        assert_eq!(
            pr.merge_commit, None,
            "no oid to re-verify against main — the ladder must fall through, not conclude"
        );
    }

    // ── pr_for_head, the rung that exists because `pr:` is often null ─────────

    #[test]
    fn recorded_pr_list_for_a_branch_finds_the_squash_merge() {
        let d = fixture_dir(&[(&fixture_name_head(RECORDED_BRANCH), LIST_MERGED)]);
        let list = gh_with(d.path(), GhCfg::Auto)
            .pr_for_head(RECORDED_BRANCH)
            .unwrap();
        assert_eq!(list.len(), 1);
        let pr = merged_pr(&list).expect("the recorded branch landed by squash");
        assert_eq!(pr.number, 36723);
        assert_eq!(
            pr.merge_commit.as_deref(),
            Some("baca93a73975d805dd6c6975487d496a53377c5c")
        );
    }

    /// `gh pr list` exits 0 and prints `[]` for a branch it has never seen. That is a
    /// successful answer carrying no evidence — never "this branch did not merge".
    #[test]
    fn no_pr_for_the_branch_is_an_empty_ok_not_an_error_and_not_a_no() {
        let d = fixture_dir(&[(&fixture_name_head("ks/t-cccc-never"), LIST_NONE)]);
        let list = gh_with(d.path(), GhCfg::Auto)
            .pr_for_head("ks/t-cccc-never")
            .unwrap();
        assert!(list.is_empty());
        assert!(merged_pr(&list).is_none());
    }

    /// Branch names carry `/`; fixture files cannot.
    #[test]
    fn fixture_names_survive_a_slash_in_the_branch() {
        assert_eq!(
            fixture_name_head("ks/t-9c41-rate-limit"),
            "head-ks-t-9c41-rate-limit"
        );
        assert_eq!(fixture_name_head("feature/a b"), "head-feature-a-b");
        assert_eq!(fixture_name_pr(142), "pr-142");
    }

    /// A reused branch name: two merged PRs, and the answer must be the newest merge —
    /// both through `merged_pr` and through §7's plain `.find(state == MERGED)`, which is
    /// why `pr_for_head` sorts before returning.
    #[test]
    fn a_reused_branch_reports_its_most_recent_merge_first() {
        let body = r#"[
          {"number":10,"state":"MERGED","mergedAt":"2026-01-01T00:00:00Z",
           "mergeCommit":{"oid":"1111111111111111111111111111111111111111"},
           "headRefOid":"aaaaaaa","url":"https://x/pull/10"},
          {"number":20,"state":"OPEN","mergedAt":null,"mergeCommit":null,
           "headRefOid":"ccccccc","url":"https://x/pull/20"},
          {"number":30,"state":"MERGED","mergedAt":"2026-06-01T00:00:00Z",
           "mergeCommit":{"oid":"2222222222222222222222222222222222222222"},
           "headRefOid":"bbbbbbb","url":"https://x/pull/30"}
        ]"#;
        let d = fixture_dir(&[(&fixture_name_head("ks/reused"), body)]);
        let list = gh_with(d.path(), GhCfg::Auto)
            .pr_for_head("ks/reused")
            .unwrap();
        assert_eq!(merged_pr(&list).unwrap().number, 30);
        assert_eq!(
            list.iter()
                .find(|p| p.state == PrState::Merged)
                .unwrap()
                .number,
            30,
            "§7's ladder takes the first MERGED entry, so the sort has to put it there"
        );
    }

    // ── the inconclusive half of the tri-state ───────────────────────────────

    /// The single most important property in this file: every way of failing produces a
    /// reason, and none of them produces an answer. A boolean `merged: bool` API could not
    /// express any of these rows.
    #[test]
    fn every_failure_is_inconclusive_with_evidence_never_a_negative() {
        let d = fixture_dir(&[("pr-7", "{ this is not json"), ("head-x", "{}")]);
        let gh = gh_with(d.path(), GhCfg::Auto);

        // 1. a fixture that was never recorded
        let missing = why(gh.pr_view(404));
        assert!(
            missing.contains("no recorded") && missing.contains("pr-404.json"),
            "a missing recording must name itself: {missing}"
        );

        // 2. a response that does not parse
        let garbled = why(gh.pr_view(7));
        assert!(
            garbled.contains("could not read"),
            "unparseable JSON must be inconclusive: {garbled}"
        );

        // 3. valid JSON of the wrong shape (an object where a list belongs)
        let wrong_shape = why(gh.pr_for_head("x"));
        assert!(
            wrong_shape.contains("could not read"),
            "a wrong-shaped response must be inconclusive: {wrong_shape}"
        );

        // 4. `gh` switched off by policy — and no subprocess spawned to learn it
        let off = gh_never();
        assert!(!off.available());
        let disabled = why(off.pr_view(1));
        assert!(
            disabled.contains("never"),
            "the policy that declined must be named: {disabled}"
        );
        assert!(why(off.pr_for_head("ks/anything")).contains("never"));
    }

    /// `never` outranks the fixture seam, so a test can record fixtures for some tickets
    /// and still exercise the ladder's no-`gh` path.
    #[test]
    fn never_beats_the_fixture_seam() {
        let d = fixture_dir(&[(&fixture_name_pr(36723), VIEW_SQUASH)]);
        let gh = gh_with(d.path(), GhCfg::Never);
        assert!(!gh.available());
        assert!(why(gh.pr_view(36723)).contains("never"));
    }

    #[test]
    fn fixtures_make_gh_available_without_probing_anything() {
        let d = fixture_dir(&[]);
        // `root` points at a directory that is not a git repo at all: if `available` had
        // to shell out to `gh` or to `git` to answer, this could not be true.
        assert!(gh_with(d.path(), GhCfg::Auto).available());
        assert!(gh_with(d.path(), GhCfg::Always).available());
    }

    /// A non-GitHub `origin` — which is every `TestRepo`, whose origin is a bare local
    /// path — must answer "unavailable" under `auto` without probing.
    #[test]
    fn a_non_github_origin_is_unavailable_under_auto() {
        let gh = Gh {
            root: PathBuf::from("/nonexistent"),
            slug: OnceLock::from(None),
            mode: GhCfg::Auto,
            fixtures: None,
            authed: OnceLock::new(),
        };
        assert!(!gh.available());
        let w = why(gh.pr_view(1));
        assert!(w.contains("not a GitHub remote"), "{w}");
    }

    // ── slug parsing ─────────────────────────────────────────────────────────

    #[test]
    fn slugs_parse_from_every_remote_url_git_writes() {
        for url in [
            "git@github.com:cli/cli.git",
            "git@github.com:cli/cli",
            "https://github.com/cli/cli.git",
            "https://github.com/cli/cli",
            "https://github.com/cli/cli/",
            "ssh://git@github.com/cli/cli.git",
            "https://user:token@github.com/cli/cli.git",
            "github.com/cli/cli",
            "https://www.github.com/cli/cli",
            "git@github.corp.example:cli/cli.git",
        ] {
            assert_eq!(
                parse_slug(url).as_deref(),
                Some("cli/cli"),
                "failed to read a slug out of {url}"
            );
        }
    }

    #[test]
    fn non_github_remotes_have_no_slug() {
        for url in [
            // every TestRepo's origin
            "/private/var/folders/xx/T/.tmpAbCdEf/origin.git",
            "../origin.git",
            "git@gitlab.com:cli/cli.git",
            "https://bitbucket.org/cli/cli.git",
            "https://git.corp.example/cli/cli.git",
            "https://github.com/cli",
            "https://github.com/cli/cli/blob/trunk/README.md",
            "",
        ] {
            assert_eq!(
                parse_slug(url),
                None,
                "{url} must not read as a GitHub slug"
            );
        }
    }

    // ── argv ─────────────────────────────────────────────────────────────────

    /// A regression with teeth: `gh auth status` has no `--repo` flag and exits 1 when it
    /// is handed one. A base command that carried the flag therefore reported *every*
    /// authenticated machine as unauthenticated — `available()` false, rung 2 skipped,
    /// every real squash merge quietly demoted to `unknown`. Nothing hermetic could see
    /// it, because the fixture seam never spawns `gh` at all; only the live test caught
    /// it, and this pins it.
    #[test]
    fn the_repo_flag_rides_with_the_subcommands_that_accept_it() {
        let gh = Gh {
            root: PathBuf::from("/nonexistent"),
            slug: OnceLock::from(Some("denoland/deno".to_string())),
            mode: GhCfg::Always,
            fixtures: None,
            authed: OnceLock::new(),
        };
        let argv = |c: Command| {
            c.get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            argv(gh.cmd(&["auth", "status"])),
            ["auth", "status"],
            "`gh auth status` takes no --repo; adding one turns the probe into a hard no"
        );
        assert_eq!(
            argv(gh.pr_cmd(&["pr", "view", "36723", "--json", PR_FIELDS])),
            [
                "pr",
                "view",
                "36723",
                "--json",
                PR_FIELDS,
                "--repo",
                "denoland/deno"
            ]
        );
        // No slug, no flag — `gh` falls back to inferring the repo from `root`.
        let anon = Gh {
            slug: OnceLock::from(None),
            ..gh
        };
        assert_eq!(argv(anon.pr_cmd(&["pr", "list"])), ["pr", "list"]);
    }

    // ── the live path ────────────────────────────────────────────────────────

    /// The recorded corpus is only worth anything while it is still what `gh` prints, and
    /// the fixture seam means no ordinary test ever exercises the subprocess, the argv, or
    /// the parse of a live response. This closes both holes at once: it runs the real
    /// `gh`, against real public repositories, and demands the wire answer agree with the
    /// recording byte for byte in every field rung 2 uses.
    ///
    /// Read-only, and `#[ignore]`d because it needs a network and an authenticated `gh`:
    ///
    /// ```text
    /// cargo test --lib gh::tests::live -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "hits the network: re-validates tests/fixtures/gh/ against live `gh`"]
    fn live_gh_still_prints_what_the_corpus_recorded() {
        fn live(slug: &str) -> Gh {
            Gh {
                root: std::env::current_dir().unwrap(),
                slug: OnceLock::from(Some(slug.to_string())),
                mode: GhCfg::Always,
                fixtures: None,
                authed: OnceLock::new(),
            }
        }
        fn recorded(json: &str) -> PrInfo {
            serde_json::from_str::<PrWire>(json).unwrap().into_info()
        }
        #[track_caller]
        fn same(live: &PrInfo, rec: &PrInfo) {
            assert_eq!(live.number, rec.number);
            assert_eq!(live.state, rec.state);
            assert_eq!(live.merged_at, rec.merged_at);
            assert_eq!(live.merge_commit, rec.merge_commit);
            assert_eq!(live.head_ref_oid, rec.head_ref_oid);
            assert_eq!(live.url, rec.url);
        }

        let deno = live("denoland/deno");
        assert!(
            deno.available(),
            "`gh auth status` must succeed to run this"
        );

        // The squash merge — the shape rung 2 exists for.
        same(&deno.pr_view(36723).unwrap(), &recorded(VIEW_SQUASH));
        // A closed-unmerged PR, over the wire, is still an answer and not an error.
        same(&deno.pr_view(36734).unwrap(), &recorded(VIEW_CLOSED));
        // `MERGED` with no merge commit at all.
        same(
            &live("rails/rails").pr_view(13).unwrap(),
            &recorded(VIEW_MERGED_NO_COMMIT),
        );
        // The merge-commit flavour, from a repo that does not squash.
        same(
            &live("cli/cli").pr_view(9000).unwrap(),
            &recorded(VIEW_MERGE_COMMIT),
        );

        // …and the rung that exists because `pr:` is so often null.
        let rec_list: Vec<PrInfo> = serde_json::from_str::<Vec<PrWire>>(LIST_MERGED)
            .unwrap()
            .into_iter()
            .map(PrWire::into_info)
            .collect();
        let list = deno.pr_for_head(RECORDED_BRANCH).unwrap();
        same(
            merged_pr(&list).expect("that branch really did land"),
            merged_pr(&rec_list).unwrap(),
        );

        // A branch GitHub has never heard of: exit 0, `[]`, and not one word of "no".
        let none = deno.pr_for_head("kanspec/no-such-branch-xyz").unwrap();
        assert!(none.is_empty());
        assert!(merged_pr(&none).is_none());

        // A PR number that does not exist: `gh` exits 1, and that is inconclusive.
        let missing = why(deno.pr_view(999_999_999));
        assert!(missing.contains("exited 1"), "{missing}");
        assert!(missing.contains("PullRequest"), "{missing}");
    }

    // ── evidence formatting ──────────────────────────────────────────────────

    #[test]
    fn evidence_is_one_bounded_line() {
        let raw = format!(
            "GraphQL: nope\n  (repository.pullRequest)\n{}",
            "x".repeat(500)
        );
        let d = detail(&raw);
        assert!(!d.contains('\n'));
        assert!(d.starts_with("GraphQL: nope (repository.pullRequest) x"));
        assert_eq!(
            d.chars().count(),
            DETAIL_MAX + 1,
            "truncated, plus the ellipsis"
        );
    }
}
