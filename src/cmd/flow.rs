//! `ready / start / ship / park / drop` — the daily loop.
//!
//! Every handler here has the same shape as the contract's worked example: gather every
//! subprocess, network call and clock read into a `*Facts` value **before** the lock, then
//! hand a pure planner to `Store::transact`. The server calls these same functions inside
//! `spawn_blocking`, so a POST and the equivalent CLI verb produce byte-identical files.
//!
//! Owner: **S5**.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::{DropArgs, ParkArgs, ReadyArgs, ShipArgs, StartArgs};
use crate::ctx::Ctx;
use crate::derive::{self, Badge};
use crate::error::{GateCode, KsError, Result};
use crate::fm::Yv;
use crate::ids::Minter;
use crate::ids::{ProposalId, QuirkId, SpecName, TicketId};
use crate::keys::TicketKey;
use crate::model::{DecisionStatus, QuirkStatus, Snapshot, Ticket};
use crate::out::join;
use crate::out::{glyph, Color, Line, Render, Style};
use crate::plan::{Facts, Op, Plan, ShipFacts, StartFacts};
use crate::rulesdoc::Scope;
use crate::store::Store;
use crate::transitions::{self, State, Verb};
use crate::{fix, fixes};

// ── ready ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ReadyReport {
    pub rows: Vec<ReadyRow>,
    /// todo tickets that are NOT ready, and what is holding each one
    pub blocked: Vec<BlockedRow>,
    pub next: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ReadyRow {
    pub id: TicketId,
    pub title: String,
    pub spec: Option<SpecName>,
    pub proposal: Option<ProposalId>,
    pub deps: Vec<TicketId>,
}

#[derive(Debug, Serialize)]
pub struct BlockedRow {
    pub id: TicketId,
    pub title: String,
    pub blocked_by: Vec<TicketId>,
}

pub fn ready(ctx: &Ctx, a: &ReadyArgs) -> Result<ReadyReport> {
    ctx.require_initialized()?;
    let snap = ctx.snapshot()?;
    let spec = a.spec.as_deref().map(SpecName::parse).transpose()?;
    let matches = |t: &Ticket| spec.as_ref().is_none_or(|s| t.fm.spec.as_ref() == Some(s));

    let rows: Vec<ReadyRow> = derive::ready_queue(&snap)
        .into_iter()
        .filter(|t| matches(t))
        .take(a.limit)
        .map(|t| ReadyRow {
            id: t.fm.id.clone(),
            title: t.fm.title.clone(),
            spec: t.fm.spec.clone(),
            proposal: t.fm.proposal.clone(),
            deps: t.fm.deps.clone(),
        })
        .collect();

    // The blocked tail is not decoration: "nothing is ready" and "everything is waiting on
    // t-31aa" are different situations with different next commands.
    let blocked: Vec<BlockedRow> = snap
        .tickets
        .values()
        .filter(|t| t.fm.state == State::Todo && !derive::is_ready(&snap, t))
        .filter(|t| matches(t))
        .map(|t| BlockedRow {
            id: t.fm.id.clone(),
            title: t.fm.title.clone(),
            blocked_by: derive::blocked_by(&snap, t).into_iter().cloned().collect(),
        })
        .collect();

    let next = match rows.first() {
        Some(r) => vec![format!("{} start {}", ctx.invoked_as, r.id)],
        None if blocked.is_empty() => vec![format!("{} new \"...\"", ctx.invoked_as)],
        None => vec![format!("{} status", ctx.invoked_as)],
    };
    Ok(ReadyReport {
        rows,
        blocked,
        next,
    })
}

impl Render for ReadyReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        for r in &self.rows {
            let mut chips: Vec<String> = Vec::new();
            if let Some(s) = &r.spec {
                chips.push(s.to_string());
            }
            if let Some(p) = &r.proposal {
                chips.push(p.to_string());
            }
            // Through `spoken`, like every `Fix`: a `ks` user is told to run `ks`.
            let mut line = Line::state(State::Todo, &r.title)
                .id(&r.id)
                .fix(format!("kanspec start {}", r.id));
            if !chips.is_empty() {
                line = line.dim(format!("· {}", chips.join(" · ")));
            }
            line.write(w, st)?;
        }
        if self.rows.is_empty() {
            Line::new('·', "nothing is claimable")
                .fix(self.next.first().cloned().unwrap_or_default())
                .write(w, st)?;
        }
        for b in &self.blocked {
            Line::new(
                '·',
                format!("{} — blocked by {}", b.title, ids(&b.blocked_by)),
            )
            .id(&b.id)
            .write(w, st)?;
        }
        Ok(())
    }
}

fn ids(v: &[TicketId]) -> String {
    if v.is_empty() {
        return "nothing".to_string();
    }
    join(v, ", ")
}

// ── start ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StartReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub branch: String,
    pub worktree: Option<String>,
    pub claimed_by: String,
    /// the context block from the DESIGN.md transcript
    pub spec: Option<SpecName>,
    pub spec_rules: usize,
    pub decisions_in_scope: Vec<String>,
    pub quirks_matching: Vec<QuirkId>,
    pub url: Option<String>,
    pub next: Vec<String>,
    /// whether the primary worktree was switched onto the ticket branch
    pub checked_out: bool,
    /// The one-line hazard notice a `--worktree`-less claim earns under `sync = "batch"`.
    /// See [`tracker_note`].
    pub tracker_note: Option<String>,
    /// HEAD carries commits the configured main does not, so the ticket branch — cut from
    /// main — starts without them. See [`integration_check`].
    pub base_note: Option<String>,
}

