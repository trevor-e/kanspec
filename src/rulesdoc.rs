//! `rules ≡ prime` — THE standing-rules generator and THE renderer (invariant 3).
//!
//! `kanspec rules` writes exactly [`render_text`] and stops. `kanspec prime` writes
//! exactly [`render_text`], then `"\n"`, then the live slice. Byte-identity is a property
//! of the **call graph**: there is physically no second formatter to drift from.
//!
//! [`build`] is pure over `&Snapshot`, and a `Snapshot` cannot contain a closed proposal
//! body, so invariant 4 ("closed proposals bind nothing") is enforced by what the
//! generator's INPUT TYPE can hold.
//!
//! Owner: **S6**.

// Wave-0 skeleton. The bodies below are `todo!("S6: …")`; these two allows exist ONLY so
// the skeleton compiles clippy-clean and MUST be deleted by S6 when the bodies land.
#![allow(unused_variables, dead_code)]

use globset::GlobSet;
use serde::Serialize;

use crate::error::Result;
use crate::git::ChangedPath;
use crate::ids::{DecisionId, ProposalId, QuirkId, SpecName};
use crate::model::{Severity, Snapshot};

/// A path scope. Empty means unscoped — every standing rule, in full.
pub struct Scope {
    pub paths: Vec<String>,
    set: GlobSet,
}

impl Scope {
    pub fn none() -> Scope {
        todo!("S6: an empty GlobSet that matches nothing, with `paths` empty")
    }
    pub fn of(paths: &[String]) -> Result<Scope> {
        todo!("S6: compile each path (or glob) into a GlobSet; a bad glob is KsError::Invalid")
    }
    /// The `prime` path: scope to what the branch actually touched.
    pub fn from_branch(touched: &[ChangedPath]) -> Result<Scope> {
        todo!("S6: Scope::of over the changed paths")
    }
    pub fn matches(&self, p: &str) -> bool {
        todo!("S6: empty scope matches everything; else GlobSet::is_match")
    }
    pub fn is_unscoped(&self) -> bool {
        self.paths.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RulesDoc {
    /// ACCEPTED only; full text iff scope matches, one-liners otherwise
    pub decisions: Vec<StandingDecision>,
    /// ACTIVE only, path-matched
    pub quirks: Vec<StandingQuirk>,
    /// with `{p-xxxx}` provenance tokens
    pub spec_rules: Vec<StandingRule>,
    pub counts: Counts,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandingDecision {
    pub id: DecisionId,
    pub title: String,
    pub source: Option<String>,
    pub scope: Vec<String>,
    pub accepted: String,
    /// present only when the scope matched — context economy is the whole point
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandingQuirk {
    pub id: QuirkId,
    pub title: String,
    pub paths: Vec<String>,
    pub severity: Severity,
    pub source: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandingRule {
    pub spec: SpecName,
    pub anchor: String,
    pub text: String,
    pub provenance: Vec<ProposalId>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Counts {
    pub decisions: usize,
    pub quirks: usize,
    pub spec_rules: usize,
    pub capabilities: usize,
}

/// THE generator. `rules`, `rules --path`, and `prime` all call exactly this.
pub fn build(s: &Snapshot, scope: &Scope) -> RulesDoc {
    todo!("S6: accepted decisions + active quirks + spec rules, path-filtered by `scope`")
}

/// THE renderer — the ONLY way a `RulesDoc` becomes bytes.
pub fn render_text(d: &RulesDoc) -> String {
    todo!("S6: the DESIGN.md `kanspec rules` transcript, ending with `Closed proposals bind nothing.`")
}

/// `rules --audit` / `--adopt`.
pub fn audit(s: &Snapshot, d: &RulesDoc) -> Vec<AuditWarning> {
    todo!("S6: decisions whose source proposal closed long ago with zero live references; spec rules with no provenance token")
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditWarning {
    pub subject: String,
    pub message: String,
    pub fix: String,
    /// whether `--adopt` can repair it
    pub adoptable: bool,
}
