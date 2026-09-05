//! `tests/board.rs`
//!
//! Proves: the `BoardModel` — one value behind `kanspec board`, `board --export` and
//! `/api/board` — pinned by insta snapshots **whose filters were written on day one**, not
//! retrofitted after the first flake.
//!
//! What the filters normalize, and why each is load-bearing:
//!
//! * **temp-dir paths** — every fixture repo lives somewhere different;
//! * **SHAs** — a real merge into a real origin produces real, new object names;
//! * **timestamps** — `KANSPEC_NOW` freezes the *clock*, but `git commit` stamps the real
//!   one, so `last_commit_at` (and therefore every age derived from it) moves per run;
//! * **ages** — for the same reason, one run's `0s` is the next run's `1s`;
//! * **minted ids** — a `t-` id is minted against the snapshot's taken-id set, so it is
//!   swapped for a stable, same-width alias (`t-rdyy`, `t-blkd`, …) that also makes the
//!   snapshot readable without shifting a single aligned column.
//!
//! The fixture is the six REAL merge shapes plus four tickets created by real verbs, so
//! the badges in the snapshot are the ladder's actual verdicts — including the two shapes
//! that must land on `unknown` (R-4). A board that badges all six confidently is a board
//! that guesses.
//!
//! Owner: **S8**.

mod common;

use std::path::Path;

use common::TestRepo;
use serde_json::Value;

// ─────────────────────────────────────────────────────────────────────────────
// fixture
// ─────────────────────────────────────────────────────────────────────────────

