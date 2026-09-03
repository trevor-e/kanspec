//! The invariant prover, as a **registry**. A new check is one entry in [`CHECKS`] plus
//! one function — never an edit to a match arm somewhere else.
//!
//! `doctor` is where R-1 and R-2 are paid for: multi-file plans are not atomic and the
//! seals stop at the file boundary, so the answer is *detection*, run at the next verb and
//! in CI, not prevention.
//!
//! **What a check may read.** [`RunCheck`] is `fn(&Snapshot) -> Vec<Finding>` on purpose:
//! a check that could shell out would be a second, slower, un-unit-testable copy of
//! `scan`. Three invariants in DESIGN.md are therefore only partly provable here, and each
//! one says so at its function rather than pretending: `check_frontmatter_writable` (the
//! `Snapshot` carries each entity's BODY but not its raw frontmatter text),
//! `check_ledger_complete` (closed proposal bodies are deliberately never loaded — that is
//! invariant 4) and `check_immutable_decisions` (needs a git diff of the decision file).
//!
//! Owner: **S4**.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::derive;
use crate::error::Result;
use crate::ids::ProposalId;
use crate::keys::RESERVED_DERIVED;
use crate::model::{DecisionStatus, Prescription, ProposalStatus, QuirkStatus, Snapshot};
use crate::plan::{Op, Plan};
use crate::transitions;

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

impl Finding {
    /// Every check's finding is unfixable unless its registry row says otherwise, so that
    /// is the default here and `closed_agree` overrides it.
    fn new(
        check: &'static str,
        severity: Severity,
        subject: impl ToString,
        message: String,
        fix: String,
    ) -> Finding {
        Finding {
            check,
            severity,
            subject: subject.to_string(),
            message,
            fix,
            fixable: false,
        }
    }
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
        // No mechanical repair, and `kanspec repair` is deliberately NOT the prescription
        // (see `check_log_trail`): it is the last resort, not the first.
        fix: None,
    },
    Check {
        id: "attested_state",
        about: "no ticket rests in a terminal state a human only attested to",
        run: check_attested_state,
        fix: None, // there is nothing to rewrite — the remedy is evidence, or a truer state
    },
    Check {
        id: "unproven_close",
        about: "no ticket is `done` on the word of its own ## Log alone",
        run: check_unproven_close,
        fix: None, // same: evidence, a recorded waiver, or a truer state — never a rewrite
    },
    Check {
        id: "reserved_keys",
        about: "no entity's frontmatter carries a derived key (merged, in_main, ci, …)",
        run: check_reserved_keys,
        // NOTE (deviation from the wave-0 stub, reported as a request to F): there is no
        // mechanical repair, and that is a CONSEQUENCE of invariant 1 rather than an
        // omission. Stripping `merged:` needs an op that names the key `merged` — and
        // `keys::Key` deliberately has no such variant, while `Plan::validate` refuses any
        // `SetFields` naming a `RESERVED_DERIVED` key. Removing it would take a new
        // `Op::RemoveFields { entity, keys: Vec<String> }` taking RAW key names, which is
        // F's call, not S4's. Until then the finding names the file and the line to delete.
        fix: None,
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
        // All three path-scoped records: `scan` records liveness for a spec's `code:`
        // (`SpecAnchor.dead_globs`), an accepted decision's `scope:` and an active quirk's
        // `paths:` (`GitState.{decision,quirk}_dead_globs`), by one definition.
        about: "every spec `code:`, decision `scope:` and quirk `paths:` glob matches at \
                least one tracked file",
        run: check_dead_globs,
        fix: None,
    },
    Check {
        id: "cache_rows_dropped",
        about: "every row in cache/gitstate.json satisfies the invariants its readers rely on",
        run: check_cache_rows_dropped,
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
    let mut out: Vec<Finding> = CHECKS.iter().flat_map(|c| (c.run)(snap)).collect();
    out.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.check.cmp(b.check))
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.message.cmp(&b.message))
    });
    out
}

/// Turns fixable findings into one plan; `cmd::doctor` hands it to `Store::transact`, so
/// `--fix` uses the same write path as every verb.
pub fn plan_fixes(snap: &Snapshot, findings: &[Finding]) -> Result<Plan> {
    let mut plan = Plan::empty();
    for f in findings.iter().filter(|f| f.fixable) {
        let Some(fix) = CHECKS.iter().find(|c| c.id == f.check).and_then(|c| c.fix) else {
            continue;
        };
        fix(snap, f, &mut plan)?;
    }
    Ok(plan)
}

// ── the checks ───────────────────────────────────────────────────────────────

/// R-2, mechanically: the seals bind the tool, the `## Log` binds the human. A
/// `sed -i 's/state: review/state: done/'` leaves no log entry, so the fold through the
/// SAME oracle the write path uses cannot reach the state the file claims.
///
/// **The prescription is never `kanspec repair` first.** It used to be, and that made the
/// warning its own laundry: `sed` the frontmatter to `done`, run the command `doctor` put
/// under the finding, and the repo went green with an unmerged ticket closed inside it.
/// `repair` still exists and still rescues (D-12) — but it is the LAST resort, so the fix
/// line names the honest remedy for the violation actually found: put the frontmatter back
/// to the state the log reached, or put the log's own lines right. Both restore the truth;
/// an attestation only overwrites the question.
fn check_log_trail(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in s.tickets.values() {
        let Err(v) = transitions::prove(t) else {
            continue;
        };
        let id = &t.fm.id;
        let path = t.path.display();
        let fix = match &v {
            // The divergence case IS the hand-edit, and undoing it is mechanical and
            // lossless: the `## Log` already says which state this ticket legally reached.
            transitions::LogViolation::Divergence { replayed, .. } => {
                format!("edit {path} and set `state: {replayed}`")
            }
            transitions::LogViolation::OutOfOrder { .. } => {
                format!("edit {path} and put the ## Log lines back in date order")
            }
            _ => format!("edit {path} and repair the ## Log lines above the break"),
        };
        out.push(Finding::new(
            "log_trail",
            Severity::Error,
            id,
            // The escape stays discoverable, and stays described as what it is: a recorded
            // human attestation, badged from then on, for history that is genuinely lost.
            format!(
                "{v} — if the history is genuinely unrecoverable, `kanspec repair {id} \
                 --why \"...\"` attests it and the ticket is badged attested from then on"
            ),
            fix,
        ));
    }
    out
}

