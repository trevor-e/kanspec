//! `propose / review / approve / close / abandon` — the v0.2 review centrepiece.
//!
//! `close` is THE gate: it refuses until every `[cN]` and `[pN]` item has a disposition,
//! then sets `status: closed` **and** moves the directory to `proposals/closed/` (both, so
//! `doctor` can verify they agree). After that there is no code path by which closed prose
//! reaches a future session — `Snapshot` cannot hold a closed proposal's body.
//!
//! Owner: **V2**, v0.2.

use serde::Serialize;

use crate::cli::{AbandonArgs, ApproveArgs, CloseArgs, ProposeArgs, ReviewArgs};
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::fm::{self, Yv};
use crate::ids::{ItemRef, ProposalId, SpecName, TicketId};
use crate::keys::{Key, ProposalKey};
use crate::model::{Proposal, Snapshot};
use crate::out::{glyph, Line};
use crate::plan::{EntityRef, Op, Plan};
use crate::store::Store;
use crate::model::ProposalStatus;
use crate::out::{Render, Style};
use crate::{fix, fixes};

#[derive(Debug, Serialize)]
pub struct ProposeReport {
    pub id: ProposalId,
    pub title: String,
    pub status: ProposalStatus,
    pub path: String,
    pub next: Vec<String>,
}

/// The one-page format: Why / Changes / Prescriptions / Tickets, and nothing else. No
/// design.md, no tasks.md, no delta-spec files, no SHALL grammar — a one-line tweak is a
/// five-line proposal.
///
/// The `[cN]`/`[pN]`/`[tN]` ids are VISIBLE TEXT because they are the comment anchors and
/// the disposition keys; the scaffold seeds `c1` so the first bullet is written in the
/// shape the rest of the machinery reads.
fn scaffold(id: &ProposalId, title: &str, specs: &[SpecName], created: chrono::NaiveDate) -> String {
    format!(
        "---\nid: {id}\ntitle: {}\nstatus: draft\nspecs: {}\napproved: null\nledger: []\n\
         created: {created}\n---\n\
         ## Why\n\n\
         ## Changes\n- [c1] \n\n\
         ## Prescriptions\n\n\
         ## Tickets\n",
        fm::emit(&Yv::s(title), false),
        fm::emit(
            &Yv::list(specs.iter().map(ToString::to_string).collect::<Vec<_>>()),
            true
        ),
    )
}

/// Mint the `p-` id and scaffold proposal.md.
pub fn propose(ctx: &Ctx, a: &ProposeArgs) -> Result<ProposeReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;

    // Fail on an unknown capability HERE, before the lock: a proposal whose `specs:` names
    // nothing is one whose review page can inline no spec context, and finding that out at
    // review time is finding it out too late.
    let mut specs = Vec::new();
    for raw in &a.spec {
        let n = SpecName::parse(raw)?;
        if !snap.specs.contains_key(&n) {
            return Err(KsError::not_found(
                "spec",
                n.to_string(),
                fixes![fix!("{} spec new {n}", ctx.invoked_as)],
            ));
        }
        specs.push(n);
    }

    let title = a.title.clone();
    let created = ctx.now.date_naive();
    let mut minted: Option<ProposalId> = None;
    Store::open(ctx).transact(None, &ctx.invocation(), |_s, m| {
        let id = m.proposal(&title)?;
        let contents = scaffold(&id, &title, &specs, created);
        // The directory carries the slug so a teammate browsing the repo reads
        // `p-7de2-login-rate-limiting/`, not `p-7de2/`.
        let slug = crate::ids::slug(&title);
        minted = Some(id.clone());
        let mut plan = Plan::of(vec![Op::CreateProposal {
            id: id.clone(),
            slug,
            contents,
        }]);
        plan.mint(EntityRef::Proposal(id));
        Ok(plan)
    })?;
    let id = minted
        .ok_or_else(|| KsError::internal(anyhow::anyhow!("`propose` minted no proposal")))?;

    let after = ctx.snapshot()?;
    let path = after
        .proposals
        .get(&id)
        .map(|p| rel(ctx, &ctx.layout.proposal_md(&p.dir)))
        .unwrap_or_default();

    Ok(ProposeReport {
        status: crate::model::ProposalStatus::Draft,
        path,
        next: vec![
            format!("edit the Changes bullets, then {} review {id}", ctx.invoked_as),
        ],
        title: a.title.clone(),
        id,
    })
}

