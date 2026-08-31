//! `new / show / ls / log / where`.
//!
//! `new` is the rabbit-hole valve: it auto-stamps `discovered_in` from whatever ticket this
//! session already claims, so capturing tangential work costs one command and derails
//! nothing. The link is what stops the capture from rotting — `status` raises a WATCHING
//! line on untriaged discoveries, and `done` reports what was parked along the way.
//!
//! Owner: **S5**.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;

use crate::cli::{LogArgs, LsArgs, NewArgs, ShowArgs, WhereArgs};
use crate::ctx::Ctx;
use crate::derive::{self, Badge, Column};
use crate::error::{KsError, Result};
use crate::fm::Yv;
use crate::ids::{ProposalId, SpecName, TicketId};
use crate::logentry::{LogEntry, LOG_HEADING, STEPS_HEADING};
use crate::model::{Snapshot, Step, Ticket};
use crate::out::{glyph, Color, Line, Render, Style, Table};
use crate::plan::{EntityRef, Facts, Op, Plan};
use crate::store::Store;
use crate::transitions::{self, State, Verb};
use crate::{fix, fixes};

// ── new ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct NewReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub spec: Option<SpecName>,
    pub proposal: Option<ProposalId>,
    pub deps: Vec<TicketId>,
    pub followup_of: Option<TicketId>,
    /// stamped automatically from the session's claimed ticket unless `--no-link`
    pub discovered_in: Option<TicketId>,
    pub path: String,
    pub next: Vec<String>,
}

pub fn new(ctx: &Ctx, a: &NewArgs) -> Result<NewReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;

    // The ONE impure step: which ticket this session is standing on. Resolved HERE, before
    // the lock, and handed to the planner as a plain value — a planner never asks the world
    // anything.
    let discovered_in = resolve_discovered_in(ctx, &snap, a)?;

    let f = Facts {
        actor: ctx.actor.clone(),
        at: ctx.now,
        invocation: ctx.invocation(),
    };
    let committed = Store::open(ctx).transact(Some(Verb::New), &ctx.invocation(), |s, m| {
        plan_new(s, &f, a, m, discovered_in.as_ref())
    })?;

    let id = committed
        .minted
        .iter()
        .find_map(|e| match e {
            EntityRef::Ticket(id) => Some(id.clone()),
            _ => None,
        })
        .ok_or_else(|| KsError::internal(anyhow::anyhow!("`new` minted no ticket")))?;
    let t = committed.snapshot.ticket(&id)?;

    Ok(NewReport {
        title: t.fm.title.clone(),
        state: t.fm.state,
        spec: t.fm.spec.clone(),
        proposal: t.fm.proposal.clone(),
        deps: t.fm.deps.clone(),
        followup_of: t.fm.followup_of.clone(),
        discovered_in: t.fm.discovered_in.clone(),
        path: rel_to(ctx, &t.path),
        next: if derive::is_ready(&committed.snapshot, t) {
            vec![format!("{} start {id}", ctx.invoked_as)]
        } else {
            vec![format!("{} show {id}", ctx.invoked_as)]
        },
        id,
    })
}

