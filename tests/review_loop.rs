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
    for section in ["## Why", "## Changes", "## Prescriptions", "## Tickets"] {
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
}

/// The directory name carries human-readable text, which is the whole reason
/// `Op::CreateProposal` exists instead of a plain `CreateEntity`: the id alone would give
/// `proposals/p-7de2/`, which is fine for the machine and useless for the teammate
/// browsing the repo on GitHub.
#[test]
fn a_proposal_directory_is_slugged_and_still_resolves_by_bare_id() {
    let repo = TestRepo::new();
    repo.ks(["spec", "new", "auth", "--feature", "Login", "--code", "src/**"])
        .ok();
    repo.ks(["propose", "Move browser auth to HttpOnly cookies", "--spec", "auth"])
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
    assert!(repo.ks(["ls"]).ok().stdout.contains("Rate-limit login endpoint"));
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
    assert_eq!(repo.ks(["doctor"]).code, 0, "{}", repo.ks(["doctor"]).stdout);
    // Leftover scope became a VISIBLE board ticket, which is the whole point of followup.
    assert!(repo.ks(["ls"]).ok().stdout.contains("emit an event"));
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
    assert!(!repo.ks(["rules", "--audit"]).ok().stdout.contains("no provenance"));
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
    let p1 = model.items.iter().find(|i| i.kind == 'p' && i.id.n == 1).unwrap();
    assert_eq!(p1.badge.as_deref(), Some("PROMOTE → decision"));
    assert!(!p1.text.contains("promote:"), "{}", p1.text);
    let p2 = model.items.iter().find(|i| i.kind == 'p' && i.id.n == 2).unwrap();
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
    assert!(model.context[0].rules.iter().any(|r| r.anchor == "auth.jwt"));
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
    assert!(src.contains("ledger: []"), "an abandon dispositions nothing: {src}");
    assert!(!repo.exists(&format!(".kanspec/proposals/closed/{p}/proposal.md")));
    assert_eq!(repo.ks(["abandon", id, "--why", "again"]).code, 1);
}
