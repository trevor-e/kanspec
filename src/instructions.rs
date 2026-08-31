//! The embedded `docs/` topic printer. Long-form workflow docs ship **inside the binary**
//! and version with it, which is the entire reason they can stay out of CLAUDE.md without
//! rotting.
//!
//! This file is also where the CLAUDE.md snippet lives. That looks like it belongs in
//! `setup.rs`, and it does not: `tests/single_write_path.rs` holds `setup.rs` and
//! `hooks.rs` — the two files that write *outside* the store — to the rule that they may
//! never name a path under the store, and the snippet's own text names
//! `proposals/closed/` in the line that tells agents never to read it. The snippet is
//! documentation; documentation lives here; `setup.rs` asks for it and writes it.
//!
//! Owner: **S7**.

use rust_embed::RustEmbed;
use serde::Serialize;

use crate::error::{KsError, Result};
use crate::{fix, fixes};

/// `build.rs` prints `cargo:rerun-if-changed=docs`, because rust-embed tracks existing
/// FILES but not the DIRECTORY — a newly added topic is otherwise silently absent from a
/// release binary.
#[derive(RustEmbed)]
#[folder = "docs/"]
pub struct Docs;

/// The order `kanspec instructions` lists topics in: the order of the workflow, not the
/// order of the filesystem. Anything in `docs/` that is not named here still appears,
/// after these, alphabetically — so adding a topic is adding a file.
pub const TOPIC_ORDER: &[&str] = &["start", "done", "review", "close", "config"];

#[derive(Debug, Clone, Serialize)]
pub struct Topic {
    pub name: String,
    pub title: String,
}

/// Every embedded topic, in a stable order.
pub fn topics() -> Vec<Topic> {
    let mut names: Vec<String> = Docs::iter()
        .filter_map(|f| f.strip_suffix(".md").map(str::to_string))
        .collect();
    names.sort();
    names.sort_by_key(|n| {
        TOPIC_ORDER
            .iter()
            .position(|t| t == n)
            .unwrap_or(TOPIC_ORDER.len())
    });
    names
        .into_iter()
        .map(|name| {
            let title = body(&name).as_deref().map(title_of).unwrap_or_default();
            Topic { name, title }
        })
        .collect()
}

/// One topic's markdown. Markdown *is* the terminal rendering here: these docs are written
/// as plain prose with fenced blocks, so re-rendering them would only cost fidelity.
pub fn render(topic: &str) -> Result<String> {
    let name = topic.trim().trim_end_matches(".md");
    body(name).ok_or_else(|| {
        let known: Vec<String> = topics().into_iter().map(|t| t.name).collect();
        KsError::not_found(
            "instructions topic",
            name.to_string(),
            fixes![
                fix!("{} instructions", crate::cli::invoked_as()),
                fix!(
                    "{} instructions {}",
                    crate::cli::invoked_as(),
                    known.first().map(String::as_str).unwrap_or("start")
                ),
            ],
        )
    })
}

/// The raw bytes of one topic, or `None` when there is no such topic.
pub fn body(name: &str) -> Option<String> {
    let f = Docs::get(&format!("{name}.md"))?;
    Some(String::from_utf8_lossy(f.data.as_ref()).into_owned())
}