pub fn start(ctx: &Ctx, a: &StartArgs) -> Result<StartReport> {
    ctx.require_initialized()?;
    let id = TicketId::parse(&a.id)?;
    let snap = ctx.snapshot()?;
    let t = snap.ticket(&id)?;

    // Refuse the illegal move BEFORE creating a branch. The authoritative check still
    // happens inside the lock against a fresh snapshot — this one exists so a second
    // claimant does not leave a branch behind on its way to being told no.
    transitions::require(&id, t.fm.state, Verb::Start)?;

    // ── every subprocess happens HERE, before the lock ───────────────────────
    let base = ctx.git.resolve_main(&ctx.cfg.main)?;
    let branch = branch_name(ctx, &id, &t.fm.title);
    let existed = branch_exists(ctx, &branch);
    // Refuse to cut a branch that could not see the store, and say when the cut leaves
    // HEAD's own commits behind. A branch that already exists is not being cut from
    // anything, so neither question applies to it.
    let base_note = if existed {
        None
    } else {
        integration_check(ctx, &base)?
    };
    let want_wt = wants_worktree(ctx, a);
    let (wt_display, wt_abs) = if want_wt {
        let rel = worktree_rel(ctx, &id);
        let abs = ctx.repo.primary_root().join(&rel);
        (Some(rel), Some(abs))
    } else {
        (None, None)
    };

    // The branch's fork point, for the `## Log` note and the branch fact — NEVER for
    // `head:`. `head:` is written by `ship` out of real git output, and the ladder's guard
    // 0b keys off exactly that: a SHA from a live branch tip with no recorded `head:` is
    // how it recognises a branch that never carried a commit. Stamp it here and every
    // freshly-started ticket reads back as MERGED by ancestry (§2.16).
    let head = ctx.git.head_sha(if existed { &branch } else { &base })?;

    // Whether the primary is safe to move — measured NOW, before this verb writes a byte.
    //
    // ROUND-C FIX (integration). As shipped this ran a bare `git status --porcelain -uno`
    // AFTER `transact` returned. In any repo that COMMITS `.kanspec/` — which DESIGN.md
    // does — the ticket file this verb had just rewritten was then the only dirty entry, so
    // the guard refused every time and `checked_out` was unreachable in practice; the
    // dogfood run found it on the first claim. The load-bearing half of the fix is the
    // `.kanspec/**` exclusion in `primary_is_movable`; hoisting the call above the write is
    // the second half, and is what §2.16 asks of every other subprocess in this handler.
    let movable = wt_abs.is_none() && primary_is_movable(ctx);

    let made = create_branch(ctx, &id, &branch, &base, existed, wt_abs.as_deref())?;

    let f = StartFacts {
        base: ctx.facts(),
        branch: branch.clone(),
        worktree: wt_display.clone(),
        head,
    };
    let committed = match Store::open(ctx).transact(Some(Verb::Start), &ctx.invocation(), |s, m| {
        plan_start(s, &f, a, m)
    }) {
        Ok(c) => c,
        // The claim is what makes any of this real. If it is refused, undo exactly what
        // this invocation created — an orphan branch a later `start` would then refuse to
        // reuse is a worse failure than the one being reported.
        Err(e) => {
            unmake(ctx, &made);
            return Err(e);
        }
    };

    // The per-branch dispatch key the `prepare-commit-msg` / `commit-msg` pair reads back
    // (D-5). Spelled through `hooks::branch_ticket_key` so `start` and the hook cannot
    // disagree about it. Best effort: a repo whose config is read-only still has a claim.
    let _ = ctx.git.run(&[
        "config",
        &crate::hooks::branch_ticket_key(&branch),
        id.as_str(),
    ]);

    let checked_out = movable && switch_now(ctx, &branch);

    let t = committed.snapshot.ticket(&id)?;
    let cx = claim_context(&committed.snapshot, t);
    let mut next: Vec<String> = Vec::new();
    if let Some(p) = &wt_display {
        next.push(format!("cd {}", p.display()));
    } else if !checked_out {
        next.push(format!("git switch {branch}"));
    }
    next.push(format!("{} ship {id} --pr <n>", ctx.invoked_as));

    Ok(StartReport {
        title: t.fm.title.clone(),
        state: t.fm.state,
        tracker_note: tracker_note(ctx, a, &branch, &base),
        base_note,
        branch,
        worktree: wt_display.map(|p| p.display().to_string()),
        claimed_by: t.fm.claimed_by.clone().unwrap_or_else(|| ctx.actor.label()),
        spec: t.fm.spec.clone(),
        spec_rules: cx.spec_rules,
        decisions_in_scope: cx.decisions,
        quirks_matching: cx.quirks,
        url: Some(format!("http://127.0.0.1:{}/t/{id}", ctx.cfg.port)),
        next,
        checked_out,
        id,
    })
}

/// PURE. Refuses a second claim, and `require(from, Start)` supplies the typed refusal.
pub fn plan_start(s: &Snapshot, f: &StartFacts, a: &StartArgs, _m: &Minter) -> Result<Plan> {
    let id = TicketId::parse(&a.id)?;
    let t = s.ticket(&id)?;
    transitions::require(&id, t.fm.state, Verb::Start)?;

    // A `todo` ticket that still names a claimant was never released. `park` clears it, and
    // saying so beats silently stealing the claim.
    if t.fm.state == State::Todo {
        if let Some(who) = t.fm.claimed_by.as_deref().filter(|c| !c.is_empty()) {
            if who != f.base.actor.label() {
                return Err(KsError::conflict(
                    format!("{id} is still claimed by {who}"),
                    fixes![
                        fix!("kanspec park {id} --why \"reclaiming\""),
                        fix!("kanspec show {id}"),
                    ],
                ));
            }
        }
    }

    // DESIGN.md's own log note, verbatim.
    let detail = if f.worktree.is_some() {
        "branch + worktree created"
    } else {
        "branch created"
    };

    // NOTE the absent key: `TicketKey::Head` is NOT written here (§2.16's ⚠). The wave-0
    // stub's sketch listed it; writing it would make every freshly-started ticket read back
    // as MERGED by ancestry, a verified false positive for work that never happened.
    let mut also = vec![
        (TicketKey::Branch, Yv::s(f.branch.clone())),
        (TicketKey::ClaimedBy, Yv::s(f.base.actor.label())),
    ];
    also.push((
        TicketKey::Worktree,
        Yv::opt_s(f.worktree.as_ref().map(|p| p.display().to_string())),
    ));

    Ok(Plan::of(vec![Op::Transition {
        id,
        verb: Verb::Start,
        actor: f.base.actor.clone(),
        at: f.base.at,
        detail: detail.to_string(),
        also,
    }]))
}

