//! `propose / review / approve / close / abandon` — the v0.2 review centrepiece.
//!
//! `close` is THE gate: it refuses until every `[cN]` and `[pN]` item has a disposition,
//! then sets `status: closed` **and** moves the directory to `proposals/closed/` (both, so
//! `doctor` can verify they agree). After that there is no code path by which closed prose
//! reaches a future session — `Snapshot` cannot hold a closed proposal's body.
//!
//! Owner: **V2**, v0.2.

// Wave-0 skeleton. The bodies below are `todo!("V2: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by V2 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::{AbandonArgs, ApproveArgs, CloseArgs, ProposeArgs, ReviewArgs};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::ids::{ItemRef, ProposalId, TicketId};
use crate::model::ProposalStatus;
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
pub struct ProposeReport {
    pub id: ProposalId,
    pub title: String,
    pub status: ProposalStatus,
    pub path: String,
    pub next: Vec<String>,
}

pub fn propose(ctx: &Ctx, a: &ProposeArgs) -> Result<ProposeReport> {
    todo!("V2: mint the p- id and scaffold proposal.md — Why / Changes / Prescriptions / Tickets")
}

impl Render for ProposeReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: the created path, then the `→ kanspec review` next step")
    }
}

#[derive(Debug, Serialize)]
pub struct ReviewReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    pub url: String,
    pub unresolved: usize,
    pub next: Vec<String>,
}

pub fn review(ctx: &Ctx, a: &ReviewArgs) -> Result<ReviewReport> {
    todo!("V2: draft -> review, then print the review page URL")
}

impl Render for ReviewReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: the review URL and the open-thread count")
    }
}

#[derive(Debug, Serialize)]
pub struct ApproveReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    pub approved: String,
    /// the `[tN]` items minted into board tickets
    pub minted: Vec<TicketId>,
    pub next: Vec<String>,
}

pub fn approve(ctx: &Ctx, a: &ApproveArgs) -> Result<ApproveReport> {
    todo!("V2: REFUSE while unresolved threads exist (a human resolving one is the recorded waiver); stamp who/when; mint the [tN] items")
}

impl Render for ApproveReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: who approved, when, and the minted ticket ids")
    }
}

#[derive(Debug, Serialize)]
pub struct CloseReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    /// the disposition ledger, stamped into the closed proposal's frontmatter
    pub ledger: Vec<Disposition>,
    pub moved_to: String,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum Disposition {
    /// auto-detected via the `{p-xxxx}` token
    Shipped {
        item: ItemRef,
        rule: String,
    },
    Promoted {
        item: ItemRef,
        record: String,
    },
    Followup {
        item: ItemRef,
        ticket: TicketId,
    },
    Dropped {
        item: ItemRef,
        note: String,
    },
    /// `(temp until t-x)` whose guard ticket landed
    Expired {
        item: ItemRef,
        reason: String,
    },
}

pub fn close(ctx: &Ctx, a: &CloseArgs) -> Result<CloseReport> {
    todo!("V2: enumerate every [cN]/[pN]; auto-recognize shipped items and expirable temps; REFUSE naming the exact command per unmet item; else stamp the ledger and Op::MoveDir into proposals/closed/")
}

impl Render for CloseReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: the DESIGN.md ledger transcript, one `· cN <disposition>` per item")
    }
}

#[derive(Debug, Serialize)]
pub struct AbandonReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    pub why: String,
    pub next: Vec<String>,
}

pub fn abandon(ctx: &Ctx, a: &AbandonArgs) -> Result<AbandonReport> {
    todo!("V2: status -> abandoned with the recorded why; linked tickets are NOT touched")
}

impl Render for AbandonReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: `✕ p-7de2 abandoned — <why>`")
    }
}
