//! `BoardModel` — ONE struct for `kanspec board` **and** `/api/board`.
//!
//! The terminal board, the SPA and `board --export` all render this same value, so the
//! browser and the terminal cannot disagree about what a column contains. Every derived
//! field on it comes from `derive::compute`, which has exactly one implementation of
//! "stalled".
//!
//! **The merge badge is never a guess.** It is `derive::badge` verbatim — one of
//! `unpushed / pushed / in main (method [#pr] · checked_at) / unknown (why) /
//! never scanned` — and `badge_text` is baked here rather than at print time, because
//! `Render` has no clock and `snap.now` does.
//!
//! Owner: **S8**.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::ctx::Ctx;
use crate::derive::{self, Attention, Badge, Column, Staleness};
use crate::error::Result;
use crate::ids::{ProposalId, SpecName, TicketId};
use crate::model::{Snapshot, Ticket};
use crate::out::{glyph, Color, Line, Style};
use crate::transitions::State;

/// The six columns, in DESIGN.md's order. `Column::Dropped` is deliberately absent: a
/// dropped ticket is not on the board, it is in `kanspec ls --all`.
pub const COLUMNS: &[(Column, &str)] = &[
    (Column::Backlog, "BACKLOG"),
    (Column::Ready, "READY"),
    (Column::Doing, "DOING"),
    (Column::Review, "REVIEW"),
    (Column::InMain, "IN MAIN ⇂"),
    (Column::Done, "DONE (7d)"),
];

/// DESIGN.md's `DONE (7d)`: the closed column is a *recently* closed column, so the board
/// does not grow without bound and "done" keeps meaning "just landed".
pub const DONE_WINDOW_SECS: u64 = 7 * 86_400;

