//! The merge-detection ladder and the sealed proof.
//!
//! [`MergedProof`] has private fields, **no `Default`, no `Deserialize`, no
//! `From<MergeFact>`**. The only constructors live in this file and each one ran a real
//! ladder, so `plan_done`'s signature makes invariant 1 a *compile-time* guarantee: a
//! `done` that never consulted git does not build. `tests/proof_is_sealed.rs` greps for
//! the impls that would break it — it *will* be tempting the first time someone wants a
//! fast `status`.
//!
//! # What the ladder is allowed to say
//!
//! Every rung answers **merged / not-merged / inconclusive**, and only a rung that can
//! *prove absence* may say no. Ancestry-negative is inconclusive, because a squash-merged
//! branch is genuinely not an ancestor of main. That asymmetry is invariant 2, and it is
//! why `unknown` is a first-class verdict carrying its reason rather than an error path: a
//! confident wrong answer is the worst thing this file can produce, and an honest
//! `unknown (squash suspected, no gh)` is a success.
//!
//! # The two storage tiers, and why they differ
//!
//! The ladder's result goes to `cache/gitstate.json` as a plain [`crate::cache::MergeFact`]
//! — **badge-grade**, forgeable by a text editor, and disposable. The `done` gate never
//! reads it: [`proof_for_done`] re-runs the ladder and mints a fresh, sealed
//! [`MergedProof`] — **proof-grade**. That split is J-8, and [`Detection::to_fact`] is
//! deliberately one-way.
//!
//! The one human input is [`plan_confirm`], and it is stored in the third place: the
//! **ticket's own `## Log`** (D-11). An attestation is an asserted act with an actor, so it
//! must survive `rm -rf cache/` and be visibly signed. Every subsequent [`scan_all`] reads
//! it back out of the log and projects it into the cache, which is what makes the override
//! outlive a cache wipe instead of needing to be repeated.
//!
//! Owner: **S3**.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::cache::{BranchFact, GitState, MergeFact, MergeStatus, SpecAnchor};
use crate::ctx::{Actor, Ctx};
use crate::derive::note_sha;
use crate::error::{GateCode, GateDetail, KsError, Result};
use crate::gh::{merged_pr, Gh, GhUnavailable};
use crate::git::{Git, Method, Pathspec, RungTrace, Sha, Tri, Unknown};
use crate::ids::TicketId;
use crate::logentry::LOG_HEADING;
use crate::model::{DecisionStatus, QuirkStatus, Snapshot, Spec, Ticket};
use crate::plan::{EntityRef, Op, Plan};
use crate::transitions::Verb;
use crate::{fix, fixes};

/// The `## Log` note `scan --confirm` writes and [`confirmed_proof`] reads back. It is a
/// stable prefix rather than free prose, because this line **is** the attestation's
/// storage — the cache is not. `derive::PROOF_NOTE` mirrors it (that file may import
/// nothing from here), and [`note_sha`] is the one parser both sides read it back through.
const CONFIRM_NOTE: &str = "in main";

// ─────────────────────────────────────────────────────────────────────────────
// The seals
// ─────────────────────────────────────────────────────────────────────────────

/// Same-module privacy — no `pub(in …)`, which does not compile in a flat layout (E0742).
///
/// The seal is four absences, each of which `tests/proof_is_sealed.rs` greps for and each
/// of which is demonstrated here as a compile-fail doctest. First, the honest half of the
/// split — the *cache* DTO really is deserializable, which is what makes these tests
/// meaningful rather than a compiler that simply cannot see `serde`:
///
/// ```
/// fn de<T: serde::de::DeserializeOwned>() {}
/// fn dflt<T: Default>() {}
/// de::<kanspec::cache::MergeFact>();     // badge-grade: a text editor can write one
/// dflt::<kanspec::cache::GitState>();
/// ```
///
/// …and now the proof-grade value, which none of that is true of:
///
/// ```compile_fail
/// fn de<T: serde::de::DeserializeOwned>() {}
/// de::<kanspec::scan::MergedProof>();    // E0277: no `Deserialize` — a cache cannot mint one
/// ```
///
/// ```compile_fail
/// fn dflt<T: Default>() {}
/// dflt::<kanspec::scan::MergedProof>();  // E0277: no `Default` — there is no empty proof
/// ```
///
/// The last one needs its own control, since a typo'd path would "pass" a compile-fail
/// test on its own. Reading the SHA through the accessor compiles; reaching for the field
/// does not:
///
/// ```
/// fn read(p: &kanspec::scan::MergedProof) -> &kanspec::git::Sha { p.sha() }
/// ```
///
/// ```compile_fail
/// // E0616: private field, and no public constructor anywhere in the crate.
/// fn forge(p: &kanspec::scan::MergedProof) -> &kanspec::git::Sha { &p.sha }
/// ```
#[derive(Clone, Debug, Serialize)]
pub struct MergedProof {
    ticket: TicketId,
    sha: Sha,
    method: Method,
    pr: Option<u64>,
    checked_at: DateTime<Utc>,
}

impl MergedProof {
    /// One of the TWO mints in the crate (the other is [`confirmed_proof`]), and it takes a
    /// [`Detection`] — a value only [`ladder`] can build.
    fn from_detection(ticket: &TicketId, d: &Detection) -> Option<MergedProof> {
        match &d.verdict {
            Verdict::Landed { sha, method, pr } => Some(MergedProof {
                ticket: ticket.clone(),
                sha: sha.clone(),
                method: *method,
                pr: *pr,
                checked_at: d.checked_at,
            }),
            _ => None,
        }
    }

    pub fn ticket(&self) -> &TicketId {
        &self.ticket
    }
    pub fn sha(&self) -> &Sha {
        &self.sha
    }
    pub fn method(&self) -> Method {
        self.method
    }
    pub fn pr(&self) -> Option<u64> {
        self.pr
    }
    pub fn checked_at(&self) -> DateTime<Utc> {
        self.checked_at
    }
    /// `"IN MAIN (gh-pr #142 · checked 11s ago)"`
    ///
    /// `now` is the CALLER'S clock — `ctx.now` — never `Utc::now()`. Determinism in this
    /// crate comes from exactly three env overrides (§9), and a badge that reads the wall
    /// clock renders "checked 3h ago" under `KANSPEC_NOW`, making every snapshot test of a
    /// transcript that shows it unstable. `derive::Badge::text` takes the same argument for
    /// the same reason.
    pub fn badge(&self, now: DateTime<Utc>) -> String {
        let pr = self.pr.map(|n| format!(" #{n}")).unwrap_or_default();
        format!(
            "IN MAIN ({}{pr} · checked {})",
            self.method,
            crate::out::rel_time(self.checked_at, now)
        )
    }
}

