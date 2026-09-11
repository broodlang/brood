//! What this binary **is**: its version, build id, stdlib id and build time, and the
//! cargo features it was compiled with — the `(system/…)` identity surface.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::LispResult;

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "system/features",
        Arity::exact(0),
        Sig::nullary(seq),
        &[],
        "The optional build features this runtime was compiled with, as a vector of keywords (e.g. [:jit :treesit :gui]). A *bound* builtin does not imply a working one — with the `gui` feature off, `gui-open` is still bound and raises at call time — so an app that degrades rather than fails must ask the build, not `bound?`. `system/feature?` is the predicate over this.",
        features);
    primitives.def(
        "system/build-id",
        Arity::exact(0),
        Sig::nullary(string),
        &[],
        "This brood build's identity as \"<version>+<git-sha>+<binary-stamp>\" (e.g. \"0.1.0+dcab7ca+18f2e1a9b3c4d5e6\") — the correct staleness stamp for an on-disk cache of anything the kernel computes. Changes on any rebuild, committed or not: the binary-stamp half is this executable's own mtime, read at runtime, so it can't go stale the way a git-sha-only stamp would across an uncommitted local rebuild.",
        build_id);
    primitives.def(
        "%build-stdimage?",
        Arity::exact(0),
        Sig::new(vec![], bool_ty),
        &[],
        "Was this binary built with the stdlib startup image compiled in (`./configure --with-stdimage`, the default)? False for a build that must never touch a cache directory. The per-run switch is `BROOD_NO_STDIMAGE=1`; this is the build-time one, the same two layers the JIT has.",
        build_stdimage);
    primitives.def(
        "system/stdlib-id",
        Arity::exact(0),
        Sig::nullary(string),
        &[],
        "This build's STANDARD LIBRARY identity as \"<version>+<git-sha>+<content-hash>\" — like system/build-id, but hashing what is baked in rather than this executable's mtime. Every binary built from one tree (brood, nest, brood-lsp) reports the SAME system/stdlib-id, so they can share one cache of anything derived from std/ — the stdlib startup image is keyed on it. Use system/build-id for a cache of something binary-specific, and this for a cache of something the standard library determines. It changes on any edit to any `.blsp`, committed or not.",
        stdlib_id);
    primitives.def(
        "system/brood-version",
        Arity::exact(0),
        Sig::nullary(string),
        &[],
        "This runtime's semantic version as a string (e.g. \"0.1.0\") — just the semver, without the git-sha/binary-stamp that `system/build-id` carries. What a project's `:brood` manifest constraint is checked against (a project can require `:brood \">= 0.2\"` and `nest` refuses an older runtime with a clear message).",
        brood_version);
    primitives.def(
        "system/build-time",
        Arity::exact(0),
        Sig::nullary(int.union(nil_ty)),
        &[],
        "When THIS executable was built, as Unix epoch milliseconds (the unit os/now speaks, so datetime/epoch-ms-> formats it directly); nil when the platform won't say. It is the binary's own mtime — the fact system/build-id's third field encodes as an opaque hex stamp, in a unit a human can read — so it changes on any rebuild, unlike a baked-in build-script constant. This is the RUNTIME's build time; a `nest release` bundle's own is a different fact, stamped at release and read via project/build-info.",
        build_time);
}

/// `(system/features)` — the optional build features this runtime was compiled with, as a
/// vector of keywords (e.g. `[:jit :treesit :gui]`).
///
/// The point is that a *bound* builtin does not imply a working one: with the `gui`
/// feature off, `gui-open` is still bound and still raises at call time, so
/// `(bound? 'gui-open)` answers "yes" on a runtime that cannot open a window. An
/// app that wants to degrade rather than fail needs to ask the build, not the
/// environment — and the only alternative was provoking the error and matching on
/// its prose, which silently breaks whenever the message is reworded.
pub(super) fn features(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    // Order is stable (declaration order, not cfg order) so a printed value diffs
    // cleanly between builds.
    let mut out = Vec::new();
    if cfg!(feature = "gui") {
        out.push(value::kw("gui"));
    }
    if cfg!(feature = "gui-gpu") {
        out.push(value::kw("gui-gpu"));
    }
    if cfg!(feature = "audio") {
        out.push(value::kw("audio"));
    }
    if cfg!(feature = "clipboard") {
        out.push(value::kw("clipboard"));
    }
    if cfg!(feature = "jit") {
        out.push(value::kw("jit"));
    }
    if cfg!(feature = "treesit") {
        out.push(value::kw("treesit"));
    }
    if cfg!(feature = "wasm") {
        out.push(value::kw("wasm"));
    }
    if cfg!(feature = "dev-tools") {
        out.push(value::kw("dev-tools"));
    }
    if cfg!(feature = "perf-stats") {
        out.push(value::kw("perf-stats"));
    }
    Ok(heap.alloc_vector(out))
}

/// `(system/build-id)` — this `brood` build's identity, `"<version>+<git-sha>+<binary-
/// stamp>"` (e.g. `"0.1.0+dcab7ca+18f2e1a9b3c4d5e6"`). The correct staleness
/// stamp for an on-disk cache of anything the kernel computes (the checker's
/// own logic is Rust, so its results are not portable across binaries).
///
/// The git-sha half is baked in at compile time (`BROOD_GIT_SHA`) and is
/// **not** by itself a reliable staleness stamp: it's `git rev-parse --short
/// HEAD`, which doesn't change across an uncommitted rebuild on the same
/// commit (exactly the case during active development on the checker
/// itself), and `build.rs`'s `rerun-if-changed` only watches `.git/HEAD`/
/// `.git/refs/heads` — a plain source edit + rebuild doesn't even re-run it.
/// The `binary-stamp` half (this executable's own mtime, read at *runtime*
/// via [`binary_stamp`]) closes that gap: it changes on literally any
/// rebuild, committed or not, for any reason, with no `build.rs` changes
/// needed — correct by construction rather than by tracking which source
/// files matter to which cache.
pub(super) fn build_id(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = build_id_string();
    Ok(heap.alloc_string(&id))
}

