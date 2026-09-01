//! **THE ONE WRITE PATH.** `Store::transact` is the only public mutator in the crate, and
//! the four primitives at the bottom of this file are the only functions that move a byte
//! under `.kanspec/`.
//!
//! Module privacy gets 90% of that; Rust cannot forbid `std::fs` crate-wide, so the last
//! 10% is `tests/single_write_path.rs` — named after the invariant rather than pretended
//! away (R-3). Its allowlist is exactly `lock.rs` (creates the lockfile it then locks) and
//! `cmd/init.rs` (scaffolds `.kanspec/` before a store can exist), and the allowlist's own
//! length is asserted so it cannot grow silently.
//!
//! Owner: **S1**.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::fm::{self, MdDoc, SetOutcome, Yv};
use crate::ids::{DecisionId, ItemKind, ItemRef, Minter, ProposalId, QuirkId, SpecName, TicketId};
use crate::lock::{LockOwner, LockToken};
use crate::logentry::{self, LogEntry, LOG_HEADING};
use crate::model::{
    CommentOp, Decision, Item, Prescription, PromoteAs, Proposal, Quirk, Rule, Snapshot, Spec,
    Step, Ticket,
};
use crate::plan::{EntityRef, Op, Plan};
use crate::transitions::{self, Verb};
use crate::{fix, fixes};

/// Bumped on every successful `transact`. Process-local by design: it exists so the server
/// can memoize a snapshot and invalidate it synchronously on write (D-22), and a number
/// shared across processes would need the very store it is meant to describe.
static REV: AtomicU64 = AtomicU64::new(0);

// ─────────────────────────────────────────────────────────────────────────────
// Loading
// ─────────────────────────────────────────────────────────────────────────────

/// Glob-and-parse. ~55ms at 2000 tickets (measured). Loads OPEN proposals only; walks
/// `proposals/closed/` for IDS ONLY, never bodies — which is what makes invariant 4 a
/// property of `Snapshot`'s type rather than of anyone's discipline.
pub fn load_snapshot(ctx: &Ctx) -> Result<Snapshot> {
    let l = &ctx.layout;
    let mut snap = Snapshot::empty(ctx.cfg.clone(), ctx.now);

    for path in md_files(&l.tickets_dir()) {
        let (fm_text, body, mtime) = read_entity(&path)?;
        let fm: crate::model::TicketFm = parse_fm(&fm_text, &path)?;
        let ticket = Ticket {
            steps: fm::steps(&body)
                .into_iter()
                .map(|(index, done, text)| Step { index, done, text })
                .collect(),
            log: logentry::parse_log(&body),
            fm,
            path: path.clone(),
            body,
            mtime,
        };
        // The FILENAME is the id. A ticket whose frontmatter disagrees is exactly the
        // hand-edit that would make `deps:` point at nothing, so it is refused loudly.
        let named = file_id::<TicketId>(&path)?;
        if named != ticket.fm.id {
            return Err(mismatch(&path, named.as_str(), ticket.fm.id.as_str()));
        }
        snap.tickets.insert(ticket.fm.id.clone(), ticket);
    }

    for path in md_files(&l.specs_dir()) {
        let (fm_text, body, _) = read_entity(&path)?;
        let fm: crate::model::SpecFm = parse_fm(&fm_text, &path)?;
        let name = SpecName::parse(&stem(&path)).map_err(|e| at_path(e, &path))?;
        let rules = parse_rules(&body);
        snap.specs.insert(
            name.clone(),
            Spec {
                name,
                fm,
                path,
                body,
                rules,
            },
        );
    }

    for path in md_files(&l.decisions_dir()) {
        let (fm_text, body, _) = read_entity(&path)?;
        let fm: crate::model::DecisionFm = parse_fm(&fm_text, &path)?;
        let named = file_id::<DecisionId>(&path)?;
        if named != fm.id {
            return Err(mismatch(&path, named.as_str(), fm.id.as_str()));
        }
        let scope = fm.scope.clone();
        snap.decisions.insert(
            fm.id.clone(),
            Decision {
                fm,
                path,
                body,
                scope,
            },
        );
    }

    for path in md_files(&l.quirks_dir()) {
        let (fm_text, body, _) = read_entity(&path)?;
        let fm: crate::model::QuirkFm = parse_fm(&fm_text, &path)?;
        let named = file_id::<QuirkId>(&path)?;
        if named != fm.id {
            return Err(mismatch(&path, named.as_str(), fm.id.as_str()));
        }
        snap.quirks.insert(fm.id.clone(), Quirk { fm, path, body });
    }

    // OPEN proposals: full bodies. `proposals/closed/` contributes IDS ONLY — there is
    // physically no value through which a closed proposal's prose can reach the standing
    // rules generator.
    for dir in sub_dirs(&l.proposals_dir()) {
        if dir.file_name().is_some_and(|n| n == "closed") {
            continue;
        }
        let md = l.proposal_md(&dir);
        if !md.is_file() {
            continue;
        }
        let (fm_text, body, _) = read_entity(&md)?;
        let fm: crate::model::ProposalFm = parse_fm(&fm_text, &md)?;
        let items = parse_items(&fm.id, &body);
        let comments = read_comments(&l.comments_jsonl(&dir))?;
        if !comments.is_empty() {
            snap.comments.insert(fm.id.clone(), comments);
        }
        snap.proposals.insert(
            fm.id.clone(),
            Proposal {
                fm,
                dir,
                body,
                items,
            },
        );
    }
    for dir in sub_dirs(&l.proposals_closed_dir()) {
        if let Some(id) = dir_proposal_id(&dir) {
            snap.closed_ids.insert(id);
        }
    }

    snap.git = crate::cache::load(l);
    snap.rev = REV.load(Ordering::Relaxed);
    Ok(snap)
}

