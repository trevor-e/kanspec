//! `Triage` — one typed value, two front doors.
//!
//! The interactive prompts and the `--json` flags BOTH construct this, and only this
//! reaches `plan_done`. The agent path and the human path therefore cannot diverge in what
//! they record.
//!
//! Owner: **S5**.

// Wave-0 skeleton. The bodies below are `todo!("S5: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S5 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::DoneArgs;
use crate::error::Result;
use crate::git::ChangedPath;
use crate::ids::SpecName;
use crate::model::{Severity, Snapshot, Ticket};

#[derive(Debug, Clone, Serialize)]
pub struct Triage {
    pub steps: Vec<StepDisposition>,
    pub quirks: Vec<NewQuirk>,
    pub decisions: Vec<NewDecision>,
    pub spec: SpecCheck,
}

/// No fourth option: every unchecked step is spawned, dropped with a reason, or marked
/// actually done.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum StepDisposition {
    Spawn { index: usize, title: String },
    Drop { index: usize, why: String },
    ActuallyDone { index: usize },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "spec_check", rename_all = "snake_case")]
pub enum SpecCheck {
    EditedOnBranch {
        specs: Vec<SpecName>,
    },
    /// the recorded `--spec-unchanged` waiver, visible on the board
    Unchanged {
        why: String,
    },
    /// the branch touched no spec's globs
    NotApplicable,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewQuirk {
    pub title: String,
    pub paths: Vec<String>,
    pub severity: Severity,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewDecision {
    pub title: String,
    pub scope: Vec<String>,
}

impl Triage {
    /// Non-interactive. REFUSES with a typed error naming BOTH flags when neither
    /// `--spawn` nor `--no-followups` is present: clap cannot express "required iff
    /// --json" (`required_if_eq` works on values, and `global + required` is the
    /// debug-only panic recon found), and the handler yields a better message anyway.
    pub fn from_args(
        t: &Ticket,
        a: &DoneArgs,
        touched: &[ChangedPath],
        s: &Snapshot,
    ) -> Result<Triage> {
        todo!("S5: refuse when unchecked steps exist and neither --spawn nor --no-followups was passed; same for quirks/decisions; map --drop-step N:REASON")
    }

    /// Interactive. Same value, prompted one key at a time.
    pub fn prompt(
        t: &Ticket,
        a: &DoneArgs,
        touched: &[ChangedPath],
        s: &Snapshot,
    ) -> Result<Triage> {
        todo!("S5: the DESIGN.md close-out transcript — [s]pawn / [d]rop / [x] actually done, then the knowledge checkpoint")
    }

    /// Which specs the branch's changed paths belong to — the input to both front doors.
    pub fn specs_touched(touched: &[ChangedPath], s: &Snapshot) -> Vec<SpecName> {
        todo!("S5: match each ChangedPath against every spec's code globs via globset")
    }
}
