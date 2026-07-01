//! `nest release` mechanism — the runtime-resolution + target-triple plumbing
//! behind the `cmd_release` orchestration in `main.rs` (ADR-038). Collection of
//! the project's sources is *policy* (Brood: `project/bundle-collect`) and byte
//! assembly is in `brood::bundle`; this module is the Rust glue that picks which
//! base runtime to append to and names per-target artifacts. Split out of
//! `main.rs` to keep the thin `nest` shell thin (ADR-028).

/// The lean+gui `brood` runtime baked into this `nest` at install time, so
/// `nest release` can append an app to it with **no Rust toolchain** (ADR-038).
/// `build.rs` writes this file: the prebuilt runtime when `make install` sets
/// `BROOD_EMBED_RUNTIME`, otherwise empty — and an empty slice means "nothing
/// embedded", so `nest release` falls back to building the runtime from source.
const EMBEDDED_RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded-runtime"));

/// The triple this `nest` was built for (baked in by `build.rs`) — a `--target`
/// equal to it is served by the embedded runtime, no cache entry needed.
const HOST_TRIPLE: &str = env!("NEST_HOST_TRIPLE");

/// The base runtime bytes to append the app to — the single **lean + gui**
/// runtime (no test/observer/MCP/doc/hot-reload/REPL/GC-debug surface, but the
/// windowed backend kept so any app runs). Priority:
///   1. `--runtime PATH` — read it (a prebuilt/cross runtime you supply).
///   2. with `--target`, the local runtime cache
///      (`~/.cache/brood/runtimes/<triple>/brood`) — you populate it with lean
///      runtimes built on/for each target (cross-compiling is out of scope,
///      ADR-038); a `--target` equal to the host's own triple falls through to:
///   3. the runtime embedded in *this* `nest` at install time — the **no-Rust**
///      path (`make install` bakes it in; see `build.rs`).
///   4. built on demand from the workspace source (cargo fallback — needs Rust),
///      for a plain `cargo build` of `nest` that embedded nothing.
pub(crate) fn resolve_runtime(runtime: Option<&str>, target: Option<&str>) -> Vec<u8> {
    if let Some(r) = runtime {
        return std::fs::read(r).unwrap_or_else(|e| {
            eprintln!("nest release: cannot read runtime binary {r}: {e}");
            std::process::exit(1);
        });
    }
    if let Some(t) = target {
        if let Some(path) = runtime_cache_path(t) {
            if path.is_file() {
                return std::fs::read(&path).unwrap_or_else(|e| {
                    eprintln!(
                        "nest release: cannot read cached runtime {}: {e}",
                        path.display()
                    );
                    std::process::exit(1);
                });
            }
            // The host's own triple needs no prebuilt — the embedded/built
            // runtime below *is* a runtime for it.
            if t != HOST_TRIPLE {
                eprintln!(
                    "nest release: no cached runtime for {t}.\nBuild the lean runtime on/for \
                     that target:\n  cargo build --profile release-lean -p cli \
                     --no-default-features --features brood/gui --target {t}\nthen place the \
                     `brood` binary at {} (or pass --runtime PATH). Cross-compiling is out of \
                     scope for `nest release` itself (ADR-038).",
                    path.display()
                );
                std::process::exit(2);
            }
        } else if t != HOST_TRIPLE {
            eprintln!(
                "nest release: cannot locate the runtime cache (no $XDG_CACHE_HOME or $HOME) — \
                 pass --runtime PATH for {t}"
            );
            std::process::exit(2);
        }
    }
    if !EMBEDDED_RUNTIME.is_empty() {
        // Baked in by `make install` — append with no toolchain at all.
        return EMBEDDED_RUNTIME.to_vec();
    }
    // No embedded runtime (a plain `cargo build` of `nest`): build one from source.
    let path = build_lean_runtime();
    std::fs::read(&path).unwrap_or_else(|e| {
        eprintln!(
            "nest release: cannot read built runtime {}: {e}",
            path.display()
        );
        std::process::exit(1);
    })
}

/// Where a prebuilt lean runtime for `triple` lives in the local cache:
/// `$XDG_CACHE_HOME/brood/runtimes/<triple>/brood` (falling back to
/// `~/.cache`; `brood.exe` for Windows triples). `None` if no cache base can
/// be determined. Mirrors `prelude_source_path` in `crates/lisp/src/lib.rs`.
pub(crate) fn runtime_cache_path(triple: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let bin = if is_windows_triple(triple) {
        "brood.exe"
    } else {
        "brood"
    };
    Some(base.join("brood/runtimes").join(triple).join(bin))
}

