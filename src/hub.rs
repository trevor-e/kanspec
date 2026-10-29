//! The hub: a shared ticket board that every repo's tickets are MIRRORED to, so agents in
//! different repos and worktrees can see each other's claims and talk through comments
//! before anything reaches git.
//!
//! The repo stays the truth. The board is a copy that `transact` pushes after every write
//! that touches a ticket, and nothing here is ever read back into `.kanspec/`. Two calls go
//! the other way, and both are about OTHER sessions: `start` asks the board for the claim
//! first, so an agent in another clone cannot hold the same ticket, and `prime` reads the
//! comments left on this project's tickets.
//!
//! Off unless `[hub] url` is set. Every failure but a refused claim is a WARNING, printed
//! once to stderr after the command's own output: a board that is down or slow must never
//! cost a verb. `kanspec hub sync` re-pushes everything once it is back.
//!
//! The wire is the board's REST surface (telkins.com's `/api/tickets`):
//! `PUT /refs/<project>/<id>` upserts a ticket by its kanspec id and claims or releases it
//! in the same call, `POST /projects` creates the project, `GET /comments` reads.
//!
//! **Shells out to `curl`**, as git and gh are shelled out to, so the binary gains no HTTP
//! stack. The token goes to curl on stdin (`--config -`), never in argv where `ps` shows it.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ctx::Ctx;
use crate::error::{GateCode, KsError, Result};
use crate::ids::TicketId;
use crate::model::{Snapshot, Ticket};
use crate::transitions::State;
use crate::{fix, fixes};

/// `[hub]` in `.kanspec/config.toml`. Committed with the repo, so it holds no secret: the
/// token comes from the environment or a file only this user can read.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HubCfg {
    /// The board's ticket API, like `https://telkins.com/api/tickets`. Empty = off.
    pub url: String,
    /// This repo's project on the board; it becomes the board's key prefix.
    pub project: String,
    /// `owner/name` for pull request links; read from `origin` when empty.
    pub repo: String,
    /// The environment variable holding the token. Checked first.
    pub token_env: String,
    /// A file holding the token, `~/` allowed. Read when the variable is unset.
    pub token_file: String,
    /// Per request. A verb waits at most this long on a board that is down.
    pub timeout_secs: u64,
}

impl Default for HubCfg {
    fn default() -> HubCfg {
        HubCfg {
            url: String::new(),
            project: String::new(),
            repo: String::new(),
            token_env: "KANSPEC_HUB_TOKEN".into(),
            token_file: String::new(),
            timeout_secs: 5,
        }
    }
}

/// `prime` runs from SessionStart, so it waits less than a verb does.
const PRIME_TIMEOUT_SECS: u64 = 3;
/// How far back `prime` looks for comments, and how many it shows.
const COMMENT_DAYS: i64 = 3;
const COMMENT_TOP: usize = 5;

// ─────────────────────────────────────────────────────────────────────────────
// warnings — collected here, printed once by `run()` after the command's output
// ─────────────────────────────────────────────────────────────────────────────

static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn warn(msg: impl Into<String>) {
    let msg = msg.into();
    if let Ok(mut w) = WARNINGS.lock() {
        // the server runs many verbs in one process and never drains, so this is capped
        if !w.contains(&msg) && w.len() < 20 {
            w.push(msg);
        }
    }
}

/// Everything the hub had to say during this command, oldest first.
pub fn take_warnings() -> Vec<String> {
    WARNINGS
        .lock()
        .map(|mut w| std::mem::take(&mut *w))
        .unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
// the client
// ─────────────────────────────────────────────────────────────────────────────

pub struct Hub {
    base: String,
    project: String,
    repo: Option<String>,
    token: String,
    timeout: u64,
}

/// What the board answered. `status` 0 never reaches a caller: a request curl could not
/// complete is an `Err` carrying curl's own words.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    fn error(&self) -> String {
        self.body["error"]
            .as_str()
            .map_or_else(|| format!("HTTP {}", self.status), str::to_string)
    }
}

