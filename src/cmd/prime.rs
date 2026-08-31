//! `prime` — the agent working set, injected by the SessionStart and PreCompact hooks
//! (~1.5k tokens).
//!
//! Two sections: (1) the standing rules, **byte-identical** to `kanspec rules`; (2) the
//! live slice — the claimed ticket, unresolved comment counts, the ready-queue top 5, and
//! the `status` anomaly lines. Path-scoped injection is the context-economy answer: an
//! agent in `src/auth/` never pays for the billing quirks.
//!
//! **`prime` refreshes merge state before it reads it.** DESIGN.md lists SessionStart among
//! `scan`'s triggers; wiring SessionStart to `prime` alone would hand every cold session a
//! merge state as old as the last time somebody happened to run a verb — the one fact this
//! tool exists to be right about. The refresh is *throttled* (skipped while the cache is
//! younger than `[windows] fetch_max_age_secs`) and *best effort* (a held lock or an
//! offline remote costs the session nothing), and every status line carries the cache's own
//! age either way, so a skipped refresh is visible rather than silent.
//!
//! Owner: **S6**.

use std::time::Duration;

use serde::Serialize;

use crate::cli::PrimeArgs;
use crate::ctx::Ctx;
use crate::derive::{self, Attention, Derived, Owner};
use crate::error::Result;
use crate::git::Tri;
use crate::ids::TicketId;
use crate::model::{Snapshot, Ticket};
use crate::out::{Render, Style};
use crate::plan::{Op, Plan};
use crate::rulesdoc::{RulesDoc, Scope};
use crate::scan::{self, ScanOpts};
use crate::store::Store;
use crate::transitions::{State, Verb};

/// How many ready tickets and anomaly lines the payload carries. The budget is ~1.5k
/// tokens for the WHOLE injection, and the standing rules are the half that earns its
/// keep, so the live slice is capped rather than allowed to grow with the backlog.
const READY_TOP: usize = 5;
const ANOMALY_TOP: usize = 8;

#[derive(Debug, Serialize)]
pub struct PrimeReport {
    /// `prime --json .standing == rules --json .data`
    pub standing: RulesDoc,
    pub live: LiveSlice,
    pub scope: Vec<String>,
    /// The rendered payload, produced by [`payload`] and written VERBATIM by `Render`.
    /// Carried on the report because `Render::human` has no `Ctx` to rebuild it from, and
    /// rebuilding it a second way is exactly the drift invariant 3 forbids.
    #[serde(skip)]
    text: String,
}

#[derive(Debug, Serialize)]
pub struct LiveSlice {
    pub claimed: Option<TicketId>,
    pub claimed_title: Option<String>,
    pub unresolved: usize,
    pub ready: Vec<TicketId>,
    pub anomalies: Vec<Attention>,
    /// the bounded failure excerpt, when the claimed branch has a fresh failed local job
    pub ci_digest: Option<String>,
}

pub fn prime(ctx: &Ctx, a: &PrimeArgs) -> Result<PrimeReport> {
    ctx.require_initialized()?;
    let mut snap = ctx.snapshot()?;

    // Every subprocess and every network call happens HERE, before anything is rendered
    // and before the short cache-write transaction (§2.16).
    if refresh(ctx, &snap) {
        snap = ctx.snapshot()?;
    }

    let scope = if a.paths.is_empty() {
        branch_scope(ctx)?
    } else {
        Scope::of(&a.paths)?
    };
    let dv = derive::compute(&snap);
    let standing = crate::rulesdoc::build(&snap, &scope);
    let text = payload(ctx, &snap, &dv, &scope);

    let claimed = claimed_ticket(ctx, &snap);
    Ok(PrimeReport {
        live: LiveSlice {
            claimed: claimed.map(|t| t.fm.id.clone()),
            claimed_title: claimed.map(|t| t.fm.title.clone()),
            unresolved: claimed
                .and_then(|t| t.fm.proposal.as_ref())
                .map(|p| derive::unresolved(&snap, p))
                .unwrap_or(0),
            ready: dv.ready.iter().take(READY_TOP).cloned().collect(),
            anomalies: dv.attention.iter().take(ANOMALY_TOP).cloned().collect(),
            // The CI provider layer is v0.2 (`ci.rs`, ARCHITECTURE §1); until it lands
            // there is no digest to ride in, and inventing one would be a guess.
            ci_digest: None,
        },
        standing,
        scope: scope.paths.clone(),
        text,
    })
}

/// The payload, produced in **exactly one place**. `rules` writes the first line of this
/// and stops; `prime` writes all of it. That is the whole of invariant 3.
pub fn payload(ctx: &Ctx, s: &Snapshot, dv: &Derived, scope: &Scope) -> String {
    let standing = crate::rulesdoc::render_text(&crate::rulesdoc::build(s, scope));
    format!("{standing}\n{}", live_slice(ctx, s, dv))
}

