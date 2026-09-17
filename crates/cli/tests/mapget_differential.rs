//! **`PrimOp::MapGet` must answer exactly what `get` answers** (default ON since 2026-09-17;
//! `BROOD_NO_MAPGET=1` is the plain-call baseline every case below is compared against).
//!
//! `PrimOp::MapGet` gives a CHAMP map read a primitive, which vectors (`VectorRef`) and the
//! mutable table (`TableGet`) already had and maps did not. The point is not the probe itself
//! — it is that `leaf_body_qualifies` demands a **call-free** body, so while every map read
//! compiled to a call, no body that read a field could be leaf-inlined: not a `defrecord`
//! accessor, not an ability impl, not the map-shaped helpers that are most of Brood.
//!
//! The risk is correspondingly wide, because `get` is polymorphic and its map branch is, in
//! its own words, "the hottest path in the language (4796 call sites)". The prim answers a
//! present value and a **plain map's** miss (`Heap::map_get_inline`); a non-map receiver
//! and a **record's** miss must still reach the real `get`, which owns the set / string /
//! integer-index branches and `%lookup-miss` (where a record whose contents are not its
//! fields resolves through the `Lookup` ability). Each of those is a case below — the
//! `Lookup` record's miss most of all, since it is the one miss the prim must NOT answer.
//!
//! Both hot loops run past the tiering threshold on purpose. The native lowering deopts when
//! the probe declines, and a tier-2-only divergence is exactly the class that read as green
//! through an entire suite once already (ADR-294's addendum).

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

fn run(file: &PathBuf, mapget: bool, extra: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(file);
    if mapget {
        cmd.env_remove("BROOD_NO_MAPGET");
    } else {
        cmd.env("BROOD_NO_MAPGET", "1");
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

/// Every branch of `get`, plus two loops long enough to reach the native tier.
const EVERY_BRANCH: &str = "\
(defrecord cir (r))\n\
(def m {:a 1 :b nil})\n\
(def s #{:x})\n\
(def rec (cir 21))\n\
(io/puts (str \"hit \" (get m :a)))\n\
(io/puts (str \"stored-nil \" (pr-str (get m :b))))\n\
(io/puts (str \"absent \" (pr-str (get m :zz))))\n\
(io/puts (str \"default \" (get m :zz :none)))\n\
(io/puts (str \"vector \" (get [10 20 30] 1)))\n\
(io/puts (str \"string \" (get \"abc\" 1)))\n\
(io/puts (str \"set \" (pr-str (get s :x))))\n\
(io/puts (str \"nil-coll \" (pr-str (get nil :k))))\n\
(io/puts (str \"record \" (get rec :r)))\n\
(defrecord vir (seed))\n\
(impl Lookup vir (lookup-get (r k) [:virtual k (get r :seed)]))\n\
(def v (vir 7))\n\
(io/puts (str \"lookup-hit \" (get v :seed)))\n\
(io/puts (str \"lookup-miss \" (pr-str (get v :zz))))\n\
(io/puts (str \"record-miss \" (pr-str (get rec :zz))))\n\
(defn hits (n acc) (if (= n 0) acc (hits (- n 1) (+ acc (get rec :r)))))\n\
(defn misses (n acc) (if (= n 0) acc (misses (- n 1) (+ acc (if (get m :zz) 1 0)))))\n\
(defn vmisses (n acc) (if (= n 0) acc (vmisses (- n 1) (+ acc (nth (get v :zz) 2)))))\n\
(io/puts (str \"hot \" (hits 60000 0) \" \" (misses 60000 0) \" \" (vmisses 60000 0)))\n";

#[test]
fn a_map_read_primitive_answers_what_get_answers() {
    let (_dir, file) = fixture("mapget", EVERY_BRANCH);
    // The two reference arms are pinned to the native ceiling, and the loops are 60 000
    // iterations (the tiering threshold is well under that): the `differential
    // (tree-walker)` job runs the whole suite under `BROOD_VM=0`, where these arms would
    // otherwise tree-walk ~1M `get`s each on a 2-core runner — the run hit nextest's 120 s
    // cap there (2026-09-17). The tree-walker still covers every branch below, through the
    // `BROOD_TIER=0` run of the loop that follows.
    let pin = &[("BROOD_TIER", "2")];
    let off = run(&file, false, pin);
    let on = run(&file, true, pin);
    assert!(
        on.contains("hot 1260000 0 420000"),
        "the hot loops must compute correctly with the prim on:\n{on}"
    );
    assert!(
        on.contains("lookup-miss [:virtual :zz 7]"),
        "a `Lookup` record's miss must still reach `%lookup-miss`:\n{on}"
    );
    assert_eq!(off, on, "the prim changed an ANSWER");

    // Every tier, because the native lowering is a different implementation of the same
    // rule: the VM defers by returning `Ok(None)`, the native one by deopting.
    for tier in ["0", "1", "2"] {
        let tiered = run(&file, true, &[("BROOD_TIER", tier)]);
        assert_eq!(
            off, tiered,
            "BROOD_TIER={tier} with the prim on disagrees with the default build"
        );
    }
}

#[test]
fn the_prim_is_what_makes_a_field_reading_body_inlinable() {
    // Non-vacuity, and the reason the prim exists. A body that reads a field is rejected by
    // `leaf_body_qualifies` while the read is a call; with the prim it is call-free and the
    // leaf inliner derives a splice. Asserted through `BROOD_INLINE_DBG`, which names the
    // arm a derivation was built for — so this fails if the prim stops being reachable from
    // `get`, which a differential on answers alone could never notice.
    let source = "\
(defrecord cir (r))\n\
(def rec (cir 21))\n\
(defn body (x) (* (get x :r) 2))\n\
(defn hot (n acc) (if (= n 0) acc (hot (- n 1) (+ acc (body rec)))))\n\
(io/puts (str (hot 300000 0)))\n";
    let (_dir, file) = fixture("mapget-inline", source);
    // Pin the tier ceiling to Native. This asserts a JIT derivation happened, and the
    // `differential (tree-walker)` CI job runs the whole suite under `BROOD_VM=0` — where
    // nothing ever lowers, so `[leaf …]` cannot print and the assertion fails for a reason
    // that has nothing to do with the prim (KI-69's class, and its fix: a JIT-asserting
    // test names the ceiling it needs rather than inheriting the job's). `BROOD_TIER` wins
    // over the job's `BROOD_VM`/`BROOD_NO_JIT` by the ADR-222 precedence, so this test
    // checks the same thing on every job.
    let env = &[("BROOD_INLINE_DBG", "1"), ("BROOD_TIER", "2")];
    let off = run(&file, false, env);
    let on = run(&file, true, env);
    // The marker is the DERIVATION line (`leaf probe hot sites=…`). Since 2026-09-12 the
    // trace also names why a probe declines (`leaf probe hot declined: …`), so a bare
    // `leaf probe hot ` matches both and the "without the prim" half of this test read as
    // failing on every binary, baseline included.
    assert!(
        !off.contains("leaf probe hot sites="),
        "without the prim a field-reading body must NOT be inlinable — if this starts \
         passing, the premise of the whole change has changed:\n{off}"
    );
    assert!(
        on.contains("leaf probe hot sites="),
        "with the prim the field-reading body must become leaf-inlinable:\n{on}"
    );
}