impl Hub {
    /// `None` when the hub is off. Configured but without a token is a warning, not an
    /// error: the repo still works, the board just doesn't hear about it.
    pub fn open(ctx: &Ctx) -> Option<Hub> {
        let c = &ctx.cfg.hub;
        if c.url.trim().is_empty() {
            return None;
        }
        if c.project.trim().is_empty() {
            warn("[hub] url is set but project is empty; nothing was mirrored");
            return None;
        }
        let Some(token) = token(c) else {
            warn(format!(
                "no board token: set {} or [hub] token_file; nothing was mirrored",
                c.token_env
            ));
            return None;
        };
        let repo = if c.repo.is_empty() {
            origin_slug(ctx)
        } else {
            Some(c.repo.clone())
        };
        Some(Hub {
            base: c.url.trim().trim_end_matches('/').to_string(),
            project: c.project.trim().to_string(),
            repo,
            token,
            timeout: c.timeout_secs.max(1),
        })
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> std::result::Result<Reply, String> {
        let mut cfg = String::new();
        let mut line = |k: &str, v: &str| cfg.push_str(&format!("{k} = \"{}\"\n", quote(v)));
        line("url", &format!("{}{path}", self.base));
        line("request", method);
        line("header", &format!("Authorization: Bearer {}", self.token));
        line("header", "Accept: application/json");
        if let Some(b) = body {
            line("header", "Content-Type: application/json");
            line("data-binary", &b.to_string());
        }
        line("max-time", &self.timeout.to_string());
        line("write-out", "\n%{http_code}");

        let mut child = Command::new("curl")
            .args(["--silent", "--show-error", "--config", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run curl: {e}"))?;
        child
            .stdin
            .take()
            .ok_or("curl has no stdin")?
            .write_all(cfg.as_bytes())
            .map_err(|e| format!("could not talk to curl: {e}"))?;
        let out = child
            .wait_with_output()
            .map_err(|e| format!("curl failed: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
        let status: u16 = code.trim().parse().unwrap_or(0);
        if !out.status.success() || status == 0 {
            let err = String::from_utf8_lossy(&out.stderr);
            // `curl: (7) Failed to connect ...` -> `Failed to connect ...`
            let line = err.trim().trim_start_matches("curl: ");
            let line = line
                .strip_prefix('(')
                .and_then(|l| l.split_once(") "))
                .map_or(line, |(_, rest)| rest);
            return Err(line.lines().next().unwrap_or("no answer").to_string());
        }
        Ok(Reply {
            status,
            body: serde_json::from_str(body).unwrap_or(Value::Null),
        })
    }

    /// Upsert one ticket. A board that has never heard of this project gets it created and
    /// the push retried once, so pointing a repo at the board needs no other step.
    fn put(&self, t: &Ticket, claimed_by: Option<&str>) -> std::result::Result<Reply, String> {
        let path = format!("/refs/{}/{}", self.project, t.fm.id);
        let body = self.payload(t, claimed_by);
        let reply = self.call("PUT", &path, Some(&body))?;
        if reply.status == 404 && reply.error().starts_with("No project") {
            self.ensure_project()?;
            return self.call("PUT", &path, Some(&body));
        }
        Ok(reply)
    }

    fn ensure_project(&self) -> std::result::Result<(), String> {
        let body = json!({ "slug": self.project, "repo": self.repo });
        let reply = self.call("POST", "/projects", Some(&body))?;
        // 409: it exists, which is all this wanted
        if reply.ok() || reply.status == 409 {
            Ok(())
        } else {
            Err(reply.error())
        }
    }

    fn payload(&self, t: &Ticket, claimed_by: Option<&str>) -> Value {
        let fm = &t.fm;
        json!({
            "title": fm.title,
            // the `## Log` is this repo's audit trail, not something the board shows
            "body": without_log(&t.body),
            "state": board_state(fm.state),
            "branch": fm.branch,
            "pr": match (fm.pr, &self.repo) {
                (Some(n), Some(repo)) => Some(format!("https://github.com/{repo}/pull/{n}")),
                (Some(n), None) => Some(format!("#{n}")),
                (None, _) => None,
            },
            "deps": fm.deps.iter().map(ToString::to_string).collect::<Vec<_>>(),
            // a closed ticket is nobody's; the board clears the claim itself
            "claimed_by": if fm.state.terminal() { None } else { claimed_by },
        })
    }
}

/// kanspec's five states onto the board's columns. The board's `backlog` has no kanspec
/// counterpart and is never sent.
fn board_state(s: State) -> &'static str {
    match s {
        State::Todo => "todo",
        State::Doing => "in_progress",
        State::Review => "in_review",
        State::Done => "done",
        State::Dropped => "canceled",
    }
}

fn without_log(body: &str) -> &str {
    let cut = body
        .find(&format!("\n{}", crate::logentry::LOG_HEADING))
        .or_else(|| body.starts_with(crate::logentry::LOG_HEADING).then_some(0))
        .unwrap_or(body.len());
    body[..cut].trim()
}

/// curl's config syntax: a double-quoted value with backslash escapes
fn quote(v: &str) -> String {
    let mut o = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o
}

fn token(c: &HubCfg) -> Option<String> {
    if let Ok(t) = std::env::var(&c.token_env) {
        if !t.trim().is_empty() {
            return Some(t.trim().to_string());
        }
    }
    if c.token_file.trim().is_empty() {
        return None;
    }
    let path = expand_home(c.token_file.trim());
    std::fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map_or_else(|| PathBuf::from(p), |h| PathBuf::from(h).join(rest)),
        None => PathBuf::from(p),
    }
}

fn origin_slug(ctx: &Ctx) -> Option<String> {
    let o = ctx
        .git
        .run(&["config", "--get", "remote.origin.url"])
        .ok()?;
    (o.code == 0).then_some(())?;
    crate::gh::parse_slug(o.out.trim())
}

// ─────────────────────────────────────────────────────────────────────────────
// the calls kanspec makes
// ─────────────────────────────────────────────────────────────────────────────

/// After a write: push every ticket the plan touched, as the post-write snapshot has it.
/// Runs after the lock is released, so a slow board never holds up another verb.
pub fn mirror(ctx: &Ctx, snap: &Snapshot, ids: &[TicketId]) {
    if ids.is_empty() {
        return;
    }
    let Some(hub) = Hub::open(ctx) else { return };
    for id in ids {
        let Some(t) = snap.tickets.get(id) else {
            continue;
        };
        match hub.put(t, t.fm.claimed_by.as_deref()) {
            Ok(r) if r.ok() => {}
            Ok(r) if r.status == 409 => warn(format!(
                "the board disagrees about {id}: {}. `kanspec show {id}` has this repo's view",
                r.error()
            )),
            Ok(r) => warn(format!(
                "the board refused {id}: {}. `kanspec hub sync` retries",
                r.error()
            )),
            Err(e) => {
                warn(format!(
                    "the board is unreachable ({e}); `kanspec hub sync` catches it up"
                ));
                // one timeout per verb is enough
                return;
            }
        }
    }
}

/// Before `start` cuts a branch: take the claim on the board, where an agent in another
/// clone or repo would have taken it. Refused there is refused here. An unreachable board
/// is a warning, so working offline still works.
pub fn claim(ctx: &Ctx, t: &Ticket, who: &str) -> Result<()> {
    let Some(hub) = Hub::open(ctx) else {
        return Ok(());
    };
    let mut claimed = t.clone();
    claimed.fm.state = State::Doing;
    match hub.put(&claimed, Some(who)) {
        Ok(r) if r.ok() => Ok(()),
        Ok(r) if r.status == 409 => {
            let id = &t.fm.id;
            Err(KsError::gate(
                GateCode::ClaimedOnBoard,
                format!("{id} is taken on the board: {}", r.error()),
                fixes![
                    fix!("kanspec ready"),
                    fix!("ask its holder in a comment on the board, then retry")
                ],
            ))
        }
        Ok(r) => {
            warn(format!(
                "the board did not record the claim on {}: {}",
                t.fm.id,
                r.error()
            ));
            Ok(())
        }
        Err(e) => {
            warn(format!(
                "the board is unreachable ({e}); claimed locally only. `kanspec hub sync` \
                 catches it up"
            ));
            Ok(())
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BoardComment {
    pub ticket: String,
    pub title: String,
    pub author: String,
    pub body: String,
    pub created: DateTime<Utc>,
}

/// For `prime`: recent comments on this project's tickets that someone else wrote — what
/// other sessions want this one to know. `None` when the hub is off; empty when it is
/// unreachable or quiet.
pub fn recent_comments(ctx: &Ctx) -> Option<Vec<BoardComment>> {
    let mut hub = Hub::open(ctx)?;
    hub.timeout = hub.timeout.min(PRIME_TIMEOUT_SECS);
    let since = (ctx.now - chrono::Duration::days(COMMENT_DAYS)).to_rfc3339();
    let path = format!(
        "/comments?project={}&since={}",
        hub.project,
        since.replace('+', "%2B")
    );
    let reply = match hub.call("GET", &path, None) {
        Ok(r) if r.ok() => r,
        Ok(r) => {
            warn(format!(
                "the board's comments are unavailable: {}",
                r.error()
            ));
            return Some(Vec::new());
        }
        Err(e) => {
            warn(format!(
                "the board is unreachable ({e}); prime shows no board comments"
            ));
            return Some(Vec::new());
        }
    };
    let me = ctx.actor.label();
    let mut out: Vec<BoardComment> = reply
        .body
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let author = c["author"].as_str()?.to_string();
            (author != me).then_some(())?;
            Some(BoardComment {
                // a mirrored ticket is named by its kanspec id; anything else by its key
                ticket: c["ref"].as_str().or_else(|| c["key"].as_str())?.to_string(),
                title: c["title"].as_str().unwrap_or_default().to_string(),
                author,
                body: c["body"].as_str().unwrap_or_default().to_string(),
                created: c["created"].as_str()?.parse().ok()?,
            })
        })
        .collect();
    // newest last, and only the last few
    let skip = out.len().saturating_sub(COMMENT_TOP);
    out.drain(..skip);
    Some(out)
}

/// `kanspec hub <cmd>`
pub fn hub(ctx: &Ctx, a: &crate::cli::HubArgs) -> Result<SyncReport> {
    ctx.require_initialized()?;
    match a.cmd {
        crate::cli::HubCommand::Sync => sync(ctx, &ctx.snapshot()?),
    }
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub configured: bool,
    pub project: Option<String>,
    pub pushed: usize,
    pub failed: Vec<String>,
}

/// `kanspec hub sync`: push every ticket, for a board that was down, a repo just pointed
/// at the board, or a claim made offline.
pub fn sync(ctx: &Ctx, snap: &Snapshot) -> Result<SyncReport> {
    let Some(hub) = Hub::open(ctx) else {
        return Ok(SyncReport {
            configured: false,
            project: None,
            pushed: 0,
            failed: Vec::new(),
        });
    };
    hub.ensure_project()
        .map_err(|e| unreachable(&hub.base, &e))?;
    // dependencies first, so the board can link a ticket to the ones it waits on
    let mut ids: Vec<&TicketId> = snap.tickets.keys().collect();
    ids.sort_by_key(|id| snap.tickets[*id].fm.deps.len());
    let mut pushed = 0;
    let mut failed = Vec::new();
    for _pass in 0..2 {
        pushed = 0;
        failed.clear();
        for id in &ids {
            let t = &snap.tickets[*id];
            match hub.put(t, t.fm.claimed_by.as_deref()) {
                Ok(r) if r.ok() => pushed += 1,
                Ok(r) => failed.push(format!("{id}: {}", r.error())),
                Err(e) => return Err(unreachable(&hub.base, &e)),
            }
        }
        // a second pass links dependencies pushed after the tickets that wait on them
        if !snap.tickets.values().any(|t| !t.fm.deps.is_empty()) {
            break;
        }
    }
    Ok(SyncReport {
        configured: true,
        project: Some(hub.project),
        pushed,
        failed,
    })
}

fn unreachable(base: &str, e: &str) -> KsError {
    KsError::gate(
        GateCode::HubUnreachable,
        format!("the board at {base} did not answer: {e}"),
        fixes![
            fix!("check [hub] url in .kanspec/config.toml and the network, then retry"),
            fix!("kanspec hub sync")
        ],
    )
}

impl crate::out::Render for SyncReport {
    fn human(&self, w: &mut dyn std::io::Write, _st: &crate::out::Style) -> std::io::Result<()> {
        if !self.configured {
            return writeln!(
                w,
                "no board configured: add [hub] url and project to .kanspec/config.toml \
                 (`kanspec instructions config`)"
            );
        }
        writeln!(
            w,
            "pushed {} tickets to {}",
            self.pushed,
            self.project.as_deref().unwrap_or("the board")
        )?;
        for f in &self.failed {
            writeln!(w, "  ✗ {f}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_survives_curls_config_parser() {
        assert_eq!(quote(r#"a "b" \c"#), r#"a \"b\" \\c"#);
        assert_eq!(quote("line\nnext"), "line\\nnext");
    }

    #[test]
    fn every_state_has_a_column() {
        for s in crate::transitions::ALL_STATES {
            assert!(!board_state(*s).is_empty());
        }
        assert_eq!(board_state(State::Doing), "in_progress");
        assert_eq!(board_state(State::Dropped), "canceled");
    }

    #[test]
    fn the_log_stays_home() {
        let body = format!(
            "Do it.\n\n## Steps\n- [ ] a\n\n{}\n- 2026 todo new\n",
            crate::logentry::LOG_HEADING
        );
        assert_eq!(without_log(&body), "Do it.\n\n## Steps\n- [ ] a");
        assert_eq!(without_log("plain"), "plain");
    }

    #[test]
    fn off_without_a_url() {
        assert!(HubCfg::default().url.is_empty());
    }
}
