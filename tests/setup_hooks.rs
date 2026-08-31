//! `tests/setup_hooks.rs`
//!
//! Proves: foreign-hook preservation; core.hooksPath; the .d/ dispatch order
//!
//! The invariant with a name here is **reversibility**. kanspec installs itself into three
//! places it does not own — the hooks directory, CLAUDE.md, and an agent's settings.json —
//! and every one of them may already contain somebody else's work. Each test below is a
//! sentence about that: what was there is still there, it still runs, and `--remove` puts
//! the repository back the way it found it.
//!
//! Owner: **S7**.

mod common;

use std::path::{Path, PathBuf};

use common::TestRepo;

/// A husky-style hook: does real work, and its exit code matters.
const HUSKY: &str = "#!/bin/sh\n# husky\nnpx --no-install commitlint\ntouch \"$(dirname \"$0\")/../husky-ran\"\nexit 0\n";

fn hook_path(repo: &TestRepo, hook: &str) -> PathBuf {
    repo.root.join(".git").join("hooks").join(hook)
}

fn write_exec(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The harness template ships a `.kanspec/` so every other slice can test without waiting
/// on `init`. These tests are about `init` itself, so they start from a repo without one.
fn without_store(repo: &TestRepo) {
    std::fs::remove_dir_all(repo.root.join(".kanspec")).unwrap();
}

// ─────────────────────────────────────────────────────────────────────────────
// init
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn init_scaffolds_a_working_store_from_nothing() {
    let repo = TestRepo::new();
    without_store(&repo);

    let r = repo.ks(["init"]).ok();
    assert!(r.stdout.contains("config.toml"), "{}", r.stdout);

    for f in [
        ".kanspec/config.toml",
        ".kanspec/tickets/.gitkeep",
        ".kanspec/specs/.gitkeep",
        ".kanspec/decisions/.gitkeep",
        ".kanspec/quirks/.gitkeep",
        ".kanspec/proposals/closed/.gitkeep",
        ".kanspec/.gitignore",
        ".gitattributes",
    ] {
        assert!(repo.exists(f), "init did not create {f}");
    }
    assert!(repo.root.join(".kanspec/cache").is_dir());

    // Every default present and commented, so the knobs are discoverable without reading
    // source — and parseable, so the file it writes is one it can read back.
    let cfg = repo.read(".kanspec/config.toml");
    for knob in [
        "main",
        "id_width",
        "sync",
        "port",
        "branch_prefix",
        "[paths]",
        "[windows]",
        "stale_merges",
        "[git]",
        "[ci]",
        "provider",
        "[hooks]",
        "landcheck",
    ] {
        assert!(cfg.contains(knob), "config.toml never mentions {knob}");
    }

    // The one union-merged file, via a built-in git strategy: zero per-clone setup.
    assert!(repo
        .read(".gitattributes")
        .contains("comments.jsonl merge=union"));
    // The cache holds every derived git fact, which is exactly why it must not travel.
    assert!(repo.read(".kanspec/.gitignore").contains("cache/"));
    repo.git(&["add", "-A"]);
    assert!(
        !repo.git(&["status", "--porcelain"]).contains("cache"),
        "the cache is gitignored"
    );
}

#[test]
fn init_from_a_linked_worktree_scaffolds_the_primary_one() {
    let repo = TestRepo::new();
    without_store(&repo);
    let wt = repo.worktree("wt1");
    // The harness commits its scaffold, so a fresh checkout materialises a copy of it.
    std::fs::remove_dir_all(wt.join(".kanspec")).unwrap();

    repo.ks_in(&wt, ["init"]).ok();

    assert!(
        repo.exists(".kanspec/config.toml"),
        "init scaffolded the linked worktree instead of the primary one"
    );
    assert!(
        !wt.join(".kanspec").exists(),
        "a second store appeared in the linked worktree"
    );
    // Hooks live in the common git dir, which is shared by every worktree.
    assert!(repo.exists(".git/hooks/prepare-commit-msg"));
}

#[test]
fn init_is_idempotent_and_never_clobbers_an_edit() {
    let repo = TestRepo::new();
    without_store(&repo);
    repo.ks(["init"]).ok();

    // Everything a user might have touched by hand.
    repo.write(".kanspec/config.toml", "port = 6001\nmain = \"origin/trunk\"\n");
    repo.write(".gitattributes", "*.png binary\n.kanspec/proposals/**/comments.jsonl merge=union\n");
    repo.write(".kanspec/tickets/t-aaaa.md", "---\nid: t-aaaa\n---\nbody\n");

    let r = repo.ks(["init"]).ok();
    assert_eq!(
        repo.read(".kanspec/config.toml"),
        "port = 6001\nmain = \"origin/trunk\"\n",
        "a second init rewrote the user's config"
    );
    assert_eq!(
        repo.read(".gitattributes"),
        "*.png binary\n.kanspec/proposals/**/comments.jsonl merge=union\n",
        "a second init duplicated the merge=union line"
    );
    assert!(repo.exists(".kanspec/tickets/t-aaaa.md"));
    assert!(
        r.stdout.contains("already present"),
        "a re-init should say so:\n{}",
        r.stdout
    );
}

#[test]
fn init_appends_the_union_line_to_a_gitattributes_it_did_not_write() {
    let repo = TestRepo::new();
    without_store(&repo);
    repo.write(".gitattributes", "*.png binary\n");

    repo.ks(["init"]).ok();
    let text = repo.read(".gitattributes");
    assert!(text.starts_with("*.png binary\n"), "{text:?}");
    assert!(text.contains("comments.jsonl merge=union"), "{text:?}");
}

#[test]
fn init_refuses_to_claim_a_projection_path_that_already_exists_ungenerated() {
    let repo = TestRepo::new();
    without_store(&repo);
    repo.write("KANSPEC-FEATURES.md", "# our own hand-written feature list\n");

    let r = repo.ks(["init"]);
    assert_eq!(r.code, 1, "stdout: {}\nstderr: {}", r.stdout, r.stderr);
    assert!(
        r.stderr.contains("KANSPEC-FEATURES.md"),
        "the refusal must name the file: {}",
        r.stderr
    );
    assert!(
        r.stderr.contains("[paths]"),
        "and its one-command fix: {}",
        r.stderr
    );
    assert_eq!(
        repo.read("KANSPEC-FEATURES.md"),
        "# our own hand-written feature list\n",
        "the file it refused to claim must be untouched"
    );

    // `[paths]` renames the projection, and then init proceeds. This is the fix the
    // refusal named, run end to end.
    repo.write(
        ".kanspec/config.toml",
        "[paths]\nfeatures = \"docs/FEATURES.md\"\n",
    );
    repo.ks(["init"]).ok();
    assert!(repo.exists(".kanspec/tickets/.gitkeep"));
    // A projection in a subdirectory must not drag the repo-root files down with it.
    assert!(
        repo.exists(".gitattributes") && !repo.exists("docs/.gitattributes"),
        "the [paths] table moved .gitattributes out of the repo root"
    );
}

#[test]
fn a_generated_projection_is_kanspecs_to_claim() {
    let repo = TestRepo::new();
    without_store(&repo);
    // The header `project::render_*` stamps on both projections.
    repo.write(
        "KANSPEC-FEATURES.md",
        "<!-- GENERATED by kanspec. Edit the sources (.kanspec/specs, .kanspec/decisions), \
         not this file. -->\n# Feature map\n",
    );
    repo.ks(["init"]).ok();
}

// ─────────────────────────────────────────────────────────────────────────────
// git hooks
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn init_installs_every_hook_executable_and_identifiable() {
    let repo = TestRepo::new();
    without_store(&repo);
    repo.ks(["init"]).ok();

    for hook in ["post-merge", "post-checkout", "prepare-commit-msg", "commit-msg"] {
        let p = hook_path(&repo, hook);
        let body = read(&p);
        assert!(body.starts_with("#!/bin/sh\n"), "{hook} has no shebang");
        assert!(body.contains("kanspec-managed hook"), "{hook} is unmarked");
        assert!(body.ends_with('\n'), "{hook} has no trailing newline");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "git silently ignores a non-x {hook}");
        }
    }
}