impl Render for StartReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // DESIGN.md's claim transcript, line for line.
        writeln!(
            w,
            "  {}  {}  {}          ({})",
            crate::out::paint("claimed", Color::Green, st.color),
            crate::out::paint(self.id.as_str(), Color::Bold, st.color),
            self.title,
            crate::out::paint(
                &format!("logged: {} · {}", self.state, self.claimed_by),
                Color::Dim,
                st.color
            ),
        )?;
        let wt = match &self.worktree {
            Some(p) => format!("    worktree {p}"),
            None if self.checked_out => "    (checked out here)".to_string(),
            None => String::new(),
        };
        writeln!(w, "  branch   {}{wt}", self.branch)?;
        writeln!(w, "  context  {}", self.context_line())?;
        if let Some(u) = &self.url {
            writeln!(w, "  board    {u}")?;
        }
        if let Some(note) = &self.tracker_note {
            writeln!(
                w,
                "  {} {}",
                crate::out::paint("⚠ tracker", Color::Yellow, st.color),
                note
            )?;
        }
        if let Some(note) = &self.base_note {
            writeln!(
                w,
                "  {} {}",
                crate::out::paint("⚠ base", Color::Yellow, st.color),
                note
            )?;
        }
        for n in &self.next {
            writeln!(w, "  {} {n}", glyph::FIX)?;
        }
        Ok(())
    }
}

