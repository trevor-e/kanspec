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
use crate::error::{GateCode, KsError, Result};
use crate::fm::{self, Yv};
use crate::ids::{ItemRef, ProposalId, SpecName, TicketId};
use crate::keys::{Key, ProposalKey};
use crate::model::ProposalStatus;
use crate::model::{Proposal, Snapshot};
use crate::out::{glyph, Line};
use crate::out::{Render, Style};
use crate::plan::{EntityRef, Op, Plan};
use crate::store::Store;
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
fn scaffold(
    id: &ProposalId,
    title: &str,
    specs: &[SpecName],
    created: chrono::NaiveDate,
) -> String {
    format!(
        "---\nid: {id}\ntitle: {}\nstatus: draft\nspecs: {}\napproved: null\nledger: []\n\
         created: {created}\n---\n\
         ## Why\n\n\
         ## Changes\n- [c1] \n\n\
         ## Testing and verification\n\n\
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
    let done = Store::open(ctx).transact(None, &ctx.invocation(), |_s, m| {
        let id = m.proposal(&title)?;
        let contents = scaffold(&id, &title, &specs, created);
        // The directory carries the slug so a teammate browsing the repo reads
        // `p-7de2-login-rate-limiting/`, not `p-7de2/`.
        let slug = crate::ids::slug(&title);
        let mut plan = Plan::of(vec![Op::CreateProposal {
            id: id.clone(),
            slug,
            contents,
        }]);
        plan.mint(EntityRef::Proposal(id));
        Ok(plan)
    })?;
    let id = done
        .minted
        .iter()
        .find_map(|e| match e {
            EntityRef::Proposal(p) => Some(p.clone()),
            _ => None,
        })
        .ok_or_else(|| KsError::internal(anyhow::anyhow!("`propose` minted no proposal")))?;
    // `Committed` carries the post-write snapshot, read under the lock: no second load.
    let path = done
        .snapshot
        .proposals
        .get(&id)
        .map(|p| ctx.rel(&ctx.layout.proposal_md(&p.dir)))
        .unwrap_or_default();

    Ok(ProposeReport {
        status: ProposalStatus::Draft,
        path,
        next: vec![format!(
            "edit the Changes bullets, then {} review {id}",
            ctx.invoked_as
        )],
        title,
        id,
    })
}

/// Resolve a proposal id, telling a CLOSED one apart from a missing one — closed proposals
/// bind nothing, and saying "not found" about one that is merely closed sends a human
/// looking for a file that is right there. Every verb here and in `cmd::comment` refuses
/// through this one function, so the two refusals cannot drift apart.
pub(crate) fn proposal_or_refuse<'a>(
    ctx: &Ctx,
    s: &'a Snapshot,
    id: &ProposalId,
) -> Result<&'a Proposal> {
    s.proposals.get(id).ok_or_else(|| {
        if s.closed_ids.contains(id.as_str()) {
            KsError::gate(
                GateCode::ProposalClosed,
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

fn open_proposal<'a>(ctx: &Ctx, s: &'a Snapshot, raw: &str) -> Result<&'a Proposal> {
    proposal_or_refuse(ctx, s, &ProposalId::parse(raw)?)
}

/// The proposal as it stands INSIDE a transaction — re-read under the lock, so a plan
/// never extends a ledger somebody else just rewrote.
pub(crate) fn live<'a>(sn: &'a Snapshot, pid: &ProposalId) -> Result<&'a Proposal> {
    sn.proposals.get(pid).ok_or_else(|| {
        KsError::not_found("proposal", pid.to_string(), fixes![fix!("kanspec board")])
    })
}

