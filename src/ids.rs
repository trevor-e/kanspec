//! Per-kind id newtypes, the visible-text anchors, and the [`Minter`].
//!
//! The `t-`/`p-`/`D-`/`q-` prefixes are load-bearing twice: type safety (a `QuirkId` will
//! not fit where a `TicketId` is wanted), and stopping a bare 4-hex id like `0x1f` from
//! being reinterpreted as a YAML number.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::{KsError, Result};
use crate::{fix, fixes};

/// One invocation declares every kind AND `ALL_PREFIXES`, so a sixth kind cannot be added
/// without also becoming a "foreign prefix" the other five reject.
macro_rules! id_kinds {
    ($($name:ident, $prefix:literal, $noun:literal;)+) => { $(
        #[doc = concat!("A `", $prefix, "`-prefixed ", $noun, " id.")]
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;
            pub const NOUN: &'static str = $noun;

            /// Accepts `t-9c41` and bare `9c41`; rejects a foreign prefix BY TYPE.
            /// Accepts >= 4 hex so `id_width` can grow without breaking old ids.
            pub fn parse(s: &str) -> Result<Self> {
                let raw = s.trim();
                let body = match raw.strip_prefix(Self::PREFIX) {
                    Some(b) => b,
                    // A bare body is accepted; a body wearing SOMEONE ELSE'S prefix is not.
                    None if crate::ids::has_foreign_prefix(raw, Self::PREFIX) => {
                        return Err(crate::ids::bad_id(Self::NOUN, Self::PREFIX, raw))
                    }
                    None => raw,
                };
                // Reddit-style: `t-9c41-rate-limit-login` IS `t-9c41`. Everything after the
                // hex body is a cosmetic slug — pasted from a listing, a branch name or a
                // proposal directory — and is ignored, so a title edit can never break an
                // id someone copied.
                let body = body.split('-').next().unwrap_or(body);
                if !crate::ids::is_id_body(body) {
                    return Err(crate::ids::bad_id(Self::NOUN, Self::PREFIX, raw));
                }
                Ok($name(format!("{}{}", Self::PREFIX, body)))
            }

            /// The prefixed form — `t-9c41`, never the bare body.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// The hex body without its prefix.
            pub fn body(&self) -> &str {
                &self.0[Self::PREFIX.len()..]
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;
            fn try_from(s: String) -> std::result::Result<Self, String> {
                $name::parse(&s).map_err(|e| e.to_string())
            }
        }

        impl From<$name> for String {
            fn from(v: $name) -> String {
                v.0
            }
        }
    )+
        /// Every prefix in the crate — used to tell "bare body" apart from "wrong kind".
        pub const ALL_PREFIXES: &[&str] = &[$($prefix),+];
    };
}

id_kinds! {
    TicketId, "t-", "ticket";
    ProposalId, "p-", "proposal";
    DecisionId, "D-", "decision";
    QuirkId, "q-", "quirk";
    CommentId, "cm-", "comment";
}

fn has_foreign_prefix(raw: &str, mine: &str) -> bool {
    ALL_PREFIXES
        .iter()
        .any(|p| *p != mine && raw.starts_with(p))
}

fn is_id_body(b: &str) -> bool {
    b.len() >= 4
        && b.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn bad_id(noun: &'static str, prefix: &'static str, raw: &str) -> KsError {
    KsError::invalid(
        format!("`{raw}` is not a {noun} id (expected `{prefix}` + at least 4 lowercase hex)"),
        fixes![fix!("kanspec ls"), fix!("kanspec status")],
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Spec names and the visible-text anchors (invariant 5)
// ─────────────────────────────────────────────────────────────────────────────

/// A spec's file stem — `auth`, `payments`, `billing.webhooks`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecName(String);

impl SpecName {
    pub fn parse(s: &str) -> Result<SpecName> {
        let s = s.trim();
        let ok = !s.is_empty()
            && s.len() <= 64
            && s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
            && s.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')
            });
        if !ok {
            return Err(KsError::invalid(
                format!("`{s}` is not a spec name (lowercase, digits, `-`, `_`, `.`)"),
                fixes![fix!("kanspec spec new \"{s}\""), fix!("kanspec features")],
            ));
        }
        Ok(SpecName(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SpecName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `c` change · `p` prescription · `t` ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Change,
    Prescription,
    Ticket,
}

impl ItemKind {
    pub const fn letter(self) -> char {
        match self {
            ItemKind::Change => 'c',
            ItemKind::Prescription => 'p',
            ItemKind::Ticket => 't',
        }
    }
    pub const fn from_letter(c: char) -> Option<ItemKind> {
        match c {
            'c' => Some(ItemKind::Change),
            'p' => Some(ItemKind::Prescription),
            't' => Some(ItemKind::Ticket),
            _ => None,
        }
    }
}

/// A proposal item anchor: `p-7de2#c3`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ItemRef {
    pub proposal: ProposalId,
    pub kind: ItemKind,
    pub n: u16,
}

impl ItemRef {
    pub fn parse(s: &str) -> Result<ItemRef> {
        let bad = || {
            KsError::invalid(
                format!("`{s}` is not an item anchor (expected `p-7de2#c3`)"),
                fixes![fix!("kanspec comments"), fix!("kanspec rules")],
            )
        };
        let (p, item) = s.trim().split_once('#').ok_or_else(bad)?;
        let proposal = ProposalId::parse(p)?;
        let mut ch = item.chars();
        let kind = ch.next().and_then(ItemKind::from_letter).ok_or_else(bad)?;
        let n: u16 = ch.as_str().parse().map_err(|_| bad())?;
        Ok(ItemRef { proposal, kind, n })
    }
}

impl std::fmt::Display for ItemRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}{}", self.proposal, self.kind.letter(), self.n)
    }
}

