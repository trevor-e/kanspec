//! `setup claude|cursor|codex` — the agent snippet plus the agent-side hooks, installed
//! and **uninstalled symmetrically**.
//!
//! Permanent context cost is ~10 lines; the long-form docs live behind
//! `kanspec instructions` and version with the binary, so they never rot in CLAUDE.md.
//! (The snippet's text itself lives in `instructions.rs` — see the note there.)
//!
//! Owner: **S7**. Writes only outside the store (CLAUDE.md, agent settings), never inside
//! it, and — like `hooks.rs` — it *plans* those writes and hands them to
//! `cmd::init::apply`, which is the one function in this slice that moves a byte.
//!
//! **What "symmetrically" has to mean.** A settings file is the user's, not ours. Install
//! merges into whatever is there and leaves every foreign hook alone; `--remove` takes out
//! exactly the entries kanspec would install and nothing else, restores any git hook
//! kanspec displaced, and leaves a settings file that held only our hooks gone rather than
//! empty.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::cli::Agent;
use crate::ctx::Ctx;
use crate::error::{KsError, Result};
use crate::hooks::{read, Edit};
use crate::{fix, fixes};

/// The exact snippet DESIGN.md specifies. Delimited so `--remove` can excise precisely
/// what was added, leaving the rest of a hand-maintained CLAUDE.md untouched.
pub const SNIPPET_BEGIN: &str = "<!-- kanspec:begin -->";
pub const SNIPPET_END: &str = "<!-- kanspec:end -->";

/// The agent-side hooks, per DESIGN.md's table. `Stop` is installed but config-gated by
/// `[hooks] landcheck` (D-14).
pub const AGENT_HOOKS: &[(&str, &str, &str)] = &[
    (
        "SessionStart",
        "kanspec prime",
        "inject ~1.5k tokens of live state",
    ),
    ("PreCompact", "kanspec prime", "re-inject after compaction"),
    (
        "PostToolUse",
        "kanspec quirks --touch",
        "warn at the moment of touching a landmine",
    ),
    (
        "Stop",
        "kanspec landcheck",
        "block a session ending with inconsistent state (opt-in)",
    ),
];

/// The tool names whose writes can step on a landmine. Anything that edits a file.
const EDIT_TOOLS: &str = "Edit|Write|MultiEdit|NotebookEdit";

#[derive(Debug, Clone, Serialize)]
pub struct SetupChange {
    pub path: PathBuf,
    pub what: &'static str,
    pub changed: bool,
}

/// Where an agent keeps its always-on context, and its hook registry if it has one.
///
/// Only Claude Code has a hook registry kanspec knows how to write. Cursor and Codex get
/// the context snippet and the git hooks; `settings` is `None` for them rather than a path
/// nothing ever writes, so the report cannot claim a file it never touched.
pub struct AgentFiles {
    pub context: PathBuf,
    pub settings: Option<PathBuf>,
}

pub fn agent_files(ctx: &Ctx, agent: Agent) -> AgentFiles {
    let root = ctx.repo.primary_root().to_path_buf();
    match agent {
        Agent::Claude => AgentFiles {
            context: root.join("CLAUDE.md"),
            settings: Some(root.join(".claude").join("settings.json")),
        },
        Agent::Cursor => AgentFiles {
            context: root.join(".cursorrules"),
            settings: None,
        },
        Agent::Codex => AgentFiles {
            context: root.join("AGENTS.md"),
            settings: None,
        },
    }
}

pub fn agent_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Cursor => "cursor",
        Agent::Codex => "codex",
    }
}

/// The markdown snippet installed into the agent's always-on context file.
pub fn snippet(invoked_as: &str) -> String {
    format!(
        "{SNIPPET_BEGIN}\n{}{SNIPPET_END}\n",
        crate::instructions::agent_snippet(invoked_as)
    )
}

