//! The embedded `docs/` topic printer. Long-form workflow docs ship **inside the binary**
//! and version with it, which is the entire reason they can stay out of CLAUDE.md without
//! rotting.
//!
//! Owner: **S7**.

// Wave-0 skeleton. The bodies below are `todo!("S7: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S7 when the bodies land.
#![allow(unused_variables, dead_code)]

use rust_embed::RustEmbed;
use serde::Serialize;

use crate::error::Result;

/// `build.rs` prints `cargo:rerun-if-changed=docs`, because rust-embed tracks existing
/// FILES but not the DIRECTORY — a newly added topic is otherwise silently absent from a
/// release binary.
#[derive(RustEmbed)]
#[folder = "docs/"]
pub struct Docs;

#[derive(Debug, Clone, Serialize)]
pub struct Topic {
    pub name: String,
    pub title: String,
}

/// Every embedded topic, in a stable order.
pub fn topics() -> Vec<Topic> {
    todo!("S7: Docs::iter(), stripping `.md`, title from the first `# ` line")
}

/// One topic's markdown, rendered for a terminal.
pub fn render(topic: &str) -> Result<String> {
    todo!("S7: Docs::get(topic.md) or a NotFound naming `kanspec instructions` with the topic list")
}