/// A spec rule anchor: `auth#lockout` (written `[auth.lockout]` in the spec body).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleRef {
    pub spec: SpecName,
    pub rule: String,
}

impl RuleRef {
    pub fn parse(s: &str) -> Result<RuleRef> {
        let bad = || {
            KsError::invalid(
                format!("`{s}` is not a rule anchor (expected `auth#lockout`)"),
                fixes![fix!("kanspec rules"), fix!("kanspec spec show auth")],
            )
        };
        // Accept both the anchor form `auth#lockout` and the in-body form `auth.lockout`.
        let (sp, rule) = s
            .trim()
            .split_once('#')
            .or_else(|| s.trim().split_once('.'))
            .ok_or_else(bad)?;
        if rule.is_empty() {
            return Err(bad());
        }
        Ok(RuleRef {
            spec: SpecName::parse(sp)?,
            rule: rule.to_string(),
        })
    }
    /// The `[auth.lockout]` form that appears in spec bodies.
    pub fn anchor(&self) -> String {
        format!("{}.{}", self.spec, self.rule)
    }
}

impl std::fmt::Display for RuleRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.spec, self.rule)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Minting
// ─────────────────────────────────────────────────────────────────────────────

/// How many collisions we tolerate before widening. 16 bits is 65536, so birthday
/// collisions bite near ~300 entities; collision-freedom is a property of EXCLUSION, not
/// entropy, and `parse` accepts >= 4 hex precisely so the width can grow.
const RETRIES_BEFORE_WIDENING: u32 = 64;
/// Total attempts before giving up and telling the human to widen `id_width` by hand.
const MAX_ATTEMPTS: u32 = 4096;

/// Minting is check-and-retry against the FRESH in-lock snapshot's id set — which
/// includes `closed_ids`.
pub struct Minter<'s> {
    taken: &'s HashSet<String>,
    seed: u64,
    width: usize,
}

