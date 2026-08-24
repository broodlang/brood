//! Pins who may read `inline_installed` — the KI-48 family, generalised.
//!
//! `frame_size_callsites.rs` next door pins half the rule: a frame's size is a racy read, so
//! only a frame BUILDER may take it, and it must capture it once and tell every consumer.
//! That was not enough, because it governs only ONE of the two values that have to agree.
//!
//! Two facts describe one frame: **which code will run** (`jit_code`, which the background
//! inline upgrade swaps) and **how big the frame is** (`nslots` vs `inline_nslots`, selected
//! by `inline_installed`). The two-stage swap stores the flag BEFORE the pointer precisely so
//! a reader that Acquire-loads `jit_code == inline_code` is guaranteed to see the flag true —
//! which holds only for a reader that reads **code first**. Read flag-then-code and the
//! Release/Acquire chain guarantees nothing: `vm_run_bc` and `dispatch` sized the frame from
//! the flag and then called `jit_tier`, which re-loaded `jit_code`, so a peer process (the
//! `CompiledArm` is shared across a runtime since ADR-215) swapping in that window left the
//! caller holding a small frame while the *inlined* native ran against it and raw-wrote past
//! the frame top — a 12-slot overshoot on `fold` (`nslots` 13, `inline_nslots` 25).
//!
//! The generalised rule this file enforces for the pair it can name:
//!
//!   > Any pair of values that must agree about ONE frame has to be read as ONE snapshot by
//!   > whoever builds that frame. Telling a consumer the size is not enough if the consumer
//!   > re-derives the code pointer — or vice versa.
//!
//! In practice that means sizing from the pointer you loaded (`frame_size_for_code`) rather
//! than from a second, independently-racing read of the flag, or — when the frame is already
//! built — handing the size to `jit_tier_in_frame`, which declines the entry if the code it
//! loads wants a bigger one. `jit_tier` no longer has a size-free spelling at all, so that
//! half is now enforced by the compiler; this test covers the flag, which does not have a
//! type to hide behind.
//!
//! Same two limits as its sibling: file-granular (a new, wrong read inside an already-listed
//! file passes), and it proves only that someone had to justify the read.
//!
//! Verified by sabotage: an unlisted file containing `arm.inline_installed.load(Acquire)`
//! fails this test with that file named.

use std::path::Path;

/// Files permitted to read the racy `inline_installed` flag, with the reason.
const ALLOWED: &[(&str, &str)] = &[
    (
        "crates/lisp/src/eval/compile/ir.rs",
        "declares it, and `frame_size_for_new_entry` is the one sanctioned read (documented \
         racy, for frame BUILDERS only)",
    ),
    (
        "crates/lisp/src/eval/compile/jit_runtime.rs",
        "performs the swap that sets it, and clears it on epoch invalidation; its own sizing \
         reads go through `frame_size_for_code` (the pointer, not the flag)",
    ),
    (
        "crates/lisp/src/eval/compile/vm_run_bc.rs",
        "the trampoline: reads it ONCE to decide the size this frame is built to, captures \
         that into `frame_nslots`, and hands it to `jit_tier_in_frame` / `jit_dispatch_tail`",
    ),
    (
        "crates/lisp/src/eval/compile/dispatch.rs",
        "gates its `push_frame`-sized fast path on the flag being false; the entry is \
         re-checked against the built size by `jit_tier_in_frame`, so a flip in the window \
         declines rather than overshoots",
    ),
    (
        "crates/lisp/src/eval/compile/mod.rs",
        "arm construction only — initialises the field to false; it never reads it",
    ),
    (
        "crates/lisp/src/eval/compile/tests.rs",
        "tests that stage the flag directly to assert the guard's behaviour",
    ),
];

#[test]
fn only_justified_readers_touch_the_racy_inline_installed_flag() {
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
            // Actual reads only — not the prose in the doc comments that explains the hazard.
            let reads = text
                .lines()
                .filter(|l| {
                    let t = l.trim_start();
                    !t.starts_with("//") && l.contains("inline_installed")
                })
                .count();
            if reads == 0 {
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if !ALLOWED.iter().any(|(f, _)| rel.ends_with(f)) {
                offenders.push(format!("  {rel} ({reads} mention(s))"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "`inline_installed` is HALF of a pair that must be read as one snapshot: it says which \
         frame size the *currently installed* code wants, and the background inline upgrade \
         flips it at any moment — in another process, since the `CompiledArm` is shared \
         (ADR-215).\nNew reader(s) outside the allowlist:\n{}\n\n\
         If you need the frame size for a code pointer you already hold, use \
         `frame_size_for_code(arm, code)`. If you have already BUILT a frame, pass its size to \
         `jit_tier_in_frame`. Only add a file here if it BUILDS a frame from this read and \
         tells every consumer the result — re-deriving it is KI-48 (and KI-26, and two ADR-210 \
         bugs before that).",
        offenders.join("\n")
    );
}
