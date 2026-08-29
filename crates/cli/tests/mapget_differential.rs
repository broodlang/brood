//! **`BROOD_MAPGET=1` must answer exactly what `get` answers.**
//!
//! `PrimOp::MapGet` gives a CHAMP map read a primitive, which vectors (`VectorRef`) and the
//! mutable table (`TableGet`) already had and maps did not. The point is not the probe itself
//! — it is that `leaf_body_qualifies` demands a **call-free** body, so while every map read
//! compiled to a call, no body that read a field could be leaf-inlined: not a `defrecord`
//! accessor, not an ability impl, not the map-shaped helpers that are most of Brood.
//!
//! The risk is correspondingly wide, because `get` is polymorphic and its map branch is, in
//! its own words, "the hottest path in the language (4796 call sites)". The prim inlines
//! **only a present, non-nil value**; a non-map receiver, an absent key and a stored `nil`
//! must all still reach the real `get`, which owns the set / string / integer-index branches
//! and `%lookup-miss` (where a record whose contents are not its fields resolves through the
//! `Lookup` ability). Each of those is a case below.
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
        cmd.env("BROOD_MAPGET", "1");
    } else {
        cmd.env_remove("BROOD_MAPGET");
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
(defn hits (n acc) (if (= n 0) acc (hits (- n 1) (+ acc (get rec :r)))))\n\
(defn misses (n acc) (if (= n 0) acc (misses (- n 1) (+ acc (if (get m :zz) 1 0)))))\n\
(io/puts (str \"hot \" (hits 300000 0) \" \" (misses 300000 0)))\n";

#[test]
fn a_map_read_primitive_answers_what_get_answers() {
    let (_dir, file) = fixture("mapget", EVERY_BRANCH);
    let off = run(&file, false, &[]);
    let on = run(&file, true, &[]);
    assert!(
        on.contains("hot 6300000 0"),
        "the hot loops must compute correctly with the prim on:\n{on}"
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
    let off = run(&file, false, &[("BROOD_INLINE_DBG", "1")]);
    let on = run(&file, true, &[("BROOD_INLINE_DBG", "1")]);
    assert!(
        !off.contains("leaf probe hot "),
        "without the prim a field-reading body must NOT be inlinable — if this starts \
         passing, the premise of the whole change has changed:\n{off}"
    );
    assert!(
        on.contains("leaf probe hot "),
        "with the prim the field-reading body must become leaf-inlinable:\n{on}"
    );
}
