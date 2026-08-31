//! `tests/single_write_path.rs`
//!
//! Proves: source grep — no fs mutation outside `store.rs` + the 2-file allowlist
//!
//! Module privacy gets 90% of "one write path"; Rust cannot forbid `std::fs` crate-wide,
//! so this is the last 10%, and it is named after the invariant rather than pretended away
//! (R-3). A `use std::fs as f;` alias walks straight past it. That is the honest cost: this
//! raises the price of a violation from zero to "you had to work around a test named after
//! the invariant" — a deterrent, not a guarantee.
//!
//! Owner: **S1**.

use std::path::{Path, PathBuf};

/// The only files besides `store.rs` allowed to mutate the filesystem under `.kanspec/`.
/// Its LENGTH is asserted, so the list cannot grow silently in a review that only reads
/// the diff of the file being added.
const ALLOWLIST: &[&str] = &[
    // creates the very lockfile it then flocks
    "lock.rs",
    // scaffolds `.kanspec/` before a store can exist
    "cmd/init.rs",
];

/// Files that PLAN edits outside `.kanspec/` — git hooks and the agent snippet in
/// CLAUDE.md. They are held to a second rule on top of the first: they contain no mutator
/// (the grep above covers them like everything else, and they pass it by planning a typed
/// `hooks::Edit` list that the allowlisted `cmd::init::apply` executes), and on top of that
/// they may never name a path under `.kanspec/` — that ground is `store.rs`'s alone.
///
/// `project.rs` is deliberately NOT here: it *generates* `KANSPEC-*.md`, but it hands the
/// bytes to `Op::WriteGenerated`, so it is not a writer at all.
const OUTSIDE_WRITERS: &[&str] = &["hooks.rs", "setup.rs"];

/// Every mutating shape worth grepping for. Deliberately syntactic: the point is that a
/// reviewer sees one of these tokens in a diff and stops.
const MUTATORS: &[&str] = &[
    "fs::write",
    "fs::rename",
    "File::create",
    "OpenOptions",
    "fs::remove",
    "fs::create_dir",
    "fs::copy",
    "fs::set_permissions",
    "fs::hard_link",
];

fn src_files() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out.sort();
    assert!(out.len() > 30, "the source tree shrank: {}", out.len());
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, std::fs::read_to_string(&p).unwrap()));
        }
    }
}

/// Strips `#[cfg(test)] mod tests { … }` — a unit test writing a fixture into a temp dir is
/// not a second write path into anyone's repository.
fn without_tests(src: &str) -> String {
    let Some(at) = src.find("#[cfg(test)]") else {
        return src.to_string();
    };
    src[..at].to_string()
}

#[test]
fn nothing_outside_store_rs_mutates_the_filesystem() {
    let mut violations: Vec<String> = Vec::new();
    for (rel, src) in src_files() {
        if rel == "store.rs" || ALLOWLIST.contains(&rel.as_str()) {
            continue;
        }
        let body = without_tests(&src);
        for (i, line) in body.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") || line.starts_with("///") || line.starts_with("//!") {
                continue;
            }
            for m in MUTATORS {
                if line.contains(m) {
                    violations.push(format!("{rel}:{}: {line}", i + 1));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "`Store::transact` is the only public mutator in the crate, and the four primitives \
         in store.rs are the only functions that move a byte. These do not go through it:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn the_allowlist_cannot_grow_silently() {
    assert_eq!(
        ALLOWLIST.len(),
        2,
        "the allowlist is exactly `lock.rs` and `cmd/init.rs`. Adding a third entry is a \
         change to the single-write-path invariant, not a change to a test."
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for f in ALLOWLIST.iter().chain(OUTSIDE_WRITERS) {
        assert!(
            root.join(f).is_file(),
            "the allowlist names a missing file: {f}"
        );
    }
}

#[test]
fn the_files_that_write_outside_kanspec_never_name_a_path_inside_it() {
    let mut violations: Vec<String> = Vec::new();
    for (rel, src) in src_files() {
        if !OUTSIDE_WRITERS.contains(&rel.as_str()) {
            continue;
        }
        for (i, line) in without_tests(&src).lines().enumerate() {
            let t = line.trim();
            if t.starts_with("//") || t.starts_with("///") || t.starts_with("//!") {
                continue;
            }
            if t.contains(".kanspec") {
                violations.push(format!("{rel}:{}: {t}", i + 1));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "these files plan edits outside the store, so they must never name a path under \
         `.kanspec/` — that is `store.rs`'s ground:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn every_write_primitive_demands_the_lock_token_by_type() {
    let store = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store.rs"))
        .unwrap();
    for f in ["write_atomic", "append_line", "move_dir", "create_new"] {
        let sig = store
            .lines()
            .find(|l| l.contains(&format!("fn {f}(")))
            .unwrap_or_else(|| panic!("store.rs no longer defines {f}"));
        assert!(
            sig.contains("pub(crate)"),
            "{f} must not be reachable from outside the crate: {sig}"
        );
        assert!(
            sig.contains("&LockToken"),
            "{f} must take a `&LockToken`, so \"the lock is held\" is a borrow-checker \
             fact at the call site rather than a convention: {sig}"
        );
    }
}

#[test]
fn the_only_public_mutator_is_store_transact() {
    // A second public entry point into writing would make the invariant a matter of
    // discipline again. `Store::transact` is the one, and `Store` has no other `pub fn`
    // that takes `&mut` anything.
    let store = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/store.rs"))
        .unwrap();
    let public_fns: Vec<&str> = store
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("pub fn "))
        .collect();
    assert_eq!(
        public_fns.iter().filter(|l| l.contains("transact")).count(),
        1
    );
    for l in &public_fns {
        assert!(
            l.contains("transact") || l.contains("load_snapshot") || l.contains("open"),
            "store.rs grew a public function that is neither the write path nor the read \
             path: {l}"
        );
    }
}

#[test]
fn the_test_harness_itself_is_not_a_second_write_path() {
    // The harness writes plenty — into temp dirs it created. What it must never do is
    // reach into a repository it did not build.
    let harness: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/common/mod.rs");
    let src = std::fs::read_to_string(&harness).unwrap();
    assert!(
        src.contains("tempfile::tempdir"),
        "every fixture repo must live in a TempDir"
    );
    assert!(
        !src.contains("std::env::current_dir"),
        "the harness must never operate on the developer's own checkout"
    );
}
