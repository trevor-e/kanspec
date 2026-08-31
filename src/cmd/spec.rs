//! `spec new|show|grep` — the living per-capability specs.
//!
//! Specs are edited **on the implementation branch** and reviewed in the PR like any code:
//! no archive-time merge, no deferred delta debt, no bot commits to trunk.
//!
//! Owner: **S6**.

use serde::Serialize;

use crate::cli::{SpecArgs, SpecCommand};
use crate::ctx::Ctx;
use crate::derive::{self, Staleness};
use crate::error::Result;
use crate::fm::{self, Yv};
use crate::ids::SpecName;
use crate::model::Rule;
use crate::out::{glyph, Color, Line, Render, Style};
use crate::plan::{EntityRef, Op, Plan};
use crate::project;
use crate::store::Store;

#[derive(Debug, Serialize)]
#[serde(tag = "spec", rename_all = "snake_case")]
pub enum SpecReport {
    Created {
        name: SpecName,
        path: String,
        next: Vec<String>,
    },
    Shown {
        name: SpecName,
        feature: String,
        code: Vec<String>,
        rules: Vec<Rule>,
        staleness: Staleness,
        next: Vec<String>,
    },
    Grepped {
        pattern: String,
        hits: Vec<GrepHit>,
    },
}

#[derive(Debug, Serialize)]
pub struct GrepHit {
    pub spec: SpecName,
    pub anchor: String,
    pub text: String,
    pub line: usize,
}

pub fn spec(ctx: &Ctx, a: &SpecArgs) -> Result<SpecReport> {
    ctx.require_initialized()?;
    match &a.cmd {
        SpecCommand::New {
            name,
            feature,
            code,
        } => new(ctx, name, feature.as_deref(), code),
        SpecCommand::Show { name } => show(ctx, name),
        SpecCommand::Grep { pattern } => grep(ctx, pattern),
    }
}

fn new(ctx: &Ctx, raw: &str, feature: Option<&str>, code: &[String]) -> Result<SpecReport> {
    let name = SpecName::parse(raw)?;
    // Fail on a bad glob HERE, before the lock: a spec whose `code:` cannot compile feeds
    // the staleness tripwire nothing, and finding that out later is finding it out never.
    crate::rulesdoc::Scope::of(code)?;

    let contents = scaffold(&name, feature.unwrap_or(raw), code);
    let entity = EntityRef::Spec(name.clone());
    Store::open(ctx).transact(None, &ctx.invocation(), |_s, _m| {
        Ok(Plan::of(vec![Op::CreateEntity {
            entity: entity.clone(),
            contents: contents.clone(),
        }]))
    })?;
    // D-20, second half: the committed projections are rewritten from the state this write
    // produced. See `project::regenerate` for why it is a second transaction.
    project::regenerate(ctx)?;

    Ok(SpecReport::Created {
        path: rel(ctx, &ctx.layout.spec(&name)),
        next: vec![
            format!("{} spec show {name}", ctx.invoked_as),
            format!("{} rules --path <file>", ctx.invoked_as),
        ],
        name,
    })
}

/// The frontmatter DESIGN.md's spec carries, and a `## Rules` heading to write into.
/// Emitted through `fm::emit`, so a `feature:` one-liner containing a colon or a glob
/// containing a brace cannot produce YAML that fails to parse back.
fn scaffold(name: &SpecName, feature: &str, code: &[String]) -> String {
    format!(
        "---\nfeature: {}\ncode: {}\n---\n# {name}\n\n## Rules\n",
        fm::emit(&Yv::s(feature), false),
        fm::emit(&Yv::list(code.to_vec()), false),
    )
}

