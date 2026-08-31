//! `propose / review / approve / close / abandon` — the v0.2 review centrepiece.
//!
//! `close` is THE gate: it refuses until every `[cN]` and `[pN]` item has a disposition,
//! then sets `status: closed` **and** moves the directory to `proposals/closed/` (both, so
//! `doctor` can verify they agree). After that there is no code path by which closed prose
//! reaches a future session — `Snapshot` cannot hold a closed proposal's body.
//!
//! Owner: **V2**, v0.2.

// Wave-0 skeleton. The bodies below are unwritten; these two allows exist ONLY so the
// skeleton compiles clippy-clean and MUST be deleted by V2 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::{AbandonArgs, ApproveArgs, CloseArgs, ProposeArgs, ReviewArgs};
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::ids::{ItemRef, ProposalId, TicketId};
use crate::model::ProposalStatus;
use crate::out::{Render, Style};
use crate::{fix, fixes};

/// One wording for "this verb is v0.2", mirroring `ci::not_yet_v02` (S7's precedent).
///
/// **Round-D hardening.** These handlers were `todo!()`, so every hidden v0.2 arm exited
/// **101** with a backtrace and no `--json` envelope — outside §2.1's documented code set
/// (0/1/2/64/69/70), and indistinguishable to a wrapper script from a crashed tool. A
/// `Gate` exits 1 and carries its fix list, so an agent that reaches for a v0.2 verb is
/// told what to run instead.
fn not_yet_v02(ctx: &Ctx, verb: &str, what: &str) -> KsError {
    KsError::gate(
        "proposal_v02",
        format!("`{verb}` lands in v0.2 — {what}"),
        fixes![
            fix!("{} new \"...\" --spec <spec>", ctx.invoked_as),
            fix!("{} decide \"...\" --scope \"src/**\"", ctx.invoked_as),
            fix!("{} board", ctx.invoked_as),
        ],
    )
}

#[derive(Debug, Serialize)]
pub struct ProposeReport {
    pub id: ProposalId,
    pub title: String,
    pub status: ProposalStatus,
    pub path: String,
    pub next: Vec<String>,
}

/// V2: mint the `p-` id and scaffold proposal.md — Why / Changes / Prescriptions / Tickets.
pub fn propose(ctx: &Ctx, a: &ProposeArgs) -> Result<ProposeReport> {
    Err(not_yet_v02(
        ctx,
        "propose",
        "the proposal surface (review threads, the disposition ledger) is the v0.2 cut",
    ))
}

impl Render for ProposeReport {
    /// V2: the created path, then the `→ kanspec review` next step.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
    }
}

/// The shared placeholder rendering for the reports below. Every one of them is
/// unreachable while its handler refuses — the handler is the type's only constructor — but
/// a real body means reaching one is a wrong answer rather than a panic, and each keeps its
/// intended transcript in the doc comment above it for V2 to write.
fn next_lines(next: &[String], w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
    for n in next {
        crate::out::Line::new(crate::out::glyph::FIX, "next")
            .fix(n.as_str())
            .write(w, st)?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ReviewReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    pub url: String,
    pub unresolved: usize,
    pub next: Vec<String>,
}

/// V2: draft -> review, then print the review page URL.
pub fn review(ctx: &Ctx, a: &ReviewArgs) -> Result<ReviewReport> {
    Err(not_yet_v02(
        ctx,
        "review",
        "the review page and its comment threads are the v0.2 cut",
    ))
}

impl Render for ReviewReport {
    /// V2: the review URL and the open-thread count.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
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

/// V2: REFUSE while unresolved threads exist (a human resolving one is the recorded
/// waiver); stamp who/when; mint the `[tN]` items.
pub fn approve(ctx: &Ctx, a: &ApproveArgs) -> Result<ApproveReport> {
    Err(not_yet_v02(
        ctx,
        "approve",
        "approval is gated on the unresolved-thread count, which needs the v0.2 comment store",
    ))
}

impl Render for ApproveReport {
    /// V2: who approved, when, and the minted ticket ids.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
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

/// V2: enumerate every `[cN]`/`[pN]`; auto-recognize shipped items and expirable temps;
/// REFUSE naming the exact command per unmet item; else stamp the ledger and
/// `Op::MoveDir` into `proposals/closed/`.
pub fn close(ctx: &Ctx, a: &CloseArgs) -> Result<CloseReport> {
    Err(not_yet_v02(
        ctx,
        "close",
        "THE disposition gate needs the v0.2 item ledger — closing a proposal without it \
         would drop prescriptions silently, which is the one thing this verb exists to stop",
    ))
}

impl Render for CloseReport {
    /// V2: the DESIGN.md ledger transcript, one `· cN <disposition>` per item.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct AbandonReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    pub why: String,
    pub next: Vec<String>,
}

/// V2: status -> abandoned with the recorded why; linked tickets are NOT touched.
pub fn abandon(ctx: &Ctx, a: &AbandonArgs) -> Result<AbandonReport> {
    Err(not_yet_v02(
        ctx,
        "abandon",
        "there are no proposals to abandon until `propose` lands in v0.2",
    ))
}

impl Render for AbandonReport {
    /// V2: `✕ p-7de2 abandoned — <why>`.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
    }
}