impl<'s> Minter<'s> {
    /// Only `Store::transact` constructs one, under the lock, against the FRESH in-lock
    /// snapshot's id set; `seed` comes from `KANSPEC_ID_SEED` in tests.
    pub(crate) fn new(taken: &'s HashSet<String>, seed: u64, width: usize) -> Minter<'s> {
        Minter {
            taken,
            seed,
            width: width.clamp(4, 16),
        }
    }

    pub fn ticket(&self, title: &str) -> Result<TicketId> {
        TicketId::parse(&self.mint(TicketId::PREFIX, title)?)
    }
    pub fn proposal(&self, title: &str) -> Result<ProposalId> {
        ProposalId::parse(&self.mint(ProposalId::PREFIX, title)?)
    }
    pub fn decision(&self, title: &str) -> Result<DecisionId> {
        DecisionId::parse(&self.mint(DecisionId::PREFIX, title)?)
    }
    pub fn quirk(&self, title: &str) -> Result<QuirkId> {
        QuirkId::parse(&self.mint(QuirkId::PREFIX, title)?)
    }
    pub fn comment(&self, body: &str) -> Result<CommentId> {
        CommentId::parse(&self.mint(CommentId::PREFIX, body)?)
    }

    fn mint(&self, prefix: &'static str, subject: &str) -> Result<String> {
        for attempt in 0..MAX_ATTEMPTS {
            let width = self.width + (attempt / RETRIES_BEFORE_WIDENING) as usize;
            let width = width.min(16);
            let h = fnv1a64(&[
                &self.seed.to_le_bytes(),
                prefix.as_bytes(),
                subject.as_bytes(),
                &attempt.to_le_bytes(),
            ]);
            let candidate = format!("{prefix}{:0width$x}", h & mask(width), width = width);
            if !self.taken.contains(&candidate) {
                return Ok(candidate);
            }
        }
        Err(KsError::conflict(
            format!("could not mint a free `{prefix}` id after {MAX_ATTEMPTS} attempts"),
            fixes![
                fix!("kanspec doctor"),
                fix!("edit .kanspec/config.toml and raise id_width"),
            ],
        ))
    }
}

const fn mask(hex_digits: usize) -> u64 {
    if hex_digits >= 16 {
        u64::MAX
    } else {
        (1u64 << (hex_digits * 4)) - 1
    }
}

/// FNV-1a, hand-rolled so a `KANSPEC_ID_SEED` fixture stays stable across Rust releases
/// (`DefaultHasher` explicitly does not promise that).
fn fnv1a64(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in *p {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// The slug budget of a display [`label`]: `t-9c41-rate-limit-login`, not the whole
/// 40-char branch slug, so the id column stays a column. Cut at a word boundary, so a
/// label is always a prefix of the branch / proposal-directory name minted from the same
/// title — and `parse` accepts either.
pub const LABEL_SLUG_MAX: usize = 20;

/// The human form of an id: the key plus a short title slug — `D-0174-dates-not-booleans`.
///
/// Purely cosmetic. The key is the only thing stored, cited or parsed; the slug rides
/// beside it wherever a human reads the id, the way a Reddit URL carries the post title.
/// Every `parse` drops the slug again, so a label pastes straight into any verb.
pub fn label(id: &impl std::fmt::Display, title: &str) -> String {
    format!("{id}-{}", short_slug(&slug(title)))
}

/// The longest `-`-bounded prefix of a slug within [`LABEL_SLUG_MAX`]; a single word
/// longer than the budget is cut hard rather than dropped.
fn short_slug(full: &str) -> &str {
    if full.len() <= LABEL_SLUG_MAX {
        return full;
    }
    // `slug` is ASCII, so byte indexing is char indexing.
    match full[..=LABEL_SLUG_MAX].rfind('-') {
        Some(i) if i > 0 => &full[..i],
        _ => &full[..LABEL_SLUG_MAX],
    }
}

/// `"Rate-limit login"` -> `"rate-limit-login"`, <= 40 chars, never leading/trailing `-`.
pub fn slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len().min(40));
    let mut dash = true; // suppress a leading dash
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("untitled");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_bare_and_prefixed_and_rejects_foreign() {
        assert_eq!(TicketId::parse("t-9c41").unwrap().as_str(), "t-9c41");
        assert_eq!(TicketId::parse("9c41").unwrap().as_str(), "t-9c41");
        assert_eq!(TicketId::parse(" t-9c41 ").unwrap().as_str(), "t-9c41");
        assert_eq!(TicketId::parse("9c41ab").unwrap().body(), "9c41ab");
        assert!(
            TicketId::parse("q-11ba").is_err(),
            "foreign prefix must be rejected"
        );
        assert!(
            TicketId::parse("t-9C41").is_err(),
            "uppercase hex is not an id"
        );
        assert!(
            TicketId::parse("t-9c4").is_err(),
            "fewer than 4 hex is not an id"
        );
        assert!(TicketId::parse("t-zzzz").is_err());
        assert_eq!(DecisionId::parse("D-8c1a").unwrap().as_str(), "D-8c1a");
        assert_eq!(CommentId::parse("cm-88f1").unwrap().as_str(), "cm-88f1");
    }

    #[test]
    fn parse_ignores_a_cosmetic_slug_like_a_reddit_url() {
        assert_eq!(
            TicketId::parse("t-9c41-rate-limit-login").unwrap().as_str(),
            "t-9c41"
        );
        assert_eq!(
            TicketId::parse("9c41-rate-limit").unwrap().as_str(),
            "t-9c41"
        );
        assert_eq!(TicketId::parse("t-9c41-").unwrap().as_str(), "t-9c41");
        // a proposal DIRECTORY name is a valid proposal id
        assert_eq!(
            ProposalId::parse("p-8fdb-adopting-kanspec-on-an-existing-project")
                .unwrap()
                .as_str(),
            "p-8fdb"
        );
        assert_eq!(
            DecisionId::parse("D-0174-dates-not-booleans")
                .unwrap()
                .as_str(),
            "D-0174"
        );
        // the slug never rescues a bad key
        assert!(TicketId::parse("t-9c4-rate-limit").is_err());
        assert!(TicketId::parse("q-11ba-some-quirk").is_err());
        assert!(TicketId::parse("t--rate-limit").is_err());
        // a wide id keeps its whole body
        assert_eq!(TicketId::parse("t-9c41ab-title").unwrap().body(), "9c41ab");
    }

    #[test]
    fn label_is_key_plus_a_short_word_bounded_slug_that_parse_accepts() {
        let id = TicketId::parse("t-9c41").unwrap();
        assert_eq!(
            label(&id, "Rate-limit login endpoint"),
            "t-9c41-rate-limit-login"
        );
        assert_eq!(
            label(
                &id,
                "Readable ids: slug beside every key, accepted by every verb"
            ),
            "t-9c41-readable-ids-slug"
        );
        assert_eq!(label(&id, "Short"), "t-9c41-short");
        assert_eq!(label(&id, ""), "t-9c41-untitled");
        // exactly at the budget keeps the whole word; a single over-long word is cut hard
        assert_eq!(
            label(&id, "abcdefghi-abcdefghij"),
            "t-9c41-abcdefghi-abcdefghij"
        );
        assert_eq!(label(&id, "abcdefghij-abcdefghij"), "t-9c41-abcdefghij");
        assert_eq!(
            label(&id, "abcdefghijklmnopqrstuvwxyz"),
            "t-9c41-abcdefghijklmnopqrst"
        );
        let d = DecisionId::parse("D-0174").unwrap();
        assert_eq!(
            label(&d, "Dates, not booleans"),
            "D-0174-dates-not-booleans"
        );
        // the label is a prefix of the branch slug and round-trips through parse
        let full = slug("Readable ids: slug beside every key, accepted by every verb");
        assert!(full.starts_with("readable-ids-slug"));
        for title in ["Rate-limit login endpoint", "x", "Ünïcode — title!"] {
            assert_eq!(TicketId::parse(&label(&id, title)).unwrap(), id, "{title}");
        }
    }

    #[test]
    fn ids_round_trip_through_serde_as_plain_strings() {
        let id = TicketId::parse("t-9c41").unwrap();
        let j = serde_json::to_string(&id).unwrap();
        assert_eq!(j, "\"t-9c41\"");
        assert_eq!(serde_json::from_str::<TicketId>(&j).unwrap(), id);
        assert!(serde_json::from_str::<TicketId>("\"q-11ba\"").is_err());
    }

    #[test]
    fn anchors_round_trip() {
        let i = ItemRef::parse("p-7de2#c3").unwrap();
        assert_eq!(i.kind, ItemKind::Change);
        assert_eq!(i.n, 3);
        assert_eq!(i.to_string(), "p-7de2#c3");
        let r = RuleRef::parse("auth#lockout").unwrap();
        assert_eq!(r.anchor(), "auth.lockout");
        assert_eq!(RuleRef::parse("auth.lockout").unwrap(), r);
    }

    #[test]
    fn minting_excludes_taken_ids_and_is_seed_deterministic() {
        let empty = HashSet::new();
        let a = Minter::new(&empty, 7, 4)
            .ticket("Rate-limit login")
            .unwrap();
        let b = Minter::new(&empty, 7, 4)
            .ticket("Rate-limit login")
            .unwrap();
        assert_eq!(a, b, "same seed + same title must mint the same id");
        assert_eq!(a.body().len(), 4);

        let taken: HashSet<String> = std::iter::once(a.as_str().to_string()).collect();
        let c = Minter::new(&taken, 7, 4)
            .ticket("Rate-limit login")
            .unwrap();
        assert_ne!(a, c, "a taken id must be retried past");
    }

    #[test]
    fn minting_widens_when_the_space_is_exhausted() {
        // Every 4-hex `t-` id is taken; the minter must step to 5 hex.
        let taken: HashSet<String> = (0..=0xffffu32).map(|n| format!("t-{n:04x}")).collect();
        let id = Minter::new(&taken, 1, 4).ticket("crowded").unwrap();
        assert_eq!(id.body().len(), 5, "width must step to 5 hex, got {id}");
    }

    #[test]
    fn slugs_are_bounded_and_clean() {
        assert_eq!(slug("Rate-limit login"), "rate-limit-login");
        assert_eq!(slug("  ...Hello, World!!  "), "hello-world");
        assert_eq!(slug("!!!"), "untitled");
        assert!(slug(&"x y".repeat(100)).len() <= 40);
        assert!(!slug(&"x y".repeat(100)).ends_with('-'));
    }
}
