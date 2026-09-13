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
//! **The spec-rules budget** is the same bargain one level down. Full text is earned by a
//! path match, but on a real corpus a single touched file can match several capabilities
//! and a wide branch can match twenty (measured on an 83-spec / 666-rule repo: a median
//! commit injects ~1.1k tokens of spec rules, the widest ~14.6k). So matched specs are
//! RANKED — the most specific glob first, then the spec covering most of the touched paths
//! — and shown in that order while `[prime] spec_budget_tokens` is unspent; every spec
//! past the budget is still NAMED, with its rule count and the command that shows it.
//! Nothing is hidden silently, the first-ranked spec always shows in full, and the budget
//! lives here so `rules --path` and `prime` elide identically (invariant 3).
//!
//! Owner: **S6**.

use globset::{GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use serde::Serialize;

use crate::error::{KsError, Result};
use crate::git::ChangedPath;
use crate::ids::{DecisionId, ProposalId, QuirkId, SpecName, TicketId};
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
    /// `paths`, compiled one by one, so [`Scope::rank`] can count WHICH paths an entity
    /// reaches rather than only whether any did.
    each: Vec<GlobMatcher>,
}

/// How strongly an entity's globs reach into a scope — what orders specs for the budget.
/// Derived `Ord` is field order, so a plain `>` reads "more deserving of the budget":
/// the more specific glob wins, and between equals the entity covering more of the touched
/// paths does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Rank {
    /// The most specific glob that matched: leading literal segments, then whether the glob
    /// names an exact path at all. `src/auth/login.ts` (3, exact) beats `src/auth/**`
    /// (2, glob) beats `src/**` (1, glob): a spec that names the file is more this file's
    /// spec than a spec that owns the directory.
    pub specificity: (usize, bool),
    /// How much of the branch this entity OWNS, in millionths of a path: each touched path
    /// is worth one, split evenly among the entities that match it most specifically, and
    /// an entity that matches it less specifically than some other gets nothing for it.
    ///
    /// The tiebreak used to be the raw path count below, and on the trial a 52-rule
    /// `tasks` spec outranked the 16-rule spec the ticket was about because it named
    /// `models.py` and `schemas.py` — files every spec in that repo names. Coverage by a
    /// file everybody names is not evidence of being the branch's spec; coverage by a
    /// file nobody else names is (t-2ba7).
    pub ownership: u64,
    /// How many of the scope's paths some glob matched — how much of the branch this
    /// entity touches at all. Last, because it is what the two fields above refine.
    pub paths: usize,
}

/// One path of [`Rank::ownership`], as an integer so `Rank` stays `Ord` by field order.
/// Shares are `SCALE / owners`, so the error per path is under a millionth and a real
/// difference — at least `1 / owners²` — is never rounded into a tie.
const OWNERSHIP_SCALE: u64 = 1_000_000;

impl Scope {
    pub fn none() -> Scope {
        // An empty builder cannot fail, and the empty `GlobSet` it builds matches nothing;
        // `matches` short-circuits on `paths` for the unscoped case, so the two never
        // disagree.
        Scope::of(&[]).expect("an empty scope always compiles")
    }

