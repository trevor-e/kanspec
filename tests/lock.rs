//! `tests/lock.rs`
//!
//! Proves: 20-process contention; `kill -9` mid-transaction releases the flock
//!
//! Both properties are about *processes*, not threads, so both are tested with real ones.
//! The `kill -9` case is the whole reason `flock(2)` was chosen over a pidfile (J-5): the
//! kernel releases the lock when the process dies, so there is no stale-lock reaper to get
//! subtly wrong and no way for a killed agent to wedge the repo.
//!
//! Owner: **S1**.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::TestRepo;
use kanspec::lock::{LockOwner, LockToken};

const HOLDERS: usize = 20;

/// Set on a child process to tell it which role to play. The child re-executes THIS test
/// binary, because it is the only executable that both links the lock and can be told what
/// to do without inventing a second binary.
const ROLE: &str = "KANSPEC_TEST_LOCK_ROLE";
const ROOT: &str = "KANSPEC_TEST_LOCK_ROOT";

/// `Layout::open` is `pub(crate)` — deliberately, since `KanspecDir` is the only thing
/// that can name a file under `.kanspec/`. So a test gets one the same way every command
/// does: through `Ctx`.
fn ctx_at(root: &Path) -> kanspec::ctx::Ctx {
    common::ctx_at(root)
}

fn spawn_child(root: &Path, role: &str) -> std::process::Child {
    Command::new(std::env::current_exe().expect("the test binary's own path"))
        .args(["--exact", "--ignored", "--nocapture", "lock_child"])
        .env(ROLE, role)
        .env(ROOT, root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the child must start")
}

/// Not part of the suite. Run only by the tests below, via `--ignored --exact lock_child`.
#[test]
#[ignore]
fn lock_child() {
    let Ok(role) = std::env::var(ROLE) else {
        return; // invoked by a bare `cargo test -- --ignored`; nothing to do
    };
    let root = PathBuf::from(std::env::var(ROOT).expect("a root"));
    let ctx = ctx_at(&root);
    let layout = &ctx.layout;
    let counter = root.join("counter");

    match role.as_str() {
        // Increment a counter inside the critical section, with a gap wide enough that an
        // unlocked run loses updates essentially every time.
        "increment" => {
            let token = LockToken::acquire(
                layout,
                LockOwner::here("increment", chrono::Utc::now()),
                Duration::from_secs(30),
            )
            .expect("20 processes must all get their turn");
            let n: u64 = std::fs::read_to_string(&counter)
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            std::thread::sleep(Duration::from_millis(5));
            std::fs::write(&counter, format!("{}\n", n + 1)).unwrap();
            drop(token);
        }
        // Take the lock, announce it, then block forever waiting to be killed.
        "hold" => {
            let _token = LockToken::acquire(
                layout,
                LockOwner::here("kanspec ship t-9c41", chrono::Utc::now()),
                Duration::from_secs(30),
            )
            .expect("an uncontended lock");
            std::fs::write(root.join("held"), "1").unwrap();
            std::thread::sleep(Duration::from_secs(600));
        }
        other => panic!("unknown role {other}"),
    }
}

#[test]
fn twenty_concurrent_processes_serialise_on_one_lock() {
    let repo = TestRepo::new();
    let ctx = ctx_at(&repo.root);
    std::fs::create_dir_all(ctx.layout.cache_dir()).unwrap();
    std::fs::write(repo.root.join("counter"), "0\n").unwrap();

    let children: Vec<_> = (0..HOLDERS)
        .map(|_| spawn_child(&repo.root, "increment"))
        .collect();
    for mut c in children {
        let status = c.wait().expect("the child must finish");
        assert!(status.success(), "a contender failed: {status}");
    }

    let n: u64 = std::fs::read_to_string(repo.root.join("counter"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        n, HOLDERS as u64,
        "the critical section is read-modify-write; a lost update means the lock let two \
         processes in at once"
    );
    // Every holder released, so the note is empty again — a stale pid can never be shown.
    assert!(LockToken::read_owner(&ctx.layout).is_none());
}

#[test]
fn kill_dash_nine_mid_transaction_releases_the_lock() {
    let repo = TestRepo::new();
    let ctx = ctx_at(&repo.root);
    let layout = &ctx.layout;
    std::fs::create_dir_all(layout.cache_dir()).unwrap();

    let mut holder = spawn_child(&repo.root, "hold");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !repo.root.join("held").exists() {
        assert!(Instant::now() < deadline, "the holder never took the lock");
        std::thread::sleep(Duration::from_millis(20));
    }

    // While it is held, we are refused — with the holder NAMED, which is the whole point
    // of writing the owner note after acquiring rather than before.
    let e = LockToken::acquire(
        layout,
        LockOwner::here("kanspec status", chrono::Utc::now()),
        Duration::from_millis(120),
    )
    .unwrap_err();
    assert_eq!(e.kind(), "environment");
    assert_eq!(e.code(), Some("lock_held"));
    assert!(
        e.to_string().contains("kanspec ship t-9c41"),
        "the refusal must name the holder: {e}"
    );
    assert!(e.fixes().iter().next().is_some(), "invariant 9");

    // SIGKILL: no destructor runs, no note is truncated, no cleanup happens at all.
    let killed = Command::new("kill")
        .args(["-9", &holder.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(killed.success());
    let _ = holder.wait();

    // The kernel dropped the flock when the process died. There is no reaper here, and
    // that is exactly why `kill -9` on an agent cannot wedge the repo.
    let recovered = LockToken::acquire(
        layout,
        LockOwner::here("kanspec status", chrono::Utc::now()),
        Duration::from_secs(5),
    )
    .expect("a dead holder's lock is free immediately");
    // The DEAD process's note is still on disk — which is why `read_owner` is only ever
    // used for a message, never to decide whether the lock is free.
    drop(recovered);
}

#[test]
fn the_lockfile_lives_in_the_gitignored_cache_and_never_reaches_git() {
    let repo = TestRepo::new();
    let ctx = common::ctx_at(&repo.root);
    let token = LockToken::acquire(
        &ctx.layout,
        LockOwner::here("kanspec status", chrono::Utc::now()),
        Duration::from_secs(5),
    )
    .unwrap();
    assert!(token.path().is_file());
    assert!(
        ctx.git.is_ignored(token.path()),
        "the lockfile must be gitignored, or every transaction dirties the tree"
    );
    assert!(!ctx.git.is_tracked(token.path()));
    assert_eq!(
        ctx.git.dirty_kanspec().unwrap(),
        0,
        "taking the lock must not show up as a pending tracker change"
    );
}
