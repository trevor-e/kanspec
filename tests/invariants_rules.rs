//! `tests/invariants_rules.rs`
//!
//! Proves **invariant 3**: `kanspec prime`'s stdout starts with `kanspec rules`' stdout,
//! byte for byte, on the REAL BINARY, across N different path scopes.
//!
//! Running the binary is the whole point. Calling `rulesdoc::render_text` twice and
//! comparing proves only that a pure function is pure; what can actually break invariant 3
//! is a *handler* — a header line, a trailing newline, a colour code, a `writeln!` where a
//! `write!` belonged — sitting between the generator and the terminal. Only a diff of two
//! real processes' stdout sees that. And the scopes are parameterized because byte-identity
//! is trivial when both sides render everything: it becomes meaningful exactly when
//! path-scoped injection is deciding which decision bodies and which quirks appear.
//!
//! Also proves what the identity is *for*: invariant 4 (a closed proposal's prose is
//! structurally unreachable from the generator), path-scoped context economy, and the
//! mechanical regeneration of the committed projections.
//!
//! Owner: **S6**.

mod common;

use common::TestRepo;

/// The scopes the identity is checked under. Unscoped is the audit surface; a concrete
/// file and a glob are the two shapes a human types; a scope matching nothing must still
/// produce a complete one-liner list; two paths at once must not double anything.
fn scopes() -> Vec<Vec<&'static str>> {
    vec![
        vec![],
        vec!["src/auth/login.ts"],
        vec!["src/billing/**"],
        vec!["nonexistent/**"],
        vec!["src/auth/login.ts", "src/billing/charge.ts"],
    ]
}

fn with_paths(verb: &str, scope: &[&str]) -> Vec<String> {
    let mut args = vec![verb.to_string()];
    for p in scope {
        args.push("--path".to_string());
        args.push((*p).to_string());
    }
    args
}

/// A corpus with both halves of the path-scoping story: an auth capability and a billing
/// capability, a decision and a quirk in each, and rule bullets carrying `{p-xxxx}`
/// provenance exactly as an implementation branch writes them.
fn seed(repo: &TestRepo) {
    repo.ks([
        "spec",
        "new",
        "auth",
        "--feature",
        "Login (JWT 24h), lockout after 5 failures, GitHub OAuth",
        "--code",
        "src/auth/**",
    ])
    .ok();
    repo.ks([
        "spec",
        "new",
        "billing",
        "--feature",
        "Stripe charges + smart retries",
        "--code",
        "src/billing/**",
    ])
    .ok();

    append_rule(
        repo,
        "auth",
        "- [auth.lockout] 5 failed logins within 10m locks the account for 15m. {p-7de2}",
    );
    append_rule(
        repo,
        "auth",
        "- [auth.jwt] Login issues a JWT valid 24h in an httpOnly cookie. {p-02cc}",
    );
    append_rule(
        repo,
        "billing",
        "- [billing.cents] Money amounts are integer cents. {p-19f0}",
    );

    repo.ks([
        "quirk",
        "add",
        "Stripe webhooks replay in staging",
        "--paths",
        "src/billing/**",
        "--sev",
        "landmine",
    ])
    .ok();
    repo.ks([
        "quirk",
        "add",
        "Session middleware rotates on every 401",
        "--paths",
        "src/auth/**",
        "--sev",
        "gotcha",
    ])
    .ok();

    accept(repo, "Rate-limit state lives in Redis only", "src/auth/**");
    accept(repo, "Money amounts are integer cents", "src/billing/**");
}

fn append_rule(repo: &TestRepo, spec: &str, bullet: &str) {
    let rel = format!(".kanspec/specs/{spec}.md");
    let body = repo.read(&rel);
    repo.write(&rel, &format!("{body}{bullet}\n"));
}

/// `decide` mints a PROPOSED decision (invariant 8: agents never self-accept); the human
/// accepts it. `TestRepo` runs as `KANSPEC_ACTOR_KIND=human`, which is what makes the
/// second half legal at all.
fn accept(repo: &TestRepo, title: &str, scope: &str) -> String {
    let out: serde_json::Value = repo.json(&["decide", title, "--scope", scope]);
    let id = out["id"]
        .as_str()
        .expect("decide reports the minted id")
        .to_string();
    repo.ks(["accept", &id]).ok();
    id
}

