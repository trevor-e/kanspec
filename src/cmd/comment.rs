//! `comments / comment add|reply|resolve / promote / expire` — the review loop.
//!
//! Comments travel browser -> localhost POST -> in-repo JSONL -> `--json` CLI read
//! (invariant 7). Never a clipboard hop; never server-only memory. The `quote` field
//! captures the item's text at comment time, so threads survive edits and a deleted item
//! moves its thread to a visible orphan tray instead of losing it (invariant 5).
//!
//! Owner: **V2**, v0.2.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cli::{CommentArgs, CommentCommand, CommentsArgs, ExpireArgs, PromoteArgs};
use crate::cmd::proposal::{live, next_lines, proposal_or_refuse};
use crate::cmd::ticket::facts;
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::ids::{CommentId, ItemRef, ProposalId};
use crate::model::{CommentOp, CommentOpKind, Proposal, Snapshot};
use crate::out::{glyph, Color, Line, Render, Style};
use crate::plan::{EntityRef, Op, Plan};
use crate::store::Store;
use crate::{fix, fixes};

#[derive(Debug, Serialize)]
pub struct CommentsReport {
    pub threads: Vec<Thread>,
    /// threads whose target item no longer exists — visible, never silently lost
    pub orphaned: Vec<Thread>,
    pub next: Vec<String>,
}

/// `{target, quote, body}` is the whole payload an agent needs — self-locating with zero
/// page context.
#[derive(Debug, Clone, Serialize)]
pub struct Thread {
    pub id: CommentId,
    pub target: String,
    pub quote: Option<String>,
    pub body: String,
    pub author: Option<String>,
    pub at: DateTime<Utc>,
    pub replies: Vec<Reply>,
    pub resolved: Option<String>,
    /// the stored quote no longer matches the item — "edited since — view diff"
    pub edited_since: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub by: String,
    pub body: String,
    pub at: DateTime<Utc>,
}

/// Which proposals a read is about. `None` means every open one.
///
/// A ticket id is accepted and resolved through its `proposal:` field, because "show me the
/// review feedback on what I am working on" is the question an agent standing on a ticket
/// actually has, and making it retype the proposal id is friction with no information in it.
fn scope_of(ctx: &Ctx, s: &Snapshot, id: Option<&str>) -> Result<Vec<ProposalId>> {
    let Some(raw) = id else {
        return Ok(s.proposals.keys().cloned().collect());
    };
    if let Ok(pid) = ProposalId::parse(raw) {
        // A CLOSED proposal is a different refusal from a missing one: its threads are
        // real history, they are just not reachable — closed proposals bind nothing.
        proposal_or_refuse(ctx, s, &pid)?;
        return Ok(vec![pid]);
    }
    let tid = crate::ids::TicketId::parse(raw)?;
    let t = s.ticket(&tid)?;
    match &t.fm.proposal {
        Some(pid) if s.proposals.contains_key(pid) => Ok(vec![pid.clone()]),
        _ => Err(KsError::not_found(
            "proposal for ticket",
            tid.to_string(),
            fixes![fix!("{} show {tid}", ctx.invoked_as)],
        )),
    }
}

