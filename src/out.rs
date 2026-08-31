//! One rendering layer. `--json` **is** the `Serialize` impl, so the two surfaces cannot
//! drift and there is zero hand-written JSON in the crate.
//!
//! [`Render`] is monomorphic on purpose: no `dyn`, no `serde_json::Value` per render, no
//! coherence-trapping blanket impl, and — the decisive property for a parallel build —
//! each command's payload struct and its `impl Render` live in THAT COMMAND'S OWN FILE,
//! so `out.rs` never grows a variant or a match arm.
//!
//! Handlers NEVER print. [`emit`] is the single emit point.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};

use crate::cli::ColorChoice;
use crate::ctx::OutMode;
use crate::error::{KsError, Result};

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub color: bool,
    pub width: usize,
    pub quiet: bool,
}

impl Style {
    pub fn plain() -> Style {
        Style {
            color: false,
            width: 100,
            quiet: false,
        }
    }
}

/// The second and LAST trait in the crate.
pub trait Render: serde::Serialize {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()>;
}

/// The single emit point. A broken pipe (`kanspec ls | head`) is success, not an error.
pub fn emit<R: Render>(r: &R, mode: &OutMode) -> Result<()> {
    let stdout = std::io::stdout();
    let mut w = stdout.lock();
    let res = match mode {
        OutMode::Json => {
            let s = serde_json::to_string_pretty(r).map_err(KsError::internal)?;
            writeln!(w, "{s}")
        }
        OutMode::Human { color } => {
            let st = Style {
                color: *color,
                width: term_width(),
                quiet: false,
            };
            r.human(&mut w, &st).and_then(|()| w.flush())
        }
    };
    match res {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(KsError::internal(e)),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The one-glyph state badges (DESIGN.md: `○ todo ◐ doing ◈ review ⇂ in-main
// ● done ✕ dropped`)
// ─────────────────────────────────────────────────────────────────────────────

/// The glyphs that are NOT a stored `State`. `IN_MAIN` and `DISCOVERED` are *derived*
/// overlays — computed from git and from `discovered_in`, never written to a file — which
/// is exactly why they live here rather than in `transitions::State`.
pub mod glyph {
    /// git says the work is on main; the ticket is not closed yet
    pub const IN_MAIN: char = '⇂';
    /// tangential work captured mid-ticket (`discovered_in`)
    pub const DISCOVERED: char = '◇';
    /// a refusal
    pub const FAIL: char = '✗';
    /// a clean check
    pub const OK: char = '✓';
    /// the next command
    pub const FIX: char = '→';
}

/// The ONE mapping from a stored state to its badge. `State::glyph()` is the source of
/// truth for the character; this returns it as the `char` [`Line`] wants, so no command
/// has to convert — and `state_glyph_agrees_with_the_state_table` proves the two cannot
/// drift.
pub fn state_glyph(s: crate::transitions::State) -> char {
    s.glyph().chars().next().unwrap_or('?')
}

/// The ONE colour policy for a state. Terminal states are quiet; the two that can go
/// wrong are the two that are loud.
pub fn state_color(s: crate::transitions::State) -> Color {
    use crate::transitions::State as S;
    match s {
        S::Todo => Color::Dim,
        S::Doing => Color::Yellow,
        S::Review => Color::Blue,
        S::Done => Color::Green,
        S::Dropped => Color::Dim,
    }
}

/// The shared human primitive: most output is a list of these.
///
/// ```text
/// ⇂ t-31aa   in main 2h (gh-pr #142 · checked 4m ago)   → kanspec done t-31aa
/// ```
#[derive(Clone, Debug, Default)]
pub struct Line {
    pub glyph: char,
    pub id: Option<String>,
    pub text: String,
    pub dim: Option<String>,
    pub fix: Option<String>,
    pub url: Option<String>,
}

impl Line {
    pub fn new(glyph: char, text: impl Into<String>) -> Line {
        Line {
            glyph,
            text: text.into(),
            ..Line::default()
        }
    }
    /// A line badged with a ticket's stored state — the shape `status`, `ls` and the
    /// board's terminal rendering all use, so the glyph cannot vary by command.
    pub fn state(state: crate::transitions::State, text: impl Into<String>) -> Line {
        Line::new(state_glyph(state), text)
    }
    pub fn id(mut self, id: impl std::fmt::Display) -> Line {
        self.id = Some(id.to_string());
        self
    }
    pub fn dim(mut self, d: impl Into<String>) -> Line {
        self.dim = Some(d.into());
        self
    }
    /// An EMPTY fix is no fix. Every caller spells the owed verb
    /// `next.first().cloned().unwrap_or_default()`, and a terminal ticket owes nothing — so
    /// without this guard `Some("")` reached `write` and rendered a dangling `→` with
    /// nothing after it (round C: `kanspec show` on a `done` ticket). Guarding here fixes
    /// all eight call sites at once and keeps the ninth from having to remember.
    pub fn fix(mut self, f: impl Into<String>) -> Line {
        let f = f.into();
        self.fix = (!f.trim().is_empty()).then_some(f);
        self
    }
    pub fn url(mut self, u: impl Into<String>) -> Line {
        self.url = Some(u.into());
        self
    }

    pub fn write(&self, w: &mut dyn Write, st: &Style) -> std::io::Result<()> {
        let mut left = format!(" {} ", self.glyph);
        if let Some(id) = &self.id {
            left.push_str(&format!("{:<9}", paint(id, Color::Bold, st.color)));
        }
        left.push_str(&self.text);
        if let Some(d) = &self.dim {
            left.push(' ');
            left.push_str(&paint(d, Color::Dim, st.color));
        }
        // The fix (or URL) is the point of the line: right-align it when it fits, and put
        // it on its own continuation line when it does not.
        let tail = self
            .fix
            .as_deref()
            .map(|f| format!("→ {f}"))
            .or_else(|| self.url.clone());
        match tail {
            None => writeln!(w, "{left}"),
            Some(t) => {
                let visible = visible_len(&left) + 3 + t.chars().count();
                if visible <= st.width {
                    let pad = st.width - visible + 3;
                    writeln!(w, "{left}{:pad$}{}", "", paint(&t, Color::Cyan, st.color))
                } else {
                    writeln!(w, "{left}\n    {}", paint(&t, Color::Cyan, st.color))
                }
            }
        }
    }
}

/// ANSI escapes do not occupy columns; count what the terminal actually shows.
fn visible_len(s: &str) -> usize {
    let mut n = 0usize;
    let mut in_esc = false;
    for c in s.chars() {
        if in_esc {
            if c == 'm' {
                in_esc = false;
            }
        } else if c == '\u{1b}' {
            in_esc = true;
        } else {
            n += 1;
        }
    }
    n
}

/// comfy-table preset wrapper — one table style for the whole product.
pub struct Table;

impl Table {
    // A factory over a FOREIGN type, deliberately: `out::Table` is a namespace for the
    // one table style in the product, not a value anyone constructs.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(headers: &[&str], st: &Style) -> comfy_table::Table {
        let mut t = comfy_table::Table::new();
        Table::kanspec_preset(&mut t, st);
        t.set_header(headers.iter().map(|h| h.to_string()).collect::<Vec<_>>());
        t
    }

    /// NOTE for slices: comfy-table **8.0** replaced 7.x's `load_preset(&str)` with
    /// `load_style(TableStyle)`. `presets::NOTHING` is the borderless style kanspec uses —
    /// aligned columns, no box drawing, because the board's glyphs are the visual grammar.
    pub fn kanspec_preset(t: &mut comfy_table::Table, st: &Style) {
        t.load_style(comfy_table::presets::NOTHING)
            .set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
            .set_width(st.width.min(u16::MAX as usize) as u16);
    }
}

/// `"14m ago"`, `"3h"`, `"2d"` — the freshness stamp on every badge.
pub fn rel_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds();
    let (n, unit) = match secs.abs() {
        s if s < 60 => (s, "s"),
        s if s < 3_600 => (s / 60, "m"),
        s if s < 86_400 => (s / 3_600, "h"),
        s if s < 2_592_000 => (s / 86_400, "d"),
        s => (s / 2_592_000, "mo"),
    };
    if secs < 0 {
        format!("in {n}{unit}")
    } else {
        format!("{n}{unit} ago")
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Color {
    Red,
    Green,
    Yellow,
    Blue,
    Cyan,
    Dim,
    Bold,
}

pub fn paint(s: &str, c: Color, on: bool) -> String {
    if !on {
        return s.to_string();
    }
    use owo_colors::OwoColorize;
    match c {
        Color::Red => s.red().to_string(),
        Color::Green => s.green().to_string(),
        Color::Yellow => s.yellow().to_string(),
        Color::Blue => s.blue().to_string(),
        Color::Cyan => s.cyan().to_string(),
        Color::Dim => s.dimmed().to_string(),
        Color::Bold => s.bold().to_string(),
    }
}

static COLOR: AtomicBool = AtomicBool::new(false);

/// Set ONCE in `run()` from `--color`, never from the env dance: recon proved
/// `CLICOLOR_FORCE` beats `NO_COLOR` in owo-colors' supports-color, which is the opposite
/// of what the no-color.org convention says, so kanspec decides the policy itself.
pub fn apply_color_policy(choice: ColorChoice) -> bool {
    let on = match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
                false
            } else if std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0") {
                true
            } else {
                std::io::stdout().is_terminal()
            }
        }
    };
    COLOR.store(on, Ordering::Relaxed);
    on
}

