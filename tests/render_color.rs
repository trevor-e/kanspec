//! `tests/render_color.rs`
//!
//! Proves: **`--color` changes bytes, never layout.**
//!
//! For every command that renders an `out::Line`, the output of `--color always` with its
//! ANSI escapes stripped is BYTE-IDENTICAL to the output of `--color never` — same
//! columns, same padding, same right-aligned fix, same exit code.
//!
//! WHY THIS FILE EXISTS. The adversarial audit's #1 finding was a padding bug that hit
//! 100% of human sessions on every id-bearing command:
//!
//! ```text
//! $ kanspec ready --color never
//!  ○ t-ec64   Padding probe ticket · auth        → kanspec start t-ec64
//! $ kanspec ready --color always
//!  ○ <bold>t-ec64</bold>Padding probe ticket · auth   → kanspec start t-ec64
//! ```
//!
//! `format!("{:<9}", paint(id, Bold, true))` counts the escapes as columns, decides the
//! field is already over-full, and pads nothing. **All 463 tests were structurally blind to
//! it**: a test captures a pipe, a pipe is not a terminal, and every assertion in the suite
//! therefore only ever saw the colourless rendering. Colour was an untested output mode.
//!
//! The property below closes that hole for good, for every current and future command: it
//! needs no per-command expectations, so a command added tomorrow is covered the moment its
//! name is added to the list.
//!
//! Owner: **F** (`src/out.rs`).

mod common;

use common::TestRepo;
use kanspec::out::{pad_visible, paint, visible_len, Color};
use serde_json::Value;

// ─────────────────────────────────────────────────────────────────────────────
// the helper, tested directly
// ─────────────────────────────────────────────────────────────────────────────

/// Drop every ANSI escape sequence. Written from the ECMA-48 grammar rather than by
/// reusing `out::visible_len`, deliberately: a bug shared by the code under test and its
/// test cancels out and proves nothing.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if it.peek() == Some(&'[') {
            it.next();
            // CSI: parameter bytes `0`..=`?`, intermediates ` `..=`/`, then a final byte
            // in `@`..=`~` — which is why the `[` above must be consumed FIRST.
            for c in it.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        } else {
            it.next();
        }
    }
    out
}

#[test]
fn strip_ansi_is_an_honest_oracle() {
    assert_eq!(strip_ansi("plain"), "plain");
    assert_eq!(strip_ansi("\u{1b}[1mt-ec64\u{1b}[0m"), "t-ec64");
    assert_eq!(strip_ansi("\u{1b}[38;5;9mred\u{1b}[0m"), "red");
    // It must not eat the `[`-less escapes, nor anything that merely looks like one.
    assert_eq!(strip_ansi("a[1mb"), "a[1mb");
    for c in [
        Color::Red,
        Color::Green,
        Color::Cyan,
        Color::Dim,
        Color::Bold,
    ] {
        assert_eq!(strip_ansi(&paint("t-ec64", c, true)), "t-ec64");
    }
}

/// THE UNIT TEST OF THE PADDING HELPER, with a painted input — the exact shape the shipped
/// code got wrong.
#[test]
fn pad_visible_pads_a_painted_string_by_its_visible_width() {
    let painted = paint("t-ec64", Color::Bold, true);
    assert!(
        painted.chars().count() > "t-ec64".chars().count(),
        "the premise: escapes make a painted id longer than it looks — {painted:?}"
    );
    assert_eq!(visible_len(&painted), 6);

    let padded = pad_visible(&painted, 9);
    assert_eq!(strip_ansi(&padded), "t-ec64   ", "{padded:?}");
    assert_eq!(visible_len(&padded), 9, "{padded:?}");

    // The bug itself, pinned so nobody reintroduces the specifier: `{:<9}` over the SAME
    // painted string pads nothing at all.
    assert_eq!(
        strip_ansi(&format!("{painted:<9}")),
        "t-ec64",
        "a width specifier over a painted string is a silent no-op"
    );
    assert_ne!(strip_ansi(&format!("{painted:<9}")), strip_ansi(&padded));

    // Unpainted input stays byte-identical to the specifier it replaces…
    assert_eq!(pad_visible("t-ec64", 9), format!("{:<9}", "t-ec64"));
    assert_eq!(pad_visible("", 4), "    ");
    // …and an over-wide field is pushed right, never truncated.
    assert_eq!(pad_visible("t-abcdefghij", 9), "t-abcdefghij");
    assert_eq!(pad_visible(&painted, 2), painted);
}

// ─────────────────────────────────────────────────────────────────────────────
// the property, against the REAL binary
// ─────────────────────────────────────────────────────────────────────────────

/// Two repos built by the identical script. `TestRepo` pins the clock, the actor and the
/// id seed, so the twins mint the same ids and stay in lockstep through the mutating verbs
/// — which is what lets `new`, `ship` and `done` be compared at all: each twin runs the
/// mutation exactly once, in the same order, under a different `--color`.
struct Pair<'r> {
    plain: &'r TestRepo,
    painted: &'r TestRepo,
}

