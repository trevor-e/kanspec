//! `board [--export board.md]` — the terminal board, and the markdown snapshot for a PR.
//!
//! Renders exactly the `board::BoardModel` the browser gets from `/api/board`, so the
//! cold CLI and the live page cannot disagree about a column's contents.
//!
//! Owner: **S8**.

// Wave-0 skeleton. The bodies below are `todo!("S8: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S8 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::board::BoardModel;
use crate::cli::BoardArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::out::{Render, Style};

#[derive(Debug, Serialize)]
pub struct BoardReport {
    pub model: BoardModel,
    /// where `--export` wrote the snapshot
    pub exported: Option<String>,
    /// how stale the cached git facts are, printed beside the board so a badge never lies
    pub cache_age_secs: Option<u64>,
    pub next: Vec<String>,
}

pub fn board(ctx: &Ctx, a: &BoardArgs) -> Result<BoardReport> {
    todo!("S8: board::build; with --export, board::render_markdown through Store::transact so the write path stays single")
}

impl Render for BoardReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S8: board::render_terminal, then the cache age line")
    }
}
