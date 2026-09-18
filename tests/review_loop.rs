//! The v0.2 review loop end to end: `propose → review → comment → approve → close`.
//!
//! The three properties this file exists to hold, none of which a unit test can see:
//! approval CANNOT paper over unresolved feedback, `close` CANNOT drop an item silently,
//! and a closed proposal binds nothing.

mod common;

use common::TestRepo;

/// A proposal with one Change, one typed prescription and one ticket bullet — the shape
/// DESIGN.md's worked example uses.
fn seed(repo: &TestRepo) {
    repo.ks([
        "spec",
        "new",
        "auth",
        "--feature",
        "Login",
        "--code",
        "src/**",
    ])
    .ok();
    repo.ks(["propose", "Login rate limiting", "--spec", "auth"])
        .ok();
}

fn only_proposal(repo: &TestRepo) -> String {
    let dir = repo.root.join(".kanspec/proposals");
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.starts_with("p-"))
        .expect("a proposal directory")
}

fn body(repo: &TestRepo, p: &str, items: &str) {
    let rel = format!(".kanspec/proposals/{p}/proposal.md");
    let src = repo.read(&rel);
    let (fm, _) = src.split_once("\n---\n").expect("frontmatter");
    repo.write(&rel, &format!("{fm}\n---\n{items}"));
}

#[test]
fn propose_scaffolds_the_one_page_format_and_nothing_else() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let src = repo.read(&format!(".kanspec/proposals/{p}/proposal.md"));
    for section in [
        "## Why",
        "## Changes",
        "## Testing and verification",
        "## Prescriptions",
        "## Tickets",
    ] {
        assert!(src.contains(section), "{src}");
    }
    // The things OpenSpec had and kanspec dropped, asserted as absences.
    assert!(!src.contains("SHALL"), "{src}");
    assert!(!src.contains("#### Scenario"), "{src}");
    assert!(
        !repo.exists(&format!(".kanspec/proposals/{p}/design.md")),
        "no design.md artifact"
    );
    assert!(
        !repo.exists(&format!(".kanspec/proposals/{p}/tasks.md")),
        "no tasks.md artifact"
    );
    assert!(src.contains("status: draft"), "{src}");

    // p-67f0 c3: the scaffold cuts along the capability by default — one `[tN]`
    // placeholder per named spec — and carries the sizing norm as prose the author reads.
    assert!(src.contains("- [t1] (spec: auth) "), "{src}");
    assert!(!src.contains("[t2]"), "one spec, one placeholder:\n{src}");
    assert!(src.contains("One ticket is one PR is one session"), "{src}");

    // The scaffold is still a valid proposal that reviews cleanly: the placeholder is an
    // item with an empty title, the norm is prose the item parser skips.
    let id = &p[..6];
    let out: serde_json::Value = repo.json(&["review", id]);
    assert_eq!(out["status"], "review");
    assert_eq!(out["budgets"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(out["budgets"][0]["spec"], "auth");
}

#[test]
fn propose_pre_mints_one_ticket_placeholder_per_capability() {
    let repo = TestRepo::new();
    seed(&repo);
    repo.ks([
        "spec",
        "new",
        "billing",
        "--feature",
        "Charges",
        "--code",
        "src/billing/**",
    ])
    .ok();
    repo.ks([
        "propose",
        "Two capabilities",
        "--spec",
        "auth",
        "--spec",
        "billing",
    ])
    .ok();
    let dir = repo.root.join(".kanspec/proposals");
    let two = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.contains("two-capabilities"))
        .expect("the second proposal directory");
    let src = repo.read(&format!(".kanspec/proposals/{two}/proposal.md"));
    assert!(
        src.contains("- [t1] (spec: auth) \n- [t2] (spec: billing) \n"),
        "{src}"
    );

    // No spec at all: one bare placeholder, same norm.
    repo.ks(["propose", "Nothing named"]).ok();
    let dir = repo.root.join(".kanspec/proposals");
    let none = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.contains("nothing-named"))
        .expect("the third proposal directory");
    let src = repo.read(&format!(".kanspec/proposals/{none}/proposal.md"));
    assert!(src.contains("## Tickets\n"), "{src}");
    assert!(src.contains("- [t1] \n"), "{src}");
    assert!(
        !src.contains("- [t1] (spec:"),
        "no capability, no spec on the placeholder:\n{src}"
    );
    assert!(src.contains("One ticket is one PR is one session"), "{src}");
}