fn live_slice(ctx: &Ctx, s: &Snapshot, dv: &Derived) -> String {
    let mut o = String::new();
    o.push_str(&format!(
        "LIVE SLICE for {} (merge state {})\n",
        ctx.actor.label(),
        s.git.freshness(s.now),
    ));

    match claimed_ticket(ctx, s) {
        Some(t) => {
            let unresolved =
                t.fm.proposal
                    .as_ref()
                    .map(|p| derive::unresolved(s, p))
                    .unwrap_or(0);
            o.push_str(&format!(
                " CLAIMED  {}  {} · {}{}{}\n",
                t.fm.id,
                t.fm.title,
                t.fm.state,
                t.fm.spec
                    .as_ref()
                    .map(|sp| format!(" · spec {sp}"))
                    .unwrap_or_default(),
                if unresolved > 0 {
                    format!(" · {unresolved} unresolved review threads")
                } else {
                    String::new()
                },
            ));
            if let Some(b) = &t.fm.branch {
                o.push_str(&format!("          branch {b}\n"));
            }
        }
        None => o.push_str(" CLAIMED  (nothing) — claim before coding: kanspec start <id>\n"),
    }

    let ready: Vec<&TicketId> = dv.ready.iter().take(READY_TOP).collect();
    o.push_str(&format!(" READY ({} of {})\n", ready.len(), dv.ready.len()));
    for id in ready {
        let title = s
            .tickets
            .get(id)
            .map(|t| t.fm.title.as_str())
            .unwrap_or("(missing)");
        let spec = s
            .tickets
            .get(id)
            .and_then(|t| t.fm.spec.as_ref())
            .map(|sp| format!(" · {sp}"))
            .unwrap_or_default();
        o.push_str(&format!("  {id}  {title}{spec}\n"));
    }

    let anomalies: Vec<&Attention> = dv.attention.iter().take(ANOMALY_TOP).collect();
    o.push_str(&format!(
        " STATUS ({} of {})\n",
        anomalies.len(),
        dv.attention.len()
    ));
    for x in &anomalies {
        // Every anomaly line carries its one-command fix — that is what makes drift
        // self-healing rather than something an agent has to be told about (invariant 9).
        o.push_str(&format!(
            "  {} {}  {}{}\n",
            owner_tag(x.owner),
            x.subject,
            x.line,
            if x.fix.is_empty() {
                String::new()
            } else {
                format!("  → {}", x.fix)
            },
        ));
    }
    if dv.attention.is_empty() {
        o.push_str("  nothing owed — every open item has its next verb\n");
    }
    o
}

fn owner_tag(o: Owner) -> &'static str {
    match o {
        Owner::You => "[you]",
        Owner::Agent => "[me] ",
        Owner::Watching => "[   ]",
    }
}

/// Which ticket is this session working on? The branch is the strongest signal — it is
/// what `start` wrote and what the commit-msg hook keys off — and a claim by this actor is
/// the fallback for an agent that has not checked out yet.
fn claimed_ticket<'s>(ctx: &Ctx, s: &'s Snapshot) -> Option<&'s Ticket> {
    if let Some(branch) = ctx.git.current_branch() {
        if let Some(t) = s
            .tickets
            .values()
            .find(|t| t.fm.branch.as_deref() == Some(&branch))
        {
            return Some(t);
        }
    }
    let me = ctx.actor.label();
    s.tickets
        .values()
        .find(|t| t.fm.state == State::Doing && t.fm.claimed_by.as_deref() == Some(&me))
}

/// Scope to what the branch actually touched. On `main`, or when git declines to answer,
/// that is nothing — and an unscoped payload is the honest answer, not a broken one.
fn branch_scope(ctx: &Ctx) -> Result<Scope> {
    let Ok(main) = ctx.git.resolve_main(&ctx.cfg.main) else {
        return Ok(Scope::none());
    };
    match ctx.git.changed_paths(&main, "HEAD") {
        Tri::Yes(paths) => Scope::from_branch(&paths),
        _ => Ok(Scope::none()),
    }
}

/// The throttled refresh. Returns `true` when the cache actually moved, so the caller
/// knows to reload.
fn refresh(ctx: &Ctx, snap: &Snapshot) -> bool {
    let max = Duration::from_secs(ctx.cfg.windows.fetch_max_age_secs.max(1));
    if !snap.git.is_stale(ctx.now, max) {
        return false;
    }
    // BEST EFFORT, deliberately. `prime` runs from a SessionStart hook: a held lock, a
    // remote that is down or a repo mid-rebase must cost the agent its refresh, never its
    // session. The freshness stamp on every status line is what keeps the skip visible.
    let Ok((state, token)) = scan::scan_all(
        ctx,
        snap,
        ScanOpts {
            fetch: true,
            only: None,
            quiet: true,
        },
    ) else {
        return false;
    };
    Store::open(ctx)
        .transact(Verb::Confirm, &ctx.invocation(), move |_s, _m| {
            Ok(Plan::of(vec![Op::WriteGitState { token, state }]))
        })
        .is_ok()
}

impl Render for PrimeReport {
    fn human(&self, w: &mut dyn std::io::Write, _st: &Style) -> std::io::Result<()> {
        // VERBATIM. It MUST start with the exact bytes `kanspec rules` prints, and
        // `tests/invariants_rules.rs` diffs the two real binaries to prove it.
        write!(w, "{}", self.text)
    }
}
