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
    /// `max(GC_FLOOR, 2 * live)`), or once the nursery has spent its fresh-buffer budget —
    /// an object count alone cannot see a vector of a million elements (KI-307). Cheap: a
    /// sum of the slab lengths and two compares.
    #[inline]
    pub fn gc_due(&self) -> bool {
        self.gc_enabled
            && (self.local_live_count() >= self.gc_threshold || self.nursery_budget_left == 0)
    }

    /// Spend `bytes` of a fresh buffer — a spilled vector, string text, a byte blob — from
    /// the nursery's budget, the byte half of [`gc_due`](Self::gc_due).
    #[inline]
    pub(in crate::core::heap) fn note_fresh_buffer(&mut self, bytes: usize) {
        self.nursery_budget_left = self.nursery_budget_left.saturating_sub(budget_units(bytes));
    }

    /// Nursery growth factor: the next collection triggers at `growth × live`. Default **2.0**,
    /// overridable with `BROOD_GC_GROWTH` — the A/B lever this had no way to measure before.
    ///
    /// Lowering it is known NOT to reduce peak memory on large-structure rows (the peak
    /// there is the live structure itself, doubled by to-space); it stays a knob for A/B.
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

    /// Everything this process's heap holds, in bytes — both generations, their retained
    /// capacity, and what their entries own (vector buffers, string text, shared blobs).
    /// O(entries): the measure memory pressure compares processes by, not an observer's
    /// per-park figure (that is [`local_bytes`](Self::local_bytes)).
    pub fn footprint(&self) -> usize {
        owned_bytes(&self.local) + self.old_opt().map_or(0, owned_bytes)
    }

    /// **`(hibernate)` — the opt-in deep shrink.** Unconditionally does everything
    /// [`trim_parked`](Self::trim_parked) does — collect, shrink the slabs, shrink the root
    /// vectors — and additionally **drops this process's inline-cache tables**, which the
    /// automatic path deliberately does not.
    ///
    /// Why this is a *builtin* and not a policy: dropping the ICs on every park would cost
    /// a process that parks in a loop the caches it built entering its hot loop. The
    /// *programmer* says when a process is going idle for a long time: use it in a process
    /// about to wait a long while (a pooled connection, an idle session actor); do not use
    /// it in a request loop.
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
        // Base counters reset in lockstep with the registry + tables (ADR-096's guards
        // rely on the epoch/sym checks, not on unique ids).
        self.next_ic_base.set(0);
        self.next_gic_base.set(0);
        self.arm_ic_blocks.borrow_mut().shrink_to_fit();
        // The ability-dispatch ICs are caches on exactly the same terms (every entry is
        // validated against `global_epoch` before use), so "drop the caches" has to
        // include them.
        self.dispatch_ics.borrow_mut().clear();
        self.dispatch_ics.borrow_mut().shrink_to_fit();
        // The compiled-body cache is a cache too, and by far the largest thing a
        // long-idle process can give back. Shared arms (ADR-175) are held by the runtime,
        // so dropping our reference is cheap and they re-install on the next call.
        self.vm_cache.write().clear();
        let after =
            slab_capacity_bytes(&self.local) + self.old_opt().map_or(0, slab_capacity_bytes);
        // Reset the auto-trim high-water: after a deliberate deep shrink, the next
        // ordinary park should judge growth from here, not from the pre-hibernate peak.
        self.park_trim_mark = park_trim_probe(&self.local);
        before.saturating_sub(after)
    }

    /// Would [`trim_parked`](Self::trim_parked) do anything — has capacity grown enough
    /// since the last trim? The gate runs on EVERY park, so it is three loads and a compare
    /// — see `park_trim_probe`. Only capacity accumulated *since the last trim* counts; an
    /// absolute threshold fails in both directions (`PARK_TRIM_GROWTH_SLOTS`). Asked on its
    /// own by the park, which runs the trim itself outside the mailbox lock.
    pub fn park_trim_due(&self) -> bool {
        park_trim_due(park_trim_probe(&self.local), self.park_trim_mark)
    }

    /// **Trim a process that is about to park.** Returns the bytes of retained capacity
    /// handed back, or `0` if the process was below the threshold and nothing was done.
    ///
    /// A parked process is quiescent and may stay so for the life of the program, so the
    /// two things a *running* heap is right to keep are exactly wrong here: uncollected
    /// garbage (a process that allocates and then parks never reaches another safepoint)
    /// and the slab `Vec`s' high-water capacity (which a nursery flip deliberately
    /// preserves — see `Slabs::with_capacity_like`). So: collect, then shrink, at the one
    /// moment we know the process has nothing to do.
    ///
    /// **Soundness.** The captured continuation (`Suspended`) holds only *control* state —
    /// its frames reference the operand stack and frame slots by index, and those live on
    /// this heap's own `roots`/`env_roots`, which `collect` traces. That is the same
    /// invariant a running process relies on at every safepoint, so collecting here is no
    /// more dangerous than collecting one instruction earlier.
    ///
    /// **Threshold.** Skipped unless capacity grew by `PARK_TRIM_GROWTH_SLOTS` since the
    /// last trim, so a latency-sensitive process that parks constantly with a small heap
    /// pays one integer comparison and nothing else.
    pub fn trim_parked(&mut self) -> usize {
        if !self.park_trim_due() {
            return 0;
        }
        let probe = park_trim_probe(&self.local);
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
        // messages. Against the high-water, a process only
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
        if limit.is_none() && self.extras.is_none() {
            return None; // clearing a limit nobody set
        }
        let extras = self.extras_mut();
        if limit.is_none() {
            extras.proc_limit_hit = None;
        }
        std::mem::replace(&mut extras.proc_mem_limit, limit)
    }

    /// This process's heap limit, if set. Backs the `(process-flag :max-heap)` read.
    pub fn proc_mem_limit(&self) -> Option<usize> {
        self.extras().and_then(|e| e.proc_mem_limit)
    }

    /// `(process-flag :send-errors on)` — should a `send` to a disconnected node
    /// raise `:noconnection` (vs the default silent drop)? Setter returns the
    /// previous value.
    pub fn proc_send_errors(&self) -> bool {
        self.extras().is_some_and(|e| e.proc_send_errors)
    }

    pub fn set_proc_send_errors(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.extras_mut().proc_send_errors, on)
    }

    /// `(proc/flag :no-break)` — does a breakpoint pass this process through rather than
    /// park it? Setter returns the previous value.
    pub fn proc_no_break(&self) -> bool {
        self.extras().is_some_and(|e| e.proc_no_break)
    }

    pub fn set_proc_no_break(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.extras_mut().proc_no_break, on)
    }

    /// `(proc/flag :responsive)` — how this process treats a blocking native call
    /// (ADR-517). Setter returns the previous mode.
    pub(crate) fn proc_responsive(&self) -> crate::process::responsive::Responsive {
        self.extras()
            .map_or_default(|extras| extras.proc_responsive)
    }

    pub(crate) fn set_proc_responsive(
        &mut self,
        mode: crate::process::responsive::Responsive,
    ) -> crate::process::responsive::Responsive {
        std::mem::replace(&mut self.extras_mut().proc_responsive, mode)
    }

    /// Take the sticky over-limit flag (post-collection live bytes) — the eval/VM
    /// safepoint probe. Clearing on read means the raise happens exactly once;
    /// if the process catches it and keeps allocating, the next collection
    /// re-arms the flag.
    pub fn take_proc_limit_hit(&mut self) -> Option<usize> {
        self.extras.as_mut().and_then(|e| e.proc_limit_hit.take())
    }

    /// Is a per-process heap-limit breach waiting to be raised? A peek for the native
    /// loop's poll, which must not consume it — the VM raises it at its own safepoint.
    pub fn proc_limit_pending(&self) -> bool {
        self.extras().is_some_and(|e| e.proc_limit_hit.is_some())
    }

    /// Post-collection heap-limit check: called at the end of both collection
    /// paths (legacy flip + generational), where the slabs hold exactly the
    /// survivors — so the figure is *live* data, never reclaimable garbage.
    /// O(1) (slab lens × sizes); no-op unless a limit is set.
    pub(super) fn note_proc_limit(&mut self) {
        if let Some(limit) = self.proc_mem_limit() {
            let live = slab_bytes(&self.local) + self.old_opt().map_or(0, slab_bytes);
            if live > limit {
                self.extras_mut().proc_limit_hit = Some(live);
            }
        }
    }

    /// GC observability counters (Tier-1; `docs/memory-review.md` §7), as a
    /// `(runs, copied, reclaimed)` triple of cumulative figures since process
    /// start: collections performed, LOCAL objects relocated, LOCAL objects
    /// dropped. Backs the `(dev/gc-stats)` builtin. Minor and major collections
    /// each count as one run.
    pub fn gc_counters(&self) -> (u64, u64, u64) {
        let (copied, reclaimed) = self
            .extras()
            .map_or((0, 0), |e| (e.gc_copied, e.gc_reclaimed));
        (self.gc_runs, copied, reclaimed)
    }

    /// GC pause durations `(total_ns, max_ns, last_ns)` — the timing tier's
    /// per-process figures (cumulative wall time in collections, worst single
    /// pause, most recent pause). Backs `(dev/gc-stats)`'s `:pause-*-us` keys.
    pub fn gc_pause_ns(&self) -> (u64, u64, u64) {
        self.extras()
            .map_or((0, 0, 0), |e| (e.gc_ns_total, e.gc_ns_max, e.gc_ns_last))
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
        self.extras()
            .and_then(|e| e.gc_trace)
            .unwrap_or_else(super::gc_trace_default)
    }

    /// Turn per-collection GC trace logging on/off for this process (each
    /// minor/major collection then prints a one-line stderr summary). Backs
    /// `(gc-trace on/off)`.
    pub fn set_gc_trace(&mut self, on: bool) {
        self.extras_mut().gc_trace = Some(on);
    }
}

