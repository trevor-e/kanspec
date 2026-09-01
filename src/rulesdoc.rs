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
//! **The one rule that governs every section below:** the *one-liner* surface is complete —
//! every accepted decision and (unscoped) every active quirk is listed, because `rules` is
//! the audit surface and an audit that hides a binding rule is worthless — while *full
//! text* is earned by an actual path match. That is the context economy DESIGN.md asks
//! for: an agent working in `src/auth/` never pays for the billing quirks, but it is still
//! told they exist.
//!
//! Owner: **S6**.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Serialize;

use crate::error::{KsError, Result};
use crate::git::ChangedPath;
use crate::ids::{DecisionId, ProposalId, QuirkId, SpecName};
use crate::model::{DecisionStatus, QuirkStatus, Severity, Snapshot};
use crate::{fix, fixes};

/// The marker a pre-kanspec rule wears once a human has adopted it, so `rules --audit`
/// stops asking about a bullet that was already answered. It is visible text in the rule
/// bullet, exactly like `{p-xxxx}` — invariant 5.
pub const ADOPTED_TOKEN: &str = "{pre-kanspec}";

/// A path scope. Empty means unscoped — every standing rule, in full.
pub struct Scope {
    pub paths: Vec<String>,
    set: GlobSet,
}

impl Scope {
    pub fn none() -> Scope {
        Scope {
            paths: Vec::new(),
            // An empty `GlobSet` matches nothing; `matches` short-circuits on `paths` for
            // the unscoped case, so the two never disagree.
            set: GlobSetBuilder::new()
                .build()
                .unwrap_or_else(|_| GlobSet::empty()),
        }
    }

    pub fn of(paths: &[String]) -> Result<Scope> {
        let mut b = GlobSetBuilder::new();
        for p in paths {
            b.add(compile(p)?);
        }
        let set = b.build().map_err(|e| {
            KsError::invalid(
                format!("cannot compile the path scope: {e}"),
                fixes![fix!("kanspec rules")],
            )
        })?;
        Ok(Scope {
            paths: paths.to_vec(),
            set,
        })
    }

    /// The `prime` path: scope to what the branch actually touched.
    pub fn from_branch(touched: &[ChangedPath]) -> Result<Scope> {
        let mut paths: Vec<String> = Vec::new();
        for c in touched {
            for p in [Some(&c.path), c.renamed_from.as_ref()]
                .into_iter()
                .flatten()
            {
                if !paths.contains(p) {
                    paths.push(p.clone());
                }
            }
        }
        Scope::of(&paths)
    }

    /// Does this scope cover the concrete path `p`? An **unscoped** scope covers
    /// everything, which is what makes `kanspec rules` with no `--path` the complete
    /// audit surface.
    pub fn matches(&self, p: &str) -> bool {
        self.paths.is_empty() || self.set.is_match(p)
    }

    /// Does an entity whose reach is described by `globs` (a decision's `scope:`, a quirk's
    /// `paths:`, a spec's `code:`) overlap this scope?
    ///
    /// Deliberately **false when unscoped**: overlap is what earns an entity its full text,
    /// and injecting every body into every session is the context blow-up path-scoping
    /// exists to prevent. Matching runs in BOTH directions — the entity's globs against the
    /// scope's paths, and the scope's globs against the entity's glob strings — because
    /// `--path src/auth/login.ts` names a file while `--path "src/**"` names a glob, and a
    /// human typing either means the same question.
    pub fn touches(&self, globs: &[String]) -> bool {
        if self.paths.is_empty() || globs.is_empty() {
            return false;
        }
        if globs.iter().any(|g| self.set.is_match(g)) {
            return true;
        }
        let mut b = GlobSetBuilder::new();
        for g in globs {
            // A rotted glob is `doctor`'s finding, not a reason to refuse an answer here.
            if let Ok(g) = compile(g) {
                b.add(g);
            }
        }
        match b.build() {
            Ok(set) => self.paths.iter().any(|p| set.is_match(p)),
            Err(_) => false,
        }
    }