/// The first `# ` heading, which is the one-line title in the topic list.
fn title_of(md: &str) -> String {
    md.lines()
        .find_map(|l| l.strip_prefix("# "))
        .unwrap_or("")
        .trim()
        .to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// The agent contract snippet
// ─────────────────────────────────────────────────────────────────────────────

/// DESIGN.md's agent contract, verbatim. Ten lines of permanent context: everything
/// longer lives behind `kanspec instructions` and versions with the binary.
///
/// Do not "improve" the wording without changing DESIGN.md first — this text is the
/// product's entire behavioural contract with an agent, and it was written to be short
/// enough that nobody deletes it to save tokens.
const AGENT_SNIPPET: &str = r#"## kanspec
This repo tracks work, specs, and standing rules with kanspec. `kanspec prime` is auto-injected
at session start; run it yourself if context feels missing.
- Find work: `kanspec ready --json`. Claim before coding: `kanspec start <id>` (creates branch/worktree).
- Diff ready: `kanspec ship <id> --pr <n>`. Finish: `kanspec done <id>` — it will gate you; answer its flags.
- Never state whether something is merged. Merge state is git-detected; report `kanspec show <id>` output.
- Review feedback is work: `kanspec comments --unresolved --json`; address each item, then
  `kanspec comment resolve <cm-id> --note "what changed"`.
- Standing rules are `kanspec rules` output ONLY. Closed proposals bind nothing — never read
  .kanspec/proposals/closed/. Never edit an accepted decision; propose one with `kanspec decide`.
- Out-of-scope work you uncover (>5 min): `kanspec new "..."` — one command; it auto-links
  discovered_in to your claimed ticket. Park it and keep going; do NOT expand your current ticket.
  Gotcha learned the hard way: `kanspec quirk add "..." --paths <glob>`.
- Change state ONLY via kanspec verbs — never hand-edit frontmatter. Workflow details: `kanspec instructions`.
"#;

/// The snippet as the binary the user actually invoked would spell it. Only the *commands*
/// are rewritten (they are all inside backticks), never the `## kanspec` heading and never
/// the store path — so under the default `kanspec` name the text is byte-identical to
/// DESIGN.md.
pub fn agent_snippet(invoked_as: &str) -> String {
    if invoked_as == "kanspec" {
        return AGENT_SNIPPET.to_string();
    }
    AGENT_SNIPPET.replace("`kanspec ", &format!("`{invoked_as} "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_topic_design_md_names_is_embedded_and_titled() {
        let topics = topics();
        for want in TOPIC_ORDER {
            let t = topics
                .iter()
                .find(|t| t.name == *want)
                .unwrap_or_else(|| panic!("`kanspec instructions {want}` has no doc"));
            assert!(!t.title.is_empty(), "{want} has no `# ` title line");
            let text = render(want).unwrap();
            assert!(
                text.len() > 400,
                "{want} is still a placeholder ({} bytes)",
                text.len()
            );
        }
    }

    #[test]
    fn the_topic_list_is_workflow_order_not_filesystem_order() {
        let names: Vec<String> = topics().into_iter().map(|t| t.name).collect();
        assert_eq!(&names[..TOPIC_ORDER.len()], TOPIC_ORDER);
    }

    #[test]
    fn an_unknown_topic_is_a_typed_refusal_naming_the_list() {
        let e = render("nonsuch").unwrap_err();
        assert_eq!(e.kind(), "not_found");
        assert!(e
            .fixes()
            .iter()
            .any(|f| f.as_str().contains("instructions")));
    }

    #[test]
    fn the_snippet_is_design_mds_text_and_stays_under_the_context_budget() {
        let s = agent_snippet("kanspec");
        assert!(s.starts_with("## kanspec\n"));
        // ~10 lines of permanent context is the promise in DESIGN.md.
        assert!(s.lines().count() <= 18, "{} lines", s.lines().count());
        for must in [
            "kanspec ready --json",
            "kanspec start <id>",
            "Never state whether something is merged",
            "Closed proposals bind nothing",
            "never hand-edit frontmatter",
        ] {
            assert!(s.contains(must), "the snippet lost: {must}");
        }
    }

    #[test]
    fn the_ks_alias_rewrites_commands_and_nothing_else() {
        let s = agent_snippet("ks");
        assert!(s.starts_with("## kanspec\n"), "the heading is a name");
        assert!(s.contains("`ks ready --json`"));
        assert!(
            s.contains(".kanspec/proposals/closed/"),
            "the store path is not a command"
        );
        assert!(!s.contains("`kanspec "));
    }
}