// ─────────────────────────────────────────────────────────────────────────────
// Invariant 3
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn prime_stdout_starts_with_rules_stdout_across_every_scope() {
    let repo = TestRepo::new();
    seed(&repo);

    for scope in scopes() {
        let r = repo.ks(with_paths("rules", &scope)).ok().stdout;
        let p = repo.ks(with_paths("prime", &scope)).ok().stdout;
        assert!(
            !r.trim().is_empty(),
            "the rules surface is empty for scope {scope:?}"
        );
        assert!(
            p.starts_with(&r),
            "invariant 3 broke for scope {scope:?}\n--- rules ---\n{r}\n--- prime ---\n{p}"
        );
        // The audit surface IS the injection surface: nothing may be *added* on the way to
        // an agent either, so prime's extra bytes must be the live slice and nothing else.
        assert!(
            p[r.len()..].starts_with("\nLIVE SLICE"),
            "prime grew something between the generator and the live slice: {:?}",
            &p[r.len()..r.len() + 40.min(p.len() - r.len())]
        );
    }
}

#[test]
fn the_identity_is_meaningful_because_the_scopes_actually_differ() {
    let repo = TestRepo::new();
    seed(&repo);

    let unscoped = repo.ks(["rules"]).ok().stdout;
    let auth = repo
        .ks(["rules", "--path", "src/auth/login.ts"])
        .ok()
        .stdout;
    let billing = repo.ks(["rules", "--path", "src/billing/**"]).ok().stdout;
    let nothing = repo.ks(["rules", "--path", "nonexistent/**"]).ok().stdout;

    assert_ne!(auth, billing, "path scoping changed nothing");
    assert_ne!(auth, unscoped);
    assert_ne!(nothing, auth);

    // An agent working in src/auth/ never pays for the billing quirks.
    assert!(auth.contains("Session middleware rotates"), "{auth}");
    assert!(!auth.contains("Stripe webhooks replay"), "{auth}");
    assert!(billing.contains("Stripe webhooks replay"), "{billing}");
    assert!(!billing.contains("Session middleware rotates"), "{billing}");

    // …but it is still TOLD the other decisions exist. One-liners are complete; full text
    // is what the path match earns.
    for text in [&unscoped, &auth, &billing, &nothing] {
        assert!(
            text.contains("Rate-limit state lives in Redis only"),
            "{text}"
        );
        assert!(text.contains("Money amounts are integer cents"), "{text}");
    }
    assert!(
        auth.contains("Sliding window") || !unscoped.contains("Sliding window"),
        "a scoped decision body must not leak into the unscoped listing"
    );
    // Scoped injection pulls in the touched capability's rules, unscoped counts them.
    assert!(auth.contains("[auth.lockout]"), "{auth}");
    assert!(!unscoped.contains("[auth.lockout]"), "{unscoped}");
    assert!(
        unscoped.contains("SPEC RULES: 3 across 2 capabilities"),
        "{unscoped}"
    );
}

