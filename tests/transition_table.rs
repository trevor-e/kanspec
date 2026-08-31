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
// Invariant 9, mechanically: a suggested fix that does not RUN is not a fix
// ─────────────────────────────────────────────────────────────────────────────
//
// `error.rs` makes `Fixes` non-empty by type, which proves a refusal names *something*.
// Nothing proved the something was a command. It was not: `require` built the second line
// by lowercasing the first legal `Verb`, so every refusal from a terminal state — i.e. the
// most common mistake there is, acting on an already-closed ticket — advised
// `kanspec confirm t-ea32`, which clap answers with `unrecognized subcommand` and exit 64.
// `confirm` is a flag on `scan`; four verbs need a `--why`; `new` needs a title.
//
// So the property is checked against the REAL clap tree rather than against a second list
// of what the tree is believed to contain. A flag that later becomes mandatory, a verb that
// is renamed, a ninth `Verb` — each one fails here instead of in a user's terminal.

/// Split a fix line the way a shell would: the placeholders are quoted (`--why "..."`), and
/// `split_whitespace` would hand clap a literal `"..."` with the quotes still attached —
/// which parses, and would let a broken quoting bug through.
fn argv(cmd: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut started) = (Vec::new(), String::new(), false, false);
    for c in cmd.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    assert!(!quoted, "unbalanced quote in {cmd:?}");
    if started {
        out.push(cur);
    }
    out
}

#[test]
fn the_shell_split_the_other_assertions_lean_on_actually_works() {
    assert_eq!(
        argv("ks park t-9c41 --why \"a b\""),
        ["ks", "park", "t-9c41", "--why", "a b"]
    );
    assert_eq!(argv("kanspec show t-9c41"), ["kanspec", "show", "t-9c41"]);
    assert_eq!(argv("kanspec new \"\""), ["kanspec", "new", ""]);
}

/// Parses `cmd` against the real tree, `Cli::try_parse_from` and all.
#[track_caller]
fn must_run(cmd: &str, why: &str) {
    use clap::Parser;
    let argv = argv(cmd);
    assert!(argv.len() > 1, "{why}: `{cmd}` names no subcommand");
    if let Err(e) = kanspec::cli::Cli::try_parse_from(&argv) {
        panic!("{why}: `{cmd}` is not a runnable command — clap says:\n{e}");
    }
}

/// THE regression: every illegal `(state, verb)` pair, every fix line it names, parsed.
#[test]
fn every_illegal_pair_suggests_commands_the_real_cli_accepts() {
    let id = kanspec::ids::TicketId::parse("t-ea32").unwrap();
    let mut checked = 0;
    for &(from, verb, to) in TABLE {
        let (Some(from), None) = (from, to) else {
            continue;
        };
        let e = require(&id, from, verb).unwrap_err();
        for fix in e.fixes().iter() {
            must_run(
                fix.as_str(),
                &format!("invariant 9: `{verb}` from {from} offered a fix"),
            );
            checked += 1;
        }
    }
    // 21 illegal pairs from a real state (the 40-pair matrix minus 19 legal), 2 fixes each.
    assert_eq!(checked, 42, "every illegal pair's every fix was parsed");
}

/// ...and the mapping itself, over ALL_VERBS. `require` only ever names the FIRST legal verb
/// of a state, which reaches just `start`, `ship` and `confirm` — so the other five would
/// never be parsed by the test above, and `park`'s missing `--why` would ship unnoticed
/// until the day someone reordered `FROM_DOING`.
#[test]
fn every_verb_names_an_invocation_the_real_cli_accepts() {
    let id = kanspec::ids::TicketId::parse("t-ea32").unwrap();
    for &v in ALL_VERBS {
        for ks in ["kanspec", "ks"] {
            let cmd = v.command_as(ks, &id);
            assert!(
                cmd.starts_with(&format!("{ks} ")),
                "`{v}` ignored how the binary was invoked: {cmd}"
            );
            must_run(&cmd, &format!("Verb::{v}'s invocation"));
        }
        // the default spelling is the process-wide `invoked_as`, unset here => `kanspec`
        assert_eq!(v.command(&id), v.command_as("kanspec", &id));
    }
}

/// The four verbs whose command carries a mandatory flag, written out literally, so an edit
/// that drops one fails by NAME rather than only as "clap rejected something".
#[test]
fn the_verbs_that_are_not_their_own_subcommand_are_spelled_out() {
    let id = kanspec::ids::TicketId::parse("t-ea32").unwrap();
    assert_eq!(
        V::Confirm.command_as("kanspec", &id),
        // `confirm` is not a subcommand at all — this is the spelling docs/done.md prints
        "kanspec scan --confirm t-ea32 --why \"...\"",
    );
    assert_eq!(
        V::Repair.command_as("ks", &id),
        "ks repair t-ea32 --why \"...\""
    );
    assert_eq!(
        V::Park.command_as("ks", &id),
        "ks park t-ea32 --why \"...\""
    );
    assert_eq!(
        V::Drop.command_as("ks", &id),
        "ks drop t-ea32 --why \"...\""
    );
    // and the plain ones stay plain
    assert_eq!(V::Start.command_as("ks", &id), "ks start t-ea32");
    assert_eq!(V::Ship.command_as("ks", &id), "ks ship t-ea32");
    assert_eq!(V::Done.command_as("ks", &id), "ks done t-ea32");
}

