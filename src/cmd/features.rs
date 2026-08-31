//! `features [--stale] [--confirm <spec> --why "…"]`.
//!
//! `--confirm` is the human's "no behaviour change" attestation. It is stored as
//! `stale_ack: {sha, at, by, why}` in the **spec's frontmatter** — git-tracked, so it
//! survives `rm -rf cache/` — and never as a counter reset, because a counter in a
//! disposable cache silently resets to zero on wipe and UNDER-fires the tripwire (D-10).
//!
//! `derive::staleness` then counts merges from `max(last_edit_at, stale_ack.at)`, so the
//! tripwire resets **because the attestation is newer**, with nobody incrementing or
//! decrementing anything.
//!
//! Owner: **S6**.

use serde::Serialize;

use crate::cli::FeaturesArgs;
use crate::ctx::Ctx;
use crate::derive::Staleness;
use crate::error::{KsError, Result};
use crate::fm::Yv;
use crate::ids::SpecName;
use crate::keys::{Key, SpecKey};
use crate::out::{glyph, Color, Line, Render, Style, Table};
use crate::plan::{EntityRef, Op, Plan};
use crate::project::{self, FeatureRow};
use crate::store::Store;
use crate::transitions::Verb;
use crate::{fix, fixes};

#[derive(Debug, Serialize)]
pub struct FeaturesReport {
    pub rows: Vec<FeatureRow>,
    /// the path the projection was regenerated to
    pub written: Option<String>,
    pub confirmed: Option<SpecName>,
    pub next: Vec<String>,
}

pub fn features(ctx: &Ctx, a: &FeaturesArgs) -> Result<FeaturesReport> {
    ctx.require_initialized()?;
    if let Some(raw) = a.confirm.as_deref() {
        return confirm(ctx, raw, a.why.as_deref().unwrap_or_default());
    }

    let snap = ctx.snapshot()?;
    let mut rows = project::feature_rows(&snap);
    if a.stale {
        rows.retain(|r| !matches!(r.staleness, Staleness::Ok));
    }
    let next = rows
        .iter()
        .filter(|r| !matches!(r.staleness, Staleness::Ok))
        .take(3)
        .map(|r| crate::cmd::spec::stale_fix(ctx, &r.spec, &r.staleness))
        .collect();

    Ok(FeaturesReport {
        rows,
        written: None,
        confirmed: None,
        next,
    })
}

/// The tripwire's one-key resolution: "I looked, and nothing about this capability's
/// behaviour changed."
fn confirm(ctx: &Ctx, raw: &str, why: &str) -> Result<FeaturesReport> {
    let name = SpecName::parse(raw)?;
    let why = why.trim().to_string();
    if why.is_empty() {
        return Err(KsError::invalid(
            "an attestation without a reason is not an attestation",
            fixes![fix!(
                "{} features --confirm {name} --why \"...\"",
                ctx.invoked_as
            )],
        ));
    }
    let snap = ctx.snapshot()?;
    snap.spec(&name)?;

    // Every subprocess happens BEFORE the lock (§2.16). The SHA anchors the attestation to
    // the code that was actually looked at: `last_edit_sha` when a scan has recorded one,
    // otherwise whatever HEAD is right now.
    let sha = match snap
        .git
        .specs
        .get(&name)
        .and_then(|x| x.last_edit_sha.clone())
    {
        Some(s) => s,
        None => ctx
            .git
            .head_sha("HEAD")
            .map(|h| h.sha().as_str().to_string())
            .unwrap_or_default(),
    };

    let ack = Yv::Map(vec![
        ("sha".into(), Yv::s(sha)),
        (
            "at".into(),
            Yv::s(ctx.now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        ),
        ("by".into(), Yv::s(ctx.actor.label())),
        ("why".into(), Yv::s(&why)),
    ]);
    Store::open(ctx).transact(Verb::Confirm, &ctx.invocation(), |s, _m| {
        s.spec(&name)?;
        Ok(Plan::of(vec![Op::SetFields {
            entity: EntityRef::Spec(name.clone()),
            sets: vec![(Key::Spec(SpecKey::StaleAck), ack.clone())],
        }]))
    })?;
    project::regenerate(ctx)?;

    let snap = ctx.snapshot()?;
    Ok(FeaturesReport {
        rows: project::feature_rows(&snap)
            .into_iter()
            .filter(|r| r.spec == name)
            .collect(),
        written: Some(rel(ctx, ctx.layout.features_md())),
        confirmed: Some(name),
        next: vec![format!("{} features --stale", ctx.invoked_as)],
    })
}

fn rel(ctx: &Ctx, p: &std::path::Path) -> String {
    p.strip_prefix(ctx.repo.primary_root())
        .unwrap_or(p)
        .display()
        .to_string()
}

/// One glyph per verdict, so the terminal table, the board's feature strip and the
/// projection's `Fresh?` column can be read the same way at a glance.
fn dot(s: &Staleness) -> char {
    match s {
        Staleness::Ok => glyph::OK,
        Staleness::Stale { .. } => '⚠',
        Staleness::DeadGlobs { .. } => '⚠',
        Staleness::NeverScanned => '·',
    }
}

impl Render for FeaturesReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        if let Some(name) = &self.confirmed {
            Line::new(glyph::OK, format!("{name}: no behaviour change recorded"))
                .dim("· stale_ack written to the spec frontmatter")
                .write(w, st)?;
            if let Some(p) = &self.written {
                Line::new('▸', format!("{p} regenerated")).write(w, st)?;
            }
            return Ok(());
        }

        if self.rows.is_empty() {
            writeln!(
                w,
                " {} no specs are stale",
                crate::out::paint(&glyph::OK.to_string(), Color::Green, st.color)
            )?;
            return Ok(());
        }

        let mut t = Table::new(&["", "Feature", "Spec", "Code", "Shipped", "Fresh?"], st);
        for r in &self.rows {
            t.add_row(vec![
                dot(&r.staleness).to_string(),
                r.feature.clone(),
                r.spec.to_string(),
                r.code.join(" "),
                r.last_shipped.clone().unwrap_or_else(|| "—".into()),
                project::fresh_cell(&r.staleness),
            ]);
        }
        writeln!(w, "{t}")?;
        for n in &self.next {
            writeln!(
                w,
                "  {} {}",
                glyph::FIX,
                crate::out::paint(n, Color::Cyan, st.color)
            )?;
        }
        Ok(())
    }
}
