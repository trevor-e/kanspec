//! THE close-out gate — interactive and `--json`, one typed value between them.
//!
//! Four cheap steps: verify merged (refuse otherwise; `--no-code --why` is the recorded
//! escape), leftover triage (spawn / drop-with-reason / actually-done — **no fourth
//! option**), the knowledge checkpoint, then log the transition and flag the proposal
//! settling if this was its last live ticket.
//!
//! `plan_done` takes a `Landed`, whose two constructors are both sealed in `scan.rs`, so a
//! `done` that never consulted git **does not compile**. And the proof is re-run FRESH here
//! rather than read out of `cache/gitstate.json`: a 60-second-old "merged" is a badge, not a
//! gate.
//!
//! Owner: **S5**.

use std::io::IsTerminal;

use chrono::SecondsFormat;
use serde::Serialize;

use crate::cli::DoneArgs;
use crate::ctx::{Ctx, OutMode};
use crate::error::{GateCode, KsError, Result};
use crate::fm::Yv;
use crate::git::ChangedPath;
use crate::ids::{DecisionId, Minter, ProposalId, QuirkId, TicketId};
use crate::keys::TicketKey;
use crate::logentry::LogEntry;
use crate::model::Snapshot;
use crate::out::join;
use crate::out::{glyph, Color, Line, Render, Style};
use crate::plan::{DoneFacts, EntityRef, Op, Plan};
use crate::scan::{self, Landed, NoCodeWaiver};
use crate::store::Store;
use crate::transitions::{self, State, Verb};
use crate::triage::{specs_matching, SpecCheck, Triage};
use crate::{derive, fix, fixes};

#[derive(Debug, Serialize)]
pub struct DoneReport {
    pub id: TicketId,
    pub title: String,
    pub state: State,
    /// how the gate was satisfied: the ladder's badge, or the recorded `--no-code` reason
    pub landed: String,
    pub method: Option<String>,
    pub sha: Option<String>,
    pub spawned: Vec<Spawned>,
    pub dropped_steps: Vec<String>,
    pub marked_done: Vec<usize>,
    pub quirks: Vec<QuirkId>,
    pub decisions: Vec<DecisionId>,
    pub spec_check: SpecCheck,
    /// the proposal whose last live ticket this was
    pub settling: Option<ProposalId>,
    /// `settling` as a human reads it
    #[serde(skip)]
    pub settling_label: Option<String>,
    /// the `code:` globs the branch actually landed inside — what the checkpoint is about
    pub spec_globs: Vec<String>,
    /// "parked 2 discovered tickets" — nothing captured along the way rots silently
    pub discovered: Vec<TicketId>,
    pub next: Vec<String>,
}

/// A followup minted by the leftover triage, with the step it came out of. Carrying the
/// title is what lets the transcript read `spawned t-c412 "test concurrent same-key
/// requests"` rather than making a human go and look the id up.
#[derive(Debug, Serialize)]
pub struct Spawned {
    pub id: TicketId,
    pub title: String,
    pub from_step: usize,
}

