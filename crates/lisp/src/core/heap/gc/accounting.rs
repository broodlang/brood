//! When to collect, and what it costs: the trigger (`gc_due` / growth policy), live
//! counts and byte estimates, hibernation and parked-process trimming, the per-process
//! memory limit (ADR-043) and the counters `(gc-stats)` reads.

use super::*;

impl Heap {
    /// Is GC armed on this heap? `false` for the prelude *builder* (we don't
    /// collect during the one-shot build/freeze) and `true` for every real
    /// process heap. Lets the evaluator skip the safepoint check cheaply when
    /// it isn't applicable.
    pub fn gc_enabled(&self) -> bool {
        self.gc_enabled
    }

    /// Should the next safepoint run a collection? Compares LOCAL live count
    /// against the adaptive threshold (recomputed by [`Self::collect`] as
    /// `max(GC_FLOOR, 2 * live)`). Cheap: an addition over six small `usize`s
    /// and a compare.
    #[inline]
    pub fn gc_due(&self) -> bool {
        self.gc_enabled && self.local_live_count() >= self.gc_threshold
    }

    /// Nursery growth factor: the next collection triggers at `growth × live`. Default **2.0**,
    /// overridable with `BROOD_GC_GROWTH` — the A/B lever this had no way to measure before.
    ///
    /// **Measured 2026-07-30 and it is NOT a lever — do not re-run this experiment.** The GC
    /// review named the `2 × live` growth as the dominant term in `sort`'s ~190 MB peak. It is
    /// not: sweeping 2.0 → 1.5 → 1.25 → 1.1 moves that row not at all (183–187 MB, inside
    /// noise), and `bintree` likewise. The peak is structural instead — ~750 000 cons cells live
    /// at once (the input list plus the new sorted one) at 48 bytes each, doubled by the copying
    /// collector's to-space and again by `Vec` capacity growth — and it is shared with every
    /// persistent-structure language (Elixir 160 MB, Clojure 124 MB on the same row).
    ///
    /// Kept as a knob because it made that refutation possible and costs one `OnceLock` read per
    /// collection, not because lowering it is known to help.
    pub(super) fn gc_growth() -> f64 {
        static G: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
        *G.get_or_init(|| {
            std::env::var("BROOD_GC_GROWTH")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|g| *g >= 1.05 && *g <= 8.0)
                .unwrap_or(2.0)
        })
    }

    /// LOCAL live-object count = `Σ slab.len()` over the LOCAL slabs. The metric
    /// the threshold tracks; also exposed for tests asserting reclamation in
    /// long-running loops. The collector is a moving copy collector that never
    /// reuses a slot in place (survivors are relocated into fresh slabs and the
    /// dead are dropped wholesale), so the live count is simply the slab lengths —
    /// there is no free list to subtract.
    pub fn local_live_count(&self) -> usize {
        slab_live_count(&self.local)
    }

    /// An estimate of this process's LOCAL heap footprint in **bytes** — the
    /// occupied slab entries weighted by element size (`len * size_of` per slab).
    /// Cheap (no traversal); counts the slab arrays themselves, not nested/shared
    /// content (inner vectors, string bytes, `Arc`-shared ropes), so it's a
    /// comparative figure for an observer, not an exact RSS. Bump-allocated, so it
    /// reflects allocation since the last arena reset / collection. Backs
    /// `process-info`'s `:memory` (published on `receive`).
    pub fn local_bytes(&self) -> usize {
        slab_bytes(&self.local)
    }

    /// **Trim a process that is about to park.** Returns the bytes of retained capacity
    /// handed back, or `0` if the process was below the threshold and nothing was done.
    ///
    /// A parked process is quiescent and may stay so for the life of the program, so the
    /// two things a *running* heap is right to keep are exactly wrong here:
    ///
    /// 1. **Uncollected garbage.** A process that allocates and then parks never reaches
    ///    another safepoint, so its dead data is pinned indefinitely. Measured 2026-07-28:
    ///    100k processes that consed 1,000 pairs and parked cost **54.1 KB each**; the same
    ///    with an explicit collection first cost **19.0 KB**.
    /// 2. **Retained capacity.** After collecting, the slab `Vec`s still hold their
    ///    high-water capacity (a nursery flip deliberately preserves it — see
    ///    `Slabs::with_capacity_like`). That is the 19.0 KB against **5.4 KB** for a process
    ///    that never allocated at all.
    ///
    /// So: collect, then shrink. This is Erlang's `hibernate/0` move, applied automatically
    /// at the one moment we know the process has nothing to do.
    ///
    /// **Soundness.** The captured continuation (`Suspended`) holds only *control* state —
    /// its frames reference the operand stack and frame slots by index, and those live on
    /// this heap's own `roots`/`env_roots`, which `collect` traces. That is the same
    /// invariant a running process relies on at every safepoint, so collecting here is no
    /// more dangerous than collecting one instruction earlier.
    ///
    /// **Threshold.** Skipped entirely below `PARK_TRIM_MIN_BYTES` of retained capacity, so
    /// a latency-sensitive process that parks constantly with a tiny heap (a ping-pong
    /// responder, a `gen` server handling small messages) pays one integer comparison and
    /// nothing else.
    /// **`(hibernate)` — the opt-in deep shrink** (Erlang's `erlang:hibernate/3`).
    /// Unconditionally does everything [`trim_parked`] does — collect, shrink the slabs,
    /// shrink the root vectors — and additionally **drops this process's inline-cache
    /// tables**, which the automatic path deliberately does not.
    ///
    /// Why this is a *builtin* and not a policy: dropping the ICs was measured
    /// (2026-07-29) at 4.53 → 3.89 KB per parked process, but doing it automatically on
    /// park costs `pingpong` +26% and `ring` +18% — a process that parks in a loop must
    /// keep the caches it built entering its hot loop. Restricting it to the first park
    /// barely helped (+11.5%). ERTS reached the same fork and resolved it the same way:
    /// the *programmer* says when a process is going idle for a long time. This is that
    /// call. Use it in a process about to wait a long while (a pooled connection, an idle
    /// session actor); do not use it in a request loop.
    ///
    /// Sound because the IC tables are pure caches — every entry is validated against
    /// `(sym, argc, epoch)` before use, so discarding them costs a miss and a
    /// re-resolution, never a wrong answer. `runtime_collect` already clears them out
    /// from under live arms on exactly that reasoning; the currently-running activation's
    /// `ic_bases` then names a block in an empty table, and every probe simply misses.
    ///
    /// Returns the bytes of slab capacity handed back (the IC drop is not counted — it is
    /// not slab capacity).
    pub fn hibernate(&mut self) -> usize {
        let before =
            slab_capacity_bytes(&self.local) + self.old_opt().map_or(0, slab_capacity_bytes);
        self.collect(&mut [], &mut []);
        shrink_slabs(&mut self.local);
        if let Some(o) = self.old.as_deref_mut() {
            shrink_slabs(o);
        }
        self.roots.shrink_to_fit();
        self.env_roots.shrink_to_fit();
        // The part the automatic path won't do.
        self.vm_call_ics.borrow_mut().clear();
        self.vm_call_ics.borrow_mut().shrink_to_fit();
        self.vm_fast_links.borrow_mut().clear();
        self.vm_fast_links.borrow_mut().shrink_to_fit();
        self.vm_global_ics.borrow_mut().clear();
        self.vm_global_ics.borrow_mut().shrink_to_fit();
        // Blocks index the tables just dropped, so they go in lockstep; the next
        // activation re-resolves a fresh block.
        self.arm_ic_blocks.borrow_mut().clear();
        // M2b: base counters reset in lockstep with the registry + tables, so
        // recycled site-id space starts at 0 exactly as it did when bases came
        // from `table.len()` (ADR-096's guards rely on the epoch/sym checks, not
        // on unique ids).
        self.next_ic_base.set(0);
        self.next_gic_base.set(0);
        self.arm_ic_blocks.borrow_mut().shrink_to_fit();
        // The ability-dispatch ICs are caches on exactly the same terms (every entry is
        // validated against `global_epoch` before use), so "drop the caches" has to
        // include them — leaving them behind was an inconsistency, not a decision. Memory
        // only: a long-idle actor that dispatched over an ability keeps a `HashMap` of
        // per-op ways alive for nothing.
        self.dispatch_ics.borrow_mut().clear();
        self.dispatch_ics.borrow_mut().shrink_to_fit();
        // The compiled-body cache is a cache too, and by far the largest thing a
        // long-idle process can give back. Shared arms (ADR-175) are held by the runtime,
        // so dropping our reference is cheap and they re-install on the next call.
        self.vm_cache.borrow_mut().clear();
        let after =
            slab_capacity_bytes(&self.local) + self.old_opt().map_or(0, slab_capacity_bytes);
        // Reset the auto-trim high-water: after a deliberate deep shrink, the next
        // ordinary park should judge growth from here, not from the pre-hibernate peak.
        self.park_trim_mark = park_trim_probe(&self.local);
        before.saturating_sub(after)
    }

    pub fn trim_parked(&mut self) -> usize {
        // The gate runs on EVERY park, so it is three loads and a compare — see
        // `park_trim_probe`. Only capacity accumulated *since the last trim* counts; an
        // absolute threshold fails in both directions (`PARK_TRIM_GROWTH_SLOTS`).
        let probe = park_trim_probe(&self.local);
        if probe.saturating_sub(self.park_trim_mark) < PARK_TRIM_GROWTH_SLOTS {
            return 0;
        }
        let before =
            slab_capacity_bytes(&self.local) + self.old_opt().map_or(0, slab_capacity_bytes);
        self.collect(&mut [], &mut []);
        shrink_slabs(&mut self.local);
        if let Some(o) = self.old.as_deref_mut() {
            shrink_slabs(o);
        }
        self.roots.shrink_to_fit();
        self.env_roots.shrink_to_fit();
        let after =
            slab_capacity_bytes(&self.local) + self.old_opt().map_or(0, slab_capacity_bytes);
        // Mark the **pre-trim high-water**, not the shrunken size. Marking `after` (the
        // obvious choice) makes the gate oscillate: the process shrinks to X, regrows to
        // X + 4 KiB, trims again, and a busy responder pays a collection every few
        // messages — measured as `pingpong` +8.5%. Against the high-water, a process only
        // trims when it accumulates *beyond a size it has already reached*, so a steady
        // working set trims once and never again.
        self.park_trim_mark = probe;
        before.saturating_sub(after)
    }

    /// Set this process's heap limit (bytes; `None` = unlimited), returning the
    /// previous setting — the `(process-flag :max-heap n)` mechanism. Clearing
    /// the limit also clears a pending hit, so `(process-flag :max-heap nil)`
    /// inside a `catch` genuinely rescues the process.
    pub fn set_proc_mem_limit(&mut self, limit: Option<usize>) -> Option<usize> {
        if limit.is_none() {
            self.proc_limit_hit = None;
        }
        std::mem::replace(&mut self.proc_mem_limit, limit)
    }

    /// This process's heap limit, if set. Backs the `(process-flag :max-heap)` read.
    pub fn proc_mem_limit(&self) -> Option<usize> {
        self.proc_mem_limit
    }

    /// `(process-flag :send-errors on)` — should a `send` to a disconnected node
    /// raise `:noconnection` (vs Erlang's silent drop)? Setter returns the
    /// previous value.
    pub fn proc_send_errors(&self) -> bool {
        self.proc_send_errors
    }

    pub fn set_proc_send_errors(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.proc_send_errors, on)
    }

    /// Take the sticky over-limit flag (post-collection live bytes) — the eval/VM
    /// safepoint probe. Clearing on read means the raise happens exactly once;
    /// if the process catches it and keeps allocating, the next collection
    /// re-arms the flag.
    pub fn take_proc_limit_hit(&mut self) -> Option<usize> {
        self.proc_limit_hit.take()
    }

    /// Post-collection heap-limit check: called at the end of both collection
    /// paths (legacy flip + generational), where the slabs hold exactly the
    /// survivors — so the figure is *live* data, never reclaimable garbage.
    /// O(1) (slab lens × sizes); no-op unless a limit is set.
    pub(super) fn note_proc_limit(&mut self) {
        if let Some(limit) = self.proc_mem_limit {
            let live = slab_bytes(&self.local) + self.old_opt().map_or(0, slab_bytes);
            if live > limit {
                self.proc_limit_hit = Some(live);
            }
        }
    }

    /// GC observability counters (Tier-1; `docs/memory-review.md` §7), as a
    /// `(runs, copied, reclaimed)` triple of cumulative figures since process
    /// start: collections performed, LOCAL objects relocated, LOCAL objects
    /// dropped. Backs the `(dev/gc-stats)` builtin. Counts both Stage-B safepoint
    /// collections and bare [`flush`](Self::flush) calls (they share [`arena_flip`]).
    pub fn gc_counters(&self) -> (u64, u64, u64) {
        (self.gc_runs, self.gc_copied, self.gc_reclaimed)
    }

    /// GC pause durations `(total_ns, max_ns, last_ns)` — the timing tier's
    /// per-process figures (cumulative wall time in collections, worst single
    /// pause, most recent pause). Backs `(dev/gc-stats)`'s `:pause-*-us` keys.
    pub fn gc_pause_ns(&self) -> (u64, u64, u64) {
        (self.gc_ns_total, self.gc_ns_max, self.gc_ns_last)
    }

    /// The current adaptive GC threshold (LOCAL live-object count that triggers
    /// the next safepoint collection). The slow/stable dial — exposed so an
    /// observer can see how close the heap is to its next collection.
    pub fn gc_threshold(&self) -> usize {
        self.gc_threshold
    }

    /// The RUNTIME closure count at which the next safepoint attempts a shared-code
    /// compaction (`max(BROOD_RT_GC_FLOOR, 2 * live)`; `usize::MAX` when auto-collect
    /// is off). The RUNTIME counterpart of [`gc_threshold`](Self::gc_threshold) —
    /// surfaced so an observer can see how close the shared region is to compacting.
    pub fn rt_gc_threshold(&self) -> usize {
        self.rt_gc_threshold
    }

    /// Whether per-collection GC tracing is on for this process. Backs the
    /// no-arg `(%)` query.
    pub fn gc_trace(&self) -> bool {
        self.gc_trace
    }

    /// Turn per-collection GC trace logging on/off for this process (each
    /// minor/major collection then prints a one-line stderr summary). Backs
    /// `(gc-trace on/off)`.
    pub fn set_gc_trace(&mut self, on: bool) {
        self.gc_trace = on;
    }
}
