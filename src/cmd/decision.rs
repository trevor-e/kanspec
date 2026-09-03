//! `decide / accept / supersede / revoke / why`.
//!
//! Agents write decisions at exactly three moments and **never self-accept** (invariant 8):
//! `plan_accept` and `plan_revoke` take a `&HumanActor`, whose only constructor refuses an
//! `Actor::Agent`, so an agent session literally cannot call them.
//!
//! `supersede` takes one too, and that is a deliberate reading of the same invariant
//! rather than an over-application of it. Superseding *flips the old decision out of
//! `accepted`* in the same plan that mints the replacement — `doctor::check_immutable_
//! decisions` makes an `accepted` record carrying `superseded_by:` a hard Error, so the
//! back-link and the status flip cannot be separated — and an agent that could retire a
//! standing rule unilaterally has the kill switch invariant 8 exists to keep out of its
//! hands. What the agent CAN do is what DESIGN.md's third update trigger describes: the
//! replacement is minted `proposed`, and a human still has to accept it before it binds.
//!
//! Accepted bodies are immutable; the only legal mutations are status flips and back-links.
//!
//! Owner: **S6**.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cli::{AcceptArgs, DecideArgs, RevokeArgs, SupersedeArgs, WhyArgs};
use crate::cmd::ticket::{facts, first_minted, join, rel_to, write_next};
use crate::ctx::{Ctx, HumanActor};
use crate::error::{GateCode, KsError, Result};
use crate::fm::{self, Yv};
use crate::ids::{DecisionId, ItemRef, Minter, ProposalId, QuirkId, RuleRef, TicketId};
use crate::keys::{DecisionKey, Key};
use crate::model::{DecisionStatus, Snapshot};
use crate::out::{glyph, Color, Line, Render, Style};
use crate::plan::{EntityRef, Facts, Op, Plan};
use crate::store::{Committed, Store};
use crate::{fix, fixes};

// ── decide ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DecideReport {
    pub id: DecisionId,
    pub title: String,
    /// always `proposed` — a mid-implementation discovery is captured, never enacted
    pub status: DecisionStatus,
    pub source: Option<String>,
    pub scope: Vec<String>,
    pub path: String,
    pub url: Option<String>,
    pub next: Vec<String>,
}

pub fn decide(ctx: &Ctx, a: &DecideArgs) -> Result<DecideReport> {
    ctx.require_initialized()?;
    // Every glob is compiled before the lock: a decision whose `scope:` cannot compile
    // steers nobody, and `prime` would silently never inject it.
    crate::rulesdoc::Scope::of(&a.scope)?;
    let f = facts(ctx);
    let done =
        Store::open(ctx).transact(None, &ctx.invocation(), |s, m| plan_decide(s, &f, a, m))?;
    let id = minted_decision(&done)?;

    Ok(DecideReport {
        title: a.title.trim().to_string(),
        status: DecisionStatus::Proposed,
        source: a.from.clone(),
        scope: a.scope.clone(),
        path: rel_to(ctx, &ctx.layout.decision(&id)),
        url: Some(format!("http://127.0.0.1:{}/d/{id}", ctx.cfg.port)),
        next: vec![
            format!("{} accept {id}", ctx.invoked_as),
            format!("{} rules", ctx.invoked_as),
        ],
        id,
    })
}

/// PURE. Mints `status: proposed` and nothing else — a proposed decision is not injected
/// as a standing rule, it sits in the YOU section of `status` until a human acts.
pub fn plan_decide(s: &Snapshot, f: &Facts, a: &DecideArgs, m: &Minter) -> Result<Plan> {
    let title = a.title.trim();
    if title.is_empty() {
        return Err(KsError::invalid(
            "a decision needs a title stated as a claim",
            fixes![fix!(
                "kanspec decide \"Rate-limit state lives in Redis only\""
            )],
        ));
    }
    // Provenance must point at something that exists, or `kanspec why` walks off a cliff
    // the moment anyone audits the rule.
    if let Some(src) = a.from.as_deref() {
        check_source(s, src)?;
    }
    let id = m.decision(title)?;
    let mut plan = Plan::of(vec![Op::CreateEntity {
        entity: EntityRef::Decision(id.clone()),
        contents: scaffold(
            &id,
            title,
            f.at,
            a.from.as_deref(),
            &a.scope,
            None,
            &madr(title),
        ),
    }]);
    plan.mint(EntityRef::Decision(id));
    Ok(plan)
}