pub fn done(ctx: &Ctx, a: &DoneArgs) -> Result<DoneReport> {
    ctx.require_initialized()?;
    let id = TicketId::parse(&a.id)?;
    let snap = ctx.snapshot()?;
    let t = snap.ticket(&id)?;

    // Refuse an illegal move before spending a ladder run on it. The authoritative check
    // still happens inside the lock, against a fresh snapshot.
    transitions::require(&id, t.fm.state, Verb::Done)?;

    // ── step 1: verify merged ────────────────────────────────────────────────
    //
    // Every subprocess and network call in this handler happens between here and
    // `transact`, never inside it.
    let landed = if a.no_code {
        // Step 1 refuses before step 2 asks anything: a caller who cannot use the escape
        // should hear that, not be interviewed about leftover steps first. `plan_done`
        // repeats this check as the backstop every other caller (the server's POST handler,
        // a future verb) goes through — the refusal that matters is the one in the planner,
        // and the refusal that reads well is this one.
        if t.fm.state == State::Review {
            return Err(no_code_from_review(&id));
        }
        // `record` is the ONLY constructor, and it refuses an empty reason. The op it
        // pushes is discarded with this probe plan — `plan_done` records the DURABLE one
        // into the plan it is building (§2.15's S5 note). Minting it here is what turns an
        // unexplained escape into a refusal *before* the lock is taken.
        let mut probe = Plan::empty();
        Landed::NoCode(NoCodeWaiver::record(
            &mut probe,
            &id,
            a.why.as_deref().unwrap_or_default(),
            &ctx.actor,
            ctx.now,
        )?)
    } else {
        // FRESH: `proof_for_done` re-runs the whole ladder rather than trusting the cache.
        Landed::Proof(scan::proof_for_done(ctx, t)?)
    };

    // The branch's own changed paths, for the knowledge checkpoint. `scan::touched_paths`
    // rather than `Git::changed_paths` — the three-dot diff collapses to nothing the moment
    // a true merge puts the branch's commits on main, and that function is where the
    // trailer-based fallback lives.
    let main = ctx
        .git
        .resolve_main(&ctx.cfg.main)
        .unwrap_or_else(|_| ctx.cfg.main.clone());
    let touched: Vec<ChangedPath> = scan::touched_paths(&ctx.git, t, &main)
        .into_iter()
        .map(|path| ChangedPath {
            status: 'M',
            path,
            renamed_from: None,
        })
        .collect();

    // ── steps 2 + 3: leftover triage and the knowledge checkpoint ────────────
    //
    // One typed value, two front doors. A pipe is not a person: without a terminal the
    // gate takes flags only, so "nothing left" can never be inferred from silence.
    let triage = if interactive(ctx) {
        Triage::prompt(t, a, &touched, &snap)?
    } else {
        Triage::from_args(t, a, &touched, &snap)?
    };

    // What the checkpoint is actually about: the globs the branch's changes fell inside.
    let spec_globs: Vec<String> = specs_matching(&touched, &snap)
        .iter()
        .filter_map(|n| snap.specs.get(n))
        .flat_map(|sp| sp.fm.code.clone())
        .collect();

    let landed_line = describe(ctx, &landed, &main);
    let (method, sha) = match &landed {
        Landed::Proof(p) => (
            Some(p.method().to_string()),
            Some(p.sha().short().to_string()),
        ),
        Landed::NoCode(_) => (None, None),
    };

    let f = DoneFacts {
        base: ctx.facts(),
        landed,
        touched,
    };
    let committed = Store::open(ctx).transact(Some(Verb::Done), &ctx.invocation(), |s, m| {
        plan_done(s, &f, &triage, a, m)
    })?;
    // D-20. The knowledge checkpoint above mints quirks and decisions, so `done` is a verb
    // that mutates the sources `KANSPEC-ARCHITECTURE.md` is projected from — and it is the
    // LAST verb of the daily loop, so a landmine captured here would otherwise sit unseen
    // in the committed page until somebody happened to run `scan`. See
    // `project::regenerate` for why this is a second transaction rather than more ops.

    // ── step 4: what the close-out actually produced ─────────────────────────
    let snap = &committed.snapshot;
    let t = snap.ticket(&id)?;
    // `Plan::minted` preserves the order `plan_done` pushed them in, which is step order.
    let spawned: Vec<Spawned> = committed
        .minted_of(|e| match e {
            EntityRef::Ticket(id) => Some(id.clone()),
            _ => None,
        })
        .into_iter()
        .zip(triage.spawns())
        .map(|(id, (from_step, title))| Spawned {
            id,
            title: title.to_string(),
            from_step,
        })
        .collect();
    let quirks: Vec<QuirkId> = committed.minted_of(|e| match e {
        EntityRef::Quirk(q) => Some(q.clone()),
        _ => None,
    });
    let decisions: Vec<DecisionId> = committed.minted_of(|e| match e {
        EntityRef::Decision(d) => Some(d.clone()),
        _ => None,
    });

    // "Settling" is derived, never stored: the moment the last linked ticket goes terminal
    // the proposal surfaces as *close me*, so close-out is prompted, never remembered.
    let settling =
        t.fm.proposal
            .as_ref()
            .and_then(|p| snap.proposals.get(p))
            .filter(|p| derive::settling(snap, p))
            .map(|p| p.fm.id.clone());

    // DESIGN.md's mechanism for keeping rabbit-hole captures visible. Without this line a
    // discovered ticket is a field in a file nobody re-reads.
    let discovered: Vec<TicketId> = snap
        .tickets
        .values()
        .filter(|o| o.fm.discovered_in.as_ref() == Some(&id))
        .map(|o| o.fm.id.clone())
        .collect();

    let mut next: Vec<String> = Vec::new();
    if let Some(p) = &settling {
        next.push(format!("{} close {p}", ctx.invoked_as));
    }
    if let Some(first) = spawned.first() {
        next.push(format!("{} start {}", ctx.invoked_as, first.id));
    }
    for d in &decisions {
        // Invariant 8: an agent mints a PROPOSED decision and a human accepts it.
        next.push(format!("{} accept {d}", ctx.invoked_as));
    }
    if !discovered.is_empty() {
        next.push(format!("{} ls", ctx.invoked_as));
    }
    if next.is_empty() {
        next.push(format!("{} ready", ctx.invoked_as));
    }

    Ok(DoneReport {
        title: t.fm.title.clone(),
        state: t.fm.state,
        landed: landed_line,
        method,
        sha,
        spawned,
        dropped_steps: triage
            .dropped()
            .into_iter()
            .map(|(i, why)| format!("[{i}] {why}"))
            .collect(),
        marked_done: triage.actually_done(),
        quirks,
        decisions,
        spec_check: triage.spec.clone(),
        spec_globs,
        settling_label: settling.as_ref().map(|p| snap.label(p)),
        settling,
        discovered,
        next,
        id,
    })
}