fn md_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "md"))
        .filter(|p| !stem(p).starts_with('.'))
        .collect();
    out.sort();
    out
}

fn sub_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// `p-19f0-money-as-cents` -> `p-19f0`.
fn dir_proposal_id(dir: &Path) -> Option<String> {
    let name = dir.file_name()?.to_string_lossy().into_owned();
    let mut it = name.splitn(3, '-');
    let (a, b) = (it.next()?, it.next()?);
    ProposalId::parse(&format!("{a}-{b}"))
        .ok()
        .map(|id| id.as_str().to_string())
}

/// `(frontmatter text, body, mtime)`.
fn read_entity(p: &Path) -> Result<(String, String, SystemTime)> {
    let src = std::fs::read_to_string(p).map_err(|e| io_err(p, "read", e))?;
    let doc = fm::split(&src).map_err(|e| {
        KsError::invalid(
            format!("{}: {e}", p.display()),
            fixes![
                fix!("open {} and add a `---` frontmatter block", p.display()),
                fix!("kanspec doctor"),
            ],
        )
    })?;
    let mtime = std::fs::metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok((doc.fm, doc.body, mtime))
}

fn parse_fm<T: serde::de::DeserializeOwned>(fm_text: &str, p: &Path) -> Result<T> {
    serde_yaml_ng::from_str(fm_text).map_err(|e| {
        KsError::invalid(
            format!("{}: {e}", p.display()),
            fixes![
                fix!("open {} and fix the frontmatter", p.display()),
                fix!("kanspec doctor"),
            ],
        )
    })
}

fn file_id<I>(p: &Path) -> Result<I>
where
    I: IdFromStem,
{
    I::from_stem(&stem(p)).map_err(|e| at_path(e, p))
}

/// A tiny shim so the three id kinds whose FILENAME is authoritative share one code path.
pub(crate) trait IdFromStem: Sized {
    fn from_stem(s: &str) -> Result<Self>;
}
impl IdFromStem for TicketId {
    fn from_stem(s: &str) -> Result<Self> {
        TicketId::parse(s)
    }
}
impl IdFromStem for DecisionId {
    fn from_stem(s: &str) -> Result<Self> {
        DecisionId::parse(s)
    }
}
impl IdFromStem for QuirkId {
    fn from_stem(s: &str) -> Result<Self> {
        QuirkId::parse(s)
    }
}

fn mismatch(p: &Path, named: &str, inside: &str) -> KsError {
    KsError::invalid(
        format!(
            "{} is named `{named}` but its frontmatter says `{inside}`",
            p.display()
        ),
        fixes![
            fix!(
                "git mv {} $(dirname {})/{inside}.md",
                p.display(),
                p.display()
            ),
            fix!("kanspec doctor"),
        ],
    )
}

fn at_path(e: KsError, p: &Path) -> KsError {
    KsError::invalid(
        format!("{}: {e}", p.display()),
        fixes![fix!("kanspec doctor")],
    )
}

fn io_err(p: &Path, what: &str, e: std::io::Error) -> KsError {
    KsError::internal(anyhow::anyhow!("cannot {what} {}: {e}", p.display()))
}

/// `- [auth.lockout] 5 failed logins ... {p-7de2}` — bracket ids are stable merge keys and
/// comment anchors; `{p-xxxx}` tokens are provenance back to the proposal that shipped the
/// rule.
fn parse_rules(body: &str) -> Vec<Rule> {
    let mut out = Vec::new();
    for (i, raw) in body.lines().enumerate() {
        let t = raw.trim();
        let Some(rest) = t.strip_prefix("- [") else {
            continue;
        };
        let Some((anchor, tail)) = rest.split_once(']') else {
            continue;
        };
        // `- [ ]` / `- [x]` are checkboxes, not rules.
        if anchor.trim().is_empty() || anchor.eq_ignore_ascii_case("x") {
            continue;
        }
        let mut provenance = Vec::new();
        let mut items = Vec::new();
        let mut text = String::new();
        let mut rest = tail.trim();
        while let Some(open) = rest.rfind('{') {
            let Some(close) = rest[open..].find('}') else {
                break;
            };
            let token = &rest[open + 1..open + close];
            // `{p-7de2#c1}` names the exact item this rule satisfies; `{p-7de2}` names only
            // the proposal. An item token ALSO contributes its proposal, so every consumer
            // that only knows about `provenance` keeps working unchanged.
            if let Ok(item) = crate::ids::ItemRef::parse(token) {
                provenance.push(item.proposal.clone());
                items.push(item);
                rest = rest[..open].trim_end();
                continue;
            }
            match ProposalId::parse(token) {
                Ok(id) => {
                    provenance.push(id);
                    rest = rest[..open].trim_end();
                }
                Err(_) => break,
            }
        }
        provenance.reverse();
        items.reverse();
        text.push_str(rest);
        out.push(Rule {
            anchor: anchor.trim().to_string(),
            text,
            provenance,
            items,
            line: i + 1,
        });
    }
    out
}