/// The directory name carries human-readable text, which is the whole reason
/// `Op::CreateProposal` exists instead of a plain `CreateEntity`: the id alone would give
/// `proposals/p-7de2/`, which is fine for the machine and useless for the teammate
/// browsing the repo on GitHub.
#[test]
fn a_proposal_directory_is_slugged_and_still_resolves_by_bare_id() {
    let repo = TestRepo::new();
    repo.ks([
        "spec",
        "new",
        "auth",
        "--feature",
        "Login",
        "--code",
        "src/**",
    ])
    .ok();
    repo.ks([
        "propose",
        "Move browser auth to HttpOnly cookies",
        "--spec",
        "auth",
    ])
    .ok();
    let dir = only_proposal(&repo);
    assert!(
        dir.contains("-move-browser-auth-to-httponly-cookies"),
        "the slug is the human-readable half: {dir}"
    );

    // …and every verb still takes the BARE id, because the id is what is authoritative.
    let id = &dir[..6];
    assert_eq!(repo.ks(["review", id]).code, 0);
    assert_eq!(repo.ks(["doctor"]).code, 0);
    // Minting the same id twice would take an existing comment log with it.
    assert!(repo
        .read(&format!(".kanspec/proposals/{dir}/proposal.md"))
        .contains(&format!("id: {id}")));
}

#[test]
fn approve_refuses_while_a_thread_is_unresolved_and_resolving_it_is_the_waiver() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n\n## Prescriptions\n\n## Tickets\n- [t1] Rate-limit login endpoint\n",
    );
    repo.ks(["review", id]).ok();

    let r = repo.ks(["comment", "add", &format!("{id}#c1"), "--body", "too broad"]);
    assert_eq!(r.code, 0, "{}", r.stderr);

    // THE gate.
    let blocked = repo.ks(["approve", id]);
    assert_eq!(blocked.code, 1, "{}", blocked.stdout);
    assert!(blocked.stderr.contains("unresolved"), "{}", blocked.stderr);
    // Invariant 9: a refusal names its next command.
    assert!(blocked.stderr.contains("→"), "{}", blocked.stderr);

    // The waiver is a RECORDED act, not a flag: resolving with a note is the only way past.
    let cm = repo
        .read(&format!(".kanspec/proposals/{p}/comments.jsonl"))
        .lines()
        .next()
        .and_then(|l| l.split("\"id\":\"").nth(1).map(|s| s[..7].to_string()))
        .expect("a thread id");
    repo.ks(["comment", "resolve", &cm, "--note", "narrowed to lockout"])
        .ok();

    let ok = repo.ks(["approve", id]).ok();
    assert!(ok.stdout.contains("approved"), "{}", ok.stdout);
    // `[tN]` bullets became real board tickets.
    assert!(repo
        .ks(["ls"])
        .ok()
        .stdout
        .contains("Rate-limit login endpoint"));
    // Re-approving is refused rather than re-stamping and re-minting.
    assert_eq!(repo.ks(["approve", id]).code, 1);
}

#[test]
fn close_refuses_to_drop_an_item_silently_and_names_the_command_per_item() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n- [c2] emit an event\n\n## Prescriptions\n- [p1] (promote: decision) lockout state lives in Redis\n\n## Tickets\n",
    );
    repo.ks(["review", id]).ok();
    repo.ks(["approve", id]).ok();

    // Nothing shipped, nothing promoted: every item is unmet and each is NAMED.
    let r = repo.ks(["close", id]);
    assert_eq!(r.code, 1, "{}", r.stdout);
    for item in ["[c1]", "[c2]", "[p1]"] {
        assert!(r.stderr.contains(item), "{item} unnamed in: {}", r.stderr);
    }
    assert!(r.stderr.contains("promote"), "{}", r.stderr);

    // A spec rule carrying the proposal's token is what makes a Change `shipped` — a fact
    // about the corpus, never a flag anybody types.
    let spec = repo.read(".kanspec/specs/auth.md");
    repo.write(
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.lockout] 5 failed logins lock the account. {{{id}}}\n"),
    );
    repo.ks([
        "promote",
        &format!("{id}#p1"),
        "--as",
        "decision",
        "--scope",
        "src/**",
    ])
    .ok();

    // c2 still has no home, so the gate still holds.
    let still = repo.ks(["close", id]);
    assert_eq!(still.code, 1, "{}", still.stdout);
    assert!(still.stderr.contains("[c2]"), "{}", still.stderr);

    let done = repo.ks(["close", id, "--followup", "c2"]).ok();
    assert!(done.stdout.contains("shipped"), "{}", done.stdout);
    assert!(done.stdout.contains("followup"), "{}", done.stdout);

    // Rule 1: closed proposals bind nothing — status AND location agree, and `doctor`
    // is the thing that checks they do.
    let closed = repo.read(&format!(".kanspec/proposals/closed/{p}/proposal.md"));
    assert!(closed.contains("status: closed"), "{closed}");
    assert!(!repo.exists(&format!(".kanspec/proposals/{p}/proposal.md")));
    assert_eq!(
        repo.ks(["doctor"]).code,
        0,
        "{}",
        repo.ks(["doctor"]).stdout
    );
    // Leftover scope became a VISIBLE board ticket, which is the whole point of followup.
    assert!(repo.ks(["ls"]).ok().stdout.contains("emit an event"));
}

