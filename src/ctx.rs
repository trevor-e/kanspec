//! `Ctx` — built ONCE in `run()` before dispatch, and provably `Send + Sync`.
//!
//! Because it is built before the dispatch match, worktree unification and actor/clock
//! injection touch every command without any command knowing they exist.
//!
//! NOTE what is absent: no `Ui`, no `RefCell`, no `Rc`, no handle to stdout. Presentation
//! lives in `out.rs`. That is what makes `Arc<Ctx>` crossable into `spawn_blocking`, which
//! is what lets the server's POST handlers call the very same `cmd::*` functions the CLI
//! calls (§2.16).
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cli::Cli;
use crate::config::Config;
use crate::error::{EnvCode, KsError, Result};
use crate::gh::Gh;
use crate::git::Git;
use crate::model::Snapshot;
use crate::out::Style;
use crate::paths::{KanspecDir, Layout, Repo};
use crate::{fix, fixes};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    Human { name: String },
    Agent { session: String, tool: String },
}

impl Actor {
    /// `KANSPEC_ACTOR` + `KANSPEC_ACTOR_KIND` (tests) >
    /// `CLAUDE_SESSION_ID`/`CURSOR_SESSION_ID`/`CODEX_SESSION_ID` (agent) >
    /// `git config user.email` > `$USER` (human).
    pub fn detect() -> Actor {
        if let Ok(name) = std::env::var("KANSPEC_ACTOR") {
            let kind = std::env::var("KANSPEC_ACTOR_KIND").unwrap_or_default();
            return if kind == "agent" {
                let (tool, session) = name.split_once('/').unwrap_or(("agent", name.as_str()));
                Actor::Agent {
                    session: session.to_string(),
                    tool: tool.to_string(),
                }
            } else {
                Actor::Human { name }
            };
        }
        for (var, tool) in [
            ("CLAUDE_SESSION_ID", "claude"),
            ("CURSOR_SESSION_ID", "cursor"),
            ("CODEX_SESSION_ID", "codex"),
        ] {
            if let Ok(session) = std::env::var(var) {
                if !session.is_empty() {
                    return Actor::Agent {
                        session,
                        tool: tool.to_string(),
                    };
                }
            }
        }
        let name = git_user_email()
            .or_else(|| std::env::var("USER").ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        Actor::Human { name }
    }

    /// `"trevor"` | `"claude/sess-a91"`. Never contains whitespace: the `## Log` column
    /// grammar is parsed by splitting.
    pub fn label(&self) -> String {
        let raw = match self {
            Actor::Human { name } => name.clone(),
            Actor::Agent { session, tool } => format!("{tool}/{session}"),
        };
        raw.split_whitespace().collect::<Vec<_>>().join("-")
    }

    pub fn is_agent(&self) -> bool {
        matches!(self, Actor::Agent { .. })
    }
}

/// The email's local part is the readable half; a bare `$USER` is the fallback.
fn git_user_email() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["config", "--get", "user.email"])
        .env_remove("GIT_DIR")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let email = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if email.is_empty() {
        return None;
    }
    Some(email.split('@').next().unwrap_or(&email).to_string())
}

/// Invariant 8, mechanically (D-18). Private field; the ONLY constructor refuses an
/// `Actor::Agent`, and `plan_accept`/`plan_revoke` take `&HumanActor`. An agent session
/// literally cannot call them. It is a proof token, not a wrapper: the planners never read
/// the actor back (they take it from `Facts`), so nothing is stored.
#[derive(Debug)]
pub struct HumanActor(());

