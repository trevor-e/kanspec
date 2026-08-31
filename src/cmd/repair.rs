//! `repair <id> --why "…"` — the recorded log reset (D-12).
//!
//! Without it, replay-on-commit turns any imported or already-broken repo into a
//! permanently unwritable one. `Verb::Repair` is the ONE verb whose logged state is
//! authoritative in `transitions::replay`, which is precisely why it demands a human
//! attestation and records the actor.
//!
//! R-2, stated plainly: the seals bind the *tool*, the Log binds the *human*. `repair` is
//! where the human answers for a Log that no longer adds up — it never lets anyone *choose*
//! a state, it records that a person vouched for the one already in the file.
//!
//! Owner: **S3**.

use serde::Serialize;

use crate::cli::RepairArgs;
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::ids::{Minter, TicketId};
use crate::logentry::LogEntry;
use crate::model::Snapshot;
use crate::out::{glyph, Line, Render, Style};
use crate::plan::{Facts, Op, Plan};
use crate::store::Store;
use crate::transitions::{self, State, Verb};
use crate::{fix, fixes};

#[derive(Debug, Serialize)]
pub struct RepairReport {
    pub id: TicketId,
    pub title: String,
    /// what the log replayed to before the repair, if anything
    pub was: Option<String>,
    pub state: State,
    pub why: String,
    pub by: String,
    pub next: Vec<String>,
}

pub fn repair(ctx: &Ctx, a: &RepairArgs) -> Result<RepairReport> {
    ctx.require_initialized()?;
    let id = TicketId::parse(&a.id)?;
    let snap = ctx.snapshot()?;
    let t = snap.ticket(&id)?;
    let title = t.fm.title.clone();
    // What the log said BEFORE the attestation — read here, because after the transaction
    // it necessarily replays clean.
    let was = transitions::replay(&t.log).ok().map(|s| s.to_string());

    let f = Facts {
        actor: ctx.actor.clone(),
        at: ctx.now,
        invocation: ctx.invocation(),
    };
    let done = Store::open(ctx).transact(Verb::Repair, &ctx.invocation(), |s, m| {
        plan_repair(s, &f, a, m)
    })?;

    let t = done.snapshot.ticket(&id)?;
    Ok(RepairReport {
        state: t.fm.state,
        title,
        was,
        why: a.why.trim().to_string(),
        by: ctx.actor.label(),
        next: vec![format!("kanspec log {id}"), "kanspec doctor".to_string()],
        id,
    })
}

/// PURE. The attested state is the ticket's CURRENT frontmatter state — repair records
/// that a human vouched for it, it does not let anyone choose a new one.
pub fn plan_repair(s: &Snapshot, f: &Facts, a: &RepairArgs, _m: &Minter) -> Result<Plan> {
    let id = TicketId::parse(&a.id)?;
    let t = s.ticket(&id)?;
    let why = a.why.trim();
    if why.is_empty() {
        return Err(KsError::gate(
            "repair_without_why",
            format!("`{id}` cannot be repaired without a recorded reason"),
            fixes![
                fix!("kanspec repair {id} --why \"imported from the old tracker\""),
                fix!("kanspec log {id}"),
            ],
        ));
    }

    // `repair` is the escape hatch for a log that does not add up, not a rubber stamp.
    // Attesting to a ticket that already replays cleanly would make its logged state
    // authoritative from then on — weakening the very chain `prove` walks — for no gain.
    if transitions::prove(t).is_ok() {
        return Err(KsError::gate(
            "nothing_to_repair",
            format!(
                "{id}'s ## Log already replays to {} — nothing to attest",
                t.fm.state
            ),
            fixes![fix!("kanspec log {id}"), fix!("kanspec show {id}")],
        ));
    }

    // Simulate the exact line `Store::transact` is about to write, and replay the whole
    // log with it. `Verb::Repair` is authoritative for its OWN entry, so it recovers an
    // empty log and a hand-edited `state:` — but `replay` fails at the FIRST illegal entry
    // and never reaches a later reset, so a log with an illegal step in the middle is
    // beyond this verb. Saying so here beats letting `transact` refuse the write from
    // three layers down with a message about a plan the user never wrote.
    let entry = LogEntry {
        at: f.at,
        state: t.fm.state,
        actor: f.actor.label(),
        verb: Verb::Repair,
        note: Some(detail(t.fm.state, why)),
    };
    let mut replayed = t.log.clone();
    replayed.push(entry);
    if let Err(v) = transitions::replay(&replayed) {
        return Err(KsError::gate(
            "repair_cannot_reset",
            format!("{id}: repair cannot reset this ## Log — {v}"),
            fixes![
                fix!("kanspec log {id}"),
                fix!(
                    "open {} and delete the ## Log lines before the break",
                    t.path.display()
                ),
                fix!("kanspec doctor"),
            ],
        ));
    }

    Ok(Plan::of(vec![Op::Transition {
        id,
        verb: Verb::Repair,
        actor: f.actor.clone(),
        at: f.at,
        detail: detail(t.fm.state, why),
        also: Vec::new(),
    }]))
}

