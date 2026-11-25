//! **A function holding a `(fn …)` literal lowers natively, and answers exactly what it
//! answers on the VM** (ADR-803).
//!
//! The literal is built by `brood_rt_make_closure`, which runs the VM's own `MakeClosure` arm
//! on captures the native code stages on `roots`. Each shape below is one way that can go
//! wrong: a handle live on the operand stack BELOW the literal (it must be spilled across the
//! callback), a `letrec` self-name (the closure binds itself into the env it captures), a
//! capture-free literal (its eighth sighting promotes it to RUNTIME, the one route from this
//! callback to a collection), a literal returned as the arm's value, and the same functions run
//! from many processes at once. Each also branches or builds a collection: an arm of only
//! loads, calls and the literal is kept out on purpose, and `plumbing` checks that it is.
//!
//! Two runs of the same program: `BROOD_NO_JIT_CLOSURES=1` (the baseline — every one of these
//! arms is refused by the subset, which the test asserts, so the lever is what changes the
//! tier) and the default (every arm must settle `:native`). The answers must match line for line.

use std::path::PathBuf;
use std::process::Command;

mod support;
use support::{TempDir, temp_dir};

fn fixture(source: &str) -> (TempDir, PathBuf) {
    let dir = temp_dir("jit-closure-literal");
    let file = dir.path.join("probe.blsp");
    std::fs::write(&file, source).expect("write fixture");
    (dir, file)
}

fn run(file: &PathBuf, closures: bool) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(file)
        .env("BROOD_NO_CHECK", "1")
        // The tier this test is about; the differential job's `BROOD_TIER=0` would leave
        // nothing to compare.
        .env_remove("BROOD_TIER");
    if closures {
        cmd.env_remove("BROOD_NO_JIT_CLOSURES");
    } else {
        cmd.env("BROOD_NO_JIT_CLOSURES", "1");
    }
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const PROGRAM: &str = r#"
;; A handle (the vector's first element, read as a handle) sits under the literal.
(defn pair-up (xs k) [(first xs) (map xs (fn (x) (conj x k)))])
;; A letrec self-name: the closure is bound into its own captured env, and reads itself
;; there as a VALUE (a direct self-call would compile to a loop and never look).
(defn count-down (n)
  (if (< n 0)
    0
    (letrec (go (fn (i acc) (if (= i 0) acc (apply go [(- i 1) (+ acc i)])))) (go n 0))))
;; Capture-free: built thousands of times, so it is promoted from native code.
(defn doubled (xs) (if (empty? xs) [] (map xs (fn (x) (* x 2)))))
;; The literal is the arm's value, called after the arm has returned.
(defn adder (n label) (if (= n 0) nil (fn (x) [label (+ x n)])))
;; Captures of several kinds — a float, a map, a string — read back by the closure.
(defn tagged (xs scale meta name)
  (if (empty? xs) [] (fold xs [] (fn (acc x) (conj acc [name (get meta :k) (* scale x)])))))
;; Only call plumbing: native code would run the VM's own callbacks, so the literal does
;; not bring it into the subset.
(defn plumbing (xs k) (map xs (fn (x) (+ x k))))

(def xs [(list 1 2) (list 3 4) (list 5 6)])
(def nums [1 2 3 4 5])

(defn round (i)
  [(pair-up xs i)
   (count-down (math/rem i 50))
   (doubled nums)
   ((adder i :a) 10)
   (tagged nums 0.5 {:k i} "t")
   (plumbing nums i)])

(defn drive (i acc) (if (= i 0) acc (drive (- i 1) (round i))))
(def last-round (drive 3000 nil))
(io/puts (str "answer " (pr-str last-round)))
(io/puts (str "answer " (pr-str (round 7))))

;; The same functions from sixteen processes at once, each hot in its own right.
(def me (self))
(defn worker (parent seed) (send parent [seed (do (drive 500 nil) (round (+ 7 seed)))]))
(doseq (seed (range 16)) (spawn (worker me seed)))
(defn gather (got acc) (if (= got 16) acc (gather (+ got 1) (receive ([seed r] (assoc acc seed r))))))
(def gathered (gather 0 {}))
(io/puts (str "answer workers " (pr-str (map (range 16) (fn (seed) (get gathered seed))))))

(io/puts (str "native-tier " (%native-tier?)))
(defn state-of (f argc) (let (s (dev/jit-state f argc)) [(get s :state) (get s :reason)]))
(io/puts (str "state pair-up " (pr-str (state-of pair-up 2))))
(io/puts (str "state count-down " (pr-str (state-of count-down 1))))
(io/puts (str "state doubled " (pr-str (state-of doubled 1))))
(io/puts (str "state adder " (pr-str (state-of adder 2))))
(io/puts (str "state tagged " (pr-str (state-of tagged 4))))
(io/puts (str "state plumbing " (pr-str (state-of plumbing 2))))
"#;

const ARMS: [&str; 5] = ["pair-up", "count-down", "doubled", "adder", "tagged"];

fn lines_with<'a>(output: &'a str, prefix: &str) -> Vec<&'a str> {
    output.lines().filter(|l| l.starts_with(prefix)).collect()
}

#[test]
fn closure_literals_lower_natively_and_answer_as_the_vm_does() {
    let (_dir, file) = fixture(PROGRAM);
    let off = run(&file, false);
    let on = run(&file, true);

    // The summary lines are PRESENT (a dead runner prints none, and "none differ" would pass).
    let off_answers = lines_with(&off, "answer ");
    let on_answers = lines_with(&on, "answer ");
    assert_eq!(off_answers.len(), 3, "baseline run did not finish:\n{off}");
    assert_eq!(
        on_answers.len(),
        3,
        "default (closures on) run did not finish:\n{on}"
    );
    assert_eq!(
        off_answers, on_answers,
        "a natively built closure changed an ANSWER\n--- off ---\n{off}\n--- on ---\n{on}"
    );

    if !on.contains("native-tier true") {
        return; // no native tier in this binary: the answers are all there is to compare
    }
    for arm in ARMS {
        let off_state = lines_with(&off, &format!("state {arm} "));
        let on_state = lines_with(&on, &format!("state {arm} "));
        assert_eq!(
            off_state,
            [format!("state {arm} [:bailed \"chunk-outside-jit-subset\"]").as_str()],
            "BROOD_NO_JIT_CLOSURES=1: `{arm}` should be refused by the subset — if it lowers \
             anyway, this test no longer shows the literal is what kept it out\n{off}"
        );
        assert_eq!(
            on_state,
            [format!("state {arm} [:native nil]").as_str()],
            "default: `{arm}` did not settle native\n{on}"
        );
    }
    // A chunk of loads, calls and the literal stays out by default too: native code for it
    // costs a compile and gains nothing (matmul's setup, +2.5% instructions).
    let refused = ["state plumbing [:bailed \"chunk-outside-jit-subset\"]"];
    assert_eq!(lines_with(&off, "state plumbing "), refused, "{off}");
    assert_eq!(
        lines_with(&on, "state plumbing "),
        refused,
        "default: a pure call-plumbing arm was admitted\n{on}"
    );
}
