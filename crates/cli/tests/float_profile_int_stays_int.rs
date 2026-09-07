//! **An arm the float profile guessed wrong still answers what the VM answers.**
//!
//! `op_is_float` reads `slot_float`, which `emit.rs` calls "a single-pass approximation" —
//! and the whole float lowering rests on its stated contract: *a wrong guess is safe (a
//! deopt, not a miscompile)*. KI-109 broke that contract while fixing a real stall. It made
//! an `Int` operand **promote** rather than deopt, which is right for mixed arithmetic —
//! `(+ 1.5 1)` really is 2.5 — but the promotion was unconditional, so an arm whose float
//! guess was simply wrong computed in floats and published a float where the VM publishes
//! an int.
//!
//! The prelude's unary `-` is `(%sub 0 x)`. `math/round`'s negative branch calls it on a
//! float (`(- x)`) and on an int (`(- (math/floor …))`) in the same body, so one hot
//! negative round float-profiles the shared arm and every later `(- <int>)` in the process
//! answered a float. `(math/round -16.4)` gave `-16.0`, and pong failed 22 of 101 tests with
//! `rem: expected int, got float` (KI-114).
//!
//! The fix is `float_pair_gate`: promote only once some operand is PROVEN float at runtime,
//! which is the VM's own rule for when an op is float arithmetic at all. So this file pins
//! both directions — the wrong guess deopts, and genuinely mixed arithmetic still promotes.

use std::process::Command;

/// Every line is `label = <value>`, so a diff names the shape that broke.
///
/// `burn` drives the shared unary `-` arm with floats until it tiers float-profiled; the
/// probes then apply the very same arm, and `math/round` on a negative float, to ints.
/// `mix` is the other direction: a float slot and an int slot in one op, which must still
/// answer a float rather than deopt (the over-strict fix, which pong would not have caught).
///
/// **What this file does NOT cover, stated so it is not mistaken for coverage.** The same
/// rule now guards two more lowerings — `Prim2SlotSlot` and `Prim2`'s type-erased
/// `Op::Handle` path — and no program written for this reached either with a WRONG guess.
/// The shapes that should (`(defn add2 (a b) (+ a b))` warmed on floats; nbody's
/// `(- (nth v 0) (nth v 1))` beside a float slot) never tier: `add2` is not elected at all,
/// and the vector one thrash-latches on the INT path first, so both answer via the VM.
/// Sabotaging those two arms leaves this file green — checked, not assumed. They are fixed
/// on the argument that the unsoundness is identical, not on a test.
const PROGRAM: &str = r#"
(defn burn (n acc)
  (if (= n 0) acc (burn (- n 1) (+ acc (- 1.5)))))
(defn mix (f i) (+ f i))
(defn mix-burn (n acc)
  (if (= n 0) acc (mix-burn (- n 1) (+ (mix 0.5 1) (* acc 0.5)))))
(io/puts (str "burn = " (pr-str (burn 300000 0.0))))
(io/puts (str "mix-burn = " (pr-str (mix-burn 300000 0.0))))
(io/puts (str "neg-int = " (pr-str (- 33))))
(io/puts (str "neg-int-typed = " (->string (int? (- 33)))))
(io/puts (str "round-neg = " (pr-str (math/round -16.4))))
(io/puts (str "round-neg-typed = " (->string (int? (math/round -16.4)))))
(io/puts (str "round-pos = " (pr-str (math/round 16.4))))
(io/puts (str "mixed = " (pr-str (mix 1.5 1))))
(io/puts (str "mixed-typed = " (->string (float? (mix 1.5 1)))))
"#;

fn run(no_jit: bool) -> (String, String) {
    let path = std::env::temp_dir().join(format!(
        "brood-float-profile-{}-{}.blsp",
        std::process::id(),
        no_jit
    ));
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
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_wrong_float_guess_deopts_instead_of_publishing_a_float() {
    let (jit_out, jit_err) = run(false);
    let (vm_out, _) = run(true);
    // Presence first: two empty stdouts would "agree" (and a killed build prints nothing).
    assert_eq!(
        jit_out.lines().count(),
        9,
        "expected nine output lines from the JIT arm.\nstdout:\n{jit_out}\nstderr:\n{jit_err}"
    );
    assert_eq!(
        jit_out, vm_out,
        "the JIT and the VM disagree — a float-profiled arm applied to ints published a float \
         (KI-114). jit stderr:\n{jit_err}"
    );
    // Pin the values too, so a change that makes BOTH engines wrong is still a failure.
    for expected in [
        "neg-int = -33",
        "neg-int-typed = true",
        "round-neg = -16",
        "round-neg-typed = true",
        "round-pos = 16",
    ] {
        assert!(
            jit_out.lines().any(|l| l == expected),
            "missing `{expected}` — an int-valued expression came back a float.\n{jit_out}"
        );
    }
    // The other direction: genuinely mixed arithmetic must still promote (KI-109), not be
    // refused by an over-strict gate.
    for expected in ["mixed = 2.5", "mixed-typed = true"] {
        assert!(
            jit_out.lines().any(|l| l == expected),
            "missing `{expected}` — the gate refused a genuinely mixed float/int op.\n{jit_out}"
        );
    }
    assert!(
        !jit_err.contains("arm=mix reason=deopt-thrash-latched"),
        "`mix` deopt-thrashed to BAILED — the gate rejected a float+int op it should promote \
         (the KI-114 fix must not undo KI-109). Bail trace:\n{jit_err}"
    );
}
