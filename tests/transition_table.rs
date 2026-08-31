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

mod common;

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
///
/// The same line was wrong TWICE, in two different ways, and this asserts both are gone.
/// Round B: `kanspec confirm t-ea32` was not a command at all (`confirm` is a flag on
/// `scan`), so the fix could not be run. Round D: the runnable spelling of it could be run
/// and led nowhere — `scan --confirm` on a ticket that never had a branch answers `ship`,
/// and `ship` on a terminal ticket answered `scan --confirm`. See
/// `a_terminal_ticket_is_told_something_final_rather_than_sent_round_again`.
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
        assert!(
            !fixes.contains(&"kanspec scan --confirm t-ea32 --why \"...\""),
            "a closed ticket has nothing to attest, and `--confirm` bounces back: {fixes:?}"
        );
        assert_eq!(
            fixes,
            ["kanspec show t-ea32", "kanspec new \"...\""],
            "from {terminal}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ...and invariant 9 taken all the way: FOLLOWING the fixes has to TERMINATE
// ─────────────────────────────────────────────────────────────────────────────
//
// `every_illegal_pair_suggests_commands_the_real_cli_accepts` proves each suggestion is a
// command clap accepts. That is a property of one arrow. It cannot see a CHAIN, and the
// chains were not all finite:
//
//     $ ks done t-8e2b       ✗ t-8e2b is dropped, not doing or review — cannot done
//                            → ks scan --confirm t-8e2b --why "..."
//     $ ks scan --confirm …  ✗ t-8e2b records neither a `head:` SHA nor a resolvable branch
//                            → kanspec ship t-8e2b
//     $ kanspec ship t-8e2b  ✗ t-8e2b is dropped, not doing — cannot ship
//                            → kanspec scan --confirm t-8e2b --why "..."   ← step 2 again
//
// Every arrow there parses. A human sees the ring on the third line and stops; an agent
// whose whole contract is "run the suggested command" does not. So the property this file
// asserts is the one that makes the contract safe to automate: from EVERY (state, verb)
// the CLI can express, following the suggestions reaches a success, or a refusal with
// nothing further to run, in a bounded number of steps and WITHOUT REVISITING A STATE.
//
// The walk runs the real binary against real tickets, because a refusal's fixes come from
// the gates the command actually hits, and no table of expectations can know that
// `scan --confirm` on a branchless ticket answers `ship`.

/// The bounded part of "a small bounded number of steps". Nothing legitimate is more than
/// three deep today; six leaves room without letting a runaway walk burn the suite.
const MAX_STEPS: usize = 6;

/// Fixed ids, one per state — so a failure names a state rather than a hash, and so a
/// command string is stable while the fixture underneath it is rewritten.
fn walk_id(state: State) -> &'static str {
    match state {
        Todo => "t-0a01",
        Doing => "t-0d01",
        Review => "t-0e01",
        Done => "t-0f01",
        Dropped => "t-0c01",
    }
}

/// `## Log` lines, one legal trail per state — the same hand-written fixture shape
/// `tests/common/merges.rs` uses, so the ticket `replay`s and `Store::transact` will accept
/// a write to it.
fn walk_log(state: State) -> &'static [&'static str] {
    const NEW: &str = "- 2026-08-30T09:00Z  todo     trevor                new\n";
    const START: &str = "- 2026-08-30T10:00Z  doing    trevor                start\n";
    const SHIP: &str = "- 2026-08-30T11:00Z  review   trevor                ship\n";
    const CLOSE: &str = "- 2026-08-30T12:00Z  done     trevor                done\n";
    const DROP: &str = "- 2026-08-30T10:00Z  dropped  trevor                drop\n";
    match state {
        Todo => &[NEW],
        Doing => &[NEW, START],
        Review => &[NEW, START, SHIP],
        Done => &[NEW, START, SHIP, CLOSE],
        // `drop` straight off the backlog — the shape the transcript above was hit on, and
        // the only one that reaches a terminal state with NO branch and NO `head:`.
        Dropped => &[NEW, DROP],
    }
}

/// What one command did.
enum Said {
    Ok,
    /// a refusal: the message it printed, and the fix lines under it
    No(String, Vec<String>),
}

/// A real repo with one real ticket per state.
struct Bench {
    repo: common::TestRepo,
    /// the branch head each claimed state's fixture points at
    heads: std::collections::BTreeMap<&'static str, String>,
}