/// DESIGN.md's decision file, in `keys::DECISION_ORDER`, always minted `proposed`
/// (invariant 8: agents never self-accept, and `HumanActor` enforces the other half). Every
/// value goes through `fm::emit`, so a title full of colons cannot corrupt the file. Shared
/// with `done`'s knowledge checkpoint, which supplies its own `body`, so a decision captured
/// at close-out and one made with `decide` cannot drift in their frontmatter.
pub(crate) fn scaffold(
    id: &DecisionId,
    title: &str,
    at: DateTime<Utc>,
    source: Option<&str>,
    scope: &[String],
    supersedes: Option<&DecisionId>,
    body: &str,
) -> String {
    format!(
        "---\nid: {id}\ntitle: {}\nstatus: proposed\ndate: {}\nsource: {}\nscope: {}\n\
         supersedes: {}\nsuperseded_by: null\n---\n{body}",
        fm::emit(&Yv::s(title), false),
        at.date_naive(),
        fm::emit(&Yv::opt_s(source), false),
        fm::emit(&Yv::list(scope.to_vec()), false),
        fm::emit(&Yv::opt_s(supersedes.map(ToString::to_string)), false),
    )
}

/// MADR-minimal: Context / Decision / Consequences, one page max.
fn madr(title: &str) -> String {
    format!("## Context\n\n## Decision\n{title}\n\n## Consequences\n")
}

impl Render for DecideReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        let src = self
            .source
            .as_ref()
            .map(|s| format!("source {s} · "))
            .unwrap_or_default();
        let mut line = Line::new('▸', format!("{} created", self.title))
            .id(&self.id)
            .dim(format!("· proposed · {src}{}", self.path));
        if let Some(u) = &self.url {
            line = line.url(format!("accept: {u}"));
        }
        line.write(w, st)?;
        // Only the FIRST next step: `accept` is the one act that makes this bind.
        write_next(w, st, &self.next[..self.next.len().min(1)])
    }
}

// ── accept / supersede / revoke ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DecisionReport {
    pub id: DecisionId,
    pub title: String,
    pub status: DecisionStatus,
    pub by: String,
    /// the new record, for `supersede`
    pub replacement: Option<DecisionId>,
    pub why: Option<String>,
    pub next: Vec<String>,
}

/// The shape `accept`, `supersede` and `revoke` share. The HUMAN is checked BEFORE the lock:
/// refusing an agent must not cost a transaction, and the message is the same either way.
/// One plan, then the projections are republished (D-20).
fn flip(
    ctx: &Ctx,
    verb: &'static str,
    raw: &str,
    plan: impl FnOnce(&Snapshot, &Facts, &HumanActor, &DecisionId, &Minter) -> Result<Plan>,
) -> Result<(DecisionId, Committed)> {
    ctx.require_initialized()?;
    let who = HumanActor::require(&ctx.actor, verb)?;
    let id = DecisionId::parse(raw)?;
    let f = facts(ctx);
    let done =
        Store::open(ctx).transact(None, &ctx.invocation(), |s, m| plan(s, &f, &who, &id, m))?;
    Ok((id, done))
}

/// The report all three flips start from; `Committed::snapshot` is the post-write reload.
fn flipped(
    ctx: &Ctx,
    id: DecisionId,
    done: &Committed,
    status: DecisionStatus,
) -> Result<DecisionReport> {
    Ok(DecisionReport {
        title: done.snapshot.decision(&id)?.fm.title.clone(),
        status,
        by: ctx.actor.label(),
        replacement: None,
        why: None,
        next: vec![format!("{} rules", ctx.invoked_as)],
        id,
    })
}

pub fn accept(ctx: &Ctx, a: &AcceptArgs) -> Result<DecisionReport> {
    let (id, done) = flip(ctx, "accept", &a.id, |s, _f, who, id, _m| {
        plan_accept(s, who, id)
    })?;
    flipped(ctx, id, &done, DecisionStatus::Accepted)
}