/// Install — or, with `remove`, symmetrically uninstall — the snippet, the agent hooks and
/// the git hooks. One function for both directions, because the two must agree on exactly
/// which files they touch or `--remove` cannot be the inverse of `setup`.
///
/// Paths are reported repo-relative: every other surface in the product does, and an
/// absolute tempdir path is unreadable in a terminal and unstable in a snapshot.
pub fn run(ctx: &Ctx, agent: Agent, remove: bool) -> Result<Vec<SetupChange>> {
    let files = agent_files(ctx, agent);
    let mut edits = Vec::new();
    let mut changes = Vec::new();

    let before = read(&files.context);
    let after = if remove {
        before.as_deref().map(excise_snippet)
    } else {
        insert_snippet(&before, &snippet(ctx.invoked_as))
    };
    changes.push(plan_text(
        &mut edits,
        files.context,
        "agent context",
        before,
        after,
    ));

    if let Some(path) = files.settings {
        let before = read(&path);
        let after = if remove {
            before
                .as_deref()
                .map(|b| strip_settings(b, ctx.invoked_as))
                .transpose()?
                .flatten()
        } else {
            let entries = hook_entries(ctx.invoked_as, ctx.cfg.hooks.landcheck);
            Some(merge_settings(before.as_deref(), &entries)?)
        };
        // A settings directory that held nothing but our file goes with it. `PruneDir`
        // refuses a populated directory, so a user's own `.claude/` is safe.
        let prune = (remove && after.is_none())
            .then(|| path.parent().map(Path::to_path_buf))
            .flatten();
        changes.push(plan_text(&mut edits, path, "agent hooks", before, after));
        if let Some(p) = prune {
            edits.push(Edit::PruneDir { path: p });
        }
    }

    crate::cmd::init::apply(&edits)?;
    // The git hooks are part of "setup installs everything": merge badges and the
    // squash-surviving trailer are what make the agent contract's "never state whether
    // something is merged" answerable at all.
    let hooks = if remove {
        crate::hooks::remove(ctx)?
    } else {
        crate::hooks::install(ctx, false)?
    };
    changes.extend(hooks.into_iter().map(|h| SetupChange {
        path: h.path,
        what: "git hook",
        changed: h.action.changed(),
    }));

    let root = ctx.repo.primary_root();
    for c in &mut changes {
        c.path = crate::hooks::relative_to(root, &c.path);
    }
    Ok(changes)
}

