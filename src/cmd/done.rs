//! THE close-out gate — interactive and `--json`, one typed value between them.
//!
//! Four cheap steps: verify merged (refuse otherwise; `--no-code --why` is the recorded
//! escape), leftover triage (spawn / drop-with-reason / actually-done — **no fourth
//! option**), the knowledge checkpoint, then log the transition and flag the proposal
//! settling if this was its last live ticket.
//!
//! `plan_done` takes a `Landed`, whose two constructors are both sealed in `scan.rs`, so a
//! `done` that never consulted git **does not compile**.
//!
//! Owner: **S5**.

// Wave-0 skeleton. The bodies below are `todo!("S5: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S5 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::DoneArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::ids::{DecisionId, Minter, ProposalId, QuirkId, TicketId};
use crate::model::Snapshot;
use crate::out::{Render, Style};
use crate::plan::{DoneFacts, Plan};
use crate::transitions::State;
use crate::triage::{SpecCheck, Triage};

#[derive(Debug, Serialize)]
pub struct DoneReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    /// how the gate was satisfied: the ladder's badge, or the recorded `--no-code` reason
    pub landed: String,
    pub method: Option<String>,
    pub sha: Option<String>,
    pub spawned: Vec<TicketId>,
    pub dropped_steps: Vec<String>,
    pub marked_done: Vec<usize>,
    pub quirks: Vec<QuirkId>,
    pub decisions: Vec<DecisionId>,
    pub spec_check: SpecCheck,
    /// the proposal whose last live ticket this was
    pub settling: Option<ProposalId>,
    /// "parked 2 discovered tickets" — nothing captured along the way rots silently
    pub discovered: Vec<TicketId>,
    pub next: Vec<String>,
}

pub fn done(ctx: &Ctx, a: &DoneArgs) -> Result<DoneReport> {
    todo!("S5: scan::proof_for_done (or NoCodeWaiver), git changed_paths, then Triage::from_args in --json mode / ::prompt otherwise, all BEFORE transact(Verb::Done, .., plan_done)")
}

/// PURE. Refuses a `NoCodeWaiver` from `review`, so `--no-code` provably cannot bypass the
/// gate on an already-shipped ticket.
pub fn plan_done(
    s: &Snapshot,
    f: &DoneFacts,
    t: &Triage,
    a: &DoneArgs,
    m: &Minter,
) -> Result<Plan> {
    todo!("S5: require(Done); refuse Landed::NoCode from review; mint followup tickets, quirks and proposed decisions; MarkSteps; Op::Transition last")
}

impl Render for DoneReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S5: the DESIGN.md close-out transcript — the ✓ merged line, the triage results, the settling prompt")
    }
}
