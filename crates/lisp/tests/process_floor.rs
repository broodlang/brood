//! **The per-process memory floor is a ratchet.** Counted bytes (the counting allocator,
//! `%mem-bytes`) per PARKED process — the state every one of N live processes is in at
//! once, so the state that sets peak memory — must not exceed the figure below.
//!
//! It exists because the floor regressed by 328 B per process without anything noticing:
//! `spawn` propagated the (empty) package context to every child through `cold_mut()`,
//! which allocated the `ColdHeap` that M1 (`runtime-frontier.md` §B) had moved off the
//! worker floor — for two months, on every process ever spawned. Found 2026-09-21 by an
//! allocation histogram, not by any gate. This is that gate: it reads the per-process
//! figure and refuses a step up. Move the bound DOWN when a shaving lands; move it up
//! only with a sentence saying what the bytes buy.
//!
//! Measured in the `test` profile (opt-level 2, debug assertions): 4 439 B/proc before the
//! ColdHeap fix, 4 120 after it, 3 824 after the per-process maps became `SmallMap`s
//! (release: 4 690 / 4 345 / 3 957). The bound sits ~4% above the measured figure, which
//! is the only way a 328 B step reads as a step: at 4 600 the sabotage passed.
//! Sabotage-verified: restoring the unconditional `cold_mut()` read 4 439 against a 4 300
//! bound and reds this.
//!
//! Raised 4 000 → 4 020 (2026-10-14): the computed-call memo (ADR-406) costs 16 B per process
//! — one nullable pointer, the table itself allocated only by a process that calls a closure
//! value from native code — and buys ~240 instructions on every such call. Measured 3 976–3 985
//! before it, 3 995–3 996 after, which left 4 B under the old bound against ~9 B of run-to-run
//! jitter.
//!
//! Raised 4 020 → 4 040 (2026-10-14): KI-246's hash memo (`Heap::hash_memo`, a lazily boxed
//! `OnceCell`) is a second 8 B pointer, and it buys nested-set hashing in linear time.
//! Measured 4 020–4 021 with both memos, which left no headroom against the jitter above.
//!
//! Lowered 4 040 → 3 040 (2026-10-18), and the probe changed from growth-over-one-fleet ÷ N
//! to the slope between a warm-up fleet and a measured one (the comment in the test says
//! why): the old figure moved ~200 B between two builds of the same tree, which is more than
//! most shavings. On the slope, HEAD read 3 195–3 207 and the change 3 051–3 055: three
//! per-process caches went from a `HashMap` to a `SmallMap` (the closure-template,
//! const-closure and ability-dispatch memos), `SmallMap`'s spill is boxed (32 → 24 B each),
//! and the package-rooting and contract alias memos left the inline `Heap` for lazily
//! allocated homes (3 051–3 055). Then the mailbox's dead-alias set (72 B inline) and its
//! pending exit reason (a 32 B `Message`) were boxed, both being empty on almost every
//! process: 2 978–2 979. Jitter is ±4 B. Sabotage: the tree without the mailbox half
//! (3 053) reds this bound, and the HEAD build (3 195) reds it further.
//!
//! Lowered 3 040 → 2 940 (2026-10-18, later): the bigint, decimal, ratio, rope and native
//! slabs are `LazyVec`s — one pointer until a process allocates that kind — for 2 875–2 876.
//! The tree before it (2 978–2 979) reds this bound.
//!
//! Lowered 2 940 → 2 800 (2026-10-18, later still): the GC statistics, the `proc/flag`
//! settings, the debugger's trace context and a test's shared-code floor moved into
//! `HeapExtras`, boxed at first write; the retired code-generation list is a `LazyVec`; the
//! checker slot is one pointer. 2 747–2 748 B. The tree
//! before it (2 875–2 876) reds this bound.
//!
//! Held at 2 800 (2026-10-28): the mailbox lanes and interrupt windows (ADR-490, 491, 493) had
//! put a parked process at 2 872 (CI) — the latest-lane index (a 48 B `HashMap`) and the
//! interrupt's window state (64 B) inline on every mailbox. Both are boxed at first use now,
//! the probe's `flags` word alone staying inline: 2 780 locally.
//!
//! Raised 2 800 → 2 825 (2026-11-02), for build-to-build variation, not growth: CI read 2 802
//! on `fe79302d`. The same per-process code measured 2 787 at `63afdb45` and 2 799 at
//! `cad037a4`, whose only code change is the GUI's `bleed.rs`. Each binary repeats its own
//! figure exactly, so the 12 B is how one build lays out what a spawn allocates, and the
//! readings from `fac6c240` on span 2 786–2 802. 2 825 still reds both regressions recorded
//! above (2 831 from the byte trigger's fields, 2 872 from the mailbox lanes).
//!
//! Lowered 2 825 → 2 700 (2026-11-02, later): the debug build copied every entered arm's
//! call-site positions into a per-process table (129 B per parked process, nothing in
//! release) for JIT stale-value reports; it is filled only under `BROOD_JIT_VERIFY` now. The
//! build that read 2 799 reads 2 670. 2 700 leaves the 12 B build variation above plus
//! margin, and still reds a step the size of the byte trigger's (+44 B, 2 714) or the
//! lanes' (+85 B). Sabotage: this tree run with `BROOD_JIT_VERIFY=1` reads 2 799 and reds it.