#[derive(Debug, Clone, Serialize)]
pub struct BoardModel {
    /// the snapshot revision this was built from — the SSE payload carries it (D-22/23)
    pub rev: u64,
    pub generated_at: DateTime<Utc>,
    pub columns: Vec<ColumnModel>,
    /// the pinned strip at the top of every tab
    pub attention: Vec<Attention>,
    /// the pinned strip at the bottom of every tab
    pub features: Vec<FeatureChip>,
    pub worktrees: Vec<WorktreeRowModel>,
    pub cache_age_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnModel {
    pub column: Column,
    pub title: &'static str,
    pub cards: Vec<Card>,
}

/// Every field DESIGN.md's card spec names — and the merge badge is **never a guess**.
#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub id: TicketId,
    pub title: String,
    /// the stored state, so the SPA and the terminal draw the same one-glyph badge
    pub state: State,
    pub spec: Option<SpecName>,
    pub proposal: Option<ProposalId>,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub claimed_by: Option<String>,
    pub updated_secs: Option<u64>,
    pub unresolved: usize,
    pub badge: Badge,
    /// `Badge::text(snap.now)` baked at build time — `Render` has no clock.
    pub badge_text: String,
    pub stalled_secs: Option<u64>,
    pub discovered_in: Option<TicketId>,
    pub deps: Vec<TicketId>,
    /// the unsatisfied subset of `deps` — what a BACKLOG card is actually waiting on
    pub blocked_by: Vec<TicketId>,
    /// invariant 9, per card: the ONE command that moves this ticket forward
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeatureChip {
    pub spec: SpecName,
    pub feature: String,
    pub staleness: Staleness,
    /// the dot's caption — "3 merges since 2026-08-01", "2 globs match nothing", …
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeRowModel {
    pub path: String,
    pub branch: Option<String>,
    pub ticket: Option<TicketId>,
    pub claimed_by: Option<String>,
    pub last_commit_secs: Option<u64>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    pub badge: Badge,
    pub badge_text: String,
    /// the primary worktree — the checkout everything else is linked to
    pub primary: bool,
}

/// THE builder. `cmd::board`, `/api/board` and `board --export` all call exactly this.
///
/// `&Ctx` buys exactly two things a `Snapshot` cannot carry: the live worktree list
/// (`git worktree list --porcelain`) and the ahead/behind figure for a worktree whose
/// branch the cache has never seen. Everything else is `derive`, computed once.
pub fn build(ctx: &Ctx, snap: &Snapshot) -> Result<BoardModel> {
    // ONE aggregate pass, so the browser and the terminal cannot disagree about "stalled".
    let d = derive::compute(snap);

    let mut columns: Vec<ColumnModel> = COLUMNS
        .iter()
        .map(|(column, title)| ColumnModel {
            column: *column,
            title,
            cards: Vec::new(),
        })
        .collect();

    for t in snap.tickets.values() {
        let col = derive::column(snap, t);
        // `Column::Dropped` has no slot: `position` returning `None` drops it off the board.
        let Some(slot) = COLUMNS.iter().position(|(c, _)| *c == col) else {
            continue;
        };
        if col == Column::Done && !recently_done(snap, t) {
            continue;
        }
        columns[slot].cards.push(card(snap, t));
    }

    // FIFO within a column — the same order `derive::ready_queue` uses — so nothing rots
    // at the bottom of the backlog. DONE is the one exception: newest first, because it is
    // a "what just landed" strip rather than a queue.
    for c in &mut columns {
        if c.column == Column::Done {
            c.cards.sort_by(|a, b| {
                a.updated_secs
                    .unwrap_or(u64::MAX)
                    .cmp(&b.updated_secs.unwrap_or(u64::MAX))
                    .then_with(|| a.id.cmp(&b.id))
            });
        } else {
            c.cards.sort_by(|a, b| {
                created_of(snap, &a.id)
                    .cmp(&created_of(snap, &b.id))
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
    }

    let features = snap
        .specs
        .values()
        .map(|sp| {
            let staleness = d
                .stale
                .get(&sp.name)
                .cloned()
                .unwrap_or(Staleness::NeverScanned);
            FeatureChip {
                note: staleness_note(&staleness),
                spec: sp.name.clone(),
                feature: sp.fm.feature.clone(),
                staleness,
            }
        })
        .collect();

    Ok(BoardModel {
        rev: snap.rev,
        generated_at: snap.now,
        columns,
        attention: d.attention,
        features,
        worktrees: worktrees(ctx, snap),
        cache_age_secs: snap.git.age(snap.now).map(|d| d.as_secs()),
    })
}

fn created_of(snap: &Snapshot, id: &TicketId) -> DateTime<Utc> {
    snap.tickets
        .get(id)
        .map(|t| t.fm.created)
        .unwrap_or(snap.now)
}

/// Closed inside the DONE window. A ticket nobody has touched for a fortnight is history,
/// not board state.
fn recently_done(snap: &Snapshot, t: &Ticket) -> bool {
    idle_secs(snap, t).is_none_or(|s| s <= DONE_WINDOW_SECS)
}

/// How long since ANYTHING happened to this ticket: the later of its last log entry and
/// its branch's last commit. Deliberately the same clock `derive::stalled` uses — an agent
/// committing without running a verb is not idle, and neither is one running verbs without
/// committing.
fn idle_secs(snap: &Snapshot, t: &Ticket) -> Option<u64> {
    let log = t.log.iter().map(|e| e.at).max();
    let commit = snap
        .git
        .branches
        .get(&t.fm.id)
        .and_then(|b| b.last_commit_at);
    let last = [log, commit]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(t.fm.created);
    (snap.now - last).to_std().ok().map(|d| d.as_secs())
}

fn card(snap: &Snapshot, t: &Ticket) -> Card {
    let badge = derive::badge(snap, t);
    let blocked_by: Vec<TicketId> = derive::blocked_by(snap, t).into_iter().cloned().collect();
    Card {
        badge_text: badge.text(snap.now),
        fix: card_fix(snap, t, &blocked_by),
        id: t.fm.id.clone(),
        title: t.fm.title.clone(),
        state: t.fm.state,
        spec: t.fm.spec.clone(),
        proposal: t.fm.proposal.clone(),
        branch: t.fm.branch.clone(),
        worktree: t.fm.worktree.as_ref().map(|p| p.display().to_string()),
        claimed_by: t.fm.claimed_by.clone(),
        updated_secs: idle_secs(snap, t),
        unresolved: t
            .fm
            .proposal
            .as_ref()
            .map(|p| derive::unresolved(snap, p))
            .unwrap_or(0),
        badge,
        stalled_secs: derive::stalled(snap, t).map(|d| d.as_secs()),
        discovered_in: t.fm.discovered_in.clone(),
        deps: t.fm.deps.clone(),
        blocked_by,
    }
}

/// The one command that discharges this card. It never guesses past a gate: an in-main
/// ticket owes `done`, a stalled claim owes `park`, and a blocked card owes nothing at all
/// — its dep does.
fn card_fix(snap: &Snapshot, t: &Ticket, blocked_by: &[TicketId]) -> Option<String> {
    let id = &t.fm.id;
    if derive::in_main(snap, t).is_some() {
        return Some(format!("kanspec done {id}"));
    }
    match t.fm.state {
        State::Todo if blocked_by.is_empty() => Some(format!("kanspec start {id}")),
        State::Todo => None,
        State::Doing if derive::stalled(snap, t).is_some() => {
            Some(format!("kanspec park {id} --why \"...\""))
        }
        State::Doing => Some(format!("kanspec ship {id}")),
        State::Review => Some(format!("kanspec scan {id}")),
        State::Done | State::Dropped => None,
    }
}

fn staleness_note(s: &Staleness) -> String {
    match s {
        Staleness::Ok => "fresh".to_string(),
        Staleness::Stale { merges, since, .. } => {
            format!("{merges} merges since {}", since.format("%Y-%m-%d"))
        }
        Staleness::DeadGlobs { globs } => match globs.len() {
            1 => format!("glob matches nothing: {}", globs[0]),
            n => format!("{n} globs match nothing"),
        },
        Staleness::NeverScanned => "never scanned".to_string(),
    }
}

/// The Worktrees tab: one row per checkout, with the ahead/behind-main figure DESIGN.md
/// asks for (`git rev-list --left-right --count`).
///
/// Cache-first, live fallback. `scan` already records `(ahead, behind)` per ticket branch,
/// and reusing it keeps a board refresh from spawning a subprocess per row on every SSE
/// tick; a worktree the cache has never seen — one created by hand, or a branch with no
/// ticket — is measured live rather than left blank.
fn worktrees(ctx: &Ctx, snap: &Snapshot) -> Vec<WorktreeRowModel> {
    let Ok(rows) = ctx.git.worktrees() else {
        // A read-only view must not fail because `git worktree list` had a bad day.
        return Vec::new();
    };
    let main = if snap.git.main.is_empty() {
        ctx.git.resolve_main(&ctx.cfg.main).ok()
    } else {
        Some(snap.git.main.clone())
    };

    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        if row.bare {
            continue;
        }
        let ticket = snap
            .tickets
            .values()
            .find(|t| t.fm.worktree.as_deref() == Some(row.path.as_path()))
            .or_else(|| {
                let branch = row.branch.as_deref()?;
                snap.tickets
                    .values()
                    .find(|t| t.fm.branch.as_deref() == Some(branch))
            });
        let fact = ticket.and_then(|t| snap.git.branches.get(&t.fm.id));
        let rev = row.branch.clone().or_else(|| row.head.clone());

        let (ahead, behind) = fact
            .and_then(|f| f.ahead.zip(f.behind))
            .or_else(|| ctx.git.ahead_behind(main.as_deref()?, rev.as_deref()?))
            .map_or((None, None), |(a, b)| (Some(a), Some(b)));

        let last_commit_at = fact
            .and_then(|f| f.last_commit_at)
            .or_else(|| rev.as_deref().and_then(|r| ctx.git.last_commit_at(r)));
        let badge = ticket
            .map(|t| derive::badge(snap, t))
            .unwrap_or(Badge::NeverScanned);

        out.push(WorktreeRowModel {
            path: row.path.display().to_string(),
            branch: row.branch.clone(),
            ticket: ticket.map(|t| t.fm.id.clone()),
            claimed_by: ticket.and_then(|t| t.fm.claimed_by.clone()),
            last_commit_secs: last_commit_at
                .and_then(|at| (snap.now - at).to_std().ok())
                .map(|d| d.as_secs()),
            ahead,
            behind,
            badge_text: badge.text(snap.now),
            badge,
            // `git worktree list` always prints the primary first.
            primary: i == 0,
        });
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Rendering — two surfaces over one model
// ─────────────────────────────────────────────────────────────────────────────

/// `board --export board.md` — a markdown snapshot for a PR.
///
/// One section per column rather than one six-wide table: a PR comment renders a table
/// whose cells are multi-line card blocks as a smear, and the point of the export is that
/// a reviewer can read it.
pub fn render_markdown(m: &BoardModel) -> String {
    let mut s = String::new();
    s.push_str("# kanspec board\n\n");
    s.push_str(&format!(
        "_generated {} · {}_\n",
        m.generated_at.format("%Y-%m-%dT%H:%MZ"),
        cache_age_line(m)
    ));

    if !m.attention.is_empty() {
        s.push_str("\n## Attention\n\n");
        for a in &m.attention {
            let subject = if a.subject.is_empty() {
                String::new()
            } else {
                format!("**{}** ", a.subject)
            };
            let fix = if a.fix.is_empty() {
                String::new()
            } else {
                format!(" — `{}`", a.fix)
            };
            s.push_str(&format!("- {} {subject}{}{fix}\n", a.glyph, a.line));
        }
    }

    for c in &m.columns {
        s.push_str(&format!("\n## {} ({})\n\n", c.title, c.cards.len()));
        if c.cards.is_empty() {
            s.push_str("_empty_\n");
            continue;
        }
        s.push_str("| ticket | title | spec | branch | agent | age | merge |\n");
        s.push_str("|---|---|---|---|---|---|---|\n");
        for card in &c.cards {
            s.push_str(&format!(
                "| {} `{}` | {} | {} | {} | {} | {} | {} |\n",
                chip(card),
                card.id,
                md_escape(&card.title),
                opt(card.spec.as_ref().map(|x| x.to_string())),
                opt(card.branch.clone()),
                opt(card.claimed_by.clone()),
                card.updated_secs.map(secs_short).unwrap_or_else(dash),
                md_escape(&card.badge_text),
            ));
        }
    }

    // The "unspecced" shame lane, exported too — a ticket with no capability is the thing
    // the By-spec tab exists to make impossible to ignore.
    let unspecced: Vec<&Card> = m
        .columns
        .iter()
        .flat_map(|c| c.cards.iter())
        .filter(|c| c.spec.is_none())
        .collect();
    if !unspecced.is_empty() {
        s.push_str(&format!("\n## Unspecced ({})\n\n", unspecced.len()));
        for card in unspecced {
            s.push_str(&format!("- `{}` {}\n", card.id, md_escape(&card.title)));
        }
    }

    if !m.worktrees.is_empty() {
        s.push_str("\n## Worktrees\n\n");
        s.push_str("| path | branch | ticket | agent | last commit | ahead/behind | merge |\n");
        s.push_str("|---|---|---|---|---|---|---|\n");
        for w in &m.worktrees {
            s.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                w.path,
                opt(w.branch.clone()),
                opt(w.ticket.as_ref().map(|t| t.to_string())),
                opt(w.claimed_by.clone()),
                w.last_commit_secs.map(secs_short).unwrap_or_else(dash),
                ahead_behind(w),
                md_escape(&w.badge_text),
            ));
        }
    }

    if !m.features.is_empty() {
        s.push_str("\n## Feature map\n\n");
        for f in &m.features {
            s.push_str(&format!(
                "- {} **{}** — {} _({})_\n",
                staleness_glyph(&f.staleness),
                f.spec,
                md_escape(&f.feature),
                f.note
            ));
        }
    }
    s
}

/// The terminal board — the cold fallback that must work with the server down.
///
/// NOTE (deviation from the wave-0 stub, reported): the stub sketched a six-wide
/// comfy-table mirroring DESIGN.md's ASCII picture. At the default 100-column width that
/// is ~14 characters per column, which wraps every branch name and every badge into
/// noise; the six-column layout is the *browser's* board, where the width exists. The
/// terminal renders the same `BoardModel` as one section per column in the `out::Line`
/// grammar the rest of the CLI already uses, and keeps `out::Table` for the Worktrees
/// strip, which genuinely is a table.
pub fn render_terminal(
    m: &BoardModel,
    w: &mut dyn std::io::Write,
    st: &Style,
) -> std::io::Result<()> {
    // Pinned strip, top: the `kanspec status` attention list.
    if !m.attention.is_empty() {
        writeln!(
            w,
            " {}",
            crate::out::paint("ATTENTION", Color::Bold, st.color)
        )?;
        for a in &m.attention {
            let mut line = Line::new(a.glyph, a.line.as_str());
            if !a.subject.is_empty() {
                line = line.id(&a.subject);
            }
            line.fix(a.fix.as_str()).write(w, st)?;
        }
        writeln!(w)?;
    }

    for c in &m.columns {
        writeln!(
            w,
            " {} {}",
            crate::out::paint(c.title, Color::Bold, st.color),
            crate::out::paint(&format!("({})", c.cards.len()), Color::Dim, st.color)
        )?;
        if c.cards.is_empty() {
            writeln!(w, "   {}", crate::out::paint("—", Color::Dim, st.color))?;
        }
        for card in &c.cards {
            let mut line = Line::new(card_glyph(card), card.title.as_str()).id(&card.id);
            if let Some(f) = &card.fix {
                line = line.fix(f.as_str());
            }
            line.write(w, st)?;
            let detail = card_detail(card);
            if !detail.is_empty() {
                writeln!(
                    w,
                    "             {}",
                    crate::out::paint(&detail, Color::Dim, st.color)
                )?;
            }
        }
        writeln!(w)?;
    }

    if !m.worktrees.is_empty() {
        writeln!(
            w,
            " {}",
            crate::out::paint("WORKTREES", Color::Bold, st.color)
        )?;
        let mut t = crate::out::Table::new(
            &[
                "path", "branch", "ticket", "agent", "commit", "±main", "merge",
            ],
            st,
        );
        for row in &m.worktrees {
            t.add_row(vec![
                short_path(&row.path),
                opt(row.branch.clone()),
                opt(row.ticket.as_ref().map(|x| x.to_string())),
                opt(row.claimed_by.clone()),
                row.last_commit_secs.map(secs_short).unwrap_or_else(dash),
                ahead_behind(row),
                row.badge_text.clone(),
            ]);
        }
        writeln!(w, "{t}")?;
    }

    // Pinned strip, bottom: the feature map with its staleness dots.
    if !m.features.is_empty() {
        writeln!(
            w,
            " {}",
            crate::out::paint("FEATURES", Color::Bold, st.color)
        )?;
        for f in &m.features {
            Line::new(staleness_glyph(&f.staleness), f.feature.as_str())
                .id(&f.spec)
                .dim(f.note.as_str())
                .write(w, st)?;
        }
    }
    Ok(())
}

/// The IN-MAIN overlay outranks the stored glyph: a review ticket git says has landed is
/// `⇂`, which is the whole point of the column it is sitting in.
fn card_glyph(c: &Card) -> char {
    if matches!(c.badge, Badge::InMain { .. }) && !c.state.terminal() {
        return glyph::IN_MAIN;
    }
    crate::out::state_glyph(c.state)
}

/// The chips DESIGN.md draws under a card title, in its order.
fn card_detail(c: &Card) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = &c.discovered_in {
        parts.push(format!("{} discovered in {d}", glyph::DISCOVERED));
    }
    if let Some(s) = &c.spec {
        parts.push(s.to_string());
    }
    if let Some(p) = &c.proposal {
        parts.push(p.to_string());
    }
    if let Some(b) = &c.branch {
        parts.push(format!("⎇ {b}"));
    }
    if let Some(wt) = &c.worktree {
        parts.push(format!("⌂ {}", short_path(wt)));
    }
    if let Some(a) = &c.claimed_by {
        parts.push(a.clone());
    }
    if let Some(s) = c.updated_secs {
        parts.push(format!("{} ago", secs_short(s)));
    }
    if c.unresolved > 0 {
        parts.push(format!("{} threads open", c.unresolved));
    }
    if !c.blocked_by.is_empty() {
        parts.push(format!(
            "blocked by {}",
            c.blocked_by
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    parts.push(c.badge_text.clone());
    if let Some(s) = c.stalled_secs {
        parts.push(format!("STALLED {}", secs_short(s)));
    }
    parts.join(" · ")
}

/// The staleness dot on the pinned feature strip.
pub fn staleness_glyph(s: &Staleness) -> char {
    match s {
        Staleness::Ok => '●',
        Staleness::Stale { .. } => '⚠',
        Staleness::DeadGlobs { .. } => '◌',
        Staleness::NeverScanned => '·',
    }
}

/// Every badge above was computed from the cache, so the cache's own age is part of the
/// answer — a merge state nobody has refreshed today must say so out loud.
pub fn cache_age_line(m: &BoardModel) -> String {
    match m.cache_age_secs {
        Some(age) => format!("merge state checked {} ago", secs_short(age)),
        None => "merge state never scanned".to_string(),
    }
}

/// The one glyph the export table gets per card: the ◇ discovered chip wins over the state.
fn chip(c: &Card) -> char {
    if c.discovered_in.is_some() {
        glyph::DISCOVERED
    } else {
        card_glyph(c)
    }
}

fn ahead_behind(w: &WorktreeRowModel) -> String {
    match (w.ahead, w.behind) {
        (Some(a), Some(b)) => format!("+{a}/-{b}"),
        _ => dash(),
    }
}

fn short_path(p: &str) -> String {
    match p.rsplit_once('/') {
        Some((head, tail)) => match head.rsplit_once('/') {
            Some((_, parent)) => format!("…/{parent}/{tail}"),
            None => format!("{head}/{tail}"),
        },
        None => p.to_string(),
    }
}

fn secs_short(s: u64) -> String {
    derive::short(std::time::Duration::from_secs(s))
}

fn dash() -> String {
    "—".to_string()
}

fn opt(v: Option<String>) -> String {
    v.unwrap_or_else(dash)
}

/// A ticket title is free text and lands inside a markdown table cell.
fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive::Owner;

    fn a_card(id: &str) -> Card {
        Card {
            id: TicketId::parse(id).unwrap(),
            title: "Rate-limit login".into(),
            state: State::Doing,
            spec: None,
            proposal: None,
            branch: Some("ks/t-9c41-rate-limit-login".into()),
            worktree: None,
            claimed_by: Some("claude/sess-a91".into()),
            updated_secs: Some(840),
            unresolved: 0,
            badge: Badge::Unpushed,
            badge_text: "unpushed".into(),
            stalled_secs: None,
            discovered_in: None,
            deps: vec![],
            blocked_by: vec![],
            fix: Some(format!("kanspec ship {id}")),
        }
    }

    fn a_model() -> BoardModel {
        BoardModel {
            rev: 3,
            generated_at: chrono::Utc::now(),
            columns: COLUMNS
                .iter()
                .map(|(column, title)| ColumnModel {
                    column: *column,
                    title,
                    cards: if *column == Column::Doing {
                        vec![a_card("t-9c41")]
                    } else {
                        vec![]
                    },
                })
                .collect(),
            attention: vec![Attention {
                owner: Owner::You,
                glyph: glyph::IN_MAIN,
                subject: "t-31aa".into(),
                line: "in main 2h, not closed".into(),
                fix: "kanspec done t-31aa".into(),
                url: None,
            }],
            features: vec![FeatureChip {
                spec: SpecName::parse("auth").unwrap(),
                feature: "Authentication".into(),
                staleness: Staleness::Ok,
                note: "fresh".into(),
            }],
            worktrees: vec![WorktreeRowModel {
                path: "/tmp/wt/t-9c41".into(),
                branch: Some("ks/t-9c41-rate-limit-login".into()),
                ticket: Some(TicketId::parse("t-9c41").unwrap()),
                claimed_by: Some("claude/sess-a91".into()),
                last_commit_secs: Some(600),
                ahead: Some(2),
                behind: Some(0),
                badge: Badge::Unpushed,
                badge_text: "unpushed".into(),
                primary: false,
            }],
            cache_age_secs: Some(240),
        }
    }

    #[test]
    fn the_six_columns_are_design_mds_six_columns() {
        let titles: Vec<&str> = COLUMNS.iter().map(|(_, t)| *t).collect();
        assert_eq!(
            titles,
            vec![
                "BACKLOG",
                "READY",
                "DOING",
                "REVIEW",
                "IN MAIN ⇂",
                "DONE (7d)"
            ]
        );
        // `Dropped` must have no slot — a dropped ticket is not board state.
        assert!(!COLUMNS.iter().any(|(c, _)| *c == Column::Dropped));
    }

    #[test]
    fn the_terminal_board_carries_the_badge_and_the_cache_age() {
        let m = a_model();
        let mut buf: Vec<u8> = Vec::new();
        render_terminal(&m, &mut buf, &Style::plain()).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("DOING"), "{out}");
        assert!(out.contains("t-9c41"), "{out}");
        assert!(out.contains("unpushed"), "{out}");
        assert!(out.contains("⎇ ks/t-9c41-rate-limit-login"), "{out}");
        assert!(out.contains("+2/-0"), "{out}");
        assert_eq!(cache_age_line(&m), "merge state checked 4m ago");
    }

    /// A badge is one of six shapes and never a guess — including "we never looked".
    #[test]
    fn every_badge_shape_renders_and_none_of_them_guesses() {
        let now = chrono::Utc::now();
        for (b, want) in [
            (Badge::Unpushed, "unpushed"),
            (Badge::Pushed, "pushed"),
            (Badge::NeverScanned, "never scanned"),
        ] {
            assert_eq!(b.text(now), want);
        }
        let in_main = Badge::InMain {
            method: crate::git::Method::GhPr,
            pr: None,
            sha: "a1b9c3d".into(),
            checked_at: now,
        };
        assert!(in_main.text(now).starts_with("in main (gh-pr · checked"));
        let unknown = Badge::Unknown {
            why: "squash suspected, no gh".into(),
            checked_at: None,
        };
        assert_eq!(unknown.text(now), "unknown (squash suspected, no gh)");
    }

    #[test]
    fn the_markdown_export_has_a_section_per_column_and_the_unspecced_lane() {
        let md = render_markdown(&a_model());
        for title in COLUMNS.iter().map(|(_, t)| *t) {
            assert!(md.contains(&format!("## {title}")), "{md}");
        }
        assert!(md.contains("## Attention"), "{md}");
        assert!(md.contains("## Worktrees"), "{md}");
        assert!(md.contains("## Feature map"), "{md}");
        // The card in the fixture has no spec, so the shame lane must name it.
        assert!(md.contains("## Unspecced (1)"), "{md}");
        assert!(md.contains("merge state checked 4m ago"), "{md}");
    }

    #[test]
    fn a_title_with_a_pipe_cannot_break_the_export_table() {
        let mut m = a_model();
        m.columns[2].cards[0].title = "a | b".into();
        let md = render_markdown(&m);
        assert!(md.contains("a \\| b"), "{md}");
    }

    #[test]
    fn the_discovered_chip_is_on_the_card_that_has_one() {
        let mut c = a_card("t-66d1");
        c.discovered_in = Some(TicketId::parse("t-9c41").unwrap());
        assert!(card_detail(&c).contains("◇ discovered in t-9c41"), "{c:?}");
        assert_eq!(chip(&c), '◇');
    }

    #[test]
    fn a_stalled_card_says_so_and_a_blocked_one_names_its_dep() {
        let mut c = a_card("t-88fe");
        c.stalled_secs = Some(3 * 3600);
        c.blocked_by = vec![TicketId::parse("t-31aa").unwrap()];
        let d = card_detail(&c);
        assert!(d.contains("STALLED 3h"), "{d}");
        assert!(d.contains("blocked by t-31aa"), "{d}");
    }

    #[test]
    fn staleness_dots_are_distinct() {
        let dots = [
            staleness_glyph(&Staleness::Ok),
            staleness_glyph(&Staleness::Stale {
                merges: 3,
                since: chrono::Utc::now(),
                examples: vec![],
            }),
            staleness_glyph(&Staleness::DeadGlobs {
                globs: vec!["src/x/**".into()],
            }),
            staleness_glyph(&Staleness::NeverScanned),
        ];
        let mut uniq = dots.to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), dots.len(), "two staleness states share a dot");
    }
}
