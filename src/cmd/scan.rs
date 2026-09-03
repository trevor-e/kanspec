//! `scan [--explain] [--confirm <id> --why "…"] [--quiet] [--no-fetch]`.
//!
//! Results go to **the cache only**. The one exception is `--confirm`, which appends an
//! attributed `Verb::Confirm` line to the TICKET'S `## Log` rather than a cache entry: a
//! human attestation is an asserted act with an actor, so it must survive `rm -rf cache/`
//! and be visibly signed (D-11).
//!
//! The shape of this handler is the shape §2.16 requires of every mutating verb: every
//! subprocess and every network call happens in `scan::scan_all_detailed`, **outside** the
//! lock, and the `transact` that follows is a single pure `Op::WriteGitState`. A `gh` that
//! never answers must not hold the advisory lock while the browser and every other verb
//! wait for it.
//!
//! Owner: **S3**.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cache::MergeStatus;
use crate::cli::ScanArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::git::RungTrace;
use crate::ids::TicketId;
use crate::out::{glyph, Line, Render, Style};
use crate::plan::{Op, Plan};
use crate::scan::{self, ConfirmFacts, ScanOpts};
use crate::store::Store;
use crate::transitions::{State, Verb};

#[derive(Debug, Serialize)]
pub struct ScanReport {
    pub scanned: usize,
    pub landed: Vec<ScanRow>,
    pub unknown: Vec<ScanRow>,
    pub fetch_age_secs: Option<u64>,
    pub checked_at: DateTime<Utc>,
    /// present when `--confirm` recorded an attestation
    pub confirmed: Option<TicketId>,
    pub next: Vec<String>,
    /// `--quiet` prints nothing on success — the post-merge / post-checkout hooks run this
    /// on every checkout, and a hook that chatters gets uninstalled. `--json` still prints:
    /// silence is a *human* affordance, and an agent asking for JSON asked for an answer.
    #[serde(skip)]
    quiet: bool,
}

#[derive(Debug, Serialize)]
pub struct ScanRow {
    pub id: TicketId,
    pub title: String,
    pub badge: String,
    pub method: Option<String>,
    pub sha: Option<String>,
    pub pr: Option<u64>,
    /// the one command that moves this ticket forward (invariant 9)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// populated only with `--explain`, and always from the ladder run that produced this
    /// verdict — there is no second run that could disagree
    pub trace: Vec<RungTrace>,
}

pub fn scan(ctx: &Ctx, a: &ScanArgs) -> Result<ScanReport> {
    ctx.require_initialized()?;
    if let Some(raw) = a.confirm.as_deref() {
        return confirm(ctx, a, raw);
    }

    let snap = ctx.snapshot()?;
    let only = match a.id.as_deref() {
        Some(raw) => {
            let id = TicketId::parse(raw)?;
            // Fail on a bad id BEFORE spending a fetch and four subprocesses per ticket.
            snap.ticket(&id)?;
            Some(id)
        }
        None => None,
    };

    // THE LADDER RUNS HERE — outside the lock, where the network belongs.
    let (state, token, detections) = scan::scan_all_detailed(
        ctx,
        &snap,
        ScanOpts {
            fetch: !a.no_fetch,
            only,
        },
    )?;

    let mut report = ScanReport {
        scanned: detections.len(),
        landed: Vec::new(),
        unknown: Vec::new(),
        fetch_age_secs: state.fetch_age_secs,
        checked_at: ctx.now,
        confirmed: None,
        next: Vec::new(),
        quiet: a.quiet,
    };
    for (id, detection) in &detections {
        let Some(fact) = state.tickets.get(id) else {
            continue;
        };
        let t = snap.ticket(id)?;
        let row = ScanRow {
            id: id.clone(),
            title: t.fm.title.clone(),
            badge: crate::derive::Badge::from_fact(fact, t.fm.head.as_deref()).text(ctx.now),
            method: (fact.method != crate::git::Method::None).then(|| fact.method.to_string()),
            sha: fact.sha.as_ref().map(|s| s[..7.min(s.len())].to_string()),
            pr: fact.pr,
            fix: next_verb(t.fm.state, fact.status, id),
            trace: if a.explain {
                detection.explain().to_vec()
            } else {
                Vec::new()
            },
        };
        match fact.status {
            MergeStatus::Merged => report.landed.push(row),
            MergeStatus::Unknown => report.unknown.push(row),
        }
    }
    report.next = report
        .landed
        .iter()
        .chain(report.unknown.iter())
        .filter_map(|r| r.fix.clone())
        .take(5)
        .collect();

    // A SHORT transaction, holding the lock only for the write itself. `ScanToken` is what
    // makes "gitstate.json is written by scan and nothing else" a type fact.
    Store::open(ctx).transact(
        // A scan transitions nothing, and `transact` now says so in the type — round C
        // granted the `Option<Verb>` request S3 filed here and S5 and S6 filed again.
        None,
        &ctx.invocation(),
        move |_s, _m| Ok(Plan::of(vec![Op::WriteGitState { token, state }])),
    )?;

    // D-20, wired by S6 (the one line S6 writes in this file). The committed
    // `KANSPEC-FEATURES.md` / `KANSPEC-ARCHITECTURE.md` projections are rewritten from the
    // state this scan just produced — a *second* transaction, deliberately, because a
    // planner only ever sees the snapshot as it was BEFORE its own plan (D-34), so a
    // spec created or edited by this very transaction would be missing from a map
    // regenerated inside that closure — one scan out of date, for ever.
    //
    // (This comment used to justify the second transaction by the `Fresh?` column being
    // "computed from the very `GitState` the plan above wrote". That column is GONE: it
    // was computed from the gitignored cache, so the same commit rendered different bytes
    // on different machines. The D-34 reason above is the one that still holds.)
    // See `project::regenerate`.
    Ok(report)
}

