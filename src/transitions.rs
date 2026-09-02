//! THE legality oracle — one definition, used by the write path and by `doctor`.
//!
//! A const table + exhaustive match, **not** typestate (D-1). Ticket state arrives from a
//! hand-editable FILE at runtime, so compile-time phases would be a lie requiring a
//! fallible downcast at every boundary. `from == None` means "does not exist yet", so
//! `New` lives in the same table and [`replay`] needs no genesis special case.
//!
//! R-2, stated plainly: the seals bind the *tool*; the Log binds the *human*.
//! `sed -i 's/state: review/state: done/'` still works — [`replay`] proves the state was
//! never legally reached, at the very next verb and in CI.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Fix, Fixes, KsError, Result};
use crate::ids::TicketId;
use crate::logentry::LogEntry;
use crate::model::Ticket;
use crate::{fix, fixes};

// ─────────────────────────────────────────────────────────────────────────────
// State / Verb
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Todo,
    Doing,
    Review,
    Done,
    Dropped,
}

impl State {
    /// `○ todo ◐ doing ◈ review ● done ✕ dropped` (the in-main overlay `⇂` is derived,
    /// never stored, so it is not a `State` and has no glyph here).
    pub const fn glyph(self) -> &'static str {
        match self {
            State::Todo => "○",
            State::Doing => "◐",
            State::Review => "◈",
            State::Done => "●",
            State::Dropped => "✕",
        }
    }
    /// The frontmatter text, exactly.
    pub const fn as_str(self) -> &'static str {
        match self {
            State::Todo => "todo",
            State::Doing => "doing",
            State::Review => "review",
            State::Done => "done",
            State::Dropped => "dropped",
        }
    }
    pub const fn terminal(self) -> bool {
        matches!(self, State::Done | State::Dropped)
    }
    pub fn parse(s: &str) -> Option<State> {
        ALL_STATES.iter().copied().find(|st| st.as_str() == s)
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verb {
    New,
    Start,
    Ship,
    Done,
    Park,
    Drop,
    Confirm,
    Repair,
}

impl Verb {
    pub const fn as_str(self) -> &'static str {
        match self {
            Verb::New => "new",
            Verb::Start => "start",
            Verb::Ship => "ship",
            Verb::Done => "done",
            Verb::Park => "park",
            Verb::Drop => "drop",
            Verb::Confirm => "confirm",
            Verb::Repair => "repair",
        }
    }
    pub fn parse(s: &str) -> Option<Verb> {
        ALL_VERBS.iter().copied().find(|v| v.as_str() == s)
    }

    /// The verb's REAL invocation, mandatory flags included — mint it here, never by
    /// lowercasing the name.
    ///
    /// [`Verb::as_str`] is the `## Log` spelling, and FIVE of the eight verbs are not
    /// invoked that way: `confirm` is an **option on `scan`**, not a subcommand;
    /// `park`/`drop`/`repair`/`scan --confirm` all refuse without `--why`; and `new` takes a
    /// title, not the id it has not minted yet. So the obvious
    /// `format!("kanspec {verb} {id}")` answered `kanspec start <a done ticket>` — the
    /// single most common mistake there is — with `kanspec confirm t-ea32`, which exits 64
    /// with `unrecognized subcommand`. Invariant 9 does not say "name a fix", it says name
    /// the *one command that fixes it*, so a fix that cannot be run is the invariant broken.
    ///
    /// The match is exhaustive on purpose: a ninth `Verb` cannot compile until someone
    /// writes down how it is actually invoked. `tests/transition_table.rs` then parses every
    /// string produced here against the real clap tree, so a flag that later becomes
    /// mandatory is a failing test rather than advice that fails in the user's terminal.
    pub fn command(self, id: &TicketId) -> String {
        self.command_as(crate::cli::invoked_as(), id)
    }

    /// [`Verb::command`] against an explicit binary name — `kanspec` or `ks`, whichever the
    /// user actually typed. Split out because [`require`]'s signature is the frozen one in
    /// ARCHITECTURE §2.4 and carries no `&Ctx`: `command` reads the same process-wide
    /// `invoked_as` that `Ctx` is built from, and this is the seam tests pin both spellings
    /// through without racing a `OnceLock`.
    pub fn command_as(self, ks: &str, id: &TicketId) -> String {
        match self {
            // `new` MINTS an id, so it takes the title it has instead of the id it lacks.
            Verb::New => format!("{ks} new \"...\""),
            Verb::Start => format!("{ks} start {id}"),
            Verb::Ship => format!("{ks} ship {id}"),
            Verb::Done => format!("{ks} done {id}"),
            // `--why` is a `String`, not an `Option<String>` — clap refuses without it.
            Verb::Park => format!("{ks} park {id} --why \"...\""),
            Verb::Drop => format!("{ks} drop {id} --why \"...\""),
            // NOT a subcommand: `--confirm` is an option on `scan`, and it `requires = "why"`.
            Verb::Confirm => format!("{ks} scan --confirm {id} --why \"...\""),
            Verb::Repair => format!("{ks} repair {id} --why \"...\""),
        }
    }
}

