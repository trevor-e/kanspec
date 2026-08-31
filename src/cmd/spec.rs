//! `spec new|show|grep` — the living per-capability specs.
//!
//! Specs are edited **on the implementation branch** and reviewed in the PR like any code:
//! no archive-time merge, no deferred delta debt, no bot commits to trunk.
//!
//! Owner: **S6**.

// Wave-0 skeleton. The bodies below are `todo!("S6: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S6 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::SpecArgs;
use crate::ctx::Ctx;
use crate::derive::Staleness;
use crate::error::Result;
use crate::ids::SpecName;
use crate::model::Rule;
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
#[serde(tag = "spec", rename_all = "snake_case")]
pub enum SpecReport {
    Created {
        name: SpecName,
        path: String,
        next: Vec<String>,
    },
    Shown {
        name: SpecName,
        feature: String,
        code: Vec<String>,
        rules: Vec<Rule>,
        staleness: Staleness,
        next: Vec<String>,
    },
    Grepped {
        pattern: String,
        hits: Vec<GrepHit>,
    },
}

#[derive(Debug, Serialize)]
pub struct GrepHit {
    pub spec: SpecName,
    pub anchor: String,
    pub text: String,
    pub line: usize,
}

pub fn spec(ctx: &Ctx, a: &SpecArgs) -> Result<SpecReport> {
    todo!("S6: new -> transact(Op::CreateEntity) with the frontmatter scaffold; show/grep are read-only")
}

impl Render for SpecReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: the rule bullets with their {{p-xxxx}} provenance tokens, plus the staleness verdict")
    }
}
