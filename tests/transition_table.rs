//! `tests/transition_table.rs`
//!
//! Proves: exhaustive (State x Verb); the replay matrix; the repair reset
//!
//! Owner: **S4**.
//!
//! `transitions.rs` is the legality oracle for the WHOLE product: the write path, the
//! board's drag-to-verb, `doctor`, and every refusal message quote it. It is also frozen,
//! so a regression in it can only come from a rebase or a merge — which is exactly the
//! kind of break a table written INDEPENDENTLY of the implementation catches and a
//! property test derived from it does not.
//!
//! Every expectation below is a literal, transcribed from DESIGN.md's lifecycle and
//! ARCHITECTURE.md §2.4 by hand. Nothing here calls `next()` to decide what `next()`
//! should say.

use kanspec::transitions::{
    allowed_from, allowed_slice, next, prove, replay, require, states_for, states_str, verbs_str,
    LogViolation, State, Verb, ALL_STATES, ALL_VERBS,
};

use State::*;
use Verb as V;

/// THE table, written out. 6 rows (5 states + "does not exist yet") x 8 verbs = 48 pairs,
/// of which the 40 that start from a real state are the (State x Verb) matrix.
const TABLE: &[(Option<State>, Verb, Option<State>)] = &[
    // ── from nothing: only `new` ─────────────────────────────────────────────
    (None, V::New, Some(Todo)),
    (None, V::Start, None),
    (None, V::Ship, None),
    (None, V::Done, None),
    (None, V::Park, None),
    (None, V::Drop, None),
    (None, V::Confirm, None),
    (None, V::Repair, None),
    // ── todo ─────────────────────────────────────────────────────────────────
    (Some(Todo), V::New, None),
    (Some(Todo), V::Start, Some(Doing)),
    (Some(Todo), V::Ship, None),
    (Some(Todo), V::Done, None), // `done` from todo is the gate nobody may skip
    (Some(Todo), V::Park, None),
    (Some(Todo), V::Drop, Some(Dropped)),
    (Some(Todo), V::Confirm, Some(Todo)),
    (Some(Todo), V::Repair, Some(Todo)),
    // ── doing ────────────────────────────────────────────────────────────────
    (Some(Doing), V::New, None),
    (Some(Doing), V::Start, None), // a second claim is a refusal, not a no-op
    (Some(Doing), V::Ship, Some(Review)),
    (Some(Doing), V::Done, Some(Done)), // requires a NoCodeWaiver
    (Some(Doing), V::Park, Some(Todo)),
    (Some(Doing), V::Drop, Some(Dropped)),
    (Some(Doing), V::Confirm, Some(Doing)),
    (Some(Doing), V::Repair, Some(Doing)),
    // ── review ───────────────────────────────────────────────────────────────
    (Some(Review), V::New, None),
    (Some(Review), V::Start, Some(Doing)), // rework
    (Some(Review), V::Ship, None),
    (Some(Review), V::Done, Some(Done)), // requires a MergedProof
    (Some(Review), V::Park, None),
    (Some(Review), V::Drop, Some(Dropped)),
    (Some(Review), V::Confirm, Some(Review)),
    (Some(Review), V::Repair, Some(Review)),
    // ── done: terminal ───────────────────────────────────────────────────────
    (Some(Done), V::New, None),
    (Some(Done), V::Start, None),
    (Some(Done), V::Ship, None),
    (Some(Done), V::Done, None),
    (Some(Done), V::Park, None),
    (Some(Done), V::Drop, None),
    (Some(Done), V::Confirm, Some(Done)),
    (Some(Done), V::Repair, Some(Done)),
    // ── dropped: terminal ────────────────────────────────────────────────────
    (Some(Dropped), V::New, None),
    (Some(Dropped), V::Start, None),
    (Some(Dropped), V::Ship, None),
    (Some(Dropped), V::Done, None),
    (Some(Dropped), V::Park, None),
    (Some(Dropped), V::Drop, None),
    (Some(Dropped), V::Confirm, Some(Dropped)),
    (Some(Dropped), V::Repair, Some(Dropped)),
];

