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

/// Every writable ticket frontmatter key, in DESIGN.md's order.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketKey {
    Id,
    Title,
    State,
    Spec,
    Proposal,
    Item,
    Deps,
    FollowupOf,
    DiscoveredIn,
    Branch,
    Worktree,
    ClaimedBy,
    Pr,
    Head,
    SpecUnchanged,
    Created,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpecKey {
    Feature,
    Code,
    StaleAck,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKey {
    Id,
    Title,
    Status,
    Specs,
    Approved,
    Ledger,
    Created,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKey {
    Id,
    Title,
    Status,
    Date,
    Source,
    Scope,
    Supersedes,
    SupersededBy,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuirkKey {
    Id,
    Title,
    Paths,
    Severity,
    Status,
    Source,
    FixedBy,
}

impl FmKey for TicketKey {
    const ORDER: &'static [TicketKey] = &[
        TicketKey::Id,
        TicketKey::Title,
        TicketKey::State,
        TicketKey::Spec,
        TicketKey::Proposal,
        TicketKey::Item,
        TicketKey::Deps,
        TicketKey::FollowupOf,
        TicketKey::DiscoveredIn,
        TicketKey::Branch,
        TicketKey::Worktree,
        TicketKey::ClaimedBy,
        TicketKey::Pr,
        TicketKey::Head,
        TicketKey::SpecUnchanged,
        TicketKey::Created,
    ];
    fn as_str(self) -> &'static str {
        match self {
            TicketKey::Id => "id",
            TicketKey::Title => "title",
            TicketKey::State => "state",
            TicketKey::Spec => "spec",
            TicketKey::Proposal => "proposal",
            TicketKey::Item => "item",
            TicketKey::Deps => "deps",
            TicketKey::FollowupOf => "followup_of",
            TicketKey::DiscoveredIn => "discovered_in",
            TicketKey::Branch => "branch",
            TicketKey::Worktree => "worktree",
            TicketKey::ClaimedBy => "claimed_by",
            TicketKey::Pr => "pr",
            TicketKey::Head => "head",
            TicketKey::SpecUnchanged => "spec_unchanged",
            TicketKey::Created => "created",
        }
    }
}

impl FmKey for SpecKey {
    const ORDER: &'static [SpecKey] = &[SpecKey::Feature, SpecKey::Code, SpecKey::StaleAck];
    fn as_str(self) -> &'static str {
        match self {
            SpecKey::Feature => "feature",
            SpecKey::Code => "code",
            SpecKey::StaleAck => "stale_ack",
        }
    }
}

impl FmKey for ProposalKey {
    const ORDER: &'static [ProposalKey] = &[
        ProposalKey::Id,
        ProposalKey::Title,
        ProposalKey::Status,
        ProposalKey::Specs,
        ProposalKey::Approved,
        ProposalKey::Ledger,
        ProposalKey::Created,
    ];
    fn as_str(self) -> &'static str {
        match self {
            ProposalKey::Id => "id",
            ProposalKey::Title => "title",
            ProposalKey::Status => "status",
            ProposalKey::Specs => "specs",
            ProposalKey::Approved => "approved",
            ProposalKey::Ledger => "ledger",
            ProposalKey::Created => "created",
        }
    }
}

impl FmKey for DecisionKey {
    const ORDER: &'static [DecisionKey] = &[
        DecisionKey::Id,
        DecisionKey::Title,
        DecisionKey::Status,
        DecisionKey::Date,
        DecisionKey::Source,
        DecisionKey::Scope,
        DecisionKey::Supersedes,
        DecisionKey::SupersededBy,
    ];
    fn as_str(self) -> &'static str {
        match self {
            DecisionKey::Id => "id",
            DecisionKey::Title => "title",
            DecisionKey::Status => "status",
            DecisionKey::Date => "date",
            DecisionKey::Source => "source",
            DecisionKey::Scope => "scope",
            DecisionKey::Supersedes => "supersedes",
            DecisionKey::SupersededBy => "superseded_by",
        }
    }
}

impl FmKey for QuirkKey {
    const ORDER: &'static [QuirkKey] = &[
        QuirkKey::Id,
        QuirkKey::Title,
        QuirkKey::Paths,
        QuirkKey::Severity,
        QuirkKey::Status,
        QuirkKey::Source,
        QuirkKey::FixedBy,
    ];
    fn as_str(self) -> &'static str {
        match self {
            QuirkKey::Id => "id",
            QuirkKey::Title => "title",
            QuirkKey::Paths => "paths",
            QuirkKey::Severity => "severity",
            QuirkKey::Status => "status",
            QuirkKey::Source => "source",
            QuirkKey::FixedBy => "fixed_by",
        }
    }
}

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

pub const TICKET_ORDER: &[&str] = &[
    "id",
    "title",
    "state",
    "spec",
    "proposal",
    "item",
    "deps",
    "followup_of",
    "discovered_in",
    "branch",
    "worktree",
    "claimed_by",
    "pr",
    "head",
    "spec_unchanged",
    "created",
];
pub const SPEC_ORDER: &[&str] = &["feature", "code", "stale_ack"];
pub const PROPOSAL_ORDER: &[&str] = &[
    "id", "title", "status", "specs", "approved", "ledger", "created",
];
pub const DECISION_ORDER: &[&str] = &[
    "id",
    "title",
    "status",
    "date",
    "source",
    "scope",
    "supersedes",
    "superseded_by",
];
pub const QUIRK_ORDER: &[&str] = &[
    "id", "title", "paths", "severity", "status", "source", "fixed_by",
];

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
    fn order_arrays_agree_with_the_enums() {
        fn check<K: FmKey>(order: &[&str]) {
            let from_enum: Vec<&str> = K::ORDER.iter().map(|k| k.as_str()).collect();
            assert_eq!(from_enum, order);
        }
        check::<TicketKey>(TICKET_ORDER);
        check::<SpecKey>(SPEC_ORDER);
        check::<ProposalKey>(PROPOSAL_ORDER);
        check::<DecisionKey>(DECISION_ORDER);
        check::<QuirkKey>(QUIRK_ORDER);
    }

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
