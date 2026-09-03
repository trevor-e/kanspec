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
//! command. And it is **validated**: the file is tool-owned but anything can write it, so
//! every row a reader trusts the shape of is checked once, in [`GitState::validated`],
//! and a row that breaks an invariant is dropped with a reason `doctor` surfaces — never
//! handed to a consumer that then has to guard against it (t-c0f5).
//!
//! Owner: **S3**.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{KsError, Result};
use crate::git::Method;
use crate::ids::{DecisionId, QuirkId, SpecName, TicketId};
use crate::paths::Layout;

/// Bumped whenever a DTO below changes shape. A file stamped with anything else is
/// discarded rather than migrated — it is a cache.
pub const GITSTATE_VERSION: u32 = 2;

/// What the ladder concluded. Two outcomes, because the ladder has two: it proves a merge
/// or it declines with a reason. No rung can prove ABSENCE — a `+` cherry line cannot tell
/// an unmerged branch from a multi-commit squash (D-3) — so there is no `NotMerged`: a
/// variant nothing emits would only give readers a case to mishandle (t-c060).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStatus {
    Merged,
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
    /// Glob rot beyond specs: an ACCEPTED decision's `scope:` globs that match zero
    /// tracked files, keyed by id. Only rotted entries are recorded — an absent id is a
    /// decision whose every glob is live (or one that is not standing).
    pub decision_dead_globs: BTreeMap<DecisionId, Vec<String>>,
    /// The same for an ACTIVE quirk's `paths:`.
    pub quirk_dead_globs: BTreeMap<QuirkId, Vec<String>>,
    /// Rows [`load`] dropped because they broke an invariant, each with why. Never
    /// serialised: the next `scan` rewrites the file whole, and `doctor` reports these
    /// until it does.
    #[serde(skip)]
    pub dropped: Vec<String>,
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
            decision_dead_globs: BTreeMap::new(),
            quirk_dead_globs: BTreeMap::new(),
            dropped: Vec::new(),
        }
    }
}

impl GitState {
    /// THE list of invariants the cache's readers rely on, enforced once at load. A row
    /// that breaks one is dropped and named in [`GitState::dropped`]; the rest of the file
    /// is kept, because a disposable cache must never fail a command and one bad row must
    /// not cost the badges every other row still answers.
    ///
    /// - A dead-glob row names at least one glob: `scan` records only rotted entries, so
    ///   an absent id already means "no rot" and an empty list is a row nothing should
    ///   index into (`doctor` once panicked on exactly that).
    pub fn validated(mut self) -> GitState {
        let mut dropped = Vec::new();
        self.decision_dead_globs.retain(|id, dead| {
            let ok = !dead.is_empty();
            if !ok {
                dropped.push(format!(
                    "decision_dead_globs[{id}]: empty list — an absent id already means no rot"
                ));
            }
            ok
        });
        self.quirk_dead_globs.retain(|id, dead| {
            let ok = !dead.is_empty();
            if !ok {
                dropped.push(format!(
                    "quirk_dead_globs[{id}]: empty list — an absent id already means no rot"
                ));
            }
            ok
        });
        self.dropped = dropped;
        self
    }

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
        Ok(s) if s.version == GITSTATE_VERSION => s.validated(),
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
    fn a_row_that_breaks_an_invariant_is_dropped_with_a_reason_and_the_rest_kept() {
        let s: GitState = serde_json::from_str(
            r#"{"version":2,"scanned_at":"2026-08-31T12:00:00Z","main":"origin/main",
                "quirk_dead_globs":{"q-11ba":[],"q-22cd":["src/gone/**"]},
                "decision_dead_globs":{"D-8c1a":[]}}"#,
        )
        .unwrap();
        let v = s.validated();
        assert_eq!(v.quirk_dead_globs.len(), 1, "the live row stays");
        assert!(v.decision_dead_globs.is_empty());
        assert_eq!(v.dropped.len(), 2, "{:?}", v.dropped);
        assert!(v.dropped[0].contains("D-8c1a") && v.dropped[1].contains("q-11ba"));
        assert!(!v.is_empty(), "dropping rows does not un-scan the cache");
        // `dropped` never travels through the file.
        let j = serde_json::to_string(&v).unwrap();
        assert!(!j.contains("dropped"), "{j}");
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
            r#"{"version":2,"tickets":{"not-a-ticket-id":{}}}"#,
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
    }
}
