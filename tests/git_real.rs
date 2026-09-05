//! `tests/git_real.rs`
//!
//! Proves: the git wrapper against REAL git — the recon matrix, reproduced. Every merge
//! shape is a real merge into a real bare `origin`; nothing here is mocked, because the
//! second implementation of git does not exist and a `trait GitBackend` would only let the
//! suite agree with itself.
//!
//! The two scenarios the wrapper is most likely to get wrong get their own tests: a
//! multi-commit **squash** (where ancestry is genuinely negative and `git cherry` reports
//! `+`, i.e. the answer that must NEVER be read as "not merged"), and a **deleted branch**
//! (where every remaining rung has to work off a bare SHA).
//!
//! Owner: **S2**.

mod common;

use std::path::Path;

use common::merges::{ExpectedStatus, Shape};
use common::TestRepo;
use kanspec::git::{Pathspec, Tri, Unknown};

/// `Tri` has no `PartialEq` (it carries an `Unknown` with a whole trace in it), so tests
/// name the shape they expect rather than comparing values.
fn yes<T>(t: Tri<T>, what: &str) -> T {
    match t {
        Tri::Yes(v) => v,
        Tri::No => panic!("{what}: expected Yes, got No"),
        Tri::Unknown(u) => panic!("{what}: expected Yes, got {}", u.badge()),
    }
}

fn is_no<T: std::fmt::Debug>(t: &Tri<T>, what: &str) {
    match t {
        Tri::No => {}
        Tri::Yes(v) => panic!("{what}: expected No, got Yes({v:?})"),
        Tri::Unknown(u) => panic!("{what}: expected No, got {}", u.badge()),
    }
}

fn unknown<T: std::fmt::Debug>(t: Tri<T>, what: &str) -> Unknown {
    match t {
        Tri::Unknown(u) => u,
        Tri::Yes(v) => panic!("{what}: expected Unknown, got Yes({v:?})"),
        Tri::No => panic!("{what}: expected Unknown, got No — a git failure is NEVER a No"),
    }
}

#[test]
fn the_recon_merge_matrix_reproduces_against_real_git() {
    let repo = TestRepo::with_merges();
    let ctx = common::ctx_at(&repo.root);
    let git = &ctx.git;
    let main = git
        .resolve_main(&ctx.cfg.main)
        .expect("origin/main resolves");
    assert_eq!(main, "origin/main");

    for (shape, id, expected) in common::merges::all_expectations() {
        let head = git
            .head_sha(shape.branch())
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let sha = head.sha().clone();

        // Guard 0: the object is present, so nothing below may answer `unknown` for the
        // wrong reason.
        assert!(git.object_exists(&sha), "{id}: head must be in the store");

        // Guard 0b's input. NOTE: 0 here does NOT mean "the branch contributed nothing" —
        // a true merge reads 0 too, because its commits are literally on main. See
        // `Shape::commits_ahead_of_main`.
        let ahead = yes(
            git.commits_ahead(&main, &sha),
            &format!("{id} commits_ahead"),
        );
        assert_eq!(ahead as usize, shape.commits_ahead_of_main(), "{id}");

        let ancestor = git.is_ancestor(&sha, &main);
        let cherry = yes(git.cherry(&main, &sha).into(), &format!("{id} cherry"));
        let trailer = yes(
            git.grep_trailer(&main, &kanspec::ids::TicketId::parse(&id).unwrap()),
            &format!("{id} trailer"),
        );
        let plus = cherry.iter().filter(|l| !l.upstream).count();
        let minus = cherry.iter().filter(|l| l.upstream).count();

        match shape {
            Shape::TrueMerge => {
                yes(ancestor, "true merge is an ancestor");
                assert_eq!(
                    cherry.len(),
                    0,
                    "a merged branch has no commits beyond main"
                );
                assert_eq!(trailer.len(), 2, "both trailers reached main");
            }
            Shape::SquashGitNative => {
                is_no(&ancestor, "a squash is genuinely NOT an ancestor");
                // The rung the design called "last resort for squashes" reports the exact
                // opposite for a multi-commit squash (D-3).
                assert_eq!((plus, minus), (2, 0), "{id}: cherry sees +2, not merged");
                // git's own squash body indents the trailer four spaces, which is why the
                // grep must not be anchored to the start of a line.
                assert_eq!(trailer.len(), 1, "{id}: the squash commit carries it");
            }
            Shape::SquashGhTitleOnly => {
                is_no(&ancestor, "a squash is genuinely NOT an ancestor");
                assert_eq!((plus, minus), (2, 0));
                assert_eq!(
                    trailer.len(),
                    0,
                    "{id}: a title-only squash message destroys the trailer"
                );
                // All four rungs decline. This is R-4, verified, not hypothesised.
            }
            Shape::Rebase => {
                is_no(&ancestor, "cherry-picked commits are not the same objects");
                assert_eq!((plus, minus), (0, 2), "{id}: every patch-id is upstream");
                assert_eq!(trailer.len(), 2);
            }
            Shape::SquashOneCommit => {
                is_no(&ancestor, "still a squash");
                assert_eq!((plus, minus), (0, 1), "{id}: one commit keeps its patch-id");
                assert_eq!(trailer.len(), 0, "{id}: title-only message");
            }
            Shape::Never => {
                is_no(&ancestor, "never merged");
                assert_eq!((plus, minus), (2, 0));
                assert_eq!(trailer.len(), 0);
            }
        }

        // The wrapper's job is to report evidence; only the ladder turns it into a
        // verdict. Assert here only what the evidence itself already settles.
        let evidence_says_merged = matches!(git.is_ancestor(&sha, &main), Tri::Yes(()))
            || (!cherry.is_empty() && plus == 0)
            || !trailer.is_empty();
        match expected {
            ExpectedStatus::Merged => assert!(evidence_says_merged, "{id} must be provable"),
            ExpectedStatus::Unknown => {
                assert!(!evidence_says_merged, "{id} must NOT look merged")
            }
        }
    }
}

