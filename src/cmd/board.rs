//! `board [--export board.md]` — the terminal board, and the markdown snapshot for a PR.
//!
//! Renders exactly the `board::BoardModel` the browser gets from `/api/board`, so the
//! cold CLI and the live page cannot disagree about a column's contents.
//!
//! This is the fallback that has to work with the server down, which is why it prints the
//! cache's own age beside the board: every badge on it was computed from `cache/
//! gitstate.json`, and a merge state nobody refreshed today must say so out loud rather
//! than look current.
//!
//! `--export` writes through `Store::transact` like every other byte kanspec puts on disk
//! — `board.rs` and this file contain no `fs::write`, and `tests/single_write_path.rs`
//! greps for exactly that.
//!
//! Owner: **S8**.

use serde::Serialize;

use crate::board::{self, BoardModel};
use crate::cli::BoardArgs;
use crate::ctx::Ctx;
use crate::derive;
use crate::error::Result;
use crate::out::{glyph, Color, Line, Render, Style};
use crate::plan::{Op, Plan};
use crate::store::Store;

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
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let model = board::build(ctx, &snap)?;
    let cache_age_secs = model.cache_age_secs;

    let exported = match &a.export {
        None => None,
        Some(rel) => {
            // Relative to where the human stands, not to the primary root: `board
            // --export board.md` from a linked worktree writes it where they can see it.
            let path = if rel.is_absolute() {
                rel.clone()
            } else {
                ctx.repo.here().join(rel)
            };
            let contents = board::render_markdown(&model);
            let target = path.clone();
            // Not a ticket verb, so `None` — the transaction commits as `kanspec: update`
            // under `sync = "commit"` rather than borrowing a state verb (D-31).
            Store::open(ctx).transact(None, &ctx.invocation(), move |_s, _m| {
                Ok(Plan::of(vec![Op::WriteGenerated {
                    path: target,
                    contents,
                }]))
            })?;
            Some(path.display().to_string())
        }
    };

    // The board's own next steps are the attention strip's, deduped — the same list
    // `status` prints, so the two surfaces cannot offer different next verbs.
    let mut next: Vec<String> = Vec::new();
    for at in &model.attention {
        if !at.fix.is_empty() && !next.contains(&at.fix) {
            next.push(at.fix.clone());
        }
        if next.len() == 5 {
            break;
        }
    }
    if snap.git.is_empty() {
        next.push(format!("{} scan", ctx.invoked_as));
    }

    Ok(BoardReport {
        model,
        exported,
        cache_age_secs,
        next,
    })
}

impl Render for BoardReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // `--export` writes a snapshot INSTEAD of printing the board (clap's own wording).
        if let Some(path) = &self.exported {
            let cards: usize = self.model.columns.iter().map(|c| c.cards.len()).sum();
            Line::new(glyph::OK, format!("wrote {cards} cards to {path}"))
                .dim(board::cache_age_line(&self.model))
                .write(w, st)?;
            return Ok(());
        }

        write!(w, "{}", board::render_terminal(&self.model, st))?;

        // The freshness stamp: every badge above came out of the disposable cache.
        match self.cache_age_secs {
            Some(age) => writeln!(
                w,
                "   {}",
                crate::out::paint(
                    &format!(
                        "merge state checked {} ago",
                        derive::short(std::time::Duration::from_secs(age))
                    ),
                    Color::Dim,
                    st.color
                )
            ),
            None => Line::new('·', "merge state never scanned")
                .fix("kanspec scan")
                .write(w, st),
        }
    }
}