#[test]
fn a_pre_existing_hook_survives_install_and_is_restored_exactly_by_remove() {
    let repo = TestRepo::new();
    let hook = hook_path(&repo, "post-merge");
    write_exec(&hook, HUSKY);

    repo.ks(["setup", "claude"]).ok();

    let displaced = repo.root.join(".git/hooks/post-merge.d/10-post-merge");
    assert_eq!(read(&displaced), HUSKY, "the foreign hook was not preserved");
    assert!(
        read(&hook).contains("kanspec-managed hook"),
        "kanspec did not take the entrypoint"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&displaced).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "the displaced hook lost its +x");
    }

    repo.ks(["setup", "claude", "--remove"]).ok();

    assert_eq!(read(&hook), HUSKY, "--remove did not restore it byte for byte");
    assert!(
        !repo.root.join(".git/hooks/post-merge.d").exists(),
        "the .d/ directory should be gone once it is empty"
    );
    for hook in ["post-checkout", "prepare-commit-msg", "commit-msg"] {
        assert!(
            !hook_path(&repo, hook).exists(),
            "{hook} was installed by kanspec and should be gone"
        );
    }
}

#[test]
fn the_displaced_hook_runs_first_and_its_exit_code_wins() {
    let repo = TestRepo::new();
    let hook = hook_path(&repo, "post-merge");
    let marker = repo.root.join("husky-ran");
    write_exec(
        &hook,
        // Builtins only, so this proves the dispatcher and nothing about the environment.
        &format!("#!/bin/sh\n: > {}\nexit 0\n", shell_quote(&marker.to_string_lossy())),
    );
    repo.ks(["init"]).ok();

    // Run the installed entrypoint the way git would, with kanspec deliberately absent so
    // only the dispatch is under test.
    let out = std::process::Command::new("/bin/sh")
        .arg(&hook)
        .current_dir(&repo.root)
        .env("KANSPEC_BIN", "/nonexistent/kanspec")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{:?}", out);
    assert!(marker.exists(), "the displaced hook never ran");

    // A .d/ script that fails must fail the hook: that is the whole reason for displacing
    // instead of overwriting.
    write_exec(
        &repo.root.join(".git/hooks/post-merge.d/10-post-merge"),
        "#!/bin/sh\nexit 3\n",
    );
    let out = std::process::Command::new("/bin/sh")
        .arg(&hook)
        .current_dir(&repo.root)
        .env("KANSPEC_BIN", "/nonexistent/kanspec")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "the exit code was swallowed");
}

