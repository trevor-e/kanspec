//! `comments / comment add|reply|resolve / promote / expire` — the review loop.
//!
//! Comments travel browser -> localhost POST -> in-repo JSONL -> `--json` CLI read
//! (invariant 7). Never a clipboard hop; never server-only memory. The `quote` field
//! captures the item's text at comment time, so threads survive edits and a deleted item
//! moves its thread to a visible orphan tray instead of losing it (invariant 5).
//!
//! Owner: **V2**, v0.2.

// Wave-0 skeleton. The bodies below are unwritten; these two allows exist ONLY so the
// skeleton compiles clippy-clean and MUST be deleted by V2 when the bodies land.
#![allow(unused_variables, dead_code)]

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cli::{CommentArgs, CommentsArgs, ExpireArgs, PromoteArgs};
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::ids::{CommentId, ItemRef};
use crate::out::{Render, Style};
use crate::{fix, fixes};

/// One wording for "this verb is v0.2", mirroring `ci::not_yet_v02` (S7's precedent).
///
/// **Round-D hardening.** These handlers were `todo!()`, so every hidden v0.2 arm exited
/// **101** with a backtrace and no `--json` envelope — outside §2.1's documented code set
/// (0/1/2/64/69/70). A `Gate` exits 1 and carries its fix list.
fn not_yet_v02(ctx: &Ctx, verb: &str, what: &str) -> KsError {
    KsError::gate(
        "comment_v02",
        format!("`{verb}` lands in v0.2 — {what}"),
        fixes![
            fix!("{} board", ctx.invoked_as),
            fix!("{} decide \"...\" --scope \"src/**\"", ctx.invoked_as),
            fix!("{} quirk add \"...\" --paths \"src/**\"", ctx.invoked_as),
        ],
    )
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
pub struct CommentsReport {
    pub threads: Vec<Thread>,
    /// threads whose target item no longer exists — visible, never silently lost
    pub orphaned: Vec<Thread>,
    pub next: Vec<String>,
}

/// `{target, quote, body}` is the whole payload an agent needs — self-locating with zero
/// page context.
#[derive(Debug, Serialize)]
pub struct Thread {
    pub id: CommentId,
    pub target: String,
    pub quote: Option<String>,
    pub body: String,
    pub author: Option<String>,
    pub at: DateTime<Utc>,
    pub replies: Vec<Reply>,
    pub resolved: Option<String>,
    /// the stored quote no longer matches the item — "edited since — view diff"
    pub edited_since: bool,
}

#[derive(Debug, Serialize)]
pub struct Reply {
    pub by: String,
    pub body: String,
    pub at: DateTime<Utc>,
}

/// V2: read comments.jsonl, dedupe by `(id, op, at)` (D-19), fold ops into threads, filter
/// by `--unresolved`.
///
/// It refuses rather than printing an empty thread list: "no comments" and "kanspec cannot
/// read comments yet" are different facts, and an agent told the first would stop looking.
pub fn comments(ctx: &Ctx, a: &CommentsArgs) -> Result<CommentsReport> {
    Err(not_yet_v02(
        ctx,
        "comments",
        "the comment store lands in v0.2 — an empty thread list here would read as \
         `no open threads`, which is a different fact",
    ))
}

impl Render for CommentsReport {
    /// V2: one block per thread — target, quote, body, replies — then the orphan tray.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct CommentReport {
    pub id: CommentId,
    pub op: &'static str,
    pub target: Option<String>,
    pub next: Vec<String>,
}

/// V2: `Op::AppendJsonl` one row; `resolve` requires `--note`, which is what changed.
pub fn comment(ctx: &Ctx, a: &CommentArgs) -> Result<CommentReport> {
    Err(not_yet_v02(
        ctx,
        "comment",
        "there is nothing to comment on until `propose` and the review page land in v0.2",
    ))
}

impl Render for CommentReport {
    /// V2: `▸ cm-88f1 resolved` plus the remaining open-thread count.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct PromoteReport {
    pub item: ItemRef,
    pub record: String,
    /// always `proposed` for a decision — agents never self-accept (invariant 8)
    pub status: String,
    pub url: Option<String>,
    pub next: Vec<String>,
}

/// V2: mint the standing record with `source:` pre-filled from the item anchor; a decision
/// lands PROPOSED (invariant 8 — agents never self-accept).
pub fn promote(ctx: &Ctx, a: &PromoteArgs) -> Result<PromoteReport> {
    Err(not_yet_v02(
        ctx,
        "promote",
        "promotion reads its `source:` from a proposal item anchor, which lands in v0.2 — \
         `decide` mints the same standing record by hand today",
    ))
}

impl Render for PromoteReport {
    /// V2: `▸ D-8c1a created (proposed) · source p-7de2#p1 · accept: <url>`.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct ExpireReport {
    pub item: ItemRef,
    pub reason: String,
    pub next: Vec<String>,
}

/// V2: record the expiry disposition; auto-suggested the moment a `(temp until t-x)` guard
/// ticket lands.
pub fn expire(ctx: &Ctx, a: &ExpireArgs) -> Result<ExpireReport> {
    Err(not_yet_v02(
        ctx,
        "expire",
        "an expiry is a disposition on a proposal item, and items land in v0.2",
    ))
}

impl Render for ExpireReport {
    /// V2: `✕ p-7de2#p2 expired — <reason>`.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        next_lines(&self.next, w, st)
    }
}
