//! **Numeric lists cross a WASM component call by memory copy, and answer exactly what the
//! general path answers** (ADR-806).
//!
//! The same program runs with `BROOD_NO_WASM_FAST_LISTS=1` (a `Val` per element, the general
//! path) and without: every answer and every error message must match line for line, and the
//! copy must be faster. A test of answers alone cannot tell the two paths apart, which is the
//! point of the fast path; the timing line can. At 5 600 floats a call the copy measured 13x
//! faster (439 -> 32 us), so the 3x asserted here leaves a loaded CI runner a wide margin.
//! `tests/wasm_lists_test.blsp` holds the same kernels' cases in the suite. The component's
//! allocator wraps around, so the loop never runs out of linear memory.

use std::path::PathBuf;
use std::process::Command;

mod support;
use support::temp_dir;

const PROGRAM: &str = r#"(def wat "(component
     (core module $m
       (memory (export \"mem\") 4)
       (global $next (mut i32) (i32.const 1024))
       (func $alloc (export \"realloc\") (param i32 i32 i32 i32) (result i32)
         (local $p i32)
         ;; align up to 8
         (if (i32.gt_u (i32.add (global.get $next) (local.get 3)) (i32.const 250000))
           (then (global.set $next (i32.const 1024))))
         (local.set $p (i32.and (i32.add (global.get $next) (i32.const 7)) (i32.const -8)))
         (global.set $next (i32.add (local.get $p) (local.get 3)))
         (local.get $p))
       (func $ret (param $ptr i32) (param $len i32) (result i32)
         (i32.store (i32.const 16) (local.get $ptr))
         (i32.store (i32.const 20) (local.get $len))
         (i32.const 16))
       ;; twice(xs: list<float64>) -> list<float64>
       (func (export \"twice\") (param $xs i32) (param $n i32) (result i32)
         (local $out i32) (local $i i32)
         (local.set $out (call $alloc (i32.const 0) (i32.const 0) (i32.const 8)
                           (i32.mul (local.get $n) (i32.const 8))))
         (block $done
           (loop $loop
             (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
             (f64.store
               (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 8)))
               (f64.mul (f64.const 2)
                 (f64.load (i32.add (local.get $xs) (i32.mul (local.get $i) (i32.const 8))))))
             (local.set $i (i32.add (local.get $i) (i32.const 1)))
             (br $loop)))
         (call $ret (local.get $out) (local.get $n)))
       ;; gather(xs: list<float64>, at: list<s32>) -> list<float64>: out[i] = xs[at[i]]
       (func (export \"gather\") (param $xs i32) (param $xn i32) (param $at i32) (param $n i32)
         (result i32)
         (local $out i32) (local $i i32)
         (local.set $out (call $alloc (i32.const 0) (i32.const 0) (i32.const 8)
                           (i32.mul (local.get $n) (i32.const 8))))
         (block $done
           (loop $loop
             (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
             (f64.store
               (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 8)))
               (f64.load (i32.add (local.get $xs)
                 (i32.mul (i32.load (i32.add (local.get $at) (i32.mul (local.get $i) (i32.const 4))))
                          (i32.const 8)))))
             (local.set $i (i32.add (local.get $i) (i32.const 1)))
             (br $loop)))
         (call $ret (local.get $out) (local.get $n)))
       ;; halve(xs: list<float32>) -> list<float32>
       (func (export \"halve\") (param $xs i32) (param $n i32) (result i32)
         (local $out i32) (local $i i32)
         (local.set $out (call $alloc (i32.const 0) (i32.const 0) (i32.const 4)
                           (i32.mul (local.get $n) (i32.const 4))))
         (block $done
           (loop $loop
             (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
             (f32.store
               (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 4)))
               (f32.mul (f32.const 0.5)
                 (f32.load (i32.add (local.get $xs) (i32.mul (local.get $i) (i32.const 4))))))
             (local.set $i (i32.add (local.get $i) (i32.const 1)))
             (br $loop)))
         (call $ret (local.get $out) (local.get $n)))
       ;; bump(xs: list<u32>) -> list<u32>: each plus one (wrapping, as u32 does)
       (func (export \"bump\") (param $xs i32) (param $n i32) (result i32)
         (local $out i32) (local $i i32)
         (local.set $out (call $alloc (i32.const 0) (i32.const 0) (i32.const 4)
                           (i32.mul (local.get $n) (i32.const 4))))
         (block $done
           (loop $loop
             (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
             (i32.store
               (i32.add (local.get $out) (i32.mul (local.get $i) (i32.const 4)))
               (i32.add (i32.const 1)
                 (i32.load (i32.add (local.get $xs) (i32.mul (local.get $i) (i32.const 4))))))
             (local.set $i (i32.add (local.get $i) (i32.const 1)))
             (br $loop)))
         (call $ret (local.get $out) (local.get $n)))
       ;; touch(xs: list<float64>): no result
       (func (export \"touch\") (param i32 i32)))
     (core instance $i (instantiate $m))
     (func $twice (param \"xs\" (list float64)) (result (list float64))
       (canon lift (core func $i \"twice\") (memory (core memory $i \"mem\"))
         (realloc (core func $i \"realloc\"))))
     (func $gather (param \"xs\" (list float64)) (param \"at\" (list s32)) (result (list float64))
       (canon lift (core func $i \"gather\") (memory (core memory $i \"mem\"))
         (realloc (core func $i \"realloc\"))))
     (func $halve (param \"xs\" (list float32)) (result (list float32))
       (canon lift (core func $i \"halve\") (memory (core memory $i \"mem\"))
         (realloc (core func $i \"realloc\"))))
     (func $bump (param \"xs\" (list u32)) (result (list u32))
       (canon lift (core func $i \"bump\") (memory (core memory $i \"mem\"))
         (realloc (core func $i \"realloc\"))))
     (func $touch (param \"xs\" (list float64))
       (canon lift (core func $i \"touch\") (memory (core memory $i \"mem\"))
         (realloc (core func $i \"realloc\"))))
     (export \"twice\" (func $twice))
     (export \"gather\" (func $gather))
     (export \"halve\" (func $halve))
     (export \"bump\" (func $bump))
     (export \"touch\" (func $touch)))")

(def inst (wasm/instantiate wat))
(defn shown (thunk) (try (pr-str (thunk)) (catch e (str "error: " (error-message e)))))
(doseq (line [(shown (fn () (wasm/call inst "twice" [1.0 -1.5 0.25])))
              (shown (fn () (wasm/call inst "twice" (list 1 2.0))))
              (shown (fn () (wasm/call inst "twice" [])))
              (shown (fn () (wasm/call inst "twice" [[1.0 2.0] [3.0] (list 4 5.0)])))
              (shown (fn () (wasm/call inst "twice" [[[1.0]]])))
              (shown (fn () (wasm/call inst "gather" [10.0 20.0 30.0] [2 0 2])))
              (shown (fn () (wasm/call inst "halve" [1.0 -4.0])))
              (shown (fn () (wasm/call inst "bump" [0 7 4294967295])))
              (shown (fn () (wasm/call inst "touch" [1.0 2.0])))
              (shown (fn () (wasm/call inst "gather" [1.0] [5000000000])))
              (shown (fn () (wasm/call inst "bump" [-1])))
              (shown (fn () (wasm/call inst "twice" [1.0 :x])))
              (shown (fn () (wasm/call inst "twice" 3)))])
  (io/puts (str "answer " line)))
(def xs (into [] (map (range 5600) (fn (i) (* 0.5 i)))))
(defn loop-calls (n) (when (> n 0) (wasm/call inst "twice" xs) (loop-calls (- n 1))))
(loop-calls 20)
(def t0 (os/now-ns))
(loop-calls 300)
(io/puts (str "ns " (- (os/now-ns) t0)))
"#;

fn run(file: &PathBuf, fast: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(file).env("BROOD_NO_CHECK", "1");
    if fast {
        cmd.env_remove("BROOD_NO_WASM_FAST_LISTS");
    } else {
        cmd.env("BROOD_NO_WASM_FAST_LISTS", "1");
    }
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn answers(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter(|line| line.starts_with("answer "))
        .collect()
}

fn nanos(output: &str) -> u64 {
    output
        .lines()
        .find_map(|line| line.strip_prefix("ns "))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no timing line:\n{output}"))
}

#[test]
fn numeric_lists_copy_and_answer_as_the_general_path_does() {
    let dir = temp_dir("wasm-fast-lists");
    let file = dir.path.join("lists.blsp");
    std::fs::write(&file, PROGRAM).expect("write fixture");
    let general = run(&file, false);
    let fast = run(&file, true);
    assert_eq!(
        answers(&general).len(),
        13,
        "general path did not finish:\n{general}"
    );
    assert_eq!(
        answers(&general),
        answers(&fast),
        "the copy changed an answer or an error\n--- general ---\n{general}\n--- copy ---\n{fast}"
    );
    let (slow, quick) = (nanos(&general), nanos(&fast));
    assert!(
        quick * 3 < slow,
        "the copy is not faster: {quick} ns against the general path's {slow} ns for 300 calls \
         of 5 600 floats — is the fast path taken?"
    );
}
