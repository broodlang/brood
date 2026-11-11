//! `nest release` mechanism — the runtime-resolution + target-triple plumbing
//! behind the `cmd_release` orchestration in `main.rs` (ADR-038). Collection of
//! the project's sources is *policy* (Brood: `project-release/bundle-collect`), and so is
//! naming the artifacts (`project-release/release-plan`); byte assembly is in
//! `brood::bundle`. What is left here is the Rust glue that can only live in this
//! binary: picking which base runtime to append to, out of the one embedded in
//! *this* `nest` at install time, the local cache, or a `--runtime` path. Split out
//! of `main.rs` to keep the thin `nest` shell thin (ADR-028).

/// The lean+gui `brood` runtime baked into this `nest` at install time, so
/// `nest release` can append an app to it with **no Rust toolchain** (ADR-038).
/// `build.rs` writes this file: the prebuilt runtime when `make install` sets
/// `BROOD_EMBED_RUNTIME`, otherwise empty — and an empty slice means "nothing
/// embedded", so `nest release` falls back to building the runtime from source.
const EMBEDDED_RUNTIME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/embedded-runtime"));

/// The triple this `nest` was built for (baked in by `build.rs`) — a `--target`
/// equal to it is served by the embedded runtime, no cache entry needed. It is also the
/// one `--target` whose artifact can actually be executed here, so the only one
/// `nest release`'s boot check can run.
pub(crate) const HOST_TRIPLE: &str = env!("NEST_HOST_TRIPLE");