/// PURE. Refuses a `NoCodeWaiver` from `review`, so `--no-code` provably cannot bypass the
/// gate on an already-shipped ticket.
pub fn plan_done(
    s: &Snapshot,
    f: &DoneFacts,
    t: &Triage,
    a: &DoneArgs,
    m: &Minter,
) -> Result<Plan> {
    let id = TicketId::parse(&a.id)?;
    let ticket = s.ticket(&id)?;
    let from = ticket.fm.state;
    transitions::require(&id, from, Verb::Done)?;

    let mut plan = Plan::empty();

    // ── the gate, as a type ──────────────────────────────────────────────────
    let landed_note = match &f.landed {
        crate::scan::Landed::Proof(p) => {
            format!(
                "in main {} via {}{}",
                p.sha().short(),
                p.method(),
                p.pr().map(|n| format!(" #{n}")).unwrap_or_default()
            )
        }
        crate::scan::Landed::NoCode(w) => {
            // `--no-code` is the chore/docs escape from `doing`. A ticket already in
            // `review` has a branch, a head SHA and a PR: closing THAT without git truth is
            // the exact bypass the gate exists to stop.
            if from == State::Review {
                return Err(no_code_from_review(&id));
            }
            // The DURABLE record — a prose line under the ticket's `## Log`, shaped so
            // `logentry::parse_log` skips it and `replay` is unaffected.
            NoCodeWaiver::record(&mut plan, &id, w.why(), &f.base.actor, w.at())?;
            format!("no-code: {}", w.why())
        }
    };

    // ── leftover triage: the followups this close-out mints ──────────────────
    let mut spawned: Vec<TicketId> = Vec::new();
    for (index, title) in t.spawns() {
        // `Minter` excludes the SNAPSHOT's ids, and a plan's own mints are not in it yet —
        // so two entities minted from the same subject in one plan would collide and
        // `Plan::validate` would refuse the whole close-out. Salting the subject with the
        // step index (and, below, the ordinal) makes the subjects distinct by construction.
        let new_id = m.ticket(&format!("{title}#{index}"))?;
        let genesis = LogEntry {
            at: f.base.at,
            state: State::Todo,
            actor: f.base.actor.label(),
            verb: Verb::New,
            note: Some(format!("followup of {id}")),
        };
        let contents = crate::cmd::ticket::scaffold(&crate::cmd::ticket::TicketScaffold {
            id: &new_id,
            title,
            // A followup carries its parent's spec and proposal: it is unfinished scope
            // from the same story, not a new one (DESIGN.md's two link types).
            spec: ticket.fm.spec.as_ref(),
            proposal: ticket.fm.proposal.as_ref(),
            deps: &[],
            followup_of: Some(&id),
            discovered_in: None,
            created: f.base.at,
            genesis: &genesis,
        });
        plan.push(Op::CreateEntity {
            entity: EntityRef::Ticket(new_id.clone()),
            contents,
        })
        .mint(EntityRef::Ticket(new_id.clone()));
        spawned.push(new_id);
    }

    // Only "actually done" ticks a box. A spawned or dropped step stays unchecked on
    // purpose: the ticket's own record must not claim work that moved elsewhere or died.
    let checks: Vec<(usize, bool)> = t.actually_done().into_iter().map(|i| (i, true)).collect();
    if !checks.is_empty() {
        plan.push(Op::MarkSteps {
            id: id.clone(),
            checks,
        });
    }

    // ── the knowledge checkpoint's captures ──────────────────────────────────
    // Both records are minted through the SAME scaffolds `quirk add` and `decide` use, so a
    // landmine or a decision captured at close-out cannot drift from a hand-captured one.
    // `status: active` and `source:` point home (provenance rule 3); the decision is
    // `proposed`, never `accepted` — invariant 8, with `HumanActor` enforcing the other half.
    for (n, q) in t.quirks.iter().enumerate() {
        let qid = m.quirk(&format!("{}#{n}", q.title))?;
        plan.push(Op::CreateEntity {
            entity: EntityRef::Quirk(qid.clone()),
            contents: crate::cmd::quirk::scaffold(&qid, &q.title, &q.paths, q.severity, Some(&id)),
        })
        .mint(EntityRef::Quirk(qid));
    }
    for (n, d) in t.decisions.iter().enumerate() {
        let did = m.decision(&format!("{}#{n}", d.title))?;
        let body = format!(
            "## Context\nRecorded while closing out {id} on {when}.\n\n## Decision\n{}\n\n\
             ## Consequences\n(unfilled — a human accepts this, and should say what it costs \
             first)\n",
            d.title,
            when = f.base.at.to_rfc3339_opts(SecondsFormat::Secs, true),
        );
        plan.push(Op::CreateEntity {
            entity: EntityRef::Decision(did.clone()),
            contents: crate::cmd::decision::scaffold(
                &did,
                &d.title,
                f.base.at,
                Some(id.as_str()),
                &d.scope,
                None,
                &body,
            ),
        })
        .mint(EntityRef::Decision(did));
    }

    // ── the transition, last: the authoritative file goes last (R-1) ─────────
    let mut also: Vec<(TicketKey, Yv)> = Vec::new();
    if let Some(why) = t.spec_unchanged() {
        // Recorded on the ticket and visible on the board: skippable, but every skip is an
        // auditable act.
        also.push((TicketKey::SpecUnchanged, Yv::s(why)));
    }

    let mut detail = landed_note;
    if !spawned.is_empty() {
        detail.push_str(&format!(" · spawned {}", join(&spawned, ", ")));
    }
    let dropped = t.dropped();
    if !dropped.is_empty() {
        detail.push_str(&format!(" · dropped {} step(s)", dropped.len()));
    }
    if let Some(why) = t.spec_unchanged() {
        detail.push_str(&format!(" · spec unchanged: {why}"));
    }

    plan.push(Op::Transition {
        id,
        verb: Verb::Done,
        actor: f.base.actor.clone(),
        at: f.base.at,
        detail,
        also,
    });
    Ok(plan)
}

