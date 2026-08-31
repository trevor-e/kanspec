//! `cache/gitstate.json` DTOs — the SOLE home of derived git facts, gitignored and
//! disposable.
//!
//! These are **badge-grade** values: plain `Deserialize` structs a hand-edited cache file
//! could forge. The **proof-grade** value is `scan::MergedProof`, which is sealed and
//! minted fresh at the gate. That split is the whole of J-8: `rm -rf .kanspec/cache`
//! changes nothing but freshness stamps (`tests/cache_wipe.rs`).
//!
//! [`load`] is **total**: any error — missing, truncated, written by a newer kanspec —
//! yields [`GitState::default`], because a disposable cache must never be able to fail a
//! command.
//!
//! Owner: **S3**.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{KsError, Result};
use crate::git::Method;
use crate::ids::{SpecName, TicketId};
use crate::paths::Layout;

/// Bumped whenever a DTO below changes shape. A file stamped with anything else is
/// discarded rather than migrated — it is a cache.
pub const GITSTATE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStatus {
    Merged,
    NotMerged,
    Unknown,
}

/// One ticket's last ladder result. `why` carries `Unknown::badge()` so the card can
/// explain itself without re-running anything.
///
/// ⚠ `why` is therefore the COMPLETE badge text — `"unknown (squash suspected, no gh)"`,
/// wrapper included — not the bare reason. `cmd/scan.rs` prints it raw, which is correct;
/// any consumer that supplies its own `unknown (…)` wrapper must strip this one first, or
/// the word appears twice. `derive::bare_reason` is that unwrap, and round C's
/// `kanspec ls` shipped the doubled form before it existed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MergeFact {
    pub status: MergeStatus,
    pub sha: Option<String>,
    pub method: Method,
    pub pr: Option<u64>,
    pub why: Option<String>,
    pub checked_at: DateTime<Utc>,
    /// The ticket's changed paths, recomputed into spec staleness at READ time —
    /// never an accumulated counter (D-10).
    #[serde(default)]
    pub changed: Vec<String>,
}

/// Branch facts for the Worktrees tab and the STALLED tripwire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchFact {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub last_commit_at: Option<DateTime<Utc>>,
    pub pushed: bool,
}

/// A spec's last-edit anchor, against which merges touching its `code:` globs are counted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpecAnchor {
    pub last_edit_sha: Option<String>,
    pub last_edit_at: Option<DateTime<Utc>>,
    pub merges_since: u32,
    /// globs matching zero files — glob rot, an amber dot on the feature strip
    #[serde(default)]
    pub dead_globs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitState {
    pub version: u32,
    pub scanned_at: Option<DateTime<Utc>>,
    /// the resolved main ref these facts were computed against
    pub main: String,
    pub fetch_age_secs: Option<u64>,
    pub tickets: BTreeMap<TicketId, MergeFact>,
    pub branches: BTreeMap<TicketId, BranchFact>,
    pub specs: BTreeMap<SpecName, SpecAnchor>,
}

impl Default for GitState {
    fn default() -> GitState {
        GitState {
            version: GITSTATE_VERSION,
            scanned_at: None,
            main: String::new(),
            fetch_age_secs: None,
            tickets: BTreeMap::new(),
            branches: BTreeMap::new(),
            specs: BTreeMap::new(),
        }
    }
}

impl MergeFact {
    /// How long ago this ticket's ladder last ran. `None` never happens for a fact that
    /// came out of `scan`; it exists so a clock skewed backwards reads as "unknown age"
    /// rather than as a negative duration.
    pub fn age(&self, now: DateTime<Utc>) -> Option<Duration> {
        (now - self.checked_at).to_std().ok()
    }
    /// Past the badge's freshness window. The gate NEVER consults this — `done` re-runs
    /// the ladder, because a 60s-old `merged` is not a proof (J-8).
    pub fn is_stale(&self, now: DateTime<Utc>, max_age: Duration) -> bool {
        match self.age(now) {
            Some(a) => a > max_age,
            None => false,
        }
    }
}

impl GitState {
    /// True when nothing has ever been scanned — the `NeverScanned` badge.
    pub fn is_empty(&self) -> bool {
        self.scanned_at.is_none()
    }

    /// Age of the CACHE ITSELF — how long ago `scan` last completed a pass. `None` means
    /// "never scanned", which is a different card from "scanned, and stale".
    pub fn age(&self, now: DateTime<Utc>) -> Option<Duration> {
        self.scanned_at.and_then(|t| (now - t).to_std().ok())
    }

    /// `true` when the whole cache is older than `max_age` **or** has never been written.
    /// Both render the same way to a human ("run `kanspec scan`"), and conflating them in
    /// a *badge* would be the wrong call — which is why [`GitState::is_empty`] stays
    /// separate.
    pub fn is_stale(&self, now: DateTime<Utc>, max_age: Duration) -> bool {
        match self.age(now) {
            None => true,
            Some(a) => a > max_age,
        }
    }

    /// `"checked 4m ago"` / `"never scanned"` — the freshness stamp every badge carries.
    pub fn freshness(&self, now: DateTime<Utc>) -> String {
        match self.scanned_at {
            None => "never scanned".to_string(),
            Some(t) => format!("checked {}", crate::out::rel_time(t, now)),
        }
    }
}