impl Bench {
    fn new() -> Bench {
        let repo = common::TestRepo::new();
        // Force the one mock seam ON. With `$KANSPEC_GH_FIXTURES` set, a fixture that is
        // not there is `GhUnavailable` rather than a live `gh` call — the walk must not
        // depend on whether this machine has `gh`, or reach the network.
        repo.gh_fixture("the-walk-never-asks-gh", "{}");
        let mut heads = std::collections::BTreeMap::new();
        for state in [Doing, Review, Done] {
            let id = walk_id(state);
            let branch = format!("ks/{id}-walk");
            repo.git(&["checkout", "--quiet", "-B", &branch, "main"]);
            let file = format!("src/auth/{}.ts", id.replace('-', "_"));
            repo.write(&file, "export const walk = 1;\n");
            // `add -A` would sweep up the ticket files sitting untracked in the working
            // tree and commit them onto THIS branch (merges.rs makes the same point).
            repo.git(&["add", "--", &file]);
            repo.git(&[
                "commit",
                "--quiet",
                "-m",
                &format!("{id}: work\n\nKanspec: {id}\n"),
            ]);
            heads.insert(id, repo.sha("HEAD").trim().to_string());
            repo.git(&["checkout", "--quiet", "main"]);
        }
        Bench { repo, heads }
    }

    /// (Re)writes the fixture for `state` and returns its id.
    ///
    /// Called before EVERY command rather than reasoned about: a refusal moves nothing (the
    /// walk asserts that below), but a suggestion that SUCCEEDS does, and the next branch of
    /// the walk has to start from the same place this one did.
    fn seed(&self, state: State) -> &'static str {
        let id = walk_id(state);
        let mut s = format!(
            "---\nid: {id}\ntitle: the suggestion walk ({state})\nstate: {state}\ndeps: []\n"
        );
        // `start` writes `branch:` and `claimed_by:`; only `ship` writes `head:` (§2.16).
        if let Some(head) = self.heads.get(id) {
            s += &format!("branch: ks/{id}-walk\nclaimed_by: trevor\n");
            if matches!(state, Review | Done) {
                s += &format!("head: {head}\n");
            }
        }
        s += "created: 2026-08-30T09:00:00Z\n---\nThe fixture the suggestion walk runs \
              against.\n\n## Log\n";
        for line in walk_log(state) {
            s += line;
        }
        self.repo.write(&format!(".kanspec/tickets/{id}.md"), &s);
        id
    }

    /// The state on disk right now — read from the file, not from a second `kanspec` run.
    fn state_of(&self, id: &str) -> State {
        let body = self.repo.read(&format!(".kanspec/tickets/{id}.md"));
        let raw = body
            .lines()
            .find_map(|l| l.strip_prefix("state:"))
            .unwrap_or_else(|| panic!("{id} has no `state:` line"))
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        State::parse(&raw).unwrap_or_else(|| panic!("`{raw}` is not a state"))
    }

    /// Runs one suggested command for real, exactly as it was printed.
    fn run(&self, cmd: &str) -> Said {
        let mut args = argv(cmd);
        assert_eq!(
            args.first().map(String::as_str),
            Some("kanspec"),
            "the walk only follows `kanspec` commands: {cmd}"
        );
        args.remove(0);
        args.push("--json".to_string());
        let r = self.repo.ks(&args);
        if r.code == 0 {
            return Said::Ok;
        }
        let v: serde_json::Value = serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
            panic!(
                "`{cmd}` exited {} without JSON ({e}):\n{}",
                r.code, r.stdout
            )
        });
        let fixes = v["error"]["fix"]
            .as_array()
            .unwrap_or_else(|| panic!("invariant 9: `{cmd}` refused with no fix list:\n{v}"))
            .iter()
            .map(|f| f.as_str().unwrap_or_default().to_string())
            .collect();
        Said::No(
            v["error"]["message"].as_str().unwrap_or("?").to_string(),
            fixes,
        )
    }
}