impl StartReport {
    /// `spec auth (3 rules) · 1 decision in scope (D-8c1a) · 2 quirks match paths (q-11ba)`
    pub fn context_line(&self) -> String {
        let spec = match &self.spec {
            Some(s) => format!(
                "spec {s} ({} rule{})",
                self.spec_rules,
                plural(self.spec_rules)
            ),
            None => "no spec".to_string(),
        };
        let d = format!(
            "{} decision{} in scope{}",
            self.decisions_in_scope.len(),
            plural(self.decisions_in_scope.len()),
            listed(&self.decisions_in_scope)
        );
        let q = format!(
            "{} quirk{} {} paths{}",
            self.quirks_matching.len(),
            plural(self.quirks_matching.len()),
            if self.quirks_matching.len() == 1 {
                "matches"
            } else {
                "match"
            },
            listed(&self.quirks_matching)
        );
        format!("{spec} · {d} · {q}")
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn listed<T: std::fmt::Display>(v: &[T]) -> String {
    if v.is_empty() {
        String::new()
    } else {
        format!(" ({})", join(v, ", "))
    }
}

// ── the claim-time context ───────────────────────────────────────────────────

/// What is already known about the ground this ticket is about to touch.
///
/// Read from the REAL knowledge entities in the snapshot — `specs/`, `decisions/`,
/// `quirks/` — which `store::load_snapshot` has parsed since wave 1.
///
/// It deliberately does NOT go through `rulesdoc::build`: `build` assembles the full
/// rendered rules document, and this needs three counts. The *scoping* does go through
/// `rulesdoc::Scope` — invariant 3 wants ONE definition of "in scope", and this is it.
struct ClaimContext {
    spec_rules: usize,
    decisions: Vec<String>,
    quirks: Vec<QuirkId>,
}

fn claim_context(s: &Snapshot, t: &Ticket) -> ClaimContext {
    let spec = t.fm.spec.as_ref().and_then(|n| s.specs.get(n));
    // At claim time there is no diff to match against — the work has not happened yet — so
    // the only honest scope is the spec's own `code:` globs. `Scope::touches` asks
    // glob-versus-glob BOTH ways (`src/auth/**` covers `src/auth/login.ts`, and a decision
    // scoped at `src/auth/login.ts` is in scope for a spec that owns `src/auth/**`), and
    // over-reports rather than under-reports, which is the right direction for a landmine
    // warning. A spec with no globs, or one whose hand-edited glob cannot compile, reaches
    // nothing; an entity with no globs is reached by nothing.
    let scope = spec.and_then(|sp| Scope::of(&sp.fm.code).ok());
    let touches = |globs: &[String]| scope.as_ref().is_some_and(|sc| sc.touches(globs));

    // Proposed decisions are NOT standing rules (invariant 8): they sit in `status` until a
    // human accepts them, and they must not steer a claim.
    let decisions = s
        .decisions
        .iter()
        .filter(|(_, d)| d.fm.status == DecisionStatus::Accepted && touches(&d.scope))
        .map(|(id, _)| id.to_string())
        .collect();
    let quirks = s
        .quirks
        .iter()
        .filter(|(_, q)| q.fm.status == QuirkStatus::Active && touches(&q.fm.paths))
        .map(|(id, _)| id.clone())
        .collect();
    ClaimContext {
        spec_rules: spec.map(|sp| sp.rules.len()).unwrap_or(0),
        decisions,
        quirks,
    }
}

// ── the git plumbing `start` owns ────────────────────────────────────────────

/// `ks/<id>-<slug>` — `branch_prefix` is configurable (D-21).
fn branch_name(ctx: &Ctx, id: &TicketId, title: &str) -> String {
    format!("{}{id}-{}", ctx.cfg.branch_prefix, crate::ids::slug(title))
}

/// `<worktree_dir>/<id>`, kept RELATIVE to the primary root exactly as DESIGN.md's
/// frontmatter shows it — an absolute temp path in a committed file is not portable.
fn worktree_rel(ctx: &Ctx, id: &TicketId) -> PathBuf {
    let path = ctx.cfg.worktree_dir.join(id.as_str());
    // These paths are committed in tickets and pasted into shell commands.
    // Forward slashes work on both platforms and keep the tracker portable.
    #[cfg(windows)]
    let path = PathBuf::from(path.to_string_lossy().replace('\\', "/"));
    path
}

fn branch_exists(ctx: &Ctx, branch: &str) -> bool {
    ctx.git
        .run(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .map(|o| o.code == 0)
        .unwrap_or(false)
}

/// What this invocation created, so a refused claim can put it back.
#[derive(Default)]
struct Made {
    branch: Option<String>,
    worktree: Option<PathBuf>,
}

fn create_branch(
    ctx: &Ctx,
    id: &TicketId,
    branch: &str,
    base: &str,
    existed: bool,
    worktree: Option<&Path>,
) -> Result<Made> {
    let mut made = Made::default();
    match (worktree, existed) {
        // `worktree add -b` creates both in one call, and `--no-track` is mandatory:
        // without it a later `git push` from the ticket branch targets main (D-21).
        (Some(p), false) => {
            ctx.git.worktree_add(p, branch, base)?;
            made.branch = Some(branch.to_string());
            made.worktree = Some(p.to_path_buf());
        }
        (Some(p), true) => {
            if !p.exists() {
                let out = ctx.git.run(&[
                    "worktree",
                    "add",
                    "--no-track",
                    &p.to_string_lossy(),
                    branch,
                ])?;
                if out.code != 0 {
                    return Err(KsError::conflict(
                        format!("cannot attach a worktree to `{branch}`: {}", out.err.trim()),
                        fixes![
                            fix!("git worktree list"),
                            fix!("kanspec where --branch {branch}")
                        ],
                    ));
                }
                made.worktree = Some(p.to_path_buf());
            }
        }
        (None, false) => {
            let out = ctx.git.run(&["branch", "--no-track", branch, base])?;
            if out.code != 0 {
                // A failed `git branch` normally creates nothing (permissions, a ref-name
                // collision, invalid storage). Suggesting `branch -D` in that case sends
                // the user to delete a ref that never existed and hides the useful retry.
                // Keep the cleanup only for the narrow race where the ref appeared after
                // `existed` was measured but before this command returned.
                let next = if branch_exists(ctx, branch) {
                    fixes![fix!("git branch -D {branch}"), fix!("kanspec start {id}"),]
                } else {
                    fixes![fix!("kanspec start {id}")]
                };
                return Err(KsError::conflict(
                    format!("cannot create branch `{branch}`: {}", out.err.trim()),
                    next,
                ));
            }
            made.branch = Some(branch.to_string());
        }
        (None, true) => {}
    }
    Ok(made)
}

/// Best effort, and deliberately silent: this runs while a refusal is already on its way
/// out, and a second error would bury the first.
fn unmake(ctx: &Ctx, made: &Made) {
    if let Some(p) = &made.worktree {
        let _ = ctx.git.worktree_remove(p, true);
    }
    if let Some(b) = &made.branch {
        let _ = ctx.git.branch_delete(b, true);
    }
}

/// May the PRIMARY worktree be moved onto the ticket branch without surprising anyone?
///
/// Only when the caller is standing in the primary and the tree carries no work of its own.
/// **Call this BEFORE `transact`** — see the call site: after the write, the ticket file
/// this verb just changed is itself the dirt, and the answer is permanently `false`.
///
/// `-uno` drops untracked files (they never block a checkout). `.kanspec/` is excluded on
/// top of that because under the default `sync = "batch"` the tracker is *expected* to be
/// dirty — that is what batching means — and those files are identical on a branch that was
/// forked from `base` a moment ago, so carrying them across is exactly right. What must
/// stop the switch is uncommitted work in the user's OWN source, and that is all this now
/// looks at.
fn primary_is_movable(ctx: &Ctx) -> bool {
    if ctx.repo.linked() {
        return false;
    }
    ctx.git
        .run(&[
            "status",
            "--porcelain",
            "-uno",
            "--",
            ".",
            ":(exclude,glob,top).kanspec/**",
        ])
        .map(|o| o.code == 0 && o.out.trim().is_empty())
        .unwrap_or(false)
}

/// The one line a `--worktree`-less claim earns, said at claim time rather than discovered
/// three commands later.
///
/// Without `--worktree` the PRIMARY worktree — the tree that owns `.kanspec/` — ends up on
/// the ticket branch, either because `start` switched it or because the report just told the
/// agent to. Under `sync = "batch"` (the default, D-13) the tracker is deliberately left
/// dirty there, so the near-universal `git add -A && git commit` sweeps
/// `.kanspec/tickets/t-xxxx.md` onto the feature branch. main's board freezes, and the next
/// verb re-dirties the file so `git checkout main` starts refusing outright.
///
/// kanspec does not own the user's git commands, so this cannot be *prevented* — the honest
/// move is to name the hazard and the alternative in one line, and to make `status` able to
/// see and undo it afterwards ([`crate::cmd::status::TrackerDrift`]).
///
/// Deliberately carries no `→ fix`: the only command that would swap this claim for the
/// worktree arrangement is `start --worktree`, and `start` on a `doing` ticket is an illegal
/// transition. A fix line that refuses when you run it is worse than no fix line.
/// `--worktree` / `--no-worktree` beat the repo's `worktree =` setting, which is off by
/// default. The flags are the one-off; the config is what the repo has settled on.
fn wants_worktree(ctx: &Ctx, a: &StartArgs) -> bool {
    if a.worktree {
        return true;
    }
    if a.no_worktree {
        return false;
    }
    ctx.cfg.worktree
}

/// Can a ticket branch be cut from `base` at all, and does the cut leave anything behind?
///
/// Found on a trial that integrated through a branch `origin/main` had never seen: `start`
/// cut the ticket branch from main, the branch had no `.kanspec/`, and nothing said so
/// until the next verb failed to find a store. The fix was `main = "<branch>"` in
/// config.toml, which nothing suggested. Two shapes, told apart by whether the store is
/// committed on HEAD but absent from `base`:
///
/// - **main is blind** — the store lives on a branch main does not contain. Refused, with
///   the config line as the fix: a claim whose branch cannot see the board is not a claim.
/// - **main is behind** — HEAD *is* main, just unpushed. Refused, with `git push` as the
///   fix: the `switch` that follows a claim would delete the tracked store from the working
///   tree, because the ticket branch does not have it.
///
/// When the store is fine but HEAD still carries commits `base` lacks, the claim goes
/// through with one line saying the ticket branch starts without them. A ticket branch of
/// another claim is expected to be ahead, so it earns no line.
fn integration_check(ctx: &Ctx, base: &str) -> Result<Option<String>> {
    // Detached HEAD is nowhere in particular; there is nothing to compare.
    let Some(on) = ctx.git.current_branch() else {
        return Ok(None);
    };
    let store = ctx.store_marker();
    let main = ctx.git.short_name(base);
    let ahead = ctx
        .git
        .ahead_behind(base, "HEAD")
        .map(|(ahead, _)| ahead)
        .unwrap_or(0);
    if ctx.git.carries("HEAD", &store) && !ctx.git.carries(base, &store) {
        if on == main {
            return Err(KsError::gate(
                GateCode::MainBehind,
                format!(
                    "{base} does not carry {store} yet — {on} is {ahead} commit{} ahead of \
                     it, and a ticket branch cut from {base} would have no store",
                    plural(ahead as usize)
                ),
                fixes![fix!("git push origin {on}")],
            ));
        }
        let suggest = if ctx.git.ref_exists(&format!("refs/remotes/origin/{on}")) {
            format!("origin/{on}")
        } else {
            on.clone()
        };
        return Err(KsError::gate(
            GateCode::MainBlind,
            format!(
                "{base} has never carried {store} — the store lives on {on}, and a ticket \
                 branch cut from {base} would have none"
            ),
            fixes![
                fix!("set `main = \"{suggest}\"` in .kanspec/config.toml"),
                fix!("kanspec instructions config"),
            ],
        ));
    }
    if ahead > 0 && !on.starts_with(&ctx.cfg.branch_prefix) {
        return Ok(Some(format!(
            "{on} is {ahead} commit{} ahead of {base} — the ticket branch is cut from {base} \
             and starts without them",
            plural(ahead as usize)
        )));
    }
    Ok(None)
}

fn tracker_note(ctx: &Ctx, a: &StartArgs, branch: &str, base: &str) -> Option<String> {
    if wants_worktree(ctx, a) || ctx.cfg.sync != crate::config::SyncMode::Batch {
        return None;
    }
    Some(format!(
        "`git add -A` on {branch} commits .kanspec/ where {base} never sees it \
         — `--worktree` keeps the tracker on {base}"
    ))
}

/// The switch itself, run after the claim actually landed — an attribution to a claim that
/// must have happened. Best effort: a refused checkout costs the report a line, never the
/// claim.
fn switch_now(ctx: &Ctx, branch: &str) -> bool {
    ctx.git
        .run(&["switch", branch])
        .map(|o| o.code == 0)
        .unwrap_or(false)
}

// ── ship ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ShipReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub head: String,
    pub pr: Option<u64>,
    pub badge: Badge,
    /// a red or still-running local CI run WARNS, never blocks
    pub ci_warning: Option<String>,
    pub next: Vec<String>,
}

pub fn ship(ctx: &Ctx, a: &ShipArgs) -> Result<ShipReport> {
    ctx.require_initialized()?;
    let id = TicketId::parse(&a.id)?;
    let snap = ctx.snapshot()?;
    let t = snap.ticket(&id)?;
    transitions::require(&id, t.fm.state, Verb::Ship)?;

    // Every subprocess happens HERE, before the lock. The SHA is read FROM GIT — the only
    // value that can reach `head:` comes from a `HeadSha`, which only `git.rs` mints, so
    // "never typed by an agent" is a property of the type rather than of anyone's manners.
    let rev = t.fm.branch.clone().unwrap_or_else(|| "HEAD".to_string());
    let head = ctx.git.head_sha(&rev).map_err(|e| {
        // A ticket claimed without a branch, or a branch someone deleted: say which,
        // because "cannot resolve HEAD" is not actionable.
        KsError::gate(
            GateCode::NoHeadToRecord,
            format!("{id}: cannot read a head SHA from `{rev}` — {e}"),
            fixes![
                fix!("git switch -c {rev}"),
                fix!("kanspec show {id}"),
                fix!("kanspec park {id} --why \"no branch\""),
            ],
        )
    })?;

    // A branch that never carried a commit has nothing to review, and shipping it would
    // record MAIN'S OWN SHA as `head:` — which the ladder then answers MERGED by ancestry,
    // a verified false positive for work that never happened. The ladder's guard 0b covers
    // the branch-tip case; this covers the recorded-`head:` case, which guard 0b cannot see
    // by construction (§2.15). "Cannot answer" is not a refusal: only a measured zero is.
    // A measured zero is also what a branch looks like once every commit of its own has
    // landed — merged before `ship` ran. The trailer tells the two apart, exactly as the
    // ladder's guard 0b does (t-174c): a branch whose ticket is on main has plenty to
    // record, and refusing it would leave the ticket unshippable forever.
    if let Ok(base) = ctx.git.resolve_main(&ctx.cfg.main) {
        let landed = matches!(
            ctx.git.grep_trailer(&base, &id),
            crate::git::Tri::Yes(ref shas) if !shas.is_empty()
        );
        if !landed
            && matches!(
                ctx.git.commits_ahead(&base, head.sha()),
                crate::git::Tri::Yes(0)
            )
        {
            return Err(KsError::gate(
                GateCode::NothingToShip,
                format!("{id}: `{rev}` carries no commits that {base} does not already have"),
                fixes![
                    fix!("git commit -m \"...\" && git push origin {rev}"),
                    fix!("kanspec park {id} --why \"not started yet\""),
                    fix!("kanspec done {id} --no-code --why \"docs only\""),
                ],
            ));
        }
    }

    let f = ShipFacts {
        base: ctx.facts(),
        head,
    };
    let committed = Store::open(ctx).transact(Some(Verb::Ship), &ctx.invocation(), |s, m| {
        plan_ship(s, &f, a, m)
    })?;

    let t = committed.snapshot.ticket(&id)?;
    Ok(ShipReport {
        title: t.fm.title.clone(),
        state: t.fm.state,
        head: t.fm.head.clone().unwrap_or_default(),
        pr: t.fm.pr,
        badge: derive::badge(&committed.snapshot, t),
        // v0.2 (S7's `ci.rs`, behind the non-default `ci-homerunner` feature): DESIGN.md's
        // "ship warns when the head SHA's latest local run is red". Deliberately absent
        // rather than faked — a warning nobody computed is worse than no warning.
        ci_warning: None,
        next: vec![
            format!("{} scan {id}", ctx.invoked_as),
            format!("{} done {id}", ctx.invoked_as),
        ],
        id,
    })
}

/// PURE. The `head:` value can only come from a `HeadSha`, which only `git.rs` mints —
/// DESIGN.md's second gear, enforced by the type.
pub fn plan_ship(s: &Snapshot, f: &ShipFacts, a: &ShipArgs, _m: &Minter) -> Result<Plan> {
    let id = TicketId::parse(&a.id)?;
    let t = s.ticket(&id)?;
    transitions::require(&id, t.fm.state, Verb::Ship)?;

    let sha = f.head.sha().as_str().to_string();
    let mut also = vec![(TicketKey::Head, Yv::s(sha.clone()))];
    // A `--pr` that is absent leaves whatever `ship` recorded last time: re-shipping to fix
    // a SHA must not silently forget the PR number rung 2 needs.
    if let Some(n) = a.pr {
        also.push((TicketKey::Pr, Yv::Int(n as i64)));
    }

    let detail = match a.pr.or(t.fm.pr) {
        Some(n) => format!("head {} · PR #{n}", &sha[..7.min(sha.len())]),
        None => format!("head {}", &sha[..7.min(sha.len())]),
    };
    Ok(Plan::of(vec![Op::Transition {
        id,
        verb: Verb::Ship,
        actor: f.base.actor.clone(),
        at: f.base.at,
        detail,
        also,
    }]))
}

impl Render for ShipReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::state(self.state, &self.title)
            .id(&self.id)
            .dim(format!(
                "· head {}{}",
                &self.head[..7.min(self.head.len())],
                self.pr.map(|n| format!(" · PR #{n}")).unwrap_or_default()
            ))
            .fix(self.next.last().cloned().unwrap_or_default())
            .write(w, st)?;
        if let Some(warn) = &self.ci_warning {
            writeln!(w, "   ⚠ {warn}")?;
        }
        writeln!(
            w,
            "   {}",
            crate::out::paint(
                "the head SHA was read from git, never typed",
                Color::Dim,
                st.color
            )
        )
    }
}

