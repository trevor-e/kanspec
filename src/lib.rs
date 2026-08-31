//! kanspec — a kanban board and spec-review surface over plain git-tracked files.
//!
//! **The three things that carry the whole design:**
//!
//! 1. [`ctx::Ctx`] is built once in [`run`] *before* dispatch, so worktree unification and
//!    actor/clock injection touch every command without any command knowing they exist.
//! 2. `store::Store::transact` is the only code that writes a byte under `.kanspec/`. It
//!    takes the flock, reloads a fresh `Snapshot` *inside* the lock, runs a **pure
//!    planner** `fn(&Snapshot, &Facts, &Args, &Minter) -> Result<Plan>`, validates,
//!    applies, and replays each touched ticket's log before releasing.
//! 3. [`out::Render`]`: Serialize` makes `--json` the `Serialize` impl, so the two
//!    surfaces cannot drift.
//!
//! Compile-time guarantees are spent in exactly four places where they buy a real safety
//! property — `scan::MergedProof`, `keys::TicketKey`, `ctx::HumanActor`,
//! `paths::KanspecDir` — and nowhere else.

// The eight error shapes are wide by design: `KsError` carries a non-empty `Fixes` plus,
// for the two errors whose output quality IS the product, a structured `GateDetail`.
// Boxing it would trade that away for a stack size nobody measured, in a process that
// runs one command and exits.
#![allow(clippy::result_large_err)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches};

pub mod board;
pub mod cache;
pub mod ci;
pub mod cli;
pub mod cmd;
pub mod config;
pub mod ctx;
pub mod derive;
pub mod doctor;
pub mod error;
pub mod fm;
pub mod gh;
pub mod git;
pub mod hooks;
pub mod ids;
pub mod instructions;
pub mod keys;
pub mod lock;
pub mod logentry;
pub mod model;
pub mod out;
pub mod paths;
pub mod plan;
pub mod project;
pub mod rulesdoc;
pub mod scan;
pub mod server;
pub mod setup;
pub mod store;
pub mod transitions;
pub mod triage;

use crate::cli::{Cli, Command};
use crate::ctx::{Ctx, OutMode};
use crate::error::{code, Result};
use crate::out::Render;

