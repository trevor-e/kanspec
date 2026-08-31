//! `quirk add|fix` and `quirks [--paths] [--touch]`.
//!
//! Capture friction near zero is why this registry accretes where ADR-era logs died; the
//! `--touch` form is the PostToolUse hook, which re-warns **at the moment an agent writes
//! a matching file** — the instant that actually prevents a stepped-on landmine.
//!
//! Owner: **S6**.

// Wave-0 skeleton. The bodies below are `todo!("S6: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S6 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::{QuirkArgs, QuirksArgs};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::ids::{QuirkId, TicketId};
use crate::model::{QuirkStatus, Severity};
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
#[serde(tag = "quirk", rename_all = "snake_case")]
pub enum QuirkReport {
    Added {
        id: QuirkId,
        title: String,
        paths: Vec<String>,
        severity: Severity,
        source: Option<TicketId>,
        next: Vec<String>,
    },
    Fixed {
        id: QuirkId,
        title: String,
        by: TicketId,
        next: Vec<String>,
    },
}

pub fn quirk(ctx: &Ctx, a: &QuirkArgs) -> Result<QuirkReport> {
    todo!("S6: add -> mint + Op::CreateEntity; fix -> Op::SetFields{{status: fixed, fixed_by}} — retired only by evidence")
}

impl Render for QuirkReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: `▸ q-11ba captured · landmine · src/billing/**`")
    }
}

#[derive(Debug, Serialize)]
pub struct QuirksReport {
    pub rows: Vec<QuirkRow>,
    /// set by the `--touch` hook form: the file that matched
    pub touched: Option<String>,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct QuirkRow {
    pub id: QuirkId,
    pub title: String,
    pub paths: Vec<String>,
    pub severity: Severity,
    pub status: QuirkStatus,
    pub source: Option<TicketId>,
    pub body: Option<String>,
}

pub fn quirks(ctx: &Ctx, a: &QuirksArgs) -> Result<QuirksReport> {
    todo!("S6: active quirks filtered by --paths; --touch matches ONE file and prints at most a one-liner")
}

impl Render for QuirksReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: severity-sorted lines; under --touch print nothing when nothing matched")
    }
}
