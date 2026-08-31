//! `tests/proof_is_sealed.rs`
//!
//! Proves: nothing but a real ladder run can produce a `MergedProof`, so `kanspec done`
//! **cannot compile** without git-derived truth.
//!
//! The seal has three layers, and this file guards all three:
//!
//! 1. **The compiler.** `MergedProof` has private fields, no `Default`, no `Deserialize`
//!    and no `From`, so `plan_done(.., Landed, ..)` is unreachable without one of the two
//!    sanctioned mints. That half is demonstrated where it lives — as `compile_fail`
//!    doctests on the type itself in `src/scan.rs`, run by `cargo test --doc`. This file
//!    asserts those doctests still exist, because a deleted proof proves nothing.
//! 2. **A source grep** (R-3, stated out loud): Rust cannot express "no `impl Deserialize`
//!    for this type, ever", and adding one *will* be tempting the first time someone wants
//!    a fast `status`. A grep raises the cost of that from zero to "you had to edit a test
//!    named after the invariant". It is a deterrent, not a guarantee — a `use serde as s;`
//!    alias walks past it.
//! 3. **Behaviour.** The strongest of the three, and the only one that holds regardless of
//!    how a derived fact got somewhere: a hand-forged `cache/gitstate.json` claiming a merge
//!    changes nothing at the gate, because the gate re-runs the ladder and never reads the
//!    cache.
//!
//! Owner: **S3**.

mod common;

use std::path::{Path, PathBuf};

use common::merges::Shape;
use common::{ctx_at, TestRepo};

use kanspec::cache::MergeFact;
use kanspec::ids::TicketId;
use kanspec::scan;

/// Traits that would each open a non-ladder route to a `MergedProof`.
const FORBIDDEN: &[&str] = &[
    "Deserialize",
    "Default",
    "From",
    "TryFrom",
    "FromStr",
    "Clone", // only via derive on the struct itself, which is checked separately
];

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read(rel: &str) -> String {
    let p = src_dir().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("src/ is readable").flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// The `pub struct MergedProof { … }` block, plus the attribute lines directly above it.
fn struct_block(scan: &str) -> (String, String) {
    let at = scan
        .find("pub struct MergedProof {")
        .expect("scan.rs must still define MergedProof");
    let end = scan[at..].find("\n}").expect("an unterminated struct") + at;
    let head = scan[..at]
        .rsplit("///")
        .next()
        .unwrap_or_default()
        .to_string();
    // The fields only — the `pub struct` line itself is not a public field.
    let fields = scan[at..end]
        .split_once('\n')
        .map(|(_, f)| f.to_string())
        .unwrap_or_default();
    (head, fields)
}

// ─────────────────────────────────────────────────────────────────────────────
// layer 2 — the grep
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn merged_proof_derives_nothing_that_could_conjure_one() {
    let scan = read("scan.rs");
    let (attrs, block) = struct_block(&scan);
    let derive = attrs
        .lines()
        .find(|l| l.trim_start().starts_with("#[derive("))
        .unwrap_or("");
    for bad in FORBIDDEN {
        if *bad == "Clone" {
            continue; // cloning a proof you already hold conjures nothing
        }
        assert!(
            !derive.contains(bad),
            "MergedProof derives {bad}, which is a route to a proof nobody earned: {derive}"
        );
    }
    // Serialize is not only allowed but required — `done --json` prints the proof.
    assert!(derive.contains("Serialize"), "{derive}");
    assert!(
        !block.contains("pub "),
        "every field of MergedProof must be private, or the struct literal is a public \
         constructor:\n{block}"
    );
}

#[test]
fn nothing_in_the_crate_implements_a_forbidden_trait_for_merged_proof() {
    let mut files = Vec::new();
    rust_files(&src_dir(), &mut files);
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if !line.starts_with("impl") || !line.contains("for MergedProof") {
                continue;
            }
            let (head, _) = line.split_once("for MergedProof").unwrap();
            for bad in FORBIDDEN {
                assert!(
                    !head.contains(bad),
                    "{}:{}: `{line}` — a {bad} impl makes a proof forgeable, and \
                     `kanspec done` would compile without ever consulting git",
                    f.display(),
                    n + 1
                );
            }
        }
    }
}

/// Two mints, both in `scan.rs`, both downstream of a real ladder run: `from_detection`
/// (a `Detection` only `ladder` can build) and `confirmed_proof` (a `Verb::Confirm` line a
/// human signed, re-resolved through git). There is no third.
#[test]
fn a_merged_proof_is_constructed_in_exactly_two_places_and_both_are_in_scan_rs() {
    let scan = read("scan.rs");
    let literals = scan
        .match_indices("MergedProof {")
        .filter(|(i, _)| !scan[..*i].ends_with("struct ") && !scan[..*i].ends_with("impl "))
        .count();
    assert_eq!(
        literals, 2,
        "expected exactly two mints (from_detection, confirmed_proof); found {literals}"
    );
    assert!(scan.contains("fn from_detection("));
    assert!(scan.contains("pub fn confirmed_proof("));

    let mut files = Vec::new();
    rust_files(&src_dir(), &mut files);
    for f in files {
        if f.ends_with("scan.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(
            !text.contains("MergedProof {"),
            "{} builds a MergedProof outside scan.rs",
            f.display()
        );
    }
}