/// The base runtime bytes to append the app to — the single **lean + gui**
/// runtime (no test/observer/MCP/doc/hot-reload/REPL/GC-debug surface, but the
/// windowed backend kept so any app runs). Priority:
///   1. `--runtime PATH` — read it (a prebuilt/cross runtime you supply).
///   2. with `--target`, a runtime you placed in the local cache
///      (`~/.cache/brood/runtimes/<triple>/brood`), built on/for that target;
///   3. with a `--target` other than the host's, the runtime downloaded from the
///      release matching this `nest` (`runtimes/v<version>/<triple>/brood`, ADR-714) —
///      `cmd_release` fetches it first ([`runtime_to_fetch`]); the host's own
///      triple falls through to:
///   4. the runtime embedded in *this* `nest` at install time — the **no-Rust**
///      path (`make install` bakes it in; see `build.rs`).
///   5. built on demand from the workspace source (cargo fallback — needs Rust),
///      for a plain `cargo build` of `nest` that embedded nothing.
pub(crate) fn resolve_runtime(runtime: Option<&str>, target: Option<&str>) -> Vec<u8> {
    if let Some(r) = runtime {
        return std::fs::read(r).unwrap_or_else(|e| {
            eprintln!("nest release: cannot read runtime binary {r}: {e}");
            std::process::exit(1);
        });
    }
    if let Some(t) = target {
        let placed = runtime_cache_path(t);
        // The host's own triple needs no download: the embedded/built runtime below
        // *is* a runtime for it.
        let fetched = if t == HOST_TRIPLE {
            None
        } else {
            fetched_runtime_path(t)
        };
        for path in [&placed, &fetched].into_iter().flatten() {
            if path.is_file() {
                return std::fs::read(path).unwrap_or_else(|e| {
                    eprintln!(
                        "nest release: cannot read cached runtime {}: {e}",
                        path.display()
                    );
                    std::process::exit(1);
                });
            }
        }
        if t != HOST_TRIPLE {
            let place = placed
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "the runtime cache".to_string());
            eprintln!(
                "nest release: no runtime for {t} on this machine, and none was downloaded.\n\
                 Place a lean runtime built for it at {place}, or pass --runtime PATH."
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

/// Where a runtime downloaded for `triple` is kept (ADR-714):
/// `$XDG_CACHE_HOME/brood/runtimes/v<version>/<triple>/brood`, under THIS `nest`'s
/// version, because a runtime is right only for the release that built it — a cached
/// runtime from an older release must not be appended to by a newer `nest`.
pub(crate) fn fetched_runtime_path(triple: &str) -> Option<std::path::PathBuf> {
    let placed = runtime_cache_path(triple)?;
    let bin = placed.file_name()?.to_owned();
    let runtimes = placed.parent()?.parent()?;
    Some(
        runtimes
            .join(format!("v{}", env!("CARGO_PKG_VERSION")))
            .join(triple)
            .join(bin),
    )
}

/// The runtime `nest release` must download before it can append for `target`, as
/// `(triple, where to keep it)`: only when nothing nearer serves it — no `--runtime`,
/// not the host's own triple, nothing placed in the cache by hand, nothing downloaded
/// before.
pub(crate) fn runtime_to_fetch<'a>(
    runtime: Option<&str>,
    target: Option<&'a str>,
) -> Option<(&'a str, std::path::PathBuf)> {
    let triple = target?;
    if runtime.is_some() || triple == HOST_TRIPLE {
        return None;
    }
    if runtime_cache_path(triple).is_some_and(|p| p.is_file()) {
        return None;
    }
    let fetched = fetched_runtime_path(triple)?;
    (!fetched.is_file()).then_some((triple, fetched))
}

/// Where a prebuilt lean runtime for `triple` lives in the local cache:
/// `$XDG_CACHE_HOME/brood/runtimes/<triple>/brood` (falling back to
/// `~/.cache`; `brood.exe` for Windows triples). `None` if no cache base can
/// be determined ([`brood::boot::cache_dir`]).
pub(crate) fn runtime_cache_path(triple: &str) -> Option<std::path::PathBuf> {
    let dir = brood::boot::cache_dir()?;
    let bin = if triple.contains("windows") {
        "brood.exe"
    } else {
        "brood"
    };
    Some(dir.join("runtimes").join(triple).join(bin))
}

/// Build the single lean `brood` runtime from the workspace this `nest` was
/// built in — the fallback when no runtime was embedded at install time (a plain
/// `cargo build` of `nest`). `--no-default-features` (no test/observer/MCP/doc/
/// reload/REPL/GC-debug) plus this `nest`'s own features (`lean_runtime_features`:
/// `gui`, and `gui-gpu` / `audio` / … when it has them), under the `release-lean`
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
    // The runtime mirrors the features THIS `nest` was built with (minus the dev
    // tooling): a shipped app draws on the GPU and plays sound exactly when the `nest`
    // that shipped it does, rather than always getting the bare `gui` build, which
    // draws none of the pixel-space ops a game is made of.
    let features = lean_runtime_features();
    eprintln!(
        "nest release: building the lean runtime (stripped + LTO; one-time) with {}…",
        features
    );
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--profile",
            "release-lean",
            "--no-default-features",
            "--features",
            &features,
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

/// The `--features` list for the lean runtime: this `nest`'s own brood features, minus
/// the dev tooling (`dev-tools`, `perf-stats`) a shipped app must not carry, and always
/// at least `gui` (the historical baseline). `brood/`-qualified, comma-joined, as cargo
/// wants them for the `cli` package.
fn lean_runtime_features() -> String {
    let mut names: Vec<&str> = brood::builtins::compiled_features()
        .into_iter()
        .filter(|f| !matches!(*f, "dev-tools" | "perf-stats"))
        .collect();
    if !names.contains(&"gui") {
        names.push("gui");
    }
    names
        .iter()
        .map(|f| format!("brood/{f}"))
        .collect::<Vec<_>>()
        .join(",")
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

#[cfg(test)]
mod tests {
    use super::{HOST_TRIPLE, fetched_runtime_path, runtime_cache_path, runtime_to_fetch};

    #[test]
    fn a_downloaded_runtime_is_kept_under_this_nests_version() {
        if let Some(p) = fetched_runtime_path("x86_64-pc-windows-msvc") {
            let s = p.to_string_lossy().into_owned();
            let want = format!(
                "brood/runtimes/v{}/x86_64-pc-windows-msvc/brood.exe",
                env!("CARGO_PKG_VERSION")
            );
            assert!(s.ends_with(&want), "{s} should end with {want}");
        }
    }

    #[test]
    fn only_a_foreign_target_with_nothing_nearer_is_downloaded() {
        // No target, an explicit --runtime, or the host's own triple: never a download.
        assert!(runtime_to_fetch(None, None).is_none());
        assert!(runtime_to_fetch(Some("/some/brood"), Some("x86_64-pc-windows-msvc")).is_none());
        assert!(runtime_to_fetch(None, Some(HOST_TRIPLE)).is_none());
        // A triple no machine has a runtime for (so nothing is placed or cached) is, when
        // there is a cache to keep it in.
        let unknown = "riscv64gc-unknown-none-elf";
        if fetched_runtime_path(unknown).is_some() {
            let (triple, path) = runtime_to_fetch(None, Some(unknown))
                .expect("a foreign target with nothing cached is downloaded");
            assert_eq!(triple, unknown);
            assert_eq!(Some(path), fetched_runtime_path(unknown));
        }
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
