//! p-97d6 c5, end to end through real git: two branches that each changed a projection's
//! sources merge **without a conflict in either generated file**, and the merge commit
//! carries the projections regenerated from the MERGED store — byte-identical to a fresh
//! regeneration, with nothing left dirty.
//!
//! Two pieces make that true, and the test would fail without either: the
//! `merge.kanspec.driver` git config (`init` writes it; the `.gitattributes` lines route
//! both projections to it) keeps git from stopping on the table, and the `post-merge` hook
//! regenerates from the merged store and folds the result into the merge commit git just
//! made. The hooks and the driver resolve the binary through `KANSPEC_BIN`, exactly as a
//! cargo target dir would.

mod common;

use std::path::Path;
use std::process::Command;

use common::TestRepo;

/// `git -C <dir> …` with the hooks and the merge driver pointed at the binary under test.
fn git_hooked(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("KANSPEC_BIN", env!("CARGO_BIN_EXE_kanspec"))
        .env("KANSPEC_NOW", common::NOW)
        .env("KANSPEC_ACTOR", common::ACTOR)
        .env("GIT_AUTHOR_NAME", "kanspec test")
        .env("GIT_AUTHOR_EMAIL", "test@kanspec.invalid")
        .env("GIT_COMMITTER_NAME", "kanspec test")
        .env("GIT_COMMITTER_EMAIL", "test@kanspec.invalid")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[track_caller]
fn git_ok(dir: &Path, args: &[&str]) -> String {
    let (code, out, err) = git_hooked(dir, args);
    assert_eq!(code, 0, "git {} failed:\n{out}\n{err}", args.join(" "));
    out
}

/// A landmine changes `KANSPEC-ARCHITECTURE.md`'s landmine section AND its `(N)` count —
/// the shape a plain text merge cannot resolve when two branches each add one.
fn add_landmine(repo: &TestRepo, title: &str, glob: &str) {
    repo.ks(["quirk", "add", title, "--paths", glob, "--sev", "landmine"])
        .ok();
}

#[test]
fn two_branches_that_each_closed_a_ticket_merge_without_a_projection_conflict() {
    let repo = TestRepo::new();
    // `init` over the harness's store: idempotent on the files, and it installs the hooks,
    // the driver config and the `merge=kanspec` attribute lines this test is about.
    repo.ks(["init"]).ok();
    let attrs = repo.read(".gitattributes");
    assert!(
        attrs.contains("/KANSPEC-ARCHITECTURE.md merge=kanspec"),
        "{attrs}"
    );
    let driver = repo.git(&["config", "--local", "--get", "merge.kanspec.driver"]);
    assert!(driver.contains("merge-driver %O %A %B %P"), "{driver}");

    // A baseline with one landmine, so both branches EDIT the section rather than create it.
    add_landmine(
        &repo,
        "Baseline: staging replays webhooks",
        "src/billing/**",
    );
    git_ok(&repo.root, &["add", "-A"]);
    git_ok(&repo.root, &["commit", "--quiet", "-m", "baseline"]);

    // Branch A adds one landmine; branch B, from the same base, adds another.
    git_ok(&repo.root, &["checkout", "-q", "-b", "a"]);
    add_landmine(&repo, "A: the auth cache is per process", "src/auth/**");
    git_ok(&repo.root, &["add", "-A"]);
    git_ok(&repo.root, &["commit", "--quiet", "-m", "a"]);

    git_ok(&repo.root, &["checkout", "-q", "main"]);
    git_ok(&repo.root, &["checkout", "-q", "-b", "b"]);
    add_landmine(
        &repo,
        "B: charge retries are not idempotent",
        "src/billing/**",
    );
    git_ok(&repo.root, &["add", "-A"]);
    git_ok(&repo.root, &["commit", "--quiet", "-m", "b"]);

    // Control: without the driver this IS a conflict — both branches changed the same
    // `## Landmines (N)` line and inserted adjacent bullets. Proved by asking git for a
    // plain text merge of the three versions.
    let base = repo.git(&["show", "main:KANSPEC-ARCHITECTURE.md"]);
    let ours = repo.git(&["show", "a:KANSPEC-ARCHITECTURE.md"]);
    let theirs = repo.git(&["show", "b:KANSPEC-ARCHITECTURE.md"]);
    let tmp = repo.root.join(".kanspec/cache");
    std::fs::create_dir_all(&tmp).unwrap();
    for (name, text) in [("base", &base), ("ours", &ours), ("theirs", &theirs)] {
        std::fs::write(tmp.join(name), text).unwrap();
    }
    let (plain, _, _) = repo.git_try(&[
        "merge-file",
        "-p",
        ".kanspec/cache/ours",
        ".kanspec/cache/base",
        ".kanspec/cache/theirs",
    ]);
    assert!(
        plain > 0,
        "the fixture must be a real conflict for the driver to have anything to retire"
    );

    // The merge itself: no conflict, one commit, and the driver + hook did their halves.
    git_ok(&repo.root, &["checkout", "-q", "a"]);
    let (code, out, err) = git_hooked(&repo.root, &["merge", "--no-edit", "b"]);
    assert_eq!(code, 0, "the merge stopped:\n{out}\n{err}");
    assert!(
        !repo.read("KANSPEC-ARCHITECTURE.md").contains("<<<<<<<"),
        "a generated file reached the tree wearing conflict markers"
    );

    // The merge commit carries BOTH landmines, counted, exactly as a fresh regeneration
    // renders them — and the tree is clean, because the `post-merge` hook folded the
    // regeneration into the commit rather than leaving it for a `kanspec: sync` commit.
    let committed = repo.git(&["show", "HEAD:KANSPEC-ARCHITECTURE.md"]);
    assert!(committed.contains("## Landmines (3)"), "{committed}");
    assert!(
        committed.contains("A: the auth cache is per process"),
        "{committed}"
    );
    assert!(
        committed.contains("B: charge retries are not idempotent"),
        "{committed}"
    );
    let status = repo.git(&[
        "status",
        "--porcelain",
        "--",
        "KANSPEC-ARCHITECTURE.md",
        "KANSPEC-FEATURES.md",
    ]);
    assert_eq!(
        status.trim(),
        "",
        "the merge commit left a projection dirty:\n{status}"
    );

    repo.ks(["regenerate"]).ok();
    assert_eq!(
        repo.read("KANSPEC-ARCHITECTURE.md"),
        committed,
        "the committed projection is not what the merged store regenerates to"
    );

    // The amend is guarded: a fast-forward creates no commit of ours, and the hook must
    // not rewrite the one it landed on — even though the tree it lands on is stale by
    // construction (branch `c` never regenerated after its quirk was hand-planted).
    git_ok(&repo.root, &["checkout", "-q", "-b", "c", "main"]);
    repo.write(
        ".kanspec/quirks/q-cafe.md",
        "---\nid: q-cafe\ntitle: C mine\npaths: [src/c/**]\nseverity: landmine\n\
         status: active\nsource: null\nfixed_by: null\n---\nC mine\n",
    );
    git_ok(&repo.root, &["add", "-A"]);
    git_ok(&repo.root, &["commit", "--quiet", "--no-verify", "-m", "c"]);
    let c_tip = repo.sha("c");
    git_ok(&repo.root, &["checkout", "-q", "main"]);
    let (code, out, err) = git_hooked(&repo.root, &["merge", "--ff-only", "c"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(
        repo.sha("HEAD"),
        c_tip,
        "a fast-forward is not a merge commit of ours to amend"
    );
    // ...the regeneration is staged for the user's next commit instead, and says so.
    let status = repo.git(&["status", "--porcelain", "--", "KANSPEC-ARCHITECTURE.md"]);
    assert!(status.starts_with("M "), "{status:?}");
}

/// The driver on its own, as git calls it: a clean three-way merge is written in place, a
/// conflicting one resolves to ours — and it exits 0 both times, because a non-zero exit
/// is how a driver reports a conflict and a generated file never gets to have one.
#[test]
fn the_driver_never_exits_non_zero_and_never_leaves_markers() {
    let repo = TestRepo::new();
    let dir = repo.root.join(".kanspec/cache");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, body: &str| {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p.to_string_lossy().into_owned()
    };

    // Clean: each side changed a different line.
    let base = write("base", "a\nb\nc\n");
    let ours = write("ours", "A\nb\nc\n");
    let theirs = write("theirs", "a\nb\nC\n");
    repo.ks(["merge-driver", &base, &ours, &theirs, "KANSPEC-FEATURES.md"])
        .ok();
    assert_eq!(std::fs::read_to_string(&ours).unwrap(), "A\nb\nC\n");

    // Conflicting: both sides changed the same line. Ours wins, no markers, exit 0.
    let base = write("base2", "a\nb\nc\n");
    let ours = write("ours2", "a\nOURS\nc\n");
    let theirs = write("theirs2", "a\nTHEIRS\nc\n");
    let r = repo.ks(["merge-driver", &base, &ours, &theirs, "KANSPEC-FEATURES.md"]);
    assert_eq!(r.code, 0, "{}\n{}", r.stdout, r.stderr);
    let after = std::fs::read_to_string(&ours).unwrap();
    assert_eq!(
        after, "a\nOURS\nc\n",
        "ours, exactly, with no conflict markers"
    );
    assert!(
        r.stdout.is_empty(),
        "git ignores a driver's stdout; ours stays silent"
    );
}