/// The one wording for "the escape is not available here", raised in two places on purpose:
/// the handler says it first, so it arrives before the triage interview, and `plan_done`
/// says it again, so no caller can reach the transition without passing it.
fn no_code_from_review(id: &TicketId) -> KsError {
    KsError::gate(
        GateCode::NoCodeFromReview,
        format!("{id} was shipped for review — `--no-code` cannot close work that has a branch"),
        fixes![
            fix!("kanspec scan --explain {id}"),
            fix!("kanspec scan --confirm {id} --why \"...\""),
            fix!("kanspec drop {id} --why \"...\""),
        ],
    )
}

/// Interactive iff a person is actually there. `--json` is an agent asking for an answer,
/// and a piped stdin cannot consent to anything.
fn interactive(ctx: &Ctx) -> bool {
    matches!(ctx.out, OutMode::Human { .. }) && std::io::stdin().is_terminal()
}

/// The `✓ merged verified: …` line, prepared where the clock and the resolved main branch
/// are still in scope.
fn describe(ctx: &Ctx, landed: &Landed, main: &str) -> String {
    match landed {
        Landed::Proof(p) => format!(
            "merged verified: {} reachable from {main} (method: {}{} · checked {})",
            p.sha().short(),
            p.method(),
            p.pr().map(|n| format!(" #{n}")).unwrap_or_default(),
            crate::out::rel_time(p.checked_at(), ctx.now),
        ),
        Landed::NoCode(w) => format!(
            "no code to verify — recorded by {} at {}: {}",
            w.by(),
            w.at().to_rfc3339_opts(SecondsFormat::Secs, true),
            w.why()
        ),
    }
}