#[cfg(test)]
mod extras_tests {
    use crate::core::heap::Heap;

    /// Reading any of the rarely-written fields at its default must not allocate their box
    /// (`Heap::extras`): every process reads several on its way to parking, and a read that
    /// allocated would put the bytes back on the floor (`process_floor.rs`) unnoticed by
    /// anything else. Writing one allocates, and the value reads back.
    #[test]
    fn reading_the_defaults_leaves_the_extras_unallocated() {
        let mut heap = Heap::new();
        assert_eq!(heap.proc_mem_limit(), None);
        assert!(!heap.proc_send_errors() && !heap.proc_no_break());
        assert_eq!(heap.take_proc_limit_hit(), None);
        assert!(!heap.proc_limit_pending());
        assert_eq!(heap.gc_counters().1, 0);
        assert_eq!(heap.gc_pause_ns(), (0, 0, 0));
        let _ = heap.gc_trace();
        let _ = heap.floor();
        assert_eq!(heap.set_proc_mem_limit(None), None);
        #[cfg(feature = "dev-tools")]
        {
            assert!(heap.trace_context().is_none());
            assert!(!heap.trace_context_own());
            heap.set_trace_context(None, false);
        }
        assert!(
            heap.extras().is_none(),
            "a default read or a no-op clear allocated"
        );

        assert_eq!(heap.set_proc_mem_limit(Some(1 << 20)), None);
        assert!(heap.extras().is_some());
        assert_eq!(heap.proc_mem_limit(), Some(1 << 20));
        assert!(!heap.set_proc_no_break(true));
        assert!(heap.proc_no_break());
        heap.set_gc_trace(true);
        assert!(heap.gc_trace());
    }
}
