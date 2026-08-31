//! `new / show / ls / log / where`.
//!
//! Owner: **S5**.

// Wave-0 skeleton. The bodies below are `todo!("S5: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S5 when the bodies land.
#![allow(unused_variables, dead_code)]

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cli::{LogArgs, LsArgs, NewArgs, ShowArgs, WhereArgs};
use crate::ctx::Ctx;
use crate::derive::{Badge, Column};
use crate::error::Result;
use crate::ids::{ProposalId, SpecName, TicketId};
use crate::logentry::LogEntry;
use crate::model::Step;
use crate::out::{Render, Style};
use crate::plan::{Facts, Plan};
use crate::transitions::State;

// ── new ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct NewReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub spec: Option<SpecName>,
    pub proposal: Option<ProposalId>,
    pub deps: Vec<TicketId>,
    pub followup_of: Option<TicketId>,
    /// stamped automatically from the session's claimed ticket unless `--no-link`
    pub discovered_in: Option<TicketId>,
    pub path: String,
    pub next: Vec<String>,
}

pub fn new(ctx: &Ctx, a: &NewArgs) -> Result<NewReport> {
    todo!("S5: resolve --from / the session's claimed ticket for discovered_in, then Store::transact(Verb::New, .., plan_new)")
}

/// PURE. `discovered_in` is already resolved into `a` by the handler — a planner never
/// asks the world anything.
pub fn plan_new(
    s: &crate::model::Snapshot,
    f: &Facts,
    a: &NewArgs,
    m: &crate::ids::Minter,
) -> Result<Plan> {
    todo!("S5: mint the id, Op::CreateEntity with the scaffolded body, Op::Transition{{verb: New}}")
}

impl Render for NewReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!(
            "S5: `created t-9c41  <title>` plus the file path and the `→ kanspec start` next step"
        )
    }
}

// ── show ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ShowReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub column: Column,
    pub badge: Badge,
    pub spec: Option<SpecName>,
    pub proposal: Option<ProposalId>,
    pub deps: Vec<TicketId>,
    pub blocked_by: Vec<TicketId>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub claimed_by: Option<String>,
    pub pr: Option<u64>,
    pub head: Option<String>,
    pub created: DateTime<Utc>,
    pub steps: Vec<Step>,
    pub log: Vec<LogEntry>,
    /// the ONE owed verb, already spelled out
    pub next: Vec<String>,
}

pub fn show(ctx: &Ctx, a: &ShowArgs) -> Result<ShowReport> {
    todo!("S5: snapshot + derive::{{badge, column, blocked_by}}, then the owed verb")
}

impl Render for ShowReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: the header line, the context block, the steps, then the owed verb")
    }
}

// ── ls ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct LsReport {
    pub rows: Vec<LsRow>,
    pub filtered: Vec<String>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct LsRow {
    pub id: TicketId,
    pub state: State,
    pub title: String,
    pub spec: Option<SpecName>,
    pub claimed_by: Option<String>,
    pub badge: Badge,
    pub updated_secs: Option<u64>,
}

pub fn ls(ctx: &Ctx, a: &LsArgs) -> Result<LsReport> {
    todo!("S5: apply --spec/--mine/--stalled/--unmerged/--all; non-terminal only unless --all")
}

impl Render for LsReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: out::Table with the glyph, id, title, spec chip, claimant and badge")
    }
}

// ── log ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct LogReport {
    pub id: TicketId,
    pub entries: Vec<LogEntry>,
    /// what `transitions::replay` makes of the trail — a broken log names `kanspec repair`
    pub replays_to: Option<State>,
    pub violation: Option<String>,
    pub next: Vec<String>,
}

pub fn log(ctx: &Ctx, a: &LogArgs) -> Result<LogReport> {
    todo!("S5: print the trail and its replay verdict, so a hand-edit is visible here too")
}

impl Render for LogReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: the raw log lines, then the replay verdict")
    }
}

// ── where ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct WhereReport {
    pub branch: Option<String>,
    pub worktree: String,
    pub ticket: Option<TicketId>,
    pub title: Option<String>,
    pub state: Option<State>,
    pub linked_worktree: bool,
    /// where the writes actually land — the point of worktree unification
    pub primary_kanspec: String,
    pub next: Vec<String>,
}

/// `where` is a keyword, so the handler is `where_is`.
pub fn where_is(ctx: &Ctx, a: &WhereArgs) -> Result<WhereReport> {
    todo!("S5: resolve the current (or --branch) branch to the ticket that claims it")
}

impl Render for WhereReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: branch / worktree / ticket, plus the primary .kanspec path")
    }
}