fn show(ctx: &Ctx, raw: &str) -> Result<SpecReport> {
    let snap = ctx.snapshot()?;
    let name = SpecName::parse(raw)?;
    let spec = snap.spec(&name)?;
    let staleness = derive::staleness(&snap, spec);
    let mut next = vec![format!("{} rules --path <file>", ctx.invoked_as)];
    if !matches!(staleness, Staleness::Ok) {
        next.insert(0, stale_fix(ctx, &name, &staleness));
    }
    Ok(SpecReport::Shown {
        name: spec.name.clone(),
        feature: spec.fm.feature.clone(),
        code: spec.fm.code.clone(),
        rules: spec.rules.clone(),
        staleness,
        next,
    })
}

/// The one-command fix for each staleness verdict, shared with `features`.
pub fn stale_fix(ctx: &Ctx, name: &SpecName, s: &Staleness) -> String {
    match s {
        Staleness::Stale { .. } => {
            format!("{} features --confirm {name} --why \"...\"", ctx.invoked_as)
        }
        Staleness::DeadGlobs { .. } => format!("{} doctor", ctx.invoked_as),
        Staleness::NeverScanned => format!("{} scan", ctx.invoked_as),
        Staleness::Ok => format!("{} features", ctx.invoked_as),
    }
}

fn grep(ctx: &Ctx, pattern: &str) -> Result<SpecReport> {
    let snap = ctx.snapshot()?;
    let needle = pattern.to_lowercase();
    let mut hits = Vec::new();
    for spec in snap.specs.values() {
        for r in &spec.rules {
            if r.text.to_lowercase().contains(&needle) || r.anchor.to_lowercase().contains(&needle)
            {
                hits.push(GrepHit {
                    spec: spec.name.clone(),
                    anchor: r.anchor.clone(),
                    text: r.text.clone(),
                    line: r.line,
                });
            }
        }
    }
    Ok(SpecReport::Grepped {
        pattern: pattern.to_string(),
        hits,
    })
}

fn rel(ctx: &Ctx, p: &std::path::Path) -> String {
    p.strip_prefix(ctx.repo.primary_root())
        .unwrap_or(p)
        .display()
        .to_string()
}

impl Render for SpecReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        match self {
            SpecReport::Created { name, path, next } => {
                Line::new('▸', format!("spec {name} created"))
                    .dim(format!("· {path}"))
                    .write(w, st)?;
                for n in next {
                    writeln!(
                        w,
                        "  {} {}",
                        glyph::FIX,
                        crate::out::paint(n, Color::Cyan, st.color)
                    )?;
                }
            }
            SpecReport::Shown {
                name,
                feature,
                code,
                rules,
                staleness,
                next,
            } => {
                writeln!(
                    w,
                    " {} — {feature}",
                    crate::out::paint(name.as_str(), Color::Bold, st.color)
                )?;
                if !code.is_empty() {
                    writeln!(
                        w,
                        "   {}",
                        crate::out::paint(
                            &format!("code {}", code.join(" ")),
                            Color::Dim,
                            st.color
                        )
                    )?;
                }
                for r in rules {
                    write!(w, "  [{}] {}", r.anchor, r.text)?;
                    for p in &r.provenance {
                        write!(w, " {{{p}}}")?;
                    }
                    writeln!(w)?;
                }
                if rules.is_empty() {
                    writeln!(w, "  (no rules yet)")?;
                }
                let ok = matches!(staleness, Staleness::Ok);
                let mut line = Line::new(
                    if ok { glyph::OK } else { '⚠' },
                    project::fresh_cell(staleness),
                );
                // A healthy spec is not owed a verb, so it does not get handed one:
                // invariant 9 is "every REFUSAL names its fix", not "every line nags".
                if !ok {
                    if let Some(n) = next.first() {
                        line = line.fix(n);
                    }
                }
                line.write(w, st)?;
            }
            SpecReport::Grepped { pattern, hits } => {
                for h in hits {
                    Line::new('·', format!("[{}] {}", h.anchor, h.text))
                        .id(&h.spec)
                        .write(w, st)?;
                }
                if hits.is_empty() {
                    writeln!(w, " no rule mentions {pattern:?}")?;
                }
            }
        }
        Ok(())
    }
}