/// Short, human-friendly artifact suffix for a target triple — `macos-arm64`,
/// `linux-x86_64`, `linux-musl-x86_64`, `windows-x86_64`. An unrecognized OS
/// keeps the whole triple (always unambiguous, just longer).
pub(crate) fn target_suffix(triple: &str) -> String {
    let arch = match triple.split('-').next().unwrap_or(triple) {
        "aarch64" => "arm64",
        a => a,
    };
    let os = if triple.contains("apple-darwin") {
        "macos"
    } else if triple.contains("windows") {
        "windows"
    } else if triple.contains("linux") {
        // Keep the libc visible so a gnu + musl matrix can't collide.
        if triple.ends_with("musl") {
            "linux-musl"
        } else {
            "linux"
        }
    } else if triple.contains("freebsd") {
        "freebsd"
    } else {
        return triple.to_string();
    };
    format!("{os}-{arch}")
}

/// Whether a target triple is a Windows target (artifact gets `.exe`).
pub(crate) fn is_windows_triple(triple: &str) -> bool {
    triple.contains("windows")
}

/// Build the single lean+gui `brood` runtime from the workspace this `nest` was
/// built in — the fallback when no runtime was embedded at install time (a plain
/// `cargo build` of `nest`). `--no-default-features` (no test/observer/MCP/doc/
/// reload/REPL/GC-debug) `+ --features brood/gui`, under the `release-lean`
/// profile (strip + LTO + one codegen unit). Cached under `target/release-lean/`
/// (never clobbering the dev `target/release/`); returns the binary's path.
fn build_lean_runtime() -> std::path::PathBuf {
    let workspace = workspace_dir();
    let cli_manifest = workspace.join("crates/cli/Cargo.toml");
    if !cli_manifest.exists() {
        eprintln!(
            "nest release: no runtime is embedded in this `nest`, and the brood source isn't at \
             {} to build one.\nReinstall (`make install`) to bake the runtime in, or pass \
             --runtime PATH to a prebuilt one.",
            workspace.display()
        );
        std::process::exit(2);
    }
    let lean_bin = workspace.join("target/release-lean/brood");
    eprintln!("nest release: building the lean+gui runtime (stripped + LTO; one-time)…");
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--profile",
            "release-lean",
            "--no-default-features",
            "--features",
            "brood/gui",
        ])
        .arg("--manifest-path")
        .arg(&cli_manifest)
        .status();
    match status {
        Ok(s) if s.success() => lean_bin,
        Ok(s) => {
            eprintln!(
                "nest release: runtime build failed (cargo exited {:?})",
                s.code()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("nest release: could not run cargo to build the runtime: {e}");
            std::process::exit(1);
        }
    }
}

/// The brood workspace root, as baked in at *this* `nest`'s build time
/// (`crates/nest` → up two). For the in-repo dev workflow this is the live
/// source tree; if it's gone (installed `nest`, source moved), the caller hits a
/// clear "pass --runtime" error.
fn workspace_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Human-friendly byte size for the release summary (e.g. `4.2 MB`).
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut n = bytes as f64;
    let mut u = 0;
    while n >= 1024.0 && u < UNITS.len() - 1 {
        n /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{n:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::{is_windows_triple, runtime_cache_path, target_suffix};

    #[test]
    fn target_suffix_maps_common_triples() {
        assert_eq!(target_suffix("aarch64-apple-darwin"), "macos-arm64");
        assert_eq!(target_suffix("x86_64-apple-darwin"), "macos-x86_64");
        assert_eq!(target_suffix("x86_64-unknown-linux-gnu"), "linux-x86_64");
        assert_eq!(target_suffix("aarch64-unknown-linux-gnu"), "linux-arm64");
        // musl keeps the libc visible so a gnu + musl matrix can't collide.
        assert_eq!(
            target_suffix("x86_64-unknown-linux-musl"),
            "linux-musl-x86_64"
        );
        assert_eq!(target_suffix("x86_64-pc-windows-msvc"), "windows-x86_64");
        assert_eq!(target_suffix("x86_64-unknown-freebsd"), "freebsd-x86_64");
        // An unrecognized OS keeps the whole triple — unambiguous, just longer.
        assert_eq!(target_suffix("wasm32-wasip1"), "wasm32-wasip1");
    }

    #[test]
    fn windows_triples_get_exe() {
        assert!(is_windows_triple("x86_64-pc-windows-msvc"));
        assert!(is_windows_triple("x86_64-pc-windows-gnu"));
        assert!(!is_windows_triple("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn runtime_cache_path_is_per_triple() {
        // Path shape only — don't touch the real env (tests run in parallel).
        let p = runtime_cache_path("aarch64-apple-darwin");
        if let Some(p) = p {
            let s = p.to_string_lossy().into_owned();
            assert!(
                s.ends_with("brood/runtimes/aarch64-apple-darwin/brood"),
                "{s}"
            );
        }
        if let Some(p) = runtime_cache_path("x86_64-pc-windows-msvc") {
            assert!(p.to_string_lossy().ends_with("brood.exe"));
        }
    }
}