    pub fn of(paths: &[String]) -> Result<Scope> {
        let mut b = GlobSetBuilder::new();
        let mut each = Vec::with_capacity(paths.len());
        for p in paths {
            let g = compile(p)?;
            each.push(g.compile_matcher());
            b.add(g);
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
            each,
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
        self.rank(globs).is_some()
    }

    /// [`touches`](Scope::touches) with the strength of the overlap: `None` is no overlap
    /// (so `touches` is exactly `rank(..).is_some()` — ONE definition of "in scope"), and a
    /// `Some` carries what the budget sorts on. An entity ranked alone owns every path it
    /// matches; [`rank_all`](Scope::rank_all) is the corpus-aware form the budget uses.
    pub fn rank(&self, globs: &[String]) -> Option<Rank> {
        self.rank_all(std::iter::once(globs)).pop().flatten()
    }

    /// [`rank`](Scope::rank) for a whole corpus at once, so ownership can be shared: one
    /// `Option<Rank>` per entity, in input order, `None` where the entity does not touch
    /// the scope. Same two-direction matching as always (a glob matching the path, or the
    /// scope's own glob matching the entity's glob).
    pub fn rank_all<'g>(
        &self,
        entities: impl IntoIterator<Item = &'g [String]>,
    ) -> Vec<Option<Rank>> {
        // Per entity, per scope path: the most specific glob that matched it, if any.
        let hits: Vec<Vec<Option<(usize, bool)>>> =
            entities.into_iter().map(|globs| self.hits(globs)).collect();
        // Per scope path: the best specificity any entity reached it with, and how many
        // entities reached it that well — the path's owners.
        let mut best: Vec<Option<((usize, bool), u64)>> = vec![None; self.paths.len()];
        for h in &hits {
            for (slot, s) in best.iter_mut().zip(h) {
                let Some(s) = *s else { continue };
                match slot {
                    Some((b, n)) if *b == s => *n += 1,
                    Some((b, _)) if *b > s => {}
                    _ => *slot = Some((s, 1)),
                }
            }
        }
        hits.iter()
            .map(|h| {
                let mut specificity: Option<(usize, bool)> = None;
                let mut ownership = 0u64;
                let mut paths = 0usize;
                for (s, owner) in h.iter().zip(&best) {
                    let Some(s) = *s else { continue };
                    paths += 1;
                    if specificity.is_none_or(|b| s > b) {
                        specificity = Some(s);
                    }
                    if let Some((b, n)) = owner {
                        if *b == s {
                            ownership += OWNERSHIP_SCALE / n;
                        }
                    }
                }
                specificity.map(|specificity| Rank {
                    specificity,
                    ownership,
                    paths,
                })
            })
            .collect()
    }