fn rel(ctx: &Ctx, p: &std::path::Path) -> String {
    p.strip_prefix(ctx.repo.primary_root())
        .unwrap_or(p)
        .display()
        .to_string()
}

/// Resolve a proposal id, telling a CLOSED one apart from a missing one — closed proposals
/// bind nothing, and saying "not found" about one that is merely closed sends a human
/// looking for a file that is right there.
fn open_proposal<'a>(ctx: &Ctx, s: &'a Snapshot, raw: &str) -> Result<&'a Proposal> {
    let id = ProposalId::parse(raw)?;
    s.proposals.get(&id).ok_or_else(|| {
        if s.closed_ids.contains(id.as_str()) {
            KsError::gate(
                "proposal_closed",
                format!("{id} is closed — closed proposals bind nothing"),
                fixes![fix!("{} rules", ctx.invoked_as)],
            )
        } else {
            KsError::not_found(
                "proposal",
                id.to_string(),
                fixes![fix!("{} board", ctx.invoked_as)],
            )
        }
    })
}

impl Render for ProposeReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::new('▸', format!("{} created", self.title))
            .id(&self.id)
            .dim(format!("· draft · {}", self.path))
            .fix(self.next.first().cloned().unwrap_or_default())
            .write(w, st)
    }
}

/// The shared `→ next` tail every report in this module ends with.
fn next_lines(next: &[String], w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
    for n in next {
        crate::out::Line::new(crate::out::glyph::FIX, "next")
            .fix(n.as_str())
            .write(w, st)?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ReviewReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    pub url: String,
    pub unresolved: usize,
    pub next: Vec<String>,
}

/// How many threads on this proposal nobody has resolved — the number `approve` gates on
/// and `review` reports. Counts the ORPHAN tray too: a thread whose item was deleted is
/// the most, not the least, likely to be the one being dodged.
pub fn unresolved(s: &Snapshot, p: &Proposal) -> usize {
    let ops = s.comments.get(&p.fm.id).map(Vec::as_slice).unwrap_or(&[]);
    let (live, orphan) = crate::cmd::comment::fold_threads(p, ops);
    live.iter()
        .chain(orphan.iter())
        .filter(|t| t.resolved.is_none())
        .count()
}

/// draft -> review, then print the review page URL.
pub fn review(ctx: &Ctx, a: &ReviewArgs) -> Result<ReviewReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let p = open_proposal(ctx, &snap, &a.id)?;
    let id = p.fm.id.clone();

    use crate::model::ProposalStatus as S;
    match p.fm.status {
        // Re-review is how iteration works: comment -> agent edit -> re-review.
        S::Draft | S::Review => {}
        S::Approved => {
            return Err(KsError::gate(
                "already_approved",
                format!("{id} is already approved — reopening it would unstamp the approval"),
                fixes![fix!("{} close {id}", ctx.invoked_as)],
            ))
        }
        S::Closed | S::Abandoned => {
            return Err(KsError::gate(
                "proposal_terminal",
                format!("{id} is {} — it steers nothing", status_word(p.fm.status)),
                fixes![fix!("{} board", ctx.invoked_as)],
            ))
        }
    }

    let open = unresolved(&snap, p);
    if p.fm.status != S::Review {
        Store::open(ctx).transact(None, &ctx.invocation(), |_s, _m| {
            Ok(Plan::of(vec![Op::SetFields {
                entity: EntityRef::Proposal(id.clone()),
                sets: vec![(
                    Key::Proposal(ProposalKey::Status),
                    Yv::s("review"),
                )],
            }]))
        })?;
    }

    Ok(ReviewReport {
        url: format!("http://127.0.0.1:{}/p/{id}", ctx.cfg.port),
        unresolved: open,
        // The URL is only live while `up` is running, and saying so beats a dead link.
        next: vec![format!("{} up", ctx.invoked_as)],
        status: S::Review,
        id,
    })
}

