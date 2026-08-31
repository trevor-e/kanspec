//! `rules [--path <file>] [--audit] [--adopt]`.
//!
//! Invariant 3: this command's stdout is **byte-identical** to the standing-rules section
//! of `kanspec prime`, because both call `rulesdoc::build` then `rulesdoc::render_text`
//! and there is physically no second formatter. `tests/invariants_rules.rs` asserts it on
//! the real binary, across several scopes.
//!
//! Owner: **S6**.

// Wave-0 skeleton. The bodies below are `todo!("S6: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S6 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::RulesArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::out::{Render, Style};
use crate::rulesdoc::{AuditWarning, RulesDoc};

#[derive(Debug, Serialize)]
pub struct RulesReport {
    /// `prime --json .standing == rules --json .data` — the JSON half of invariant 3
    pub data: RulesDoc,
    pub scope: Vec<String>,
    pub warnings: Vec<AuditWarning>,
    pub adopted: Vec<String>,
}

pub fn rules(ctx: &Ctx, a: &RulesArgs) -> Result<RulesReport> {
    todo!("S6: Scope::of(a.paths), rulesdoc::build, plus rulesdoc::audit under --audit and a transact under --adopt")
}

impl Render for RulesReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: write rulesdoc::render_text(&self.data) VERBATIM and nothing else — a header or a trailing newline here breaks invariant 3; audit warnings go to stderr")
    }
}
