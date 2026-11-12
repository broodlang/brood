//! **A function with a bytes literal compiles to the VM, so its `receive` parks cleanly.**
//!
//! `#b"…"` was missing from the compiler's self-evaluating literals (ratios and decimals were
//! added to that list earlier for the same reason), so any function containing one ran on the
//! tree-walker. A `receive` under the tree-walker cannot be state-captured: it blocks its OS
//! worker (a *dirty* receive), and the scheduler starts an overflow thread to keep the pool
//! going. `http/request` has `#b""` in its body, so every HTTP request dirty-blocked twice and
//! 500 concurrent `http/fetch`es grew past 64 overflow threads (found 2026-10-20, porting
//! brood-benchmarks' `http` row). Counted by the runtime's own `dirty-receive-block` line
//! under `BROOD_JIT_BAIL_TRACE`.

use std::process::Command;

const PROGRAM: &str = r#"
(def me (self))
;; The bytes literal is the whole point: without it this compiles either way.
(defn waits (tag)
  (let (marker #b"\r\n")
    (receive ([^tag v] (+ v (count marker))) (after 5000 :timeout))))
(io/puts (str "compiled=" (not (nil? (%vm-arm-ops waits 1)))))
(def kids (map (range 0 50) (fn (i) (spawn (send me [:r (waits :go)])))))
;; Every child reaches its `receive` before `:go` exists, so all fifty park: the thing under
;; test happens fifty times rather than whenever the scheduler happens to be slow.
(proc/sleep 100)
(doseq (k kids) (send k [:go 1]))
;; Collected by a tail loop, not a `fold` callback: a `receive` under a native HOF is the
;; unparkable shape this test counts, and the collector must not be one.
(defn collect (left acc) (if (= left 0) acc (collect (dec left) (+ acc (receive ([:r v] v))))))
(io/puts (str "sum=" (collect 50 0)))
"#;

#[test]
fn a_bytes_literal_keeps_its_function_on_the_vm_and_its_receive_clean() {
    let path =
        std::env::temp_dir().join(format!("brood-bytes-literal-{}.blsp", std::process::id()));
    std::fs::write(&path, PROGRAM).expect("write program");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_JIT_BAIL_TRACE", "1")
        // The tree-walker job runs everything on tier 0, where every receive is dirty by
        // design; this is about the VM.
        .env_remove("BROOD_TIER")
        .arg(&path)
        .output()
        .expect("run brood");
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("sum=150"),
        "every process should answer 1 + the literal's 2 bytes:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("compiled=true"),
        "a function containing #b\"…\" must compile to the VM:\n{stdout}\n{stderr}"
    );
    let dirty = stderr
        .lines()
        .filter(|l| l.contains("dirty-receive-block"))
        .count();
    assert_eq!(
        dirty, 0,
        "{dirty} receives blocked their OS worker: a function with a bytes literal ran on the \
         tree-walker\n{stderr}"
    );
}
