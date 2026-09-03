//! The typed edit vocabulary. A planner returns a [`Plan`]; `Store::transact` validates
//! and applies it. Nothing else in the crate describes an edit.
//!
//! Two things make invariant 1 structural rather than aspirational:
//! [`Op::Transition`] computes its own destination through `transitions::next` (so a
//! caller cannot pass a state the table never produces) and emits the frontmatter delta
//! AND the `## Log` line in ONE staged write to ONE file (so omitting the log append is
//! *unavailable*, not merely rejected); and every non-state write names a closed
//! [`Key`], so a derived key cannot be NAMED here, let alone written.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cache::GitState;
use crate::ctx::Actor;
use crate::error::{Fix, GateCode, KsError, Result};
use crate::fm::Yv;
use crate::ids::{DecisionId, ProposalId, QuirkId, SpecName, TicketId};
use crate::keys::{Key, TicketKey};
use crate::model::Snapshot;
use crate::paths::Layout;
use crate::scan::ScanToken;
use crate::transitions::Verb;
use crate::{fix, fixes};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum EntityRef {
    Ticket(TicketId),
    Proposal(ProposalId),
    Spec(SpecName),
    Decision(DecisionId),
    Quirk(QuirkId),
}

impl EntityRef {
    pub fn noun(&self) -> &'static str {
        match self {
            EntityRef::Ticket(_) => "ticket",
            EntityRef::Proposal(_) => "proposal",
            EntityRef::Spec(_) => "spec",
            EntityRef::Decision(_) => "decision",
            EntityRef::Quirk(_) => "quirk",
        }
    }
    pub fn id(&self) -> String {
        match self {
            EntityRef::Ticket(i) => i.to_string(),
            EntityRef::Proposal(i) => i.to_string(),
            EntityRef::Spec(n) => n.to_string(),
            EntityRef::Decision(i) => i.to_string(),
            EntityRef::Quirk(i) => i.to_string(),
        }
    }
    pub fn exists_in(&self, snap: &Snapshot) -> bool {
        match self {
            EntityRef::Ticket(i) => snap.tickets.contains_key(i),
            EntityRef::Proposal(i) => snap.proposals.contains_key(i),
            EntityRef::Spec(n) => snap.specs.contains_key(n),
            EntityRef::Decision(i) => snap.decisions.contains_key(i),
            EntityRef::Quirk(i) => snap.quirks.contains_key(i),
        }
    }
}

impl std::fmt::Display for EntityRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.noun(), self.id())
    }
}

#[derive(Debug, PartialEq)]
pub enum Op {
    /// The ONLY op that can touch `state:`.
    Transition {
        id: TicketId,
        verb: Verb,
        actor: Actor,
        at: DateTime<Utc>,
        /// the `## Log` note — `Ctx::invocation()` plus whatever the verb wants recorded
        detail: String,
        also: Vec<(TicketKey, Yv)>,
    },
    /// Non-state fields on any entity.
    SetFields {
        entity: EntityRef,
        sets: Vec<(Key, Yv)>,
    },
    /// errors if the file exists
    CreateEntity { entity: EntityRef, contents: String },
    AppendSection {
        entity: EntityRef,
        heading: &'static str,
        line: String,
    },
    /// `done` triage: actually-done
    MarkSteps {
        id: TicketId,
        checks: Vec<(usize, bool)>,
    },
    /// `propose`: create `proposals/<id>-<slug>/proposal.md`.
    ///
    /// Separate from [`Op::CreateEntity`] because a proposal is the one entity whose
    /// DIRECTORY NAME carries human-readable text. `CreateEntity` resolves its path through
    /// `Layout::path_for`, which has only the id, so it can only ever produce the
    /// un-slugged `proposals/p-7de2/` — fine for the machine (`dir_proposal_id` reads both
    /// forms) and useless for the teammate browsing the repo, who is exactly who the slug
    /// is for.
    ///
    /// It errors if the directory exists, like `CreateEntity`: minting is the only way a
    /// proposal id comes into being, and clobbering one would take its comment log with it.
    CreateProposal {
        id: ProposalId,
        /// `ids::slug(title)` — derived, never free text
        slug: String,
        contents: String,
    },
    /// `rules --adopt`: stamp the adoption token onto a pre-kanspec rule bullet.
    ///
    /// The ONLY op that rewrites a line inside an entity body, and deliberately the
    /// narrowest one that can be: it appends one fixed token to the `- [anchor]` bullet the
    /// snapshot parsed at `line`, in a spec, and can reach nothing else. `SetFields` is
    /// frontmatter-only and `AppendSection` appends, so a migrated corpus had no way to
    /// answer `rules --audit` before this existed.
    ///
    /// `line` is safe to carry across a multi-stamp plan because stamping never changes the
    /// body's line count, and the planner runs against the snapshot reloaded INSIDE the
    /// lock; `fm::stamp_rule` re-verifies the anchor anyway.
    StampRule {
        spec: SpecName,
        anchor: String,
        line: usize,
    },
    /// `comments.jsonl` (v0.2)
    AppendJsonl { path: PathBuf, line: String },
    /// `close`: -> `proposals/closed/`
    MoveDir { from: PathBuf, to: PathBuf },
    /// Generated bytes at a path the tracker does not own: the `KANSPEC-*.md` projections
    /// (D-20) and, since round D, `board --export <file>`.
    ///
    /// It is the only op that writes arbitrary bytes to an arbitrary path, so it is also
    /// the only one whose `entity()` is `None` by design rather than by omission — there is
    /// no entity to name. Two properties keep that safe and both are load-bearing:
    /// `writes_tracked_file()` is **false** for it (see below), and `Plan::validate` permits
    /// it precisely because it can never touch `state:` or any derived key — `Key` cannot
    /// name one, and this op does not go through `Key` at all.
    ///
    /// It is NOT a general file writer: a caller that wants to write an *entity* uses
    /// `CreateEntity` or `SetFields`, which are the ops the seals apply to.
    WriteGenerated { path: PathBuf, contents: String },
    /// `cache/gitstate.json`. `ScanToken` is minted solely by `scan::scan_all`, so
    /// "written by scan and nothing else" is a type fact — and the write happens INSIDE
    /// the lock.
    WriteGitState { token: ScanToken, state: GitState },
}

