//! **`PrimOp3::MapGet3` and `PrimOp3::MapAssoc` must answer exactly what `get` and `assoc`
//! answer** (ADR-367; `BROOD_NO_MAPGET=1` / `BROOD_NO_MAPASSOC=1` are the plain-call baselines
//! every case below is compared against).
//!
//! The 3-arity `(get m k default)` is every record read-with-default in the language, and
//! `(assoc m k v)` every record update; both compiled to a full call where the 2-arity read had
//! been a primitive since ADR-296. The inline rules are deliberately narrow — `get`'s answers a
//! present non-nil value, or the default when absent, and DECLINES a record's nil result so
//! `%lookup-miss` (the `Lookup` ability) still owns it; `assoc`'s answers a map receiver only,
//! so a vector, a record and every type error stay in Brood. Each of those is a case below,
//! the `Lookup` record's nil-default miss most of all: it is the one result the prim must NOT
//! answer, and the one a default-hiding bug would change silently.
//!
//! Both hot loops run past the tiering threshold on purpose: the native lowering is a second
//! implementation of the same rule (it deopts where the VM returns `None`), and `assoc`'s
//! callback ALLOCATES from native code, which is the class a differential on answers alone
//! catches only if the loop actually tiers.

use std::path::PathBuf;
use std::process::Command;

mod support;

struct TempDir {
    path: PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn fixture(tag: &str, source: &str) -> (TempDir, PathBuf) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create temp dir");
    let file = path.join("probe.blsp");
    std::fs::write(&file, source).expect("write fixture");
    (TempDir { path }, file)
}

