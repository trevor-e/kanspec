//! axum 0.8: `/api/*`, `/events` SSE, and an embedded-asset fallback.
//!
//! **The server is a view and a poller, never a write authority.** There is no server-side
//! write code at all: every POST handler is
//! `spawn_blocking(move || cmd::flow::ship(&ctx, &a))` over the very same `cmd::*` function
//! the CLI calls, which is exactly what `Arc<Ctx>: Send + Sync` buys.
//!
//! axum 0.8 routes use `{id}`, **not** `:id` — `:id` *panics* at `Router::route()`.
//!
//! Owner: **S8**. The only async in the crate.

// Wave-0 skeleton. The bodies below are `todo!("S8: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S8 when the bodies land.
#![allow(unused_variables, dead_code)]

use std::sync::Arc;

use rust_embed::RustEmbed;
use serde::Serialize;

use crate::ctx::Ctx;
use crate::error::Result;

/// The board SPA: vanilla JS, no build step. `build.rs` prints
/// `cargo:rerun-if-changed=assets` because rust-embed tracks existing FILES but not the
/// DIRECTORY.
#[derive(RustEmbed)]
#[folder = "assets/"]
pub struct Assets;

#[derive(Clone)]
pub struct AppState {
    pub ctx: Arc<Ctx>,
    /// `transact` publishes the post-write snapshot SYNCHRONOUSLY, so a POST's own refetch
    /// can never see the pre-write state (D-22); the fs-watcher only invalidates.
    pub rev: Arc<std::sync::atomic::AtomicU64>,
    pub events: tokio::sync::broadcast::Sender<Tick>,
}

/// `{rev, n}` **only** — the SPA refetches `/api/board`. macOS FSEvents coalesces
/// create+remove+modify for one delete, so any event-kind-derived delta is a bug farm
/// (D-23).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Tick {
    pub rev: u64,
    pub n: u64,
}

/// The error shape every handler maps into — the same envelope `KsError::to_json` writes,
/// so an agent hitting the API and an agent running the CLI read identical refusals.
pub struct ApiError(pub crate::error::KsError);

/// Binds 127.0.0.1:<port>, foreground, Ctrl-C exits in under a second.
pub async fn serve(ctx: Arc<Ctx>, port: u16) -> Result<()> {
    todo!("S8: build the router, bind loopback, spawn the notify watcher and the 60s scan poll, await ctrl_c")
}

pub fn router(state: AppState) -> axum::Router {
    todo!("S8: /api/board, /api/ticket/{{id}}, /api/rules, /api/status, POST verbs, /events, embedded-asset fallback — braces, never `:id`")
}

/// The 60s merge-detection poll, so the In-main column populates itself while you watch.
async fn scan_loop(state: AppState) {
    todo!("S8: every 60s, spawn_blocking(scan::scan_all) then publish a Tick")
}

/// `notify` on `.kanspec/`, so CLI and agent edits appear in the browser within a frame.
async fn watch_loop(state: AppState) {
    todo!("S8: notify watcher, debounced, publishing a Tick per settled batch")
}

/// The 12-line embedded-asset fallback that replaces tower-http entirely.
async fn asset(path: &str) -> Option<(axum::http::HeaderMap, Vec<u8>)> {
    todo!("S8: Assets::get(path) or index.html, with the mime from rust-embed's metadata")
}
