//! `rules [--path <file>] [--full] [--audit] [--adopt]`.
//!
//! Invariant 3: this command's stdout is **byte-identical** to the standing-rules section
//! of `kanspec prime`, because both call `rulesdoc::build` then `rulesdoc::render_text`
//! and there is physically no second formatter. `tests/invariants_rules.rs` asserts it on
//! the real binary, across several scopes. `--full` is the one deliberate departure: it
//! lifts the spec-rules budget for a human checking what the budget named but did not
//! show, and `prime` has no such flag.
//!
//! Owner: **S6**.

use serde::Serialize;

use crate::cli::RulesArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::out::{Color, Render, Style};
use crate::plan::{Op, Plan};
use crate::rulesdoc::{self, AuditWarning, RulesDoc, Scope, SpecBudget};
use crate::store::Store;

#[derive(Debug, Serialize)]
pub struct RulesReport {
    /// `prime --json .standing == rules --json .data` — the JSON half of invariant 3
    pub data: RulesDoc,
    pub scope: Vec<String>,
    pub warnings: Vec<AuditWarning>,
    pub adopted: Vec<String>,
    /// `--budget <spec>`: what an agent will have to read before touching that capability
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<SpecBudget>,
    /// `--audit` prints only the warnings, so the default stdout stays the injection
    /// surface byte for byte. `--adopt` does the same with its own summary — invariant 3 is
    /// about the DEFAULT stdout, and both flags return before reaching `render_text`.
    #[serde(skip)]
    audit: bool,
    #[serde(skip)]
    adopt: bool,
}

pub fn rules(ctx: &Ctx, a: &RulesArgs) -> Result<RulesReport> {
    ctx.require_initialized()?;
    if let Some(name) = &a.budget {
        return budget(ctx, name);
    }
    let scope = Scope::of(&a.paths)?;

    // Adoption is what a MIGRATED corpus needs: an imported rule has no proposal to point
    // at, and without a way to answer that, `rules --audit` warns about every one of them
    // forever and a human learns to stop reading it.
    //
    // The planner is PURE (§2.16): it stamps whatever `rulesdoc::adoptable` finds in the
    // snapshot reloaded inside the lock. What was adopted is then MEASURED — the bullets
    // that were adoptable before and are not after — rather than recorded by the planner as
    // a side effect, so the summary and the `--json` body describe the same corpus.
    // One store load before the transaction, and `Committed.snapshot` after it (§2.16):
    // the report describes the corpus as it is NOW, without parsing the store a third
    // time to find out.
    let mut snap = ctx.snapshot()?;
    let before = a.adopt.then(|| adoptable_names(&snap));
    if a.adopt {
        let done = Store::open(ctx).transact(None, &ctx.invocation(), |s, _m| {
            // An empty plan is a success that writes nothing (`Store::transact` short-
            // circuits it), which is exactly right for a second `--adopt`.
            Ok(Plan::of(
                rulesdoc::adoptable(s)
                    .into_iter()
                    .map(|x| Op::StampRule {
                        spec: x.spec,
                        anchor: x.anchor,
                        line: x.line,
                    })
                    .collect(),
            ))
        })?;
        // A spec body was rewritten; `Store::transact` republishes the projections itself
        // (t-0769), and the snapshot it hands back is the corpus after the stamps.
        snap = done.snapshot;
    }

    let adopted = before
        .map(|b| {
            let after = adoptable_names(&snap);
            b.into_iter().filter(|n| !after.contains(n)).collect()
        })
        .unwrap_or_default();
    let data = if a.full {
        rulesdoc::build_full(&snap, &scope)
    } else {
        rulesdoc::build(&snap, &scope)
    };
    let warnings = if a.audit || a.adopt {
        rulesdoc::audit(&snap, &data)
    } else {
        Vec::new()
    };
    Ok(RulesReport {
        data,
        scope: a.paths.clone(),
        warnings,
        adopted,
        budget: None,
        audit: a.audit,
        adopt: a.adopt,
    })
}

/// `rules --budget <spec>`: the figure `review` puts under every `[tN]` (p-67f0), for a
/// human checking it. The surface is every tracked file under the spec's `code:` globs,
/// listed by ONE `git ls-files` so git does the matching with the semantics the rest of
/// the tool uses; the generator is then pointed at exactly those files, so `data` is what
/// `rules --path <each of them>` would print with the budget lifted.
fn budget(ctx: &Ctx, name: &str) -> Result<RulesReport> {
    let snap = ctx.snapshot()?;
    let name = crate::ids::SpecName::parse(name)?;
    let files = spec_files(ctx, &snap, &name)?;
    let figure = rulesdoc::spec_budget(&snap, &name, &files)?;
    let data = if figure.scoped && !files.is_empty() {
        rulesdoc::build_full(&snap, &Scope::of(&files)?)
    } else {
        // No surface to scope by: an empty scope would be the whole corpus, which is not
        // what this spec's agent reads. Show the spec's own rules alone.
        let mut d = rulesdoc::build_full(&snap, &Scope::none());
        d.decisions.clear();
        d.quirks.clear();
        d.spec_rules.retain(|r| r.spec == name);
        d.elided.clear();
        d
    };
    Ok(RulesReport {
        data,
        scope: files,
        warnings: Vec::new(),
        adopted: Vec::new(),
        budget: Some(figure),
        audit: false,
        adopt: false,
    })
}

