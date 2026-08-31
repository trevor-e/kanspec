// cargo-dist ships every `[[bin]]`; a symlink does not survive the release archive.
fn main() -> std::process::ExitCode {
    kanspec::run("ks")
}
