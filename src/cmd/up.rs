//! `up [--port]` and `open` — the ONLY async entry point in the crate.
//!
//! Foreground, loopback-only, zero private state: kill it and nothing is lost, and the CLI
//! never needs it. The server is a view and a poller, never a write authority.
//!
//! NOTE (deviation from the wave-0 stub, reported): the listening banner goes to
//! **stderr**, before the runtime starts, not through `Render`. `Render` runs after the
//! handler returns, and this handler returns only once Ctrl-C has been pressed — a banner
//! rendered there would appear at shutdown, which is the one moment it is useless. stdout
//! stays exclusively `out::emit`'s, so `--json` and the human surface still cannot drift:
//! `kanspec up --json` prints exactly one object, and it prints it on exit.
//!
//! Owner: **S8**.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::cli::{OpenArgs, UpArgs};
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::ids::{DecisionId, ProposalId, TicketId};
use crate::out::{glyph, Color, Line, Render, Style};
use crate::{fix, fixes};

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
    ctx.require_initialized()?;
    let port = a.port.unwrap_or(ctx.cfg.port);
    let url = format!("http://127.0.0.1:{port}");

    // The server owns its `Ctx`: `Arc<Ctx>` must outlive this call frame's borrow, and
    // `server::request_ctx` is the same constructor every request uses.
    let anchor = Arc::new(crate::server::request_ctx(ctx)?);

    if a.open {
        let target = url.clone();
        // Poll rather than sleep-and-hope: the browser opens the moment the listener is
        // up, and never at all if the bind failed.
        std::thread::spawn(move || {
            for _ in 0..30 {
                if listening_at(port) {
                    launch_browser(&target);
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(KsError::internal)?;
    let served = rt.block_on(crate::server::serve(anchor, port));
    // Ctrl-C must be instant. A `spawn_blocking` scan mid-flight cannot be cancelled, and
    // dropping the runtime would WAIT for it, so the wait is bounded here instead.
    rt.shutdown_timeout(Duration::from_millis(250));
    served?;

    Ok(UpReport {
        url,
        port,
        stopped: true,
    })
}

impl Render for UpReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::new(glyph::OK, format!("stopped — {} is free again", self.url))
            .dim(format!("port {}", self.port))
            .write(w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct OpenReport {
    pub url: String,
    pub launched: bool,
    pub next: Vec<String>,
}

/// `open [id]` — the board, or the page for one entity.
///
/// It refuses an id nothing owns rather than opening a URL that will 404, and it never
/// pretends to have launched anything: with no server listening it prints the URL and the
/// one command that makes it real.
pub fn open(ctx: &Ctx, a: &OpenArgs) -> Result<OpenReport> {
    ctx.require_initialized()?;
    let port = ctx.cfg.port;
    let url = format!("http://127.0.0.1:{port}{}", route_for(ctx, a.id.as_deref())?);

    let up_is_running = listening_at(port);
    let launched = up_is_running && launch_browser(&url);

    let mut next = Vec::new();
    if !up_is_running {
        next.push(format!("{} up", ctx.invoked_as));
    }
    Ok(OpenReport {
        url,
        launched,
        next,
    })
}

/// `/t/<id>`, `/p/<id>`, `/d/<id>`, or the board. The id is resolved against the store, so
/// a typo is a typed refusal here rather than an empty page in the browser.
fn route_for(ctx: &Ctx, id: Option<&str>) -> Result<String> {
    let Some(raw) = id else {
        return Ok("/".to_string());
    };
    let snap = ctx.snapshot()?;

    if raw.starts_with(ProposalId::PREFIX) {
        let p = ProposalId::parse(raw)?;
        snap.proposal(&p)?;
        return Ok(format!("/p/{p}"));
    }
    if raw.starts_with(DecisionId::PREFIX) {
        let d = DecisionId::parse(raw)?;
        snap.decision(&d)?;
        return Ok(format!("/d/{d}"));
    }
    // A bare `9c41` is a ticket: tickets are what a human types by hand.
    let t = TicketId::parse(raw).map_err(|_| {
        KsError::invalid(
            format!("`{raw}` is not a ticket, proposal or decision id"),
            fixes![fix!("kanspec ls"), fix!("kanspec open")],
        )
    })?;
    snap.ticket(&t)?;
    Ok(format!("/t/{t}"))
}

fn listening_at(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// `KANSPEC_NO_BROWSER` exists so a test suite (and a headless CI box) can exercise this
/// path without a window appearing.
fn launch_browser(url: &str) -> bool {
    if std::env::var_os("KANSPEC_NO_BROWSER").is_some() {
        return false;
    }
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

impl Render for OpenReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        if self.launched {
            return Line::new(glyph::OK, "opened in your browser")
                .dim(&self.url)
                .write(w, st);
        }
        // Nothing is listening: say the URL, and say the one command that makes it live.
        writeln!(w, " {}", crate::out::paint(&self.url, Color::Cyan, st.color))?;
        for n in &self.next {
            Line::new(glyph::FIX, "nothing is listening yet")
                .fix(n.as_str())
                .write(w, st)?;
        }
        Ok(())
    }
}