/// Takes `&HumanActor`, so invariant 8 is a type fact rather than a runtime check anyone
/// could forget to write.
pub fn plan_accept(s: &Snapshot, who: &HumanActor, id: &DecisionId) -> Result<Plan> {
    let _ = who;
    let d = s.decision(id)?;
    if d.fm.status != DecisionStatus::Proposed {
        return Err(KsError::gate(
            GateCode::DecisionNotProposed,
            format!(
                "{id} is `{}`, not `proposed` — nothing to accept",
                status_word(d.fm.status)
            ),
            fixes![fix!("kanspec rules"), fix!("kanspec why {id}")],
        ));
    }
    // The BODY freezes here: from this point the only legal mutations are status flips and
    // back-links, and `doctor::check_immutable_decisions` proves the record half of it.
    Ok(Plan::of(vec![Op::SetFields {
        entity: EntityRef::Decision(id.clone()),
        sets: vec![(Key::Decision(DecisionKey::Status), Yv::s("accepted"))],
    }]))
}

pub fn supersede(ctx: &Ctx, a: &SupersedeArgs) -> Result<DecisionReport> {
    let (id, done) = flip(ctx, "supersede", &a.id, |s, f, who, id, m| {
        plan_supersede(s, f, who, id, &a.with, m)
    })?;
    let replacement = minted_decision(&done)?;
    let mut r = flipped(ctx, id, &done, DecisionStatus::Superseded)?;
    r.next
        .insert(0, format!("{} accept {replacement}", ctx.invoked_as));
    r.replacement = Some(replacement);
    Ok(r)
}

/// Mints the replacement as PROPOSED and flips the old one, with bidirectional links, in
/// ONE plan.
///
/// The two halves cannot be split across transactions: an `accepted` record carrying
/// `superseded_by:` is a `doctor` Error, so a plan that wrote only the back-link would
/// leave the repo failing CI until the second half landed. Two files in one plan is R-1's
/// window, and the authoritative file — the old decision, the one agents are still being
/// steered by — is ordered LAST.
pub fn plan_supersede(
    s: &Snapshot,
    f: &Facts,
    who: &HumanActor,
    id: &DecisionId,
    with: &str,
    m: &Minter,
) -> Result<Plan> {
    let _ = who;
    let old = s.decision(id)?;
    if old.fm.status != DecisionStatus::Accepted {
        return Err(KsError::gate(
            GateCode::DecisionNotAccepted,
            format!(
                "{id} is `{}` — only an accepted decision can be superseded",
                status_word(old.fm.status)
            ),
            fixes![
                fix!("kanspec accept {id}"),
                fix!("kanspec decide \"{}\"", with.trim()),
            ],
        ));
    }
    let title = with.trim();
    if title.is_empty() {
        return Err(KsError::invalid(
            "the replacement decision needs a title",
            fixes![fix!("kanspec supersede {id} --with \"...\"")],
        ));
    }
    let new = m.decision(title)?;

    let mut plan = Plan::empty();
    // The replacement inherits the scope it replaces, so the successor steers exactly the
    // paths the predecessor did until a human narrows it.
    plan.push(Op::CreateEntity {
        entity: EntityRef::Decision(new.clone()),
        contents: scaffold(
            &new,
            title,
            f.at,
            old.fm.source.as_deref(),
            &old.scope,
            Some(id),
            &madr(title),
        ),
    });
    plan.push(Op::SetFields {
        entity: EntityRef::Decision(id.clone()),
        sets: vec![
            (Key::Decision(DecisionKey::Status), Yv::s("superseded")),
            (
                Key::Decision(DecisionKey::SupersededBy),
                Yv::s(new.to_string()),
            ),
        ],
    });
    plan.mint(EntityRef::Decision(new));
    Ok(plan)
}

pub fn revoke(ctx: &Ctx, a: &RevokeArgs) -> Result<DecisionReport> {
    let (id, done) = flip(ctx, "revoke", &a.id, |s, f, who, id, _m| {
        plan_revoke(s, f, who, id, &a.why)
    })?;
    let mut r = flipped(ctx, id, &done, DecisionStatus::Revoked)?;
    r.why = Some(a.why.clone());
    Ok(r)
}