impl std::fmt::Display for Verb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub const ALL_STATES: &[State] = &[
    State::Todo,
    State::Doing,
    State::Review,
    State::Done,
    State::Dropped,
];
pub const ALL_VERBS: &[Verb] = &[
    Verb::New,
    Verb::Start,
    Verb::Ship,
    Verb::Done,
    Verb::Park,
    Verb::Drop,
    Verb::Confirm,
    Verb::Repair,
];

// ─────────────────────────────────────────────────────────────────────────────
// THE table
// ─────────────────────────────────────────────────────────────────────────────

/// `Confirm` and `Repair` ARE in the table: a `scan --confirm` that could not find a
/// legal successor would wedge the ticket it was meant to unblock.
pub const fn next(from: Option<State>, verb: Verb) -> Option<State> {
    use State::*;
    use Verb as V;
    match (from, verb) {
        (None, V::New) => Some(Todo),
        (Some(Todo), V::Start) => Some(Doing),
        (Some(Review), V::Start) => Some(Doing), // rework
        (Some(Doing), V::Ship) => Some(Review),
        (Some(Doing), V::Park) => Some(Todo),
        (Some(Review), V::Done) => Some(Done), // requires MergedProof
        (Some(Doing), V::Done) => Some(Done),  // requires NoCodeWaiver
        (Some(Todo), V::Drop) | (Some(Doing), V::Drop) | (Some(Review), V::Drop) => Some(Dropped),
        (Some(s), V::Confirm) => Some(s), // non-transition, recorded
        (Some(s), V::Repair) => Some(s),  // non-transition, recorded
        _ => None,
    }
}

/// `&'static [Verb]`, because `KsError::IllegalTransition` holds one without allocating.
pub const fn allowed_slice(from: Option<State>) -> &'static [Verb] {
    use State::*;
    use Verb as V;
    match from {
        None => &[V::New],
        Some(Todo) => &[V::Start, V::Drop, V::Confirm, V::Repair],
        Some(Doing) => &[V::Ship, V::Done, V::Park, V::Drop, V::Confirm, V::Repair],
        Some(Review) => &[V::Start, V::Done, V::Drop, V::Confirm, V::Repair],
        Some(Done) | Some(Dropped) => &[V::Confirm, V::Repair],
    }
}

pub fn allowed_from(from: Option<State>) -> Vec<Verb> {
    allowed_slice(from).to_vec()
}

/// The states a verb is legal FROM — the `{}` slot of `KsError::IllegalTransition`.
pub const fn states_for(verb: Verb) -> &'static [State] {
    use State::*;
    use Verb as V;
    match verb {
        V::New => &[],
        V::Start => &[Todo, Review],
        V::Ship => &[Doing],
        V::Done => &[Doing, Review],
        V::Park => &[Doing],
        V::Drop => &[Todo, Doing, Review],
        V::Confirm | V::Repair => ALL_STATES,
    }
}

/// "start, done, or drop" — an Oxford-comma list, because these land in user-facing prose.
pub fn verbs_str(vs: &[Verb]) -> String {
    join_or(&vs.iter().map(|v| v.as_str()).collect::<Vec<_>>())
}

/// "doing" · "doing or review" · "todo, doing, or review".
pub fn states_str(ss: &[State]) -> String {
    join_or(&ss.iter().map(|s| s.as_str()).collect::<Vec<_>>())
}