/// The other half of D-12, and the reason `repair` cannot be a quiet exit. A ticket whose
/// terminal state was *attested* rather than *replayed* is legitimate — that is the whole
/// point of the verb — but it must never be indistinguishable from one the gate proved.
///
/// Two grades, because two very different things arrive here:
///
/// - **Error** — `done` with nothing in the `## Log` saying the work landed. `plan_repair`
///   refuses to write this (`repair_cannot_close`), so what remains is a hand-written
///   `repair` line or one an older binary minted: an unmerged ticket closed by assertion,
///   which is precisely the claim this tool exists to refuse. It leads `status` too, since
///   `cmd::status` promotes doctor's errors to YOU lines.
/// - **Warning** — everything else: an attested `dropped`, or a `done` whose close the log
///   still carries. Nothing is broken; the state simply rests on a person's word, and that
///   stays on the record instead of ageing into a fact.
fn check_attested_state(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in s.tickets.values() {
        if !t.fm.state.terminal() {
            continue;
        }
        let Some(a) = derive::attested(t) else {
            continue;
        };
        let id = &t.fm.id;
        let when = a.at.format("%Y-%m-%d");
        let (severity, message, fix) =
            if t.fm.state == transitions::State::Done && !derive::logged_close(t) {
                (
                    Severity::Error,
                    format!(
                        "is `done` because {} attested it on {when}, and nothing in its ## Log \
                         says the work ever landed — this close was vouched for, never proven",
                        a.actor
                    ),
                    format!("kanspec scan --confirm {id} --why \"...\""),
                )
            } else {
                (
                    Severity::Warning,
                    format!(
                        "reached `{}` by attestation ({} on {when}) — a human's word, not a \
                         replayed trail",
                        t.fm.state, a.actor
                    ),
                    format!("kanspec log {id}"),
                )
            };
        out.push(Finding::new("attested_state", severity, id, message, fix));
    }
    out
}

/// The hole `log_trail` and `attested_state` between them still left open, and the reason
/// DESIGN.md's invariant 10 no longer says `doctor` *proves* every state was reached
/// legally.
///
/// `log_trail` replays the trail and `attested_state` catches the one verb whose state is
/// authoritative. Neither sees the two-edit forgery: `sed` the frontmatter to `done`, then
/// append ONE well-formed `done` line. The trail replays *perfectly* to the state the file
/// claims, no `repair` verb appears — and until this check the repo answered
/// `13 checks passed`, exit 0, with a provably unmerged ticket closed inside it.
///
/// What a fabricated line cannot write is corroboration. Every close the gate grants
/// records the commit it was granted against, or the durable `--no-code` waiver; a
/// `scan --confirm` records a commit too; an attestation is badged everywhere. A `done`
/// carrying none of those, on a ticket that names a branch, a head or a PR, was not
/// written by this tool's gate — and that is a claim, not a proof, so it is an Error and
/// `cmd::status` promotes it to a YOU line.
///
/// **Three deliberate silences, so this never cries wolf:**
/// - `dropped` — a drop is an act, not a merge; `--why` is the whole of its evidence.
/// - a trail that does not replay — that is `check_log_trail`'s finding, and the forgery
///   this exists for replays perfectly, so the two never speak about the same ticket.
/// - a ticket naming no branch, head or PR — nothing for git to place, so there is no
///   corroboration to demand. That is the honest edge of the detection, and the narrowed
///   invariant 10 names it rather than papering over it.
fn check_unproven_close(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in s.tickets.values() {
        if t.fm.state != transitions::State::Done {
            continue;
        }
        if transitions::prove(t).is_err() {
            continue;
        }
        if derive::close_evidence(s, t).is_some() {
            continue;
        }
        let id = &t.fm.id;
        out.push(Finding::new(
            "unproven_close",
            Severity::Error,
            id,
            format!(
                "is `done` with nothing outside its own ## Log behind the close: no commit \
                 recorded by the gate, no `--no-code` waiver, no attestation, and no ladder run \
                 that ever saw it in main. A ## Log is plain text, so a `done` line proves a \
                 `done` line was written — only git can corroborate that the work landed. If it \
                 did land, `kanspec scan --confirm {id} --why \"...\"` records the commit; if \
                 there was never any code, the close needed `--no-code --why`; if it never \
                 landed, put `state:` back to the state the log reached"
            ),
            // Ask GIT first. A TARGETED scan re-runs the ladder even on a terminal ticket,
            // so this is the diagnostic AND — when the work really did land — the repair,
            // while laundering nothing when it did not.
            format!("kanspec scan --explain {id}"),
        ));
    }
    out
}

/// The READ-side half of invariant 1. `keys.rs` makes a derived key unwritable BY TYPE;
/// this catches the file that ARRIVED with one — a hand-edit, an import, a bad merge —
/// which has no write path to blame.
fn check_reserved_keys(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for (subject, path, extra) in frontmatters(s) {
        let keys = reserved(extra);
        if keys.is_empty() {
            continue;
        }
        let listed = keys
            .iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Finding::new(
            "reserved_keys",
            Severity::Error,
            subject,
            format!(
                "frontmatter claims the derived {} {listed} — kanspec computes {} from git and \
                 never writes {} into a file",
                if keys.len() == 1 { "fact" } else { "facts" },
                if keys.len() == 1 { "it" } else { "them" },
                if keys.len() == 1 { "it" } else { "them" },
            ),
            // There is no verb for this on purpose (see the registry entry): naming the
            // file and the lines IS the one-command fix.
            format!("edit {} and delete the line(s) above", path.display()),
        ));
    }
    out
}

/// Every entity's frontmatter as `(subject, file, keys the schema did not claim)` — the one
/// walk the two frontmatter checks share, so a new entity kind is added in one place and
/// both checks name it the same way.
fn frontmatters(s: &Snapshot) -> Vec<(String, &Path, &BTreeMap<String, serde_yaml_ng::Value>)> {
    let mut v = Vec::new();
    for t in s.tickets.values() {
        v.push((t.fm.id.to_string(), &*t.path, &t.fm.extra));
    }
    for sp in s.specs.values() {
        v.push((format!("spec {}", sp.name), &*sp.path, &sp.fm.extra));
    }
    for d in s.decisions.values() {
        v.push((d.fm.id.to_string(), &*d.path, &d.fm.extra));
    }
    for q in s.quirks.values() {
        v.push((q.fm.id.to_string(), &*q.path, &q.fm.extra));
    }
    for p in s.proposals.values() {
        v.push((p.fm.id.to_string(), &*p.dir, &p.fm.extra));
    }
    v
}

/// Exactly `keys::RESERVED_DERIVED`, in its own order so two entities report identically.
fn reserved(extra: &BTreeMap<String, serde_yaml_ng::Value>) -> Vec<&'static str> {
    RESERVED_DERIVED
        .iter()
        .copied()
        .filter(|k| extra.contains_key(*k))
        .collect()
}

/// The pure half of "can `fm::set` edit this file in place?".
///
/// `fm::writable` compares the YAML parser's key set against the line indexer's, and it
/// needs the raw frontmatter TEXT — which a `Snapshot` does not carry (entities keep their
/// `body`, not their frontmatter). What IS visible is every key the schema did not claim,
/// in `extra`, and that is where a hand-edit puts an unindexable key. Two things are
/// caught here, both of which would make the very next write to the file refuse:
///
/// - a key the line indexer cannot see at all (`fm::parse_key` accepts
///   `[A-Za-z0-9_.\-/]+` followed by `: `), so a surgical `set` would APPEND a duplicate;
/// - a value spanning more than one line, which `fm::set` reports as
///   `ReplacedMultiline` — a hard error in `Store::transact` (R-9).
///
/// Not caught: an unindexable key on a key the schema DOES claim (it would have to be
/// spelled correctly to deserialize), and a top-level flow mapping. Both need the raw
/// text — see the module header.
fn check_frontmatter_writable(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for (subject, path, extra) in frontmatters(s) {
        for (k, v) in extra {
            if !indexable(k) {
                out.push(Finding::new(
                    "frontmatter_writable",
                    Severity::Error,
                    &subject,
                    format!(
                        "frontmatter key `{k}` is not one kanspec can edit in place; the next \
                         field update on this file would append a duplicate key"
                    ),
                    format!(
                        "edit {} and rewrite `{k}` as a plain `key: value`",
                        path.display()
                    ),
                ));
            }
            if multiline(v) {
                out.push(Finding::new(
                    "frontmatter_writable",
                    Severity::Warning,
                    &subject,
                    format!(
                        "the value of `{k}` spans more than one line; a surgical field update on \
                         this file will refuse rather than reformat it (R-9)"
                    ),
                    format!("edit {} and put `{k}` on one line", path.display()),
                ));
            }
        }
    }
    out
}

