// build.rs — MANDATORY. rust-embed's include_bytes! tracks existing FILES but not the
// DIRECTORY, so a newly added asset is silently absent from a release binary (verified:
// build finished in 0.08s and the marker string was not in the binary).
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=docs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let package = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let version = repository_revision(&manifest)
        .map(|revision| format!("{package}+g{revision}"))
        .unwrap_or(package);
    println!("cargo:rustc-env=KANSPEC_BUILD_VERSION={version}");
}

/// Stamp path installs with the source revision, but never borrow a hash from a parent
/// repository when Cargo is building an unpacked crate without its own `.git` directory.
fn repository_revision(manifest: &str) -> Option<String> {
    let output = |args: &[&str]| {
        let out = Command::new("git")
            .args(["-C", manifest])
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let top = output(&["rev-parse", "--show-toplevel"])?;
    if Path::new(&top).canonicalize().ok()? != Path::new(manifest).canonicalize().ok()? {
        return None;
    }
    output(&["rev-parse", "--short=12", "HEAD"]).filter(|s| !s.is_empty())
}