fn join_or(parts: &[&str]) -> String {
    match parts {
        [] => "nothing".to_string(),
        [a] => (*a).to_string(),
        [a, b] => format!("{a} or {b}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

/// The typed refusal every mutating verb goes through.
///
/// Both fix lines come from [`onward`], which builds them out of [`Verb::command_as`] and
/// `invoked_as` — never from the verb's name and never from a hardcoded `kanspec`: a `ks`
/// user is told to run `ks`, and the suggested verb is spelled the way clap accepts it.
pub fn require(id: &TicketId, from: State, verb: Verb) -> Result<State> {
    match next(Some(from), verb) {
        Some(to) => Ok(to),
        None => Err(KsError::IllegalTransition {
            id: id.clone(),
            from,
            verb,
            allowed: allowed_slice(Some(from)),
            fix: onward(id, from),
        }),
    }
}

/// What to do instead — and it has to be something that ENDS.
///
/// Invariant 9 asks for the one command that fixes this. For a ticket that can still move
/// that is the first verb the table allows, and those chains are short and true: `start` on
/// a `todo` claims it, `ship` on a `doing` reviews it. Nothing below changes them.
///
/// A **terminal** ticket is the case that was broken. `done` and `dropped` allow only
/// `confirm` and `repair`, and naming the first of those — `scan --confirm` — sent an agent
/// round a ring it could not get out of, because on a ticket that never had a branch
/// `--confirm` has nothing to attest and answers `ship`, and `ship` answers `scan --confirm`:
///
/// ```text
/// ✗ t-8e2b is dropped, not doing or review — cannot done
///   → ks scan --confirm t-8e2b --why "..."
/// ✗ `t-8e2b` records neither a `head:` SHA nor a resolvable branch to confirm
///   → kanspec ship t-8e2b
/// ✗ t-8e2b is dropped, not doing — cannot ship
///   → kanspec scan --confirm t-8e2b --why "..."      ← and round again
/// ```
///
/// A human reads the third line and stops. An agent whose whole contract is "run the
/// suggested command" does not, so a fix that is *runnable* is not yet a fix that is
/// *followable*: the chain has to terminate as well. Neither of the two verbs a terminal
/// state allows is worth naming here — `confirm` attests a merge for work that is over, and
/// `repair` refuses outright on a ticket whose `## Log` already replays (`nothing_to_repair`).
/// The honest advice for a closed ticket is the true, final one: look at what happened, or
/// open a NEW ticket for the work that still wants doing.
///
/// `tests/transition_table.rs` walks the whole suggestion graph with the real binary and
/// fails on a repeated command, so a future fix line that bounces is a failing test rather
/// than an agent spinning in someone's terminal.
fn onward(id: &TicketId, from: State) -> Fixes {
    let ks = crate::cli::invoked_as();
    if from.terminal() {
        // `New` mints an id rather than taking one, so `command_as` spells it with the
        // title it needs — the same single source of truth as every other suggestion.
        return fixes![
            fix!("{ks} show {id}"),
            Fix::cmd(Verb::New.command_as(ks, id)),
        ];
    }
    fixes![
        fix!("{ks} show {id}"),
        Fix::cmd(allowed_slice(Some(from))[0].command_as(ks, id)),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// The mechanical proof (invariant 10)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, thiserror::Error)]
#[serde(tag = "break", rename_all = "snake_case")]
pub enum LogViolation {
    #[error("the ## Log is empty — no state was ever reached")]
    Empty,
    #[error("the ## Log opens with `{first}`, not `new`")]
    NoGenesis { first: Verb },
    #[error("log entry {index}: `{verb}` is not legal from {}", from.map(|s| s.as_str()).unwrap_or("nothing"))]
    IllegalStep {
        index: usize,
        from: Option<State>,
        verb: Verb,
    },
    #[error("log entry {index} records `{logged}` where the table says `{legal}`")]
    StateMismatch {
        index: usize,
        logged: State,
        legal: State,
    },
    #[error("log entry {index} ({at}) goes backwards in time")]
    OutOfOrder { index: usize, at: DateTime<Utc> },
    #[error("frontmatter says `{frontmatter}` but the ## Log replays to `{replayed}`")]
    Divergence { replayed: State, frontmatter: State },
}

/// Folds the ticket's own `## Log` through the SAME oracle the write path uses. Checks
/// three things: legal sequence, each entry's RECORDED state against the legal successor
/// (a doctored log LINE, not just a doctored frontmatter field), and timestamp
/// monotonicity.
///
/// # Two passes, and why the split is load-bearing (D-12)
///
/// `Verb::Repair` is the one verb whose logged state is **authoritative** rather than
/// derived, so the legality fold begins at the LAST repair entry and everything before it
/// is what the attested reset is *for*. Folding strictly forward instead — returning at the
/// first bad entry, never reaching a later reset — left `repair` unable to rescue an
/// `IllegalStep` or a `StateMismatch`, which is exactly what an import and a union-merged
/// `## Log` (two `start` lines) produce. `Store::transact` re-proves the staged bytes with
/// no repair exemption, so such a ticket could not even have the repair written to it: D-12
/// says those repos must be recoverable rather than permanently unwritable.
///
/// The clock, by contrast, is checked over the **whole** log, before and after any reset. A
/// `## Log` is a chronological record, and a repair line must never be usable to launder a
/// temporal anomaly — so an out-of-order pair stays a refusal whatever is appended after
/// it, and its fix is the safe, mechanical one the message names: put the lines back in
/// order. Attesting a state is a human's to do; rewriting when things happened is not.
pub fn replay(entries: &[LogEntry]) -> std::result::Result<State, LogViolation> {
    // ── pass 1: the clock, over every entry — no repair exemption ────────────
    let mut last: Option<DateTime<Utc>> = None;
    for (i, e) in entries.iter().enumerate() {
        if last.is_some_and(|prev| e.at < prev) {
            return Err(LogViolation::OutOfOrder { index: i, at: e.at });
        }
        last = Some(e.at);
    }

    // ── pass 2: legality, from the last attested reset ───────────────────────
    let start = entries
        .iter()
        .rposition(|e| e.verb == Verb::Repair)
        .unwrap_or(0);
    let mut cur: Option<State> = None;
    for (i, e) in entries.iter().enumerate().skip(start) {
        cur = match e.verb {
            Verb::Repair => Some(e.state),
            v => {
                // `next` has exactly one `(None, verb)` entry — `new` — so a log opening
                // with anything else is a missing genesis. Naming it that way is the
                // wording a human can act on; `IllegalStep { from: None }` is not.
                if cur.is_none() && v != Verb::New {
                    return Err(LogViolation::NoGenesis { first: v });
                }
                let n = next(cur, v).ok_or(LogViolation::IllegalStep {
                    index: i,
                    from: cur,
                    verb: v,
                })?;
                if n != e.state {
                    return Err(LogViolation::StateMismatch {
                        index: i,
                        logged: e.state,
                        legal: n,
                    });
                }
                Some(n)
            }
        };
    }
    cur.ok_or(LogViolation::Empty)
}

/// Called per touched ticket by `Store::transact` on commit AND by `doctor`.
pub fn prove(t: &Ticket) -> std::result::Result<(), LogViolation> {
    let replayed = replay(&t.log)?;
    if replayed != t.fm.state {
        return Err(LogViolation::Divergence {
            replayed,
            frontmatter: t.fm.state,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_and_allowed_slice_agree_over_every_pair() {
        for from in [None]
            .into_iter()
            .chain(ALL_STATES.iter().copied().map(Some))
        {
            for &verb in ALL_VERBS {
                let legal = next(from, verb).is_some();
                let listed = allowed_slice(from).contains(&verb);
                assert_eq!(
                    legal, listed,
                    "({from:?}, {verb}) table/allowed_slice disagree"
                );
            }
        }
    }

    #[test]
    fn every_verb_agrees_with_states_for() {
        for &verb in ALL_VERBS {
            for &st in ALL_STATES {
                assert_eq!(
                    next(Some(st), verb).is_some(),
                    states_for(verb).contains(&st),
                    "({st}, {verb}) states_for disagrees with the table"
                );
            }
        }
    }

    #[test]
    fn terminal_states_accept_only_confirm_and_repair() {
        for st in [State::Done, State::Dropped] {
            assert_eq!(allowed_slice(Some(st)), &[Verb::Confirm, Verb::Repair][..]);
        }
    }

    /// The rule behind [`onward`], stated as a property rather than as the two literal
    /// strings `tests/transition_table.rs` pins: a closed ticket is never sent at a verb its
    /// own state allows. Both of those — `confirm` and `repair` — answer questions about
    /// work that is over, and pointing at one is how the refusal chain came to have no end.
    #[test]
    fn a_terminal_refusal_never_names_a_verb_that_comes_straight_back() {
        let id = TicketId::parse("t-ea32").unwrap();
        for st in [State::Done, State::Dropped] {
            let offered = onward(&id, st);
            for &v in allowed_slice(Some(st)) {
                let cmd = v.command_as("kanspec", &id);
                assert!(
                    !offered.iter().any(|f| f.as_str() == cmd),
                    "a {st} ticket was told `{cmd}`, which lands it back here"
                );
            }
            // ...and it IS told the two things that end the chain: look, or open a new one.
            assert_eq!(
                offered.iter().map(Fix::as_str).collect::<Vec<_>>(),
                [
                    "kanspec show t-ea32",
                    Verb::New.command_as("kanspec", &id).as_str()
                ]
            );
        }
        // A ticket that can still move keeps the advice it had: the first legal verb.
        for st in [State::Todo, State::Doing, State::Review] {
            assert_eq!(
                onward(&id, st).iter().last().map(Fix::as_str),
                Some(
                    allowed_slice(Some(st))[0]
                        .command_as("kanspec", &id)
                        .as_str()
                )
            );
        }
    }

    #[test]
    fn join_or_reads_like_english() {
        assert_eq!(states_str(&[State::Doing]), "doing");
        assert_eq!(
            states_str(&[State::Doing, State::Review]),
            "doing or review"
        );
        assert_eq!(
            states_str(&[State::Todo, State::Doing, State::Review]),
            "todo, doing, or review"
        );
    }
}