    /// Does an entity reach into this scope, counting **an entity with no globs at all as
    /// reaching everywhere**?
    ///
    /// A decision with an empty `scope:` and a quirk with empty `paths:` are the most
    /// binding records there are — nothing bounds them — so treating them as matching
    /// nothing would inject the *least* of exactly the rules that steer the most. The
    /// distinction is kept out of [`touches`](Scope::touches), which stays a plain
    /// syntactic overlap test, because a spec with no `code:` globs means something else
    /// entirely: no path association, and therefore no reason to push its rules at an agent
    /// working somewhere unrelated.
    pub fn reaches(&self, globs: &[String]) -> bool {
        !self.is_unscoped() && (globs.is_empty() || self.touches(globs))
    }

    pub fn is_unscoped(&self) -> bool {
        self.paths.is_empty()
    }
}

/// `literal_separator` matches git's `:(glob)` semantics — `src/*.ts` is one directory
/// deep, `src/**` is all of them — so a scope counts what a `Pathspec` would have counted.
fn compile(g: &str) -> Result<globset::Glob> {
    GlobBuilder::new(g)
        .literal_separator(true)
        .build()
        .map_err(|e| {
            KsError::invalid(
                format!("`{g}` is not a valid path glob: {e}"),
                // Every caller of this — `rules --path`, `spec new --code`,
                // `quirk add --paths`, `decide --scope` — hands a human-typed glob, so the
                // fix names the mistake that actually produces this error rather than one
                // command's spelling of it.
                fixes![
                    fix!("quote the glob so the shell does not eat it: \"src/auth/**\""),
                    fix!("kanspec doctor"),
                ],
            )
        })
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
    /// the WHOLE corpus, not the injected subset — `rules` says "41 across 6 capabilities"
    /// whether or not this scope pulled any of them in
    pub spec_rules: usize,
    pub capabilities: usize,
}

/// THE generator. `rules`, `rules --path`, and `prime` all call exactly this.
pub fn build(s: &Snapshot, scope: &Scope) -> RulesDoc {
    let mut decisions = Vec::new();
    for d in s.decisions.values() {
        // PROPOSED is not standing: a pending decision sits in the YOU section of `status`
        // until a human acts, visible but never silently binding (invariant 8). Superseded
        // and revoked are, by definition, no longer steering anyone.
        if d.fm.status != DecisionStatus::Accepted {
            continue;
        }
        // `reaches`, not `touches`: an accepted decision with an empty `scope:` bounds
        // nothing, so wherever the agent is standing, it is standing inside it.
        let matched = scope.reaches(&d.scope);
        decisions.push(StandingDecision {
            id: d.fm.id.clone(),
            title: d.fm.title.clone(),
            source: d.fm.source.clone(),
            scope: d.scope.clone(),
            accepted: d.fm.date.to_string(),
            body: matched.then(|| trimmed(&d.body)),
        });
    }

    let mut quirks = Vec::new();
    for q in s.quirks.values() {
        if q.fm.status != QuirkStatus::Active {
            continue;
        }
        let matched = scope.reaches(&q.fm.paths);
        if !scope.is_unscoped() && !matched {
            continue;
        }
        let body = trimmed(&q.body);
        quirks.push(StandingQuirk {
            id: q.fm.id.clone(),
            title: q.fm.title.clone(),
            paths: q.fm.paths.clone(),
            severity: q.fm.severity,
            source: q.fm.source.as_ref().map(|t| t.to_string()),
            // `quirk add` seeds the body from the title, so repeating it would double every
            // landmine in the payload for no information.
            body: (matched && !body.is_empty() && body != q.fm.title).then_some(body),
        });
    }
    // Landmine before gotcha before debt: the thing that costs you an afternoon leads.
    quirks.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.id.cmp(&b.id)));

    let mut spec_rules = Vec::new();
    let mut total_rules = 0usize;
    for spec in s.specs.values() {
        total_rules += spec.rules.len();
        if !scope.touches(&spec.fm.code) {
            continue;
        }
        for r in &spec.rules {
            spec_rules.push(StandingRule {
                spec: spec.name.clone(),
                anchor: r.anchor.clone(),
                text: r.text.clone(),
                provenance: r.provenance.clone(),
            });
        }
    }

    RulesDoc {
        counts: Counts {
            decisions: decisions.len(),
            quirks: quirks.len(),
            spec_rules: total_rules,
            capabilities: s.specs.len(),
        },
        decisions,
        quirks,
        spec_rules,
    }
}