struct Fixture {
    repo: TestRepo,
    /// minted id -> the stable alias the snapshots show
    aliases: Vec<(String, &'static str)>,
}

/// The fixture's clock, deliberately ahead of any wall clock that will ever run this
/// suite.
///
/// `KANSPEC_NOW` freezes *kanspec's* clock; it cannot freeze git's, and `git commit`
/// stamps real time. Every age on the board is `snap.now - max(last log entry, branch's
/// last commit)`, so with the harness's own `2026-08-31T12:00Z` the branch commits made
/// *during the test* are in kanspec's future, `Duration::to_std()` fails, and every
/// `updated_secs` flips from a number to `null` — which changes the snapshot's SHAPE, not
/// just a value, and no filter can normalize that. A clock the commits are always behind
/// makes the presence of every field deterministic; the filters normalize the magnitudes.
///
/// The side effect is deliberate and useful: at this distance every tripwire has fired, so
/// the snapshot also pins how STALLED and the dwell lines render.
const CLOCK: &str = "2099-01-01T00:00:00Z";

#[track_caller]
fn ks(repo: &TestRepo, args: &[&str]) -> common::Run {
    repo.ks_env(args, &[("KANSPEC_NOW", CLOCK)])
}

#[track_caller]
fn json(repo: &TestRepo, args: &[&str]) -> Value {
    let mut all: Vec<&str> = args.to_vec();
    all.push("--json");
    let r = ks(repo, &all);
    assert_eq!(
        r.code,
        0,
        "`kanspec {}` exited {}\n{}\n{}",
        all.join(" "),
        r.code,
        r.stdout,
        r.stderr
    );
    serde_json::from_str(&r.stdout).unwrap_or_else(|e| {
        panic!(
            "`kanspec {}` did not print JSON ({e}):\n{}",
            all.join(" "),
            r.stdout
        )
    })
}

/// The `BoardModel` itself — the value `/api/board` serves, which `kanspec board --json`
/// carries inside its report. Every assertion below is about that value, not about the
/// report wrapper, precisely because the browser and the terminal share it.
#[track_caller]
fn model(repo: &TestRepo) -> Value {
    json(repo, &["board"])["model"].clone()
}

#[track_caller]
fn new_ticket(repo: &TestRepo, args: &[&str]) -> String {
    json(repo, args)["id"]
        .as_str()
        .expect("`new` reports the minted id")
        .to_string()
}

/// The six merge shapes (six `review` tickets on `auth`) plus, by real verbs:
/// a READY ticket, a BACKLOG ticket blocked on it, a DOING ticket with a linked worktree,
/// and an UNSPECCED ticket carrying `discovered_in` — which is the ◇ chip and the shame
/// lane in one card.
fn fixture() -> Fixture {
    let repo = TestRepo::with_merges();
    // NOTE: no bare duration in this fixture's prose. The filters below normalize the
    // durations kanspec renders, and a `24h` in a feature description would be normalized
    // with them — after which the snapshot would stop meaning what it says.
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login sessions, lockout after five failures\ncode: [src/auth/**]\n---\n\
         # auth\n\n## Rules\n- [auth.jwt] Login issues a signed session token. {p-02cc}\n",
    );
    repo.git(&["add", "-A", "--", ".kanspec"]);
    repo.git(&["commit", "--quiet", "-m", "spec"]);
    repo.push("main");

    let ready = new_ticket(&repo, &["new", "Rotate the signing key", "--spec", "auth"]);
    let doing = new_ticket(&repo, &["new", "Alert wiring", "--spec", "auth"]);
    let blocked = new_ticket(
        &repo,
        &["new", "Session rotation", "--spec", "auth", "--dep", &doing],
    );
    // No `--spec`: this is the card the "unspecced" shame lane exists for. `--from` stamps
    // `discovered_in`, which is the ◇ chip every prior plan dropped.
    let discovered = new_ticket(&repo, &["new", "Retry the webhook", "--from", &doing]);

    ks(&repo, &["start", &doing, "--worktree"]).ok();

    // The ladder runs for real here: four shapes land, two stay `unknown`.
    ks(&repo, &["scan"]).ok();

    Fixture {
        repo,
        // Six characters each, exactly like a real `t-` id, so the aliases do not shift
        // a single aligned column in the terminal snapshot.
        aliases: vec![
            (ready, "t-rdyy"),
            (doing, "t-dong"),
            (blocked, "t-blkd"),
            (discovered, "t-disc"),
        ],
    }
}

/// Every filter the snapshots need, in the order they must be applied.
fn settings(f: &Fixture) -> insta::Settings {
    let mut s = insta::Settings::clone_current();

    // Paths first: the longest, most specific strings, before anything inside them can be
    // rewritten by a later filter.
    for p in fixture_paths(&f.repo) {
        s.add_filter(&escape(&p), "[repo]");
    }
    for (id, alias) in &f.aliases {
        s.add_filter(&escape(id), *alias);
    }
    // `board::short_path` elides the temp dir's own parents, so the filters above never
    // see the whole path; the tempfile stem is what survives.
    s.add_filter(r"\.tmp[A-Za-z0-9]{4,}", "[tmp]");
    // `2026-08-30T11:00Z`, `2026-08-31T12:00:00Z`.
    s.add_filter(r"\d{4}-\d{2}-\d{2}T[0-9:.+-]+Z", "[ts]");
    s.add_filter(r"\d{4}-\d{2}-\d{2}", "[date]");
    s.add_filter(r"\b[0-9a-f]{7,40}\b", "[sha]");
    // Every `derive::short` duration that can move. `KANSPEC_NOW` freezes kanspec's clock
    // but not git's, so a branch's `last_commit_at` — and every age computed from it — is
    // real wall time, and in a year's time it is a different unit again.
    //
    // Each of these is anchored rather than global: an unanchored `\d+[smhd]` filter also
    // eats the literal `DONE (7d)` column title and any duration in a spec's own prose,
    // which turns a snapshot into a lie about its own output.
    s.add_filter(r"\b\d+(s|m|h|d|mo) ago\b", "[age] ago");
    s.add_filter(r"in main \d+(s|m|h|d|mo) \(", "in main [age] (");
    // A markdown table cell holds the value alone; a comfy-table cell holds it plus the
    // padding that its own width decides, so both the value AND the padding collapse — the
    // ahead/behind column right after it is the anchor.
    s.add_filter(r"\| \d+(s|m|h|d|mo) \|", "| [age] |");
    s.add_filter(r"\s+(?:\d+(?:s|m|h|d|mo)|—)\s+(\+\d+/-\d+)", " [age] $1");
    // Whatever is left — the dwell and STALLED wordings, which each spell their duration
    // differently. A duration opening a parenthesis is NOT one of them: that is the
    // literal `DONE (7d)` column title, and eating it would make the snapshot claim the
    // board prints a window it does not.
    s.add_filter(r"([ ·])\d+(?:s|m|h|d|mo)([ ,:·)\n])", "$1[age]$2");
    for key in [
        "updated_secs",
        "cache_age_secs",
        "last_commit_secs",
        "stalled_secs",
    ] {
        s.add_filter(&format!("\"{key}\": \\d+"), format!("\"{key}\": \"[age]\""));
    }
    s.set_prepend_module_to_snapshot(false);
    s
}

/// `regex::escape`, hand-rolled: `regex` is insta's dependency, not ours, and Rule 5 says
/// no slice edits `Cargo.toml`. Only the metacharacters the `regex` crate actually accepts
/// a backslash in front of are escaped.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The temp-dir path in every form it can reach a snapshot: as `git` reports it, and as
/// `Repo::discover` canonicalizes it (macOS `/var` -> `/private/var`).
fn fixture_paths(repo: &TestRepo) -> Vec<String> {
    let mut out = Vec::new();
    let tmp = repo.root.parent().unwrap_or(Path::new("/")).to_path_buf();
    // CI may put TEMP behind an 8.3 alias (RUNNER~1). Git reports the expanded
    // path without Rust's verbatim prefix, so include that spelling as well.
    let primary = kanspec::paths::Repo::discover(&repo.root, None).unwrap();
    for p in [
        std::fs::canonicalize(&tmp).unwrap_or_else(|_| tmp.clone()),
        primary.primary_root().parent().unwrap().to_path_buf(),
        tmp,
    ] {
        let s = p.to_string_lossy().to_string();
        if !out.contains(&s) {
            out.push(s.clone());
        }
        for spelling in [s.replace('\\', "/"), s.replace('\\', "\\\\")] {
            if !out.contains(&spelling) {
                out.push(spelling);
            }
        }
    }
    // Longest first, so `/private/var/…` is rewritten before `/var/…` can nibble at it.
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// the snapshots
// ─────────────────────────────────────────────────────────────────────────────

/// The wire shape `/api/board` serves and `kanspec board --json` prints — the same value.
#[test]
fn the_board_model_is_stable() {
    let f = fixture();
    let board = model(&f.repo);
    settings(&f).bind(|| {
        insta::assert_json_snapshot!("board_model", board);
    });
}

/// The cold fallback: the terminal board, with the cached git state AND its age.
#[test]
fn the_terminal_board_is_stable() {
    let f = fixture();
    let r = ks(&f.repo, &["board"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    settings(&f).bind(|| {
        insta::assert_snapshot!("board_terminal", r.stdout);
    });
}

/// `board --export board.md` — the snapshot a human pastes into a PR.
#[test]
fn the_markdown_export_is_stable() {
    let f = fixture();
    let r = ks(&f.repo, &["board", "--export", "board.md"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(f.repo.exists("board.md"), "--export wrote nothing");
    settings(&f).bind(|| {
        insta::assert_snapshot!("board_export", f.repo.read("board.md"));
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// the properties a snapshot alone would not pin
// ─────────────────────────────────────────────────────────────────────────────

/// **Never a guess.** Every card's badge is one of exactly six shapes, and the two merge
/// shapes that defeat all four rungs must say `unknown` with a reason rather than pick a
/// side (R-4, D-3).
#[test]
fn every_badge_is_one_of_the_six_shapes_and_two_of_them_are_honestly_unknown() {
    let f = fixture();
    let board = model(&f.repo);
    const SHAPES: &[&str] = &[
        "unpushed",
        "pushed",
        "pr_open",
        "in_main",
        "unknown",
        "never_scanned",
    ];

    // The six shape tickets are the ones with a real merge behind them; the four created
    // by verbs in this fixture have never been pushed and are not what R-4 is about.
    const SHAPE_TICKETS: &[&str] = &["t-9c41", "t-88fe", "t-dddd", "t-bbbb", "t-eeee", "t-cccc"];

    let mut unknowns = 0;
    let mut in_main = 0;
    let mut seen = 0;
    for col in board["columns"].as_array().expect("columns") {
        for card in col["cards"].as_array().expect("cards") {
            seen += 1;
            let badge = card["badge"]["badge"].as_str().expect("a badge tag");
            let shape = card["id"]
                .as_str()
                .is_some_and(|id| SHAPE_TICKETS.contains(&id));
            assert!(SHAPES.contains(&badge), "invented badge {badge}: {card}");
            let text = card["badge_text"].as_str().expect("badge text");
            assert!(!text.is_empty(), "a badge with no text: {card}");
            match badge {
                "unknown" => {
                    unknowns += usize::from(shape);
                    // The reason is the whole value of an `unknown` — and it must not be
                    // the doubled `unknown (unknown (…))` D-38 fixed at the seam.
                    assert!(
                        text.starts_with("unknown (") && !text.contains("unknown (unknown"),
                        "an unknown badge must carry ONE bare reason: {text}"
                    );
                }
                "in_main" => in_main += usize::from(shape),
                _ => {}
            }
        }
    }
    assert!(seen >= 10, "the fixture lost cards: {seen}");
    assert_eq!(
        unknowns, 2,
        "exactly two of the six merge shapes defeat every rung (R-4)"
    );
    assert_eq!(in_main, 4, "the other four land, and the board says so");
}

/// The six columns, in DESIGN.md's order, with `dropped` deliberately absent.
#[test]
fn the_columns_are_design_mds_columns_and_the_cards_are_where_git_says() {
    let f = fixture();
    let board = model(&f.repo);
    let cols = board["columns"].as_array().expect("columns");
    let titles: Vec<&str> = cols.iter().map(|c| c["title"].as_str().unwrap()).collect();
    assert_eq!(
        titles,
        vec![
            "BACKLOG",
            "READY",
            "DOING",
            "REVIEW",
            "IN MAIN ⇂",
            "DONE (7d)"
        ]
    );

    let find = |id: &str| -> String {
        for c in cols {
            for card in c["cards"].as_array().unwrap() {
                if card["id"] == id {
                    return c["column"].as_str().unwrap().to_string();
                }
            }
        }
        panic!("{id} is on no column:\n{board}");
    };
    let alias = |a: &str| -> String {
        f.aliases
            .iter()
            .find(|(_, x)| *x == a)
            .map(|(id, _)| id.clone())
            .unwrap()
    };

    assert_eq!(find(&alias("t-rdyy")), "ready");
    assert_eq!(find(&alias("t-dong")), "doing");
    // Blocked on a ticket nobody has finished: BACKLOG, not READY.
    assert_eq!(find(&alias("t-blkd")), "backlog");
    // IN MAIN is a derived overlay over `review`, computed from git and nothing else.
    assert_eq!(find("t-9c41"), "in_main");
    assert_eq!(find("t-cccc"), "review");
}

/// The ◇ discovered chip and the "unspecced" lane — DESIGN's two mechanisms for keeping
/// rabbit-hole captures visible, both dropped by every prior plan.
#[test]
fn a_discovered_ticket_keeps_its_link_and_lands_in_the_unspecced_lane() {
    let f = fixture();
    let board = model(&f.repo);
    let doing = f
        .aliases
        .iter()
        .find(|(_, a)| *a == "t-dong")
        .unwrap()
        .0
        .clone();

    let mut found = false;
    let mut unspecced = 0;
    for col in board["columns"].as_array().unwrap() {
        for card in col["cards"].as_array().unwrap() {
            if card["spec"].is_null() {
                unspecced += 1;
            }
            if card["discovered_in"].as_str() == Some(doing.as_str()) {
                found = true;
                assert!(
                    card["spec"].is_null(),
                    "the fixture's discovered ticket has no capability yet"
                );
            }
        }
    }
    assert!(
        found,
        "the ◇ chip's source field never reached a card:\n{board}"
    );
    assert_eq!(
        unspecced, 1,
        "the shame lane holds exactly the unspecced card"
    );

    // Both surfaces name it, so neither can quietly drop it.
    let md = {
        ks(&f.repo, &["board", "--export", "board.md"]).ok();
        f.repo.read("board.md")
    };
    assert!(md.contains("## Unspecced (1)"), "{md}");
    assert!(ks(&f.repo, &["board"])
        .ok()
        .stdout
        .contains("◇ discovered in"));
}

/// DESIGN.md's Worktrees tab: one row per checkout with the ahead/behind-main figure from
/// `git rev-list --left-right --count`, which every prior plan omitted.
#[test]
fn the_worktrees_tab_carries_ahead_behind_main_for_every_row() {
    let f = fixture();
    let board = model(&f.repo);
    let rows = board["worktrees"].as_array().expect("worktrees");
    assert!(
        rows.len() >= 2,
        "the fixture has a linked worktree: {rows:?}"
    );

    let claimed = rows
        .iter()
        .find(|r| !r["ticket"].is_null())
        .expect("the linked worktree resolves to its ticket");
    assert!(
        claimed["ahead"].is_number() && claimed["behind"].is_number(),
        "the ahead/behind figure is the point of this tab: {claimed}"
    );
    assert!(claimed["branch"].as_str().unwrap().starts_with("ks/"));
    assert!(!claimed["badge_text"].as_str().unwrap().is_empty());
    assert_eq!(rows[0]["primary"], Value::Bool(true));
}

/// The cold path: with `cache/` wiped, the board still renders — and says out loud that
/// nothing has been scanned rather than showing badges that look current.
#[test]
fn a_wiped_cache_makes_the_board_say_so_instead_of_guessing() {
    let f = fixture();
    std::fs::remove_dir_all(f.repo.root.join(".kanspec/cache")).expect("the cache is wipeable");

    let r = ks(&f.repo, &["board"]);
    assert_eq!(
        r.code, 0,
        "the board must survive a cache wipe:\n{}",
        r.stderr
    );
    assert!(
        r.stdout.contains("merge state never scanned"),
        "a board with no facts must say so:\n{}",
        r.stdout
    );

    let board = model(&f.repo);
    assert!(board["cache_age_secs"].is_null());
    for col in board["columns"].as_array().unwrap() {
        for card in col["cards"].as_array().unwrap() {
            let badge = card["badge"]["badge"].as_str().unwrap();
            assert!(
                badge != "in_main",
                "a wiped cache cannot leave a ticket claiming to be merged: {card}"
            );
        }
    }
}

/// `board` is a read, and every read supports `--json`; `--export` is a write, and every
/// write goes through `Store::transact`. The second half is what keeps the export honest
/// when the lock is contended.
#[test]
fn export_writes_through_the_store_and_reports_where() {
    let f = fixture();
    let out = json(&f.repo, &["board", "--export", "docs/board.md"]);
    let path = out["exported"].as_str().expect("--export reports its path");
    assert!(path.ends_with("docs/board.md"), "{path}");
    assert!(
        f.repo.exists("docs/board.md"),
        "--export must create the parent directory, like every other staged write"
    );
    // The human rendering of an export is the receipt, not a second copy of the board.
    let r = ks(&f.repo, &["board", "--export", "docs/board.md"]);
    assert!(r.stdout.contains("wrote"), "{}", r.stdout);
    assert!(!r.stdout.contains("BACKLOG"), "{}", r.stdout);
}

// ─────────────────────────────────────────────────────────────────────────────
// `kanspec up` — the live board
//
// A hand-rolled HTTP client, deliberately: Rule 5 says no slice edits `Cargo.toml`, and
// four one-shot loopback requests plus one SSE stream do not need a client crate. Every
// one-shot request sends `Connection: close`, so `read_to_end` terminates on the response.
// ─────────────────────────────────────────────────────────────────────────────

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Server {
    child: Child,
    port: u16,
}

impl Server {
    /// Spawns the REAL binary — the same `kanspec up` a human runs — against `repo`.
    fn start(repo: &TestRepo) -> Server {
        let port = free_port();
        let child = Command::new(env!("CARGO_BIN_EXE_kanspec"))
            .current_dir(&repo.root)
            .args(["up", "--port", &port.to_string()])
            .env("KANSPEC_NOW", CLOCK)
            .env("KANSPEC_ACTOR", common::ACTOR)
            .env("KANSPEC_ACTOR_KIND", "human")
            .env("KANSPEC_ID_SEED", common::ID_SEED)
            .env("KANSPEC_NO_BROWSER", "1")
            .env("NO_COLOR", "1")
            .env_remove("CLAUDE_SESSION_ID")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the binary must be runnable");

        let s = Server { child, port };
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return s;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("`kanspec up` never started listening on {port}");
    }

    fn get(&self, path: &str) -> (u16, String) {
        request(self.port, "GET", path, None)
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        request(self.port, "POST", path, Some(body))
    }

    fn json(&self, path: &str) -> Value {
        let (code, body) = self.get(path);
        assert_eq!(code, 200, "GET {path} -> {code}\n{body}");
        serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("GET {path} is not JSON ({e}):\n{body}"))
    }

    /// SIGINT, then the wall-clock time to exit.
    #[cfg(unix)]
    fn interrupt(&mut self) -> Duration {
        let started = Instant::now();
        let signalled = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(signalled, "could not signal the server");
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return started.elapsed(),
                Ok(None) if started.elapsed() > Duration::from_secs(20) => {
                    panic!("`kanspec up` ignored SIGINT")
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => panic!("waiting on the server failed: {e}"),
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Ask the OS for a port, then let go of it. A loopback dev server is the one place this
/// is honest: nothing else on the box is racing for a random high port.
fn free_port() -> u16 {
    let l = TcpListener::bind(("127.0.0.1", 0)).expect("a free port");
    l.local_addr().expect("a bound address").port()
}

fn request(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("the server is listening");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).expect("the request is sent");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("the response arrives");
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("not an HTTP response:\n{text}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

/// `kanspec up` serves the same `BoardModel` the terminal renders, plus the embedded SPA.
#[test]
fn up_serves_the_board_and_the_embedded_spa() {
    let f = fixture();
    let server = Server::start(&f.repo);

    // The API and the CLI are the same value, not two renderings of two computations.
    let over_http = server.json("/api/board");
    let over_cli = model(&f.repo);
    assert_eq!(
        over_http["columns"], over_cli["columns"],
        "the browser and the terminal disagree about the board"
    );
    assert_eq!(over_http["worktrees"], over_cli["worktrees"]);

    // The SPA is embedded in the binary, not read off disk.
    let (code, html) = server.get("/");
    assert_eq!(code, 200);
    assert!(html.contains("<title>kanspec</title>"), "{html}");
    assert!(html.contains("/app.js"), "{html}");
    for asset in ["/app.js", "/style.css"] {
        let (code, body) = server.get(asset);
        assert_eq!(code, 200, "{asset} -> {code}");
        assert!(!body.is_empty(), "{asset} is empty");
    }
    // A deep link the SPA owns serves the shell; a mistyped API path does not.
    assert_eq!(server.get("/t/t-9c41").0, 200);
    assert_eq!(server.get("/api/nope").0, 404);

    // The other read endpoints the page and an agent may ask for.
    assert!(server.json("/api/status")["you"].is_array());
    assert!(server.json("/api/rules").is_object());
    assert_eq!(server.json("/api/ticket/t-9c41")["id"], "t-9c41");
}

/// **THE server invariant.** A POST and the equivalent CLI verb call the same `cmd::*`
/// function through the same `Store::transact`, so the ticket file they write is
/// byte-identical — not merely equivalent.
#[test]
fn a_post_and_the_equivalent_cli_verb_write_byte_identical_files() {
    let through_http = {
        let repo = plain_repo();
        let id = new_ticket_in(&repo, "Rate-limit login endpoint");
        let server = Server::start(&repo);
        let (code, body) = server.post(&format!("/api/ticket/{id}/start"), "{}");
        assert_eq!(code, 200, "POST start -> {code}\n{body}");
        (id.clone(), repo.read(&format!(".kanspec/tickets/{id}.md")))
    };
    let through_cli = {
        let repo = plain_repo();
        let id = new_ticket_in(&repo, "Rate-limit login endpoint");
        ks(&repo, &["start", &id]).ok();
        (id.clone(), repo.read(&format!(".kanspec/tickets/{id}.md")))
    };

    // Minting is deterministic under a fixed seed against the same taken-id set, so the
    // two repos even agree on the id — which is what makes a byte comparison meaningful.
    assert_eq!(
        through_http.0, through_cli.0,
        "the two runs minted different ids"
    );
    assert_eq!(
        through_http.1, through_cli.1,
        "the board and the CLI wrote different bytes for the same verb"
    );
    // And it really did transition — a test that compares two unchanged files proves
    // nothing.
    assert!(
        through_http.1.contains("state: doing"),
        "{}",
        through_http.1
    );
    assert!(through_http.1.contains(" start"), "{}", through_http.1);
}

/// A gate refusal over HTTP is the CLI's own envelope, fix list included — an agent
/// hitting the API and an agent running the CLI read identical refusals.
#[test]
fn an_illegal_move_bounces_with_the_typed_reason() {
    let repo = plain_repo();
    let id = new_ticket_in(&repo, "Rate-limit login endpoint");
    let server = Server::start(&repo);

    // `todo -> ship` is not in the transition table.
    let (code, body) = server.post(&format!("/api/ticket/{id}/ship"), "{}");
    assert_eq!(code, 409, "a refusal is a conflict, not a 500:\n{body}");
    let v: Value = serde_json::from_str(&body).expect("a JSON envelope");
    assert_eq!(v["ok"], Value::Bool(false));
    assert_eq!(v["error"]["kind"], "illegal_transition");
    assert!(
        !v["error"]["fix"].as_array().expect("a fix list").is_empty(),
        "invariant 9 holds over HTTP too: {v}"
    );

    // A ticket that does not exist is the same shape, never an HTML page.
    let (code, body) = server.post("/api/ticket/t-0000/start", "{}");
    assert_eq!(code, 409, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["error"]["kind"],
        "not_found"
    );
}

/// The live half: a CLI verb run in another terminal reaches an open SSE stream, and
/// re-fetching then shows the move.
#[test]
fn a_cli_verb_in_another_terminal_pushes_a_tick_to_an_open_sse_stream() {
    let repo = plain_repo();
    let id = new_ticket_in(&repo, "Rate-limit login endpoint");
    let server = Server::start(&repo);

    let mut sse = TcpStream::connect(("127.0.0.1", server.port)).expect("the server listens");
    sse.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    sse.write_all(b"GET /events HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n")
        .unwrap();
    // Drain the response head, so the stream is certainly subscribed before the verb runs.
    let mut head = [0u8; 256];
    let n = sse.read(&mut head).expect("the SSE head arrives");
    let head = String::from_utf8_lossy(&head[..n]).into_owned();
    assert!(head.contains("200"), "{head}");
    assert!(head.contains("text/event-stream"), "{head}");

    // …another terminal.
    ks(&repo, &["start", &id]).ok();

    let mut seen = String::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && !seen.contains("event: tick") {
        let mut buf = [0u8; 1024];
        match sse.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(_) => break,
        }
    }
    assert!(
        seen.contains("event: tick"),
        "the fs-watcher never told the browser anything:\n{seen}"
    );
    // D-23: the payload is `{rev, n}` and nothing else — the page refetches.
    assert!(seen.contains("\"rev\""), "{seen}");
    assert!(seen.contains("\"n\""), "{seen}");

    // And the refetch shows the move the other terminal made.
    let board = server.json("/api/board");
    let doing = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["column"] == "doing")
        .expect("a doing column");
    assert!(
        doing["cards"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == id.as_str()),
        "the live board did not pick up the CLI's claim:\n{doing}"
    );
}

/// Ctrl-C is instant **with an SSE client attached** — which is the case that breaks a
/// naive `with_graceful_shutdown`, because an SSE response never completes on its own.
#[test]
#[cfg(unix)]
fn ctrl_c_exits_in_under_a_second_even_with_an_sse_client_connected() {
    let repo = plain_repo();
    let mut server = Server::start(&repo);

    let mut sse = TcpStream::connect(("127.0.0.1", server.port)).expect("the server listens");
    sse.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    sse.write_all(b"GET /events HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n")
        .unwrap();
    let mut head = [0u8; 128];
    let _ = sse.read(&mut head);

    let elapsed = server.interrupt();
    assert!(
        elapsed < Duration::from_millis(1500),
        "Ctrl-C took {elapsed:?} with an SSE client attached — it must be immediate"
    );
}

/// A port already in use prints a typed refusal that names its fix. It must not panic, and
/// it must not print the listening banner first.
#[test]
fn a_busy_port_is_a_refusal_that_names_its_fix() {
    let repo = plain_repo();
    let server = Server::start(&repo);

    let r = repo.ks_env(
        ["up", "--port", &server.port.to_string(), "--json"],
        &[("KANSPEC_NOW", CLOCK), ("KANSPEC_NO_BROWSER", "1")],
    );
    assert_ne!(
        r.code, 0,
        "a busy port must not look like success:\n{}",
        r.stdout
    );
    let v: Value = serde_json::from_str(&r.stdout)
        .unwrap_or_else(|e| panic!("a refusal must still be JSON ({e}):\n{}", r.stdout));
    assert_eq!(v["error"]["code"], "port_in_use", "{v}");
    let fixes = v["error"]["fix"].as_array().expect("a fix list");
    assert!(
        fixes
            .iter()
            .any(|f| f.as_str().unwrap_or("").contains("--port")),
        "{v}"
    );
    assert!(
        !r.stderr.contains("serving the board"),
        "the banner must not print before the socket is ours:\n{}",
        r.stderr
    );
}

/// A repo with no merge fixtures — cheap, and enough for anything about the write path.
fn plain_repo() -> TestRepo {
    let repo = TestRepo::new();
    repo.write(
        ".kanspec/specs/auth.md",
        "---\nfeature: Login sessions\ncode: [src/auth/**]\n---\n# auth\n\n## Rules\n",
    );
    repo.git(&["add", "-A", "--", ".kanspec"]);
    repo.git(&["commit", "--quiet", "-m", "spec"]);
    repo.push("main");
    repo
}

fn new_ticket_in(repo: &TestRepo, title: &str) -> String {
    new_ticket(repo, &["new", title, "--spec", "auth"])
}

/// t-ec60: the Review-queue tab. A proposal in review is a row with its unresolved count
/// and the one command that moves it; `status`'s YOU line and this row read the same
/// `derive::unresolved`, so they cannot disagree.
#[test]
fn the_review_queue_lists_proposals_in_review_with_their_open_threads() {
    let repo = TestRepo::new();
    ks(
        &repo,
        &[
            "spec",
            "new",
            "auth",
            "--feature",
            "Login",
            "--code",
            "src/**",
        ],
    )
    .ok();
    ks(&repo, &["propose", "Login rate limiting", "--spec", "auth"]).ok();
    let dir = std::fs::read_dir(repo.root.join(".kanspec/proposals"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with("p-"))
        .expect("a proposal directory");
    let id = dir[..6].to_string();
    let rel = format!(".kanspec/proposals/{dir}/proposal.md");
    let src = repo.read(&rel);
    let (fm, _) = src.split_once("\n---\n").expect("frontmatter");
    repo.write(
        &rel,
        &format!("{fm}\n---\n## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n\n## Prescriptions\n\n## Tickets\n"),
    );

    // A draft is nobody's to approve: not queued.
    assert!(model(&repo)["review_queue"].as_array().unwrap().is_empty());

    ks(&repo, &["review", &id]).ok();
    let q = model(&repo)["review_queue"].as_array().unwrap().clone();
    assert_eq!(q.len(), 1, "{q:?}");
    assert_eq!(q[0]["id"], id);
    assert_eq!(q[0]["unresolved"], 0);
    assert_eq!(q[0]["fix"], format!("kanspec approve {id}"), "{q:?}");
    assert_eq!(q[0]["specs"], serde_json::json!(["auth"]));

    // An open thread: the count moves and the fix becomes the page, where the thread is.
    ks(
        &repo,
        &["comment", "add", &format!("{id}#c1"), "--body", "too broad"],
    )
    .ok();
    let q = model(&repo)["review_queue"].as_array().unwrap().clone();
    assert_eq!(q[0]["unresolved"], 1, "{q:?}");
    assert!(
        q[0]["fix"].as_str().unwrap().contains(&format!("/p/{id}")),
        "{q:?}"
    );

    // The terminal and markdown boards carry the same queue.
    let term = ks(&repo, &["board"]).ok().stdout;
    assert!(
        term.contains("REVIEW QUEUE") && term.contains("1 open"),
        "{term}"
    );
    ks(&repo, &["board", "--export", "board.md"]).ok();
    let md = repo.read("board.md");
    assert!(
        md.contains("## Review queue (1)") && md.contains(&id),
        "{md}"
    );

    // Approved: gone from the queue.
    let cm: Value = json(&repo, &["comments"]);
    let cid = cm["threads"][0]["id"].as_str().unwrap().to_string();
    ks(&repo, &["comment", "resolve", &cid, "--note", "narrowed"]).ok();
    ks(&repo, &["approve", &id]).ok();
    assert!(model(&repo)["review_queue"].as_array().unwrap().is_empty());
}
