//! Worktree unification, made structurally unavoidable.
//!
//! [`KanspecDir`] has a private field, no `From<PathBuf>` and no public constructor;
//! [`Layout`] is the only thing that can name a kanspec file, and [`Repo::discover`] is
//! the only source of one. "Every command resolves git-common-dir" is therefore not a
//! rule anyone can forget — there is no other way to name a file under `.kanspec/`.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Config;
use crate::error::{EnvCode, KsError, Result};
use crate::ids::{DecisionId, QuirkId, SpecName, TicketId};
use crate::plan::EntityRef;
use crate::{fix, fixes};

/// ALL FIELDS PRIVATE: a command that could reach `primary_root` directly could write
/// outside `.kanspec/` without going through [`Layout`].
#[derive(Clone, Debug)]
pub struct Repo {
    primary_root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    here: PathBuf,
    linked: bool,
}

impl Repo {
    /// ONE `git rev-parse --path-format=absolute --git-common-dir --git-dir --show-toplevel`
    /// (via `std::process::Command` — `Git` needs a root, so discovery cannot use it).
    ///
    /// 1. exit 128              -> `Environment{NotARepo}`
    /// 2. `git_dir == common_dir` -> `primary_root = show_toplevel` (survives `--separate-git-dir`)
    /// 3. else                  -> first `worktree <path>` stanza of `git worktree list --porcelain -z`
    /// 4. sanity: `primary_root`'s gitdir must resolve to `common_dir`, else
    ///    `Environment{AmbiguousWorktree}` — a typed refusal, never a guess.
    pub fn discover(cwd: &Path, repo_flag: Option<&Path>) -> Result<Repo> {
        let here = repo_flag
            .map(Path::to_path_buf)
            .unwrap_or_else(|| cwd.to_path_buf());
        let here = canon(&here);

        let out = rev_parse(&here)?;
        let mut lines = out.lines();
        let common_dir = canon(Path::new(lines.next().unwrap_or_default().trim()));
        let git_dir = canon(Path::new(lines.next().unwrap_or_default().trim()));
        let toplevel = lines.next().unwrap_or_default().trim().to_string();

        let linked = git_dir != common_dir;
        let primary_root = if !linked {
            // Not `common_dir.parent()`: `--separate-git-dir` puts the git dir anywhere.
            if toplevel.is_empty() {
                return Err(KsError::environment(
                    EnvCode::NotARepo,
                    "git reported no working tree (bare repository?)",
                    fixes![fix!("cd into a checkout of the repository")],
                ));
            }
            canon(Path::new(&toplevel))
        } else {
            first_worktree(&here)?
        };

        // 4. The sanity check. A linked worktree whose primary does not agree with the
        // common dir means we would write into the wrong `.kanspec/`; refuse instead.
        let primary_git_dir = rev_parse(&primary_root)
            .ok()
            .and_then(|o| o.lines().next().map(|l| canon(Path::new(l.trim()))));
        if primary_git_dir.as_deref() != Some(common_dir.as_path()) {
            return Err(KsError::environment(
                EnvCode::AmbiguousWorktree,
                format!(
                    "cannot tell which worktree owns .kanspec/: {} resolves to {}, not {}",
                    primary_root.display(),
                    primary_git_dir
                        .as_deref()
                        .unwrap_or(Path::new("<none>"))
                        .display(),
                    common_dir.display(),
                ),
                fixes![
                    fix!("git worktree list"),
                    fix!("kanspec --repo <primary-worktree> status"),
                ],
            ));
        }

        Ok(Repo {
            primary_root,
            git_dir,
            common_dir,
            here,
            linked,
        })
    }

    pub fn primary_root(&self) -> &Path {
        &self.primary_root
    }
    /// Where the user stands — for `where`, hooks, and branch resolution.
    pub fn here(&self) -> &Path {
        &self.here
    }
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }
    /// True when the caller stands in a linked worktree rather than the primary one.
    pub fn linked(&self) -> bool {
        self.linked
    }
}

/// The one `git` spawn in this file. `Git` needs a root, so discovery cannot use it.
fn git_out(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // git sets GIT_DIR when running hooks, and env beats `-C`.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|e| {
            KsError::environment(
                EnvCode::GitMissing,
                format!("cannot run `git`: {e}"),
                fixes![fix!("install git and re-run")],
            )
        })
}

