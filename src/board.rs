//! `BoardModel` — ONE struct for `kanspec board` **and** `/api/board`.
//!
//! The terminal board, the SPA and `board --export` all render this same value, so the
//! browser and the terminal cannot disagree about what a column contains. Every derived
//! field on it comes from `derive::compute`, which has exactly one implementation of
//! "stalled".
//!
//! Owner: **S8**.

// Wave-0 skeleton. The bodies below are `todo!("S8: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S8 when the bodies land.
#![allow(unused_variables, dead_code)]

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::ctx::Ctx;
use crate::derive::{Attention, Badge, Column, Staleness};
use crate::error::Result;
use crate::ids::{ProposalId, SpecName, TicketId};
use crate::model::Snapshot;

#[derive(Debug, Clone, Serialize)]
pub struct BoardModel {
    /// the snapshot revision this was built from — the SSE payload carries it (D-22/23)
    pub rev: u64,
    pub generated_at: DateTime<Utc>,
    pub columns: Vec<ColumnModel>,
    /// the pinned strip at the top of every tab
    pub attention: Vec<Attention>,
    /// the pinned strip at the bottom of every tab
    pub features: Vec<FeatureChip>,
    pub worktrees: Vec<WorktreeRowModel>,
    pub cache_age_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnModel {
    pub column: Column,
    pub title: &'static str,
    pub cards: Vec<Card>,
}

/// Every field DESIGN.md's card spec names — and the merge badge is **never a guess**.
#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub id: TicketId,
    pub title: String,
    pub spec: Option<SpecName>,
    pub proposal: Option<ProposalId>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub claimed_by: Option<String>,
    pub updated_secs: Option<u64>,
    pub unresolved: usize,
    pub badge: Badge,
    pub stalled_secs: Option<u64>,
    pub discovered_in: Option<TicketId>,
    pub deps: Vec<TicketId>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureChip {
    pub spec: SpecName,
    pub feature: String,
    pub staleness: Staleness,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeRowModel {
    pub path: String,
    pub branch: Option<String>,
    pub ticket: Option<TicketId>,
    pub claimed_by: Option<String>,
    pub last_commit_secs: Option<u64>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub badge: Badge,
}

/// THE builder. `cmd::board`, `/api/board` and `board --export` all call exactly this.
pub fn build(ctx: &Ctx, snap: &Snapshot) -> Result<BoardModel> {
    todo!(
        "S8: derive::compute once, then bucket every ticket by derive::column into the six columns"
    )
}

/// `board --export board.md` — a markdown snapshot for a PR.
pub fn render_markdown(m: &BoardModel) -> String {
    todo!("S8: the DESIGN.md column layout as a markdown table, with badges and the cache age")
}

/// The terminal board.
pub fn render_terminal(m: &BoardModel, st: &crate::out::Style) -> String {
    todo!("S8: comfy-table columns via out::Table, one card block per ticket")
}
