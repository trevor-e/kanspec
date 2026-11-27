//! `tests/glob_rot.rs`
//!
//! Glob rot on the REAL binary, for all three path-scoped records. `doctor` checks globs
//! against tracked files directly, even when a fresh CI checkout has no disposable scan
//! cache. A record that lost SOME globs is a Warning, one that lost them ALL is an Error,
//! because at that point it steers nothing and CI is the only thing left that will notice.

mod common;

use common::TestRepo;

#[test]
fn a_record_whose_every_glob_rotted_fails_doctor_and_a_partial_one_warns() {
    let repo = TestRepo::new();
    // A spec moored to nothing, a quirk warning nobody, a decision half adrift.
    repo.ks([
        "spec",
        "new",
        "ghost",
        "--feature",
        "Nothing here",
        "--code",
        "src/gone/**",
    ])
    .ok();
    repo.ks([
        "quirk",
        "add",
        "Retries double-charge",
        "--paths",
        "src/vanished/**",
        "--sev",
        "landmine",
    ])
    .ok();
    let d: serde_json::Value = repo.json(&[
        "decide",
        "Charges are idempotent",
        "--scope",
        "src/billing/**",
        "--scope",
        "src/moved/**",
    ]);
    let id = d["id"].as_str().unwrap().to_string();
    repo.ks(["accept", &id]).ok();
    let r = repo.ks(["doctor"]);
    let out = format!("{}{}", r.stdout, r.stderr);
    assert_ne!(r.code, 0, "two records steer nothing; CI must fail:\n{out}");
    assert!(
        out.contains("spec ghost") && out.contains("steers nothing"),
        "{out}"
    );
    assert!(
        out.contains("quirk q-") && out.contains("warns nobody"),
        "{out}"
    );
    // Half adrift: still steering `src/billing/**`, so a warning that names the rot.
    assert!(out.contains(&format!("decision {id}")), "{out}");
    assert!(out.contains("src/moved/**"), "{out}");
    assert!(
        !out.contains("EVERY scope glob"),
        "one live glob remains, so this is not the Error grade:\n{out}"
    );

    // The one-definition property: fix the tracked file and the next doctor sees it live,
    // without relying on a preceding scan or cache write.
    repo.write("src/vanished/retry.ts", "export const retry = 1;\n");
    repo.git(&["add", "-A"]);
    repo.commit("the quirk's path exists again");
    let out = {
        let r = repo.ks(["doctor"]);
        format!("{}{}", r.stdout, r.stderr)
    };
    assert!(!out.contains("warns nobody"), "{out}");
    assert!(
        out.contains("spec ghost"),
        "the spec is still adrift:\n{out}"
    );
}

#[test]
fn a_moved_file_can_be_repointed_without_losing_the_quirk() {
    let repo = TestRepo::new();
    let title = "Retry charges carefully";
    let added: serde_json::Value = repo.json(&[
        "quirk",
        "add",
        title,
        "--paths",
        "src/billing/**",
        "--sev",
        "landmine",
    ]);
    let id = added["id"].as_str().unwrap();
    repo.git(&["mv", "src/billing/charge.ts", "src/auth/payment.ts"]);
    let doctor = repo.ks(["doctor"]);
    assert_ne!(doctor.code, 0);
    assert!(doctor.stdout.contains(&format!("quirk paths {id} --paths")));

    let file = format!(".kanspec/quirks/{id}.md");
    let original = repo.read(&file);
    let invalid = repo.ks(["quirk", "paths", id, "--paths", "README.md", "--paths", "["]);
    assert_ne!(invalid.code, 0);
    assert_eq!(repo.read(&file), original);

    let updated: serde_json::Value = repo.json(&[
        "quirk",
        "paths",
        id,
        "--paths",
        "src/auth/payment.ts",
        "--paths",
        "README.md",
    ]);
    assert_eq!(
        updated["paths"],
        serde_json::json!(["src/auth/payment.ts", "README.md"])
    );
    repo.ks(["doctor"]).ok();
    for file in ["src/auth/payment.ts", "README.md"] {
        let listed: serde_json::Value = repo.json(&["quirks", "--touch", file]);
        let quirk = &listed["rows"][0];
        assert_eq!(quirk["id"], id);
        assert_eq!(quirk["severity"], "landmine");
    }
    assert!(repo
        .ks(["quirks", "--touch", "src/billing/charge.ts"])
        .ok()
        .stdout
        .is_empty());
}