fn rev_parse(dir: &Path) -> Result<String> {
    let out = git_out(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--git-dir",
            "--show-toplevel",
        ],
    )?;
    if !out.status.success() {
        return Err(KsError::environment(
            EnvCode::NotARepo,
            format!("{} is not inside a git repository", dir.display()),
            fixes![fix!("git init"), fix!("cd into your repository")],
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The FIRST stanza of `git worktree list --porcelain` is always the primary worktree.
fn first_worktree(dir: &Path) -> Result<PathBuf> {
    let out = git_out(dir, &["worktree", "list", "--porcelain", "-z"])?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split('\0')
        .find_map(|rec| rec.strip_prefix("worktree "))
        .map(|p| canon(Path::new(p)))
        .ok_or_else(|| {
            KsError::environment(
                EnvCode::AmbiguousWorktree,
                "git worktree list named no primary worktree",
                fixes![fix!("git worktree list"), fix!("git worktree repair")],
            )
        })
}

/// `canonicalize` when the path exists (macOS `/tmp` -> `/private/tmp` matters for the
/// `git_dir == common_dir` comparison), otherwise the path unchanged.
pub(crate) fn canon(p: &Path) -> PathBuf {
    let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    #[cfg(windows)]
    let p = command_path(p);
    p
}

/// Git for Windows rejects verbatim paths when creating worktrees. Canonicalize
/// first, then spell ordinary drive/UNC paths in the form external commands use.
/// Device namespaces retain their original spelling.
#[cfg(windows)]
fn command_path(p: PathBuf) -> PathBuf {
    use std::path::{Component, Prefix};
    let mut parts = p.components();
    let prefix = match parts.next() {
        Some(Component::Prefix(prefix)) => prefix.kind(),
        _ => return p,
    };
    let mut out = match prefix {
        Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
        Prefix::VerbatimUNC(server, share) => {
            let mut prefix = std::ffi::OsString::from(r"\\");
            prefix.push(server);
            prefix.push(r"\");
            prefix.push(share);
            PathBuf::from(prefix)
        }
        _ => return p,
    };
    out.extend(parts);
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// KanspecDir — the sealed root
// ─────────────────────────────────────────────────────────────────────────────

/// Private field, no `From<PathBuf>`, no public constructor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KanspecDir(PathBuf);

impl KanspecDir {
    pub(crate) fn resolve(repo: &Repo) -> KanspecDir {
        KanspecDir(repo.primary_root().join(".kanspec"))
    }
    pub fn display(&self) -> std::path::Display<'_> {
        self.0.display()
    }
    pub fn exists(&self) -> bool {
        self.0.is_dir()
    }
    /// The one file `KanspecDir` can name on its own: `Config::load` needs only a
    /// `KanspecDir`, which is what keeps `Layout::open(repo, cfg)` non-circular.
    pub fn config_toml(&self) -> PathBuf {
        self.join("config.toml")
    }
    fn join(&self, s: impl AsRef<Path>) -> PathBuf {
        self.0.join(s)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Layout — every path in the product
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Layout {
    /// The primary worktree root, carried explicitly.
    ///
    /// It used to be derived as `features_md.parent()`, on the reasoning that the
    /// projections live at the repo root. That reasoning is only true while `[paths]
    /// features` names a bare filename: `features = "docs/FEATURES.md"` made `repo_root()`
    /// return `<root>/docs` and `gitattributes()` point at `<root>/docs/.gitattributes`,
    /// i.e. a configurable path silently relocated a fixed one. The root is an input, so
    /// it is stored rather than inferred.
    root: PathBuf,
    ks: KanspecDir,
    features_md: PathBuf,
    architecture_md: PathBuf,
}

impl Layout {
    /// Non-circular: `KanspecDir::resolve` needs only `Repo`; `Config::load` needs only
    /// `KanspecDir`; `Layout::open` needs both.
    pub(crate) fn open(repo: &Repo, cfg: &Config) -> Layout {
        let root = repo.primary_root();
        Layout {
            root: root.to_path_buf(),
            ks: KanspecDir::resolve(repo),
            features_md: root.join(&cfg.paths.features),
            architecture_md: root.join(&cfg.paths.architecture),
        }
    }

    pub fn ks(&self) -> &KanspecDir {
        &self.ks
    }
    pub fn config_toml(&self) -> PathBuf {
        self.ks.config_toml()
    }
    pub fn tickets_dir(&self) -> PathBuf {
        self.ks.join("tickets")
    }
    pub fn ticket(&self, id: &TicketId) -> PathBuf {
        self.tickets_dir().join(format!("{id}.md"))
    }
    pub fn specs_dir(&self) -> PathBuf {
        self.ks.join("specs")
    }
    pub fn spec(&self, n: &SpecName) -> PathBuf {
        self.specs_dir().join(format!("{n}.md"))
    }
    pub fn decisions_dir(&self) -> PathBuf {
        self.ks.join("decisions")
    }
    pub fn decision(&self, id: &DecisionId) -> PathBuf {
        self.decisions_dir().join(format!("{id}.md"))
    }
    pub fn quirks_dir(&self) -> PathBuf {
        self.ks.join("quirks")
    }
    pub fn quirk(&self, id: &QuirkId) -> PathBuf {
        self.quirks_dir().join(format!("{id}.md"))
    }
    pub fn proposals_dir(&self) -> PathBuf {
        self.ks.join("proposals")
    }
    /// Moved here ONLY by `kanspec close`, after the gate. Bodies under it are never read.
    pub fn proposals_closed_dir(&self) -> PathBuf {
        self.proposals_dir().join("closed")
    }
    pub fn proposal_dir(&self, id: &crate::ids::ProposalId, slug: &str) -> PathBuf {
        self.proposals_dir().join(format!("{id}-{slug}"))
    }
    pub fn proposal_md(&self, dir: &Path) -> PathBuf {
        dir.join("proposal.md")
    }
    pub fn comments_jsonl(&self, dir: &Path) -> PathBuf {
        dir.join("comments.jsonl")
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.ks.join("cache")
    }
    pub fn lock(&self) -> PathBuf {
        self.cache_dir().join("lock")
    }
    pub fn gitstate(&self) -> PathBuf {
        self.cache_dir().join("gitstate.json")
    }
    /// Honours `[paths] features`.
    pub fn features_md(&self) -> &Path {
        &self.features_md
    }
    pub fn architecture_md(&self) -> &Path {
        &self.architecture_md
    }
    /// `.gitattributes` at the repo root — `init` writes the `merge=union` line for
    /// `comments.jsonl` there.
    pub fn gitattributes(&self) -> PathBuf {
        self.root.join(".gitattributes")
    }
    /// The primary worktree root. Fixed at `open()` from `Repo::primary_root`, never
    /// derived from a configurable path — see the field's note.
    pub fn repo_root(&self) -> &Path {
        &self.root
    }

    /// The `Op` applier's dispatch. Proposals are directories whose name carries a slug,
    /// so the id alone needs one prefix scan to resolve; the fallback is the un-slugged
    /// directory, which is what `CreateEntity` would have produced.
    pub fn path_for(&self, e: &EntityRef) -> PathBuf {
        match e {
            EntityRef::Ticket(id) => self.ticket(id),
            EntityRef::Spec(n) => self.spec(n),
            EntityRef::Decision(id) => self.decision(id),
            EntityRef::Quirk(id) => self.quirk(id),
            EntityRef::Proposal(id) => {
                let prefix = format!("{id}-");
                for dir in [self.proposals_dir(), self.proposals_closed_dir()] {
                    if let Ok(rd) = std::fs::read_dir(&dir) {
                        for entry in rd.flatten() {
                            let name = entry.file_name();
                            let name = name.to_string_lossy();
                            if name.starts_with(&prefix) || name == id.as_str() {
                                return dir.join(name.as_ref());
                            }
                        }
                    }
                }
                self.proposals_dir().join(id.as_str())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_at(root: &Path) -> Layout {
        layout_with(root, &Config::default())
    }

    #[cfg(windows)]
    #[test]
    fn git_command_paths_preserve_drive_and_unc_roots() {
        for (input, expected) in [
            (r"\\?\C:\work space\repo", r"C:\work space\repo"),
            (r"\\?\UNC\server\share\repo", r"\\server\share\repo"),
            (r"C:\repo", r"C:\repo"),
            (r"\\.\pipe\name", r"\\.\pipe\name"),
        ] {
            assert_eq!(command_path(PathBuf::from(input)), Path::new(expected));
        }
    }

    fn layout_with(root: &Path, cfg: &Config) -> Layout {
        Layout {
            root: root.to_path_buf(),
            ks: KanspecDir(root.join(".kanspec")),
            features_md: root.join(&cfg.paths.features),
            architecture_md: root.join(&cfg.paths.architecture),
        }
    }

    #[test]
    fn every_path_hangs_off_the_primary_kanspec_dir() {
        let l = layout_at(Path::new("/repo"));
        assert_eq!(l.config_toml(), Path::new("/repo/.kanspec/config.toml"));
        assert_eq!(
            l.ticket(&TicketId::parse("t-9c41").unwrap()),
            Path::new("/repo/.kanspec/tickets/t-9c41.md")
        );
        assert_eq!(
            l.spec(&SpecName::parse("auth").unwrap()),
            Path::new("/repo/.kanspec/specs/auth.md")
        );
        assert_eq!(
            l.decision(&DecisionId::parse("D-8c1a").unwrap()),
            Path::new("/repo/.kanspec/decisions/D-8c1a.md")
        );
        assert_eq!(
            l.quirk(&QuirkId::parse("q-11ba").unwrap()),
            Path::new("/repo/.kanspec/quirks/q-11ba.md")
        );
        assert_eq!(l.lock(), Path::new("/repo/.kanspec/cache/lock"));
        assert_eq!(
            l.gitstate(),
            Path::new("/repo/.kanspec/cache/gitstate.json")
        );
        assert_eq!(l.features_md(), Path::new("/repo/KANSPEC-FEATURES.md"));
        assert_eq!(
            l.architecture_md(),
            Path::new("/repo/KANSPEC-ARCHITECTURE.md")
        );
        assert_eq!(l.gitattributes(), Path::new("/repo/.gitattributes"));
    }

    /// `repo_root()` is an input, not something inferred from a configurable path. It was
    /// once `features_md.parent()`, so `[paths] features = "docs/FEATURES.md"` moved the
    /// repo root — and `.gitattributes`, which `init` writes the `merge=union` line to —
    /// into `docs/`. Renaming a projection must not relocate a fixed path.
    #[test]
    fn a_projection_in_a_subdirectory_does_not_move_the_repo_root() {
        let mut cfg = Config::default();
        cfg.paths.features = "docs/FEATURES.md".into();
        cfg.paths.architecture = "docs/deep/ARCH.md".into();
        let l = layout_with(Path::new("/repo"), &cfg);

        assert_eq!(l.features_md(), Path::new("/repo/docs/FEATURES.md"));
        assert_eq!(l.repo_root(), Path::new("/repo"));
        assert_eq!(l.gitattributes(), Path::new("/repo/.gitattributes"));
    }

    #[test]
    fn path_for_dispatches_over_every_entity_kind() {
        let l = layout_at(Path::new("/repo"));
        let t = TicketId::parse("t-9c41").unwrap();
        assert_eq!(l.path_for(&EntityRef::Ticket(t.clone())), l.ticket(&t));
        let n = SpecName::parse("auth").unwrap();
        assert_eq!(l.path_for(&EntityRef::Spec(n.clone())), l.spec(&n));
        let p = crate::ids::ProposalId::parse("p-7de2").unwrap();
        // No such directory on disk -> the un-slugged fallback.
        assert_eq!(
            l.path_for(&EntityRef::Proposal(p)),
            Path::new("/repo/.kanspec/proposals/p-7de2")
        );
    }

    /// kanspec's own checkout is a git repo, which is the cheapest honest fixture — but it
    /// is a *primary* worktree only when the suite runs from the main checkout. Every agent
    /// working in a `.claude/worktrees/` linked worktree runs it from a linked one, so the
    /// shape assertions branch instead of assuming. Both arms assert, so neither is a
    /// vacuous pass: the properties that must hold everywhere are checked unconditionally,
    /// and each shape's distinguishing fact is checked in its own arm.
    #[test]
    fn discover_finds_this_very_repository() {
        let here = std::env::current_dir().unwrap();
        let repo = Repo::discover(&here, None).expect("kanspec's own checkout is a git repo");

        // True in both shapes, and the whole point of `Repo`: the primary root is the
        // crate's own checkout, and the store hangs off it and not off the linked worktree.
        assert!(repo.primary_root().join(".git").exists());
        assert!(
            repo.primary_root().join("Cargo.toml").is_file(),
            "primary_root must resolve to the crate root, not to a linked worktree: {}",
            repo.primary_root().display()
        );
        assert_eq!(
            KanspecDir::resolve(&repo).config_toml(),
            repo.primary_root().join(".kanspec/config.toml")
        );

        if repo.linked() {
            // A linked worktree has its own git dir *inside* the shared common dir.
            assert_ne!(
                repo.git_dir(),
                repo.common_dir(),
                "a linked worktree's git dir is its own"
            );
            assert!(
                repo.git_dir().starts_with(repo.common_dir()),
                "{} is not under {}",
                repo.git_dir().display(),
                repo.common_dir().display()
            );
        } else {
            assert_eq!(repo.git_dir(), repo.common_dir());
        }
    }

    #[test]
    fn discover_refuses_outside_a_repository() {
        let tmp = std::env::temp_dir();
        // `/tmp` is only a useful fixture when it is genuinely not inside a repo.
        if Repo::discover(&tmp, None).is_ok() {
            return;
        }
        let e = Repo::discover(&tmp, None).unwrap_err();
        assert_eq!(e.exit_code(), crate::error::code::ENVIRONMENT);
        assert!(e.fixes().iter().next().is_some());
    }
}
