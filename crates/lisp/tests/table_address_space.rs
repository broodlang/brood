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
        .eval_str("(every? tables (fn (t) (and (table/has? t 5) (not (table/has? t 6)))))")
        .expect("has?");
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
        .eval_str(
            "[(table/get t 0) (table/get t 8388607) (table/get t 4000000 :none) (table/count t)]",
        )
        .expect("get");
    assert_eq!(interp.print(got), "[1 2 :none 2]");
}