/// Fold one proposal's op log into threads, splitting the ones whose target item is gone.
///
/// The ops arrive from `store::read_comments` already deduped by `(id, op, at)` and sorted
/// by time, so this is a pure regroup: the first `comment` row seeds a thread, `reply` rows
/// append in order, and the last `resolve` row closes it.
///
/// A `reply`/`resolve` whose `comment` row is missing is DROPPED rather than synthesised
/// into a headless thread — with `merge=union` a half-arrived thread is a normal transient
/// state of the file, and inventing a thread with no body would render as feedback nobody
/// wrote.
pub fn fold_threads(p: &Proposal, ops: &[CommentOp]) -> (Vec<Thread>, Vec<Thread>) {
    let mut by_id: BTreeMap<String, Thread> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    for op in ops {
        let key = op.id.as_str().to_string();
        match op.op {
            CommentOpKind::Comment => {
                let Some(target) = op.target.clone() else {
                    continue;
                };
                if by_id.contains_key(&key) {
                    continue;
                }
                order.push(key.clone());
                by_id.insert(
                    key,
                    Thread {
                        id: op.id.clone(),
                        target,
                        quote: op.quote.clone(),
                        body: op.body.clone().unwrap_or_default(),
                        author: op.author.clone(),
                        at: op.at,
                        replies: Vec::new(),
                        resolved: None,
                        edited_since: false,
                    },
                );
            }
            CommentOpKind::Reply => {
                if let Some(t) = by_id.get_mut(&key) {
                    t.replies.push(Reply {
                        by: op
                            .by
                            .clone()
                            .or_else(|| op.author.clone())
                            .unwrap_or_default(),
                        body: op.body.clone().unwrap_or_default(),
                        at: op.at,
                    });
                }
            }
            CommentOpKind::Resolve => {
                if let Some(t) = by_id.get_mut(&key) {
                    // The note is the whole point of a resolve, so an empty one still
                    // resolves the thread but renders as the honest empty string rather
                    // than leaving it open and losing the act.
                    t.resolved = Some(op.note.clone().unwrap_or_default());
                }
            }
        }
    }

    let (mut live, mut orphaned) = (Vec::new(), Vec::new());
    for key in order {
        let Some(mut t) = by_id.remove(&key) else {
            continue;
        };
        let item = ItemRef::parse(&t.target)
            .ok()
            .and_then(|r| p.items.iter().find(|i| i.id == r));
        match item {
            Some(i) => {
                // Invariant 5: the stored quote is what makes "edited since — view diff"
                // detectable without a second store. Trimmed on both sides so a reflowed
                // bullet does not read as an edit.
                t.edited_since = t
                    .quote
                    .as_deref()
                    .is_some_and(|q| q.trim() != i.text.trim());
                live.push(t);
            }
            // The item is gone. The thread is NOT dropped — it moves to a visible tray, so
            // deleting a bullet cannot quietly delete the objection to it.
            None => orphaned.push(t),
        }
    }
    (live, orphaned)
}

/// Read comments.jsonl, fold ops into threads, filter by `--unresolved`.
pub fn comments(ctx: &Ctx, a: &CommentsArgs) -> Result<CommentsReport> {
    ctx.require_initialized()?;
    let s = ctx.snapshot()?;
    let scope = scope_of(ctx, &s, a.id.as_deref())?;

    let (mut threads, mut orphaned) = (Vec::new(), Vec::new());
    for pid in &scope {
        let Some(p) = s.proposals.get(pid) else {
            continue;
        };
        let ops = s.comments.get(pid).map(Vec::as_slice).unwrap_or(&[]);
        let (live, orphan) = fold_threads(p, ops);
        threads.extend(live);
        orphaned.extend(orphan);
    }
    if a.unresolved {
        threads.retain(|t| t.resolved.is_none());
        // An orphaned thread nobody resolved is exactly the kind that gets lost, so
        // `--unresolved` narrows the tray the same way rather than hiding it.
        orphaned.retain(|t| t.resolved.is_none());
    }

    // The same count `approve` gates on — orphans included, so a tray full of unanswered
    // objections is never reported as nothing left to do.
    let open: usize = scope
        .iter()
        .map(|pid| crate::derive::unresolved(&s, pid))
        .sum();
    let next = if open > 0 {
        vec![format!(
            "{} comment resolve <cm-id> --note \"...\"",
            ctx.invoked_as
        )]
    } else {
        Vec::new()
    };
    Ok(CommentsReport {
        threads,
        orphaned,
        next,
    })
}

impl Render for CommentsReport {
    /// One block per thread — target, quote, body, replies — then the orphan tray.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        if self.threads.is_empty() && self.orphaned.is_empty() {
            writeln!(w, " ✓ no review threads")?;
            return Ok(());
        }
        for t in &self.threads {
            thread_block(t, w, st)?;
        }
        if !self.orphaned.is_empty() {
            // A tray with a heading, because the whole point is that these are visible.
            writeln!(
                w,
                "\n ORPHANED ({}) — the item these threads point at is gone",
                self.orphaned.len()
            )?;
            for t in &self.orphaned {
                thread_block(t, w, st)?;
            }
        }
        next_lines(&self.next, w, st)
    }
}