/// The human's kill switch — also `&HumanActor`.
pub fn plan_revoke(
    s: &Snapshot,
    f: &Facts,
    who: &HumanActor,
    id: &DecisionId,
    why: &str,
) -> Result<Plan> {
    let _ = who;
    let d = s.decision(id)?;
    let why = why.trim();
    if why.is_empty() {
        return Err(KsError::invalid(
            "revoking a standing rule is an asserted act — it needs a reason",
            fixes![fix!("kanspec revoke {id} --why \"...\"")],
        ));
    }
    if matches!(
        d.fm.status,
        DecisionStatus::Revoked | DecisionStatus::Superseded
    ) {
        return Err(KsError::gate(
            GateCode::DecisionNotStanding,
            format!(
                "{id} is already `{}` — it steers nobody",
                status_word(d.fm.status)
            ),
            fixes![fix!("kanspec rules")],
        ));
    }
    Ok(Plan::of(vec![
        Op::SetFields {
            entity: EntityRef::Decision(id.clone()),
            sets: vec![(Key::Decision(DecisionKey::Status), Yv::s("revoked"))],
        },
        // The reason lands in the record itself, where the next reader of the decision
        // meets it — a status flip with the story kept somewhere else is how a registry
        // becomes unreadable.
        Op::AppendSection {
            entity: EntityRef::Decision(id.clone()),
            heading: "## Consequences",
            line: format!(
                "- REVOKED {} by {}: {why}",
                f.at.date_naive(),
                f.actor.label()
            ),
        },
    ]))
}

impl Render for DecisionReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        let glyph = match self.status {
            DecisionStatus::Accepted => glyph::OK,
            DecisionStatus::Revoked => glyph::FAIL,
            _ => '▸',
        };
        let mut line = Line::new(
            glyph,
            format!(
                "{} — {} by {}",
                self.title,
                status_word(self.status),
                self.by
            ),
        )
        .id(&self.id);
        if let Some(r) = &self.replacement {
            line = line.dim(format!("· replaced by {r}"));
        }
        if let Some(why) = &self.why {
            line = line.dim(format!("· why: {why}"));
        }
        line.write(w, st)?;
        write_next(w, st, &self.next)
    }
}

/// The ONE spelling of a decision's status in human output, so `decide`, `accept`,
/// `supersede`, `revoke` and every refusal message agree on the word.
fn status_word(s: DecisionStatus) -> &'static str {
    match s {
        DecisionStatus::Proposed => "proposed",
        DecisionStatus::Accepted => "accepted",
        DecisionStatus::Superseded => "superseded",
        DecisionStatus::Revoked => "revoked",
    }
}

// ── why ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct WhyReport {
    pub anchor: String,
    pub rule: Option<String>,
    pub proposal: Option<ProposalId>,
    pub item: Option<String>,
    pub tickets: Vec<TicketId>,
    pub prs: Vec<u64>,
    pub decisions: Vec<DecisionId>,
    pub next: Vec<String>,
}

