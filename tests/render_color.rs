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

use std::path::Path;
use std::process::Command;

use common::TestRepo;
use kanspec::out::{pad_visible, paint, spoken_as, visible_len, Color};
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

// ─────────────────────────────────────────────────────────────────────────────
// (a) DISPLAY WIDTH — a column is what the terminal DRAWS, not a `char`
//
// ROUND-E REGRESSION (ARCHITECTURE D-50, reopened). `visible_len` counted `char`s, so a
// wide glyph measured one column where the terminal draws two:
//
//     chars=100  columns=100   plain ascii title
//     chars=100  columns=102   emoji title
//     chars=100  columns=117   CJK title
//
// Every budget in `out.rs` flows through that one function, so a CJK title was
// right-aligned into seventeen columns the terminal did not have and the line WRAPPED —
// the same broken alignment the padding bug above produced, from the opposite direction,
// and equally invisible to the colour-vs-plain property, because colour is not what
// changes.
// ─────────────────────────────────────────────────────────────────────────────

/// Every character in this file's fixtures that a terminal draws TWO columns wide. Small
/// and enumerated on purpose: [`columns`] is the *independent* oracle the width assertions
/// below are judged against, and an oracle that called `unicode-width` would share every
/// bug with the code under test — the same reason `strip_ansi` is hand-written above.
const WIDE: &[char] = &[
    '認', '証', 'ト', 'ー', 'ク', 'ン', 'の', '有', '効', '期', '限', 'を', '延', '長', 'す', 'る',
    '🚀',
];

/// 16 Japanese characters, every one East Asian *Wide*: 16 `char`s, 32 columns.
const CJK_TITLE: &str = "認証トークンの有効期限を延長する";
/// 6 emoji among ASCII: 29 `char`s, 35 columns.
const EMOJI_TITLE: &str = "🚀🚀🚀 ship the rate limiter 🚀🚀🚀";
/// The control. Same shape, no wide glyph, so `chars` and columns agree — its rendering
/// must not move by a single byte.
const ASCII_TITLE: &str = "Extend the auth token expiry";

/// The independent display-width oracle: one column per character, except the enumerated
/// [`WIDE`] ones, which are two.
fn columns(s: &str) -> usize {
    strip_ansi(s)
        .chars()
        .map(|c| if WIDE.contains(&c) { 2 } else { 1 })
        .sum()
}

#[test]
fn the_width_oracle_is_honest() {
    assert_eq!(columns("t-ec64"), 6);
    assert_eq!(columns(CJK_TITLE), 32);
    assert_eq!(columns(EMOJI_TITLE), 35);
    assert_eq!(columns(ASCII_TITLE), ASCII_TITLE.chars().count());
    assert_eq!(
        columns("\u{1b}[1m認証\u{1b}[0m"),
        4,
        "escapes are not columns"
    );
    // The premise of the whole finding, as an assertion: counting `char`s is not counting
    // columns.
    assert_eq!(CJK_TITLE.chars().count(), 16);
    assert_eq!(EMOJI_TITLE.chars().count(), 29);
}

#[test]
fn visible_len_measures_columns_not_chars() {
    // ASCII is untouched — the id column, and every existing assertion in the suite,
    // depend on this half staying byte-for-byte what it always was.
    assert_eq!(visible_len("t-ec64"), 6);
    assert_eq!(visible_len(ASCII_TITLE), ASCII_TITLE.chars().count());

    // …and a wide glyph is two columns, painted or not.
    assert_eq!(visible_len(CJK_TITLE), columns(CJK_TITLE));
    assert_eq!(visible_len(EMOJI_TITLE), columns(EMOJI_TITLE));
    assert_eq!(
        visible_len(&paint(CJK_TITLE, Color::Bold, true)),
        columns(CJK_TITLE),
        "SGR escapes still occupy no columns"
    );
    assert!(
        visible_len(CJK_TITLE) > CJK_TITLE.chars().count(),
        "the bug itself: `chars().count()` under-counts a CJK title by one column per glyph"
    );

    // A combining mark is drawn ON the previous glyph, so it adds nothing: `e` + U+0301 is
    // one column, not two.
    assert_eq!(visible_len("e\u{301}"), 1);
    assert_eq!(visible_len(""), 0);

    // Padding is the point: `pad_visible` budgets through `visible_len`, so it pads by
    // COLUMNS.
    assert_eq!(columns(&pad_visible(CJK_TITLE, 40)), 40);
    assert_eq!(columns(&pad_visible("認証", 9)), 9);
    assert_eq!(pad_visible("認証", 9), "認証     ", "4 columns + 5 spaces");
}