impl Op {
    /// Does this step change a file the `sync = "commit"` auto-commit would actually
    /// commit — i.e. a tracked file **under `.kanspec/`**?
    ///
    /// `WriteGitState` does not: `cache/` is gitignored, disposable, and rebuilt from
    /// nothing by the next `scan`. `Store::transact` reads this to decide whether
    /// `sync = "commit"` has anything to commit — without it, a `scan` (whose entire plan is
    /// one cache write) runs `git add -A -- .kanspec/**` and sweeps whatever tracker edits
    /// happened to be pending into a commit labelled after the scan, and the `post-merge`
    /// hook's `kanspec scan --quiet` reaches for git's index in the middle of a merge.
    ///
    /// ROUND-4 CORRECTION (S6, reported as a request to F). `WriteGenerated` does not
    /// either, for the same reason arrived at from the other side: it writes
    /// `KANSPEC-FEATURES.md` / `KANSPEC-ARCHITECTURE.md` at the **repo root**, and
    /// `Git::commit_kanspec` scopes every one of its three git calls to
    /// `:(glob,top).kanspec/**`. A projection rewrite therefore can never be part of that
    /// commit — so counting it here does not commit the projection, it only makes `scan`
    /// (which regenerates them, D-20) sweep a human's pending tracker edit into a commit
    /// labelled `kanspec: confirm`. That is precisely the harm the paragraph above exists
    /// to prevent, reached by a second route; `scan_ladder.rs::a_scan_commits_nothing_…`
    /// fails on the nose without this arm. If the projections should ever be auto-committed
    /// too, the fix is to widen `commit_kanspec`'s pathspec, not to re-arm this predicate.
    pub fn writes_tracked_file(&self) -> bool {
        !matches!(self, Op::WriteGitState { .. } | Op::WriteGenerated { .. })
    }

    /// Does this step change something `KANSPEC-FEATURES.md` / `KANSPEC-ARCHITECTURE.md`
    /// are projected from? A spec, decision or quirk record; a rule stamp (a spec body);
    /// or the git facts the feature map's `Fresh?` column reads. `Store::transact`
    /// republishes the projections from the post-write snapshot whenever a plan answers
    /// yes, so no handler can forget to (t-0769).
    pub fn touches_projection(&self) -> bool {
        match self {
            Op::StampRule { .. } | Op::WriteGitState { .. } => true,
            _ => matches!(
                self.entity(),
                Some(EntityRef::Spec(_) | EntityRef::Decision(_) | EntityRef::Quirk(_))
            ),
        }
    }

