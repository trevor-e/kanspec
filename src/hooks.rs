//! Git hook installation, resolved through `git rev-parse --git-path hooks` (D-7):
//! `core.hooksPath` (husky, lefthook) makes `.git/hooks` inert, so hardcoding it silently
//! installs nothing.
//!
//! kanspec installs itself as the **entrypoint** and moves any pre-existing hook to
//! `<hook>.d/10-<name>`. A naive append is unsafe two ways — `exit 0` starvation, and a
//! missing trailing newline welding two scripts into one line.
//!
//! Git has **no per-branch hooks** (D-5): `prepare-commit-msg` is one repo-wide hook that
//! dispatches on `git symbolic-ref --short HEAD` and skips `$2 ∈ {merge, squash, commit}`.
//! It is installed by `init`, not by `start`.
//!
//! Owner: **S7**. Writes only under `.git/`, never under `.kanspec/`.

// Wave-0 skeleton. The bodies below are `todo!("S7: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S7 when the bodies land.
#![allow(unused_variables, dead_code)]

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::ctx::Ctx;
use crate::error::Result;

/// Every git hook kanspec owns, with what it runs.
pub const HOOKS: &[(&str, &str)] = &[
    ("post-merge", "kanspec scan --quiet"),
    ("post-checkout", "kanspec scan --quiet"),
    ("prepare-commit-msg", "append the `Kanspec: <id>` trailer"),
];

/// The marker line that tells install from re-install, and ours from theirs.
pub const MARKER: &str = "# kanspec-managed hook — do not edit; see `kanspec init --refresh-hooks`";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookAction {
    Installed,
    Refreshed,
    /// a foreign hook was displaced to `<hook>.d/10-<name>`
    Displaced,
    Removed,
    /// the foreign hook was moved back
    Restored,
    Unchanged,
}

#[derive(Debug, Clone, Serialize)]
pub struct HookReport {
    pub hook: &'static str,
    pub path: PathBuf,
    pub action: HookAction,
    pub note: Option<String>,
}

/// `rev-parse --git-path hooks`, honouring `core.hooksPath`.
pub fn hooks_dir(ctx: &Ctx) -> Result<PathBuf> {
    todo!("S7: ctx.git.hooks_dir(), creating it if absent")
}

pub fn install(ctx: &Ctx) -> Result<Vec<HookReport>> {
    todo!("S7: for each HOOKS entry — displace any foreign hook to <hook>.d/10-<name>, then write the kanspec entrypoint (executable, trailing newline, MARKER)")
}

/// Symmetric uninstall: restores exactly the hook kanspec displaced.
pub fn remove(ctx: &Ctx) -> Result<Vec<HookReport>> {
    todo!("S7: remove the kanspec entrypoint and move <hook>.d/10-<name> back if it is the only entry")
}

/// The dispatcher body every installed hook shares: run every executable in `<hook>.d/` in
/// name order, then the kanspec action, propagating the first non-zero exit.
pub fn dispatcher_script(hook: &str, action: &str, invoked_as: &str) -> String {
    todo!("S7: the /bin/sh dispatcher with MARKER, `set -e`, the .d/ loop, and the kanspec call")
}

/// Whether this file is one kanspec wrote.
pub fn is_ours(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|t| t.contains(MARKER))
        .unwrap_or(false)
}