/// The `## Log` note: the state a human vouched for, and why. Read by a person and by
/// `doctor`, never by a planner — `replay` takes the state from the entry's own column.
fn detail(state: State, why: &str) -> String {
    format!("attested {state} — {why}")
}

impl Render for RepairReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        Line::new(
            glyph::OK,
            format!("## Log repaired — replays to {} again", self.state),
        )
        .id(&self.id)
        .fix(
            self.next
                .first()
                .cloned()
                .unwrap_or_else(|| format!("kanspec log {}", self.id)),
        )
        .write(w, st)?;
        match &self.was {
            Some(s) => writeln!(w, "   was: the log replayed to {s}")?,
            None => writeln!(w, "   was: the log did not replay at all")?,
        }
        writeln!(w, "   attested by {}: {}", self.by, self.why)?;
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
    use crate::model::Ticket;
    use chrono::{DateTime, TimeZone, Utc};

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 30, h, 0, 0).unwrap()
    }

    fn entry(h: u32, state: State, verb: Verb) -> LogEntry {
        LogEntry {
            at: at(h),
            state,
            actor: "trevor".into(),
            verb,
            note: None,
        }
    }

    /// A ticket whose frontmatter says `state` and whose `## Log` holds `log`.
    fn snap_with(state: State, log: Vec<LogEntry>) -> Snapshot {
        let mut s = Snapshot::empty(Config::default(), at(12));
        let fm: crate::model::TicketFm = serde_yaml_ng::from_str(&format!(
            "id: t-9c41\ntitle: fixture\nstate: {state}\ncreated: 2026-08-30T09:00:00Z\n"
        ))
        .unwrap();
        s.tickets.insert(
            fm.id.clone(),
            Ticket {
                fm,
                path: std::path::PathBuf::from(".kanspec/tickets/t-9c41.md"),
                body: String::new(),
                steps: Vec::new(),
                log,
                mtime: std::time::SystemTime::UNIX_EPOCH,
            },
        );
        s
    }

    fn args(why: &str) -> RepairArgs {
        RepairArgs {
            id: "t-9c41".into(),
            why: why.into(),
        }
    }

    fn facts() -> Facts {
        Facts {
            actor: crate::ctx::Actor::Human {
                name: "trevor".into(),
            },
            at: at(12),
            invocation: "kanspec repair t-9c41".into(),
        }
    }

    #[track_caller]
    fn plan(s: &Snapshot, a: &RepairArgs) -> Result<Plan> {
        let taken = s.taken_ids();
        let m = Minter::new(&taken, 7, 4);
        plan_repair(s, &facts(), a, &m)
    }

    /// `Plan` is deliberately not `Debug` (it holds a `ScanToken`), so `unwrap_err` is
    /// unavailable on a planner's result.
    #[track_caller]
    fn refusal(r: Result<Plan>) -> &'static str {
        match r {
            Ok(p) => panic!("expected a refusal, got a plan with {} ops", p.ops.len()),
            Err(e) => e.code().unwrap_or(e.kind()),
        }
    }

    /// D-12's whole reason to exist: an imported ticket has no log at all, so `replay`
    /// cannot reach ANY state and the write path refuses every subsequent verb. Repair is
    /// the human attestation that unwedges it.
    #[test]
    fn an_imported_ticket_with_no_log_is_exactly_what_repair_recovers() {
        let s = snap_with(State::Review, vec![]);
        let p = plan(&s, &args("imported from the old tracker")).expect("repairable");
        let Some(Op::Transition {
            verb, at, detail, ..
        }) = p.ops.first()
        else {
            panic!("repair records one attributed non-transition");
        };
        assert_eq!(p.ops.len(), 1);
        assert_eq!(*verb, Verb::Repair);
        assert_eq!(*at, facts().at);
        // The attested state is the one already in the file — repair records, it does not
        // let anyone choose.
        assert_eq!(detail, "attested review — imported from the old tracker");
    }

    /// A hand-edited `state:` — the R-2 case. The log replays legally to `review`, the
    /// frontmatter claims `done`, and `prove` catches the divergence at the next verb.
    #[test]
    fn a_hand_edited_state_is_recoverable_by_attestation() {
        let s = snap_with(
            State::Done,
            vec![
                entry(9, State::Todo, Verb::New),
                entry(10, State::Doing, Verb::Start),
                entry(11, State::Review, Verb::Ship),
            ],
        );
        let p = plan(&s, &args("closed by hand during the migration")).expect("repairable");
        match p.ops.first() {
            Some(Op::Transition { detail, .. }) => {
                assert!(detail.starts_with("attested done"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
    }

    /// `replay` fails at the FIRST illegal entry and never reaches a later reset, so a log
    /// broken in the middle is beyond this verb. Saying so — naming the entry and the file
    /// — beats a refusal from three layers down inside `transact`.
    #[test]
    fn a_log_broken_in_the_middle_is_named_rather_than_silently_wedged() {
        let s = snap_with(
            State::Review,
            vec![
                entry(9, State::Todo, Verb::New),
                // `ship` is not legal from `todo`.
                entry(10, State::Review, Verb::Ship),
            ],
        );
        assert_eq!(
            refusal(plan(&s, &args("no idea how this happened"))),
            "repair_cannot_reset"
        );
    }

    /// A log entry dated after the repair would replay backwards in time.
    #[test]
    fn an_attestation_that_predates_the_last_entry_is_refused() {
        let s = snap_with(
            State::Review,
            vec![entry(23, State::Todo, Verb::New)], // replays to todo, not review
        );
        assert_eq!(
            refusal(plan(&s, &args("clock skew"))),
            "repair_cannot_reset"
        );
    }

    #[test]
    fn repair_demands_a_reason_and_refuses_a_healthy_ticket() {
        let broken = snap_with(State::Review, vec![]);
        assert_eq!(refusal(plan(&broken, &args("   "))), "repair_without_why");

        let healthy = snap_with(
            State::Review,
            vec![
                entry(9, State::Todo, Verb::New),
                entry(10, State::Doing, Verb::Start),
                entry(11, State::Review, Verb::Ship),
            ],
        );
        assert_eq!(
            refusal(plan(&healthy, &args("just in case"))),
            "nothing_to_repair"
        );
    }

    #[test]
    fn repairing_a_ticket_that_does_not_exist_is_a_not_found() {
        let s = Snapshot::empty(Config::default(), at(12));
        assert_eq!(refusal(plan(&s, &args("x"))), "not_found");
    }
}
