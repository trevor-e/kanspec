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

use crate::cache::{MergeFact, MergeStatus};
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
    pub not_landed: Vec<ScanRow>,
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
            quiet: a.quiet,
        },
    )?;

    let mut report = ScanReport {
        scanned: detections.len(),
        landed: Vec::new(),
        not_landed: Vec::new(),
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
            badge: badge(fact),
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
            MergeStatus::NotMerged => report.not_landed.push(row),
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
    //
    // NOTE (D-20, reported): the `KANSPEC-FEATURES.md` / `KANSPEC-ARCHITECTURE.md`
    // regeneration belongs in this same plan — `project::plan_regenerate` is S6's and is
    // still `todo!()`, so wiring it now would panic every `scan`. It is two pushed `Op`s
    // when S6 lands.
    Store::open(ctx).transact(
        // `Verb` is a TICKET transition verb and a scan transitions nothing; it reaches
        // only the `sync = "commit"` message. `Confirm` is the table's own "non-transition,
        // recorded" verb, which is the closest honest fit. (Request to F: `transact` wants
        // an `Option<Verb>`, or a `Verb::Scan` that the transition table never accepts.)
        Verb::Confirm,
        &ctx.invocation(),
        move |_s, _m| Ok(Plan::of(vec![Op::WriteGitState { token, state }])),
    )?;
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
    let sha = ticket_rev(t).and_then(|rev| ctx.git.head_sha(&rev).ok());
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
    let done = Store::open(ctx).transact(Verb::Confirm, &ctx.invocation(), |s, _m| {
        scan::plan_confirm(s, &f, &id)
    })?;

    // Read the attestation straight back out of the ticket we just wrote: if it does not
    // survive the round trip through the `## Log`, it did not survive at all.
    let t = done.snapshot.ticket(&id)?;
    let proof = scan::confirmed_proof(&ctx.git, t);
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
            fix: next_verb(t.fm.state, MergeStatus::Merged, &id),
            trace: Vec::new(),
        }],
        not_landed: Vec::new(),
        unknown: Vec::new(),
        fetch_age_secs: None,
        checked_at: ctx.now,
        confirmed: Some(id.clone()),
        next: next_verb(t.fm.state, MergeStatus::Merged, &id)
            .into_iter()
            .collect(),
        quiet: a.quiet,
    })
}

/// `head:` if recorded, else the branch — DESIGN.md's "head-or-tip", the same rev the
/// ladder reasons about.
fn ticket_rev(t: &crate::model::Ticket) -> Option<String> {
    t.fm.head
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "null")
        .or_else(|| {
            t.fm.branch
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .map(str::to_string)
}

/// `"in main (gh-pr #142 · a1b9c3d)"` · `"unknown (squash suspected, no gh)"`.
fn badge(f: &MergeFact) -> String {
    match f.status {
        MergeStatus::Merged => {
            let pr = f.pr.map(|n| format!(" #{n}")).unwrap_or_default();
            let sha = f
                .sha
                .as_ref()
                .map(|s| format!(" · {}", &s[..7.min(s.len())]))
                .unwrap_or_default();
            format!("in main ({}{pr}{sha})", f.method)
        }
        MergeStatus::NotMerged => "not in main".to_string(),
        // `Unknown::badge()`, carried through the cache so a card explains itself without
        // re-running anything.
        MergeStatus::Unknown => f
            .why
            .clone()
            .unwrap_or_else(|| "unknown (no reason recorded)".to_string()),
    }
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

        for (g, rows) in [
            (glyph::IN_MAIN, &self.landed),
            ('?', &self.unknown),
            ('·', &self.not_landed),
        ] {
            for r in rows {
                let mut line = Line::new(g, r.badge.clone()).id(&r.id);
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
            " {} scanned · {} in main · {} unknown · {} not in main{age}",
            self.scanned,
            self.landed.len(),
            self.unknown.len(),
            self.not_landed.len(),
        )
    }
}