/// The same seal, one layer out: `gitstate.json` is written by `scan` and nothing else,
/// because `Op::WriteGitState` demands a `ScanToken` and only `scan_all` mints one.
#[test]
fn the_scan_token_is_minted_only_by_scan_rs() {
    let mut files = Vec::new();
    rust_files(&src_dir(), &mut files);
    for f in files {
        if f.ends_with("scan.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(
            !text.contains("ScanToken("),
            "{} constructs a ScanToken; the cache would stop having a single writer",
            f.display()
        );
    }
    let scan = read("scan.rs");
    let mints = scan
        .match_indices("ScanToken(())")
        .filter(|(i, _)| !scan[..*i].ends_with("struct "))
        .count();
    assert_eq!(mints, 1, "one mint, in scan_all_detailed");
}

/// Layer 1 lives in `src/scan.rs` as doctests so it is checked by the compiler itself.
/// Deleting them would delete the only mechanical half of this proof.
#[test]
fn the_compile_fail_doctests_that_prove_the_seal_still_exist() {
    let scan = read("scan.rs");
    assert_eq!(
        scan.matches("```compile_fail").count(),
        3,
        "expected the Deserialize, Default and private-field compile-fail doctests"
    );
    for probe in [
        "de::<kanspec::scan::MergedProof>();",
        "dflt::<kanspec::scan::MergedProof>();",
        "&p.sha }",
    ] {
        assert!(scan.contains(probe), "the doctest for {probe:?} is gone");
    }
    // …and each one has a positive control, so it cannot pass by failing to resolve a path.
    assert!(scan.contains("de::<kanspec::cache::MergeFact>();"));
    assert!(scan.contains("dflt::<kanspec::cache::GitState>();"));
    assert!(scan.contains("{ p.sha() }"));
}

/// The honest half of J-8, asserted at runtime: the CACHE row really is a plain
/// deserializable DTO. That is what makes the absences above meaningful rather than an
/// accident of a compiler that cannot see `serde`.
#[test]
fn the_badge_grade_cache_row_is_deserializable_precisely_because_it_is_not_a_proof() {
    let f: MergeFact = serde_json::from_str(
        r#"{"status":"merged","sha":"a1b9c3d","method":"gh_pr","pr":142,
            "why":null,"checked_at":"2026-08-31T12:00:00Z","changed":[]}"#,
    )
    .expect("the cache DTO is Deserialize by design — a text editor can write one");
    assert_eq!(f.status, kanspec::cache::MergeStatus::Merged);
}

// ─────────────────────────────────────────────────────────────────────────────
// layer 3 — behaviour, the only layer no alias can walk past
// ─────────────────────────────────────────────────────────────────────────────

/// A hand-forged cache is the exact attack the split exists to defeat: an agent that can
/// write files can write `"status":"merged"` into `gitstate.json`. The gate never reads it.
#[test]
fn a_forged_cache_claiming_a_merge_does_not_move_the_gate() {
    let repo = TestRepo::with_merges();
    let id = Shape::Never.ticket();
    repo.ks(["scan"]).ok();

    // Forge every ticket into "merged", by hand, exactly as an agent would.
    let path = repo.root.join(".kanspec/cache/gitstate.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let forged = text.replace("\"unknown\"", "\"merged\"");
    assert_ne!(forged, text, "the fixture must contain an unknown to forge");
    std::fs::write(&path, &forged).unwrap();

    // The badge believes it — that is what "badge-grade" means, and why the badge is not
    // the gate.
    let state: kanspec::cache::GitState = serde_json::from_str(&forged).unwrap();
    assert_eq!(
        state.tickets[&TicketId::parse(id).unwrap()].status,
        kanspec::cache::MergeStatus::Merged
    );

    // The gate does not, because it re-runs the ladder against real git.
    let ctx = ctx_at(&repo.root);
    let snap = ctx.snapshot().unwrap();
    let t = snap.ticket(&TicketId::parse(id).unwrap()).unwrap();
    let err = scan::proof_for_done(&ctx, t)
        .expect_err("a forged cache row must not be able to close a ticket");
    assert_eq!(err.code(), Some("not_landed"));

    // And the next scan overwrites the forgery with the truth.
    repo.ks(["scan"]).ok();
    let state: kanspec::cache::GitState =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        state.tickets[&TicketId::parse(id).unwrap()].status,
        kanspec::cache::MergeStatus::Unknown
    );
}

/// The other half of the same story: a `merged: true` key typed into a TICKET is not a
/// derived fact either. `keys.rs` has no variant for it, so it has no write path at all —
/// and the loader keeps it in `extra` (where `doctor` finds it) rather than believing it.
#[test]
fn a_merged_key_typed_into_a_ticket_reaches_nothing_that_decides() {
    let repo = TestRepo::with_merges();
    let id = Shape::Never.ticket();
    let path = format!(".kanspec/tickets/{id}.md");
    let body = repo
        .read(&path)
        .replace("state: review", "state: review\nmerged: true");
    repo.write(&path, &body);

    let ctx = ctx_at(&repo.root);
    let snap = ctx.snapshot().unwrap();
    let t = snap.ticket(&TicketId::parse(id).unwrap()).unwrap();
    assert!(
        t.fm.extra.contains_key("merged"),
        "an unknown key is parked in `extra`, never merged into the model"
    );
    assert_eq!(
        scan::proof_for_done(&ctx, t).err().and_then(|e| e.code()),
        Some("not_landed"),
        "a ticket cannot assert its own merge state"
    );
}