#[test]
fn the_scan_hooks_are_silent_when_kanspec_is_not_on_the_path() {
    let repo = TestRepo::new();
    without_store(&repo);
    repo.ks(["init"]).ok();

    for hook in ["post-merge", "post-checkout"] {
        let out = std::process::Command::new("/bin/sh")
            .arg(hook_path(&repo, hook))
            .args(["a", "b", "1"])
            .current_dir(&repo.root)
            .env("PATH", "/nonexistent")
            .env_remove("KANSPEC_BIN")
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{hook} must not fail the git command that ran it: {out:?}"
        );
        assert!(out.stderr.is_empty(), "{hook} was noisy: {out:?}");
    }
}

#[test]
fn core_hooks_path_is_honoured_rather_than_silently_bypassed() {
    let repo = TestRepo::new();
    without_store(&repo);
    // husky and lefthook both do exactly this, and it makes `.git/hooks` inert.
    repo.git(&["config", "core.hooksPath", ".githooks"]);
    write_exec(
        &repo.root.join(".githooks/post-merge"),
        "#!/bin/sh\n# lefthook\nexit 0\n",
    );

    repo.ks(["init"]).ok();

    assert!(
        repo.exists(".githooks/prepare-commit-msg"),
        "init installed into .git/hooks, which core.hooksPath has made inert"
    );
    assert!(
        !repo.exists(".git/hooks/prepare-commit-msg"),
        "init installed into the inert directory as well"
    );
    assert_eq!(
        read(&repo.root.join(".githooks/post-merge.d/10-post-merge")),
        "#!/bin/sh\n# lefthook\nexit 0\n"
    );
}

