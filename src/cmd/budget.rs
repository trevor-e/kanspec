//! The context budget under every `[tN]` at review (p-67f0 c1).
//!
//! Tickets are cut at proposal time, and nothing used to look at the cut until the work
//! had outgrown a session. This is the look: for each ticket item, what the agent will
//! have to READ before writing a line — the standing rules in scope for its capability
//! (the figure `rules --budget` prints, from the generator `prime` spends) and the code
//! under the implementing spec's `code:` globs as files and lines, narrowed to the paths
//! a `[cN]` names in backticks when it names any.
//!
//! Every number here is derived from the store and the tree at read time and never
//! written into the proposal. Nothing about the WORK is estimated: the line says what the
//! ticket reads, and says `surface unknown` when a spec names no code rather than guessing.

use serde::Serialize;

use crate::ctx::Ctx;
use crate::error::Result;
use crate::ids::SpecName;
use crate::model::{Proposal, Snapshot};
use crate::rulesdoc::{self, SpecBudget};

/// The code under a ticket's globs — what it will have to read besides the rules.
#[derive(Debug, Clone, Serialize)]
pub struct Surface {
    pub files: usize,
    pub lines: usize,
}

/// One `[tN]`'s budget line, as the page, `review`, `approve` and `--json` all carry it.
#[derive(Debug, Clone, Serialize)]
pub struct ItemBudget {
    /// `t1`
    pub item: String,
    /// the capability the ticket will be minted under, as `approve` resolves it
    pub spec: Option<SpecName>,
    /// the reading list — `None` when the spec is not in the store
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads: Option<SpecBudget>,
    /// the code surface — `None` when the spec names no `code:` globs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<Surface>,
    /// the backticked paths from the implemented `[cN]` bullets that narrowed the surface
    pub narrowed_to: Vec<String>,
    /// the `· S ·` estimate the author wrote, if any
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// the sub-bullets under the item — the steps the ticket will be minted with
    pub steps: usize,
    /// the one line every surface prints, so they cannot drift
    pub line: String,
}

/// The budget line for every `[tN]` of `p`, in item order.
pub fn for_proposal(ctx: &Ctx, s: &Snapshot, p: &Proposal) -> Result<Vec<ItemBudget>> {
    let mut out = Vec::new();
    for i in &p.items {
        if i.id.kind != crate::ids::ItemKind::Ticket {
            continue;
        }
        let item = format!("{}{}", i.id.kind.letter(), i.id.n);
        let bullet = super::proposal::split_ticket_bullet(&i.text);
        let spec = super::proposal::ticket_spec(p, &bullet).and_then(|n| SpecName::parse(&n).ok());
        // The paths the ticket's changes name: `- [c1] auth: lockout in `src/auth/lockout.rs``.
        let named: Vec<String> = bullet
            .changes
            .iter()
            .filter_map(|tag| {
                p.items
                    .iter()
                    .find(|c| format!("{}{}", c.id.kind.letter(), c.id.n) == *tag)
            })
            .flat_map(|c| paths_named(&c.text))
            .collect();
        let mut b = for_item(ctx, s, item, spec, &named)?;
        b.size = bullet.size.clone();
        b.steps = i.steps.len();
        if b.steps > 0 {
            b.line.push_str(&format!(
                " · {} step{}",
                b.steps,
                if b.steps == 1 { "" } else { "s" }
            ));
        }
        out.push(b);
    }
    Ok(out)
}

fn for_item(
    ctx: &Ctx,
    s: &Snapshot,
    item: String,
    spec: Option<SpecName>,
    named: &[String],
) -> Result<ItemBudget> {
    let Some(name) = spec else {
        return Ok(ItemBudget {
            line: "no spec — no budget; name one with (spec: x)".to_string(),
            item,
            spec: None,
            reads: None,
            surface: None,
            narrowed_to: Vec::new(),
            size: None,
            steps: 0,
        });
    };
    if !s.specs.contains_key(&name) {
        return Ok(ItemBudget {
            line: format!("spec {name} is not in the store — no budget"),
            item,
            spec: Some(name),
            reads: None,
            surface: None,
            narrowed_to: Vec::new(),
            size: None,
            steps: 0,
        });
    }
    let files = super::rules::spec_files(ctx, s, &name)?;
    let reads = rulesdoc::spec_budget(s, &name, &files)?;
    let mut line = reads.reads_phrase();
    let (surface, narrowed_to) = if !reads.scoped {
        line.push_str(" · surface unknown — spec names no code");
        (None, Vec::new())
    } else {
        let (chosen, narrowed) = narrow(&files, named);
        let lines: usize = chosen
            .iter()
            .map(|f| count_lines(&ctx.git.root().join(f)))
            .sum();
        line.push_str(&format!(
            " · surface {} file{}, {} line{}",
            chosen.len(),
            if chosen.len() == 1 { "" } else { "s" },
            lines_short(lines),
            if lines == 1 { "" } else { "s" },
        ));
        if !narrowed.is_empty() {
            line.push_str(&format!(" under {}", narrowed.join(", ")));
        }
        (
            Some(Surface {
                files: chosen.len(),
                lines,
            }),
            narrowed,
        )
    };
    Ok(ItemBudget {
        item,
        spec: Some(name),
        reads: Some(reads),
        surface,
        narrowed_to,
        size: None,
        steps: 0,
        line,
    })
}