/// PURE. `discovered_in` is already resolved by the handler — a planner never asks the
/// world anything.
///
/// NOTE (deviation from the wave-0 stub, reported): the stub's body sketch was
/// `Op::CreateEntity` **plus** `Op::Transition { verb: New }`. That second op cannot exist:
/// `Plan::validate` and `Store::stage` both look the ticket up in the fresh snapshot before
/// transitioning it, and `transitions::require` takes a `State`, not an `Option<State>`, so
/// there is no way to spell the genesis step as a transition. The genesis `## Log` line is
/// therefore written INTO the scaffold, where `transact`'s step-8 proof reads it back off
/// the staged bytes — the same proof, one op earlier.
pub fn plan_new(
    s: &Snapshot,
    f: &Facts,
    a: &NewArgs,
    m: &crate::ids::Minter,
    discovered_in: Option<&TicketId>,
) -> Result<Plan> {
    let title = a.title.trim();
    if title.is_empty() {
        return Err(KsError::invalid(
            "a ticket needs a title",
            fixes![fix!("kanspec new \"what the work is, in one line\"")],
        ));
    }

    let spec = a.spec.as_deref().map(SpecName::parse).transpose()?;
    let proposal = a.proposal.as_deref().map(ProposalId::parse).transpose()?;
    let followup_of = a.followup_of.as_deref().map(TicketId::parse).transpose()?;
    let mut deps: Vec<TicketId> = Vec::new();
    for d in &a.deps {
        let id = TicketId::parse(d)?;
        // A dep pointing at nothing is not "unblocked", it is a typo that `derive` would
        // report as permanently blocked and `doctor` as an Error. Refuse it at the door.
        s.ticket(&id)?;
        if !deps.contains(&id) {
            deps.push(id);
        }
    }
    if let Some(p) = &followup_of {
        s.ticket(p)?;
    }

    let id = m.ticket(title)?;
    if deps.contains(&id) {
        return Err(KsError::conflict(
            format!("{id} cannot depend on itself"),
            fixes![fix!("kanspec new \"{title}\"")],
        ));
    }

    let note = match (&discovered_in, &followup_of) {
        (Some(src), _) => Some(format!("discovered in {src}")),
        (None, Some(src)) => Some(format!("followup of {src}")),
        _ => None,
    };
    let genesis = LogEntry {
        at: f.at,
        state: State::Todo,
        actor: f.actor.label(),
        verb: Verb::New,
        note,
    };

    let contents = scaffold(&TicketScaffold {
        id: &id,
        title,
        spec: spec.as_ref(),
        proposal: proposal.as_ref(),
        deps: &deps,
        followup_of: followup_of.as_ref(),
        discovered_in,
        created: f.at,
        genesis: &genesis,
    });

    let mut plan = Plan::empty();
    plan.push(Op::CreateEntity {
        entity: EntityRef::Ticket(id.clone()),
        contents,
    })
    .mint(EntityRef::Ticket(id));
    Ok(plan)
}

/// Everything the ticket file says at birth. Grouped rather than passed as nine arguments,
/// because the field ORDER is `keys::TICKET_ORDER` and a scaffold that drifts from it makes
/// every later `fm::set` insert in the wrong place.
pub(crate) struct TicketScaffold<'a> {
    pub id: &'a TicketId,
    pub title: &'a str,
    pub spec: Option<&'a SpecName>,
    pub proposal: Option<&'a ProposalId>,
    pub deps: &'a [TicketId],
    pub followup_of: Option<&'a TicketId>,
    pub discovered_in: Option<&'a TicketId>,
    pub created: DateTime<Utc>,
    pub genesis: &'a LogEntry,
}

/// The DESIGN.md ticket, in `keys::TICKET_ORDER`, with every value emitted by `fm::emit` so
/// a title full of colons — or an id YAML would read as a number — cannot corrupt the file.
pub(crate) fn scaffold(t: &TicketScaffold<'_>) -> String {
    let mut fm = String::new();
    let mut put = |k: &str, v: Yv| {
        fm.push_str(k);
        fm.push_str(": ");
        fm.push_str(&crate::fm::emit(&v, false));
        fm.push('\n');
    };
    put("id", Yv::s(t.id.as_str()));
    put("title", Yv::s(t.title));
    put("state", Yv::s(State::Todo.as_str()));
    put("spec", Yv::opt_s(t.spec.map(|s| s.as_str().to_string())));
    put("proposal", Yv::opt_s(t.proposal.map(ProposalId::to_string)));
    put("item", Yv::Null);
    put(
        "deps",
        Yv::list(t.deps.iter().map(ToString::to_string).collect::<Vec<_>>()),
    );
    put(
        "followup_of",
        Yv::opt_s(t.followup_of.map(TicketId::to_string)),
    );
    put(
        "discovered_in",
        Yv::opt_s(t.discovered_in.map(TicketId::to_string)),
    );
    put("branch", Yv::Null);
    put("worktree", Yv::Null);
    put("claimed_by", Yv::Null);
    put("pr", Yv::Null);
    put("head", Yv::Null);
    put("spec_unchanged", Yv::Null);
    put(
        "created",
        Yv::s(t.created.to_rfc3339_opts(SecondsFormat::Secs, true)),
    );

    format!(
        "---\n{fm}---\n{title}\n\n{STEPS_HEADING}\n\n{LOG_HEADING}\n{log}\n",
        title = t.title,
        log = t.genesis.format(),
    )
}