#[test]
fn a_promote_spec_prescription_ships_as_a_rule_carrying_its_exact_item_token() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n\n## Prescriptions\n- [p1] (promote: spec) a lockout is always logged\n\n## Tickets\n",
    );
    repo.ks(["review", id]).ok();
    repo.ks(["approve", id]).ok();

    // The verb the old advice pointed at refuses by design, and says what to write.
    let r = repo.ks(["promote", &format!("{id}#p1"), "--as", "spec"]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains(&format!("{{{id}#p1}}")), "{}", r.stderr);

    // A bare proposal token ships the CHANGE (positional) but never the prescription —
    // a prescription is a specific promise, and only its own token can answer it.
    let spec = repo.read(".kanspec/specs/auth.md");
    repo.write(
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.lockout] 5 failed logins lock the account. {{{id}}}\n"),
    );
    let r = repo.ks(["close", id]);
    assert_eq!(r.code, 1, "{}", r.stdout);
    assert!(r.stderr.contains("[p1]"), "{}", r.stderr);
    assert!(
        !r.stderr.contains("promote") || r.stderr.contains(&format!("{{{id}#p1}}")),
        "the advice must not ring back to the refusing verb: {}",
        r.stderr
    );

    // The exact item token is the disposition.
    let spec = repo.read(".kanspec/specs/auth.md");
    repo.write(
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.lockout-logged] Every lockout is logged. {{{id}#p1}}\n"),
    );
    let done = repo.ks(["close", id]).ok();
    assert!(
        done.stdout.contains("p1 shipped→auth#auth.lockout-logged"),
        "{}",
        done.stdout
    );
    assert_eq!(repo.ks(["doctor"]).code, 0);
}

#[test]
fn a_promoted_decision_lands_proposed_so_an_agent_never_self_accepts() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n\n## Prescriptions\n- [p1] (promote: decision) lockout state lives in Redis\n\n## Tickets\n",
    );
    repo.ks(["review", id]).ok();
    repo.ks(["approve", id]).ok();
    let r = repo
        .ks([
            "promote",
            &format!("{id}#p1"),
            "--as",
            "decision",
            "--scope",
            "src/**",
        ])
        .ok();
    assert!(r.stdout.contains("proposed"), "{}", r.stdout);
    // Invariant 8: a proposed decision is NOT injected as a standing rule.
    let rules = repo.ks(["rules"]).ok().stdout;
    assert!(rules.contains("DECISIONS (0)"), "{rules}");
    // And it points home, so `why` can walk the chain.
    assert!(r.stdout.contains(&format!("{id}#p1")), "{}", r.stdout);
    // The `(promote: decision)` marker typed the prescription; it is not the title
    // (t-5f2b: D-0174 on the trial carried it in every listing).
    let did = r
        .stdout
        .split_whitespace()
        .find(|w| w.starts_with("D-"))
        .expect("the minted id")
        .to_string();
    let file = std::fs::read_dir(repo.root.join(".kanspec/decisions"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with(&did))
        .unwrap_or_else(|| panic!("a file for {did}"));
    let record = std::fs::read_to_string(&file).unwrap();
    let title = record
        .lines()
        .find_map(|l| l.strip_prefix("title:"))
        .expect("a title line")
        .trim();
    assert!(
        title.contains("lockout state lives in Redis") && !title.contains("promote:"),
        "{title}"
    );
}

#[test]
fn a_deleted_item_moves_its_thread_to_the_orphan_tray_rather_than_losing_it() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    let items = "## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n\n## Prescriptions\n\n## Tickets\n";
    body(&repo, &p, items);
    repo.ks(["comment", "add", &format!("{id}#c1"), "--body", "too broad"])
        .ok();

    // Edit the item: the stored quote is what makes this detectable.
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout after 3 failures\n\n## Prescriptions\n\n## Tickets\n",
    );
    let edited = repo.ks(["comments"]).ok().stdout;
    assert!(edited.contains("edited since"), "{edited}");

    // Delete it: the objection survives, visibly.
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n\n## Prescriptions\n\n## Tickets\n",
    );
    let orphaned = repo.ks(["comments"]).ok().stdout;
    assert!(orphaned.contains("ORPHANED"), "{orphaned}");
    assert!(orphaned.contains("too broad"), "{orphaned}");
}

/// The accuracy half of shipped-detection: a rule that names its ITEM is attributed to
/// that item, whatever order the corpus happens to be in. Order-pairing alone would get
/// this backwards, and the ledger would record a plausible lie.
#[test]
fn an_item_level_token_beats_the_positional_fallback() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n- [c2] emit an event\n\n## Prescriptions\n\n## Tickets\n",
    );
    repo.ks(["review", id]).ok();
    repo.ks(["approve", id]).ok();

    // Deliberately written in the REVERSE order to the Changes list: the event rule comes
    // first in the corpus, so positional pairing would call it c1.
    let spec = repo.read(".kanspec/specs/auth.md");
    repo.write(
        ".kanspec/specs/auth.md",
        &format!(
            "{spec}- [auth.evt] Lockout emits an event. {{{id}#c2}}\n             - [auth.lockout] 5 failed logins lock the account. {{{id}#c1}}\n"
        ),
    );

    let done = repo.ks(["close", id]).ok();
    assert!(
        done.stdout.contains("c1 shipped→auth#auth.lockout"),
        "c1 must be attributed to the rule that NAMES it:\n{}",
        done.stdout
    );
    assert!(
        done.stdout.contains("c2 shipped→auth#auth.evt"),
        "{}",
        done.stdout
    );
    // The token still counts as ordinary provenance, so nothing else has to know about it.
    assert!(!repo
        .ks(["rules", "--audit"])
        .ok()
        .stdout
        .contains("no provenance"));
}

