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

    // A CONTENT hash of the embedded standard library — every `std/**/*.blsp` plus the
    // prelude. `system/build-id` cannot serve here: it embeds the executable's own mtime, so
    // `brood`, `nest` and `brood-lsp` from one tree get three different ids and each would
    // write its own ~2 MB stdlib startup image. This id depends only on what is baked in,
    // so they share one. Computed here rather than as a `const fn` because const-eval hits
    // its step limit hashing ~1 MB, and at runtime it would cost ~1 ms of a ~23 ms boot.
    let root = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect_blsp(&root.join("std"), &mut files);
    files.sort();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &files {
        for chunk in [
            f.strip_prefix(&root)
                .unwrap_or(f)
                .to_string_lossy()
                .as_bytes(),
            &std::fs::read(f).unwrap_or_default(),
        ] {
            for b in chunk {
                hash ^= *b as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rustc-env=BROOD_STDLIB_HASH={hash:x}");
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

/// Every `.blsp` under `dir`, recursively — the set the stdlib content hash covers.
fn collect_blsp(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                collect_blsp(&p, out);
            } else if p.extension().is_some_and(|x| x == "blsp") {
                out.push(p);
            }
        }
    }
}