/// `--confirm` — the recorded human override for the genuinely ambiguous case.
///
/// It writes to the ticket, not to the cache: the next `scan` re-reads the attestation out
/// of the `## Log` and projects it into a fresh cache, which is what makes the override
/// survive `rm -rf .kanspec/cache` instead of having to be repeated.
fn confirm(ctx: &Ctx, a: &ScanArgs, raw: &str) -> Result<ScanReport> {
    let id = TicketId::parse(raw)?;
    let snap = ctx.snapshot()?;
    let t = snap.ticket(&id)?;

    // Every git call happens BEFORE the lock (§2.16). The SHA the human is attesting to is
    // resolved through git so the log line names a commit that provably exists.
    let sha = crate::derive::ticket_rev(t).and_then(|(rev, _)| ctx.git.head_sha(&rev).ok());
    let f = ConfirmFacts {
        sha: sha.map(|h| h.sha().clone()),
        actor: ctx.actor.clone(),
        at: ctx.now,
        // clap's `--confirm` `requires = "why"`, so the `unwrap_or_default` below can only
        // be reached by a caller that bypassed argv parsing — and `plan_confirm` refuses an
        // empty reason anyway.
        why: a.why.clone().unwrap_or_default(),
        invocation: ctx.invocation(),
    };
    // A genuine ticket verb act, unlike the cache write above: `plan_confirm` pushes
    // `Op::Transition { verb: Confirm }` and the attestation lands in the `## Log` (D-11).
    let done = Store::open(ctx).transact(Some(Verb::Confirm), &ctx.invocation(), |s, _m| {
        scan::plan_confirm(s, &f, &id)
    })?;

    // Read the attestation straight back out of the ticket we just wrote: if it does not
    // survive the round trip through the `## Log`, it did not survive at all.
    let t = done.snapshot.ticket(&id)?;
    let proof = scan::confirmed_proof(&ctx.git, t);
    let fix = next_verb(t.fm.state, MergeStatus::Merged, &id);
    Ok(ScanReport {
        scanned: 1,
        landed: vec![ScanRow {
            id: id.clone(),
            title: t.fm.title.clone(),
            badge: match &proof {
                Some(p) => format!("in main (confirmed · {})", p.sha().short()),
                None => "in main (confirmed)".to_string(),
            },
            method: Some(crate::git::Method::HumanConfirm.to_string()),
            sha: proof.as_ref().map(|p| p.sha().short().to_string()),
            pr: t.fm.pr,
            fix: fix.clone(),
            trace: Vec::new(),
        }],
        unknown: Vec::new(),
        fetch_age_secs: None,
        checked_at: ctx.now,
        confirmed: Some(id.clone()),
        next: fix.into_iter().collect(),
        quiet: a.quiet,
    })
}

/// The one command that moves this ticket forward — invariant 9, on a per-row basis.
fn next_verb(state: State, status: MergeStatus, id: &TicketId) -> Option<String> {
    match (status, state) {
        (MergeStatus::Merged, State::Review) => Some(format!("kanspec done {id}")),
        (MergeStatus::Merged, State::Doing) => Some(format!("kanspec ship {id}")),
        (MergeStatus::Unknown, State::Review) => Some(format!("kanspec scan --explain {id}")),
        _ => None,
    }
}

impl Render for ScanReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // `--quiet`: the git hooks run this on every checkout and merge.
        if self.quiet {
            return Ok(());
        }
        if let Some(id) = &self.confirmed {
            for r in &self.landed {
                Line::new(glyph::OK, format!("recorded: {}", r.badge))
                    .id(id)
                    .fix(
                        r.fix
                            .clone()
                            .unwrap_or_else(|| format!("kanspec show {id}")),
                    )
                    .write(w, st)?;
            }
            writeln!(
                w,
                "   the attestation is on the ticket's ## Log, so it survives a cache wipe"
            )?;
            return Ok(());
        }

        for (g, rows) in [(glyph::IN_MAIN, &self.landed), ('?', &self.unknown)] {
            for r in rows {
                // The badge is the one merge-state vocabulary (`Badge::text`); what landed
                // rides beside it, dim, so a row still names the commit.
                let mut line = Line::new(g, r.badge.clone()).id(&r.id);
                if let Some(sha) = &r.sha {
                    line = line.dim(sha.clone());
                }
                if let Some(f) = &r.fix {
                    line = line.fix(f.clone());
                }
                line.write(w, st)?;
                // `--explain`: the ladder's reasoning, rung by rung, in the order it ran.
                for t in &r.trace {
                    writeln!(
                        w,
                        "    {:<10} {:<52} exit {:<4} {} [{}]",
                        t.method.as_str(),
                        t.cmd,
                        t.exit,
                        t.saw,
                        t.verdict
                    )?;
                }
            }
        }

        let age = match self.fetch_age_secs {
            Some(s) => format!(
                " · fetched {}",
                crate::out::rel_time(
                    self.checked_at - chrono::Duration::seconds(s as i64),
                    self.checked_at,
                )
            ),
            None => String::new(),
        };
        writeln!(
            w,
            " {} scanned · {} in main · {} unknown{age}",
            self.scanned,
            self.landed.len(),
            self.unknown.len(),
        )
    }
}