/// …and the fallback still holds when nobody named an item: pair in order, and REFUSE for
/// any change the corpus has no rule for. Mis-attribution is survivable; inventing a
/// change that shipped is not.
#[test]
fn the_positional_fallback_refuses_rather_than_inventing_a_shipped_change() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n- [c2] emit an event\n\n## Prescriptions\n\n## Tickets\n",
    );
    repo.ks(["review", id]).ok();
    repo.ks(["approve", id]).ok();

    // ONE bare-token rule for TWO changes.
    let spec = repo.read(".kanspec/specs/auth.md");
    repo.write(
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.lockout] 5 failed logins lock the account. {{{id}}}\n"),
    );

    let r = repo.ks(["close", id]);
    assert_eq!(r.code, 1, "{}", r.stdout);
    assert!(r.stderr.contains("[c2]"), "{}", r.stderr);
    assert!(!r.stderr.contains("[c1]"), "c1 had evidence: {}", r.stderr);
}

/// The review page is assembled server-side in one snapshot read, so the browser can never
/// render a combination of proposal, threads and spec text that never existed on disk.
#[test]
fn the_review_page_carries_the_threads_the_badges_and_the_spec_as_it_stands() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\ncredential stuffing hit staging\n\n## Changes\n- [c1] lockout after 5 failures\n\n## Prescriptions\n- [p1] (promote: decision) state lives in Redis\n- [p2] (temp until t-31aa) keep the captcha\n\n## Tickets\n- [t1] Rate-limit login endpoint\n",
    );
    let spec = repo.read(".kanspec/specs/auth.md");
    repo.write(
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.jwt] Login issues a JWT valid 24h.\n"),
    );
    repo.ks(["comment", "add", &format!("{id}#c1"), "--body", "too broad"])
        .ok();

    let ctx = common::ctx_at(&repo.root);
    let model = kanspec::cmd::proposal::page(&ctx, id).expect("the page assembles");

    assert_eq!(model.why, "credential stuffing hit staging");
    // The prescription markers become BADGES, and the text does not repeat them.
    let p1 = model
        .items
        .iter()
        .find(|i| i.kind == 'p' && i.id.n == 1)
        .unwrap();
    assert_eq!(p1.badge.as_deref(), Some("PROMOTE → decision"));
    assert!(!p1.text.contains("promote:"), "{}", p1.text);
    let p2 = model
        .items
        .iter()
        .find(|i| i.kind == 'p' && i.id.n == 2)
        .unwrap();
    assert!(p2.badge.as_deref().unwrap().starts_with("TEMP"), "{p2:?}");

    // The thread is attached to the item it targets, not to a flat list the page must sort.
    let c1 = model.items.iter().find(|i| i.kind == 'c').unwrap();
    assert_eq!(c1.threads.len(), 1);
    assert_eq!(c1.threads[0].body, "too broad");
    assert_eq!(model.unresolved, 1);
    assert!(model.blocked_by.is_some(), "the gate is stated on the page");

    // …and the spec as it stands today rides along, so the delta is reviewed against
    // current truth without opening a file.
    assert_eq!(model.context.len(), 1);
    assert!(model.context[0]
        .rules
        .iter()
        .any(|r| r.anchor == "auth.jwt"));
}

// ─────────────────────────────────────────────────────────────────────────────
// The context budget under every [tN] (p-67f0 c1)
// ─────────────────────────────────────────────────────────────────────────────

