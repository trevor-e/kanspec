//! The merge-detection ladder and the sealed proof.
//!
//! [`MergedProof`] has private fields, **no `Default`, no `Deserialize`, no
//! `From<MergeFact>`**. The only constructors live in this file and each one ran a real
//! ladder, so `plan_done`'s signature makes invariant 1 a *compile-time* guarantee: a
//! `done` that never consulted git does not build. `tests/proof_is_sealed.rs` greps for
//! the impls that would break it — it *will* be tempting the first time someone wants a
//! fast `status`.
//!
//! Owner: **S3**.

// Wave-0 skeleton. The bodies below are `todo!("S3: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S3 when the bodies land.
#![allow(unused_variables, dead_code)]

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cache::{GitState, MergeFact};
use crate::ctx::{Actor, Ctx};
use crate::error::Result;
use crate::gh::Gh;
use crate::git::{Git, Method, RungTrace, Sha, Unknown};
use crate::ids::TicketId;
use crate::model::{Snapshot, Ticket};
use crate::plan::Plan;

// ─────────────────────────────────────────────────────────────────────────────
// The seals
// ─────────────────────────────────────────────────────────────────────────────

/// Same-module privacy — no `pub(in …)`, which does not compile in a flat layout (E0742).
#[derive(Clone, Debug, Serialize)]
pub struct MergedProof {
    ticket: TicketId,
    sha: Sha,
    method: Method,
    pr: Option<u64>,
    checked_at: DateTime<Utc>,
}

impl MergedProof {
    pub fn ticket(&self) -> &TicketId {
        &self.ticket
    }
    pub fn sha(&self) -> &Sha {
        &self.sha
    }
    pub fn method(&self) -> Method {
        self.method
    }
    pub fn pr(&self) -> Option<u64> {
        self.pr
    }
    pub fn checked_at(&self) -> DateTime<Utc> {
        self.checked_at
    }
    /// `"IN MAIN (gh-pr #142 · checked 11s ago)"`
    pub fn badge(&self) -> String {
        let pr = self.pr.map(|n| format!(" #{n}")).unwrap_or_default();
        format!(
            "IN MAIN ({}{pr} · checked {})",
            self.method,
            crate::out::rel_time(self.checked_at, Utc::now())
        )
    }
}

/// The chore/docs escape — a DIFFERENT type, so the landed path cannot accept it, and
/// [`NoCodeWaiver::record`] needs `&mut Plan` so the waiver is DURABLE before it is
/// usable. `plan_done` additionally refuses it from `review`, so `--no-code` provably
/// cannot bypass the gate on an already-shipped ticket.
#[derive(Clone, Debug, Serialize)]
pub struct NoCodeWaiver {
    why: String,
    by: String,
    at: DateTime<Utc>,
}

impl NoCodeWaiver {
    /// Refuses an empty `why`: an unexplained escape is the one thing this type exists to
    /// prevent.
    pub fn record(
        plan: &mut Plan,
        id: &TicketId,
        why: &str,
        by: &Actor,
        at: DateTime<Utc>,
    ) -> Result<NoCodeWaiver> {
        todo!("S3: refuse an empty why; push the durable Op recording it; return the waiver")
    }
    pub fn why(&self) -> &str {
        &self.why
    }
    pub fn by(&self) -> &str {
        &self.by
    }
    pub fn at(&self) -> DateTime<Utc> {
        self.at
    }
}

/// What `plan_done` accepts. Two constructors, two very different stories, one gate.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "how", rename_all = "snake_case")]
pub enum Landed {
    Proof(MergedProof),
    NoCode(NoCodeWaiver),
}

/// Minted ONLY by [`scan_all`]; required by `Op::WriteGitState`. That is what makes
/// "gitstate.json is written by scan and nothing else" a type fact rather than a comment.
#[derive(Debug, PartialEq)]
pub struct ScanToken(());

// ─────────────────────────────────────────────────────────────────────────────
// The ladder's value types
// ─────────────────────────────────────────────────────────────────────────────

/// Three-state per rung. Ancestry-NEGATIVE is `Inconclusive`, not `NotMerged`: a
/// squash-merged branch is genuinely not an ancestor of main. **Only rungs that can PROVE
/// absence may say No.**
pub enum Rung {
    Merged(Evidence),
    NotMerged(Evidence),
    Inconclusive(Unknown),
}

#[derive(Clone, Debug, Serialize)]
pub struct Evidence {
    pub method: Method,
    pub saw: String,
    pub sha: Option<Sha>,
    pub pr: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Landed {
        sha: Sha,
        method: Method,
        pr: Option<u64>,
    },
    NotLanded,
    Unknown(Unknown),
}

/// A completed ladder run. Carries its own `rungs`, so `scan --explain` is a property of
/// the value [`ladder`] already produced — there is no SECOND ladder run that could
/// disagree with the first.
#[derive(Clone, Debug, Serialize)]
pub struct Detection {
    verdict: Verdict,
    checked_at: DateTime<Utc>,
    fetch_age_secs: Option<u64>,
    rungs: Vec<RungTrace>,
}