fn thread_block(t: &Thread, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
    let g = if t.resolved.is_some() {
        glyph::OK
    } else {
        '·'
    };
    let mut head = Line::new(g, t.target.clone()).id(t.id.clone());
    if let Some(a) = &t.author {
        head = head.dim(a.clone());
    }
    head.write(w, st)?;
    if let Some(q) = &t.quote {
        let tail = if t.edited_since {
            "  (edited since)"
        } else {
            ""
        };
        writeln!(
            w,
            "   {}{}",
            crate::out::paint(&format!("“{q}”"), Color::Dim, st.color),
            tail
        )?;
    }
    writeln!(w, "   {}", t.body)?;
    for r in &t.replies {
        writeln!(w, "   ↳ {}: {}", r.by, r.body)?;
    }
    if let Some(note) = &t.resolved {
        writeln!(
            w,
            "   {}",
            crate::out::paint(&format!("resolved: {note}"), Color::Green, st.color)
        )?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct CommentReport {
    pub id: CommentId,
    pub op: &'static str,
    pub target: Option<String>,
    pub next: Vec<String>,
}

/// Find the proposal that owns thread `cm`, and the thread's target — a reply names only a
/// thread id, so the file it appends to has to be looked up.
fn thread_home<'a>(
    ctx: &Ctx,
    s: &'a Snapshot,
    cm: &CommentId,
) -> Result<(&'a Proposal, String, bool)> {
    for (pid, ops) in &s.comments {
        let Some(seed) = ops
            .iter()
            .find(|o| o.id == *cm && o.op == CommentOpKind::Comment)
        else {
            continue;
        };
        let Some(p) = s.proposals.get(pid) else {
            continue;
        };
        let resolved = ops
            .iter()
            .any(|o| o.id == *cm && o.op == CommentOpKind::Resolve);
        return Ok((p, seed.target.clone().unwrap_or_default(), resolved));
    }
    Err(KsError::not_found(
        "thread",
        cm.to_string(),
        fixes![fix!("{} comments", ctx.invoked_as)],
    ))
}

/// What a `comment` invocation resolved to before the lock. The `CommentId` for an `add`
/// is deliberately absent: ids are minted by the `Minter` INSIDE `transact`, against the
/// id set reloaded under the lock, so a concurrent write cannot hand out the same one.
struct Prepared {
    pid: ProposalId,
    path: std::path::PathBuf,
    kind: CommentOpKind,
    /// the thread's item anchor — written into the row on `add`, shown on every op
    target: String,
    /// present on `add` — the quote captured at comment time
    quote: Option<String>,
    body: Option<String>,
    note: Option<String>,
    /// present on `reply`/`resolve` — the thread being appended to
    existing: Option<CommentId>,
}

fn prepare(ctx: &Ctx, s: &Snapshot, a: &CommentArgs) -> Result<Prepared> {
    match &a.cmd {
        CommentCommand::Add { target, body } => {
            let item = ItemRef::parse(target)?;
            let p = proposal_or_refuse(ctx, s, &item.proposal)?;
            // A thread on an item that does not exist would be born orphaned. Refusing
            // here is what keeps the orphan tray meaningful: everything in it USED to
            // point at something.
            let found = p.items.iter().find(|i| i.id == item).ok_or_else(|| {
                KsError::not_found(
                    "item",
                    item.to_string(),
                    fixes![fix!("{} comments {}", ctx.invoked_as, item.proposal)],
                )
            })?;
            Ok(Prepared {
                pid: item.proposal.clone(),
                path: ctx.layout.comments_jsonl(&p.dir),
                kind: CommentOpKind::Comment,
                target: item.to_string(),
                // Invariant 5: the item's text AT COMMENT TIME. This is the whole
                // mechanism behind "edited since" and the orphan tray.
                quote: Some(found.text.clone()),
                body: Some(body.clone()),
                note: None,
                existing: None,
            })
        }
        CommentCommand::Reply { id, body } => {
            let cm = CommentId::parse(id)?;
            let (p, target, _) = thread_home(ctx, s, &cm)?;
            Ok(Prepared {
                pid: p.fm.id.clone(),
                path: ctx.layout.comments_jsonl(&p.dir),
                kind: CommentOpKind::Reply,
                target,
                quote: None,
                body: Some(body.clone()),
                note: None,
                existing: Some(cm),
            })
        }
        CommentCommand::Resolve { id, note } => {
            let cm = CommentId::parse(id)?;
            let (p, target, already) = thread_home(ctx, s, &cm)?;
            if already {
                // Not a silent second row: a resolve that changed nothing must not read
                // as one that did.
                return Err(KsError::conflict(
                    format!("{cm} is already resolved"),
                    fixes![fix!("{} comments --unresolved", ctx.invoked_as)],
                ));
            }
            Ok(Prepared {
                pid: p.fm.id.clone(),
                path: ctx.layout.comments_jsonl(&p.dir),
                kind: CommentOpKind::Resolve,
                target,
                quote: None,
                body: None,
                note: Some(note.clone()),
                existing: Some(cm),
            })
        }
    }
}

/// `Op::AppendJsonl` one row; `resolve` requires `--note`, which is what changed.
///
/// Every row is written through the same store transaction the CLI and the server both go
/// through (invariant 7) — the browser's POST is not a second write path, it calls this.
pub fn comment(ctx: &Ctx, a: &CommentArgs) -> Result<CommentReport> {
    ctx.require_initialized()?;
    let s = ctx.snapshot()?;
    let prep = prepare(ctx, &s, a)?;

    // A `comment` row seeds a thread; a `reply`/`resolve` row acts on one. The seed carries
    // the target and the `author`; a later act carries `by` — the two roles the DESIGN.md
    // JSONL keeps apart.
    let seeds = matches!(prep.kind, CommentOpKind::Comment);
    let mut minted: Option<CommentId> = None;
    let done = Store::open(ctx).transact(None, &ctx.invocation(), |_s, m| {
        let id = match &prep.existing {
            Some(cm) => cm.clone(),
            None => m.comment(prep.body.as_deref().unwrap_or_default())?,
        };
        let row = CommentOp {
            id: id.clone(),
            op: prep.kind,
            target: seeds.then(|| prep.target.clone()),
            quote: prep.quote.clone(),
            body: prep.body.clone(),
            note: prep.note.clone(),
            author: seeds.then(|| ctx.actor.label()),
            by: (!seeds).then(|| ctx.actor.label()),
            at: ctx.now,
        };
        let line = serde_json::to_string(&row).map_err(|e| {
            KsError::internal(anyhow::anyhow!("cannot encode the comment row: {e}"))
        })?;
        minted = Some(id);
        Ok(Plan::of(vec![Op::AppendJsonl {
            path: prep.path.clone(),
            line,
        }]))
    })?;
    let id = minted
        .ok_or_else(|| KsError::internal(anyhow::anyhow!("`comment` minted no thread id")))?;

    // The open count describes the file as it now is — `Committed` carries the post-write
    // snapshot — and it is the same count `approve` gates on, orphans included.
    let pid = prep.pid;
    let open = crate::derive::unresolved(&done.snapshot, &pid);

    Ok(CommentReport {
        id,
        op: match prep.kind {
            CommentOpKind::Comment => "comment",
            CommentOpKind::Reply => "reply",
            CommentOpKind::Resolve => "resolve",
        },
        target: Some(prep.target),
        next: if open > 0 {
            vec![format!("{} comments {pid} --unresolved", ctx.invoked_as)]
        } else {
            vec![format!("{} approve {pid}", ctx.invoked_as)]
        },
    })
}

impl Render for CommentReport {
    /// `▸ cm-88f1 resolved` plus the remaining open-thread count.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        let past = match self.op {
            "comment" => "commented",
            "reply" => "replied",
            _ => "resolved",
        };
        let mut l = Line::new('▸', past).id(self.id.clone());
        if let Some(t) = &self.target {
            l = l.dim(t.clone());
        }
        l.fix(self.next.first().cloned().unwrap_or_default())
            .write(w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct PromoteReport {
    pub item: ItemRef,
    pub record: String,
    /// always `proposed` for a decision — agents never self-accept (invariant 8)
    pub status: String,
    pub url: Option<String>,
    pub next: Vec<String>,
}

/// Resolve a prescription anchor to its proposal and item, refusing the three ways it can
/// be wrong: unknown proposal, closed proposal, missing item.
fn prescription<'a>(
    ctx: &Ctx,
    s: &'a Snapshot,
    raw: &str,
) -> Result<(&'a Proposal, ItemRef, String)> {
    let item = ItemRef::parse(raw)?;
    let p = proposal_or_refuse(ctx, s, &item.proposal)?;
    let found = p.items.iter().find(|i| i.id == item).ok_or_else(|| {
        KsError::not_found(
            "item",
            item.to_string(),
            fixes![fix!("{} close {}", ctx.invoked_as, item.proposal)],
        )
    })?;
    Ok((p, item, found.text.clone()))
}

/// Append one disposition to a proposal's `ledger:`, preserving what is already there.
///
/// The ledger is the ONE place `close` and `doctor::check_ledger_complete` both read, via
/// `derive::dispositioned` — so an act that dispositions an item must land here or the gate
/// will ask for it again.
fn ledger_op(p: &Proposal, entry: String) -> Op {
    let mut ledger = p.fm.ledger.clone();
    ledger.push(entry);
    Op::SetFields {
        entity: EntityRef::Proposal(p.fm.id.clone()),
        sets: vec![(
            crate::keys::Key::Proposal(crate::keys::ProposalKey::Ledger),
            crate::fm::Yv::list(ledger),
        )],
    }
}

/// The standing record a promotion minted — whichever of decision or quirk the plan made.
/// Read back off the plan rather than trusted from the arm that built it, so the ledger
/// line and the report can never name different ids.
fn minted_record(minted: &[EntityRef]) -> Result<String> {
    minted
        .iter()
        .find(|e| matches!(e, EntityRef::Decision(_) | EntityRef::Quirk(_)))
        .map(EntityRef::id)
        .ok_or_else(|| KsError::internal(anyhow::anyhow!("`promote` minted no record")))
}

/// Mint the standing record with `source:` pre-filled from the item anchor; a decision
/// lands PROPOSED (invariant 8 — agents never self-accept).
pub fn promote(ctx: &Ctx, a: &PromoteArgs) -> Result<PromoteReport> {
    ctx.require_initialized()?;
    let s = ctx.snapshot()?;
    let (p, item, text) = prescription(ctx, &s, &a.item)?;

    if crate::derive::dispositioned(&p.fm.ledger, &item) {
        return Err(KsError::conflict(
            format!("{item} is already dispositioned"),
            fixes![fix!("{} close {}", ctx.invoked_as, item.proposal)],
        ));
    }
    // Compile the globs BEFORE the lock: a record whose scope cannot compile steers
    // nobody, and `prime` would silently never inject it.
    crate::rulesdoc::Scope::of(&a.scope)?;

    let pid = item.proposal.clone();
    let anchor = item.to_string();
    let title = text.trim().to_string();
    if title.is_empty() {
        return Err(KsError::invalid(
            format!("{item} has no text to promote"),
            fixes![fix!("{} comments {pid}", ctx.invoked_as)],
        ));
    }

    let (record, status, url, next) = match a.as_kind {
        crate::cli::PromoteKind::Spec => {
            // NOT a gap. DESIGN.md's spec workflow is that a rule bullet is written on the
            // IMPLEMENTATION branch carrying its `{p-xxxx}` token, and `close` then
            // auto-recognises it as `shipped`. Minting a rule here would create a second,
            // competing write path for the one record type that is deliberately edited as
            // ordinary code and reviewed in the PR.
            return Err(KsError::gate(
                "promote_spec_ships_in_code",
                format!(
                    "a spec rule is written on the branch that implements it, not minted \
                     here — add the bullet with the exact item token {{{item}}} (a bare \
                     {{{pid}}} names the proposal, not this prescription) and `close` will \
                     recognise {item} as shipped"
                ),
                fixes![
                    fix!("add `- [<spec>.<rule>] … {{{item}}}` in .kanspec/specs/"),
                    fix!("{} rules --audit", ctx.invoked_as),
                    fix!("{} close {pid}", ctx.invoked_as),
                ],
            ));
        }
        crate::cli::PromoteKind::Decision => {
            let args = crate::cli::DecideArgs {
                title: title.clone(),
                from: Some(anchor.clone()),
                scope: a.scope.clone(),
            };
            let f = facts(ctx);
            let done = Store::open(ctx).transact(None, &ctx.invocation(), |sn, m| {
                let mut plan = crate::cmd::decision::plan_decide(sn, &f, &args, m)?;
                let id = minted_record(&plan.minted)?;
                plan.push(ledger_op(live(sn, &pid)?, format!("{item} promoted→{id}")));
                Ok(plan)
            })?;
            let id = minted_record(&done.minted)?;
            // Invariant 8: a promoted decision is PROPOSED. An agent running `promote`
            // has not thereby accepted anything — a human does that on its own page.
            (
                id.clone(),
                "proposed",
                Some(format!("http://127.0.0.1:{}/d/{id}", ctx.cfg.port)),
                format!("{} accept {id}", ctx.invoked_as),
            )
        }
        crate::cli::PromoteKind::Quirk => {
            let scope = a.scope.clone();
            let done = Store::open(ctx).transact(None, &ctx.invocation(), |sn, m| {
                let qid = m.quirk(&title)?;
                let mut plan = Plan::of(vec![Op::CreateEntity {
                    entity: EntityRef::Quirk(qid.clone()),
                    // A quirk's `source:` is typed `Option<TicketId>` — it records the
                    // TICKET where the landmine was learned, so a proposal anchor has no
                    // slot in it. The ledger entry below is where this promotion's
                    // provenance actually lives.
                    contents: crate::cmd::quirk::scaffold(
                        &qid,
                        &title,
                        &scope,
                        crate::model::Severity::Landmine,
                        None,
                    ),
                }]);
                plan.mint(EntityRef::Quirk(qid.clone()));
                plan.push(ledger_op(live(sn, &pid)?, format!("{item} promoted→{qid}")));
                Ok(plan)
            })?;
            (
                minted_record(&done.minted)?,
                "active",
                None,
                format!("{} quirks", ctx.invoked_as),
            )
        }
    };
    crate::project::regenerate(ctx)?;
    Ok(PromoteReport {
        item,
        record,
        status: status.to_string(),
        url,
        next: vec![next],
    })
}

impl Render for PromoteReport {
    /// `▸ D-8c1a created (proposed) · source p-7de2#p1 · accept: <url>`.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        let mut l = Line::new('▸', format!("{} created", self.record))
            .dim(format!("({}) · source {}", self.status, self.item));
        if let Some(u) = &self.url {
            l = l.url(format!("accept: {u}"));
        }
        l.write(w, st)?;
        next_lines(&self.next, w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct ExpireReport {
    pub item: ItemRef,
    pub reason: String,
    pub next: Vec<String>,
}

/// Record the expiry disposition — the `(temp until t-x)` half of expire-or-promote.
///
/// `close` auto-expires a temp whose guard ticket git says has landed, so this verb is for
/// the other case: a prescription that stopped applying for a reason a human has to state.
pub fn expire(ctx: &Ctx, a: &ExpireArgs) -> Result<ExpireReport> {
    ctx.require_initialized()?;
    let s = ctx.snapshot()?;
    let (p, item, _) = prescription(ctx, &s, &a.item)?;
    if crate::derive::dispositioned(&p.fm.ledger, &item) {
        return Err(KsError::conflict(
            format!("{item} is already dispositioned"),
            fixes![fix!("{} close {}", ctx.invoked_as, item.proposal)],
        ));
    }
    let reason = a.reason.trim().to_string();
    if reason.is_empty() {
        return Err(KsError::invalid(
            format!("{item} needs a reason it no longer applies"),
            fixes![fix!("{} expire {item} --reason \"...\"", ctx.invoked_as)],
        ));
    }
    let entry = format!("{item} expired ({reason})");
    let pid = item.proposal.clone();
    Store::open(ctx).transact(None, &ctx.invocation(), |sn, _m| {
        Ok(Plan::of(vec![ledger_op(live(sn, &pid)?, entry.clone())]))
    })?;
    Ok(ExpireReport {
        next: vec![format!("{} close {pid}", ctx.invoked_as)],
        item,
        reason,
    })
}

impl Render for ExpireReport {
    /// `✕ p-7de2#p2 expired — <reason>`.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::new(glyph::FAIL, "expired")
            .id(self.item.to_string())
            .dim(self.reason.clone())
            .fix(self.next.first().cloned().unwrap_or_default())
            .write(w, st)
    }
}