/// Every `[tN]` carries what it will have to read — rules in scope and the code under its
/// spec's globs — derived at read time, on `review`, `approve`, the page and `--json`;
/// a `[cN]` that names a path in backticks narrows the surface to it; a spec with no
/// `code:` says the surface is unknown rather than guessing.
#[test]
fn every_ticket_item_shows_what_it_will_have_to_read() {
    let repo = TestRepo::new();
    repo.ks([
        "spec",
        "new",
        "auth",
        "--feature",
        "Login",
        "--code",
        "src/auth/**",
    ])
    .ok();
    repo.ks(["spec", "new", "docs", "--feature", "The handbook"])
        .ok();
    let spec = repo.read(".kanspec/specs/auth.md");
    repo.write(
        ".kanspec/specs/auth.md",
        &format!("{spec}- [auth.jwt] Login issues a JWT valid 24h. {{p-0001}}\n"),
    );
    repo.write("src/auth/lockout.rs", "a\nb\nc\n");
    repo.write("src/auth/session.rs", "a\nb\n");
    repo.commit("auth code");
    repo.ks([
        "propose",
        "Login rate limiting",
        "--spec",
        "auth",
        "--spec",
        "docs",
    ])
    .ok();
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] auth: lockout counter in `src/auth/lockout.rs`\n- [c2] auth: the session middleware\n- [c3] docs: the handbook page\n\n## Prescriptions\n\n## Tickets\n- [t1] (spec: auth) The lockout · S · implements: c1\n- [t2] (spec: auth) The middleware · M · implements: c2\n- [t3] (spec: docs) The handbook · S · implements: c3\n",
    );

    let out: serde_json::Value = repo.json(&["review", id]);
    let b = out["budgets"].as_array().expect("one budget per [tN]");
    assert_eq!(b.len(), 3);

    // t1: its change names a file, so the surface is that file alone — and the reading
    // list is still the whole capability's, because rules are per spec, not per file.
    assert_eq!(b[0]["item"], "t1");
    assert_eq!(b[0]["spec"], "auth");
    assert_eq!(b[0]["reads"]["rules"], 1);
    assert_eq!(b[0]["surface"]["files"], 1);
    assert_eq!(b[0]["surface"]["lines"], 3);
    assert_eq!(b[0]["narrowed_to"][0], "src/auth/lockout.rs");
    let line = b[0]["line"].as_str().unwrap();
    assert!(line.contains("reads 1 rule"), "{line}");
    assert!(
        line.contains("surface 1 file, 3 lines under src/auth/lockout.rs"),
        "{line}"
    );

    // t2: nothing named, so the whole spec surface — the template's own
    // `src/auth/login.ts` included, because the surface is what is TRACKED under the globs.
    let template_lines = repo.read("src/auth/login.ts").matches('\n').count();
    assert_eq!(b[1]["surface"]["files"], 3);
    assert_eq!(b[1]["surface"]["lines"], 5 + template_lines as u64);
    assert!(b[1]["narrowed_to"].as_array().unwrap().is_empty());

    // t3: a spec with no code globs has no surface to measure, and the line says so
    // instead of guessing.
    assert_eq!(b[2]["spec"], "docs");
    assert!(b[2].get("surface").is_none(), "{}", b[2]);
    assert!(
        b[2]["line"].as_str().unwrap().contains("surface unknown"),
        "{}",
        b[2]
    );

    // The human rendering carries the same lines, one per item.
    let human = repo.ks(["review", id]).ok().stdout;
    assert!(
        human.contains("[t1]") && human.contains("reads 1 rule"),
        "{human}"
    );
    assert!(
        human.contains("[t3]") && human.contains("surface unknown"),
        "{human}"
    );

    // The page carries it on the item, and nowhere else.
    let ctx = common::ctx_at(&repo.root);
    let model = kanspec::cmd::proposal::page(&ctx, id).expect("the page assembles");
    let t1 = model
        .items
        .iter()
        .find(|i| i.kind == 't' && i.id.n == 1)
        .unwrap();
    assert_eq!(t1.budget.as_ref().unwrap().line, line);
    assert!(model
        .items
        .iter()
        .filter(|i| i.kind == 'c')
        .all(|i| i.budget.is_none()));

    // `approve` reports what the human approved over, then mints as before.
    let out: serde_json::Value = repo.json(&["approve", id]);
    assert_eq!(out["budgets"].as_array().unwrap().len(), 3);
    assert_eq!(out["budgets"][0]["line"], line);
    assert_eq!(out["minted"].as_array().unwrap().len(), 3);

    // Nothing was written into the proposal: the budget is derived, never stored.
    let src = repo.read(&format!(".kanspec/proposals/{p}/proposal.md"));
    assert!(!src.contains("reads 1 rule"), "{src}");
}

/// p-67f0 c4: sub-bullets under a `[tN]` mint as that ticket's `## Steps`, the estimate
/// segment is read onto the budget line, and a bare `[tN]` still mints an empty list.
#[test]
fn sub_bullets_under_a_ticket_item_mint_as_its_steps() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout\n\n## Prescriptions\n\n## Tickets\n- [t1] Rate-limit login endpoint · L · implements: c1\n  - lockout counter in Redis\n  - 429 + Retry-After on lock\n  - test: two concurrent requests\n- [t2] A bare item\n",
    );
    let out: serde_json::Value = repo.json(&["review", id]);
    assert_eq!(out["budgets"][0]["steps"], 3);
    assert_eq!(out["budgets"][0]["size"], "L");
    assert!(
        out["budgets"][0]["line"]
            .as_str()
            .unwrap()
            .ends_with("· 3 steps"),
        "{}",
        out["budgets"][0]
    );
    assert_eq!(out["budgets"][1]["steps"], 0);
    assert!(out["budgets"][1].get("size").is_none());

    let ctx = common::ctx_at(&repo.root);
    let model = kanspec::cmd::proposal::page(&ctx, id).expect("the page assembles");
    let t1 = model
        .items
        .iter()
        .find(|i| i.kind == 't' && i.id.n == 1)
        .unwrap();
    assert_eq!(t1.steps.len(), 3, "the page shows the steps-to-be");

    let out: serde_json::Value = repo.json(&["approve", id]);
    let minted = out["minted"].as_array().unwrap();
    assert_eq!(minted.len(), 2);
    let t1 = repo.read(&format!(
        ".kanspec/tickets/{}.md",
        minted[0].as_str().unwrap()
    ));
    let steps: Vec<&str> = t1.lines().filter(|l| l.starts_with("- [ ] ")).collect();
    assert_eq!(
        steps,
        [
            "- [ ] lockout counter in Redis",
            "- [ ] 429 + Retry-After on lock",
            "- [ ] test: two concurrent requests",
        ]
    );
    assert!(
        !t1.contains("· L ·") && !t1.contains("size"),
        "the estimate is never stored:\n{t1}"
    );
    let t2 = repo.read(&format!(
        ".kanspec/tickets/{}.md",
        minted[1].as_str().unwrap()
    ));
    assert!(
        !t2.contains("- [ ]"),
        "a bare item mints an empty list, exactly as before"
    );

    // `new --step` is the same scaffold by hand, in order.
    let out: serde_json::Value =
        repo.json(&["new", "By hand", "--step", "first", "--step", "second"]);
    let t = repo.read(&format!(
        ".kanspec/tickets/{}.md",
        out["id"].as_str().unwrap()
    ));
    let steps: Vec<&str> = t.lines().filter(|l| l.starts_with("- [ ] ")).collect();
    assert_eq!(steps, ["- [ ] first", "- [ ] second"]);
}