impl Pair<'_> {
    /// Run `args` on both twins — colourless on one, painted on the other — and assert the
    /// painted run is the colourless run plus escapes. Returns whether colour was actually
    /// emitted, so the caller can refuse a vacuous pass.
    #[track_caller]
    fn check(&self, args: &[&str]) -> bool {
        let mut never = vec!["--color", "never"];
        never.extend_from_slice(args);
        let mut always = vec!["--color", "always"];
        always.extend_from_slice(args);

        let plain = self.plain.ks(&never);
        let painted = self.painted.ks(&always);
        let what = args.join(" ");

        assert_eq!(
            painted.code,
            plain.code,
            "`kanspec {what}` exited {} painted and {} plain\n--- painted ---\n{}{}\n--- plain ---\n{}{}",
            painted.code,
            plain.code,
            painted.stdout,
            painted.stderr,
            plain.stdout,
            plain.stderr,
        );
        assert_eq!(
            self.scrub(&strip_ansi(&painted.stdout)),
            self.scrub(&plain.stdout),
            "`kanspec {what} --color always` is not `--color never` plus escapes.\n\
             --- --color always (escapes shown as ESC) ---\n{}\n\
             --- --color always, stripped ---\n{}\n\
             --- --color never ---\n{}",
            show(&painted.stdout),
            strip_ansi(&painted.stdout),
            plain.stdout,
        );
        assert_eq!(
            self.scrub(&strip_ansi(&painted.stderr)),
            self.scrub(&plain.stderr),
            "`kanspec {what} --color always` diverged on STDERR:\n{}\n---\n{}",
            show(&painted.stderr),
            plain.stderr,
        );
        painted.stdout.contains('\u{1b}') || painted.stderr.contains('\u{1b}')
    }

    /// The twins live in different temp dirs, so anything that prints a path — `where`'s
    /// absolute worktree, the board's `…/<parent>/<tail>` shortening — would differ on the
    /// tmp name alone. Fold both twins' roots onto one token. Every form is the same
    /// LENGTH in both repos, so column widths are unaffected and the property stays honest.
    fn scrub(&self, s: &str) -> String {
        let mut out = s.to_string();
        for repo in [self.plain, self.painted] {
            let tmp = repo.root.parent().unwrap_or(&repo.root).to_path_buf();
            let canonical = std::fs::canonicalize(&tmp).unwrap_or_else(|_| tmp.clone());
            // Longest first: the basename is a substring of both absolute forms.
            for form in [
                canonical.display().to_string(),
                tmp.display().to_string(),
                tmp.file_name().unwrap_or_default().to_string_lossy().into(),
            ] {
                if !form.is_empty() {
                    out = out.replace(&form, "<TMP>");
                }
            }
        }
        mask_shas(&out)
    }

    /// A step applied to both twins and compared by nobody — the git-side work between
    /// commands.
    fn both(&self, f: impl Fn(&TestRepo)) {
        f(self.plain);
        f(self.painted);
    }
}

