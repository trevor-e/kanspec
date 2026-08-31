//! The Stop hook — **the ONLY minter of `BlockToken`, and therefore the only source of
//! exit code 2 in the entire crate.**
//!
//! Exit 2 blocks an agent session from ending while the tracker disagrees with the working
//! tree, and exit 0 once clean — no loops. The seal is colocated here because `pub(in …)`
//! requires an ancestor module and this layout is flat (E0742): a plain private field in
//! the file that mints the value does the same job and compiles.
//!
//! Opt-in via `[hooks] landcheck` (D-14): installed by `setup`, config-gated, default off.
//!
//! Owner: **V2**, v0.2.

// Wave-0 skeleton. The bodies below are `todo!("V2: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by V2 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::LandcheckArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::out::{Render, Style};

/// Private field: only this file can mint one, so `grep -r 'BlockToken'` is a complete
/// audit of every route to exit 2.
#[derive(Debug)]
pub struct BlockToken(());

#[derive(Debug)]
pub enum Status {
    Ok,
    Violation,
    Block(BlockToken),
}

impl Status {
    pub fn code(self) -> u8 {
        match self {
            Status::Ok => 0,
            Status::Violation => 1,
            Status::Block(_) => 2,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LandcheckReport {
    pub blocked: bool,
    /// each one names the exact verb to run
    pub reasons: Vec<String>,
    pub next: Vec<String>,
    #[serde(skip)]
    status: Status,
}

impl LandcheckReport {
    /// Borrowing, unlike `Status::code`, because `dispatch` needs the code without
    /// consuming the report it still has to emit.
    pub fn exit_code(&self) -> u8 {
        match self.status {
            Status::Ok => crate::error::code::OK,
            Status::Violation => crate::error::code::VIOLATION,
            Status::Block(_) => crate::error::code::BLOCK,
        }
    }
}

pub fn landcheck(ctx: &Ctx, a: &LandcheckArgs) -> Result<LandcheckReport> {
    todo!("V2: block when a committed diff exists but the claimed ticket had no update this session, when unresolved comments target a proposal this session edited, or when `done` skipped the knowledge check; --dry-run reports without minting a BlockToken; honour [hooks] landcheck")
}

impl Render for LandcheckReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("V2: print nothing when clean; else one line per reason, each with its verb")
    }
}
