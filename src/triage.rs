//! `Triage` — one typed value, two front doors.
//!
//! The interactive prompts and the `--json` flags BOTH construct this, and only this
//! reaches `plan_done`. The agent path and the human path therefore cannot diverge in what
//! they record.
//!
//! The shape of that guarantee is worth stating, because it is easy to build the same type
//! two ways and still diverge: [`Triage::from_args`] and [`Triage::prompt`] each *collect*
//! answers in their own idiom, then funnel through one private `assemble`, which owns every
//! rule — no fourth option for a step, a drop reason that is actually a reason, a quirk that
//! actually names paths. What differs between the two doors is only the two rules that are
//! *about* non-interactivity: an agent must pass its "nothing left" and "no quirks" claims
//! EXPLICITLY, because an omission is not a claim, whereas a human answering a prompt has
//! already made one.
//!
//! Owner: **S5**.

use std::io::{BufRead, Write};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Serialize;

use crate::cli::DoneArgs;
use crate::error::{KsError, Result};
use crate::git::ChangedPath;
use crate::ids::{SpecName, TicketId};
use crate::model::{Severity, Snapshot, Step, Ticket};
use crate::{fix, fixes};

/// Where a spec lives, relative to the repo root — the form `git diff --name-status`
/// prints. `KanspecDir` is always `<primary_root>/.kanspec` (it has no `[paths]` knob), so
/// this is a fact about the layout rather than a guess about the config.
fn spec_rel_path(name: &SpecName) -> String {
    format!(".kanspec/specs/{}.md", name.as_str())
}

#[derive(Debug, Clone, Serialize)]
pub struct Triage {
    pub steps: Vec<StepDisposition>,
    pub quirks: Vec<NewQuirk>,
    pub decisions: Vec<NewDecision>,
    pub spec: SpecCheck,
}

/// No fourth option: every unchecked step is spawned, dropped with a reason, or marked
/// actually done.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum StepDisposition {
    Spawn { index: usize, title: String },
    Drop { index: usize, why: String },
    ActuallyDone { index: usize },
}

impl StepDisposition {
    pub fn index(&self) -> usize {
        match self {
            StepDisposition::Spawn { index, .. }
            | StepDisposition::Drop { index, .. }
            | StepDisposition::ActuallyDone { index } => *index,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "spec_check", rename_all = "snake_case")]
pub enum SpecCheck {
    EditedOnBranch {
        specs: Vec<SpecName>,
    },
    /// the recorded `--spec-unchanged` waivers, one per spec, visible on the board
    Unchanged {
        waivers: Vec<SpecWaiver>,
    },
    /// the branch touched no spec's globs
    NotApplicable,
}

/// One `--spec-unchanged "<spec>:<reason>"`. The spec is `None` only for a reason offered
/// where nothing demanded one — recorded anyway, because an explicit "no behaviour change"
/// is never worse than silence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpecWaiver {
    pub spec: Option<SpecName>,
    pub why: String,
}