/// `(system/stdlib-id)` — the embedded standard library's content identity. See
/// [`stdlib_id_string`]: same for every binary built from one tree, which is what lets them
/// share a stdlib startup image.
/// `(%build-stdimage?)` — was this binary built with the stdlib startup image compiled in?
///
/// Two layers govern the image, the same way they govern the JIT: a **build-time** choice
/// (`./configure --without-stdimage`, i.e. this cargo feature) for a deployment that must
/// never touch a cache directory — read-only, sandboxed, or simply a machine that should
/// grow no ~2 MB file — and a **runtime** one (`BROOD_NO_STDIMAGE=1`) for A/B and bisect
/// on one binary. The runtime lever has to stay runtime: you cannot A/B two builds without
/// confounding the build.
pub(super) fn build_stdimage(_: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::Bool(cfg!(feature = "stdimage")))
}

pub(super) fn stdlib_id(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_string(&stdlib_id_string()))
}

/// `(system/brood-version)` — this runtime's semantic version (`CARGO_PKG_VERSION`,
/// e.g. `"0.1.0"`): the string a project's `:brood` manifest constraint is
/// checked against (ADR-209). Just the semver — the git-sha and binary-stamp
/// live in `system/build-id`. The kernel is the only place this value exists, so it is
/// a primitive; the policy that reads a `:brood` constraint and refuses an
/// incompatible runtime is Brood (`std/tool/project.blsp`).
pub(super) fn brood_version(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(heap.alloc_string(env!("CARGO_PKG_VERSION")))
}

/// The `(system/build-id)` string as plain Rust — shared with the boot cache
/// (`lib.rs`), which uses it as the staleness key for the expanded-prelude
/// cache (the prelude is `include_str!`'d, so any binary change covers it).
/// A **content** id for the embedded standard library: version + git sha + a hash of every
/// baked-in module's source. Unlike [`build_id_string`] it carries no executable mtime, so
/// `brood`, `nest` and `brood-lsp` built from one tree all report the SAME id — which is what
/// lets them share one stdlib startup image instead of writing ~2 MB each.
///
/// The hash is computed at COMPILE time. Hashing ~1 MB of sources per process would cost
/// around a millisecond of a ~23 ms boot, and the boot path is exactly where this is read;
/// a `const fn` moves that to the build for free. It also means the id changes precisely
/// when the stdlib does — an uncommitted edit to a `.blsp` invalidates the image, which the
/// git sha alone would not catch.
pub(crate) fn stdlib_id_string() -> String {
    format!(
        "{}+{}+{}",
        env!("CARGO_PKG_VERSION"),
        env!("BROOD_GIT_SHA"),
        env!("BROOD_STDLIB_HASH")
    )
}

pub(crate) fn build_id_string() -> String {
    format!(
        "{}+{}+{}",
        env!("CARGO_PKG_VERSION"),
        env!("BROOD_GIT_SHA"),
        binary_stamp()
    )
}

/// This running executable's own last-modified time, as a hex nanosecond
/// stamp — computed once per process (`OnceLock`) since it never changes
/// mid-run. `"unknown"` if the executable path or its metadata can't be read
/// (e.g. a sandboxed environment with no `/proc/self/exe`-equivalent) — a
/// stable-but-uninformative fallback, not a crash; the git-sha half of
/// `build_id` still carries some staleness signal in that case.
fn binary_stamp() -> &'static str {
    static STAMP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    STAMP.get_or_init(|| {
        binary_mtime()
            .map(|d| format!("{:x}", d.as_nanos()))
            .unwrap_or_else(|| "unknown".to_string())
    })
}

/// This running executable's own last-modified time, computed once per process
/// (`OnceLock`) since it never changes mid-run. `None` when the executable path or
/// its metadata can't be read — the same sandbox case [`binary_stamp`] reports as
/// `"unknown"`.
fn binary_mtime() -> Option<std::time::Duration> {
    static MTIME: std::sync::OnceLock<Option<std::time::Duration>> = std::sync::OnceLock::new();
    *MTIME.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| std::fs::metadata(p).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
    })
}

/// `(system/build-time)` — when THIS executable was built, as Unix epoch
/// milliseconds (the unit `os/now` speaks, so `datetime/epoch-ms->` formats it
/// directly); nil when the platform won't say.
///
/// It is the binary's own mtime — the same fact `build_id`'s third field encodes as
/// an opaque hex stamp, given a unit a human can read. A `build.rs` timestamp would
/// have been the obvious alternative and is the wrong one: cargo only re-runs a build
/// script when one of its `rerun-if-changed` paths moves, so a plain source edit and
/// rebuild leaves the baked-in constant reading whenever the script last ran, which
/// is precisely the reading nobody wants from a "when was this built" question. The
/// mtime changes on any rebuild for any reason, with no build-script bookkeeping.
///
/// This is the RUNTIME's build time. A `nest release` bundle's own build time is a
/// different fact, stamped into the bundle at release (`project/build-info`).
pub(super) fn build_time(_: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(match binary_mtime() {
        Some(d) => Value::int(d.as_millis() as i64),
        None => Value::Nil,
    })
}