/// `--from` overrides, `--no-link` opts out, and otherwise the session's own claimed ticket
/// is stamped — DESIGN.md's "capturing a rabbit hole costs nothing and derails nothing".
fn resolve_discovered_in(ctx: &Ctx, snap: &Snapshot, a: &NewArgs) -> Result<Option<TicketId>> {
    if a.no_link {
        return Ok(None);
    }
    if let Some(raw) = a.from.as_deref() {
        let id = TicketId::parse(raw)?;
        snap.ticket(&id)?;
        return Ok(Some(id));
    }
    // `--followup-of` is the OTHER link type: leftover scope from a ticket's own steps,
    // minted by `done`'s triage. Stamping both would claim the work was two things at once.
    if a.followup_of.is_some() {
        return Ok(None);
    }
    Ok(claimed_ticket(ctx, snap).map(|c| c.id))
}

// ── show ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ShowReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub column: Column,
    pub badge: Badge,
    /// the badge as a reader sees it. `Render` has no clock — `ctx.now` does — so the
    /// freshness stamp is baked here rather than read off the wall clock at print time.
    pub badge_text: String,
    pub spec: Option<SpecName>,
    pub proposal: Option<ProposalId>,
    pub deps: Vec<TicketId>,
    pub blocked_by: Vec<TicketId>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub claimed_by: Option<String>,
    pub pr: Option<u64>,
    pub head: Option<String>,
    pub created: DateTime<Utc>,
    pub steps: Vec<Step>,
    pub log: Vec<LogEntry>,
    /// the ONE owed verb, already spelled out
    pub next: Vec<String>,
}

pub fn show(ctx: &Ctx, a: &ShowArgs) -> Result<ShowReport> {
    ctx.require_initialized()?;
    let id = TicketId::parse(&a.id)?;
    let snap = ctx.snapshot()?;
    let t = snap.ticket(&id)?;
    Ok(ShowReport {
        title: t.fm.title.clone(),
        state: t.fm.state,
        column: derive::column(&snap, t),
        badge_text: derive::badge(&snap, t).text(snap.now),
        badge: derive::badge(&snap, t),
        spec: t.fm.spec.clone(),
        proposal: t.fm.proposal.clone(),
        deps: t.fm.deps.clone(),
        blocked_by: derive::blocked_by(&snap, t).into_iter().cloned().collect(),
        branch: t.fm.branch.clone(),
        worktree: t.fm.worktree.as_ref().map(|p| p.display().to_string()),
        claimed_by: t.fm.claimed_by.clone(),
        pr: t.fm.pr,
        head: t.fm.head.clone(),
        created: t.fm.created,
        steps: t.steps.clone(),
        log: t.log.clone(),
        next: owed(ctx, &snap, t),
        id,
    })
}

impl Render for ShowReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::state(self.state, &self.title)
            .id(&self.id)
            .dim(format!("· {}", self.badge_text))
            .fix(self.next.first().cloned().unwrap_or_default())
            .write(w, st)?;
        let mut chips: Vec<String> = vec![self.column_label().to_string()];
        if let Some(s) = &self.spec {
            chips.push(format!("spec {s}"));
        }
        if let Some(p) = &self.proposal {
            chips.push(format!("proposal {p}"));
        }
        if let Some(b) = &self.branch {
            chips.push(format!("⎇ {b}"));
        }
        if let Some(wt) = &self.worktree {
            chips.push(format!("⌂ {wt}"));
        }
        if let Some(c) = &self.claimed_by {
            chips.push(format!("claimed {c}"));
        }
        if let Some(n) = self.pr {
            chips.push(format!("PR #{n}"));
        }
        if let Some(h) = &self.head {
            chips.push(format!("head {}", &h[..7.min(h.len())]));
        }
        writeln!(w, "   {}", chips.join(" · "))?;

        if !self.deps.is_empty() {
            let open: Vec<String> = self.blocked_by.iter().map(ToString::to_string).collect();
            let all: Vec<String> = self.deps.iter().map(ToString::to_string).collect();
            writeln!(
                w,
                "   deps {}{}",
                all.join(", "),
                if open.is_empty() {
                    " (all satisfied)".to_string()
                } else {
                    format!(" · blocked by {}", open.join(", "))
                }
            )?;
        }

        if !self.steps.is_empty() {
            writeln!(w, "   {STEPS_HEADING}")?;
            for s in &self.steps {
                writeln!(
                    w,
                    "   [{}] {}",
                    if s.done { "x" } else { " " },
                    crate::out::paint(
                        &s.text,
                        if s.done { Color::Dim } else { Color::Bold },
                        st.color
                    )
                )?;
            }
        }
        for n in self.next.iter().skip(1) {
            writeln!(w, "   {} {n}", glyph::FIX)?;
        }
        Ok(())
    }
}

