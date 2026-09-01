//! axum 0.8: `/api/*`, `/events` SSE, and an embedded-asset fallback.
//!
//! **The server is a view and a poller, never a write authority.** There is no server-side
//! write code at all: every POST handler is
//! `spawn_blocking(move || cmd::flow::ship(&ctx, &a))` over the very same `cmd::*` function
//! the CLI calls, which is exactly what `Arc<Ctx>: Send + Sync` buys. The 60s scan poll is
//! `cmd::scan::scan` for the same reason — going straight to `scan::scan_all` would have
//! been a second, quieter implementation that silently skipped D-20's projection rewrite.
//!
//! axum 0.8 routes use `{id}`, **not** `:id` — `:id` *panics* at `Router::route()`.
//!
//! **Why every request builds a fresh `Ctx`** (deviation from the wave-0 sketch, reported):
//! `Ctx::now` is stamped once by `Ctx::open`, and `load_snapshot` copies it into
//! `Snapshot::now`. A server that reused one `Ctx` for eight hours would therefore compute
//! every dwell, every STALLED window and every `checked Nm ago` against its own start time,
//! and — far worse — write that start time into the `## Log` of every ticket a POST moved,
//! which `replay`'s monotonicity check turns into a permanently unwritable ticket the
//! moment a CLI verb has already logged a later time. `AppState.ctx` stays the anchor
//! (`primary_root`, the port, the layout the watcher watches); the clock is re-read per
//! request, which costs two `git rev-parse`s on loopback.
//!
//! Owner: **S8**. The only async in the crate.

use std::future::IntoFuture;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use notify::{RecursiveMode, Watcher};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::cli::{
    Cli, ColorChoice, Command, DoneArgs, DropArgs, ParkArgs, ScanArgs, ShipArgs, ShowArgs,
    StartArgs, StatusArgs,
};
use crate::ctx::Ctx;
use crate::error::{code, KsError, Result};
use crate::{fix, fixes};

/// The board SPA: vanilla JS, no build step. `build.rs` prints
/// `cargo:rerun-if-changed=assets` because rust-embed tracks existing FILES but not the
/// DIRECTORY.
#[derive(RustEmbed)]
#[folder = "assets/"]
pub struct Assets;

#[derive(Clone)]
pub struct AppState {
    /// The anchor: the primary worktree, the layout the watcher watches, the style the
    /// banner uses. Every *request* builds its own `Ctx` from it — see the module header.
    pub ctx: Arc<Ctx>,
    /// This run's state generation. A POST publishes it only after `Store::transact` has
    /// returned, so the tick a browser reacts to can never be older than the write that
    /// caused it (D-22); the fs-watcher only invalidates, and the SPA refetches (D-23).
    pub rev: Arc<AtomicU64>,
    pub events: tokio::sync::broadcast::Sender<Tick>,
}

/// `{rev, n}` **only** — the SPA refetches `/api/board`. macOS FSEvents coalesces
/// create+remove+modify for one delete, so any event-kind-derived delta is a bug farm
/// (D-23).
///
/// `rev` is this server run's state generation, bumped once per settled batch; `n` is how
/// many raw events that batch coalesced, which is the only number worth carrying because
/// the page is going to refetch either way.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Tick {
    pub rev: u64,
    pub n: u64,
}

/// The error shape every handler maps into — the same envelope `KsError::to_json` writes,
/// so an agent hitting the API and an agent running the CLI read identical refusals.
#[derive(Debug)]
pub struct ApiError(pub crate::error::KsError);