/// Commit SHAs differ between the twins: git stamps a real wall-clock committer date and
/// the two repos are built microseconds apart, so `ship`'s `head`, `scan --explain`'s
/// ladder trace and `done`'s proof all name different objects. Mask each hex run of 7+
/// characters that contains a digit — `defaced` is hex-looking English and stays — with a
/// run of `x` of the SAME LENGTH, so every column width the property is actually about
/// survives the masking. Ticket ids (`t-c7ec`, four chars) are far too short to be caught.
fn mask_shas(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        let sha = run.len() >= 7 && run.len() <= 40 && run.chars().any(|c| c.is_ascii_digit());
        if sha {
            out.extend(std::iter::repeat_n('x', run.len()));
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if c.is_ascii_hexdigit() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

#[test]
fn mask_shas_masks_only_shas() {
    assert_eq!(
        mask_shas("head 40fb71d · PR #145"),
        "head xxxxxxx · PR #145"
    );
    assert_eq!(mask_shas("t-c7ec"), "t-c7ec");
    assert_eq!(mask_shas("q-11ba · D-8c1a"), "q-11ba · D-8c1a");
    assert_eq!(mask_shas("a defaced facade"), "a defaced facade");
    assert_eq!(mask_shas("0s ago"), "0s ago");
    assert_eq!(mask_shas("abc1234").len(), "abc1234".len());
}

/// ESC rendered visibly, so a failure message shows WHERE the escapes are.
fn show(s: &str) -> String {
    s.replace('\u{1b}', "ESC")
}

/// The knowledge and the tickets every comparison runs against. Deliberately not a happy
/// path: it leaves one ticket claimed with real commits on its branch, one untouched in
/// `todo`, and a spec/decision/quirk on the ground the branch edits — so `status`, `board`
/// and `scan` all have something painted to say.
fn scenario(repo: &TestRepo) -> (String, String) {
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login (JWT 24h), lockout after 5 failures\ncode: [src/auth/**]\n---\n\
         # auth\n\n## Rules\n- [auth.jwt] Login issues a JWT valid 24h. {p-02cc}\n",
    );
    repo.write(
        ".kanspec/decisions/D-8c1a.md",
        "---\nid: D-8c1a\ntitle: Rate-limit state lives in Redis only\nstatus: accepted\n\
         date: 2026-08-20\nsource: null\nscope: [src/auth/**]\nsupersedes: null\n\
         superseded_by: null\n---\n## Decision\nSliding window in Redis.\n",
    );
    repo.write(
        ".kanspec/quirks/q-11ba.md",
        "---\nid: q-11ba\ntitle: Sessions rotate on every redeploy\npaths: [src/auth/**]\n\
         severity: landmine\nstatus: active\nsource: null\nfixed_by: null\n---\nBody.\n",
    );
    repo.git(&["add", "-A", "--", ".kanspec"]);
    repo.git(&["commit", "--quiet", "-m", "knowledge"]);
    repo.push("main");

    let one = mint(
        repo,
        &["new", "Rate-limit login endpoint", "--spec", "auth"],
    );
    let two = mint(repo, &["new", "Session middleware leaks a listener"]);

    // `start` moves the primary onto the ticket branch, which is what makes `where` and
    // `status` interesting below.
    repo.ks(["start", one.as_str()]).ok();
    let branch = format!("ks/{one}-rate-limit-login-endpoint");
    repo.write("src/auth/lockout.ts", "export const lockout = () => {};\n");
    let spec = repo.read(".kanspec/specs/auth.md");
    repo.write(
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.lockout] 5 failed logins in 10m locks the account 15m.\n"),
    );
    repo.git(&["add", "-A", "--", "src", ".kanspec/specs"]);
    repo.git(&[
        "commit",
        "--quiet",
        "-m",
        &format!("{one}: lockout counter\n\nKanspec: {one}\n"),
    ]);
    repo.push(&branch);

    (one, two)
}

/// `kanspec new … --json`, returning the minted id. Setup output is never compared, so the
/// JSON surface is the right way to read it back.
fn mint(repo: &TestRepo, args: &[&str]) -> String {
    let v: Value = repo.json(args);
    v["id"]
        .as_str()
        .unwrap_or_else(|| panic!("`kanspec {}` minted no id: {v}", args.join(" ")))
        .to_string()
}

#[test]
fn every_command_renders_the_same_layout_painted_as_it_does_plain() {
    let plain = TestRepo::new();
    let painted = TestRepo::new();
    let ids = scenario(&plain);
    assert_eq!(
        ids,
        scenario(&painted),
        "the twins must mint the same ids, or nothing below can be compared"
    );
    let (one, two) = ids;
    let branch = format!("ks/{one}-rate-limit-login-endpoint");
    let p = Pair {
        plain: &plain,
        painted: &painted,
    };

    // The audit's own probe, verbatim: a `new` whose id and title were rendered jammed
    // together the moment colour was on.
    let mut painted_commands = 0usize;
    let mut check = |args: &[&str], must_paint: bool| {
        let saw = p.check(args);
        if must_paint {
            assert!(
                saw,
                "`kanspec {}` emitted no ANSI under `--color always` — the comparison above \
                 passed vacuously and proves nothing",
                args.join(" ")
            );
            painted_commands += 1;
        }
    };

    check(&["new", "Padding probe ticket", "--spec", "auth"], true);
    check(&["ready"], true);
    check(&["status"], true);
    check(&["board"], true);
    check(&["show", &one], true);
    check(&["show", &two], true);
    check(&["where"], true);
    check(&["ls"], false);
    check(&["log", &one], false);
    check(&["quirks"], true);
    check(&["features"], false);
    check(&["rules"], false);
    check(&["prime"], false);
    check(&["why", "auth.jwt"], false);

    // The refusal path renders through `KsError::render_human`, which paints too: a
    // `todo` ticket cannot be closed, and the ✗ / → of that answer must line up as well.
    check(&["done", &two], true);

    // A REAL squash merge into the real bare origin, so `scan`, `status` and `done` all
    // have git-derived truth to render. Applied to both twins, compared on neither.
    p.both(|r| {
        r.git(&["switch", "--quiet", "main"]);
        r.git(&["merge", "--quiet", "--squash", &branch]);
        r.git(&[
            "commit",
            "--quiet",
            "-m",
            &format!("land the lockout counter\n\nKanspec: {one}\n"),
        ]);
        r.push("main");
    });

    check(&["ship", &one, "--pr", "145"], true);
    check(&["scan"], true);
    check(&["scan", "--explain"], true);
    // The money line: `⇂ <id>   in main …, not closed  → kanspec done <id>`.
    check(&["status"], true);
    check(&["board"], true);
    check(&["done", &one, "--no-followups", "--no-quirks"], true);
    check(&["status"], true);
    check(&["ls"], false);
    check(&["doctor"], false);

    assert!(
        painted_commands >= 12,
        "only {painted_commands} commands actually emitted colour; this test is supposed to \
         cover the whole id-bearing surface"
    );
}