#[test]
fn a_squash_merged_branch_survives_deletion_of_the_branch_itself() {
    let repo = TestRepo::with_merges();
    let shape = Shape::SquashGitNative;
    let head = repo.sha(shape.branch());

    // Delete the branch locally AND on origin — the state a repo lands in the moment a
    // squash-merged PR is merged with "delete branch" ticked.
    repo.git(&["push", "--quiet", "origin", "--delete", shape.branch()]);
    repo.git(&["branch", "-D", shape.branch()]);
    repo.git(&["fetch", "--quiet", "--prune", "origin"]);
    let (code, _, _) = repo.git_try(&["rev-parse", "--verify", "--quiet", shape.branch()]);
    assert_ne!(code, 0, "the branch really is gone");

    let ctx = common::ctx_at(&repo.root);
    let git = &ctx.git;
    let main = git.resolve_main(&ctx.cfg.main).unwrap();

    // `head:` is why the ticket records a SHA at ship time: the branch name is gone, but
    // every rung still has something to ask about.
    let sha = git.head_sha(&head).expect("the object outlives the ref");
    assert!(git.object_exists(sha.sha()));
    is_no(&git.is_ancestor(sha.sha(), &main), "still a squash");
    let cherry = yes(git.cherry(&main, sha.sha()).into(), "cherry on a bare SHA");
    assert_eq!(
        cherry.len(),
        2,
        "cherry works off a bare SHA, not just a ref"
    );
    let trailer = yes(
        git.grep_trailer(
            &main,
            &kanspec::ids::TicketId::parse(shape.ticket()).unwrap(),
        ),
        "trailer",
    );
    assert_eq!(
        trailer.len(),
        1,
        "the trailer lives on MAIN, so branch deletion cannot touch it"
    );
}

