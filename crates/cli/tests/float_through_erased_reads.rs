//! **An arm computing on floats it reads out of vectors stays native.**
//!
//! `(defn dot (a b) (+ (* (nth a 0) (nth b 0)) (* (nth a 1) (nth b 1))))` has no float
//! param and no float literal, so the tier-time profile — which types only params — put its
//! `*` on the integer path. Every call then deopted at the `as_int` tag guard, sixteen in a
//! row latched the arm BAILED, and a 2D vector library's every function ran interpreted for
//! the rest of the process: ~400 ns a call for 6 ns of arithmetic. The deopt feedback now
//! recognises an integer guard deopting on a `Float` (reasons 20/21 carry the observed tag)
//! and re-tiers the arm in float context after four of them (`float_deopt_feedback`).
//!
//! Three assertions, on the real `brood` entry point, because the seam is the lowering:
//! the result matches the VM's (`BROOD_NO_JIT=1`), `BROOD_JIT_BAIL_TRACE=1` names the
//! re-tier, and it names no `deopt-thrash-latched` for the arm afterwards. The comparison
//! arms need no re-tier at all: `<`/`<=` on operands nothing types dispatch by tag at
//! runtime (`cmp_dispatch`), so `inside?` (four destructured floats, and a `count` against
//! a literal) and `both-ints` (two let-bound ints compared inside a float-context arm — the
//! shape an optimistic float guess deopted on every call) lower once and stay native.

use std::process::Command;

const PROGRAM: &str = r#"
(defn dot (a b) (+ (* (nth a 0) (nth b 0)) (* (nth a 1) (nth b 1))))
(defn inside? ([ax0 ay0 ax1 ay1] [bx0 by0 bx1 by1])
  (and (<= ax0 bx1) (>= ax1 bx0) (<= ay0 by1) (>= ay1 by0) (< (count [ax0]) 4)))
(defn both-ints (v k)
  (let (n (count v) j (nth v 0) s (* 0.5 k))
    (if (< j n) (+ s 1.0) s)))
(defn loop-sum (i acc)
  (if (>= i 20000)
    acc
    (loop-sum (+ i 1)
      (+ acc (dot [1.5 (* 1.0 i)] [2.0 0.5])
         (both-ints [1 2 3] 2.0)
         (if (inside? [0.0 0.0 10.0 10.0] [(* 0.001 i) 5.0 15.0 15.0]) 1.0 0.0)))))
(io/puts (loop-sum 0 0.0))
(io/puts (dot [1 2] [3 4]))
"#;

fn run(no_jit: bool) -> (String, String) {
    let path = std::env::temp_dir().join(format!("brood-float-erased-{}.blsp", std::process::id()));
    std::fs::write(&path, PROGRAM).expect("write program");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_JIT_BAIL_TRACE", "1")
        .env_remove("BROOD_NO_JIT")
        .env_remove("BROOD_TIER")
        .env_remove("BROOD_VM");
    if no_jit {
        cmd.env("BROOD_NO_JIT", "1");
    }
    let out = cmd.arg(&path).output().expect("run brood");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn float_arithmetic_on_vector_reads_retiers_in_float_context_and_matches_the_vm() {
    let (jit_out, jit_err) = run(false);
    let (vm_out, _) = run(true);
    assert!(
        jit_out.lines().count() == 2 && vm_out.lines().count() == 2,
        "expected two output lines from each arm.\njit:\n{jit_out}\nvm:\n{vm_out}"
    );
    assert_eq!(jit_out, vm_out, "the JIT and the VM disagree");
    // `(dot [1 2] [3 4])` is integer arithmetic and must stay an int (11, not 11.0): the
    // float context is a guess the tag guards keep sound.
    assert!(
        jit_out.lines().nth(1) == Some("11"),
        "int vectors must dot to an int:\n{jit_out}"
    );
    assert!(
        jit_err.contains("[jit-relower] arm=dot reason=float-through-erased-reads"),
        "`dot` was never re-tiered in float context. Bail trace:\n{jit_err}"
    );
    for arm in ["dot", "inside?", "both-ints"] {
        assert!(
            !jit_err.contains(&format!("arm={arm} reason=deopt-thrash-latched")),
            "`{arm}` deopt-thrashed to BAILED. Bail trace:\n{jit_err}"
        );
    }
    // The comparisons dispatch at runtime, so neither comparison arm needs a re-tier.
    for arm in ["inside?", "both-ints"] {
        assert!(
            !jit_err.contains(&format!("[jit-relower] arm={arm}")),
            "`{arm}` should lower once, its comparisons dispatched by tag. Bail trace:\n{jit_err}"
        );
    }
}