/// `fm::parse_key`'s accepted shape, mirrored. The two are unit-tested against each other
/// below rather than shared, because `fm.rs` is S1's file.
fn indexable(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
}

fn multiline(v: &serde_yaml_ng::Value) -> bool {
    use serde_yaml_ng::Value as V;
    match v {
        V::String(s) => s.contains('\n'),
        V::Mapping(_) => true,
        V::Sequence(items) => items.iter().any(multiline),
        _ => false,
    }
}

/// A dep pointing at nothing is an Error — `derive::dep_satisfied` refuses to satisfy it,
/// so the dependent ticket is silently unclaimable until this is fixed. A dep pointing at
/// a DROPPED ticket is only a Warning: it satisfies (D-17), and the warning exists so the
/// "why did this unblock?" question has an answer.
fn check_orphan_deps(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in s.tickets.values() {
        let id = &t.fm.id;
        for dep in &t.fm.deps {
            let (severity, message) = match s.tickets.get(dep) {
                None => (
                    Severity::Error,
                    format!("dep {dep} does not exist — {id} can never become ready"),
                ),
                Some(d) if d.fm.state == transitions::State::Dropped => (
                    Severity::Warning,
                    format!(
                        "dep {dep} was dropped; it counts as satisfied (D-17), so {id} is \
                         claimable on work that never happened"
                    ),
                ),
                Some(_) => continue,
            };
            out.push(Finding::new(
                "orphan_deps",
                severity,
                id,
                message,
                format!("kanspec show {id}"),
            ));
        }
    }
    out
}

fn check_dep_cycles(s: &Snapshot) -> Vec<Finding> {
    derive::dep_cycles(s)
        .into_iter()
        .map(|cycle| {
            let ids: Vec<String> = cycle.iter().map(|i| i.to_string()).collect();
            let first = ids.first().cloned().unwrap_or_default();
            Finding::new(
                "dep_cycles",
                Severity::Error,
                &first,
                format!(
                    "dependency cycle: {} → {first} — every ticket on it is permanently blocked",
                    ids.join(" → ")
                ),
                format!("kanspec show {first}"),
            )
        })
        .collect()
}

/// Rows `cache::load` refused to hand to a reader. A Warning, not an Error: the cache is
/// disposable and the next `scan` rewrites it — but a file something wrote by hand is
/// worth a line, or the badge that went quiet has no explanation.
fn check_cache_rows_dropped(s: &Snapshot) -> Vec<Finding> {
    s.git
        .dropped
        .iter()
        .map(|why| {
            Finding::new(
                "cache_rows_dropped",
                Severity::Warning,
                "cache/gitstate.json",
                format!("dropped on load: {why}"),
                "kanspec scan".to_string(),
            )
        })
        .collect()
}

/// Glob rot. `scan` records which of a record's globs matched zero files; nothing here
/// re-walks the filesystem, because a check that shelled out would be a second, worse copy
/// of `scan`.
///
/// Severity follows the damage, for all three kinds: a record that lost SOME globs still
/// steers the files it kept (Warning); one that lost them ALL steers nothing — a quirk is
/// never injected, a decision's body is never injected in full, a spec's rules reach no
/// path — which is indistinguishable from having deleted it, and a rename is how it
/// happens (Error, so CI notices).
fn check_dead_globs(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for (id, dead) in &s.git.decision_dead_globs {
        let total = s.decisions.get(id).map_or(0, |d| d.scope.len());
        rot(
            &mut out,
            format!("decision {id}"),
            total,
            dead,
            format!(
                "EVERY scope glob matches no files: {} — this decision steers nothing in \
                 full; `prime` lists it as a one-liner for every path and injects its \
                 body for none",
                dead.join(", ")
            ),
            format!("scope globs match no files: {}", dead.join(", ")),
            format!("kanspec why {id}"),
        );
    }
    for (id, dead) in &s.git.quirk_dead_globs {
        let total = s.quirks.get(id).map_or(0, |q| q.fm.paths.len());
        rot(
            &mut out,
            format!("quirk {id}"),
            total,
            dead,
            format!(
                "EVERY path glob matches no files: {} — this quirk now warns nobody; \
                 `prime` and the PostToolUse hook inject it for no path",
                dead.join(", ")
            ),
            format!("path globs match no files: {}", dead.join(", ")),
            format!(
                "kanspec quirks --touch {}",
                dead.first().map(String::as_str).unwrap_or_default()
            ),
        );
    }
    for (name, anchor) in &s.git.specs {
        // A spec that lost SOME globs still steers the files it kept; one that lost them
        // ALL steers nothing at all — `prime` injects none of its rules for any path, and
        // the staleness tripwire has nothing to count. That is indistinguishable from
        // having deleted the spec, and a refactor is how it happens: rename the one file a
        // spec names and its rules stop reaching the code they govern, silently.
        let dead = &anchor.dead_globs;
        let total = s.specs.get(name).map_or(0, |sp| sp.fm.code.len());
        rot(
            &mut out,
            format!("spec {name}"),
            total,
            dead,
            format!(
                "EVERY code glob matches no files: {} — this spec now steers nothing, \
                 and `prime` injects none of its rules for any path",
                dead.join(", ")
            ),
            format!(
                "code globs match no files: {} — the staleness tripwire cannot fire for \
                 this spec",
                dead.join(", ")
            ),
            format!("kanspec spec show {name}"),
        );
    }
    out
}

/// One rotted record, graded. Warning while the record is partially moored; Error once it
/// is fully adrift (`dead` covers every glob it has), because at that point CI is the only
/// thing left that will notice.
fn rot(
    out: &mut Vec<Finding>,
    subject: String,
    total: usize,
    dead: &[String],
    all_dead_msg: String,
    some_dead_msg: String,
    fix: String,
) {
    // `cache::load` drops an empty row before it gets here (t-c0f5); this is the same
    // rule stated where the value is read, so a caller with an unloaded `GitState` is
    // still safe.
    if dead.is_empty() {
        return;
    }
    let (severity, message) = if total > 0 && dead.len() >= total {
        (Severity::Error, all_dead_msg)
    } else {
        (Severity::Warning, some_dead_msg)
    };
    out.push(Finding::new("dead_globs", severity, subject, message, fix));
}

