//! The surgical frontmatter writer: lossless split, key index, byte-exact `set`.
//!
//! `gray_matter` is deleted from DESIGN.md's crate list (D-2): it cannot serialize at all,
//! and it mutates the content it hands back, so no byte-identical write path can exist on
//! top of it. `yaml-edit` silently truncates on read and welds lines on write.
//! `serde_yaml_ng::to_string` is **never** called: a no-op round trip reformats 11 of 22
//! lines. `serde_yaml_ng` is a read-side deserializer and nothing else.
//!
//! [`set`] replaces ONLY the value's byte range, so inline comments, key order, quoting
//! style, block scalars, unknown future keys and CRLF all survive; `ship` produces a
//! 3-line real git diff, and 100 round-tripping edits reproduce the file byte-identically.
//!
//! Owner: **S1**.

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum FmError {
    #[error("the file has no `---` frontmatter fence")]
    NoFrontmatter,
    #[error("the frontmatter fence is never closed")]
    Unterminated,
}

/// `open + fm + close + body` reconstructs the input **byte-exactly**. That property is
/// the whole point of this type: everything downstream edits spans inside `fm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdDoc {
    /// the opening fence including any BOM and its line ending, e.g. `"---\n"` / `"---\r\n"`
    pub open: String,
    /// the frontmatter text between the fences, verbatim
    pub fm: String,
    /// the closing fence including its line ending
    pub close: String,
    /// everything after the closing fence, verbatim
    pub body: String,
}

impl MdDoc {
    pub fn render(&self) -> String {
        format!("{}{}{}{}", self.open, self.fm, self.close, self.body)
    }

    /// The line ending this document already uses — every insertion copies it, so a CRLF
    /// file stays CRLF and a LF file never grows a stray `\r`.
    fn nl(&self) -> &'static str {
        if self.open.ends_with("\r\n") || self.fm.contains("\r\n") || self.body.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        }
    }
}

/// `(start, content_end, next_start)` for every line in `s`, CRLF-aware: `content_end`
/// excludes the line ending, `next_start` includes it. A file with no trailing newline
/// yields a final span whose `next_start == s.len()`.
fn lines_of(s: &str) -> Vec<(usize, usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        match s[i..].find('\n').map(|k| i + k) {
            Some(n) => {
                let mut ce = n;
                if ce > i && b[ce - 1] == b'\r' {
                    ce -= 1;
                }
                out.push((i, ce, n + 1));
                i = n + 1;
            }
            None => {
                out.push((i, s.len(), s.len()));
                i = s.len();
            }
        }
    }
    out
}

pub fn split(src: &str) -> std::result::Result<MdDoc, FmError> {
    let ls = lines_of(src);
    if ls.is_empty() {
        return Err(FmError::NoFrontmatter);
    }
    let (s0, e0, n0) = ls[0];
    // A UTF-8 BOM belongs to `open`, so `render()` still rebuilds the input byte-exactly.
    if src[s0..e0].trim_start_matches('\u{feff}').trim_end() != "---" {
        return Err(FmError::NoFrontmatter);
    }
    for &(st, ce, nx) in &ls[1..] {
        let line = &src[st..ce];
        // Only a column-0 `---` closes the fence; an indented one belongs to a block
        // scalar or a nested structure.
        if line.trim_end() == "---" && !line.starts_with([' ', '\t']) {
            return Ok(MdDoc {
                open: src[..n0].to_string(),
                fm: src[n0..st].to_string(),
                close: src[st..nx].to_string(),
                body: src[nx..].to_string(),
            });
        }
    }
    Err(FmError::Unterminated)
}

// ─────────────────────────────────────────────────────────────────────────────
// The key index
// ─────────────────────────────────────────────────────────────────────────────

/// The byte range of one top-level key's value, plus where its block ends — enough to
/// replace a value without touching anything else on the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpan {
    pub key: String,
    /// 0-based line index within [`MdDoc::fm`]
    pub line: usize,
    /// last line index of this key's block, INCLUSIVE (`== line` for a scalar)
    pub block_end: usize,
    /// byte range of the VALUE within [`MdDoc::fm`], excluding any trailing `#` comment
    /// and the whitespace before it
    pub val: (usize, usize),
    pub multiline: bool,
}

/// `key:` at column 0, followed by end-of-line or whitespace. Returns the key and the byte
/// offset just past the colon.
fn parse_key(line: &str) -> Option<(String, usize)> {
    if line.is_empty() || line.starts_with([' ', '\t', '#', '-']) {
        return None;
    }
    let mut end = 0;
    for (i, c) in line.char_indices() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/' {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 || line.as_bytes().get(end) != Some(&b':') {
        return None;
    }
    match line.as_bytes().get(end + 1) {
        None | Some(b' ') | Some(b'\t') => {}
        _ => return None,
    }
    Some((line[..end].to_string(), end + 1))
}

