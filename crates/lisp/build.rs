use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BROOD_GIT_SHA={sha}");
    // Re-run only when the git head actually moves. These paths must be
    // ABSOLUTE and EXISTING: they resolve relative to the package dir
    // (crates/lisp), where `.git` does not exist — and cargo re-runs a build
    // script on EVERY build when a rerun-if-changed path is missing. That
    // silently recompiled `brood` (and its dependents) on every invocation of
    // every profile — invisible-ish in incremental dev builds, ~a minute per
    // `cargo fuzz` invocation (found 2026-07-23 chasing "4 execs/minute").
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let root = std::path::Path::new(&manifest).join("../..");
    for p in [root.join(".git/HEAD"), root.join(".git/refs/heads")] {
        if p.exists() {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
