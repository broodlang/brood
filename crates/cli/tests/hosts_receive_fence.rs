//! **An arm whose non-tail callees reach a `receive` is refused the native tier by name,
//! before it is lowered** (`compile::arm_hosts_receive`, asked in `jit_tier_in_frame`).
//!
//! `make tier-audit` reported `bench-supervisor/fill reason=suspend-latched` on the
//! `supervisor` row: `fill` — a named self-tail loop with one non-tail call — passed the
//! profitability gate, lowered (twice: small native and its xcall re-lowering), ran its
//! first `start-child` → `gen/call` → `receive` nested under its native gateway, parked
//! its OS worker DIRTY once, and was latched `BAILED` by `jit_latch_suspend_host`. The
//! steady state was right (the VM), the route to it was two wasted compiles and a dirty
//! park, and the audit could not tell it from a thrasher. The direct `%receive` fence sees
//! only the arm's own chunk; this one follows the arm's non-tail call sites through named,
//! already-compiled callees.
//!
//! Pinned two ways: the `[jit-bail] arm=fill reason=hosts-receive` line must appear (so the
//! fence, not some other refusal, decided), and there must be no `suspend-latched` and no
//! `dirty-receive-block` for the run. Sabotage-verified: skipping the fence puts
//! `suspend-latched` and one dirty park back. A TAIL site is deliberately not followed —
//! `vm_direct_call.rs`'s `via` (a tail call into a receiving callee) must keep lowering,
//! and that file asserts it does.

use std::process::Command;

const PROGRAM: &str = r#"
(def root (self))
(defn echo () (receive ([:ping from] (do (send from :pong) (echo)))))
(def e (spawn (echo)))
(defn rpc (v) (do (send e [:ping (self)]) (receive (:pong v))))
;; `fill`'s shape: a named self-tail loop whose ONE non-tail call reaches a receive two
;; named hops down (`fill` → `rpc` → `%receive`). Nothing else keeps it off the JIT: no
;; float slot, no `try`, a plain int loop.
(defn fill (i acc)
  (if (= i 0) acc (fill (- i 1) (+ acc (rpc 1)))))
(defn run (n)
  (let (w (spawn (send root [:r (fill n 0)])))
    (receive ([:r v] v) (after 60000 :timeout))))
(io/puts (str "run = " (pr-str (run 20000))))
"#;

#[test]
fn an_arm_whose_callee_receives_is_refused_by_name_and_never_parks_dirty() {
    let path = std::env::temp_dir().join(format!(
        "brood-hosts-receive-fence-{}.blsp",
        std::process::id()
    ));
    std::fs::write(&path, PROGRAM).expect("write program");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_JIT_BAIL_TRACE", "1")
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
        stdout.contains("run = 20000"),
        "the receives did not all round-trip:\n{stdout}\n{stderr}"
    );
    // No JIT (a no-jit build or a ceiling below native): nothing tiers, so the fence is
    // never asked — say so rather than fail.
    if !stderr.lines().any(|l| l.starts_with("[jit-")) {
        eprintln!("hosts_receive_fence: no JIT activity at all (no JIT, or the tier ceiling is below native) — vacuous");
        return;
    }
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("[jit-bail] arm=fill reason=hosts-receive")),
        "`fill` was not refused by the receive fence — either it never got hot enough to be \
         asked, or another refusal got there first; find out which before trusting this \
         file:\n{stderr}"
    );
    let latched = stderr
        .lines()
        .filter(|l| l.contains("suspend-latched"))
        .count();
    assert_eq!(
        latched, 0,
        "an arm was latched off the native tier by a parked receive — it lowered when the \
         fence should have refused it:\n{stderr}"
    );
    let dirty = stderr
        .lines()
        .filter(|l| l.contains("dirty-receive-block"))
        .count();
    assert_eq!(
        dirty, 0,
        "{dirty} receives parked their OS worker (dirty blocks): a native arm hosted a receive"
    );
}
