//! The `## Log` line grammar, in one place.
//!
//! ```text
//! - 2026-08-30T14:20Z  doing     claude/sess-a91       start (branch + worktree created)
//!   └ %Y-%m-%dT%H:%MZ  state     actor (<=20, padded)  verb + optional " (note)"
//! ```
//!
//! The resulting `state` is **stored**, not a format-time parameter, so
//! `parse(format(e)) == e` and [`crate::transitions::replay`] can catch a line whose
//! printed state disagrees with its verb.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::Serialize;

use crate::transitions::{State, Verb};

pub const LOG_HEADING: &str = "## Log";
pub const STEPS_HEADING: &str = "## Steps";

/// The timestamp format written into the log — minute resolution, always UTC.
const TS_FMT: &str = "%Y-%m-%dT%H:%MZ";
const STATE_COL: usize = 9;
const ACTOR_COL: usize = 22;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub at: DateTime<Utc>,
    /// The resulting state.
    pub state: State,
    /// `Actor::label()` — never contains whitespace, which is what makes the column
    /// grammar parseable by splitting.
    pub actor: String,
    pub verb: Verb,
    pub note: Option<String>,
}

impl LogEntry {
    pub fn format(&self) -> String {
        let ts = self.at.format(TS_FMT).to_string();
        let note = match &self.note {
            Some(n) if !n.trim().is_empty() => format!(" ({})", sanitize_note(n)),
            _ => String::new(),
        };
        format!(
            "- {ts}  {state:<STATE_COL$}{actor:<ACTOR_COL$}{verb}{note}",
            state = self.state.as_str(),
            actor = sanitize_actor(&self.actor),
            verb = self.verb.as_str(),
        )
    }

    /// `None` == not a log line, so prose under `## Log` survives untouched.
    pub fn parse(line: &str) -> Option<LogEntry> {
        let rest = line.trim().strip_prefix("- ")?;
        let mut it = rest.split_whitespace();
        let at = parse_ts(it.next()?)?;
        let state = State::parse(it.next()?)?;
        let actor = it.next()?.to_string();
        // The verb token tolerates the historical `kanspec <verb>` spelling and the `ks`
        // alias, so a log written by an older binary still replays.
        let mut tok = it.next()?;
        if tok == "kanspec" || tok == "ks" {
            tok = it.next()?;
        }
        let verb = Verb::parse(tok)?;
        let tail = it.collect::<Vec<_>>().join(" ");
        let note = tail
            .strip_prefix('(')
            .and_then(|t| t.strip_suffix(')'))
            .map(|t| t.to_string())
            .filter(|t| !t.is_empty());
        Some(LogEntry {
            at,
            state,
            actor,
            verb,
            note,
        })
    }
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    // Minute resolution is what we write; second resolution is accepted so a hand-written
    // or imported line still replays.
    for fmt in [TS_FMT, "%Y-%m-%dT%H:%M:%SZ"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(s, fmt) {
            return Utc.from_local_datetime(&n).single();
        }
    }
    None
}

/// Actors never contain whitespace: the column grammar is parsed by splitting.
fn sanitize_actor(a: &str) -> String {
    a.chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .take(crate::ctx::LABEL_MAX)
        .collect()
}

/// A note is delimited by `(` … `)` at end of line; an unbalanced paren would break
/// `parse(format(e)) == e`, so it is flattened here rather than silently corrupting a log.
fn sanitize_note(n: &str) -> String {
    n.trim()
        .replace(['\n', '\r'], " ")
        .replace('(', "[")
        .replace(')', "]")
}

/// Extracts every parseable entry under the `## Log` heading of a ticket body.
/// Non-matching lines (prose, blanks) are skipped, never rejected.
pub fn parse_log(body: &str) -> Vec<LogEntry> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in body.lines() {
        let t = line.trim_end();
        if t.trim_start().starts_with("## ") {
            inside = t.trim() == LOG_HEADING;
            continue;
        }
        if inside {
            if let Some(e) = LogEntry::parse(t) {
                out.push(e);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(verb: Verb, state: State, note: Option<&str>) -> LogEntry {
        LogEntry {
            at: Utc.with_ymd_and_hms(2026, 8, 30, 14, 20, 0).unwrap(),
            state,
            actor: "claude/sess-a91".into(),
            verb,
            note: note.map(str::to_string),
        }
    }

    #[test]
    fn format_then_parse_is_the_identity() {
        for entry in [
            e(Verb::New, State::Todo, None),
            e(Verb::Start, State::Doing, Some("branch + worktree created")),
            e(Verb::Done, State::Done, Some("no-code: docs only")),
            e(
                Verb::Confirm,
                State::Review,
                Some("squash, verified by hand"),
            ),
        ] {
            let line = entry.format();
            assert_eq!(
                LogEntry::parse(&line).as_ref(),
                Some(&entry),
                "line: {line}"
            );
        }
    }

    #[test]
    fn the_design_md_lines_parse() {
        let a =
            LogEntry::parse("- 2026-08-30T14:02Z  todo   trevor           kanspec new").unwrap();
        assert_eq!(
            (a.verb, a.state, a.actor.as_str()),
            (Verb::New, State::Todo, "trevor")
        );
        let b = LogEntry::parse(
            "- 2026-08-30T14:20Z  doing  claude/sess-a91  kanspec start (branch + worktree created)",
        )
        .unwrap();
        assert_eq!(b.verb, Verb::Start);
        assert_eq!(b.note.as_deref(), Some("branch + worktree created"));
    }

    #[test]
    fn prose_under_the_heading_is_not_an_entry() {
        assert!(LogEntry::parse("some prose").is_none());
        assert!(LogEntry::parse("- not a timestamp").is_none());
        assert!(LogEntry::parse("- 2026-08-30T14:02Z  wat  trevor  new").is_none());
    }

    #[test]
    fn parse_log_reads_only_the_log_section() {
        let body = "\
Implement [auth.lockout].

## Steps
- [x] lockout counter
- 2026-08-30T14:02Z  todo   trevor  new

## Log
notes about the log
- 2026-08-30T14:02Z  todo   trevor           new
- 2026-08-30T14:20Z  doing  claude/sess-a91  start (branch created)

## Afterword
- 2026-08-30T15:00Z  done   trevor           done
";
        let log = parse_log(body);
        assert_eq!(log.len(), 2, "only the ## Log section counts: {log:#?}");
        assert_eq!(log[0].verb, Verb::New);
        assert_eq!(log[1].verb, Verb::Start);
    }
}