/// The policy `run()` already decided. `Ctx::open` reads it; nothing else should.
pub fn color_enabled() -> bool {
    COLOR.load(Ordering::Relaxed)
}

pub fn term_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .filter(|w| *w >= 40)
        .unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn rel_time_reads_like_the_transcripts() {
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        assert_eq!(
            rel_time(now - chrono::Duration::seconds(11), now),
            "11s ago"
        );
        assert_eq!(
            rel_time(now - chrono::Duration::minutes(14), now),
            "14m ago"
        );
        assert_eq!(rel_time(now - chrono::Duration::hours(3), now), "3h ago");
        assert_eq!(rel_time(now - chrono::Duration::days(2), now), "2d ago");
        assert_eq!(rel_time(now + chrono::Duration::hours(2), now), "in 2h");
    }

    #[test]
    fn a_line_carries_its_fix() {
        let mut buf: Vec<u8> = Vec::new();
        Line::new('⇂', "in main 2h")
            .id("t-31aa")
            .fix("kanspec done t-31aa")
            .write(&mut buf, &Style::plain())
            .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with(" ⇂ t-31aa"), "{s:?}");
        assert!(s.contains("→ kanspec done t-31aa"), "{s:?}");
    }

    /// ROUND-C REGRESSION. A terminal ticket owes no verb, so every `.fix(next.first()
    /// .cloned().unwrap_or_default())` call site hands `fix` an empty string. It must
    /// render as no arrow at all — `kanspec show` on a `done` ticket printed a dangling
    /// `→` with nothing after it.
    #[test]
    fn an_empty_fix_prints_no_arrow_at_all() {
        for empty in ["", "   "] {
            let mut buf: Vec<u8> = Vec::new();
            Line::new('●', "Rate-limit login endpoint")
                .id("t-63b9")
                .fix(empty)
                .write(&mut buf, &Style::plain())
                .unwrap();
            let s = String::from_utf8(buf).unwrap();
            assert!(
                !s.contains('→'),
                "empty fix {empty:?} still drew an arrow: {s:?}"
            );
            assert_eq!(
                s.trim_end(),
                " ● t-63b9   Rate-limit login endpoint",
                "{s:?}"
            );
        }
    }

    #[test]
    fn paint_is_a_no_op_without_color() {
        assert_eq!(paint("x", Color::Red, false), "x");
        assert!(paint("x", Color::Red, true).len() > 1);
        assert_eq!(visible_len(&paint("abc", Color::Red, true)), 3);
    }

    #[test]
    fn state_glyph_agrees_with_the_state_table_and_covers_design_mds_legend() {
        use crate::transitions::{State, ALL_STATES};
        // The two must not drift: `State::glyph()` is the source of truth, and this is the
        // only conversion of it anywhere in the crate.
        for &s in ALL_STATES {
            assert_eq!(state_glyph(s).to_string(), s.glyph(), "{s}");
            let _ = state_color(s);
        }
        // DESIGN.md's legend, verbatim: `○ todo ◐ doing ◈ review ⇂ in-main ● done ✕ dropped`
        assert_eq!(state_glyph(State::Todo), '○');
        assert_eq!(state_glyph(State::Doing), '◐');
        assert_eq!(state_glyph(State::Review), '◈');
        assert_eq!(state_glyph(State::Done), '●');
        assert_eq!(state_glyph(State::Dropped), '✕');
        assert_eq!(glyph::IN_MAIN, '⇂');
    }

    #[test]
    fn a_status_line_renders_the_shape_design_md_shows() {
        let mut buf: Vec<u8> = Vec::new();
        Line::new(
            glyph::IN_MAIN,
            "in main 2h (gh-pr #142 · checked 4m ago), not closed",
        )
        .id("t-31aa")
        .fix("kanspec done t-31aa")
        .write(&mut buf, &Style::plain())
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with(" ⇂ t-31aa   in main 2h"), "{s:?}");
        assert!(s.trim_end().ends_with("→ kanspec done t-31aa"), "{s:?}");

        let mut buf: Vec<u8> = Vec::new();
        Line::state(
            crate::transitions::State::Doing,
            "STALLED: no commits for 3h",
        )
        .id("t-88fe")
        .fix("kanspec park t-88fe --why \"...\"")
        .write(&mut buf, &Style::plain())
        .unwrap();
        assert!(String::from_utf8(buf).unwrap().starts_with(" ◐ t-88fe"));
    }
}
