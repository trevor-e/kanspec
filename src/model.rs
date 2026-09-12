//! Entity structs plus [`Snapshot`]. **Types only, zero IO** — `store.rs` fills them.
//!
//! Note what is absent from [`TicketFm`], forever: `merged`, `in_main`, `ready`,
//! `stalled`, `ci`, `checked_at`. Derived state is computed FROM a `Snapshot` and never
//! stored IN one; the whole design is legible in that one struct.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::cache::GitState;
use crate::config::Config;
use crate::error::{KsError, Result};
use crate::ids::{CommentId, DecisionId, ItemRef, ProposalId, QuirkId, SpecName, TicketId};
use crate::logentry::LogEntry;
use crate::transitions::State;
use crate::{fix, fixes};

// ─────────────────────────────────────────────────────────────────────────────
// Ticket
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct TicketFm {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    #[serde(default)]
    pub spec: Option<SpecName>,
    #[serde(default)]
    pub proposal: Option<ProposalId>,
    /// which proposal ticket-item minted this — `t1`
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub deps: Vec<TicketId>,
    /// leftover scope spawned by a closed ticket's triage
    #[serde(default)]
    pub followup_of: Option<TicketId>,
    /// tangential work found while doing another ticket
    #[serde(default)]
    pub discovered_in: Option<TicketId>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub worktree: Option<PathBuf>,
    #[serde(default)]
    pub claimed_by: Option<String>,
    #[serde(default)]
    pub pr: Option<u64>,
    /// A plain `String`: read from git by `ship`/`done`, never typed by an agent —
    /// enforced upstream, because the only value that can be WRITTEN here comes from
    /// `HeadSha`, which only `git.rs` can mint.
    #[serde(default)]
    pub head: Option<String>,
    /// The recorded knowledge-checkpoint waiver, visible on the board.
    #[serde(default)]
    pub spec_unchanged: Option<String>,
    pub created: DateTime<Utc>,
    /// Load-bearing: a key written by a NEWER kanspec is never dropped by an older one,
    /// and `doctor::check_reserved_keys` scans exactly this map against
    /// [`crate::keys::RESERVED_DERIVED`].
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

#[derive(Debug, Clone)]
pub struct Ticket {
    pub fm: TicketFm,
    pub path: PathBuf,
    /// Everything after the closing fence, verbatim — this is the TRUTH the surgical
    /// writer edits; `steps` and `log` are parsed views of it.
    pub body: String,
    pub steps: Vec<Step>,
    pub log: Vec<LogEntry>,
    pub mtime: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Step {
    pub index: usize,
    pub done: bool,
    pub text: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Spec
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct SpecFm {
    /// the one-liner that feeds the generated feature map
    pub feature: String,
    /// the globs that feed the staleness tripwire
    #[serde(default)]
    pub code: Vec<String>,
    #[serde(default)]
    pub stale_ack: Option<StaleAck>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

/// The human's "no behaviour change" attestation. In the spec's FRONTMATTER (git-tracked)
/// rather than the cache, so it survives `rm -rf cache/` — D-10.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleAck {
    pub sha: String,
    pub at: DateTime<Utc>,
    pub by: String,
    pub why: String,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub name: SpecName,
    pub fm: SpecFm,
    pub path: PathBuf,
    pub body: String,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    /// `"auth.lockout"` — the visible bracket id; `RuleRef` renders it as `auth#lockout`.
    pub anchor: String,
    pub text: String,
    /// the `{p-xxxx}` tokens: provenance back to the proposal that shipped this rule
    pub provenance: Vec<ProposalId>,
    /// the `{p-xxxx#c1}` tokens: provenance back to the exact proposal ITEM this rule
    /// satisfies. Optional by design — a bullet may name only its proposal — but when it is
    /// there `close` uses it instead of guessing which Change a rule shipped.
    ///
    /// An item token also contributes its proposal to `provenance` above, so every existing
    /// consumer (the feature map's `last_shipped`, `rules --audit`, `why`) sees it without
    /// knowing this field exists.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ItemRef>,
    pub line: usize,
}

// ─────────────────────────────────────────────────────────────────────────────
// Decision
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct DecisionFm {
    pub id: DecisionId,
    pub title: String,
    pub status: DecisionStatus,
    pub date: NaiveDate,
    /// provenance: the exact proposal item that spawned this — `p-7de2#p1`
    #[serde(default)]
    pub source: Option<String>,
    /// where it steers agents (prime injects full text on path match)
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub supersedes: Option<DecisionId>,
    #[serde(default)]
    pub superseded_by: Option<DecisionId>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub fm: DecisionFm,
    pub path: PathBuf,
    pub body: String,
    /// `fm.scope` normalized for matching — the value `rulesdoc::Scope` compiles.
    pub scope: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Superseded,
    Revoked,
}

// ─────────────────────────────────────────────────────────────────────────────
// Quirk
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct QuirkFm {
    pub id: QuirkId,
    pub title: String,
    #[serde(default)]
    pub paths: Vec<String>,
    pub severity: Severity,
    pub status: QuirkStatus,
    /// the ticket where we learned this
    #[serde(default)]
    pub source: Option<TicketId>,
    /// retired only by evidence
    #[serde(default)]
    pub fixed_by: Option<TicketId>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

#[derive(Debug, Clone)]
pub struct Quirk {
    pub fm: QuirkFm,
    pub path: PathBuf,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Landmine,
    Gotcha,
    Debt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuirkStatus {
    Active,
    Fixed,
    Stale,
}

// ─────────────────────────────────────────────────────────────────────────────
// v0.2 types, declared in wave 0 so V2 never edits this file
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct ProposalFm {
    pub id: ProposalId,
    pub title: String,
    pub status: ProposalStatus,
    #[serde(default)]
    pub specs: Vec<SpecName>,
    /// stamped by `kanspec approve`: `"2026-08-31T09:12Z trevor"`
    #[serde(default)]
    pub approved: Option<String>,
    /// the disposition ledger, stamped at close
    #[serde(default)]
    pub ledger: Vec<String>,
    pub created: NaiveDate,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml_ng::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Draft,
    Review,
    Approved,
    Closed,
    Abandoned,
}

#[derive(Debug, Clone)]
pub struct Proposal {
    pub fm: ProposalFm,
    pub dir: PathBuf,
    pub body: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub id: ItemRef,
    pub text: String,
    pub prescription: Option<Prescription>,
}

/// Every prescription is typed: `(temp until t-x)` dies when its guard ticket lands;
/// `(promote: decision|spec|quirk)` must become a standing record at close. `Untyped` is
/// a `doctor` warning and a close blocker.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Prescription {
    TempUntil(TicketId),
    Promote(PromoteAs),
    Untyped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromoteAs {
    Decision,
    Spec,
    Quirk,
}

/// One row of a proposal's `comments.jsonl`. Deduped on read by `(id, op, at)` — one
/// `cm-` id legitimately carries `comment` + `reply` + `resolve` rows (D-19).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentOp {
    pub id: CommentId,
    pub op: CommentOpKind,
    /// `p-7de2#c3` — present on `comment`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// the item's text at comment time, so threads survive edits (invariant 5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// required on `resolve`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// `"agent"` when an agent process wrote the row — carried beside the label, because
    /// the label is a git identity that an agent relaying a human's words borrows
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentOpKind {
    Comment,
    Reply,
    Resolve,
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot
// ─────────────────────────────────────────────────────────────────────────────

/// Everything on disk, loaded once, plus the disposable cache.
#[derive(Default)]
pub struct Snapshot {
    pub tickets: BTreeMap<TicketId, Ticket>,
    /// OPEN proposals ONLY. Closed proposal BODIES are never read from disk, so there is
    /// physically no value through which closed prose can reach `rulesdoc::build` or
    /// `prime`. Invariant 4 is a property of the generator's INPUT TYPE.
    pub proposals: BTreeMap<ProposalId, Proposal>,
    /// Ids of closed proposals — for collision-free minting and NOTHING else. Omitting
    /// closed bodies would otherwise reopen an id collision against `proposals/closed/`.
    pub closed_ids: HashSet<String>,
    pub specs: BTreeMap<SpecName, Spec>,
    pub decisions: BTreeMap<DecisionId, Decision>,
    pub quirks: BTreeMap<QuirkId, Quirk>,
    /// id+op deduped on read
    pub comments: BTreeMap<ProposalId, Vec<CommentOp>>,
    /// the gitignored cache — the SOLE home of derived git facts
    pub git: GitState,
    pub cfg: Config,
    pub now: DateTime<Utc>,
    /// Bumped on every successful `transact`. The server memoizes on it; the SSE payload
    /// carries it (D-22/D-23).
    pub rev: u64,
}

/// An id that can look up its own title in a [`Snapshot`] to render as a display label.
/// See [`crate::ids::label`]: the slug is cosmetic, the key is what is stored and parsed.
pub trait Labeled {
    fn label_in(&self, s: &Snapshot) -> String;
}

impl Labeled for TicketId {
    fn label_in(&self, s: &Snapshot) -> String {
        match s.tickets.get(self) {
            Some(t) => crate::ids::label(self, &t.fm.title),
            None => self.to_string(),
        }
    }
}
impl Labeled for ProposalId {
    fn label_in(&self, s: &Snapshot) -> String {
        match s.proposals.get(self) {
            Some(p) => crate::ids::label(self, &p.fm.title),
            None => self.to_string(),
        }
    }
}
impl Labeled for DecisionId {
    fn label_in(&self, s: &Snapshot) -> String {
        match s.decisions.get(self) {
            Some(d) => crate::ids::label(self, &d.fm.title),
            None => self.to_string(),
        }
    }
}
impl Labeled for QuirkId {
    fn label_in(&self, s: &Snapshot) -> String {
        match s.quirks.get(self) {
            Some(q) => crate::ids::label(self, &q.fm.title),
            None => self.to_string(),
        }
    }
}
impl<T: Labeled> Labeled for &T {
    fn label_in(&self, s: &Snapshot) -> String {
        (*self).label_in(s)
    }
}

impl Snapshot {
    /// `t-9c41-rate-limit-login` for a known id, the bare key for one the snapshot does not
    /// hold (a closed ticket cited as a `source:`, say). Never an error: a label is cosmetic.
    pub fn label<L: Labeled>(&self, id: L) -> String {
        id.label_in(self)
    }
    /// The labels of many ids, joined — `blocked by t-31aa-lockout-table, t-66d1-...`.
    pub fn labels<'a, L: Labeled + 'a>(
        &self,
        ids: impl IntoIterator<Item = &'a L>,
        sep: &str,
    ) -> String {
        ids.into_iter()
            .map(|i| i.label_in(self))
            .collect::<Vec<_>>()
            .join(sep)
    }

    pub fn ticket(&self, id: &TicketId) -> Result<&Ticket> {
        self.tickets.get(id).ok_or_else(|| {
            KsError::not_found(
                "ticket",
                id.to_string(),
                fixes![fix!("kanspec ls --all"), fix!("kanspec status")],
            )
        })
    }
    pub fn spec(&self, n: &SpecName) -> Result<&Spec> {
        self.specs.get(n).ok_or_else(|| {
            KsError::not_found(
                "spec",
                n.to_string(),
                fixes![fix!("kanspec features"), fix!("kanspec spec new \"{n}\"")],
            )
        })
    }
    pub fn decision(&self, id: &DecisionId) -> Result<&Decision> {
        self.decisions.get(id).ok_or_else(|| {
            KsError::not_found("decision", id.to_string(), fixes![fix!("kanspec rules")])
        })
    }
    pub fn quirk(&self, id: &QuirkId) -> Result<&Quirk> {
        self.quirks.get(id).ok_or_else(|| {
            KsError::not_found("quirk", id.to_string(), fixes![fix!("kanspec quirks")])
        })
    }
    pub fn proposal(&self, id: &ProposalId) -> Result<&Proposal> {
        self.proposals.get(id).ok_or_else(|| {
            KsError::not_found(
                "proposal",
                id.to_string(),
                fixes![
                    fix!("kanspec status"),
                    fix!("closed proposals bind nothing — they are not loaded"),
                ],
            )
        })
    }

    /// Every id the minter must avoid — **including** `closed_ids`.
    pub fn taken_ids(&self) -> HashSet<String> {
        let mut out: HashSet<String> = self.closed_ids.clone();
        out.extend(self.tickets.keys().map(|k| k.as_str().to_string()));
        out.extend(self.proposals.keys().map(|k| k.as_str().to_string()));
        out.extend(self.decisions.keys().map(|k| k.as_str().to_string()));
        out.extend(self.quirks.keys().map(|k| k.as_str().to_string()));
        out.extend(
            self.comments
                .values()
                .flatten()
                .map(|c| c.id.as_str().to_string()),
        );
        out
    }

    /// An empty snapshot — the base every pure planner unit test builds on.
    pub fn empty(cfg: Config, now: DateTime<Utc>) -> Snapshot {
        Snapshot {
            cfg,
            now,
            ..Snapshot::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKET: &str = r#"
id: t-9c41
title: Rate-limit login endpoint
state: doing
spec: auth
proposal: p-7de2
item: t1
deps: [t-31aa]
followup_of: null
discovered_in: null
branch: ks/t-9c41-rate-limit-login
worktree: ../kanspec-wt/t-9c41
claimed_by: claude/sess-a91
pr: 142
head: null
created: 2026-08-30T14:02:11Z
"#;

    #[test]
    fn the_design_md_ticket_frontmatter_deserializes() {
        let fm: TicketFm = serde_yaml_ng::from_str(TICKET).unwrap();
        assert_eq!(fm.id.as_str(), "t-9c41");
        assert_eq!(fm.state, State::Doing);
        assert_eq!(fm.spec.as_ref().unwrap().as_str(), "auth");
        assert_eq!(fm.deps.len(), 1);
        assert!(fm.followup_of.is_none());
        assert_eq!(fm.pr, Some(142));
        assert!(fm.head.is_none());
        assert!(fm.extra.is_empty(), "no stray keys: {:?}", fm.extra);
    }

    #[test]
    fn an_unknown_future_key_lands_in_extra_and_is_never_dropped() {
        let fm: TicketFm =
            serde_yaml_ng::from_str(&format!("{TICKET}merged: true\nfuture_knob: 7\n")).unwrap();
        assert!(
            fm.extra.contains_key("merged"),
            "doctor scans exactly this map"
        );
        assert!(fm.extra.contains_key("future_knob"));
    }

    #[test]
    fn the_design_md_spec_and_quirk_and_decision_frontmatter_deserialize() {
        let s: SpecFm = serde_yaml_ng::from_str(
            "feature: Login (JWT 24h), lockout after 5 failures\ncode: [src/auth/**]\n",
        )
        .unwrap();
        assert_eq!(s.code, ["src/auth/**"]);
        assert!(s.stale_ack.is_none());

        let q: QuirkFm = serde_yaml_ng::from_str(
            "id: q-11ba\ntitle: Stripe webhooks replay in staging\npaths: [src/billing/**]\n\
             severity: landmine\nstatus: active\nsource: t-8812\nfixed_by: null\n",
        )
        .unwrap();
        assert_eq!(q.severity, Severity::Landmine);
        assert_eq!(q.status, QuirkStatus::Active);
        assert_eq!(q.source.unwrap().as_str(), "t-8812");

        let d: DecisionFm = serde_yaml_ng::from_str(
            "id: D-8c1a\ntitle: Rate-limit state lives in Redis only\nstatus: accepted\n\
             date: 2026-09-02\nsource: p-7de2#p1\nscope: [src/auth/**]\n\
             supersedes: null\nsuperseded_by: null\n",
        )
        .unwrap();
        assert_eq!(d.status, DecisionStatus::Accepted);
        assert_eq!(d.date.to_string(), "2026-09-02");
    }

    #[test]
    fn the_design_md_comment_rows_deserialize() {
        for line in [
            r#"{"id":"cm-88f1","op":"comment","target":"p-7de2#c3","quote":"ops: alert","body":"scope creep","author":"trevor","at":"2026-08-30T16:02:00Z"}"#,
            r#"{"id":"cm-88f1","op":"reply","by":"agent:claude","body":"Agreed.","at":"2026-08-30T16:21:40Z"}"#,
            r#"{"id":"cm-88f1","op":"resolve","by":"agent:claude","note":"c3 -> p-8a10","at":"2026-08-30T16:21:41Z"}"#,
        ] {
            let c: CommentOp = serde_json::from_str(line).unwrap();
            assert_eq!(c.id.as_str(), "cm-88f1");
        }
    }

    #[test]
    fn taken_ids_includes_closed_proposal_ids() {
        let mut s = Snapshot::empty(Config::default(), Utc::now());
        s.closed_ids.insert("p-19f0".to_string());
        assert!(s.taken_ids().contains("p-19f0"));
    }
}