/// THE END-TO-END HALF, against the real binary: a CJK title and an emoji title must fit
/// the terminal they are rendered for, and every right-aligned fix must still land in the
/// same column as every other one.
///
/// The arithmetic, for whoever has to debug this later: a fix that fits is padded by
/// `width - visible + 3`, so the line ends at EXACTLY `width` columns. That identity is
/// what "the arrows line up" means, and it is what the `char` count broke — `ready`
/// believed the CJK row was 16 columns narrower than it was, padded as though it fit, and
/// drew a 116-column line into a 100-column terminal.
#[test]
fn a_wide_title_renders_inside_the_column_budget_and_stays_aligned() {
    let repo = TestRepo::new();
    for title in [ASCII_TITLE, CJK_TITLE, EMOJI_TITLE] {
        repo.ks(["new", title]).ok();
    }

    let out = repo.ks_env(["ready"], &[("COLUMNS", "100")]).ok();
    for title in [ASCII_TITLE, CJK_TITLE, EMOJI_TITLE] {
        assert!(
            out.stdout.contains(title),
            "`ready` never rendered {title:?}:\n{}",
            out.stdout
        );
    }

    let mut aligned = 0usize;
    for line in out.stdout.lines() {
        assert!(
            columns(line) <= 100,
            "a {}-column line was drawn into a 100-column terminal, so it WRAPPED:\n{line}\n\
             --- full output ---\n{}",
            columns(line),
            out.stdout
        );
        // A fix that fits is right-aligned, and every such line therefore ends at the
        // width — which is the whole reason the arrows form a column.
        if line.contains('→') && !line.starts_with("    ") {
            assert_eq!(
                columns(line),
                100,
                "this fix is not in the same column as the others:\n{line}\n--- full output \
                 ---\n{}",
                out.stdout
            );
            aligned += 1;
        }
    }
    assert_eq!(
        aligned, 3,
        "all three titles must render a right-aligned fix, or this proves nothing:\n{}",
        out.stdout
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (b) invoked_as — the tool speaks the name it was called by
//
// 123 fix strings are hardcoded `fix!("kanspec …")`, so a user of the `ks` binary was told
// to run `kanspec` by every refusal outside the transition layer. `Fix::cmd` now rewrites
// the COMMAND WORD through `out::spoken`, and that is where the whole subtlety lives: fix
// strings that contain `kanspec` for another reason — a path under `.kanspec/`, a commit
// message — must come through untouched, and a naive `replace` corrupts every one of them.
// ─────────────────────────────────────────────────────────────────────────────

/// The rewrite rule, stated as a table. The first block is what MUST change; the second is
/// what must not, and every entry in it is a real fix string in `src/` today.
#[test]
fn spoken_rewrites_the_command_word_and_nothing_else() {
    for (input, want) in [
        ("kanspec doctor", "ks doctor"),
        ("kanspec done t-31aa", "ks done t-31aa"),
        ("kanspec", "ks"),
        (
            "kanspec park t-88fe --why \"...\"",
            "ks park t-88fe --why \"...\"",
        ),
        // A command quoted inside prose — both of these ship today.
        (
            "compare against `kanspec init --help` defaults",
            "compare against `ks init --help` defaults",
        ),
        (
            "ask your human to run `kanspec accept D-8c1a`",
            "ask your human to run `ks accept D-8c1a`",
        ),
    ] {
        assert_eq!(spoken_as(input, "ks"), want, "{input:?} was not rewritten");
        assert_eq!(
            spoken_as(input, "kanspec"),
            input,
            "{input:?} must be byte-identical for the `kanspec` binary"
        );
    }

    // THE HAZARD. Every one of these is a real fix string, and `replace("kanspec", "ks")`
    // mangles all of them — into `.ks/config.toml`, a commit message nobody wrote, and a
    // ticket path that does not exist.
    for untouched in [
        "git add -A .kanspec && git commit -m \"kanspec: sync\"",
        "set [ci] provider = \"none\" in .kanspec/config.toml",
        "edit .kanspec/config.toml and raise id_width",
        "edit .kanspec/specs/auth.md on this branch",
        "open /w/.kanspec/tickets/t-31aa.md and add a `---` frontmatter block",
        "git mv .kanspec/tickets/t-31aa.md .kanspec/tickets/t-9c41.md",
        "unset KANSPEC_NOW",
        "set [hooks] landcheck = false in .kanspec/config.toml",
        // The word as prose, and the word as the prefix of a longer one.
        "kanspec: sync",
        "kanspecish",
        "run kanspec-legacy instead",
    ] {
        assert_eq!(
            spoken_as(untouched, "ks"),
            untouched,
            "the rewrite CORRUPTED a fix string that says `kanspec` for another reason"
        );
    }

    // Mixed: the command word changes, the path in the same string does not.
    assert_eq!(
        spoken_as("kanspec doctor --fix .kanspec/tickets/t-31aa.md", "ks"),
        "ks doctor --fix .kanspec/tickets/t-31aa.md"
    );
}

/// `CARGO_BIN_EXE_<name>` is set for every `[[bin]]`. The shared harness knows only the
/// `kanspec` one, and the OTHER one is the entire point of this test.
fn bin(name: &str) -> &'static str {
    match name {
        "kanspec" => env!("CARGO_BIN_EXE_kanspec"),
        "ks" => env!("CARGO_BIN_EXE_ks"),
        other => panic!("no such bin: {other}"),
    }
}

/// `TestRepo::ks_in_env`'s determinism pins, applied to whichever binary is named.
fn run_bin(name: &str, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin(name))
        .current_dir(cwd)
        .args(args)
        .env("KANSPEC_NOW", common::NOW)
        .env("KANSPEC_ACTOR", common::ACTOR)
        .env("KANSPEC_ACTOR_KIND", "human")
        .env("KANSPEC_ID_SEED", common::ID_SEED)
        .env("NO_COLOR", "1")
        .env_remove("CLAUDE_SESSION_ID")
        .env_remove("CURSOR_SESSION_ID")
        .env_remove("CODEX_SESSION_ID")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("the binary must be runnable");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The `  → <command>` lines a human refusal writes to stderr.
fn fix_lines(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("→ "))
        .map(str::to_string)
        .collect()
}