/// THE renderer — the ONLY way a `RulesDoc` becomes bytes.
///
/// Nothing here reads a clock, a `Style`, a terminal width or an env var: the same
/// `RulesDoc` renders to the same bytes in `rules` and in `prime`, on any terminal, which
/// is what makes invariant 3 a property of the call graph rather than of discipline.
pub fn render_text(d: &RulesDoc) -> String {
    let mut o = String::new();
    o.push_str(
        "STANDING RULES steering agents now \
         (= byte-identical to the rules section of `kanspec prime`)\n",
    );

    // ── decisions ────────────────────────────────────────────────────────────
    o.push_str(&format!(" DECISIONS ({})\n", d.decisions.len()));
    let w = d.decisions.iter().map(|x| x.title.len()).max().unwrap_or(0);
    for x in &d.decisions {
        o.push_str(&format!("  {:<7}  {:<w$}", x.id.to_string(), x.title));
        if let Some(src) = &x.source {
            o.push_str(&format!("  ← {src}"));
        }
        if !x.scope.is_empty() {
            o.push_str(&format!("  scope {}", x.scope.join(" ")));
        }
        o.push_str(&format!("  accepted {}\n", x.accepted));
        push_body(&mut o, x.body.as_deref());
    }

    // ── quirks ───────────────────────────────────────────────────────────────
    o.push_str(&format!(
        " QUIRKS ({}, injected by path match)\n",
        d.quirks.len()
    ));
    let w = d.quirks.iter().map(|x| x.title.len()).max().unwrap_or(0);
    for x in &d.quirks {
        o.push_str(&format!("  {:<7}  {:<w$}", x.id.to_string(), x.title));
        if let Some(src) = &x.source {
            o.push_str(&format!("  ← {src}"));
        }
        if !x.paths.is_empty() {
            o.push_str(&format!("  paths {}", x.paths.join(" ")));
        }
        o.push_str(&format!("  {}\n", severity_word(x.severity)));
        push_body(&mut o, x.body.as_deref());
    }

    // ── spec rules ───────────────────────────────────────────────────────────
    o.push_str(&format!(
        " SPEC RULES: {} across {} {} \
         (`kanspec why <anchor>` walks one home; `rules --audit` flags any with no source)\n",
        d.counts.spec_rules,
        d.counts.capabilities,
        if d.counts.capabilities == 1 {
            "capability"
        } else {
            "capabilities"
        },
    ));
    let mut last: Option<&SpecName> = None;
    for r in &d.spec_rules {
        if last != Some(&r.spec) {
            o.push_str(&format!("  {}\n", r.spec));
            last = Some(&r.spec);
        }
        o.push_str(&format!("   [{}] {}", r.anchor, r.text));
        for p in &r.provenance {
            o.push_str(&format!(" {{{p}}}"));
        }
        o.push('\n');
    }

    o.push_str("Nothing outside this list is served to agents. Closed proposals bind nothing.\n");
    o
}

fn push_body(o: &mut String, body: Option<&str>) {
    let Some(body) = body else { return };
    for line in body.lines() {
        if line.trim().is_empty() {
            o.push('\n');
        } else {
            o.push_str(&format!("      {line}\n"));
        }
    }
}

fn trimmed(s: &str) -> String {
    s.trim_matches(['\n', '\r']).trim_end().to_string()
}

pub fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Landmine => "landmine",
        Severity::Gotcha => "gotcha",
        Severity::Debt => "debt",
    }
}