/// `- [c1] …`, `- [p1] (promote: decision) …`, `- [t1] …` under a proposal's headings.
/// Every prescription is typed; an untyped one is a `doctor` warning and a close blocker,
/// which is why [`Prescription::Untyped`] is a value rather than a parse failure.
fn parse_items(pid: &ProposalId, body: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for raw in body.lines() {
        let t = raw.trim();
        let Some(rest) = t.strip_prefix("- [") else {
            continue;
        };
        let Some((tag, tail)) = rest.split_once(']') else {
            continue;
        };
        let anchor = format!("{pid}#{}", tag.trim());
        let Ok(id) = ItemRef::parse(&anchor) else {
            continue;
        };
        let text = tail.trim().to_string();
        let prescription = (id.kind == ItemKind::Prescription).then(|| parse_prescription(&text));
        out.push(Item {
            id,
            text,
            prescription,
        });
    }
    out
}

fn parse_prescription(text: &str) -> Prescription {
    let Some(open) = text.find('(') else {
        return Prescription::Untyped;
    };
    let Some(close) = text[open..].find(')') else {
        return Prescription::Untyped;
    };
    let inner = text[open + 1..open + close].trim();
    if let Some(t) = inner.strip_prefix("temp until") {
        return match TicketId::parse(t.trim()) {
            Ok(id) => Prescription::TempUntil(id),
            Err(_) => Prescription::Untyped,
        };
    }
    if let Some(k) = inner.strip_prefix("promote:") {
        return match k.trim() {
            "decision" => Prescription::Promote(PromoteAs::Decision),
            "spec" => Prescription::Promote(PromoteAs::Spec),
            "quirk" => Prescription::Promote(PromoteAs::Quirk),
            _ => Prescription::Untyped,
        };
    }
    Prescription::Untyped
}

/// Append-only, `merge=union`, deduped on `(id, op, at)` — one `cm-` id legitimately
/// carries `comment` + `reply` + `resolve` rows (D-19). An unparseable row is SKIPPED, not
/// fatal: `merge=union` can interleave a conflict marker into the file, and losing the
/// board over it would be the wrong trade.
fn read_comments(p: &Path) -> Result<Vec<CommentOp>> {
    let Ok(text) = std::fs::read_to_string(p) else {
        return Ok(Vec::new());
    };
    let mut seen: std::collections::BTreeSet<(String, crate::model::CommentOpKind, i64)> =
        std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(c) = serde_json::from_str::<CommentOp>(line) else {
            continue;
        };
        if seen.insert((c.id.as_str().to_string(), c.op, c.at.timestamp_millis())) {
            out.push(c);
        }
    }
    out.sort_by(|a, b| a.at.cmp(&b.at).then(a.op.cmp(&b.op)));
    Ok(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Writing
// ─────────────────────────────────────────────────────────────────────────────

pub struct Committed {
    pub snapshot: Snapshot,
    pub touched: Vec<PathBuf>,
    pub minted: Vec<EntityRef>,
    pub rev: u64,
}

pub struct Store<'c> {
    ctx: &'c Ctx,
}

/// One file's staged edit. Nothing here has touched the disk yet, which is what lets step
/// 8 (`prove`) REFUSE a plan instead of having to undo one.
enum Staged {
    /// an entity file, edited in place or created whole
    Doc { doc: MdDoc, create: bool },
    /// generated projections and the gitstate cache
    Raw(Vec<u8>),
    /// `comments.jsonl`
    Append(Vec<String>),
}