/// First byte index of a trailing `#` comment in `v`, honouring quotes.
fn comment_start(v: &str) -> Option<usize> {
    let b = v.as_bytes();
    let mut q: Option<u8> = None;
    for i in 0..b.len() {
        match q {
            Some(qc) => {
                if b[i] == qc {
                    q = None;
                }
            }
            None => match b[i] {
                b'\'' | b'"' => q = Some(b[i]),
                b'#' if i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t' => return Some(i),
                _ => {}
            },
        }
    }
    None
}

/// CRLF- and quote-aware. Top-level keys only: nested maps are a [`KeySpan`] with
/// `multiline: true`, never a separate entry.
pub fn index(fm: &str) -> Vec<KeySpan> {
    let ls = lines_of(fm);
    let mut keys: Vec<KeySpan> = Vec::new();
    for (li, &(st, ce, _)) in ls.iter().enumerate() {
        let line = &fm[st..ce];
        let Some((key, after_colon)) = parse_key(line) else {
            continue;
        };
        let rest = &line[after_colon..];
        let lead = rest.len() - rest.trim_start().len();
        let vs = after_colon + lead;
        let ve = comment_start(&fm[st + vs..ce]).map_or(ce, |c| st + vs + c);
        let ve = st + vs + fm[st + vs..ve].trim_end().len();

        // How far does this key's value block extend? Everything indented or blank
        // belongs to it; trailing blank lines belong to nobody.
        let mut ex = li + 1;
        while ex < ls.len() {
            let (s2, e2, _) = ls[ex];
            let l2 = &fm[s2..e2];
            if l2.trim().is_empty() || l2.starts_with([' ', '\t']) {
                ex += 1;
            } else {
                break;
            }
        }
        while ex > li + 1 {
            let (s2, e2, _) = ls[ex - 1];
            if fm[s2..e2].trim().is_empty() {
                ex -= 1;
            } else {
                break;
            }
        }
        let block_end = ex - 1;
        let value_txt = &fm[st + vs..ve];
        let multiline = block_end > li || value_txt.starts_with('|') || value_txt.starts_with('>');
        keys.push(KeySpan {
            key,
            line: li,
            block_end,
            val: (st + vs, ve),
            multiline,
        });
    }
    keys
}

// ─────────────────────────────────────────────────────────────────────────────
// Values
// ─────────────────────────────────────────────────────────────────────────────

/// The value vocabulary the write path can express. Deliberately small: no v0.1 or v0.2
/// field holds a nested map, and [`SetOutcome::ReplacedMultiline`] is where that
/// assumption fails loudly if the schema ever grows one (R-9).
#[derive(Debug, Clone, PartialEq)]
pub enum Yv {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<Yv>),
    /// A one-line FLOW mapping — `{sha: a1b9c3d, at: 2026-08-31T12:00:00Z, …}`.
    ///
    /// ADDED BY S6 (reported as a request to F). `spec.stale_ack` is the one v0.1 field
    /// whose type is a struct, and `SpecKey::StaleAck` + `model::StaleAck` were both in the
    /// frozen contract with no `Yv` able to express their value: every other spelling
    /// (`Yv::Str` of `"{…}"`, a flow sequence) comes back from `serde_yaml_ng` as a scalar
    /// or a sequence and fails to deserialize into `StaleAck`, which would make the whole
    /// spec unreadable. Flow style — never a block map — keeps the value on ONE line, so
    /// `index` reports `multiline: false` and a second `features --confirm` is an ordinary
    /// `SetOutcome::Replaced` rather than R-9's hard refusal.
    Map(Vec<(String, Yv)>),
}

impl Yv {
    pub fn s(v: impl Into<String>) -> Yv {
        Yv::Str(v.into())
    }
    /// `None -> Yv::Null`, which renders as the literal `null` DESIGN.md's frontmatter
    /// uses, not as an empty value.
    pub fn opt_s(v: Option<impl Into<String>>) -> Yv {
        v.map_or(Yv::Null, Yv::s)
    }
    pub fn list(v: impl IntoIterator<Item = String>) -> Yv {
        Yv::List(v.into_iter().map(Yv::Str).collect())
    }
}

/// Scalars YAML would reinterpret if we emitted them bare.
const RESERVED: &[&str] = &[
    "true", "false", "null", "yes", "no", "on", "off", "y", "n", "~",
];

