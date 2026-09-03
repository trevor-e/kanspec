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

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Serialize;

use crate::cache::{MergeFact, MergeStatus};
use crate::git::Method;
use crate::ids::{ItemRef, ProposalId, SpecName, TicketId};
use crate::logentry::LogEntry;
use crate::model::{
    CommentOpKind, DecisionStatus, Proposal, ProposalStatus, Snapshot, Spec, Ticket,
};
use crate::out::{glyph, rel_time, state_glyph};
use crate::transitions::{State, Verb};

// ── the dependency graph ─────────────────────────────────────────────────────

/// A dep is satisfied when it is TERMINAL (done or dropped) or IN-MAIN. Dropped counts as
/// satisfied: blocking forever on a dropped dep is worse, and `doctor::check_orphan_deps`
/// warns on a dep pointing at a dropped ticket (D-17).
pub fn dep_satisfied(s: &Snapshot, dep: &TicketId) -> bool {
    match s.tickets.get(dep) {
        // A dep pointing at nothing is NOT satisfied. Treating a missing ticket as "done"
        // would let a typo unblock work silently; `doctor::check_orphan_deps` raises it as
        // an Error, and until it is fixed the dependent ticket stays visibly blocked.
        None => false,
        Some(t) => t.fm.state.terminal() || in_main(s, t).is_some(),
    }
}

/// todo && all deps satisfied.
pub fn is_ready(s: &Snapshot, t: &Ticket) -> bool {
    t.fm.state == State::Todo && t.fm.deps.iter().all(|d| dep_satisfied(s, d))
}

pub fn ready_queue(s: &Snapshot) -> Vec<&Ticket> {
    let mut out: Vec<&Ticket> = s.tickets.values().filter(|t| is_ready(s, t)).collect();
    // Oldest first: the queue is FIFO, so nothing rots at the bottom of the backlog.
    out.sort_by(|a, b| {
        a.fm.created
            .cmp(&b.fm.created)
            .then_with(|| a.fm.id.cmp(&b.fm.id))
    });
    out
}

/// The unsatisfied subset of `t.fm.deps`, in the order the ticket lists them.
///
/// NOTE (deviation from the wave-0 stub, reported): `t` is tied to the snapshot's
/// lifetime. The stub's `t: &Ticket` cannot return a dep that is MISSING from the
/// snapshot — there is no `&'s TicketId` for a ticket that does not exist — and silently
/// dropping exactly the broken deps would render "blocked by nothing" for the one case a
/// human most needs named. Every call site takes its ticket out of the snapshot anyway.
pub fn blocked_by<'s>(s: &'s Snapshot, t: &'s Ticket) -> Vec<&'s TicketId> {
    t.fm.deps.iter().filter(|d| !dep_satisfied(s, d)).collect()
}

/// Every dependency cycle, each reported once, rotated so its smallest id leads. A cycle
/// is a `doctor` finding, not a crash: `is_ready` already answers `false` for every ticket
/// on one, so the graph being wrong degrades the queue rather than the process.
pub fn dep_cycles(s: &Snapshot) -> Vec<Vec<TicketId>> {
    const UNVISITED: u8 = 0;
    const ON_PATH: u8 = 1;
    const DONE: u8 = 2;

    fn walk<'s>(
        s: &'s Snapshot,
        node: &'s TicketId,
        mark: &mut BTreeMap<&'s TicketId, u8>,
        path: &mut Vec<&'s TicketId>,
        out: &mut BTreeSet<Vec<TicketId>>,
    ) {
        mark.insert(node, ON_PATH);
        path.push(node);
        if let Some(t) = s.tickets.get(node) {
            for dep in &t.fm.deps {
                // A dep pointing at nothing cannot be part of a cycle; it is
                // `check_orphan_deps`'s finding, not this one's.
                let Some((key, _)) = s.tickets.get_key_value(dep) else {
                    continue;
                };
                match mark.get(key).copied().unwrap_or(UNVISITED) {
                    UNVISITED => walk(s, key, mark, path, out),
                    ON_PATH => {
                        if let Some(at) = path.iter().position(|p| *p == key) {
                            out.insert(canonical_cycle(&path[at..]));
                        }
                    }
                    _ => {}
                }
            }
        }
        path.pop();
        mark.insert(node, DONE);
    }

    let mut mark: BTreeMap<&TicketId, u8> = BTreeMap::new();
    let mut out: BTreeSet<Vec<TicketId>> = BTreeSet::new();
    for root in s.tickets.keys() {
        if mark.contains_key(root) {
            continue;
        }
        let mut path: Vec<&TicketId> = Vec::new();
        walk(s, root, &mut mark, &mut path, &mut out);
    }
    out.into_iter().collect()
}

/// One cycle, rotated so the smallest id leads — the same loop found from three different
/// roots must compare equal, or `doctor` reports it three times.
fn canonical_cycle(cycle: &[&TicketId]) -> Vec<TicketId> {
    let at = cycle
        .iter()
        .enumerate()
        .min_by_key(|(_, id)| **id)
        .map(|(i, _)| i)
        .unwrap_or(0);
    cycle[at..]
        .iter()
        .chain(cycle[..at].iter())
        .map(|id| (*id).clone())
        .collect()
}

// ── the git overlay (reads ONLY s.git — the gitignored cache) ─────────────────

pub fn merge_fact<'s>(s: &'s Snapshot, t: &Ticket) -> Option<&'s MergeFact> {
    s.git.tickets.get(&t.fm.id)
}

/// doing|review && Merged — the IN MAIN column, a pure overlay.
///
/// Non-terminal rather than literally `doing|review`: a ticket parked back to `todo` after
/// its branch landed is in exactly the same "git says this shipped, nobody closed it"
/// situation, and it is the one the tripwire most needs to catch.
pub fn in_main<'s>(s: &'s Snapshot, t: &Ticket) -> Option<&'s MergeFact> {
    if t.fm.state.terminal() {
        return None;
    }
    merge_fact(s, t).filter(|f| f.status == MergeStatus::Merged)
}

/// THE SEAM between `cache::MergeFact.why` and `Badge::Unknown.why`, and the two disagree
/// on purpose-built wording.
///
/// `cache::MergeFact.why` stores `git::Unknown::badge()`, which is already the COMPLETE
/// card text — `"unknown (no branch or head SHA recorded)"` — because `cmd/scan.rs` prints
/// it raw. `Badge::text` supplies its own `unknown (…)` wrapper so it can append
/// `· checked <age>`. Handing the stored badge straight through therefore rendered
/// `unknown (unknown (no branch or head SHA recorded) · checked 7s ago)`.
///
/// Round-C fix (integration): unwrap once here, so exactly one layer owns the wrapper.
/// Total and idempotent — a reason that arrives bare is returned unchanged, so this stays
/// correct if the cache is ever changed to store the bare half.
fn bare_reason(why: &str) -> String {
    why.strip_prefix("unknown (")
        .and_then(|r| r.strip_suffix(')'))
        .unwrap_or(why)
        .to_string()
}

// ── attestation: the state a human vouched for (D-12) ────────────────────────

/// The standing attestation on a ticket: the LAST `repair` entry, when the state it
/// attested is still the ticket's state.
///
/// `repair` is the one verb whose logged state is authoritative (D-12), which makes it the
/// one route to a state that was *vouched for* rather than *replayed*. That distinction is
/// derived — it is read back out of the `## Log`, never written into frontmatter — so it
/// survives `rm -rf cache/`, travels through git, and cannot be set by a field.
///
/// A later ordinary verb clears it by construction: once `start` moves a repaired-to-`todo`
/// ticket on, the last repair entry no longer names the current state and the ticket is
/// back to standing on its own trail.
pub fn attested(t: &Ticket) -> Option<&LogEntry> {
    t.log
        .iter()
        .rev()
        .find(|e| e.verb == Verb::Repair)
        .filter(|e| e.state == t.fm.state)
}

/// Whether the ticket's own `## Log` carries a close-shaped line, read out of the record
/// that travels through git rather than out of the disposable cache (J-8: a cached
/// `merged` is a badge, never a gate).
///
/// Two verbs count, and the gate that writes each already demanded proof:
/// - a `done` entry — `plan_done` writes one only against a sealed `MergedProof` or a
///   recorded `--no-code` waiver, and neither can be minted without git or a signed reason;
/// - a `confirm` entry — `scan --confirm`, the recorded human override (D-11), which
///   `plan_confirm` refuses without both a commit SHA and a reason.
///
/// **This answers a weaker question than it looks like.** The `## Log` is plain text in a
/// file anyone can edit, so a `done` line proves that a `done` line was written — not that
/// a gate ever wrote it. The verb is the right input for the question `cmd::repair` asks
/// (is there already a close here, so that re-attesting one asserts nothing new?), and the
/// wrong input for "was this close granted?". [`close_evidence`] is that second question.
pub fn logged_close(t: &Ticket) -> bool {
    t.log
        .iter()
        .any(|e| matches!(e.verb, Verb::Done | Verb::Confirm))
}

/// The gate's own spelling of a recorded [`crate::scan::MergedProof`]. `plan_done` writes
/// `in main a1b9c3d via ancestry #142`; `plan_confirm` writes
/// `in main a1b9c3d — squash merged by hand`. Both open with the same two words and both
/// name a commit, because neither value can be minted without a SHA git resolved.
///
/// The prefix is MIRRORED from `scan::CONFIRM_NOTE` rather than imported — this file may
/// import nothing from `scan` (`tests/purity.rs`) — so the sharing runs the other way:
/// `scan::confirmed_proof` reads its own notes back through [`note_sha`] below, and
/// `the_note_grammar_mirrors_the_gates_own_spelling` keeps the prefix honest.
const PROOF_NOTE: &str = "in main";

/// The durable half of a `--no-code` close. `scan::NoCodeWaiver::record` appends
/// `  no-code waiver by <actor> at <ts>: <why>` as PROSE under `## Log` — deliberately not
/// shaped like a [`LogEntry`], so `replay` skips it — and refuses an empty reason.
/// Mirrored for the same reason as [`PROOF_NOTE`].
const NO_CODE_WAIVER: &str = "no-code waiver by ";

