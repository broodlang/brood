//! Pins who may call `CompiledArm::frame_size_for_new_entry`.
//!
//! That read loads `inline_installed`, which the background inline upgrade flips at any
//! moment. It is correct **only** for code that is BUILDING a frame and captures the result;
//! any consumer that re-derives it to interpret a frame that already exists can get a
//! different size than the frame was built to, and then the staged `[callee, args…]` region
//! is written at one offset and read at another.
//!
//! That is KI-48 (two captured crashes; `root_at(9)` on a len-8 roots stack), and it was the
//! third appearance of this exact anti-pattern — KI-26 and the two ADR-210 bugs were the
//! others. Fixing the two live instances does not stop a fourth, so this test makes a new
//! call site a deliberate act: add it here with a note saying which frame it BUILDS.
//!
//! This is a source-text guard, in the same spirit as the `debug_flags` catalogue test and
//! `prelude_manifest.rs`. Two limits, stated so a green run is not over-read:
//!
//!   * it is **file-granular** — a new, wrong call inside an already-allowlisted file passes.
//!     Those six files are the ones to review by hand when this code changes.
//!   * it cannot prove a caller is correct, only that someone had to justify it.
//!
//! Verified by sabotage: reintroducing the read in `dispatch.rs` (a consumer, not a builder)
//! fails this test with that file named.

use std::path::Path;

/// Files permitted to call it, with the reason. A frame BUILDER may; a consumer may not.
const ALLOWED: &[(&str, &str)] = &[
    (
        "crates/lisp/src/eval/compile/jit_runtime.rs",
        "builds the callee frame before entering native; captures once into `frame_nslots` \
         and passes it on (its own comment: the two must agree on the same frame boundary)",
    ),
    (
        "crates/lisp/src/eval/compile/mod.rs",
        "`hof_apply_native` builds the HOF fast frame; captures once into `nslots`",
    ),
    (
        "crates/lisp/src/core/heap/vm_cache.rs",
        "records the size into a FastLink slot for a later native entry; protected by \
         `invalidate_fast_links_for` at the swap (NOT by an epoch bump — see the comment)",
    ),
    (
        "crates/lisp/src/eval/compile/vm_run_bc.rs",
        "the trampoline: decides and captures the size this frame is built to, then tells \
         `jit_dispatch_tail`",
    ),
    (
        "crates/lisp/src/eval/compile/ir.rs",
        "the definition itself",
    ),
    (
        "crates/lisp/src/eval/compile/tests.rs",
        "tests that assert the racy read's behaviour directly",
    ),
];

#[test]
fn only_frame_builders_read_the_racy_active_frame_size() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut offenders = Vec::new();

    let mut stack = vec![root.join("crates/lisp/src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read file");
            // Only actual calls, not the prose in doc comments that explains the hazard.
            let calls = text
                .lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !t.starts_with("//") && l.contains("frame_size_for_new_entry(")
                })
                .count();
            if calls == 0 {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if !ALLOWED.iter().any(|(f, _)| rel.ends_with(f)) {
                offenders.push(format!("  {rel} ({calls} call(s))"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "`frame_size_for_new_entry()` is a RACY read of `inline_installed` — correct only \
         where a frame is being BUILT, and only if the result is captured and passed on.\n\
         New call site(s) outside the allowlist:\n{}\n\n\
         If this code BUILDS a frame, add it to ALLOWED in \
         crates/lisp/tests/frame_size_callsites.rs with the reason. If it merely needs to \
         know an existing frame's size, it must be TOLD that size instead — re-deriving it \
         is KI-48 (and KI-26, and two ADR-210 bugs before that).",
        offenders.join("\n")
    );
}
