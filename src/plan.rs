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
use crate::error::{KsError, Result};
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
    /// `comments.jsonl` (v0.2)
    AppendJsonl { path: PathBuf, line: String },
    /// `close`: -> `proposals/closed/`
    MoveDir { from: PathBuf, to: PathBuf },
    /// `KANSPEC-*.md` ONLY
    WriteGenerated { path: PathBuf, contents: String },
    /// `cache/gitstate.json`. `ScanToken` is minted solely by `scan::scan_all`, so
    /// "written by scan and nothing else" is a type fact — and the write happens INSIDE
    /// the lock.
    WriteGitState { token: ScanToken, state: GitState },
}

impl Op {
    /// The entity a plan step is about, when it has one. `MoveDir`, `AppendJsonl`,
    /// `WriteGenerated` and `WriteGitState` name raw paths instead.
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
                Op::Transition { id, verb, .. } => {
                    if !transitioned.insert(id) {
                        return Err(KsError::conflict(
                            format!("plan transitions {id} twice — a ticket has one owed verb"),
                            fixes![fix!("kanspec show {id}")],
                        ));
                    }
                    if !snap.tickets.contains_key(id) {
                        return Err(KsError::not_found(
                            "ticket",
                            id.to_string(),
                            fixes![fix!("kanspec ls --all")],
                        ));
                    }
                    let _ = verb;
                }
                Op::CreateEntity { entity, .. } => {
                    if entity.exists_in(snap) {
                        return Err(KsError::conflict(
                            format!("{entity} already exists — refusing to overwrite it"),
                            fixes![fix!("kanspec show {}", entity.id())],
                        ));
                    }
                    if !created.insert(entity.clone()) {
                        return Err(KsError::conflict(
                            format!("plan creates {entity} twice"),
                            fixes![fix!("kanspec doctor")],
                        ));
                    }
                }
                Op::SetFields { entity, sets } => {
                    require_present(entity, snap, &created)?;
                    // The key enums make this unreachable; assert it anyway, because a
                    // future `Key` variant is exactly the mistake this catches.
                    for (k, _) in sets {
                        if crate::keys::RESERVED_DERIVED.contains(&k.as_str()) {
                            return Err(KsError::gate(
                                "derived_key_write",
                                format!("`{k}` is a derived fact — it has no write path"),
                                fixes![fix!("kanspec scan"), fix!("kanspec doctor")],
                            ));
                        }
                    }
                }
                Op::AppendSection { entity, .. } => require_present(entity, snap, &created)?,
                Op::MarkSteps { id, .. } => {
                    if !snap.tickets.contains_key(id) {
                        return Err(KsError::not_found(
                            "ticket",
                            id.to_string(),
                            fixes![fix!("kanspec ls --all")],
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
