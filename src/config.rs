//! `.kanspec/config.toml`. Every field carries a default, and the whole struct is
//! `#[serde(default)]`, so a **missing** file is `Config::default()` rather than an error —
//! a `.kanspec/` without a config.toml is legal. A **malformed** one is
//! `KsError::Invalid` carrying toml's own span, because a typo in a config is the one
//! error where "which line" is the entire message.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{KsError, Result};
use crate::paths::KanspecDir;
use crate::{fix, fixes};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// `origin/main`; resolved via `symbolic-ref` if unset.
    pub main: String,
    pub id_width: usize,
    pub sync: SyncMode,
    pub port: u16,
    pub branch_prefix: String,
    /// Whether `start` creates a linked worktree without being asked.
    ///
    /// Off by default, and deliberately a REPO setting rather than a personal one: whether
    /// worktrees suit a project is a property of the project — untracked `.env` files,
    /// build output, `node_modules`, absolute paths baked into configs — so it is equally
    /// true for every teammate and every agent working in it. One person works it out; the
    /// committed config settles it for everyone.
    pub worktree: bool,
    pub worktree_dir: PathBuf,
    pub lock_timeout_secs: u64,
    pub paths: Paths,
    pub windows: Windows,
    pub git: GitCfg,
    pub ci: CiCfg,
    pub hooks: HooksCfg,
    pub prime: PrimeCfg,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            main: "origin/main".into(),
            id_width: 4,
            sync: SyncMode::Batch,
            port: 5757,
            branch_prefix: "ks/".into(),
            worktree: false,
            worktree_dir: PathBuf::from("../kanspec-wt"),
            lock_timeout_secs: 5,
            paths: Paths::default(),
            windows: Windows::default(),
            git: GitCfg::default(),
            ci: CiCfg::default(),
            hooks: HooksCfg::default(),
            prime: PrimeCfg::default(),
        }
    }
}

/// The injection surface's context economy.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrimeCfg {
    /// The spec-rules budget of `prime` (and so of `rules --path`), in tokens at four
    /// bytes each; `0` lifts it. Matched specs are shown in rank order while the budget is
    /// unspent and named past it, so the payload overshoots by at most one spec.
    ///
    /// 2 000 is a floor on usefulness rather than a ceiling on cost: it fits a file's own
    /// spec plus its neighbours on a 666-rule corpus, and trims the widest branches there
    /// from ~14k tokens to ~2.5k. DESIGN.md's "~1.5k" was written before any corpus
    /// existed; the number is a knob precisely because one repo's specs are another's
    /// noise.
    pub spec_budget_tokens: usize,
}

impl Default for PrimeCfg {
    fn default() -> PrimeCfg {
        PrimeCfg {
            spec_budget_tokens: 2_000,
        }
    }
}

/// The generated projections, published where the team looks (repo root by default).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Paths {
    pub features: PathBuf,
    pub architecture: PathBuf,
}

impl Default for Paths {
    fn default() -> Paths {
        Paths {
            features: PathBuf::from("KANSPEC-FEATURES.md"),
            architecture: PathBuf::from("KANSPEC-ARCHITECTURE.md"),
        }
    }
}

/// The tripwire windows. `Copy`, so `derive.rs` can take them by value and stay pure.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Windows {
    /// doing, no commit/update
    pub stall_secs: u64,
    pub review_dwell_secs: u64,
    pub in_main_dwell_secs: u64,
    pub settling_dwell_secs: u64,
    pub discovered_dwell_secs: u64,
    /// spec staleness tripwire
    pub stale_merges: u32,
    pub fetch_max_age_secs: u64,
}

impl Default for Windows {
    fn default() -> Windows {
        Windows {
            stall_secs: 7_200,
            review_dwell_secs: 604_800,
            in_main_dwell_secs: 86_400,
            settling_dwell_secs: 259_200,
            discovered_dwell_secs: 604_800,
            stale_merges: 3,
            fetch_max_age_secs: 300,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// mutations dirty the working tree; `status` reminds when N changes are pending
    #[default]
    Batch,
    /// every verb auto-commits `kanspec: <verb> <id>` to the current branch
    Commit,
    /// team mode, v0.4 — a dedicated `kanspec/state` branch
    Branch,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitCfg {
    /// `git fetch origin` before a scan (skippable; results are then stamped with age)
    pub fetch: bool,
    pub gh: GhMode,
}

impl Default for GitCfg {
    fn default() -> GitCfg {
        GitCfg {
            fetch: true,
            gh: GhMode::Auto,
        }
    }
}

/// Whether rung 2 of the merge ladder may shell out to `gh`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GhMode {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CiCfg {
    pub provider: CiProvider,
    pub homerunner: HomerunnerCfg,
}

/// The origin remote's `owner/name` in homerunner's `[[repos]]` -> homerunner; else `gh`
/// authed -> gh; else none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CiProvider {
    #[default]
    Auto,
    Homerunner,
    Gh,
    None,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HomerunnerCfg {
    /// not on PATH by design
    pub bin: PathBuf,
    pub db: PathBuf,
    pub api: String,
}

impl Default for HomerunnerCfg {
    fn default() -> HomerunnerCfg {
        HomerunnerCfg {
            bin: PathBuf::from("~/dev/homerunner/target/release/homerunner"),
            db: PathBuf::from("~/.local/share/homerunner/homerunner.db"),
            api: "http://127.0.0.1:4123".into(),
        }
    }
}

/// v0.2, DEFAULT FALSE (D-14): landcheck is installed but config-gated.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HooksCfg {
    pub landcheck: bool,
}

impl Config {
    /// A MISSING file is `Config::default()`, not an error. A malformed one is `Invalid`
    /// with the toml span.
    pub fn load(ks: &KanspecDir) -> Result<Config> {
        let path = ks.config_toml();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => {
                return Err(KsError::invalid(
                    format!("cannot read {}: {e}", path.display()),
                    fixes![fix!("kanspec doctor")],
                ))
            }
        };
        Config::parse(&text, &path.display().to_string())
    }