impl Render for DoneReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // DESIGN.md's close-out transcript.
        writeln!(
            w,
            "{} {}",
            crate::out::paint(&glyph::OK.to_string(), Color::Green, st.color),
            self.landed
        )?;

        let triaged = self.spawned.len() + self.dropped_steps.len() + self.marked_done.len();
        if triaged > 0 {
            writeln!(
                w,
                "Leftover triage — {triaged} unchecked step{}:",
                if triaged == 1 { "" } else { "s" }
            )?;
            for s in &self.spawned {
                writeln!(
                    w,
                    "  {} spawned {} \"{}\" (followup_of {})",
                    glyph::OK,
                    s.id,
                    s.title,
                    self.id
                )?;
            }
            for d in &self.dropped_steps {
                writeln!(w, "  {} dropped {d}", glyph::FAIL)?;
            }
            for i in &self.marked_done {
                writeln!(w, "  {} step {i} was actually done", glyph::OK)?;
            }
        }

        match &self.spec_check {
            SpecCheck::EditedOnBranch { specs } => writeln!(
                w,
                "Knowledge check — branch touched {} (spec: {}): spec edited on this branch {}",
                self.spec_globs.join(", "),
                join(specs, ", "),
                glyph::OK
            )?,
            SpecCheck::Unchanged { waivers } => writeln!(
                w,
                "Knowledge check — branch touched {}: spec unchanged, recorded on the \
                 ticket — {}",
                if self.spec_globs.is_empty() {
                    "no spec's code".to_string()
                } else {
                    self.spec_globs.join(", ")
                },
                join(waivers, " · ")
            )?,
            SpecCheck::NotApplicable => {}
        }
        for q in &self.quirks {
            writeln!(w, "  {} captured quirk {q}", glyph::OK)?;
        }
        for d in &self.decisions {
            writeln!(
                w,
                "  {} proposed decision {d} — a human accepts it",
                glyph::OK
            )?;
        }

        let settling = self
            .settling_label
            .as_deref()
            .or(self.settling.as_ref().map(ProposalId::as_str))
            .map(|p| format!(" · {p} is settling (last ticket landed)"))
            .unwrap_or_default();
        Line::state(self.state, format!("done{settling}"))
            .id(crate::ids::label(&self.id, &self.title))
            .fix(self.next.first().cloned().unwrap_or_default())
            .write(w, st)?;

        // The capture ledger: what this ticket parked along the way, so a rabbit hole is
        // visible at the moment its parent closes rather than months later.
        if !self.discovered.is_empty() {
            writeln!(
                w,
                "   {} parked {} discovered ticket{} along the way: {}",
                glyph::DISCOVERED,
                self.discovered.len(),
                if self.discovered.len() == 1 { "" } else { "s" },
                join(&self.discovered, ", ")
            )?;
        }
        for n in self.next.iter().skip(1) {
            writeln!(w, "   {} {n}", glyph::FIX)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ctx::Actor;
    use crate::model::{Step, Ticket, TicketFm};
    use crate::plan::Facts;
    use chrono::{DateTime, TimeZone, Utc};

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap()
    }

    fn args(extra: &[&str]) -> DoneArgs {
        use clap::Parser as _;
        let mut argv: Vec<&str> = vec!["kanspec", "done", "t-9c41"];
        argv.extend_from_slice(extra);
        match crate::cli::Cli::try_parse_from(argv).expect("argv").command {
            crate::cli::Command::Done(a) => a,
            other => panic!("{other:?}"),
        }
    }

    fn snap_with(state: State, steps: &[(bool, &str)]) -> Snapshot {
        let mut s = Snapshot::empty(Config::default(), at());
        let fm: TicketFm = serde_yaml_ng::from_str(&format!(
            "id: t-9c41\ntitle: Rate-limit login endpoint\nstate: {state}\nspec: auth\n\
             created: 2026-08-30T09:00:00Z\n"
        ))
        .unwrap();
        s.tickets.insert(
            fm.id.clone(),
            Ticket {
                fm,
                path: std::path::PathBuf::from(".kanspec/tickets/t-9c41.md"),
                body: String::new(),
                steps: steps
                    .iter()
                    .enumerate()
                    .map(|(i, (done, text))| Step {
                        index: i + 1,
                        done: *done,
                        text: (*text).to_string(),
                    })
                    .collect(),
                log: Vec::new(),
                mtime: std::time::SystemTime::UNIX_EPOCH,
            },
        );
        s
    }

    fn facts(landed: Landed) -> DoneFacts {
        DoneFacts {
            base: Facts {
                actor: Actor::Human {
                    name: "trevor".into(),
                },
                at: at(),
                invocation: "kanspec done t-9c41".into(),
            },
            landed,
            touched: Vec::new(),
        }
    }

    /// A `NoCodeWaiver` can only be made by `record`, which is exactly the point: this
    /// helper is the whole surface a test has, and it is the same one the handler uses.
    fn waiver(why: &str) -> Landed {
        let mut probe = Plan::empty();
        Landed::NoCode(
            NoCodeWaiver::record(
                &mut probe,
                &TicketId::parse("t-9c41").unwrap(),
                why,
                &Actor::Human {
                    name: "trevor".into(),
                },
                at(),
            )
            .expect("a reason was given"),
        )
    }

    fn triage(s: &Snapshot, t: &Ticket, a: &DoneArgs) -> Triage {
        Triage::from_args(t, a, &[], s).expect("a complete triage")
    }

    #[track_caller]
    fn refusal<T>(r: Result<T>) -> &'static str {
        match r {
            Ok(_) => panic!("expected a refusal"),
            Err(e) => e.code().unwrap_or(e.kind()),
        }
    }

    #[track_caller]
    fn plan(s: &Snapshot, f: &DoneFacts, t: &Triage, a: &DoneArgs) -> Result<Plan> {
        let taken = s.taken_ids();
        plan_done(s, f, t, a, &Minter::new(&taken, 20260831, 4))
    }

    /// Invariant 1's sharpest edge: `--no-code` is the chore/docs escape from `doing`, and
    /// it provably cannot close a ticket that was already shipped for review.
    #[test]
    fn no_code_cannot_close_a_shipped_ticket() {
        let s = snap_with(State::Review, &[]);
        let t = s.ticket(&TicketId::parse("t-9c41").unwrap()).unwrap();
        let a = args(&[
            "--no-code",
            "--why",
            "docs only",
            "--no-followups",
            "--no-quirks",
        ]);
        let tri = triage(&s, t, &a);
        assert_eq!(
            refusal(plan(&s, &facts(waiver("docs only")), &tri, &a)),
            "no_code_from_review"
        );

        // …and from `doing`, where it belongs, it lands with a durable, signed record.
        let s = snap_with(State::Doing, &[]);
        let t = s.ticket(&TicketId::parse("t-9c41").unwrap()).unwrap();
        let tri = triage(&s, t, &a);
        let p = plan(&s, &facts(waiver("docs only")), &tri, &a).expect("the recorded escape");
        let waiver_line = p.ops.iter().find_map(|o| match o {
            Op::AppendSection { line, .. } => Some(line.clone()),
            _ => None,
        });
        let line = waiver_line.expect("the waiver is written to the ticket, not just claimed");
        assert!(line.contains("no-code waiver by trevor"), "{line}");
        assert!(line.contains("docs only"), "{line}");
        // …and it is PROSE: a second parseable log entry for one act would break the very
        // proof the log exists for.
        assert!(
            crate::logentry::LogEntry::parse(&line).is_none(),
            "the waiver line must not parse as a transition: {line}"
        );
    }

    #[test]
    fn an_empty_reason_is_not_a_waiver() {
        let mut probe = Plan::empty();
        assert!(NoCodeWaiver::record(
            &mut probe,
            &TicketId::parse("t-9c41").unwrap(),
            "   ",
            &Actor::Human {
                name: "trevor".into()
            },
            at(),
        )
        .is_err());
    }

    /// The triage's three outcomes, as `Op`s: a spawn mints a linked ticket, an
    /// actually-done ticks its box, and a drop does neither — it is recorded in the log.
    #[test]
    fn the_triage_becomes_exactly_the_ops_it_promised() {
        let s = snap_with(
            State::Doing,
            &[(false, "429"), (false, "tests"), (false, "docs")],
        );
        let t = s.ticket(&TicketId::parse("t-9c41").unwrap()).unwrap();
        let a = args(&[
            "--spawn",
            "429 + Retry-After",
            "--drop-step",
            "2:covered by the integration suite",
            "--actually-done",
            "3",
            "--no-quirks",
        ]);
        let tri = triage(&s, t, &a);
        let p = plan(&s, &facts(landed(State::Doing)), &tri, &a).expect("a complete close-out");

        let created: Vec<&EntityRef> = p
            .ops
            .iter()
            .filter_map(|o| match o {
                Op::CreateEntity { entity, .. } => Some(entity),
                _ => None,
            })
            .collect();
        assert_eq!(created.len(), 1, "one spawn, one ticket");
        let contents = p
            .ops
            .iter()
            .find_map(|o| match o {
                Op::CreateEntity { contents, .. } => Some(contents.clone()),
                _ => None,
            })
            .unwrap();
        let doc = crate::fm::split(&contents).unwrap();
        let fm: TicketFm = serde_yaml_ng::from_str(&doc.fm).unwrap();
        assert_eq!(fm.followup_of.as_ref().map(|i| i.as_str()), Some("t-9c41"));
        assert_eq!(fm.spec.as_ref().map(|s| s.as_str()), Some("auth"));
        assert!(fm.discovered_in.is_none(), "a followup is not a discovery");

        // Only the actually-done step ticks; the spawned and dropped ones stay unchecked,
        // because the ticket must not claim work that moved elsewhere or died.
        let checks = p.ops.iter().find_map(|o| match o {
            Op::MarkSteps { checks, .. } => Some(checks.clone()),
            _ => None,
        });
        assert_eq!(checks, Some(vec![(3, true)]));

        // The transition is LAST, and its note carries what happened.
        match p.ops.last() {
            Some(Op::Transition { verb, detail, .. }) => {
                assert_eq!(*verb, Verb::Done);
                assert!(detail.contains("spawned t-"), "{detail}");
                assert!(detail.contains("dropped 1 step"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// `MergedProof` is sealed: `scan.rs` mints one only from a real ladder run against a
    /// real `Ctx`, which is exactly the property that makes `done` uncheatable — and exactly
    /// why the PROOF path is proved end to end in `tests/lifecycle.rs` against a real squash
    /// merge rather than here. The ops below are identical for both `Landed` variants, so
    /// the recorded escape is the honest way to exercise them without a repository.
    fn landed(state: State) -> Landed {
        let _ = state;
        waiver("docs only")
    }

    #[test]
    fn the_spec_waiver_lands_on_the_ticket_where_the_board_can_see_it() {
        let s = snap_with(State::Doing, &[]);
        let t = s.ticket(&TicketId::parse("t-9c41").unwrap()).unwrap();
        let a = args(&[
            "--no-followups",
            "--no-quirks",
            "--spec-unchanged",
            "refactor only",
        ]);
        let tri = triage(&s, t, &a);
        let p = plan(&s, &facts(landed(State::Doing)), &tri, &a).unwrap();
        match p.ops.last() {
            Some(Op::Transition { also, detail, .. }) => {
                assert_eq!(
                    also,
                    &[(TicketKey::SpecUnchanged, Yv::s("refactor only"))],
                    "the skip is recorded, so it is auditable"
                );
                assert!(detail.contains("spec unchanged: refactor only"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_captured_quirk_is_born_active_and_points_home() {
        let s = snap_with(State::Doing, &[]);
        let t = s.ticket(&TicketId::parse("t-9c41").unwrap()).unwrap();
        let a = args(&[
            "--no-followups",
            "--quirk",
            "redis flushes on redeploy",
            "--quirk-paths",
            "src/auth/**",
        ]);
        let tri = triage(&s, t, &a);
        let p = plan(&s, &facts(landed(State::Doing)), &tri, &a).unwrap();
        let contents = p
            .ops
            .iter()
            .find_map(|o| match o {
                Op::CreateEntity {
                    entity: EntityRef::Quirk(_),
                    contents,
                } => Some(contents.clone()),
                _ => None,
            })
            .expect("the quirk was captured, not just prompted for");
        let doc = crate::fm::split(&contents).unwrap();
        let fm: crate::model::QuirkFm = serde_yaml_ng::from_str(&doc.fm).unwrap();
        assert_eq!(fm.status, crate::model::QuirkStatus::Active);
        assert_eq!(fm.source.as_ref().map(|s| s.as_str()), Some("t-9c41"));
        assert_eq!(fm.paths, ["src/auth/**"]);
    }

    /// Invariant 8, at the moment of capture: `done` mints a PROPOSED decision, never an
    /// accepted one, whoever is running it.
    #[test]
    fn a_decision_made_mid_ticket_is_proposed_never_accepted() {
        let s = snap_with(State::Doing, &[]);
        let t = s.ticket(&TicketId::parse("t-9c41").unwrap()).unwrap();
        let a = args(&[
            "--no-followups",
            "--no-quirks",
            "--decision",
            "Rate-limit state lives in Redis only",
        ]);
        let tri = triage(&s, t, &a);
        let p = plan(&s, &facts(landed(State::Doing)), &tri, &a).unwrap();
        let contents = p
            .ops
            .iter()
            .find_map(|o| match o {
                Op::CreateEntity {
                    entity: EntityRef::Decision(_),
                    contents,
                } => Some(contents.clone()),
                _ => None,
            })
            .expect("the decision was recorded");
        let doc = crate::fm::split(&contents).unwrap();
        let fm: crate::model::DecisionFm = serde_yaml_ng::from_str(&doc.fm).unwrap();
        assert_eq!(fm.status, crate::model::DecisionStatus::Proposed);
        assert_eq!(fm.source.as_deref(), Some("t-9c41"));
    }

    #[test]
    fn closing_a_todo_ticket_is_the_typed_refusal_not_a_gate() {
        let s = snap_with(State::Todo, &[]);
        let t = s.ticket(&TicketId::parse("t-9c41").unwrap()).unwrap();
        let a = args(&["--no-followups", "--no-quirks"]);
        let tri = triage(&s, t, &a);
        assert_eq!(
            refusal(plan(&s, &facts(landed(State::Doing)), &tri, &a)),
            "illegal_transition"
        );
    }
}
