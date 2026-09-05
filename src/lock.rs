//! Native file locks: `flock(2)` on Unix, `LockFileEx` on Windows. The kernel
//! releases the lock when the process dies, so there is
//! **no stale-lock reaper** to get subtly wrong (pid reuse, clock skew) and `kill -9`
//! mid-transaction cannot wedge the repo (J-5).
//!
//! The private field is the **write capability**: every byte-writing primitive in `store`
//! takes `&LockToken`, and [`LockToken::acquire`] is the only constructor, so "the lock is
//! held" is a borrow-checker fact at the call site rather than a convention.
//!
//! Owner: **S1**. This file and `cmd/init.rs` are the only entries on
//! `tests/single_write_path.rs`'s allowlist — it creates the very lockfile it then locks.

#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{EnvCode, KsError, Result};
use crate::paths::Layout;
use crate::{fix, fixes};

/// How often we retry `LOCK_EX|LOCK_NB`. Short enough that a fast transaction behind
/// another fast transaction is imperceptible; long enough not to spin a core.
const POLL: Duration = Duration::from_millis(25);

#[derive(Debug)]
pub struct LockToken {
    file: std::fs::File,
    path: PathBuf,
}

/// Written AFTER acquiring, so a reader may legitimately see it empty -> render "held by
/// an unknown process", never a wrong pid. A lock alone gives a blocked syscall and nothing
/// to print, but invariant 9 demands a fix line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockOwner {
    pub pid: u32,
    pub host: String,
    pub cmd: String,
    pub at: DateTime<Utc>,
}

impl LockOwner {
    /// The note this process would write: its own pid, host and invocation.
    pub fn here(cmd: impl Into<String>, at: DateTime<Utc>) -> LockOwner {
        LockOwner {
            pid: std::process::id(),
            host: hostname(),
            cmd: cmd.into(),
            at,
        }
    }

    /// `"pid 4821 on rimu running `kanspec ship t-9c41`"` — the contention message.
    pub fn describe(&self) -> String {
        format!(
            "pid {} on {} running `{}` since {}",
            self.pid,
            self.host,
            self.cmd,
            self.at.format("%Y-%m-%dT%H:%M:%SZ")
        )
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "localhost".to_string())
}

impl LockToken {
    /// Nonblocking exclusive lock, 25ms poll to `timeout` (default 5s).
    pub fn acquire(layout: &Layout, owner: LockOwner, timeout: Duration) -> Result<LockToken> {
        let path = layout.lock();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| {
                KsError::internal(anyhow::anyhow!("cannot create {}: {e}", dir.display()))
            })?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| {
                KsError::internal(anyhow::anyhow!("cannot open {}: {e}", path.display()))
            })?;

        let deadline = Instant::now() + timeout;
        loop {
            if try_lock(&file).map_err(|err| {
                KsError::internal(anyhow::anyhow!("lock({}) failed: {err}", path.display()))
            })? {
                let token = LockToken { file, path };
                token.write_note(&owner);
                return Ok(token);
            }
            if Instant::now() >= deadline {
                let held = LockToken::read_owner(layout)
                    .map(|o| o.describe())
                    .unwrap_or_else(|| "an unknown process".to_string());
                return Err(KsError::environment(
                    EnvCode::LockHeld,
                    format!(
                        "another kanspec write is in flight ({held}); waited {}s for {}",
                        timeout.as_secs(),
                        path.display()
                    ),
                    fixes![
                        fix!("wait for it to finish and re-run"),
                        fix!("kanspec doctor"),
                    ],
                ));
            }
            std::thread::sleep(POLL);
        }
    }

    /// Who currently holds it, for the contention message. `None` renders as "held by an
    /// unknown process" — the note is written after acquiring, so empty is legitimate.
    pub fn read_owner(layout: &Layout) -> Option<LockOwner> {
        // An empty or half-written note simply fails to parse: no owner, never a wrong one.
        serde_json::from_str(&std::fs::read_to_string(layout.lock()).ok()?).ok()
    }

    /// The lockfile, for diagnostics. Nothing may write to it except this module.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn write_note(&self, owner: &LockOwner) {
        // Best-effort: a note we could not write costs a nicer error message, never the
        // transaction. The flock itself is the mutual exclusion.
        use std::io::{Seek, Write};
        let Ok(json) = serde_json::to_string(owner) else {
            return;
        };
        let mut f = &self.file;
        let _ = f.set_len(0);
        let _ = f.seek(std::io::SeekFrom::Start(0));
        let _ = f.write_all(json.as_bytes());
        let _ = f.flush();
    }
}

impl Drop for LockToken {
    fn drop(&mut self) {
        // Truncating the note is the only cleanup that matters: the kernel drops the
        // native lock when `file` closes, including after forced process termination.
        let _ = self.file.set_len(0);
    }
}

