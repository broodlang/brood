//! **The VM→native direct call keeps a `receive` down a native callee's tail chain a CLEAN
//! suspend** (`docs/compute-frontier.md` §7.12).
//!
//! An interpreted frame's non-tail `Inst::Call` to an already-native callee runs the callee
//! in place instead of round-tripping through the driver. The first cut of that used the
//! native→native fast link, whose outcome-4 (staged tail call) handling is NESTED: the tail
//! chain ran under a native gateway, so a `receive` two calls down could not be
//! state-captured and parked its whole OS worker instead — 19 820 dirty parks on the
//! `supervisor` benchmark row, where the frame path has none. The shape is every
//! `gen/call` behind a one-line wrapper: `fill` (interpreted) → `start-child` (native, one
//! tail call) → `gen/call` → `receive`.
//!
//! This pins the fix — every non-value outcome is handed back to the driver and honoured at
//! frame level — by the runtime's own counter of dirty parks
//! (`process::dirty_receive_block_count`, printed by `BROOD_JIT_BAIL_TRACE` as
//! `dirty-receive-block`), and refuses to be vacuous: it asserts the wrapper DID lower, via
//! `BROOD_JIT_DUMP_IR`, so a tree where the shape stops tiering fails loudly instead of
//! passing by accident. Sabotage-verified: dispatching the staged tail call nested
//! (`apply_value` in `exec_chunk`'s `DirectNative::Tail` arm) turns the count into
//! thousands.

use std::process::Command;

const PROGRAM: &str = r#"
(def root (self))
;; `rpc` receives, so its own chunk is fenced off the JIT. `via` is one tail call behind a
;; vector build — a named defn that lowers — and is what the interpreted driver calls
;; NON-tail: the direct call's shape. `drive` stays on the VM: the `try` puts its chunk out
;; of the JIT subset.
(defn echo () (receive ([:ping from] (do (send from :pong) (echo)))))
(def e (spawn (echo)))
(defn rpc (v) (do (send e [:ping (self)]) (receive (:pong (nth v 0)))))
;; NOT `(defn via (x) (rpc x))`: a pure delegating wrapper is a passthrough the VM redirects
;; without ever pushing its frame, so it never tiers. Building the argument keeps it an arm
;; of its own — the same shape as `supervisor/start-child`'s `(gen/call sup [:start-child spec])`.
(defn via (x) (rpc [x]))
(defn drive (i acc)
  (if (= i 0) acc (drive (- i 1) (+ acc (via 1) (try 0 (catch _ 0))))))
;; Run in a spawned process: only a green process in a capture run can be parked cleanly
;; (or dirtily) at all — the root thread's receive always blocks.
(defn run (n)
  (let (w (spawn (send root [:r (drive n 0)])))
    (receive ([:r v] v) (after 60000 :timeout))))
(io/puts (str "run = " (pr-str (run 20000))))
(io/puts (str "run2 = " (pr-str (run 20000))))
"#;

#[test]
fn a_native_wrapper_tail_calling_into_a_receive_never_parks_dirty() {
    let path =
        std::env::temp_dir().join(format!("brood-vm-direct-call-{}.blsp", std::process::id()));
    std::fs::write(&path, PROGRAM).expect("write program");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_JIT_BAIL_TRACE", "1")
        .env("BROOD_JIT_DUMP_IR", "1")
        .env_remove("BROOD_NO_JIT")
        .env_remove("BROOD_TIER")
        .env_remove("BROOD_VM")
        .arg(&path)
        .output()
        .expect("run brood");
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("run = 20000") && stdout.contains("run2 = 20000"),
        "the parked receives did not all round-trip:\n{stdout}\n{stderr}"
    );
    // No `[jit-ir]` line at all means nothing lowered in this process — a no-JIT build or
    // a ceiling below native (`BROOD_NO_JIT`/`BROOD_TIER`) — and the path under test does
    // not exist; say so rather than fail.
    if !stderr.lines().any(|l| l.starts_with("[jit-ir]")) {
        eprintln!("vm_direct_call: nothing lowered (no JIT, or the tier ceiling is below native) — vacuous");
        return;
    }
    // Vacuity guard: the wrapper must have gone native, or nothing here was exercised.
    let via_lowered = stderr
        .lines()
        .any(|l| l.starts_with("[jit-ir]") && l.contains("(via)"));
    assert!(
        via_lowered,
        "`via` never lowered, so the direct call was never taken — the shape under test \
         no longer tiers; find out why before trusting this file:\n{stderr}"
    );
    let dirty = stderr
        .lines()
        .filter(|l| l.contains("dirty-receive-block"))
        .count();
    assert_eq!(
        dirty, 0,
        "{dirty} receives parked their OS worker (dirty blocks) under a native frame: the \
         VM→native direct call ran a tail chain nested instead of handing it to the driver"
    );
}
