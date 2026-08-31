//! Git hook installation, resolved through `git rev-parse --git-path hooks` (D-7):
//! `core.hooksPath` (husky, lefthook) makes `.git/hooks` inert, so hardcoding it silently
//! installs nothing.
//!
//! kanspec installs itself as the **entrypoint** and moves any pre-existing hook to
//! `<hook>.d/10-<name>`. A naive append is unsafe two ways — `exit 0` starvation, and a
//! missing trailing newline welding two scripts into one line.
//!
//! Git has **no per-branch hooks** (D-5): the message hooks are repo-wide, dispatch on
//! `git symbolic-ref --short HEAD` -> [`BRANCH_TICKET_KEY`], and skip
//! `$2 ∈ {merge, squash, commit}`. They are installed by `init`, not by `start`, and on a
//! branch with no claim they cost one `git config` read and exit.
//!
//! Owner: **S7**. Writes only under the git dir, never under the store.
//!
//! **Where the bytes actually move.** This file *plans* — it reads the hooks directory and
//! produces a `Vec<Edit>` — and `cmd::init::apply` is the one function that moves a byte
//! for this slice, mirroring the planner/applier split `Store::transact` uses for the
//! store. That is not decoration: `tests/single_write_path.rs` allowlists exactly
//! `lock.rs` and `cmd/init.rs`, so a second writer here would be a second write path.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::{fix, fixes};

/// Every git hook kanspec owns, with what it runs.
///
/// The trailer is stamped by **two** hooks, which sounds like one too many and is not:
/// `prepare-commit-msg` runs *before* the editor, so on an interactive commit the message
/// is still empty and there is nothing to stamp — and stamping it anyway would make an
/// empty message non-empty, quietly destroying git's "an empty message aborts the commit".
/// So `prepare-commit-msg` handles the messages that already have content (`-m`, `-F`,
/// `-t`) and `commit-msg`, which runs *after* the editor, handles the rest. Each no-ops
/// when the other has already stamped, so no commit ever gets two.
pub const HOOKS: &[(&str, &str)] = &[
    ("post-merge", "kanspec scan --quiet"),
    ("post-checkout", "kanspec scan --quiet"),
    ("prepare-commit-msg", "append the `Kanspec: <id>` trailer"),
    (
        "commit-msg",
        "append the `Kanspec: <id>` trailer after the editor",
    ),
];

/// The marker line that tells install from re-install, and ours from theirs.
pub const MARKER: &str = "# kanspec-managed hook — do not edit; see `kanspec init --refresh-hooks`";

/// The env var that points the installed hooks at a binary that is not on git's `PATH` —
/// a cargo target dir, a release candidate, the test suite. Absent, the hooks call the
/// name the user invoked and no-op when it is not installed.
pub const BIN_ENV: &str = "KANSPEC_BIN";

/// The git-config key `kanspec start` writes and the `prepare-commit-msg` hook reads back:
/// `branch.<name>.kanspec-ticket`. Git has **no per-branch hooks** (D-5), so this key *is*
/// the per-branch dispatch — one repo-wide hook, one config read, and a clean no-op on
/// every branch that is not a ticket branch.
///
/// `start` and the hook must spell it identically, so they share this one definition.
pub const BRANCH_TICKET_KEY: &str = "kanspec-ticket";