/// `abandon` makes no claim that anything was dispositioned, so it must never stamp a
/// ledger or move the directory — only `close` earns those.
#[test]
fn abandon_records_a_why_without_claiming_a_disposition() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    repo.ks(["abandon", id, "--why", "superseded"]).ok();
    let src = repo.read(&format!(".kanspec/proposals/{p}/proposal.md"));
    assert!(src.contains("status: abandoned"), "{src}");
    assert!(src.contains("superseded"), "{src}");
    assert!(
        src.contains("ledger: []"),
        "an abandon dispositions nothing: {src}"
    );
    assert!(!repo.exists(&format!(".kanspec/proposals/closed/{p}/proposal.md")));
    assert_eq!(repo.ks(["abandon", id, "--why", "again"]).code, 1);
}

// ── the audit surfaces the migration needed ─────────────────────────────────

/// `spec grep <p>` answers "which specs mention X" — a question you could answer by
/// reading. `--missing` answers "which specs DO NOT", which you cannot: a spec is missing
/// a rule silently, and nothing about the file looks wrong. That inversion is what turns
/// grep into an audit.
#[test]
fn spec_grep_missing_names_the_specs_with_no_matching_rule() {
    let repo = TestRepo::new();
    for (name, rule) in [
        (
            "auth",
            "- [auth.tenant] Another household's session is 404.",
        ),
        ("appearance", "- [appearance.accent] The accent is cobalt."),
    ] {
        repo.ks(["spec", "new", name, "--feature", "F", "--code", "src/**"])
            .ok();
        let p = format!(".kanspec/specs/{name}.md");
        let s = repo.read(&p);
        repo.write(&p, &format!("{s}{rule}\n"));
    }

    let hit = repo.ks(["spec", "grep", "tenant"]).ok().stdout;
    assert!(hit.contains("auth"), "{hit}");
    assert!(!hit.contains("appearance"), "{hit}");

    // The inverse names the OTHER one — and never the covered one.
    let missing = repo.ks(["spec", "grep", "tenant", "--missing"]).ok().stdout;
    assert!(missing.contains("appearance"), "{missing}");
    assert!(
        !missing.contains("auth"),
        "a covered spec must not be listed: {missing}"
    );
    // A `--missing` row is about the SPEC, so it carries no rule anchor to render.
    assert!(
        !missing.contains("[]"),
        "empty anchor leaked into the render: {missing}"
    );
}

/// The reverse of the dead-glob check. `doctor` asks "does this glob match a file"; the
/// question that actually loses you steering is "does this file match a spec" — new code
/// is uncovered by default, `prime` injects nothing for it, and nothing says so.
#[test]
fn features_uncovered_names_tracked_files_no_spec_claims() {
    let repo = TestRepo::new();
    repo.write("src/covered.rs", "// covered\n");
    repo.write("src/orphan.rs", "// nobody claims me\n");
    repo.git(&["add", "-A"]);
    repo.commit("add sources");
    repo.ks([
        "spec",
        "new",
        "auth",
        "--feature",
        "F",
        "--code",
        "src/covered.rs",
    ])
    .ok();

    let r = repo.ks(["features", "--uncovered", "src/**"]).ok();
    assert!(r.stdout.contains("src/orphan.rs"), "{}", r.stdout);
    assert!(
        !r.stdout.contains("src/covered.rs"),
        "a claimed file must not be listed: {}",
        r.stdout
    );

    // Widen the spec to claim it, and the report goes clean rather than staying stale.
    let p = ".kanspec/specs/auth.md";
    let s = repo.read(p);
    repo.write(p, &s.replace("code: [src/covered.rs]", "code: [src/**]"));
    let after = repo.ks(["features", "--uncovered", "src/**"]).ok();
    assert!(
        after.stdout.contains("every tracked file there is claimed"),
        "{}",
        after.stdout
    );
}

