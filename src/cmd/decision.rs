//! `decide / accept / supersede / revoke / why`.
//!
//! Agents write decisions at exactly three moments and **never self-accept** (invariant 8):
//! `plan_accept` and `plan_revoke` take a `&HumanActor`, whose only constructor refuses an
//! `Actor::Agent`, so an agent session literally cannot call them.
//!
//! Accepted bodies are immutable; the only legal mutations are status flips and back-links.
//!
//! Owner: **S6**.

// Wave-0 skeleton. The bodies below are `todo!("S6: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S6 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::{AcceptArgs, DecideArgs, RevokeArgs, SupersedeArgs, WhyArgs};
use crate::ctx::{Ctx, HumanActor};
use crate::error::Result;
use crate::ids::{DecisionId, Minter, ProposalId, TicketId};
use crate::model::{DecisionStatus, Snapshot};
use crate::out::{Render, Style};
use crate::plan::{Facts, Plan};

// ── decide ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DecideReport {
    pub id: DecisionId,
    pub title: String,
    /// always `proposed` — a mid-implementation discovery is captured, never enacted
    pub status: DecisionStatus,
    pub source: Option<String>,
    pub scope: Vec<String>,
    pub path: String,
    pub url: Option<String>,
    pub next: Vec<String>,
}

pub fn decide(ctx: &Ctx, a: &DecideArgs) -> Result<DecideReport> {
    todo!("S6: transact minting a PROPOSED decision with source pre-filled from --from")
}

/// PURE. Mints `status: proposed` and nothing else — a proposed decision is not injected
/// as a standing rule, it sits in the YOU section of `status` until a human acts.
pub fn plan_decide(s: &Snapshot, f: &Facts, a: &DecideArgs, m: &Minter) -> Result<Plan> {
    todo!("S6: mint the D- id, Op::CreateEntity with the MADR-minimal scaffold")
}

impl Render for DecideReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: `▸ D-8c1a created (proposed) · source p-7de2#p1 · accept: <url>`")
    }
}

// ── accept / supersede / revoke ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DecisionReport {
    pub id: DecisionId,
    pub title: String,
    pub status: DecisionStatus,
    pub by: String,
    /// the new record, for `supersede`
    pub replacement: Option<DecisionId>,
    pub why: Option<String>,
    pub next: Vec<String>,
}

pub fn accept(ctx: &Ctx, a: &AcceptArgs) -> Result<DecisionReport> {
    todo!("S6: HumanActor::require(&ctx.actor, \"accept\")? BEFORE the lock, then transact(plan_accept)")
}

/// Takes `&HumanActor`, so invariant 8 is a type fact rather than a runtime check anyone
/// could forget to write.
pub fn plan_accept(s: &Snapshot, f: &Facts, who: &HumanActor, id: &DecisionId) -> Result<Plan> {
    todo!("S6: refuse unless status == proposed; Op::SetFields{{status: accepted}} — the body freezes here")
}

pub fn supersede(ctx: &Ctx, a: &SupersedeArgs) -> Result<DecisionReport> {
    todo!("S6: mint the replacement as PROPOSED, then flip the old one with bidirectional links")
}

pub fn revoke(ctx: &Ctx, a: &RevokeArgs) -> Result<DecisionReport> {
    todo!("S6: HumanActor::require(&ctx.actor, \"revoke\")?, then transact(plan_revoke)")
}

/// The human's kill switch — also `&HumanActor`.
pub fn plan_revoke(
    s: &Snapshot,
    f: &Facts,
    who: &HumanActor,
    id: &DecisionId,
    why: &str,
) -> Result<Plan> {
    todo!("S6: Op::SetFields{{status: revoked}} plus the why appended to the body's Consequences")
}

impl Render for DecisionReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: `● D-8c1a accepted by trevor` plus the `→ kanspec rules` next step")
    }
}

// ── why ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct WhyReport {
    pub anchor: String,
    pub rule: Option<String>,
    pub proposal: Option<ProposalId>,
    pub item: Option<String>,
    pub tickets: Vec<TicketId>,
    pub prs: Vec<u64>,
    pub decisions: Vec<DecisionId>,
    pub next: Vec<String>,
}

pub fn why(ctx: &Ctx, a: &WhyArgs) -> Result<WhyReport> {
    todo!("S6: walk rule -> {{p-xxxx}} token -> proposal item -> tickets -> PR, for a RuleRef, an ItemRef or a ticket id")
}

impl Render for WhyReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: the chain as indented steps, each naming the record it came from")
    }
}