/// The suggestions this file is answerable for: every invocation [`Verb::command_as`] mints,
/// plus the read-only lookups a refusal may point at.
///
/// A fix outside it ends the walk, for one of two reasons. `git commit -m "…"` and
/// `edit .kanspec/specs/auth.md on this branch` are not commands this tool can run at all —
/// there is nothing to auto-follow, which is exactly the honest terminus this test is
/// looking for. And a fix that carries a DISPOSITION flag (`--no-code`, `--no-followups`,
/// `--spawn`, `--drop-step`) is an answer to a question the close-out gate asked, not a
/// suggested transition; that gate owns its own chain and its own tests.
fn vocabulary(id: &str) -> Vec<String> {
    let tid = kanspec::ids::TicketId::parse(id).expect("a walk id");
    let mut v: Vec<String> = ALL_VERBS
        .iter()
        .map(|verb| verb.command_as("kanspec", &tid))
        .collect();
    v.push(format!("kanspec show {id}"));
    v.push(format!("kanspec log {id}"));
    v.push(format!("kanspec scan --explain {id}"));
    v
}

/// `--why "not started yet"` -> `--why "..."`, `new "rate-limit login"` -> `new "..."`.
///
/// Two suggestions that differ only in their prose are the SAME suggestion: they run the
/// same verb against the same ticket and get the same answer. Collapsing the prose is what
/// lets the walk recognise the ring in the transcript above — and what stops it counting a
/// re-worded reason as progress.
fn normalize(cmd: &str) -> String {
    let t = argv(cmd);
    let mut out: Vec<String> = Vec::with_capacity(t.len());
    let mut title_seen = false;
    for (i, tok) in t.iter().enumerate() {
        let after_why = i > 0 && t[i - 1] == "--why";
        let is_new_title = t.get(1).map(String::as_str) == Some("new")
            && i > 1
            && !tok.starts_with('-')
            && !std::mem::replace(&mut title_seen, true);
        out.push(if after_why || is_new_title {
            "...".to_string()
        } else {
            tok.clone()
        });
    }
    out.iter()
        .map(|tok| {
            if tok.is_empty() || tok == "..." || tok.contains(char::is_whitespace) {
                format!("\"{tok}\"")
            } else {
                tok.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn the_suggestion_normaliser_the_walk_leans_on_actually_works() {
    let id = kanspec::ids::TicketId::parse("t-ea32").unwrap();
    // a re-worded reason is the same suggestion
    assert_eq!(
        normalize("kanspec park t-ea32 --why \"not started yet\""),
        V::Park.command_as("kanspec", &id)
    );
    assert_eq!(
        normalize("kanspec scan --confirm t-ea32 --why \"squash merged by hand\""),
        V::Confirm.command_as("kanspec", &id)
    );
    assert_eq!(
        normalize("kanspec new \"rate-limit the login endpoint\""),
        V::New.command_as("kanspec", &id)
    );
    // ...but a disposition flag is a DIFFERENT command, and stays one
    assert_ne!(
        normalize("kanspec done t-ea32 --no-code --why \"docs only\""),
        V::Done.command_as("kanspec", &id)
    );
    assert_eq!(normalize("kanspec show t-ea32"), "kanspec show t-ea32");
    // and every minted invocation is already in its own normal form
    for &v in ALL_VERBS {
        let cmd = v.command_as("kanspec", &id);
        assert_eq!(normalize(&cmd), cmd, "`{v}`");
    }
}

/// One node: run `cmd` against a fresh ticket in `root`, then follow everything it suggests.
fn follow(
    b: &Bench,
    root: State,
    cmd: &str,
    chain: &mut Vec<(String, String)>,
    proven: &mut std::collections::BTreeSet<(&'static str, String)>,
) {
    let key = (root.as_str(), normalize(cmd));
    // THE property. A command already standing in the chain that led here means an agent
    // following the arrows is going round, and will go round for ever.
    if let Some(i) = chain.iter().position(|(c, _)| normalize(c) == key.1) {
        panic!(
            "the suggested fixes never terminate — from a {root} ticket, step {} sends an \
             agent back to step {}:\n\n{}",
            chain.len() + 1,
            i + 1,
            walk_transcript(chain, cmd),
        );
    }
    assert!(
        chain.len() < MAX_STEPS,
        "from a {root} ticket the suggestions are still going after {MAX_STEPS} steps:\n\n{}",
        walk_transcript(chain, cmd),
    );
    if proven.contains(&key) {
        return;
    }

    let id = b.seed(root);
    if let Said::No(message, fixes) = b.run(cmd) {
        // The whole walk rests on this: a refusal is a refusal, so every branch explored
        // below starts from the same ticket the branch above did.
        assert_eq!(
            b.state_of(id),
            root,
            "`{cmd}` refused AND moved the ticket off {root}"
        );
        let next: Vec<String> = fixes
            .iter()
            .filter(|f| vocabulary(id).contains(&normalize(f)))
            .cloned()
            .collect();
        chain.push((
            cmd.to_string(),
            format!("{message}\n     → {}", fixes.join("\n     → ")),
        ));
        for f in next {
            follow(b, root, &f, chain, proven);
        }
        chain.pop();
    }
    proven.insert(key);
}

fn walk_transcript(chain: &[(String, String)], last: &str) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for (i, (cmd, said)) in chain.iter().enumerate() {
        let _ = writeln!(s, "  {}. $ {cmd}\n     ✗ {said}", i + 1);
    }
    let _ = writeln!(s, "  {}. $ {last}", chain.len() + 1);
    s
}

/// THE regression: the suggestion GRAPH, walked from every (state, verb) the CLI can say.
#[test]
fn following_the_suggested_fixes_terminates_from_every_state_and_verb() {
    let b = Bench::new();
    let mut proven = std::collections::BTreeSet::new();
    for &state in ALL_STATES {
        let id = kanspec::ids::TicketId::parse(walk_id(state)).unwrap();
        for &verb in ALL_VERBS {
            // `new` MINTS an id; there is no way to aim it at an existing ticket, so the
            // `(state, new)` refusals in TABLE have no CLI invocation to walk from.
            if verb == V::New {
                continue;
            }
            follow(
                &b,
                state,
                &verb.command_as("kanspec", &id),
                &mut Vec::new(),
                &mut proven,
            );
        }
    }
    // Every root, plus everything they suggest, actually ran.
    assert!(
        proven.len() >= ALL_STATES.len() * (ALL_VERBS.len() - 1),
        "the walk explored only {} nodes",
        proven.len()
    );
    // ...and it really did FOLLOW, rather than quietly filtering every suggestion away and
    // passing on an empty graph. Neither of these is a verb, so the only way the walk can
    // have run them is that a refusal pointed at them.
    for (state, cmd) in [
        (Todo, format!("kanspec show {}", walk_id(Todo))),
        (
            Dropped,
            V::New.command_as("kanspec", &kanspec::ids::TicketId::parse("t-0c01").unwrap()),
        ),
    ] {
        assert!(
            proven.contains(&(state.as_str(), cmd.clone())),
            "no {state} refusal ever sent the walk to `{cmd}`"
        );
    }

    // ...and terminating is the floor, not the goal. What a closed ticket is told to do has
    // to WORK on the spot — look at it, or open a new ticket — which is the difference
    // between a chain that ends and a chain that gives up.
    let id = kanspec::ids::TicketId::parse(b.seed(Dropped)).unwrap();
    for fix in require(&id, Dropped, V::Start).unwrap_err().fixes().iter() {
        assert!(
            matches!(b.run(fix.as_str()), Said::Ok),
            "a dropped ticket is told `{fix}`, and it does not even run"
        );
    }
}

/// The end of the chain, spelled out for the two states a ticket cannot legally leave.
///
/// A `done` or `dropped` ticket has nowhere to go: the table allows only `confirm` and
/// `repair`, and both of those answer a question about work that is over. So the refusal
/// says the true, final thing — look at it, or open a new ticket — rather than naming a
/// verb that will bounce straight back here.
#[test]
fn a_terminal_ticket_is_told_something_final_rather_than_sent_round_again() {
    let id = kanspec::ids::TicketId::parse("t-ea32").unwrap();
    for terminal in [Done, Dropped] {
        for &verb in ALL_VERBS {
            if next(Some(terminal), verb).is_some() {
                continue;
            }
            let e = require(&id, terminal, verb).unwrap_err();
            let fixes: Vec<&str> = e.fixes().iter().map(|f| f.as_str()).collect();
            assert_eq!(
                fixes,
                ["kanspec show t-ea32", "kanspec new \"...\""],
                "`{verb}` from {terminal}"
            );
            for f in &fixes {
                must_run(f, &format!("`{verb}` from {terminal}"));
            }
        }
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