fn run(file: &PathBuf, prims: bool, extra: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(file).env("BROOD_NO_CHECK", "1");
    if prims {
        cmd.env_remove("BROOD_NO_MAPGET");
        cmd.env_remove("BROOD_NO_MAPASSOC");
    } else {
        cmd.env("BROOD_NO_MAPGET", "1");
        cmd.env("BROOD_NO_MAPASSOC", "1");
    }
    for (k, v) in extra {
        cmd.env(k, v);
    }
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Every branch of the 3-arity `get` and of the single-pair `assoc`, plus two loops long
/// enough to reach the native tier.
const EVERY_BRANCH: &str = "\
(defrecord cir (r))\n\
(def m {:a 1 :b nil})\n\
(def s #{:x})\n\
(def rec (cir 21))\n\
(io/puts (str \"hit \" (get m :a :d)))\n\
(io/puts (str \"stored-nil \" (pr-str (get m :b :d))))\n\
(io/puts (str \"absent \" (get m :zz :d)))\n\
(io/puts (str \"absent-nil-default \" (pr-str (get m :zz nil))))\n\
(io/puts (str \"vector \" (get [10 20 30] 1 :d) \" \" (get [10 20 30] 9 :d)))\n\
(io/puts (str \"string \" (get \"abc\" 1 :d) \" \" (get \"abc\" 9 :d)))\n\
(io/puts (str \"set \" (pr-str (get s :x :d)) \" \" (pr-str (get s :y :d))))\n\
(io/puts (str \"nil-coll \" (pr-str (get nil :k :d))))\n\
(io/puts (str \"record \" (get rec :r :d) \" \" (get rec :zz :d)))\n\
(io/puts (str \"record-nil-default \" (pr-str (get rec :zz nil))))\n\
(defrecord vir (seed))\n\
(impl Lookup vir (lookup-get (r k) [:virtual k (get r :seed)]))\n\
(def v (vir 7))\n\
(io/puts (str \"lookup-hit \" (get v :seed :d)))\n\
(io/puts (str \"lookup-miss-default \" (pr-str (get v :zz :d))))\n\
(io/puts (str \"lookup-miss-nil \" (pr-str (get v :zz nil))))\n\
(io/puts (str \"assoc-map \" (pr-str (assoc m :c 3)) \" \" (pr-str (assoc m :a 9)) \" \" (pr-str (assoc {} :k nil))))\n\
(io/puts (str \"assoc-vector \" (pr-str (assoc [10 20 30] 1 99))))\n\
(io/puts (str \"assoc-record \" (get (assoc rec :r 5) :r 0) \" \" (record? (assoc rec :r 5))))\n\
(io/puts (str \"assoc-vector-oob \" (try (assoc [1 2] 5 0) (catch e :error))))\n\
(io/puts (str \"assoc-non-coll \" (try (assoc 42 :k 1) (catch e :error))))\n\
(defn reads (n acc) (if (= n 0) acc (reads (- n 1) (+ acc (get rec :r 0) (get m :zz 2)))))\n\
(defn builds (n st) (if (= n 0) (get st :n 0) (builds (- n 1) (assoc st :n n))))\n\
(defn vmisses (n acc) (if (= n 0) acc (vmisses (- n 1) (+ acc (nth (get v :zz nil) 2)))))\n\
(io/puts (str \"hot \" (reads 60000 0) \" \" (builds 60000 {:n 0 :other 1}) \" \" (vmisses 60000 0)))\n";

#[test]
fn the_map_prim3_ops_answer_what_get_and_assoc_answer() {
    let (_dir, file) = fixture("mapprim3", EVERY_BRANCH);
    // The reference arms are pinned to the native ceiling: the `differential (tree-walker)`
    // job runs the whole suite under `BROOD_VM=0`, where the 60 000-iteration loops would
    // tree-walk for minutes on a 2-core runner; the tree-walker still covers every branch
    // through the `BROOD_TIER=0` run below.
    let pin = &[("BROOD_TIER", "2")];
    let off = run(&file, false, pin);
    let on = run(&file, true, pin);
    assert!(
        on.contains("hot 1380000 1 420000"),
        "the hot loops must compute correctly with the prims on:\n{on}"
    );
    assert!(
        on.contains("lookup-miss-nil [:virtual :zz 7]"),
        "a `Lookup` record's nil-default miss must still reach `%lookup-miss`:\n{on}"
    );
    assert!(
        on.contains("lookup-miss-default :d"),
        "a non-nil default is answered without consulting `Lookup`, as `get` does:\n{on}"
    );
    assert_eq!(off, on, "a prim changed an ANSWER");

    // Every tier, because the native lowering is a different implementation of the same
    // rule: the VM defers by leaving `done` empty, the native one by deopting.
    for tier in ["0", "1", "2"] {
        let tiered = run(&file, true, &[("BROOD_TIER", tier)]);
        assert_eq!(
            off, tiered,
            "BROOD_TIER={tier} with the prims on disagrees with the plain-call build"
        );
    }
}

#[test]
fn the_hot_arms_stay_native_with_the_prims_on() {
    // Non-vacuity for the native half: an arm whose `(get m k d)` / `(assoc m k v)` lowers to
    // a callback that declines on every activation would deopt-thrash and latch BAILED — the
    // KI-132 class, right answers slowly, that no differential on answers sees. Ask the arm.
    //
    // `m` is a `def`, not a literal in the loop body: a `MakeMap` in an arm puts the whole
    // arm outside the JIT subset today, which would bail `reads` for a reason that has
    // nothing to do with the prims (found writing this test).
    let source = "\
(defrecord cir (r))\n\
(def rec (cir 21))\n\
(def m {:a 1})\n\
(defn reads (n acc) (if (= n 0) acc (reads (- n 1) (+ acc (get rec :r 0) (get m :zz 2)))))\n\
(defn builds (n st) (if (= n 0) (get st :n 0) (builds (- n 1) (assoc st :n n))))\n\
(defn settle (f argc batch rounds) (batch) (let (st (get (%jit-arm-state f argc) :state)) (if (or (= st :native) (= st :bailed) (= rounds 0)) st (settle f argc batch (- rounds 1)))))\n\
(io/puts (str (reads 60000 0) \" \" (builds 60000 {:n 0})))\n\
(io/puts (str (settle reads 2 (fn () (reads 20000 0)) 40) \" \" (settle builds 2 (fn () (builds 20000 {:n 0})) 40)))\n";
    let (_dir, file) = fixture("mapprim3-native", source);
    let on = run(&file, true, &[("BROOD_TIER", "2")]);
    // `%jit-arm-state` is dev-tools only; the lean sibling `brood` beside a release `nest`
    // has no such name and this half asserts nothing there.
    if on.contains("unbound symbol") {
        return;
    }
    assert!(
        on.contains("1380000 1\n:native :native"),
        "both arms must compute correctly AND settle native with the prims on:\n{on}"
    );
}