/// Every `[pN]` must be typed: `(temp until t-x)` dies when its guard ticket lands,
/// `(promote: decision|spec|quirk)` must become a standing record at close. An untyped one
/// is a close blocker, so it is a warning here and a refusal there.
fn check_untyped_prescriptions(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for p in s.proposals.values() {
        for item in &p.items {
            if !matches!(item.prescription, Some(Prescription::Untyped)) {
                continue;
            }
            out.push(Finding::new(
                "untyped_prescriptions",
                Severity::Warning,
                &p.fm.id,
                format!(
                    "{} is an untyped prescription — say `(temp until t-x)` or `(promote: …)`",
                    item.id
                ),
                format!("kanspec close {}", p.fm.id),
            ));
        }
    }
    out
}

/// Closed proposal BODIES are never loaded — that absence IS invariant 4 — so this can
/// only prove the ledger of a proposal that says `status: closed` while still living
/// outside `proposals/closed/`, which `check_closed_agree` is already shouting about. The
/// full check belongs to the `close` gate, which has the body in hand (reported to F).
fn check_ledger_complete(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for p in s.proposals.values() {
        if p.fm.status != ProposalStatus::Closed {
            continue;
        }
        let open: Vec<String> = p
            .items
            .iter()
            .filter(|i| !derive::dispositioned(&p.fm.ledger, &i.id))
            .map(|i| i.id.to_string())
            .collect();
        if open.is_empty() {
            continue;
        }
        out.push(Finding::new(
            "ledger_complete",
            Severity::Error,
            &p.fm.id,
            format!(
                "closed with {} undispositioned item(s): {}",
                open.len(),
                open.join(", ")
            ),
            format!("kanspec close {}", p.fm.id),
        ));
    }
    out
}

/// `status: closed` and the directory must agree, atomically — `doctor` verifies they do.
/// Only one direction is reachable: a proposal under `proposals/closed/` is never loaded,
/// so a closed-by-location one with an open status is invisible here by construction.
fn check_closed_agree(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for p in s.proposals.values() {
        if p.fm.status != ProposalStatus::Closed {
            continue;
        }
        out.push(Finding {
            fixable: true,
            ..Finding::new(
                "closed_agree",
                Severity::Error,
                &p.fm.id,
                format!(
                    "says `status: closed` but still lives at {} — closed prose must be \
                     unreachable (invariant 4)",
                    p.dir.display()
                ),
                "kanspec doctor --fix".to_string(),
            )
        });
    }
    out
}

/// R-1, made visible. A plan touching a ticket AND a proposal AND `comments.jsonl` is not
/// atomic, so it can be interrupted between renames; what that leaves behind is a
/// dangling cross-file reference. Every one of them is checked here — the `deps:` case is
/// `check_orphan_deps`'s, so it is deliberately not repeated.
fn check_half_applied(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut broke = |subject: String, message: String, fix: String| {
        out.push(Finding::new(
            "half_applied",
            Severity::Error,
            subject,
            message,
            fix,
        ));
    };

    for t in s.tickets.values() {
        let id = &t.fm.id;
        if let Some(p) = &t.fm.proposal {
            // A closed proposal is not loaded but IS remembered by id, so pointing at one
            // is legal — pointing at nothing at all is not.
            if !s.proposals.contains_key(p) && !s.closed_ids.contains(p.as_str()) {
                broke(
                    id.to_string(),
                    format!("`proposal: {p}` names a proposal that does not exist"),
                    format!("kanspec show {id}"),
                );
            }
        }
        if let Some(sp) = &t.fm.spec {
            if !s.specs.contains_key(sp) {
                broke(
                    id.to_string(),
                    format!("`spec: {sp}` names a spec that does not exist"),
                    format!("kanspec spec new \"{sp}\""),
                );
            }
        }
        for (field, other) in [
            ("followup_of", t.fm.followup_of.as_ref()),
            ("discovered_in", t.fm.discovered_in.as_ref()),
        ] {
            if let Some(o) = other {
                if !s.tickets.contains_key(o) {
                    broke(
                        id.to_string(),
                        format!("`{field}: {o}` names a ticket that does not exist"),
                        format!("kanspec show {id}"),
                    );
                }
            }
        }
    }

    for q in s.quirks.values() {
        for (field, t) in [
            ("source", q.fm.source.as_ref()),
            ("fixed_by", q.fm.fixed_by.as_ref()),
        ] {
            if let Some(t) = t {
                if !s.tickets.contains_key(t) {
                    broke(
                        q.fm.id.to_string(),
                        format!("`{field}: {t}` names a ticket that does not exist"),
                        "kanspec quirks".to_string(),
                    );
                }
            }
        }
        // A quirk retired without evidence is the mirror case: the status moved, the
        // link did not.
        if q.fm.status == QuirkStatus::Fixed && q.fm.fixed_by.is_none() {
            broke(
                q.fm.id.to_string(),
                "is `status: fixed` with no `fixed_by:` — a quirk is retired only by evidence"
                    .to_string(),
                format!("kanspec quirk fix {} --by t-xxxx", q.fm.id),
            );
        }
    }

    for d in s.decisions.values() {
        for (field, other) in [
            ("supersedes", d.fm.supersedes.as_ref()),
            ("superseded_by", d.fm.superseded_by.as_ref()),
        ] {
            let Some(o) = other else { continue };
            let Some(target) = s.decisions.get(o) else {
                broke(
                    d.fm.id.to_string(),
                    format!("`{field}: {o}` names a decision that does not exist"),
                    "kanspec rules".to_string(),
                );
                continue;
            };
            // `supersede` writes BOTH links in one plan across TWO files. Half of it
            // landing is exactly R-1.
            let back = match field {
                "supersedes" => target.fm.superseded_by.as_ref(),
                _ => target.fm.supersedes.as_ref(),
            };
            if back != Some(&d.fm.id) {
                broke(
                    d.fm.id.to_string(),
                    format!("`{field}: {o}` is not answered by a back-link on {o}"),
                    format!("kanspec why {}", d.fm.id),
                );
            }
        }
    }
    out
}

/// R-7. Same-kind duplicates cannot survive the store — it keys by id and refuses a file
/// whose name and `id:` disagree — so what is left, and what a bad merge actually
/// produces, is an id claimed on BOTH sides of the closed/open proposal boundary.
fn check_duplicate_ids(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for p in s.proposals.values() {
        if !s.closed_ids.contains(p.fm.id.as_str()) {
            continue;
        }
        out.push(Finding::new(
            "duplicate_ids",
            Severity::Error,
            &p.fm.id,
            format!(
                "{} is claimed by an open proposal at {} AND by one under proposals/closed/ — \
                 4 hex is 65,536 ids and the exclusion guarantee is per-machine (R-7)",
                p.fm.id,
                p.dir.display()
            ),
            format!("kanspec show {}", p.fm.id),
        ));
    }
    out
}

/// Accepted decision bodies are immutable: the only legal mutations are status flips and
/// back-links. Proving the BODY did not change needs a git diff of the file against its
/// last commit, which a `fn(&Snapshot)` cannot do (see the module header). What is
/// provable here is the half that lives in the record itself: a decision that was
/// superseded or revoked in one field while its `status:` still says `accepted`.
fn check_immutable_decisions(s: &Snapshot) -> Vec<Finding> {
    let mut out = Vec::new();
    for d in s.decisions.values() {
        if d.fm.status != DecisionStatus::Accepted {
            continue;
        }
        if let Some(by) = &d.fm.superseded_by {
            out.push(Finding::new(
                "immutable_decisions",
                Severity::Error,
                &d.fm.id,
                format!(
                    "is still `status: accepted` while `superseded_by: {by}` — it is binding \
                     agents right now"
                ),
                format!("kanspec supersede {} --with \"...\"", d.fm.id),
            ));
        }
    }
    out
}