impl<'c> Store<'c> {
    pub fn open(ctx: &'c Ctx) -> Store<'c> {
        Store { ctx }
    }

    /// THE single write path. CLI handlers and axum POST handlers call it byte-identically,
    /// because both go through the same `cmd::*` function.
    ///
    /// 1. `LockToken::acquire` (typed, diagnosable contention error)
    /// 2. load a FRESH `Snapshot` *inside* the lock — never trust one taken before we had
    ///    exclusivity
    /// 3. build a `Minter` over `snap.taken_ids()` (incl. `closed_ids`)
    /// 4. run the PURE planner; all validation lives there
    /// 5. `Plan::validate`
    /// 6. `transitions::prove()` on every ticket the plan TRANSITIONS, as it arrived —
    ///    the write path proves its own legality, so a hand-edit is caught by the very
    ///    NEXT VERB (`Verb::Repair` is exempt, D-12)
    /// 7. stage per file in memory; `fm::writable()` runs on load, BEFORE any byte moves,
    ///    and `SetOutcome::ReplacedMultiline` is a HARD error
    /// 8. `transitions::prove()` again on the STAGED bytes, so an illegal plan is refused
    ///    rather than half-applied
    /// 9. apply: tmp + `fs::rename` + fsync the dir, per file. A ticket's frontmatter
    ///    delta and its `## Log` line are ONE write to ONE file; cross-file plans order
    ///    the authoritative file LAST (R-1)
    /// 10. `sync = "commit"` -> `git add -A .kanspec && git commit -m "kanspec: <verb> <id>"`
    /// 11. drop the lock; return the post-write snapshot with `rev + 1`
    ///
    /// gh/network calls MUST happen BEFORE `transact` (see `Facts`): a wedged subprocess
    /// inside the lock stalls the browser and every CLI verb for the lock timeout.
    ///
    /// `verb` is `None` for a plan that transitions no ticket — a cache write, a projection
    /// rewrite, or any knowledge verb (`spec new`, `decide`, `accept`, `quirk add`,
    /// `features --confirm`). ROUND-C CONTRACT CHANGE, requested independently by S3, S5
    /// and S6: `Verb` is a TICKET transition verb and reaches only this commit subject, so
    /// ten of the crate's twenty call sites were passing `Verb::Confirm` purely to satisfy
    /// the parameter — and under `sync = "commit"` that wrote `kanspec: confirm auth` into
    /// git history for what was actually `spec new auth`. `Confirm` means a specific thing
    /// (D-11, the human merge override); borrowing it as filler made the log lie. `None`
    /// commits as `kanspec: update <id>`.
    pub fn transact<F>(&self, verb: Option<Verb>, cmdline: &str, planner: F) -> Result<Committed>
    where
        F: FnOnce(&Snapshot, &Minter) -> Result<Plan>,
    {
        let ctx = self.ctx;
        ctx.require_initialized()?;

        // 1 — the lock, held for every step below.
        let token = LockToken::acquire(
            &ctx.layout,
            LockOwner::here(cmdline, ctx.now),
            Duration::from_secs(ctx.cfg.lock_timeout_secs),
        )?;

        // 2 — a snapshot taken before we had exclusivity is a TOCTOU, so it is reloaded.
        let snap = load_snapshot(ctx)?;

        // 3/4 — mint against the FRESH id set (which includes closed proposal ids), then
        // run the pure planner. Every decision the verb makes happens here, with no IO.
        let taken = snap.taken_ids();
        let minter = Minter::new(&taken, id_seed(), ctx.cfg.id_width);
        let plan = planner(&snap, &minter)?;

        // 5
        plan.validate(&snap)?;
        if plan.is_empty() {
            // Nothing to do is a success, not a write: `scan` on an unchanged repo and a
            // re-run `ship` both land here.
            let rev = REV.load(Ordering::Relaxed);
            return Ok(Committed {
                snapshot: snap,
                touched: Vec::new(),
                minted: plan.minted,
                rev,
            });
        }

        // 6 — prove the ticket AS IT ARRIVED, before the plan gets a chance to launder
        // it. A hand-edited `state:` is legal to transition FROM, so the post-write proof
        // would replay clean and the edit would wash out silently. Proving the pre-state
        // is what makes "a hand-edit is caught by the very next verb" true rather than
        // aspirational. `Repair` is exempt by design (D-12): it is the one verb whose
        // whole job is to make an already-broken ticket writable again.
        for op in &plan.ops {
            let Op::Transition { id, verb, .. } = op else {
                continue;
            };
            if *verb == Verb::Repair {
                continue;
            }
            let t = snap.ticket(id)?;
            transitions::prove(t).map_err(|v| broken_log(id, v))?;
        }

        // 7 — stage every file in memory. `fm::writable` runs on load, BEFORE the first
        // `set`, which is what turns the indexer's one real hazard (a non-plain key
        // silently gaining a duplicate) into a typed refusal.
        let mut staged: Vec<(PathBuf, Staged)> = Vec::new();
        let mut moves: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut authoritative: HashSet<PathBuf> = HashSet::new();
        for op in &plan.ops {
            self.stage(op, &snap, &mut staged, &mut moves, &mut authoritative)?;
        }

        // 8 — and prove the STAGED bytes too. The contract says "prove on commit";
        // proving before the write is strictly stronger, because an illegal plan is
        // refused instead of half-applied.
        for (path, s) in &staged {
            let Staged::Doc { doc, .. } = s else { continue };
            if !is_ticket_path(&ctx.layout, path) {
                continue;
            }
            let t = staged_ticket(path, doc)?;
            transitions::prove(&t).map_err(|v| broken_log(&t.fm.id, v))?;
        }

        // 9 — the authoritative file goes LAST, so an interruption leaves the ticket
        // without its transition rather than with a transition whose side effects never
        // happened.
        staged.sort_by_key(|(p, _)| authoritative.contains(p));

        let mut touched = Vec::new();
        for (path, s) in &staged {
            match s {
                Staged::Doc { doc, create: true } => {
                    create_new(path, doc.render().as_bytes(), &token)?
                }
                Staged::Doc { doc, create: false } => {
                    write_atomic(path, doc.render().as_bytes(), &token)?
                }
                Staged::Raw(bytes) => write_atomic(path, bytes, &token)?,
                Staged::Append(lines) => {
                    for line in lines {
                        append_line(path, line, &token)?;
                    }
                }
            }
            touched.push(path.clone());
        }
        for (from, to) in &moves {
            move_dir(from, to, &token)?;
            touched.push(to.clone());
        }

        // 10 — but only for a plan that changed something git tracks. A `scan`'s entire
        // plan is one write to the GITIGNORED cache, and committing for it would (a) sweep
        // whatever tracker edits were pending into a commit labelled after the scan, and
        // (b) have the `post-merge` hook's `kanspec scan --quiet` reach for git's index in
        // the middle of a merge.
        if ctx.cfg.sync == crate::config::SyncMode::Commit
            && plan.ops.iter().any(Op::writes_tracked_file)
        {
            let subject = plan
                .ops
                .iter()
                .find_map(|o| match o {
                    Op::Transition { id, .. } => Some(id.to_string()),
                    Op::CreateProposal { id, .. } => Some(id.to_string()),
                    Op::StampRule { spec, .. } => Some(spec.to_string()),
                    _ => o.entity().map(EntityRef::id),
                })
                .unwrap_or_else(|| "store".to_string());
            let label = verb.map_or("update", |v| v.as_str());
            ctx.git
                .commit_kanspec(&format!("kanspec: {label} {subject}"))?;
        }

        // 11 — the post-write view, still under the lock, so the caller (and the server's
        // memo) can never observe the pre-write state.
        let mut snapshot = load_snapshot(ctx)?;
        let rev = REV.fetch_add(1, Ordering::Relaxed) + 1;
        snapshot.rev = rev;
        drop(token);
        Ok(Committed {
            snapshot,
            touched,
            minted: plan.minted,
            rev,
        })
    }

    fn stage(
        &self,
        op: &Op,
        snap: &Snapshot,
        staged: &mut Vec<(PathBuf, Staged)>,
        moves: &mut Vec<(PathBuf, PathBuf)>,
        authoritative: &mut HashSet<PathBuf>,
    ) -> Result<()> {
        let l = &self.ctx.layout;
        match op {
            Op::Transition {
                id,
                verb,
                actor,
                at,
                detail,
                also,
            } => {
                let path = l.ticket(id);
                let from = snap.ticket(id)?.fm.state;
                // `Op::Transition` computes its own destination through the ONE oracle, so
                // a caller cannot pass a state the table never produces.
                let to = transitions::require(id, from, *verb)?;
                let doc = doc_at(&path, staged, false)?;
                set_key(
                    doc,
                    "state",
                    &Yv::s(to.as_str()),
                    crate::keys::TICKET_ORDER,
                    &path,
                )?;
                for (k, v) in also {
                    use crate::keys::FmKey as _;
                    set_key(doc, k.as_str(), v, crate::keys::TICKET_ORDER, &path)?;
                }
                // The frontmatter delta and the `## Log` line are ONE write to ONE file,
                // so omitting the log append is unavailable rather than merely rejected.
                let entry = LogEntry {
                    at: *at,
                    state: to,
                    actor: actor.label(),
                    verb: *verb,
                    note: Some(detail.clone()).filter(|d| !d.trim().is_empty()),
                };
                fm::append_to_section(doc, LOG_HEADING, &entry.format());
                authoritative.insert(path);
            }
            Op::SetFields { entity, sets } => {
                let path = l.path_for(entity);
                let path = entity_file(entity, path, l);
                let doc = doc_at(&path, staged, false)?;
                for (k, v) in sets {
                    set_key(doc, k.as_str(), v, k.order(), &path)?;
                }
            }
            Op::CreateEntity { entity, contents } => {
                let path = entity_file(entity, l.path_for(entity), l);
                let doc = fm::split(contents).map_err(|e| {
                    KsError::invalid(
                        format!("cannot create {entity}: {e}"),
                        fixes![fix!("kanspec doctor")],
                    )
                })?;
                writable_or_refuse(&doc, &path)?;
                staged.push((path, Staged::Doc { doc, create: true }));
            }
            Op::AppendSection {
                entity,
                heading,
                line,
            } => {
                let path = entity_file(entity, l.path_for(entity), l);
                let doc = doc_at(&path, staged, false)?;
                fm::append_to_section(doc, heading, line);
            }
            Op::MarkSteps { id, checks } => {
                let path = l.ticket(id);
                let doc = doc_at(&path, staged, false)?;
                for (index, done) in checks {
                    if !fm::mark_step(doc, *index, *done) {
                        return Err(KsError::invalid(
                            format!("{id} has no step {index}"),
                            fixes![fix!("kanspec show {id}")],
                        ));
                    }
                }
            }
            Op::CreateProposal { id, slug, contents } => {
                let path = l.proposal_md(&l.proposal_dir(id, slug));
                let doc = fm::split(contents).map_err(|e| {
                    KsError::invalid(
                        format!("cannot create proposal {id}: {e}"),
                        fixes![fix!("kanspec doctor")],
                    )
                })?;
                writable_or_refuse(&doc, &path)?;
                staged.push((path, Staged::Doc { doc, create: true }));
            }
            Op::StampRule { spec, anchor, line } => {
                let path = l.spec(spec);
                let doc = doc_at(&path, staged, false)?;
                if !fm::stamp_rule(doc, *line, anchor, crate::rulesdoc::ADOPTED_TOKEN) {
                    // The snapshot was reloaded inside the lock, so this means the bullet
                    // moved between planning and applying — refuse rather than stamp a
                    // token onto whatever line took its place.
                    return Err(KsError::conflict(
                        format!("spec {spec} no longer has rule [{anchor}] on line {line}"),
                        fixes![fix!("kanspec rules --audit")],
                    ));
                }
            }
            Op::AppendJsonl { path, line } => match slot(staged, path) {
                Some(Staged::Append(lines)) => lines.push(line.clone()),
                _ => staged.push((path.clone(), Staged::Append(vec![line.clone()]))),
            },
            Op::MoveDir { from, to } => moves.push((from.clone(), to.clone())),
            Op::WriteGenerated { path, contents } => {
                staged.push((path.clone(), Staged::Raw(contents.as_bytes().to_vec())))
            }
            Op::WriteGitState { state, .. } => staged.push((
                l.gitstate(),
                Staged::Raw(crate::cache::render(state)?.into_bytes()),
            )),
        }
        Ok(())
    }
}

/// A proposal's `EntityRef` names a DIRECTORY; every other kind names the file itself.
fn entity_file(e: &EntityRef, p: PathBuf, l: &crate::paths::Layout) -> PathBuf {
    match e {
        EntityRef::Proposal(_) => l.proposal_md(&p),
        _ => p,
    }
}

fn slot<'a>(staged: &'a mut [(PathBuf, Staged)], path: &Path) -> Option<&'a mut Staged> {
    staged.iter_mut().find(|(p, _)| p == path).map(|(_, s)| s)
}

