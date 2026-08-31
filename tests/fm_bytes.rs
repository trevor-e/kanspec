//! `tests/fm_bytes.rs`
//!
//! Proves: byte-stability, 100-edit idempotence, CRLF, the adversarial frontmatter corpus
//!
//! The property under test is the one the whole write path rests on: **kanspec never
//! re-emits a byte it was not asked to change.** A tool that reformats a human's ticket on
//! every `ship` is a tool whose diffs nobody reads, and a diff nobody reads is a gate
//! nobody trusts.
//!
//! Owner: **S1**.

use std::path::{Path, PathBuf};

use kanspec::fm::{self, SetOutcome, Yv};
use kanspec::keys::TICKET_ORDER;

fn corpus() -> Vec<(PathBuf, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/frontmatter");
    let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("no corpus at {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter(|p| p.file_name().is_some_and(|n| n != "README.md"))
        .map(|p| {
            // read as BYTES then decode, so a CRLF fixture cannot be silently normalised
            // by anything on the way in.
            let bytes = std::fs::read(&p).unwrap();
            (p, String::from_utf8(bytes).expect("the corpus is UTF-8"))
        })
        .collect();
    out.sort();
    assert!(out.len() >= 8, "the corpus shrank: {} files", out.len());
    out
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn splitting_and_re_rendering_every_fixture_is_the_identity() {
    for (p, src) in corpus() {
        let doc = fm::split(&src).unwrap_or_else(|e| panic!("{}: {e}", name(&p)));
        assert_eq!(
            doc.render(),
            src,
            "{} did not survive split -> render",
            name(&p)
        );
        // The four pieces really do partition the file — no overlap, no gap.
        assert_eq!(
            doc.open.len() + doc.fm.len() + doc.close.len() + doc.body.len(),
            src.len(),
            "{}",
            name(&p)
        );
    }
}

#[test]
fn every_fixture_is_writable_and_the_indexer_agrees_with_the_parser() {
    for (p, src) in corpus() {
        let doc = fm::split(&src).unwrap();
        fm::writable(&doc.fm)
            .unwrap_or_else(|e| panic!("{} must be writable in place: {e}", name(&p)));

        // Every key the YAML parser sees is a key `set` can address; that equivalence is
        // exactly what `writable` promises, so assert it from the other side too.
        let parsed: serde_yaml_ng::Mapping =
            serde_yaml_ng::from_str(&doc.fm).unwrap_or_else(|e| panic!("{}: {e}", name(&p)));
        for key in parsed.keys().filter_map(|k| k.as_str()) {
            assert!(
                fm::index(&doc.fm).iter().any(|k| k.key == key),
                "{}: the indexer cannot see `{key}`",
                name(&p)
            );
        }
    }
}

#[test]
fn one_hundred_round_tripping_edits_reproduce_every_ticket_byte_identically() {
    for (p, src) in corpus() {
        let doc0 = fm::split(&src).unwrap();
        // Only tickets carry the keys a flow verb writes.
        let keys: Vec<String> = fm::index(&doc0.fm).into_iter().map(|k| k.key).collect();
        if !keys.iter().any(|k| k == "state") {
            continue;
        }
        let original: Vec<(String, String)> = keys
            .iter()
            .filter(|k| ["state", "pr", "head"].contains(&k.as_str()))
            .map(|k| {
                let span = fm::index(&doc0.fm)
                    .into_iter()
                    .find(|s| &s.key == k)
                    .unwrap();
                (k.clone(), doc0.fm[span.val.0..span.val.1].to_string())
            })
            .collect();

        // Only keys the file ALREADY has: inserting a brand-new key is a real change, and
        // the property under test is that changing a value and changing it back is not.
        let mut doc = doc0.clone();
        for i in 0..100 {
            for (k, _) in &original {
                let churn = match k.as_str() {
                    "pr" => Yv::Int(i),
                    "head" => Yv::s("a1b9c3d"),
                    _ => Yv::s("review"),
                };
                fm::set(&mut doc, k, &churn, TICKET_ORDER);
            }
            // …and back to exactly what the file said to begin with.
            for (k, v) in &original {
                fm::set(&mut doc, k, &raw_value(v), TICKET_ORDER);
            }
        }
        assert!(!original.is_empty(), "{}: nothing to churn", name(&p));
        assert_eq!(
            doc.render(),
            src,
            "{}: 100 round trips must be the identity",
            name(&p)
        );
    }
}

/// Turn the literal text a fixture holds back into the `Yv` that re-emits it verbatim.
fn raw_value(text: &str) -> Yv {
    match text {
        "null" | "~" | "" => Yv::Null,
        "true" => Yv::Bool(true),
        "false" => Yv::Bool(false),
        t => match t.parse::<i64>() {
            Ok(n) => Yv::Int(n),
            Err(_) => Yv::s(t.trim_matches('\'').trim_matches('"')),
        },
    }
}

#[test]
fn a_ship_changes_exactly_the_lines_it_names_and_nothing_else() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/frontmatter/ticket-annotated.md"),
    )
    .unwrap();
    let mut doc = fm::split(&src).unwrap();
    assert_eq!(
        fm::set(&mut doc, "state", &Yv::s("review"), TICKET_ORDER),
        SetOutcome::Replaced
    );
    assert_eq!(
        fm::set(&mut doc, "pr", &Yv::Int(142), TICKET_ORDER),
        SetOutcome::Replaced
    );
    assert_eq!(
        fm::set(&mut doc, "head", &Yv::s("a1b9c3d"), TICKET_ORDER),
        SetOutcome::Replaced
    );
    fm::append_to_section(
        &mut doc,
        "## Log",
        "- 2026-08-31T10:14Z  review   claude/sess-a91       ship (pr 142)",
    );
    let after = doc.render();

    let before_lines: Vec<&str> = src.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();
    assert_eq!(
        after_lines.len(),
        before_lines.len() + 1,
        "one appended log line, and nothing else"
    );
    let changed: Vec<&str> = before_lines
        .iter()
        .zip(&after_lines)
        .filter(|(a, b)| a != b)
        .map(|(_, b)| *b)
        .collect();
    assert_eq!(
        changed.len(),
        3,
        "expected 3 changed lines, got {changed:?}"
    );
    assert!(changed[0].starts_with("state: review"));
    assert!(changed[1].starts_with("pr: 142"));
    assert!(changed[2].starts_with("head: a1b9c3d"));

    // Everything a full re-serialize would have destroyed.
    assert!(
        after.contains("state: review              # todo | doing | review | done | dropped"),
        "the inline comment survives, padding and all"
    );
    assert!(after.contains("\n\n# set by kanspec start\n"));
    assert!(after.contains("estimate: \"S\""), "deliberate quoting kept");
    assert!(after.contains("deps: [t-31aa, t-8812]"), "flow style kept");
    assert!(
        after.contains("severity_hint_v2: high    # a key a NEWER kanspec wrote; never drop it"),
        "a key a newer kanspec wrote is never dropped, comment and all"
    );
    assert!(after.contains("- [x] lockout counter in Redis, sliding window"));
    assert!(after.ends_with("ship (pr 142)\n"));
}