/// Turn a before/after pair into an [`Edit`] — or into nothing at all when they agree,
/// which is what makes running `setup` twice a no-op. `after == None` means the file
/// should not exist: that is how a settings file kanspec created goes away again.
fn plan_text(
    edits: &mut Vec<Edit>,
    path: PathBuf,
    what: &'static str,
    before: Option<String>,
    after: Option<String>,
) -> SetupChange {
    let changed = before != after;
    if changed {
        edits.push(match after {
            Some(contents) => Edit::Write {
                path: path.clone(),
                contents,
                exec: false,
            },
            None => Edit::Remove { path: path.clone() },
        });
    }
    SetupChange {
        path,
        what,
        changed,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The context snippet — a delimited block, so removal is byte-exact
// ─────────────────────────────────────────────────────────────────────────────

/// Insert or refresh the delimited block. An existing block is replaced in place, so the
/// snippet can be upgraded without moving it and without touching a line around it.
fn insert_snippet(existing: &Option<String>, block: &str) -> Option<String> {
    let Some(text) = existing.as_deref() else {
        return Some(block.to_string());
    };
    if let Some((start, end)) = block_span(text) {
        return Some(format!("{}{block}{}", &text[..start], &text[end..]));
    }
    // One blank line between the user's last line and ours, and no more — so removal has
    // exactly one separator to take back out.
    let sep = if text.is_empty() || text.ends_with("\n\n") {
        ""
    } else if text.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    Some(format!("{text}{sep}{block}"))
}

/// The exact inverse of [`insert_snippet`] for a block that sits at the end of the file:
/// the blank line that separated it goes too, so a CLAUDE.md that ended with a newline
/// comes back byte-identical.
fn excise_snippet(text: &str) -> String {
    let Some((start, end)) = block_span(text) else {
        return text.to_string();
    };
    let mut before = text[..start].to_string();
    let after = &text[end..];
    if after.is_empty() && before.ends_with("\n\n") {
        before.pop();
    }
    format!("{before}{after}")
}

/// Byte range of the whole delimited block, including the newline that ends it.
fn block_span(text: &str) -> Option<(usize, usize)> {
    let start = text.find(SNIPPET_BEGIN)?;
    let end_at = text[start..].find(SNIPPET_END)? + start + SNIPPET_END.len();
    let end = match text[end_at..].find('\n') {
        Some(nl) => end_at + nl + 1,
        None => end_at,
    };
    Some((start, end))
}

// ─────────────────────────────────────────────────────────────────────────────
// The hook registry — merged, never overwritten
// ─────────────────────────────────────────────────────────────────────────────

/// One row per hook kanspec installs: `(event, matcher, command)`.
///
/// The Stop hook is the one that can *block* a session, so it is installed only when
/// `[hooks] landcheck = true` (D-14). Off by default is not timidity: a Stop hook that
/// refuses to let a session end is the single most disruptive thing this tool can do, and
/// it should be a thing you turned on.
pub fn hook_entries(invoked_as: &str, landcheck: bool) -> Vec<(&'static str, String, String)> {
    let ks = invoked_as;
    let mut v = vec![
        ("SessionStart", String::new(), format!("{ks} prime")),
        ("PreCompact", String::new(), format!("{ks} prime")),
        (
            "PostToolUse",
            EDIT_TOOLS.to_string(),
            // The hook payload arrives as JSON on stdin, so the edited path has to be
            // read out of it. No jq, no warning — never a failed tool call.
            format!(
                "f=$(jq -r '.tool_input.file_path // empty' 2>/dev/null); \
                 [ -n \"$f\" ] && {ks} quirks --touch \"$f\"; exit 0"
            ),
        ),
    ];
    if landcheck {
        v.push(("Stop", String::new(), format!("{ks} landcheck")));
    }
    v
}

/// The events kanspec is allowed to prune on `--remove`. Anything outside this set is the
/// user's, even when it is empty.
fn is_our_event(event: &str) -> bool {
    AGENT_HOOKS.iter().any(|(e, _, _)| *e == event)
}

/// Is this a hook entry kanspec owns? Matched on the command, because a settings file has
/// nowhere to hang a marker that the agent would not have to understand. Both shipped bin
/// names count, and so does whatever name this process was invoked under — otherwise a
/// symlinked binary would stack a fresh entry on every `setup` and `--remove` would strip
/// none of them.
fn is_kanspec_command(cmd: &str, invoked_as: &str) -> bool {
    ["prime", "quirks --touch", "landcheck"].iter().any(|verb| {
        [invoked_as, "kanspec", "ks"]
            .iter()
            .any(|bin| word(cmd, bin, verb))
    })
}

/// `bin verb` at a word boundary, so `works prime` and `/opt/ks-tools prime` do not count.
fn word(cmd: &str, bin: &str, verb: &str) -> bool {
    let needle = format!("{bin} {verb}");
    cmd.match_indices(&needle).any(|(i, _)| {
        cmd[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && !"-_./".contains(c))
    })
}

/// Merge our entries into whatever settings the user already has.
///
/// Everything foreign is preserved: other events, other matchers inside our events, other
/// commands inside our matcher group, and every key outside `hooks`. The only thing that
/// is ever rewritten is an entry that is already ours.
pub fn merge_settings(
    existing: Option<&str>,
    entries: &[(&str, String, String)],
) -> Result<String> {
    let mut root = parse(existing)?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| bad("`hooks` is not an object"))?;

    for (event, matcher, command) in entries {
        let list = hooks
            .entry(event.to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| bad(&format!("`hooks.{event}` is not an array")))?;

        let group = match list.iter().position(|g| matcher_of(g) == matcher.as_str()) {
            Some(i) => &mut list[i],
            None => {
                let mut g = Map::new();
                if !matcher.is_empty() {
                    g.insert("matcher".into(), json!(matcher));
                }
                g.insert("hooks".into(), json!([]));
                list.push(Value::Object(g));
                list.last_mut().expect("just pushed")
            }
        };
        let inner = group
            .as_object_mut()
            .and_then(|g| g.entry("hooks").or_insert_with(|| json!([])).as_array_mut())
            .ok_or_else(|| bad(&format!("`hooks.{event}[].hooks` is not an array")))?;

        // The entry being merged is spelled with the binary that ran `setup`, so its own
        // command word is the third name a foreign entry might carry.
        let invoked_as = command.split_whitespace().next().unwrap_or("kanspec");
        match inner
            .iter()
            .position(|h| is_kanspec_command(command_of(h), invoked_as))
        {
            Some(i) => inner[i] = json!({ "type": "command", "command": command }),
            None => inner.push(json!({ "type": "command", "command": command })),
        }
    }
    render(&root)
}

/// Take out exactly what [`merge_settings`] put in. `None` means the file held nothing but
/// kanspec's hooks and should go away rather than linger as `{}`.
pub fn strip_settings(existing: &str, invoked_as: &str) -> Result<Option<String>> {
    let mut root = parse(Some(existing))?;
    if let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) {
        for (event, list) in hooks.iter_mut() {
            if !is_our_event(event) {
                continue;
            }
            let Some(groups) = list.as_array_mut() else {
                continue;
            };
            groups.retain_mut(|g| {
                let Some(inner) = g.get_mut("hooks").and_then(Value::as_array_mut) else {
                    return true;
                };
                let had = inner.len();
                inner.retain(|h| !is_kanspec_command(command_of(h), invoked_as));
                // A group we emptied is a group we created. One the user left empty is
                // theirs, and stays.
                !(inner.is_empty() && had > 0)
            });
        }
        hooks.retain(|k, v| !(is_our_event(k) && v.as_array().is_some_and(|a| a.is_empty())));
    }
    if root
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        root.remove("hooks");
    }
    if root.is_empty() {
        return Ok(None);
    }
    render(&root).map(Some)
}