impl From<KsError> for ApiError {
    fn from(e: KsError) -> ApiError {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // A gate refusal is a 409, not a 500: the request was well formed and the tool
        // said no. The body is byte-for-byte the CLI's `--json` error envelope.
        let status = match self.0.exit_code() {
            code::ENVIRONMENT => StatusCode::PRECONDITION_FAILED,
            code::INTERNAL => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::CONFLICT,
        };
        (status, Json(self.0.to_json())).into_response()
    }
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

/// Binds 127.0.0.1:<port>, foreground, Ctrl-C exits in under a second.
///
/// Shutdown is a `select!` against the serve future rather than
/// `with_graceful_shutdown`: an SSE response never completes on its own, so waiting for
/// open connections to drain would wait for the browser tab to be closed. Dropping the
/// serve future closes the listener and every live connection at once, which is what
/// "Ctrl-C is instant even with two SSE tabs open" actually requires.
pub async fn serve(ctx: Arc<Ctx>, port: u16) -> Result<()> {
    let (events, _rx) = tokio::sync::broadcast::channel::<Tick>(64);
    let state = AppState {
        ctx: ctx.clone(),
        rev: Arc::new(AtomicU64::new(0)),
        events,
    };

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| bind_error(port, &e))?;
    // The banner goes out only once the socket is actually ours, and on stderr, so stdout
    // stays exclusively `out::emit`'s and a refused bind never prints "serving …" first.
    banner(&ctx, port);

    let watcher = tokio::spawn(watch_loop(state.clone()));
    let scanner = tokio::spawn(scan_loop(state.clone()));

    let served = axum::serve(listener, router(state)).into_future();
    tokio::select! {
        r = served => r.map_err(|e| KsError::internal(anyhow::Error::new(e)))?,
        _ = shutdown_signal() => {}
    }
    watcher.abort();
    scanner.abort();
    Ok(())
}

/// The listening banner. See `cmd::up`'s header for why this is not `Render`: this handler
/// returns only once Ctrl-C has been pressed, and a banner printed then is useless.
fn banner(ctx: &Ctx, port: u16) {
    let st = ctx.style();
    let mut err = std::io::stderr();
    let _ = crate::out::Line::new(
        crate::out::glyph::OK,
        format!("serving the board on http://127.0.0.1:{port}"),
    )
    .dim("foreground · loopback only · Ctrl-C to stop")
    .write(&mut err, &st);
    let _ = crate::out::Line::new(
        '·',
        "the CLI never needs this — every verb works with it down",
    )
    .write(&mut err, &st);
}