#[cfg(unix)]
fn try_lock(file: &std::fs::File) -> std::io::Result<bool> {
    // SAFETY: the file owns the descriptor and outlives this synchronous call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EWOULDBLOCK | libc::EINTR) => Ok(false),
        _ => Err(err),
    }
}

#[cfg(windows)]
fn try_lock(file: &std::fs::File) -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    // Lock one byte well beyond the owner note. Windows locks are mandatory, so
    // locking its contents would prevent other sessions from reading the holder.
    // A lock beyond EOF does not extend the file and is released on handle close.
    let mut overlapped = OVERLAPPED::default();
    overlapped.Anonymous.Anonymous.Offset = 0;
    overlapped.Anonymous.Anonymous.OffsetHigh = 0x7fff_ffff;
    // SAFETY: the handle and initialized OVERLAPPED outlive this synchronous,
    // nonblocking call; the file was opened with read/write access.
    let locked = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        )
    };
    if locked != 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        Ok(false)
    } else {
        Err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn layout_in(dir: &std::path::Path) -> Layout {
        // A `Layout` can only come from a `Repo`, and a `Repo` can only come from git —
        // so the fixture is a real (empty) repository, which is also the honest one.
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(dir)
            .status()
            .expect("git must be runnable")
            .success());
        let repo = crate::paths::Repo::discover(dir, None).expect("a fresh repo is discoverable");
        Layout::open(&repo, &Config::default())
    }

    #[test]
    fn the_second_acquirer_is_refused_with_the_holder_named() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = layout_in(tmp.path());

        let owner = LockOwner::here("kanspec ship t-9c41", Utc::now());
        let held = LockToken::acquire(&layout, owner, Duration::from_millis(50)).unwrap();
        assert!(held.path().exists());

        // The note stays readable while either platform's lock is held.
        let seen = LockToken::read_owner(&layout).expect("the note is written after acquiring");
        assert_eq!(seen.pid, std::process::id());
        assert!(seen.describe().contains("kanspec ship t-9c41"));

        let contender = LockToken::acquire(
            &layout,
            LockOwner::here("competing writer", Utc::now()),
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(contender.to_string().contains("kanspec ship t-9c41"));
        assert_eq!(LockToken::read_owner(&layout).unwrap().pid, seen.pid);
        assert!(std::fs::metadata(held.path()).unwrap().len() < 4096);

        drop(held);
        // Dropping truncates the note, so a stale pid can never be reported.
        assert!(LockToken::read_owner(&layout).is_none());
    }

    #[test]
    fn a_released_lock_is_immediately_reacquirable() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = layout_in(tmp.path());
        for i in 0..5 {
            let t = LockToken::acquire(
                &layout,
                LockOwner::here(format!("kanspec run {i}"), Utc::now()),
                Duration::from_millis(200),
            )
            .unwrap_or_else(|e| panic!("acquire {i} failed: {e}"));
            drop(t);
        }
    }

    #[test]
    fn an_empty_or_corrupt_note_reads_as_unknown_never_as_a_wrong_pid() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = layout_in(tmp.path());
        std::fs::create_dir_all(layout.cache_dir()).unwrap();
        for body in ["", "   \n", "{not json", "{}"] {
            std::fs::write(layout.lock(), body).unwrap();
            assert!(
                LockToken::read_owner(&layout).is_none(),
                "note {body:?} must not yield an owner"
            );
        }
    }

    #[test]
    #[ignore = "child process for the crash-release test"]
    fn lock_holder_child() {
        let Some(root) = std::env::var_os("KANSPEC_TEST_LOCK_ROOT") else {
            return;
        };
        let root = std::path::Path::new(&root);
        let layout = layout_in(root);
        let _held = LockToken::acquire(
            &layout,
            LockOwner::here("child lock holder", Utc::now()),
            Duration::from_secs(2),
        )
        .unwrap();
        std::fs::write(root.join("holder.ready"), "ready").unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn killing_the_holder_releases_the_lock_without_a_stale_lock_reaper() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let layout = layout_in(tmp.path());
        let mut child = ChildGuard(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lock::tests::lock_holder_child", "--ignored"])
                .env("KANSPEC_TEST_LOCK_ROOT", tmp.path())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !tmp.path().join("holder.ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "lock holder exited early"
            );
            assert!(
                Instant::now() < deadline,
                "lock holder did not become ready"
            );
            std::thread::sleep(POLL);
        }
        let refusal = LockToken::acquire(
            &layout,
            LockOwner::here("parent contender", Utc::now()),
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(refusal.to_string().contains("child lock holder"));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let recovered = LockToken::acquire(
            &layout,
            LockOwner::here("recovered writer", Utc::now()),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(
            LockToken::read_owner(&layout).unwrap().cmd,
            "recovered writer"
        );
        drop(recovered);
        assert!(LockToken::read_owner(&layout).is_none());
    }
}