#[test]
fn a_head_that_is_no_longer_in_the_object_store_is_unknown_not_not_merged() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    let git = &ctx.git;

    // `head_sha` of a ref that does not exist is a typed error, not a silent None.
    let e = git.head_sha("origin/nonexistent").unwrap_err();
    assert_eq!(e.kind(), "git");
    assert!(e.fixes().iter().next().is_some(), "invariant 9");

    // A MISSING BASE is the same class of failure as a gc'd head: git could not answer.
    let head = git.head_sha("HEAD").unwrap();
    let u = unknown(
        git.is_ancestor(head.sha(), "origin/nonexistent"),
        "is_ancestor with a missing base",
    );
    match &u {
        Unknown::GitFailed { code, cmd, .. } => {
            assert_eq!(*code, 128, "git's `cannot answer` code");
            assert!(cmd.contains("merge-base --is-ancestor"), "{cmd}");
        }
        other => panic!("expected GitFailed, got {other:?}"),
    }
    assert!(u.badge().starts_with("unknown ("));

    unknown(
        git.commits_ahead("origin/nonexistent", head.sha()),
        "commits_ahead",
    );
    unknown(
        git.grep_trailer(
            "origin/nonexistent",
            &kanspec::ids::TicketId::parse("t-9c41").unwrap(),
        ),
        "grep_trailer",
    );
    unknown(
        git.cherry("origin/nonexistent", head.sha()).into(),
        "cherry",
    );
    unknown(
        git.changed_paths("origin/nonexistent", "HEAD"),
        "changed_paths",
    );
}

#[test]
fn a_branch_with_no_commits_of_its_own_is_caught_before_ancestry_can_lie() {
    let repo = TestRepo::new();
    repo.git(&["checkout", "--quiet", "-b", "ks/t-0000-empty", "main"]);
    repo.git(&["checkout", "--quiet", "main"]);
    let ctx = common::ctx_at(&repo.root);
    let git = &ctx.git;
    let main = git.resolve_main(&ctx.cfg.main).unwrap();
    let head = git.head_sha("ks/t-0000-empty").unwrap();

    // This is the VERIFIED false positive: a fresh `start` branch is trivially an ancestor
    // of main, so rung 1 alone would report MERGED for work that never happened.
    yes(git.is_ancestor(head.sha(), &main), "trivially an ancestor");
    assert_eq!(
        yes(git.commits_ahead(&main, head.sha()), "commits_ahead"),
        0,
        "the guard that makes the false positive unreachable"
    );
}

#[test]
fn changed_paths_uses_three_dots_and_keeps_renames_distinguishable() {
    let repo = TestRepo::new();
    repo.git(&["checkout", "--quiet", "-b", "ks/t-1111-work", "main"]);
    repo.git(&["mv", "src/billing/charge.ts", "src/billing/charges.ts"]);
    repo.write(
        "src/auth/new.ts",
        "// nothing like the file we deleted, so -M cannot pair them\nexport function freshlyAdded(): number {\n  return 42;\n}\n",
    );
    std::fs::remove_file(repo.root.join("src/auth/login.ts")).unwrap();
    repo.commit("branch work");
    // Main moves too — with a 2-dot range this leaks into the branch's own changes.
    repo.git(&["checkout", "--quiet", "main"]);
    repo.write("README.md", "# fixture, edited on main\n");
    repo.commit("main moves on");

    let ctx = common::ctx_at(&repo.root);
    let changed = yes(
        ctx.git.changed_paths("main", "ks/t-1111-work"),
        "changed_paths",
    );
    let mut got: Vec<(char, String, Option<String>)> = changed
        .iter()
        .map(|c| (c.status, c.path.clone(), c.renamed_from.clone()))
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ('A', "src/auth/new.ts".to_string(), None),
            ('D', "src/auth/login.ts".to_string(), None),
            (
                'R',
                "src/billing/charges.ts".to_string(),
                Some("src/billing/charge.ts".to_string())
            ),
        ],
        "3-dot keeps main's own edit out; -M keeps the rename whole"
    );
    assert!(
        !got.iter().any(|(_, p, _)| p == "README.md"),
        "a 2-dot range would leak main's edit into the branch's diff"
    );
}