/// The staged `MdDoc` for `path`, loading it from disk on first touch. The `fm::writable`
/// guard runs HERE — before any `set` — which is the only point at which refusing is free.
fn doc_at<'a>(
    path: &Path,
    staged: &'a mut Vec<(PathBuf, Staged)>,
    _create: bool,
) -> Result<&'a mut MdDoc> {
    if let Some(i) = staged.iter().position(|(p, _)| p == path) {
        return match &mut staged[i].1 {
            Staged::Doc { doc, .. } => Ok(doc),
            _ => Err(KsError::conflict(
                format!(
                    "{} is written two different ways in one plan",
                    path.display()
                ),
                fixes![fix!("kanspec doctor")],
            )),
        };
    }
    let src = std::fs::read_to_string(path).map_err(|e| io_err(path, "read", e))?;
    let doc = fm::split(&src).map_err(|e| {
        KsError::invalid(
            format!("{}: {e}", path.display()),
            fixes![fix!("kanspec doctor")],
        )
    })?;
    writable_or_refuse(&doc, path)?;
    staged.push((path.to_path_buf(), Staged::Doc { doc, create: false }));
    match &mut staged.last_mut().expect("just pushed").1 {
        Staged::Doc { doc, .. } => Ok(doc),
        _ => unreachable!("just pushed a Doc"),
    }
}

