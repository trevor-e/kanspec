//! The eight closed error shapes, the exit-code table, and the "next command" renderer.
//!
//! **This file never grows.** A new refusal is
//! `KsError::gate("undispositioned", msg, fixes![fix!("kanspec close {id}")])` **in the
//! raising agent's own file** — `code: &'static str` is the stable JSON discriminator, so
//! agent-facing error kinds stay as precise as a per-situation enum without making
//! `error.rs` the worst merge magnet in a nine-agent build.
//!
//! Invariant 9 ("every anomaly names its one-command fix") is a *type* constraint here:
//! [`Fixes`] is non-empty by construction, so you cannot build a `KsError` without naming
//! the next command — and unlike a `debug_assert` that survives `cargo build --release`.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use std::fmt::Write as _;
use std::io::Write as _;

use serde::Serialize;

use crate::ctx::OutMode;
use crate::ids::TicketId;
use crate::transitions::{State, Verb};

// ─────────────────────────────────────────────────────────────────────────────
// Fix / Fixes — non-empty BY TYPE
// ─────────────────────────────────────────────────────────────────────────────

/// One runnable next command. Rendered as `  → kanspec show t-9c41`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Fix(String);

impl Fix {
    pub fn cmd(s: impl Into<String>) -> Fix {
        Fix(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Fix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Non-empty BY TYPE: head + tail. `debug_assert` is deleted by `--release`; this is not.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Fixes(Fix, Vec<Fix>);

impl Fixes {
    pub fn one(f: Fix) -> Fixes {
        Fixes(f, Vec::new())
    }
    pub fn new(head: Fix, rest: Vec<Fix>) -> Fixes {
        Fixes(head, rest)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Fix> {
        std::iter::once(&self.0).chain(self.1.iter())
    }
    /// The head fix — the one a single-line renderer shows.
    pub fn head(&self) -> &Fix {
        &self.0
    }
}

/// `fix!("kanspec done {id}")` — one formatted next command.
#[macro_export]
macro_rules! fix {
    ($($t:tt)*) => { $crate::error::Fix::cmd(format!($($t)*)) };
}

/// `fixes![fix!("a"), fix!("b")]` — a non-empty fix list; the first argument is mandatory.
#[macro_export]
macro_rules! fixes {
    ($h:expr $(, $r:expr)* $(,)?) => { $crate::error::Fixes::new($h, vec![$($r),*]) };
}

// ─────────────────────────────────────────────────────────────────────────────
// The shapes
// ─────────────────────────────────────────────────────────────────────────────

/// Why the environment is unusable. Distinguishing these is what lets a wrapper script
/// tell "no `.kanspec/` here" apart from "a gate refused" (J-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvCode {
    NotARepo,
    NotInitialized,
    LockHeld,
    AmbiguousWorktree,
    GitMissing,
}

/// Structured payloads for the TWO errors whose output quality *is* the product.
/// Agents constructing a new refusal use `GateDetail::Plain` and never edit this file.
#[derive(Debug, Serialize)]
#[serde(tag = "detail", rename_all = "snake_case")]
pub enum GateDetail {
    Plain,
    NotLanded { trace: Vec<crate::git::RungTrace> },
    Undispositioned { items: Vec<crate::ids::ItemRef> },
}

#[derive(Debug, thiserror::Error)]
pub enum KsError {
    // NOTE the deviation from the contract's literal `#[error]` attribute, which reads
    // `"{id} is {from}, not {}, — cannot {verb}", verbs_str(*allowed)`: that renders
    // "t-9c41 is review, not start, done, drop, — cannot ship", which contradicts the
    // contract's own worked example ("✗ t-9c41 is review, not doing — cannot ship").
    // The `{}` slot is the state(s) the verb is legal FROM, so it is filled by
    // `states_for(verb)`; `allowed` is kept as a field because it is the useful thing
    // to put in the JSON envelope and in the caller's `Fixes`.
    #[error("{id} is {from}, not {} — cannot {verb}",
            crate::transitions::states_str(crate::transitions::states_for(*verb)))]
    IllegalTransition {
        id: TicketId,
        from: State,
        verb: Verb,
        allowed: &'static [Verb],
        fix: Fixes,
    },
    #[error("{kind} {id} not found")]
    NotFound {
        kind: &'static str,
        id: String,
        fix: Fixes,
    },
    #[error("{message}")]
    Gate {
        code: &'static str,
        message: String,
        detail: GateDetail,
        fix: Fixes,
    },
    #[error("{message}")]
    Environment {
        code: EnvCode,
        message: String,
        fix: Fixes,
    },
    #[error("{message}")]
    Git {
        message: String,
        cmd: String,
        exit: i32,
        fix: Fixes,
    },
    /// duplicate claim, id collision
    #[error("{message}")]
    Conflict { message: String, fix: Fixes },
    /// bad args, bad YAML, bad id
    #[error("{message}")]
    Invalid { message: String, fix: Fixes },
    #[error("internal error: {source}")]
    Internal {
        #[source]
        source: anyhow::Error,
        fix: Fixes,
    },
}

impl KsError {
    pub fn gate(code: &'static str, message: impl Into<String>, fix: Fixes) -> KsError {
        KsError::Gate {
            code,
            message: message.into(),
            detail: GateDetail::Plain,
            fix,
        }
    }

