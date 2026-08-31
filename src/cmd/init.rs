//! `init [--refresh-hooks]` — scaffold the store, `.gitattributes` (`merge=union` for
//! `comments.jsonl`), the git hooks, and the `cache/` gitignore entry.
//!
//! Owner: **S7**. This file and `lock.rs` are the only entries on
//! `tests/single_write_path.rs`'s allowlist: `init` scaffolds `.kanspec/` before a store
//! can exist, so it necessarily writes without a `LockToken`. [`apply`] is therefore also
//! where `hooks.rs` and `setup.rs` send the bytes they plan — one writer for the whole
//! slice, rather than three.
//!
//! **Idempotence is a property of the plan, not of the applier.** Everything below decides
//! by reading what is on disk and emits no edit when nothing would change, so re-running
//! `init` over a repo somebody has been using for a month is a no-op that cannot eat a
//! hand-edited config, a hand-written `.gitattributes` line, or a husky hook.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::cli::InitArgs;
use crate::config::Config;
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::hooks::{relative_to as rel, Edit, HookReport};
use crate::out::{glyph, Color, Line, Render, Style};
use crate::{fix, fixes};

/// The `merge=union` line, a **built-in** git strategy needing zero per-clone setup.
pub const GITATTRIBUTES_LINE: &str = ".kanspec/proposals/**/comments.jsonl merge=union";

/// What lands in `.kanspec/.gitignore`. The cache is disposable and per-machine: every
/// derived git fact lives in it, which is exactly why it must never travel through git.
const CACHE_IGNORE: &str = "cache/";

/// The entity directories. `.gitkeep` because git cannot track an empty directory, and a
/// scaffold that vanishes on the first `git add -A` is not a scaffold.
const ENTITY_DIRS: &[&str] = &[
    "tickets",
    "specs",
    "decisions",
    "quirks",
    "proposals",
    "proposals/closed",
];

#[derive(Debug, Serialize)]
pub struct InitReport {
    pub root: PathBuf,
    pub created: Vec<PathBuf>,
    pub already_present: Vec<PathBuf>,
    pub hooks: Vec<HookReport>,
    pub next: Vec<String>,
}

