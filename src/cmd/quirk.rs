//! `quirk add|fix` and `quirks [--paths] [--touch]`.
//!
//! Capture friction near zero is why this registry accretes where ADR-era logs died; the
//! `--touch` form is the PostToolUse hook, which re-warns **at the moment an agent writes
//! a matching file** — the instant that actually prevents a stepped-on landmine.
//!
//! Owner: **S6**.

use serde::Serialize;

use crate::cli::{QuirkArgs, QuirkCommand, QuirksArgs, SeverityArg};
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::fm::{self, Yv};
use crate::ids::{QuirkId, TicketId};
use crate::keys::{Key, QuirkKey};
use crate::model::{QuirkStatus, Severity};
use crate::out::{glyph, Color, Line, Render, Style};
use crate::plan::{EntityRef, Op, Plan};
use crate::project;
use crate::rulesdoc::{severity_word, Scope};
use crate::store::Store;
use crate::{fix, fixes};

#[derive(Debug, Serialize)]
#[serde(tag = "quirk", rename_all = "snake_case")]
pub enum QuirkReport {
    Added {
        id: QuirkId,
        title: String,
        paths: Vec<String>,
        severity: Severity,
        source: Option<TicketId>,
        next: Vec<String>,
    },
    Fixed {
        id: QuirkId,
        title: String,
        by: TicketId,
        next: Vec<String>,
    },
}

pub fn quirk(ctx: &Ctx, a: &QuirkArgs) -> Result<QuirkReport> {
    ctx.require_initialized()?;
    match &a.cmd {
        QuirkCommand::Add {
            title,
            paths,
            severity,
            from,
        } => add(ctx, title, paths, severity_of(*severity), from.as_deref()),
        QuirkCommand::Fix { id, by } => fix_quirk(ctx, id, by),
    }
}

fn add(
    ctx: &Ctx,
    title: &str,
    paths: &[String],
    severity: Severity,
    from: Option<&str>,
) -> Result<QuirkReport> {
    if title.trim().is_empty() {
        return Err(KsError::invalid(
            "a quirk needs a title — one line, written while the burn is fresh",
            fixes![fix!(
                "{} quirk add \"...\" --paths \"src/**\"",
                ctx.invoked_as
            )],
        ));
    }
    // A glob that cannot compile matches nothing, so the landmine warning would never fire
    // — a silently useless quirk is worse than no quirk at all.
    Scope::of(paths)?;
    let source = from.map(TicketId::parse).transpose()?;

    let title = title.trim().to_string();
    let done = Store::open(ctx).transact(None, &ctx.invocation(), |s, m| {
        if let Some(t) = &source {
            // Provenance must point at a real ticket; `doctor` would otherwise find it.
            s.ticket(t)?;
        }
        let id = m.quirk(&title)?;
        let mut plan = Plan::of(vec![Op::CreateEntity {
            entity: EntityRef::Quirk(id.clone()),
            contents: scaffold(&id, &title, paths, severity, source.as_ref()),
        }]);
        plan.mint(EntityRef::Quirk(id));
        Ok(plan)
    })?;

    let id = minted_quirk(&done)?;
    project::regenerate(ctx)?;
    Ok(QuirkReport::Added {
        next: vec![
            format!("{} quirks --paths \"{}\"", ctx.invoked_as, first(paths)),
            format!("{} quirk fix {id} --by <ticket>", ctx.invoked_as),
        ],
        id,
        title,
        paths: paths.to_vec(),
        severity,
        source,
    })
}

fn fix_quirk(ctx: &Ctx, raw: &str, by: &str) -> Result<QuirkReport> {
    let id = QuirkId::parse(raw)?;
    let by = TicketId::parse(by)?;
    let snap = ctx.snapshot()?;
    let title = snap.quirk(&id)?.fm.title.clone();

    Store::open(ctx).transact(None, &ctx.invocation(), |s, _m| {
        let q = s.quirk(&id)?;
        // "Retired only by evidence": the ticket that claims the fix must exist, and a
        // quirk already retired is not evidence of anything new.
        s.ticket(&by)?;
        if q.fm.status != QuirkStatus::Active {
            return Err(KsError::gate(
                "quirk_not_active",
                format!("{id} is already `{}`", status_word(q.fm.status)),
                fixes![fix!("kanspec quirks")],
            ));
        }
        Ok(Plan::of(vec![Op::SetFields {
            entity: EntityRef::Quirk(id.clone()),
            sets: vec![
                (Key::Quirk(QuirkKey::Status), Yv::s("fixed")),
                (Key::Quirk(QuirkKey::FixedBy), Yv::s(by.to_string())),
            ],
        }]))
    })?;
    project::regenerate(ctx)?;

    Ok(QuirkReport::Fixed {
        next: vec![format!("{} quirks", ctx.invoked_as)],
        id,
        title,
        by,
    })
}

fn scaffold(
    id: &QuirkId,
    title: &str,
    paths: &[String],
    severity: Severity,
    source: Option<&TicketId>,
) -> String {
    format!(
        "---\nid: {id}\ntitle: {}\npaths: {}\nseverity: {}\nstatus: active\nsource: {}\n\
         fixed_by: null\n---\n{title}\n",
        fm::emit(&Yv::s(title), false),
        fm::emit(&Yv::list(paths.to_vec()), false),
        severity_word(severity),
        fm::emit(&Yv::opt_s(source.map(|t| t.to_string())), false),
    )
}