/// Walks rule → `{p-xxxx}` token → proposal item → tickets → PR, from whichever end the
/// caller has: a rule anchor, an item anchor, a ticket, a decision or a quirk.
///
/// The chain deliberately stops at a CLOSED proposal's id: `Snapshot` holds closed
/// proposal ids and no closed bodies, so `why` can say *which* proposal shipped a rule
/// without any code path through which closed prose could reach a session (invariant 4).
pub fn why(ctx: &Ctx, a: &WhyArgs) -> Result<WhyReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let raw = a.anchor.trim();

    let mut r = WhyReport {
        anchor: raw.to_string(),
        rule: None,
        proposal: None,
        item: None,
        tickets: Vec::new(),
        prs: Vec::new(),
        decisions: Vec::new(),
        next: Vec::new(),
    };

    // Each id kind is accepted only if it names something that EXISTS — `DecisionId::parse`
    // cannot see that, and `why` must not report an empty chain for a typo'd id. A well-
    // formed id that resolves to nothing is the typed `not_found` at the end, not a
    // complaint that `D-zzzz` is no rule reference.
    let known = |raw: &str| -> Option<&'static str> {
        [
            ("decision", DecisionId::parse(raw).is_ok()),
            ("ticket", TicketId::parse(raw).is_ok()),
            ("quirk", QuirkId::parse(raw).is_ok()),
        ]
        .into_iter()
        .find_map(|(kind, ok)| ok.then_some(kind))
    };
    if let Ok(item) = ItemRef::parse(raw) {
        r.proposal = Some(item.proposal.clone());
        r.item = Some(format!("{}{}", item.kind.letter(), item.n));
        r.decisions = sourced_by(&snap, &item.to_string());
    } else if let Some(id) = DecisionId::parse(raw)
        .ok()
        .filter(|i| snap.decisions.contains_key(i))
    {
        let d = snap.decision(&id)?;
        r.rule = Some(d.fm.title.clone());
        r.decisions = std::iter::once(id)
            .chain(d.fm.supersedes.clone())
            .chain(d.fm.superseded_by.clone())
            .collect();
        if let Some(src) = &d.fm.source {
            r.item = Some(src.clone());
            r.proposal = ProposalId::parse(src.split('#').next().unwrap_or(src)).ok();
        }
    } else if let Some(id) = TicketId::parse(raw)
        .ok()
        .filter(|i| snap.tickets.contains_key(i))
    {
        let t = snap.ticket(&id)?;
        r.rule = Some(t.fm.title.clone());
        r.proposal = t.fm.proposal.clone();
        r.item = t.fm.item.clone();
        r.tickets = vec![id.clone()];
        r.prs = t.fm.pr.into_iter().collect();
        r.decisions = sourced_by(&snap, id.as_str());
    } else if let Some(id) = QuirkId::parse(raw)
        .ok()
        .filter(|i| snap.quirks.contains_key(i))
    {
        let q = snap.quirk(&id)?;
        r.rule = Some(q.fm.title.clone());
        r.tickets = q.fm.source.clone().into_iter().collect();
    } else if let Some(kind) = known(raw) {
        return Err(KsError::not_found(
            kind,
            raw.to_string(),
            fixes![fix!("kanspec ls --all"), fix!("kanspec rules")],
        ));
    } else {
        let rr = RuleRef::parse(raw)?;
        let spec = snap.spec(&rr.spec)?;
        let anchor = rr.anchor();
        let rule = spec
            .rules
            .iter()
            .find(|x| x.anchor == anchor || x.anchor == rr.rule)
            .ok_or_else(|| {
                KsError::not_found(
                    "rule",
                    anchor.clone(),
                    fixes![
                        fix!("kanspec spec show {}", rr.spec),
                        fix!("kanspec spec grep \"{}\"", rr.rule),
                    ],
                )
            })?;
        r.rule = Some(rule.text.clone());
        r.proposal = rule.provenance.first().cloned();
        if let Some(p) = &r.proposal {
            r.decisions = snap
                .decisions
                .values()
                .filter(|d| {
                    d.fm.source
                        .as_deref()
                        .is_some_and(|s| s.starts_with(p.as_str()))
                })
                .map(|d| d.fm.id.clone())
                .collect();
        }
    }

    // Whatever the entry point, the tickets are the ones that carried the work, and their
    // PRs are where the diff was actually reviewed.
    if r.tickets.is_empty() {
        if let Some(p) = &r.proposal {
            for t in snap.tickets.values() {
                if t.fm.proposal.as_ref() == Some(p)
                    && r.item
                        .as_deref()
                        .is_none_or(|i| t.fm.item.as_deref() == Some(i) || t.fm.item.is_none())
                {
                    r.tickets.push(t.fm.id.clone());
                }
            }
        }
    }
    if r.prs.is_empty() {
        r.prs = r
            .tickets
            .iter()
            .filter_map(|id| snap.tickets.get(id).and_then(|t| t.fm.pr))
            .collect();
    }

    // Never hand back the anchor the caller already typed: a fix that re-runs the command
    // you just ran is not a next step.
    let onward = r.decisions.iter().find(|d| d.as_str() != raw);
    r.next = match (&r.proposal, onward) {
        (_, Some(d)) => vec![format!("{} why {d}", ctx.invoked_as)],
        (Some(_), None) => vec![format!("{} rules", ctx.invoked_as)],
        _ => vec![format!("{} rules --audit", ctx.invoked_as)],
    };
    Ok(r)
}