    /// Split out from [`Config::load`] so the parse half is testable without a filesystem.
    pub fn parse(text: &str, origin: &str) -> Result<Config> {
        let cfg: Config = toml::from_str(text).map_err(|e| {
            KsError::invalid(
                format!("{origin} is not valid config: {e}"),
                fixes![
                    fix!("kanspec doctor"),
                    fix!("compare against `kanspec init --help` defaults"),
                ],
            )
        })?;
        // `sync = "branch"` parses — the variant exists so v0.4 can fill it in — but
        // NOTHING implements it: `Store::transact` auto-commits only under `Commit`, and
        // `status` suppresses its "N tracker changes pending" line for `Branch` on the
        // assumption that a branch-syncing repo has nothing pending. Together that is the
        // worst shape a config knob can have: it syncs nothing AND silences the one line
        // that would have said so. Refusing is the only honest answer until it is built.
        if cfg.sync == SyncMode::Branch {
            return Err(KsError::gate(
                "sync_branch_v04",
                format!(
                    "{origin}: `sync = \"branch\"` lands in v0.4 — nothing implements it \
                     yet, and it would silently commit and push nothing"
                ),
                fixes![
                    fix!("sync = \"batch\"   # dirty the tree; commit when you commit"),
                    fix!("sync = \"commit\"  # auto-commit every verb"),
                ],
            ));
        }
        Ok(cfg)
    }

    /// What `init` writes — every default made visible, with the comments that explain
    /// which knob is which. Round-trips through [`Config::parse`] (asserted in tests).
    pub fn render_default() -> String {
        let d = Config::default();
        format!(
            r#"# kanspec config. Every key below is the default; delete any line to keep it.
# Docs: `kanspec instructions config`

main             = "{main}"      # the branch merge detection resolves against
id_width         = {id_width}                 # hex digits in a minted id; widens on collision
sync             = "{sync}"           # batch | commit   (branch = team mode, v0.4)
port             = {port}              # `kanspec up` binds 127.0.0.1:<port>
branch_prefix    = "{branch_prefix}"             # `start` creates <prefix><id>-<slug>
worktree         = {worktree}                # `start` makes a worktree without being asked?
                                 # off by default: worktrees suit some repos badly
                                 # (untracked .env, build output, node_modules).
                                 # Override either way with --worktree / --no-worktree.
worktree_dir     = "{worktree_dir}"  # where those worktrees go
lock_timeout_secs = {lock}                # how long a verb waits for the advisory lock

[paths]
# The generated projections. `init` refuses to claim a path that already exists
# un-generated, so plain FEATURES.md is yours to take if it is free.
features     = "{features}"
architecture = "{architecture}"

[windows]
stall_secs            = {stall}     # doing, no commit or update -> STALLED
review_dwell_secs     = {review}   # in review this long -> a WATCHING line
in_main_dwell_secs    = {in_main}    # landed but not closed
settling_dwell_secs   = {settling}   # proposal's last ticket landed, not closed
discovered_dwell_secs = {discovered}   # discovered_in ticket sitting untriaged
stale_merges          = {stale}        # merges touching a spec's globs before it is stale
fetch_max_age_secs    = {fetch_max}      # older than this and a scan says so on the badge

[git]
fetch = {fetch}            # `git fetch origin` before a scan
gh    = "{gh}"        # auto | always | never — rung 2 of the merge ladder

[ci]
provider = "{provider}"        # auto | homerunner | gh | none

[ci.homerunner]
bin = "{hr_bin}"
db  = "{hr_db}"
api = "{hr_api}"

[hooks]
landcheck = {landcheck}         # the Stop hook. Opt-in; v0.2.

[prime]
spec_budget_tokens = {spec_budget}   # spec rules `prime` injects, in tokens; 0 = no budget.
                                 # Matched specs show in rank order until it is spent;
                                 # the rest are named. `rules --full` lifts it.
"#,
            main = d.main,
            id_width = d.id_width,
            sync = "batch",
            port = d.port,
            branch_prefix = d.branch_prefix,
            worktree = d.worktree,
            worktree_dir = d.worktree_dir.display(),
            lock = d.lock_timeout_secs,
            features = d.paths.features.display(),
            architecture = d.paths.architecture.display(),
            stall = d.windows.stall_secs,
            review = d.windows.review_dwell_secs,
            in_main = d.windows.in_main_dwell_secs,
            settling = d.windows.settling_dwell_secs,
            discovered = d.windows.discovered_dwell_secs,
            stale = d.windows.stale_merges,
            fetch_max = d.windows.fetch_max_age_secs,
            fetch = d.git.fetch,
            gh = "auto",
            provider = "auto",
            hr_bin = d.ci.homerunner.bin.display(),
            hr_db = d.ci.homerunner.db.display(),
            hr_api = d.ci.homerunner.api,
            landcheck = d.hooks.landcheck,
            spec_budget = d.prime.spec_budget_tokens,
        )
    }