#[test]
fn the_table_is_exhaustive_and_every_pair_is_written_down() {
    assert_eq!(TABLE.len(), 48, "6 rows x 8 verbs");
    let real: usize = TABLE.iter().filter(|(f, _, _)| f.is_some()).count();
    assert_eq!(real, 40, "the (State x Verb) matrix is 5 x 8");

    for from in std::iter::once(None).chain(ALL_STATES.iter().copied().map(Some)) {
        for &verb in ALL_VERBS {
            assert_eq!(
                TABLE
                    .iter()
                    .filter(|(f, v, _)| *f == from && *v == verb)
                    .count(),
                1,
                "({from:?}, {verb}) appears exactly once in the written table"
            );
        }
    }
}

#[test]
fn next_agrees_with_the_written_table_over_all_48_pairs() {
    for &(from, verb, want) in TABLE {
        assert_eq!(
            next(from, verb),
            want,
            "({from:?}, {verb}) -> expected {want:?}"
        );
    }
}

#[test]
fn allowed_from_lists_exactly_the_legal_verbs() {
    for from in std::iter::once(None).chain(ALL_STATES.iter().copied().map(Some)) {
        let want: Vec<Verb> = TABLE
            .iter()
            .filter(|(f, _, to)| *f == from && to.is_some())
            .map(|(_, v, _)| *v)
            .collect();
        assert_eq!(allowed_from(from), want, "allowed_from({from:?})");
        assert_eq!(allowed_slice(from), want.as_slice());
        assert!(
            !want.is_empty(),
            "every state must offer at least one verb, or a ticket in it is unwritable"
        );
    }
}

#[test]
fn states_for_is_the_transpose_of_the_table() {
    for &verb in ALL_VERBS {
        let want: Vec<State> = TABLE
            .iter()
            .filter(|(f, v, to)| *v == verb && f.is_some() && to.is_some())
            .map(|(f, _, _)| f.unwrap())
            .collect();
        assert_eq!(states_for(verb), want.as_slice(), "states_for({verb})");
    }
    // `new` is legal from NO state — it is the genesis verb.
    assert!(states_for(V::New).is_empty());
}

#[test]
fn require_returns_the_successor_or_a_refusal_naming_the_legal_states() {
    let id = kanspec::ids::TicketId::parse("t-9c41").unwrap();
    for &(from, verb, want) in TABLE {
        let Some(from) = from else { continue };
        match (require(&id, from, verb), want) {
            (Ok(got), Some(want)) => assert_eq!(got, want, "({from}, {verb})"),
            (Err(e), None) => {
                assert_eq!(e.kind(), "illegal_transition");
                assert!(
                    e.fixes().iter().next().is_some(),
                    "invariant 9: ({from}, {verb}) named no fix"
                );
                // The message names the states the verb IS legal from, never the verb.
                assert!(
                    e.to_string()
                        .contains(&states_str(states_for(verb)).to_string()),
                    "({from}, {verb}) said {e}"
                );
            }
            (got, want) => panic!("({from}, {verb}) -> {got:?}, expected {want:?}"),
        }
    }
}