    /// The entity a plan step is about, when it has one. `MoveDir`, `AppendJsonl`,
    /// `WriteGenerated` and `WriteGitState` name raw paths instead; `StampRule` names a
    /// `SpecName` rather than owning an `EntityRef`, so it answers `None` here and
    /// `Store::transact` reads its subject off the variant directly.
    pub fn entity(&self) -> Option<&EntityRef> {
        match self {
            Op::SetFields { entity, .. }
            | Op::CreateEntity { entity, .. }
            | Op::AppendSection { entity, .. } => Some(entity),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Plan {
    pub ops: Vec<Op>,
    pub minted: Vec<EntityRef>,
    pub note: Option<String>,
}

impl Plan {
    pub fn of(ops: Vec<Op>) -> Plan {
        Plan {
            ops,
            ..Plan::default()
        }
    }
    pub fn empty() -> Plan {
        Plan::default()
    }
    pub fn push(&mut self, op: Op) -> &mut Plan {
        self.ops.push(op);
        self
    }
    pub fn mint(&mut self, e: EntityRef) -> &mut Plan {
        self.minted.push(e);
        self
    }
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Belt-and-braces (the key enums already make a derived write unreachable), plus id
    /// uniqueness and one-transition-per-ticket-per-plan.
    pub fn validate(&self, snap: &Snapshot) -> Result<()> {
        let mut transitioned: HashSet<&TicketId> = HashSet::new();
        let mut created: HashSet<EntityRef> = HashSet::new();

        for op in &self.ops {
            match op {
                Op::Transition { id, .. } => {
                    if !transitioned.insert(id) {
                        return Err(KsError::conflict(
                            format!("plan transitions {id} twice — a ticket has one owed verb"),
                            fixes![fix!("kanspec show {id}")],
                        ));
                    }
                    require_ticket(snap, id)?;
                }
                Op::MarkSteps { id, .. } => require_ticket(snap, id)?,
                Op::CreateEntity { entity, .. } => {
                    let fix = fix!("kanspec show {}", entity.id());
                    require_fresh(snap, &mut created, entity.clone(), false, fix)?;
                }
                // A closed proposal is not in `snap.proposals`, but its id is still taken.
                Op::CreateProposal { id, .. } => {
                    let taken = snap.closed_ids.contains(id.as_str());
                    let e = EntityRef::Proposal(id.clone());
                    require_fresh(snap, &mut created, e, taken, fix!("kanspec board"))?;
                }
                Op::SetFields { entity, sets } => {
                    require_present(entity, snap, &created)?;
                    // The key enums make this unreachable; assert it anyway, because a
                    // future `Key` variant is exactly the mistake this catches.
                    for (k, _) in sets {
                        if crate::keys::RESERVED_DERIVED.contains(&k.as_str()) {
                            return Err(KsError::gate(
                                GateCode::DerivedKeyWrite,
                                format!("`{k}` is a derived fact — it has no write path"),
                                fixes![fix!("kanspec scan"), fix!("kanspec doctor")],
                            ));
                        }
                    }
                }
                Op::AppendSection { entity, .. } => require_present(entity, snap, &created)?,
                Op::StampRule { spec, .. } => {
                    if !snap.specs.contains_key(spec) {
                        return Err(KsError::not_found(
                            "spec",
                            spec.to_string(),
                            fixes![fix!("kanspec features")],
                        ));
                    }
                }
                Op::AppendJsonl { .. }
                | Op::MoveDir { .. }
                | Op::WriteGenerated { .. }
                | Op::WriteGitState { .. } => {}
            }
        }

        let mut minted: HashSet<&EntityRef> = HashSet::new();
        for m in &self.minted {
            if !minted.insert(m) {
                return Err(KsError::conflict(
                    format!("plan mints {m} twice"),
                    fixes![fix!("kanspec doctor")],
                ));
            }
        }
        Ok(())
    }

    /// Every file the plan writes, deduped and in a stable order. `Store::transact` runs
    /// `fm::writable()` over these BEFORE any byte moves.
    pub fn touched(&self, layout: &Layout) -> Vec<PathBuf> {
        let mut out: BTreeSet<PathBuf> = BTreeSet::new();
        for op in &self.ops {
            match op {
                Op::Transition { id, .. } | Op::MarkSteps { id, .. } => {
                    out.insert(layout.ticket(id));
                }
                Op::SetFields { entity, .. }
                | Op::CreateEntity { entity, .. }
                | Op::AppendSection { entity, .. } => {
                    out.insert(layout.path_for(entity));
                }
                Op::CreateProposal { id, slug, .. } => {
                    out.insert(layout.proposal_md(&layout.proposal_dir(id, slug)));
                }
                Op::StampRule { spec, .. } => {
                    out.insert(layout.spec(spec));
                }
                Op::AppendJsonl { path, .. } | Op::WriteGenerated { path, .. } => {
                    out.insert(path.clone());
                }
                Op::MoveDir { from, to } => {
                    out.insert(from.clone());
                    out.insert(to.clone());
                }
                Op::WriteGitState { .. } => {
                    out.insert(layout.gitstate());
                }
            }
        }
        out.into_iter().collect()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The planner's input — gathered BEFORE `transact`
// ─────────────────────────────────────────────────────────────────────────────

/// Everything a planner needs from the outside world, gathered BEFORE `transact`. The
/// planner is GENUINELY pure: no `Ctx`, no git, no gh, no clock, no fs. That is what makes
/// every planner a table-driven unit test against a hand-built `Snapshot` with zero IO —
/// the single best testing seam in the design.
///
/// `Facts` lives here, beside `Plan`, because it is the *input* half of the same
/// vocabulary and every slice's planner needs it (`scan.rs` keeps its own `ConfirmFacts`
/// for the same reason `MergedProof` lives there: it is minted by that file alone).
pub struct Facts {
    pub actor: Actor,
    pub at: DateTime<Utc>,
    pub invocation: String,
}

pub struct StartFacts {
    pub base: Facts,
    pub branch: String,
    pub worktree: Option<PathBuf>,
    pub head: crate::git::HeadSha,
}

pub struct ShipFacts {
    pub base: Facts,
    pub head: crate::git::HeadSha,
}

pub struct DoneFacts {
    pub base: Facts,
    /// a `MergedProof` from a freshly re-run ladder, or a recorded `NoCodeWaiver`
    pub landed: crate::scan::Landed,
    pub touched: Vec<crate::git::ChangedPath>,
}

fn require_present(
    entity: &EntityRef,
    snap: &Snapshot,
    created: &HashSet<EntityRef>,
) -> Result<()> {
    if entity.exists_in(snap) || created.contains(entity) {
        return Ok(());
    }
    Err(KsError::not_found(
        entity.noun(),
        entity.id(),
        fixes![fix!("kanspec status")],
    ))
}

fn require_ticket(snap: &Snapshot, id: &TicketId) -> Result<()> {
    if snap.tickets.contains_key(id) {
        return Ok(());
    }
    Err(KsError::not_found(
        "ticket",
        id.to_string(),
        fixes![fix!("kanspec ls --all")],
    ))
}

/// A create refuses to overwrite (`also_taken` covers ids the snapshot holds only as
/// `closed_ids`) and refuses to create the same entity twice in one plan.
fn require_fresh(
    snap: &Snapshot,
    created: &mut HashSet<EntityRef>,
    e: EntityRef,
    also_taken: bool,
    fix: Fix,
) -> Result<()> {
    if also_taken || e.exists_in(snap) {
        return Err(KsError::conflict(
            format!("{e} already exists — refusing to overwrite it"),
            fixes![fix],
        ));
    }
    if created.contains(&e) {
        return Err(KsError::conflict(
            format!("plan creates {e} twice"),
            fixes![fix!("kanspec doctor")],
        ));
    }
    created.insert(e);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn snap() -> Snapshot {
        Snapshot::empty(Config::default(), Utc::now())
    }

    fn tid(s: &str) -> TicketId {
        TicketId::parse(s).unwrap()
    }

    #[test]
    fn a_plan_touching_a_missing_ticket_is_refused() {
        let p = Plan::of(vec![Op::MarkSteps {
            id: tid("t-9c41"),
            checks: vec![(1, true)],
        }]);
        let e = p.validate(&snap()).unwrap_err();
        assert_eq!(e.kind(), "not_found");
    }

    #[test]
    fn create_then_set_in_one_plan_is_legal() {
        let e = EntityRef::Quirk(QuirkId::parse("q-11ba").unwrap());
        let p = Plan::of(vec![
            Op::CreateEntity {
                entity: e.clone(),
                contents: "---\n---\n".into(),
            },
            Op::SetFields {
                entity: e,
                sets: vec![(Key::Quirk(crate::keys::QuirkKey::Status), Yv::s("fixed"))],
            },
        ]);
        p.validate(&snap()).unwrap();
    }

    #[test]
    fn a_plan_that_mints_the_same_entity_twice_is_refused() {
        let e = EntityRef::Ticket(tid("t-9c41"));
        let mut p = Plan::empty();
        p.mint(e.clone()).mint(e);
        assert_eq!(p.validate(&snap()).unwrap_err().kind(), "conflict");
    }

    #[test]
    fn touched_maps_ops_to_files_and_dedupes() {
        let repo = std::env::current_dir().unwrap();
        let r = crate::paths::Repo::discover(&repo, None).unwrap();
        let layout = Layout::open(&r, &Config::default());
        let id = tid("t-9c41");
        let p = Plan::of(vec![
            Op::MarkSteps {
                id: id.clone(),
                checks: vec![],
            },
            Op::AppendSection {
                entity: EntityRef::Ticket(id.clone()),
                heading: "## Log",
                line: "- x".into(),
            },
        ]);
        assert_eq!(p.touched(&layout), vec![layout.ticket(&id)]);
    }
}
