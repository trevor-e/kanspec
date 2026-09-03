//! `features [--stale] [--confirm <spec> --why "…"]`.
//!
//! `--confirm` is the human's "no behaviour change" attestation. It is stored as
//! `stale_ack: {sha, at, by, why}` in the **spec's frontmatter** — git-tracked, so it
//! survives `rm -rf cache/` — and never as a counter reset, because a counter in a
//! disposable cache silently resets to zero on wipe and UNDER-fires the tripwire (D-10).
//!
//! `derive::staleness` then counts merges from `max(last_edit_at, stale_ack.at)`, so the
//! tripwire resets **because the attestation is newer**, with nobody incrementing or
//! decrementing anything. Sharper still, and the rule the reset actually depends on: an ack
//! that POSTDATES `last_edit_at` **supersedes** the recorded `SpecAnchor::merges_since`
//! outright rather than being subtracted from it — `scan` measures that count from the last
//! edit and from nowhere else, so once the ack is newer the count answers a question the
//! human has already closed. Subtracting instead would leave `--confirm` unable to clear a
//! spec until the ack commit itself reached main and was re-scanned, and a decrement is
//! precisely the counter D-10 forbids.
//!
//! Plain `features` is a **read**: it renders the map live and writes nothing. Keeping the
//! committed `KANSPEC-FEATURES.md` current is the job of the verbs that change a spec, a
//! decision or a quirk — each of which calls `project::regenerate` — and of `scan`, which
//! catches the hand-edits DESIGN.md's workflow makes on an implementation branch. Reading
//! the map must never be the only way to refresh the file a teammate sees on GitHub.
//!
//! Owner: **S6**.

use serde::Serialize;

use crate::cli::FeaturesArgs;
use crate::cmd::ticket::{rel_to, write_next};
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
use crate::{fix, fixes};

#[derive(Debug, Serialize)]
pub struct FeaturesReport {
    pub rows: Vec<FeatureRow>,
    /// the path the projection was regenerated to, `None` when no byte moved
    pub written: Option<String>,
    pub confirmed: Option<SpecName>,
    pub next: Vec<String>,
    /// `--uncovered`: tracked files no spec's `code:` globs claim
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered: Vec<String>,
    /// whether `--uncovered` was the question. An empty `uncovered` means "everything is
    /// claimed" only when it WAS asked; on the ordinary path it just means the field is
    /// unused, and rendering "all clear" for a repo with no specs at all would be a lie.
    #[serde(skip)]
    uncovered_query: bool,
}

pub fn features(ctx: &Ctx, a: &FeaturesArgs) -> Result<FeaturesReport> {
    ctx.require_initialized()?;
    if let Some(raw) = a.confirm.as_deref() {
        return confirm(ctx, raw, a.why.as_deref().unwrap_or_default());
    }
    if let Some(pathspec) = a.uncovered.as_deref() {
        return uncovered(ctx, pathspec);
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
        uncovered: Vec::new(),
        uncovered_query: false,
        rows,
        written: None,
        confirmed: None,
        next,
    })
}

/// `features --uncovered <pathspec>` — the reverse of the dead-glob check.
///
/// `doctor` answers "does this spec's glob match anything". The question it CANNOT answer
/// is the one that actually loses you steering: **does this file match any spec?** A spec
/// with a rotted glob is loud (its rules stop reaching code, and the check fires); a file
/// no spec claims is silent — `prime` injects nothing for it and nothing anywhere says so.
/// New code is uncovered by default, which is exactly when a rule would have helped.
///
/// The pathspec is required rather than defaulted, because only the human knows what
/// counts as source here: defaulting to everything tracked would list the README, the
/// lockfiles and every fixture, and a report that is mostly noise is one nobody reads.
///
/// Implemented as ONE `git ls-files` with every spec glob subtracted as an
/// `:(exclude)` pathspec, so git does the matching with the same semantics the rest of
/// the tool uses — not a second globbing implementation that could disagree with it.
fn uncovered(ctx: &Ctx, pathspec: &str) -> Result<FeaturesReport> {
    let snap = ctx.snapshot()?;
    let mut specs: Vec<crate::git::Pathspec> = vec![crate::git::Pathspec::glob(pathspec)];
    for spec in snap.specs.values() {
        for g in &spec.fm.code {
            specs.push(crate::git::Pathspec::exclude_glob(g));
        }
    }
    let out = ctx
        .git
        .run_ps(&["ls-files", "-z"], &specs)
        .map_err(|e| KsError::internal(anyhow::anyhow!("cannot list tracked files: {e}")))?;
    let files: Vec<String> = out
        .out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();

    Ok(FeaturesReport {
        rows: Vec::new(),
        written: None,
        confirmed: None,
        uncovered_query: true,
        next: if files.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "{} spec new <name> --code \"<glob>\"",
                ctx.invoked_as
            )]
        },
        uncovered: files,
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
    let done = Store::open(ctx).transact(None, &ctx.invocation(), |s, _m| {
        s.spec(&name)?;
        Ok(Plan::of(vec![Op::SetFields {
            entity: EntityRef::Spec(name.clone()),
            sets: vec![(Key::Spec(SpecKey::StaleAck), ack.clone())],
        }]))
    })?;
    // Reported only when a byte actually moved. An attestation lands in the spec's
    // frontmatter, and the committed feature map is a function of `feature:`/`code:`/
    // provenance — so it usually does NOT move, and claiming "regenerated" every time
    // would put a line in front of the human that `git status` then contradicts.
    let written = done
        .regenerated
        .iter()
        .any(|p| p == ctx.layout.features_md())
        .then(|| rel_to(ctx, ctx.layout.features_md()));

    let snap = &done.snapshot;
    Ok(FeaturesReport {
        rows: project::feature_rows(snap)
            .into_iter()
            .filter(|r| r.spec == name)
            .collect(),
        written,
        confirmed: Some(name),
        next: vec![format!("{} features --stale", ctx.invoked_as)],
        uncovered: Vec::new(),
        uncovered_query: false,
    })
}

/// One glyph per verdict, so the terminal table, the board's feature strip and the
/// `spec show` footer can be read the same way at a glance. All three are LIVE reads; the
/// committed projection carries no freshness at all (see `project`'s module header).
fn dot(s: &Staleness) -> char {
    match s {
        Staleness::Ok => glyph::OK,
        Staleness::Stale { .. } | Staleness::DeadGlobs { .. } => '⚠',
        Staleness::NeverScanned => '·',
    }
}

impl Render for FeaturesReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        // `--uncovered` answers a different question from the feature map, so it prints a
        // different thing and stops rather than also dumping the table.
        if self.uncovered_query {
            if self.uncovered.is_empty() {
                Line::new(glyph::OK, "every tracked file there is claimed by a spec")
                    .write(w, st)?;
                return Ok(());
            }
            Line::new(
                '⚠',
                format!(
                    "{} tracked file(s) no spec claims — `prime` injects nothing for them",
                    self.uncovered.len()
                ),
            )
            .write(w, st)?;
            for f in self.uncovered.iter().take(40) {
                writeln!(w, "   {f}")?;
            }
            if self.uncovered.len() > 40 {
                writeln!(w, "   … {} more", self.uncovered.len() - 40)?;
            }
            for n in &self.next {
                Line::new(glyph::FIX, "next").fix(n.as_str()).write(w, st)?;
            }
            return Ok(());
        }
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
        write_next(w, st, &self.next)
    }
}