impl std::fmt::Display for SpecWaiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.spec {
            Some(s) => write!(f, "{s}: {}", self.why),
            None => write!(f, "{}", self.why),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NewQuirk {
    pub title: String,
    pub paths: Vec<String>,
    pub severity: Severity,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewDecision {
    pub title: String,
    pub scope: Vec<String>,
}

/// The answers ALREADY supplied, re-rendered as flags, with a leading space when non-empty.
///
/// Every `done` gate suggests the flag that answers IT. Suggesting that flag alone is a
/// command that discards every answer already given, and the gates are checked in order, so
/// the advice rang:
///
/// ```text
/// $ kanspec done t-9c41                   ✗ followups_unanswered → done t-9c41 --no-followups
/// $ kanspec done t-9c41 --no-followups    ✗ quirks_unanswered    → done t-9c41 --no-quirks
/// $ kanspec done t-9c41 --no-quirks       ✗ followups_unanswered → done t-9c41 --no-followups
/// ```
///
/// A human sees the loop on the third line; an agent whose contract is "run the suggested
/// command" does not, and this is the ORDINARY close-out, not an edge case. So every
/// suggestion below is built as *what you already said* + *the answer this gate wants*.
///
/// `adds` names the flags the suggestion is about to append, because three pairs are
/// declared `conflicts_with` in `cli.rs` — `--spawn`/`--no-followups`,
/// `--quirk`/`--no-quirks`, `--decision`/`--no-decisions`. Carrying the negative into a
/// suggestion that supplies the positive would emit a command clap REFUSES, which is a
/// worse failure than the ring: it does not even parse.
fn carried(a: &DoneArgs, adds: &[&str]) -> String {
    // Symmetric: naming either half suppresses the other.
    const OPPOSED: &[(&str, &str)] = &[
        ("--spawn", "--no-followups"),
        ("--quirk", "--no-quirks"),
        ("--decision", "--no-decisions"),
    ];
    let blocked = |flag: &str| {
        adds.iter().any(|add| {
            OPPOSED
                .iter()
                .any(|(x, y)| (*x == flag && *y == *add) || (*y == flag && *x == *add))
        })
    };

    let mut f: Vec<String> = Vec::new();
    for (name, on) in [
        ("--no-followups", a.no_followups),
        ("--no-quirks", a.no_quirks),
        ("--no-decisions", a.no_decisions),
        ("--no-code", a.no_code),
    ] {
        if on && !blocked(name) {
            f.push(name.to_string());
        }
    }
    for (name, vals) in [
        ("--spawn", &a.spawn),
        ("--drop-step", &a.drop_step),
        ("--quirk", &a.quirk),
        ("--quirk-paths", &a.quirk_paths),
        ("--decision", &a.decision),
    ] {
        if blocked(name) {
            continue;
        }
        for v in vals {
            f.push(format!("{name} \"{v}\""));
        }
    }
    for n in &a.actually_done {
        f.push(format!("--actually-done {n}"));
    }
    // A blanket `--spec-unchanged` being replaced by per-spec ones must not ride along.
    if !adds.contains(&"--spec-unchanged") {
        for v in &a.spec_unchanged {
            f.push(format!("--spec-unchanged \"{v}\""));
        }
    }
    if let Some(v) = &a.why {
        f.push(format!("--why \"{v}\""));
    }
    if f.is_empty() {
        String::new()
    } else {
        format!(" {}", f.join(" "))
    }
}

impl Triage {
    /// Non-interactive. REFUSES with a typed error naming BOTH flags when neither
    /// `--spawn` nor `--no-followups` is present: clap cannot express "required iff
    /// --json" (`required_if_eq` works on values, and `global + required` is the
    /// debug-only panic recon found), and the handler yields a better message anyway.
    pub fn from_args(
        t: &Ticket,
        a: &DoneArgs,
        touched: &[ChangedPath],
        s: &Snapshot,
    ) -> Result<Triage> {
        let id = &t.fm.id;

        // ── the two claims an omission cannot make ───────────────────────────
        //
        // These fire even when there is nothing left to say. "No leftover work" and "no
        // quirks" are ASSERTIONS an agent makes and the ticket records; absence of a flag
        // is silence, and silence is exactly the failure mode `done` exists to close.
        if a.spawn.is_empty() && !a.no_followups {
            return Err(KsError::gate(
                "followups_unanswered",
                format!(
                    "{id}: leftover work is a recorded claim — say whether any remains, \
                     even if none does"
                ),
                fixes![
                    fix!(
                        "kanspec done {id}{} --no-followups",
                        carried(a, &["--no-followups"])
                    ),
                    fix!(
                        "kanspec done {id}{} --spawn \"what is left\"",
                        carried(a, &["--spawn"])
                    ),
                    fix!("kanspec show {id}"),
                ],
            ));
        }
        if a.quirk.is_empty() && !a.no_quirks {
            return Err(KsError::gate(
                "quirks_unanswered",
                format!(
                    "{id}: a landmine is cheapest to record while the burn is fresh — say \
                     whether you hit one"
                ),
                fixes![
                    fix!(
                        "kanspec done {id}{} --no-quirks",
                        carried(a, &["--no-quirks"])
                    ),
                    fix!(
                        "kanspec done {id}{} --quirk \"what bites\" --quirk-paths \"src/**\"",
                        carried(a, &["--quirk"])
                    ),
                ],
            ));
        }

        let steps = steps_from_args(t, a)?;
        let quirks = quirks_from_args(t, a)?;
        let decisions = decisions_from_args(a, touched, s);
        let spec = spec_check(t, &a.spec_unchanged, touched, s, a)?;
        assemble(t, steps, quirks, decisions, spec, a)
    }

    /// Interactive. Same value, prompted one key at a time.
    ///
    /// Prompts go to **stderr** and answers come from stdin, so stdout stays exactly what
    /// `out::emit` renders — a handler still never prints its report, and
    /// `kanspec done t-9c41 | jq` is not corrupted by a question.
    pub fn prompt(
        t: &Ticket,
        a: &DoneArgs,
        touched: &[ChangedPath],
        s: &Snapshot,
    ) -> Result<Triage> {
        let mut io = Prompter::stdio();
        Triage::prompt_with(&mut io, t, a, touched, s)
    }

    /// The prompt loop over an injected reader/writer, so the transcript is unit-testable
    /// without a terminal.
    pub(crate) fn prompt_with(
        io: &mut Prompter<'_>,
        t: &Ticket,
        a: &DoneArgs,
        touched: &[ChangedPath],
        s: &Snapshot,
    ) -> Result<Triage> {
        // Flags still count in interactive mode: an answer already given is not asked for
        // again. Everything below only fills what the flags left open.
        let mut steps = steps_from_args(t, a)?;
        let answered: Vec<usize> = steps.iter().map(StepDisposition::index).collect();
        let open: Vec<Step> = unchecked(t)
            .into_iter()
            .filter(|st| !answered.contains(&st.index))
            .cloned()
            .collect();

        if !open.is_empty() {
            io.say(&format!(
                "Leftover triage — {} unchecked step{}:",
                open.len(),
                if open.len() == 1 { "" } else { "s" }
            ))?;
        }
        for st in &open {
            io.say(&format!("  [ ] \"{}\"", st.text))?;
            steps.push(ask_step(io, st)?);
        }

        // ── the knowledge checkpoint ─────────────────────────────────────────
        // Flags answer first; a flag that is itself wrong (a blanket reason over several
        // specs, a spec the branch never touched) is refused here exactly as it would be
        // non-interactively. Then one question per spec still uncovered — never one answer
        // for all of them (t-852e).
        let st = spec_state(touched, s);
        let mut waivers = parse_waivers(&t.fm.id, &a.spec_unchanged, &st, touched, s, a)?;
        let remaining = st.remaining(&waivers);
        if remaining.is_empty() {
            if let SpecCheck::EditedOnBranch { specs } = st.resolve(&waivers) {
                io.say(&format!(
                    "Knowledge check — branch touched {} (spec: {}): spec edited on this branch ✓",
                    globs_of_specs(&st.matched, s).join(", "),
                    names(&specs)
                ))?;
            }
        }
        for name in &remaining {
            io.say(&format!(
                "Knowledge check — branch touched {} (spec: {name}): spec NOT edited on this branch",
                where_matched(name, touched, s)
            ))?;
            // Empty stays a refusal — a waiver with no reason is the "fourth option" this
            // gate exists to remove.
            let why = io.ask(&format!("  {name} unchanged — why? "))?;
            if why.trim().is_empty() {
                return Err(uncovered(&t.fm.id, &st, &waivers, touched, s, a));
            }
            waivers.push(SpecWaiver {
                spec: Some(name.clone()),
                why: why.trim().to_string(),
            });
        }
        let spec = st.resolve(&waivers);

        // ── one-key capture, while the burn is fresh ─────────────────────────
        let default = default_scope(touched, s);
        let mut quirks = quirks_from_args(t, a)?;
        if quirks.is_empty() && !a.no_quirks {
            if let Some((title, paths)) =
                ask_capture(io, "Quirks discovered? [enter = none]: ", "paths", &default)?
            {
                // `quirk add`'s own default grade — see `quirks_from_args`.
                quirks.push(NewQuirk {
                    title,
                    paths,
                    severity: Severity::Gotcha,
                });
            }
        }

        let mut decisions = decisions_from_args(a, touched, s);
        if decisions.is_empty() && !a.no_decisions {
            if let Some((title, scope)) =
                ask_capture(io, "Decisions made? [enter = none]: ", "scope", &default)?
            {
                decisions.push(NewDecision { title, scope });
            }
        }

        assemble(t, steps, quirks, decisions, spec, a)
    }

    /// The followup titles this triage spawns, in step order — what `plan_done` mints.
    pub fn spawns(&self) -> Vec<(usize, &str)> {
        self.steps
            .iter()
            .filter_map(|d| match d {
                StepDisposition::Spawn { index, title } => Some((*index, title.as_str())),
                _ => None,
            })
            .collect()
    }

    /// The step indices to tick, in ascending order.
    pub fn actually_done(&self) -> Vec<usize> {
        let mut out: Vec<usize> = self
            .steps
            .iter()
            .filter_map(|d| match d {
                StepDisposition::ActuallyDone { index } => Some(*index),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// The dropped steps and their recorded reasons.
    pub fn dropped(&self) -> Vec<(usize, &str)> {
        self.steps
            .iter()
            .filter_map(|d| match d {
                StepDisposition::Drop { index, why } => Some((*index, why.as_str())),
                _ => None,
            })
            .collect()
    }

    /// The `--spec-unchanged` waivers this triage recorded, as the one line the ticket
    /// carries: `auth: refactor only · billing: no behaviour change`.
    pub fn spec_unchanged(&self) -> Option<String> {
        match &self.spec {
            SpecCheck::Unchanged { waivers } if !waivers.is_empty() => Some(
                waivers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" · "),
            ),
            _ => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The one place every rule lives
// ─────────────────────────────────────────────────────────────────────────────

/// The shared validator both front doors funnel through. Every rule that is *about the
/// triage itself* — rather than about non-interactivity — is here, exactly once.
fn assemble(
    t: &Ticket,
    steps: Vec<StepDisposition>,
    quirks: Vec<NewQuirk>,
    decisions: Vec<NewDecision>,
    spec: SpecCheck,
    // The answers already supplied. These gates all fire AFTER the followups and quirks
    // gates have passed, so a suggestion that named only its own flag would discard the two
    // answers that got the caller this far and ring straight back. See [`carried`].
    a: &DoneArgs,
) -> Result<Triage> {
    let id = &t.fm.id;
    let open = unchecked(t);

    // Every disposition names a step that exists AND is actually unchecked, once.
    let mut seen: Vec<usize> = Vec::new();
    for d in &steps {
        let i = d.index();
        if !open.iter().any(|st| st.index == i) {
            return Err(KsError::invalid(
                format!(
                    "{id} has no unchecked step {i} — steps are numbered from 1, and {} \
                     {} unchecked",
                    open.len(),
                    if open.len() == 1 { "is" } else { "are" }
                ),
                fixes![fix!("kanspec show {id}")],
            ));
        }
        if seen.contains(&i) {
            return Err(KsError::conflict(
                format!("{id}: step {i} is dispositioned twice — it has one outcome"),
                fixes![fix!("kanspec show {id}")],
            ));
        }
        seen.push(i);
    }

    // NO FOURTH OPTION. A step nobody spoke for is the whole failure mode.
    let orphans: Vec<&Step> = open
        .iter()
        .copied()
        .filter(|st| !seen.contains(&st.index))
        .collect();
    if !orphans.is_empty() {
        let listed = orphans
            .iter()
            .map(|st| format!("[{}] \"{}\"", st.index, st.text))
            .collect::<Vec<_>>()
            .join(", ");
        let first = orphans[0].index;
        return Err(KsError::gate(
            "steps_undispositioned",
            format!(
                "{id}: {} unchecked step{} with no outcome — {listed}",
                orphans.len(),
                if orphans.len() == 1 { "" } else { "s" }
            ),
            fixes![
                fix!(
                    "kanspec done {id}{} --spawn \"what is left\"",
                    carried(a, &["--spawn"])
                ),
                fix!(
                    "kanspec done {id}{} --drop-step \"{first}:superseded\"",
                    carried(a, &["--drop-step"])
                ),
                fix!(
                    "kanspec done {id}{} --actually-done {first}",
                    carried(a, &["--actually-done"])
                ),
            ],
        ));
    }

    for d in &steps {
        match d {
            StepDisposition::Spawn { title, .. } if title.trim().is_empty() => {
                return Err(KsError::invalid(
                    format!("{id}: a spawned followup needs a title"),
                    fixes![fix!(
                        "kanspec done {id}{} --spawn \"what is left\"",
                        carried(a, &["--spawn"])
                    )],
                ))
            }
            StepDisposition::Drop { index, why } if why.trim().is_empty() => {
                return Err(KsError::gate(
                    "step_dropped_without_reason",
                    format!("{id}: step {index} was dropped without a reason"),
                    fixes![fix!(
                        "kanspec done {id}{} --drop-step \"{index}:why it will never be done\"",
                        carried(a, &["--drop-step"])
                    )],
                ))
            }
            _ => {}
        }
    }

    for q in &quirks {
        if q.title.trim().is_empty() {
            return Err(KsError::invalid(
                format!("{id}: a quirk needs a title"),
                fixes![fix!(
                    "kanspec done {id}{} --quirk \"what bites\"",
                    carried(a, &["--quirk"])
                )],
            ));
        }
        // A quirk with no paths never fires: the PostToolUse hook and `prime` both reach
        // it by path match, so pathless is the same as absent, only harder to notice.
        if q.paths.is_empty() {
            return Err(KsError::gate(
                "quirk_without_paths",
                format!(
                    "{id}: quirk \"{}\" names no paths — a quirk nothing matches never warns \
                     anyone",
                    q.title
                ),
                fixes![fix!(
                    "kanspec done {id}{} --quirk \"{}\" --quirk-paths \"src/**\"",
                    carried(a, &["--quirk"]),
                    q.title
                )],
            ));
        }
    }

    for d in &decisions {
        if d.title.trim().is_empty() {
            return Err(KsError::invalid(
                format!("{id}: a decision needs a title"),
                fixes![fix!(
                    "kanspec done {id}{} --decision \"what was decided\"",
                    carried(a, &["--decision"])
                )],
            ));
        }
    }

    let mut steps = steps;
    steps.sort_by_key(StepDisposition::index);
    Ok(Triage {
        steps,
        quirks,
        decisions,
        spec,
    })
}

fn unchecked(t: &Ticket) -> Vec<&Step> {
    t.steps.iter().filter(|s| !s.done).collect()
}

/// `--drop-step "3:superseded"` / `--actually-done 3` / `--spawn "…"`.
///
/// The two indexed flags claim their steps by number; `--spawn` titles are then paired with
/// what is LEFT, in step order, which is what makes the transcript's one-key `s` and the
/// agent's `--spawn "…"` describe the same act.
fn steps_from_args(t: &Ticket, a: &DoneArgs) -> Result<Vec<StepDisposition>> {
    let id = &t.fm.id;
    let mut out: Vec<StepDisposition> = Vec::new();

    for raw in &a.drop_step {
        let (n, why) = raw.split_once(':').ok_or_else(|| {
            KsError::invalid(
                format!("--drop-step wants `N:REASON`, got `{raw}`"),
                fixes![fix!(
                    "kanspec done {id} --drop-step \"1:superseded by t-9d02\""
                )],
            )
        })?;
        let index: usize = n.trim().parse().map_err(|_| {
            KsError::invalid(
                format!("--drop-step wants a step NUMBER before the colon, got `{n}`"),
                fixes![fix!("kanspec show {id}")],
            )
        })?;
        out.push(StepDisposition::Drop {
            index,
            why: why.trim().to_string(),
        });
    }
    for index in &a.actually_done {
        out.push(StepDisposition::ActuallyDone { index: *index });
    }

    let claimed: Vec<usize> = out.iter().map(StepDisposition::index).collect();
    let mut open = unchecked(t)
        .into_iter()
        .filter(|st| !claimed.contains(&st.index));
    for title in &a.spawn {
        let Some(st) = open.next() else {
            // A spawn with no step behind it has no index to carry, and inventing one
            // would make the ticket's own `## Steps` disagree with its followups. Work
            // that was never a step is `kanspec new`'s job.
            return Err(KsError::gate(
                "spawn_without_step",
                format!(
                    "{id}: `--spawn \"{title}\"` has no unchecked step left to attach to — \
                     leftover triage spawns from the ticket's own steps"
                ),
                fixes![
                    fix!("kanspec new \"{title}\" --followup-of {id}"),
                    fix!("kanspec show {id}"),
                ],
            ));
        };
        out.push(StepDisposition::Spawn {
            index: st.index,
            title: title.trim().to_string(),
        });
    }
    Ok(out)
}

/// `--quirk "…"` × N with `--quirk-paths` globs. One quirk takes every glob; N quirks take
/// one glob each, positionally.
fn quirks_from_args(t: &Ticket, a: &DoneArgs) -> Result<Vec<NewQuirk>> {
    let id = &t.fm.id;
    if a.quirk.is_empty() {
        return Ok(Vec::new());
    }
    if a.quirk.len() > 1 && a.quirk_paths.len() != a.quirk.len() {
        return Err(KsError::invalid(
            format!(
                "{id}: {} quirks but {} --quirk-paths — with more than one quirk each needs \
                 its own paths, in order",
                a.quirk.len(),
                a.quirk_paths.len()
            ),
            fixes![fix!(
                "kanspec done {id} --quirk \"a\" --quirk-paths \"src/a/**\" --quirk \"b\" \
                 --quirk-paths \"src/b/**\""
            )],
        ));
    }
    Ok(a.quirk
        .iter()
        .enumerate()
        .map(|(i, title)| NewQuirk {
            title: title.trim().to_string(),
            paths: if a.quirk.len() == 1 {
                a.quirk_paths.clone()
            } else {
                vec![a.quirk_paths[i].clone()]
            },
            // `quirk add`'s own default. `done` has no `--sev` flag, and the CLI tree is
            // frozen — a landmine captured here is re-graded with `kanspec quirk`.
            severity: Severity::Gotcha,
        })
        .collect())
}

/// `--decision "…"` × N, each steering the specs this branch actually touched.
fn decisions_from_args(a: &DoneArgs, touched: &[ChangedPath], s: &Snapshot) -> Vec<NewDecision> {
    a.decision
        .iter()
        .map(|title| NewDecision {
            title: title.trim().to_string(),
            scope: default_scope(touched, s),
        })
        .collect()
}

/// DESIGN's anti-rot gear 2: the branch touched a spec's `code:` globs, so either the spec
/// moved with it or somebody said out loud why it did not — per spec, so a real change in
/// one of five cannot hide behind one blanket answer (t-852e).
fn spec_check(
    t: &Ticket,
    raw: &[String],
    touched: &[ChangedPath],
    s: &Snapshot,
    // The answers already supplied — this gate is reached last of all. See [`carried`].
    a: &DoneArgs,
) -> Result<SpecCheck> {
    let id = &t.fm.id;
    let st = spec_state(touched, s);
    let waivers = parse_waivers(id, raw, &st, touched, s, a)?;
    if st.remaining(&waivers).is_empty() {
        return Ok(st.resolve(&waivers));
    }
    Err(uncovered(id, &st, &waivers, touched, s, a))
}

/// What the branch did to the specs whose code it touched.
struct SpecState {
    /// specs whose `code:` globs the branch's changed paths fall inside
    matched: Vec<SpecName>,
    /// of those, and any other, the specs whose file the branch edited
    edited: Vec<SpecName>,
    /// matched but not edited — each owed a spec edit or a waiver
    unedited: Vec<SpecName>,
}

impl SpecState {
    /// The unedited specs no waiver answers for.
    fn remaining(&self, waivers: &[SpecWaiver]) -> Vec<SpecName> {
        self.unedited
            .iter()
            .filter(|n| !waivers.iter().any(|w| w.spec.as_ref() == Some(n)))
            .cloned()
            .collect()
    }

    /// The check's outcome once nothing remains. A waiver is recorded whenever it was
    /// offered, even where nothing demanded it: an explicit "no behaviour change" on the
    /// ticket is never worse than silence.
    fn resolve(&self, waivers: &[SpecWaiver]) -> SpecCheck {
        if !waivers.is_empty() {
            return SpecCheck::Unchanged {
                waivers: waivers.to_vec(),
            };
        }
        if self.matched.is_empty() && self.edited.is_empty() {
            return SpecCheck::NotApplicable;
        }
        SpecCheck::EditedOnBranch {
            specs: self.edited.clone(),
        }
    }
}

fn spec_state(touched: &[ChangedPath], s: &Snapshot) -> SpecState {
    let matched = specs_matching(touched, s);
    let edited = specs_edited(touched, s);
    let unedited = matched
        .iter()
        .filter(|n| !edited.contains(n))
        .cloned()
        .collect();
    SpecState {
        matched,
        edited,
        unedited,
    }
}

/// `--spec-unchanged` × N into typed waivers. `<spec>:<reason>` names its spec; a bare
/// reason is taken for the one unedited spec when there is exactly one, and refused when
/// there are several — that is the blanket answer a real change hides behind.
fn parse_waivers(
    id: &TicketId,
    raw: &[String],
    st: &SpecState,
    touched: &[ChangedPath],
    s: &Snapshot,
    a: &DoneArgs,
) -> Result<Vec<SpecWaiver>> {
    let mut out: Vec<SpecWaiver> = Vec::new();
    for r in raw {
        let r = r.trim();
        let named = r
            .split_once(':')
            .and_then(|(n, why)| SpecName::parse(n.trim()).ok().map(|n| (n, why.trim())))
            .filter(|(n, _)| s.specs.contains_key(n));
        let (spec, why) = match named {
            Some((n, why)) => {
                if !st.matched.contains(&n) {
                    return Err(KsError::invalid(
                        format!(
                            "{id}: --spec-unchanged names {n}, but the branch touched none of its code (touched: {})",
                            names(&st.matched)
                        ),
                        fixes![fix!(
                            "kanspec done {id}{}{}",
                            carried(a, &["--spec-unchanged"]),
                            per_spec_flags(&st.unedited, "no behaviour change")
                        )],
                    ));
                }
                (Some(n), why)
            }
            None => match st.unedited.as_slice() {
                [one] => (Some(one.clone()), r),
                [] => (None, r),
                many => {
                    return Err(KsError::gate(
                        "spec_unchanged_blanket",
                        format!(
                            "{id}: --spec-unchanged \"{r}\" answers for {} specs at once — say which:{}",
                            many.len(),
                            per_spec_lines(many, touched, s)
                        ),
                        fixes![fix!(
                            "kanspec done {id}{}{}",
                            carried(a, &["--spec-unchanged"]),
                            per_spec_flags(many, r)
                        )],
                    ));
                }
            },
        };
        if why.is_empty() {
            return Err(KsError::invalid(
                format!("{id}: --spec-unchanged needs a reason, not just a spec"),
                fixes![fix!(
                    "kanspec done {id}{}{}",
                    carried(a, &["--spec-unchanged"]),
                    per_spec_flags(&st.unedited, "no behaviour change")
                )],
            ));
        }
        if out.iter().any(|w| w.spec == spec) {
            continue;
        }
        out.push(SpecWaiver {
            spec,
            why: why.to_string(),
        });
    }
    Ok(out)
}

/// The refusal: one line per uncovered spec with the glob that caught it, and a fix that
/// answers for each one by name.
fn uncovered(
    id: &TicketId,
    st: &SpecState,
    waivers: &[SpecWaiver],
    touched: &[ChangedPath],
    s: &Snapshot,
    a: &DoneArgs,
) -> KsError {
    let remaining = st.remaining(waivers);
    let mut fixes: Vec<crate::error::Fix> = remaining
        .iter()
        .take(3)
        .map(|n| crate::error::Fix::cmd(format!("edit .kanspec/specs/{n}.md on this branch")))
        .collect();
    fixes.push(crate::error::Fix::cmd(format!(
        "kanspec done {id}{}{}",
        carried(a, &[]),
        per_spec_flags(&remaining, "no behaviour change")
    )));
    fixes.push(crate::error::Fix::cmd(format!(
        "kanspec spec show {}",
        remaining[0]
    )));
    let head = fixes.remove(0);
    KsError::gate(
        "spec_unchanged_unrecorded",
        format!(
            "{id}: the branch touched code {} own{} without editing {}:{}",
            plural_specs(remaining.len()),
            if remaining.len() == 1 { "s" } else { "" },
            if remaining.len() == 1 { "it" } else { "them" },
            per_spec_lines(&remaining, touched, s)
        ),
        crate::error::Fixes::new(head, fixes),
    )
}

fn plural_specs(n: usize) -> String {
    if n == 1 {
        "1 spec".to_string()
    } else {
        format!("{n} specs")
    }
}

/// `\n  auth — src/auth/** matched src/auth/login.ts`, one per spec.
fn per_spec_lines(specs: &[SpecName], touched: &[ChangedPath], s: &Snapshot) -> String {
    specs
        .iter()
        .map(|n| format!("\n  {n} — {}", where_matched(n, touched, s)))
        .collect()
}

/// ` --spec-unchanged "auth:<why>" --spec-unchanged "billing:<why>"`.
fn per_spec_flags(specs: &[SpecName], why: &str) -> String {
    specs
        .iter()
        .map(|n| format!(" --spec-unchanged \"{n}:{why}\""))
        .collect()
}

/// `src/auth/** matched src/auth/login.ts` — the spec's glob that caught the branch, and
/// the first changed path it caught, so a spec named through a broad glob is visibly so.
fn where_matched(name: &SpecName, touched: &[ChangedPath], s: &Snapshot) -> String {
    let Some(spec) = s.specs.get(name) else {
        return "(spec missing)".to_string();
    };
    for g in &spec.fm.code {
        let Some(set) = compile(std::slice::from_ref(g)) else {
            continue;
        };
        if let Some(c) = touched.iter().find(|c| set.is_match(&c.path)) {
            return format!("{g} matched {}", c.path);
        }
    }
    globs_of_specs(std::slice::from_ref(name), s).join(", ")
}

/// Specs whose `code:` globs the branch's changed paths fall inside — the input to both
/// front doors, and what `done` reports as the checkpoint's globs.
pub(crate) fn specs_matching(touched: &[ChangedPath], s: &Snapshot) -> Vec<SpecName> {
    let mut out: Vec<SpecName> = Vec::new();
    for (name, spec) in &s.specs {
        let Some(set) = compile(&spec.fm.code) else {
            continue;
        };
        if touched.iter().any(|c| set.is_match(&c.path)) {
            out.push(name.clone());
        }
    }
    out
}

/// Specs whose own FILE the branch edited — the thing the checkpoint is actually asking
/// about.
fn specs_edited(touched: &[ChangedPath], s: &Snapshot) -> Vec<SpecName> {
    s.specs
        .keys()
        .filter(|n| {
            let rel = spec_rel_path(n);
            touched
                .iter()
                .any(|c| c.path == rel || c.renamed_from.as_deref() == Some(rel.as_str()))
        })
        .cloned()
        .collect()
}

/// The globs a captured quirk or decision should steer by, when nobody named any: the
/// `code:` globs of the specs this branch actually touched.
fn default_scope(touched: &[ChangedPath], s: &Snapshot) -> Vec<String> {
    globs_of_specs(&specs_matching(touched, s), s)
}

fn globs_of_specs(names: &[SpecName], s: &Snapshot) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if let Some(spec) = s.specs.get(n) {
            for g in &spec.fm.code {
                if !out.contains(g) {
                    out.push(g.clone());
                }
            }
        }
    }
    out
}

/// `literal_separator` matches git's `:(glob)` semantics — `src/*.ts` is one directory
/// deep, `src/**` is all of them — so the checkpoint counts what `scan` counted.
fn compile(globs: &[String]) -> Option<GlobSet> {
    let mut b = GlobSetBuilder::new();
    let mut any = false;
    for g in globs {
        if let Ok(glob) = GlobBuilder::new(g).literal_separator(true).build() {
            b.add(glob);
            any = true;
        }
    }
    any.then(|| b.build().ok()).flatten()
}

fn names(v: &[SpecName]) -> String {
    if v.is_empty() {
        return "no spec".to_string();
    }
    v.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
}

fn split_globs(raw: &str, default: &[String]) -> Vec<String> {
    let picked: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_string)
        .collect();
    if picked.is_empty() {
        default.to_vec()
    } else {
        picked
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The interactive door
// ─────────────────────────────────────────────────────────────────────────────

/// Questions out, answers in. Held behind a struct so the whole `done` transcript is a unit
/// test over two byte buffers rather than something only a human can exercise.
pub(crate) struct Prompter<'io> {
    input: Box<dyn BufRead + 'io>,
    output: Box<dyn Write + 'io>,
}

impl<'io> Prompter<'io> {
    fn stdio() -> Prompter<'io> {
        Prompter {
            input: Box::new(std::io::BufReader::new(std::io::stdin())),
            // stderr, so stdout stays exactly the rendered report.
            output: Box::new(std::io::stderr()),
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted(answers: &'io str, sink: &'io mut Vec<u8>) -> Prompter<'io> {
        Prompter {
            input: Box::new(answers.as_bytes()),
            output: Box::new(sink),
        }
    }

    fn say(&mut self, line: &str) -> Result<()> {
        writeln!(self.output, "{line}").map_err(KsError::internal)
    }

    fn ask(&mut self, q: &str) -> Result<String> {
        write!(self.output, "{q}").map_err(KsError::internal)?;
        self.output.flush().map_err(KsError::internal)?;
        let mut line = String::new();
        let n = self.input.read_line(&mut line).map_err(KsError::internal)?;
        if n == 0 {
            // EOF mid-gate. Answering for the user is the one thing this gate may never
            // do, so it says which flags carry the answer instead.
            return Err(KsError::gate(
                "triage_input_closed",
                "the close-out gate needs an answer and stdin closed",
                fixes![
                    fix!("kanspec done <id> --json --no-followups --no-quirks"),
                    fix!("kanspec instructions done"),
                ],
            ));
        }
        Ok(line.trim_end_matches(['\n', '\r']).to_string())
    }
}

/// `[s]pawn ticket / [d]rop with reason / [x] actually done` — and nothing else. An
/// unrecognised key re-asks; there is deliberately no key that means "skip".
fn ask_step(io: &mut Prompter<'_>, st: &Step) -> Result<StepDisposition> {
    for _ in 0..8 {
        let key = io.ask("  [s]pawn ticket / [d]rop with reason / [x] actually done: ")?;
        match key.trim().to_ascii_lowercase().as_str() {
            "s" | "spawn" => {
                let title = io.ask(&format!("    title [enter = \"{}\"]: ", st.text))?;
                let title = if title.trim().is_empty() {
                    st.text.clone()
                } else {
                    title.trim().to_string()
                };
                return Ok(StepDisposition::Spawn {
                    index: st.index,
                    title,
                });
            }
            "d" | "drop" => {
                let why = io.ask("    reason: ")?;
                return Ok(StepDisposition::Drop {
                    index: st.index,
                    why: why.trim().to_string(),
                });
            }
            "x" | "done" => return Ok(StepDisposition::ActuallyDone { index: st.index }),
            _ => io.say("  answer s, d or x — there is no fourth option")?,
        }
    }
    Err(KsError::gate(
        "triage_unanswered",
        format!("step {} was never dispositioned", st.index),
        fixes![
            fix!("kanspec done <id> --drop-step \"{}:reason\"", st.index),
            fix!("kanspec instructions done"),
        ],
    ))
}

/// One capture — a quirk or a decision: its title (enter = none), then the globs it steers
/// by, defaulting to the `code:` globs of the specs this branch touched. `None` when the
/// human said nothing, which is a claim a prompt CAN make (see the module header).
fn ask_capture(
    io: &mut Prompter<'_>,
    question: &str,
    what: &str,
    default: &[String],
) -> Result<Option<(String, Vec<String>)>> {
    let title = io.ask(question)?;
    if title.trim().is_empty() {
        return Ok(None);
    }
    let hint = if default.is_empty() {
        String::new()
    } else {
        format!(" [enter = {}]", default.join(", "))
    };
    let raw = io.ask(&format!("  {what} (comma-separated globs){hint}: "))?;
    Ok(Some((title.trim().to_string(), split_globs(&raw, default))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ids::TicketId;
    use crate::model::{Spec, SpecFm, Ticket, TicketFm};
    use chrono::{TimeZone, Utc};

    fn args(extra: &[&str]) -> DoneArgs {
        use clap::Parser as _;
        let mut argv: Vec<&str> = vec!["kanspec", "done", "t-9c41"];
        argv.extend_from_slice(extra);
        match crate::cli::Cli::try_parse_from(argv).expect("argv").command {
            crate::cli::Command::Done(a) => a,
            other => panic!("{other:?}"),
        }
    }

    fn ticket(steps: &[(bool, &str)]) -> Ticket {
        let fm: TicketFm = serde_yaml_ng::from_str(
            "id: t-9c41\ntitle: fixture\nstate: review\ncreated: 2026-08-30T09:00:00Z\n",
        )
        .unwrap();
        Ticket {
            fm,
            path: std::path::PathBuf::from(".kanspec/tickets/t-9c41.md"),
            body: String::new(),
            steps: steps
                .iter()
                .enumerate()
                .map(|(i, (done, text))| Step {
                    index: i + 1,
                    done: *done,
                    text: (*text).to_string(),
                })
                .collect(),
            log: Vec::new(),
            mtime: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    fn snap_with_spec() -> Snapshot {
        let mut s = Snapshot::empty(
            Config::default(),
            Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap(),
        );
        let name = SpecName::parse("auth").unwrap();
        let fm: SpecFm = serde_yaml_ng::from_str("feature: Login\ncode: [src/auth/**]\n").unwrap();
        s.specs.insert(
            name.clone(),
            Spec {
                name,
                fm,
                path: std::path::PathBuf::from(".kanspec/specs/auth.md"),
                body: String::new(),
                rules: Vec::new(),
            },
        );
        s
    }

    fn snap_with_specs(specs: &[(&str, &str)]) -> Snapshot {
        let mut s = snap_with_spec();
        s.specs.clear();
        for (name, glob) in specs {
            let name = SpecName::parse(name).unwrap();
            let fm: SpecFm =
                serde_yaml_ng::from_str(&format!("feature: F\ncode: [{glob}]\n")).unwrap();
            s.specs.insert(
                name.clone(),
                Spec {
                    path: std::path::PathBuf::from(format!(".kanspec/specs/{name}.md")),
                    name,
                    fm,
                    body: String::new(),
                    rules: Vec::new(),
                },
            );
        }
        s
    }

    /// t-852e: five specs matched through broad globs were named as one clump, and one
    /// blanket reason answered for all of them. Now: one line per spec with the glob that
    /// caught it, and one answer per spec.
    #[test]
    fn several_unedited_specs_are_answered_one_by_one_never_with_a_blanket_reason() {
        let t = ticket(&[]);
        let s = snap_with_specs(&[
            ("auth", "src/auth/**"),
            ("billing", "src/billing/**"),
            ("wide", "src/**"),
        ]);
        let touched = changed(&["src/auth/login.ts", "src/billing/charge.ts"]);
        let base = ["--no-followups", "--no-quirks"];

        // Unanswered: every uncovered spec on its own line, with the glob that caught it.
        let e = Triage::from_args(&t, &args(&base), &touched, &s).unwrap_err();
        assert_eq!(e.code(), Some("spec_unchanged_unrecorded"));
        let msg = e.to_string();
        for line in [
            "auth — src/auth/** matched src/auth/login.ts",
            "billing — src/billing/** matched src/billing/charge.ts",
            "wide — src/** matched src/auth/login.ts",
        ] {
            assert!(msg.contains(line), "{msg}");
        }
        let fix = e
            .fixes()
            .iter()
            .map(|f| f.as_str().to_string())
            .collect::<Vec<_>>();
        assert!(
            fix.iter().any(|f| f.contains("--spec-unchanged \"auth:")
                && f.contains("--spec-unchanged \"billing:")
                && f.contains("--spec-unchanged \"wide:")),
            "the fix answers for each spec by name: {fix:?}"
        );

        // A blanket reason over three specs is the thing a real change hides behind.
        let mut blanket = base.to_vec();
        blanket.extend(["--spec-unchanged", "refactor only"]);
        let e = Triage::from_args(&t, &args(&blanket), &touched, &s).unwrap_err();
        assert_eq!(e.code(), Some("spec_unchanged_blanket"), "{e}");
        assert!(e.to_string().contains("3 specs at once"), "{e}");
        let fix = e.fixes().iter().next().unwrap().as_str().to_string();
        assert!(
            fix.contains("--spec-unchanged \"auth:refactor only\"")
                && !fix.contains("--spec-unchanged \"refactor only\""),
            "the blanket is replaced, not carried: {fix}"
        );

        // Two of three answered by name: the third is still owed, alone.
        let mut two = base.to_vec();
        two.extend([
            "--spec-unchanged",
            "auth: renamed a helper",
            "--spec-unchanged",
            "wide:no behaviour change",
        ]);
        let e = Triage::from_args(&t, &args(&two), &touched, &s).unwrap_err();
        assert_eq!(e.code(), Some("spec_unchanged_unrecorded"));
        let msg = e.to_string();
        assert!(
            msg.contains("billing —") && !msg.contains("auth —"),
            "{msg}"
        );

        // All three: recorded per spec, in the order given.
        let mut three = two.clone();
        three.extend(["--spec-unchanged", "billing:same"]);
        let ok = Triage::from_args(&t, &args(&three), &touched, &s).unwrap();
        assert_eq!(
            ok.spec_unchanged().as_deref(),
            Some("auth: renamed a helper · wide: no behaviour change · billing: same")
        );

        // A spec the branch never touched cannot be waived — that is a typo or a lie.
        let mut wrong = base.to_vec();
        wrong.extend(["--spec-unchanged", "auth:x", "--spec-unchanged", "wide:x"]);
        let s2 = snap_with_specs(&[
            ("auth", "src/auth/**"),
            ("wide", "src/**"),
            ("docs", "docs/**"),
        ]);
        let ok = Triage::from_args(&t, &args(&wrong), &changed(&["src/auth/a.ts"]), &s2).unwrap();
        assert!(ok.spec_unchanged().is_some());
        wrong.extend(["--spec-unchanged", "docs:x"]);
        let e =
            Triage::from_args(&t, &args(&wrong), &changed(&["src/auth/a.ts"]), &s2).unwrap_err();
        assert!(
            e.to_string()
                .contains("names docs, but the branch touched none of its code"),
            "{e}"
        );

        // A reason with no spec where nothing was demanded is still recorded.
        let mut free = base.to_vec();
        free.extend(["--spec-unchanged", "docs only"]);
        let ok = Triage::from_args(&t, &args(&free), &changed(&["README.md"]), &s).unwrap();
        assert_eq!(ok.spec_unchanged().as_deref(), Some("docs only"));
    }

    fn changed(paths: &[&str]) -> Vec<ChangedPath> {
        paths
            .iter()
            .map(|p| ChangedPath {
                status: 'M',
                path: (*p).to_string(),
                renamed_from: None,
            })
            .collect()
    }

    #[track_caller]
    fn code(r: Result<Triage>) -> &'static str {
        match r {
            Ok(t) => panic!("expected a refusal, got {t:?}"),
            Err(e) => e.code().unwrap_or(e.kind()),
        }
    }

    /// `"a b"` is one argument.
    fn split(cmd: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let (mut q, mut any) = (false, false);
        for c in cmd.chars() {
            match c {
                '"' => {
                    q = !q;
                    any = true;
                }
                ' ' if !q => {
                    if any || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        any = false;
                    }
                }
                _ => cur.push(c),
            }
        }
        if any || !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    /// THE regression. Each `done` gate suggests the flag that answers IT; when the
    /// suggestion named only that flag it discarded every answer already given, and since
    /// the gates are checked in order the advice rang:
    ///
    /// ```text
    /// done t-9c41                  → done t-9c41 --no-followups
    /// done t-9c41 --no-followups   → done t-9c41 --no-quirks      ← drops --no-followups
    /// done t-9c41 --no-quirks      → done t-9c41 --no-followups   ← step 2 again, for ever
    /// ```
    ///
    /// Verified on the REAL binary over a genuinely merged ticket before the fix: this is
    /// the ordinary daily close-out, not an edge case. The property is not "each arrow
    /// parses" (`tests/cli_well_formed.rs` owns that) but that FOLLOWING them terminates.
    #[test]
    fn following_the_suggested_fix_through_the_done_gates_terminates() {
        // A ticket with an unchecked step, so the step gate is reached too and the walk
        // passes through more than the two flag gates.
        let t = ticket(&[(false, "wire the thing")]);
        let s = snap_with_spec();
        let touched = changed(&["src/auth/login.rs"]);

        let mut extra: Vec<String> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        let mut trail = String::new();
        for step in 1..=10 {
            let key = extra.join(" ");
            trail.push_str(&format!("\n  {step}. kanspec done t-9c41 {key}"));
            assert!(
                !seen.contains(&key),
                "the `done` gates ring — an agent whose contract is \"run the suggested \
                 command\" never gets out:{trail}"
            );
            seen.push(key);

            let a = args(&extra.iter().map(String::as_str).collect::<Vec<_>>());
            let e = match Triage::from_args(&t, &a, &touched, &s) {
                // Terminated in a SUCCESS, not merely by giving up.
                Ok(_) => {
                    assert!(
                        step > 1,
                        "the fixture refused nothing — the walk proved nothing"
                    );
                    return;
                }
                Err(e) => e,
            };
            trail.push_str(&format!("\n       ✗ {}", e.code().unwrap_or(e.kind())));

            // What an agent does: run the first suggestion that is a `done` command.
            let next = e
                .fixes()
                .iter()
                .map(|f| f.as_str().to_string())
                .find(|f| f.starts_with("kanspec done t-9c41"))
                .unwrap_or_else(|| panic!("no `done` suggestion to follow:{trail}"));
            trail.push_str(&format!("\n       → {next}"));
            extra = split(&next).split_off(3);
        }
        panic!("the suggested fixes did not terminate in 10 steps:{trail}");
    }

    /// The binding correction, as a test: an omission is not a claim.
    #[test]
    fn nothing_left_must_be_said_out_loud_even_when_nothing_is_left() {
        let t = ticket(&[(true, "already done")]);
        let s = snap_with_spec();
        assert_eq!(
            code(Triage::from_args(&t, &args(&[]), &[], &s)),
            "followups_unanswered"
        );
        assert_eq!(
            code(Triage::from_args(&t, &args(&["--no-followups"]), &[], &s)),
            "quirks_unanswered"
        );
        Triage::from_args(&t, &args(&["--no-followups", "--no-quirks"]), &[], &s)
            .expect("both claims made");
    }

    /// No fourth option: an unchecked step that nobody spoke for stops the close-out.
    #[test]
    fn an_unchecked_step_with_no_outcome_is_refused_by_name() {
        let t = ticket(&[(true, "a"), (false, "429 + Retry-After")]);
        let s = snap_with_spec();
        assert_eq!(
            code(Triage::from_args(
                &t,
                &args(&["--no-followups", "--no-quirks"]),
                &[],
                &s
            )),
            "steps_undispositioned"
        );
    }

    #[test]
    fn the_three_dispositions_cover_three_steps_and_nothing_else_is_offered() {
        let t = ticket(&[(false, "one"), (false, "two"), (false, "three")]);
        let s = snap_with_spec();
        let tri = Triage::from_args(
            &t,
            &args(&[
                "--drop-step",
                "2:superseded",
                "--actually-done",
                "3",
                "--spawn",
                "finish one",
                "--no-quirks",
            ]),
            &[],
            &s,
        )
        .expect("every step spoken for");
        assert_eq!(tri.spawns(), vec![(1, "finish one")]);
        assert_eq!(tri.dropped(), vec![(2, "superseded")]);
        assert_eq!(tri.actually_done(), vec![3]);
    }

    #[test]
    fn a_drop_without_a_reason_is_not_a_disposition() {
        let t = ticket(&[(false, "one")]);
        let s = snap_with_spec();
        assert_eq!(
            code(Triage::from_args(
                &t,
                &args(&["--drop-step", "1:   ", "--no-followups", "--no-quirks"]),
                &[],
                &s
            )),
            "step_dropped_without_reason"
        );
    }

    #[test]
    fn a_spawn_with_no_step_behind_it_is_sent_to_kanspec_new() {
        let t = ticket(&[(true, "done already")]);
        let s = snap_with_spec();
        assert_eq!(
            code(Triage::from_args(
                &t,
                &args(&["--spawn", "something else", "--no-quirks"]),
                &[],
                &s
            )),
            "spawn_without_step"
        );
    }

    #[test]
    fn a_quirk_that_matches_nothing_never_warns_anyone() {
        let t = ticket(&[]);
        let s = snap_with_spec();
        assert_eq!(
            code(Triage::from_args(
                &t,
                &args(&["--no-followups", "--quirk", "webhooks replay"]),
                &[],
                &s
            )),
            "quirk_without_paths"
        );
    }

    /// Anti-rot gear 2: touching a spec's globs without touching the spec needs a recorded
    /// reason — and the reason lands on the ticket.
    #[test]
    fn touching_a_specs_code_without_editing_the_spec_demands_a_recorded_reason() {
        let t = ticket(&[]);
        let s = snap_with_spec();
        let touched = changed(&["src/auth/login.ts"]);
        assert_eq!(
            code(Triage::from_args(
                &t,
                &args(&["--no-followups", "--no-quirks"]),
                &touched,
                &s
            )),
            "spec_unchanged_unrecorded"
        );

        let ok = Triage::from_args(
            &t,
            &args(&[
                "--no-followups",
                "--no-quirks",
                "--spec-unchanged",
                "refactor only",
            ]),
            &touched,
            &s,
        )
        .expect("a recorded waiver satisfies the checkpoint");
        assert_eq!(
            ok.spec_unchanged().as_deref(),
            Some("auth: refactor only"),
            "a bare reason with one spec unedited is that spec's"
        );

        let edited = Triage::from_args(
            &t,
            &args(&["--no-followups", "--no-quirks"]),
            &changed(&["src/auth/login.ts", ".kanspec/specs/auth.md"]),
            &s,
        )
        .expect("the spec moved with the code");
        assert!(matches!(edited.spec, SpecCheck::EditedOnBranch { .. }));
    }

    #[test]
    fn a_branch_that_touched_no_specs_globs_is_not_applicable() {
        let t = ticket(&[]);
        let s = snap_with_spec();
        let tri = Triage::from_args(
            &t,
            &args(&["--no-followups", "--no-quirks"]),
            &changed(&["README.md"]),
            &s,
        )
        .unwrap();
        assert!(matches!(tri.spec, SpecCheck::NotApplicable));
    }

    /// THE point of the type: the human path and the agent path produce the same value.
    #[test]
    fn the_prompt_and_the_flags_build_the_same_triage() {
        let t = ticket(&[(false, "429 + Retry-After")]);
        let s = snap_with_spec();
        let touched = changed(&["src/auth/login.ts"]);

        let flags = Triage::from_args(
            &t,
            &args(&[
                "--spawn",
                "429 + Retry-After",
                "--spec-unchanged",
                "no behaviour change",
                "--quirk",
                "redis flushes on redeploy",
                "--quirk-paths",
                "src/auth/**",
            ]),
            &touched,
            &s,
        )
        .expect("the agent door");

        // s · <enter accepts the step's own text> · <spec waiver> · quirk · paths · no decision
        let script = "s\n\nno behaviour change\nredis flushes on redeploy\nsrc/auth/**\n\n";
        let mut sink: Vec<u8> = Vec::new();
        let mut io = Prompter::scripted(script, &mut sink);
        let prompted =
            Triage::prompt_with(&mut io, &t, &args(&[]), &touched, &s).expect("the human door");
        drop(io);

        assert_eq!(
            serde_json::to_value(&flags).unwrap(),
            serde_json::to_value(&prompted).unwrap(),
            "the two front doors must record the same act"
        );
        let transcript = String::from_utf8(sink).unwrap();
        assert!(
            transcript.contains("Leftover triage — 1 unchecked step:"),
            "{transcript}"
        );
        assert!(
            transcript.contains("[s]pawn ticket / [d]rop with reason / [x] actually done:"),
            "{transcript}"
        );
    }

    #[test]
    fn the_prompt_offers_no_fourth_key() {
        let t = ticket(&[(false, "one")]);
        let s = snap_with_spec();
        let mut sink: Vec<u8> = Vec::new();
        // `q`, `skip`, then a real answer.
        let mut io = Prompter::scripted("q\nskip\nx\n\n\n", &mut sink);
        let tri = Triage::prompt_with(&mut io, &t, &args(&[]), &[], &s).unwrap();
        drop(io);
        assert_eq!(tri.actually_done(), vec![1]);
        let transcript = String::from_utf8(sink).unwrap();
        assert_eq!(
            transcript.matches("there is no fourth option").count(),
            2,
            "{transcript}"
        );
    }

    /// A closed stdin cannot be read as consent.
    #[test]
    fn eof_in_the_middle_of_the_gate_is_a_refusal_not_a_default() {
        let t = ticket(&[(false, "one")]);
        let s = snap_with_spec();
        let mut sink: Vec<u8> = Vec::new();
        let mut io = Prompter::scripted("", &mut sink);
        assert_eq!(
            code(Triage::prompt_with(&mut io, &t, &args(&[]), &[], &s)),
            "triage_input_closed"
        );
    }

    #[test]
    fn specs_matching_is_the_input_both_doors_share() {
        let s = snap_with_spec();
        let hit = specs_matching(&changed(&["src/auth/login.ts"]), &s);
        assert_eq!(hit.iter().map(|n| n.as_str()).collect::<Vec<_>>(), ["auth"]);
        assert!(specs_matching(&changed(&["src/billing/x.ts"]), &s).is_empty());
        // `literal_separator`: `src/auth/**` crosses directories exactly like git's
        // `:(glob)`, which is what makes this agree with `scan`.
        assert_eq!(
            specs_matching(&changed(&["src/auth/deep/nested.ts"]), &s).len(),
            1
        );
        let _ = TicketId::parse("t-9c41").unwrap();
    }
}
