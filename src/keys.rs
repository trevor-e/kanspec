//! Invariant 1 as an **absent variant**.
//!
//! Every writable frontmatter key in the crate is a variant below. There is no `Merged`,
//! no `InMain`, no `Landed`, no `Ready`, no `Ci`. Every write in the crate passes through
//! [`Key`] (see `plan::Op::SetFields`), so "no command or UI action can set merge state"
//! is a *type fact*, not a deny-list anyone has to remember to consult.
//!
//! [`RESERVED_DERIVED`] is the other half: the READ-side deny-list for a file that
//! *arrives* with `merged: true` — a hand-edit, an import, a bad merge — which has no
//! write path to blame. `doctor` scans every entity's `#[serde(flatten)] extra` map
//! against it.
//!
//! Owner: **F** (foundation). Frozen after wave 0.

use serde::Serialize;

pub trait FmKey: Copy + PartialEq + 'static {
    /// Canonical schema order, for INSERTION only — `fm::set` never reorders an existing
    /// file, it only needs to know where a brand-new key belongs.
    const ORDER: &'static [Self];
    fn as_str(self) -> &'static str;
}

/// One list per entity kind emits the enum, its `FmKey` impl and the `&[&str]` order
/// constant, so the four could never disagree.
macro_rules! fm_keys {
    ($(#[$m:meta])* $name:ident, $order:ident { $($var:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($var),+ }
        impl FmKey for $name {
            const ORDER: &'static [$name] = &[$($name::$var),+];
            fn as_str(self) -> &'static str {
                match self { $($name::$var => $s),+ }
            }
        }
        pub const $order: &[&str] = &[$($s),+];
    };
}

fm_keys!(
    /// Every writable ticket frontmatter key, in DESIGN.md's order.
    TicketKey, TICKET_ORDER {
        Id => "id", Title => "title", State => "state", Spec => "spec", Proposal => "proposal",
        Item => "item", Deps => "deps", FollowupOf => "followup_of",
        DiscoveredIn => "discovered_in", Branch => "branch", Worktree => "worktree",
        ClaimedBy => "claimed_by", Pr => "pr", Head => "head", SpecUnchanged => "spec_unchanged",
        Created => "created",
    }
);
fm_keys!(SpecKey, SPEC_ORDER { Feature => "feature", Code => "code", StaleAck => "stale_ack" });
fm_keys!(ProposalKey, PROPOSAL_ORDER {
    Id => "id", Title => "title", Status => "status", Specs => "specs", Approved => "approved",
    Ledger => "ledger", Created => "created",
});
fm_keys!(DecisionKey, DECISION_ORDER {
    Id => "id", Title => "title", Status => "status", Date => "date", Source => "source",
    Scope => "scope", Supersedes => "supersedes", SupersededBy => "superseded_by",
});
fm_keys!(QuirkKey, QUIRK_ORDER {
    Id => "id", Title => "title", Paths => "paths", Severity => "severity", Status => "status",
    Source => "source", FixedBy => "fixed_by",
});

/// The single key type that crosses a module boundary.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(untagged)]
pub enum Key {
    Ticket(TicketKey),
    Spec(SpecKey),
    Proposal(ProposalKey),
    Decision(DecisionKey),
    Quirk(QuirkKey),
}

impl Key {
    pub fn as_str(self) -> &'static str {
        match self {
            Key::Ticket(k) => k.as_str(),
            Key::Spec(k) => k.as_str(),
            Key::Proposal(k) => k.as_str(),
            Key::Decision(k) => k.as_str(),
            Key::Quirk(k) => k.as_str(),
        }
    }

    /// `fm::set`'s insertion-point hint: the canonical key order for this entity kind.
    pub fn order(self) -> &'static [&'static str] {
        match self {
            Key::Ticket(_) => TICKET_ORDER,
            Key::Spec(_) => SPEC_ORDER,
            Key::Proposal(_) => PROPOSAL_ORDER,
            Key::Decision(_) => DECISION_ORDER,
            Key::Quirk(_) => QUIRK_ORDER,
        }
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// READ-side deny-list. A file that ARRIVES with `merged: true` — a hand-edit, an import,
/// a bad merge — has no write path to blame, so `doctor` scans every entity's
/// `#[serde(flatten)] extra` map against this.
pub const RESERVED_DERIVED: &[&str] = &[
    "merged",
    "merged_at",
    "in_main",
    "landed",
    "ready",
    "blocked",
    "stalled",
    "settling",
    "stale",
    "dwell",
    "ci",
    "checked_at",
    "method",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_writable_key_is_a_derived_key() {
        for k in TICKET_ORDER
            .iter()
            .chain(SPEC_ORDER)
            .chain(PROPOSAL_ORDER)
            .chain(DECISION_ORDER)
            .chain(QUIRK_ORDER)
        {
            assert!(
                !RESERVED_DERIVED.contains(k),
                "`{k}` is both writable and derived — invariant 1 is broken"
            );
        }
    }

    #[test]
    fn key_order_dispatches_per_entity_kind() {
        assert_eq!(Key::Ticket(TicketKey::Head).as_str(), "head");
        assert_eq!(Key::Ticket(TicketKey::Head).order(), TICKET_ORDER);
        assert_eq!(Key::Quirk(QuirkKey::FixedBy).as_str(), "fixed_by");
    }
}