fn writable_or_refuse(doc: &MdDoc, path: &Path) -> Result<()> {
    fm::writable(&doc.fm).map_err(|why| {
        KsError::invalid(
            format!("{}: {why}", path.display()),
            fixes![
                fix!(
                    "open {} and rewrite the keys as plain `key: value`",
                    path.display()
                ),
                fix!("kanspec doctor"),
            ],
        )
    })
}

fn set_key(doc: &mut MdDoc, key: &str, val: &Yv, order: &[&str], path: &Path) -> Result<()> {
    match fm::set(doc, key, val, order) {
        // The ONE lossy path in the writer: collapsing a block scalar or a nested map to
        // one line. No v0.1 or v0.2 field holds one, so this is a hard refusal rather than
        // a silent reformat — and it is where the schema growing one fails loudly (R-9).
        SetOutcome::ReplacedMultiline => Err(KsError::invalid(
            format!(
                "{}: `{key}` holds a multi-line YAML value, which kanspec will not rewrite \
                 in place",
                path.display()
            ),
            fixes![
                fix!("edit {} by hand", path.display()),
                fix!("kanspec doctor"),
            ],
        )),
        _ => Ok(()),
    }
}

/// The one wording for "this ticket's `## Log` does not add up". `repair` is always in the
/// fix list, because a ticket nobody can write is worse than a ticket somebody has to
/// re-attest (D-12).
fn broken_log(id: &TicketId, v: crate::transitions::LogViolation) -> KsError {
    KsError::gate(
        "log_violation",
        format!("{id}: {v}"),
        fixes![
            fix!("kanspec log {id}"),
            fix!("kanspec repair {id} --why \"...\""),
        ],
    )
}