fn status_word(s: crate::model::ProposalStatus) -> &'static str {
    use crate::model::ProposalStatus as S;
    match s {
        S::Draft => "draft",
        S::Review => "in review",
        S::Approved => "approved",
        S::Closed => "closed",
        S::Abandoned => "abandoned",
    }
}

impl Render for ReviewReport {
    /// The review URL and the open-thread count.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        let dim = match self.unresolved {
            0 => "· in review · no open threads".to_string(),
            1 => "· in review · 1 open thread".to_string(),
            n => format!("· in review · {n} open threads"),
        };
        Line::new('▸', "up for review")
            .id(&self.id)
            .dim(dim)
            .url(self.url.clone())
            .write(w, st)?;
        next_lines(&self.next, w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct ApproveReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    pub approved: String,
    /// the `[tN]` items minted into board tickets
    pub minted: Vec<TicketId>,
    pub next: Vec<String>,
}

/// REFUSE while unresolved threads exist (a human resolving one is the recorded waiver);
/// stamp who/when; mint the `[tN]` items.
pub fn approve(ctx: &Ctx, a: &ApproveArgs) -> Result<ApproveReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let p = open_proposal(ctx, &snap, &a.id)?;
    let id = p.fm.id.clone();

    use crate::model::ProposalStatus as S;
    match p.fm.status {
        S::Draft | S::Review => {}
        S::Approved => {
            return Err(KsError::conflict(
                format!("{id} is already approved{}", stamp_tail(p)),
                fixes![fix!("{} close {id}", ctx.invoked_as)],
            ))
        }
        S::Closed | S::Abandoned => {
            return Err(KsError::gate(
                "proposal_terminal",
                format!("{id} is {} — it steers nothing", status_word(p.fm.status)),
                fixes![fix!("{} board", ctx.invoked_as)],
            ))
        }
    }

    // THE gate. Unresolved feedback is the one thing approval must not paper over — and
    // the escape hatch is deliberately a recorded act, not a flag: a human who disagrees
    // resolves the thread with a note, and that note IS the waiver.
    let open = unresolved(&snap, p);
    if open > 0 {
        return Err(KsError::gate(
            "unresolved_threads",
            format!(
                "cannot approve {id} — {open} unresolved review thread{}",
                if open == 1 { "" } else { "s" }
            ),
            fixes![
                fix!("{} comments {id} --unresolved", ctx.invoked_as),
                fix!(
                    "{} comment resolve <cm-id> --note \"...\"",
                    ctx.invoked_as
                ),
            ],
        ));
    }

    // `[tN]` bullets become real board tickets. The text before the first `·` is the
    // title; the rest is the DESIGN.md `· S · deps: t-31aa` tail, which `new` already
    // knows how to take as flags.
    let wanted: Vec<(ItemRef, String, Vec<String>)> = p
        .items
        .iter()
        .filter(|i| i.id.kind == crate::ids::ItemKind::Ticket)
        .map(|i| {
            let (title, deps) = split_ticket_bullet(&i.text);
            (i.id.clone(), title, deps)
        })
        .collect();

    // A re-run must not mint a second copy of every ticket, so anything already linked to
    // this proposal counts as minted. The `[tN]` anchor rides in the ledger.
    let already: usize = snap
        .tickets
        .values()
        .filter(|t| t.fm.proposal.as_ref() == Some(&id))
        .count();

    let stamp = format!("{} {}", ctx.now.format("%Y-%m-%dT%H:%MZ"), ctx.actor.label());
    let spec = p.fm.specs.first().map(ToString::to_string);
    let pid = id.clone();
    let done = Store::open(ctx).transact(None, &ctx.invocation(), |sn, m| {
        let p = sn.proposals.get(&pid).ok_or_else(|| {
            KsError::not_found("proposal", pid.to_string(), fixes![fix!("kanspec board")])
        })?;
        let mut plan = Plan::of(vec![Op::SetFields {
            entity: EntityRef::Proposal(pid.clone()),
            sets: vec![
                (Key::Proposal(ProposalKey::Status), Yv::s("approved")),
                (Key::Proposal(ProposalKey::Approved), Yv::s(stamp.clone())),
            ],
        }]);
        let mut ledger = p.fm.ledger.clone();
        if already == 0 {
            let f = crate::plan::Facts {
                actor: ctx.actor.clone(),
                at: ctx.now,
                invocation: ctx.invocation(),
            };
            for (anchor, title, deps) in &wanted {
                let args = crate::cli::NewArgs {
                    title: title.clone(),
                    spec: spec.clone(),
                    proposal: Some(pid.to_string()),
                    deps: deps.clone(),
                    followup_of: None,
                    from: None,
                    // An approval is not a session standing on a ticket, so there is no
                    // `discovered_in` to stamp — these tickets come from the proposal.
                    no_link: true,
                };
                let sub = crate::cmd::ticket::plan_new(sn, &f, &args, m, None)?;
                for op in sub.ops {
                    plan.push(op);
                }
                for e in sub.minted {
                    if let EntityRef::Ticket(t) = &e {
                        ledger.push(format!("{anchor} minted→{t}"));
                    }
                    plan.mint(e);
                }
            }
        }
        if ledger != p.fm.ledger {
            plan.push(Op::SetFields {
                entity: EntityRef::Proposal(pid.clone()),
                sets: vec![(Key::Proposal(ProposalKey::Ledger), Yv::list(ledger))],
            });
        }
        Ok(plan)
    })?;

    let minted: Vec<TicketId> = done
        .minted
        .iter()
        .filter_map(|e| match e {
            EntityRef::Ticket(t) => Some(t.clone()),
            _ => None,
        })
        .collect();

    Ok(ApproveReport {
        status: S::Approved,
        approved: stamp,
        next: vec![
            format!("{} ready", ctx.invoked_as),
            format!("{} close {id}", ctx.invoked_as),
        ],
        minted,
        id,
    })
}