#[test]
fn the_refusal_reads_like_the_contracts_worked_example() {
    let id = kanspec::ids::TicketId::parse("t-9c41").unwrap();
    let e = require(&id, Review, V::Ship).unwrap_err();
    assert_eq!(e.to_string(), "t-9c41 is review, not doing — cannot ship");
    assert_eq!(
        verbs_str(&[V::Start, V::Done, V::Drop]),
        "start, done, or drop"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// replay — the mechanical proof (invariant 10)
// ─────────────────────────────────────────────────────────────────────────────

mod trail {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};
    use kanspec::logentry::LogEntry;

    pub fn at(min: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap() + chrono::Duration::minutes(min)
    }

    pub fn e(min: i64, verb: Verb, state: State) -> LogEntry {
        LogEntry {
            at: at(min),
            state,
            actor: "trevor".into(),
            verb,
            note: None,
        }
    }

    pub fn by(min: i64, actor: &str, verb: Verb, state: State) -> LogEntry {
        LogEntry {
            actor: actor.into(),
            ..e(min, verb, state)
        }
    }
}

use trail::{by, e};

#[test]
fn every_legal_trail_replays_to_the_state_its_last_entry_records() {
    let cases: Vec<(&str, Vec<kanspec::logentry::LogEntry>, State)> = vec![
        ("new", vec![e(0, V::New, Todo)], Todo),
        (
            "new -> start",
            vec![e(0, V::New, Todo), e(1, V::Start, Doing)],
            Doing,
        ),
        (
            "the DESIGN.md happy path",
            vec![
                e(0, V::New, Todo),
                e(1, V::Start, Doing),
                e(2, V::Ship, Review),
                e(3, V::Done, Done),
            ],
            Done,
        ),
        (
            "rework: review -> doing -> review",
            vec![
                e(0, V::New, Todo),
                e(1, V::Start, Doing),
                e(2, V::Ship, Review),
                e(3, V::Start, Doing),
                e(4, V::Ship, Review),
            ],
            Review,
        ),
        (
            "park and re-claim",
            vec![
                e(0, V::New, Todo),
                e(1, V::Start, Doing),
                e(2, V::Park, Todo),
                e(3, V::Start, Doing),
            ],
            Doing,
        ),
        (
            "the no-code close: doing -> done",
            vec![
                e(0, V::New, Todo),
                e(1, V::Start, Doing),
                e(2, V::Done, Done),
            ],
            Done,
        ),
        (
            "a recorded human confirmation is a non-transition",
            vec![
                e(0, V::New, Todo),
                e(1, V::Start, Doing),
                e(2, V::Ship, Review),
                e(3, V::Confirm, Review),
                e(4, V::Done, Done),
            ],
            Done,
        ),
        (
            "dropped from the backlog",
            vec![e(0, V::New, Todo), e(1, V::Drop, Dropped)],
            Dropped,
        ),
        (
            "two entries in the same minute are in order",
            vec![e(0, V::New, Todo), e(0, V::Start, Doing)],
            Doing,
        ),
    ];
    for (name, log, want) in cases {
        assert_eq!(replay(&log), Ok(want), "{name}");
    }
}

/// **FINDING, reported to F as a request against the frozen `transitions.rs`:**
/// `LogViolation::NoGenesis` is UNCONSTRUCTIBLE. `replay` folds from `cur = None`, and the
/// table has no `(None, verb)` entry but `new`, so a log that opens with anything else
/// trips `IllegalStep { index: 0, from: None }` first and `NoGenesis` is never built.
///
/// The break IS caught either way — this is a message-quality gap, not a safety one — but
/// `NoGenesis`'s wording ("the ## Log opens with `start`, not `new`") is the one a human
/// can act on, and the variant is currently dead code. The fix is one branch at the top of
/// `replay`; it is F's file, so this test pins TODAY'S behaviour and names the gap rather
/// than quietly asserting the nicer message.
#[test]
fn a_log_that_does_not_open_with_new_is_caught_as_an_illegal_step_not_as_no_genesis() {
    assert_eq!(
        replay(&[e(0, V::Start, Doing)]),
        Err(LogViolation::IllegalStep {
            index: 0,
            from: Option::None,
            verb: V::Start,
        }),
        "if this ever returns NoGenesis, delete this test and restore the one below"
    );
    // The variant still renders, so the day `replay` starts building it nothing else moves.
    assert_eq!(
        LogViolation::NoGenesis { first: V::Start }.to_string(),
        "the ## Log opens with `start`, not `new`"
    );
}

#[test]
fn every_log_violation_variant_is_reachable_from_a_real_break() {
    // empty: a ticket whose `## Log` was deleted
    assert_eq!(replay(&[]), Err(LogViolation::Empty));

    // illegal step: two claims in a row — the cross-machine double-claim shape (R-8)
    assert_eq!(
        replay(&[
            e(0, V::New, Todo),
            by(1, "trevor", V::Start, Doing),
            by(2, "claude/sess-a91", V::Start, Doing),
        ]),
        Err(LogViolation::IllegalStep {
            index: 2,
            from: Some(Doing),
            verb: V::Start,
        })
    );

    // state mismatch: the LINE was doctored, not the frontmatter
    assert_eq!(
        replay(&[e(0, V::New, Todo), e(1, V::Start, Done)]),
        Err(LogViolation::StateMismatch {
            index: 1,
            logged: Done,
            legal: Doing,
        })
    );

    // out of order: a union merge interleaved two machines' lines
    assert_eq!(
        replay(&[e(5, V::New, Todo), e(1, V::Start, Doing)]),
        Err(LogViolation::OutOfOrder {
            index: 1,
            at: trail::at(1),
        })
    );
}

#[test]
fn every_violation_says_what_broke_in_words_a_human_can_act_on() {
    for v in [
        LogViolation::Empty,
        LogViolation::NoGenesis { first: V::Start },
        LogViolation::IllegalStep {
            index: 2,
            from: Some(Doing),
            verb: V::Start,
        },
        LogViolation::StateMismatch {
            index: 1,
            logged: Done,
            legal: Doing,
        },
        LogViolation::OutOfOrder {
            index: 1,
            at: trail::at(1),
        },
        LogViolation::Divergence {
            replayed: Review,
            frontmatter: Done,
        },
    ] {
        let s = v.to_string();
        assert!(!s.is_empty() && !s.contains("LogViolation"), "{s}");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// repair — the one verb whose logged state is authoritative (D-12)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn repair_resets_the_replay_to_its_own_recorded_state() {
    // The two breaks `repair` genuinely rescues today.
    //
    // 1. Divergence — the classic `sed -i 's/state: review/state: done/'`. The log is
    //    legal; it just does not reach what the file claims.
    let legal = vec![
        e(0, V::New, Todo),
        e(1, V::Start, Doing),
        e(2, V::Ship, Review),
    ];
    assert!(prove(&fixture::ticket(Done, legal.clone())).is_err());
    let mut repaired = legal.clone();
    repaired.push(e(3, V::Repair, Done));
    assert_eq!(replay(&repaired), Ok(Done));
    assert!(prove(&fixture::ticket(Done, repaired.clone())).is_ok());

    // 2. An empty log — the whole `## Log` section was deleted.
    assert_eq!(replay(&[]), Err(LogViolation::Empty));
    assert_eq!(replay(&[e(0, V::Repair, Doing)]), Ok(Doing));

    // ...and the trail continues normally from the reset point.
    let mut onward = repaired;
    onward.push(e(4, V::Confirm, Done));
    assert_eq!(replay(&onward), Ok(Done));
}

/// **FINDING, reported to F as a request against the frozen `transitions.rs`:**
/// `replay` folds strictly forward and returns on the FIRST bad entry, so a `Verb::Repair`
/// line appended after an `IllegalStep` / `StateMismatch` / `OutOfOrder` is never reached.
/// D-12 says repair exists so that "an imported or already-broken repo" is recoverable
/// rather than permanently unwritable — and those three violations are exactly what an
/// import and a union-merged `## Log` produce (see the two-`start` case above).
///
/// `Store::transact` step 8 re-proves the STAGED bytes with no `Repair` exemption, so the
/// repair cannot even be written: the ticket stays unwritable forever.
///
/// The fix is local to `replay`: begin the fold at the LAST `Repair` entry, seeding
/// `cur = Some(entry.state)` and `last = Some(entry.at)` and skipping everything before
/// it. That is F's file, so this test pins TODAY'S behaviour and names the gap.
#[test]
fn repair_does_not_yet_rescue_a_log_whose_earlier_entries_are_illegal() {
    for broken in [
        // an illegal step (an import, or two machines' claims union-merged)
        vec![e(0, V::New, Todo), e(1, V::Done, Done)],
        // a doctored line
        vec![e(0, V::New, Todo), e(1, V::Start, Done)],
        // lines interleaved out of order by a merge
        vec![e(5, V::New, Todo), e(1, V::Start, Doing)],
    ] {
        assert!(replay(&broken).is_err());
        let mut attempted = broken.clone();
        attempted.push(e(9, V::Repair, Todo));
        assert!(
            replay(&attempted).is_err(),
            "if this starts passing, `repair` became the reset D-12 describes — \
             delete this test and assert Ok(Todo) instead: {attempted:#?}"
        );
    }
}

#[test]
fn repair_is_legal_from_every_state_including_the_terminal_ones() {
    // The whole point of D-12: an ALREADY-BROKEN repo must be recoverable, whatever it
    // claims to be.
    for &st in ALL_STATES {
        assert_eq!(next(Some(st), V::Repair), Some(st));
        assert!(allowed_slice(Some(st)).contains(&V::Repair));
    }
    // Repair cannot conjure a ticket that never existed.
    assert_eq!(next(None, V::Repair), None);
}

#[test]
fn repair_does_not_launder_the_entries_after_it() {
    // A repair to `todo` followed by an illegal `ship` is still illegal: the reset is a
    // new starting point, not an amnesty.
    let log = vec![
        e(0, V::New, Todo),
        e(1, V::Repair, Todo),
        e(2, V::Ship, Review),
    ];
    assert_eq!(
        replay(&log),
        Err(LogViolation::IllegalStep {
            index: 2,
            from: Some(Todo),
            verb: V::Ship,
        })
    );
}

#[test]
fn repair_still_obeys_the_clock() {
    let log = vec![
        e(5, V::New, Todo),
        e(1, V::Repair, Done), // backdated
    ];
    assert!(matches!(
        replay(&log),
        Err(LogViolation::OutOfOrder { index: 1, .. })
    ));
}

// ─────────────────────────────────────────────────────────────────────────────
// prove — replay PLUS the frontmatter it must agree with
// ─────────────────────────────────────────────────────────────────────────────

mod fixture {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::SystemTime;

    use kanspec::logentry::LogEntry;
    use kanspec::model::{Ticket, TicketFm};

    pub fn ticket(state: State, log: Vec<LogEntry>) -> Ticket {
        Ticket {
            fm: TicketFm {
                id: kanspec::ids::TicketId::parse("t-9c41").unwrap(),
                title: "Rate-limit login endpoint".into(),
                state,
                spec: None,
                proposal: None,
                item: None,
                deps: Vec::new(),
                followup_of: None,
                discovered_in: None,
                branch: None,
                worktree: None,
                claimed_by: None,
                pr: None,
                head: None,
                spec_unchanged: None,
                created: super::trail::at(0),
                extra: BTreeMap::new(),
            },
            path: PathBuf::from(".kanspec/tickets/t-9c41.md"),
            body: String::new(),
            steps: Vec::new(),
            log,
            mtime: SystemTime::UNIX_EPOCH,
        }
    }
}

#[test]
fn prove_catches_the_hand_edited_frontmatter_the_log_never_reached() {
    let log = vec![
        e(0, V::New, Todo),
        e(1, V::Start, Doing),
        e(2, V::Ship, Review),
    ];
    assert!(prove(&fixture::ticket(Review, log.clone())).is_ok());

    // `sed -i 's/state: review/state: done/'` — R-2, exactly.
    assert_eq!(
        prove(&fixture::ticket(Done, log)),
        Err(LogViolation::Divergence {
            replayed: Review,
            frontmatter: Done,
        })
    );
}

#[test]
fn every_state_is_provable_by_some_legal_trail() {
    // If a state were unreachable through the table, a ticket could exist in it only by
    // hand-edit — which would make `doctor` unsatisfiable rather than strict.
    let trails: Vec<(State, Vec<kanspec::logentry::LogEntry>)> = vec![
        (Todo, vec![e(0, V::New, Todo)]),
        (Doing, vec![e(0, V::New, Todo), e(1, V::Start, Doing)]),
        (
            Review,
            vec![
                e(0, V::New, Todo),
                e(1, V::Start, Doing),
                e(2, V::Ship, Review),
            ],
        ),
        (
            Done,
            vec![
                e(0, V::New, Todo),
                e(1, V::Start, Doing),
                e(2, V::Ship, Review),
                e(3, V::Done, Done),
            ],
        ),
        (Dropped, vec![e(0, V::New, Todo), e(1, V::Drop, Dropped)]),
    ];
    for (state, log) in trails {
        assert!(prove(&fixture::ticket(state, log)).is_ok(), "{state}");
    }
}

#[test]
fn the_glyph_legend_is_one_character_per_state_and_they_are_all_different() {
    let mut seen: Vec<&str> = Vec::new();
    for &s in ALL_STATES {
        assert_eq!(s.glyph().chars().count(), 1, "{s}");
        assert!(!seen.contains(&s.glyph()), "{s} reuses a glyph");
        seen.push(s.glyph());
        // and the frontmatter spelling round-trips
        assert_eq!(State::parse(s.as_str()), Some(s));
    }
    for &v in ALL_VERBS {
        assert_eq!(Verb::parse(v.as_str()), Some(v));
    }
}
