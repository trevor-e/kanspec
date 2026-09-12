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

use chrono::{DateTime, Utc};
use unicode_width::UnicodeWidthStr;

use crate::cli::ColorChoice;
use crate::ctx::OutMode;
use crate::error::{KsError, Result};

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub color: bool,
    pub width: usize,
    /// `"kanspec"` or `"ks"`: every fix a `Line` prints is spelled with it, at write time,
    /// so a pure producer (derive, transitions) writes `kanspec …` and never knows the
    /// binary's name (t-a535).
    pub invoked_as: &'static str,
}

impl Style {
    pub fn plain() -> Style {
        Style {
            invoked_as: "kanspec",
            color: false,
            width: 100,
        }
    }
}

/// The second and LAST trait in the crate.
pub trait Render: serde::Serialize {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()>;
}

/// `a, b, c` — the ONE spelling of a joined list in human output.
pub fn join<T: std::fmt::Display>(v: impl IntoIterator<Item = T>, sep: &str) -> String {
    v.into_iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(sep)
}

/// The `  → next` lines under a knowledge verb's report: two-space indent, painted cyan.
/// (The ticket verbs use a three-space, unpainted variant on purpose — DESIGN.md's
/// transcripts — so this is not for them.)
pub fn write_next(w: &mut dyn Write, st: &Style, next: &[String]) -> std::io::Result<()> {
    for n in next {
        writeln!(w, "  {} {}", glyph::FIX, paint(n, Color::Cyan, st.color))?;
    }
    Ok(())
}

