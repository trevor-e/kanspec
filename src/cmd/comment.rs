//! `comments / comment add|reply|resolve / promote / expire` — the review loop.
//!
//! Comments travel browser -> localhost POST -> in-repo JSONL -> `--json` CLI read
//! (invariant 7). Never a clipboard hop; never server-only memory. The `quote` field
//! captures the item's text at comment time, so threads survive edits and a deleted item
//! moves its thread to a visible orphan tray instead of losing it (invariant 5).
//!
//! Owner: **V2**, v0.2.

// Wave-0 skeleton. The bodies below are `todo!("V2: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by V2 when the bodies land.
#![allow(unused_variables, dead_code)]

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cli::{CommentArgs, CommentsArgs, ExpireArgs, PromoteArgs};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::ids::{CommentId, ItemRef};
use crate::out::{Render, Style};

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

pub fn comments(ctx: &Ctx, a: &CommentsArgs) -> Result<CommentsReport> {
    todo!("V2: read comments.jsonl, dedupe by (id, op, at) (D-19), fold ops into threads, filter by --unresolved")
}

impl Render for CommentsReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: one block per thread — target, quote, body, replies — then the orphan tray")
    }
}

#[derive(Debug, Serialize)]
pub struct CommentReport {
    pub id: CommentId,
    pub op: &'static str,
    pub target: Option<String>,
    pub next: Vec<String>,
}

pub fn comment(ctx: &Ctx, a: &CommentArgs) -> Result<CommentReport> {
    todo!("V2: Op::AppendJsonl one row; `resolve` requires --note, which is what changed")
}

impl Render for CommentReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: `▸ cm-88f1 resolved` plus the remaining open-thread count")
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

pub fn promote(ctx: &Ctx, a: &PromoteArgs) -> Result<PromoteReport> {
    todo!("V2: mint the standing record with `source:` pre-filled from the item anchor; a decision lands PROPOSED")
}

impl Render for PromoteReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: `▸ D-8c1a created (proposed) · source p-7de2#p1 · accept: <url>`")
    }
}

#[derive(Debug, Serialize)]
pub struct ExpireReport {
    pub item: ItemRef,
    pub reason: String,
    pub next: Vec<String>,
}

pub fn expire(ctx: &Ctx, a: &ExpireArgs) -> Result<ExpireReport> {
    todo!("V2: record the expiry disposition; auto-suggested the moment a `(temp until t-x)` guard ticket lands")
}

impl Render for ExpireReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: `✕ p-7de2#p2 expired — <reason>`")
    }
}