impl HumanActor {
    pub fn require(a: &Actor, verb: &'static str) -> Result<HumanActor> {
        if a.is_agent() {
            return Err(KsError::gate(
                "agent_cannot_self_accept",
                format!("`{verb}` is a human act — agents never self-accept standing rules"),
                fixes![
                    fix!("ask your human to run `kanspec {verb} <id>`"),
                    fix!("kanspec status"),
                ],
            ));
        }
        Ok(HumanActor(()))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum OutMode {
    Human { color: bool },
    Json,
}

impl OutMode {
    /// The one place `--json` and `--color` become a mode. `run()` needs it BEFORE a `Ctx`
    /// exists (to render the refusal when `Ctx::open` itself fails) and `Ctx::open` needs
    /// it again, so both call this rather than each deciding colour on their own.
    pub fn from_cli(cli: &Cli) -> OutMode {
        if cli.json {
            OutMode::Json
        } else {
            OutMode::Human {
                color: crate::out::apply_color_policy(cli.color),
            }
        }
    }
}

pub struct Ctx {
    pub repo: Repo,
    pub layout: Layout,
    pub cfg: Config,
    pub git: Git,
    pub gh: Gh,
    pub actor: Actor,
    /// `KANSPEC_NOW`-overridable => deterministic goldens.
    pub now: DateTime<Utc>,
    pub out: OutMode,
    pub invoked_as: &'static str,
    /// The command line as the user typed it — what `## Log` notes and `sync = "commit"`
    /// subjects record. A `Ctx` built for a browser action overrides it with the verb the
    /// browser asked for, so the note never claims a CLI run that did not happen.
    pub invocation: String,
}

impl Ctx {
    pub fn open(cli: &Cli, cwd: &Path) -> Result<Ctx> {
        let repo = Repo::discover(cwd, cli.repo.as_deref())?;
        let ks = KanspecDir::resolve(&repo);
        let cfg = Config::load(&ks)?;
        let layout = Layout::open(&repo, &cfg);
        let git = Git::bind(repo.primary_root());
        let gh = Gh::detect(&git, &cfg.git.gh);
        Ok(Ctx {
            repo,
            layout,
            cfg,
            git,
            gh,
            actor: Actor::detect(),
            now: detect_now()?,
            out: OutMode::from_cli(cli),
            invoked_as: crate::cli::invoked_as(),
            invocation: std::iter::once(crate::cli::invoked_as().to_string())
                .chain(std::env::args().skip(1))
                .collect::<Vec<_>>()
                .join(" "),
        })
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        crate::store::load_snapshot(self)
    }

    /// `.kanspec/config.toml` relative to the primary root, forward slashes — the one path
    /// whose presence in a commit proves that commit carries the store
    /// (`git cat-file -e <rev>:<this>`). Used by `start`, `init` and `status` to tell a
    /// `main` that has never seen `.kanspec/` from one that is merely behind.
    pub fn store_marker(&self) -> String {
        crate::hooks::relative_to(self.repo.primary_root(), &self.layout.config_toml())
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// `"kanspec ship --pr 142"` — the Log note, and `Store::transact`'s `cmdline`.
    pub fn invocation(&self) -> String {
        self.invocation.clone()
    }

    pub fn style(&self) -> Style {
        Style {
            color: matches!(self.out, OutMode::Human { color: true }),
            width: crate::out::term_width(),
        }
    }

    /// Every command that touches `.kanspec/` calls this first — the one place the
    /// "run `kanspec init`" refusal is worded.
    pub fn require_initialized(&self) -> Result<()> {
        if self.layout.ks().exists() {
            return Ok(());
        }
        Err(KsError::environment(
            EnvCode::NotInitialized,
            format!("no kanspec store at {}", self.layout.ks().display()),
            fixes![fix!("{} init", self.invoked_as)],
        ))
    }
}

fn detect_now() -> Result<DateTime<Utc>> {
    match std::env::var("KANSPEC_NOW") {
        Err(_) => Ok(Utc::now()),
        Ok(s) => s.parse::<DateTime<Utc>>().map_err(|e| {
            KsError::invalid(
                format!("KANSPEC_NOW is not an RFC3339 timestamp: {e}"),
                fixes![fix!("unset KANSPEC_NOW")],
            )
        }),
    }
}

/// The decisive property for a parallel build: `Arc<Ctx>` crosses into `spawn_blocking`,
/// so `server.rs` needs no write code of its own.
const _: fn() = || {
    fn need<T: Send + Sync + 'static>() {}
    need::<std::sync::Arc<Ctx>>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_never_contain_whitespace() {
        let a = Actor::Human {
            name: "Trevor Elkins".into(),
        };
        assert_eq!(a.label(), "Trevor-Elkins");
        let b = Actor::Agent {
            session: "sess-a91".into(),
            tool: "claude".into(),
        };
        assert_eq!(b.label(), "claude/sess-a91");
        assert!(b.is_agent() && !a.is_agent());
    }

    #[test]
    fn an_agent_cannot_mint_a_human_actor() {
        let agent = Actor::Agent {
            session: "sess-a91".into(),
            tool: "claude".into(),
        };
        let e = HumanActor::require(&agent, "accept").unwrap_err();
        assert_eq!(e.kind(), "gate");
        assert_eq!(e.code(), Some("agent_cannot_self_accept"));

        let human = Actor::Human {
            name: "trevor".into(),
        };
        assert!(HumanActor::require(&human, "accept").is_ok());
    }
}