/// `rules --audit` / `--adopt`.
///
/// Two decays, both of them "this rule is still steering agents and nobody remembers why":
/// an accepted decision whose source proposal is closed and which no live ticket
/// references, and a spec rule with no provenance token at all.
pub fn audit(s: &Snapshot, d: &RulesDoc) -> Vec<AuditWarning> {
    let mut out = Vec::new();

    for x in &d.decisions {
        // A decision minted by `kanspec decide` with no `--from` legitimately has no
        // source: DESIGN.md's second update trigger is exactly "a real architectural call
        // made outside a proposal". Warning on it would fire on the normal path and teach
        // a human to stop reading the audit.
        let Some(src) = x.source.as_deref() else {
            continue;
        };
        // `p-7de2#p1` -> `p-7de2`. A source that names a ticket is not a proposal decay.
        let Ok(pid) = ProposalId::parse(src.split('#').next().unwrap_or(src)) else {
            continue;
        };
        if !s.closed_ids.contains(pid.as_str()) {
            continue;
        }
        let refs = s
            .tickets
            .values()
            .filter(|t| !t.fm.state.terminal() && t.fm.proposal.as_ref() == Some(&pid))
            .count();
        if refs == 0 {
            out.push(AuditWarning {
                subject: x.id.to_string(),
                message: format!(
                    "source proposal {pid} is closed; 0 references from open tickets — \
                     still wanted?"
                ),
                fix: format!("kanspec revoke {} --why \"...\"", x.id),
                adoptable: false,
            });
        }
    }

    for a in adoptable(s) {
        out.push(AuditWarning {
            subject: format!("spec {} [{}]", a.spec, a.anchor),
            message: "no provenance token (pre-kanspec) — adopt or delete".to_string(),
            fix: "kanspec rules --adopt".to_string(),
            adoptable: true,
        });
    }
    out
}

/// One pre-kanspec rule bullet: carries no `{p-xxxx}` and has not been adopted.
#[derive(Debug, Clone, Serialize)]
pub struct Adoptable {
    pub spec: SpecName,
    pub anchor: String,
    /// 1-based line within the spec's BODY — what `Op::StampRule` rewrites
    pub line: usize,
}

