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
use crate::git::Pathspec;
use crate::out::{glyph, Color, Line, Render, Style};

/// The board's own files. Deliberately NOT all of `.kanspec/**`: DESIGN.md's two-tier sync
/// model says the KNOWLEDGE layer (specs, decisions, quirks, proposals) *should* ride the
/// implementation branch and be reviewed like code, so a spec commit on a ticket branch is
/// correct and must never be reported as drift. Ticket state is the other tier — it is the
/// board, it belongs on main, and it is the one that wedges the daily loop.
const TRACKER: &str = ".kanspec/tickets/**";

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
    /// Ticket state that has ended up on a branch main cannot see. `None` is the normal
    /// answer; `Some` is the wedge, and it is measured from git, never asserted.
    pub tracker_drift: Option<TrackerDrift>,
    pub next: Vec<String>,
}

/// **The `git add -A` wedge, made visible.**
///
/// `start` without `--worktree` leaves the PRIMARY worktree — the one that owns
/// `.kanspec/` — sitting on the ticket branch. Under `sync = "batch"` (the default, D-13)
/// the tracker is *expected* to be dirty there, so the near-universal
/// `git add -A && git commit` sweeps `.kanspec/tickets/t-xxxx.md` onto the feature branch.
/// From then on:
///
/// * main's board is frozen at whatever state it had when the branch forked, and
/// * the next verb dirties the ticket file again, so `git checkout main` refuses with
///   *"Your local changes to the following files would be overwritten by checkout"* — and
///   the agent is stuck with every kanspec surface reporting that all is well.
///
/// kanspec does not own the user's git commands and cannot prevent this. It can measure it:
/// three-dot diff for "the branch carries tracker state main does not have", two-dot diff
/// intersected with the dirty set for "the switch will actually refuse".
#[derive(Debug, Serialize)]
pub struct TrackerDrift {
    /// the branch the PRIMARY worktree is standing on
    pub branch: String,
    /// the branch `git switch` would land on — a LOCAL name, never `origin/main`
    pub main: String,
    /// tracker files this branch carries that `main` does not
    pub committed: Vec<String>,
    /// of those, the ones a `git switch {main}` would refuse to overwrite right now
    pub blocking: Vec<String>,
    /// every tracker path the recovery has to move, committed and pending alike
    pub paths: Vec<String>,
    /// the recovery, verified end to end by `tests/lifecycle.rs`
    pub fix: String,
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
        // DO NOT "fix" the `kanspec` in `-m "kanspec: sync"` or in the `.kanspec` pathspec
        // when the D-44 sweep routes the remaining hardcoded command strings through
        // `invoked_as()`. Neither is a command word: one is a commit MESSAGE and the other
        // is a directory that is spelled `.kanspec` for a `ks` user too. `out::spoken`
        // rewrites only in command position precisely so it cannot reach them, and
        // `spoken_rewrites_the_command_word_and_nothing_else` pins both against a naive
        // `replace`. This string is a report field, not a `Fix`, so nothing rewrites it at
        // all today.
        //
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