pub fn init(ctx: &Ctx, a: &InitArgs) -> Result<InitReport> {
    // Before a single byte moves: `init` must not claim a projection path somebody else
    // wrote. See `refuse_taken_projections`.
    refuse_taken_projections(ctx)?;

    let scaffold = plan_scaffold(ctx);
    apply(&scaffold.edits)?;

    // Hooks are a separately idempotent step `init` always runs, so a repo initialised
    // before a hook existed catches up with `init --refresh-hooks` and never needs
    // re-scaffolding.
    let mut hooks = crate::hooks::install(ctx, a.refresh_hooks)?;

    let root = ctx.repo.primary_root().to_path_buf();
    for h in &mut hooks {
        h.path = rel(&root, &h.path);
    }
    Ok(InitReport {
        created: scaffold.created,
        already_present: scaffold.already_present,
        hooks,
        next: vec![
            format!("{} setup claude", ctx.invoked_as),
            format!("{} new \"the first thing to do\"", ctx.invoked_as),
        ],
        root,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// The plan
// ─────────────────────────────────────────────────────────────────────────────

struct Scaffold {
    edits: Vec<Edit>,
    created: Vec<PathBuf>,
    already_present: Vec<PathBuf>,
}

fn plan_scaffold(ctx: &Ctx) -> Scaffold {
    let root = ctx.repo.primary_root().to_path_buf();
    // `Layout` is the only thing allowed to name a file in the store, and it deliberately
    // exposes no accessor for the store root itself — the config lives at its top, so its
    // parent is the one honest way to ask.
    let store = ctx
        .layout
        .config_toml()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.join(".kanspec"));

    let mut s = Scaffold {
        edits: vec![Edit::MkDir {
            path: store.clone(),
        }],
        created: Vec::new(),
        already_present: Vec::new(),
    };

    // config.toml, with every default present and commented so the knobs are discoverable.
    // NEVER rewritten: this is the file a user edits.
    s.create(&root, ctx.layout.config_toml(), Config::render_default());

    for d in ENTITY_DIRS {
        let dir = store.join(d);
        s.edits.push(Edit::MkDir { path: dir.clone() });
        s.create(&root, dir.join(".gitkeep"), String::new());
    }
    // The cache is gitignored, so it gets no `.gitkeep` — nothing should preserve it.
    s.edits.push(Edit::MkDir {
        path: ctx.layout.cache_dir(),
    });
    s.append_line(&root, store.join(".gitignore"), CACHE_IGNORE);
    // `Layout::gitattributes()` used to derive the repo root from the features
    // projection's parent, which stopped being the repo root the moment `[paths] features`
    // named a subdirectory. `Layout` now carries the root explicitly (round-A fix in
    // paths.rs), so this goes back through the one type allowed to name a path.
    s.append_line(&root, ctx.layout.gitattributes(), GITATTRIBUTES_LINE);
    s
}

impl Scaffold {
    /// Create `path` with `contents` — or record that it is already there and leave every
    /// byte of it alone.
    fn create(&mut self, root: &Path, path: PathBuf, contents: String) {
        if path.exists() {
            self.already_present.push(rel(root, &path));
            return;
        }
        self.created.push(rel(root, &path));
        self.edits.push(Edit::Write {
            path,
            contents,
            exec: false,
        });
    }

    /// Append one line to a file that may or may not exist and may or may not already
    /// contain it — `.gitattributes` and the store's `.gitignore` both belong to the user.
    fn append_line(&mut self, root: &Path, path: PathBuf, line: &str) {
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        if existing.lines().any(|l| l.trim() == line) {
            self.already_present.push(rel(root, &path));
            return;
        }
        let mut next = existing;
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(line);
        next.push('\n');
        self.created.push(rel(root, &path));
        self.edits.push(Edit::Write {
            path,
            contents: next,
            exec: false,
        });
    }
}

/// `KANSPEC-FEATURES.md` / `KANSPEC-ARCHITECTURE.md` are **generated** to the repo root and
/// committed, so `init` claims those two paths. If either already exists and kanspec did
/// not write it, claiming it would mean a later `scan` silently overwrites a file somebody
/// wrote by hand.
///
/// This is a refusal, not a warning line: a warning is exit 0, and an exit 0 an agent can
/// ignore is how the file gets eaten. The `[paths]` table renames either projection, and
/// the refusal names that fix.
fn refuse_taken_projections(ctx: &Ctx) -> Result<()> {
    let root = ctx.repo.primary_root().to_path_buf();
    let taken: Vec<(&str, PathBuf)> = [
        ("features", ctx.layout.features_md().to_path_buf()),
        ("architecture", ctx.layout.architecture_md().to_path_buf()),
    ]
    .into_iter()
    .filter(|(_, p)| {
        std::fs::read_to_string(p)
            .map(|t| !crate::project::is_ours(&t))
            .unwrap_or(false)
    })
    .collect();

    let Some((key, first)) = taken.first() else {
        return Ok(());
    };
    let names = taken
        .iter()
        .map(|(_, p)| rel(&root, p).display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(KsError::gate(
        "projection_path_taken",
        format!(
            "{names} already exists and kanspec did not generate it — refusing to claim it"
        ),
        fixes![
            fix!(
                "set [paths] {key} = \"...\" in {} and re-run `{} init`",
                rel(&root, &ctx.layout.config_toml()).display(),
                ctx.invoked_as
            ),
            fix!(
                "mv {} {}.bak",
                rel(&root, first).display(),
                rel(&root, first).display()
            ),
        ],
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// The applier — the ONE function in S7 that moves a byte
// ─────────────────────────────────────────────────────────────────────────────

/// Apply a planned [`Edit`] list.
///
/// It lives in this file, and not in `hooks.rs` or `setup.rs`, because
/// `tests/single_write_path.rs` allowlists exactly `lock.rs` and `cmd/init.rs` — so those
/// two plan and this one writes, which is the same split `Store::transact` makes for the
/// store. Nothing here is clever: every decision was made by the planner that produced the
/// list, and this is deliberately the boring end.
pub(crate) fn apply(edits: &[Edit]) -> Result<Vec<PathBuf>> {
    let mut touched = Vec::new();
    for e in edits {
        match e {
            Edit::MkDir { path } => {
                std::fs::create_dir_all(path).map_err(|e| io_err(path, "create", e))?;
            }
            Edit::Write {
                path,
                contents,
                exec,
            } => {
                if let Some(d) = path.parent() {
                    std::fs::create_dir_all(d).map_err(|e| io_err(d, "create", e))?;
                }
                std::fs::write(path, contents).map_err(|e| io_err(path, "write", e))?;
                if *exec {
                    make_executable(path)?;
                }
                touched.push(path.clone());
            }
            Edit::Move { from, to } => {
                if let Some(d) = to.parent() {
                    std::fs::create_dir_all(d).map_err(|e| io_err(d, "create", e))?;
                }
                std::fs::rename(from, to).map_err(|e| io_err(from, "move", e))?;
                touched.push(to.clone());
            }
            Edit::Remove { path } => match std::fs::remove_file(path) {
                Ok(()) => touched.push(path.clone()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err(path, "remove", e)),
            },
            // "if empty" IS the error: `remove_dir` refuses a populated directory, which
            // is exactly the guard we want, so a failure here is a no-op by design.
            Edit::PruneDir { path } => {
                if std::fs::remove_dir(path).is_ok() {
                    touched.push(path.clone());
                }
            }
        }
    }
    Ok(touched)
}

/// A hook without the executable bit is silently never run by git — the single most
/// confusing way for hook installation to "succeed".
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| io_err(path, "stat", e))?
        .permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms).map_err(|e| io_err(path, "chmod", e))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn io_err(p: &Path, what: &str, e: std::io::Error) -> KsError {
    KsError::internal(anyhow::anyhow!("cannot {what} {}: {e}", p.display()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Render
// ─────────────────────────────────────────────────────────────────────────────

impl Render for InitReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        writeln!(
            w,
            " {} {}",
            crate::out::paint("kanspec", Color::Bold, st.color),
            self.root.display()
        )?;
        for p in &self.created {
            Line::new(glyph::OK, p.display().to_string()).write(w, st)?;
        }
        for p in &self.already_present {
            Line::new('·', p.display().to_string())
                .dim("already present")
                .write(w, st)?;
        }
        for h in &self.hooks {
            let mut line = Line::new(
                if h.action.changed() { glyph::OK } else { '·' },
                format!("hook {:<20} {}", h.hook, h.action.as_str()),
            );
            if let Some(n) = &h.note {
                line = line.dim(n.clone());
            }
            line.write(w, st)?;
        }
        for n in &self.next {
            writeln!(w, " {} {n}", glyph::FIX)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gitattributes_line_names_the_one_union_merged_file() {
        // `merge=union` is a built-in strategy: no per-clone merge-driver config, which is
        // the whole reason the ops log can be a JSONL at all.
        assert!(GITATTRIBUTES_LINE.ends_with(" merge=union"));
        assert!(GITATTRIBUTES_LINE.contains("comments.jsonl"));
    }

    #[test]
    fn apply_is_idempotent_and_prunes_only_empty_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("hooks");
        let f = d.join("post-merge");
        let edits = vec![
            Edit::MkDir { path: d.clone() },
            Edit::Write {
                path: f.clone(),
                contents: "#!/bin/sh\n".into(),
                exec: true,
            },
        ];
        apply(&edits).unwrap();
        apply(&edits).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "#!/bin/sh\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "git silently ignores a non-x hook");
        }

        // A populated directory survives PruneDir; an empty one does not.
        apply(&[Edit::PruneDir { path: d.clone() }]).unwrap();
        assert!(d.is_dir(), "a directory with a file in it must survive");
        apply(&[Edit::Remove { path: f }, Edit::PruneDir { path: d.clone() }]).unwrap();
        assert!(!d.exists());

        // Removing what is not there is not an error — `--remove` runs twice sometimes.
        apply(&[Edit::Remove {
            path: tmp.path().join("nope"),
        }])
        .unwrap();
    }
}