#[test]
fn a_refusal_names_the_binary_the_user_actually_typed() {
    let repo = TestRepo::new();

    // The same refusal under both bins: every fix names the binary that refused, and the
    // human and `--json` surfaces agree on it — they are one string, because `Fix` is
    // `#[serde(transparent)]` and the rewrite happens once, at construction.
    for (name, other) in [("kanspec", "ks"), ("ks", "kanspec")] {
        let (code, _, stderr) = run_bin(name, &repo.root, &["show", "t-0000"]);
        assert_eq!(code, 1, "`{name} show t-0000` must refuse:\n{stderr}");
        let fixes = fix_lines(&stderr);
        assert!(!fixes.is_empty(), "no fix at all:\n{stderr}");
        for f in &fixes {
            assert!(
                f.starts_with(&format!("{name} ")),
                "`{name}` told the user to run `{f}` — it must speak its own name:\n{stderr}"
            );
            assert!(
                !f.starts_with(&format!("{other} ")),
                "`{name}` told the user to run the OTHER binary: {f:?}"
            );
        }

        let (_, stdout, _) = run_bin(name, &repo.root, &["show", "t-0000", "--json"]);
        let v: Value = serde_json::from_str(&stdout).expect("a JSON refusal envelope");
        let json: Vec<String> = v["error"]["fix"]
            .as_array()
            .expect("error.fix is an array")
            .iter()
            .map(|f| f.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            json, fixes,
            "the human and `--json` refusals named different commands"
        );
    }

    // THE GUARD, end to end. `store::read_entity` raises ONE refusal carrying both halves:
    // a `.kanspec/` path that must survive verbatim, and a `kanspec doctor` command that
    // must not. A naive `replace` turns the path into `.ks/tickets/…` — a file that does
    // not exist, inside advice whose entire promise is that it RUNS.
    repo.write(".kanspec/tickets/t-9999.md", "no frontmatter at all\n");
    let (code, _, stderr) = run_bin("ks", &repo.root, &["ls"]);
    assert_eq!(
        code, 1,
        "a ticket without frontmatter must refuse:\n{stderr}"
    );
    assert!(
        stderr
            .replace('\\', "/")
            .contains(".kanspec/tickets/t-9999.md"),
        "the `.kanspec/` PATH was corrupted by the rename:\n{stderr}"
    );
    assert!(
        !stderr.replace('\\', "/").contains(".ks/tickets"),
        "a naive replace rewrote a path into one that does not exist:\n{stderr}"
    );
    let fixes = fix_lines(&stderr);
    assert!(
        fixes.iter().any(|f| f == "ks doctor"),
        "the COMMAND half of the same refusal still says `kanspec`: {fixes:?}"
    );
    assert!(
        fixes
            .iter()
            .any(|f| f.replace('\\', "/").contains(".kanspec/tickets/t-9999.md")),
        "the PATH half of the same refusal was rewritten: {fixes:?}"
    );
}