    /// A gate refusal carrying a structured payload — the ladder trace or the
    /// undispositioned item list. Pre-formatting either into `message` would throw away
    /// the `--explain`-grade output that is this tool's selling point.
    pub fn gate_detail(
        code: &'static str,
        message: impl Into<String>,
        detail: GateDetail,
        fix: Fixes,
    ) -> KsError {
        KsError::Gate {
            code,
            message: message.into(),
            detail,
            fix,
        }
    }

    pub fn not_found(kind: &'static str, id: impl Into<String>, fix: Fixes) -> KsError {
        KsError::NotFound {
            kind,
            id: id.into(),
            fix,
        }
    }

    pub fn invalid(message: impl Into<String>, fix: Fixes) -> KsError {
        KsError::Invalid {
            message: message.into(),
            fix,
        }
    }

    pub fn conflict(message: impl Into<String>, fix: Fixes) -> KsError {
        KsError::Conflict {
            message: message.into(),
            fix,
        }
    }

    pub fn environment(code: EnvCode, message: impl Into<String>, fix: Fixes) -> KsError {
        KsError::Environment {
            code,
            message: message.into(),
            fix,
        }
    }

    /// There is NO `#[from] std::io::Error`. A bare `?` on a file op cannot produce a
    /// fix-less error; every call site converts deliberately.
    pub fn internal(e: impl Into<anyhow::Error>) -> KsError {
        KsError::Internal {
            source: e.into(),
            fix: fixes![fix!("kanspec doctor")],
        }
    }

    /// TOTAL over the enum — every variant, including `Internal`. Invariant 9.
    pub fn fixes(&self) -> &Fixes {
        match self {
            KsError::IllegalTransition { fix, .. }
            | KsError::NotFound { fix, .. }
            | KsError::Gate { fix, .. }
            | KsError::Environment { fix, .. }
            | KsError::Git { fix, .. }
            | KsError::Conflict { fix, .. }
            | KsError::Invalid { fix, .. }
            | KsError::Internal { fix, .. } => fix,
        }
    }

    /// The stable JSON discriminator. Agents branch on this; it never changes shape.
    pub fn kind(&self) -> &'static str {
        match self {
            KsError::IllegalTransition { .. } => "illegal_transition",
            KsError::NotFound { .. } => "not_found",
            KsError::Gate { .. } => "gate",
            KsError::Environment { .. } => "environment",
            KsError::Git { .. } => "git",
            KsError::Conflict { .. } => "conflict",
            KsError::Invalid { .. } => "invalid",
            KsError::Internal { .. } => "internal",
        }
    }

    /// The situation code inside the shape — `Gate`'s `code`, `Environment`'s `EnvCode`.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            KsError::Gate { code, .. } => Some(code),
            KsError::Environment { code, .. } => Some(match code {
                EnvCode::NotARepo => "not_a_repo",
                EnvCode::NotInitialized => "not_initialized",
                EnvCode::LockHeld => "lock_held",
                EnvCode::AmbiguousWorktree => "ambiguous_worktree",
                EnvCode::GitMissing => "git_missing",
            }),
            _ => None,
        }
    }

    pub fn detail(&self) -> Option<&GateDetail> {
        match self {
            KsError::Gate { detail, .. } => Some(detail),
            _ => None,
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            KsError::Environment { .. } => code::ENVIRONMENT, // 69
            KsError::Internal { .. } => code::INTERNAL,       // 70
            _ => code::VIOLATION,                             // 1
        }
    }

    /// human -> stderr; json -> a `{"ok":false,…}` envelope on stdout, so an agent that
    /// only reads stdout still gets the refusal and its fix list.
    pub fn render(&self, mode: &OutMode) {
        match mode {
            OutMode::Json => {
                let mut out = std::io::stdout().lock();
                let _ = writeln!(
                    out,
                    "{}",
                    serde_json::to_string_pretty(&self.to_json())
                        .unwrap_or_else(|_| r#"{"ok":false}"#.to_string())
                );
            }
            OutMode::Human { color } => {
                let mut err = std::io::stderr().lock();
                let _ = err.write_all(self.render_human(*color).as_bytes());
            }
        }
    }

    /// The exact bytes `render` writes in human mode. Split out so it is unit-testable
    /// without capturing a global stream.
    pub fn render_human(&self, color: bool) -> String {
        use crate::out::{paint, Color};
        let mut s = String::new();
        let _ = writeln!(s, "{} {}", paint("✗", Color::Red, color), self);

        // The two structured payloads render their evidence between the message and the
        // fixes — a ladder trace is the whole reason `done` refused.
        if let Some(GateDetail::NotLanded { trace }) = self.detail() {
            for t in trace {
                let _ = writeln!(
                    s,
                    "    {:<10} {:<48} exit {:<3} {}",
                    t.method.as_str(),
                    t.cmd,
                    t.exit,
                    t.saw
                );
            }
        }
        if let Some(GateDetail::Undispositioned { items }) = self.detail() {
            for i in items {
                let _ = writeln!(s, "    {i}");
            }
        }
        for f in self.fixes().iter() {
            let _ = writeln!(s, "  {} {}", paint("→", Color::Cyan, color), f.as_str());
        }
        s
    }

    /// `{"ok":false,"error":{kind,code,message,detail,fix,exit}}`
    pub fn to_json(&self) -> serde_json::Value {
        let fixes: Vec<&str> = self.fixes().iter().map(Fix::as_str).collect();
        let mut error = serde_json::json!({
            "kind": self.kind(),
            "message": self.to_string(),
            "fix": fixes,
            "exit": self.exit_code(),
        });
        if let Some(code) = self.code() {
            error["code"] = serde_json::Value::String(code.to_string());
        }
        if let Some(d) = self.detail() {
            if !matches!(d, GateDetail::Plain) {
                error["detail"] = serde_json::to_value(d).unwrap_or(serde_json::Value::Null);
            }
        }
        if let KsError::IllegalTransition {
            id, from, allowed, ..
        } = self
        {
            error["ticket"] = serde_json::Value::String(id.to_string());
            error["from"] = serde_json::to_value(from).unwrap_or(serde_json::Value::Null);
            error["allowed"] = serde_json::to_value(allowed).unwrap_or(serde_json::Value::Null);
        }
        serde_json::json!({ "ok": false, "error": error })
    }
}

