use std::process::Command;

/// `git <args>` run at the REPO ROOT (cargo runs a build script in the package dir,
/// `crates/lisp`, where a `-- crates std` pathspec matches nothing and `--git-path` answers
/// relative to the wrong place), its trimmed stdout — `None` when git is absent, this is
/// not a checkout, the command fails, or the output is empty.
fn git(args: &[&str]) -> Option<String> {
    let root = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("../..");
    Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    // `<short-sha>`, or `<short-sha>-dirty` when a file the BINARY is built from (`crates/`,
    // `std/`) differs from the commit. Two binaries built from one commit — one clean, one
    // carrying an uncommitted checker or std/ change — used to report the same version and
    // were indistinguishable by `--version`, `system/build-id`, a crash dump or a test
    // footer; a `nest check` run against each disagreed and nothing said why (2026-09-13).
    // Scoped to the build's inputs on purpose: a docs-only edit does not change the binary.
    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = git(&[
        "status",
        "--porcelain",
        "--untracked-files=no",
        "--",
        "crates",
        "std",
    ])
    .is_some();
    let sha = if dirty { format!("{sha}-dirty") } else { sha };
    println!("cargo:rustc-env=BROOD_GIT_SHA={sha}");

    // A CONTENT hash of the embedded standard library — every `std/**/*.blsp` plus the
    // prelude. `system/build-id` cannot serve here: it embeds the executable's own mtime, so
    // `brood`, `nest` and `brood-lsp` from one tree get three different ids and each would
    // write its own ~2 MB stdlib startup image. This id depends only on what is baked in,
    // so they share one. Computed here rather than as a `const fn` because const-eval hits
    // its step limit hashing ~1 MB, and recomputing it at runtime costs 2.2M instructions
    // (`mix_chunk` below — it was 13.6M, 14.5% of a `brood --check`, a byte at a time).
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
            mix_chunk(&mut hash, chunk);
        }
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rustc-env=BROOD_STDLIB_HASH={hash:x}");
    // Re-run when the git head moves, or when a source of THIS crate changes (so the dirty
    // marker above is re-evaluated — the crate is recompiling in that case anyway, and
    // the build script is two git commands). The git paths are asked of git itself:
    // in a worktree `.git` is a FILE pointing elsewhere, so `<root>/.git/HEAD` does not
    // exist there and the head never re-triggered. Every path must be ABSOLUTE and
    // EXISTING: cargo re-runs a build script on EVERY build when a rerun-if-changed path
    // is missing, which silently recompiled `brood` (and its dependents) on every
    // invocation of every profile (found 2026-07-23 chasing "4 execs/minute").
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let root = std::path::Path::new(&manifest).join("../..");
    let git_path = |what: &str| {
        git(&["rev-parse", "--git-path", what]).map(|p| {
            let p = std::path::PathBuf::from(p);
            if p.is_absolute() {
                p
            } else {
                root.join(p)
            }
        })
    };
    let mut watched = vec![std::path::Path::new(&manifest).join("src")];
    watched.extend(git_path("HEAD"));
    watched.extend(git_path("refs/heads"));
    for p in watched {
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

/// FNV-1a over 8-byte WORDS, the chunk's length mixed in first, a byte tail for the
/// remainder. **Mirrored verbatim in `crates/lisp/src/cli_support.rs`**, which recomputes
/// this hash from the tree to tell a developer their binary is older than their edits; a
/// build script cannot share code with the crate it builds, so the algorithm lives twice and
/// `the_runtime_and_build_time_stdlib_hashes_agree` is the gate that keeps the copies equal.
///
/// A byte at a time cost 13.6M instructions over 3 MB of `std/` — 14.5% of a release
/// `brood --check` (callgrind, 2026-09-19, KI-150). The comment above used to estimate "~1 ms
/// of a ~23 ms boot" for it, unmeasured.
fn mix_chunk(hash: &mut u64, chunk: &[u8]) {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    *hash ^= chunk.len() as u64;
    *hash = hash.wrapping_mul(PRIME);
    let (words, tail) = chunk.as_chunks::<8>();
    for w in words {
        *hash ^= u64::from_le_bytes(*w);
        *hash = hash.wrapping_mul(PRIME);
    }
    for b in tail {
        *hash ^= *b as u64;
        *hash = hash.wrapping_mul(PRIME);
    }
}