/// The backticked tokens in a change bullet that look like paths: a slash somewhere, or
/// a file extension, and no whitespace. `` `src/auth/lockout.rs` `` and `` `src/auth/**` ``
/// qualify; `` `done` `` and `` `--json` `` do not.
pub fn paths_named(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else { break };
        let tok = &after[..end];
        if looks_like_path(tok) && !out.iter().any(|o| o == tok) {
            out.push(tok.to_string());
        }
        rest = &after[end + 1..];
    }
    out
}

fn looks_like_path(t: &str) -> bool {
    !t.is_empty()
        && !t.contains(char::is_whitespace)
        && !t.starts_with('-')
        && (t.contains('/')
            || t.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric())
            }))
}

/// The files under the spec's globs that the named paths reach — as the file itself, as
/// a directory prefix, or as a glob. When nothing named reaches anything, the surface is
/// the whole spec: a change that names a path outside the spec's globs narrows nothing,
/// which is also worth seeing.
pub fn narrow(files: &[String], named: &[String]) -> (Vec<String>, Vec<String>) {
    if named.is_empty() {
        return (files.to_vec(), Vec::new());
    }
    let mut chosen: Vec<String> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for n in named {
        let n = n.trim_start_matches("./").trim_end_matches('/');
        let matcher = n
            .contains(['*', '?', '[', '{'])
            .then(|| {
                globset::GlobBuilder::new(n)
                    .literal_separator(true)
                    .build()
                    .ok()
                    .map(|g| g.compile_matcher())
            })
            .flatten();
        let mut hit = false;
        for f in files {
            let reached = f == n
                || f.starts_with(&format!("{n}/"))
                || matcher.as_ref().is_some_and(|m| m.is_match(f));
            if reached {
                hit = true;
                if !chosen.contains(f) {
                    chosen.push(f.clone());
                }
            }
        }
        if hit {
            used.push(n.to_string());
        }
    }
    if chosen.is_empty() {
        (files.to_vec(), Vec::new())
    } else {
        (chosen, used)
    }
}

/// Lines in a tracked text file; a file that cannot be read as text counts for none, so
/// a binary fixture never inflates the surface.
fn count_lines(p: &std::path::Path) -> usize {
    match std::fs::read(p) {
        Ok(bytes) if !bytes.contains(&0) => bytes.iter().filter(|b| **b == b'\n').count(),
        _ => 0,
    }
}

/// `2.1k` — lines as a human reads them on a budget line.
fn lines_short(n: usize) -> String {
    if n < 1000 {
        n.to_string()
    } else {
        format!("{:.1}k", n as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backticked_paths_are_named_and_flags_are_not() {
        let got = paths_named(
            "auth: lockout in `src/auth/lockout.rs`, gated by `--json`; see `done` and `src/auth/**` and `Cargo.toml`",
        );
        assert_eq!(
            got,
            vec!["src/auth/lockout.rs", "src/auth/**", "Cargo.toml"]
        );
        assert!(paths_named("no ticks here").is_empty());
        assert!(paths_named("an `unclosed tick").is_empty());
    }

    fn files() -> Vec<String> {
        [
            "src/auth/lockout.rs",
            "src/auth/session/mw.rs",
            "src/auth/mod.rs",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn a_named_file_directory_or_glob_narrows_the_surface() {
        let (c, n) = narrow(&files(), &["src/auth/lockout.rs".into()]);
        assert_eq!(c, vec!["src/auth/lockout.rs"]);
        assert_eq!(n, vec!["src/auth/lockout.rs"]);

        let (c, _) = narrow(&files(), &["src/auth/session".into()]);
        assert_eq!(c, vec!["src/auth/session/mw.rs"]);

        let (c, _) = narrow(&files(), &["src/auth/*.rs".into()]);
        assert_eq!(c, vec!["src/auth/lockout.rs", "src/auth/mod.rs"]);
    }

    #[test]
    fn a_path_outside_the_globs_narrows_nothing_and_says_so() {
        let (c, n) = narrow(&files(), &["src/billing/charge.rs".into()]);
        assert_eq!(c, files(), "the whole spec: nothing named reached it");
        assert!(n.is_empty());
        let (c, n) = narrow(&files(), &[]);
        assert_eq!(c, files());
        assert!(n.is_empty());
    }

    #[test]
    fn lines_read_short() {
        assert_eq!(lines_short(999), "999");
        assert_eq!(lines_short(2140), "2.1k");
    }
}