/// The commit a close's own log entry names, when the gate recorded one.
pub fn note_sha(note: &str) -> Option<&str> {
    let rest = note.trim().strip_prefix(PROOF_NOTE)?.trim_start();
    let tok = rest.split_whitespace().next()?;
    // git's own shape: 7 to 64 lowercase hex. A `via`/`—` that arrived where a SHA should
    // be is a note that names no commit, which is the answer, not a parse failure.
    (tok.len() >= 7
        && tok.len() <= 64
        && tok
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
    .then_some(tok)
}

/// What stands behind a close BESIDES the ticket's own claim of one.
///
/// [`logged_close`] can only answer "is there a close-shaped line here?". This asks the
/// question that actually gates trust: **what did a one-line append not also write?** A
/// hand-edited `state: done` plus one fabricated `done` line replays perfectly and carries
/// a close, and every existing check is silent on it — the trail is well-formed, and no
/// `repair` verb appears. What such a forgery cannot produce is corroboration.
///
/// Five answers, and every legitimate close has one:
///
/// - **`Attested`** — a human vouched for the state (D-12). `doctor::check_attested_state`
///   already reports and badges this one; it is listed first so nothing says it twice.
/// - **`InMain`** — a ladder run put this ticket in main. Read out of `s.git`, the
///   gitignored cache, which is computed from git and which no verb can write.
/// - **`Proof`** — the commit the gate recorded when it granted the close, or the one a
///   `scan --confirm` named. Git-tracked, so it survives `rm -rf cache/`.
/// - **`NoCode`** — the recorded `--no-code` waiver, signed and dated in the file.
/// - **`NothingToLand`** — the ticket names no branch, no head and no PR, so there is no
///   code for git to place and nothing to corroborate against. Demanding proof there would
///   be theatre, not detection, and this is the boundary DESIGN.md's invariant 10 now
///   states out loud.
///
/// `None` is the forged close: a terminal state reached by a line that only says so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseEvidence {
    Attested,
    InMain,
    Proof(String),
    NoCode,
    NothingToLand,
}

pub fn close_evidence(s: &Snapshot, t: &Ticket) -> Option<CloseEvidence> {
    if attested(t).is_some() {
        return Some(CloseEvidence::Attested);
    }
    if merge_fact(s, t).is_some_and(|f| f.status == MergeStatus::Merged) {
        return Some(CloseEvidence::InMain);
    }
    let sha = t.log.iter().rev().find_map(|e| match e.verb {
        Verb::Done | Verb::Confirm => e.note.as_deref().and_then(note_sha),
        _ => None,
    });
    if let Some(sha) = sha {
        // A note is plain text in the log the forger is already editing, so it corroborates
        // only until git contradicts it — and git never does: no rung can prove absence
        // (D-3), so there is no verdict that could. `Unknown` deliberately does NOT
        // disqualify — a genuinely old close whose branch was deleted and gc'd reads
        // unknown forever, and flagging it would punish the legitimate case to catch
        // nothing the next rung does not already catch.
        return Some(CloseEvidence::Proof(sha.to_string()));
    }
    if t.body
        .lines()
        .any(|l| l.trim_start().starts_with(NO_CODE_WAIVER))
    {
        // The gate refuses `--no-code` on a ticket that was shipped for review, so a trail
        // carrying a Ship verb alongside a waiver is self-contradictory: that pair is one
        // the gate would never have granted. Mirroring the gate's own rule here is what
        // makes the waiver cost more than one appended line to forge.
        let shipped = t.log.iter().any(|e| e.verb == Verb::Ship);
        if !shipped {
            return Some(CloseEvidence::NoCode);
        }
    }
    if !names_a_rev(t) {
        return Some(CloseEvidence::NothingToLand);
    }
    None
}

/// Where the SHA the ladder reasons about came from. Load-bearing for guard 0b — see
/// [`ladder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadOrigin {
    /// The `head:` frontmatter field, written by `ship` from real git output. Its presence
    /// is proof the branch carried commits of its own.
    Recorded,
    /// The branch tip, resolved live. Says nothing about whether the branch ever carried a
    /// commit.
    BranchTip,
}

/// `head:` if recorded, else the branch — DESIGN.md's "head-or-tip" — as a rev string for
/// the git calls that take one, tagged with where it came from (load-bearing for guard 0b).
/// The ONE definition, here because it is pure: `scan.rs` and `cmd/scan.rs` import it, and
/// [`names_a_rev`] widens it by `pr:` (t-d223 retired the private mirror that lived here).
pub fn ticket_rev(t: &Ticket) -> Option<(String, HeadOrigin)> {
    let named = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "null")
            .map(str::to_string)
    };
    named(&t.fm.head)
        .map(|h| (h, HeadOrigin::Recorded))
        .or_else(|| named(&t.fm.branch).map(|b| (b, HeadOrigin::BranchTip)))
}

/// Whether git has anything to be asked ABOUT this ticket: a branch, a head commit, or a
/// PR number — [`ticket_rev`] widened by `pr:`, because rung 2 answers from a PR number
/// alone.
fn names_a_rev(t: &Ticket) -> bool {
    ticket_rev(t).is_some() || t.fm.pr.is_some()
}

/// The badge every card carries — **never a guess**. Precedence is evidence-first: a
/// recorded ladder verdict outranks the branch's push state, and "we have no fact" is
/// `NeverScanned`, never `Unpushed`.
///
/// An attested CLOSE outranks all of it. A ticket that reached `done` or `dropped` because
/// a human vouched for it (D-12) must never wear a badge that reads like proof, and the
/// cache — which stops scanning a ticket the moment it goes terminal — is exactly where a
/// stale `in main` would otherwise come from. `unknown` naming the attestation is the
/// honest card: git proved nothing here, a person said so, and the card says which.
pub fn badge(s: &Snapshot, t: &Ticket) -> Badge {
    if t.fm.state.terminal() {
        if let Some(a) = attested(t) {
            return Badge::Unknown {
                why: format!(
                    "attested {} by {} on {}",
                    a.state,
                    a.actor,
                    a.at.format("%Y-%m-%d")
                ),
                checked_at: None,
            };
        }
    }
    let pushed = s.git.branches.get(&t.fm.id).map(|b| b.pushed);
    match merge_fact(s, t) {
        Some(f) => Badge::from_fact(f, t.fm.head.as_deref()),
        // No ladder verdict. The branch facts still say whether the work left the machine,
        // which is the difference between "nothing to detect yet" and "never looked". The
        // ticket's own `pr:` is NOT a badge: git has not been asked, and "PR #N open" would
        // be a claim about state nobody verified (`tests/cache_wipe.rs`).
        None => match pushed {
            Some(true) => Badge::Pushed,
            Some(false) => Badge::Unpushed,
            None => Badge::NeverScanned,
        },
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "badge", rename_all = "snake_case")]
pub enum Badge {
    Unpushed,
    Pushed,
    InMain {
        method: Method,
        pr: Option<u64>,
        sha: String,
        checked_at: DateTime<Utc>,
    },
    Unknown {
        why: String,
        checked_at: Option<DateTime<Utc>>,
    },
    NeverScanned,
}

impl Badge {
    /// A cached ladder result as a badge — the `scan` row's spelling and the card's are
    /// this one function (t-c060). `sha_fallback` is the ticket's own `head:` for a fact
    /// recorded without one.
    pub fn from_fact(f: &MergeFact, sha_fallback: Option<&str>) -> Badge {
        match f.status {
            MergeStatus::Merged => Badge::InMain {
                method: f.method,
                pr: f.pr,
                sha: f
                    .sha
                    .clone()
                    .or_else(|| sha_fallback.map(str::to_string))
                    .unwrap_or_default(),
                checked_at: f.checked_at,
            },
            MergeStatus::Unknown => Badge::Unknown {
                why: f
                    .why
                    .as_deref()
                    .map(bare_reason)
                    .unwrap_or_else(|| "no reason recorded".to_string()),
                checked_at: Some(f.checked_at),
            },
        }
    }

    /// `"in main (gh-pr #142 · checked 4m ago)"` — what the card, the scan row and the
    /// close-out transcript all show. The ONE spelling of merge state.
    pub fn text(&self, now: DateTime<Utc>) -> String {
        match self {
            Badge::Unpushed => "unpushed".to_string(),
            Badge::Pushed => "pushed".to_string(),
            Badge::InMain {
                method,
                pr,
                checked_at,
                ..
            } => format!(
                "in main ({method}{} · checked {})",
                pr.map(|n| format!(" #{n}")).unwrap_or_default(),
                rel_time(*checked_at, now)
            ),
            Badge::Unknown { why, checked_at } => match checked_at {
                Some(at) => format!("unknown ({why} · checked {})", rel_time(*at, now)),
                None => format!("unknown ({why})"),
            },
            Badge::NeverScanned => "never scanned".to_string(),
        }
    }
}

// ── the tripwires ────────────────────────────────────────────────────────────

/// doing, idle > `windows.stall_secs`. The idle clock is the LATER of the ticket's last
/// log entry and its branch's last commit, so an agent that is committing without running
/// a verb is not STALLED, and neither is one that is running verbs without committing.
pub fn stalled(s: &Snapshot, t: &Ticket) -> Option<Duration> {
    if t.fm.state != State::Doing {
        return None;
    }
    let idle = idle(s, t)?;
    (idle > secs(s.cfg.windows.stall_secs)).then_some(idle)
}

pub fn dwell(s: &Snapshot, t: &Ticket) -> Option<Tripwire> {
    let w = s.cfg.windows;
    let idle = idle(s, t);
    let over =
        |armed: bool, d: Option<Duration>, window: u64| d.filter(|d| armed && *d > secs(window));

    // In-main-but-not-closed outranks everything: it is the one tripwire whose fix is a
    // single verb (`done`), and a review-dwelling ticket that has ALSO landed is the same
    // ticket.
    if let Some(d) = over(in_main(s, t).is_some(), idle, w.in_main_dwell_secs) {
        return Some(Tripwire::InMainNotClosed(d));
    }
    if let Some(d) = over(t.fm.state == State::Review, idle, w.review_dwell_secs) {
        return Some(Tripwire::ReviewDwell(d));
    }
    if let Some(d) = over(
        untriaged_discovery(t),
        since(s, t.fm.created),
        w.discovered_dwell_secs,
    ) {
        return Some(Tripwire::DiscoveredUntriaged(d));
    }
    None
}