fn stamp_tail(p: &Proposal) -> String {
    p.fm.approved
        .as_deref()
        .map(|a| format!(" ({a})"))
        .unwrap_or_default()
}

/// `Rate-limit login endpoint · S · deps: t-31aa` -> title + dep ids.
///
/// The estimate segment is dropped on purpose: `new` has no estimate flag, and inventing a
/// place to put it would be a field the board never reads.
fn split_ticket_bullet(text: &str) -> (String, Vec<String>) {
    let mut title = text.trim();
    let mut deps = Vec::new();
    if let Some((head, rest)) = text.split_once('·') {
        title = head.trim();
        for seg in rest.split('·') {
            let seg = seg.trim();
            if let Some(list) = seg.strip_prefix("deps:") {
                deps.extend(
                    list.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToString::to_string),
                );
            }
        }
    }
    (title.to_string(), deps)
}

impl Render for ApproveReport {
    /// Who approved, when, and the minted ticket ids.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::new(glyph::OK, "approved")
            .id(&self.id)
            .dim(self.approved.clone())
            .write(w, st)?;
        for t in &self.minted {
            Line::new('○', "spawned").id(t.to_string()).write(w, st)?;
        }
        next_lines(&self.next, w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct CloseReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    /// the disposition ledger, stamped into the closed proposal's frontmatter
    pub ledger: Vec<Disposition>,
    pub moved_to: String,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum Disposition {
    /// auto-detected via the `{p-xxxx}` token
    Shipped {
        item: ItemRef,
        rule: String,
    },
    Promoted {
        item: ItemRef,
        record: String,
    },
    Followup {
        item: ItemRef,
        ticket: TicketId,
    },
    Dropped {
        item: ItemRef,
        note: String,
    },
    /// `(temp until t-x)` whose guard ticket landed
    Expired {
        item: ItemRef,
        reason: String,
    },
}

/// The evidence that a `[cN]` shipped: spec rule bullets carrying this proposal's token,
/// in a stable (spec, line) order.
///
/// `--shipped` is deliberately not a flag — shipped-ness is a fact about the spec corpus,
/// not an assertion a human gets to make — so this is the ONLY way a Change item is
/// recognised as landed.
///
/// Two grades of evidence, and the stronger one wins:
/// - **`{p-7de2#c1}`** names the exact item. The mapping is STATED, so the ledger records
///   what actually happened rather than a guess.
/// - **`{p-7de2}`** names only the proposal. There is no way to know which Change such a
///   rule implements, so these are paired with the leftover items in order — and when
///   there are fewer of them than items, the remainder stay unmet and the gate refuses.
///   It can mis-attribute WHICH rule shipped which change; it can never invent a change
///   that shipped, which is the property that matters.
struct Evidence {
    /// `p-7de2#c1` -> `auth#lockout`, from an item-level token
    exact: std::collections::BTreeMap<String, String>,
    /// rules carrying only the bare proposal token, in corpus order
    loose: Vec<String>,
}

fn shipped_rules(s: &Snapshot, id: &ProposalId) -> Evidence {
    let mut exact = std::collections::BTreeMap::new();
    let mut loose = Vec::new();
    for spec in s.specs.values() {
        for r in &spec.rules {
            let anchor = format!("{}#{}", spec.name, r.anchor);
            let named: Vec<_> = r.items.iter().filter(|i| i.proposal == *id).collect();
            if !named.is_empty() {
                for i in named {
                    exact.entry(i.to_string()).or_insert_with(|| anchor.clone());
                }
            } else if r.provenance.iter().any(|p| p == id) {
                loose.push(anchor);
            }
        }
    }
    Evidence { exact, loose }
}

/// A `(temp until t-x)` prescription whose guard ticket git says has landed. Merge state is
/// COMPUTED, never asserted, so an auto-expiry rests on the same evidence `done` does.
fn landed_guard(s: &Snapshot, item: &crate::model::Item) -> Option<TicketId> {
    let crate::model::Prescription::TempUntil(t) = item.prescription.as_ref()? else {
        return None;
    };
    let ticket = s.tickets.get(t)?;
    let fact = crate::derive::merge_fact(s, ticket)?;
    (fact.status == crate::cache::MergeStatus::Merged).then(|| t.clone())
}

/// THE close gate: enumerate every `[cN]`/`[pN]`, auto-recognise shipped items and
/// expirable temps, and REFUSE naming the exact command per unmet item.
pub fn close(ctx: &Ctx, a: &CloseArgs) -> Result<CloseReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let p = open_proposal(ctx, &snap, &a.id)?;
    let id = p.fm.id.clone();

    use crate::model::ProposalStatus as S;
    match p.fm.status {
        S::Approved => {}
        S::Abandoned | S::Closed => {
            return Err(KsError::gate(
                "proposal_terminal",
                format!("{id} is already {}", status_word(p.fm.status)),
                fixes![fix!("{} board", ctx.invoked_as)],
            ))
        }
        S::Draft | S::Review => {
            return Err(KsError::gate(
                "not_approved",
                format!(
                    "{id} is {} — close is the END of an approved proposal, not a way out \
                     of one",
                    status_word(p.fm.status)
                ),
                fixes![
                    fix!("{} approve {id}", ctx.invoked_as),
                    fix!("{} abandon {id} --why \"...\"", ctx.invoked_as),
                ],
            ))
        }
    }

    // 1 — the dispositions the human named on the command line.
    let mut ledger = p.fm.ledger.clone();
    let mut dispositions: Vec<Disposition> = Vec::new();
    let mut followups: Vec<(ItemRef, String)> = Vec::new();

    let resolve_item = |raw: &str| -> Result<ItemRef> {
        // Both spellings: the full anchor `p-7de2#c3` and the short `c3` the body uses.
        let anchor = if raw.contains('#') {
            raw.to_string()
        } else {
            format!("{id}#{raw}")
        };
        let item = ItemRef::parse(&anchor)?;
        if !p.items.iter().any(|i| i.id == item) {
            return Err(KsError::not_found(
                "item",
                item.to_string(),
                fixes![fix!("{} close {id}", ctx.invoked_as)],
            ));
        }
        Ok(item)
    };

    for raw in &a.followup {
        let item = resolve_item(raw)?;
        let text = p
            .items
            .iter()
            .find(|i| i.id == item)
            .map(|i| i.text.clone())
            .unwrap_or_default();
        followups.push((item, text));
    }
    for raw in &a.dropped {
        let item = resolve_item(raw)?;
        // clap already enforces `--dropped` requires `--note`; this is the value.
        let note = a.note.clone().unwrap_or_default();
        ledger.push(format!("{item} dropped ({note})"));
        dispositions.push(Disposition::Dropped { item, note });
    }

    // 2 — what the world already says. Shipped Changes are read off the spec corpus;
    // expirable temps off git merge state. Neither is a claim anybody typed.
    let evidence = shipped_rules(&snap, &id);
    let mut loose_left = evidence.loose.iter();

    let mut unmet: Vec<(ItemRef, String, Vec<String>)> = Vec::new();
    for i in &p.items {
        // `[tN]` items became tickets at approve; they are not close-gated.
        if i.id.kind == crate::ids::ItemKind::Ticket {
            continue;
        }
        // A `--followup` is dispositioned by THIS invocation: its ledger entry cannot be
        // written until the ticket is minted inside the transaction, so the gate has to
        // know about it from the command line, not from the ledger it is about to grow.
        if crate::derive::dispositioned(&ledger, &i.id)
            || followups.iter().any(|(f, _)| *f == i.id)
        {
            continue;
        }
        match i.id.kind {
            crate::ids::ItemKind::Change => {
                // An item-level token beats the positional fallback: it says which rule
                // shipped THIS change, so nothing has to be inferred.
                let named = evidence.exact.get(&i.id.to_string()).cloned();
                if let Some(rule) = named.or_else(|| loose_left.next().cloned()) {
                    ledger.push(format!("{} shipped→{rule}", i.id));
                    dispositions.push(Disposition::Shipped {
                        item: i.id.clone(),
                        rule,
                    });
                } else {
                    unmet.push((
                        i.id.clone(),
                        format!("\"{}\" — not found in any spec", i.text),
                        vec![
                            format!(
                                "{} close {id} --followup {}{}",
                                ctx.invoked_as,
                                i.id.kind.letter(),
                                i.id.n
                            ),
                            format!(
                                "{} close {id} --dropped {}{} --note \"...\"",
                                ctx.invoked_as,
                                i.id.kind.letter(),
                                i.id.n
                            ),
                        ],
                    ));
                }
            }
            crate::ids::ItemKind::Prescription => {
                if let Some(t) = landed_guard(&snap, i) {
                    let reason = format!("{t} landed");
                    ledger.push(format!("{} expired ({reason})", i.id));
                    dispositions.push(Disposition::Expired {
                        item: i.id.clone(),
                        reason,
                    });
                    continue;
                }
                let fixes = match &i.prescription {
                    Some(crate::model::Prescription::Promote(kind)) => vec![format!(
                        "{} promote {} --as {} --scope \"src/**\"",
                        ctx.invoked_as,
                        i.id,
                        promote_word(*kind)
                    )],
                    Some(crate::model::Prescription::TempUntil(t)) => vec![
                        format!("{} scan --confirm", ctx.invoked_as),
                        format!("{} expire {} --reason \"{t} is not landing\"", ctx.invoked_as, i.id),
                    ],
                    // An untyped prescription is a close blocker BY DESIGN: nobody can say
                    // what it was supposed to become, which is exactly the silent drop this
                    // gate exists to stop.
                    _ => vec![
                        format!("type it `(promote: decision)` or `(temp until t-x)` in the body"),
                        format!("{} expire {} --reason \"...\"", ctx.invoked_as, i.id),
                    ],
                };
                unmet.push((i.id.clone(), format!("\"{}\"", i.text), fixes));
            }
            crate::ids::ItemKind::Ticket => {}
        }
    }

    // 3 — a followup still needs its ticket, which is minted in the same transaction.
    if !unmet.is_empty() {
        let mut msg = format!(
            "cannot close {id} — {} item{} undispositioned:",
            unmet.len(),
            if unmet.len() == 1 { "" } else { "s" }
        );
        for (item, what, _) in &unmet {
            msg.push_str(&format!("\n  [{}{}] {what}", item.kind.letter(), item.n));
        }
        // Invariant 9: every refusal names its next command — here, the exact command for
        // each unmet item, which is what turns a twelve-item close into two keys.
        let mut flat: Vec<crate::error::Fix> = Vec::new();
        for (_, _, f) in &unmet {
            for one in f {
                flat.push(crate::error::Fix::cmd(one.clone()));
            }
        }
        flat.truncate(6);
        let head = flat.remove(0);
        return Err(KsError::gate(
            "undispositioned_items",
            msg,
            crate::error::Fixes::new(head, flat),
        ));
    }

    // 4 — close. `status: closed` AND the move to `proposals/closed/`, in ONE transaction,
    // because `doctor` cross-checks that the two agree and a half-applied close is exactly
    // the state it would flag.
    let dir = p.dir.clone();
    let dest = ctx
        .layout
        .proposals_closed_dir()
        .join(dir.file_name().unwrap_or_default());
    let spec = p.fm.specs.first().map(ToString::to_string);
    let pid = id.clone();
    let done = Store::open(ctx).transact(None, &ctx.invocation(), |sn, m| {
        let p = sn.proposals.get(&pid).ok_or_else(|| {
            KsError::not_found("proposal", pid.to_string(), fixes![fix!("kanspec board")])
        })?;
        let mut ledger = ledger.clone();
        let mut plan = Plan::empty();
        let f = crate::plan::Facts {
            actor: ctx.actor.clone(),
            at: ctx.now,
            invocation: ctx.invocation(),
        };
        for (item, text) in &followups {
            let args = crate::cli::NewArgs {
                title: text.clone(),
                spec: spec.clone(),
                // Deliberately NOT linked to the proposal being closed: a closed proposal
                // binds nothing, and a live ticket pointing into `proposals/closed/` is
                // the stale-consent shape this whole gate exists to prevent.
                proposal: None,
                deps: Vec::new(),
                followup_of: None,
                from: None,
                no_link: true,
            };
            let sub = crate::cmd::ticket::plan_new(sn, &f, &args, m, None)?;
            for op in sub.ops {
                plan.push(op);
            }
            for e in sub.minted {
                if let EntityRef::Ticket(t) = &e {
                    ledger.push(format!("{item} followup→{t}"));
                }
                plan.mint(e);
            }
        }
        // The ledger and the status ride the SAME `SetFields`, so a plan that stamps one
        // without the other is unavailable rather than merely wrong.
        plan.push(Op::SetFields {
            entity: EntityRef::Proposal(pid.clone()),
            sets: vec![
                (Key::Proposal(ProposalKey::Status), Yv::s("closed")),
                (Key::Proposal(ProposalKey::Ledger), Yv::list(ledger)),
            ],
        });
        plan.push(Op::MoveDir {
            from: dir.clone(),
            to: dest.clone(),
        });
        let _ = p;
        Ok(plan)
    })?;

    for (item, _) in &followups {
        if let Some(EntityRef::Ticket(t)) = done.minted.iter().find(|e| {
            matches!(e, EntityRef::Ticket(_))
        }) {
            dispositions.push(Disposition::Followup {
                item: item.clone(),
                ticket: t.clone(),
            });
        }
    }

    Ok(CloseReport {
        status: S::Closed,
        ledger: dispositions,
        moved_to: rel(ctx, &dest),
        next: vec![format!("{} rules", ctx.invoked_as)],
        id,
    })
}

fn promote_word(k: crate::model::PromoteAs) -> &'static str {
    match k {
        crate::model::PromoteAs::Decision => "decision",
        crate::model::PromoteAs::Spec => "spec",
        crate::model::PromoteAs::Quirk => "quirk",
    }
}