fn is_ticket_path(l: &crate::paths::Layout, p: &Path) -> bool {
    p.parent() == Some(l.tickets_dir().as_path())
}

/// Re-parses a staged ticket so `prove` sees exactly the bytes that are about to land.
fn staged_ticket(path: &Path, doc: &MdDoc) -> Result<Ticket> {
    let fm: crate::model::TicketFm = parse_fm(&doc.fm, path)?;
    Ok(Ticket {
        steps: fm::steps(&doc.body)
            .into_iter()
            .map(|(index, done, text)| Step { index, done, text })
            .collect(),
        log: logentry::parse_log(&doc.body),
        fm,
        path: path.to_path_buf(),
        body: doc.body.clone(),
        mtime: SystemTime::now(),
    })
}

/// `KANSPEC_ID_SEED` in tests; otherwise a per-process value. Minting is check-and-retry
/// against the in-lock id set, so collision-freedom is a property of EXCLUSION rather than
/// of this number's entropy.
fn id_seed() -> u64 {
    if let Ok(s) = std::env::var("KANSPEC_ID_SEED") {
        if let Ok(n) = s.parse::<u64>() {
            return n;
        }
    }
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

// ─────────────────────────────────────────────────────────────────────────────
// The ONLY fs-mutating functions in the crate
// ─────────────────────────────────────────────────────────────────────────────

/// tmp + `fs::rename` + fsync the containing directory.
pub(crate) fn write_atomic(p: &Path, bytes: &[u8], _t: &LockToken) -> Result<()> {
    let dir = parent_of(p)?;
    std::fs::create_dir_all(dir).map_err(|e| io_err(dir, "create", e))?;
    let tmp = dir.join(format!(
        ".{}.tmp.{}",
        p.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| io_err(&tmp, "create", e))?;
        f.write_all(bytes).map_err(|e| io_err(&tmp, "write", e))?;
        f.sync_all().map_err(|e| io_err(&tmp, "fsync", e))?;
    }
    std::fs::rename(&tmp, p).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_err(p, "rename onto", e)
    })?;
    fsync_dir(dir);
    Ok(())
}

/// `comments.jsonl` — append-only, `merge=union`, id-deduped on read.
pub(crate) fn append_line(p: &Path, line: &str, _t: &LockToken) -> Result<()> {
    let dir = parent_of(p)?;
    std::fs::create_dir_all(dir).map_err(|e| io_err(dir, "create", e))?;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .map_err(|e| io_err(p, "open", e))?;
    writeln!(f, "{}", line.trim_end()).map_err(|e| io_err(p, "append to", e))?;
    f.sync_all().map_err(|e| io_err(p, "fsync", e))?;
    Ok(())
}

/// `close`: `proposals/<id>-<slug>` -> `proposals/closed/<id>-<slug>`.
pub(crate) fn move_dir(a: &Path, b: &Path, _t: &LockToken) -> Result<()> {
    if b.exists() {
        return Err(KsError::conflict(
            format!("{} already exists — refusing to overwrite it", b.display()),
            fixes![fix!("kanspec doctor")],
        ));
    }
    let dir = parent_of(b)?;
    std::fs::create_dir_all(dir).map_err(|e| io_err(dir, "create", e))?;
    std::fs::rename(a, b).map_err(|e| io_err(a, "move", e))?;
    fsync_dir(dir);
    Ok(())
}

/// Errors if the file already exists — `Op::CreateEntity` must never clobber.
pub(crate) fn create_new(p: &Path, bytes: &[u8], _t: &LockToken) -> Result<()> {
    let dir = parent_of(p)?;
    std::fs::create_dir_all(dir).map_err(|e| io_err(dir, "create", e))?;
    let mut f = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(KsError::conflict(
                format!("{} already exists — refusing to overwrite it", p.display()),
                fixes![fix!("kanspec doctor")],
            ))
        }
        Err(e) => return Err(io_err(p, "create", e)),
    };
    f.write_all(bytes).map_err(|e| io_err(p, "write", e))?;
    f.sync_all().map_err(|e| io_err(p, "fsync", e))?;
    fsync_dir(dir);
    Ok(())
}

fn parent_of(p: &Path) -> Result<&Path> {
    p.parent().ok_or_else(|| {
        KsError::internal(anyhow::anyhow!("{} has no parent directory", p.display()))
    })
}

