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
