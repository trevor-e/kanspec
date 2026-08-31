//! The CI signal — the fourth computed fact, after merge state, staleness and stuck-ness.
//! **Read-only, advisory, never a state driver**: a red build does not move a card.
//!
//! Absence of a row renders as "no local CI seen", **not** "no CI ran" — enrichment lag
//! and hosted-runner jobs both make absence ambiguous.
//!
//! `rusqlite` sits behind the non-default `ci-homerunner` feature (D-24): bundled SQLite
//! is by far the largest clean-build cost in the set, for code v0.1 never calls.
//!
//! Owner: **S7**, v0.2.
//!
//! **What is real in v0.1 and what is not.** [`detect_provider`] is real: the `[ci]` table
//! parses, an explicit `provider` is honoured, and `auto` resolves the same way it will in
//! v0.2 — homerunner if this repo is one of its, else `gh` if `gh` is authed, else none.
//! The *readers* are v0.2 and refuse with a typed error that says so, rather than
//! panicking or, worse, rendering a confident "no CI" that is really "not implemented".
//! There is deliberately **no `trait CiProvider`**: the second implementation does not
//! exist yet, and ARCHITECTURE.md spends the crate's trait budget on `Render` and `FmKey`.
//! Dispatch is a match on the [`CiProvider`] enum the config already parses.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::CiArgs;
use crate::config::CiProvider;
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::ids::TicketId;
use crate::out::{glyph, Line, Render, Style};
use crate::{fix, fixes};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiState {
    Green,
    Red,
    Running,
    /// no row for this SHA — ambiguous, and labelled as such
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiStatus {
    pub state: CiState,
    /// the failing job name, e.g. `api-fuzz`
    pub job: Option<String>,
    pub sha: Option<String>,
    pub provider: CiProvider,
    pub checked_at: DateTime<Utc>,
    /// homerunner journals only jobs that landed on its runners
    pub local_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Digest {
    pub job: String,
    pub excerpt: String,
    /// `homerunner exec <id>` still works — verified with `docker image inspect`
    pub kept_workspace: Option<String>,
}

/// The origin remote's `owner/name` in homerunner's `[[repos]]` -> homerunner; else `gh`
/// authed -> gh; else none.
///
/// Never returns [`CiProvider::Auto`]: `auto` is a question, and this is the answer.
///
/// The `gh` rung asks `Gh::available()`, which is `git.rs`'s owner's to implement — this is
/// the one call in S7 that reaches into a neighbour's body, and it is why `run` below
/// reports the *configured* provider rather than the resolved one until the readers land.
pub fn detect_provider(ctx: &Ctx) -> CiProvider {
    match ctx.cfg.ci.provider {
        CiProvider::Auto => {
            if homerunner_knows_this_repo(ctx) {
                CiProvider::Homerunner
            } else if ctx.cfg.git.gh != crate::config::GhMode::Never && ctx.gh.available() {
                CiProvider::Gh
            } else {
                CiProvider::None
            }
        }
        explicit => explicit,
    }
}

/// `owner/name` of `origin`, as homerunner's config spells it.
fn origin_slug(ctx: &Ctx) -> Option<String> {
    let out = ctx.git.run(&["remote", "get-url", "origin"]).ok()?;
    if out.code != 0 {
        return None;
    }
    let url = out.out.trim().trim_end_matches(".git");
    let tail = url.rsplit(':').next().unwrap_or(url);
    let parts: Vec<&str> = tail.trim_matches('/').rsplit('/').take(2).collect();
    (parts.len() == 2).then(|| format!("{}/{}", parts[1], parts[0]))
}

/// A textual scan of `~/.config/homerunner/config.toml` for this repo's slug. Deliberately
/// not a parse: kanspec does not own that schema, and a `[[repos]]` block it cannot model
/// must not become a hard error in an advisory signal.
fn homerunner_knows_this_repo(ctx: &Ctx) -> bool {
    let Some(slug) = origin_slug(ctx) else {
        return false;
    };
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let cfg = std::path::Path::new(&home).join(".config/homerunner/config.toml");
    std::fs::read_to_string(cfg).is_ok_and(|t| t.contains(&slug))
}

pub fn status_for(ctx: &Ctx, id: &TicketId) -> Result<CiStatus> {
    let provider = detect_provider(ctx);
    match provider {
        CiProvider::None | CiProvider::Auto => Ok(CiStatus {
            state: CiState::None,
            job: None,
            sha: None,
            provider: CiProvider::None,
            checked_at: ctx.now,
            local_only: false,
        }),
        _ => {
            let _ = id;
            Err(not_yet_v02(ctx, provider))
        }
    }
}

/// `kanspec ci why t-9c41` — resolves ticket -> SHA -> latest failed `gh_job_id` and
/// shells to `homerunner why <id> --json` at the configured absolute path. The excerpt
/// heuristics live in that binary and stay there.
pub fn why(ctx: &Ctx, id: &TicketId) -> Result<Digest> {
    let _ = id;
    Err(not_yet_v02(ctx, detect_provider(ctx)))
}

/// A normal read-only connection with `PRAGMA query_only=ON` — **never** immutable/URI-ro
/// mode, because the live data sits in the WAL.
#[cfg(feature = "ci-homerunner")]
pub fn homerunner_status(ctx: &Ctx, sha: &str) -> Result<CiStatus> {
    let _ = sha;
    Err(not_yet_v02(ctx, CiProvider::Homerunner))
}

/// `gh run list --commit <sha>` — the same chip and the same rules for repos without
/// homerunner.
pub fn gh_status(ctx: &Ctx, sha: &str) -> Result<CiStatus> {
    let _ = sha;
    Err(not_yet_v02(ctx, CiProvider::Gh))
}

/// One wording for "detected, not read yet", so nobody mistakes an unimplemented reader
/// for a repository with no CI.
fn not_yet_v02(ctx: &Ctx, provider: CiProvider) -> KsError {
    KsError::gate(
        "ci_reader_v02",
        format!(
            "CI provider `{}` is detected, but reading it lands in v0.2 — kanspec will not \
             report a build state it did not read",
            provider_name(provider)
        ),
        fixes![
            fix!("gh run list"),
            fix!("set [ci] provider = \"none\" in .kanspec/config.toml"),
            fix!("{} status", ctx.invoked_as),
        ],
    )
}

pub fn provider_name(p: CiProvider) -> &'static str {
    match p {
        CiProvider::Auto => "auto",
        CiProvider::Homerunner => "homerunner",
        CiProvider::Gh => "gh",
        CiProvider::None => "none",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The `ci` command. It lives here, not under `cmd/`, because §1's module tree gives
// the CI surface exactly one file and one owner.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CiReport {
    pub provider: CiProvider,
    pub rows: Vec<CiRow>,
    /// present for `ci why <id>`
    pub digest: Option<Digest>,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CiRow {
    pub id: TicketId,
    pub title: String,
    pub status: CiStatus,
}

pub fn run(ctx: &Ctx, a: &CiArgs) -> Result<CiReport> {
    ctx.require_initialized()?;
    // The CONFIGURED provider, deliberately, not the resolved one: resolving `auto` means
    // asking `gh` whether it is authed, and no v0.1 code path would use the answer. A
    // hidden verb must not shell out — or vary its output by whether the machine running
    // it happens to be logged in — to print a line that says "v0.2".
    let provider = ctx.cfg.ci.provider;
    if let Some(crate::cli::CiCommand::Why { id }) = &a.cmd {
        // `why` is the one place the resolved provider matters, because it is the one
        // place a reader would actually be called.
        let digest = why(ctx, &TicketId::parse(id)?)?;
        return Ok(CiReport {
            provider: detect_provider(ctx),
            rows: Vec::new(),
            digest: Some(digest),
            next: Vec::new(),
        });
    }
    // Deliberately not an error: `ci` is advisory, and refusing to *list* would make a
    // hidden v0.2 verb the one command in the tree whose exit code depends on whether the
    // machine running it happens to have `gh` authed.
    let next = match provider {
        CiProvider::None => vec![
            "[ci] provider = \"none\" — kanspec reads no CI signal here".to_string(),
            format!("{} status", ctx.invoked_as),
        ],
        p => vec![
            format!(
                "per-ticket CI state lands in v0.2 — [ci] provider = \"{}\" is parsed and \
                 kept until then",
                provider_name(p)
            ),
            format!("{} status", ctx.invoked_as),
        ],
    };
    Ok(CiReport {
        provider,
        rows: Vec::new(),
        digest: None,
        next,
    })
}

impl Render for CiReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        writeln!(w, " CI provider: {}", provider_name(self.provider))?;
        for r in &self.rows {
            let chip = match r.status.state {
                CiState::Green => format!("{} CI", glyph::OK),
                CiState::Red => format!(
                    "{} {}",
                    glyph::FAIL,
                    r.status.job.as_deref().unwrap_or("failed")
                ),
                CiState::Running => "● running".to_string(),
                CiState::None => "– no local CI seen".to_string(),
            };
            Line::new('·', format!("{:<10} {chip}", r.title))
                .id(r.id.to_string())
                .write(w, st)?;
        }
        if let Some(d) = &self.digest {
            writeln!(w, " {} {}", glyph::FAIL, d.job)?;
            writeln!(w, "{}", d.excerpt)?;
            if let Some(k) = &d.kept_workspace {
                writeln!(w, " {} homerunner exec {k}", glyph::FIX)?;
            }
        }
        for n in &self.next {
            writeln!(w, " {} {n}", glyph::FIX)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_provider_is_never_second_guessed() {
        // The one property that has to hold today: `provider = "none"` parses and wins, so
        // a repo can opt out of CI detection entirely and nothing shells out.
        let c = crate::config::Config::parse("[ci]\nprovider = \"none\"\n", "<test>").unwrap();
        assert_eq!(c.ci.provider, CiProvider::None);
        let d = crate::config::Config::default();
        assert_eq!(d.ci.provider, CiProvider::Auto, "auto is the default");
        assert_eq!(provider_name(CiProvider::Homerunner), "homerunner");
    }
}