use brood::Interp;

const FLOOR_BYTES: i64 = 2700;

#[test]
fn a_parked_process_costs_no_more_than_the_floor() {
    // The floor is the VM's with shared arms, the shipped configuration. The tree-walker
    // (`BROOD_TIER=0`, CI's differential job) has no floor to ratchet: its `spawn` copies the
    // spawner's whole env frame into the child — here the growing `acc` list — so a process
    // costs O(n) and the fleet O(n²). Skipped by ceiling rather than absorbed into the bound.
    if brood::eval::compile::tier_ceiling() == brood::eval::compile::Tier::TreeWalk {
        eprintln!("process_floor: not the shipped engine configuration — nothing to ratchet");
        return;
    }
    let mut interp = Interp::new();
    // The SLOPE between two fleet sizes in one run, not the growth over one fleet divided by
    // its size: a one-time allocation inside the window (a body compiled, a cache warmed, a
    // table grown) is in the second and divides out of the first, and how big it is varies
    // from build to build — 3 587 B/proc at n = 5 000 against 3 131 at 40 000 on one binary,
    // and ~200 B between two builds of the same tree at 20 000 (2026-10-18). The warm-up
    // fleet takes the one-time costs; the measured fleet pays only for itself.
    let prog = r#"
        (defn parked () (receive ([:go] nil)))
        ;; NOT `(spawn-hold (+ i 1) (cons (spawn (parked)) acc))`: under the tree-walker
        ;; (`BROOD_TIER=0`, CI's differential job) a spawn copies the spawner's whole env
        ;; frame into the child, and a frame holding the growing `acc` makes the fleet
        ;; O(n²) — 20k children asked for a 7 GiB block and a 7 GB GitHub runner was
        ;; shut down rather than failed, three pushes running (2026-09-21). A `mapv`
        ;; frame holds one index and nothing that grows.
        (defn spawn-all (n) (seq/mapv (range n) (fn (_) (spawn (parked)))))
        (def warm (spawn-all 5000))
        (proc/sleep 200)
        (def b0 (%mem-bytes))
        (def n 20000)
        (def kids (spawn-all n))
        (proc/sleep 200)
        (def per (math/quot (- (%mem-bytes) b0) n))
        ;; Release them so the runtime tears down cleanly.
        (fold (into warm kids) nil (fn (_ p) (send p [:go])))
        per
    "#;
    let v = interp.eval_str(prog).expect("the probe ran");
    let per: i64 = interp.print(v).parse().expect("an integer byte count");
    eprintln!("parked process: {per} B/proc counted (floor {FLOOR_BYTES})");
    assert!(
        per <= FLOOR_BYTES,
        "a parked process costs {per} B of counted allocation, over the {FLOOR_BYTES} B floor — \
         something new is allocated per spawn; run the allocation histogram \
         (docs/runtime-frontier.md §B) before raising this number"
    );
}