    /// Per scope path, the most specific of `globs` that reaches it. Empty when either side
    /// is empty, so every caller sees "no overlap" the same way.
    fn hits(&self, globs: &[String]) -> Vec<Option<(usize, bool)>> {
        if self.paths.is_empty() || globs.is_empty() {
            return vec![None; self.paths.len()];
        }
        // A rotted glob is `doctor`'s finding, not a reason to refuse an answer here.
        let theirs: Vec<(&str, GlobMatcher)> = globs
            .iter()
            .filter_map(|g| compile(g).ok().map(|c| (g.as_str(), c.compile_matcher())))
            .collect();
        self.paths
            .iter()
            .zip(&self.each)
            .map(|(p, mine)| {
                theirs
                    .iter()
                    .filter(|(g, m)| m.is_match(p) || mine.is_match(g))
                    .map(|(g, _)| glob_specificity(g))
                    .max()
            })
            .collect()
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

/// (leading literal segments, no wildcard anywhere). `src/auth/login.ts` → `(3, true)`;
/// `src/auth/**` → `(2, false)`; `src/auth/Badge*` → `(2, false)`; `**` → `(0, false)`.
fn glob_specificity(g: &str) -> (usize, bool) {
    let wild = |s: &str| s.contains(['*', '?', '[', '{']);
    let literal = g.split('/').take_while(|seg| !wild(seg)).count();
    (literal, !wild(g))
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
    /// with `{p-xxxx}` provenance tokens; matched specs in rank order, within the budget
    pub spec_rules: Vec<StandingRule>,
    /// matched specs past the budget — NAMED, never silently dropped
    pub elided: Vec<ElidedSpec>,
    pub counts: Counts,
}

/// A spec the scope matched whose rules the budget did not reach. It rides in the payload
/// as one line — name, rule count, the command that shows it — so an agent is told what it
/// was not shown, and `rules --full` lifts the budget for the human checking.
#[derive(Debug, Clone, Serialize)]
pub struct ElidedSpec {
    pub spec: SpecName,
    pub rules: usize,
    pub rank: Rank,
}

/// The token estimate the budget is spent in. Four bytes per token is the usual English
/// prose figure and errs on the generous side for the `[anchor]`-heavy lines here; the
/// budget is a ceiling on context cost, not a promise of an exact count.
pub const CHARS_PER_TOKEN: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct StandingDecision {
    pub id: DecisionId,
    /// the id as a human reads it — `D-0174-dates-not-booleans`; the board shows it
    pub label: String,
    pub title: String,
    /// the provenance as a human reads it: a ticket or proposal source is labeled
    pub source: Option<String>,
    pub scope: Vec<String>,
    pub accepted: String,
    /// present only when the scope matched — context economy is the whole point
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StandingQuirk {
    pub id: QuirkId,
    /// the id as a human reads it — `q-11ba-stripe-webhooks-replay`
    pub label: String,
    pub title: String,
    pub paths: Vec<String>,
    pub severity: Severity,
    /// the source ticket, labeled
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

/// THE generator. `rules`, `rules --path`, and `prime` all call exactly this, under the
/// configured `[prime] spec_budget_tokens`.
pub fn build(s: &Snapshot, scope: &Scope) -> RulesDoc {
    let budget = match s.cfg.prime.spec_budget_tokens {
        0 => None,
        t => Some(t.saturating_mul(CHARS_PER_TOKEN)),
    };
    build_within(s, scope, budget)
}

/// `rules --full`: the same generator with the budget lifted. It is a separate entry
/// point rather than a flag on [`build`] so that the injection path cannot reach it by
/// accident — `prime` has no `--full`.
pub fn build_full(s: &Snapshot, scope: &Scope) -> RulesDoc {
    build_within(s, scope, None)
}

fn build_within(s: &Snapshot, scope: &Scope, budget_chars: Option<usize>) -> RulesDoc {
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
            label: s.label(&d.fm.id),
            id: d.fm.id.clone(),
            title: d.fm.title.clone(),
            source: d.fm.source.as_deref().map(|src| label_source(s, src)),
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
            label: s.label(&q.fm.id),
            id: q.fm.id.clone(),
            title: q.fm.title.clone(),
            paths: q.fm.paths.clone(),
            severity: q.fm.severity,
            source: q.fm.source.as_ref().map(|t| s.label(t)),
            // `quirk add` seeds the body from the title, so repeating it would double every
            // landmine in the payload for no information.
            body: (matched && !body.is_empty() && body != q.fm.title).then_some(body),
        });
    }
    // Landmine before gotcha before debt: the thing that costs you an afternoon leads.
    quirks.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.id.cmp(&b.id)));

    // Rank the matched specs: most specific glob first, then the one OWNING more of the
    // touched paths (a path shared with other specs counts for its share), then the one
    // touching more of them, then name — so the order is a function of the corpus and the
    // scope, never of `BTreeMap` iteration luck. Ranked as a corpus, because ownership is
    // a fact about all the specs together, not about one.
    let mut total_rules = 0usize;
    let candidates: Vec<&crate::model::Spec> = s
        .specs
        .values()
        .inspect(|spec| total_rules += spec.rules.len())
        .filter(|spec| !spec.rules.is_empty())
        .collect();
    let ranks = scope.rank_all(candidates.iter().map(|spec| spec.fm.code.as_slice()));
    let mut ranked: Vec<(Rank, &crate::model::Spec)> = candidates
        .iter()
        .zip(ranks)
        .filter_map(|(spec, rank)| rank.map(|r| (r, *spec)))
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

    // The budget is SOFT and spent in rank order: a spec is shown while the budget is not
    // yet spent, so the payload overshoots by at most one spec and the first-ranked spec
    // always shows in full, however large. Skipping a big spec to fit a small one behind it
    // would put a lesser match in front of an agent and hide the file's own spec — the
    // opposite of what the ranking is for. Spent is measured in the RENDERED bytes, from
    // the same line builders `render_text` uses, so the budget is a fact about the payload
    // and not about a second estimate of it.
    let mut spec_rules = Vec::new();
    let mut elided = Vec::new();
    let mut spent = 0usize;
    for (rank, spec) in ranked {
        if budget_chars.is_some_and(|b| spent >= b) {
            elided.push(ElidedSpec {
                spec: spec.name.clone(),
                rules: spec.rules.len(),
                rank,
            });
            continue;
        }
        spent += spec_line(&spec.name).len();
        for r in &spec.rules {
            let sr = StandingRule {
                spec: spec.name.clone(),
                anchor: r.anchor.clone(),
                text: r.text.clone(),
                provenance: r.provenance.clone(),
            };
            spent += rule_line(&sr).len();
            spec_rules.push(sr);
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
        elided,
    }
}