/// Every bullet `--audit` warns about and `--adopt` stamps, in `BTreeMap` spec order then
/// file order. ONE definition, so the two flags can never disagree about what is adoptable
/// — and so the count in the refusal-free summary is the count in the audit.
pub fn adoptable(s: &Snapshot) -> Vec<Adoptable> {
    let mut out = Vec::new();
    for spec in s.specs.values() {
        for r in &spec.rules {
            // A rule carrying either token has an answer already: real provenance, or a
            // human who adopted it. `--adopt` must be idempotent across a 800-rule import.
            if !r.provenance.is_empty() || r.text.contains(ADOPTED_TOKEN) {
                continue;
            }
            out.push(Adoptable {
                spec: spec.name.clone(),
                anchor: r.anchor.clone(),
                line: r.line,
            });
        }
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditWarning {
    pub subject: String,
    pub message: String,
    pub fix: String,
    /// whether `--adopt` can repair it
    pub adoptable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ids::{DecisionId, QuirkId, SpecName, TicketId};
    use crate::model::{Decision, DecisionFm, Quirk, QuirkFm, Rule, Spec, SpecFm};
    use chrono::{NaiveDate, TimeZone, Utc};
    use std::collections::BTreeMap;

    fn snap() -> Snapshot {
        Snapshot::empty(
            Config::default(),
            Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap(),
        )
    }

    fn decision(id: &str, status: DecisionStatus, scope: &[&str]) -> Decision {
        let scope: Vec<String> = scope.iter().map(|s| s.to_string()).collect();
        Decision {
            fm: DecisionFm {
                id: DecisionId::parse(id).unwrap(),
                title: "Rate-limit state lives in Redis only".into(),
                status,
                date: NaiveDate::from_ymd_opt(2026, 9, 2).unwrap(),
                source: Some("p-7de2#p1".into()),
                scope: scope.clone(),
                supersedes: None,
                superseded_by: None,
                extra: BTreeMap::new(),
            },
            path: "x".into(),
            body: "## Decision\nSliding window in Redis.\n".into(),
            scope,
        }
    }

    fn quirk(id: &str, paths: &[&str], status: QuirkStatus) -> Quirk {
        Quirk {
            fm: QuirkFm {
                id: QuirkId::parse(id).unwrap(),
                title: "Stripe webhooks replay in staging".into(),
                paths: paths.iter().map(|s| s.to_string()).collect(),
                severity: Severity::Landmine,
                status,
                source: Some(TicketId::parse("t-8812").unwrap()),
                fixed_by: None,
                extra: BTreeMap::new(),
            },
            path: "x".into(),
            body: "Handlers MUST be idempotent.\n".into(),
        }
    }

    fn spec(name: &str, code: &[&str], rules: &[(&str, &str)]) -> Spec {
        Spec {
            name: SpecName::parse(name).unwrap(),
            fm: SpecFm {
                feature: "Login".into(),
                code: code.iter().map(|s| s.to_string()).collect(),
                stale_ack: None,
                extra: BTreeMap::new(),
            },
            path: "x".into(),
            body: String::new(),
            rules: rules
                .iter()
                .enumerate()
                .map(|(i, (a, t))| Rule {
                    anchor: a.to_string(),
                    text: t.to_string(),
                    provenance: vec![ProposalId::parse("p-7de2").unwrap()],
                    items: Vec::new(),
                    line: i + 1,
                })
                .collect(),
        }
    }

    fn corpus() -> Snapshot {
        let mut s = snap();
        let d = decision("D-8c1a", DecisionStatus::Accepted, &["src/auth/**"]);
        s.decisions.insert(d.fm.id.clone(), d);
        let p = decision("D-2c77", DecisionStatus::Proposed, &["src/billing/**"]);
        s.decisions.insert(p.fm.id.clone(), p);
        let q = quirk("q-11ba", &["src/billing/**"], QuirkStatus::Active);
        s.quirks.insert(q.fm.id.clone(), q);
        let f = quirk("q-22cd", &["src/auth/**"], QuirkStatus::Fixed);
        s.quirks.insert(f.fm.id.clone(), f);
        let sp = spec(
            "auth",
            &["src/auth/**"],
            &[("auth.lockout", "5 failed logins lock the account.")],
        );
        s.specs.insert(sp.name.clone(), sp);
        s
    }

    #[test]
    fn a_proposed_decision_is_never_a_standing_rule() {
        let d = build(&corpus(), &Scope::none());
        assert_eq!(d.decisions.len(), 1, "only the ACCEPTED one steers agents");
        assert_eq!(d.decisions[0].id.as_str(), "D-8c1a");
    }

    #[test]
    fn a_fixed_quirk_is_retired_from_the_payload() {
        let d = build(&corpus(), &Scope::none());
        assert_eq!(d.quirks.len(), 1);
        assert_eq!(d.quirks[0].id.as_str(), "q-11ba");
    }

    #[test]
    fn full_text_is_earned_by_a_path_match_and_one_liners_are_complete() {
        let s = corpus();
        let unscoped = build(&s, &Scope::none());
        assert!(
            unscoped.decisions[0].body.is_none(),
            "unscoped is the audit surface: one-liners, never every body"
        );

        let auth = Scope::of(&["src/auth/login.ts".to_string()]).unwrap();
        let scoped = build(&s, &auth);
        assert!(scoped.decisions[0].body.is_some(), "the scope matched");
        assert!(
            scoped.quirks.is_empty(),
            "an agent in src/auth/ never pays for the billing quirks"
        );
        assert_eq!(scoped.spec_rules.len(), 1, "auth's rules ARE injected");
    }

    #[test]
    fn a_glob_scope_matches_the_same_glob_written_as_a_path() {
        let s = corpus();
        let billing = Scope::of(&["src/billing/**".to_string()]).unwrap();
        let d = build(&s, &billing);
        assert_eq!(d.quirks.len(), 1, "the billing landmine is in scope");
        assert!(d.quirks[0].body.is_some());
        assert!(d.decisions[0].body.is_none(), "auth is not in this scope");
    }

    #[test]
    fn a_record_that_bounds_nothing_reaches_every_scope() {
        let mut s = corpus();
        let d = decision("D-4a90", DecisionStatus::Accepted, &[]);
        s.decisions.insert(d.fm.id.clone(), d);
        let q = quirk("q-7c02", &[], QuirkStatus::Active);
        s.quirks.insert(q.fm.id.clone(), q);

        for path in [
            "src/auth/login.ts",
            "src/billing/charge.ts",
            "docs/readme.md",
        ] {
            let d = build(&s, &Scope::of(&[path.to_string()]).unwrap());
            let unbounded = d
                .decisions
                .iter()
                .find(|x| x.id.as_str() == "D-4a90")
                .expect("an unscoped decision is still listed");
            assert!(
                unbounded.body.is_some(),
                "a decision with no `scope:` binds everywhere, including {path}"
            );
            assert!(d.quirks.iter().any(|x| x.id.as_str() == "q-7c02"), "{path}");
        }
    }

    #[test]
    fn a_scope_matching_nothing_still_lists_every_decision_as_a_one_liner() {
        let d = build(
            &corpus(),
            &Scope::of(&["nonexistent/**".to_string()]).unwrap(),
        );
        assert_eq!(d.decisions.len(), 1);
        assert!(d.decisions[0].body.is_none());
        assert!(d.quirks.is_empty() && d.spec_rules.is_empty());
        assert_eq!(d.counts.spec_rules, 1, "the corpus count is not scoped");
    }

    #[test]
    fn the_rendered_text_carries_the_closed_proposals_bind_nothing_line() {
        let text = render_text(&build(&corpus(), &Scope::none()));
        assert!(
            text.starts_with("STANDING RULES steering agents now"),
            "{text}"
        );
        assert!(text.ends_with("Closed proposals bind nothing.\n"), "{text}");
        assert!(text.contains("D-8c1a"));
        assert!(
            !text.contains("D-2c77"),
            "a proposed decision is not standing"
        );
    }

    #[test]
    fn rendering_is_a_pure_function_of_the_doc() {
        let s = corpus();
        let scope = Scope::of(&["src/auth/login.ts".to_string()]).unwrap();
        // The property invariant 3 rests on: same doc, same bytes, every time.
        assert_eq!(
            render_text(&build(&s, &scope)),
            render_text(&build(&s, &scope))
        );
    }

    #[test]
    fn audit_flags_a_spec_rule_with_no_provenance_and_stops_once_adopted() {
        let mut s = corpus();
        let sp = spec(
            "ops",
            &["src/ops/**"],
            &[("ops.alert", "Alert over 100/hr.")],
        );
        let name = sp.name.clone();
        s.specs.insert(name.clone(), sp);
        s.specs.get_mut(&name).unwrap().rules[0].provenance.clear();

        let d = build(&s, &Scope::none());
        let w = audit(&s, &d);
        assert!(w
            .iter()
            .any(|x| x.adoptable && x.subject.contains("ops.alert")));

        s.specs.get_mut(&name).unwrap().rules[0].text =
            format!("Alert over 100/hr. {ADOPTED_TOKEN}");
        assert!(audit(&s, &d).iter().all(|x| !x.adoptable));
    }

    #[test]
    fn audit_flags_a_decision_whose_source_proposal_closed_with_nothing_left_referencing_it() {
        let mut s = corpus();
        s.closed_ids.insert("p-7de2".to_string());
        let d = build(&s, &Scope::none());
        let w = audit(&s, &d);
        assert!(
            w.iter()
                .any(|x| x.subject == "D-8c1a" && x.message.contains("closed")),
            "{w:?}"
        );
    }
}