fn minted_quirk(done: &crate::store::Committed) -> Result<QuirkId> {
    done.minted
        .iter()
        .find_map(|e| match e {
            EntityRef::Quirk(id) => Some(id.clone()),
            _ => None,
        })
        .ok_or_else(|| KsError::internal(anyhow::anyhow!("the quirk plan minted no quirk id")))
}

fn first(paths: &[String]) -> String {
    paths.first().cloned().unwrap_or_else(|| "**".to_string())
}

fn severity_of(s: SeverityArg) -> Severity {
    match s {
        SeverityArg::Landmine => Severity::Landmine,
        SeverityArg::Gotcha => Severity::Gotcha,
        SeverityArg::Debt => Severity::Debt,
    }
}

fn status_word(s: QuirkStatus) -> &'static str {
    match s {
        QuirkStatus::Active => "active",
        QuirkStatus::Fixed => "fixed",
        QuirkStatus::Stale => "stale",
    }
}

impl Render for QuirkReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        let (line, next) = match self {
            QuirkReport::Added {
                id,
                title,
                paths,
                severity,
                ..
            } => (
                Line::new('▸', title.as_str()).id(id).dim(format!(
                    "· {} · {}",
                    severity_word(*severity),
                    paths.join(" ")
                )),
                self.next(),
            ),
            QuirkReport::Fixed { id, title, by, .. } => (
                Line::new(glyph::OK, format!("{title} — retired by {by}")).id(id),
                self.next(),
            ),
        };
        line.write(w, st)?;
        for n in next {
            writeln!(
                w,
                "  {} {}",
                glyph::FIX,
                crate::out::paint(n, Color::Cyan, st.color)
            )?;
        }
        Ok(())
    }
}

impl QuirkReport {
    fn next(&self) -> &[String] {
        match self {
            QuirkReport::Added { next, .. } | QuirkReport::Fixed { next, .. } => next,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// `quirks` — the listing, and the PostToolUse hook
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct QuirksReport {
    pub rows: Vec<QuirkRow>,
    /// set by the `--touch` hook form: the file that matched
    pub touched: Option<String>,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct QuirkRow {
    pub id: QuirkId,
    pub title: String,
    pub paths: Vec<String>,
    pub severity: Severity,
    pub status: QuirkStatus,
    pub source: Option<TicketId>,
    pub body: Option<String>,
}

pub fn quirks(ctx: &Ctx, a: &QuirksArgs) -> Result<QuirksReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;

    // `--touch <file>` asks one question about ONE file, so the file is the scope.
    let touched = a.touch.as_deref().map(|p| repo_relative(ctx, p));
    let scope = match &touched {
        Some(f) => Scope::of(std::slice::from_ref(f))?,
        None => Scope::of(&a.paths)?,
    };

    let mut rows: Vec<QuirkRow> = snap
        .quirks
        .values()
        .filter(|q| q.fm.status == QuirkStatus::Active)
        .filter(|q| scope.is_unscoped() || scope.touches(&q.fm.paths))
        .map(|q| QuirkRow {
            id: q.fm.id.clone(),
            title: q.fm.title.clone(),
            paths: q.fm.paths.clone(),
            severity: q.fm.severity,
            status: q.fm.status,
            source: q.fm.source.clone(),
            body: {
                let b = q.body.trim().to_string();
                (!b.is_empty() && b != q.fm.title).then_some(b)
            },
        })
        .collect();
    rows.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.id.cmp(&b.id)));

    let next = if rows.is_empty() {
        Vec::new()
    } else {
        vec![format!(
            "{} quirk fix {} --by <ticket>",
            ctx.invoked_as, rows[0].id
        )]
    };
    Ok(QuirksReport {
        rows,
        touched,
        next,
    })
}

/// The hook is handed whatever path the agent's tool used — absolute, or relative to
/// wherever that agent happens to stand, which in a linked worktree is not the primary
/// root. Quirk globs are written against the repo root, so the path is normalized to it.
fn repo_relative(ctx: &Ctx, p: &std::path::Path) -> String {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        ctx.repo.here().join(p)
    };
    let rel = abs
        .strip_prefix(ctx.repo.primary_root())
        .or_else(|_| abs.strip_prefix(ctx.repo.here()))
        .unwrap_or(p);
    rel.to_string_lossy().replace('\\', "/")
}

impl Render for QuirksReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // The hook form is silent when nothing matched. A PostToolUse hook that chatters on
        // every write gets uninstalled, and then the landmine warning is worth nothing.
        if self.touched.is_some() {
            for r in &self.rows {
                Line::new('⚠', format!("{} — {}", severity_word(r.severity), r.title))
                    .id(&r.id)
                    .write(w, st)?;
            }
            return Ok(());
        }

        for r in &self.rows {
            Line::new('⚠', &r.title)
                .id(&r.id)
                .dim(format!(
                    "· {} · {}{}",
                    severity_word(r.severity),
                    r.paths.join(" "),
                    r.source
                        .as_ref()
                        .map(|t| format!(" ← {t}"))
                        .unwrap_or_default()
                ))
                .write(w, st)?;
            if let Some(b) = &r.body {
                for line in b.lines() {
                    writeln!(w, "      {line}")?;
                }
            }
        }
        if self.rows.is_empty() {
            writeln!(
                w,
                " {} no active quirks here",
                crate::out::paint(&glyph::OK.to_string(), Color::Green, st.color)
            )?;
        }
        Ok(())
    }
}
