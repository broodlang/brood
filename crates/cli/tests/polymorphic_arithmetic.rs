//! **Arithmetic whose operand is sometimes an int and sometimes a float stays native.**
//!
//! Two shapes deopted for good before ADR-780:
//!
//! - `walk`, the shape of `b2d-scenery/block-walk` and dino's `hanging-lianas`: a self-tail
//!   loop computing `(* (count cards) (roll i))` — an operand that can never be a float
//!   beside a call's result. The float-context re-tier (ADR-378) could not help: the float
//!   path refuses the never-float `count`, so the multiply stayed on the integer path and
//!   deopted (`handle-int-guard(tag=4)`) on every activation, and a self-tail loop is never
//!   deopt-watched, so nothing ever latched or re-tiered it. `weight` makes the erased
//!   operand an int on even steps, so the dispatched multiply runs both of its branches.
//! - `magnitude`, the shape of `math/abs` with one int caller among float callers: each
//!   static guess deopts on the other kind of number, and sixteen in a row latched the arm
//!   BAILED for every caller. Deopt feedback now re-tiers it with that arithmetic dispatched
//!   by tag (`[jit-relower] … reason=polymorphic-arithmetic`), after which neither kind
//!   deopts.
//! - `walk-carry`: a self-tail loop whose `acc` the profile typed `Int` (register-carried),
//!   then handed a float through a let-bound `nth`. The back edge copied the payload into the
//!   int register unchecked, so `(< acc 100)` compared a float's bit pattern and answered
//!   `false` for 3.5. `walk`'s `acc` (int 0, then a float sum) rides the same edge. The back
//!   edge now tag-checks a boxed arg for a carried slot (`self-call-carry-tag`), and sixteen
//!   of those box the slot like an entry-tag deopt.
//!
//! Asserted on the real `brood` entry point: the JIT's output matches the VM's
//! (`BROOD_TIER=1`) — ints stay ints — and `BROOD_JIT_BAIL_TRACE=1` shows each arm settling:
//! a handful of deopts while the feedback learns, not one per activation.

use std::process::Command;

const PROGRAM: &str = r#"
(defn roll (key seed)
  (let (value (* (math/sin (+ (* key 12.9898) seed)) 43758.5453)) (- value (math/floor value))))
(defn weight (i) (if (= 0 (math/rem i 2)) 2 (roll i 1.0)))
(defn walk (cards i end acc)
  (if (>= i end)
    acc
    (let (pick (math/floor (* (count cards) (roll i 1.0)))
          scaled (* (count cards) (weight i)))
      (walk cards (+ i 1) end (+ acc (nth cards pick) scaled)))))
(defn magnitude (n) (if (< n 0) (- n) n))
(defn float-sum (i acc)
  (if (>= i 200) acc (float-sum (+ i 1) (+ acc (magnitude (- 0.5 (* 0.01 i)))))))
(defn frames (frame total acc)
  (if (>= frame total)
    acc
    (frames (+ frame 1)
      total
      (+ acc (walk [1 2 3 4 5] 0 40 0) (magnitude -3) (float-sum 0 0.0)))))
(defn walk-carry (i acc values)
  (if (>= i 3) (< acc 100) (let (next (nth values i)) (walk-carry (+ i 1) next values))))
(defn warm (n) (if (>= n 2000) nil (do (walk-carry 0 0 [1 2 3]) (warm (+ n 1)))))
(warm 0)
(io/puts (walk-carry 0 0 [1.5 2.5 3.5]))
(io/puts (frames 0 400 0))
(io/puts (magnitude -3))
(io/puts (magnitude -2.5))
"#;

fn run(no_jit: bool) -> (String, String) {
    let path = std::env::temp_dir().join(format!(
        "brood-polymorphic-arithmetic-{}-{}.blsp",
        std::process::id(),
        u8::from(no_jit)
    ));
    std::fs::write(&path, PROGRAM).expect("write program");
    let mut command = Command::new(env!("CARGO_BIN_EXE_brood"));
    command
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_JIT_BAIL_TRACE", "1")
        .env("BROOD_TIER", if no_jit { "1" } else { "2" });
    let output = command.arg(&path).output().expect("run brood");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy_owned(output.stdout),
        String::from_utf8_lossy_owned(output.stderr),
    )
}

fn deopts_of(trace: &str, arm: &str) -> usize {
    let needle = format!("[jit-deopt] arm={arm} ");
    trace
        .lines()
        .filter(|line| line.starts_with(&needle))
        .count()
}

#[test]
fn polymorphic_arithmetic_settles_native_and_matches_the_vm() {
    let (jit_out, jit_err) = run(false);
    let (vm_out, _) = run(true);
    assert!(
        jit_out.lines().count() == 4 && vm_out.lines().count() == 4,
        "expected four output lines from each tier.\njit:\n{jit_out}\nvm:\n{vm_out}"
    );
    assert_eq!(jit_out, vm_out, "the JIT and the VM disagree");
    assert_eq!(
        jit_out.lines().nth(2),
        Some("3"),
        "an int's magnitude must stay an int:\n{jit_out}"
    );
    // A few deopts while the feedback learns (four to float context, four more to
    // dispatch, sixteen back-edge carry deopts before `walk`'s `acc` is boxed); before the
    // fix `walk` deopted on every activation that ran native (167 of 400) and `magnitude`
    // latched BAILED.
    for arm in ["walk", "magnitude"] {
        let deopts = deopts_of(&jit_err, arm);
        assert!(
            deopts <= 40,
            "`{arm}` deopted {deopts} times: it never settled. Bail trace:\n{jit_err}"
        );
        assert!(
            !jit_err.contains(&format!("arm={arm} reason=deopt-thrash-latched")),
            "`{arm}` deopt-thrashed to BAILED. Bail trace:\n{jit_err}"
        );
    }
    assert!(
        jit_err.contains("[jit-relower] arm=magnitude reason=polymorphic-arithmetic"),
        "`magnitude` was never re-tiered with dispatched arithmetic. Bail trace:\n{jit_err}"
    );
}