#[test]
fn merges_touching_counts_merges_not_commits() {
    let repo = TestRepo::new();
    let since = repo.sha("main");

    // One PR, two commits, one first-parent merge.
    repo.git(&["checkout", "--quiet", "-b", "feature", "main"]);
    repo.write("src/auth/a.ts", "1\n");
    repo.commit("a");
    repo.write("src/auth/b.ts", "2\n");
    repo.commit("b");
    repo.git(&["checkout", "--quiet", "main"]);
    repo.git(&[
        "merge",
        "--quiet",
        "--no-ff",
        "-m",
        "Merge feature",
        "feature",
    ]);
    repo.push("main");
    repo.git(&["fetch", "--quiet", "origin"]);

    let ctx = common::ctx_at(&repo.root);
    let since = ctx.git.head_sha(&since).unwrap();
    let globs = [Pathspec::glob("src/auth/**")];
    assert_eq!(
        yes(
            ctx.git.merges_touching(since.sha(), "origin/main", &globs),
            "merges_touching"
        ),
        1,
        "`--first-parent`, or the tripwire over-fires by the size of every PR (D-9)"
    );
    // A glob that matches nothing is 0, not an error — that is what glob rot looks like.
    assert_eq!(
        yes(
            ctx.git
                .merges_touching(since.sha(), "origin/main", &[Pathspec::glob("src/gone/**")]),
            "dead glob"
        ),
        0
    );
    // And the anchor itself.
    let (sha, _at) = ctx
        .git
        .last_touch("origin/main", &Pathspec::glob("src/auth/**"))
        .expect("src/auth was touched");
    assert!(ctx.git.object_exists(&sha));
    assert!(
        ctx.git
            .last_touch("origin/main", &Pathspec::glob("src/never/**"))
            .is_none(),
        "no anchor is None, which is NOT the same as `0 merges since`"
    );
}

#[test]
fn a_pathspec_is_cwd_independent() {
    let repo = TestRepo::new();
    let since = repo.sha("main");
    repo.write("src/auth/x.ts", "1\n");
    repo.commit("touch auth");
    repo.push("main");
    repo.git(&["fetch", "--quiet", "origin"]);

    // The wrapper is bound to the primary root, and `Pathspec` always carries `:(glob,top)`
    // — so standing in a subdirectory cannot change the answer. Without `top`, recon
    // measured the same query returning 0 instead of 2.
    let from_root = common::ctx_at(&repo.root);
    let from_sub = common::ctx_at(&repo.root.join("src"));
    let since_root = from_root.git.head_sha(&since).unwrap();
    let globs = [Pathspec::glob("src/auth/**")];
    let a = yes(
        from_root
            .git
            .merges_touching(since_root.sha(), "origin/main", &globs),
        "from root",
    );
    let b = yes(
        from_sub
            .git
            .merges_touching(since_root.sha(), "origin/main", &globs),
        "from src/",
    );
    assert_eq!((a, b), (1, 1));
}

#[test]
fn the_wrapper_reports_worktrees_with_the_primary_first() {
    let repo = TestRepo::new();
    let wt = repo.worktree("t-9c41");

    for cwd in [repo.root.clone(), wt.clone(), wt.join("src")] {
        let ctx = common::ctx_at(&cwd);
        let rows = ctx.git.worktrees().expect("worktree list");
        assert!(rows.len() >= 2, "{rows:#?}");
        assert_eq!(
            rows[0].path.canonicalize().unwrap(),
            repo.root.canonicalize().unwrap(),
            "the FIRST stanza is always the primary worktree, wherever this runs from"
        );
        assert_eq!(rows[0].branch.as_deref(), Some("main"));
        assert!(!rows[0].bare && !rows[0].detached);
        let linked = rows
            .iter()
            .find(|r| r.path.canonicalize().ok() == wt.canonicalize().ok())
            .expect("the linked worktree is listed");
        assert_eq!(linked.branch.as_deref(), Some("wt/t-9c41"));
        assert!(linked.head.is_some());
    }
}