#[test]
fn refresh_hooks_catches_up_a_repo_initialised_before_a_hook_existed() {
    let repo = TestRepo::new();
    without_store(&repo);
    repo.ks(["init"]).ok();

    // The state a repo initialised by an older kanspec is in: a store, and hooks that are
    // missing or stale.
    std::fs::remove_file(hook_path(&repo, "commit-msg")).unwrap();
    write_exec(
        &hook_path(&repo, "post-merge"),
        "#!/bin/sh\n# kanspec-managed hook — do not edit; see `kanspec init --refresh-hooks`\n# v0\nexit 0\n",
    );
    repo.write(".kanspec/config.toml", "port = 6001\n");

    let r = repo.ks(["init", "--refresh-hooks"]).ok();

    assert!(repo.exists(".git/hooks/commit-msg"), "{}", r.stdout);
    assert!(
        read(&hook_path(&repo, "post-merge")).contains("scan --quiet"),
        "a stale kanspec hook was not refreshed"
    );
    assert_eq!(
        repo.read(".kanspec/config.toml"),
        "port = 6001\n",
        "--refresh-hooks re-scaffolded over the user's config"
    );
}

#[test]
fn the_trailer_hook_stamps_a_ticket_branch_and_nothing_else() {
    let repo = TestRepo::new();
    without_store(&repo);
    repo.ks(["init"]).ok();

    // `kanspec start` records this; git has no per-branch hooks, so the key IS the
    // dispatch (D-5).
    repo.git(&["config", "branch.main.kanspec-ticket", "t-9c41"]);
    repo.write("src/auth/login.ts", "export const login = () => 1;\n");
    repo.commit("rate-limit login");
    let msg = repo.git(&["log", "-1", "--format=%B"]);
    assert!(
        msg.contains("Kanspec: t-9c41"),
        "the squash-surviving signal never landed:\n{msg}"
    );

    // A branch with no claim pays one `git config` read and stamps nothing.
    repo.git(&["checkout", "-q", "-b", "housekeeping"]);
    repo.write("README.md", "# fixture\n\nmore\n");
    repo.commit("unrelated");
    let msg = repo.git(&["log", "-1", "--format=%B"]);
    assert!(!msg.contains("Kanspec:"), "stamped a branch with no ticket:\n{msg}");
}

#[test]
fn an_empty_commit_message_still_aborts_the_commit() {
    // The trailer must never be the thing that makes a message non-empty: `git commit`,
    // editor, quit-without-typing has to keep aborting.
    let repo = TestRepo::new();
    without_store(&repo);
    repo.ks(["init"]).ok();
    repo.git(&["config", "branch.main.kanspec-ticket", "t-9c41"]);
    repo.write("src/auth/login.ts", "export const login = () => 2;\n");
    repo.git(&["add", "-A"]);

    let (code, _, err) = repo.git_try(&["-c", "core.editor=true", "commit"]);
    assert_ne!(code, 0, "an empty message committed anyway: {err}");
    assert!(err.contains("empty commit message"), "{err}");
}

// ─────────────────────────────────────────────────────────────────────────────
// setup <agent>
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn setup_claude_installs_the_snippet_and_removes_it_byte_exactly() {
    let repo = TestRepo::new();
    let original = "# My project\n\nHouse rules live here.\n";
    repo.write("CLAUDE.md", original);

    repo.ks(["setup", "claude"]).ok();

    let text = repo.read("CLAUDE.md");
    assert!(text.starts_with(original), "the user's text moved:\n{text}");
    for line in [
        "## kanspec",
        "kanspec ready --json",
        "Never state whether something is merged",
        "Closed proposals bind nothing",
        "never hand-edit frontmatter",
    ] {
        assert!(text.contains(line), "the snippet lost `{line}`:\n{text}");
    }

    // Twice is once.
    repo.ks(["setup", "claude"]).ok();
    assert_eq!(repo.read("CLAUDE.md"), text, "a second setup stacked up");

    repo.ks(["setup", "claude", "--remove"]).ok();
    assert_eq!(repo.read("CLAUDE.md"), original, "--remove is not byte-exact");
}

