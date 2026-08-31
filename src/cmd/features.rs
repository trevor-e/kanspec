//! `features [--stale] [--confirm <spec> --why "…"]`.
//!
//! `--confirm` is the human's "no behaviour change" attestation. It is stored as
//! `stale_ack: {sha, at, by, why}` in the **spec's frontmatter** — git-tracked, so it
//! survives `rm -rf cache/` — and never as a counter reset, because a counter in a
//! disposable cache silently resets to zero on wipe and UNDER-fires the tripwire (D-10).
//!
//! Owner: **S6**.

// Wave-0 skeleton. The bodies below are `todo!("S6: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S6 when the bodies land.
#![allow(unused_variables, dead_code)]

use serde::Serialize;

use crate::cli::FeaturesArgs;
use crate::ctx::Ctx;
use crate::error::Result;
use crate::ids::SpecName;
use crate::out::{Render, Style};
use crate::project::FeatureRow;

#[derive(Debug, Serialize)]
pub struct FeaturesReport {
    pub rows: Vec<FeatureRow>,
    /// the path the projection was regenerated to
    pub written: Option<String>,
    pub confirmed: Option<SpecName>,
    pub next: Vec<String>,
}

pub fn features(ctx: &Ctx, a: &FeaturesArgs) -> Result<FeaturesReport> {
    todo!("S6: project::feature_rows, filtered by --stale; --confirm transacts the stale_ack into the spec frontmatter and regenerates KANSPEC-FEATURES.md")
}

impl Render for FeaturesReport {
    fn human(&self, w: &mut dyn std::io::Write, st: &Style) -> std::io::Result<()> {
        todo!("S6: the feature table with a staleness dot per row, and the fix for every tripwired spec")
    }
}
