//! A table's address-space cost tracks the keys it touches (KI-142). The dense region
//! used to be ONE 64 MB reservation per table, made on the first dense write whatever
//! the key, so a memo table holding five small ints cost 64 MB of address space; the
//! regex DFA memos (two per compiled pattern) put `regex_test` alone at 100 such regions
//! — 6.4 GB — which is what aborted the in-language suite under its `ulimit -v`. The
//! region is now a directory of lazily-mapped 512 KB chunks.
//!
//! Asserted on the entry point a program reaches — `table/put` — by reading the
//! process's own `VmSize`, so a regression in the mechanism (a region reserved per table
//! again, or a chunk that grew back to the region) fails here whatever piece of the
//! table it moved to. Linux only: `/proc/self/status` is the meter.

#[cfg(target_os = "linux")]
use brood::Interp;

#[cfg(target_os = "linux")]
fn vm_size_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("/proc/self/status");
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmSize:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse().ok())
        .expect("a VmSize line")
}

/// One hundred tables, each holding one small int key, must not reserve a hundred
/// regions: the growth is a hundred chunks (50 MB) plus the shells, well under the
/// 6.4 GB the old layout took and under a single old region per table.
#[cfg(target_os = "linux")]
#[test]
fn a_table_of_one_small_key_reserves_a_chunk_not_a_region() {
    let mut interp = Interp::new();
    interp
        .eval_str("(def tables (into [] (map (range 100) (fn (_) (table/new)))))")
        .expect("create");
    let before = vm_size_kb();
    interp
        .eval_str("(each tables (fn (t) (table/put t 5 true)))")
        .expect("put");
    let grew_mb = vm_size_kb().saturating_sub(before) / 1024;
    // 100 chunks × 512 KB = 50 MB; the old layout grew by 6400 MB here.
    assert!(
        grew_mb < 200,
        "100 one-key tables grew the address space by {grew_mb} MB — a region per table again?"
    );
    // …and the keys are all there: the chunk is a real store, not a decline to the
    // hashed side.
    let ok = interp
        .eval_str(
            "(every? tables (fn (t) (and (table/contains? t 5) (not (table/contains? t 6)))))",
        )
        .expect("contains?");
    assert_eq!(interp.print(ok), "true");
}

/// A far key maps its own chunk and nothing between: two keys 8M apart cost two chunks,
/// not the span.
#[cfg(target_os = "linux")]
#[test]
fn a_far_key_maps_its_own_chunk_only() {
    let mut interp = Interp::new();
    interp.eval_str("(def t (table/new))").expect("create");
    let before = vm_size_kb();
    interp
        .eval_str("(do (table/put t 0 1) (table/put t 8388607 2))")
        .expect("put");
    let grew_mb = vm_size_kb().saturating_sub(before) / 1024;
    assert!(grew_mb < 8, "two chunks are 1 MB; grew {grew_mb} MB");
    let got = interp
        .eval_str("[(table/get t 0) (table/get t 8388607) (table/get t 4000000 :none) (count t)]")
        .expect("get");
    assert_eq!(interp.print(got), "[1 2 :none 2]");
}

/// A table that is gone gives its registry slot — shell and reserved chunks — to the next
/// `table/new`. Before, a dropped table kept both for good, so every short-lived dense
/// table cost one 512 KB chunk of address space for the life of the runtime. Each case
/// below makes 2000 such tables, one at a time: without reuse that is ~1000 MB of growth,
/// with it one chunk.
#[cfg(target_os = "linux")]
fn growth_mb_over(interp: &mut Interp, src: &str) -> u64 {
    let before = vm_size_kb();
    let out = interp.eval_str(src).expect("run");
    assert_eq!(interp.print(out), ":ok", "{src}");
    vm_size_kb().saturating_sub(before) / 1024
}

/// The linear-map rewrite builds its loop on a private table; the wrapper now frees it.
/// Without that a plain tally exhausted a 16 GB `ulimit -v` after ~25 000 calls.
#[cfg(target_os = "linux")]
#[test]
fn a_rewritten_map_loop_reuses_its_table_across_calls() {
    let mut interp = Interp::new();
    interp
        .eval_str(
            "(defn tally (xs m) (if (empty? xs) m (tally (rest xs) (%map-int-add m (first xs) 1))))
             (def xs (range 50))
             (defn spin (n) (if (= n 0) :ok (do (tally xs {}) (spin (dec n)))))
             (spin 10)",
        )
        .expect("warm");
    let counted = interp.eval_str("(count (tally xs {}))").expect("tally");
    assert_eq!(interp.print(counted), "50");
    let grew = growth_mb_over(&mut interp, "(spin 2000)");
    assert!(
        grew < 100,
        "2000 rewritten tallies grew the address space by {grew} MB"
    );
}

/// `table/drop` of a table that never left its process reuses the slot.
#[cfg(target_os = "linux")]
#[test]
fn a_dropped_table_s_slot_is_reused() {
    let mut interp = Interp::new();
    let grew = growth_mb_over(
        &mut interp,
        "(do (defn churn (n) (if (= n 0) :ok (let (t (table/new)) (table/put t 5 true) (table/drop t) (churn (dec n)))))
             (churn 2000))",
    );
    assert!(
        grew < 100,
        "2000 created-and-dropped tables grew the address space by {grew} MB"
    );
}

/// A process's tables die with it, and their slots are reused — counted in the table
/// store's own chunks, which are its whole address-space footprint (a chunk is never
/// unmapped, and a reused slot keeps its chunks). This case once read the process's
/// `VmSize`, like the others here; across 2000 spawned and exited processes the allocator's
/// arenas grew it by 192 MB on a 4-core CI runner with every table reclaimed, and that was
/// indistinguishable from the leak the case guards.
#[cfg(target_os = "linux")]
#[test]
fn a_process_s_tables_are_reclaimed_when_it_exits() {
    let mut interp = Interp::new();
    // Warm first: the first `spawn` starts the worker pool, whose stacks and allocator
    // arenas are about a gigabyte of address space on their own.
    interp
        .eval_str(
            "(defn one-child ()
               (let (pid (spawn (let (t (table/new)) (table/put t 5 true)))
                     ref (monitor pid))
                 (receive ([:down ^ref _ _] :ok))))
             (defn children (n) (if (= n 0) :ok (do (one-child) (children (dec n)))))
             (children 10)",
        )
        .expect("warm");
    let before = brood::core::table::chunks_mapped();
    interp.eval_str("(children 2000)").expect("children");
    let mapped = brood::core::table::chunks_mapped() - before;
    // Each child's table writes one key, so each table needs one chunk; reused slots bring
    // theirs along. A few may be new while an exit's reclaim is still in flight.
    assert!(
        mapped < 16,
        "2000 exited table owners mapped {mapped} new chunks — their slots were not reused"
    );
}
