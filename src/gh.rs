//! `gh` JSON, and **the one and only mock seam in the crate**.
//!
//! `$KANSPEC_GH_FIXTURES` points at recorded `gh` JSON. You cannot create a real GitHub PR
//! in a test, and rung 2 is the only rung that catches a title-only squash — the case that
//! defeats all four. **Git itself is never mocked**, and there is no `trait GhBackend`.
//!
//! Owner: **S2**.

// Wave-0 skeleton. The bodies below are `todo!("S2: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S2 when the bodies land.
#![allow(unused_variables, dead_code)]

use std::path::PathBuf;
// NOTE the deviation from §2.11's literal `authed: OnceCell<bool>`: `std::cell::OnceCell`
// is `!Sync`, which breaks `Arc<Ctx>: Send + Sync` — the assertion §2.6 makes a hard
// compile-time fact, and the property that lets the server's POST handlers call the very
// same `cmd::*` functions inside `spawn_blocking`. `OnceLock` is the `Sync` equivalent and
// memoizes identically.
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::git::Git;

/// The `[git] gh` policy. Aliased here so `Gh::detect(git, cfg: &GhCfg)` reads exactly as
/// the contract writes it while there is still only one definition of the enum.
pub use crate::config::GhMode as GhCfg;

pub struct Gh {
    slug: Option<String>,
    mode: GhCfg,
    fixtures: Option<PathBuf>,
    authed: OnceLock<bool>,
}

/// Never an error type: a `gh` failure is inconclusive, never "not merged" (invariant 2).
#[derive(Debug, Clone, Serialize)]
pub struct GhUnavailable(pub String);

impl std::fmt::Display for GhUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub state: PrState,
    pub merged_at: Option<DateTime<Utc>>,
    pub merge_commit: Option<String>,
    pub head_ref_oid: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

impl Gh {
    /// Cheap and total: reads the fixture env var and the config policy. Everything that
    /// can fail (auth, the repo slug) is deferred to [`Gh::available`], because `Ctx::open`
    /// runs before every command including `--help`-adjacent ones.
    pub(crate) fn detect(git: &Git, cfg: &GhCfg) -> Gh {
        let _ = git;
        Gh {
            slug: None,
            mode: *cfg,
            fixtures: std::env::var_os("KANSPEC_GH_FIXTURES").map(PathBuf::from),
            authed: OnceLock::new(),
        }
    }

    /// `gh auth status`, cached for the process. Always `false` under
    /// `[git] gh = "never"`, always `true` when fixtures are recorded.
    pub fn available(&self) -> bool {
        todo!("S2: honour GhCfg; fixtures => true; else memoize `gh auth status` in `authed`")
    }

    pub fn pr_view(&self, n: u64) -> std::result::Result<PrInfo, GhUnavailable> {
        todo!("S2: gh pr view <n> --json number,state,mergedAt,mergeCommit,headRefOid,url (or the fixture)")
    }

    /// REQUIRED, not optional: a squash-merged ticket with `pr: null` would otherwise skip
    /// the only rung that can see a title-only squash.
    pub fn pr_for_head(&self, branch: &str) -> std::result::Result<Vec<PrInfo>, GhUnavailable> {
        todo!("S2: gh pr list --head <branch> --state all --json … (or the fixture)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_json_from_gh_deserializes() {
        let p: PrInfo = serde_json::from_str(
            r#"{"number":142,"state":"MERGED","merged_at":"2026-08-30T16:02:00Z",
                "merge_commit":"a1b9c3d","head_ref_oid":"deadbee","url":"https://x/pull/142"}"#,
        )
        .unwrap();
        assert_eq!(p.state, PrState::Merged);
        assert_eq!(p.number, 142);
    }
}
