//! The derived-state projection. **PURE**: no `use std::fs`, no `use std::process`, no
//! `Utc::now()` — `now` is a `Snapshot` field. `tests/purity.rs` greps for all three and
//! fails the build on a hit.
//!
//! That purity is what makes the part of the product most likely to be wrong, and hardest
//! to reproduce, testable as table-driven unit tests over struct literals in microseconds.
//!
//! `derive` receives git facts through exactly one channel — `Snapshot.git`, the
//! gitignored cache — so "derived facts are never stored" reduces to "the projection's
//! only git input is a disposable file that cannot travel through git".
//!
//! Owner: **S4**.

// Wave-0 skeleton. The bodies below are `todo!("S4: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S4 when the bodies land.
#![allow(unused_variables, dead_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cache::MergeFact;
use crate::git::Method;
use crate::ids::{ProposalId, SpecName, TicketId};
use crate::model::{Proposal, Snapshot, Spec, Ticket};

// ── the dependency graph ─────────────────────────────────────────────────────

/// A dep is satisfied when it is TERMINAL (done or dropped) or IN-MAIN. Dropped counts as
/// satisfied: blocking forever on a dropped dep is worse, and `doctor::check_orphan_deps`
/// warns on a dep pointing at a dropped ticket (D-17).
pub fn dep_satisfied(s: &Snapshot, dep: &TicketId) -> bool {
    todo!("S4: terminal state OR an in-main MergeFact; a missing dep is NOT satisfied")
}

/// todo && all deps satisfied.
pub fn is_ready(s: &Snapshot, t: &Ticket) -> bool {
    todo!("S4: state == Todo && every dep satisfied")
}

pub fn ready_queue(s: &Snapshot) -> Vec<&Ticket> {
    todo!("S4: every is_ready ticket, oldest first")
}

pub fn blocked_by<'s>(s: &'s Snapshot, t: &Ticket) -> Vec<&'s TicketId> {
    todo!("S4: the unsatisfied subset of t.fm.deps")
}

pub fn dep_cycles(s: &Snapshot) -> Vec<Vec<TicketId>> {
    todo!("S4: DFS over deps, returning each cycle once — a doctor finding, not a crash")
}

// ── the git overlay (reads ONLY s.git — the gitignored cache) ─────────────────

pub fn merge_fact<'s>(s: &'s Snapshot, t: &Ticket) -> Option<&'s MergeFact> {
    todo!("S4: s.git.tickets.get(&t.fm.id)")
}

/// doing|review && Merged — the IN MAIN column, a pure overlay.
pub fn in_main<'s>(s: &'s Snapshot, t: &Ticket) -> Option<&'s MergeFact> {
    todo!("S4: merge_fact filtered to non-terminal tickets with MergeStatus::Merged")
}

pub fn badge(s: &Snapshot, t: &Ticket) -> Badge {
    todo!("S4: NeverScanned | Unpushed | Pushed | PrOpen | InMain | Unknown, in that precedence")
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "badge", rename_all = "snake_case")]
pub enum Badge {
    Unpushed,
    Pushed,
    PrOpen {
        n: u64,
    },
    InMain {
        method: Method,
        sha: String,
        checked_at: DateTime<Utc>,
    },
    Unknown {
        why: String,
        checked_at: Option<DateTime<Utc>>,
    },
    NeverScanned,
}

// ── the tripwires ────────────────────────────────────────────────────────────

/// doing, idle > `windows.stall_secs`.
pub fn stalled(s: &Snapshot, t: &Ticket) -> Option<Duration> {
    todo!("S4: Doing && now - max(last commit, last log entry) > cfg.windows.stall_secs")
}

pub fn dwell(s: &Snapshot, t: &Ticket) -> Option<Tripwire> {
    todo!("S4: review / in-main-not-closed / discovered-untriaged windows")
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "tripwire", rename_all = "snake_case")]
pub enum Tripwire {
    ReviewDwell(Duration),
    InMainNotClosed(Duration),
    SettlingDwell(Duration),
    DiscoveredUntriaged(Duration),
}

/// Every linked ticket terminal — "settling: close me".
pub fn settling(s: &Snapshot, p: &Proposal) -> bool {
    todo!("S4: every ticket whose proposal == p.fm.id is terminal, and there is at least one")
}

/// v0.2 — unresolved comment threads on a proposal.
pub fn unresolved(s: &Snapshot, p: &ProposalId) -> usize {
    todo!("V2/S4: comment ops deduped by (id, op, at); a thread is open until a `resolve` row")
}

pub fn double_claims(s: &Snapshot) -> Vec<(TicketId, Vec<String>)> {
    todo!("S4: tickets whose claimed_by disagrees with the branch's committer after a sync (R-8)")
}

/// Spec staleness, RECOMPUTED at read time from per-ticket cached `changed_paths` matched
/// against the spec's `code:` globs since its last-edit anchor. **Never an accumulated
/// counter**: a counter in a disposable cache silently resets to zero on `rm -rf cache/`
/// and UNDER-fires the tripwire — the dangerous direction (D-10).
pub fn staleness(s: &Snapshot, spec: &Spec) -> Staleness {
    todo!("S4: count merges touching spec.fm.code since the anchor; honour stale_ack; DeadGlobs from the cache")
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "staleness", rename_all = "snake_case")]
pub enum Staleness {
    Ok,
    Stale {
        merges: u32,
        since: DateTime<Utc>,
        examples: Vec<TicketId>,
    },
    DeadGlobs {
        globs: Vec<String>,
    },
    NeverScanned,
}

// ── the board / status aggregates ────────────────────────────────────────────

pub fn column(s: &Snapshot, t: &Ticket) -> Column {
    todo!("S4: Backlog | Ready | Doing | Review | InMain | Done | Dropped")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    Backlog,
    Ready,
    Doing,
    Review,
    InMain,
    Done,
    Dropped,
}

/// THE status list: every attention line, grouped, each already carrying its fix.
pub fn attention(s: &Snapshot) -> Vec<Attention> {
    todo!("S4: one line per owed verb — in-main-not-closed, settling, stalled, dwell, stale specs, double claims")
}

#[derive(Debug, Clone, Serialize)]
pub struct Attention {
    pub owner: Owner,
    pub glyph: char,
    pub subject: String,
    pub line: String,
    pub fix: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    You,
    Agent,
    Watching,
}

/// The one aggregate `status`, `board`, `up` and `prime` all read, so the terminal, the
/// browser and the agent cannot disagree — there is exactly one implementation of
/// "stalled".
pub fn compute(s: &Snapshot) -> Derived {
    todo!("S4: one pass building every field below")
}

#[derive(Debug, Clone, Serialize)]
pub struct Derived {
    pub ready: Vec<TicketId>,
    pub blocked: BTreeMap<TicketId, Vec<TicketId>>,
    pub in_main: BTreeMap<TicketId, Badge>,
    pub stalled: BTreeMap<TicketId, Duration>,
    pub settling: Vec<ProposalId>,
    pub dwell: BTreeMap<TicketId, Tripwire>,
    pub stale: BTreeMap<SpecName, Staleness>,
    pub attention: Vec<Attention>,
}
