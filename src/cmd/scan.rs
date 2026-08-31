//! `scan [--explain] [--confirm <id> --why "…"] [--quiet] [--no-fetch]`.
//!
//! Results go to **the cache only**. The one exception is `--confirm`, which appends an
//! attributed `Verb::Confirm` line to the TICKET'S `## Log` rather than a cache entry: a
//! human attestation is an asserted act with an actor, so it must survive `rm -rf cache/`
//! and be visibly signed (D-11).
//!
//! Owner: **S3**.

// Wave-0 skeleton. The bodies below are `todo!("S3: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S3 when the bodies land.
#![allow(unused_variables, dead_code)]

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cli::ScanArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::git::RungTrace;
use crate::ids::TicketId;
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
pub struct ScanReport {
    pub scanned: usize,
    pub landed: Vec<ScanRow>,
    pub not_landed: Vec<ScanRow>,
    pub unknown: Vec<ScanRow>,
    pub fetch_age_secs: Option<u64>,
    pub checked_at: DateTime<Utc>,
    /// present when `--confirm` recorded an attestation
    pub confirmed: Option<TicketId>,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ScanRow {
    pub id: TicketId,
    pub title: String,
    pub badge: String,
    pub method: Option<String>,
    pub sha: Option<String>,
    pub pr: Option<u64>,
    /// populated only with `--explain`, and always from the ladder run that produced this
    /// verdict — there is no second run that could disagree
    pub trace: Vec<RungTrace>,
}

pub fn scan(ctx: &Ctx, a: &ScanArgs) -> Result<ScanReport> {
    todo!("S3: --confirm takes the plan_confirm path; else scan::scan_all OUTSIDE the lock, then a short transact for Op::WriteGitState and the KANSPEC-*.md regeneration (D-20)")
}

impl Render for ScanReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S3: nothing at all when --quiet; else one line per ticket, and the full rung table under --explain")
    }
}
