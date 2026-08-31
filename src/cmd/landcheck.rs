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

// Wave-0 skeleton. The bodies below are unwritten; these two allows exist ONLY so the
// skeleton compiles clippy-clean and MUST be deleted by V2 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::LandcheckArgs;
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::out::{Render, Style};
use crate::{fix, fixes};

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

/// V2 lands here: block when a committed diff exists but the claimed ticket had no update
/// this session, when unresolved comments target a proposal this session edited, or when
/// `done` skipped the knowledge check; `--dry-run` reports without minting a `BlockToken`;
/// honour `[hooks] landcheck`.
///
/// **Until then it REFUSES rather than panics** (round-D hardening). A `todo!()` here
/// exited 101 with a backtrace and no `--json` envelope — outside §2.1's documented code
/// set entirely, and read by a wrapper script as a crashed tool rather than a tool that
/// declined. It must never accidentally exit 2 either: 2 is the sealed Stop-hook block,
/// and an unimplemented check that blocks every agent session from ending is strictly
/// worse than one that says so. `KsError::Gate` exits 1.
pub fn landcheck(ctx: &Ctx, a: &LandcheckArgs) -> Result<LandcheckReport> {
    Err(KsError::gate(
        "landcheck_v02",
        "`landcheck` lands in v0.2 — kanspec will not report a session clean by declining \
         to look at it",
        fixes![
            fix!("set [hooks] landcheck = false in .kanspec/config.toml"),
            fix!("{} status", ctx.invoked_as),
            fix!("{} doctor", ctx.invoked_as),
        ],
    ))
}

impl Render for LandcheckReport {
    /// V2 prints nothing when clean; else one line per reason, each with its verb.
    ///
    /// Unreachable while `landcheck` above refuses — the handler is this type's only
    /// constructor — but written as a real body rather than a `todo!()` so that reaching it
    /// is a wrong answer instead of a panic.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        for r in &self.reasons {
            crate::out::Line::new(crate::out::glyph::FIX, r.as_str()).write(w, st)?;
        }
        Ok(())
    }
}