fn sourced_by(s: &Snapshot, src: &str) -> Vec<DecisionId> {
    s.decisions
        .values()
        .filter(|d| d.fm.source.as_deref() == Some(src))
        .map(|d| d.fm.id.clone())
        .collect()
}

impl Render for WhyReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        writeln!(
            w,
            " {}",
            crate::out::paint(&self.anchor, Color::Bold, st.color)
        )?;
        if let Some(rule) = &self.rule {
            // `record`, not `rule`: the same field carries a spec rule's text, a decision's
            // title, a ticket's title or a quirk's title, depending on where the walk
            // started. The JSON key stays `rule` — it is the contract's name for it.
            writeln!(w, "   record     {rule}")?;
        }
        match (&self.proposal, &self.item) {
            (Some(p), Some(i)) => writeln!(w, "   proposal   {p} · item {i}")?,
            (Some(p), None) => writeln!(w, "   proposal   {p}")?,
            (None, Some(i)) => writeln!(w, "   source     {i}")?,
            (None, None) => writeln!(w, "   proposal   (none recorded)")?,
        }
        if !self.tickets.is_empty() {
            writeln!(w, "   tickets    {}", join(&self.tickets, " "))?;
        }
        if !self.prs.is_empty() {
            writeln!(
                w,
                "   prs        {}",
                join(self.prs.iter().map(|p| format!("#{p}")), " ")
            )?;
        }
        if !self.decisions.is_empty() {
            writeln!(w, "   decisions  {}", join(&self.decisions, " "))?;
        }
        write_next(w, st, &self.next)
    }
}

// ── shared ───────────────────────────────────────────────────────────────────

fn minted_decision(done: &Committed) -> Result<DecisionId> {
    first_minted(done, "decision", |e| match e {
        EntityRef::Decision(id) => Some(id.clone()),
        _ => None,
    })
}