/// The single entry point. Both bins are three lines over this.
///
/// `main` uses `try_get_matches()` + `from_arg_matches_mut` and returns `ExitCode`; it
/// **never** calls `Cli::parse()` (which internally `process::exit(2)`s on any typo'd
/// flag) and never calls `process::exit`. Clap's native 2 is remapped to 64, because 2 is
/// reachable from exactly one module in this crate — `cmd::landcheck`, sealed by
/// `BlockToken`'s private field — so the Stop-hook contract stays auditable by grep.
pub fn run(invoked_as: &'static str) -> ExitCode {
    cli::set_invoked_as(invoked_as);

    let cmd = Cli::command().name(invoked_as).bin_name(invoked_as);
    let cli = match cmd
        .try_get_matches()
        .and_then(|mut m| Cli::from_arg_matches_mut(&mut m))
    {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return ExitCode::from(match e.kind() {
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion => {
                    code::OK
                }
                // Bare `kanspec` / bare `kanspec comment` land here. USAGE, not OK, so an
                // agent that runs an incomplete command does not think it succeeded (D-16).
                _ => code::USAGE,
            });
        }
    };

    let color = out::apply_color_policy(cli.color);
    let mode = if cli.json {
        OutMode::Json
    } else {
        OutMode::Human { color }
    };

    let ctx = match Ctx::open(&cli, &cwd()) {
        Ok(c) => c,
        Err(e) => {
            e.render(&mode);
            return ExitCode::from(e.exit_code());
        }
    };

    match dispatch(&ctx, &cli) {
        Ok(c) => ExitCode::from(c),
        Err(e) => {
            e.render(&ctx.out);
            ExitCode::from(e.exit_code())
        }
    }
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Returns `Result<u8>` rather than `Result<()>` so a **successful** run can still exit
/// non-zero (`doctor` -> 1, `landcheck` -> 2) without going through the error renderer.
///
/// Every arm is `emit(handler(ctx, args)?)`. Nothing else happens here: handlers never
/// print, and this match is the only place a `cmd::*` function is named, which is what
/// lets `tests/json_matrix.rs` walk the clap tree and assert every command supports
/// `--json`.
pub fn dispatch(ctx: &Ctx, cli: &Cli) -> Result<u8> {
    use Command as C;
    match &cli.command {
        // ── SETUP ────────────────────────────────────────────────────────────
        C::Init(a) => ok(&cmd::init::init(ctx, a)?, ctx),
        C::Setup(a) => ok(&cmd::setup::setup(ctx, a)?, ctx),
        C::Doctor(a) => {
            // A clean prove is exit 0; any Error-severity finding is exit 1, so `doctor`
            // is a CI gate without a wrapper script.
            let r = cmd::doctor::doctor(ctx, a)?;
            out::emit(&r, &ctx.out)?;
            Ok(r.exit_code())
        }
        C::Instructions(a) => ok(&cmd::setup::instructions(ctx, a)?, ctx),
        C::Completions(a) => ok(&cmd::setup::completions(ctx, a)?, ctx),

        // ── TICKETS ──────────────────────────────────────────────────────────
        C::New(a) => ok(&cmd::ticket::new(ctx, a)?, ctx),
        C::Ready(a) => ok(&cmd::flow::ready(ctx, a)?, ctx),
        C::Start(a) => ok(&cmd::flow::start(ctx, a)?, ctx),
        C::Ship(a) => ok(&cmd::flow::ship(ctx, a)?, ctx),
        C::Done(a) => ok(&cmd::done::done(ctx, a)?, ctx),
        C::Park(a) => ok(&cmd::flow::park(ctx, a)?, ctx),
        C::Drop(a) => ok(&cmd::flow::drop_ticket(ctx, a)?, ctx),
        C::Show(a) => ok(&cmd::ticket::show(ctx, a)?, ctx),
        C::Log(a) => ok(&cmd::ticket::log(ctx, a)?, ctx),
        C::Where(a) => ok(&cmd::ticket::where_is(ctx, a)?, ctx),
        C::Ls(a) => ok(&cmd::ticket::ls(ctx, a)?, ctx),
        C::Repair(a) => ok(&cmd::repair::repair(ctx, a)?, ctx),

        // ── STATUS & GIT TRUTH ───────────────────────────────────────────────
        C::Status(a) => ok(&cmd::status::status(ctx, a)?, ctx),
        C::Scan(a) => ok(&cmd::scan::scan(ctx, a)?, ctx),
        C::Board(a) => ok(&cmd::board::board(ctx, a)?, ctx),
        C::Up(a) => ok(&cmd::up::up(ctx, a)?, ctx),
        C::Open(a) => ok(&cmd::up::open(ctx, a)?, ctx),
        C::Ci(a) => ok(&ci::run(ctx, a)?, ctx),

        // ── PROVENANCE & DECISIONS ───────────────────────────────────────────
        C::Rules(a) => ok(&cmd::rules::rules(ctx, a)?, ctx),
        C::Why(a) => ok(&cmd::decision::why(ctx, a)?, ctx),
        C::Decide(a) => ok(&cmd::decision::decide(ctx, a)?, ctx),
        C::Accept(a) => ok(&cmd::decision::accept(ctx, a)?, ctx),
        C::Supersede(a) => ok(&cmd::decision::supersede(ctx, a)?, ctx),
        C::Revoke(a) => ok(&cmd::decision::revoke(ctx, a)?, ctx),

        // ── KNOWLEDGE ────────────────────────────────────────────────────────
        C::Features(a) => ok(&cmd::features::features(ctx, a)?, ctx),
        C::Spec(a) => ok(&cmd::spec::spec(ctx, a)?, ctx),
        C::Quirk(a) => ok(&cmd::quirk::quirk(ctx, a)?, ctx),
        C::Quirks(a) => ok(&cmd::quirk::quirks(ctx, a)?, ctx),
        C::Prime(a) => ok(&cmd::prime::prime(ctx, a)?, ctx),

        // ── PROPOSALS & REVIEW (v0.2) ────────────────────────────────────────
        C::Propose(a) => ok(&cmd::proposal::propose(ctx, a)?, ctx),
        C::Review(a) => ok(&cmd::proposal::review(ctx, a)?, ctx),
        C::Comments(a) => ok(&cmd::comment::comments(ctx, a)?, ctx),
        C::Comment(a) => ok(&cmd::comment::comment(ctx, a)?, ctx),
        C::Approve(a) => ok(&cmd::proposal::approve(ctx, a)?, ctx),
        C::Close(a) => ok(&cmd::proposal::close(ctx, a)?, ctx),
        C::Abandon(a) => ok(&cmd::proposal::abandon(ctx, a)?, ctx),
        C::Promote(a) => ok(&cmd::comment::promote(ctx, a)?, ctx),
        C::Expire(a) => ok(&cmd::comment::expire(ctx, a)?, ctx),
        C::Landcheck(a) => {
            // The ONLY route to exit 2 in the whole crate, sealed by `BlockToken`.
            let r = cmd::landcheck::landcheck(ctx, a)?;
            out::emit(&r, &ctx.out)?;
            Ok(r.exit_code())
        }
    }
}

fn ok<R: Render>(r: &R, ctx: &Ctx) -> Result<u8> {
    out::emit(r, &ctx.out)?;
    Ok(code::OK)
}
