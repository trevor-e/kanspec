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

/// The Stop hook: exit 2 while the tracker disagrees with the working tree.
///
/// It blocks on facts the session can still act on, and each reason names the verb that
/// clears it — a block with no stated exit is a loop, which is the failure mode a Stop hook
/// has to avoid above all others.
///
/// Two blocking conditions, both COMPUTED rather than asserted:
/// 1. **git says the work landed and nobody closed the ticket** — `in_main`, the exact
///    "did it merge?" ambiguity kanspec exists to kill.
/// 2. **unresolved review threads on a proposal a live ticket implements** — feedback must
///    be un-ignorable work, not scrollback.
///
/// Deliberately NOT here: dwell and staleness tripwires. Those are `status` lines a human
/// judges, and a Stop hook that blocks on "this has been open a while" blocks on something
/// the session cannot clear, which is how a hook earns being switched off.
///
/// `--dry-run` reports the same reasons and mints no [`BlockToken`], so a human can ask
/// "what would block me?" without the answer being an exit code.
///
/// Config-gated (D-14): with `[hooks] landcheck = false` this is a clean exit 0 and says
/// nothing, because a hook the repo opted out of must not editorialise.
pub fn landcheck(ctx: &Ctx, a: &LandcheckArgs) -> Result<LandcheckReport> {
    ctx.require_initialized()?;
    if !ctx.cfg.hooks.landcheck && !a.dry_run {
        return Ok(LandcheckReport {
            blocked: false,
            reasons: Vec::new(),
            next: Vec::new(),
            status: Status::Ok,
        });
    }
    let s = ctx.snapshot()?;
    let mut reasons = Vec::new();

    // 1 — git says it landed; the ticket says otherwise.
    for t in s.tickets.values() {
        if crate::derive::in_main(&s, t).is_some() {
            reasons.push(format!(
                "{} is in main and still open → {} done {}",
                t.fm.id, ctx.invoked_as, t.fm.id
            ));
        }
    }

    // 2 — review feedback nobody answered, on a proposal this session is implementing.
    for t in s.tickets.values() {
        if t.fm.state.terminal() {
            continue;
        }
        let Some(pid) = &t.fm.proposal else { continue };
        let Some(p) = s.proposals.get(pid) else {
            continue;
        };
        let open = crate::cmd::proposal::unresolved(&s, p);
        if open > 0 {
            reasons.push(format!(
                "{pid} has {open} unresolved review thread{} → {} comments {pid} --unresolved",
                if open == 1 { "" } else { "s" },
                ctx.invoked_as
            ));
        }
    }
    reasons.sort();
    reasons.dedup();

    let blocked = !reasons.is_empty();
    let next = if blocked {
        vec![format!("{} status", ctx.invoked_as)]
    } else {
        Vec::new()
    };
    Ok(LandcheckReport {
        blocked,
        reasons,
        next,
        // `--dry-run` answers the question without BEING the answer: no `BlockToken` is
        // minted, so the seal still means "this exit 2 came from a real Stop-hook block".
        status: match (blocked, a.dry_run) {
            (true, false) => Status::Block(BlockToken(())),
            (true, true) => Status::Violation,
            (false, _) => Status::Ok,
        },
    })
}

impl Render for LandcheckReport {
    /// Prints nothing when clean; else one line per reason, each with its verb.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        for r in &self.reasons {
            crate::out::Line::new(crate::out::glyph::FIX, r.as_str()).write(w, st)?;
        }
        Ok(())
    }
}