impl Detection {
    fn seal(v: Verdict, at: DateTime<Utc>, age: Option<u64>, r: Vec<RungTrace>) -> Detection {
        Detection {
            verdict: v,
            checked_at: at,
            fetch_age_secs: age,
            rungs: r,
        }
    }
    pub fn verdict(&self) -> &Verdict {
        &self.verdict
    }
    pub fn checked_at(&self) -> DateTime<Utc> {
        self.checked_at
    }
    pub fn fetch_age_secs(&self) -> Option<u64> {
        self.fetch_age_secs
    }
    pub fn explain(&self) -> &[RungTrace] {
        &self.rungs
    }
    /// Down-converts the sealed, in-process value to the plain cache DTO. The cache is
    /// badge-grade; the gate is proof-grade. **There is deliberately no inverse.**
    pub fn to_fact(&self, changed: Vec<String>) -> MergeFact {
        todo!("S3: Verdict -> MergeStatus + sha/method/pr/why, stamped with checked_at")
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The ladder
// ─────────────────────────────────────────────────────────────────────────────

/// THE LADDER, in the recon-corrected order. Every rung returns a verdict; **exit 128
/// anywhere is Unknown, never No.**
///
/// - **guard 0** `rev-parse --verify --quiet '<head>^{commit}'` -> `HeadNotInObjectStore`
/// - **guard 0b** `rev-list --count <main>..<head> == 0` -> `ZeroCommitBranch`
///   (a fresh `start` branch is trivially an ancestor of main — a VERIFIED false MERGED
///   for work that never happened)
/// - **1 ANCESTRY** `merge-base --is-ancestor` — 0 = Merged, 1 = next rung, 128 = Unknown
/// - **2 GH** `pr view <n>` / `pr list --head <branch>`; MERGED -> RE-VERIFY with
///   `is-ancestor(mergeCommit.oid)`; mismatch -> `GhMergedButNotAncestor`; any gh failure
///   -> `GhUnavailable`, NEVER NotMerged
/// - **3 TRAILER** `log <main> -E --grep 'Kanspec: t-9c41([^0-9a-f]|$)'` — unanchored and
///   boundary-terminated. Blind to reverts, so it is weighted BELOW ancestry
/// - **4 PATCH-ID** `cherry <main> <head>` — relabelled *rebase/cherry-pick detection*
///   (D-3). Merged iff output non-empty AND every line is `-`; any `+` ->
///   `SquashSuspectedNoGh`
/// - **5** otherwise Unknown, with every `RungTrace` attached
pub fn ladder(
    git: &Git,
    gh: &Gh,
    t: &Ticket,
    main: &str,
    fetch_age: Option<Duration>,
    now: DateTime<Utc>,
) -> Detection {
    todo!("S3: the six rungs above, each pushing a RungTrace, sealed by Detection::seal")
}

/// The `done` gate. **RE-RUNS the ladder** rather than trusting the cache — "a 60s-old
/// merged is not a gate".
pub fn proof_for_done(ctx: &Ctx, t: &Ticket) -> Result<MergedProof> {
    todo!("S3: run the ladder; Landed -> MergedProof; else Gate{{NotLanded{{trace}}}} naming scan --explain / --confirm")
}

pub struct ScanOpts {
    pub fetch: bool,
    pub only: Option<TicketId>,
    pub quiet: bool,
}

/// Runs the ladder across every non-terminal ticket + spec anchors + branch facts. The
/// ONLY producer of [`ScanToken`]. Runs **outside** the lock (gh/network); the caller then
/// opens a short `transact` to persist via `Op::WriteGitState`.
pub fn scan_all(ctx: &Ctx, snap: &Snapshot, opts: ScanOpts) -> Result<(GitState, ScanToken)> {
    todo!("S3: optional fetch, ladder per non-terminal ticket, spec anchors, branch facts")
}

pub struct ConfirmFacts {
    pub sha: Option<Sha>,
    pub actor: Actor,
    pub at: DateTime<Utc>,
    pub why: String,
    pub invocation: String,
}

/// The recorded human override. Appends an attributed `Verb::Confirm` line to the
/// TICKET'S `## Log`, not a cache entry: a human attestation is an ASSERTED ACT WITH AN
/// ACTOR, so it must survive `rm -rf cache/` and be visibly signed (D-11).
pub fn plan_confirm(snap: &Snapshot, f: &ConfirmFacts, id: &TicketId) -> Result<Plan> {
    todo!("S3: refuse an empty why; emit Op::Transition{{verb: Confirm}} with the attestation as its note")
}

/// Reads a recorded confirmation back out of the log — the ONLY non-ladder route to a
/// [`MergedProof`].
pub fn confirmed_proof(t: &Ticket) -> Option<MergedProof> {
    todo!("S3: find the newest Verb::Confirm entry and seal it with Method::HumanConfirm")
}