/// `1 open thread` / `3 open threads` — the count-and-noun pair every gate message spells.
pub(crate) fn plural(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// The refusal every verb gives a closed or abandoned proposal. `already` picks the
/// wording: `close` and `abandon` say "is already closed", `review` and `approve` say it
/// steers nothing.
fn refuse_terminal(ctx: &Ctx, p: &Proposal, already: bool) -> Result<()> {
    use ProposalStatus as S;
    if !matches!(p.fm.status, S::Closed | S::Abandoned) {
        return Ok(());
    }
    let (id, word) = (&p.fm.id, status_word(p.fm.status));
    Err(KsError::gate(
        GateCode::ProposalTerminal,
        if already {
            format!("{id} is already {word}")
        } else {
            format!("{id} is {word} — it steers nothing")
        },
        fixes![fix!("{} board", ctx.invoked_as)],
    ))
}

/// Fold a `plan_new` sub-plan into `plan`, recording `<anchor> <word>→<t-id>` in `ledger`
/// for every ticket it minted. `approve` and `close` both mint tickets this way, and that
/// ledger line is the ONE record `page` and `derive::dispositioned` read back.
fn absorb(plan: &mut Plan, sub: Plan, ledger: &mut Vec<String>, anchor: &ItemRef, word: &str) {
    plan.ops.extend(sub.ops);
    for e in sub.minted {
        if let EntityRef::Ticket(t) = &e {
            ledger.push(format!("{anchor} {word}→{t}"));
        }
        plan.mint(e);
    }
}

/// `c3` — the short anchor the body uses and the transcript prints.
fn short(i: &ItemRef) -> String {
    format!("{}{}", i.kind.letter(), i.n)
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

/// The shared `→ next` tail every report in this module and `cmd::comment` ends with.
pub(crate) fn next_lines(
    next: &[String],
    w: &mut dyn std::io::Write,
    st: &Style,
) -> std::io::Result<()> {
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
    /// where `--export` wrote the static page
    pub exported: Option<String>,
    pub next: Vec<String>,
}

/// How many threads on this proposal nobody has resolved — the number `approve` gates on
/// and `review` reports.
///
/// Delegates to [`crate::derive::unresolved`] rather than counting again. `board`, `prime`
/// and `status` already read that one, and two definitions of the number a GATE turns on is
/// how you get `status` reporting "no open threads" while `approve` refuses. It counts a
/// thread whose target item was deleted, which is right: an orphaned objection is the one
/// most likely to be getting dodged, not the least.
pub fn unresolved(s: &Snapshot, p: &Proposal) -> usize {
    crate::derive::unresolved(s, &p.fm.id)
}

/// draft -> review, then print the review page URL.
pub fn review(ctx: &Ctx, a: &ReviewArgs) -> Result<ReviewReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let p = open_proposal(ctx, &snap, &a.id)?;
    let id = p.fm.id.clone();

    use crate::model::ProposalStatus as S;
    refuse_terminal(ctx, p, false)?;
    // Re-review is how iteration works: comment -> agent edit -> re-review. An approved
    // proposal is the exception, because reopening it would unstamp the approval.
    if p.fm.status == S::Approved {
        return Err(KsError::gate(
            GateCode::AlreadyApproved,
            format!("{id} is already approved — reopening it would unstamp the approval"),
            fixes![fix!("{} close {id}", ctx.invoked_as)],
        ));
    }

    let open = unresolved(&snap, p);
    let needs_move = p.fm.status != S::Review;
    let mut snap = snap;
    if needs_move {
        let done = Store::open(ctx).transact(None, &ctx.invocation(), |_s, _m| {
            Ok(Plan::of(vec![Op::SetFields {
                entity: EntityRef::Proposal(id.clone()),
                sets: vec![(Key::Proposal(ProposalKey::Status), Yv::s("review"))],
            }]))
        })?;
        snap = done.snapshot;
    }

    // `--export`: the page as one static file, so it can be read on a phone or pasted
    // into a PR when the loopback server is out of reach (t-f3a4). Comment-less on
    // purpose: threads still live on the served page, and a comment relayed by hand is
    // recorded as whoever relays it — see `via` on the row.
    let exported = match &a.export {
        None => None,
        Some(rel) => {
            let path = if rel.is_absolute() {
                rel.clone()
            } else {
                ctx.repo.here().join(rel)
            };
            let html = crate::board::render_page_html(&page_of(ctx, &snap, &id.to_string())?);
            let target = path.clone();
            Store::open(ctx).transact(None, &ctx.invocation(), move |_s, _m| {
                Ok(Plan::of(vec![Op::WriteGenerated {
                    path: target,
                    contents: html,
                }]))
            })?;
            Some(path.display().to_string())
        }
    };

    Ok(ReviewReport {
        url: format!("http://127.0.0.1:{}/p/{id}", ctx.cfg.port),
        unresolved: open,
        exported,
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
            n => format!("· in review · {}", plural(n, "open thread")),
        };
        Line::new('▸', "up for review")
            .id(&self.id)
            .dim(dim)
            .url(self.url.clone())
            .write(w, st)?;
        if let Some(p) = &self.exported {
            Line::new(glyph::OK, format!("exported {p}"))
                .dim("static, comment-less — threads stay on the served page")
                .write(w, st)?;
        }
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
    refuse_terminal(ctx, p, false)?;
    if p.fm.status == S::Approved {
        return Err(KsError::conflict(
            format!("{id} is already approved{}", stamp_tail(p)),
            fixes![fix!("{} close {id}", ctx.invoked_as)],
        ));
    }

    // THE gate. Unresolved feedback is the one thing approval must not paper over — and
    // the escape hatch is deliberately a recorded act, not a flag: a human who disagrees
    // resolves the thread with a note, and that note IS the waiver.
    let open = unresolved(&snap, p);
    if open > 0 {
        return Err(KsError::gate(
            GateCode::UnresolvedThreads,
            format!(
                "cannot approve {id} — {}",
                plural(open, "unresolved review thread")
            ),
            fixes![
                fix!("{} comments {id} --unresolved", ctx.invoked_as),
                fix!("{} comment resolve <cm-id> --note \"...\"", ctx.invoked_as),
            ],
        ));
    }

    // `[tN]` bullets become real board tickets. The text before the first `·` is the
    // title; the rest is the DESIGN.md `· S · deps: t-31aa` tail, which `new` already
    // knows how to take as flags. The spec is the bullet's own `(spec: x)`, else the spec
    // of the `[cN]` it names, else the proposal's first (t-85de).
    let wanted: Vec<(ItemRef, String, Vec<String>, Option<String>)> = p
        .items
        .iter()
        .filter(|i| i.id.kind == crate::ids::ItemKind::Ticket)
        .map(|i| {
            let bullet = split_ticket_bullet(&i.text);
            let spec = ticket_spec(p, &bullet);
            (i.id.clone(), bullet.title, bullet.deps, spec)
        })
        .collect();

    // A re-run must not mint a second copy of every ticket, so anything already linked to
    // this proposal counts as minted. The `[tN]` anchor rides in the ledger.
    let already: usize = snap
        .tickets
        .values()
        .filter(|t| t.fm.proposal.as_ref() == Some(&id))
        .count();

    let stamp = format!(
        "{} {}",
        ctx.now.format("%Y-%m-%dT%H:%MZ"),
        ctx.actor.label()
    );
    let pid = id.clone();
    let done = Store::open(ctx).transact(None, &ctx.invocation(), |sn, m| {
        let p = live(sn, &pid)?;
        let mut plan = Plan::of(vec![Op::SetFields {
            entity: EntityRef::Proposal(pid.clone()),
            sets: vec![
                (Key::Proposal(ProposalKey::Status), Yv::s("approved")),
                (Key::Proposal(ProposalKey::Approved), Yv::s(stamp.clone())),
            ],
        }]);
        let mut ledger = p.fm.ledger.clone();
        if already == 0 {
            let f = ctx.facts();
            for (anchor, title, deps, spec) in &wanted {
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
                absorb(&mut plan, sub, &mut ledger, anchor, "minted");
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

/// A `[tN]` bullet, taken apart.
#[derive(Debug, Default, PartialEq, Eq)]
struct TicketBullet {
    title: String,
    deps: Vec<String>,
    /// `(spec: playbooks)` at the head of the bullet
    spec: Option<String>,
    /// the `[cN]` tags the bullet says it implements: `· implements: c1, c2` or `· c1`
    changes: Vec<String>,
}

/// `(spec: playbooks) Rate-limit login endpoint · S · deps: t-31aa · implements: c1`
/// -> title, dep ids, the named spec, the named changes.
///
/// The estimate segment is dropped on purpose: `new` has no estimate flag, and inventing a
/// place to put it would be a field the board never reads.
fn split_ticket_bullet(text: &str) -> TicketBullet {
    let mut b = TicketBullet::default();
    let mut text = text.trim();
    if let Some(rest) = text.strip_prefix("(spec:") {
        if let Some((name, tail)) = rest.split_once(')') {
            b.spec = Some(name.trim().to_string());
            text = tail.trim();
        }
    }
    let mut title = text;
    if let Some((head, rest)) = text.split_once('·') {
        title = head.trim();
        for seg in rest.split('·') {
            let seg = seg.trim();
            let list = |l: &str| -> Vec<String> {
                l.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect()
            };
            if let Some(l) = seg.strip_prefix("deps:") {
                b.deps.extend(list(l));
            } else if let Some(l) = seg.strip_prefix("implements:") {
                b.changes.extend(list(l));
            } else {
                // A bare `c1, c2` segment: every entry is a change tag, or it is not a
                // change list at all (the estimate `S` is one such segment).
                let items = list(seg);
                if !items.is_empty() && items.iter().all(|i| is_change_tag(i)) {
                    b.changes.extend(items);
                }
            }
        }
    }
    b.title = title.to_string();
    b
}

/// `c1`, `c12` — a change anchor's tag, without its proposal.
fn is_change_tag(s: &str) -> bool {
    s.strip_prefix('c')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

/// Which spec a minted ticket belongs to (t-85de). On the trial all three tickets minted
/// from one proposal got its first spec, and the content ticket belonged to another.
///
/// 1. the bullet's own `(spec: x)`;
/// 2. the spec of the first `[cN]` the bullet names — a change bullet opens with its spec,
///    `- [c1] auth: 5 failed logins …` (DESIGN.md), when the proposal spans several;
/// 3. the proposal's first spec.
fn ticket_spec(p: &Proposal, b: &TicketBullet) -> Option<String> {
    if let Some(s) = &b.spec {
        return Some(s.clone());
    }
    let named: Vec<&SpecName> = p.fm.specs.iter().collect();
    for tag in &b.changes {
        let Some(change) = p
            .items
            .iter()
            .find(|i| format!("{}{}", i.id.kind.letter(), i.id.n) == *tag)
        else {
            continue;
        };
        let Some((head, _)) = change.text.split_once(':') else {
            continue;
        };
        if let Some(s) = named.iter().find(|s| s.as_str() == head.trim()) {
            return Some(s.to_string());
        }
    }
    p.fm.specs.first().map(ToString::to_string)
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
    refuse_terminal(ctx, p, true)?;
    if p.fm.status != S::Approved {
        return Err(KsError::gate(
            GateCode::NotApproved,
            format!(
                "{id} is {} — close is the END of an approved proposal, not a way out \
                 of one",
                status_word(p.fm.status)
            ),
            fixes![
                fix!("{} approve {id}", ctx.invoked_as),
                fix!("{} abandon {id} --why \"...\"", ctx.invoked_as),
            ],
        ));
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
        if crate::derive::dispositioned(&ledger, &i.id) || followups.iter().any(|(f, _)| *f == i.id)
        {
            continue;
        }
        if i.id.kind == crate::ids::ItemKind::Change {
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
                        format!("{} close {id} --followup {}", ctx.invoked_as, short(&i.id)),
                        format!(
                            "{} close {id} --dropped {} --note \"...\"",
                            ctx.invoked_as,
                            short(&i.id)
                        ),
                    ],
                ));
            }
            continue;
        }
        // A prescription. A `(temp until t-x)` whose guard landed expires on its own.
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
            // A `(promote: spec)` prescription ships the way a change does: as a rule
            // bullet written on the implementing branch, carrying `{p-x#pN}`. The exact
            // token is required — a bare `{p-x}` names the proposal, not the item, and a
            // prescription is never handed the positional fallback a change gets.
            Some(crate::model::Prescription::Promote(crate::model::PromoteAs::Spec)) => {
                if let Some(rule) = evidence.exact.get(&i.id.to_string()).cloned() {
                    ledger.push(format!("{} shipped→{rule}", i.id));
                    dispositions.push(Disposition::Shipped {
                        item: i.id.clone(),
                        rule,
                    });
                    continue;
                }
                // `promote --as spec` refuses by design (a rule is written, not minted),
                // so the advice must name the write, not the verb that bounces.
                vec![format!(
                    "add `- [<spec>.<rule>] … {{{}}}` to .kanspec/specs/<spec>.md, then {} close {id}",
                    i.id, ctx.invoked_as
                )]
            }
            Some(crate::model::Prescription::Promote(kind)) => vec![format!(
                "{} promote {} --as {} --scope \"src/**\"",
                ctx.invoked_as,
                i.id,
                promote_word(*kind)
            )],
            Some(crate::model::Prescription::TempUntil(t)) => vec![
                format!("{} scan --confirm", ctx.invoked_as),
                format!(
                    "{} expire {} --reason \"{t} is not landing\"",
                    ctx.invoked_as, i.id
                ),
            ],
            // An untyped prescription is a close blocker BY DESIGN: nobody can say what
            // it was supposed to become, which is exactly the silent drop this gate
            // exists to stop.
            _ => vec![
                "type it `(promote: decision)` or `(temp until t-x)` in the body".to_string(),
                format!("{} expire {} --reason \"...\"", ctx.invoked_as, i.id),
            ],
        };
        unmet.push((i.id.clone(), format!("\"{}\"", i.text), fixes));
    }

    // 3 — a followup still needs its ticket, which is minted in the same transaction.
    if !unmet.is_empty() {
        let mut msg = format!(
            "cannot close {id} — {} undispositioned:",
            plural(unmet.len(), "item")
        );
        for (item, what, _) in &unmet {
            msg.push_str(&format!("\n  [{}] {what}", short(item)));
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
            GateCode::UndispositionedItems,
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
        let mut ledger = ledger.clone();
        let mut plan = Plan::empty();
        let f = ctx.facts();
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
            absorb(&mut plan, sub, &mut ledger, item, "followup");
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
        Ok(plan)
    })?;

    // One ticket per followup, in minting order — the same pairing the ledger inside the
    // transaction recorded, so the transcript and the file cannot name different tickets.
    let mut tickets = done.minted.iter().filter_map(|e| match e {
        EntityRef::Ticket(t) => Some(t.clone()),
        _ => None,
    });
    for (item, _) in &followups {
        if let Some(ticket) = tickets.next() {
            dispositions.push(Disposition::Followup {
                item: item.clone(),
                ticket,
            });
        }
    }

    Ok(CloseReport {
        status: S::Closed,
        ledger: dispositions,
        moved_to: ctx.rel(&dest),
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
                Disposition::Followup { item, ticket } => (item, format!("followup→{ticket}")),
                Disposition::Dropped { item, note } => (item, format!("dropped ({note})")),
                Disposition::Expired { item, reason } => (item, format!("expired ({reason})")),
            };
            writeln!(w, "   · {} {what}", short(item))?;
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
    refuse_terminal(ctx, p, true)?;

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

// ─────────────────────────────────────────────────────────────────────────────
// The review page model — everything `/p/<id>` needs, assembled once
// ─────────────────────────────────────────────────────────────────────────────

/// The proposal typeset for review, with its threads already attached to the items they
/// target and the CURRENT spec text inlined.
///
/// Assembled server-side, in one snapshot read, for the reason invariant 7 exists: the page
/// is a pure view of the repo. If the browser had to stitch a proposal, its comments and
/// the spec corpus together from three endpoints, it would be able to render a combination
/// that never existed on disk.
#[derive(Debug, Serialize)]
pub struct ProposalPage {
    pub id: ProposalId,
    pub title: String,
    pub status: crate::model::ProposalStatus,
    pub specs: Vec<SpecName>,
    pub approved: Option<String>,
    pub created: String,
    /// the `## Why` prose, verbatim
    pub why: String,
    /// `why` split at its first sentence — the page shows the headline and folds the rest
    pub why_headline: String,
    pub why_detail: String,
    pub items: Vec<PageItem>,
    /// Every `## ` section that is not one of the four the machinery reads (Why, Changes,
    /// Prescriptions, Tickets), in file order — `Testing and verification` from the
    /// scaffold, a repo's own `Security impact`, the `Abandoned` note. Passed through so
    /// a section an author wrote is never invisible on the page.
    pub sections: Vec<PageSection>,
    /// threads whose target item is gone — shown, never dropped
    pub orphaned: Vec<crate::cmd::comment::Thread>,
    pub unresolved: usize,
    /// what `approve`/`close` would refuse with right now, or none
    pub blocked_by: Option<String>,
    /// the current rules of every spec this proposal names — the "review the delta against
    /// today's truth without opening files" half
    pub context: Vec<SpecContext>,
}

/// A free-form section, split for skimming: prose first, then each `- ` bullet as a
/// headline + folded detail, exactly like an item.
#[derive(Debug, Serialize)]
pub struct PageSection {
    pub heading: String,
    pub prose: String,
    pub bullets: Vec<Headline>,
}

/// A bullet or item as the page skims it: the first sentence stands for the whole, the
/// rest is one tap away. Reviewers asked for this in so many words — "one or two
/// sentences per bullet, and if I want to click in further I could" — and it costs the
/// author nothing beyond writing the first sentence as the summary.
#[derive(Debug, Serialize)]
pub struct Headline {
    pub headline: String,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct PageItem {
    pub id: ItemRef,
    /// `c` | `p` | `t`
    pub kind: char,
    pub text: String,
    /// `text` split at its first sentence
    pub headline: String,
    pub detail: String,
    /// `TEMP` / `PROMOTE → decision` — the badge the page draws
    pub badge: Option<String>,
    pub threads: Vec<crate::cmd::comment::Thread>,
    /// a `[tN]` item's minted ticket, once `approve` has run
    pub ticket: Option<TicketId>,
    pub dispositioned: bool,
}

#[derive(Debug, Serialize)]
pub struct SpecContext {
    pub spec: SpecName,
    pub feature: String,
    pub rules: Vec<crate::model::Rule>,
}

/// Assemble [`ProposalPage`]. Read-only: it can refuse, and it can never write.
pub fn page(ctx: &Ctx, raw: &str) -> Result<ProposalPage> {
    ctx.require_initialized()?;
    let s = ctx.snapshot()?;
    page_of(ctx, &s, raw)
}

/// [`page`] over a snapshot the caller already holds (§2.16: one store load per command).
pub fn page_of(ctx: &Ctx, s: &Snapshot, raw: &str) -> Result<ProposalPage> {
    let p = open_proposal(ctx, s, raw)?;

    let ops = s.comments.get(&p.fm.id).map(Vec::as_slice).unwrap_or(&[]);
    let (live, orphaned) = crate::cmd::comment::fold_threads(p, ops);

    // The ledger records `p-7de2#t1 minted→t-9c41`, so a `[tN]` card can link its ticket
    // without a second convention.
    let ticket_of = |item: &ItemRef| -> Option<TicketId> {
        let needle = format!("{item} minted→");
        p.fm.ledger
            .iter()
            .find_map(|l| l.split_once(&needle).map(|(_, t)| t.trim().to_string()))
            .and_then(|t| TicketId::parse(&t).ok())
    };

    let items = p
        .items
        .iter()
        .map(|i| {
            // The `(promote: decision)` marker becomes the badge, so leaving it in the
            // text too would render it twice on the card.
            let text = strip_marker(&i.text);
            let (headline, detail) = split_headline(&text);
            PageItem {
                kind: i.id.kind.letter(),
                text,
                headline,
                detail,
                badge: match &i.prescription {
                    Some(crate::model::Prescription::TempUntil(t)) => {
                        Some(format!("TEMP until {t}"))
                    }
                    Some(crate::model::Prescription::Promote(k)) => {
                        Some(format!("PROMOTE → {}", promote_word(*k)))
                    }
                    // An untyped prescription is a close blocker, so the page says so where
                    // the author can still fix it rather than at close time.
                    Some(crate::model::Prescription::Untyped) => Some("UNTYPED".to_string()),
                    None => None,
                },
                threads: live
                    .iter()
                    .filter(|t| t.target == i.id.to_string())
                    .cloned()
                    .collect(),
                ticket: ticket_of(&i.id),
                dispositioned: crate::derive::dispositioned(&p.fm.ledger, &i.id),
                id: i.id.clone(),
            }
        })
        .collect();

    let context =
        p.fm.specs
            .iter()
            .filter_map(|n| s.specs.get(n))
            .map(|sp| SpecContext {
                spec: sp.name.clone(),
                feature: sp.fm.feature.clone(),
                rules: sp.rules.clone(),
            })
            .collect();

    let open = unresolved(s, p);
    let why = section(&p.body, "## Why");
    let (why_headline, why_detail) = split_headline(&why);
    Ok(ProposalPage {
        why_headline,
        why_detail,
        sections: extra_sections(&p.body),
        title: p.fm.title.clone(),
        status: p.fm.status,
        specs: p.fm.specs.clone(),
        approved: p.fm.approved.clone(),
        created: p.fm.created.to_string(),
        why,
        items,
        orphaned,
        unresolved: open,
        blocked_by: (open > 0).then(|| plural(open, "unresolved review thread")),
        context,
        id: p.fm.id.clone(),
    })
}

/// `(promote: decision) rate-limit state in Redis` -> `rate-limit state in Redis`.
///
/// Shared with `promote`: the marker is the prescription's TYPE, and a minted decision or
/// quirk records its type in its own frontmatter, so a title that kept the marker would say
/// it twice — and say it forever, on every `rules` line and every page (t-5f2b).
pub(crate) fn strip_marker(text: &str) -> String {
    let t = text.trim();
    if !t.starts_with('(') {
        return t.to_string();
    }
    match t.find(')') {
        Some(i) => t[i + 1..].trim().to_string(),
        None => t.to_string(),
    }
}

/// The four headings the machinery reads; everything else is an author's section.
const KNOWN_SECTIONS: [&str; 4] = ["## Why", "## Changes", "## Prescriptions", "## Tickets"];

/// Every author-written `## ` section, in file order, each split for skimming.
fn extra_sections(body: &str) -> Vec<PageSection> {
    let mut out = Vec::new();
    let mut cur: Option<(String, Vec<String>)> = None;
    for line in body.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            if let Some((heading, lines)) = cur.take() {
                out.push(page_section(heading, &lines));
            }
            cur = (!KNOWN_SECTIONS.contains(&line.trim_end()))
                .then(|| (h.trim().to_string(), Vec::new()));
            continue;
        }
        if let Some((_, lines)) = cur.as_mut() {
            lines.push(line.to_string());
        }
    }
    if let Some((heading, lines)) = cur {
        out.push(page_section(heading, &lines));
    }
    out
}

fn page_section(heading: String, lines: &[String]) -> PageSection {
    let mut prose = String::new();
    let mut bullets = Vec::new();
    for l in lines {
        if let Some(b) = l.trim_start().strip_prefix("- ") {
            let (headline, detail) = split_headline(b.trim());
            bullets.push(Headline { headline, detail });
        } else {
            prose.push_str(l);
            prose.push('\n');
        }
    }
    PageSection {
        heading,
        prose: prose.trim().to_string(),
        bullets,
    }
}

/// `text` split at the end of its first sentence: the headline the page shows, and the
/// detail it folds. A sentence ends at `.`, `!` or `?` followed by whitespace, ignoring
/// anything inside backticks (`POST .../finish` is not three sentences) and a dotted
/// abbreviation (`e.g.`, `vs.`) whose word is short. No sentence end means the whole text
/// is the headline and the detail is empty.
pub fn split_headline(text: &str) -> (String, String) {
    let t = text.trim();
    let bytes = t.as_bytes();
    let mut in_code = false;
    let mut word_start = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'`' => in_code = !in_code,
            b' ' | b'\n' if !in_code => word_start = i + 1,
            b'.' | b'!' | b'?' if !in_code => {
                let ends_here = bytes.get(i + 1).is_none_or(|n| n.is_ascii_whitespace());
                if !ends_here {
                    continue;
                }
                // `e.g.` / `vs.` / `i.e.` — a short dotted token is not a sentence end.
                let word = &t[word_start..i];
                if b == b'.'
                    && word.len() <= 3
                    && !word.is_empty()
                    && word.chars().all(|c| c.is_ascii_alphabetic() || c == '.')
                    && word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                {
                    continue;
                }
                let head = t[..=i].trim().to_string();
                let rest = t[i + 1..].trim().to_string();
                return (head, rest);
            }
            _ => {}
        }
    }
    (t.to_string(), String::new())
}

/// The prose under one `## ` heading, up to the next one.
fn section(body: &str, heading: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in body.lines() {
        if line.trim_end() == heading {
            inside = true;
            continue;
        }
        if inside && line.starts_with("## ") {
            break;
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod page_tests {
    use super::*;

    #[test]
    fn the_headline_is_the_first_sentence_and_backticks_do_not_end_one() {
        let (h, d) = split_headline(
            "Finishing onboarding activates that playbook. `POST .../onboarding/finish` goes through the existing path.",
        );
        assert_eq!(h, "Finishing onboarding activates that playbook.");
        assert_eq!(
            d,
            "`POST .../onboarding/finish` goes through the existing path."
        );

        let (h, d) = split_headline(
            "Add a `new-home-catchup` playbook with eight one-time items. Dryer vent first.",
        );
        assert_eq!(
            h,
            "Add a `new-home-catchup` playbook with eight one-time items."
        );
        assert_eq!(d, "Dryer vent first.");

        let (h, d) = split_headline("Out of scope: the affiliate toolkit and an MCP surface.");
        assert_eq!(h, "Out of scope: the affiliate toolkit and an MCP surface.");
        assert_eq!(d, "", "one sentence is all headline");

        let (h, _) =
            split_headline("Use a date, e.g. 2026-09-01, never a boolean. Recency is computed.");
        assert_eq!(
            h, "Use a date, e.g. 2026-09-01, never a boolean.",
            "`e.g.` is not a sentence end"
        );

        let (h, d) = split_headline("A future date is a 400 (validation). Nothing derives it");
        assert_eq!(h, "A future date is a 400 (validation).");
        assert_eq!(d, "Nothing derives it");
    }

    #[test]
    fn author_sections_pass_through_in_order_and_the_four_known_ones_do_not() {
        let body = "## Why\nbecause.\n\n## Changes\n- [c1] x\n\n## Security impact\nnone: no new surface.\n\n## Testing and verification\n- Backend tests: a future date is 400. Owner-only activation.\n- Hand check on the dev stack.\n\n## Prescriptions\n\n## Tickets\n- [t1] y\n\n## Abandoned\nsuperseded by p-1\n";
        let s = extra_sections(body);
        let heads: Vec<&str> = s.iter().map(|x| x.heading.as_str()).collect();
        assert_eq!(
            heads,
            ["Security impact", "Testing and verification", "Abandoned"]
        );
        assert_eq!(s[0].prose, "none: no new surface.");
        assert!(s[0].bullets.is_empty());
        assert_eq!(s[1].bullets.len(), 2);
        assert_eq!(
            s[1].bullets[0].headline,
            "Backend tests: a future date is 400."
        );
        assert_eq!(s[1].bullets[0].detail, "Owner-only activation.");
        assert_eq!(s[1].bullets[1].detail, "");
        assert_eq!(s[2].prose, "superseded by p-1");
    }

    #[test]
    fn the_scaffold_carries_testing_and_verification_between_changes_and_prescriptions() {
        let s = scaffold(
            &ProposalId::parse("p-1c51").unwrap(),
            "t",
            &[],
            chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap(),
        );
        let c = s.find("## Changes").unwrap();
        let v = s.find("## Testing and verification").unwrap();
        let p = s.find("## Prescriptions").unwrap();
        assert!(c < v && v < p, "{s}");
    }
}
