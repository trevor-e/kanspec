//! `init [--refresh-hooks]` — scaffold `.kanspec/`, `.gitattributes` (`merge=union` for
//! `comments.jsonl`), the git hooks, and the `cache/` gitignore entry.
//!
//! Owner: **S7**. This file and `lock.rs` are the only entries on
//! `tests/single_write_path.rs`'s allowlist: `init` scaffolds `.kanspec/` before a store
//! can exist, so it necessarily writes without a `LockToken`.

// Wave-0 skeleton. The bodies below are `todo!("S7: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S7 when the bodies land.
#![allow(unused_variables, dead_code)]

use std::path::PathBuf;

use serde::Serialize;

use crate::cli::InitArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::hooks::HookReport;
use crate::out::{Render, Style};

/// The `merge=union` line, a **built-in** git strategy needing zero per-clone setup.
pub const GITATTRIBUTES_LINE: &str = ".kanspec/proposals/**/comments.jsonl merge=union";

#[derive(Debug, Serialize)]
pub struct InitReport {
    pub root: PathBuf,
    pub created: Vec<PathBuf>,
    pub already_present: Vec<PathBuf>,
    pub hooks: Vec<HookReport>,
    /// `init` REFUSES to claim a projection path that already exists un-generated
    pub refused_paths: Vec<PathBuf>,
    pub next: Vec<String>,
}

pub fn init(ctx: &Ctx, a: &InitArgs) -> Result<InitReport> {
    todo!("S7: scaffold .kanspec/{{tickets,specs,decisions,quirks,proposals/closed,cache}}, config.toml from Config::render_default, cache/ into .gitignore, the merge=union .gitattributes line, then hooks::install; --refresh-hooks reinstalls hooks over an existing store")
}

impl Render for InitReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S7: one line per created path, then the hook table, then `→ kanspec setup claude`")
    }
}