/// The bug as the audit hit it, end to end: `kanspec start <a done ticket>`.
#[test]
fn starting_a_done_ticket_is_refused_with_a_command_that_exists() {
    let id = kanspec::ids::TicketId::parse("t-ea32").unwrap();
    for terminal in [Done, Dropped] {
        let e = require(&id, terminal, V::Start).unwrap_err();
        let fixes: Vec<&str> = e.fixes().iter().map(|f| f.as_str()).collect();
        assert!(
            !fixes.contains(&"kanspec confirm t-ea32"),
            "`confirm` is a flag on `scan`, never a subcommand: {fixes:?}"
        );
        assert_eq!(
            fixes,
            [
                "kanspec show t-ea32",
                "kanspec scan --confirm t-ea32 --why \"...\"",
            ],
            "from {terminal}"
        );
    }
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

/// A log that does not open with `new` is a MISSING GENESIS, and says so.
///
/// (History, so it is not re-broken: `replay` used to fold from `cur = None` straight into
/// `next`, and since the table has exactly one `(None, verb)` entry — `new` — every such
/// log tripped `IllegalStep { index: 0, from: None }` first and `NoGenesis` was dead code.
/// The break was caught either way; the wording was not the one a human can act on. Round B
/// added the branch.)
#[test]
fn a_log_that_does_not_open_with_new_is_a_missing_genesis_by_name() {
    for opener in [V::Start, V::Ship, V::Done, V::Park, V::Drop, V::Confirm] {
        assert_eq!(
            replay(&[e(0, opener, Doing)]),
            Err(LogViolation::NoGenesis { first: opener }),
            "a `## Log` opening with `{opener}` has no genesis"
        );
    }
    assert_eq!(
        LogViolation::NoGenesis { first: V::Start }.to_string(),
        "the ## Log opens with `start`, not `new`"
    );
    // `repair` is the exception, and the whole of D-12: an attested reset IS a genesis.
    assert_eq!(replay(&[e(0, V::Repair, Doing)]), Ok(Doing));
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

/// D-12 in full: `replay` folds legality from the LAST `Repair`, so an attested reset
/// rescues a log whose EARLIER entries are illegal — an import, or the two-`start` line
/// pair a union-merged `## Log` produces. Before round B the fold returned at the first bad
/// entry and never reached the reset, and since `Store::transact` re-proves the staged
/// bytes with no repair exemption, such a ticket could not even have the repair written to
/// it: permanently unwritable, which is the exact outcome D-12 exists to prevent.
#[test]
fn repair_rescues_a_log_whose_earlier_entries_are_illegal() {
    for broken in [
        // an illegal step — an import, or two machines' claims union-merged
        vec![e(0, V::New, Todo), e(1, V::Done, Done)],
        vec![
            e(0, V::New, Todo),
            by(1, "a", V::Start, Doing),
            by(2, "b", V::Start, Doing),
        ],
        // a doctored LINE, not just a doctored frontmatter field
        vec![e(0, V::New, Todo), e(1, V::Start, Done)],
        // no genesis at all
        vec![e(0, V::Ship, Review)],
    ] {
        assert!(
            replay(&broken).is_err(),
            "the fixture must actually be broken: {broken:#?}"
        );
        let mut attested = broken.clone();
        attested.push(e(9, V::Repair, Todo));
        assert_eq!(
            replay(&attested),
            Ok(Todo),
            "an attested reset is a new genesis: {attested:#?}"
        );
    }
}

/// The half repair deliberately does NOT rescue. A `## Log` is a chronological record, and
/// an appended attestation must not be able to launder a temporal anomaly — otherwise the
/// one record that binds the human (R-2) is forgeable by a line at the bottom. The clock is
/// therefore checked over the whole log, with no repair exemption, and the fix the message
/// names is the safe mechanical one: put the lines back in order.
#[test]
fn repair_cannot_launder_a_log_that_runs_backwards_in_time() {
    let interleaved = vec![e(5, V::New, Todo), e(1, V::Start, Doing)];
    assert!(matches!(
        replay(&interleaved),
        Err(LogViolation::OutOfOrder { index: 1, .. })
    ));
    let mut attested = interleaved;
    attested.push(e(9, V::Repair, Todo));
    assert!(
        matches!(
            replay(&attested),
            Err(LogViolation::OutOfOrder { index: 1, .. })
        ),
        "a repair line at the bottom must not make an out-of-order log replay clean"
    );
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