/// The chore/docs escape — a DIFFERENT type, so the landed path cannot accept it, and
/// [`NoCodeWaiver::record`] needs `&mut Plan` so the waiver is DURABLE before it is
/// usable. `plan_done` additionally refuses it from `review`, so `--no-code` provably
/// cannot bypass the gate on an already-shipped ticket.
#[derive(Clone, Debug, Serialize)]
pub struct NoCodeWaiver {
    why: String,
    by: String,
    at: DateTime<Utc>,
}

impl NoCodeWaiver {
    /// Refuses an empty `why`: an unexplained escape is the one thing this type exists to
    /// prevent.
    ///
    /// The durable record is a **prose line under the ticket's `## Log`** — deliberately
    /// not shaped like a [`crate::logentry::LogEntry`], so `transitions::replay` skips it
    /// (a second parseable entry for one act would break the very proof the log exists
    /// for) while a human reading the file, or `git log -p`, sees the waiver and its
    /// author. The `done` transition's own note carries the same `why`; this line is what
    /// survives independently of the planner that wrote it.
    pub fn record(
        plan: &mut Plan,
        id: &TicketId,
        why: &str,
        by: &Actor,
        at: DateTime<Utc>,
    ) -> Result<NoCodeWaiver> {
        let why = why.trim();
        if why.is_empty() {
            return Err(KsError::gate(
                GateCode::NoCodeWithoutWhy,
                format!("`{id}` cannot close as no-code without a recorded reason"),
                fixes![
                    fix!("kanspec done {id} --no-code --why \"docs only\""),
                    fix!("kanspec scan --explain {id}"),
                ],
            ));
        }
        let by = by.label();
        plan.push(Op::AppendSection {
            entity: EntityRef::Ticket(id.clone()),
            heading: LOG_HEADING,
            line: format!(
                "  no-code waiver by {by} at {}: {why}",
                at.format("%Y-%m-%dT%H:%MZ")
            ),
        });
        Ok(NoCodeWaiver {
            why: why.to_string(),
            by,
            at,
        })
    }
    pub fn why(&self) -> &str {
        &self.why
    }
    pub fn by(&self) -> &str {
        &self.by
    }
    pub fn at(&self) -> DateTime<Utc> {
        self.at
    }
}

/// What `plan_done` accepts. Two constructors, two very different stories, one gate.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "how", rename_all = "snake_case")]
pub enum Landed {
    Proof(MergedProof),
    NoCode(NoCodeWaiver),
}

/// Minted ONLY by [`scan_all`]; required by `Op::WriteGitState`. That is what makes
/// "gitstate.json is written by scan and nothing else" a type fact rather than a comment.
#[derive(Debug, PartialEq)]
pub struct ScanToken(());

// ─────────────────────────────────────────────────────────────────────────────
// The ladder's value types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Landed {
        sha: Sha,
        method: Method,
        pr: Option<u64>,
    },
    NotLanded,
    Unknown(Unknown),
}

/// A completed ladder run. Carries its own `rungs`, so `scan --explain` is a property of
/// the value [`ladder`] already produced — there is no SECOND ladder run that could
/// disagree with the first.
#[derive(Clone, Debug, Serialize)]
pub struct Detection {
    verdict: Verdict,
    checked_at: DateTime<Utc>,
    fetch_age_secs: Option<u64>,
    rungs: Vec<RungTrace>,
}