#[test]
fn every_wrapper_call_from_a_linked_worktree_is_anchored_to_the_primary() {
    let repo = TestRepo::new();
    let wt = repo.worktree("t-9c41");
    let ctx = common::ctx_at(&wt);

    assert!(ctx.repo.linked(), "we really are in a linked worktree");
    assert_ne!(ctx.repo.git_dir(), ctx.repo.common_dir());
    assert_eq!(
        ctx.repo.primary_root().canonicalize().unwrap(),
        repo.root.canonicalize().unwrap()
    );
    assert_eq!(
        ctx.git.root().canonicalize().unwrap(),
        repo.root.canonicalize().unwrap(),
        "`git -C <primary_root>` — never the cwd"
    );
    assert_eq!(
        std::path::PathBuf::from(ctx.layout.ks().display().to_string())
            .canonicalize()
            .unwrap(),
        repo.root.canonicalize().unwrap().join(".kanspec"),
        "one board per machine, via git-common-dir"
    );
    // Hooks live in the COMMON dir and are shared by every worktree, so the resolved path
    // is identical from either side.
    assert_eq!(
        ctx.git.hooks_dir().unwrap(),
        common::ctx_at(&repo.root).git.hooks_dir().unwrap()
    );
    // And the consequence, stated plainly: because the wrapper is anchored to the
    // primary, `current_branch()` reports the PRIMARY's branch, not the one the human is
    // standing on. The branch "here" comes from `worktrees()` matched against
    // `Repo::here()` — which keeps `git -C <primary_root>` an exceptionless rule instead
    // of a rule with a cwd-shaped hole in it.
    assert_eq!(ctx.git.current_branch().as_deref(), Some("main"));
    let here = ctx.repo.here().canonicalize().unwrap();
    let mine = ctx
        .git
        .worktrees()
        .unwrap()
        .into_iter()
        .find(|r| r.path.canonicalize().ok() == Some(here.clone()))
        .expect("the worktree we stand in is listed");
    assert_eq!(mine.branch.as_deref(), Some("wt/t-9c41"));
}

#[test]
fn core_hooks_path_moves_the_hooks_directory_out_from_under_dot_git() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    let default = ctx.git.hooks_dir().unwrap();
    assert!(default.ends_with("hooks"), "{}", default.display());

    // husky / lefthook / pre-commit all do this, and it makes `.git/hooks` completely
    // inert — a hook installed there would be silently dead (D-7).
    let custom = repo.root.join("myhooks");
    std::fs::create_dir_all(&custom).unwrap();
    repo.git(&["config", "core.hooksPath", &custom.to_string_lossy()]);
    let moved = common::ctx_at(&repo.root).git.hooks_dir().unwrap();
    assert_eq!(
        moved.canonicalize().unwrap(),
        custom.canonicalize().unwrap()
    );
}

#[test]
fn worktree_add_classifies_its_failures_by_stderr_not_by_exit_code() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    let base = repo.root.parent().unwrap().join("wt2");
    std::fs::create_dir_all(&base).unwrap();

    ctx.git
        .worktree_add(&base.join("a"), "ks/t-2222-a", "main")
        .expect("the happy path");
    assert!(base.join("a").join("README.md").is_file());
    // `--no-track` is mandatory: without it a push from the ticket branch targets main.
    let (_, upstream, _) = repo.git_try(&["config", "--get", "branch.ks/t-2222-a.merge"]);
    assert!(
        upstream.trim().is_empty(),
        "--no-track, or push targets main"
    );

    // Recon measured exit 255 here and 128 for everything else, so the classification is
    // by message. Both must still be typed refusals that name a fix.
    let e = ctx
        .git
        .worktree_add(&base.join("b"), "ks/t-2222-a", "main")
        .unwrap_err();
    assert_eq!(e.kind(), "conflict", "{e}");
    assert!(e.to_string().contains("ks/t-2222-a"), "{e}");
    assert!(e.fixes().iter().next().is_some());

    let e = ctx
        .git
        .worktree_add(&base.join("a"), "ks/t-3333-c", "main")
        .unwrap_err();
    assert_eq!(e.kind(), "conflict", "{e}");

    // `worktree remove` never deletes the branch, so kanspec must do it separately.
    ctx.git.worktree_remove(&base.join("a"), true).unwrap();
    assert!(!base.join("a").exists());
    assert!(
        repo.git_try(&["rev-parse", "--verify", "--quiet", "ks/t-2222-a"])
            .0
            == 0
    );
    ctx.git.branch_delete("ks/t-2222-a", true).unwrap();
    assert_ne!(
        repo.git_try(&["rev-parse", "--verify", "--quiet", "ks/t-2222-a"])
            .0,
        0
    );
    assert_eq!(
        ctx.git
            .branch_delete("ks/t-nope", false)
            .unwrap_err()
            .kind(),
        "git"
    );
}

