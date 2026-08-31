//! `doctor [--fix]` — prove the invariants; **exit 1 on violation** (run it in CI).
//!
//! Owner: **S4**.

// Wave-0 skeleton. The bodies below are `todo!("S4: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S4 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::DoctorArgs;
use crate::ctx::Ctx;
use crate::doctor::{Finding, Severity};
use crate::error::{code, Result};
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub findings: Vec<Finding>,
    pub fixed: Vec<String>,
    pub checks_run: usize,
    pub next: Vec<String>,
}

impl DoctorReport {
    /// A **successful** run can still exit non-zero, which is why `dispatch` returns
    /// `Result<u8>` rather than going through the error renderer.
    pub fn exit_code(&self) -> u8 {
        if self.findings.iter().any(|f| f.severity == Severity::Error) {
            code::VIOLATION
        } else {
            code::OK
        }
    }
}

pub fn doctor(ctx: &Ctx, a: &DoctorArgs) -> Result<DoctorReport> {
    todo!(
        "S4: doctor::run_all; with --fix, doctor::plan_fixes through Store::transact, then re-run"
    )
}

impl Render for DoctorReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S4: one out::Line per finding with its fix; `✓ N checks passed` when clean")
    }
}