fn matcher_of(group: &Value) -> &str {
    group.get("matcher").and_then(Value::as_str).unwrap_or("")
}

fn command_of(hook: &Value) -> &str {
    hook.get("command").and_then(Value::as_str).unwrap_or("")
}

fn parse(existing: Option<&str>) -> Result<Map<String, Value>> {
    match existing.map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(Map::new()),
        Some(t) => match serde_json::from_str::<Value>(t) {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) => Err(bad("the settings file is not a JSON object")),
            Err(e) => Err(bad(&format!("the settings file is not valid JSON: {e}"))),
        },
    }
}

fn render(root: &Map<String, Value>) -> Result<String> {
    let mut s = serde_json::to_string_pretty(root).map_err(KsError::internal)?;
    s.push('\n');
    Ok(s)
}

fn bad(what: &str) -> KsError {
    KsError::invalid(
        format!("cannot merge agent hooks: {what}"),
        fixes![
            fix!("fix the settings file by hand, then re-run setup"),
            fix!("kanspec doctor"),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `<bin> …` command the snippet prescribes, as a subcommand path
    /// (`["quirk", "add"]`).
    ///
    /// Backticked spans only. The snippet also names `.kanspec/proposals/closed/`, which is a
    /// store path rather than a verb — and it sits *outside* backticks for exactly that
    /// reason, which is the property `instructions.rs` already pins.
    ///
    /// A span ends at its first token that is not a bare word — `<id>`, `"…"`, `--json` — so
    /// `ship <id> --pr <n>` yields `["ship"]` while `quirk add "…" --paths <glob>` yields
    /// `["quirk", "add"]`. A trailing bare word that turns out to be a positional rather than
    /// a subcommand (`instructions start`) is dropped by the caller, which is the half that
    /// can see the tree.
    fn commands_named_in(snippet: &str, bin: &str) -> Vec<Vec<String>> {
        fn is_verb_word(t: &str) -> bool {
            !t.is_empty()
                && !t.starts_with('-')
                && t.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        }
        let mut out: Vec<Vec<String>> = Vec::new();
        // Odd spans are the insides of the backtick pairs.
        for span in snippet.split('`').skip(1).step_by(2) {
            let mut words = span.split_whitespace();
            if words.next() != Some(bin) {
                continue;
            }
            let path: Vec<String> = words
                .take_while(|w| is_verb_word(w))
                .map(str::to_string)
                .collect();
            if !path.is_empty() && !out.contains(&path) {
                out.push(path);
            }
        }
        out
    }

    fn entries() -> Vec<(&'static str, String, String)> {
        vec![
            ("SessionStart", String::new(), "kanspec prime".into()),
            (
                "PostToolUse",
                EDIT_TOOLS.into(),
                "kanspec quirks --touch x".into(),
            ),
        ]
    }

    #[test]
    fn the_snippet_block_round_trips_byte_for_byte() {
        let original = "# My project\n\nSome notes about the repo.\n";
        let with = insert_snippet(&Some(original.to_string()), &snippet("kanspec")).unwrap();
        assert!(with.starts_with(original), "the user's text stays on top");
        assert!(with.contains(SNIPPET_BEGIN) && with.contains(SNIPPET_END));
        assert_eq!(excise_snippet(&with), original, "removal is not byte-exact");
    }

    #[test]
    fn installing_twice_replaces_the_block_in_place() {
        let text = insert_snippet(&None, &snippet("kanspec")).unwrap();
        let again = insert_snippet(&Some(text.clone()), &snippet("kanspec")).unwrap();
        assert_eq!(text, again);
        // An upgraded snippet replaces the old one where it stands rather than appending.
        let upgraded = insert_snippet(
            &Some(format!("intro\n\n{text}trailer\n")),
            "<!-- kanspec:begin -->\nNEW\n<!-- kanspec:end -->\n",
        )
        .unwrap();
        assert_eq!(
            upgraded,
            "intro\n\n<!-- kanspec:begin -->\nNEW\n<!-- kanspec:end -->\ntrailer\n"
        );
    }

    #[test]
    fn merging_preserves_every_foreign_hook() {
        let foreign = r#"{
          "permissions": {"allow": ["Bash(git:*)"]},
          "hooks": {
            "SessionStart": [{"hooks": [{"type": "command", "command": "echo mine"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard.sh"}]}]
          }
        }"#;
        let merged = merge_settings(Some(foreign), &entries()).unwrap();
        let v: Value = serde_json::from_str(&merged).unwrap();

        assert_eq!(v["permissions"]["allow"][0], json!("Bash(git:*)"));
        assert_eq!(
            v["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            json!("guard.sh")
        );
        let ss = &v["hooks"]["SessionStart"][0]["hooks"];
        assert_eq!(
            ss[0]["command"],
            json!("echo mine"),
            "the foreign hook stays"
        );
        assert_eq!(ss[1]["command"], json!("kanspec prime"), "ours joins it");
        assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], json!(EDIT_TOOLS));

        // …and removal puts it back exactly as it was.
        let stripped = strip_settings(&merged, "kanspec").unwrap().unwrap();
        let back: Value = serde_json::from_str(&stripped).unwrap();
        let want: Value = serde_json::from_str(foreign).unwrap();
        assert_eq!(back, want);
    }

    #[test]
    fn a_settings_file_that_held_only_our_hooks_goes_away_again() {
        let merged = merge_settings(None, &entries()).unwrap();
        assert!(merged.ends_with('\n'));
        assert_eq!(strip_settings(&merged, "kanspec").unwrap(), None);
    }

    #[test]
    fn merging_twice_changes_nothing() {
        let once = merge_settings(None, &entries()).unwrap();
        let twice = merge_settings(Some(&once), &entries()).unwrap();
        assert_eq!(once, twice);
        // An upgraded command replaces ours in place instead of stacking up.
        let upgraded = merge_settings(
            Some(&once),
            &[("SessionStart", String::new(), "ks prime --json".into())],
        )
        .unwrap();
        let v: Value = serde_json::from_str(&upgraded).unwrap();
        assert_eq!(
            v["hooks"]["SessionStart"][0]["hooks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            v["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            json!("ks prime --json")
        );
    }

    #[test]
    fn the_stop_hook_is_installed_only_when_the_config_turns_it_on() {
        let off = hook_entries("kanspec", false);
        assert!(
            !off.iter().any(|(e, _, _)| *e == "Stop"),
            "landcheck defaults to off (D-14): a Stop hook that blocks is opt-in"
        );
        let on = hook_entries("kanspec", true);
        let stop = on.iter().find(|(e, _, _)| *e == "Stop").expect("Stop");
        assert_eq!(stop.2, "kanspec landcheck");
        assert_eq!(on.len(), off.len() + 1, "nothing else changed");
    }

    #[test]
    fn every_hook_design_md_documents_is_actually_installed() {
        let on = hook_entries("kanspec", true);
        for (event, _, why) in AGENT_HOOKS {
            let e = on
                .iter()
                .find(|(ev, _, _)| ev == event)
                .unwrap_or_else(|| panic!("{event} is documented ({why}) but never installed"));
            assert!(is_kanspec_command(&e.2, "kanspec"), "{event}: {}", e.2);
        }
        assert_eq!(on.len(), AGENT_HOOKS.len(), "an undocumented hook appeared");
    }

    #[test]
    fn only_our_commands_look_like_ours() {
        assert!(is_kanspec_command("kanspec prime", "kanspec"));
        assert!(is_kanspec_command("ks landcheck", "kanspec"));
        assert!(is_kanspec_command(
            "f=$(jq -r x); ks quirks --touch \"$f\"",
            "kanspec"
        ));
        assert!(!is_kanspec_command("echo mine", "kanspec"));
        assert!(
            !is_kanspec_command("works prime", "kanspec"),
            "word boundaries matter"
        );
        assert!(!is_kanspec_command("kanspec status", "kanspec"));
    }

    /// The agent contract may not prescribe a command the binary refuses.
    ///
    /// This snippet is the ONLY thing most agent sessions ever read about kanspec, and it
    /// is permanent context — so a line in it that exits 1 is not a small bug. Through
    /// v0.1 it prescribed `kanspec comments --unresolved --json` and `kanspec comment
    /// resolve <cm-id> --note "…"`: both are v0.2, both `#[command(hide = true)]`, both
    /// exit 1. Every session that followed the contract literally hit a refusal, and an
    /// agent that has been refused once stops trusting the block that refused it.
    ///
    /// Walked off the finished clap tree rather than off a hand-kept list, because a list
    /// is the thing that drifted. `hide` is the project's own marker for "v0.2, body not
    /// shipped" (see `cli.rs`), so hidden is disqualifying, not merely undocumented.
    #[test]
    fn the_snippet_only_prescribes_commands_the_binary_actually_has() {
        use clap::CommandFactory;
        let root = crate::cli::Cli::command();

        // Both spellings: `agent_snippet` rewrites the binary name inside backticks, and a
        // rewrite that mangled a command path would be invisible under the default name.
        for bin in ["kanspec", "ks"] {
            let text = snippet(bin);
            let named = commands_named_in(&text, bin);
            assert!(
                named.len() >= 8,
                "only {} commands found in the snippet — the extractor stopped seeing them:\n{text}",
                named.len()
            );
            for path in &named {
                let mut node = &root;
                for name in path {
                    // A bare word under a leaf is a positional (`instructions start`),
                    // not a subcommand. Nothing left to check on this span.
                    if node.get_subcommands().next().is_none() {
                        break;
                    }
                    let sub = node
                        .get_subcommands()
                        .find(|s| s.get_name() == name)
                        .unwrap_or_else(|| {
                            panic!(
                                "the agent contract prescribes `{bin} {}`, which this \
                                 binary has no command for",
                                path.join(" ")
                            )
                        });
                    assert!(
                        !sub.is_hide_set(),
                        "the agent contract prescribes `{bin} {}`, but `{name}` is hidden \
                         — hidden means v0.2, and v0.2 verbs exit 1",
                        path.join(" ")
                    );
                    node = sub;
                }
            }
        }
    }

    /// The extractor is doing real work, so it gets its own proof: a store path is not a
    /// verb, and a placeholder is not a subcommand.
    #[test]
    fn only_backticked_verbs_count_as_prescribed_commands() {
        let found = commands_named_in(
            "prose `kanspec quirk add \"x\" --paths <g>` and `kanspec ship <id> --pr <n>` \
             and `kanspec ready --json`, never read .kanspec/proposals/closed/ and \
             `git status` is not ours",
            "kanspec",
        );
        assert_eq!(
            found,
            vec![
                vec!["quirk".to_string(), "add".to_string()],
                vec!["ship".to_string()],
                vec!["ready".to_string()],
            ]
        );
    }

    #[test]
    fn a_broken_settings_file_is_a_typed_refusal_not_a_clobber() {
        let e = merge_settings(Some("{not json"), &entries()).unwrap_err();
        assert_eq!(e.kind(), "invalid");
        assert!(e.fixes().iter().next().is_some());
    }
}