/// A port already in use is a refusal that names its fix, not a panic and not a backtrace.
fn bind_error(port: u16, e: &std::io::Error) -> KsError {
    if e.kind() == std::io::ErrorKind::AddrInUse {
        return KsError::gate(
            "port_in_use",
            format!("127.0.0.1:{port} is already in use — kanspec up may already be running"),
            fixes![
                fix!("kanspec open"),
                fix!("kanspec up --port {}", port.saturating_add(1)),
            ],
        );
    }
    KsError::gate(
        "bind_failed",
        format!("cannot bind 127.0.0.1:{port}: {e}"),
        fixes![fix!("kanspec up --port {}", port.saturating_add(1))],
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

pub fn router(state: AppState) -> axum::Router {
    // axum 0.8: `{id}`. `:id` panics right here, at `Router::route()`.
    Router::new()
        .route("/api/board", get(api_board))
        .route("/api/status", get(api_status))
        .route("/api/rules", get(api_rules))
        .route("/api/ticket/{id}", get(api_ticket))
        .route("/api/ticket/{id}/start", post(post_start))
        .route("/api/ticket/{id}/ship", post(post_ship))
        .route("/api/ticket/{id}/park", post(post_park))
        .route("/api/ticket/{id}/drop", post(post_drop))
        .route("/api/ticket/{id}/done", post(post_done))
        .route("/api/scan", post(post_scan))
        .route("/events", get(events))
        .fallback(get(static_asset))
        .with_state(state)
}

// ─────────────────────────────────────────────────────────────────────────────
// The bridge: a fresh `Ctx` on a blocking thread, then a plain `cmd::*` call
// ─────────────────────────────────────────────────────────────────────────────

/// A request-scoped `Ctx`, anchored to the same primary worktree the server was started
/// in. See the module header for why this is not `Arc<Ctx>` reused.
///
/// `json: true` is load-bearing, not cosmetic: `cmd::done` is interactive under
/// `OutMode::Human` with a terminal on stdin, and the server inherited whatever stdin the
/// shell gave `kanspec up`. Declaring JSON mode is what makes every POST take the
/// flags-only door through `Triage`, so a browser can never be waiting on a prompt printed
/// into a terminal nobody is watching.
pub(crate) fn request_ctx(anchor: &Ctx) -> Result<Ctx> {
    let cli = Cli {
        json: true,
        color: ColorChoice::Never,
        repo: Some(anchor.repo.primary_root().to_path_buf()),
        command: Command::Status(StatusArgs { owner: None }),
    };
    Ctx::open(&cli, anchor.repo.primary_root())
}

/// Every handler's body: hop to a blocking thread, build a `Ctx`, call the SAME `cmd::*`
/// function the CLI calls. There is deliberately no other shape available here.
async fn blocking<T, F>(state: &AppState, f: F) -> std::result::Result<T, ApiError>
where
    F: FnOnce(&Ctx) -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    let anchor = state.ctx.clone();
    let joined = tokio::task::spawn_blocking(move || f(&request_ctx(&anchor)?)).await;
    match joined {
        Ok(r) => r.map_err(ApiError),
        Err(e) => Err(ApiError(KsError::internal(anyhow::anyhow!(
            "the worker thread failed: {e}"
        )))),
    }
}

/// An empty body is the default body. The board POSTs `{}` for `start` and a reason for
/// `park`; refusing a zero-length body would make `curl -X POST` fail for no reason.
fn body_of<T: serde::de::DeserializeOwned + Default>(
    raw: &str,
) -> std::result::Result<T, ApiError> {
    if raw.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(raw).map_err(|e| {
        ApiError(KsError::invalid(
            format!("the request body is not the JSON this verb takes: {e}"),
            fixes![fix!("POST {{}} for a verb with no options")],
        ))
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Reads
// ─────────────────────────────────────────────────────────────────────────────

async fn api_board(State(st): State<AppState>) -> ApiResult<crate::board::BoardModel> {
    Ok(Json(
        blocking(&st, |ctx| {
            ctx.require_initialized()?;
            let snap = ctx.snapshot()?;
            crate::board::build(ctx, &snap)
        })
        .await?,
    ))
}

async fn api_status(State(st): State<AppState>) -> ApiResult<crate::cmd::status::StatusReport> {
    Ok(Json(
        blocking(&st, |ctx| {
            crate::cmd::status::status(ctx, &StatusArgs { owner: None })
        })
        .await?,
    ))
}

async fn api_rules(State(st): State<AppState>) -> ApiResult<crate::cmd::rules::RulesReport> {
    Ok(Json(
        blocking(&st, |ctx| {
            crate::cmd::rules::rules(
                ctx,
                &crate::cli::RulesArgs {
                    paths: Vec::new(),
                    audit: false,
                    adopt: false,
                },
            )
        })
        .await?,
    ))
}

async fn api_ticket(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<crate::cmd::ticket::ShowReport> {
    Ok(Json(
        blocking(&st, move |ctx| {
            crate::cmd::ticket::show(ctx, &ShowArgs { id })
        })
        .await?,
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Writes — the SAME `cmd::*` functions, so a POST and a CLI verb write the same bytes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
pub struct StartBody {
    /// Absent means "whatever the repo's `worktree =` setting says" — the same three
    /// states the CLI has, so a POST and the equivalent CLI verb still agree.
    #[serde(default)]
    pub worktree: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ShipBody {
    #[serde(default)]
    pub pr: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct WhyBody {
    #[serde(default)]
    pub why: String,
}

/// The `done` gate's flag form, exactly as `DoneArgs` spells it. A drag onto DONE with
/// nothing filled in bounces with the gate's own typed refusal, which is the point.
#[derive(Debug, Default, Deserialize)]
pub struct DoneBody {
    #[serde(default)]
    pub spawn: Vec<String>,
    #[serde(default)]
    pub drop_step: Vec<String>,
    #[serde(default)]
    pub actually_done: Vec<usize>,
    #[serde(default)]
    pub no_followups: bool,
    #[serde(default)]
    pub spec_unchanged: Option<String>,
    #[serde(default)]
    pub quirk: Vec<String>,
    #[serde(default)]
    pub quirk_paths: Vec<String>,
    #[serde(default)]
    pub no_quirks: bool,
    #[serde(default)]
    pub decision: Vec<String>,
    #[serde(default)]
    pub no_decisions: bool,
    #[serde(default)]
    pub no_code: bool,
    #[serde(default)]
    pub why: Option<String>,
}

async fn post_start(
    State(st): State<AppState>,
    Path(id): Path<String>,
    raw: String,
) -> ApiResult<crate::cmd::flow::StartReport> {
    let b: StartBody = body_of(&raw)?;
    let r = blocking(&st, move |ctx| {
        crate::cmd::flow::start(
            ctx,
            &StartArgs {
                id,
                worktree: b.worktree == Some(true),
                no_worktree: b.worktree == Some(false),
            },
        )
    })
    .await?;
    publish(&st, 1);
    Ok(Json(r))
}

async fn post_ship(
    State(st): State<AppState>,
    Path(id): Path<String>,
    raw: String,
) -> ApiResult<crate::cmd::flow::ShipReport> {
    let b: ShipBody = body_of(&raw)?;
    let r = blocking(&st, move |ctx| {
        crate::cmd::flow::ship(ctx, &ShipArgs { id, pr: b.pr })
    })
    .await?;
    publish(&st, 1);
    Ok(Json(r))
}

async fn post_park(
    State(st): State<AppState>,
    Path(id): Path<String>,
    raw: String,
) -> ApiResult<crate::cmd::flow::ParkReport> {
    let b: WhyBody = body_of(&raw)?;
    let r = blocking(&st, move |ctx| {
        crate::cmd::flow::park(ctx, &ParkArgs { id, why: b.why })
    })
    .await?;
    publish(&st, 1);
    Ok(Json(r))
}

async fn post_drop(
    State(st): State<AppState>,
    Path(id): Path<String>,
    raw: String,
) -> ApiResult<crate::cmd::flow::DropReport> {
    let b: WhyBody = body_of(&raw)?;
    let r = blocking(&st, move |ctx| {
        crate::cmd::flow::drop_ticket(ctx, &DropArgs { id, why: b.why })
    })
    .await?;
    publish(&st, 1);
    Ok(Json(r))
}

async fn post_done(
    State(st): State<AppState>,
    Path(id): Path<String>,
    raw: String,
) -> ApiResult<crate::cmd::done::DoneReport> {
    let b: DoneBody = body_of(&raw)?;
    let r = blocking(&st, move |ctx| {
        crate::cmd::done::done(
            ctx,
            &DoneArgs {
                id,
                spawn: b.spawn,
                drop_step: b.drop_step,
                actually_done: b.actually_done,
                no_followups: b.no_followups,
                spec_unchanged: b.spec_unchanged,
                quirk: b.quirk,
                quirk_paths: b.quirk_paths,
                no_quirks: b.no_quirks,
                decision: b.decision,
                no_decisions: b.no_decisions,
                no_code: b.no_code,
                why: b.why,
            },
        )
    })
    .await?;
    publish(&st, 1);
    Ok(Json(r))
}

async fn post_scan(State(st): State<AppState>) -> ApiResult<crate::cmd::scan::ScanReport> {
    let r = blocking(&st, |ctx| crate::cmd::scan::scan(ctx, &quiet_scan())).await?;
    publish(&st, 1);
    Ok(Json(r))
}

fn quiet_scan() -> ScanArgs {
    ScanArgs {
        id: None,
        explain: false,
        confirm: None,
        why: None,
        quiet: true,
        no_fetch: false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SSE + the two loops that feed it
// ─────────────────────────────────────────────────────────────────────────────

fn publish(state: &AppState, n: u64) {
    let rev = state.rev.fetch_add(1, Ordering::Relaxed) + 1;
    // `send` fails only when nobody is subscribed, which is the normal case with no tab
    // open. It is not an error and never a reason to stop the loop.
    let _ = state.events.send(Tick { rev, n });
}

async fn events(
    State(st): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, std::convert::Infallible>>> {
    let stream = BroadcastStream::new(st.events.subscribe()).filter_map(|t| {
        // A lagged receiver dropped ticks; the SPA refetches the whole board on the next
        // one, so there is nothing to reconstruct and nothing to report.
        let tick = t.ok()?;
        Event::default().event("tick").json_data(tick).ok().map(Ok)
    });
    // The keep-alive is what tells a proxy-free loopback browser the stream is alive; it
    // also makes a dead server visible to the page within 15s rather than never.
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

/// The 60s merge-detection poll, so the In-main column populates itself while you watch.
///
/// It runs `cmd::scan::scan` — the CLI's own handler — rather than `scan::scan_all`, so
/// the cache write goes through `Store::transact` and D-20's `KANSPEC-*.md` regeneration
/// happens here exactly as it does for `kanspec scan`.
///
/// The *fetch* is throttled by `[windows] fetch_max_age_secs` (default 5 min) even though
/// the ladder still runs every minute: this loop lives for a whole working day, and a
/// network round trip a minute is not what a five-minute freshness window means. The
/// ladder's local rungs are what move a ticket into IN MAIN anyway; the fetch only moves
/// `origin/main`. The explicit `POST /api/scan` behind the page's `scan` button always
/// fetches — a human asking for fresh facts asked for fresh facts.
async fn scan_loop(state: AppState) {
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    // `interval`'s first tick is immediate; the board was just built, so skip it.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let done = blocking(&state, |ctx| {
            ctx.require_initialized()?;
            let fresh = ctx
                .git
                .fetch_age()
                .is_some_and(|d| d.as_secs() < ctx.cfg.windows.fetch_max_age_secs);
            let mut args = quiet_scan();
            args.no_fetch = fresh;
            crate::cmd::scan::scan(ctx, &args).map(|_| ())
        })
        .await;
        // A scan that failed (no network, no `gh`, a held lock) is not a reason to stop
        // polling — it is exactly the condition the next pass is for.
        if done.is_ok() {
            publish(&state, 1);
        }
    }
}

/// `notify` on `.kanspec/`, so CLI and agent edits appear in the browser within a frame.
async fn watch_loop(state: AppState) {
    let Some(dir) = kanspec_dir(&state.ctx) else {
        return;
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(256);
    let mut watcher =
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if res.is_ok() {
                // `try_send`: a full channel means a batch is already settling, and blocking
                // the watcher thread to say so again would only make the next batch later.
                let _ = tx.try_send(());
            }
        }) {
            Ok(w) => w,
            Err(_) => return,
        };
    if watcher.watch(&dir, RecursiveMode::Recursive).is_err() {
        return;
    }

    loop {
        if rx.recv().await.is_none() {
            return;
        }
        // Debounce. One `transact` is a tmp write, a rename and an fsync per file, and
        // macOS FSEvents coalesces unpredictably — so settle first, then publish once.
        let mut n = 1u64;
        let deadline = tokio::time::sleep(Duration::from_millis(120));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                r = rx.recv() => match r {
                    Some(()) => n += 1,
                    None => return,
                },
            }
        }
        publish(&state, n);
    }
}

/// `.kanspec/` itself. `KanspecDir` is private by design (only `Layout` can name a file
/// under it), so the directory is reached through the one public path that is exactly one
/// level below it.
fn kanspec_dir(ctx: &Ctx) -> Option<PathBuf> {
    ctx.layout.tickets_dir().parent().map(PathBuf::from)
}

// ─────────────────────────────────────────────────────────────────────────────
// The embedded SPA — the 12 lines that replace tower-http
// ─────────────────────────────────────────────────────────────────────────────

async fn static_asset(uri: Uri) -> Response {
    match asset(uri.path()).await {
        Some((headers, body)) => (headers, body).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// The 12-line embedded-asset fallback that replaces tower-http entirely.
///
/// Anything that is not a file falls back to `index.html`, so the SPA's own routes
/// (`/t/<id>`, `/p/<id>`, `/d/<id>`) survive a reload — except under `/api/`, where a
/// typo must 404 rather than hand an agent a page of HTML to parse as JSON.
async fn asset(path: &str) -> Option<(HeaderMap, Vec<u8>)> {
    let rel = path.trim_start_matches('/');
    let file = Assets::get(rel).or_else(|| {
        if rel.starts_with("api/") || rel == "events" {
            None
        } else {
            Assets::get("index.html")
        }
    })?;
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(file.metadata.mimetype()).ok()?,
    );
    // The board is a live view of a working tree; a cached copy of it is a lie.
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Some((h, file.data.into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the file header. A `:id` route panics inside
    /// `Router::route()`, so building the router at all is the assertion.
    #[test]
    fn the_router_builds_which_is_the_axum_08_brace_syntax_assertion() {
        let (events, _rx) = tokio::sync::broadcast::channel::<Tick>(8);
        // Constructing `AppState` needs a real `Ctx`; the router shape does not, so this
        // asserts the one thing that can panic — the path syntax — without one.
        let paths = [
            "/api/board",
            "/api/status",
            "/api/rules",
            "/api/ticket/{id}",
            "/api/ticket/{id}/start",
            "/api/ticket/{id}/ship",
            "/api/ticket/{id}/park",
            "/api/ticket/{id}/drop",
            "/api/ticket/{id}/done",
            "/api/scan",
            "/events",
        ];
        let mut r: Router<()> = Router::new();
        for p in paths {
            assert!(!p.contains(':'), "axum 0.8 panics on `:id`: {p}");
            r = r.route(p, get(|| async { "ok" }));
        }
        drop(events);
    }

    #[tokio::test]
    async fn the_spa_is_embedded_and_unknown_routes_fall_back_to_it() {
        let (h, body) = asset("/index.html").await.expect("index.html is embedded");
        assert!(h[header::CONTENT_TYPE].to_str().unwrap().contains("html"));
        assert!(!body.is_empty());

        // A deep link the SPA owns must serve the shell, not a 404.
        let (_, deep) = asset("/t/t-9c41")
            .await
            .expect("SPA routes serve the shell");
        assert_eq!(deep, body);

        // …but a mistyped API path must NOT hand an agent HTML to parse as JSON.
        assert!(asset("/api/bogus").await.is_none());

        for f in ["/app.js", "/style.css"] {
            assert!(asset(f).await.is_some(), "{f} is not embedded");
        }
    }

    #[test]
    fn a_port_in_use_is_a_typed_refusal_that_names_its_fix() {
        let e = bind_error(
            5757,
            &std::io::Error::new(std::io::ErrorKind::AddrInUse, "in use"),
        );
        assert_eq!(e.code(), Some("port_in_use"));
        assert!(e.to_string().contains("5757"));
        let fixes: Vec<String> = e.fixes().iter().map(|f| f.as_str().to_string()).collect();
        assert!(fixes.iter().any(|f| f.contains("--port 5758")), "{fixes:?}");
    }

    #[test]
    fn an_empty_post_body_is_the_default_body() {
        // Absent is None, not false: the board must be able to say "repo default"
        // as well as "yes" and "no", the same three states the CLI has.
        let b: StartBody = body_of("").unwrap();
        assert_eq!(b.worktree, None);
        let b: StartBody = body_of("{\"worktree\":true}").unwrap();
        assert_eq!(b.worktree, Some(true));
        let b: StartBody = body_of("{\"worktree\":false}").unwrap();
        assert_eq!(b.worktree, Some(false));
        let b: WhyBody = body_of("{\"why\":\"blocked on review\"}").unwrap();
        assert_eq!(b.why, "blocked on review");
        assert!(body_of::<StartBody>("not json").is_err());
    }
}