/// Every tracked file under the spec's `code:` globs, or none when it names no globs.
pub fn spec_files(
    ctx: &Ctx,
    snap: &crate::model::Snapshot,
    name: &crate::ids::SpecName,
) -> Result<Vec<String>> {
    let Some(spec) = snap.specs.get(name) else {
        return Ok(Vec::new());
    };
    if spec.fm.code.is_empty() {
        return Ok(Vec::new());
    }
    let ps: Vec<crate::git::Pathspec> = spec
        .fm
        .code
        .iter()
        .map(|g| crate::git::Pathspec::glob(g))
        .collect();
    let out = ctx.git.run_ps(&["ls-files", "-z"], &ps).map_err(|e| {
        crate::error::KsError::internal(anyhow::anyhow!("cannot list tracked files: {e}"))
    })?;
    Ok(out
        .out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect())
}

/// `spec [anchor]` for every adoptable bullet — the spelling the `--adopt` summary prints.
fn adoptable_names(s: &crate::model::Snapshot) -> Vec<String> {
    rulesdoc::adoptable(s)
        .iter()
        .map(|x| format!("{} [{}]", x.spec, x.anchor))
        .collect()
}

impl Render for RulesReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // `--audit` is a lint over the standing set, not a second rendering of it: it
        // prints its warnings and stops. That is what keeps the DEFAULT stdout — the thing
        // invariant 3 is about — exactly `render_text` and nothing else.
        if self.adopt {
            if self.adopted.is_empty() {
                writeln!(
                    w,
                    " ✓ nothing left to adopt — every standing rule points home"
                )?;
                return Ok(());
            }
            writeln!(
                w,
                " ▸ adopted {} pre-kanspec rule(s) — each bullet now carries {}",
                self.adopted.len(),
                rulesdoc::ADOPTED_TOKEN
            )?;
            for x in &self.adopted {
                writeln!(w, "   {x}")?;
            }
            // Adoption records that a human vouched for the text, NOT that it came from a
            // proposal. Saying so is the difference between a marker and a fake provenance.
            writeln!(
                w,
                " {} adoption is a human vouching for imported text, not real provenance",
                crate::out::paint("note", Color::Cyan, st.color)
            )?;
            for x in &self.warnings {
                writeln!(w, " ⚠ {}: {}", x.subject, x.message)?;
            }
            return Ok(());
        }

        if let Some(b) = &self.budget {
            // One line, the shape `review` prints under a `[tN]`: what the agent reads,
            // then the surface it was measured on.
            let mut line = format!(
                " {}  reads {} rule{} ({} tokens",
                crate::out::paint(b.spec.as_str(), Color::Cyan, st.color),
                b.rules,
                if b.rules == 1 { "" } else { "s" },
                rulesdoc::tokens_short(b.tokens),
            );
            if b.injected_tokens != b.tokens {
                line.push_str(&format!(
                    ", {} under the prime budget, {} spec{} named not shown",
                    rulesdoc::tokens_short(b.injected_tokens),
                    b.elided,
                    if b.elided == 1 { "" } else { "s" }
                ));
            }
            line.push(')');
            if b.decisions > 0 {
                line.push_str(&format!(
                    " · {} decision{}",
                    b.decisions,
                    if b.decisions == 1 { "" } else { "s" }
                ));
            }
            if b.quirks > 0 {
                line.push_str(&format!(
                    " · {} quirk{}",
                    b.quirks,
                    if b.quirks == 1 { "" } else { "s" }
                ));
            }
            if !b.scoped {
                line.push_str(" · surface unknown — spec names no code");
            } else {
                line.push_str(&format!(
                    " · surface {} file{}",
                    b.files,
                    if b.files == 1 { "" } else { "s" }
                ));
            }
            writeln!(w, "{line}")?;
            writeln!(
                w,
                " {} what the agent reads before touching this capability — never an estimate of the work",
                crate::out::paint("note", Color::Cyan, st.color)
            )?;
            return Ok(());
        }

        if self.audit {
            for x in &self.warnings {
                writeln!(
                    w,
                    " ⚠ {}: {} → {}",
                    x.subject,
                    x.message,
                    crate::out::paint(&x.fix, Color::Cyan, st.color)
                )?;
            }
            if self.warnings.is_empty() {
                writeln!(w, " ✓ every standing rule still points home")?;
            }
            return Ok(());
        }

        // VERBATIM, and nothing else. A header, a trailing newline or a colour code added
        // here breaks invariant 3 — which is exactly what `tests/invariants_rules.rs`
        // diffs against the real `prime` binary output.
        write!(w, "{}", rulesdoc::render_text(&self.data))
    }
}