/// `--from p-7de2#p1` or `--from t-9c41`: both must resolve, and a closed proposal counts
/// as resolved — its id is remembered even though its body is never loaded.
fn check_source(s: &Snapshot, src: &str) -> Result<()> {
    if let Ok(item) = ItemRef::parse(src) {
        let p = &item.proposal;
        if s.proposals.contains_key(p) || s.closed_ids.contains(p.as_str()) {
            return Ok(());
        }
        return Err(KsError::not_found(
            "proposal",
            p.to_string(),
            fixes![fix!("kanspec status")],
        ));
    }
    if let Ok(t) = TicketId::parse(src) {
        s.ticket(&t)?;
        return Ok(());
    }
    Err(KsError::invalid(
        format!("`{src}` is neither a proposal item (`p-7de2#p1`) nor a ticket id"),
        fixes![fix!("kanspec ls --all"), fix!("kanspec status")],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ctx::Actor;
    use crate::model::{Decision, DecisionFm};
    use chrono::{NaiveDate, TimeZone, Utc};
    use std::collections::{BTreeMap, HashSet};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap()
    }

    fn facts_lit() -> Facts {
        Facts {
            actor: Actor::Human {
                name: "trevor".into(),
            },
            at: now(),
            invocation: "kanspec decide".into(),
        }
    }

    fn human() -> HumanActor {
        HumanActor::require(
            &Actor::Human {
                name: "trevor".into(),
            },
            "accept",
        )
        .unwrap()
    }

    fn snap_with(status: DecisionStatus) -> (Snapshot, DecisionId) {
        let mut s = Snapshot::empty(Config::default(), now());
        let id = DecisionId::parse("D-8c1a").unwrap();
        s.decisions.insert(
            id.clone(),
            Decision {
                fm: DecisionFm {
                    id: id.clone(),
                    title: "Redis only".into(),
                    status,
                    date: NaiveDate::from_ymd_opt(2026, 9, 2).unwrap(),
                    source: None,
                    scope: vec!["src/auth/**".into()],
                    supersedes: None,
                    superseded_by: None,
                    extra: BTreeMap::new(),
                },
                path: "x".into(),
                body: "## Decision\nRedis.\n".into(),
                scope: vec!["src/auth/**".into()],
            },
        );
        (s, id)
    }

    fn minter(s: &Snapshot) -> (HashSet<String>, u64, usize) {
        (s.taken_ids(), 7, 4)
    }

    #[test]
    fn decide_mints_a_proposed_decision_and_never_an_accepted_one() {
        let s = Snapshot::empty(Config::default(), now());
        let (taken, seed, w) = minter(&s);
        let m = Minter::new(&taken, seed, w);
        let a = DecideArgs {
            title: "Rate-limit state lives in Redis only".into(),
            from: None,
            scope: vec!["src/auth/**".into()],
        };
        let p = plan_decide(&s, &facts_lit(), &a, &m).unwrap();
        let Op::CreateEntity { contents, .. } = &p.ops[0] else {
            panic!("expected a CreateEntity, got {:?}", p.ops[0]);
        };
        assert!(contents.contains("status: proposed"), "{contents}");
        assert!(!contents.contains("status: accepted"));
        assert_eq!(p.minted.len(), 1);
    }

    #[test]
    fn accepting_something_already_accepted_is_a_typed_refusal() {
        let (s, id) = snap_with(DecisionStatus::Accepted);
        let e = plan_accept(&s, &human(), &id)
            .err()
            .expect("an accepted decision cannot be accepted again");
        assert_eq!(e.code(), Some("decision_not_proposed"));
    }

    #[test]
    fn accept_flips_only_the_status_so_the_body_freezes() {
        let (s, id) = snap_with(DecisionStatus::Proposed);
        let p = plan_accept(&s, &human(), &id).unwrap();
        assert_eq!(p.ops.len(), 1);
        let Op::SetFields { sets, .. } = &p.ops[0] else {
            panic!("expected SetFields");
        };
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].0.as_str(), "status");
    }

    #[test]
    fn supersede_writes_the_status_flip_and_the_back_link_in_one_plan() {
        let (s, id) = snap_with(DecisionStatus::Accepted);
        let (taken, seed, w) = minter(&s);
        let m = Minter::new(&taken, seed, w);
        let p = plan_supersede(&s, &facts_lit(), &human(), &id, "Redis and Postgres", &m).unwrap();
        // An `accepted` record carrying `superseded_by:` is a doctor Error, so the two
        // halves must land together or not at all.
        let sets = p.ops.iter().find_map(|o| match o {
            Op::SetFields { sets, .. } => Some(sets),
            _ => None,
        });
        let sets = sets.expect("the old decision is flipped");
        let keys: Vec<&str> = sets.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["status", "superseded_by"]);
        let Op::CreateEntity { contents, .. } = &p.ops[0] else {
            panic!("the replacement is minted first");
        };
        assert!(contents.contains("status: proposed"), "{contents}");
        assert!(
            contents.contains(&format!("supersedes: {id}")),
            "{contents}"
        );
    }

    #[test]
    fn superseding_something_that_never_bound_anyone_is_refused() {
        let (s, id) = snap_with(DecisionStatus::Proposed);
        let (taken, seed, w) = minter(&s);
        let m = Minter::new(&taken, seed, w);
        let e = plan_supersede(&s, &facts_lit(), &human(), &id, "x", &m)
            .err()
            .expect("a proposed decision cannot be superseded");
        assert_eq!(e.code(), Some("decision_not_accepted"));
    }

    #[test]
    fn revoke_records_the_reason_in_the_record_itself() {
        let (s, id) = snap_with(DecisionStatus::Accepted);
        let p = plan_revoke(&s, &facts_lit(), &human(), &id, "Redis is gone").unwrap();
        assert!(p.ops.iter().any(|o| matches!(
            o,
            Op::AppendSection { heading, line, .. }
                if *heading == "## Consequences" && line.contains("Redis is gone")
        )));
    }

    #[test]
    fn revoking_with_an_empty_reason_is_refused() {
        let (s, id) = snap_with(DecisionStatus::Accepted);
        assert!(plan_revoke(&s, &facts_lit(), &human(), &id, "   ").is_err());
    }
}