#[test]
fn settings_json_merge_preserves_foreign_hooks() {
    let repo = TestRepo::new();
    let foreign = r#"{
  "permissions": {
    "allow": [
      "Bash(git:*)"
    ]
  },
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "command": "echo mine",
            "type": "command"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "hooks": [
          {
            "command": "guard.sh",
            "type": "command"
          }
        ],
        "matcher": "Bash"
      }
    ]
  }
}
"#;
    repo.write(".claude/settings.json", foreign);

    repo.ks(["setup", "claude"]).ok();

    let v: serde_json::Value = serde_json::from_str(&repo.read(".claude/settings.json")).unwrap();
    assert_eq!(v["permissions"]["allow"][0], "Bash(git:*)");
    assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "guard.sh");
    let session = v["hooks"]["SessionStart"][0]["hooks"].as_array().unwrap();
    assert_eq!(session[0]["command"], "echo mine", "the foreign hook was clobbered");
    assert!(
        session.iter().any(|h| h["command"] == "kanspec prime"),
        "ours never landed: {session:?}"
    );
    assert!(v["hooks"]["PreCompact"].is_array(), "PreCompact is missing");
    let post = v["hooks"]["PostToolUse"][0].clone();
    assert!(
        post["matcher"].as_str().unwrap().contains("Edit"),
        "the landmine warning must fire on edits: {post}"
    );
    assert!(
        v["hooks"]["Stop"].is_null(),
        "landcheck is opt-in (D-14) and must not be installed by default: {v}"
    );

    repo.ks(["setup", "claude", "--remove"]).ok();
    let back: serde_json::Value = serde_json::from_str(&repo.read(".claude/settings.json")).unwrap();
    let want: serde_json::Value = serde_json::from_str(foreign).unwrap();
    assert_eq!(back, want, "--remove did not restore the user's settings");
}

#[test]
fn the_stop_hook_is_installed_only_when_the_config_asks_for_it() {
    let repo = TestRepo::new();
    repo.write(".kanspec/config.toml", "[hooks]\nlandcheck = true\n");

    repo.ks(["setup", "claude"]).ok();

    let v: serde_json::Value = serde_json::from_str(&repo.read(".claude/settings.json")).unwrap();
    assert_eq!(
        v["hooks"]["Stop"][0]["hooks"][0]["command"], "kanspec landcheck",
        "{v}"
    );

    repo.ks(["setup", "claude", "--remove"]).ok();
    assert!(
        !repo.exists(".claude/settings.json"),
        "a settings file kanspec created should go away again"
    );
}

#[test]
fn setup_cursor_and_codex_write_their_own_context_file() {
    let repo = TestRepo::new();
    for (agent, file) in [("cursor", ".cursorrules"), ("codex", "AGENTS.md")] {
        repo.ks(["setup", agent]).ok();
        assert!(repo.read(file).contains("## kanspec"), "{agent}");
        assert!(
            !repo.exists(".claude/settings.json"),
            "{agent} has no settings registry kanspec knows how to write"
        );
        repo.ks(["setup", agent, "--remove"]).ok();
        assert_eq!(repo.read(file), "", "{agent}: --remove left a residue");
    }
}

#[test]
fn setup_refuses_before_init_and_names_it() {
    let repo = TestRepo::new();
    without_store(&repo);
    let r = repo.ks(["setup", "claude"]);
    assert_eq!(r.code, 69, "{} {}", r.stdout, r.stderr);
    assert!(r.stderr.contains("init"), "{}", r.stderr);
}

// ─────────────────────────────────────────────────────────────────────────────
// instructions
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn instructions_lists_every_topic_and_prints_one_verbatim() {
    let repo = TestRepo::new();
    let list = repo.ks(["instructions"]).ok();
    for topic in ["start", "done", "review", "close", "config"] {
        assert!(list.stdout.contains(topic), "{topic} is not listed");
    }
    let one = repo.ks(["instructions", "done"]).ok();
    assert!(one.stdout.starts_with("# done"), "{}", one.stdout);
    assert!(one.stdout.len() > 800, "the doc is still a placeholder");

    let bad = repo.ks(["instructions", "nonsuch"]);
    assert_eq!(bad.code, 1);
    assert!(bad.stderr.contains("instructions"), "{}", bad.stderr);
}

#[test]
fn completions_print_a_script_and_nothing_else() {
    let repo = TestRepo::new();
    let r = repo.ks(["completions", "bash"]).ok();
    assert!(r.stdout.contains("kanspec"), "{}", r.stdout);
    assert!(!r.stdout.contains('→'), "a script must be pipeable: {}", r.stdout);
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