/// The ordinary `features` table must not be hijacked by the uncovered renderer — an empty
/// result means "all claimed" only when `--uncovered` was the question.
#[test]
fn features_without_uncovered_still_renders_its_table() {
    let repo = TestRepo::new();
    repo.ks([
        "spec",
        "new",
        "auth",
        "--feature",
        "Login",
        "--code",
        "src/**",
    ])
    .ok();
    let out = repo.ks(["features"]).ok().stdout;
    assert!(out.contains("Login"), "{out}");
    assert!(!out.contains("claimed by a spec"), "{out}");
}

/// t-660d: a proposal in review is owed a human decision from the moment `review` runs,
/// not from the seven-day dwell — `approve`, or a comment.
#[test]
fn a_proposal_in_review_is_listed_under_you_until_a_human_approves_or_comments() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n\n## Prescriptions\n\n## Tickets\n",
    );
    let you = |repo: &TestRepo| -> Vec<serde_json::Value> {
        let s: serde_json::Value = repo.json(&["status"]);
        s["you"].as_array().unwrap().clone()
    };
    assert!(
        !you(&repo).iter().any(|a| a["subject"] == id),
        "a draft is nobody's to approve yet"
    );

    repo.ks(["review", id]).ok();
    let line = you(&repo)
        .into_iter()
        .find(|a| a["subject"] == id)
        .unwrap_or_else(|| panic!("the proposal in review is owed a decision"));
    assert_eq!(line["fix"], format!("kanspec approve {id}"), "{line}");
    assert!(
        line["line"].as_str().unwrap().contains("in review"),
        "{line}"
    );
    let human = repo.ks(["status"]).ok().stdout;
    assert!(human.starts_with(" YOU (1)"), "{human}");

    // A thread is the other way to discharge it: the threads line takes over.
    repo.ks(["comment", "add", &format!("{id}#c1"), "--body", "too broad"])
        .ok();
    let lines = you(&repo);
    let mine: Vec<&serde_json::Value> = lines.iter().filter(|a| a["subject"] == id).collect();
    assert_eq!(mine.len(), 1, "one line per proposal: {lines:?}");
    assert!(
        mine[0]["line"]
            .as_str()
            .unwrap()
            .contains("unresolved review threads"),
        "{lines:?}"
    );

    // Approved: nothing owed on it any more.
    let cm: serde_json::Value = repo.json(&["comments"]);
    let cid = cm["threads"][0]["id"].as_str().unwrap().to_string();
    repo.ks(["comment", "resolve", &cid, "--note", "narrowed"])
        .ok();
    repo.ks(["approve", id]).ok();
    assert!(
        !you(&repo).iter().any(|a| a["subject"] == id),
        "{:?}",
        you(&repo)
    );
}

/// t-85de: three tickets minted from one proposal all got its first spec, and the content
/// ticket belonged to another. A `[tN]` names its spec, or inherits it from the `[cN]` it
/// implements, or falls back to the proposal's first.
#[test]
fn a_minted_ticket_takes_its_own_spec_or_the_spec_of_the_change_it_implements() {
    let repo = TestRepo::new();
    for (name, code) in [("auth", "src/auth/**"), ("playbooks", "docs/**")] {
        repo.ks(["spec", "new", name, "--feature", "F", "--code", code])
            .ok();
    }
    repo.ks([
        "propose",
        "Onboarding interview",
        "--spec",
        "auth",
        "--spec",
        "playbooks",
    ])
    .ok();
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] auth: ask the three questions\n- [c2] playbooks: write the runbook\n\n## Prescriptions\n\n## Tickets\n- [t1] Interview flow · S\n- [t2] (spec: playbooks) Runbook content · S\n- [t3] Runbook wiring · S · implements: c2\n- [t4] Alerts · c1, c2\n",
    );
    repo.ks(["review", id]).ok();
    let r: serde_json::Value = repo.json(&["approve", id]);
    let minted: Vec<String> = r["minted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(minted.len(), 4, "{r}");
    let spec_of = |t: &str| -> String {
        let v: serde_json::Value = repo.json(&["show", t]);
        v["spec"].as_str().unwrap_or("").to_string()
    };
    assert_eq!(
        spec_of(&minted[0]),
        "auth",
        "the proposal's first spec, as before"
    );
    assert_eq!(spec_of(&minted[1]), "playbooks", "named on the bullet");
    assert_eq!(spec_of(&minted[2]), "playbooks", "inherited from [c2]");
    assert_eq!(
        spec_of(&minted[3]),
        "auth",
        "inherited from the first change named"
    );
    let t2 = repo.read(&format!(".kanspec/tickets/{}.md", minted[1]));
    assert!(
        t2.contains("title: Runbook content") && !t2.contains("(spec:"),
        "the marker is not the title: {t2}"
    );
}

