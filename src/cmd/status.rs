//! `status` — THE anti-stuck query. Everything non-terminal, grouped by who owes the next
//! verb (YOU / AGENT / WATCHING), one-command fix per line.
//!
//! "Stuck" means *appearing on a list* — the opposite of silent.
//!
//! Owner: **S4**.

// Wave-0 skeleton. The bodies below are `todo!("S4: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S4 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::StatusArgs;
use crate::ctx::Ctx;
use crate::derive::Attention;
use crate::error::Result;
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub you: Vec<Attention>,
    pub agent: Vec<Attention>,
    pub watching: Vec<Attention>,
    /// `sync = "batch"`: how many tracker changes are sitting uncommitted (D-13)
    pub pending_changes: u32,
    /// how old the merge-detection cache is, so a badge never lies about its freshness
    pub cache_age_secs: Option<u64>,
    pub next: Vec<String>,
}

pub fn status(ctx: &Ctx, a: &StatusArgs) -> Result<StatusReport> {
    todo!("S4: derive::attention grouped by Owner, filtered by --owner, plus git.dirty_kanspec and the cache age")
}

impl Render for StatusReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S4: the DESIGN.md status transcript — ` YOU (3)` headers with one out::Line per attention item")
    }
}
