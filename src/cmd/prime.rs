//! `prime` — the agent working set, injected by the SessionStart and PreCompact hooks
//! (~1.5k tokens).
//!
//! Two sections: (1) the standing rules, **byte-identical** to `kanspec rules`; (2) the
//! live slice — the claimed ticket, unresolved comment counts, the ready-queue top 5, and
//! the `status` anomaly lines. Path-scoped injection is the context-economy answer: an
//! agent in `src/auth/` never pays for the billing quirks.
//!
//! Owner: **S6**.

// Wave-0 skeleton. The bodies below are `todo!("S6: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S6 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::PrimeArgs;
use crate::ctx::Ctx;
use crate::derive::{Attention, Derived};
use crate::error::Result;
use crate::ids::TicketId;
use crate::model::Snapshot;
use crate::out::{Render, Style};
use crate::rulesdoc::{RulesDoc, Scope};

#[derive(Debug, Serialize)]
pub struct PrimeReport {
    /// `prime --json .standing == rules --json .data`
    pub standing: RulesDoc,
    pub live: LiveSlice,
    pub scope: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LiveSlice {
    pub claimed: Option<TicketId>,
    pub claimed_title: Option<String>,
    pub unresolved: usize,
    pub ready: Vec<TicketId>,
    pub anomalies: Vec<Attention>,
    /// the bounded failure excerpt, when the claimed branch has a fresh failed local job
    pub ci_digest: Option<String>,
}

pub fn prime(ctx: &Ctx, a: &PrimeArgs) -> Result<PrimeReport> {
    todo!("S6: Scope from --path or the branch's changed paths, rulesdoc::build, derive::compute, then the live slice")
}

/// The payload, produced in **exactly one place**. `rules` writes the first line of this
/// and stops; `prime` writes all of it. That is the whole of invariant 3.
pub fn payload(ctx: &Ctx, s: &Snapshot, dv: &Derived, scope: &Scope) -> String {
    let standing = crate::rulesdoc::render_text(&crate::rulesdoc::build(s, scope));
    format!("{standing}\n{}", live_slice(ctx, s, dv))
}

fn live_slice(ctx: &Ctx, s: &Snapshot, dv: &Derived) -> String {
    todo!("S6: the claimed ticket, unresolved counts, ready top 5, and the status anomaly lines")
}

impl Render for PrimeReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: write payload() verbatim — it MUST start with the exact bytes `kanspec rules` prints")
    }
}