#[test]
fn the_json_halves_agree_too() {
    let repo = TestRepo::new();
    seed(&repo);
    for scope in scopes() {
        let r: serde_json::Value = repo.json(
            &with_paths("rules", &scope)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        let p: serde_json::Value = repo.json(
            &with_paths("prime", &scope)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            r["data"], p["standing"],
            "prime --json .standing != rules --json .data for scope {scope:?}"
        );
    }
}

#[test]
fn a_proposed_decision_never_reaches_an_agent_until_a_human_accepts_it() {
    let repo = TestRepo::new();
    seed(&repo);
    let out: serde_json::Value = repo.json(&[
        "decide",
        "Sessions move to Postgres",
        "--scope",
        "src/auth/**",
    ]);
    let id = out["id"].as_str().unwrap().to_string();
    assert_eq!(out["status"], "proposed");

    let before = repo
        .ks(["rules", "--path", "src/auth/login.ts"])
        .ok()
        .stdout;
    assert!(!before.contains("Sessions move to Postgres"), "{before}");

    // It is VISIBLE but not BINDING, which is the distinction DESIGN.md draws: the
    // standing-rules section (everything before the live slice) must not carry it, while
    // the live slice raises it as a thing a human owes a verb on.
    let primed = repo
        .ks(["prime", "--path", "src/auth/login.ts"])
        .ok()
        .stdout;
    let (standing, live) = primed
        .split_once("\nLIVE SLICE")
        .expect("prime prints a live slice");
    assert!(
        !standing.contains("Sessions move to Postgres"),
        "{standing}"
    );
    assert!(
        live.contains("proposed decision awaits a human") && live.contains(&id),
        "a pending decision must sit in the YOU section until accepted:\n{live}"
    );

    repo.ks(["accept", &id]).ok();
    let after = repo
        .ks(["rules", "--path", "src/auth/login.ts"])
        .ok()
        .stdout;
    assert!(after.contains("Sessions move to Postgres"), "{after}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Invariant 4 — closed proposals bind nothing
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_closed_proposals_prose_is_structurally_unreachable_from_the_generator() {
    let repo = TestRepo::new();
    seed(&repo);
    repo.write(
        ".kanspec/proposals/closed/p-19f0-money-as-cents/proposal.md",
        "---\nid: p-19f0\ntitle: Money as cents\nstatus: closed\nspecs: [billing]\n\
         approved: null\nledger: []\ncreated: 2026-06-11\n---\n\
         ## Prescriptions\n- [p1] WE-WILL-ALWAYS-ROUND-HALF-UP everywhere, forever.\n",
    );

    for scope in scopes() {
        for verb in ["rules", "prime"] {
            let out = repo.ks(with_paths(verb, &scope)).ok().stdout;
            assert!(
                !out.contains("WE-WILL-ALWAYS-ROUND-HALF-UP"),
                "{verb} served a CLOSED proposal's prose for scope {scope:?}:\n{out}"
            );
        }
    }
    // The claim is on the page, too — an agent reading the payload is told the rule.
    assert!(repo
        .ks(["rules"])
        .ok()
        .stdout
        .contains("Closed proposals bind nothing."));
}

// ─────────────────────────────────────────────────────────────────────────────
// The generated projections
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_scan_after_a_spec_edit_rewrites_the_root_projections() {
    let repo = TestRepo::new();
    seed(&repo);
    repo.ks(["scan"]).ok();

    let before = repo.read("KANSPEC-FEATURES.md");
    assert!(before.contains("GENERATED by kanspec"), "{before}");
    assert!(before.contains("Login (JWT 24h)"), "{before}");

    // The spec is edited the way an implementation branch edits one.
    let rel = ".kanspec/specs/auth.md";
    repo.write(rel, &repo.read(rel).replace("JWT 24h", "JWT 12h — rotated"));

    repo.ks(["scan"]).ok();
    let after = repo.read("KANSPEC-FEATURES.md");
    assert_ne!(before, after, "the committed projection silently rotted");
    assert!(after.contains("JWT 12h — rotated"), "{after}");
}

#[test]
fn accepting_a_decision_rewrites_the_architecture_projection() {
    let repo = TestRepo::new();
    seed(&repo);
    let arch = repo.read("KANSPEC-ARCHITECTURE.md");
    assert!(arch.contains("GENERATED by kanspec"), "{arch}");
    assert!(
        arch.contains("Rate-limit state lives in Redis only"),
        "{arch}"
    );
    assert!(arch.contains("Stripe webhooks replay in staging"), "{arch}");
    assert!(
        !arch.contains("Session middleware rotates"),
        "only landmine-grade quirks belong on the architecture page:\n{arch}"
    );

    let id: serde_json::Value =
        repo.json(&["decide", "Sessions live in Redis", "--scope", "src/auth/**"]);
    let id = id["id"].as_str().unwrap().to_string();
    assert!(
        !repo
            .read("KANSPEC-ARCHITECTURE.md")
            .contains("Sessions live in Redis"),
        "a proposed decision is not architecture"
    );
    repo.ks(["accept", &id]).ok();
    assert!(repo
        .read("KANSPEC-ARCHITECTURE.md")
        .contains("Sessions live in Redis"));
}

#[test]
fn regeneration_is_idempotent_so_a_checkout_does_not_dirty_the_tree() {
    let repo = TestRepo::new();
    seed(&repo);
    repo.ks(["scan"]).ok();
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "--quiet", "-m", "projections"]);

    // The post-merge / post-checkout hooks run this on every checkout.
    repo.ks(["scan"]).ok();
    repo.ks(["scan"]).ok();
    let dirty = repo.git(&[
        "status",
        "--porcelain",
        "--",
        "KANSPEC-FEATURES.md",
        "KANSPEC-ARCHITECTURE.md",
    ]);
    assert!(
        dirty.trim().is_empty(),
        "a scan that changed nothing rewrote the projections: {dirty}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// The staleness attestation (D-10)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn features_confirm_writes_a_stale_ack_the_spec_can_still_be_read_back_through() {
    let repo = TestRepo::new();
    seed(&repo);
    repo.ks(["scan"]).ok();

    repo.ks([
        "features",
        "--confirm",
        "auth",
        "--why",
        "renamed a helper, no behaviour change",
    ])
    .ok();

    let md = repo.read(".kanspec/specs/auth.md");
    assert!(md.contains("stale_ack: {"), "{md}");
    assert!(md.contains("renamed a helper"), "{md}");
    // git-tracked, in the spec's own frontmatter — it survives `rm -rf cache/` (D-10).
    assert!(!md.contains("merges_since"), "never a counter: {md}");

    // The decisive half: the attestation must PARSE BACK. A `stale_ack` that reads as a
    // string or a sequence makes the whole spec unloadable, which would take the feature
    // map, `rules` and `prime` down with it.
    let shown: serde_json::Value = repo.json(&["spec", "show", "auth"]);
    assert_eq!(shown["name"], "auth");
    assert!(repo.ks(["rules"]).ok().stdout.contains("SPEC RULES"));

    // And a SECOND attestation replaces the first in place rather than tripping R-9's
    // multi-line refusal — the value is emitted flow-style for exactly this reason.
    repo.ks([
        "features",
        "--confirm",
        "auth",
        "--why",
        "still no behaviour change",
    ])
    .ok();
    let md = repo.read(".kanspec/specs/auth.md");
    assert_eq!(md.matches("stale_ack:").count(), 1, "{md}");
    assert!(md.contains("still no behaviour change"), "{md}");
}

#[test]
fn features_confirm_without_a_reason_is_refused() {
    let repo = TestRepo::new();
    seed(&repo);
    // clap's `requires = "why"` catches the missing flag; the handler catches a blank one.
    let r = repo.ks(["features", "--confirm", "auth", "--why", "   "]);
    assert_ne!(
        r.code, 0,
        "an attestation without a reason is not an attestation"
    );
    assert!(r.stderr.contains("--why"), "{}", r.stderr);
}

// ─────────────────────────────────────────────────────────────────────────────
// The audit surface, and the token budget
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn audit_prints_only_its_warnings_so_the_default_stdout_stays_the_injection_surface() {
    let repo = TestRepo::new();
    seed(&repo);
    append_rule(
        &repo,
        "auth",
        "- [auth.oauth] GitHub OAuth is the only SSO.",
    );

    let audit = repo.ks(["rules", "--audit"]).ok().stdout;
    assert!(audit.contains("auth.oauth"), "{audit}");
    assert!(audit.contains("no provenance token"), "{audit}");
    assert!(
        !audit.contains("STANDING RULES"),
        "--audit must not re-render the rules: {audit}"
    );

    // …and the default surface is untouched by the audit existing.
    let plain = repo.ks(["rules"]).ok().stdout;
    assert!(plain.starts_with("STANDING RULES"), "{plain}");
    assert!(repo.ks(["prime"]).ok().stdout.starts_with(&plain));
}

#[test]
fn adopt_refuses_out_loud_rather_than_exiting_zero_having_changed_nothing() {
    let repo = TestRepo::new();
    seed(&repo);
    append_rule(
        &repo,
        "auth",
        "- [auth.oauth] GitHub OAuth is the only SSO.",
    );
    let r = repo.ks(["rules", "--adopt"]);
    assert_eq!(
        r.code, 1,
        "a no-op that exits 0 is how an audit comes to look clean"
    );
    assert!(r.stderr.contains("provenance"), "{}", r.stderr);
    // Invariant 9: every refusal names its next command.
    assert!(r.stderr.contains("→"), "{}", r.stderr);
}

/// DESIGN.md budgets `prime` at ~1.5k tokens. Measured on a corpus deliberately larger
/// than a real repo's *scoped* payload: 2 capabilities, 4 decisions, 6 quirks.
///
/// ~4 bytes per token is the usual English rule of thumb; the ceiling is set at 2.5k so
/// the test fails on a regression (an unscoped body dump, an uncapped ready queue) rather
/// than on ordinary corpus growth.
#[test]
fn the_prime_payload_stays_inside_its_token_budget() {
    let repo = TestRepo::new();
    seed(&repo);
    for i in 0..4 {
        repo.ks([
            "quirk",
            "add",
            &format!("Landmine number {i} with a realistically wordy one-line description"),
            "--paths",
            "src/auth/**",
            "--sev",
            "landmine",
        ])
        .ok();
    }
    accept(
        &repo,
        "Sessions live in Redis with a 24h TTL",
        "src/auth/**",
    );
    accept(
        &repo,
        "All money crosses the wire as integer cents",
        "src/billing/**",
    );
    repo.ks(["scan"]).ok();

    for scope in [vec![], vec!["src/auth/login.ts"]] {
        let out = repo.ks(with_paths("prime", &scope)).ok().stdout;
        let tokens = out.len().div_ceil(4);
        println!("prime{scope:?}: {} bytes ≈ {tokens} tokens", out.len());
        assert!(
            tokens < 2_500,
            "prime{scope:?} is ≈{tokens} tokens, past the ~1.5k budget:\n{out}"
        );
    }
}

#[test]
fn prime_stamps_the_merge_state_it_is_reporting_from() {
    let repo = TestRepo::new();
    seed(&repo);
    // SessionStart runs `prime`, and DESIGN.md lists SessionStart among `scan`'s triggers:
    // a cold session must not be handed a merge state nobody has refreshed.
    let cold = repo.ks(["prime"]).ok().stdout;
    assert!(cold.contains("LIVE SLICE"), "{cold}");
    assert!(
        cold.contains("merge state checked"),
        "prime must stamp the cache age it read from:\n{cold}"
    );
    assert!(
        repo.exists(".kanspec/cache/gitstate.json"),
        "prime's throttled scan never ran"
    );
}

/// INVARIANT 8, THE AGENT HALF — end to end, on the real binary.
///
/// "Agents never self-accept standing rules." The mechanism is `HumanActor`, whose
/// constructor refuses an `Actor::Agent`, and which `plan_accept` / `plan_revoke` /
/// `plan_supersede` take by reference (D-18). Until round C this could only be proven at
/// unit level, because the harness hard-coded `KANSPEC_ACTOR_KIND=human` with no override —
/// so the refusal that the entire propose-then-human-accept design rests on had never once
/// been observed coming out of the actual binary. `TestRepo::ks_env` exists for this.
///
/// The positive control matters as much as the refusals: the same three commands, same
/// repo, same ids, run as a HUMAN, must succeed. Otherwise this test would still pass if
/// `accept` were broken for everyone.
#[test]
fn an_agent_cannot_accept_revoke_or_supersede_a_standing_rule() {
    let repo = TestRepo::new();
    repo.ks(["spec", "new", "auth", "--code", "src/auth/**"])
        .ok();

    // An AGENT may propose — that half must keep working, or agents cannot record anything.
    let agent = &[
        ("KANSPEC_ACTOR", "claude/sess-a91"),
        ("KANSPEC_ACTOR_KIND", "agent"),
    ];
    let proposed = repo.ks_env(
        [
            "decide",
            "rate limits live in middleware",
            "--scope",
            "src/auth/**",
        ],
        agent,
    );
    assert_eq!(
        proposed.code, 0,
        "an agent must still be able to PROPOSE:\n{}",
        proposed.stderr
    );
    let did = proposed
        .stdout
        .split_whitespace()
        .find(|w| w.starts_with("D-"))
        .expect("the minted decision id")
        .to_string();

    // ...but it may not make its own proposal binding, retire one, or swap one out.
    for args in [
        vec!["accept", did.as_str()],
        vec!["revoke", did.as_str(), "--why", "changed my mind"],
        vec!["supersede", did.as_str(), "--with", "cap retries at three"],
    ] {
        let r = repo.ks_env(&args, agent);
        assert_ne!(
            r.code,
            0,
            "invariant 8: an agent session must not be able to `{}`:\n{}",
            args.join(" "),
            r.stdout
        );
        let said = format!("{}{}", r.stdout, r.stderr);
        assert!(
            said.contains("human") || said.contains("agent"),
            "the refusal must say whose act this is — `{}` said: {said}",
            args.join(" ")
        );
    }

    // The record itself never moved: still `proposed`, and still not standing.
    let after = repo.read(&format!(".kanspec/decisions/{did}.md"));
    assert!(after.contains("status: proposed"), "{after}");
    assert!(
        !repo.ks(["rules"]).ok().stdout.contains("accepted"),
        "a decision no human accepted must not appear as a standing rule"
    );

    // POSITIVE CONTROL: the same verb, as a human, works.
    repo.ks(["accept", &did]).ok();
    let after = repo.read(&format!(".kanspec/decisions/{did}.md"));
    assert!(after.contains("status: accepted"), "{after}");
}