#[test]
fn dirty_kanspec_counts_pending_tracker_changes_and_ignores_the_cache() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    assert_eq!(
        ctx.git.dirty_kanspec(&[]).unwrap(),
        0,
        "the fixture is clean"
    );

    repo.write(".kanspec/tickets/t-9c41.md", "---\nid: t-9c41\n---\n");
    assert_eq!(
        ctx.git.dirty_kanspec(&[]).unwrap(),
        1,
        "an untracked ticket counts"
    );
    repo.write(
        ".kanspec/config.toml",
        "main = \"origin/main\"\nport = 5758\n",
    );
    assert_eq!(
        ctx.git.dirty_kanspec(&[]).unwrap(),
        2,
        "a modified file counts"
    );

    // The disposable cache is gitignored, so a `scan` every 60s never inflates the count.
    repo.write(".kanspec/cache/gitstate.json", "{\"version\":1}");
    assert_eq!(
        ctx.git.dirty_kanspec(&[]).unwrap(),
        2,
        "cache/ is invisible"
    );
    assert!(ctx.git.is_ignored(&repo.root.join(".kanspec/cache")));
    assert!(ctx
        .git
        .is_ignored(&repo.root.join(".kanspec/cache/not-created-yet.json")));
    assert!(!ctx.git.is_ignored(&repo.root.join(".kanspec/tickets")));

    // `check-ignore` proves a RULE exists; only `ls-files` proves nothing is already
    // tracked under it — a file committed before the rule stays tracked forever.
    assert!(ctx.git.is_tracked(&repo.root.join(".kanspec/config.toml")));
    assert!(!ctx
        .git
        .is_tracked(&repo.root.join(".kanspec/cache/gitstate.json")));
}

#[test]
fn commit_kanspec_stages_only_the_tracker_and_is_a_no_op_when_clean() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    let before = repo.sha("HEAD");

    ctx.git.commit_kanspec("kanspec: noop").unwrap();
    assert_eq!(
        repo.sha("HEAD"),
        before,
        "nothing staged, nothing committed"
    );

    repo.write(".kanspec/tickets/t-9c41.md", "---\nid: t-9c41\n---\n");
    repo.write("src/auth/unrelated.ts", "// a human's work in progress\n");
    ctx.git.commit_kanspec("kanspec: ship t-9c41").unwrap();
    assert_ne!(repo.sha("HEAD"), before);
    let files = repo.git(&["show", "--name-only", "--format=", "HEAD"]);
    assert_eq!(files.trim(), ".kanspec/tickets/t-9c41.md");
    assert_eq!(
        ctx.git.dirty_kanspec(&[]).unwrap(),
        0,
        "the tracker is committed"
    );
    // The human's unrelated edit is untouched.
    assert_eq!(
        repo.git(&["status", "--porcelain", "--", "src"]).trim(),
        "?? src/auth/unrelated.ts"
    );
}

#[test]
fn fetch_updates_the_freshness_clock_even_when_nothing_new_arrives() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    ctx.git.fetch().expect("a local bare origin always fetches");
    let first = ctx
        .git
        .fetch_age()
        .expect("FETCH_HEAD exists after a fetch");

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let aged = ctx.git.fetch_age().unwrap();
    assert!(aged > first, "the clock ticks: {first:?} -> {aged:?}");

    // FETCH_HEAD's mtime is rewritten on EVERY fetch, which is what makes it a valid
    // freshness clock — a remote-tracking ref's mtime is not, since refs get packed.
    ctx.git.fetch().unwrap();
    let refreshed = ctx.git.fetch_age().unwrap();
    assert!(refreshed < aged, "a no-op fetch still resets it");
}

