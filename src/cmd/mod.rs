//! `pub mod` lines ONLY.
//!
//! Rule 1 of the parallel build: **no agent adds a `mod` line.** `lib.rs` and this file
//! declare every module in wave 0, including every v0.2 module, so nine agents never
//! contend on a module list.
//!
//! Every handler has the same shape — `fn(ctx: &Ctx, a: &XArgs) -> Result<XReport>` where
//! `XReport: Render`. Handlers **never** print and never call `process::exit`; `lib.rs`
//! emits, and `out::emit` is the single emit point.
//!
//! Owner: **F** (foundation). FROZEN.

pub mod board;
pub mod comment;
pub mod decision;
pub mod doctor;
pub mod done;
pub mod features;
pub mod flow;
pub mod init;
pub mod landcheck;
pub mod prime;
pub mod proposal;
pub mod quirk;
pub mod repair;
pub mod rules;
pub mod scan;
pub mod setup;
pub mod spec;
pub mod status;
pub mod ticket;
pub mod up;
