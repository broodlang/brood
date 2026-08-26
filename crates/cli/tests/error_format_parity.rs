//! The `brood` file runner must render a top-level runtime error IDENTICALLY on
//! every engine — same editor-parseable `file:LINE:COL: kind error: message`
//! prefix under the bytecode VM/JIT and the tree-walker alike.
//!
//! Regression for the `ProgramState::crash` divergence (devlog 2026-07-16): the
//! VM path string-prepended the file onto an already-`located()` message, emitting
//! a stray space (`file: LINE:COL:`) where the tree-walker produced the canonical
//! `file:LINE:COL:`. Found by the differential fuzzer. This guards it in CI —
//! the fuzzer's own catch lives only in the (non-CI) stress sweep.

use std::process::Command;

mod support;

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Run the program at `path` under the given engine env, returning stderr. All
/// engines share ONE path so the file name in the error prefix is common (only
/// the LINE:COL: spacing — the actual bug — can differ).
fn run_stderr(path: &std::path::Path, engine_env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(path).env("BROOD_NO_CHECK", "1");
    for (k, v) in engine_env {
        cmd.env(k, v);
    }
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Every engine's stderr on a top-level error must be byte-identical.
fn assert_parity(src: &str) {
    let dir = std::env::temp_dir();
    // Unique per call: pid across test binaries, counter across parallel tests in
    // one binary (plain `cargo test` runs them as threads).
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!("brood_errfmt_{}_{n}.blsp", std::process::id()));
    std::fs::write(&path, src).expect("write temp program");
    // Default build = bytecode VM + JIT; BROOD_NO_JIT = VM only; BROOD_VM=0 = tree-walker.
    let vm_jit = run_stderr(&path, &[]);
    let vm_only = run_stderr(&path, &[("BROOD_NO_JIT", "1")]);
    let tree = run_stderr(&path, &[("BROOD_VM", "0")]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        vm_jit, tree,
        "VM/JIT vs tree-walker error format diverged\n  jit:  {vm_jit:?}\n  tree: {tree:?}"
    );
    assert_eq!(
        vm_only, tree,
        "VM-only vs tree-walker error format diverged\n  vm:   {vm_only:?}\n  tree: {tree:?}"
    );
    // And it must be the canonical editor form: `<path>:LINE:COL: ` — NO space
    // between the path and the line (the exact bug). The path ends at `.blsp`.
    let idx = tree.find(".blsp:").expect("located prefix present");
    let after = &tree[idx + ".blsp:".len()..];
    assert!(
        after.starts_with(|c: char| c.is_ascii_digit()),
        "expected `<path>.blsp:LINE:...` with no space after the path, got: {tree:?}"
    );
}

#[test]
fn type_error_format_matches_across_engines() {
    assert_parity("(defn boom (x) (bit/and x 0.25))\n(io/puts (boom 5))\n");
}

#[test]
fn unbound_error_format_matches_across_engines() {
    assert_parity("(io/puts (this-symbol-is-not-bound 1 2 3))\n");
}

#[test]
fn arity_error_format_matches_across_engines() {
    assert_parity("(defn f (a b) (+ a b))\n(io/puts (f 1))\n");
}

#[test]
fn thrown_error_format_matches_across_engines() {
    assert_parity("(defn g (n) (if (< n 0) (throw [:neg n]) n))\n(io/puts (g -5))\n");
}