/// t-8e31: labels come from git identity, so an agent relaying a human's comment and the
/// human typing it looked identical. The row carries the kind beside the label.
#[test]
fn a_row_an_agent_wrote_says_so_beside_its_label_and_a_humans_does_not() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nwhy\n\n## Changes\n- [c1] lockout after 5 failures\n\n## Prescriptions\n\n## Tickets\n",
    );
    // The human seeds the thread.
    repo.ks(["comment", "add", &format!("{id}#c1"), "--body", "too broad"])
        .ok();
    let cm: serde_json::Value = repo.json(&["comments"]);
    let cid = cm["threads"][0]["id"].as_str().unwrap().to_string();
    // An agent with a session id replies; then one that Claude Code runs with no session
    // id at all — the case that used to fall through to the git identity and look human.
    repo.ks_env(
        ["comment", "reply", &cid, "--body", "narrowed to /login"],
        &[
            ("KANSPEC_ACTOR", "claude/sess-a91"),
            ("KANSPEC_ACTOR_KIND", "agent"),
        ],
    )
    .ok();
    // A later clock: rows dedupe on (id, op, at), so two replies in the same frozen
    // minute would read back as one.
    repo.ks_env(
        ["comment", "reply", &cid, "--body", "and documented"],
        &[
            ("KANSPEC_ACTOR", ""),
            ("KANSPEC_ACTOR_KIND", ""),
            ("KANSPEC_NOW", "2026-08-31T12:01:00Z"),
            ("CLAUDECODE", "1"),
        ],
    )
    .ok();

    let cm: serde_json::Value = repo.json(&["comments"]);
    let t = &cm["threads"][0];
    assert!(t["via"].is_null(), "the human's seed carries no kind: {t}");
    assert_eq!(t["replies"][0]["via"], "agent", "{t}");
    assert_eq!(t["replies"][0]["by"], "claude/sess-a91", "{t}");
    assert_eq!(
        t["replies"][1]["via"], "agent",
        "an agent with no session id is still an agent: {t}"
    );
    assert_eq!(t["replies"][1]["by"], "claude/session", "{t}");

    let human = repo.ks(["comments"]).ok().stdout;
    assert!(
        human.contains("claude/sess-a91 via agent: narrowed"),
        "{human}"
    );
    assert!(!human.contains("trevor via"), "{human}");

    // The kind is in the row itself, where a merge or a hand reader sees it.
    let jsonl = repo.read(&format!(".kanspec/proposals/{p}/comments.jsonl"));
    assert_eq!(jsonl.matches("\"via\":\"agent\"").count(), 2, "{jsonl}");
}

/// t-f3a4: the page is loopback-only, and a reviewer on a phone had to have the proposal
/// mirrored by hand. `review --export` writes the page as one static file: the proposal,
/// every thread read-only, no script and no form.
#[test]
fn review_export_writes_the_page_as_one_static_comment_less_file() {
    let repo = TestRepo::new();
    seed(&repo);
    let p = only_proposal(&repo);
    let id = &p[..6];
    body(
        &repo,
        &p,
        "## Why\nCredential stuffing hit staging.\n\n## Changes\n- [c1] auth: lockout after 5 failures\n\n## Testing\n- run the login suite\n\n## Prescriptions\n- [p1] (promote: decision) lockout state lives in Redis <only>\n\n## Tickets\n- [t1] Rate-limit login endpoint · S\n",
    );
    repo.ks([
        "comment",
        "add",
        &format!("{id}#c1"),
        "--body",
        "too broad & vague",
    ])
    .ok();

    let r: serde_json::Value = repo.json(&["review", id, "--export", "page.html"]);
    assert_eq!(r["status"], "review");
    let exported = r["exported"].as_str().expect("where it wrote");
    assert!(exported.ends_with("page.html"), "{exported}");
    let html = std::fs::read_to_string(exported).expect("the file");

    for needle in [
        "Credential stuffing hit staging.",
        "lockout after 5 failures",
        "[c1]",
        "PROMOTE → decision",
        "lockout state lives in Redis &lt;only&gt;",
        "Rate-limit login endpoint",
        "## Testing".trim_start_matches("## "),
        "run the login suite",
        "too broad &amp; vague",
        "1 open",
        "<style>",
    ] {
        assert!(html.contains(needle), "the page lost {needle:?}:\n{html}");
    }
    assert!(
        !html.contains("<script") && !html.contains("<form") && !html.contains("<button"),
        "static and comment-less: no script, no form, no button\n{html}"
    );
    assert!(
        !html.contains("(promote:"),
        "the marker is the badge, not the text"
    );

    // Re-running exports the page as it is now: the thread resolved shows resolved.
    let cm: serde_json::Value = repo.json(&["comments"]);
    let cid = cm["threads"][0]["id"].as_str().unwrap().to_string();
    repo.ks(["comment", "resolve", &cid, "--note", "narrowed to /login"])
        .ok();
    repo.ks(["review", id, "--export", "page.html"]).ok();
    let html = repo.read("page.html");
    assert!(
        html.contains("✓ narrowed to /login") && html.contains("nothing open"),
        "{html}"
    );
    let human = repo.ks(["review", id, "--export", "page.html"]).ok().stdout;
    assert!(
        human.contains("exported") && human.contains("page.html"),
        "{human}"
    );
}
