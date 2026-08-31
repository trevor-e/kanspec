//! `up [--port]` and `open` — the ONLY async entry point in the crate.
//!
//! Foreground, loopback-only, zero private state: kill it and nothing is lost, and the CLI
//! never needs it. The server is a view and a poller, never a write authority.
//!
//! Owner: **S8**.

// Wave-0 skeleton. The bodies below are `todo!("S8: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S8 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::{OpenArgs, UpArgs};
use crate::ctx::Ctx;
use crate::error::Result;
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
pub struct UpReport {
    pub url: String,
    pub port: u16,
    /// how the run ended — this handler only returns once Ctrl-C has been handled
    pub stopped: bool,
}

/// The one place a tokio runtime is built. `dispatch` stays synchronous; async is confined
/// to this call and everything below `server::serve`.
pub fn up(ctx: &Ctx, a: &UpArgs) -> Result<UpReport> {
    todo!("S8: build a multi-thread runtime, Arc the Ctx, block_on(server::serve); Ctrl-C must return in under a second")
}

impl Render for UpReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S8: the listening banner before serving, and the stopped line after")
    }
}

#[derive(Debug, Serialize)]
pub struct OpenReport {
    pub url: String,
    pub launched: bool,
    pub next: Vec<String>,
}

pub fn open(ctx: &Ctx, a: &OpenArgs) -> Result<OpenReport> {
    todo!("S8: resolve the id to /t/<id>, /p/<id> or /d/<id>; launch the browser, or just print the URL when `up` is not listening")
}

impl Render for OpenReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S8: print the URL — and the `→ kanspec up` fix when nothing is listening")
    }
}
