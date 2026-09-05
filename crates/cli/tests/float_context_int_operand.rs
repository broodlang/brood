//! **A float-context arm applied to an int stays native, and answers what the VM answers.**
//!
//! `->float` is `(* 1.0 x)`. The `1.0` puts the arm in float context, so `x` was read through
//! `as_f64`, whose tag guard accepted `Float` alone — and `->float` is called with an int in
//! every program that converts. The arm deopted on every activation, sixteen in a row latched
//! it BAILED, and it ran interpreted for the rest of the process: on `mandelbrot` that is one
//! VM call per pixel (KI-109). The guard now promotes an `Int` with `fcvt_from_sint`, the VM's
//! own `i64 as f64`.
//!
//! Two assertions, on the real `brood` entry point, because the seam is the lowering:
//! the result matches the VM's (`BROOD_NO_JIT=1`), and `BROOD_JIT_BAIL_TRACE=1` names no
//! `deopt-thrash-latched` for the arm. The second is the one that fails without the fix.

use std::process::Command;

const PROGRAM: &str = r#"
(defn conv (x) (* 1.0 x))
(defn mixed (x) (+ (* 2.5 x) (- x 0.5)))
(defn loop-sum (i acc) (if (>= i 200000) acc (loop-sum (+ i 1) (+ acc (conv i) (mixed i)))))
(io/puts (loop-sum 0 0.0))
(io/puts (conv 9007199254740993))
(io/puts (conv -3))
"#;

fn run(no_jit: bool) -> (String, String) {
    let path = std::env::temp_dir().join(format!("brood-float-ctx-{}.blsp", std::process::id()));
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
fn an_int_operand_in_a_float_arm_is_promoted_natively_and_matches_the_vm() {
    let (jit_out, jit_err) = run(false);
    let (vm_out, _) = run(true);
    // Presence first: an empty stdout on both sides would "agree".
    assert!(
        jit_out.lines().count() == 3 && vm_out.lines().count() == 3,
        "expected three output lines from each arm.\njit:\n{jit_out}\nvm:\n{vm_out}"
    );
    assert_eq!(
        jit_out, vm_out,
        "the JIT and the VM disagree on int-promoted float arithmetic"
    );
    // 2^53+1 is not representable: both must round it the same way (the VM's `as f64`).
    assert!(
        jit_out.contains("9007199254740992"),
        "promotion must be `i64 as f64`:\n{jit_out}"
    );
    for arm in ["conv", "mixed"] {
        assert!(
            !jit_err.contains(&format!("arm={arm} reason=deopt-thrash-latched")),
            "`{arm}` deopt-thrashed to BAILED — the float-context guard rejected an int operand \
             instead of promoting it (KI-109). Bail trace:\n{jit_err}"
        );
    }
}