/// A rename is only durable once the DIRECTORY entry is on disk. Best-effort: a
/// filesystem that refuses to fsync a directory is not a reason to fail the write.
fn fsync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_carry_their_bracket_id_and_their_provenance() {
        let body = "\
# auth

## Rules
- [auth.jwt] Login issues a JWT valid 24h in an httpOnly cookie. {p-02cc}
- [auth.lockout] 5 failed logins within 10m locks the account for 15m. {p-7de2} {p-8a10}
- [auth.plain] No provenance yet.
- [ ] a checkbox is not a rule
- [x] nor is a ticked one
";
        let rules = parse_rules(body);
        assert_eq!(rules.len(), 3, "{rules:#?}");
        assert_eq!(rules[0].anchor, "auth.jwt");
        assert_eq!(
            rules[0].text,
            "Login issues a JWT valid 24h in an httpOnly cookie."
        );
        assert_eq!(
            rules[0]
                .provenance
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            ["p-02cc"]
        );
        assert_eq!(
            rules[1]
                .provenance
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            ["p-7de2", "p-8a10"]
        );
        assert!(rules[2].provenance.is_empty());
        assert_eq!(rules[0].line, 4);
    }

    #[test]
    fn proposal_items_are_anchored_and_every_prescription_is_typed() {
        let pid = ProposalId::parse("p-7de2").unwrap();
        let body = "\
## Changes
- [c1] auth: 5 failed logins in 10m locks the account 15m
- [c3] ops: alert when lockouts exceed 100/hour

## Prescriptions
- [p1] (promote: decision) Rate-limit state lives in Redis only.
- [p2] (temp until t-31aa) Keep the legacy captcha path.
- [p3] Nobody typed this one.

## Tickets
- [t1] Rate-limit login endpoint
";
        let items = parse_items(&pid, body);
        let ids: Vec<String> = items.iter().map(|i| i.id.to_string()).collect();
        assert_eq!(
            ids,
            [
                "p-7de2#c1",
                "p-7de2#c3",
                "p-7de2#p1",
                "p-7de2#p2",
                "p-7de2#p3",
                "p-7de2#t1"
            ]
        );
        assert!(matches!(
            items[2].prescription,
            Some(Prescription::Promote(PromoteAs::Decision))
        ));
        match &items[3].prescription {
            Some(Prescription::TempUntil(t)) => assert_eq!(t.as_str(), "t-31aa"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(items[4].prescription, Some(Prescription::Untyped)));
        // A change item is not a prescription at all.
        assert!(items[0].prescription.is_none());
    }

    #[test]
    fn comment_rows_dedupe_on_id_op_and_time() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("comments.jsonl");
        let comment = r#"{"id":"cm-88f1","op":"comment","target":"p-7de2#c3","body":"x","at":"2026-08-30T16:02:00Z"}"#;
        let reply = r#"{"id":"cm-88f1","op":"reply","body":"y","at":"2026-08-30T16:21:40Z"}"#;
        std::fs::write(
            &p,
            format!("{comment}\n{comment}\n{reply}\n\nnot json at all\n"),
        )
        .unwrap();
        let rows = read_comments(&p).unwrap();
        assert_eq!(rows.len(), 2, "one id carries several ops: {rows:#?}");
        assert_eq!(rows[0].op, crate::model::CommentOpKind::Comment);
        assert_eq!(rows[1].op, crate::model::CommentOpKind::Reply);
        // A missing file is no comments, never an error.
        assert!(read_comments(&tmp.path().join("nope.jsonl"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_closed_proposal_directory_yields_its_id_and_nothing_else() {
        assert_eq!(
            dir_proposal_id(Path::new("/x/proposals/closed/p-19f0-money-as-cents")).as_deref(),
            Some("p-19f0")
        );
        assert_eq!(
            // The un-slugged form `Layout::path_for` falls back to must resolve too, or a
            // closed proposal could silently stop excluding its own id from the minter.
            dir_proposal_id(Path::new("/x/proposals/closed/p-19f0")).as_deref(),
            Some("p-19f0")
        );
        assert_eq!(dir_proposal_id(Path::new("/x/proposals/closed/junk")), None);
    }

    #[test]
    fn the_write_primitives_are_atomic_and_refuse_to_clobber() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(root)
            .status()
            .unwrap()
            .success());
        let repo = crate::paths::Repo::discover(root, None).unwrap();
        let layout = crate::paths::Layout::open(&repo, &crate::config::Config::default());
        let token = LockToken::acquire(
            &layout,
            LockOwner::here("test", chrono::Utc::now()),
            Duration::from_secs(1),
        )
        .unwrap();

        let f = layout.tickets_dir().join("t-9c41.md");
        create_new(&f, b"one", &token).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "one");
        assert_eq!(
            create_new(&f, b"two", &token).unwrap_err().kind(),
            "conflict",
            "CreateEntity must never clobber"
        );
        write_atomic(&f, b"three", &token).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "three");
        // No temp files are left behind.
        let leftovers: Vec<_> = std::fs::read_dir(layout.tickets_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");

        let j = layout.cache_dir().join("comments.jsonl");
        append_line(&j, "{\"a\":1}", &token).unwrap();
        append_line(&j, "{\"a\":2}\n", &token).unwrap();
        assert_eq!(
            std::fs::read_to_string(&j).unwrap(),
            "{\"a\":1}\n{\"a\":2}\n"
        );

        let a = layout.proposals_dir().join("p-7de2-x");
        std::fs::create_dir_all(&a).unwrap();
        let b = layout.proposals_closed_dir().join("p-7de2-x");
        move_dir(&a, &b, &token).unwrap();
        assert!(b.is_dir() && !a.exists());
        std::fs::create_dir_all(&a).unwrap();
        assert_eq!(move_dir(&a, &b, &token).unwrap_err().kind(), "conflict");
    }
}
