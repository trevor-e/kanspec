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

use crate::error::{KsError, Result};
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

const FROM_NONE: &[Verb] = &[Verb::New];
const FROM_TODO: &[Verb] = &[Verb::Start, Verb::Drop, Verb::Confirm, Verb::Repair];
const FROM_DOING: &[Verb] = &[
    Verb::Ship,
    Verb::Done,
    Verb::Park,
    Verb::Drop,
    Verb::Confirm,
    Verb::Repair,
];
const FROM_REVIEW: &[Verb] = &[
    Verb::Start,
    Verb::Done,
    Verb::Drop,
    Verb::Confirm,
    Verb::Repair,
];
const FROM_TERMINAL: &[Verb] = &[Verb::Confirm, Verb::Repair];

/// `&'static [Verb]`, because `KsError::IllegalTransition` holds one without allocating.
pub const fn allowed_slice(from: Option<State>) -> &'static [Verb] {
    match from {
        None => FROM_NONE,
        Some(State::Todo) => FROM_TODO,
        Some(State::Doing) => FROM_DOING,
        Some(State::Review) => FROM_REVIEW,
        Some(State::Done) | Some(State::Dropped) => FROM_TERMINAL,
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
        V::Confirm | V::Repair => ALL_STATES_SLICE,
    }
}
const ALL_STATES_SLICE: &[State] = &[
    State::Todo,
    State::Doing,
    State::Review,
    State::Done,
    State::Dropped,
];

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
pub fn require(id: &TicketId, from: State, verb: Verb) -> Result<State> {
    match next(Some(from), verb) {
        Some(to) => Ok(to),
        None => Err(KsError::IllegalTransition {
            id: id.clone(),
            from,
            verb,
            allowed: allowed_slice(Some(from)),
            fix: fixes![
                fix!("kanspec show {id}"),
                fix!("kanspec {} {id}", allowed_slice(Some(from))[0]),
            ],
        }),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The mechanical proof (invariant 10)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "break", rename_all = "snake_case")]
pub enum LogViolation {
    Empty,
    NoGenesis {
        first: Verb,
    },
    IllegalStep {
        index: usize,
        from: Option<State>,
        verb: Verb,
    },
    StateMismatch {
        index: usize,
        logged: State,
        legal: State,
    },
    OutOfOrder {
        index: usize,
        at: DateTime<Utc>,
    },
    Divergence {
        replayed: State,
        frontmatter: State,
    },
}

impl std::fmt::Display for LogViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogViolation::Empty => write!(f, "the ## Log is empty — no state was ever reached"),
            LogViolation::NoGenesis { first } => {
                write!(f, "the ## Log opens with `{first}`, not `new`")
            }
            LogViolation::IllegalStep { index, from, verb } => write!(
                f,
                "log entry {index}: `{verb}` is not legal from {}",
                from.map(|s| s.as_str()).unwrap_or("nothing")
            ),
            LogViolation::StateMismatch {
                index,
                logged,
                legal,
            } => write!(
                f,
                "log entry {index} records `{logged}` where the table says `{legal}`"
            ),
            LogViolation::OutOfOrder { index, at } => {
                write!(f, "log entry {index} ({at}) goes backwards in time")
            }
            LogViolation::Divergence {
                replayed,
                frontmatter,
            } => write!(
                f,
                "frontmatter says `{frontmatter}` but the ## Log replays to `{replayed}`"
            ),
        }
    }
}

/// Folds the ticket's own `## Log` through the SAME oracle the write path uses. Checks
/// three things: legal sequence, each entry's RECORDED state against the legal successor
/// (a doctored log LINE, not just a doctored frontmatter field), and timestamp
/// monotonicity.
pub fn replay(entries: &[LogEntry]) -> std::result::Result<State, LogViolation> {
    let mut cur: Option<State> = None;
    let mut last: Option<DateTime<Utc>> = None;
    for (i, e) in entries.iter().enumerate() {
        if let Some(prev) = last {
            if e.at < prev {
                return Err(LogViolation::OutOfOrder { index: i, at: e.at });
            }
        }
        last = Some(e.at);
        cur = match e.verb {
            // Repair is the ONE verb whose logged state is authoritative rather than
            // derived: the human-attested reset that makes an imported or already-broken
            // repo RECOVERABLE instead of permanently unwritable (D-12).
            Verb::Repair => Some(e.state),
            v => {
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
            assert_eq!(allowed_slice(Some(st)), FROM_TERMINAL);
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