#[test]
fn fetch_against_a_broken_remote_is_a_typed_error_naming_its_fix() {
    let repo = TestRepo::new();
    repo.git(&["remote", "set-url", "origin", "/nonexistent/nope.git"]);
    let ctx = common::ctx_at(&repo.root);
    let e = ctx.git.fetch().unwrap_err();
    assert_eq!(e.kind(), "git");
    assert!(e.fixes().iter().next().is_some(), "invariant 9");
}

#[test]
fn resolve_main_falls_back_rather_than_guessing_and_refuses_when_it_cannot() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    assert_eq!(ctx.git.resolve_main("origin/main").unwrap(), "origin/main");

    // A configured ref that does not resolve falls through to origin/HEAD…
    assert_eq!(
        ctx.git.resolve_main("origin/nonexistent").unwrap(),
        "origin/main",
        "origin/HEAD names the default branch"
    );

    // …and with no remote at all, the local candidates.
    repo.git(&["remote", "remove", "origin"]);
    let ctx = common::ctx_at(&repo.root);
    assert_eq!(ctx.git.resolve_main("origin/main").unwrap(), "main");

    // With nothing at all resolvable, it REFUSES rather than picking something.
    let bare = tempfile::tempdir().unwrap();
    common::git_at(
        bare.path(),
        &["init", "--quiet", "--initial-branch=main", "empty"],
    );
    let empty = bare.path().join("empty");
    let ctx = common::ctx_at(&empty);
    let e = ctx.git.resolve_main("origin/main").unwrap_err();
    assert_eq!(e.kind(), "environment");
    assert!(e.fixes().iter().next().is_some());
}

#[test]
fn current_branch_is_none_when_detached_and_ahead_behind_reads_both_sides() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    assert_eq!(ctx.git.current_branch().as_deref(), Some("main"));

    repo.write("src/auth/x.ts", "1\n");
    repo.commit("ahead by one");
    assert_eq!(
        ctx.git.ahead_behind("origin/main", "main"),
        Some((1, 0)),
        "(ahead, behind) relative to the base"
    );
    assert_eq!(ctx.git.ahead_behind("main", "origin/main"), Some((0, 1)));
    assert!(ctx.git.last_commit_at("main").is_some());
    assert!(ctx.git.last_commit_at("origin/nonexistent").is_none());

    repo.git(&["checkout", "--quiet", "--detach", "HEAD"]);
    assert_eq!(
        ctx.git.current_branch(),
        None,
        "detached HEAD is None, not the literal string `HEAD`"
    );
}

#[test]
fn the_child_environment_is_scrubbed_so_a_hook_cannot_redirect_the_wrapper() {
    let repo = TestRepo::new();
    let wt = repo.worktree("t-9c41");
    let ctx = common::ctx_at(&repo.root);

    // This is the state git leaves behind when it runs a hook, and the environment beats
    // `-C`. Without the scrub the wrapper would read the LINKED worktree's git dir while
    // writing the PRIMARY worktree's files — a mixed, silently wrong state.
    let linked_git_dir = common::git_at(&wt, &["rev-parse", "--absolute-git-dir"])
        .trim()
        .to_string();
    let guard = EnvGuard::set("GIT_DIR", &linked_git_dir);
    let out = ctx.git.run(&["rev-parse", "--absolute-git-dir"]).unwrap();
    assert_eq!(out.code, 0, "{}", out.err);
    assert_eq!(
        Path::new(out.out.trim()).canonicalize().unwrap(),
        repo.root.join(".git").canonicalize().unwrap(),
        "GIT_DIR from the environment must not win"
    );
    drop(guard);
}

/// Restores (or removes) an env var on drop. The tests that need one run single-threaded
/// against a variable nothing else reads.
struct EnvGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, val: &str) -> EnvGuard {
        let prev = std::env::var_os(key);
        std::env::set_var(key, val);
        EnvGuard { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}
