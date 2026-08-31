//! `ready / start / ship / park / drop` — the daily loop.
//!
//! Every handler here has the same shape as the contract's worked example: gather every
//! subprocess, network call and clock read into a `*Facts` value **before** the lock, then
//! hand a pure planner to `Store::transact`. The server calls these same functions inside
//! `spawn_blocking`, so a POST and the equivalent CLI verb produce byte-identical files.
//!
//! Owner: **S5**.

// Wave-0 skeleton. The bodies below are `todo!("S5: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S5 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::{DropArgs, ParkArgs, ReadyArgs, ShipArgs, StartArgs};
use crate::ctx::Ctx;
use crate::derive::Badge;
use crate::error::Result;
use crate::ids::Minter;
use crate::ids::{ProposalId, QuirkId, SpecName, TicketId};
use crate::model::Snapshot;
use crate::out::{Render, Style};
use crate::plan::{Facts, Plan, ShipFacts, StartFacts};
use crate::transitions::State;

// ── ready ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ReadyReport {
    pub rows: Vec<ReadyRow>,
    /// todo tickets that are NOT ready, and what is holding each one
    pub blocked: Vec<BlockedRow>,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ReadyRow {
    pub id: TicketId,
    pub title: String,
    pub spec: Option<SpecName>,
    pub proposal: Option<ProposalId>,
    pub deps: Vec<TicketId>,
}

#[derive(Debug, Serialize)]
pub struct BlockedRow {
    pub id: TicketId,
    pub title: String,
    pub blocked_by: Vec<TicketId>,
}

pub fn ready(ctx: &Ctx, a: &ReadyArgs) -> Result<ReadyReport> {
    todo!("S5: derive::ready_queue, filtered by --spec and truncated to --limit, plus the blocked tail")
}

impl Render for ReadyReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: one out::Line per claimable ticket, then the blocked list with what holds it")
    }
}

// ── start ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StartReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub branch: String,
    pub worktree: Option<String>,
    pub claimed_by: String,
    /// the context block from the DESIGN.md transcript
    pub spec: Option<SpecName>,
    pub spec_rules: usize,
    pub decisions_in_scope: Vec<String>,
    pub quirks_matching: Vec<QuirkId>,
    pub url: Option<String>,
    pub next: Vec<String>,
}

pub fn start(ctx: &Ctx, a: &StartArgs) -> Result<StartReport> {
    todo!("S5: resolve branch name + head SHA + optional worktree BEFORE the lock into StartFacts, then transact(Verb::Start, .., plan_start)")
}

/// PURE. Refuses a second claim, and `require(from, Start)` supplies the typed refusal.
pub fn plan_start(s: &Snapshot, f: &StartFacts, a: &StartArgs, m: &Minter) -> Result<Plan> {
    todo!("S5: require(Start); Op::Transition with also = [Branch, Worktree, ClaimedBy, Head]")
}

impl Render for StartReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: the four-line `claimed / branch / context / board` transcript from DESIGN.md")
    }
}

// ── ship ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ShipReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub head: String,
    pub pr: Option<u64>,
    pub badge: Badge,
    /// a red or still-running local CI run WARNS, never blocks
    pub ci_warning: Option<String>,
    pub next: Vec<String>,
}

pub fn ship(ctx: &Ctx, a: &ShipArgs) -> Result<ShipReport> {
    todo!("S5: read the head SHA from git BEFORE the lock into ShipFacts, then transact(Verb::Ship, .., plan_ship)")
}

/// PURE. The `head:` value can only come from a `HeadSha`, which only `git.rs` mints —
/// DESIGN.md's second gear, enforced by the type.
pub fn plan_ship(s: &Snapshot, f: &ShipFacts, a: &ShipArgs, m: &Minter) -> Result<Plan> {
    todo!("S5: require(Ship); Op::Transition with also = [Head, Pr]")
}

impl Render for ShipReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: `shipped t-9c41` with the recorded head and PR, then the `→ kanspec done` next step")
    }
}

// ── park ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ParkReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub why: String,
    pub next: Vec<String>,
}

pub fn park(ctx: &Ctx, a: &ParkArgs) -> Result<ParkReport> {
    todo!("S5: transact(Verb::Park, .., plan_park)")
}

/// PURE.
pub fn plan_park(s: &Snapshot, f: &Facts, a: &ParkArgs, m: &Minter) -> Result<Plan> {
    todo!("S5: require(Park); Op::Transition clearing ClaimedBy, with `why` as the log note")
}

impl Render for ParkReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: `parked t-9c41 — <why>` plus the `→ kanspec ready` next step")
    }
}

// ── drop ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DropReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub why: String,
    /// tickets that depended on this one — dropped counts as satisfied (D-17)
    pub unblocked: Vec<TicketId>,
    pub next: Vec<String>,
}

/// `drop_ticket`, not `drop`: shadowing `std::mem::drop` at the call site reads badly.
pub fn drop_ticket(ctx: &Ctx, a: &DropArgs) -> Result<DropReport> {
    todo!("S5: transact(Verb::Drop, .., plan_drop)")
}

/// PURE.
pub fn plan_drop(s: &Snapshot, f: &Facts, a: &DropArgs, m: &Minter) -> Result<Plan> {
    todo!("S5: require(Drop); Op::Transition with `why` as the log note")
}

impl Render for DropReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: `dropped t-9c41 — <why>`, then anything it unblocked")
    }
}