/// Tangential work captured mid-ticket that nobody has given a spec, a dep or a drop —
/// DESIGN.md's "they can't rot silently either".
fn untriaged_discovery(t: &Ticket) -> bool {
    t.fm.state == State::Todo
        && t.fm.discovered_in.is_some()
        && t.fm.spec.is_none()
        && t.fm.deps.is_empty()
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "tripwire", rename_all = "snake_case")]
pub enum Tripwire {
    ReviewDwell(Duration),
    InMainNotClosed(Duration),
    SettlingDwell(Duration),
    DiscoveredUntriaged(Duration),
}

impl Tripwire {
    pub fn elapsed(&self) -> Duration {
        match self {
            Tripwire::ReviewDwell(d)
            | Tripwire::InMainNotClosed(d)
            | Tripwire::SettlingDwell(d)
            | Tripwire::DiscoveredUntriaged(d) => *d,
        }
    }
}

/// Every linked ticket terminal — "settling: close me".
pub fn settling(s: &Snapshot, p: &Proposal) -> bool {
    if matches!(
        p.fm.status,
        ProposalStatus::Closed | ProposalStatus::Abandoned
    ) {
        return false;
    }
    let mut any = false;
    for t in s.tickets.values() {
        if t.fm.proposal.as_ref() != Some(&p.fm.id) {
            continue;
        }
        if !t.fm.state.terminal() {
            return false;
        }
        any = true;
    }
    // A proposal with no tickets at all is not "settling" — it never started.
    any
}

/// Threads of one kind on a proposal that no `resolve` row has closed. Rows are already
/// deduped on `(id, op, at)` by the store (D-19); a thread is open until a `resolve` row
/// carries its id.
fn open_threads(s: &Snapshot, p: &ProposalId, kind: CommentOpKind) -> usize {
    let Some(ops) = s.comments.get(p) else {
        return 0;
    };
    let ids = |k: CommentOpKind| {
        ops.iter()
            .filter(|o| o.op == k)
            .map(|o| &o.id)
            .collect::<BTreeSet<_>>()
    };
    ids(kind).difference(&ids(CommentOpKind::Resolve)).count()
}

/// v0.2 — unresolved comment threads on a proposal: the human's owed verb.
pub fn unresolved(s: &Snapshot, p: &ProposalId) -> usize {
    open_threads(s, p, CommentOpKind::Comment)
}

/// v0.2 — threads someone has already answered but nobody resolved: the AGENT's owed verb.
pub fn answered(s: &Snapshot, p: &ProposalId) -> usize {
    open_threads(s, p, CommentOpKind::Reply)
}

/// Two actors that both hold the same claim (R-8). Cross-machine `start` is eventually
/// consistent, and `.gitattributes` union-merges the ticket body, so the evidence is
/// durable and git-tracked: two `start` lines in one `## Log` with no release between
/// them. Reading it out of the LOG rather than out of the cache is what makes the flag
/// survive `rm -rf cache/` — and what makes it detectable at all, since `claimed_by` can
/// only ever hold one of the two names.
pub fn double_claims(s: &Snapshot) -> Vec<(TicketId, Vec<String>)> {
    let mut out = Vec::new();
    for t in s.tickets.values() {
        let mut holders: Vec<String> = Vec::new();
        for e in &t.log {
            match e.verb {
                Verb::Start => {
                    if !holders.contains(&e.actor) {
                        holders.push(e.actor.clone());
                    }
                }
                // Every verb that ends a claim. `Confirm` and `Repair` are non-transitions
                // and `New` precedes every claim, so none of them releases one.
                Verb::Ship | Verb::Park | Verb::Drop | Verb::Done => holders.clear(),
                Verb::New | Verb::Confirm | Verb::Repair => {}
            }
        }
        if let Some(by) = &t.fm.claimed_by {
            if !holders.is_empty() && !holders.contains(by) {
                holders.push(by.clone());
            }
        }
        if holders.len() > 1 {
            out.push((t.fm.id.clone(), holders));
        }
    }
    out
}

/// Spec staleness, RECOMPUTED at read time from the git observations `scan` recorded:
/// `SpecAnchor::merges_since` — every merge on main touching the spec's `code:` globs
/// since its last-edit anchor — widened by the per-ticket cached `changed_paths`, which
/// name examples. **Never an accumulated counter**: both numbers are fresh answers to git
/// questions, so `rm -rf cache/` erases the answer (→ `NeverScanned`) rather than resetting
/// a tally to a confident zero, which would UNDER-fire the tripwire — the dangerous
/// direction (D-10).
///
/// The recorded count is the half that makes this fire on work kanspec never tracked: a
/// teammate's PR, a hotfix pushed straight to main, a dependabot bump, everything that
/// predates adoption. None of those has a ticket, so none appears in `GitState.tickets` —
/// and counting only kanspec's own merged tickets left the wire silent on exactly the
/// repos whose specs drift furthest. The two counts OVERLAP (a kanspec ticket's merge is
/// one of the merges git counted), so they combine with `max`, never a sum.
///
/// The two anchors are both durable in their own way. `last_edit_at` is a git fact
/// recomputed by every `scan`; `stale_ack` lives in the spec's own frontmatter, so the
/// human's "no behaviour change" attestation survives a cache wipe and is visibly signed.
/// Whichever is LATER is the point merges are counted from, which is exactly why
/// `features --confirm` resets the tripwire without anyone incrementing anything.
pub fn staleness(s: &Snapshot, spec: &Spec) -> Staleness {
    let Some(anchor) = s.git.specs.get(&spec.name) else {
        // No anchor is "we never looked", NOT "zero merges". The difference is the whole
        // of D-10: the wiped-cache answer must be visibly absent, never quietly clean.
        return Staleness::NeverScanned;
    };

    let dead = &anchor.dead_globs;
    if !spec.fm.code.is_empty() && dead.len() >= spec.fm.code.len() {
        // Every glob rotted: the count below would be a confident zero about code the spec
        // no longer points at, which is worse than saying so.
        return Staleness::DeadGlobs {
            globs: dead.clone(),
        };
    }

    let last_edit = anchor.last_edit_at;
    let ack = spec.fm.stale_ack.as_ref().map(|a| a.at);
    let since = [last_edit, ack].into_iter().flatten().max();

    if let Some(since) = since {
        // `scan` measured `merges_since` from `last_edit_at` and from nowhere else, so it
        // only answers the question being asked while the last edit IS the anchor. An
        // attestation that postdates it SUPERSEDES the count rather than being subtracted
        // from it — otherwise `--confirm` could not clear a spec until the ack commit
        // itself reached main, and a decrement is the counter D-10 forbids.
        let recorded = if spec.fm.code.is_empty() || ack > last_edit {
            0
        } else {
            anchor.merges_since
        };
        // kanspec's own merged tickets — already inside `recorded`, and here to NAME the
        // drift, and to answer at all when git could not (`merges_touching` declining
        // records a 0, and a 0 must never read as "nothing happened").
        let mut examples: Vec<TicketId> = Vec::new();
        if let Some(set) = globs_of(spec) {
            for (id, fact) in &s.git.tickets {
                if fact.status != MergeStatus::Merged {
                    continue;
                }
                if !fact.changed.iter().any(|p| set.is_match(p)) {
                    continue;
                }
                if landed_since(s, id, since) {
                    examples.push(id.clone());
                }
            }
        }
        let merges = recorded.max(examples.len() as u32);
        if merges >= s.cfg.windows.stale_merges.max(1) {
            examples.truncate(5);
            return Staleness::Stale {
                merges,
                since,
                examples,
            };
        }
    }

    if !dead.is_empty() {
        return Staleness::DeadGlobs {
            globs: dead.clone(),
        };
    }
    if since.is_none() {
        // D-10 at its quietest end: an anchor with no point to count FROM has not been
        // measured. "I have never looked" must never render as "I looked and it is fine".
        return Staleness::NeverScanned;
    }
    Staleness::Ok
}

/// Did this ticket's work conclude after the spec's anchor? The cache records WHEN THE
/// LADDER RAN, not when the merge happened, so the ticket's own git-tracked `## Log` is
/// the better clock — and the one that does not move every time `scan` runs.
fn landed_since(s: &Snapshot, id: &TicketId, since: DateTime<Utc>) -> bool {
    match s.tickets.get(id) {
        // A fact with no ticket behind it cannot be dated. Counting it over-fires the
        // tripwire, which is the safe direction (D-10).
        None => true,
        Some(t) => t.log.iter().map(|e| e.at).max().unwrap_or(t.fm.created) >= since,
    }
}

