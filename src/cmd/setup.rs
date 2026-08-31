//! `setup claude|cursor|codex [--remove]`, `instructions [topic]`, `completions <shell>`.
//!
//! Owner: **S7**.

// Wave-0 skeleton. The bodies below are `todo!("S7: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S7 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::{CompletionsArgs, InstructionsArgs, SetupArgs};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::instructions::Topic;
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
pub struct SetupReport {
    pub agent: &'static str,
    pub removed: bool,
    pub changes: Vec<crate::setup::SetupChange>,
    pub next: Vec<String>,
}

pub fn setup(ctx: &Ctx, a: &SetupArgs) -> Result<SetupReport> {
    todo!("S7: crate::setup::install or ::remove for a.agent, then report every changed file")
}

impl Render for SetupReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S7: one line per changed file, then the `→ kanspec prime` next step")
    }
}

#[derive(Debug, Serialize)]
pub struct InstructionsReport {
    /// present when a topic was named
    pub topic: Option<String>,
    pub text: Option<String>,
    /// the topic list, printed when no topic was named
    pub topics: Vec<Topic>,
}

pub fn instructions(ctx: &Ctx, a: &InstructionsArgs) -> Result<InstructionsReport> {
    todo!("S7: crate::instructions::render for a named topic, else ::topics for the list")
}

impl Render for InstructionsReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S7: print the topic body verbatim, or the topic list with one-line titles")
    }
}

#[derive(Debug, Serialize)]
pub struct CompletionsReport {
    pub shell: String,
    pub script: String,
}

pub fn completions(ctx: &Ctx, a: &CompletionsArgs) -> Result<CompletionsReport> {
    todo!("S7: clap_complete::generate over Cli::command() named ctx.invoked_as")
}

impl Render for CompletionsReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S7: write `script` verbatim — it is piped into the shell, so nothing else may be printed")
    }
}
