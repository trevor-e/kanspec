// build.rs — MANDATORY. rust-embed's include_bytes! tracks existing FILES but not the
// DIRECTORY, so a newly added asset is silently absent from a release binary (verified:
// build finished in 0.08s and the marker string was not in the binary).
fn main() {
    println!("cargo:rerun-if-changed=assets");
    println!("cargo:rerun-if-changed=docs");
}