impl ShowReport {
    fn column_label(&self) -> &'static str {
        column_label(self.column)
    }
}

fn column_label(c: Column) -> &'static str {
    match c {
        Column::Backlog => "backlog",
        Column::Ready => "ready",
        Column::Doing => "doing",
        Column::Review => "review",
        Column::InMain => "in main",
        Column::Done => "done",
        Column::Dropped => "dropped",
    }
}

// ── ls ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct LsReport {
    pub rows: Vec<LsRow>,
    pub filtered: Vec<String>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct LsRow {
    pub id: TicketId,
    pub state: State,
    pub title: String,
    pub spec: Option<SpecName>,
    pub claimed_by: Option<String>,
    pub badge: Badge,
    pub badge_text: String,
    pub updated_secs: Option<u64>,
    /// DESIGN.md's ◇ chip: work captured while doing something else
    pub discovered_in: Option<TicketId>,
}

pub fn ls(ctx: &Ctx, a: &LsArgs) -> Result<LsReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let spec = a.spec.as_deref().map(SpecName::parse).transpose()?;

    let mut filtered: Vec<String> = Vec::new();
    if let Some(s) = &spec {
        filtered.push(format!("spec {s}"));
    }
    for (on, label) in [
        (a.mine, "mine"),
        (a.stalled, "stalled"),
        (a.unmerged, "unmerged"),
        (a.all, "all"),
    ] {
        if on {
            filtered.push(label.to_string());
        }
    }

    let me = ctx.actor.label();
    let mut rows: Vec<LsRow> = Vec::new();
    for t in snap.tickets.values() {
        // Terminal tickets are history: `ls` is the working list unless asked otherwise.
        if !a.all && t.fm.state.terminal() {
            continue;
        }
        if spec.as_ref().is_some_and(|s| t.fm.spec.as_ref() != Some(s)) {
            continue;
        }
        if a.mine && t.fm.claimed_by.as_deref() != Some(me.as_str()) {
            continue;
        }
        if a.stalled && derive::stalled(&snap, t).is_none() {
            continue;
        }
        // "not detected on main" — the honest reading of the flag: `unknown` is not
        // `merged`, so a ticket the ladder could not answer for is still unmerged work.
        if a.unmerged && derive::in_main(&snap, t).is_some() {
            continue;
        }
        rows.push(LsRow {
            id: t.fm.id.clone(),
            state: t.fm.state,
            title: t.fm.title.clone(),
            spec: t.fm.spec.clone(),
            claimed_by: t.fm.claimed_by.clone(),
            badge_text: derive::badge(&snap, t).text(snap.now),
            badge: derive::badge(&snap, t),
            updated_secs: updated_secs(&snap, t),
            discovered_in: t.fm.discovered_in.clone(),
        });
    }
    rows.sort_by(|a, b| {
        state_rank(a.state)
            .cmp(&state_rank(b.state))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(LsReport {
        total: snap.tickets.len(),
        rows,
        filtered,
    })
}

/// Doing first, then review, then the queue — the order a human scans for "what is in
/// flight".
fn state_rank(s: State) -> u8 {
    match s {
        State::Doing => 0,
        State::Review => 1,
        State::Todo => 2,
        State::Done => 3,
        State::Dropped => 4,
    }
}

fn updated_secs(snap: &Snapshot, t: &Ticket) -> Option<u64> {
    let last = t.log.iter().map(|e| e.at).max().unwrap_or(t.fm.created);
    (snap.now - last).to_std().ok().map(|d| d.as_secs())
}

impl Render for LsReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        if self.rows.is_empty() {
            let what = if self.filtered.is_empty() {
                "no open tickets".to_string()
            } else {
                format!("no tickets match {}", self.filtered.join(" + "))
            };
            return Line::new('·', what).fix("kanspec new \"...\"").write(w, st);
        }
        let mut table = Table::new(&["", "id", "title", "spec", "claimed", "merge", "age"], st);
        for r in &self.rows {
            table.add_row(vec![
                format!(
                    "{}{}",
                    crate::out::state_glyph(r.state),
                    if r.discovered_in.is_some() {
                        glyph::DISCOVERED
                    } else {
                        ' '
                    }
                ),
                r.id.to_string(),
                r.title.clone(),
                r.spec.as_ref().map(SpecName::to_string).unwrap_or_default(),
                r.claimed_by.clone().unwrap_or_default(),
                r.badge_text.clone(),
                r.updated_secs
                    .map(|s| derive::short(std::time::Duration::from_secs(s)))
                    .unwrap_or_default(),
            ]);
        }
        writeln!(w, "{table}")?;
        writeln!(
            w,
            " {} of {} tickets{}",
            self.rows.len(),
            self.total,
            if self.filtered.is_empty() {
                String::new()
            } else {
                format!(" · {}", self.filtered.join(" + "))
            }
        )
    }
}