/// Can `s` be emitted as a plain (unquoted) YAML scalar in this context?
///
/// The cheap syntactic checks catch the obvious cases; the final round trip through the
/// very parser that will read the file back is what makes this *correct* rather than
/// merely careful. YAML's implicit typing has more traps than a deny-list can hold —
/// recon measured `0x1f` coming back as the integer 31 and `1.20` as the float 1.2 — so
/// "emit it, parse it, and only keep it plain if we get our own string back" is the one
/// rule that cannot be out-argued by the schema.
fn plain_ok(s: &str, flow: bool) -> bool {
    if s.is_empty() || s != s.trim() {
        return false;
    }
    if s.chars().any(char::is_control) {
        return false;
    }
    if b"-?:,[]{}#&*!|>'\"%@`".contains(&s.as_bytes()[0]) {
        return false;
    }
    if s.contains(": ") || s.contains(" #") || s.ends_with(':') {
        return false;
    }
    if flow && s.contains([',', '[', ']', '{', '}']) {
        return false;
    }
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(s)) {
        return false;
    }
    plain_round_trips(s, flow)
}

/// Does `s`, written bare, read back as exactly `s`?
fn plain_round_trips(s: &str, flow: bool) -> bool {
    let doc = if flow {
        format!("k: [{s}]\n")
    } else {
        format!("k: {s}\n")
    };
    let Ok(v) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&doc) else {
        return false;
    };
    let scalar = if flow {
        v.get("k")
            .and_then(|k| k.as_sequence())
            .filter(|q| q.len() == 1)
            .and_then(|q| q.first())
    } else {
        v.get("k")
    };
    scalar.and_then(serde_yaml_ng::Value::as_str) == Some(s)
}