/// The spec heading line, exactly as rendered — the budget is charged in these bytes.
fn spec_line(name: &SpecName) -> String {
    format!("  {name}\n")
}

/// One rule bullet, exactly as rendered — the budget is charged in these bytes.
fn rule_line(r: &StandingRule) -> String {
    let mut o = format!("   [{}] {}", r.anchor, r.text);
    for p in &r.provenance {
        o.push_str(&format!(" {{{p}}}"));
    }
    o.push('\n');
    o
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
            o.push_str(&spec_line(&r.spec));
            last = Some(&r.spec);
        }
        o.push_str(&rule_line(r));
    }
    // Past the budget: still NAMED. The line carries the count, the reason and the fix, so
    // an agent knows what it was not shown and a human knows why the payload stopped.
    for e in &d.elided {
        o.push_str(&format!(
            "  {} — {} rule{} not shown, over the prime budget → kanspec spec show {}\n",
            e.spec,
            e.rules,
            if e.rules == 1 { "" } else { "s" },
            e.spec,
        ));
    }

    o.push_str("Nothing outside this list is served to agents. Closed proposals bind nothing.\n");
    o
}

/// A decision's `source:` is free text — a ticket id, a proposal item anchor, or anything a
/// human typed. Whichever of those names a record the snapshot holds is labeled; the rest
/// pass through untouched.
fn label_source(s: &Snapshot, src: &str) -> String {
    if let Some(id) = TicketId::parse(src)
        .ok()
        .filter(|i| s.tickets.contains_key(i))
    {
        return s.label(&id);
    }
    if let Some(id) = ProposalId::parse(src)
        .ok()
        .filter(|i| s.proposals.contains_key(i))
    {
        return s.label(&id);
    }
    src.to_string()
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
/// an accepted decision that has stood for `[windows] decision_review_secs`, whose source
/// proposal is closed and which no live ticket and no shipped rule references; and a spec
/// rule with no provenance token at all.
pub fn audit(s: &Snapshot, d: &RulesDoc) -> Vec<AuditWarning> {
    let mut out = Vec::new();

    for x in &d.decisions {
        // The normal path is promote → accept → close, minutes apart. Asking "still
        // wanted?" the moment the proposal closes fired on every decision the trial made
        // (t-e560); DESIGN.md's own example is a proposal closed 80 days ago. The age is
        // the decision's: how long it has been steering agents.
        let Some(age) = s
            .decisions
            .get(&x.id)
            .and_then(|dd| (s.now.date_naive() - dd.fm.date).to_std().ok())
        else {
            continue;
        };
        if age.as_secs() < s.cfg.windows.decision_review_secs {
            continue;
        }
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
        // A rule that shipped under the proposal cites it forever, and a decision that
        // came out of the same proposal is the reasoning behind that rule: still wanted.
        let shipped = s
            .specs
            .values()
            .flat_map(|sp| &sp.rules)
            .filter(|r| r.provenance.contains(&pid))
            .count();
        if refs == 0 && shipped == 0 {
            out.push(AuditWarning {
                subject: x.id.to_string(),
                message: format!(
                    "source proposal {pid} is closed; accepted {} ago; 0 references from \
                     open tickets or shipped rules — still wanted?",
                    crate::derive::short(age)
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

    fn one_rule_spec(name: &str, code: &[&str]) -> Spec {
        spec(
            name,
            code,
            &[(
                &format!("{name}.rule"),
                "A rule of ordinary length, like the real ones.",
            )],
        )
    }

    fn scope(paths: &[&str]) -> Scope {
        Scope::of(&paths.iter().map(|p| p.to_string()).collect::<Vec<_>>()).unwrap()
    }

    fn shown(d: &RulesDoc) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for r in &d.spec_rules {
            let n = r.spec.to_string();
            if out.last() != Some(&n) {
                out.push(n);
            }
        }
        out
    }

    fn named(d: &RulesDoc) -> Vec<String> {
        d.elided.iter().map(|e| e.spec.to_string()).collect()
    }

    #[test]
    fn a_glob_that_names_the_file_outranks_one_that_owns_the_directory() {
        assert!(glob_specificity("src/auth/login.ts") > glob_specificity("src/auth/**"));
        assert!(glob_specificity("src/auth/**") > glob_specificity("src/**"));
        assert!(glob_specificity("src/auth/Badge*") > glob_specificity("src/**"));
        assert_eq!(glob_specificity("src/auth/Badge*"), (2, false));
        assert_eq!(glob_specificity("**"), (0, false));
        assert_eq!(glob_specificity("src/auth/login.ts"), (3, true));
    }

    #[test]
    fn matched_specs_rank_by_specificity_then_by_how_much_of_the_branch_they_cover() {
        let mut s = snap();
        for sp in [
            one_rule_spec("wide", &["src/**"]),
            one_rule_spec("dir", &["src/auth/**"]),
            one_rule_spec("file", &["src/auth/login.ts"]),
            one_rule_spec("two", &["src/auth/**", "src/billing/**"]),
            one_rule_spec("elsewhere", &["docs/**"]),
        ] {
            s.specs.insert(sp.name.clone(), sp);
        }
        let d = build(&s, &scope(&["src/auth/login.ts", "src/billing/charge.ts"]));
        // `file` names the file; `two` and `dir` are equally specific but `two` covers both
        // touched paths; `wide` owns everything and so says the least about this branch.
        assert_eq!(shown(&d), ["file", "two", "dir", "wide"]);
        assert!(
            named(&d).is_empty(),
            "the default budget holds five one-liners"
        );
        assert_eq!(
            s.specs
                .values()
                .filter(|x| scope(&["src/auth/login.ts"]).touches(&x.fm.code))
                .count(),
            4,
            "`touches` is `rank(..).is_some()` — one definition of in-scope"
        );
    }

    /// The trial shape (t-2ba7, D-59): `models.py` and `schemas.py` are named by every spec
    /// in the repo, so covering them says nothing about which spec a branch is for. The
    /// 52-rule `tasks` spec used to outrank the 16-rule `materializer` spec the ticket was
    /// about, on a tiebreak that counted those two files as full coverage.
    #[test]
    fn coverage_by_files_every_spec_names_does_not_outrank_the_files_own_spec() {
        let mut s = snap();
        let shared = ["app/models.py", "app/schemas.py"];
        for sp in [
            one_rule_spec("tasks", &["app/tasks/**", shared[0], shared[1]]),
            one_rule_spec("templates", &["app/templates/**", shared[0], shared[1]]),
            one_rule_spec("recurrence", &["app/recurrence/**", shared[0], shared[1]]),
            one_rule_spec("materializer", &["app/materializer.py"]),
        ] {
            s.specs.insert(sp.name.clone(), sp);
        }
        let d = build(
            &s,
            &scope(&["app/materializer.py", "app/models.py", "app/schemas.py"]),
        );
        // Every spec matches with an exact glob. `materializer` owns one path outright;
        // the other three each own a third of two paths.
        assert_eq!(
            shown(&d),
            ["materializer", "recurrence", "tasks", "templates"]
        );
        let r = |name: &str| {
            let sc = scope(&["app/materializer.py", "app/models.py", "app/schemas.py"]);
            let specs: Vec<&crate::model::Spec> = s.specs.values().collect();
            let ranks = sc.rank_all(specs.iter().map(|x| x.fm.code.as_slice()));
            specs
                .iter()
                .zip(ranks)
                .find(|(x, _)| x.name.to_string() == name)
                .and_then(|(_, r)| r)
                .unwrap()
        };
        assert_eq!(r("materializer").ownership, OWNERSHIP_SCALE);
        assert_eq!(r("tasks").ownership, 2 * (OWNERSHIP_SCALE / 3));
        assert_eq!(r("tasks").paths, 2, "the raw count is still reported");
        // A spec that alone names two shared files does own them: the old order holds
        // whenever nobody else competes for a path.
        s.specs.remove(&SpecName::parse("templates").unwrap());
        s.specs.remove(&SpecName::parse("recurrence").unwrap());
        let d = build(
            &s,
            &scope(&["app/materializer.py", "app/models.py", "app/schemas.py"]),
        );
        assert_eq!(shown(&d), ["tasks", "materializer"]);
        // Ranked alone, an entity owns everything it matches — `touches` is unchanged.
        assert_eq!(
            scope(&["app/models.py"]).rank(&["app/models.py".to_string()]),
            Some(Rank {
                specificity: (2, true),
                ownership: OWNERSHIP_SCALE,
                paths: 1
            })
        );
    }

    #[test]
    fn the_budget_is_spent_in_rank_order_and_names_what_it_did_not_show() {
        let mut s = snap();
        for sp in [
            one_rule_spec("wide", &["src/**"]),
            one_rule_spec("dir", &["src/auth/**"]),
            one_rule_spec("file", &["src/auth/login.ts"]),
        ] {
            s.specs.insert(sp.name.clone(), sp);
        }
        let sc = scope(&["src/auth/login.ts"]);

        // One token: spent by the first spec, however small it is.
        s.cfg.prime.spec_budget_tokens = 1;
        let d = build(&s, &sc);
        assert_eq!(shown(&d), ["file"], "the first-ranked spec always shows");
        assert_eq!(
            named(&d),
            ["dir", "wide"],
            "the rest are NAMED, in rank order"
        );
        assert_eq!(d.elided[0].rules, 1);
        assert_eq!(
            d.counts.spec_rules, 3,
            "the corpus count is not the shown count"
        );
        let text = render_text(&d);
        assert!(
            text.contains(
                "  dir — 1 rule not shown, over the prime budget → kanspec spec show dir\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("[file.rule]") && !text.contains("[dir.rule]"),
            "{text}"
        );

        // Zero lifts it; so does `rules --full`, whatever the config says.
        s.cfg.prime.spec_budget_tokens = 0;
        assert_eq!(shown(&build(&s, &sc)), ["file", "dir", "wide"]);
        s.cfg.prime.spec_budget_tokens = 1;
        let full = build_full(&s, &sc);
        assert_eq!(shown(&full), ["file", "dir", "wide"]);
        assert!(full.elided.is_empty());
        assert!(!render_text(&full).contains("not shown"));
    }

    #[test]
    fn the_budget_is_soft_so_the_spec_that_crosses_it_still_shows_whole() {
        let mut s = snap();
        let first = one_rule_spec("file", &["src/auth/login.ts"]);
        let second = spec(
            "dir",
            &["src/auth/**"],
            &[
                ("dir.a", "First of three."),
                ("dir.b", "Second of three."),
                ("dir.c", "Third of three."),
            ],
        );
        let third = one_rule_spec("wide", &["src/**"]);
        for sp in [first, second, third] {
            s.specs.insert(sp.name.clone(), sp);
        }
        let sc = scope(&["src/auth/login.ts"]);

        // The exact bytes the first spec renders to, so the budget lands INSIDE the second.
        let alone = build(&s, &scope(&["src/auth/login.ts"]));
        let first_bytes: usize = spec_line(&alone.spec_rules[0].spec).len()
            + alone
                .spec_rules
                .iter()
                .filter(|r| r.spec.as_str() == "file")
                .map(|r| rule_line(r).len())
                .sum::<usize>();
        s.cfg.prime.spec_budget_tokens = first_bytes / CHARS_PER_TOKEN + 1;

        let d = build(&s, &sc);
        assert_eq!(
            shown(&d),
            ["file", "dir"],
            "dir crossed the budget and still shows"
        );
        assert_eq!(
            d.spec_rules
                .iter()
                .filter(|r| r.spec.as_str() == "dir")
                .count(),
            3,
            "whole, not truncated mid-spec"
        );
        assert_eq!(named(&d), ["wide"]);
    }

    #[test]
    fn a_rule_less_spec_is_neither_shown_nor_named() {
        let mut s = corpus();
        let empty = spec("scaffold", &["src/auth/**"], &[]);
        s.specs.insert(empty.name.clone(), empty);
        s.cfg.prime.spec_budget_tokens = 1;
        let d = build(&s, &scope(&["src/auth/login.ts"]));
        assert_eq!(shown(&d), ["auth"]);
        assert!(named(&d).is_empty(), "nothing to show is nothing to elide");
    }

    #[test]
    fn audit_flags_a_decision_whose_source_proposal_closed_with_nothing_left_referencing_it() {
        let mut s = corpus();
        s.closed_ids.insert("p-7de2".to_string());
        // The corpus spec's rule carries `{p-7de2}`; it comes back below as the reference
        // that keeps the decision wanted.
        let auth = s.specs.remove(&SpecName::parse("auth").unwrap()).unwrap();
        let flagged = |s: &Snapshot| {
            let d = build(s, &Scope::none());
            audit(s, &d)
                .into_iter()
                .find(|x| x.subject == "D-8c1a")
                .map(|x| x.message)
        };
        // Accepted yesterday, proposal closed today: the normal promote → accept → close
        // path, not decay (t-e560). The corpus decision is dated 2026-09-02 against a
        // 2026-08-31 clock — a future date is age zero, never a warning.
        assert_eq!(flagged(&s), None, "a fresh decision is not asked about");
        s.decisions
            .get_mut(&DecisionId::parse("D-8c1a").unwrap())
            .unwrap()
            .fm
            .date = NaiveDate::from_ymd_opt(2026, 6, 1).unwrap();
        let msg = flagged(&s).expect("a 91-day-old decision with a closed source is asked about");
        assert!(
            msg.contains("p-7de2 is closed") && msg.contains("accepted 91d ago"),
            "{msg}"
        );
        // The window is a knob.
        s.cfg.windows.decision_review_secs = 365 * 86_400;
        assert_eq!(flagged(&s), None);
        s.cfg.windows.decision_review_secs = 0;
        assert!(
            flagged(&s).is_some(),
            "0 = ask as soon as the proposal closes"
        );

        // A rule that shipped under the proposal is a reference: the decision is the
        // reasoning behind a rule still steering agents.
        assert_eq!(
            auth.rules[0].provenance,
            [ProposalId::parse("p-7de2").unwrap()]
        );
        s.specs.insert(auth.name.clone(), auth);
        assert_eq!(flagged(&s), None, "a shipped rule cites the proposal");
    }
}