/// The single emit point. A broken pipe (`kanspec ls | head`) is success, not an error.
pub fn emit<R: Render>(r: &R, mode: &OutMode, invoked_as: &'static str) -> Result<()> {
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
                invoked_as,
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

/// The shared human primitive: most output is a list of these.
///
/// ```text
/// ⇂ t-31aa-lockout-table        in main 2h (gh-pr #142 · checked 4m ago)   → kanspec done t-31aa
/// ```
///
/// The id column carries a display *label* — key plus a short title slug, see
/// [`crate::ids::label`] — wherever the caller knows the title; the fix keeps the bare
/// key, which is all any verb needs.
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
            // MUST be `pad_visible`, never `{:<9}`: `paint` returns a string that already
            // carries ANSI escapes, so `t-ec64` is 6 chars plain and 14 painted, and the
            // width specifier padded nothing at all — the id and the title rendered jammed
            // together for every colour user, on every command. See `pad_visible`.
            // `ID_COL` is a floor, not the width: `id_width` is a config knob clamped to
            // 4..=16, so an id at or past `ID_COL` would abut the text with no separator —
            // the same jam, from a different cause, and invisible to a colour-vs-plain diff.
            let painted = paint(id, Color::Bold, st.color);
            let col = ID_COL.max(visible_len(&painted) + 1);
            left.push_str(&pad_visible(&painted, col));
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
            .map(|f| format!("→ {}", spoken_as(f, st.invoked_as)))
            .or_else(|| self.url.clone());
        match tail {
            None => writeln!(w, "{left}"),
            Some(t) => {
                // BOTH halves measure in columns. `t.chars().count()` was the same D-50
                // undercount as the old `visible_len`, and it is reachable through a wide
                // ticket TITLE quoted into a `--why` fix, not only through `left`.
                let visible = visible_len(&left) + 3 + visible_len(&t);
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

/// The id column in [`Line::write`] — wide enough for a full display label
/// (`t-31aa-` + [`crate::ids::LABEL_SLUG_MAX`] chars of slug) plus the three spaces
/// DESIGN.md's transcripts show before the text. Fixed rather than fitted so the text
/// column lines up across lines whose labels differ in length.
const ID_COL: usize = 2 + 4 + 1 + crate::ids::LABEL_SLUG_MAX + 3;

/// Pad `s` on the right to `width` **visible** columns, never truncating.
///
/// THE ONE WAY to pad in this crate, because `format!("{:<9}", s)` counts `char`s and a
/// painted `s` carries ANSI escapes that occupy no columns at all: `{:<9}` over a painted
/// six-character id saw fourteen chars, decided the field was already full, and emitted no
/// padding — which is why colour output jammed every id into its title while the piped,
/// colourless output every test captures stayed perfectly aligned.
///
/// Any future column in this file pads through here; nothing painted may reach a `{:<N}`.
pub fn pad_visible(s: &str, width: usize) -> String {
    let visible = visible_len(s);
    let mut out = String::with_capacity(s.len() + width.saturating_sub(visible));
    out.push_str(s);
    for _ in visible..width {
        out.push(' ');
    }
    out
}

/// ANSI escapes occupy no columns, and a wide glyph occupies two: count what the terminal
/// actually DRAWS.
///
/// THE ONE measurement in this file. [`pad_visible`] and [`Line::write`]'s right-aligned
/// tail both budget through it, so this one function governs every column in the product.
///
/// It used to count `char`s (ARCHITECTURE D-50), which is right only for ASCII:
///
/// ```text
/// chars=100  columns=100   plain ascii title
/// chars=100  columns=102   emoji title
/// chars=100  columns=117   CJK title
/// ```
///
/// A CJK title therefore measured seventeen columns short, the tail was right-aligned into
/// space the terminal did not have, and the line wrapped — destroying the alignment this
/// whole file exists to produce. `unicode-width` is the East Asian Width table (UAX #11)
/// plus the emoji rules, and it is measured over the RUN rather than per `char` so that a
/// ZWJ emoji sequence and a combining mark count once. It is already in the tree —
/// `comfy-table`, which measures the board's columns, depends on it — so the two width
/// models in the crate now agree by construction rather than by luck.
///
/// Still SGR-only, deliberately: an escape ends at the first `m`, which is right for every
/// escape kanspec emits and wrong for the OSC-8 hyperlinks it does not.
pub fn visible_len(s: &str) -> usize {
    let mut cols = 0usize;
    // The byte where the current *visible* run starts. While `in_esc`, it is stale and
    // deliberately unread: the escape's own end reassigns it.
    let mut run = 0usize;
    let mut in_esc = false;
    for (i, c) in s.char_indices() {
        if in_esc {
            if c == 'm' {
                in_esc = false;
                run = i + c.len_utf8();
            }
        } else if c == '\u{1b}' {
            cols += s[run..i].width();
            in_esc = true;
        }
    }
    if !in_esc {
        cols += s[run..].width();
    }
    cols
}

// ─────────────────────────────────────────────────────────────────────────────
// Speaking the name the user typed
// ─────────────────────────────────────────────────────────────────────────────

/// Rewrite the command word of a next-command string to the binary the user actually
/// typed — `ks` for `src/bin/ks.rs`, `kanspec` for `src/bin/kanspec.rs`.
///
/// [`Fix::cmd`](crate::error::Fix::cmd) calls this, which is what lets all ~123 hardcoded
/// `fix!("kanspec …")` call sites keep saying `kanspec` in the source and still tell a `ks`
/// user to run `ks`. Doing it at construction rather than per surface also means the human
/// rendering and the `#[serde(transparent)]` `--json` fix list cannot disagree about what
/// to run.
///
/// **A naive `replace("kanspec", ks)` CORRUPTS real fix strings**, which is the whole
/// design constraint. All three of these ship today:
///
/// ```text
/// git add -A .kanspec && git commit -m "kanspec: sync"    # a commit MESSAGE
/// open /w/.kanspec/tickets/t-31aa.md and add a `---` …    # a PATH
/// unset KANSPEC_NOW                                       # an env var
/// ```
///
/// The rewrite fires only where `kanspec` stands in COMMAND POSITION: at the very start
/// of the string, or immediately after a backtick (the fixes that quote a command inside
/// prose), and only where the word ends there — end of string, a space, or the closing
/// backtick. `.kanspec/` is preceded by `.`, `"kanspec: sync"` is preceded by `"` and
/// followed by `:`, and `KANSPEC_NOW` is not even the same bytes; none of the three can
/// match, whatever else is in the string.
/// `spoken_as(cmd, "ks")` rewrites the command word to the binary the user typed. Applied
/// at RENDER time — `Line::write`, `KsError::render`, `KsError::to_json_as` — against the
/// name the `Ctx` carries, never against a process global (t-a535).
pub fn spoken_as(cmd: &str, ks: &str) -> String {
    const NAME: &str = "kanspec";
    if ks == NAME {
        return cmd.to_string();
    }
    let mut out = String::with_capacity(cmd.len());
    let mut at = 0usize;
    while let Some(i) = cmd[at..].find(NAME) {
        let start = at + i;
        let end = start + NAME.len();
        // Command position OPENS here: the string starts, or a backtick quoted a command.
        // Never after the `.` of `.kanspec/`, nor the `"` of a commit message.
        let opens = start == 0 || cmd[..start].ends_with('`');
        // …and the word ENDS here: `kanspec: sync` and `kanspec_thing` are not commands.
        let closes = matches!(cmd[end..].chars().next(), None | Some(' ') | Some('`'));
        out.push_str(&cmd[at..start]);
        out.push_str(if opens && closes { ks } else { NAME });
        at = end;
    }
    out.push_str(&cmd[at..]);
    out
}

/// comfy-table preset wrapper — one table style for the whole product.
pub struct Table;

impl Table {
    // A factory over a FOREIGN type, deliberately: `out::Table` is a namespace for the
    // one table style in the product, not a value anyone constructs.
    #[allow(clippy::new_ret_no_self)]
    ///
    /// NOTE for slices: comfy-table **8.0** replaced 7.x's `load_preset(&str)` with
    /// `load_style(TableStyle)`. `presets::NOTHING` is the borderless style kanspec uses —
    /// aligned columns, no box drawing, because the board's glyphs are the visual grammar.
    pub fn new(headers: &[&str], st: &Style) -> comfy_table::Table {
        let mut t = comfy_table::Table::new();
        t.load_style(comfy_table::presets::NOTHING)
            .set_content_arrangement(comfy_table::ContentArrangement::Dynamic)
            .set_width(st.width.min(u16::MAX as usize) as u16)
            .set_header(headers.iter().map(|h| h.to_string()).collect::<Vec<_>>());
        t
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

/// Decided from `--color` alone (via `OutMode::from_cli`), never from the env dance: recon
/// proved `CLICOLOR_FORCE` beats `NO_COLOR` in owo-colors' supports-color, which is the
/// opposite of what the no-color.org convention says, so kanspec decides the policy itself.
pub fn apply_color_policy(choice: ColorChoice) -> bool {
    match choice {
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
    }
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
                format!(
                    " ● {}Rate-limit login endpoint",
                    pad_visible("t-63b9", ID_COL)
                ),
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

    /// ROUND-E REGRESSION (ARCHITECTURE D-50), the half `tests/render_color.rs` reaches
    /// only through `left`: the TAIL was measured with `t.chars().count()`, which is the
    /// same undercount, and a wide glyph gets into the tail whenever a fix quotes a ticket
    /// title back at you (`kanspec park <id> --why "…"`).
    ///
    /// Both budgets are now columns, so the identity that makes the arrows form a column
    /// holds for a wide line too: a fix that fits is padded until the line ends at EXACTLY
    /// `st.width`, and a fix that does not fit takes its own continuation line rather than
    /// wrapping.
    #[test]
    fn a_wide_glyph_is_budgeted_in_columns_on_both_sides_of_the_line() {
        // Wide enough that the labelled id column (`ID_COL`) plus the quoted CJK fix below
        // still fits (105 columns), and narrow enough that the wide-text case after it
        // (124 columns) does not.
        let st = Style {
            width: 110,
            ..Style::plain()
        };
        let render = |line: &Line| {
            let mut buf: Vec<u8> = Vec::new();
            line.write(&mut buf, &st).unwrap();
            String::from_utf8(buf).unwrap()
        };

        // WIDE TAIL. 16 CJK characters are 32 columns, not 16.
        let cjk = "認証トークンの有効期限を延長する";
        assert_eq!(cjk.chars().count(), 16);
        assert_eq!(visible_len(cjk), 32);
        let out = render(
            &Line::state(crate::transitions::State::Doing, "stalled")
                .id("t-88fe")
                .fix(format!("kanspec park t-88fe --why \"{cjk}\"")),
        );
        let drawn = visible_len(out.trim_end_matches('\n'));
        assert_eq!(drawn, st.width, "a fitting line ends at the width: {out:?}");

        // WIDE TEXT that fits on its own but leaves no room for the fix: the fix takes its
        // own continuation line instead of being right-aligned into columns that are not
        // there. 34 chars, 68 columns.
        let long = format!("{cjk}{cjk}認証");
        assert_eq!((long.chars().count(), visible_len(&long)), (34, 68));
        let out = render(
            &Line::state(crate::transitions::State::Todo, &long)
                .id("t-0001")
                .fix("kanspec start t-0001"),
        );
        let (first, rest) = out.trim_end_matches('\n').split_once('\n').expect("a wrap");
        assert_eq!(visible_len(first), 3 + ID_COL + 68, "{first:?}");
        assert_eq!(rest, "    → kanspec start t-0001");
        // THE BUG: the `char` count made that 3 + ID_COL + 34 + 3 + 20, which "fits", so
        // the fix was right-aligned — and the line the terminal actually drew was wider.
        assert!(
            3 + ID_COL + long.chars().count() + 3 + 20 <= st.width,
            "the old budget thought this fit"
        );
    }

    /// ROUND-D REGRESSION, ranked #1 by the adversarial audit: it hit 100% of human
    /// sessions on every id-bearing command, and all 463 tests were blind to it because a
    /// test captures a pipe and a pipe has colour off.
    ///
    /// `format!("{:<9}", paint(id, Bold, true))` sees fourteen `char`s, not six, so it pads
    /// nothing:
    ///
    /// ```text
    ///  o t-ec64   Padding probe ticket · auth      # --color never
    ///  o <b>t-ec64</b>Padding probe ticket · auth  # --color always  ← jammed
    /// ```
    #[test]
    fn padding_counts_visible_columns_not_chars() {
        let painted = paint("t-ec64", Color::Bold, true);
        assert!(
            painted.chars().count() > 6,
            "the premise: a painted id is longer than it looks — {painted:?}"
        );
        assert_eq!(visible_len(&painted), 6);

        // The helper pads a PAINTED id to the column's visible width…
        let padded = pad_visible(&painted, ID_COL);
        assert_eq!(visible_len(&padded), ID_COL, "{padded:?}");
        assert!(padded.ends_with("   "), "{padded:?}");
        // …which is exactly what the old `{:<9}` failed to do.
        assert_eq!(
            visible_len(&format!("{painted:<9}")),
            6,
            "the bug itself: a width specifier over a painted string is a no-op"
        );

        // Plain input is byte-identical to the specifier it replaces.
        assert_eq!(
            pad_visible("t-ec64", ID_COL),
            format!("{:<width$}", "t-ec64", width = ID_COL)
        );
        // Over-wide input is never truncated — an id wider than the column pushes the
        // text right rather than losing characters.
        assert_eq!(pad_visible("t-abcdefghij", 9), "t-abcdefghij");
        assert_eq!(pad_visible(&paint("wide-id-here", Color::Bold, true), 4), {
            paint("wide-id-here", Color::Bold, true)
        });
    }

    /// `id_width` is a config knob clamped to 4..=16, so `ID_COL` cannot be the whole
    /// story: at width 7 the id reached the column and the text abutted it with no gap —
    /// the original jam, from a second cause, and colour-independent, so the
    /// colour-vs-plain property could not see it.
    #[test]
    fn an_id_never_abuts_its_text_at_any_configured_width() {
        for width in 4..=16 {
            let id = format!("t-{}", "a".repeat(width - 2));
            assert_eq!(id.len(), width);
            for color in [false, true] {
                let mut buf: Vec<u8> = Vec::new();
                Line::state(crate::transitions::State::Todo, "ready")
                    .id(&id)
                    .write(
                        &mut buf,
                        &Style {
                            color,
                            ..Style::plain()
                        },
                    )
                    .unwrap();
                let rendered = strip_ansi(&String::from_utf8(buf).unwrap());
                assert!(
                    rendered.contains(&format!("{id} ")),
                    "id abuts its text at id_width={width}, color={color}: {rendered:?}"
                );
            }
        }
    }

    /// The property that generalises the fix: a painted line is the plain line plus
    /// escapes, and **nothing else** — same columns, same right-aligned fix.
    #[test]
    fn a_painted_line_is_the_plain_line_plus_escapes() {
        let lines = [
            Line::new(glyph::IN_MAIN, "in main 2h (gh-pr #142 · checked 4m ago)")
                .id("t-31aa")
                .fix("kanspec done t-31aa"),
            Line::state(
                crate::transitions::State::Todo,
                "Padding probe ticket · auth",
            )
            .id("t-ec64")
            .fix("kanspec start t-ec64"),
            Line::state(
                crate::transitions::State::Doing,
                "STALLED: no commits for 3h",
            )
            .id("t-88fe")
            .dim("claimed by trevor")
            .fix("kanspec park t-88fe --why \"...\""),
            // No id, no fix: the shape that was already correct must stay correct.
            Line::new('·', "no open tickets").dim("nothing to do"),
            // A tail too long for the width falls to its own continuation line.
            Line::new('◈', "a very long title ".repeat(6))
                .id("t-0001")
                .fix("kanspec done t-0001"),
        ];
        for line in &lines {
            let mut plain: Vec<u8> = Vec::new();
            line.write(&mut plain, &Style::plain()).unwrap();
            let mut painted: Vec<u8> = Vec::new();
            line.write(
                &mut painted,
                &Style {
                    color: true,
                    ..Style::plain()
                },
            )
            .unwrap();
            let plain = String::from_utf8(plain).unwrap();
            let painted = String::from_utf8(painted).unwrap();
            assert_eq!(
                strip_ansi(&painted),
                plain,
                "colour changed the LAYOUT, not just the bytes:\n{painted:?}"
            );
        }
    }

    /// Deliberately NOT `visible_len` in reverse: a bug shared by the code under test and
    /// its test cancels out and proves nothing.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            if c != '\u{1b}' {
                out.push(c);
                continue;
            }
            if it.peek() == Some(&'[') {
                it.next();
                // CSI: parameter and intermediate bytes, then a final byte in `@`..=`~`.
                for c in it.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            } else {
                it.next();
            }
        }
        out
    }

    #[test]
    fn state_glyph_agrees_with_the_state_table_and_covers_design_mds_legend() {
        use crate::transitions::{State, ALL_STATES};
        // The two must not drift: `State::glyph()` is the source of truth, and this is the
        // only conversion of it anywhere in the crate.
        for &s in ALL_STATES {
            assert_eq!(state_glyph(s).to_string(), s.glyph(), "{s}");
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
        assert!(s.starts_with(" ⇂ t-31aa "), "{s:?}");
        assert!(s.contains("   in main 2h"), "{s:?}");
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