/// A YAML 1.2 double-quoted scalar. `format!("{s:?}")` is NOT a substitute: Rust escapes
/// non-ASCII as `\u{1f600}`, which YAML does not accept.
fn double_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `flow` selects `[a, b]` over a block sequence — DESIGN.md's frontmatter is flow-style.
pub fn emit(v: &Yv, flow: bool) -> String {
    match v {
        Yv::Null => "null".to_string(),
        Yv::Bool(b) => b.to_string(),
        Yv::Int(i) => i.to_string(),
        Yv::Str(s) => {
            if plain_ok(s, flow) {
                s.clone()
            } else if !s.contains(['\n', '\r']) && !s.chars().any(char::is_control) {
                // Single quotes keep backslashes literal, which is what a path or a glob
                // wants; `''` is YAML's escape for a quote inside them.
                format!("'{}'", s.replace('\'', "''"))
            } else {
                double_quote(s)
            }
        }
        Yv::List(items) => {
            let inner: Vec<String> = items.iter().map(|i| emit(i, true)).collect();
            format!("[{}]", inner.join(", "))
        }
        Yv::Map(entries) => {
            let inner: Vec<String> = entries
                .iter()
                // Every VALUE is emitted in flow context, so a `,` `{` or `}` inside one is
                // quoted rather than ending the mapping.
                .map(|(k, v)| format!("{k}: {}", emit(v, true)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Writes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum SetOutcome {
    Unchanged,
    Replaced,
    /// A HARD `KsError::Invalid` in `store.rs`, never a silent collapse.
    ReplacedMultiline,
    Inserted,
}

/// Replaces ONLY the value's byte range; inserts at the canonical position from `order`
/// when the key is absent.
pub fn set(doc: &mut MdDoc, key: &str, val: &Yv, order: &[&str]) -> SetOutcome {
    let new = emit(val, false);
    let keys = index(&doc.fm);

    if let Some(k) = keys.iter().find(|k| k.key == key) {
        if !k.multiline {
            let (vs, ve) = k.val;
            if doc.fm[vs..ve] == new {
                return SetOutcome::Unchanged;
            }
            // `key:` with no value at all needs the separating space back — and an empty
            // value whose line carries a comment (`key:   # why`) needs one AFTER the value
            // too, or `null# why` reads back as the string "null# why".
            let lead = if vs == ve && doc.fm[..vs].ends_with(':') {
                " "
            } else {
                ""
            };
            let trail = if doc.fm[ve..].starts_with('#') {
                " "
            } else {
                ""
            };
            doc.fm.replace_range(vs..ve, &format!("{lead}{new}{trail}"));
            return SetOutcome::Replaced;
        }
        // A block scalar / nested map / multi-line sequence collapses to one line. The
        // ONE lossy path, which is why `store.rs` refuses it outright (R-9).
        let ls = lines_of(&doc.fm);
        let start = ls[k.line].0;
        let end = ls[k.block_end].2;
        let nl = doc.nl();
        doc.fm
            .replace_range(start..end, &format!("{key}: {new}{nl}"));
        return SetOutcome::ReplacedMultiline;
    }

    // Absent: insert before the first schema-successor that IS present, else append.
    // Appending to a frontmatter whose last line has no terminator would weld the new key
    // onto it. `split` guarantees `fm` ends at a line boundary, but a hand-built `MdDoc`
    // that ends mid-line is exactly the input this guard exists for — and it has to run
    // BEFORE `at` is measured, or the key lands in front of the newline it just added.
    let nl = doc.nl();
    if !doc.fm.is_empty() && !doc.fm.ends_with('\n') {
        doc.fm.push_str(nl);
    }
    let at = order
        .iter()
        .skip_while(|k| **k != key)
        .skip(1)
        .find_map(|succ| keys.iter().find(|k| k.key == *succ))
        .map(|k| lines_of(&doc.fm)[k.line].0)
        .unwrap_or(doc.fm.len());
    doc.fm.insert_str(at, &format!("{key}: {new}{nl}"));
    SetOutcome::Inserted
}

/// Append one line at the end of `heading`'s section in the body, creating the heading at
/// the end of the body when it is absent.
pub fn append_to_section(doc: &mut MdDoc, heading: &str, line: &str) {
    let nl = doc.nl();
    let ls = lines_of(&doc.body);
    let hi = ls
        .iter()
        .position(|&(s, e, _)| doc.body[s..e].trim_end() == heading);

    let Some(hi) = hi else {
        let mut pre = String::new();
        if !doc.body.is_empty() && !doc.body.ends_with('\n') {
            pre.push_str(nl);
        }
        if !doc.body.is_empty() && !doc.body.ends_with(&format!("{nl}{nl}")) {
            pre.push_str(nl);
        }
        doc.body.push_str(&format!("{pre}{heading}{nl}{line}{nl}"));
        return;
    };

    // The section runs to the next `## ` heading, minus its trailing blank lines.
    let mut end = ls.len();
    for (i, &(s, e, _)) in ls.iter().enumerate().skip(hi + 1) {
        if doc.body[s..e].starts_with("## ") {
            end = i;
            break;
        }
    }
    while end > hi + 1 && doc.body[ls[end - 1].0..ls[end - 1].1].trim().is_empty() {
        end -= 1;
    }
    let at = ls[end - 1].2;
    let at = if at == doc.body.len() && !doc.body.ends_with('\n') {
        doc.body.push_str(nl);
        doc.body.len()
    } else {
        at
    };
    doc.body.insert_str(at, &format!("{line}{nl}"));
}

/// One `- [ ]` / `- [x]` line under `## Steps`: its 1-based number, its state and its text.
struct Checkbox {
    /// byte index of the character inside the brackets
    mark: usize,
    done: bool,
    text: String,
}

/// Every checkbox under `## Steps`, in order. The ONE definition of "which line is step
/// N", shared by [`steps`] and [`mark_step`] so the parsed view and the writer cannot
/// disagree about numbering.
fn checkboxes(body: &str) -> Vec<Checkbox> {
    let mut out = Vec::new();
    let mut inside = false;
    for (s, e, _) in lines_of(body) {
        let raw = &body[s..e];
        let t = raw.trim_start();
        if t.starts_with("## ") {
            inside = raw.trim_end() == crate::logentry::STEPS_HEADING;
            continue;
        }
        if !inside {
            continue;
        }
        let lead = raw.len() - t.len();
        let Some(rest) = t.strip_prefix("- [") else {
            continue;
        };
        let Some(mark) = rest.chars().next() else {
            continue;
        };
        if !rest[mark.len_utf8()..].starts_with(']') {
            continue;
        }
        let done = match mark {
            ' ' => false,
            'x' | 'X' => true,
            _ => continue,
        };
        let text = rest[mark.len_utf8() + 1..].trim().to_string();
        out.push(Checkbox {
            mark: s + lead + 3,
            done,
            text,
        });
    }
    out
}

/// The parsed `- [ ]` view of a ticket body: `(1-based index, done, text)`.
pub fn steps(body: &str) -> Vec<(usize, bool, String)> {
    checkboxes(body)
        .into_iter()
        .enumerate()
        .map(|(i, c)| (i + 1, c.done, c.text))
        .collect()
}

/// `index` is the 1-based step number as rendered by `model::Step`.
pub fn mark_step(doc: &mut MdDoc, index: usize, done: bool) -> bool {
    let boxes = checkboxes(&doc.body);
    let Some(c) = index.checked_sub(1).and_then(|i| boxes.get(i)) else {
        return false;
    };
    if c.done == done {
        return true;
    }
    let ch = if done { "x" } else { " " };
    doc.body.replace_range(c.mark..c.mark + 1, ch);
    true
}

/// Is `line` the rule bullet `- [<anchor>] …`? Mirrors `store::parse_rules`, including its
/// refusal to read a `- [ ]` / `- [x]` checkbox as a rule.
fn is_rule_bullet(line: &str, anchor: &str) -> bool {
    let t = line.trim();
    let Some(rest) = t.strip_prefix("- [") else {
        return false;
    };
    let Some((a, _)) = rest.split_once(']') else {
        return false;
    };
    let a = a.trim();
    !a.is_empty() && !a.eq_ignore_ascii_case("x") && a == anchor
}

/// Append `token` to the rule bullet on 1-based body `line`, which must still be the
/// `- [anchor] …` bullet the snapshot parsed there. Returns false when it is not — a plan
/// staging several stamps into one doc must never write a token onto a line that moved
/// under it.
///
/// Stamping cannot change the body's line COUNT, so every other `line` in the same plan
/// stays valid however many stamps land first. That is the whole reason this appends
/// rather than rewrites.
///
/// Idempotent: a bullet already carrying `token` is left byte-identical, so re-running
/// `rules --adopt` is a no-op rather than a second token.
pub fn stamp_rule(doc: &mut MdDoc, line: usize, anchor: &str, token: &str) -> bool {
    let Some(i) = line.checked_sub(1) else {
        return false;
    };
    let Some(&(start, end, _)) = lines_of(&doc.body).get(i) else {
        return false;
    };
    let raw = &doc.body[start..end];
    if !is_rule_bullet(raw, anchor) {
        return false;
    }
    if raw.contains(token) {
        return true;
    }
    let at = start + raw.trim_end().len();
    doc.body.insert_str(at, &format!(" {token}"));
    true
}

/// SAFETY GUARD — `Store::transact` calls this before ANY byte moves, and
/// `doctor::check_frontmatter_writable` calls it on every file. It compares the line
/// indexer's top-level keys against `serde_yaml_ng`'s; a mismatch (a quoted key, an
/// explicit `?` key, a key with spaces) becomes a typed refusal instead of a silently
/// appended duplicate key.
pub fn writable(fm_text: &str) -> std::result::Result<(), String> {
    let val: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(fm_text).map_err(|e| format!("invalid YAML frontmatter: {e}"))?;
    let map = match &val {
        serde_yaml_ng::Value::Mapping(m) => m,
        // An empty frontmatter is legal and trivially writable.
        serde_yaml_ng::Value::Null => return Ok(()),
        _ => return Err("frontmatter is not a mapping".to_string()),
    };
    let mut yaml_keys: Vec<String> = map
        .keys()
        .map(|k| {
            k.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("{k:?}"))
        })
        .collect();
    let mut idx_keys: Vec<String> = index(fm_text).into_iter().map(|k| k.key).collect();
    yaml_keys.sort();
    idx_keys.sort();
    if yaml_keys != idx_keys {
        return Err(format!(
            "frontmatter uses YAML kanspec cannot edit in place (the parser sees \
             {yaml_keys:?}, the line index sees {idx_keys:?}); rewrite the keys as plain \
             `key: value`"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKET: &str = "\
---
id: t-9c41
title: Rate-limit login endpoint
state: doing              # todo | doing | review | done | dropped
spec: auth

# set by kanspec start
branch: ks/t-9c41-rate-limit-login
estimate: \"S\"
deps: [t-31aa]
pr: null
head: null
severity_hint_v2: high    # a key a NEWER kanspec wrote
created: 2026-08-30T14:02:11Z
---
Implement [auth.lockout].

## Steps
- [x] lockout counter
- [ ] 429 + Retry-After

## Log
- 2026-08-30T14:02Z  todo   trevor           new
";

    #[test]
    fn split_reconstructs_the_input_byte_exactly() {
        for src in [
            TICKET,
            "---\n---\n",
            "---\nid: t-1\n---\n",
            "---\r\nid: t-1\r\n---\r\nbody\r\n",
            "\u{feff}---\nid: t-1\n---\nbody",
            "---\nid: t-1\n---\nno trailing newline",
        ] {
            let d = split(src).expect("splits");
            assert_eq!(d.render(), src, "round trip broke for {src:?}");
        }
    }

    #[test]
    fn split_refuses_what_is_not_frontmatter() {
        assert_eq!(split("no fence here\n"), Err(FmError::NoFrontmatter));
        assert_eq!(split(""), Err(FmError::NoFrontmatter));
        assert_eq!(split("---\nid: t-1\n"), Err(FmError::Unterminated));
    }

    #[test]
    fn an_indented_fence_does_not_close_the_block() {
        let src = "---\nnote: |\n  ---\n  still the scalar\nid: t-1\n---\nbody\n";
        let d = split(src).unwrap();
        assert!(d.fm.contains("still the scalar"));
        assert_eq!(d.body, "body\n");
    }

    #[test]
    fn index_sees_every_top_level_key_and_no_comment_bytes() {
        let d = split(TICKET).unwrap();
        let idx = index(&d.fm);
        let keys: Vec<&str> = idx.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "id",
                "title",
                "state",
                "spec",
                "branch",
                "estimate",
                "deps",
                "pr",
                "head",
                "severity_hint_v2",
                "created"
            ]
        );
        let state = index(&d.fm).into_iter().find(|k| k.key == "state").unwrap();
        assert_eq!(&d.fm[state.val.0..state.val.1], "doing");
        assert!(!state.multiline);
    }

    #[test]
    fn a_quoted_hash_is_not_a_comment() {
        let d = split("---\ntitle: 'a # not a comment'\n---\n").unwrap();
        let k = &index(&d.fm)[0];
        assert_eq!(&d.fm[k.val.0..k.val.1], "'a # not a comment'");
    }

    #[test]
    fn a_ship_writes_three_lines_and_nothing_else() {
        let mut d = split(TICKET).unwrap();
        assert_eq!(
            set(&mut d, "state", &Yv::s("review"), crate::keys::TICKET_ORDER),
            SetOutcome::Replaced
        );
        assert_eq!(
            set(&mut d, "pr", &Yv::Int(142), crate::keys::TICKET_ORDER),
            SetOutcome::Replaced
        );
        assert_eq!(
            set(&mut d, "head", &Yv::s("a1b9c3d"), crate::keys::TICKET_ORDER),
            SetOutcome::Replaced
        );
        let after = d.render();

        let before_lines: Vec<&str> = TICKET.lines().collect();
        let after_lines: Vec<&str> = after.lines().collect();
        assert_eq!(before_lines.len(), after_lines.len(), "no lines added");
        let changed: Vec<usize> = before_lines
            .iter()
            .zip(&after_lines)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(changed.len(), 3, "only 3 lines may change: {changed:?}");

        // Everything the recon proved survives, survives.
        assert!(after.contains("state: review              # todo | doing"));
        assert!(after.contains("# set by kanspec start"));
        assert!(after.contains("estimate: \"S\""));
        assert!(after.contains("deps: [t-31aa]"));
        assert!(after.contains("severity_hint_v2: high    # a key a NEWER kanspec wrote"));
        assert!(after.ends_with("new\n"));
    }

    #[test]
    fn one_hundred_round_tripping_edits_reproduce_the_file_byte_identically() {
        let mut d = split(TICKET).unwrap();
        for i in 0..100 {
            set(&mut d, "state", &Yv::s("review"), crate::keys::TICKET_ORDER);
            set(&mut d, "pr", &Yv::Int(i), crate::keys::TICKET_ORDER);
            set(&mut d, "head", &Yv::s("a1b9c3d"), crate::keys::TICKET_ORDER);
            set(&mut d, "state", &Yv::s("doing"), crate::keys::TICKET_ORDER);
            set(&mut d, "pr", &Yv::Null, crate::keys::TICKET_ORDER);
            set(&mut d, "head", &Yv::Null, crate::keys::TICKET_ORDER);
        }
        assert_eq!(d.render(), TICKET);
    }

    #[test]
    fn setting_a_value_to_what_it_already_is_is_a_no_op() {
        let mut d = split(TICKET).unwrap();
        assert_eq!(
            set(&mut d, "state", &Yv::s("doing"), crate::keys::TICKET_ORDER),
            SetOutcome::Unchanged
        );
        assert_eq!(d.render(), TICKET);
    }

    #[test]
    fn an_absent_key_lands_at_its_canonical_position() {
        let mut d =
            split("---\nid: t-1\ntitle: x\ncreated: 2026-01-01T00:00:00Z\n---\nb\n").unwrap();
        assert_eq!(
            set(&mut d, "state", &Yv::s("todo"), crate::keys::TICKET_ORDER),
            SetOutcome::Inserted
        );
        assert_eq!(
            d.fm,
            "id: t-1\ntitle: x\nstate: todo\ncreated: 2026-01-01T00:00:00Z\n"
        );
        // `created` is `head`'s schema successor, so `head` lands just before it.
        assert_eq!(
            set(&mut d, "head", &Yv::s("a1b9c3d"), crate::keys::TICKET_ORDER),
            SetOutcome::Inserted
        );
        assert_eq!(
            d.fm,
            "id: t-1\ntitle: x\nstate: todo\nhead: a1b9c3d\ncreated: 2026-01-01T00:00:00Z\n"
        );

        // No successor present at all -> append at the end of the frontmatter.
        let mut d = split("---\nid: t-1\ntitle: x\n---\nb\n").unwrap();
        set(&mut d, "head", &Yv::s("a1b9c3d"), crate::keys::TICKET_ORDER);
        assert_eq!(d.fm, "id: t-1\ntitle: x\nhead: a1b9c3d\n");
    }

    #[test]
    fn crlf_survives_every_write() {
        let src = "---\r\nid: t-1\r\nstate: todo\r\n---\r\n## Log\r\n- old\r\n";
        let mut d = split(src).unwrap();
        set(&mut d, "state", &Yv::s("doing"), crate::keys::TICKET_ORDER);
        set(&mut d, "pr", &Yv::Int(7), crate::keys::TICKET_ORDER);
        append_to_section(&mut d, "## Log", "- new");
        let out = d.render();
        assert!(!out.contains("\n\r"), "{out:?}");
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
        assert!(out.contains("state: doing\r\n"));
        assert!(out.contains("pr: 7\r\n"));
        assert!(out.ends_with("- new\r\n"));
    }

    #[test]
    fn a_multiline_value_is_flagged_so_store_can_refuse_it() {
        let mut d = split("---\nnote: |\n  one\n  two\nid: t-1\n---\nb\n").unwrap();
        let k = index(&d.fm).into_iter().find(|k| k.key == "note").unwrap();
        assert!(k.multiline);
        assert_eq!(k.block_end, 2, "the block ends on `  two`, inclusive");
        assert_eq!(
            set(&mut d, "note", &Yv::s("flat"), &["note", "id"]),
            SetOutcome::ReplacedMultiline
        );
        assert_eq!(d.fm, "note: flat\nid: t-1\n");
    }

    #[test]
    fn every_emitted_scalar_reparses_to_the_value_we_wrote() {
        let cases = [
            "plain",
            "src/auth/**",
            "142",
            "0x1f",
            "1.20",
            "no",
            "TRUE",
            "~",
            "it's a \"quoted\" title: really",
            "- leading dash",
            "",
            "trailing space ",
            "#leading hash",
            "line one\nline two",
            "tab\there",
            "emoji \u{1f600}",
        ];
        for c in cases {
            let text = format!("k: {}\n", emit(&Yv::s(c), false));
            let v: serde_yaml_ng::Value =
                serde_yaml_ng::from_str(&text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
            assert_eq!(v["k"].as_str(), Some(c), "{text:?}");
        }
        // Lists round-trip in flow style, which is what DESIGN.md's frontmatter uses.
        let text = format!(
            "k: {}\n",
            emit(
                &Yv::list(["src/auth/**".to_string(), "a, b".to_string()]),
                false
            )
        );
        let v: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text).unwrap();
        let got: Vec<&str> = v["k"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert_eq!(got, ["src/auth/**", "a, b"]);
        assert_eq!(emit(&Yv::Null, false), "null");
        assert_eq!(emit(&Yv::Bool(true), false), "true");
        assert_eq!(emit(&Yv::List(vec![]), false), "[]");
    }

    #[test]
    fn append_to_section_lands_inside_the_log_and_creates_it_when_absent() {
        let mut d = split(TICKET).unwrap();
        append_to_section(&mut d, "## Log", "- second");
        assert!(d
            .body
            .ends_with("- 2026-08-30T14:02Z  todo   trevor           new\n- second\n"));

        // A section followed by another heading appends BEFORE that heading.
        let mut d = split("---\nid: t-1\n---\n## Log\n- a\n\n## After\ntail\n").unwrap();
        append_to_section(&mut d, "## Log", "- b");
        assert_eq!(d.body, "## Log\n- a\n- b\n\n## After\ntail\n");

        // Absent heading -> created at the end, separated by a blank line.
        let mut d = split("---\nid: t-1\n---\nprose\n").unwrap();
        append_to_section(&mut d, "## Log", "- a");
        assert_eq!(d.body, "prose\n\n## Log\n- a\n");

        // An empty body needs no leading blank line.
        let mut d = split("---\nid: t-1\n---\n").unwrap();
        append_to_section(&mut d, "## Log", "- a");
        assert_eq!(d.body, "## Log\n- a\n");
    }

    #[test]
    fn steps_and_mark_step_agree_on_the_numbering() {
        let mut d = split(TICKET).unwrap();
        assert_eq!(
            steps(&d.body),
            vec![
                (1, true, "lockout counter".to_string()),
                (2, false, "429 + Retry-After".to_string()),
            ]
        );
        assert!(mark_step(&mut d, 2, true));
        assert!(!mark_step(&mut d, 3, true), "no third step");
        assert_eq!(
            steps(&d.body)[1],
            (2, true, "429 + Retry-After".to_string())
        );
        assert!(d.body.contains("- [x] 429 + Retry-After"));
        assert!(mark_step(&mut d, 1, false));
        assert!(d.body.contains("- [ ] lockout counter"));
        // Only the mark byte moved.
        assert_eq!(d.render().len(), TICKET.len());
    }

    #[test]
    fn writable_refuses_frontmatter_the_indexer_cannot_see() {
        assert!(writable("id: t-1\ntitle: x\n").is_ok());
        assert!(
            writable("- a\n- b\n").is_err(),
            "a sequence is not a mapping"
        );
        assert!(writable("a: 1\na: 2\n").is_err(), "duplicate key");
        let e = writable("\"my key\": 1\nid: t-1\n").unwrap_err();
        assert!(e.contains("cannot edit in place"), "{e}");
        assert!(writable("").is_ok(), "empty frontmatter is writable");
        assert!(writable("id: [unterminated\n").is_err());
    }

    #[test]
    fn the_indexer_and_serde_agree_on_the_design_md_corpus() {
        let d = split(TICKET).unwrap();
        writable(&d.fm).expect("the reference ticket must be writable");
    }

    // ── stamp_rule ───────────────────────────────────────────────────────────

    const SPEC: &str = "---\nfeature: Login\ncode: [src/auth/**]\n---\n# auth\n\n## Rules\n- [auth.jwt] Login issues a JWT valid 24h.\n- [auth.lockout] 5 failed logins lock the account. {p-7de2}\n";

    fn spec_doc() -> MdDoc {
        split(SPEC).unwrap()
    }

    /// The line number comes from `store::parse_rules`, which indexes the same `doc.body`.
    fn line_of(doc: &MdDoc, anchor: &str) -> usize {
        doc.body
            .lines()
            .position(|l| l.trim().starts_with(&format!("- [{anchor}]")))
            .unwrap()
            + 1
    }

    #[test]
    fn stamp_rule_appends_the_token_to_that_bullet_and_nothing_else() {
        let mut d = spec_doc();
        let n = line_of(&d, "auth.jwt");
        assert!(stamp_rule(&mut d, n, "auth.jwt", "{pre-kanspec}"));
        assert!(d
            .body
            .contains("- [auth.jwt] Login issues a JWT valid 24h. {pre-kanspec}"));
        // the sibling rule is untouched, and the body still has the same line count
        assert!(d
            .body
            .contains("- [auth.lockout] 5 failed logins lock the account. {p-7de2}\n"));
        assert_eq!(d.body.lines().count(), spec_doc().body.lines().count());
    }

    #[test]
    fn stamp_rule_is_idempotent_so_a_second_adopt_is_a_no_op() {
        let mut d = spec_doc();
        let n = line_of(&d, "auth.jwt");
        assert!(stamp_rule(&mut d, n, "auth.jwt", "{pre-kanspec}"));
        let once = d.render();
        assert!(stamp_rule(&mut d, n, "auth.jwt", "{pre-kanspec}"));
        assert_eq!(
            once,
            d.render(),
            "a second stamp must not add a second token"
        );
    }

    /// The guard that makes a line number safe to carry in `Op::StampRule`: if the bullet
    /// moved, the stamp refuses instead of writing onto whatever took its place.
    #[test]
    fn stamp_rule_refuses_when_the_line_is_not_that_anchors_bullet() {
        let mut d = spec_doc();
        let n = line_of(&d, "auth.jwt");
        assert!(!stamp_rule(&mut d, n, "auth.lockout", "{pre-kanspec}"));
        assert!(
            !stamp_rule(&mut d, 1, "auth.jwt", "{pre-kanspec}"),
            "# auth is not a bullet"
        );
        assert!(!stamp_rule(&mut d, 9_999, "auth.jwt", "{pre-kanspec}"));
        assert!(!stamp_rule(&mut d, 0, "auth.jwt", "{pre-kanspec}"));
        assert_eq!(d.render(), SPEC, "a refused stamp must move no bytes");
    }

    /// `- [ ]` / `- [x]` are checkboxes, not rules — `store::parse_rules` skips them and so
    /// must this, or a ticket body could be stamped through a mis-planned op.
    #[test]
    fn stamp_rule_never_treats_a_checkbox_as_a_rule() {
        let src = "---\nid: t-9c41\n---\n- [ ] do the thing\n- [x] did the thing\n";
        let mut d = split(src).unwrap();
        assert!(!stamp_rule(&mut d, 1, "", "{pre-kanspec}"));
        assert!(!stamp_rule(&mut d, 2, "x", "{pre-kanspec}"));
        assert_eq!(d.render(), src);
    }

    /// The token goes after the last non-space character, so a CRLF line ending and any
    /// trailing spaces survive — `fm_bytes` byte-identity is the whole point.
    #[test]
    fn stamp_rule_lands_before_trailing_whitespace_and_crlf() {
        let src = "---\nfeature: x\n---\r\n- [a.b] text.  \r\n- [c.d] more.\n";
        let mut d = split(src).unwrap();
        assert!(stamp_rule(&mut d, 1, "a.b", "{pre-kanspec}"));
        assert!(
            d.body.starts_with("- [a.b] text. {pre-kanspec}  \r\n"),
            "{:?}",
            d.body
        );
        assert!(d.body.ends_with("- [c.d] more.\n"));
    }
}