impl Detection {
    fn seal(v: Verdict, at: DateTime<Utc>, age: Option<u64>, r: Vec<RungTrace>) -> Detection {
        Detection {
            verdict: v,
            checked_at: at,
            fetch_age_secs: age,
            rungs: r,
        }
    }
    pub fn verdict(&self) -> &Verdict {
        &self.verdict
    }
    pub fn checked_at(&self) -> DateTime<Utc> {
        self.checked_at
    }
    pub fn explain(&self) -> &[RungTrace] {
        &self.rungs
    }
    /// Down-converts the sealed, in-process value to the plain cache DTO. The cache is
    /// badge-grade; the gate is proof-grade. **There is deliberately no inverse.**
    pub fn to_fact(&self, changed: Vec<String>) -> MergeFact {
        let (status, sha, method, pr, why) = match &self.verdict {
            Verdict::Landed { sha, method, pr } => (
                MergeStatus::Merged,
                Some(sha.as_str().to_string()),
                *method,
                *pr,
                None,
            ),
            // No method concluded, so none is claimed. The rung table lives in
            // `Detection`, which is where `--explain` reads it from.
            Verdict::NotLanded => (MergeStatus::NotMerged, None, Method::None, None, None),
            // The badge explains itself without re-running anything.
            Verdict::Unknown(u) => (
                MergeStatus::Unknown,
                None,
                Method::None,
                None,
                Some(u.badge()),
            ),
        };
        MergeFact {
            status,
            sha,
            method,
            pr,
            why,
            checked_at: self.checked_at,
            changed,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The ladder
// ─────────────────────────────────────────────────────────────────────────────

/// Where the SHA the ladder reasons about came from. Load-bearing for guard 0b — see
/// [`ladder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeadOrigin {
    /// The `head:` frontmatter field, written by `ship` from real git output. Its presence
    /// is proof the branch carried commits of its own.
    Recorded,
    /// The branch tip, resolved live. Says nothing about whether the branch ever carried a
    /// commit.
    BranchTip,
}

/// `<head:>` if recorded, else the branch tip — DESIGN.md's "head-or-tip". Both are
/// resolved *through git*, because [`Sha`] has no public constructor: a SHA in this crate
/// is provably something git printed, never something an agent typed.
fn head_of(git: &Git, t: &Ticket) -> std::result::Result<(Sha, HeadOrigin), Unknown> {
    let (rev, origin) = ticket_rev(t).ok_or(Unknown::NoHead)?;
    match git.head_sha(&rev) {
        Ok(hs) => Ok((hs.sha().clone(), origin)),
        // A recorded head gc'd after reflog expiry (D-8) or rewritten away, or a branch
        // deleted after the merge with no `head:` ever recorded: nothing left to ask git
        // about. Never "not merged".
        Err(_) => Err(Unknown::HeadNotInObjectStore { sha: rev }),
    }
}

fn trace(method: Method, cmd: &str, exit: i32, saw: &str, verdict: &'static str) -> RungTrace {
    RungTrace {
        method,
        cmd: cmd.to_string(),
        exit,
        saw: saw.to_string(),
        verdict,
    }
}

/// What the `cmd` column says when the rung ran no command at all. Deliberately not
/// command-shaped: `--explain` is read as a list of things that were done, and a line in
/// that list that was never done is the defect this constant exists to prevent.
const NOT_QUERIED: &str = "(gh not queried)";

/// Rung 2's trace when `gh` declined **before a query was ever spawned**.
///
/// [`Gh::available`] collapses three different refusals into one `false` — `[git] gh =
/// "never"`, an `origin` `gh` cannot speak for, and a `gh auth status` that failed or
/// found no `gh` — and this rung used to report the third for all three. `--explain` is
/// the audit surface a user reads *precisely* when the tool has disappointed them, so that
/// is the worst possible place to guess: told `gh auth status / exit 1 / unavailable`, a
/// user whose config says `never`, or whose origin is GitLab, runs `gh auth status`
/// themselves, watches it exit 0, and concludes kanspec is lying about the rest too.
///
/// So the reason is READ BACK from the gate every live query starts with, in `gh`'s own
/// words, instead of being restated here where it can drift. With `available()` false that
/// gate refuses from config plus an already-cached probe: it spawns nothing, reads no
/// fixture, and costs exactly what the hardcoded string it replaces cost.
fn gh_declined(gh: &Gh, t: &Ticket) -> RungTrace {
    let refused = match (t.fm.pr, t.fm.branch.as_deref()) {
        (Some(n), _) => gh.pr_view(n).map(|_| ()),
        (None, b) => gh.pr_for_head(b.unwrap_or_default()).map(|_| ()),
    };
    let why = match refused {
        Err(GhUnavailable(why)) => why,
        // Unreachable while `available()` is false — the gate refuses first. If that ever
        // stops being true, say so rather than inventing a reason for the audit trail.
        Ok(()) => "`gh` declined without saying why".to_string(),
    };
    trace(Method::GhPr, NOT_QUERIED, 1, &why, "inconclusive")
}

/// THE LADDER, in the recon-corrected order. Every rung returns a verdict; **exit 128
/// anywhere is Unknown, never No.**
///
/// - **guard 0** `rev-parse --verify '<head>^{commit}'` -> `NoHead` / `HeadNotInObjectStore`
/// - **guard 0b** `rev-list --count <main>..<head> == 0` -> `ZeroCommitBranch`
///   (a fresh `start` branch is trivially an ancestor of main — a VERIFIED false MERGED
///   for work that never happened)
/// - **1 ANCESTRY** `merge-base --is-ancestor` — 0 = Merged, 1 = next rung, 128 = Unknown
/// - **2 GH** `pr view <n>` / `pr list --head <branch>`; MERGED -> RE-VERIFY with
///   `is-ancestor(mergeCommit.oid)`; mismatch -> `GhMergedButNotAncestor`; any gh failure
///   -> `GhUnavailable`, NEVER NotMerged
/// - **3 TRAILER** `log <main> -E --grep 'Kanspec: t-9c41([^0-9a-f]|$)'` — unanchored and
///   boundary-terminated. Blind to reverts, so it is weighted BELOW ancestry
/// - **4 PATCH-ID** `cherry <main> <head>` — relabelled *rebase/cherry-pick detection*
///   (D-3). Merged iff output non-empty AND every line is `-`; any `+` ->
///   `SquashSuspectedNoGh`
/// - **5** otherwise Unknown, with every `RungTrace` attached
///
/// # Guard 0b's position (a deviation from §7's literal ordering, reported)
///
/// §7 runs guard 0b before rung 1, and `tests/common/merges.rs` records why that cannot
/// stand: after a true merge the branch tip **is** reachable from main, so
/// `rev-list --count main..head` is `0` for a real merge exactly as it is for a branch that
/// never committed. Run first, the guard answers `unknown` for the one shape ancestry can
/// prove; run after a *negative* ancestry (merges.rs's other suggestion) it can never fire
/// at all, since `count == 0` implies ancestry.
///
/// The two cases are not distinguishable by reachability — once merged, your commits are on
/// main either way — so the guard keys off **provenance** instead: it fires only when the
/// SHA came from a live branch tip with no recorded `head:`. `head:` is written by `ship`
/// out of real git output, so its presence means the branch demonstrably carried work; its
/// absence plus a zero-commit branch is precisely the fresh-`start` false MERGED the guard
/// was added (D-4) to prevent.
pub fn ladder(
    git: &Git,
    gh: &Gh,
    t: &Ticket,
    main: &str,
    fetch_age: Option<Duration>,
    now: DateTime<Utc>,
) -> Detection {
    let mut tr: Vec<RungTrace> = Vec::new();
    let age = fetch_age.map(|d| d.as_secs());
    macro_rules! done {
        ($v:expr) => {
            return Detection::seal($v, now, age, tr)
        };
    }

    // ── guard 0: is there anything to ask about? ─────────────────────────────
    let (head, origin) = match head_of(git, t) {
        Ok(h) => h,
        Err(u) => {
            let saw = u.badge();
            tr.push(trace(
                Method::None,
                "git rev-parse --verify <head-or-tip>^{commit}",
                1,
                &saw,
                "unknown",
            ));
            done!(Verdict::Unknown(u))
        }
    };

    // ── guard 0b: a branch that never carried a commit is not "merged" ───────
    if origin == HeadOrigin::BranchTip {
        let cmd = format!("git rev-list --count {main}..{}", head.short());
        match git.commits_ahead(main, &head) {
            Tri::Yes(0) => {
                tr.push(trace(Method::None, &cmd, 0, "0", "unknown"));
                done!(Verdict::Unknown(Unknown::ZeroCommitBranch))
            }
            Tri::Unknown(u) => {
                let saw = u.badge();
                tr.push(trace(Method::None, &cmd, 128, &saw, "unknown"));
                done!(Verdict::Unknown(u))
            }
            _ => {}
        }
    }

    // ── rung 1: ANCESTRY — exact for true merges and fast-forwards ───────────
    let cmd = format!("git merge-base --is-ancestor {} {main}", head.short());
    match git.is_ancestor(&head, main) {
        Tri::Yes(()) => {
            tr.push(trace(Method::Ancestry, &cmd, 0, "ancestor", "merged"));
            done!(Verdict::Landed {
                sha: head,
                method: Method::Ancestry,
                pr: t.fm.pr,
            })
        }
        Tri::Unknown(u) => {
            let saw = u.badge();
            tr.push(trace(Method::Ancestry, &cmd, 128, &saw, "unknown"));
            done!(Verdict::Unknown(u))
        }
        // Ancestry-NEGATIVE is inconclusive, not NotMerged — a squash-merged branch is
        // genuinely not an ancestor. Only rungs that can PROVE absence say No.
        Tri::No => tr.push(trace(
            Method::Ancestry,
            &cmd,
            1,
            "not an ancestor",
            "inconclusive",
        )),
    }

    // ── rung 2: GH — the ONLY rung that sees a title-only squash ─────────────
    if gh.available() {
        let (cmd, prs) = match t.fm.pr {
            Some(n) => (format!("gh pr view {n}"), gh.pr_view(n).map(|p| vec![p])),
            None => match t.fm.branch.as_deref() {
                Some(b) => (
                    format!("gh pr list --head {b} --state all"),
                    gh.pr_for_head(b),
                ),
                None => ("gh pr".to_string(), Ok(Vec::new())),
            },
        };
        match prs {
            // Absent gh, unauthenticated gh, a network failure, a missing fixture: every
            // one of them is inconclusive with a reason. None of them is a negative.
            Err(GhUnavailable(why)) => tr.push(trace(Method::GhPr, &cmd, 1, &why, "inconclusive")),
            Ok(list) => match merged_pr(&list) {
                None => tr.push(trace(
                    Method::GhPr,
                    &cmd,
                    0,
                    &format!("{} pr(s), none merged", list.len()),
                    "inconclusive",
                )),
                Some(pr) => match pr.merge_commit.as_deref() {
                    // GitHub says MERGED and cannot say what landed. Not a SHA we can
                    // re-verify, so not an answer.
                    None => tr.push(trace(
                        Method::GhPr,
                        &cmd,
                        0,
                        &format!("#{} MERGED, no merge commit", pr.number),
                        "inconclusive",
                    )),
                    Some(oid) => {
                        // Re-verify gh's CLAIM as a local git FACT, and get a real SHA.
                        // `head_sha` is the only public way to turn a string into a
                        // `Sha`, and it earns its keep here: it fails when the merge
                        // commit is not in our object store at all.
                        let local = git.head_sha(oid).ok().map(|h| h.sha().clone());
                        let ancestor =
                            local.filter(|s| matches!(git.is_ancestor(s, main), Tri::Yes(())));
                        match ancestor {
                            Some(sha) => {
                                tr.push(trace(
                                    Method::GhPr,
                                    &format!("{cmd} + is-ancestor {}", sha.short()),
                                    0,
                                    &format!("#{} MERGED", pr.number),
                                    "merged",
                                ));
                                done!(Verdict::Landed {
                                    sha,
                                    method: Method::GhPr,
                                    pr: Some(pr.number),
                                })
                            }
                            // A stale fetch or a different base branch. gh's word alone is
                            // not a confident answer about THIS main.
                            None => {
                                tr.push(trace(
                                    Method::GhPr,
                                    &format!("{cmd} + is-ancestor {oid}"),
                                    1,
                                    "merge commit is not on main",
                                    "unknown",
                                ));
                                done!(Verdict::Unknown(Unknown::GhMergedButNotAncestor {
                                    merge_sha: oid.to_string(),
                                }))
                            }
                        }
                    }
                },
            },
        }
    } else {
        tr.push(gh_declined(gh, t));
    }

    // ── rung 3: TRAILER — unanchored + boundary-terminated ───────────────────
    let cmd = format!(
        "git log {main} -E --grep 'Kanspec: {}' --format=%H",
        t.fm.id.as_str()
    );
    match git.grep_trailer(main, &t.fm.id) {
        Tri::Yes(shas) if !shas.is_empty() => {
            tr.push(trace(
                Method::Trailer,
                &cmd,
                0,
                &format!("{} hit(s)", shas.len()),
                "merged",
            ));
            // Blind to reverts, so it is weighted BELOW ancestry — the badge says so.
            done!(Verdict::Landed {
                sha: shas[0].clone(),
                method: Method::Trailer,
                pr: t.fm.pr,
            })
        }
        Tri::Unknown(u) => {
            let saw = u.badge();
            tr.push(trace(Method::Trailer, &cmd, 128, &saw, "unknown"));
            done!(Verdict::Unknown(u))
        }
        _ => tr.push(trace(Method::Trailer, &cmd, 0, "0 hits", "inconclusive")),
    }

    // ── rung 4: PATCH-ID — rebase/cherry-pick + SINGLE-commit squash only ────
    // DESIGN.md rung 4 is backwards (D-3): recon measured a real 2-commit squash as `+2`,
    // i.e. NOT merged — the exact case the rung was supposed to cover.
    let cmd = format!("git cherry {main} {}", head.short());
    match git.cherry(main, &head) {
        Tri::Yes(lines) if !lines.is_empty() && lines.iter().all(|l| l.upstream) => {
            tr.push(trace(
                Method::PatchId,
                &cmd,
                0,
                &format!("all - ({} patch(es) upstream)", lines.len()),
                "merged",
            ));
            done!(Verdict::Landed {
                sha: head,
                method: Method::PatchId,
                pr: t.fm.pr,
            })
        }
        // We only reach rung 4 with commits main does not have, so an empty `cherry` means
        // the two questions disagree. Conflicting signals are unknown, by policy.
        Tri::Yes(lines) if lines.is_empty() => {
            tr.push(trace(Method::PatchId, &cmd, 0, "no output", "unknown"));
            done!(Verdict::Unknown(Unknown::ConflictingSignals {
                rungs: tr.clone()
            }))
        }
        Tri::Yes(lines) => {
            let plus = lines.iter().filter(|l| !l.upstream).count();
            tr.push(trace(
                Method::PatchId,
                &cmd,
                0,
                &format!("+{plus}"),
                "unknown",
            ));
            // A `+` line CANNOT distinguish an unmerged branch from a multi-commit squash,
            // so it is Unknown — never NotMerged. This is R-4, and it is why
            // `scan --confirm` exists.
            done!(Verdict::Unknown(Unknown::SquashSuspectedNoGh {
                plus_lines: plus
            }))
        }
        Tri::Unknown(u) => {
            let saw = u.badge();
            tr.push(trace(Method::PatchId, &cmd, 128, &saw, "unknown"));
            done!(Verdict::Unknown(u))
        }
        // `cherry` never answers `No` — a `+` line is Unknown by policy (above) — so this
        // arm is the type's exhaustiveness, not a rung: the ladder alone never reaches
        // `NotLanded`, which is R-4 stated as control flow.
        Tri::No => done!(Verdict::NotLanded),
    }
}

/// The `done` gate. **RE-RUNS the ladder** rather than trusting the cache — "a 60s-old
/// merged is not a gate".
///
/// The recorded human override ([`confirmed_proof`]) is consulted only *after* the ladder
/// declines, so a confirmation can never overrule fresh git truth — it can only speak where
/// git has nothing to say.
pub fn proof_for_done(ctx: &Ctx, t: &Ticket) -> Result<MergedProof> {
    let main = ctx.git.resolve_main(&ctx.cfg.main)?;
    let d = ladder(&ctx.git, &ctx.gh, t, &main, ctx.git.fetch_age(), ctx.now);
    if let Some(p) = MergedProof::from_detection(&t.fm.id, &d) {
        return Ok(p);
    }
    if let Some(p) = confirmed_proof(&ctx.git, t) {
        return Ok(p);
    }
    let id = &t.fm.id;
    let why = match d.verdict() {
        Verdict::Unknown(u) => u.badge(),
        _ => format!("nothing on {main} carries this ticket's work"),
    };
    Err(KsError::gate_detail(
        GateCode::NotLanded,
        format!("{id} is not on {main} — {why}"),
        // The trace IS the refusal: pre-formatting it into the message would throw away
        // the `--explain`-grade output that makes the gate arguable rather than arbitrary.
        GateDetail::NotLanded {
            trace: d.explain().to_vec(),
        },
        fixes![
            fix!("kanspec scan --explain {id}"),
            fix!("kanspec scan --confirm {id} --why \"...\""),
            fix!("kanspec done {id} --no-code --why \"...\""),
        ],
    ))
}

pub struct ScanOpts {
    pub fetch: bool,
    pub only: Option<TicketId>,
}

/// Runs the ladder across every non-terminal ticket + spec anchors + branch facts. The
/// ONLY producer of [`ScanToken`]. Runs **outside** the lock (gh/network); the caller then
/// opens a short `transact` to persist via `Op::WriteGitState`.
pub fn scan_all(ctx: &Ctx, snap: &Snapshot, opts: ScanOpts) -> Result<(GitState, ScanToken)> {
    let (state, token, _) = scan_all_detailed(ctx, snap, opts)?;
    Ok((state, token))
}

/// What a scan pass produces: the cache DTO, the capability to write it, and the sealed
/// ladder run behind every row it holds.
pub type ScanOutcome = (GitState, ScanToken, Vec<(TicketId, Detection)>);

/// [`scan_all`] plus the sealed [`Detection`] behind every fact it wrote.
///
/// `scan --explain` renders the rung table of the run that produced the verdict — there is
/// no second ladder run that could disagree with the first — and the cache DTO cannot carry
/// a trace, so the detections come back beside it rather than being re-derived. Additive:
/// [`scan_all`] keeps §2.15's signature exactly, which is what `server.rs` calls.
pub fn scan_all_detailed(ctx: &Ctx, snap: &Snapshot, opts: ScanOpts) -> Result<ScanOutcome> {
    // Everything that touches the network happens HERE, before the caller takes the lock.
    if opts.fetch && ctx.cfg.git.fetch {
        // Best effort by design: offline is not a reason to refuse an answer, it is a
        // reason to stamp the answer with the fetch age (DESIGN.md, merge detection).
        let _ = ctx.git.fetch();
    }
    let main = ctx.git.resolve_main(&ctx.cfg.main)?;
    let fetch_age = ctx.git.fetch_age();

    // A TARGETED scan must not discard the per-ticket facts it did not recompute; a full
    // scan starts clean so a deleted ticket's fact cannot outlive it. ONLY the per-ticket
    // maps carry over: spec anchors and glob rot are recomputed whole below, and keeping the
    // old maps would let a deleted spec's anchor, or a revoked decision's rot row, outlive
    // the record until the next full scan. Facts computed against a different `main` are
    // discarded either way — mixing them would answer "merged into what?" with two
    // different branches. (`cache::load` already discards any other `version`.)
    let mut state = match &opts.only {
        Some(_) if snap.git.main == main => GitState {
            tickets: snap.git.tickets.clone(),
            branches: snap.git.branches.clone(),
            ..GitState::default()
        },
        _ => GitState::default(),
    };
    state.main = main.clone();
    // When `scan` last RAN, which is what `GitState::freshness` claims. A targeted scan
    // moves it too, and that is why every `MergeFact` carries its own `checked_at`: the
    // per-ticket badge is never fresher than the ticket's own ladder run.
    state.scanned_at = Some(ctx.now);
    state.fetch_age_secs = fetch_age.map(|d| d.as_secs());

    let mut detections = Vec::new();
    for t in snap.tickets.values() {
        match &opts.only {
            Some(id) if *id != t.fm.id => continue,
            // A terminal ticket has nowhere left to go: `derive` reads its state, not its
            // merge fact, and re-asking git about it every scan costs four subprocesses.
            None if t.fm.state.terminal() => continue,
            _ => {}
        }

        let d = ladder(&ctx.git, &ctx.gh, t, &main, fetch_age, ctx.now);
        let changed = touched_paths(&ctx.git, t, &main);
        // The human attestation lives in the ticket's `## Log` (D-11), so every scan reads
        // it back and projects it into the cache. THAT is what makes `scan --confirm`
        // survive `rm -rf cache/` — the override is re-derived, never remembered.
        let fact = match (d.verdict(), confirmed_proof(&ctx.git, t)) {
            (Verdict::Landed { .. }, _) => d.to_fact(changed),
            (_, Some(p)) => confirmed_fact(&p, d.checked_at(), changed),
            (_, None) => d.to_fact(changed),
        };
        state.tickets.insert(t.fm.id.clone(), fact);
        if let Some(b) = branch_fact(&ctx.git, t, &main) {
            state.branches.insert(t.fm.id.clone(), b);
        }
        detections.push((t.fm.id.clone(), d));
    }

    // Spec anchors are recomputed WHOLE on every scan, targeted or not: `merges_since` is
    // a result, never an accumulator (D-10). A counter in a disposable cache silently
    // resets to zero on a wipe, which under-fires the tripwire — the dangerous direction.
    for (name, spec) in &snap.specs {
        state
            .specs
            .insert(name.clone(), spec_anchor(ctx, &main, spec));
    }

    // Glob rot for the other two path-scoped records, recorded the same way — only the
    // rotted globs, only for records that are standing. A revoked decision's scope and a
    // fixed quirk's paths steer nobody, so their rot is nobody's finding.
    for d in snap.decisions.values() {
        if d.fm.status != DecisionStatus::Accepted {
            continue;
        }
        let dead = dead_globs(&ctx.git, &d.scope);
        if !dead.is_empty() {
            state.decision_dead_globs.insert(d.fm.id.clone(), dead);
        }
    }
    for q in snap.quirks.values() {
        if q.fm.status != QuirkStatus::Active {
            continue;
        }
        let dead = dead_globs(&ctx.git, &q.fm.paths);
        if !dead.is_empty() {
            state.quirk_dead_globs.insert(q.fm.id.clone(), dead);
        }
    }

    Ok((state, ScanToken(()), detections))
}

/// The globs among `globs` that match no tracked file — ONE definition, so a spec's
/// `code:`, a decision's `scope:` and a quirk's `paths:` rot by the same test.
fn dead_globs(git: &Git, globs: &[String]) -> Vec<String> {
    globs
        .iter()
        .filter(|g| !glob_matches_anything(git, g))
        .cloned()
        .collect()
}

/// How many trailer-matched commits the fallback below will diff. A branch bigger than this
/// is a squash waiting to happen, and the list is a staleness input, not an audit log.
const TRAILER_DIFF_MAX: usize = 20;

/// The paths a ticket's branch changed relative to main — recorded per ticket so `derive`
/// can recompute spec staleness at READ time (D-10). Deletes are excluded: a spec glob
/// matching a path that no longer exists has not been "touched" by it.
///
/// `Git::changed_paths` is a THREE-dot diff (2 dots would leak main's own changes), and
/// three dots collapse to nothing the moment a *true* merge puts the branch's commits on
/// main — `merge-base(main, head)` is then the head itself. The commits are still
/// identifiable by the `Kanspec:` trailer the commit hook writes, so the fallback asks each
/// of them directly. A true merge with no trailers (a repo that never ran `init`) records
/// no paths, which is the honest answer rather than main's whole history.
///
/// **Public because `done` needs exactly this** for `DoneFacts.touched` (§2.16): a second
/// caller reaching for `Git::changed_paths` directly would silently reacquire the three-dot
/// hole this function exists to close.
pub fn touched_paths(git: &Git, t: &Ticket, main: &str) -> Vec<String> {
    let Some((rev, _)) = ticket_rev(t) else {
        return Vec::new();
    };
    let mut out = added_or_modified(git.changed_paths(main, &rev));
    if out.is_empty() {
        if let Tri::Yes(shas) = git.grep_trailer(main, &t.fm.id) {
            for sha in shas.iter().take(TRAILER_DIFF_MAX) {
                // `<sha>^...<sha>` — the merge base of a commit and its parent IS the
                // parent, so three dots and two agree here.
                let parent = format!("{}^", sha.as_str());
                out.extend(added_or_modified(git.changed_paths(&parent, sha.as_str())));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn added_or_modified(paths: Tri<Vec<crate::git::ChangedPath>>) -> Vec<String> {
    match paths {
        Tri::Yes(v) => v
            .into_iter()
            .filter(|c| c.status != 'D')
            .map(|c| c.path)
            .collect(),
        // "Cannot answer" is not "changed nothing", but the cache has no third state for a
        // path list, and every consumer treats an absent path as untouched.
        _ => Vec::new(),
    }
}

/// `head:` if recorded, else the branch — DESIGN.md's "head-or-tip" — as a rev string for
/// the git calls that take one, tagged with where it came from (load-bearing for guard 0b).
/// The ONE definition: `cmd/scan.rs` resolves the SHA a confirmation attests to through it.
pub(crate) fn ticket_rev(t: &Ticket) -> Option<(String, HeadOrigin)> {
    let named = |v: &Option<String>| {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "null")
            .map(str::to_string)
    };
    named(&t.fm.head)
        .map(|h| (h, HeadOrigin::Recorded))
        .or_else(|| named(&t.fm.branch).map(|b| (b, HeadOrigin::BranchTip)))
}

/// The Worktrees tab's row and the STALLED tripwire's input.
fn branch_fact(git: &Git, t: &Ticket, main: &str) -> Option<BranchFact> {
    let branch = t.fm.branch.clone();
    let (rev, _) = ticket_rev(t)?;
    let (ahead, behind) = match git.ahead_behind(main, &rev) {
        Some((a, b)) => (Some(a), Some(b)),
        None => (None, None),
    };
    Some(BranchFact {
        head: git
            .head_sha(&rev)
            .ok()
            .map(|h| h.sha().as_str().to_string()),
        ahead,
        behind,
        last_commit_at: git.last_commit_at(&rev),
        pushed: branch
            .as_deref()
            .is_some_and(|b| git.head_sha(&format!("origin/{b}")).is_ok()),
        branch,
    })
}

/// The spec's last-edit anchor plus the merges that touched its `code:` globs since. Both
/// are git questions, asked fresh every scan.
fn spec_anchor(ctx: &Ctx, main: &str, spec: &Spec) -> SpecAnchor {
    let touch = repo_relative(ctx, &spec.path)
        .and_then(|rel| ctx.git.last_touch(main, &Pathspec::glob(&rel)));
    let globs: Vec<Pathspec> = spec.fm.code.iter().map(|g| Pathspec::glob(g)).collect();
    let merges_since = match &touch {
        // `--first-parent` inside `merges_touching`: the tripwire counts MERGES, not
        // commits, or it over-fires by the size of every PR (D-9).
        Some((sha, _)) if !globs.is_empty() => match ctx.git.merges_touching(sha, main, &globs) {
            Tri::Yes(n) => n,
            _ => 0,
        },
        // No anchor is "we cannot count from anywhere", which is not "0 merges since".
        _ => 0,
    };
    SpecAnchor {
        last_edit_sha: touch.as_ref().map(|(s, _)| s.as_str().to_string()),
        last_edit_at: touch.as_ref().map(|(_, at)| *at),
        merges_since,
        dead_globs: dead_globs(&ctx.git, &spec.fm.code),
    }
}

fn repo_relative(ctx: &Ctx, p: &std::path::Path) -> Option<String> {
    p.strip_prefix(ctx.layout.repo_root())
        .ok()
        .map(|r| r.to_string_lossy().replace('\\', "/"))
}

/// Glob rot: a `code:` glob matching zero tracked files is an amber dot on the feature
/// strip, not a silently-empty staleness count.
fn glob_matches_anything(git: &Git, glob: &str) -> bool {
    match git.run_ps(&["ls-files", "-z"], &[Pathspec::glob(glob)]) {
        Ok(o) => o.code == 0 && !o.out.trim().is_empty(),
        // Cannot tell — say nothing rather than flag a glob that may be fine.
        Err(_) => true,
    }
}

/// The cache projection of a recorded human attestation. Badge-grade, like every other
/// row: the proof-grade value is minted at the gate by [`confirmed_proof`].
fn confirmed_fact(p: &MergedProof, checked_at: DateTime<Utc>, changed: Vec<String>) -> MergeFact {
    MergeFact {
        status: MergeStatus::Merged,
        sha: Some(p.sha().as_str().to_string()),
        method: Method::HumanConfirm,
        pr: p.pr(),
        why: Some(format!(
            "confirmed by hand {}",
            p.checked_at().format("%Y-%m-%dT%H:%MZ")
        )),
        checked_at,
        changed,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The recorded human override
// ─────────────────────────────────────────────────────────────────────────────

pub struct ConfirmFacts {
    pub sha: Option<Sha>,
    pub actor: Actor,
    pub at: DateTime<Utc>,
    pub why: String,
    pub invocation: String,
}

/// The recorded human override. Appends an attributed `Verb::Confirm` line to the
/// TICKET'S `## Log`, not a cache entry: a human attestation is an ASSERTED ACT WITH AN
/// ACTOR, so it must survive `rm -rf cache/` and be visibly signed (D-11).
///
/// PURE — every git call that produced `f.sha` happened before the lock.
pub fn plan_confirm(snap: &Snapshot, f: &ConfirmFacts, id: &TicketId) -> Result<Plan> {
    let t = snap.ticket(id)?;
    let why = f.why.trim();
    if why.is_empty() {
        return Err(KsError::gate(
            GateCode::ConfirmWithoutWhy,
            format!("`{id}` cannot be confirmed in main without a recorded reason"),
            fixes![
                fix!("kanspec scan --confirm {id} --why \"squash merged by hand, verified\""),
                fix!("kanspec scan --explain {id}"),
            ],
        ));
    }
    // A confirmation that names no commit is unreadable later: `confirmed_proof` needs a
    // SHA to hand the gate, and "trust me" is exactly what this tool refuses.
    let sha = f
        .sha
        .as_ref()
        .map(|s| s.as_str().to_string())
        .or_else(|| t.fm.head.clone())
        .ok_or_else(|| {
            KsError::gate(
                GateCode::ConfirmWithoutHead,
                format!("`{id}` records neither a `head:` SHA nor a resolvable branch to confirm"),
                fixes![fix!("kanspec ship {id}"), fix!("kanspec show {id}"),],
            )
        })?;
    Ok(Plan::of(vec![Op::Transition {
        id: id.clone(),
        verb: Verb::Confirm,
        actor: f.actor.clone(),
        at: f.at,
        // The note IS the storage — see `CONFIRM_NOTE`.
        detail: format!("{CONFIRM_NOTE} {sha} — {why}"),
        also: Vec::new(),
    }]))
}

/// Reads a recorded confirmation back out of the log — the ONLY non-ladder route to a
/// [`MergedProof`].
///
/// NOTE (deviation from ARCHITECTURE.md §2.15, reported): the contract's signature is
/// `confirmed_proof(t: &Ticket)`. It cannot be honoured as written — [`Sha`]'s only
/// constructor is private to `git.rs`, so a function with no `&Git` cannot produce the
/// `sha` field a `MergedProof` requires. Taking `&Git` is also the stronger seal: the
/// attested commit is re-resolved through git, so an attestation naming a commit this repo
/// does not have yields no proof at all.
pub fn confirmed_proof(git: &Git, t: &Ticket) -> Option<MergedProof> {
    let entry = t.log.iter().rev().find(|e| {
        e.verb == Verb::Confirm
            && e.note
                .as_deref()
                .is_some_and(|n| n.trim().starts_with(CONFIRM_NOTE))
    })?;
    let rev = note_sha(entry.note.as_deref()?)
        .map(str::to_string)
        .or_else(|| t.fm.head.clone())?;
    let sha = git.head_sha(&rev).ok()?.sha().clone();
    Some(MergedProof {
        ticket: t.fm.id.clone(),
        sha,
        method: Method::HumanConfirm,
        pr: t.fm.pr,
        // The moment a human looked, not the moment we read the line back.
        checked_at: entry.at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::gh::GhCfg;
    use crate::logentry::LogEntry;
    use crate::transitions::State;

    fn tid(s: &str) -> TicketId {
        TicketId::parse(s).unwrap()
    }

    /// `Plan` is deliberately not `Debug` (it holds a `ScanToken`), so `unwrap_err` is
    /// unavailable on a planner's result.
    #[track_caller]
    fn refusal(r: Result<Plan>) -> &'static str {
        match r {
            Ok(p) => panic!("expected a refusal, got a plan with {} ops", p.ops.len()),
            Err(e) => e.code().unwrap_or(e.kind()),
        }
    }

    fn at() -> DateTime<Utc> {
        "2026-08-31T12:00:00Z".parse().unwrap()
    }

    fn detection(v: Verdict) -> Detection {
        Detection::seal(v, at(), Some(11), vec![])
    }

    fn ticket(id: &str) -> Ticket {
        let fm: crate::model::TicketFm = serde_yaml_ng::from_str(&format!(
            "id: {id}\ntitle: t\nstate: review\ncreated: 2026-08-30T09:00:00Z\n"
        ))
        .unwrap();
        Ticket {
            fm,
            path: std::path::PathBuf::from(format!(".kanspec/tickets/{id}.md")),
            body: String::new(),
            steps: Vec::new(),
            log: Vec::new(),
            mtime: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    // ── the down-conversion is one-way, and lossy on purpose ─────────────────

    #[test]
    fn an_unknown_verdict_becomes_a_cache_row_that_explains_itself() {
        let d = detection(Verdict::Unknown(Unknown::SquashSuspectedNoGh {
            plus_lines: 2,
        }));
        let f = d.to_fact(vec!["src/auth/login.ts".into()]);
        assert_eq!(f.status, MergeStatus::Unknown);
        assert_eq!(f.why.as_deref(), Some("unknown (squash suspected, no gh)"));
        assert_eq!(
            f.method,
            Method::None,
            "no rung concluded, so none is named"
        );
        assert!(f.sha.is_none());
        assert_eq!(f.changed, ["src/auth/login.ts"]);
        assert_eq!(f.checked_at, at());
    }

    #[test]
    fn a_not_landed_verdict_never_carries_a_method_or_a_sha() {
        let f = detection(Verdict::NotLanded).to_fact(vec![]);
        assert_eq!(f.status, MergeStatus::NotMerged);
        assert_eq!(f.method, Method::None);
        assert!(f.sha.is_none() && f.why.is_none());
    }

    /// The proof is minted from a `Detection` and nothing else — the whole of invariant 1's
    /// compile-time half rests on there being no other route.
    #[test]
    fn only_a_landed_detection_mints_a_proof() {
        let id = tid("t-9c41");
        for v in [
            Verdict::NotLanded,
            Verdict::Unknown(Unknown::ZeroCommitBranch),
            Verdict::Unknown(Unknown::GhUnavailable {
                why: "no gh".into(),
            }),
        ] {
            assert!(
                MergedProof::from_detection(&id, &detection(v)).is_none(),
                "an inconclusive ladder must mint nothing"
            );
        }
    }

    // ── the confirmation round-trip: the note IS the storage ─────────────────

    #[test]
    fn a_confirmation_round_trips_through_the_log_line_it_writes() {
        let mut snap = Snapshot::empty(Config::default(), at());
        let id = tid("t-dddd");
        snap.tickets.insert(id.clone(), ticket("t-dddd"));
        let f = ConfirmFacts {
            sha: None,
            actor: Actor::Human {
                name: "trevor".into(),
            },
            at: at(),
            why: "  ".into(),
            invocation: "kanspec scan --confirm t-dddd".into(),
        };
        // An unexplained attestation is refused, and so is one that names no commit.
        assert_eq!(refusal(plan_confirm(&snap, &f, &id)), "confirm_without_why");
        let f = ConfirmFacts {
            why: "github squash, verified by hand".into(),
            ..f
        };
        assert_eq!(
            refusal(plan_confirm(&snap, &f, &id)),
            "confirm_without_head"
        );

        // With a SHA, the plan is exactly one recorded, attributed non-transition.
        snap.tickets.get_mut(&id).unwrap().fm.head = Some("a1b9c3d5f00".into());
        let plan = plan_confirm(&snap, &f, &id).unwrap();
        let Some(Op::Transition {
            verb, detail, at, ..
        }) = plan.ops.first()
        else {
            panic!("expected one Transition, got {} ops", plan.ops.len());
        };
        assert_eq!(*verb, Verb::Confirm);
        assert_eq!(plan.ops.len(), 1);

        // And the line it produces is parseable back into the SHA the human attested to.
        let line = LogEntry {
            at: *at,
            state: State::Review,
            actor: "trevor".into(),
            verb: Verb::Confirm,
            note: Some(detail.clone()),
        }
        .format();
        let back = LogEntry::parse(&line).expect("the confirm line is a legal log entry");
        assert_eq!(
            note_sha(back.note.as_deref().unwrap()),
            Some("a1b9c3d5f00"),
            "the attested SHA must survive the log grammar: {line}"
        );
        assert!(back
            .note
            .unwrap()
            .contains("github squash, verified by hand"));
    }

    // ── rung 2's silence has three different reasons ─────────────────────────

    /// A real repo whose branch is genuinely not on `main`, so the ladder falls past rung 1
    /// and actually reaches rung 2. `origin` is a host `gh` cannot speak for, so nothing
    /// here depends on whether THIS machine happens to be logged into GitHub.
    fn repo_off_main(dir: &std::path::Path) -> (crate::git::Git, Ticket) {
        let git = crate::git::Git::bind(dir);
        let sh = |args: &[&str]| {
            let out = git.run(args).expect("git runs");
            assert_eq!(out.code, 0, "git {args:?}: {}", out.err);
            out.out
        };
        sh(&["init", "--quiet", "--initial-branch=main", "."]);
        for (k, v) in [
            ("user.email", "t@kanspec.invalid"),
            ("user.name", "t"),
            ("commit.gpgsign", "false"),
        ] {
            sh(&["config", k, v]);
        }
        sh(&["remote", "add", "origin", "https://gitlab.com/acme/x.git"]);
        sh(&["commit", "--quiet", "--allow-empty", "-m", "base"]);
        sh(&["checkout", "--quiet", "-b", "ks/t-9c41-thing"]);
        sh(&["commit", "--quiet", "--allow-empty", "-m", "work"]);

        let mut t = ticket("t-9c41");
        t.fm.branch = Some("ks/t-9c41-thing".into());
        t.fm.head = Some(sh(&["rev-parse", "HEAD"]).trim().to_string());
        (git, t)
    }

    /// `--explain` is the surface a user reads *precisely* when the ladder has already
    /// disappointed them, so it is the worst possible place to guess. Rung 2 used to
    /// report `gh auth status / exit 1 / unavailable` for all three of `gh`'s refusals: a
    /// user on `gh = "never"` or a GitLab origin would run `gh auth status`, watch it exit
    /// 0, and conclude the tool was lying about the rest too.
    #[test]
    fn a_declining_gh_reports_its_real_reason_and_names_no_command_it_did_not_run() {
        assert!(
            std::env::var_os("KANSPEC_GH_FIXTURES").is_none(),
            "this test asserts the un-mocked gate; unset KANSPEC_GH_FIXTURES to run it"
        );
        let dir = tempfile::tempdir().expect("a temp dir");
        let (git, t) = repo_off_main(dir.path());

        // Read off the REAL ladder, not off the helper: the rung is what has to tell the
        // truth, and wiring it back to a hardcoded string must fail here.
        let rung2 = |gh: &Gh| {
            let d = ladder(&git, gh, &t, "main", None, at());
            d.explain()
                .iter()
                .find(|r| r.method == Method::GhPr)
                .unwrap_or_else(|| panic!("the ladder never reached rung 2: {:?}", d.explain()))
                .clone()
        };

        // 1. the POLICY. Nothing was asked of `gh`, and nothing was asked about auth.
        let never = Gh::detect(&git, &GhCfg::Never);
        assert!(!never.available());
        let policy = rung2(&never);
        assert!(policy.saw.contains(r#"gh = "never""#), "{}", policy.saw);

        // 2. the REMOTE. Also not an auth problem, and also not the same sentence.
        let elsewhere = Gh::detect(&git, &GhCfg::Auto);
        assert!(!elsewhere.available());
        let remote = rung2(&elsewhere);
        assert!(remote.saw.contains("not a GitHub remote"), "{}", remote.saw);
        assert_ne!(
            policy.saw, remote.saw,
            "three refusals collapsed back into one message"
        );

        for tr in [&policy, &remote] {
            // Invariant 2: a `gh` that would not answer is never a NO.
            assert_eq!(tr.verdict, "inconclusive");
            assert_eq!(
                tr.cmd, NOT_QUERIED,
                "the audit trail may not name a command that was not run"
            );
            assert!(
                !tr.cmd.contains("auth status") && !tr.saw.contains("unavailable"),
                "the hardcoded auth guess is back: {tr:?}"
            );
        }

        // The other arm of the query — no PR number — explains itself the same way, and a
        // ticket with neither a PR nor a branch still gets a reason rather than a shrug.
        let mut with_pr = t.clone();
        with_pr.fm.pr = Some(142);
        assert_eq!(gh_declined(&never, &with_pr).saw, policy.saw);
        assert_eq!(gh_declined(&never, &ticket("t-dddd")).saw, policy.saw);
    }

    #[test]
    fn a_note_that_is_not_an_attestation_yields_no_sha() {
        assert_eq!(note_sha("in main a1b9c3d — why"), Some("a1b9c3d"));
        assert_eq!(note_sha("in main — why"), None, "no SHA named");
        assert_eq!(note_sha("in main NOTHEX0 — why"), None);
        assert_eq!(note_sha("in main a1b9c3 — too short"), None);
        assert_eq!(note_sha("rework, see #12"), None);
    }

    // ── the no-code waiver ───────────────────────────────────────────────────

    #[test]
    fn a_no_code_waiver_refuses_an_empty_why_and_records_a_durable_line() {
        let mut plan = Plan::empty();
        let id = tid("t-9c41");
        let by = Actor::Human {
            name: "trevor".into(),
        };
        assert_eq!(
            NoCodeWaiver::record(&mut plan, &id, "   ", &by, at())
                .unwrap_err()
                .code(),
            Some("no_code_without_why")
        );
        assert!(plan.is_empty(), "a refused waiver plans nothing");

        let w = NoCodeWaiver::record(&mut plan, &id, " docs only ", &by, at()).unwrap();
        assert_eq!(w.why(), "docs only");
        assert_eq!(w.by(), "trevor");
        let Some(Op::AppendSection { heading, line, .. }) = plan.ops.first() else {
            panic!("the waiver must be durable before it is usable");
        };
        assert_eq!(*heading, LOG_HEADING);
        assert!(
            line.contains("docs only") && line.contains("trevor"),
            "{line}"
        );
        // Durable, but NOT a second transition: `replay` must not see it as an entry.
        assert!(
            crate::logentry::parse_log(&format!("## Log\n{line}\n")).is_empty(),
            "the waiver line must not parse as a log entry: {line}"
        );
    }
}