// ── park ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ParkReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub why: String,
    pub next: Vec<String>,
}

pub fn park(ctx: &Ctx, a: &ParkArgs) -> Result<ParkReport> {
    ctx.require_initialized()?;
    let id = TicketId::parse(&a.id)?;
    let f = ctx.facts();
    let committed = Store::open(ctx).transact(Some(Verb::Park), &ctx.invocation(), |s, m| {
        plan_park(s, &f, a, m)
    })?;
    let t = committed.snapshot.ticket(&id)?;
    Ok(ParkReport {
        title: t.fm.title.clone(),
        state: t.fm.state,
        why: a.why.trim().to_string(),
        next: vec![
            format!("{} ready", ctx.invoked_as),
            format!("{} start {id}", ctx.invoked_as),
        ],
        id,
    })
}

/// PURE.
pub fn plan_park(s: &Snapshot, f: &Facts, a: &ParkArgs, _m: &Minter) -> Result<Plan> {
    let id = TicketId::parse(&a.id)?;
    let t = s.ticket(&id)?;
    transitions::require(&id, t.fm.state, Verb::Park)?;
    let why = require_why(&id, &a.why, "park")?;

    Ok(Plan::of(vec![Op::Transition {
        id,
        verb: Verb::Park,
        actor: f.actor.clone(),
        at: f.at,
        detail: why,
        // The claim is released; the branch and worktree stay, so a later `start` picks the
        // work back up where it was rather than forking a second branch for one ticket.
        also: vec![(TicketKey::ClaimedBy, Yv::Null)],
    }]))
}

