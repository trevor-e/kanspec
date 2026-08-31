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
use crate::error::{KsError, Result};
use crate::out::{Color, Render, Style};
use crate::rulesdoc::{self, AuditWarning, RulesDoc, Scope};
use crate::{fix, fixes};

#[derive(Debug, Serialize)]
pub struct RulesReport {
    /// `prime --json .standing == rules --json .data` — the JSON half of invariant 3
    pub data: RulesDoc,
    pub scope: Vec<String>,
    pub warnings: Vec<AuditWarning>,
    pub adopted: Vec<String>,
    /// `--audit` prints only the warnings, so the default stdout stays the injection
    /// surface byte for byte.
    #[serde(skip)]
    audit: bool,
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
        // `--adopt` stamps provenance onto a pre-kanspec rule BULLET, and the typed edit
        // vocabulary has no op that rewrites a line inside an entity body: `SetFields` is
        // frontmatter-only and `AppendSection` appends. Reported as a request to F, and
        // DESIGN.md's build plan puts `rules --audit/--adopt` in v0.2 anyway.
        //
        // A refusal, not a silent no-op: an exit 0 that changed nothing is how a human
        // comes to believe the audit is clean.
        let n = warnings.iter().filter(|w| w.adoptable).count();
        let dir = ctx.layout.specs_dir();
        let dir = dir.strip_prefix(ctx.repo.primary_root()).unwrap_or(&dir);
        return Err(KsError::gate(
            "adopt_needs_a_body_edit",
            format!(
                "{n} spec rule(s) carry no provenance; kanspec cannot yet stamp a rule bullet \
                 for you"
            ),
            fixes![
                fix!("{} rules --audit", ctx.invoked_as),
                fix!(
                    "add a `{{p-xxxx}}` token to the bullet in {}/",
                    dir.display()
                ),
                fix!("{} decide \"...\" --scope \"src/**\"", ctx.invoked_as),
            ],
        ));
    }

    Ok(RulesReport {
        data,
        scope: a.paths.clone(),
        warnings,
        adopted: Vec::new(),
        audit: a.audit,
    })
}

impl Render for RulesReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // `--audit` is a lint over the standing set, not a second rendering of it: it
        // prints its warnings and stops. That is what keeps the DEFAULT stdout — the thing
        // invariant 3 is about — exactly `render_text` and nothing else.
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