/// TOTAL: never returns an error, because a disposable cache must not be able to fail a
/// command. A version mismatch, a truncated write and a missing file are all "no facts
/// yet", which renders as `NeverScanned` rather than as a wrong answer.
pub fn load(layout: &Layout) -> GitState {
    let Ok(text) = std::fs::read_to_string(layout.gitstate()) else {
        return GitState::default();
    };
    match serde_json::from_str::<GitState>(&text) {
        // A file stamped with another version is DISCARDED rather than migrated. It is a
        // cache: re-deriving it costs one `scan`, and migrating it costs a forever-branch
        // in the only code that answers "did this land?".
        Ok(s) if s.version == GITSTATE_VERSION => s,
        _ => GitState::default(),
    }
}

/// The bytes `Op::WriteGitState` writes — inside the lock, capability-gated by `ScanToken`.
pub fn render(state: &GitState) -> Result<String> {
    let mut s = serde_json::to_string_pretty(state).map_err(KsError::internal)?;
    s.push('\n');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_gitstate_round_trips() {
        let s = GitState::default();
        let j = serde_json::to_string(&s).unwrap();
        let back: GitState = serde_json::from_str(&j).unwrap();
        assert_eq!(back, s);
        assert!(back.is_empty());
    }

    #[test]
    fn a_partial_gitstate_fills_in_defaults() {
        let s: GitState = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert_eq!(s.version, 1);
        assert!(s.tickets.is_empty());
        assert!(s.is_empty());
    }

    #[test]
    fn a_merge_fact_keyed_by_ticket_id_round_trips_as_a_json_map() {
        let mut s = GitState::default();
        s.tickets.insert(
            TicketId::parse("t-9c41").unwrap(),
            MergeFact {
                status: MergeStatus::Merged,
                sha: Some("a1b9c3d".into()),
                method: Method::GhPr,
                pr: Some(142),
                why: None,
                checked_at: Utc::now(),
                changed: vec!["src/auth/lockout.ts".into()],
            },
        );
        let j = serde_json::to_string(&s).unwrap();
        assert!(
            j.contains("\"t-9c41\""),
            "ids must serialize as plain map keys: {j}"
        );
        let back: GitState = serde_json::from_str(&j).unwrap();
        assert_eq!(back, s);
    }

    fn layout_in(dir: &std::path::Path) -> Layout {
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(dir)
            .status()
            .unwrap()
            .success());
        let repo = crate::paths::Repo::discover(dir, None).unwrap();
        Layout::open(&repo, &crate::config::Config::default())
    }

    #[test]
    fn load_is_total_over_every_way_the_cache_can_be_broken() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = layout_in(tmp.path());

        // Missing file.
        assert!(load(&layout).is_empty());

        std::fs::create_dir_all(layout.cache_dir()).unwrap();
        for body in [
            "",
            "{",
            "not json at all",
            r#"{"version":999,"scanned_at":"2026-08-31T00:00:00Z"}"#,
            r#"{"version":1,"tickets":{"not-a-ticket-id":{}}}"#,
        ] {
            std::fs::write(layout.gitstate(), body).unwrap();
            let s = load(&layout);
            assert!(
                s.is_empty() && s.version == GITSTATE_VERSION,
                "a cache that cannot be read must be NO facts, not wrong facts: {body:?}"
            );
        }
    }

    #[test]
    fn a_written_cache_reloads_as_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = layout_in(tmp.path());
        std::fs::create_dir_all(layout.cache_dir()).unwrap();

        let mut s = GitState {
            scanned_at: Some(Utc::now()),
            main: "origin/main".into(),
            ..GitState::default()
        };
        s.branches.insert(
            TicketId::parse("t-9c41").unwrap(),
            BranchFact {
                branch: Some("ks/t-9c41".into()),
                head: Some("a1b9c3d".into()),
                ahead: Some(2),
                behind: Some(0),
                last_commit_at: None,
                pushed: true,
            },
        );
        s.specs.insert(
            SpecName::parse("auth").unwrap(),
            SpecAnchor {
                last_edit_sha: Some("deadbee".into()),
                last_edit_at: None,
                merges_since: 3,
                dead_globs: vec!["src/gone/**".into()],
            },
        );
        let text = render(&s).unwrap();
        assert!(text.ends_with('\n'), "a JSON file ends with a newline");
        std::fs::write(layout.gitstate(), text).unwrap();
        assert_eq!(load(&layout), s);
    }

    #[test]
    fn the_cache_reports_its_own_staleness_separately_from_never_scanned() {
        let now = Utc::now();
        let window = Duration::from_secs(300);

        let never = GitState::default();
        assert!(never.is_empty());
        assert!(never.age(now).is_none());
        assert!(never.is_stale(now, window), "never scanned is never fresh");
        assert_eq!(never.freshness(now), "never scanned");

        let mut fresh = GitState {
            scanned_at: Some(now - chrono::Duration::seconds(11)),
            ..GitState::default()
        };
        assert!(!fresh.is_empty());
        assert!(!fresh.is_stale(now, window));
        assert_eq!(fresh.freshness(now), "checked 11s ago");

        fresh.scanned_at = Some(now - chrono::Duration::seconds(3600));
        assert!(fresh.is_stale(now, window));

        // A per-ticket fact carries its own clock, so one stale ticket does not condemn
        // the whole cache.
        let f = MergeFact {
            status: MergeStatus::Merged,
            sha: None,
            method: Method::Ancestry,
            pr: None,
            why: None,
            checked_at: now - chrono::Duration::seconds(30),
            changed: vec![],
        };
        assert!(!f.is_stale(now, window));
        assert!(f.is_stale(now, Duration::from_secs(10)));
        // A clock that ran backwards must not read as "stale by a negative amount".
        assert!(!f.is_stale(now - chrono::Duration::hours(1), window));
    }
}