impl Render for ParkReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::state(self.state, format!("{} — {}", self.title, self.why))
            .id(&self.id)
            // `· ` prefix like every other dim chip in the crate: without it the reason and
            // the chip run together — `… — waiting on the limiter to land unclaimed`.
            .dim("· unclaimed")
            .fix(self.next.first().cloned().unwrap_or_default())
            .write(w, st)
    }
}

// ── drop ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DropReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    pub why: String,
    /// tickets that depended on this one — dropped counts as satisfied (D-17)
    pub unblocked: Vec<TicketId>,
    pub next: Vec<String>,
}

/// `drop_ticket`, not `drop`: shadowing `std::mem::drop` at the call site reads badly.
pub fn drop_ticket(ctx: &Ctx, a: &DropArgs) -> Result<DropReport> {
    ctx.require_initialized()?;
    let id = TicketId::parse(&a.id)?;
    let f = ctx.facts();
    let committed = Store::open(ctx).transact(Some(Verb::Drop), &ctx.invocation(), |s, m| {
        plan_drop(s, &f, a, m)
    })?;

    let snap = &committed.snapshot;
    let t = snap.ticket(&id)?;
    // Dropped counts as satisfied (D-17): blocking forever on a dropped dep is worse. Say
    // which tickets that just freed, because `doctor` will also warn about every one of
    // them and the human should see it here first.
    let unblocked: Vec<TicketId> = snap
        .tickets
        .values()
        .filter(|o| o.fm.deps.contains(&id) && derive::is_ready(snap, o))
        .map(|o| o.fm.id.clone())
        .collect();
    let mut next = vec![format!("{} ready", ctx.invoked_as)];
    if !unblocked.is_empty() {
        next.insert(0, format!("{} start {}", ctx.invoked_as, unblocked[0]));
    }
    Ok(DropReport {
        title: t.fm.title.clone(),
        state: t.fm.state,
        why: a.why.trim().to_string(),
        unblocked,
        next,
        id,
    })
}

/// PURE.
pub fn plan_drop(s: &Snapshot, f: &Facts, a: &DropArgs, _m: &Minter) -> Result<Plan> {
    let id = TicketId::parse(&a.id)?;
    let t = s.ticket(&id)?;
    transitions::require(&id, t.fm.state, Verb::Drop)?;
    let why = require_why(&id, &a.why, "drop")?;

    Ok(Plan::of(vec![Op::Transition {
        id,
        verb: Verb::Drop,
        actor: f.actor.clone(),
        at: f.at,
        detail: why,
        also: vec![(TicketKey::ClaimedBy, Yv::Null)],
    }]))
}

impl Render for DropReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::state(self.state, format!("{} — {}", self.title, self.why))
            .id(&self.id)
            .fix(self.next.first().cloned().unwrap_or_default())
            .write(w, st)?;
        if !self.unblocked.is_empty() {
            writeln!(
                w,
                "   unblocked {} (a dropped dep counts as satisfied)",
                ids(&self.unblocked)
            )?;
        }
        Ok(())
    }
}