impl Render for CloseReport {
    /// The DESIGN.md ledger transcript, one `· cN <disposition>` per item.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::new(glyph::OK, "closed")
            .id(&self.id)
            .dim(format!("· {}", self.moved_to))
            .write(w, st)?;
        for d in &self.ledger {
            let (item, what) = match d {
                Disposition::Shipped { item, rule } => (item, format!("shipped→{rule}")),
                Disposition::Promoted { item, record } => (item, format!("promoted→{record}")),
                Disposition::Followup { item, ticket } => (item, format!("followup→{ticket}")),
                Disposition::Dropped { item, note } => (item, format!("dropped ({note})")),
                Disposition::Expired { item, reason } => (item, format!("expired ({reason})")),
            };
            writeln!(w, "   · {}{} {what}", item.kind.letter(), item.n)?;
        }
        next_lines(&self.next, w, st)
    }
}

#[derive(Debug, Serialize)]
pub struct AbandonReport {
    pub id: ProposalId,
    pub status: ProposalStatus,
    pub why: String,
    pub next: Vec<String>,
}

/// status -> abandoned with the recorded why; linked tickets are NOT touched.
///
/// Deliberately not `close`: abandoning makes no claim that the items were dispositioned,
/// so it must never stamp a ledger. It also leaves the directory where it is — only `close`
/// moves a proposal to `proposals/closed/`, because that move is what `doctor` cross-checks
/// against `status: closed`.
///
/// The `why` goes in the BODY, not frontmatter: `Key` is a closed enum by design (a derived
/// key must be unnameable), so a free-text reason has no frontmatter slot — and the body is
/// where a human reads it anyway.
pub fn abandon(ctx: &Ctx, a: &AbandonArgs) -> Result<AbandonReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let p = open_proposal(ctx, &snap, &a.id)?;
    let id = p.fm.id.clone();

    use crate::model::ProposalStatus as S;
    if matches!(p.fm.status, S::Closed | S::Abandoned) {
        return Err(KsError::gate(
            "proposal_terminal",
            format!("{id} is already {}", status_word(p.fm.status)),
            fixes![fix!("{} board", ctx.invoked_as)],
        ));
    }

    let why = a.why.clone();
    let line = format!("- {} · {}", ctx.now.date_naive(), why);
    Store::open(ctx).transact(None, &ctx.invocation(), |_s, _m| {
        Ok(Plan::of(vec![
            Op::SetFields {
                entity: EntityRef::Proposal(id.clone()),
                sets: vec![(Key::Proposal(ProposalKey::Status), Yv::s("abandoned"))],
            },
            Op::AppendSection {
                entity: EntityRef::Proposal(id.clone()),
                heading: "## Abandoned",
                line: line.clone(),
            },
        ]))
    })?;

    Ok(AbandonReport {
        status: S::Abandoned,
        why: a.why.clone(),
        // Linked tickets survive on purpose: work already claimed does not evaporate
        // because the proposal that suggested it did.
        next: vec![format!("{} ls --proposal {id}", ctx.invoked_as)],
        id,
    })
}

impl Render for AbandonReport {
    /// `✕ p-7de2 abandoned — <why>`.
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::new(glyph::FAIL, "abandoned")
            .id(&self.id)
            .dim(self.why.clone())
            .write(w, st)
    }
}
