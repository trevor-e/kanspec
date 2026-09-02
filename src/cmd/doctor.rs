//! `doctor [--fix]` — prove the invariants; **exit 1 on violation** (run it in CI).
//!
//! Owner: **S4**.

use serde::Serialize;

use crate::cli::DoctorArgs;
use crate::ctx::Ctx;
use crate::doctor::{self, Finding, Severity};
use crate::error::{code, Result};
use crate::out::{glyph, Color, Line, Render, Style};
use crate::store::Store;

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
    ctx.require_initialized()?;
    let mut findings = doctor::run_all(&ctx.snapshot()?);
    let mut fixed: Vec<String> = Vec::new();

    if a.fix && findings.iter().any(|f| f.fixable) {
        // `--fix` is NOT a second writer: it plans inside the lock against a fresh
        // snapshot, exactly like every verb. That is also why the findings are recomputed
        // in there — the ones gathered above were read without exclusivity.
        let store = Store::open(ctx);
        // `None`: `plan_fixes` pushes `Op::MoveDir` and never a ticket transition, so this
        // is not a ticket verb act. Round C, when `transact` gained `Option<Verb>`.
        let committed = store.transact(None, &ctx.invocation(), |snap, _minter| {
            let fresh = doctor::run_all(snap);
            doctor::plan_fixes(snap, &fresh)
        })?;
        fixed = committed
            .touched
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        // Re-prove against what is now on disk, so the report is about the repaired repo
        // rather than the broken one.
        findings = doctor::run_all(&committed.snapshot);
    }

    let mut next: Vec<String> = Vec::new();
    for f in &findings {
        if !next.contains(&f.fix) {
            next.push(f.fix.clone());
        }
        if next.len() == 5 {
            break;
        }
    }
    if !a.fix && findings.iter().any(|f| f.fixable) {
        next.insert(0, format!("{} doctor --fix", ctx.invoked_as));
    }

    Ok(DoctorReport {
        findings,
        fixed,
        checks_run: doctor::CHECKS.len(),
        next,
    })
}

impl Render for DoctorReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        for path in &self.fixed {
            writeln!(
                w,
                " {} repaired {path}",
                crate::out::paint(&glyph::OK.to_string(), Color::Green, st.color)
            )?;
        }
        for f in &self.findings {
            let g = match f.severity {
                Severity::Error => glyph::FAIL,
                Severity::Warning => '⚠',
            };
            Line::new(g, format!("{}  {}", f.check, f.message))
                .id(&f.subject)
                .fix(&f.fix)
                .write(w, st)?;
        }
        if self.findings.is_empty() {
            writeln!(
                w,
                " {} {} checks passed",
                crate::out::paint(&glyph::OK.to_string(), Color::Green, st.color),
                self.checks_run
            )?;
        } else {
            let errors = self
                .findings
                .iter()
                .filter(|f| f.severity == Severity::Error)
                .count();
            writeln!(
                w,
                " {} checks found {errors} error(s) and {} warning(s)",
                self.checks_run,
                self.findings.len() - errors
            )?;
        }
        Ok(())
    }
}
