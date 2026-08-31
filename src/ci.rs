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

// Wave-0 skeleton. The bodies below are `todo!("S7: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S7 when the bodies land.
#![allow(unused_variables, dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::CiArgs;
use crate::config::CiProvider;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::ids::TicketId;
use crate::out::{Render, Style};

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
pub fn detect_provider(ctx: &Ctx) -> CiProvider {
    todo!("S7: honour an explicit [ci] provider; else the homerunner config scan, then gh auth, then None")
}

pub fn status_for(ctx: &Ctx, id: &TicketId) -> Result<CiStatus> {
    todo!("S7: ticket -> head SHA (branch fallback) -> provider query")
}

/// `kanspec ci why t-9c41` — resolves ticket -> SHA -> latest failed `gh_job_id` and
/// shells to `homerunner why <id> --json` at the configured absolute path. The excerpt
/// heuristics live in that binary and stay there.
pub fn why(ctx: &Ctx, id: &TicketId) -> Result<Digest> {
    todo!("S7: resolve the newest failed job for the ticket's SHA, then shell to homerunner why --json")
}

/// A normal read-only connection with `PRAGMA query_only=ON` — **never** immutable/URI-ro
/// mode, because the live data sits in the WAL.
#[cfg(feature = "ci-homerunner")]
pub fn homerunner_status(ctx: &Ctx, sha: &str) -> Result<CiStatus> {
    todo!("S7: open the journal read-only, aggregate the newest run's job rows ORDER BY COALESCE(completed_at, started_at) DESC")
}

/// `gh run list --commit <sha>` — the same chip and the same rules for repos without
/// homerunner.
pub fn gh_status(ctx: &Ctx, sha: &str) -> Result<CiStatus> {
    todo!("S7: gh run list --commit <sha> --json conclusion,status,name")
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
    todo!("S7: `ci` lists per-ticket CI state for non-terminal tickets; `ci why <id>` returns the digest")
}

impl Render for CiReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S7: one chip per ticket — `✓ CI` / `✗ api-fuzz` / `● running` / `– no local CI seen` — then the digest")
    }
}