    // `batch` ONLY, and the restriction is not laziness. `commit` mode auto-commits the
    // tracker to the branch you are standing on — that is what the user asked for, and it
    // can never wedge a checkout because nothing is ever left dirty; nagging about it every
    // time would be scolding the tool's own configured behaviour. `branch` mode (v0.4) parks
    // tracker state on a branch of its own. `batch` is the default, and the one where a
    // stray `git add -A` turns a deliberate dirty tree into a stuck one.
    let tracker_drift = match ctx.cfg.sync {
        SyncMode::Batch => tracker_drift(ctx),
        SyncMode::Commit | SyncMode::Branch => None,
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
    // A wedged working tree outranks every ticket verb: none of them can run until the
    // checkout works again, so the recovery goes to the FRONT of the agent's command list.
    if let Some(d) = &tracker_drift {
        next.insert(0, d.fix.clone());
    }

    Ok(StatusReport {
        you,
        agent,
        watching,
        pending_changes,
        cache_age_secs: snap.git.age(snap.now).map(|d| d.as_secs()),
        sync_fix,
        tracker_drift,
        next,
    })
}

/// Has ticket state ended up on a branch `main` cannot see? Measured, never guessed —
/// `None` whenever git declines to answer, because a read-only query must not fail and
/// "cannot tell" is not "you are wedged".
fn tracker_drift(ctx: &Ctx) -> Option<TrackerDrift> {
    // `ctx.git` is bound to the PRIMARY worktree, so this asks about the tree that owns
    // `.kanspec/` no matter which worktree the caller is standing in. Detached HEAD has no
    // branch to have drifted onto.
    let branch = ctx.git.current_branch()?;
    let resolved = ctx.git.resolve_main(&ctx.cfg.main).ok()?;
    // The name a human types. `main = "origin/main"` is the DEFAULT, and it is a REMOTE
    // ref — `git switch origin/main` DETACHES HEAD, so advice that pasted the configured
    // value straight through would hand the user a second wedge on top of the first.
    let main = short_name(ctx, &resolved);
    // Standing on main under either spelling: nothing has drifted anywhere. This is the
    // common case and it exits before a single diff runs.
    if branch == resolved || branch == main {
        return None;
    }

    let ps = [Pathspec::glob(TRACKER)];
    // Three-dot: what this branch ADDED since it forked. Two-dot would also count tracker
    // changes main made in the meantime, which are not this branch's doing and must not be
    // clobbered by the recovery below.
    let fork = format!("{resolved}...HEAD");
    let committed = paths_of(ctx, &["diff", "--name-only", "-z", &fork], &ps);
    // No tracker commit on this branch means no drift. A merely-dirty tracker is the NORMAL
    // state of `sync = "batch"` right after `start` — the file is still byte-identical to
    // main's, `git switch` carries it across, and warning about it would be pure noise.
    if committed.is_empty() {
        return None;
    }

    // From here on it IS drift, so the extra probes are worth their subprocesses. The
    // collision has to be measured against the commit `git switch {main}` would really land
    // on: the local branch when it exists, and otherwise the remote ref git's DWIM would
    // create it from.
    let rev = if local_branch(ctx, &main) {
        main.clone()
    } else {
        resolved.clone()
    };
    // Exactly git's own checkout rule: it refuses over a file that is modified in the
    // working tree AND whose content differs between HEAD and the branch being switched to.
    let differ = paths_of(ctx, &["diff", "--name-only", "-z", "HEAD", &rev], &ps);
    let dirty = paths_of(ctx, &["status", "--porcelain=v1", "-z"], &ps);
    let blocking: Vec<String> = differ
        .iter()
        .filter(|p| dirty.contains(p))
        .cloned()
        .collect();

    // Everything the recovery must carry to main: what the branch committed, plus what is
    // still pending (the pending edits get committed onto the branch first, so they become
    // part of the same transplant).
    let mut paths: Vec<String> = committed.iter().chain(&dirty).cloned().collect();
    paths.sort();
    paths.dedup();

    // Printed where it is read: from a linked worktree a bare `git` would operate on the
    // wrong tree, exactly as the `sync_fix` above learned.
    let g = if ctx.repo.linked() {
        format!("git -C {}", ctx.repo.primary_root().display())
    } else {
        "git".to_string()
    };
    let list = paths.join(" ");
    // Named paths rather than `.kanspec/tickets`: a wholesale checkout would also revert any
    // ticket main changed while this branch was away.
    let transplant = format!(
        "{g} switch {main} && {g} checkout {branch} -- {list} \
         && {g} commit -m \"kanspec: sync\" -- {list}"
    );
    let fix = if dirty.is_empty() {
        transplant
    } else {
        // The pending edits are the ones `git switch` refuses over, and they exist ONLY in
        // the working tree — committing them onto the branch is both what unblocks the
        // switch and what makes them survive the transplant. `commit -- <paths>` rather
        // than a bare `commit` so a half-staged source change is not swept into it.
        format!("{g} add -A -- {list} && {g} commit -m \"kanspec: sync\" -- {list} && {transplant}")
    };

    Some(TrackerDrift {
        branch,
        main,
        committed,
        blocking,
        paths,
        fix,
    })
}

/// The branch name a human would type for `resolved` — `origin/main` → `main`.
///
/// Structural rather than a guess: `refs/remotes/{resolved}` existing PROVES `resolved` is
/// a remote-tracking ref, and git forbids a `/` in a remote name, so the first component is
/// the remote and everything after it is the branch. `git switch` DWIMs that short name into
/// a local branch when there is not one already, which is exactly what the recovery wants.
fn short_name(ctx: &Ctx, resolved: &str) -> String {
    if !ref_exists(ctx, &format!("refs/remotes/{resolved}")) {
        return resolved.to_string();
    }
    match resolved.split_once('/') {
        Some((_remote, branch)) if !branch.is_empty() => branch.to_string(),
        _ => resolved.to_string(),
    }
}

fn local_branch(ctx: &Ctx, b: &str) -> bool {
    ref_exists(ctx, &format!("refs/heads/{b}"))
}

fn ref_exists(ctx: &Ctx, r: &str) -> bool {
    ctx.git
        .run(&["rev-parse", "--verify", "--quiet", r])
        .map(|o| o.code == 0)
        .unwrap_or(false)
}

/// Repo-relative paths out of a `-z` git listing. `--porcelain=v1 -z` prefixes each record
/// with `XY ` and puts a rename's ORIGINAL path in its own unprefixed field; `--name-only
/// -z` prefixes nothing. Both shapes are accepted, and `-z` means nothing is ever quoted.
/// Safe to disambiguate on byte 2 because the pathspec admits only `.kanspec/tickets/**`.
fn paths_of(ctx: &Ctx, args: &[&str], ps: &[Pathspec]) -> Vec<String> {
    let Ok(o) = ctx.git.run_ps(args, ps) else {
        return Vec::new();
    };
    if o.code != 0 {
        return Vec::new();
    }
    o.out
        .split('\0')
        .filter(|r| !r.is_empty())
        .map(|r| match r.as_bytes().get(2) {
            Some(b' ') => r[3..].to_string(),
            _ => r.to_string(),
        })
        .collect()
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

        // "nothing owed" is suppressed under drift on purpose: printing it directly above a
        // wedged working tree is the cheerful silence this whole check exists to end.
        if self.total() == 0 && self.tracker_drift.is_none() {
            writeln!(
                w,
                " {} nothing owed — every open item has its next verb",
                crate::out::paint(&glyph::OK.to_string(), Color::Green, st.color)
            )?;
        }

        match &self.tracker_drift {
            // Drift SUBSUMES the pending reminder rather than sitting next to it: while the
            // tracker is on a feature branch, `sync_fix` is not merely incomplete advice, it
            // is the trap — following it commits the ship record where main never sees it.
            Some(d) => {
                let n = d.paths.len();
                let line = if d.blocking.is_empty() {
                    format!(
                        "{n} tracker file{} committed on {} — {} cannot see the board",
                        plural(n),
                        d.branch,
                        d.main
                    )
                } else {
                    format!(
                        "`git switch {}` is blocked: {} tracker file{} changed on {}",
                        d.main,
                        d.blocking.len(),
                        plural(d.blocking.len()),
                        d.branch
                    )
                };
                Line::new('⚠', line).fix(&d.fix).write(w, st)?;
            }
            // `sync = "batch"` (the default): mutations dirty the working tree, and this is
            // the reminder that they are waiting to be committed (D-13).
            None if self.pending_changes > 0 => {
                Line::new(
                    '⧗',
                    format!("{} tracker changes pending", self.pending_changes),
                )
                .fix(&self.sync_fix)
                .write(w, st)?;
            }
            None => {}
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

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn owner_of(w: OwnerFilter) -> Owner {
    match w {
        OwnerFilter::You => Owner::You,
        OwnerFilter::Agent => Owner::Agent,
        OwnerFilter::Watching => Owner::Watching,
    }
}
