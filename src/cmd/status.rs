//! `status` — THE anti-stuck query. Everything non-terminal, grouped by who owes the next
//! verb (YOU / AGENT / WATCHING), one-command fix per line.
//!
//! "Stuck" means *appearing on a list* — the opposite of silent.
//!
//! Owner: **S4**.

use serde::Serialize;

use crate::cli::{OwnerFilter, StatusArgs};
use crate::config::SyncMode;
use crate::ctx::Ctx;
use crate::derive::{self, Attention, Owner};
use crate::doctor::{self, Severity};
use crate::error::Result;
use crate::out::{glyph, Color, Line, Render, Style};

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub you: Vec<Attention>,
    pub agent: Vec<Attention>,
    pub watching: Vec<Attention>,
    /// `sync = "batch"`: how many tracker changes are sitting uncommitted (D-13)
    pub pending_changes: u32,
    /// how old the merge-detection cache is, so a badge never lies about its freshness
    pub cache_age_secs: Option<u64>,
    /// The remediation for `pending_changes`, resolved against the primary worktree —
    /// built here rather than in `Render` because only the handler can see the repo.
    pub sync_fix: String,
    pub next: Vec<String>,
}

pub fn status(ctx: &Ctx, a: &StatusArgs) -> Result<StatusReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;

    let mut items = derive::attention(&snap);

    // A broken invariant is the most owed thing there is, so `doctor`'s Errors lead the
    // YOU group — in `run_all`'s order, which is already (severity, check, subject).
    // Warnings stay in `doctor`: `status` is the anti-stuck query, not a lint.
    let broken: Vec<Attention> = doctor::run_all(&snap)
        .into_iter()
        .filter(|f| f.severity == Severity::Error)
        .map(|f| Attention {
            owner: Owner::You,
            glyph: glyph::FAIL,
            subject: f.subject,
            line: f.message,
            fix: f.fix,
            url: None,
        })
        .collect();
    items.splice(0..0, broken);

    let group = |o: Owner| -> Vec<Attention> {
        if !a.owner.is_none_or(|w| owner_of(w) == o) {
            return Vec::new();
        }
        items.iter().filter(|i| i.owner == o).cloned().collect()
    };
    let you = group(Owner::You);
    let agent = group(Owner::Agent);
    let watching = group(Owner::Watching);

    // A read-only query must not fail because git had a bad day: a working tree we cannot
    // read is reported as "nothing pending" rather than as a refusal.
    let pending_changes = match ctx.cfg.sync {
        SyncMode::Batch => ctx
            .git
            .dirty_kanspec(&[&ctx.cfg.paths.features, &ctx.cfg.paths.architecture])
            .unwrap_or(0),
        SyncMode::Commit | SyncMode::Branch => 0,
    };

    // The tracker lives in the PRIMARY worktree, but `start --worktree` sends the agent
    // into a linked one — where a bare `git add` finds a clean tree, exits 1, and leaves
    // the primary dirty. The remediation has to name the tree it applies to.
    let sync_fix = {
        // Name a projection only when it is on disk: `git add` treats a pathspec matching
        // neither a file nor an index entry as fatal, so advice that named an
        // ungenerated projection would abort before staging anything at all.
        let mut paths = String::from(".kanspec");
        for p in [&ctx.cfg.paths.features, &ctx.cfg.paths.architecture] {
            if ctx.layout.repo_root().join(p).exists() {
                paths.push(' ');
                paths.push_str(&p.to_string_lossy());
            }
        }
        if ctx.repo.linked() {
            format!(
                "git -C {} add -A {paths} && git -C {} commit -m \"kanspec: sync\"",
                ctx.repo.primary_root().display(),
                ctx.repo.primary_root().display()
            )
        } else {
            format!("git add -A {paths} && git commit -m \"kanspec: sync\"")
        }
    };

    let mut next: Vec<String> = Vec::new();
    for i in you.iter().chain(&agent).chain(&watching) {
        if !i.fix.is_empty() && !next.contains(&i.fix) {
            next.push(i.fix.clone());
        }
        if next.len() == 5 {
            break;
        }
    }
    if snap.git.is_empty() {
        next.push(format!("{} scan", ctx.invoked_as));
    }

    Ok(StatusReport {
        you,
        agent,
        watching,
        pending_changes,
        cache_age_secs: snap.git.age(snap.now).map(|d| d.as_secs()),
        sync_fix,
        next,
    })
}

impl StatusReport {
    fn total(&self) -> usize {
        self.you.len() + self.agent.len() + self.watching.len()
    }
}

impl Render for StatusReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        for (name, group) in [
            ("YOU", &self.you),
            ("AGENT", &self.agent),
            ("WATCHING", &self.watching),
        ] {
            if group.is_empty() {
                continue;
            }
            writeln!(
                w,
                " {} ({})",
                crate::out::paint(name, Color::Bold, st.color),
                group.len()
            )?;
            for a in group {
                let mut line = Line::new(a.glyph, &a.line);
                if !a.subject.is_empty() {
                    line = line.id(&a.subject);
                }
                if !a.fix.is_empty() {
                    line = line.fix(&a.fix);
                }
                line.write(w, st)?;
            }
        }

        if self.total() == 0 {
            writeln!(
                w,
                " {} nothing owed — every open item has its next verb",
                crate::out::paint(&glyph::OK.to_string(), Color::Green, st.color)
            )?;
        }

        // `sync = "batch"` (the default): mutations dirty the working tree, and this is
        // the reminder that they are waiting to be committed (D-13).
        if self.pending_changes > 0 {
            Line::new(
                '⧗',
                format!("{} tracker changes pending", self.pending_changes),
            )
            .fix(&self.sync_fix)
            .write(w, st)?;
        }

        // Every badge above was computed from the cache, so the cache's own age is part of
        // the answer — a merge state nobody has refreshed today must say so out loud.
        match self.cache_age_secs {
            Some(age) => writeln!(
                w,
                "   {}",
                crate::out::paint(
                    &format!(
                        "merge state checked {} ago",
                        derive::short(std::time::Duration::from_secs(age))
                    ),
                    Color::Dim,
                    st.color
                )
            )?,
            None => Line::new('·', "merge state never scanned")
                .fix("kanspec scan")
                .write(w, st)?,
        }
        Ok(())
    }
}

fn owner_of(w: OwnerFilter) -> Owner {
    match w {
        OwnerFilter::You => Owner::You,
        OwnerFilter::Agent => Owner::Agent,
        OwnerFilter::Watching => Owner::Watching,
    }
}