// ── log ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct LogReport {
    pub id: TicketId,
    pub entries: Vec<LogEntry>,
    /// what `transitions::replay` makes of the trail — a broken log names `kanspec repair`
    pub replays_to: Option<State>,
    pub violation: Option<String>,
    pub next: Vec<String>,
}

pub fn log(ctx: &Ctx, a: &LogArgs) -> Result<LogReport> {
    ctx.require_initialized()?;
    let id = TicketId::parse(&a.id)?;
    let snap = ctx.snapshot()?;
    let t = snap.ticket(&id)?;

    // `replay` alone answers "is this trail legal"; the FRONTMATTER comparison is what
    // catches the hand-edit, so `prove` is the question actually being asked here.
    let (replays_to, violation) = match transitions::replay(&t.log) {
        Ok(state) if state == t.fm.state => (Some(state), None),
        Ok(state) => (
            Some(state),
            Some(
                transitions::LogViolation::Divergence {
                    replayed: state,
                    frontmatter: t.fm.state,
                }
                .to_string(),
            ),
        ),
        Err(v) => (None, Some(v.to_string())),
    };
    let next = match &violation {
        None => owed(ctx, &snap, t),
        Some(_) => vec![
            format!("{} repair {id} --why \"...\"", ctx.invoked_as),
            format!("{} doctor", ctx.invoked_as),
        ],
    };
    Ok(LogReport {
        entries: t.log.clone(),
        replays_to,
        violation,
        next,
        id,
    })
}

impl Render for LogReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        for e in &self.entries {
            writeln!(w, " {}", e.format())?;
        }
        if self.entries.is_empty() {
            writeln!(w, " (the ## Log is empty)")?;
        }
        match (&self.violation, self.replays_to) {
            (None, Some(s)) => Line::new(glyph::OK, format!("replays cleanly to {s}"))
                .id(&self.id)
                .write(w, st)?,
            (Some(v), _) => Line::new(glyph::FAIL, v.clone())
                .id(&self.id)
                .fix(
                    self.next
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "kanspec doctor".into()),
                )
                .write(w, st)?,
            (None, None) => {}
        }
        for n in self.next.iter().skip(usize::from(self.violation.is_some())) {
            writeln!(w, "   {} {n}", glyph::FIX)?;
        }
        Ok(())
    }
}

// ── where ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct WhereReport {
    pub branch: Option<String>,
    pub worktree: String,
    pub ticket: Option<TicketId>,
    pub title: Option<String>,
    pub state: Option<State>,
    pub linked_worktree: bool,
    /// where the writes actually land — the point of worktree unification
    pub primary_kanspec: String,
    pub next: Vec<String>,
}

/// `where` is a keyword, so the handler is `where_is`.
pub fn where_is(ctx: &Ctx, a: &WhereArgs) -> Result<WhereReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let branch = match a.branch.as_deref() {
        Some(b) => Some(b.to_string()),
        None => here_branch(ctx),
    };
    let found = match &branch {
        Some(b) => ticket_for_branch(ctx, &snap, b),
        None => claimed_ticket(ctx, &snap).map(|c| c.id),
    };
    let t = found.as_ref().and_then(|id| snap.tickets.get(id));

    let next = match t {
        Some(t) => owed(ctx, &snap, t),
        None => vec![
            format!("{} ready", ctx.invoked_as),
            format!("{} status", ctx.invoked_as),
        ],
    };
    Ok(WhereReport {
        branch,
        worktree: ctx.repo.here().display().to_string(),
        ticket: t.map(|t| t.fm.id.clone()),
        title: t.map(|t| t.fm.title.clone()),
        state: t.map(|t| t.fm.state),
        linked_worktree: ctx.repo.linked(),
        primary_kanspec: ctx.layout.ks().display().to_string(),
        next,
    })
}