#[test]
fn crlf_files_stay_crlf_and_lf_files_never_grow_a_carriage_return() {
    for (p, src) in corpus() {
        let crlf = src.contains("\r\n");
        let mut doc = fm::split(&src).unwrap();
        fm::set(&mut doc, "state", &Yv::s("review"), TICKET_ORDER);
        fm::set(&mut doc, "pr", &Yv::Int(142), TICKET_ORDER);
        fm::append_to_section(&mut doc, "## Log", "- appended");
        let out = doc.render();
        if crlf {
            assert_eq!(
                out.matches('\n').count(),
                out.matches("\r\n").count(),
                "{}: a CRLF file grew a bare LF",
                name(&p)
            );
        } else {
            assert!(!out.contains('\r'), "{}: an LF file grew a CR", name(&p));
        }
    }
}

#[test]
fn a_body_level_horizontal_rule_is_not_a_fence() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/frontmatter/ticket-unordered-body-rule.md"),
    )
    .unwrap();
    let doc = fm::split(&src).unwrap();
    assert!(doc.fm.contains("id: t-31aa"));
    assert!(
        doc.body.contains("\n---\n"),
        "the rule belongs to the body: {:?}",
        doc.body
    );
    // The author's key order is theirs. `set` must not reorder anything that is present.
    let keys: Vec<String> = fm::index(&doc.fm).into_iter().map(|k| k.key).collect();
    assert_eq!(keys, ["title", "created", "state", "id", "deps"]);
    let mut doc = doc;
    fm::set(&mut doc, "state", &Yv::s("doing"), TICKET_ORDER);
    let after: Vec<String> = fm::index(&doc.fm).into_iter().map(|k| k.key).collect();
    assert_eq!(after, keys, "an edit must never reorder the file");
}

#[test]
fn a_file_with_a_bom_and_no_trailing_newline_survives_an_append() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/frontmatter/ticket-bom-no-trailing-newline.md"),
    )
    .unwrap();
    assert!(src.starts_with('\u{feff}') && !src.ends_with('\n'));
    let mut doc = fm::split(&src).unwrap();
    assert!(doc.open.starts_with('\u{feff}'), "the BOM lives in `open`");
    fm::append_to_section(
        &mut doc,
        "## Log",
        "- 2026-08-31T10:14Z  doing    trevor                start",
    );
    let out = doc.render();
    assert!(out.starts_with('\u{feff}'));
    assert!(out.ends_with("start\n"));
    // The pre-existing last line must not have been welded to the new one.
    assert!(out.contains("new\n- 2026-08-31T10:14Z"), "{out:?}");
}

#[test]
fn marking_a_step_moves_exactly_one_byte() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/frontmatter/ticket-annotated.md"),
    )
    .unwrap();
    let mut doc = fm::split(&src).unwrap();
    assert_eq!(fm::steps(&doc.body).len(), 3);
    assert!(fm::mark_step(&mut doc, 2, true));
    let out = doc.render();
    assert_eq!(out.len(), src.len(), "a checkbox flip is one byte");
    let diff: Vec<usize> = src
        .bytes()
        .zip(out.bytes())
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(diff.len(), 1, "exactly one byte changed: {diff:?}");
    assert!(out.contains("- [x] 429 + Retry-After on lock"));
}
