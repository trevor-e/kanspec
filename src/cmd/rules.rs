//! `rules [--path <file>] [--audit] [--adopt]`.
//!
//! Invariant 3: this command's stdout is **byte-identical** to the standing-rules section
//! of `kanspec prime`, because both call `rulesdoc::build` then `rulesdoc::render_text`
//! and there is physically no second formatter. `tests/invariants_rules.rs` asserts it on
//! the real binary, across several scopes.
//!
//! Owner: **S6**.

use serde::Serialize;

use crate::cli::RulesArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::out::{Color, Render, Style};
use crate::plan::{Op, Plan};
use crate::project;
use crate::rulesdoc::{self, AuditWarning, RulesDoc, Scope};
use crate::store::Store;

#[derive(Debug, Serialize)]
pub struct RulesReport {
    /// `prime --json .standing == rules --json .data` — the JSON half of invariant 3
    pub data: RulesDoc,
    pub scope: Vec<String>,
    pub warnings: Vec<AuditWarning>,
    pub adopted: Vec<String>,
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
    let snap = ctx.snapshot()?;
    let scope = Scope::of(&a.paths)?;
    let data = rulesdoc::build(&snap, &scope);
    let warnings = if a.audit || a.adopt {
        rulesdoc::audit(&snap, &data)
    } else {
        Vec::new()
    };

    if a.adopt {
        // Adoption is what a MIGRATED corpus needs: an imported rule has no proposal to
        // point at, and without a way to answer that, `rules --audit` warns about every one
        // of them forever and a human learns to stop reading it.
        //
        // The planner runs against the snapshot reloaded inside the lock, so the bullets it
        // stamps are the bullets that are there — not the ones the pre-lock `warnings` above
        // saw. Those two lists agree in practice because both come from `rulesdoc::adoptable`.
        let mut adopted = Vec::new();
        Store::open(ctx).transact(None, &ctx.invocation(), |s, _m| {
            let ops: Vec<Op> = rulesdoc::adoptable(s)
                .into_iter()
                .map(|x| {
                    adopted.push(format!("{} [{}]", x.spec, x.anchor));
                    Op::StampRule {
                        spec: x.spec,
                        anchor: x.anchor,
                        line: x.line,
                    }
                })
                .collect();
            // An empty plan is a success that writes nothing (`Store::transact` short-
            // circuits it), which is exactly right for a second `--adopt`.
            Ok(Plan::of(ops))
        })?;
        // D-20: a spec body was rewritten, so the committed projections are republished
        // from the state this write produced. No projection renders rule TEXT today — the
        // feature map's `last_shipped` reads `{p-xxxx}` provenance, which `{pre-kanspec}`
        // deliberately is not — so this rewrites the same bytes. It is here anyway, because
        // the invariant is "a handler that writes a projected entity republishes", and
        // resting on a fact about today's renderers is how the committed page comes to rot.
        project::regenerate(ctx)?;

        // Re-read: the report must describe the corpus AFTER the stamps, or `--json`
        // consumers see the standing set as it was a moment ago.
        let snap = ctx.snapshot()?;
        let data = rulesdoc::build(&snap, &scope);
        let warnings = rulesdoc::audit(&snap, &data);
        return Ok(RulesReport {
            data,
            scope: a.paths.clone(),
            warnings,
            adopted,
            audit: false,
            adopt: true,
        });
    }

    Ok(RulesReport {
        data,
        scope: a.paths.clone(),
        warnings,
        adopted: Vec::new(),
        audit: a.audit,
        adopt: false,
    })
}

impl Render for RulesReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // `--audit` is a lint over the standing set, not a second rendering of it: it
        // prints its warnings and stops. That is what keeps the DEFAULT stdout — the thing
        // invariant 3 is about — exactly `render_text` and nothing else.
        if self.adopt {
            if self.adopted.is_empty() {
                writeln!(w, " ✓ nothing left to adopt — every standing rule points home")?;
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