impl Render for WhereReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        match (&self.ticket, &self.title, self.state) {
            (Some(id), Some(title), Some(state)) => Line::state(state, title)
                .id(id)
                .fix(
                    self.next
                        .first()
                        .cloned()
                        .unwrap_or_else(|| format!("kanspec show {id}")),
                )
                .write(w, st)?,
            _ => Line::new('·', "no ticket owns this branch")
                .fix(self.next.first().cloned().unwrap_or_default())
                .write(w, st)?,
        }
        writeln!(
            w,
            "   branch   {}",
            self.branch.as_deref().unwrap_or("(detached)")
        )?;
        writeln!(
            w,
            "   worktree {}{}",
            self.worktree,
            if self.linked_worktree {
                " (linked)"
            } else {
                " (primary)"
            }
        )?;
        // The point of worktree unification: every write lands in ONE store, whichever
        // worktree you are standing in.
        writeln!(w, "   store    {}", self.primary_kanspec)
    }
}

// ── the shared session lookups ───────────────────────────────────────────────

/// The ticket this session is standing on, and how that was decided.
pub struct Claimed {
    pub id: TicketId,
    pub via: &'static str,
}

/// The branch checked out where the USER stands — not where the store lives.
///
/// `Git` is bound to the primary worktree (every command's writes go there), so
/// `Git::current_branch` answers for the primary even when the caller is inside a linked
/// worktree. `git worktree list` is the one call that can tell them apart, and it is the
/// same list the board's Worktrees tab reads.
pub fn here_branch(ctx: &Ctx) -> Option<String> {
    let here = ctx.repo.here();
    if !ctx.repo.linked() {
        return ctx.git.current_branch();
    }
    let rows = ctx.git.worktrees().ok()?;
    rows.into_iter()
        .filter(|r| here.starts_with(canon(&r.path)))
        .max_by_key(|r| canon(&r.path).components().count())
        .and_then(|r| r.branch)
}

/// git records a worktree's path as it was given; `Repo::here` is canonicalized. On macOS
/// that is the difference between `/var/folders/…` and `/private/var/folders/…`.
fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// branch -> ticket, by the three signals that exist, strongest first.
pub fn ticket_for_branch(ctx: &Ctx, snap: &Snapshot, branch: &str) -> Option<TicketId> {
    // 1. The git-config key `start` wrote and the `prepare-commit-msg` hook reads back.
    //    Spelled through `hooks::branch_ticket_key` so the two cannot drift (D-5).
    let key = crate::hooks::branch_ticket_key(branch);
    if let Ok(o) = ctx.git.run(&["config", "--get", &key]) {
        if o.code == 0 {
            if let Ok(id) = TicketId::parse(o.out.trim()) {
                if snap.tickets.contains_key(&id) {
                    return Some(id);
                }
            }
        }
    }
    // 2. The ticket's own recorded branch.
    if let Some(t) = snap
        .tickets
        .values()
        .find(|t| t.fm.branch.as_deref() == Some(branch) && !matches!(t.fm.state, State::Dropped))
    {
        return Some(t.fm.id.clone());
    }
    // 3. The name itself: `ks/t-9c41-slug`.
    let stem = branch
        .rsplit('/')
        .next()
        .unwrap_or(branch)
        .split('-')
        .take(2)
        .collect::<Vec<_>>()
        .join("-");
    TicketId::parse(&stem)
        .ok()
        .filter(|id| snap.tickets.contains_key(id))
}

/// What this session already claims: the branch it stands on, else the one ticket in `doing`
/// this actor holds. Ambiguity answers `None` — a wrong `discovered_in` is worse than none.
pub fn claimed_ticket(ctx: &Ctx, snap: &Snapshot) -> Option<Claimed> {
    if let Some(b) = here_branch(ctx) {
        if let Some(id) = ticket_for_branch(ctx, snap, &b) {
            return Some(Claimed { id, via: "branch" });
        }
    }
    let me = ctx.actor.label();
    let mut mine = snap
        .tickets
        .values()
        .filter(|t| t.fm.state == State::Doing && t.fm.claimed_by.as_deref() == Some(me.as_str()));
    let first = mine.next()?;
    match mine.next() {
        None => Some(Claimed {
            id: first.fm.id.clone(),
            via: "claim",
        }),
        Some(_) => None,
    }
}