pub type Result<T> = std::result::Result<T, KsError>;

/// Exit codes. `0` ok · `1` gate refusal / invariant violation / `doctor` findings ·
/// `2` **landcheck Stop-hook block ONLY** · `64` usage · `69` environment · `70` internal.
pub mod code {
    pub const OK: u8 = 0;
    /// gate refusal / invariant violation / doctor
    pub const VIOLATION: u8 = 1;
    /// landcheck Stop hook ONLY (see `cmd::landcheck::BlockToken`)
    pub const BLOCK: u8 = 2;
    /// clap's native 2 is REMAPPED here
    pub const USAGE: u8 = 64;
    /// no repo / no `.kanspec/` / lock held / git missing
    pub const ENVIRONMENT: u8 = 69;
    pub const INTERNAL: u8 = 70;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixes_are_non_empty_by_type() {
        let f = fixes![fix!("kanspec doctor")];
        assert_eq!(f.iter().count(), 1);
        let f = fixes![fix!("a {}", 1), fix!("b"), fix!("c")];
        assert_eq!(
            f.iter().map(Fix::as_str).collect::<Vec<_>>(),
            ["a 1", "b", "c"]
        );
    }

    #[test]
    fn every_shape_names_a_fix_and_an_exit_code() {
        let cases: Vec<KsError> = vec![
            KsError::IllegalTransition {
                id: TicketId::parse("t-9c41").unwrap(),
                from: State::Review,
                verb: Verb::Ship,
                allowed: crate::transitions::allowed_slice(Some(State::Review)),
                fix: fixes![fix!("kanspec show t-9c41")],
            },
            KsError::not_found("ticket", "t-0000", fixes![fix!("kanspec ls")]),
            KsError::gate(
                "merge_unknown",
                "cannot verify",
                fixes![fix!("kanspec scan")],
            ),
            KsError::environment(EnvCode::NotARepo, "not a repo", fixes![fix!("git init")]),
            KsError::Git {
                message: "git failed".into(),
                cmd: "git log".into(),
                exit: 128,
                fix: fixes![fix!("kanspec doctor")],
            },
            KsError::conflict("dupe", fixes![fix!("kanspec doctor")]),
            KsError::invalid("bad id", fixes![fix!("kanspec ls")]),
            KsError::internal(anyhow::anyhow!("boom")),
        ];
        for e in &cases {
            assert!(e.fixes().iter().next().is_some(), "{} has no fix", e.kind());
            assert!(matches!(e.exit_code(), 1 | 69 | 70));
            assert_eq!(e.to_json()["ok"], serde_json::json!(false));
        }
    }

    #[test]
    fn illegal_transition_renders_the_contract_example() {
        let e = KsError::IllegalTransition {
            id: TicketId::parse("t-9c41").unwrap(),
            from: State::Review,
            verb: Verb::Ship,
            allowed: crate::transitions::allowed_slice(Some(State::Review)),
            fix: fixes![fix!("kanspec show t-9c41")],
        };
        assert_eq!(e.to_string(), "t-9c41 is review, not doing — cannot ship");
        assert_eq!(
            e.render_human(false),
            "✗ t-9c41 is review, not doing — cannot ship\n  → kanspec show t-9c41\n"
        );
    }
}
