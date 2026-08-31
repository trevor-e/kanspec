//! `repair <id> --why "…"` — the recorded log reset (D-12).
//!
//! Without it, replay-on-commit turns any imported or already-broken repo into a
//! permanently unwritable one. `Verb::Repair` is the ONE verb whose logged state is
//! authoritative in `transitions::replay`, which is precisely why it demands a human
//! attestation and records the actor.
//!
//! Owner: **S3**.

// Wave-0 skeleton. The bodies below are `todo!("S3: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S3 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::RepairArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::ids::{Minter, TicketId};
use crate::model::Snapshot;
use crate::out::{Render, Style};
use crate::plan::{Facts, Plan};
use crate::transitions::State;

#[derive(Debug, Serialize)]
pub struct RepairReport {
    pub id: TicketId,
    pub title: String,
    /// what the log replayed to before the repair, if anything
    pub was: Option<String>,
    pub state: State,
    pub why: String,
    pub by: String,
    pub next: Vec<String>,
}

pub fn repair(ctx: &Ctx, a: &RepairArgs) -> Result<RepairReport> {
    todo!("S3: transact(Verb::Repair, .., plan_repair)")
}

/// PURE. The attested state is the ticket's CURRENT frontmatter state — repair records
/// that a human vouched for it, it does not let anyone choose a new one.
pub fn plan_repair(s: &Snapshot, f: &Facts, a: &RepairArgs, m: &Minter) -> Result<Plan> {
    todo!("S3: refuse an empty why; Op::Transition{{verb: Repair}} recording state + actor + why")
}

impl Render for RepairReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S3: what the log said, what it says now, who attested, then `→ kanspec doctor`")
    }
}