/// The spec's `code:` globs, compiled. `literal_separator` matches git's `:(glob)`
/// semantics — `src/*.ts` is one directory deep, `src/**` is all of them — so the tripwire
/// counts what the pathspec in `scan` counted.
fn globs_of(spec: &Spec) -> Option<GlobSet> {
    let mut b = GlobSetBuilder::new();
    let mut any = false;
    for g in &spec.fm.code {
        if let Ok(glob) = GlobBuilder::new(g).literal_separator(true).build() {
            b.add(glob);
            any = true;
        }
    }
    any.then(|| b.build().ok()).flatten()
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
    match t.fm.state {
        State::Dropped => Column::Dropped,
        State::Done => Column::Done,
        _ if in_main(s, t).is_some() => Column::InMain,
        State::Review => Column::Review,
        State::Doing => Column::Doing,
        State::Todo if is_ready(s, t) => Column::Ready,
        State::Todo => Column::Backlog,
    }
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
///
/// One line per owed verb, and every line names the ONE command that discharges it —
/// "stuck" means appearing here, which is the opposite of silent.
pub fn attention(s: &Snapshot) -> Vec<Attention> {
    // Each line carries a RANK — the order DESIGN.md's transcript prints its categories
    // in — so `status`, the board's pinned strip and `prime` cannot disagree about which
    // owed verb comes first. Within a rank, by subject, so the list is stable.
    let mut you: Vec<(u8, Attention)> = Vec::new();
    let mut agent: Vec<(u8, Attention)> = Vec::new();
    let mut watching: Vec<(u8, Attention)> = Vec::new();

    // ── YOU: work only a human can discharge ─────────────────────────────────
    for t in s.tickets.values() {
        let Some(fact) = in_main(s, t) else { continue };
        let id = &t.fm.id;
        let pr = fact
            .pr
            .or(t.fm.pr)
            .map(|n| format!(" #{n}"))
            .unwrap_or_default();
        let landed = idle(s, t)
            .map(|d| format!(" {}", short(d)))
            .unwrap_or_default();
        you.push((
            YOU_IN_MAIN,
            att(
                Owner::You,
                glyph::IN_MAIN,
                id,
                format!(
                    "in main{landed} ({}{pr} · checked {}), not closed",
                    fact.method,
                    rel_time(fact.checked_at, s.now)
                ),
                format!("kanspec done {id}"),
            ),
        ));
    }

    for (id, holders) in double_claims(s) {
        you.push((
            YOU_DOUBLE_CLAIM,
            att(
                Owner::You,
                glyph::FAIL,
                &id,
                format!(
                    "double claim after sync: {} both hold it",
                    holders.join(" and ")
                ),
                format!("kanspec show {id}"),
            ),
        ));
    }

    for p in s.proposals.values() {
        let id = &p.fm.id;
        if settling(s, p) {
            let open = p
                .items
                .iter()
                .filter(|i| !dispositioned(&p.fm.ledger, &i.id))
                .count();
            let dwelt = settling_dwell(s, p)
                .map(|d| format!(" {}", short(d.elapsed())))
                .unwrap_or_default();
            let tail = if open == 0 {
                String::new()
            } else {
                format!(", {open} items undispositioned")
            };
            you.push((
                YOU_SETTLING,
                att(
                    Owner::You,
                    state_glyph(State::Done),
                    id,
                    format!("settling{dwelt}: last ticket landed{tail}"),
                    format!("kanspec close {id}"),
                ),
            ));
        }
        let open = unresolved(s, id);
        if open > 0 {
            // The review page IS the next command here: threads are answered in the
            // browser, not on the command line.
            let url = review_url(s, id);
            you.push((
                YOU_THREADS,
                Attention {
                    url: Some(url.clone()),
                    ..att(
                        Owner::You,
                        state_glyph(State::Review),
                        id,
                        format!("{open} unresolved review threads await you"),
                        url,
                    )
                },
            ));
        } else if p.fm.status == ProposalStatus::Review {
            // A proposal in review is owed a human decision from the moment `review` ran —
            // `approve`, or a comment — not from the seven-day dwell. Open threads above ARE
            // that comment; with none, the approve is what is owed (t-660d).
            let url = review_url(s, id);
            you.push((
                YOU_IN_REVIEW,
                Attention {
                    url: Some(url.clone()),
                    ..att(
                        Owner::You,
                        state_glyph(State::Review),
                        id,
                        format!("in review: {} — approve it, or comment", p.fm.title),
                        format!("kanspec approve {id}"),
                    )
                },
            ));
        }
    }

    for d in s.decisions.values() {
        if d.fm.status != DecisionStatus::Proposed {
            continue;
        }
        let id = &d.fm.id;
        you.push((
            YOU_PROPOSED_DECISION,
            att(
                Owner::You,
                state_glyph(State::Review),
                id,
                format!("proposed decision awaits a human: {}", d.fm.title),
                format!("kanspec accept {id}"),
            ),
        ));
    }

    // ── AGENT: work the next session should pick up ──────────────────────────
    let queue = ready_queue(s);
    let shown = queue.len().min(READY_SHOWN);
    for t in queue.iter().take(shown) {
        let id = &t.fm.id;
        let spec =
            t.fm.spec
                .as_ref()
                .map(|name| format!(" · {name}"))
                .unwrap_or_default();
        // A dep that landed but is not closed is why this became claimable — say so, so
        // the ready line and the YOU line above visibly belong to the same story.
        let pending = t.fm.deps.iter().find(|id| {
            s.tickets
                .get(*id)
                .is_some_and(|dep| in_main(s, dep).is_some())
        });
        let tail = pending
            .map(|d| format!(" · unblocked when {d} closed"))
            .unwrap_or_default();
        agent.push((
            AGENT_READY,
            att(
                Owner::Agent,
                state_glyph(State::Todo),
                id,
                format!("ready{spec}{tail}"),
                format!("kanspec start {id}"),
            ),
        ));
    }
    if queue.len() > shown {
        agent.push((
            AGENT_MORE,
            att(
                Owner::Agent,
                state_glyph(State::Todo),
                "",
                format!("+{} more ready", queue.len() - shown),
                "kanspec ready".to_string(),
            ),
        ));
    }

    for p in s.proposals.values() {
        let id = &p.fm.id;
        let n = answered(s, id);
        if n > 0 {
            agent.push((
                AGENT_THREADS,
                Attention {
                    url: Some(review_url(s, id)),
                    ..att(
                        Owner::Agent,
                        state_glyph(State::Review),
                        id,
                        format!("{n} answered threads await your resolve"),
                        format!("kanspec comments {id} --unresolved"),
                    )
                },
            ));
        }
    }

    // ── WATCHING: nothing is owed yet, but the clock is running ──────────────
    for t in s.tickets.values() {
        let id = &t.fm.id;
        if let Some(idle) = stalled(s, t) {
            watching.push((
                WATCH_STALLED,
                att(
                    Owner::Watching,
                    state_glyph(State::Doing),
                    id,
                    format!("STALLED: doing, no commits or updates for {}", short(idle)),
                    format!("kanspec park {id} --why \"...\""),
                ),
            ));
        }
        match dwell(s, t) {
            // In-main-not-closed already has its own YOU line above, with a longer badge.
            Some(Tripwire::InMainNotClosed(_)) | None => {}
            Some(Tripwire::ReviewDwell(d)) => watching.push((
                WATCH_REVIEW,
                att(
                    Owner::Watching,
                    state_glyph(State::Review),
                    id,
                    format!("in review {}, no movement", short(d)),
                    format!("kanspec show {id}"),
                ),
            )),
            Some(Tripwire::DiscoveredUntriaged(d)) => watching.push((
                WATCH_DISCOVERED,
                att(
                    Owner::Watching,
                    glyph::DISCOVERED,
                    id,
                    format!(
                        "discovered {} ago, still untriaged — give it a spec, a dep, or a drop",
                        short(d)
                    ),
                    format!("kanspec show {id}"),
                ),
            )),
            // Proposals, not tickets, settle — `dwell` never returns this for a ticket.
            Some(Tripwire::SettlingDwell(_)) => {}
        }
    }

    for spec in s.specs.values() {
        match staleness(s, spec) {
            Staleness::Stale { merges, .. } => watching.push((
                WATCH_STALE_SPEC,
                att(
                    Owner::Watching,
                    '⚠',
                    &spec.name,
                    format!(
                        "{merges} merges touched {} since spec last edited",
                        spec.fm.code.join(", ")
                    ),
                    "kanspec features --stale".to_string(),
                ),
            )),
            Staleness::DeadGlobs { globs } => watching.push((
                WATCH_DEAD_GLOBS,
                att(
                    Owner::Watching,
                    '⚠',
                    &spec.name,
                    format!("spec globs match no files: {}", globs.join(", ")),
                    format!("kanspec spec show {}", spec.name),
                ),
            )),
            Staleness::Ok | Staleness::NeverScanned => {}
        }
    }

    let mut out = Vec::with_capacity(you.len() + agent.len() + watching.len());
    for group in [&mut you, &mut agent, &mut watching] {
        group.sort_by(|(ra, a), (rb, b)| {
            ra.cmp(rb)
                .then_with(|| a.subject.cmp(&b.subject))
                .then_with(|| a.line.cmp(&b.line))
        });
        out.extend(group.drain(..).map(|(_, a)| a));
    }
    out
}

/// One attention line without a `url` — the shape every entry but the two review-thread
/// ones has, so each push above reads as (rank, owner, glyph, subject, line, fix).
fn att(owner: Owner, glyph: char, subject: impl ToString, line: String, fix: String) -> Attention {
    Attention {
        owner,
        glyph,
        subject: subject.to_string(),
        line,
        fix,
        url: None,
    }
}

/// How many ready tickets `status` names before it collapses the rest into one line.
const READY_SHOWN: usize = 5;

/// The order DESIGN.md's transcript prints each group's categories in. Ranks are local to
/// [`attention`]; they never reach the JSON, because what an agent branches on is `owner`
/// and `fix`, not a number that would then have to stay stable forever.
const YOU_THREADS: u8 = 0;
const YOU_IN_REVIEW: u8 = 1;
const YOU_IN_MAIN: u8 = 2;
const YOU_SETTLING: u8 = 3;
const YOU_DOUBLE_CLAIM: u8 = 4;
const YOU_PROPOSED_DECISION: u8 = 5;
const AGENT_READY: u8 = 0;
const AGENT_MORE: u8 = 1;
const AGENT_THREADS: u8 = 2;
const WATCH_STALLED: u8 = 0;
const WATCH_REVIEW: u8 = 1;
const WATCH_DISCOVERED: u8 = 2;
const WATCH_STALE_SPEC: u8 = 3;
const WATCH_DEAD_GLOBS: u8 = 4;

/// A settling proposal nobody closed. Anchored on the last thing that HAPPENED to its
/// tickets, so a `scan` cannot reset it.
fn settling_dwell(s: &Snapshot, p: &Proposal) -> Option<Tripwire> {
    if !settling(s, p) {
        return None;
    }
    let last = s
        .tickets
        .values()
        .filter(|t| t.fm.proposal.as_ref() == Some(&p.fm.id))
        .filter_map(|t| t.log.iter().map(|e| e.at).max())
        .max()?;
    since(s, last)
        .filter(|d| *d > secs(s.cfg.windows.settling_dwell_secs))
        .map(Tripwire::SettlingDwell)
}

fn review_url(s: &Snapshot, p: &ProposalId) -> String {
    format!("http://127.0.0.1:{}/p/{p}", s.cfg.port)
}

/// Does a proposal's disposition ledger account for this item? One definition, so
/// `status`'s settling line and `doctor::check_ledger_complete` cannot disagree about what
/// "undispositioned" means. Both spellings count: the full anchor `p-7de2#c3` and the
/// visible short form `c3` the proposal body itself uses.
pub fn dispositioned(ledger: &[String], item: &ItemRef) -> bool {
    let full = item.to_string();
    let short = format!("{}{}", item.kind.letter(), item.n);
    ledger.iter().any(|l| {
        l.contains(&full)
            || l.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|tok| tok == short)
    })
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
    // `ready` is the QUEUE — oldest first — not the id map's order, so `status`, `ready`
    // and the board offer the same next ticket.
    let ready: Vec<TicketId> = ready_queue(s).iter().map(|t| t.fm.id.clone()).collect();
    let mut blocked = BTreeMap::new();
    let mut in_main_map = BTreeMap::new();
    let mut stalled_map = BTreeMap::new();
    let mut dwell_map = BTreeMap::new();

    for t in s.tickets.values() {
        let id = &t.fm.id;
        if !t.fm.state.terminal() && !is_ready(s, t) {
            let b = blocked_by(s, t);
            if !b.is_empty() {
                blocked.insert(id.clone(), b.into_iter().cloned().collect());
            }
        }
        if in_main(s, t).is_some() {
            in_main_map.insert(id.clone(), badge(s, t));
        }
        if let Some(d) = stalled(s, t) {
            stalled_map.insert(id.clone(), d);
        }
        if let Some(w) = dwell(s, t) {
            dwell_map.insert(id.clone(), w);
        }
    }
    Derived {
        ready,
        blocked,
        in_main: in_main_map,
        stalled: stalled_map,
        settling: s
            .proposals
            .values()
            .filter(|p| settling(s, p))
            .map(|p| p.fm.id.clone())
            .collect(),
        dwell: dwell_map,
        stale: s
            .specs
            .values()
            .map(|sp| (sp.name.clone(), staleness(s, sp)))
            .collect(),
        attention: attention(s),
    }
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

// ── the clock, read from the snapshot and nowhere else ────────────────────────

const fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// `None` when the clock ran backwards — a negative age is "unknown", never a tripwire.
fn since(s: &Snapshot, then: DateTime<Utc>) -> Option<Duration> {
    (s.now - then).to_std().ok()
}

/// How long since ANYTHING happened to this ticket: the later of its last log entry and
/// its branch's last commit.
fn idle(s: &Snapshot, t: &Ticket) -> Option<Duration> {
    let log = t.log.iter().map(|e| e.at).max();
    let commit = s.git.branches.get(&t.fm.id).and_then(|b| b.last_commit_at);
    let last = [log, commit]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(t.fm.created);
    since(s, last)
}

/// `"3h"` / `"2d"` — a bare elapsed time, for the lines that already say "for".
pub fn short(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        s if s < 60 => format!("{s}s"),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3_600),
        s => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use chrono::TimeZone;

    use crate::cache::{BranchFact, SpecAnchor};
    use crate::config::Config;
    use crate::logentry::LogEntry;
    use crate::model::{SpecFm, StaleAck, TicketFm};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap()
    }

    fn ago(h: i64) -> DateTime<Utc> {
        now() - chrono::Duration::hours(h)
    }

    fn snap() -> Snapshot {
        Snapshot::empty(Config::default(), now())
    }

    fn tid(s: &str) -> TicketId {
        TicketId::parse(s).unwrap()
    }

    /// A ticket literal — the whole point of a pure projection is that this is all the
    /// setup a test of it needs.
    fn ticket(id: &str, state: State) -> Ticket {
        Ticket {
            fm: TicketFm {
                id: tid(id),
                title: format!("ticket {id}"),
                state,
                spec: None,
                proposal: None,
                item: None,
                deps: Vec::new(),
                followup_of: None,
                discovered_in: None,
                branch: None,
                worktree: None,
                claimed_by: None,
                pr: None,
                head: None,
                spec_unchanged: None,
                created: ago(48),
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(format!(".kanspec/tickets/{id}.md")),
            body: String::new(),
            steps: Vec::new(),
            log: Vec::new(),
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    fn put(s: &mut Snapshot, t: Ticket) {
        s.tickets.insert(t.fm.id.clone(), t);
    }

    fn entry(at: DateTime<Utc>, actor: &str, verb: Verb, state: State) -> LogEntry {
        LogEntry {
            at,
            state,
            actor: actor.to_string(),
            verb,
            note: None,
        }
    }

    fn merged(changed: &[&str]) -> MergeFact {
        MergeFact {
            status: MergeStatus::Merged,
            sha: Some("a1b9c3d".into()),
            method: Method::GhPr,
            pr: Some(142),
            why: None,
            checked_at: ago(1),
            changed: changed.iter().map(|c| c.to_string()).collect(),
        }
    }

    /// The ladder looked and could not tell — must never disqualify a note.
    fn unknown_fact() -> MergeFact {
        MergeFact {
            status: MergeStatus::Unknown,
            sha: None,
            method: Method::PatchId,
            pr: None,
            why: Some("unknown (squash suspected, no gh)".into()),
            checked_at: ago(1),
            changed: Vec::new(),
        }
    }

    // ── the dependency graph ─────────────────────────────────────────────────

    #[test]
    fn a_todo_with_no_deps_is_ready() {
        let mut s = snap();
        put(&mut s, ticket("t-0001", State::Todo));
        assert!(is_ready(&s, &s.tickets[&tid("t-0001")]));
        assert_eq!(ready_queue(&s).len(), 1);
        assert_eq!(column(&s, &s.tickets[&tid("t-0001")]), Column::Ready);
    }

    #[test]
    fn every_terminal_or_in_main_dep_satisfies_and_a_missing_one_does_not() {
        let mut s = snap();
        // done / dropped satisfy (D-17: blocking forever on a dropped dep is worse)
        put(&mut s, ticket("t-000d", State::Done));
        put(&mut s, ticket("t-00dd", State::Dropped));
        // in-main satisfies even though the ticket is still open
        put(&mut s, ticket("t-00aa", State::Review));
        s.git.tickets.insert(tid("t-00aa"), merged(&[]));
        // doing does not
        put(&mut s, ticket("t-00bb", State::Doing));

        assert!(dep_satisfied(&s, &tid("t-000d")));
        assert!(
            dep_satisfied(&s, &tid("t-00dd")),
            "dropped satisfies (D-17)"
        );
        assert!(
            dep_satisfied(&s, &tid("t-00aa")),
            "in-main satisfies (D-17)"
        );
        assert!(!dep_satisfied(&s, &tid("t-00bb")));
        assert!(
            !dep_satisfied(&s, &tid("t-9999")),
            "a dep pointing at nothing is never satisfied"
        );
    }

    #[test]
    fn blocked_by_names_every_unsatisfied_dep_including_the_missing_one() {
        let mut s = snap();
        put(&mut s, ticket("t-000d", State::Done));
        put(&mut s, ticket("t-00bb", State::Doing));
        let mut t = ticket("t-0001", State::Todo);
        t.fm.deps = vec![tid("t-000d"), tid("t-00bb"), tid("t-9999")];
        put(&mut s, t);

        let t = &s.tickets[&tid("t-0001")];
        assert!(!is_ready(&s, t));
        let blocked: Vec<String> = blocked_by(&s, t).iter().map(|d| d.to_string()).collect();
        assert_eq!(blocked, ["t-00bb", "t-9999"]);
        assert_eq!(column(&s, t), Column::Backlog);
    }

    #[test]
    fn a_cycle_is_reported_once_however_many_roots_reach_it() {
        let mut s = snap();
        for (id, dep) in [
            ("t-000a", "t-000b"),
            ("t-000b", "t-000c"),
            ("t-000c", "t-000a"),
        ] {
            let mut t = ticket(id, State::Todo);
            t.fm.deps = vec![tid(dep)];
            put(&mut s, t);
        }
        // a fourth ticket that merely POINTS INTO the cycle must not create a second one
        let mut outside = ticket("t-00ee", State::Todo);
        outside.fm.deps = vec![tid("t-000a")];
        put(&mut s, outside);

        let cycles = dep_cycles(&s);
        assert_eq!(cycles.len(), 1, "{cycles:?}");
        assert_eq!(
            cycles[0].iter().map(|i| i.to_string()).collect::<Vec<_>>(),
            ["t-000a", "t-000b", "t-000c"],
            "rotated so the smallest id leads"
        );
        assert!(!is_ready(&s, &s.tickets[&tid("t-000a")]));
    }

    #[test]
    fn a_self_dependency_is_a_one_ticket_cycle() {
        let mut s = snap();
        let mut t = ticket("t-000a", State::Todo);
        t.fm.deps = vec![tid("t-000a")];
        put(&mut s, t);
        assert_eq!(dep_cycles(&s).len(), 1);
    }

    #[test]
    fn an_acyclic_graph_has_no_cycles() {
        let mut s = snap();
        put(&mut s, ticket("t-000c", State::Todo));
        for (id, dep) in [("t-000a", "t-000c"), ("t-000b", "t-000c")] {
            let mut t = ticket(id, State::Todo);
            t.fm.deps = vec![tid(dep)];
            put(&mut s, t);
        }
        assert!(dep_cycles(&s).is_empty());
    }

    // ── the git overlay ──────────────────────────────────────────────────────

    #[test]
    fn the_badge_precedence_is_evidence_first() {
        let mut s = snap();
        put(&mut s, ticket("t-0001", State::Doing));
        let t = &s.tickets[&tid("t-0001")].clone();

        // nothing at all
        assert!(matches!(badge(&s, t), Badge::NeverScanned));

        // branch facts only
        s.git.branches.insert(
            tid("t-0001"),
            BranchFact {
                branch: Some("ks/t-0001".into()),
                head: None,
                ahead: Some(1),
                behind: Some(0),
                last_commit_at: Some(ago(1)),
                pushed: false,
            },
        );
        assert!(matches!(badge(&s, t), Badge::Unpushed));
        s.git.branches.get_mut(&tid("t-0001")).unwrap().pushed = true;
        assert!(matches!(badge(&s, t), Badge::Pushed));

        // unknown is a VALUE, never folded into "not merged"
        s.git.tickets.insert(
            tid("t-0001"),
            MergeFact {
                status: MergeStatus::Unknown,
                sha: None,
                method: Method::PatchId,
                pr: None,
                why: Some("squash suspected, no gh".into()),
                checked_at: ago(1),
                changed: vec![],
            },
        );
        assert!(matches!(badge(&s, t), Badge::Unknown { .. }));

        // ROUND-C REGRESSION. `cache::MergeFact.why` stores `git::Unknown::badge()`, which
        // is ALREADY `unknown (…)`; `Badge::text` adds that wrapper itself. Before the fix
        // this rendered `unknown (unknown (no branch or head SHA recorded) · checked 1h
        // ago)` on every unstarted ticket in `kanspec ls`. Exactly one layer owns it.
        s.git.tickets.insert(
            tid("t-0001"),
            MergeFact {
                status: MergeStatus::Unknown,
                sha: None,
                method: Method::None,
                pr: None,
                why: Some("unknown (no branch or head SHA recorded)".into()),
                checked_at: ago(1),
                changed: vec![],
            },
        );
        let text = badge(&s, t).text(now());
        assert!(
            !text.contains("unknown (unknown"),
            "the badge wrapper must be applied exactly once, got: {text}"
        );
        assert_eq!(
            text,
            "unknown (no branch or head SHA recorded · checked 1h ago)"
        );

        // merged wins over everything
        s.git.tickets.insert(tid("t-0001"), merged(&[]));
        let b = badge(&s, t);
        assert!(matches!(b, Badge::InMain { .. }));
        assert!(
            b.text(s.now).starts_with("in main (gh-pr #142 · checked"),
            "{b:?}"
        );
    }

    #[test]
    fn a_closed_ticket_is_never_in_main_however_the_cache_reads() {
        let mut s = snap();
        put(&mut s, ticket("t-0001", State::Done));
        s.git.tickets.insert(tid("t-0001"), merged(&[]));
        let t = &s.tickets[&tid("t-0001")];
        assert!(in_main(&s, t).is_none(), "done is done, not in-main");
        assert_eq!(column(&s, t), Column::Done);
    }

    // ── what stands behind a close ───────────────────────────────────────────

    /// A ticket closed the way the gate closes one: a branch, a head, and a `done` entry
    /// naming the commit the close was granted against.
    fn closed(id: &str) -> Ticket {
        let mut t = ticket(id, State::Done);
        t.fm.branch = Some(format!("ks/{id}"));
        t.fm.head = Some("3f2a19c7d4b6e8a0c1f5920b7e6d4a3c8b1f0e29".into());
        t.log = vec![
            entry(ago(3), "trevor", Verb::New, State::Todo),
            entry(ago(2), "trevor", Verb::Start, State::Doing),
            entry(ago(1), "trevor", Verb::Ship, State::Review),
            LogEntry {
                at: ago(1),
                state: State::Done,
                actor: "trevor".into(),
                verb: Verb::Done,
                note: Some("in main a1b9c3d via gh-pr #142".into()),
            },
        ];
        t
    }

    /// `note_sha` mirrors a grammar that lives in `scan.rs` behind private items, so this
    /// runs the REAL `plan_confirm` and reads its note back through the mirror. A gate
    /// that changed its spelling would fail here rather than silently stop corroborating.
    #[test]
    fn the_note_grammar_mirrors_the_gates_own_spelling() {
        use crate::ctx::Actor;
        use crate::plan::Op;
        use crate::scan::{plan_confirm, ConfirmFacts};

        let mut s = snap();
        let mut t = ticket("t-0001", State::Review);
        t.fm.head = Some("3f2a19c7d4b6e8a0c1f5920b7e6d4a3c8b1f0e29".into());
        put(&mut s, t);

        let plan = plan_confirm(
            &s,
            &ConfirmFacts {
                sha: None, // falls back to `head:`, exactly as a real `--confirm` does
                actor: Actor::Human {
                    name: "trevor".into(),
                },
                at: now(),
                why: "squash merged by hand, verified".into(),
                invocation: "kanspec scan --confirm t-0001".into(),
            },
            &tid("t-0001"),
        )
        .expect("a confirm with a head and a reason is legal");
        let detail = plan
            .ops
            .iter()
            .find_map(|op| match op {
                Op::Transition { detail, .. } => Some(detail.clone()),
                _ => None,
            })
            .expect("plan_confirm writes one transition");
        assert_eq!(
            note_sha(&detail),
            Some("3f2a19c7d4b6e8a0c1f5920b7e6d4a3c8b1f0e29"),
            "the mirror must read the gate's own note back: {detail}"
        );

        // `plan_done`'s half of the same grammar, in the shapes it actually emits.
        assert_eq!(note_sha("in main a1b9c3d via gh-pr #142"), Some("a1b9c3d"));
        assert_eq!(
            note_sha("in main a1b9c3d via trailer · spawned t-c412 · dropped 1 step(s)"),
            Some("a1b9c3d")
        );
        // Everything that names no commit reads as no commit, rather than as a parse error.
        for note in [
            "done",
            "no-code: docs only",
            "in main",
            "in main via ancestry",
            "in main deadbeg via ancestry", // 'g' is not hex
            "in main A1B9C3D via ancestry", // git prints lowercase
            "in mainland somewhere",
        ] {
            assert_eq!(note_sha(note), None, "{note:?}");
        }
    }

    #[test]
    fn a_close_is_corroborated_by_four_things_and_a_forgery_by_none_of_them() {
        // the gate's own record of the commit — git-tracked, so it outlives the cache
        let mut s = snap();
        put(&mut s, closed("t-0001"));
        assert_eq!(
            close_evidence(&s, &s.tickets[&tid("t-0001")]),
            Some(CloseEvidence::Proof("a1b9c3d".into()))
        );

        // THE forgery: the same trail, with the gate's note stripped off the close. It
        // still replays perfectly — and nothing outside the log says the work landed.
        let mut s = snap();
        let mut forged = closed("t-0002");
        forged.log.last_mut().unwrap().note = None;
        put(&mut s, forged);
        assert_eq!(close_evidence(&s, &s.tickets[&tid("t-0002")]), None);

        // a ladder run that actually saw it in main outranks the missing note
        s.git.tickets.insert(tid("t-0002"), merged(&[]));
        assert_eq!(
            close_evidence(&s, &s.tickets[&tid("t-0002")]),
            Some(CloseEvidence::InMain)
        );

        // The recorded `--no-code` waiver — prose under `## Log`, which `replay` skips.
        // The trail carries NO Ship: the gate refuses `--no-code` on a ticket shipped for
        // review, so this is the only shape a real waiver can have.
        let mut s = snap();
        let mut t = closed("t-0003");
        t.log.retain(|e| e.verb != Verb::Ship);
        t.log.last_mut().unwrap().note = Some("no-code: docs only".into());
        t.body = "## Log\n  no-code waiver by trevor at 2026-08-31T11:00Z: docs only\n".to_string();
        put(&mut s, t);
        assert_eq!(
            close_evidence(&s, &s.tickets[&tid("t-0003")]),
            Some(CloseEvidence::NoCode)
        );

        // EVASION 1 — the same waiver line appended to a trail that WAS shipped. The gate
        // would have refused that pair, so the log contradicts itself and corroborates
        // nothing. Without this, forging a close costs one copied line of prose.
        let mut s = snap();
        let mut t = closed("t-0009");
        t.log.last_mut().unwrap().note = None;
        t.body = "## Log\n  no-code waiver by trevor at 2026-08-31T11:00Z: docs only\n".to_string();
        put(&mut s, t);
        assert_eq!(close_evidence(&s, &s.tickets[&tid("t-0009")]), None);

        // …but an `unknown` verdict must NOT disqualify: an old close whose branch was
        // deleted and gc'd reads unknown forever, and flagging it punishes the honest case.
        let mut s = snap();
        put(&mut s, closed("t-0011"));
        s.git.tickets.insert(tid("t-0011"), unknown_fact());
        assert_eq!(
            close_evidence(&s, &s.tickets[&tid("t-0011")]),
            Some(CloseEvidence::Proof("a1b9c3d".into()))
        );

        // an attestation (D-12) answers first, so `doctor` says it in exactly one voice
        let mut s = snap();
        let mut t = closed("t-0004");
        t.log.last_mut().unwrap().note = None;
        t.log
            .push(entry(ago(1), "trevor", Verb::Repair, State::Done));
        put(&mut s, t);
        assert_eq!(
            close_evidence(&s, &s.tickets[&tid("t-0004")]),
            Some(CloseEvidence::Attested)
        );

        // and a ticket with no branch, head or PR leaves git nothing to be asked about
        let mut s = snap();
        let mut t = closed("t-0005");
        t.log.last_mut().unwrap().note = None;
        t.fm.branch = None;
        t.fm.head = None;
        put(&mut s, t);
        assert_eq!(
            close_evidence(&s, &s.tickets[&tid("t-0005")]),
            Some(CloseEvidence::NothingToLand)
        );
    }

    // ── the tripwires ────────────────────────────────────────────────────────

    #[test]
    fn stalled_fires_only_on_doing_and_only_past_the_window() {
        let mut s = snap();
        let mut t = ticket("t-0001", State::Doing);
        t.log = vec![entry(ago(1), "claude/sess-a91", Verb::Start, State::Doing)];
        put(&mut s, t);
        assert!(stalled(&s, &s.tickets[&tid("t-0001")]).is_none(), "1h < 2h");

        let mut t = ticket("t-0002", State::Doing);
        t.log = vec![entry(ago(3), "claude/sess-a91", Verb::Start, State::Doing)];
        put(&mut s, t);
        let d = stalled(&s, &s.tickets[&tid("t-0002")]).expect("3h > the 2h window");
        assert_eq!(short(d), "3h");

        // a commit on the branch is activity, so it clears the tripwire
        s.git.branches.insert(
            tid("t-0002"),
            BranchFact {
                branch: Some("ks/t-0002".into()),
                head: None,
                ahead: Some(1),
                behind: Some(0),
                last_commit_at: Some(ago(1)),
                pushed: false,
            },
        );
        assert!(stalled(&s, &s.tickets[&tid("t-0002")]).is_none());

        // and a review ticket is never STALLED, however long it sits
        let mut t = ticket("t-0003", State::Review);
        t.log = vec![entry(ago(100), "trevor", Verb::Ship, State::Review)];
        put(&mut s, t);
        assert!(stalled(&s, &s.tickets[&tid("t-0003")]).is_none());
    }

    #[test]
    fn a_clock_that_ran_backwards_fires_nothing() {
        let mut s = snap();
        let mut t = ticket("t-0001", State::Doing);
        t.fm.created = now() + chrono::Duration::days(2);
        t.log = vec![entry(
            now() + chrono::Duration::days(2),
            "trevor",
            Verb::Start,
            State::Doing,
        )];
        put(&mut s, t);
        assert!(stalled(&s, &s.tickets[&tid("t-0001")]).is_none());
        assert!(dwell(&s, &s.tickets[&tid("t-0001")]).is_none());
    }

    #[test]
    fn the_dwell_tripwires_are_design_mds_three_windows() {
        let mut s = snap();

        // review > 7d
        let mut t = ticket("t-0001", State::Review);
        t.log = vec![entry(ago(24 * 9), "trevor", Verb::Ship, State::Review)];
        put(&mut s, t);
        assert!(matches!(
            dwell(&s, &s.tickets[&tid("t-0001")]),
            Some(Tripwire::ReviewDwell(_))
        ));

        // in-main-not-closed > 1d outranks the review dwell on the same ticket
        s.git.tickets.insert(tid("t-0001"), merged(&[]));
        assert!(matches!(
            dwell(&s, &s.tickets[&tid("t-0001")]),
            Some(Tripwire::InMainNotClosed(_))
        ));

        // discovered, untriaged > 7d
        let mut t = ticket("t-0002", State::Todo);
        t.fm.discovered_in = Some(tid("t-0001"));
        t.fm.created = ago(24 * 9);
        put(&mut s, t);
        assert!(matches!(
            dwell(&s, &s.tickets[&tid("t-0002")]),
            Some(Tripwire::DiscoveredUntriaged(_))
        ));

        // ...but giving it a spec triages it
        s.tickets.get_mut(&tid("t-0002")).unwrap().fm.spec = Some(SpecName::parse("auth").unwrap());
        assert!(dwell(&s, &s.tickets[&tid("t-0002")]).is_none());
    }

    #[test]
    fn a_scan_cannot_reset_the_in_main_dwell() {
        // `checked_at` moves every time `scan` runs; the tripwire must not be anchored on
        // it, or a 60s scan loop makes it unfireable.
        let mut s = snap();
        let mut t = ticket("t-0001", State::Review);
        t.log = vec![entry(ago(24 * 3), "trevor", Verb::Ship, State::Review)];
        put(&mut s, t);
        let mut fact = merged(&[]);
        fact.checked_at = s.now; // scanned one second ago
        s.git.tickets.insert(tid("t-0001"), fact);
        assert!(matches!(
            dwell(&s, &s.tickets[&tid("t-0001")]),
            Some(Tripwire::InMainNotClosed(_))
        ));
    }

    // ── double claims ────────────────────────────────────────────────────────

    #[test]
    fn two_starts_with_no_release_between_them_is_a_double_claim() {
        let mut s = snap();
        let mut t = ticket("t-0001", State::Doing);
        t.fm.claimed_by = Some("claude/sess-a91".into());
        t.log = vec![
            entry(ago(5), "trevor", Verb::New, State::Todo),
            entry(ago(4), "trevor", Verb::Start, State::Doing),
            entry(ago(3), "claude/sess-a91", Verb::Start, State::Doing),
        ];
        put(&mut s, t);
        let flagged = double_claims(&s);
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].1, ["trevor", "claude/sess-a91"]);
    }

    #[test]
    fn a_rework_after_ship_is_not_a_double_claim() {
        let mut s = snap();
        let mut t = ticket("t-0001", State::Doing);
        t.log = vec![
            entry(ago(9), "trevor", Verb::New, State::Todo),
            entry(ago(8), "trevor", Verb::Start, State::Doing),
            entry(ago(7), "trevor", Verb::Ship, State::Review),
            entry(ago(6), "claude/sess-a91", Verb::Start, State::Doing),
        ];
        put(&mut s, t);
        assert!(double_claims(&s).is_empty());

        // and neither is a re-start by the same actor after a park
        let mut t = ticket("t-0002", State::Doing);
        t.log = vec![
            entry(ago(9), "trevor", Verb::Start, State::Doing),
            entry(ago(8), "trevor", Verb::Park, State::Todo),
            entry(ago(7), "trevor", Verb::Start, State::Doing),
        ];
        put(&mut s, t);
        assert!(double_claims(&s).is_empty());
    }

    // ── staleness ────────────────────────────────────────────────────────────

    fn spec_with(code: &[&str]) -> Spec {
        Spec {
            name: SpecName::parse("auth").unwrap(),
            fm: SpecFm {
                feature: "Login".into(),
                code: code.iter().map(|c| c.to_string()).collect(),
                stale_ack: None,
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(".kanspec/specs/auth.md"),
            body: String::new(),
            rules: Vec::new(),
        }
    }

    fn anchor(at: Option<DateTime<Utc>>, dead: &[&str]) -> SpecAnchor {
        anchor_counting(at, 0, dead)
    }

    /// An anchor carrying what `scan` asked git: the merges on main touching the spec's
    /// globs since `at`. A RESULT, recomputed whole every scan — not a tally anything
    /// increments (D-10).
    fn anchor_counting(at: Option<DateTime<Utc>>, merges: u32, dead: &[&str]) -> SpecAnchor {
        SpecAnchor {
            last_edit_sha: Some("deadbee".into()),
            last_edit_at: at,
            merges_since: merges,
            dead_globs: dead.iter().map(|d| d.to_string()).collect(),
        }
    }

    /// Three merged tickets touching `src/auth/**`, all after the anchor.
    fn stale_snapshot() -> (Snapshot, Spec) {
        let mut s = snap();
        let spec = spec_with(&["src/auth/**"]);
        s.git
            .specs
            .insert(spec.name.clone(), anchor(Some(ago(24 * 30)), &[]));
        for (i, id) in ["t-0001", "t-0002", "t-0003"].iter().enumerate() {
            let mut t = ticket(id, State::Done);
            t.log = vec![entry(
                ago(24 * (10 - i as i64)),
                "trevor",
                Verb::New,
                State::Todo,
            )];
            put(&mut s, t);
            s.git
                .tickets
                .insert(tid(id), merged(&["src/auth/login.ts"]));
        }
        s.specs.insert(spec.name.clone(), spec.clone());
        (s, spec)
    }

    #[test]
    fn staleness_counts_merges_touching_the_globs_since_the_anchor() {
        let (s, spec) = stale_snapshot();
        match staleness(&s, &spec) {
            Staleness::Stale {
                merges, examples, ..
            } => {
                assert_eq!(merges, 3, "the default window is 3");
                assert_eq!(examples.len(), 3);
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn staleness_ignores_merges_that_miss_the_globs_and_ones_before_the_anchor() {
        let (mut s, spec) = stale_snapshot();
        // one merge misses the globs
        s.git
            .tickets
            .insert(tid("t-0003"), merged(&["src/billing/charge.ts"]));
        assert!(matches!(staleness(&s, &spec), Staleness::Ok), "2 < 3");

        // and one predates the spec's last edit
        s.git
            .tickets
            .insert(tid("t-0003"), merged(&["src/auth/x.ts"]));
        s.tickets.get_mut(&tid("t-0003")).unwrap().log =
            vec![entry(ago(24 * 90), "trevor", Verb::New, State::Todo)];
        assert!(matches!(staleness(&s, &spec), Staleness::Ok), "2 < 3");
    }

    #[test]
    fn a_stale_ack_resets_the_tripwire_without_any_counter() {
        let (s, mut spec) = stale_snapshot();
        assert!(matches!(staleness(&s, &spec), Staleness::Stale { .. }));
        spec.fm.stale_ack = Some(StaleAck {
            sha: "a1b9c3d".into(),
            // AFTER every merge above — the human said "no behaviour change" just now.
            at: ago(1),
            by: "trevor".into(),
            why: "refactor only".into(),
        });
        assert!(
            matches!(staleness(&s, &spec), Staleness::Ok),
            "the git-tracked attestation is the new anchor (D-10)"
        );
    }

    /// THE headline case, and the one the ticket-only count could never see: every merge
    /// that touched the spec's code went through somebody else's PR, a hotfix or a
    /// dependabot bump. `GitState.tickets` is EMPTY, and the wire must still trip.
    #[test]
    fn merges_no_kanspec_ticket_ever_saw_still_trip_the_wire() {
        let mut s = snap();
        let spec = spec_with(&["src/auth/**"]);
        s.git.specs.insert(
            spec.name.clone(),
            anchor_counting(Some(ago(24 * 30)), 4, &[]),
        );
        s.specs.insert(spec.name.clone(), spec.clone());
        assert!(s.git.tickets.is_empty(), "nothing kanspec tracked");

        match staleness(&s, &spec) {
            Staleness::Stale {
                merges, examples, ..
            } => {
                assert_eq!(merges, 4, "the count git gave `scan`");
                assert!(
                    examples.is_empty(),
                    "there is no ticket to name — the drift is real anyway"
                );
            }
            other => panic!("a spec drifting under untracked merges must go stale: {other:?}"),
        }
        // ...and it reaches `status` as a WATCHING line with its one-command fix.
        let a = attention(&s)
            .into_iter()
            .find(|a| a.subject == "auth")
            .expect("a stale spec line");
        assert_eq!(
            a.line,
            "4 merges touched src/auth/** since spec last edited"
        );
        assert_eq!(a.fix, "kanspec features --stale");
    }

    #[test]
    fn the_recorded_count_and_the_tickets_are_the_same_merges_so_they_never_sum() {
        // Three merged kanspec tickets AND a recorded count of 4: the tickets are three of
        // those four merges. Summing would say 7 and over-fire by naming merges that never
        // happened; taking the larger keeps the answer git's.
        let (mut s, spec) = stale_snapshot();
        s.git.specs.insert(
            spec.name.clone(),
            anchor_counting(Some(ago(24 * 30)), 4, &[]),
        );
        match staleness(&s, &spec) {
            Staleness::Stale {
                merges, examples, ..
            } => {
                assert_eq!(merges, 4);
                assert_eq!(examples.len(), 3, "the tickets still NAME what they can");
            }
            other => panic!("expected Stale, got {other:?}"),
        }

        // And the other way round: git could not answer (a decline records a 0), so the
        // tickets carry the count on their own.
        s.git.specs.insert(
            spec.name.clone(),
            anchor_counting(Some(ago(24 * 30)), 0, &[]),
        );
        assert!(matches!(
            staleness(&s, &spec),
            Staleness::Stale { merges: 3, .. }
        ));
    }

    #[test]
    fn an_attestation_supersedes_the_recorded_count_it_postdates() {
        let mut s = snap();
        let mut spec = spec_with(&["src/auth/**"]);
        s.git.specs.insert(
            spec.name.clone(),
            anchor_counting(Some(ago(24 * 30)), 4, &[]),
        );
        s.specs.insert(spec.name.clone(), spec.clone());
        assert!(matches!(staleness(&s, &spec), Staleness::Stale { .. }));

        // `scan` measured those 4 from the spec's last edit, a month ago. The human looked
        // an hour ago and signed for it, so the count no longer answers the question — and
        // nobody decremented it: the cache still says 4.
        spec.fm.stale_ack = Some(StaleAck {
            sha: "a1b9c3d".into(),
            at: ago(1),
            by: "trevor".into(),
            why: "refactor only".into(),
        });
        assert!(
            matches!(staleness(&s, &spec), Staleness::Ok),
            "the newer attestation is the anchor (D-10)"
        );
        assert_eq!(s.git.specs[&spec.name].merges_since, 4);

        // An attestation OLDER than the spec's own last edit says nothing about merges
        // counted from that edit, so the count still stands.
        spec.fm.stale_ack.as_mut().unwrap().at = ago(24 * 60);
        assert!(matches!(
            staleness(&s, &spec),
            Staleness::Stale { merges: 4, .. }
        ));
    }

    #[test]
    fn an_anchor_with_nothing_to_count_from_is_never_a_green_tick() {
        // The spec exists and was scanned, but `git log <main> -- <spec>` found nothing:
        // it has never been committed. There is no point to count merges from, and D-10's
        // rule is the same at the quiet end — "I have never looked" may not render as
        // "I looked and it is fine".
        let mut s = snap();
        let spec = spec_with(&["src/auth/**"]);
        s.git
            .specs
            .insert(spec.name.clone(), anchor_counting(None, 0, &[]));
        s.specs.insert(spec.name.clone(), spec.clone());
        assert!(matches!(staleness(&s, &spec), Staleness::NeverScanned));

        // Glob rot still outranks it: a dead glob is something we DID observe.
        s.git.specs.insert(
            spec.name.clone(),
            anchor_counting(None, 0, &["src/auth/**"]),
        );
        assert!(matches!(staleness(&s, &spec), Staleness::DeadGlobs { .. }));

        // ...and an attestation is an anchor even with no last edit, so the spec is then
        // genuinely `Ok` rather than unmeasured.
        let mut acked = spec.clone();
        acked.fm.stale_ack = Some(StaleAck {
            sha: "a1b9c3d".into(),
            at: ago(1),
            by: "trevor".into(),
            why: "brand new spec".into(),
        });
        s.git
            .specs
            .insert(spec.name.clone(), anchor_counting(None, 0, &[]));
        assert!(matches!(staleness(&s, &acked), Staleness::Ok));
    }

    #[test]
    fn a_wiped_cache_reads_never_scanned_not_zero_merges() {
        let (mut s, spec) = stale_snapshot();
        s.git = crate::cache::GitState::default();
        assert!(
            matches!(staleness(&s, &spec), Staleness::NeverScanned),
            "an absent anchor must never read as a confident zero"
        );
    }

    #[test]
    fn glob_rot_is_reported_rather_than_counted() {
        let (mut s, spec) = stale_snapshot();
        s.git.specs.insert(
            spec.name.clone(),
            anchor(Some(ago(24 * 30)), &["src/auth/**"]),
        );
        match staleness(&s, &spec) {
            Staleness::DeadGlobs { globs } => assert_eq!(globs, ["src/auth/**"]),
            other => panic!("expected DeadGlobs, got {other:?}"),
        }
    }

    #[test]
    fn a_glob_is_matched_with_git_pathspec_semantics() {
        let mut s = snap();
        let spec = spec_with(&["src/*.ts"]);
        s.git
            .specs
            .insert(spec.name.clone(), anchor(Some(ago(24 * 30)), &[]));
        s.git
            .tickets
            .insert(tid("t-0001"), merged(&["src/auth/login.ts"]));
        s.cfg.windows.stale_merges = 1;
        assert!(
            matches!(staleness(&s, &spec), Staleness::Ok),
            "`*` does not cross a directory separator"
        );
    }

    // ── the attention list ───────────────────────────────────────────────────

    #[test]
    fn the_attention_list_reproduces_design_mds_transcript_shape() {
        let mut s = snap();

        // YOU: in main, not closed
        let mut landed = ticket("t-31aa", State::Review);
        landed.fm.pr = Some(142);
        landed.log = vec![entry(ago(2), "trevor", Verb::Ship, State::Review)];
        put(&mut s, landed);
        s.git.tickets.insert(tid("t-31aa"), merged(&[]));

        // AGENT: ready, unblocked by the landing above
        let mut next = ticket("t-66d1", State::Todo);
        next.fm.spec = Some(SpecName::parse("auth").unwrap());
        next.fm.deps = vec![tid("t-31aa")];
        put(&mut s, next);

        // WATCHING: stalled
        let mut stalled = ticket("t-88fe", State::Doing);
        stalled.log = vec![entry(ago(3), "claude/sess-a91", Verb::Start, State::Doing)];
        put(&mut s, stalled);

        let list = attention(&s);
        let line = |id: &str| {
            list.iter()
                .find(|a| a.subject == id)
                .unwrap_or_else(|| panic!("no line for {id} in {list:#?}"))
        };

        let you = line("t-31aa");
        assert_eq!(you.owner, Owner::You);
        assert_eq!(you.glyph, '⇂');
        assert!(
            you.line
                .starts_with("in main 2h (gh-pr #142 · checked 1h ago), not closed"),
            "{}",
            you.line
        );
        assert_eq!(you.fix, "kanspec done t-31aa");

        let agent = line("t-66d1");
        assert_eq!(agent.owner, Owner::Agent);
        assert_eq!(agent.line, "ready · auth · unblocked when t-31aa closed");
        assert_eq!(agent.fix, "kanspec start t-66d1");

        let watching = line("t-88fe");
        assert_eq!(watching.owner, Owner::Watching);
        assert_eq!(
            watching.line,
            "STALLED: doing, no commits or updates for 3h"
        );
        assert_eq!(watching.fix, "kanspec park t-88fe --why \"...\"");

        // grouped in owner order, and every line names its fix
        let owners: Vec<Owner> = list.iter().map(|a| a.owner).collect();
        assert_eq!(owners, [Owner::You, Owner::Agent, Owner::Watching]);
        assert!(list.iter().all(|a| !a.fix.is_empty()), "invariant 9");
    }

    #[test]
    fn a_stale_spec_is_a_watching_line() {
        let (s, _) = stale_snapshot();
        let a = attention(&s)
            .into_iter()
            .find(|a| a.subject == "auth")
            .expect("a stale spec line");
        assert_eq!(a.owner, Owner::Watching);
        assert_eq!(
            a.line,
            "3 merges touched src/auth/** since spec last edited"
        );
        assert_eq!(a.fix, "kanspec features --stale");
    }

    #[test]
    fn a_clean_board_owes_nothing() {
        let mut s = snap();
        put(&mut s, ticket("t-0001", State::Done));
        put(&mut s, ticket("t-0002", State::Dropped));
        assert!(attention(&s).is_empty());
        let d = compute(&s);
        assert!(d.ready.is_empty() && d.blocked.is_empty() && d.attention.is_empty());
    }

    #[test]
    fn compute_agrees_with_every_function_it_aggregates() {
        let mut s = snap();
        put(&mut s, ticket("t-0001", State::Todo));
        let mut doing = ticket("t-0002", State::Doing);
        doing.log = vec![entry(ago(9), "trevor", Verb::Start, State::Doing)];
        put(&mut s, doing);
        let mut blocked = ticket("t-0003", State::Todo);
        blocked.fm.deps = vec![tid("t-0002")];
        put(&mut s, blocked);

        let d = compute(&s);
        assert_eq!(
            d.ready,
            ready_queue(&s)
                .iter()
                .map(|t| t.fm.id.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(d.blocked[&tid("t-0003")], [tid("t-0002")]);
        assert!(d.stalled.contains_key(&tid("t-0002")));
        assert_eq!(d.attention.len(), attention(&s).len());
    }

    #[test]
    fn the_ready_line_collapses_past_five() {
        let mut s = snap();
        for i in 0..8 {
            put(&mut s, ticket(&format!("t-00{i}0"), State::Todo));
        }
        let agent: Vec<Attention> = attention(&s)
            .into_iter()
            .filter(|a| a.owner == Owner::Agent)
            .collect();
        assert_eq!(agent.len(), 6, "five tickets plus one summary line");
        assert!(agent.iter().any(|a| a.line == "+3 more ready"));
    }
}