    /// The serialized form of *this* config — used by `doctor --fix` when it needs to
    /// rewrite a config it repaired. Not the same as [`Config::render_default`], which is
    /// hand-commented prose for humans.
    pub fn render(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(KsError::internal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_the_default() {
        let c = Config::parse("", "<test>").unwrap();
        assert_eq!(c.main, "origin/main");
        assert_eq!(c.port, 5757);
        assert_eq!(c.windows.stall_secs, 7_200);
        assert_eq!(c.sync, SyncMode::Batch);
        assert!(!c.hooks.landcheck, "landcheck is opt-in (D-14)");
    }

    /// A knob that parses, does nothing, AND silences the warning that would reveal it is
    /// the worst shape available. `sync = "branch"` is all three until v0.4 builds it.
    #[test]
    fn sync_branch_refuses_rather_than_silently_syncing_nothing() {
        let e = Config::parse("sync = \"branch\"\n", "test").unwrap_err();
        assert_eq!(e.kind(), "gate");
        assert!(format!("{e}").contains("v0.4"), "{e}");
        // …and the two that DO work still parse.
        for ok in ["batch", "commit"] {
            Config::parse(&format!("sync = \"{ok}\"\n"), "test")
                .unwrap_or_else(|e| panic!("`{ok}` must still parse: {e}"));
        }
    }

    #[test]
    fn partial_config_keeps_every_other_default() {
        let c = Config::parse("port = 6000\n[windows]\nstall_secs = 60\n", "<test>").unwrap();
        assert_eq!(c.port, 6000);
        assert_eq!(c.windows.stall_secs, 60);
        assert_eq!(c.windows.review_dwell_secs, 604_800);
        assert_eq!(c.main, "origin/main");
    }

    #[test]
    fn an_unknown_key_is_a_typed_refusal_naming_its_fix() {
        let e = Config::parse("prot = 6000\n", "<test>").unwrap_err();
        assert_eq!(e.kind(), "invalid");
        assert!(e.fixes().iter().next().is_some());
    }

    #[test]
    fn render_default_round_trips_to_the_default() {
        let text = Config::render_default();
        let c = Config::parse(&text, "<render_default>")
            .unwrap_or_else(|e| panic!("render_default is not parseable: {e}\n{text}"));
        let d = Config::default();
        assert_eq!(c.main, d.main);
        assert_eq!(c.id_width, d.id_width);
        assert_eq!(c.sync, d.sync);
        assert_eq!(c.port, d.port);
        assert_eq!(c.branch_prefix, d.branch_prefix);
        // A knob nobody can find is a knob nobody has: `init` must write it, at its
        // default, next to the directory it governs.
        assert_eq!(c.worktree, d.worktree);
        assert!(!d.worktree, "claiming in place stays the default");
        assert!(
            text.lines()
                .any(|l| l.trim_start().starts_with("worktree ")),
            "the scaffold must carry the worktree knob:\n{text}"
        );
        assert_eq!(c.worktree_dir, d.worktree_dir);
        assert_eq!(c.lock_timeout_secs, d.lock_timeout_secs);
        assert_eq!(c.paths.features, d.paths.features);
        assert_eq!(c.paths.architecture, d.paths.architecture);
        assert_eq!(c.windows.stall_secs, d.windows.stall_secs);
        assert_eq!(c.windows.fetch_max_age_secs, d.windows.fetch_max_age_secs);
        assert_eq!(c.git.fetch, d.git.fetch);
        assert_eq!(c.git.gh, d.git.gh);
        assert_eq!(c.ci.provider, d.ci.provider);
        assert_eq!(c.hooks.landcheck, d.hooks.landcheck);
        assert_eq!(c.prime.spec_budget_tokens, d.prime.spec_budget_tokens);
        assert!(
            text.contains("[prime]") && text.contains("spec_budget_tokens"),
            "the scaffold must carry the budget knob, or nobody finds it:\n{text}"
        );
    }

    #[test]
    fn a_config_round_trips_through_its_own_serializer() {
        let c = Config {
            port: 4242,
            windows: Windows {
                stale_merges: 9,
                ..Windows::default()
            },
            ..Config::default()
        };
        let back = Config::parse(&c.render().unwrap(), "<render>").unwrap();
        assert_eq!(back.port, 4242);
        assert_eq!(back.windows.stale_merges, 9);
    }
}
