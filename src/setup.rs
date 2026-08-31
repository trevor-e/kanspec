//! `setup claude|cursor|codex` — the agent snippet plus the agent-side hooks, installed
//! and **uninstalled symmetrically**.
//!
//! Permanent context cost is ~10 lines; the long-form docs live behind
//! `kanspec instructions` and version with the binary, so they never rot in CLAUDE.md.
//!
//! Owner: **S7**. Writes only outside `.kanspec/` (CLAUDE.md, agent settings), never
//! inside it.

// Wave-0 skeleton. The bodies below are `todo!("S7: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S7 when the bodies land.
#![allow(unused_variables, dead_code)]

use std::path::PathBuf;

use serde::Serialize;

use crate::cli::Agent;
use crate::ctx::Ctx;
use crate::error::Result;

/// The exact snippet DESIGN.md specifies. Delimited so `--remove` can excise precisely
/// what was added, leaving the rest of a hand-maintained CLAUDE.md untouched.
pub const SNIPPET_BEGIN: &str = "<!-- kanspec:begin -->";
pub const SNIPPET_END: &str = "<!-- kanspec:end -->";

/// The agent-side hooks, per DESIGN.md's table. `Stop` is installed but config-gated by
/// `[hooks] landcheck` (D-14).
pub const AGENT_HOOKS: &[(&str, &str, &str)] = &[
    (
        "SessionStart",
        "kanspec prime",
        "inject ~1.5k tokens of live state",
    ),
    ("PreCompact", "kanspec prime", "re-inject after compaction"),
    (
        "PostToolUse",
        "kanspec quirks --touch",
        "warn at the moment of touching a landmine",
    ),
    (
        "Stop",
        "kanspec landcheck",
        "block a session ending with inconsistent state (opt-in)",
    ),
];

#[derive(Debug, Clone, Serialize)]
pub struct SetupReport {
    pub agent: &'static str,
    pub files: Vec<SetupChange>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupChange {
    pub path: PathBuf,
    pub what: &'static str,
    pub changed: bool,
}

/// The markdown snippet installed into the agent's always-on context file.
pub fn snippet(invoked_as: &str) -> String {
    todo!("S7: the DESIGN.md `## kanspec` snippet, wrapped in SNIPPET_BEGIN/END")
}

pub fn install(ctx: &Ctx, agent: Agent) -> Result<SetupReport> {
    todo!("S7: write the snippet into the agent's context file idempotently, register AGENT_HOOKS, install the git hooks")
}

pub fn remove(ctx: &Ctx, agent: Agent) -> Result<SetupReport> {
    todo!("S7: excise the SNIPPET_BEGIN..SNIPPET_END block and the registered hooks, restoring anything displaced")
}

/// Where each agent keeps its always-on context and its hook registry.
pub fn agent_files(ctx: &Ctx, agent: Agent) -> (PathBuf, PathBuf) {
    todo!("S7: claude -> CLAUDE.md + .claude/settings.json; cursor -> .cursorrules; codex -> AGENTS.md")
}

pub fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Cursor => "cursor",
        Agent::Codex => "codex",
    }
}