/// The full config key for one branch, e.g. `branch.ks/t-9c41-slug.kanspec-ticket`.
pub fn branch_ticket_key(branch: &str) -> String {
    format!("branch.{branch}.{BRANCH_TICKET_KEY}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookAction {
    Installed,
    Refreshed,
    /// a foreign hook was displaced to `<hook>.d/10-<name>`
    Displaced,
    Removed,
    /// the foreign hook was moved back
    Restored,
    Unchanged,
}

impl HookAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            HookAction::Installed => "installed",
            HookAction::Refreshed => "refreshed",
            HookAction::Displaced => "displaced",
            HookAction::Removed => "removed",
            HookAction::Restored => "restored",
            HookAction::Unchanged => "unchanged",
        }
    }
    /// Did this action move a byte? Drives the glyph, and `SetupChange::changed`.
    pub const fn changed(self) -> bool {
        !matches!(self, HookAction::Unchanged)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HookReport {
    pub hook: &'static str,
    pub path: PathBuf,
    pub action: HookAction,
    pub note: Option<String>,
}

/// The typed edit vocabulary for the files S7 owns **outside** the store: git hooks, the
/// agent snippet, agent settings. `plan::Op` is the same idea for the store.
///
/// Deliberately small. Everything here is idempotent by construction, because the planners
/// compare against what is on disk and simply emit no `Edit` when nothing would change —
/// which is what makes "`init` never clobbers a user's edits" a property of the plan
/// rather than a property of the applier's care.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum Edit {
    MkDir {
        path: PathBuf,
    },
    Write {
        path: PathBuf,
        contents: String,
        /// set the executable bits (unix); a hook without them is silently never run
        exec: bool,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Remove {
        path: PathBuf,
    },
    /// `rmdir` if — and only if — the directory is empty
    PruneDir {
        path: PathBuf,
    },
}

impl Edit {
    pub fn path(&self) -> &Path {
        match self {
            Edit::MkDir { path }
            | Edit::Write { path, .. }
            | Edit::Remove { path }
            | Edit::PruneDir { path } => path,
            Edit::Move { to, .. } => to,
        }
    }
}

/// `rev-parse --git-path hooks`, honouring `core.hooksPath`.
pub fn hooks_dir(ctx: &Ctx) -> Result<PathBuf> {
    ctx.git.hooks_dir()
}

/// Install (or, with `force`, unconditionally rewrite) every hook in [`HOOKS`].
///
/// A pre-existing hook that is not ours is **displaced**, not overwritten: it moves to
/// `<hook>.d/10-<hook>` and the entrypoint runs it first, propagating its exit code. That
/// is what makes a husky repo survive `kanspec init`.
pub fn install(ctx: &Ctx, force: bool) -> Result<Vec<HookReport>> {
    let (edits, reports) = plan_install(ctx, force)?;
    crate::cmd::init::apply(&edits)?;
    Ok(reports)
}

/// Symmetric uninstall: restores exactly the hook kanspec displaced.
pub fn remove(ctx: &Ctx) -> Result<Vec<HookReport>> {
    let (edits, reports) = plan_remove(ctx)?;
    crate::cmd::init::apply(&edits)?;
    Ok(reports)
}

pub fn plan_install(ctx: &Ctx, force: bool) -> Result<(Vec<Edit>, Vec<HookReport>)> {
    let dir = hooks_dir(ctx)?;
    let mut edits = vec![Edit::MkDir { path: dir.clone() }];
    let mut reports = Vec::new();

    for (hook, _) in HOOKS {
        let path = dir.join(hook);
        let body = dispatcher_script(hook, &action_for(hook, ctx.invoked_as), ctx.invoked_as);
        let existing = read(&path);

        // A hook we did not write is displaced, never clobbered.
        let mut note = None;
        if let Some(text) = existing.as_deref() {
            if !text.contains(MARKER) {
                let to = displaced_path(&dir, hook);
                if to.exists() {
                    return Err(KsError::conflict(
                        format!(
                            "{} already exists — refusing to overwrite the hook kanspec \
                             displaced last time",
                            to.display()
                        ),
                        fixes![
                            fix!("mv {} {}", to.display(), path.display()),
                            fix!("{} init --refresh-hooks", ctx.invoked_as),
                        ],
                    ));
                }
                edits.push(Edit::MkDir {
                    path: dot_d(&dir, hook),
                });
                edits.push(Edit::Move {
                    from: path.clone(),
                    to: to.clone(),
                });
                note = Some(format!(
                    "your existing {hook} moved to {} and still runs first",
                    rel(&dir, &to)
                ));
                reports.push(HookReport {
                    hook,
                    path: to,
                    action: HookAction::Displaced,
                    note: note.clone(),
                });
            }
        }

        let ours = existing.as_deref().is_some_and(|t| t.contains(MARKER));
        let action = match (&existing, ours) {
            (Some(t), true) if t == &body && !force => HookAction::Unchanged,
            (Some(_), true) => HookAction::Refreshed,
            _ => HookAction::Installed,
        };
        if action != HookAction::Unchanged {
            edits.push(Edit::Write {
                path: path.clone(),
                contents: body,
                exec: true,
            });
        }
        reports.push(HookReport {
            hook,
            path,
            action,
            note,
        });
    }
    Ok((edits, reports))
}

pub fn plan_remove(ctx: &Ctx) -> Result<(Vec<Edit>, Vec<HookReport>)> {
    let dir = hooks_dir(ctx)?;
    let mut edits = Vec::new();
    let mut reports = Vec::new();

    for (hook, _) in HOOKS {
        let path = dir.join(hook);
        if !is_ours(&path) {
            reports.push(HookReport {
                hook,
                path,
                action: HookAction::Unchanged,
                note: Some("not installed by kanspec — left alone".into()),
            });
            continue;
        }
        edits.push(Edit::Remove { path: path.clone() });
        reports.push(HookReport {
            hook,
            path: path.clone(),
            action: HookAction::Removed,
            note: None,
        });

        // Restore the displaced hook only when it is the sole occupant of `<hook>.d/`:
        // anything else in there is the user's, and moving one entry out of a set would
        // change which scripts run.
        let d = dot_d(&dir, hook);
        let displaced = displaced_path(&dir, hook);
        let entries = list(&d);
        match (displaced.is_file(), entries.len()) {
            (true, 1) => {
                edits.push(Edit::Move {
                    from: displaced,
                    to: path.clone(),
                });
                edits.push(Edit::PruneDir { path: d });
                reports.push(HookReport {
                    hook,
                    path,
                    action: HookAction::Restored,
                    note: Some("your original hook is back where it was".into()),
                });
            }
            (_, n) if n > 0 => reports.push(HookReport {
                hook,
                path: d.clone(),
                action: HookAction::Unchanged,
                note: Some(format!(
                    "{} still holds {n} script(s); nothing dispatches them now",
                    rel(&dir, &d)
                )),
            }),
            _ => {}
        }
    }
    Ok((edits, reports))
}

/// The dispatcher body every installed hook shares: run every executable in `<hook>.d/` in
/// name order, then the kanspec action, propagating the first non-zero exit.
pub fn dispatcher_script(hook: &str, action: &str, invoked_as: &str) -> String {
    format!(
        r#"#!/bin/sh
{MARKER}
# hook: {hook}
#
# Runs every executable in {hook}.d/ in name order — that is where a hook you already had
# was moved, so it still runs and its exit code still wins — and then {invoked_as}'s own
# action. The dispatch below is shell builtins only, so it works with an empty PATH.

ks_dir=${{0%/*}}
[ "$ks_dir" = "$0" ] && ks_dir=.
ks_d="$ks_dir/{hook}.d"
if [ -d "$ks_d" ]; then
  for ks_hook in "$ks_d"/*; do
    [ -f "$ks_hook" ] || continue
    [ -x "$ks_hook" ] || continue
    "$ks_hook" "$@" || exit $?
  done
fi

{action}
exit 0
"#
    )
}

/// The kanspec half of one hook. Every branch of it ends in a no-op rather than a failure:
/// a tracker must never be the reason a commit or a checkout fails.
fn action_for(hook: &str, invoked_as: &str) -> String {
    let resolve = format!(
        r#"ks_bin="${{{BIN_ENV}:-{invoked_as}}}"
command -v "$ks_bin" >/dev/null 2>&1 || exit 0"#
    );
    match hook {
        // $3 is 1 for a branch checkout and 0 for a file checkout; only the first can
        // change what is merged.
        "post-checkout" => format!(
            r#"[ "${{3:-1}}" = "1" ] || exit 0
{resolve}
"$ks_bin" scan --quiet || exit 0"#
        ),
        "post-merge" => format!(
            r#"{resolve}
"$ks_bin" scan --quiet || exit 0"#
        ),
        // Git has no per-branch hooks (D-5). `kanspec start` records
        // `branch.<name>.kanspec-ticket`; this reads it back, which is why the hook costs
        // one `git config` call on every branch that is not a ticket branch.
        "prepare-commit-msg" => format!(
            r#"case "${{2:-}}" in merge|squash|commit) exit 0 ;; esac
{}"#,
            trailer_snippet()
        ),
        // Runs after the editor, so this is where an interactive commit gets its trailer.
        // A merge has no ticket of its own; `$2` does not exist here, so ask git.
        "commit-msg" => format!(
            r#"[ -e "$(git rev-parse --git-path MERGE_HEAD 2>/dev/null)" ] && exit 0
{}"#,
            trailer_snippet()
        ),
        other => format!("# no kanspec action for {other}"),
    }
}

/// Stamp `Kanspec: <id>` on the message in `$1`, or do nothing at all. Shared verbatim by
/// both message hooks, so there is one definition of when a trailer is appropriate.
fn trailer_snippet() -> String {
    format!(
        r#"ks_branch=$(git symbolic-ref --short -q HEAD) || exit 0
[ -n "$ks_branch" ] || exit 0
ks_ticket=$(git config --get "branch.$ks_branch.{BRANCH_TICKET_KEY}" 2>/dev/null) || exit 0
[ -n "$ks_ticket" ] || exit 0
grep -q "^Kanspec: $ks_ticket$" "$1" 2>/dev/null && exit 0
# An empty message aborts the commit, and a trailer must never be the thing that makes it
# non-empty: content first, stamp second. Comment lines and anything below the `-v`
# scissors line are not content.
awk '/^[[:space:]]*#.*>8/{{exit}} /^[[:space:]]*#/{{next}} /[^[:space:]]/{{ks=1; exit}} END{{exit !ks}}' "$1" || exit 0
if grep -q '^#.*>8' "$1" 2>/dev/null; then
  # `git commit -v` puts a diff below that scissors line and discards everything under it,
  # so the trailer has to go in above it.
  git interpret-trailers --in-place --trailer "Kanspec: $ks_ticket" "$1" 2>/dev/null || exit 0
else
  printf '\nKanspec: %s\n' "$ks_ticket" >> "$1" 2>/dev/null || exit 0
fi"#
    )
}

/// Whether this file is one kanspec wrote.
pub fn is_ours(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|t| t.contains(MARKER))
        .unwrap_or(false)
}

fn dot_d(dir: &Path, hook: &str) -> PathBuf {
    dir.join(format!("{hook}.d"))
}

/// `10-` so a user can drop `20-mine` beside it and know which runs first.
fn displaced_path(dir: &Path, hook: &str) -> PathBuf {
    dot_d(dir, hook).join(format!("10-{hook}"))
}

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

/// What the dispatcher would actually run: `"$dir"/*` in `sh` skips dotfiles, so a stray
/// `.DS_Store` is not an occupant and must not stop `--remove` from restoring.
fn list(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    !p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Hook paths are absolute and long; the reports read better relative to the hooks dir.
fn rel(dir: &Path, p: &Path) -> String {
    p.strip_prefix(dir)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

/// The same idea for a report the user reads: a repo-relative path is also the thing they
/// can paste into a `git` command. A path outside the repo — `core.hooksPath` pointing
/// somewhere absolute — stays absolute, because there it is the only useful form.
pub(crate) fn relative_to(root: &Path, p: &Path) -> PathBuf {
    p.strip_prefix(root).unwrap_or(p).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hook_carries_the_marker_and_a_trailing_newline() {
        for (hook, _) in HOOKS {
            let s = dispatcher_script(hook, &action_for(hook, "kanspec"), "kanspec");
            assert!(s.starts_with("#!/bin/sh\n"));
            assert!(s.contains(MARKER), "{hook} is not identifiable as ours");
            assert!(
                s.ends_with('\n'),
                "{hook} has no trailing newline — appending to it would weld two scripts \
                 into one line"
            );
            assert!(
                s.contains(&format!("{hook}.d")),
                "{hook} does not dispatch its .d/ directory"
            );
        }
    }

    #[test]
    fn a_displaced_hook_runs_before_ours_and_its_exit_code_wins() {
        let s = dispatcher_script(
            "post-merge",
            &action_for("post-merge", "kanspec"),
            "kanspec",
        );
        let loop_at = s.find("for ks_hook").expect("the .d/ loop");
        let action_at = s.find("scan --quiet").expect("the kanspec action");
        assert!(loop_at < action_at, "ours must run last");
        assert!(s.contains(r#""$ks_hook" "$@" || exit $?"#));
    }

    #[test]
    fn the_message_hooks_no_op_without_a_ticket_and_skip_generated_messages() {
        for hook in ["prepare-commit-msg", "commit-msg"] {
            let a = action_for(hook, "kanspec");
            assert!(a.contains("branch.$ks_branch.kanspec-ticket"), "{hook}");
            // The fast path is one `git config` read: no kanspec process at all on a
            // branch that is not a ticket branch.
            assert!(!a.contains("scan"), "{hook}");
            assert!(a.contains("Kanspec: $ks_ticket"), "{hook}");
            // Already stamped -> nothing to do, which is what lets both hooks exist.
            assert!(
                a.contains(r#"grep -q "^Kanspec: $ks_ticket$" "$1""#),
                "{hook}"
            );
        }
        assert!(action_for("prepare-commit-msg", "kanspec")
            .contains(r#"case "${2:-}" in merge|squash|commit) exit 0 ;; esac"#));
        assert!(action_for("commit-msg", "kanspec").contains("MERGE_HEAD"));
    }

    #[test]
    fn a_message_with_no_content_is_never_stamped() {
        // Otherwise `git commit`, editor, quit-without-typing would produce a commit whose
        // entire message is our trailer — git's "an empty message aborts the commit",
        // silently destroyed by a tracker.
        for hook in ["prepare-commit-msg", "commit-msg"] {
            assert!(
                action_for(hook, "kanspec").contains(r#"END{exit !ks}' "$1" || exit 0"#),
                "{hook} stamps a message that has no content of its own"
            );
        }
    }

    #[test]
    fn the_scan_hooks_never_fail_the_git_command_that_ran_them() {
        for hook in ["post-merge", "post-checkout"] {
            let a = action_for(hook, "kanspec");
            assert!(a.contains(r#"command -v "$ks_bin" >/dev/null 2>&1 || exit 0"#));
            assert!(a.contains(r#""$ks_bin" scan --quiet || exit 0"#));
        }
        assert!(
            action_for("post-checkout", "kanspec").starts_with(r#"[ "${3:-1}" = "1" ] || exit 0"#),
            "a file checkout cannot change what is merged"
        );
    }

    #[test]
    fn the_alias_the_user_typed_is_the_one_the_hook_calls() {
        let a = action_for("post-merge", "ks");
        assert!(a.contains(r#"ks_bin="${KANSPEC_BIN:-ks}""#), "{a}");
    }

    #[test]
    fn displaced_hooks_land_where_remove_looks_for_them() {
        let dir = Path::new("/repo/.git/hooks");
        assert_eq!(
            displaced_path(dir, "pre-commit"),
            Path::new("/repo/.git/hooks/pre-commit.d/10-pre-commit")
        );
        assert_eq!(
            dot_d(dir, "pre-commit"),
            Path::new("/repo/.git/hooks/pre-commit.d")
        );
    }
}