// ── the fixers ───────────────────────────────────────────────────────────────

/// Moves the directory so `status: closed` and the location agree. The rename goes through
/// `Store::transact` like every other write, so `--fix` is not a second writer.
fn fix_closed_agree(s: &Snapshot, f: &Finding, p: &mut Plan) -> Result<()> {
    let Ok(id) = ProposalId::parse(&f.subject) else {
        return Ok(());
    };
    let Some(prop) = s.proposals.get(&id) else {
        return Ok(());
    };
    let (Some(parent), Some(name)) = (prop.dir.parent(), prop.dir.file_name()) else {
        return Ok(());
    };
    p.push(Op::MoveDir {
        from: prop.dir.clone(),
        to: parent.join("closed").join(name),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use chrono::{DateTime, TimeZone, Utc};

    use crate::config::Config;
    use crate::ids::{DecisionId, QuirkId, SpecName, TicketId};
    use crate::logentry::LogEntry;
    use crate::model::{
        Decision, DecisionFm, Quirk, QuirkFm, Severity as QSeverity, Spec, SpecFm, Ticket, TicketFm,
    };
    use crate::transitions::{State, Verb};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap()
    }

    fn snap() -> Snapshot {
        Snapshot::empty(Config::default(), now())
    }

    fn tid(s: &str) -> TicketId {
        TicketId::parse(s).unwrap()
    }

    fn ticket(id: &str, state: State) -> Ticket {
        Ticket {
            fm: TicketFm {
                id: tid(id),
                title: "t".into(),
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
                created: now() - chrono::Duration::days(2),
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(format!(".kanspec/tickets/{id}.md")),
            body: String::new(),
            steps: Vec::new(),
            log: Vec::new(),
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    fn entry(mins: i64, verb: Verb, state: State) -> LogEntry {
        LogEntry {
            at: now() - chrono::Duration::minutes(mins),
            state,
            actor: "trevor".into(),
            verb,
            note: None,
        }
    }

    /// `new -> start -> ship`, the legal trail every other test starts from.
    fn legal(id: &str) -> Ticket {
        let mut t = ticket(id, State::Review);
        t.log = vec![
            entry(30, Verb::New, State::Todo),
            entry(20, Verb::Start, State::Doing),
            entry(10, Verb::Ship, State::Review),
        ];
        t
    }

    fn put(s: &mut Snapshot, t: Ticket) {
        s.tickets.insert(t.fm.id.clone(), t);
    }

    fn found(all: Vec<Finding>, check: &'static str) -> Vec<Finding> {
        all.into_iter().filter(|f| f.check == check).collect()
    }

    #[test]
    fn a_legal_repo_has_no_findings_at_all() {
        let mut s = snap();
        put(&mut s, legal("t-0001"));
        assert!(run_all(&s).is_empty(), "{:#?}", run_all(&s));
    }

    #[test]
    fn every_finding_names_a_fix_and_a_registered_check() {
        let mut s = snap();
        // one ticket that breaks as much as possible at once
        let mut t = ticket("t-0001", State::Done);
        t.fm.deps = vec![tid("t-9999")];
        t.fm.spec = Some(SpecName::parse("gone").unwrap());
        t.fm.extra
            .insert("merged".into(), serde_yaml_ng::Value::Bool(true));
        t.fm.extra
            .insert("a key".into(), serde_yaml_ng::Value::Bool(true));
        put(&mut s, t);

        let findings = run_all(&s);
        assert!(!findings.is_empty());
        for f in &findings {
            assert!(!f.fix.is_empty(), "invariant 9: {f:?}");
            assert!(
                CHECKS.iter().any(|c| c.id == f.check),
                "{} is not in the registry",
                f.check
            );
        }
        // errors before warnings
        let sevs: Vec<Severity> = findings.iter().map(|f| f.severity).collect();
        let mut sorted = sevs.clone();
        sorted.sort();
        assert_eq!(sevs, sorted);
    }

    #[test]
    fn a_hand_edited_state_has_no_log_trail() {
        let mut s = snap();
        // `sed -i 's/state: review/state: done/'` — R-2, exactly
        let mut t = legal("t-0001");
        t.fm.state = State::Done;
        put(&mut s, t);
        let f = found(run_all(&s), "log_trail");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error);
        assert!(f[0].message.contains("replays to"), "{}", f[0].message);
        // The FIRST prescription is the honest one — put the frontmatter back to the state
        // the log actually reached. Prescribing `repair` here made the warning its own
        // laundry: the command doctor printed closed the ticket and cleared the finding.
        assert_eq!(
            f[0].fix,
            "edit .kanspec/tickets/t-0001.md and set `state: review`"
        );
        assert!(
            !f[0].fix.contains("repair"),
            "the attestation is the last resort, never the prescription: {}",
            f[0].fix
        );
        // …and it stays discoverable, described as what it is.
        assert!(
            f[0].message.contains("kanspec repair t-0001 --why")
                && f[0].message.contains("badged attested"),
            "{}",
            f[0].message
        );
        assert!(!f[0].fixable, "recovery is a human attestation (D-12)");
    }

    /// The hole this check closes: a `repair` line attesting `done` on a ticket whose
    /// `## Log` never closed it replays perfectly, so `log_trail` is silent — and before
    /// this check the repo reported a clean bill of health with an unmerged ticket shut
    /// inside it.
    #[test]
    fn a_close_that_was_only_attested_is_an_error_not_a_clean_bill_of_health() {
        let mut s = snap();
        let mut t = legal("t-0001");
        t.fm.state = State::Done;
        t.log.push(entry(5, Verb::Repair, State::Done));
        put(&mut s, t);

        let all = run_all(&s);
        assert!(
            found(all.clone(), "log_trail").is_empty(),
            "the attested reset replays clean — which is exactly why it needed its own check"
        );
        let f = found(all, "attested_state");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error, "this one fails CI");
        assert!(
            f[0].message.contains("vouched for, never proven"),
            "{}",
            f[0].message
        );
        assert_eq!(f[0].fix, "kanspec scan --confirm t-0001 --why \"...\"");
    }

    /// The legitimate half, and why the check is not simply an error: a rescued trail whose
    /// close the log still carries, and an attested `dropped` (a drop is an act, not a
    /// merge), are both fine — they are recorded, not condemned.
    #[test]
    fn an_attested_state_the_record_supports_is_a_warning_that_never_ages_into_a_fact() {
        let mut s = snap();
        let mut done = legal("t-0001");
        done.fm.state = State::Done;
        done.log.push(entry(9, Verb::Done, State::Done));
        done.log.push(entry(5, Verb::Repair, State::Done));
        put(&mut s, done);

        let mut dropped = ticket("t-0002", State::Dropped);
        dropped.log = vec![entry(20, Verb::Repair, State::Dropped)];
        put(&mut s, dropped);

        let f = found(run_all(&s), "attested_state");
        assert_eq!(f.len(), 2, "{f:#?}");
        assert!(f.iter().all(|f| f.severity == Severity::Warning), "{f:#?}");
        assert!(f.iter().all(|f| f.message.contains("by attestation")));
    }

    /// The attestation is spent the moment an ordinary verb moves the ticket on: a rescue
    /// back to `todo` that was then started, shipped and closed by the gate stands on its
    /// own trail again and carries no badge.
    #[test]
    fn an_attestation_a_later_verb_moved_past_is_not_a_finding() {
        let mut s = snap();
        let mut t = ticket("t-0001", State::Review);
        t.log = vec![
            entry(50, Verb::Repair, State::Todo),
            entry(40, Verb::Start, State::Doing),
            entry(30, Verb::Ship, State::Review),
        ];
        put(&mut s, t);
        assert!(found(run_all(&s), "attested_state").is_empty());
    }

    /// A ticket the gate really closed: `start` recorded the branch, `ship` the head, and
    /// `plan_done` the commit it was granted against. The corroboration is git-tracked, so
    /// it does not depend on the disposable cache being there.
    fn closed_by_the_gate(id: &str) -> Ticket {
        let mut t = legal(id);
        t.fm.state = State::Done;
        t.fm.branch = Some(format!("ks/{id}"));
        t.fm.head = Some("3f2a19c7d4b6e8a0c1f5920b7e6d4a3c8b1f0e29".into());
        t.log.push(LogEntry {
            at: now() - chrono::Duration::minutes(5),
            state: State::Done,
            actor: "trevor".into(),
            verb: Verb::Done,
            note: Some("in main a1b9c3d via gh-pr #142".into()),
        });
        t
    }

    /// THE two-edit forgery: `state: done` in the frontmatter plus ONE fabricated `done`
    /// line. It replays perfectly and carries no `repair`, so every other check is silent.
    fn forged_close(id: &str) -> Ticket {
        let mut t = closed_by_the_gate(id);
        t.log.last_mut().unwrap().note = None;
        t
    }

    #[test]
    fn a_close_nothing_outside_the_log_stands_behind_is_an_error() {
        let mut s = snap();
        put(&mut s, forged_close("t-0001"));

        let all = run_all(&s);
        assert!(
            found(all.clone(), "log_trail").is_empty(),
            "the forged trail replays clean — which is exactly why it needed its own check"
        );
        assert!(
            found(all.clone(), "attested_state").is_empty(),
            "no `repair` verb, so the attestation check never sees it"
        );
        let f = found(all, "unproven_close");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Error, "this one fails CI");
        assert!(
            f[0].message.contains("nothing outside its own ## Log"),
            "{}",
            f[0].message
        );
        assert_eq!(f[0].fix, "kanspec scan --explain t-0001");
        assert!(!f[0].fixable);
    }

    /// Every legitimate close, one per row, and every one of them silent. A check that
    /// fires on the daily loop is a check nobody leaves switched on.
    #[test]
    fn every_corroborated_close_is_silent() {
        // 1. the gate's own record of the commit it granted the close against
        let mut s = snap();
        put(&mut s, closed_by_the_gate("t-0001"));
        assert!(found(run_all(&s), "unproven_close").is_empty());

        // 2. `scan --confirm` — the recorded human override (D-11)
        let mut s = snap();
        let mut t = forged_close("t-0002");
        t.log.insert(
            3,
            LogEntry {
                at: now() - chrono::Duration::minutes(6),
                state: State::Review,
                actor: "trevor".into(),
                verb: Verb::Confirm,
                note: Some("in main 3f2a19c7 — squash merged by hand, verified".into()),
            },
        );
        put(&mut s, t);
        assert!(found(run_all(&s), "unproven_close").is_empty());

        // 3. the durable `--no-code` waiver — prose under `## Log`, signed and dated.
        // No `ship` in the trail: the gate refuses `--no-code` on work shipped for review,
        // so this is the only shape a waiver the gate actually granted can have.
        let mut s = snap();
        let mut t = forged_close("t-0003");
        t.log.retain(|e| e.verb != Verb::Ship);
        t.body = "Body.\n\n## Log\n  no-code waiver by trevor at 2026-08-31T11:00Z: docs only\n"
            .to_string();
        put(&mut s, t);
        assert!(found(run_all(&s), "unproven_close").is_empty());

        // 3b. …and the same waiver line pasted onto a trail that WAS shipped corroborates
        // nothing: the gate would have refused that pair, so the log contradicts itself.
        let mut s = snap();
        let mut t = forged_close("t-0013");
        t.body = "Body.\n\n## Log\n  no-code waiver by trevor at 2026-08-31T11:00Z: docs only\n"
            .to_string();
        put(&mut s, t);
        assert_eq!(found(run_all(&s), "unproven_close").len(), 1);

        // 4. a ladder run that actually put it in main — the cache, computed from git
        let mut s = snap();
        put(&mut s, forged_close("t-0004"));
        s.git.tickets.insert(
            tid("t-0004"),
            crate::cache::MergeFact {
                status: crate::cache::MergeStatus::Merged,
                sha: Some("a1b9c3d".into()),
                method: crate::git::Method::Ancestry,
                pr: None,
                why: None,
                checked_at: now() - chrono::Duration::hours(1),
                changed: vec![],
            },
        );
        assert!(found(run_all(&s), "unproven_close").is_empty());

        // 5. a human attestation — `attested_state` owns that one, and says so alone
        let mut s = snap();
        let mut t = forged_close("t-0005");
        t.log.push(entry(1, Verb::Repair, State::Done));
        put(&mut s, t);
        let all = run_all(&s);
        assert_eq!(found(all.clone(), "attested_state").len(), 1);
        assert!(
            found(all, "unproven_close").is_empty(),
            "one finding per break, in one voice"
        );
    }

    /// The three deliberate silences. Each one is a case where firing would be noise, not
    /// detection — and the third is the honest edge of what corroboration can see.
    #[test]
    fn the_check_stands_down_where_it_has_nothing_to_corroborate_against() {
        // a drop is an act, not a merge
        let mut s = snap();
        let mut dropped = forged_close("t-0001");
        dropped.fm.state = State::Dropped;
        dropped.log.last_mut().unwrap().state = State::Dropped;
        dropped.log.last_mut().unwrap().verb = Verb::Drop;
        put(&mut s, dropped);
        assert!(found(run_all(&s), "unproven_close").is_empty());

        // a broken trail belongs to `log_trail`, and only to it
        let mut s = snap();
        let mut t = legal("t-0002");
        t.fm.state = State::Done;
        t.fm.branch = Some("ks/t-0002".into());
        put(&mut s, t);
        let all = run_all(&s);
        assert_eq!(found(all.clone(), "log_trail").len(), 1);
        assert!(found(all, "unproven_close").is_empty());

        // no branch, no head, no PR: nothing for git to place
        let mut s = snap();
        let mut t = forged_close("t-0003");
        t.fm.branch = None;
        t.fm.head = None;
        t.fm.pr = None;
        put(&mut s, t);
        assert!(found(run_all(&s), "unproven_close").is_empty());
        // …and a bare `pr:` is enough to bring it back, because rung 2 answers from one
        let mut s = snap();
        let mut t = forged_close("t-0004");
        t.fm.branch = None;
        t.fm.head = None;
        t.fm.pr = Some(142);
        put(&mut s, t);
        assert_eq!(found(run_all(&s), "unproven_close").len(), 1);
    }

    #[test]
    fn a_doctored_log_line_is_caught_even_when_the_frontmatter_agrees() {
        let mut s = snap();
        let mut t = ticket("t-0001", State::Done);
        // the log CLAIMS `ship` reached `done`
        t.log = vec![
            entry(30, Verb::New, State::Todo),
            entry(20, Verb::Start, State::Doing),
            entry(10, Verb::Ship, State::Done),
        ];
        put(&mut s, t);
        let f = found(run_all(&s), "log_trail");
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("records `done`"), "{}", f[0].message);
    }

    #[test]
    fn a_reserved_key_in_a_file_is_the_read_side_of_invariant_1() {
        let mut s = snap();
        let mut t = legal("t-0001");
        t.fm.extra
            .insert("merged".into(), serde_yaml_ng::Value::Bool(true));
        t.fm.extra
            .insert("checked_at".into(), serde_yaml_ng::Value::Bool(true));
        t.fm.extra
            .insert("future_knob".into(), serde_yaml_ng::Value::Bool(true));
        put(&mut s, t);

        let f = found(run_all(&s), "reserved_keys");
        assert_eq!(f.len(), 1, "one finding per entity, listing every key");
        assert!(f[0].message.contains("`merged`") && f[0].message.contains("`checked_at`"));
        assert!(
            !f[0].message.contains("future_knob"),
            "a key a NEWER kanspec wrote is not a derived fact"
        );
        assert!(f[0].fix.contains(".kanspec/tickets/t-0001.md"));
    }

    /// Severity follows the damage. A spec that lost SOME globs still steers what it kept;
    /// one that lost them ALL steers nothing, `prime` injects none of its rules for any
    /// path, and CI is the only thing left that will notice — which it cannot do at
    /// exit 0. A rename is how a spec goes fully adrift, so this is not hypothetical.
    #[test]
    fn a_spec_that_lost_every_glob_fails_ci_and_one_that_lost_some_does_not() {
        let mk = |name: &str, code: Vec<String>| Spec {
            name: SpecName::parse(name).unwrap(),
            fm: SpecFm {
                feature: "F".into(),
                code,
                stale_ack: None,
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(format!(".kanspec/specs/{name}.md")),
            body: String::new(),
            rules: vec![],
        };
        let mut s = snap();
        // `partial` keeps one live glob; `adrift` keeps none.
        for (name, code, dead) in [
            (
                "partial",
                vec!["a/**".to_string(), "b/**".to_string()],
                vec!["a/**"],
            ),
            ("adrift", vec!["c/**".to_string()], vec!["c/**"]),
        ] {
            let sp = mk(name, code);
            let n = sp.name.clone();
            s.specs.insert(n.clone(), sp);
            s.git.specs.insert(
                n,
                crate::cache::SpecAnchor {
                    last_edit_sha: None,
                    last_edit_at: None,
                    merges_since: 0,
                    dead_globs: dead.iter().map(ToString::to_string).collect(),
                },
            );
        }
        let f = check_dead_globs(&s);
        let sev = |name: &str| {
            f.iter()
                .find(|x| x.subject.contains(name))
                .unwrap_or_else(|| panic!("no finding for {name}"))
                .severity
        };
        assert_eq!(sev("partial"), Severity::Warning);
        assert_eq!(sev("adrift"), Severity::Error, "this one must fail CI");
        // …and it says WHICH failure it is, because the two need different responses.
        assert!(f
            .iter()
            .find(|x| x.subject.contains("adrift"))
            .unwrap()
            .message
            .contains("steers nothing"));
    }

    /// The same grading for the other two path-scoped records. A quirk whose every path
    /// rotted is the worse case — a landmine that stopped warning — and a decision whose
    /// every scope glob rotted is a body no session will ever be shown.
    #[test]
    fn a_decision_or_quirk_that_lost_every_glob_fails_ci_too() {
        use crate::ids::{DecisionId, QuirkId};
        use crate::model::{Decision, DecisionFm, Quirk, QuirkFm, Severity as Sev};
        use chrono::NaiveDate;
        let mut s = snap();
        let scope = vec!["src/old/**".to_string(), "src/kept/**".to_string()];
        let d = Decision {
            fm: DecisionFm {
                id: DecisionId::parse("D-1a2b").unwrap(),
                title: "T".into(),
                status: DecisionStatus::Accepted,
                date: NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
                source: None,
                scope: scope.clone(),
                supersedes: None,
                superseded_by: None,
                extra: BTreeMap::new(),
            },
            path: PathBuf::from("x"),
            body: String::new(),
            scope,
        };
        s.decisions.insert(d.fm.id.clone(), d);
        s.git.decision_dead_globs.insert(
            DecisionId::parse("D-1a2b").unwrap(),
            vec!["src/old/**".into()],
        );
        let q = Quirk {
            fm: QuirkFm {
                id: QuirkId::parse("q-3c4d").unwrap(),
                title: "Landmine".into(),
                paths: vec!["src/gone/**".into()],
                severity: Sev::Landmine,
                status: QuirkStatus::Active,
                source: None,
                fixed_by: None,
                extra: BTreeMap::new(),
            },
            path: PathBuf::from("x"),
            body: String::new(),
        };
        s.quirks.insert(q.fm.id.clone(), q);
        s.git.quirk_dead_globs.insert(
            QuirkId::parse("q-3c4d").unwrap(),
            vec!["src/gone/**".into()],
        );

        let f = check_dead_globs(&s);
        let by = |needle: &str| {
            f.iter()
                .find(|x| x.subject.contains(needle))
                .unwrap_or_else(|| panic!("no finding for {needle}"))
        };
        assert_eq!(
            by("D-1a2b").severity,
            Severity::Warning,
            "one live glob remains"
        );
        assert_eq!(
            by("q-3c4d").severity,
            Severity::Error,
            "warns nobody: CI fails"
        );
        assert!(by("q-3c4d").message.contains("warns nobody"));
        assert!(by("q-3c4d").fix.contains("quirks --touch"));

        // Nothing recorded, nothing found — the cache carries only rotted entries.
        s.git.decision_dead_globs.clear();
        s.git.quirk_dead_globs.clear();
        assert!(check_dead_globs(&s).is_empty());
    }

    #[test]
    fn every_entity_kind_is_scanned_for_reserved_keys() {
        let mut s = snap();
        let mut spec = Spec {
            name: SpecName::parse("auth").unwrap(),
            fm: SpecFm {
                feature: "Login".into(),
                code: vec![],
                stale_ack: None,
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(".kanspec/specs/auth.md"),
            body: String::new(),
            rules: vec![],
        };
        spec.fm
            .extra
            .insert("stale".into(), serde_yaml_ng::Value::Bool(false));
        s.specs.insert(spec.name.clone(), spec);

        let mut q = Quirk {
            fm: QuirkFm {
                id: QuirkId::parse("q-11ba").unwrap(),
                title: "q".into(),
                paths: vec![],
                severity: QSeverity::Landmine,
                status: QuirkStatus::Active,
                source: None,
                fixed_by: None,
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(".kanspec/quirks/q-11ba.md"),
            body: String::new(),
        };
        q.fm.extra
            .insert("ci".into(), serde_yaml_ng::Value::Bool(true));
        s.quirks.insert(q.fm.id.clone(), q);

        let f = found(run_all(&s), "reserved_keys");
        assert_eq!(f.len(), 2, "{f:#?}");
        assert!(f.iter().any(|f| f.subject == "spec auth"));
        assert!(f.iter().any(|f| f.subject == "q-11ba"));
    }

    #[test]
    fn an_unindexable_key_is_caught_before_it_duplicates_itself() {
        let mut s = snap();
        let mut t = legal("t-0001");
        t.fm.extra
            .insert("a key".into(), serde_yaml_ng::Value::Bool(true));
        t.fm.extra.insert(
            "notes".into(),
            serde_yaml_ng::Value::String("two\nlines".into()),
        );
        put(&mut s, t);
        let f = found(run_all(&s), "frontmatter_writable");
        assert_eq!(f.len(), 2);
        assert!(f.iter().any(|f| f.severity == Severity::Error));
        assert!(f.iter().any(|f| f.severity == Severity::Warning));
    }

    #[test]
    fn indexable_agrees_with_fms_own_key_grammar() {
        // `fm.rs` is S1's file, so the grammar is mirrored rather than shared — this is
        // the test that keeps the mirror honest.
        for (key, ok) in [
            ("id", true),
            ("followup_of", true),
            ("severity_hint_v2", true),
            ("a.b/c-d", true),
            ("a key", false),
            ("\"quoted\"", false),
            ("", false),
            ("emoji✨", false),
        ] {
            assert_eq!(indexable(key), ok, "{key:?}");
            if !key.is_empty() {
                let text = format!("{key}: 1\n");
                let seen = crate::fm::index(&text).into_iter().any(|k| k.key == key);
                assert_eq!(seen, ok, "fm::index disagrees about {key:?}");
            }
        }
    }

    #[test]
    fn an_orphan_dep_is_an_error_and_a_dropped_one_is_a_warning() {
        let mut s = snap();
        put(&mut s, ticket("t-00dd", State::Dropped));
        let mut t = legal("t-0001");
        t.fm.deps = vec![tid("t-9999"), tid("t-00dd")];
        put(&mut s, t);

        let f = found(run_all(&s), "orphan_deps");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].severity, Severity::Error);
        assert!(f[0].message.contains("t-9999"));
        assert_eq!(f[1].severity, Severity::Warning);
        assert!(f[1].message.contains("D-17"));
    }

    #[test]
    fn a_cycle_is_one_error_naming_the_whole_loop() {
        let mut s = snap();
        for (id, dep) in [("t-000a", "t-000b"), ("t-000b", "t-000a")] {
            let mut t = legal(id);
            t.fm.deps = vec![tid(dep)];
            put(&mut s, t);
        }
        let f = found(run_all(&s), "dep_cycles");
        assert_eq!(f.len(), 1);
        assert!(
            f[0].message.contains("t-000a → t-000b → t-000a"),
            "{}",
            f[0].message
        );
    }

    #[test]
    fn a_broken_cross_file_link_is_a_half_applied_plan() {
        let mut s = snap();
        let mut t = legal("t-0001");
        t.fm.discovered_in = Some(tid("t-9999"));
        put(&mut s, t);

        let mut d = Decision {
            fm: DecisionFm {
                id: DecisionId::parse("D-8c1a").unwrap(),
                title: "d".into(),
                status: DecisionStatus::Superseded,
                date: chrono::NaiveDate::from_ymd_opt(2026, 9, 2).unwrap(),
                source: None,
                scope: vec![],
                supersedes: None,
                superseded_by: Some(DecisionId::parse("D-9999").unwrap()),
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(".kanspec/decisions/D-8c1a.md"),
            body: String::new(),
            scope: vec![],
        };
        d.fm.superseded_by = Some(DecisionId::parse("D-9999").unwrap());
        s.decisions.insert(d.fm.id.clone(), d);

        let f = found(run_all(&s), "half_applied");
        assert_eq!(f.len(), 2, "{f:#?}");
        assert!(f.iter().any(|f| f.message.contains("discovered_in")));
        assert!(f.iter().any(|f| f.message.contains("superseded_by")));
    }

    #[test]
    fn an_accepted_decision_that_was_superseded_is_still_binding_agents() {
        let mut s = snap();
        for (id, status, other) in [
            ("D-000a", DecisionStatus::Accepted, "D-000b"),
            ("D-000b", DecisionStatus::Accepted, "D-000a"),
        ] {
            let d = Decision {
                fm: DecisionFm {
                    id: DecisionId::parse(id).unwrap(),
                    title: "d".into(),
                    status,
                    date: chrono::NaiveDate::from_ymd_opt(2026, 9, 2).unwrap(),
                    source: None,
                    scope: vec![],
                    supersedes: (id == "D-000b").then(|| DecisionId::parse(other).unwrap()),
                    superseded_by: (id == "D-000a").then(|| DecisionId::parse(other).unwrap()),
                    extra: BTreeMap::new(),
                },
                path: PathBuf::from(format!(".kanspec/decisions/{id}.md")),
                body: String::new(),
                scope: vec![],
            };
            s.decisions.insert(d.fm.id.clone(), d);
        }
        let all = run_all(&s);
        assert!(
            found(all.clone(), "half_applied").is_empty(),
            "the back-links agree"
        );
        let f = found(all.clone(), "immutable_decisions");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].subject, "D-000a");
    }

    #[test]
    fn a_quirk_retired_without_evidence_is_half_applied() {
        let mut s = snap();
        let q = Quirk {
            fm: QuirkFm {
                id: QuirkId::parse("q-11ba").unwrap(),
                title: "q".into(),
                paths: vec![],
                severity: QSeverity::Landmine,
                status: QuirkStatus::Fixed,
                source: None,
                fixed_by: None,
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(".kanspec/quirks/q-11ba.md"),
            body: String::new(),
        };
        s.quirks.insert(q.fm.id.clone(), q);
        let f = found(run_all(&s), "half_applied");
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("retired only by evidence"));
    }

    #[test]
    fn plan_fixes_only_touches_fixable_findings() {
        let s = snap();
        let findings = vec![
            Finding {
                check: "log_trail",
                severity: Severity::Error,
                subject: "t-0001".into(),
                message: "m".into(),
                fix: "kanspec repair t-0001".into(),
                fixable: false,
            },
            Finding {
                check: "closed_agree",
                severity: Severity::Error,
                subject: "p-0001".into(),
                message: "m".into(),
                fix: "kanspec doctor --fix".into(),
                fixable: true,
            },
        ];
        // Neither entity exists in this snapshot, so the plan is empty rather than wrong.
        let plan = plan_fixes(&s, &findings).unwrap();
        assert!(plan.is_empty());
    }

    #[test]
    fn the_registry_ids_are_unique_and_every_entry_is_reachable() {
        let mut ids: Vec<&str> = CHECKS.iter().map(|c| c.id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate check id");
        assert_eq!(n, 15);
        // every check runs against an empty snapshot without panicking
        let s = snap();
        for c in CHECKS {
            assert!((c.run)(&s).is_empty(), "{} fires on an empty repo", c.id);
        }
    }
}
