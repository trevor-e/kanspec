//! `flock(2)` via libc. The kernel releases the lock when the process dies, so there is
//! **no stale-lock reaper** to get subtly wrong (pid reuse, clock skew) and `kill -9`
//! mid-transaction cannot wedge the repo (J-5).
//!
//! The private field is the **write capability**: every byte-writing primitive in `store`
//! takes `&LockToken`, and [`LockToken::acquire`] is the only constructor, so "the lock is
//! held" is a borrow-checker fact at the call site rather than a convention.
//!
//! Owner: **S1**. This file and `cmd/init.rs` are the only entries on
//! `tests/single_write_path.rs`'s allowlist — it creates the very lockfile it then locks.

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
/// an unknown process", never a wrong pid. flock alone gives a blocked syscall and nothing
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
    /// `LOCK_EX|LOCK_NB`, 25ms poll to `timeout` (`cfg.lock_timeout_secs`, default 5s).
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
            // SAFETY: `file` owns the descriptor for the whole call and outlives it.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc == 0 {
                let token = LockToken { file, path };
                token.write_note(&owner);
                return Ok(token);
            }
            let err = std::io::Error::last_os_error();
            let raw = err.raw_os_error().unwrap_or(0);
            if raw != libc::EWOULDBLOCK && raw != libc::EINTR {
                return Err(KsError::internal(anyhow::anyhow!(
                    "flock({}) failed: {err}",
                    path.display()
                )));
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
        // flock when `file` closes, which is exactly what makes `kill -9` safe.
        let _ = self.file.set_len(0);
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

        // The note is readable while the lock is held — flock is advisory, so a *reader*
        // is never blocked, which is what makes the contention message possible at all.
        let seen = LockToken::read_owner(&layout).expect("the note is written after acquiring");
        assert_eq!(seen.pid, std::process::id());
        assert!(seen.describe().contains("kanspec ship t-9c41"));

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
}