/// clap makes `--why` mandatory; this catches the whitespace-only version, which is the one
/// an agent actually produces.
fn require_why(id: &TicketId, why: &str, verb: &'static str) -> Result<String> {
    let why = why.trim();
    if why.is_empty() {
        return Err(KsError::gate(
            GateCode::WhyIsRequired,
            format!("`{verb} {id}` records a reason, so nothing rots silently"),
            fixes![
                fix!("kanspec {verb} {id} --why \"what actually happened\""),
                fix!("kanspec show {id}"),
            ],
        ));
    }
    Ok(why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ctx::Actor;
    use crate::model::{Decision, DecisionFm, Quirk, QuirkFm, Spec, SpecFm, Ticket, TicketFm};
    use chrono::{DateTime, TimeZone, Utc};

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 30, h, 0, 0).unwrap()
    }

    fn facts() -> Facts {
        Facts {
            actor: Actor::Human {
                name: "trevor".into(),
            },
            at: at(14),
            invocation: "kanspec start t-9c41".into(),
        }
    }

    fn snap_with(state: State) -> Snapshot {
        let mut s = Snapshot::empty(Config::default(), at(15));
        let fm: TicketFm = serde_yaml_ng::from_str(&format!(
            "id: t-9c41\ntitle: Rate-limit login endpoint\nstate: {state}\nspec: auth\n\
             created: 2026-08-30T09:00:00Z\n"
        ))
        .unwrap();
        s.tickets.insert(
            fm.id.clone(),
            Ticket {
                fm,
                path: PathBuf::from(".kanspec/tickets/t-9c41.md"),
                body: String::new(),
                steps: Vec::new(),
                log: Vec::new(),
                mtime: std::time::SystemTime::UNIX_EPOCH,
            },
        );
        s
    }

    fn with_knowledge(s: &mut Snapshot) {
        let name = SpecName::parse("auth").unwrap();
        let fm: SpecFm = serde_yaml_ng::from_str("feature: Login\ncode: [src/auth/**]\n").unwrap();
        s.specs.insert(
            name.clone(),
            Spec {
                name,
                fm,
                path: PathBuf::from(".kanspec/specs/auth.md"),
                body: String::new(),
                rules: vec![
                    rule("auth.jwt"),
                    rule("auth.lockout"),
                    rule("auth.lockout-event"),
                ],
            },
        );
        for (id, status, scope) in [
            ("D-8c1a", "accepted", "src/auth/**"),
            ("D-2c77", "accepted", "src/billing/**"),
            // A PROPOSED decision is not a standing rule and must not steer a claim.
            ("D-9999", "proposed", "src/auth/**"),
        ] {
            let fm: DecisionFm = serde_yaml_ng::from_str(&format!(
                "id: {id}\ntitle: t\nstatus: {status}\ndate: 2026-09-02\nscope: [{scope}]\n"
            ))
            .unwrap();
            let scope = fm.scope.clone();
            s.decisions.insert(
                fm.id.clone(),
                Decision {
                    fm,
                    path: PathBuf::from("d.md"),
                    body: String::new(),
                    scope,
                },
            );
        }
        for (id, status, paths) in [
            ("q-11ba", "active", "src/auth/**"),
            ("q-83d0", "active", "src/auth/session.ts"),
            ("q-0000", "fixed", "src/auth/**"),
            ("q-1111", "active", "src/billing/**"),
        ] {
            let fm: QuirkFm = serde_yaml_ng::from_str(&format!(
                "id: {id}\ntitle: t\npaths: [{paths}]\nseverity: landmine\nstatus: {status}\n"
            ))
            .unwrap();
            s.quirks.insert(
                fm.id.clone(),
                Quirk {
                    fm,
                    path: PathBuf::from("q.md"),
                    body: String::new(),
                },
            );
        }
    }

    fn rule(anchor: &str) -> crate::model::Rule {
        crate::model::Rule {
            anchor: anchor.to_string(),
            text: "t".into(),
            provenance: Vec::new(),
            items: Vec::new(),
            line: 1,
        }
    }

    fn start_args(worktree: bool) -> StartArgs {
        StartArgs {
            id: "t-9c41".into(),
            worktree,
            no_worktree: false,
        }
    }

    fn start_facts(worktree: Option<&str>) -> StartFacts {
        // `HeadSha` has no public constructor, so a planner test builds one the only way
        // anything can: out of real git output, from this very repository.
        let git = crate::git::Git::bind(Path::new(env!("CARGO_MANIFEST_DIR")));
        StartFacts {
            base: facts(),
            branch: "ks/t-9c41-rate-limit-login-endpoint".into(),
            worktree: worktree.map(PathBuf::from),
            head: git.head_sha("HEAD").expect("this crate is a git repo"),
        }
    }

    #[track_caller]
    fn plan(s: &Snapshot, f: &StartFacts, a: &StartArgs) -> Result<Plan> {
        let taken = s.taken_ids();
        let m = Minter::new(&taken, 7, 4);
        plan_start(s, f, a, &m)
    }

    #[track_caller]
    fn refusal<T>(r: Result<T>) -> &'static str {
        match r {
            Ok(_) => panic!("expected a refusal"),
            Err(e) => e.code().unwrap_or(e.kind()),
        }
    }

    /// §2.16's ⚠, as a test. `head:` at claim time makes every freshly-started ticket read
    /// back as MERGED by ancestry — a VERIFIED false positive for work that never happened,
    /// which is the single worst answer this tool can give.
    #[test]
    fn start_never_writes_head() {
        let p = plan(
            &snap_with(State::Todo),
            &start_facts(None),
            &start_args(false),
        )
        .unwrap();
        let Some(Op::Transition { also, verb, .. }) = p.ops.first() else {
            panic!("start is one transition");
        };
        assert_eq!(*verb, Verb::Start);
        assert!(
            !also.iter().any(|(k, _)| *k == TicketKey::Head),
            "plan_start must not write `head:` — see ARCHITECTURE §2.16"
        );
        let keys: Vec<&str> = also
            .iter()
            .map(|(k, _)| crate::keys::FmKey::as_str(*k))
            .collect();
        assert_eq!(keys, ["branch", "claimed_by", "worktree"]);
    }

    #[test]
    fn the_claim_is_atomic_because_the_second_one_is_refused() {
        // The first claim moved it to `doing`; the second sees the fresh in-lock snapshot.
        assert_eq!(
            refusal(plan(
                &snap_with(State::Doing),
                &start_facts(None),
                &start_args(false)
            )),
            "illegal_transition"
        );
        // Rework is legal, and is the other edge of the same table.
        assert!(plan(
            &snap_with(State::Review),
            &start_facts(None),
            &start_args(false)
        )
        .is_ok());
    }

    #[test]
    fn a_todo_ticket_someone_else_still_holds_is_not_silently_stolen() {
        let mut s = snap_with(State::Todo);
        let id = TicketId::parse("t-9c41").unwrap();
        s.tickets.get_mut(&id).unwrap().fm.claimed_by = Some("claude/sess-a91".into());
        assert_eq!(
            refusal(plan(&s, &start_facts(None), &start_args(false))),
            "conflict"
        );
    }

    #[test]
    fn the_worktree_is_recorded_as_the_relative_path_design_md_shows() {
        let p = plan(
            &snap_with(State::Todo),
            &start_facts(Some("../kanspec-wt/t-9c41")),
            &start_args(true),
        )
        .unwrap();
        let Some(Op::Transition { also, detail, .. }) = p.ops.first() else {
            panic!()
        };
        assert_eq!(detail, "branch + worktree created");
        let wt = also
            .iter()
            .find(|(k, _)| *k == TicketKey::Worktree)
            .map(|(_, v)| v.clone());
        assert_eq!(wt, Some(Yv::s("../kanspec-wt/t-9c41")));
    }

    /// The context line every prior plan stubbed. It reads the REAL knowledge entities, and
    /// it filters on status: a proposed decision and a fixed quirk steer nobody.
    #[test]
    fn the_claim_context_is_computed_from_the_real_knowledge_entities() {
        let mut s = snap_with(State::Todo);
        with_knowledge(&mut s);
        let t = s.ticket(&TicketId::parse("t-9c41").unwrap()).unwrap();
        let cx = claim_context(&s, t);
        assert_eq!(cx.spec_rules, 3);
        assert_eq!(cx.decisions, ["D-8c1a"], "accepted + in scope only");
        assert_eq!(
            cx.quirks.iter().map(|q| q.to_string()).collect::<Vec<_>>(),
            ["q-11ba", "q-83d0"],
            "active + path-matching only"
        );

        let report = StartReport {
            id: TicketId::parse("t-9c41").unwrap(),
            title: "Rate-limit login endpoint".into(),
            state: State::Doing,
            branch: "ks/t-9c41-rate-limit-login".into(),
            worktree: Some("../kanspec-wt/t-9c41".into()),
            claimed_by: "claude/sess-a91".into(),
            spec: Some(SpecName::parse("auth").unwrap()),
            spec_rules: cx.spec_rules,
            decisions_in_scope: cx.decisions,
            quirks_matching: cx.quirks,
            url: None,
            next: Vec::new(),
            checked_out: false,
            tracker_note: None,
            base_note: None,
        };
        // DESIGN.md's transcript, verbatim.
        assert_eq!(
            report.context_line(),
            "spec auth (3 rules) · 1 decision in scope (D-8c1a) · 2 quirks match paths (q-11ba, q-83d0)"
        );
    }

    #[test]
    fn a_ticket_with_no_spec_still_gets_an_honest_context_line() {
        let mut s = snap_with(State::Todo);
        with_knowledge(&mut s);
        let id = TicketId::parse("t-9c41").unwrap();
        s.tickets.get_mut(&id).unwrap().fm.spec = None;
        let t = s.ticket(&id).unwrap();
        let cx = claim_context(&s, t);
        assert_eq!(cx.spec_rules, 0);
        assert!(cx.decisions.is_empty() && cx.quirks.is_empty());
    }

    #[test]
    fn ship_records_the_sha_git_printed_and_the_pr_number() {
        let s = snap_with(State::Doing);
        let git = crate::git::Git::bind(Path::new(env!("CARGO_MANIFEST_DIR")));
        let head = git.head_sha("HEAD").unwrap();
        let sha = head.sha().as_str().to_string();
        let f = ShipFacts {
            base: facts(),
            head,
        };
        let a = ShipArgs {
            id: "t-9c41".into(),
            pr: Some(142),
        };
        let taken = s.taken_ids();
        let p = plan_ship(&s, &f, &a, &Minter::new(&taken, 7, 4)).unwrap();
        let Some(Op::Transition { also, detail, .. }) = p.ops.first() else {
            panic!()
        };
        assert!(also.contains(&(TicketKey::Head, Yv::s(sha.clone()))));
        assert!(also.contains(&(TicketKey::Pr, Yv::Int(142))));
        assert_eq!(detail, &format!("head {} · PR #142", &sha[..7]));
    }

    #[test]
    fn re_shipping_without_pr_keeps_the_recorded_one() {
        let mut s = snap_with(State::Doing);
        let id = TicketId::parse("t-9c41").unwrap();
        s.tickets.get_mut(&id).unwrap().fm.pr = Some(142);
        let git = crate::git::Git::bind(Path::new(env!("CARGO_MANIFEST_DIR")));
        let f = ShipFacts {
            base: facts(),
            head: git.head_sha("HEAD").unwrap(),
        };
        let a = ShipArgs {
            id: "t-9c41".into(),
            pr: None,
        };
        let taken = s.taken_ids();
        let p = plan_ship(&s, &f, &a, &Minter::new(&taken, 7, 4)).unwrap();
        let Some(Op::Transition { also, detail, .. }) = p.ops.first() else {
            panic!()
        };
        assert!(
            !also.iter().any(|(k, _)| *k == TicketKey::Pr),
            "an absent --pr must not erase the number rung 2 needs"
        );
        assert!(detail.contains("PR #142"), "{detail}");
    }

    #[test]
    fn park_and_drop_release_the_claim_and_demand_a_reason() {
        let s = snap_with(State::Doing);
        let taken = s.taken_ids();
        let m = Minter::new(&taken, 7, 4);

        let p = plan_park(
            &s,
            &facts(),
            &ParkArgs {
                id: "t-9c41".into(),
                why: "waiting on the redis cluster".into(),
            },
            &m,
        )
        .unwrap();
        let Some(Op::Transition { also, detail, .. }) = p.ops.first() else {
            panic!()
        };
        assert_eq!(also, &[(TicketKey::ClaimedBy, Yv::Null)]);
        assert_eq!(detail, "waiting on the redis cluster");

        assert_eq!(
            refusal(plan_park(
                &s,
                &facts(),
                &ParkArgs {
                    id: "t-9c41".into(),
                    why: "   ".into()
                },
                &m
            )),
            "why_is_required"
        );
        assert_eq!(
            refusal(plan_drop(
                &s,
                &facts(),
                &DropArgs {
                    id: "t-9c41".into(),
                    why: "\t".into()
                },
                &m
            )),
            "why_is_required"
        );
    }

    #[test]
    fn parking_a_ticket_that_was_never_claimed_is_the_typed_refusal() {
        assert_eq!(
            refusal(plan_park(
                &snap_with(State::Todo),
                &facts(),
                &ParkArgs {
                    id: "t-9c41".into(),
                    why: "x".into()
                },
                &Minter::new(&Default::default(), 7, 4),
            )),
            "illegal_transition"
        );
    }

    /// The claim scope is `rulesdoc::Scope`, so "in scope" has one definition: glob-versus-
    /// glob in both directions, and nothing when either side is empty.
    #[test]
    fn the_claim_scope_answers_both_directions_and_neither_when_empty() {
        let auth = vec!["src/auth/**".to_string()];
        let login = vec!["src/auth/login.ts".to_string()];
        let scope = |g: &[String]| Scope::of(g).unwrap();
        assert!(scope(&auth).touches(&login));
        assert!(scope(&login).touches(&auth));
        assert!(scope(&auth).touches(&auth));
        assert!(!scope(&auth).touches(&["src/billing/**".to_string()]));
        assert!(!scope(&auth).touches(&[]));
        assert!(!scope(&[]).touches(&auth));
    }
}
