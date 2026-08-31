//! The invariant prover, as a **registry**. A new check is one entry in [`CHECKS`] plus
//! one function — never an edit to a match arm somewhere else.
//!
//! `doctor` is where R-1 and R-2 are paid for: multi-file plans are not atomic and the
//! seals stop at the file boundary, so the answer is *detection*, run at the next verb and
//! in CI, not prevention.
//!
//! Owner: **S4**.

// Wave-0 skeleton. The bodies below are `todo!("S4: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S4 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::ctx::Ctx;
use crate::error::Result;
use crate::model::Snapshot;
use crate::plan::Plan;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// fails `doctor` and CI (exit 1)
    Error,
    /// an amber dot on the board's feature strip
    Warning,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// the check's stable id, e.g. `"log_trail"` — an agent branches on this
    pub check: &'static str,
    pub severity: Severity,
    /// the entity this is about, e.g. `"t-9c41"` or `"spec auth"`
    pub subject: String,
    pub message: String,
    /// invariant 9: every finding names its one-command fix
    pub fix: String,
    /// whether `--fix` can repair it mechanically
    pub fixable: bool,
}

/// Pure over the snapshot — a check never touches the filesystem or git directly.
pub type RunCheck = fn(&Snapshot) -> Vec<Finding>;
/// Returns the ops that repair one finding, applied through the ordinary
/// `Store::transact` write path — `--fix` is not a second writer.
pub type FixCheck = fn(&Snapshot, &Finding, &mut Plan) -> Result<()>;

/// One row per invariant. A new check is one entry here plus one function.
pub struct Check {
    pub id: &'static str,
    pub about: &'static str,
    pub run: RunCheck,
    pub fix: Option<FixCheck>,
}

/// THE registry. `doctor` iterates exactly this; nothing else enumerates checks.
pub static CHECKS: &[Check] = &[
    Check {
        id: "log_trail",
        about: "every ticket's state was reached by a legal, in-order logged transition",
        run: check_log_trail,
        fix: None, // recovery is `kanspec repair <id> --why` — a human attestation (D-12)
    },
    Check {
        id: "reserved_keys",
        about: "no entity's frontmatter carries a derived key (merged, in_main, ci, …)",
        run: check_reserved_keys,
        fix: Some(fix_reserved_keys),
    },
    Check {
        id: "frontmatter_writable",
        about: "every frontmatter can be edited surgically without appending a duplicate key",
        run: check_frontmatter_writable,
        fix: None,
    },
    Check {
        id: "orphan_deps",
        about: "no dep points at a missing or dropped ticket",
        run: check_orphan_deps,
        fix: None,
    },
    Check {
        id: "dep_cycles",
        about: "the dependency graph is acyclic",
        run: check_dep_cycles,
        fix: None,
    },
    Check {
        id: "dead_globs",
        about: "spec code globs, quirk paths and decision scopes match at least one file",
        run: check_dead_globs,
        fix: None,
    },
    Check {
        id: "untyped_prescriptions",
        about: "every [pN] is (temp until t-x) or (promote: …) — a close blocker",
        run: check_untyped_prescriptions,
        fix: None,
    },
    Check {
        id: "ledger_complete",
        about: "every closed proposal's ledger dispositions every [cN] and [pN]",
        run: check_ledger_complete,
        fix: None,
    },
    Check {
        id: "closed_agree",
        about: "status: closed and living under proposals/closed/ agree",
        run: check_closed_agree,
        fix: Some(fix_closed_agree),
    },
    Check {
        id: "half_applied",
        about: "no plan left a cross-file edit half-applied (R-1)",
        run: check_half_applied,
        fix: None,
    },
    Check {
        id: "duplicate_ids",
        about: "no two entities claim the same id after a merge (R-7)",
        run: check_duplicate_ids,
        fix: None,
    },
    Check {
        id: "immutable_decisions",
        about: "no accepted decision's body changed without a status change",
        run: check_immutable_decisions,
        fix: None,
    },
];

/// The whole registry, run in order. Errors sort before warnings.
pub fn run_all(snap: &Snapshot) -> Vec<Finding> {
    todo!("S4: flat_map CHECKS, then sort by (severity, check, subject)")
}

/// Turns fixable findings into one plan; `cmd::doctor` hands it to `Store::transact`, so
/// `--fix` uses the same write path as every verb.
pub fn plan_fixes(snap: &Snapshot, findings: &[Finding]) -> Result<Plan> {
    todo!("S4: for each fixable finding, call its Check::fix into a shared Plan")
}

/// A convenience for `cmd/status.rs`, which shows doctor errors as YOU lines.
pub fn run_all_ctx(ctx: &Ctx) -> Result<Vec<Finding>> {
    todo!("S4: ctx.snapshot() then run_all")
}

// ── the checks ───────────────────────────────────────────────────────────────

fn check_log_trail(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: transitions::prove() per ticket; a LogViolation is an Error naming `kanspec repair`")
}

fn check_reserved_keys(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: scan every entity's flattened `extra` map against keys::RESERVED_DERIVED")
}

fn check_frontmatter_writable(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: fm::writable() on every entity's frontmatter text")
}

fn check_orphan_deps(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: deps pointing at a missing ticket (Error) or a dropped one (Warning, D-17)")
}

fn check_dep_cycles(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: derive::dep_cycles")
}

fn check_dead_globs(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: cache SpecAnchor::dead_globs plus quirk paths and decision scopes")
}

fn check_untyped_prescriptions(s: &Snapshot) -> Vec<Finding> {
    todo!("V2/S4: Prescription::Untyped on any open proposal")
}

fn check_ledger_complete(s: &Snapshot) -> Vec<Finding> {
    todo!("V2/S4: every [cN]/[pN] of a closed proposal appears in its ledger")
}

fn check_closed_agree(s: &Snapshot) -> Vec<Finding> {
    todo!("V2/S4: status: closed <-> the directory lives under proposals/closed/")
}

fn check_half_applied(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: a ticket whose log's last entry has no matching frontmatter delta, and the mirror case")
}

fn check_duplicate_ids(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: two files claiming one id after a merge — no automated fix (R-7)")
}

fn check_immutable_decisions(s: &Snapshot) -> Vec<Finding> {
    todo!("S4: an accepted decision whose body changed in git without a status change")
}

// ── the fixers ───────────────────────────────────────────────────────────────

fn fix_reserved_keys(s: &Snapshot, f: &Finding, p: &mut Plan) -> Result<()> {
    todo!("S4: emit the ops that strip the derived key from the entity's frontmatter")
}

fn fix_closed_agree(s: &Snapshot, f: &Finding, p: &mut Plan) -> Result<()> {
    todo!("V2/S4: Op::MoveDir or Op::SetFields so status and location agree")
}