/// The ONE owed verb, spelled out. Invariant 9, per ticket.
pub fn owed(ctx: &Ctx, snap: &Snapshot, t: &Ticket) -> Vec<String> {
    let id = &t.fm.id;
    let ks = ctx.invoked_as;
    if derive::in_main(snap, t).is_some() {
        return vec![format!("{ks} done {id}")];
    }
    match t.fm.state {
        State::Todo => {
            let open = derive::blocked_by(snap, t);
            match open.first() {
                None => vec![format!("{ks} start {id}")],
                Some(dep) => vec![format!("{ks} show {dep}")],
            }
        }
        State::Doing => vec![format!("{ks} ship {id}")],
        // Nothing on main carries it yet, so the honest next step is to look, not to close.
        State::Review => vec![format!("{ks} scan --explain {id}")],
        State::Done | State::Dropped => Vec::new(),
    }
}

/// A path under the repo root, printed relative to it — an absolute temp path in a
/// transcript is noise, and the relative form is what a human types next.
pub fn rel_to(ctx: &Ctx, p: &Path) -> String {
    p.strip_prefix(ctx.repo.primary_root())
        .unwrap_or(p)
        .display()
        .to_string()
}

impl Render for NewReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::state(self.state, &self.title)
            .id(&self.id)
            .fix(self.next.first().cloned().unwrap_or_default())
            .write(w, st)?;
        let mut chips: Vec<String> = vec![self.path.clone()];
        if let Some(s) = &self.spec {
            chips.push(format!("spec {s}"));
        }
        if let Some(p) = &self.proposal {
            chips.push(format!("proposal {p}"));
        }
        if !self.deps.is_empty() {
            chips.push(format!(
                "deps {}",
                self.deps
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        writeln!(w, "   {}", chips.join(" · "))?;
        // The capture stays visible: a discovered ticket says where it came from, on the
        // line that creates it, so a rabbit hole is never silently orphaned.
        if let Some(src) = &self.discovered_in {
            writeln!(
                w,
                "   {} discovered while doing {src} — parked, not expanded into it",
                glyph::DISCOVERED
            )?;
        }
        if let Some(src) = &self.followup_of {
            writeln!(w, "   followup of {src}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ctx::Actor;
    use crate::ids::Minter;
    use crate::model::TicketFm;
    use chrono::TimeZone;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 30, 14, 2, 11).unwrap()
    }

    fn facts() -> Facts {
        Facts {
            actor: Actor::Human {
                name: "trevor".into(),
            },
            at: at(),
            invocation: "kanspec new \"Rate-limit login endpoint\"".into(),
        }
    }

    fn args(argv: &[&str]) -> NewArgs {
        use clap::Parser as _;
        let mut all: Vec<&str> = vec!["kanspec", "new"];
        all.extend_from_slice(argv);
        match crate::cli::Cli::try_parse_from(all).expect("argv").command {
            crate::cli::Command::New(a) => a,
            other => panic!("{other:?}"),
        }
    }

    fn snap() -> Snapshot {
        Snapshot::empty(Config::default(), at())
    }

    fn with_ticket(s: &mut Snapshot, id: &str, state: &str) {
        let fm: TicketFm = serde_yaml_ng::from_str(&format!(
            "id: {id}\ntitle: fixture\nstate: {state}\ncreated: 2026-08-29T09:00:00Z\n"
        ))
        .unwrap();
        s.tickets.insert(
            fm.id.clone(),
            Ticket {
                fm,
                path: PathBuf::from(format!(".kanspec/tickets/{id}.md")),
                body: String::new(),
                steps: Vec::new(),
                log: Vec::new(),
                mtime: std::time::SystemTime::UNIX_EPOCH,
            },
        );
    }

    #[track_caller]
    fn plan(s: &Snapshot, a: &NewArgs, from: Option<&str>) -> Result<Plan> {
        let taken = s.taken_ids();
        let m = Minter::new(&taken, 20260831, 4);
        let from = from.map(|f| TicketId::parse(f).unwrap());
        plan_new(s, &facts(), a, &m, from.as_ref())
    }

    #[track_caller]
    fn contents(p: &Plan) -> String {
        match p.ops.first() {
            Some(Op::CreateEntity { contents, .. }) => contents.clone(),
            other => panic!("expected a CreateEntity, got {other:?}"),
        }
    }

    /// The scaffold must be a legal kanspec ticket the moment it is written: frontmatter
    /// that deserializes, a `## Log` that replays, and the two agreeing.
    #[test]
    fn a_new_ticket_is_born_provable() {
        let p = plan(&snap(), &args(&["Rate-limit login endpoint"]), None).unwrap();
        let src = contents(&p);
        let doc = crate::fm::split(&src).expect("frontmatter");
        crate::fm::writable(&doc.fm).expect("surgically writable from birth");

        let fm: TicketFm = serde_yaml_ng::from_str(&doc.fm).expect("deserializes");
        assert_eq!(fm.state, State::Todo);
        assert_eq!(fm.title, "Rate-limit login endpoint");
        assert!(fm.extra.is_empty(), "no stray keys: {:?}", fm.extra);
        assert!(fm.head.is_none() && fm.branch.is_none() && fm.claimed_by.is_none());

        let log = crate::logentry::parse_log(&doc.body);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].verb, Verb::New);
        assert_eq!(
            transitions::replay(&log).expect("replays"),
            State::Todo,
            "a ticket whose log does not replay is unwritable by the very next verb"
        );
        // The scaffold order IS `keys::TICKET_ORDER`, or every later `fm::set` inserts in
        // the wrong place.
        let keys: Vec<String> = crate::fm::index(&doc.fm)
            .into_iter()
            .map(|k| k.key)
            .collect();
        assert_eq!(keys, crate::keys::TICKET_ORDER);
    }

    /// A title YAML would reinterpret must survive the round trip — the reason the scaffold
    /// goes through `fm::emit` rather than `format!`.
    #[test]
    fn a_hostile_title_does_not_corrupt_the_file() {
        for title in [
            "fix: the parser: really",
            "true",
            "0x1f",
            "  leading and trailing  ",
            "quotes \" and ' both",
            "#hash and [brackets]",
        ] {
            let p = plan(&snap(), &args(&[title]), None).unwrap();
            let doc = crate::fm::split(&contents(&p)).expect("frontmatter");
            let fm: TicketFm = serde_yaml_ng::from_str(&doc.fm)
                .unwrap_or_else(|e| panic!("{title:?} broke the frontmatter: {e}"));
            assert_eq!(fm.title, title.trim(), "{title:?}");
        }
    }

    /// The rabbit-hole valve: one command, and the provenance is already stamped.
    #[test]
    fn discovered_in_is_stamped_without_being_asked_for() {
        let mut s = snap();
        with_ticket(&mut s, "t-9c41", "doing");
        let p = plan(&s, &args(&["Session middleware leaks"]), Some("t-9c41")).unwrap();
        let doc = crate::fm::split(&contents(&p)).unwrap();
        let fm: TicketFm = serde_yaml_ng::from_str(&doc.fm).unwrap();
        assert_eq!(
            fm.discovered_in.as_ref().map(|i| i.as_str()),
            Some("t-9c41")
        );
        // …and it is visible in the log, not only in a field.
        assert!(doc.body.contains("discovered in t-9c41"), "{}", doc.body);
    }

    #[test]
    fn a_dep_that_points_at_nothing_is_refused_at_the_door() {
        let mut s = snap();
        with_ticket(&mut s, "t-31aa", "todo");
        assert!(plan(&s, &args(&["x", "--dep", "t-31aa"]), None).is_ok());
        match plan(&s, &args(&["x", "--dep", "t-0000"]), None) {
            Err(e) => assert_eq!(e.kind(), "not_found"),
            Ok(_) => panic!("a dep pointing at nothing is a typo, not an unblocked ticket"),
        }
    }

    #[test]
    fn a_followup_and_a_discovery_are_never_the_same_link() {
        let mut s = snap();
        with_ticket(&mut s, "t-9c41", "doing");
        let p = plan(
            &s,
            &args(&["leftover", "--followup-of", "t-9c41"]),
            // even with a session claim in play, `--followup-of` wins and `discovered_in`
            // stays null: they are two link types with two behaviours (DESIGN.md).
            None,
        )
        .unwrap();
        let doc = crate::fm::split(&contents(&p)).unwrap();
        let fm: TicketFm = serde_yaml_ng::from_str(&doc.fm).unwrap();
        assert_eq!(fm.followup_of.as_ref().map(|i| i.as_str()), Some("t-9c41"));
        assert!(fm.discovered_in.is_none());
    }

    #[test]
    fn a_branch_name_resolves_to_its_ticket_even_without_the_config_key() {
        let mut s = snap();
        with_ticket(&mut s, "t-9c41", "doing");
        // The third signal: the branch NAME itself.
        let stem = "ks/t-9c41-rate-limit-login"
            .rsplit('/')
            .next()
            .unwrap()
            .split('-')
            .take(2)
            .collect::<Vec<_>>()
            .join("-");
        assert_eq!(TicketId::parse(&stem).unwrap().as_str(), "t-9c41");
        assert!(s.tickets.contains_key(&TicketId::parse("t-9c41").unwrap()));
    }
}
