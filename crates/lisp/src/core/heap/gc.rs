//! GC: roots, collection, promotion-flip, RUNTIME compaction (child of heap).
use super::*;

impl Heap {
    /// **Arena flip with value roots only** (no env roots) — the thin
    /// [`arena_flip`](Self::arena_flip) entry used where the live set is a flat
    /// list of `Value`s and no `env` needs relocating: the heap unit tests, and
    /// any future caller that has unwound to a clean point. Deep-copies the given
    /// LOCAL-reachable `roots` (plus this heap's [`dynamics`]/[`roots`] stacks)
    /// into a fresh `Slabs`, swaps it in, and drops the old; PRELUDE/RUNTIME
    /// handles are returned unchanged; cycles terminate via forwarding tables.
    ///
    /// The *automatic* collector ([`collect`](Self::collect)) is the production
    /// path — it shares this same `arena_flip` machinery but also relocates the
    /// eval loop's live `env`. (This used to back the removed `(hibernate)`
    /// primitive, reached via an unwinding sentinel; automatic GC made that
    /// redundant — docs/memory-review.md.)
    ///
    /// **Safety contract.** No LOCAL handle outside the supplied roots /
    /// dynamics / explicit-root stack may be reachable from the Rust stack — i.e.
    /// no in-flight eval frame whose `expr`/`env` points at LOCAL — or those
    /// stale handles dangle. Satisfied by calling only from a point with no live
    /// eval frame (the tests run it on a bare heap).
    pub fn flush(&mut self, roots: &mut [Value]) {
        self.arena_flip(roots, &mut []);
    }

    /// The arena flip shared by [`flush`](Self::flush) (value roots only,
    /// no env roots) and [`collect`](Self::collect) (the eval safepoint, which
    /// also roots the live `env`). A **semi-space copy**: move every LOCAL object
    /// reachable from the value roots, env roots, the dynamic-binding stack, and
    /// the explicit root stack into fresh slabs, then drop the old slabs whole.
    ///
    /// Roots are relocated **in place** — copying MOVES handles, so the caller
    /// must use the rewritten `value_roots`/`env_roots` afterwards. Cycles
    /// (`letrec` env↔closure) terminate via the forwarding tables in `fwd`
    /// (a placeholder is allocated before recursing). PRELUDE/RUNTIME handles are
    /// returned unchanged (the promotion invariant guarantees they hold no LOCAL
    /// refs). Crucially this **never reuses a slot index** — it relocates and
    /// drops — so it cannot resurrect the slot-aliasing scheduler race that got
    /// the original in-place mark-sweep collector deleted (see
    /// `docs/claude-demo-findings.md` § Scheduler race).
    fn arena_flip(&mut self, value_roots: &mut [Value], env_roots: &mut [EnvId]) {
        // Bump the generation epoch *before* copying: survivors are re-minted
        // into the fresh slabs stamped with the NEW epoch (via `fwd.epoch`), so
        // any handle held across this flip without being relocated keeps the OLD
        // epoch and trips the debug deref check. `wrapping_add` is fine — a
        // collision needs 2^30 flips of one heap between a handle's mint and its
        // stale use.
        // Live LOCAL objects *before* the copy — survivors come out of the flip
        // below, so `before - survivors` is what this collection reclaims.
        let before = self.local_live_count();
        self.local_epoch = self.local_epoch.wrapping_add(1);
        let old = std::mem::take(&mut self.local);
        let mut fwd = FlushForward::for_source(&old);
        fwd.epoch = self.local_epoch;
        for v in value_roots.iter_mut() {
            *v = flush_value(&old, &mut self.local, &mut fwd, *v);
        }
        for e in env_roots.iter_mut() {
            *e = flush_env(&old, &mut self.local, &mut fwd, *e);
        }
        for (_, v) in self.dynamics.iter_mut() {
            *v = flush_value(&old, &mut self.local, &mut fwd, *v);
        }
        #[cfg(feature = "dev-tools")]
        if let Some(v) = &mut self.trace_context {
            *v = flush_value(&old, &mut self.local, &mut fwd, *v);
        }
        for v in self.roots.iter_mut() {
            *v = flush_value(&old, &mut self.local, &mut fwd, *v);
        }
        // The env half of the operand stack (ADR-061) — relocate in place so an
        // eval frame's `scope`/`env` held across a deeper collection survives.
        let mut env_roots = std::mem::take(&mut self.env_roots);
        for e in env_roots.iter_mut() {
            *e = flush_env(&old, &mut self.local, &mut fwd, *e);
        }
        self.env_roots = env_roots;
        // form_pos is keyed by LOCAL pair index, which the copy *relocates*.
        // Re-key it through the pair forwarding table (old idx → new idx) so a
        // collection mid-file-load doesn't lose the reader positions later error
        // messages point at; entries for pairs that didn't survive are dropped
        // with them. (Any still-live form's position survives the arena flip
        // rather than being discarded.)
        // Legacy single-space flush: nursery→nursery, so keys stay young (age 0).
        let old_form_pos = std::mem::take(&mut self.form_pos);
        for (key, pos) in old_form_pos {
            if let Some(new_idx) = fwd.pairs.lookup(key as u32) {
                self.form_pos.insert(new_idx as u64, pos);
            }
        }
        // GC observability (Tier-1). After the flip the fresh slabs hold exactly
        // the survivors, so `local_live_count()` is the survivor count. Saturating
        // so a pathological wrap can't panic on the collector hot path.
        let survivors = self.local_live_count();
        self.gc_runs = self.gc_runs.saturating_add(1);
        self.gc_copied = self.gc_copied.saturating_add(survivors as u64);
        self.gc_reclaimed = self
            .gc_reclaimed
            .saturating_add(before.saturating_sub(survivors) as u64);
        self.note_proc_limit();
        // `old` drops here, releasing every LOCAL slot the previous iteration
        // ever allocated.
    }
    // ===== GC roots and root stack ==============================================
    //
    // A small explicit root stack for the few sites (today: `eval_str` /
    // `eval_source`) that hold a `Vec<Value>` of LOCAL forms across a depth-0
    // eval call. Every other place is either already reachable from
    // `env`/`expr` at the safepoint, or sits at `GC_BLOCK > 1` where GC won't
    // fire — see `docs/memory-model.md`. Empty on the hot path.

    /// Push `v` onto the explicit root stack so it survives any GC that may run
    /// between now and the matching [`Self::truncate_roots`]. Cheap: one `Vec` push.
    pub fn push_root(&mut self, v: Value) {
        self.roots.push(v);
    }

    /// Grow `roots` to `len`, filling any new slots with `Nil` — a call frame's slot
    /// pre-fill in one `Vec::resize` instead of a per-slot `push_root` loop on the
    /// (hot) call path. `len` must be ≥ the current length (frames only grow here).
    pub fn extend_roots_to_nil(&mut self, len: usize) {
        debug_assert!(len >= self.roots.len());
        let old = self.roots.len();
        self.roots.reserve(len - old);
        // SAFETY: all-zero bytes are a valid `Value` (`Nil` is discriminant 0; the
        // payload/padding bytes are ignored for it), so one `write_bytes` replaces
        // `resize`'s per-slot 24-byte clone loop — this is the frame nil-fill on the
        // hottest path in the runtime (every call frame, both VM and JIT fast-link).
        unsafe {
            std::ptr::write_bytes(self.roots.as_mut_ptr().add(old), 0, len - old);
            self.roots.set_len(len);
        }
    }

    /// Append `n` `Value`s from `src` onto `roots` in one reserve+copy — the batch
    /// form of [`Self::push_root`] for the JIT's call staging (one FFI + one memcpy
    /// instead of `brood_rt_push` × argc).
    ///
    /// # Safety
    /// `src` must point to `n` valid, initialized `Value`s (the JIT's per-site
    /// staging stack slot, written just before the call).
    #[cfg(feature = "jit")]
    pub(crate) unsafe fn push_roots_n(&mut self, src: *const Value, n: usize) {
        let old = self.roots.len();
        self.roots.reserve(n);
        std::ptr::copy_nonoverlapping(src, self.roots.as_mut_ptr().add(old), n);
        self.roots.set_len(old + n);
    }

    /// Raw base pointer of the operand-stack/`roots` buffer, for JIT'd code to index
    /// frame slots directly (`roots_base + (base+i) * size_of::<Value>()`). Valid only
    /// while `roots` does not reallocate — a tier-1 JIT'd arm keeps operands in
    /// registers (it never `push`es), and the int-arithmetic subset never allocates
    /// (so no GC grows it), so the pointer is stable for the arm's duration. Callers
    /// outside that invariant must re-fetch after any push. See `src/jit/`.
    #[cfg(feature = "jit")]
    pub(crate) fn roots_base_ptr(&mut self) -> *mut Value {
        self.roots.as_mut_ptr()
    }

    /// Raw byte pointer to the LOCAL nursery pair slab (the flat `Vec<(Value, Value)>` backing
    /// young LOCAL `Value::Pair` handles). Valid only while no `cons` can grow the slab. Used
    /// by `brood_rt_pair_nursery_base` so the JIT can inline `first`/`rest` for LOCAL pairs
    /// instead of calling `brood_rt_car`/`cdr` per element.
    #[cfg(feature = "jit")]
    pub(crate) fn local_pair_nursery_base(&self) -> *const u8 {
        self.local.pairs.as_ptr() as *const u8
    }

    /// Raw byte pointer to the LOCAL old-generation pair slab (pairs that survived a minor
    /// GC and were promoted). Companion to [`local_pair_nursery_base`].
    #[cfg(feature = "jit")]
    pub(crate) fn local_pair_old_base(&self) -> *const u8 {
        self.old.pairs.as_ptr() as *const u8
    }

    /// Raw byte pointer to the LOCAL nursery **vector** slab, so a no-call/no-GC
    /// JIT arm can inline a small-vector element read (`slot + JIT_ITEMS_OFF +
    /// i*STRIDE`) instead of calling `brood_rt_vector_ref` — the vector analog of
    /// [`local_pair_nursery_base`]. Each slot is a [`VecStore`] (stride
    /// [`VecStore::JIT_STRIDE`]); the JIT reads the discriminant + inline `len`
    /// and deopts for a spilled (large) vector.
    #[cfg(feature = "jit")]
    pub(crate) fn local_vec_nursery_base(&self) -> *const u8 {
        self.local.vectors.as_ptr() as *const u8
    }

    /// Raw byte pointer to the LOCAL old-generation vector slab. Companion to
    /// [`local_vec_nursery_base`].
    #[cfg(feature = "jit")]
    pub(crate) fn local_vec_old_base(&self) -> *const u8 {
        self.old.vectors.as_ptr() as *const u8
    }

    /// Current root-stack depth, for a balanced `truncate_roots(roots_len())`
    /// guard around a region that may push variable numbers of roots.
    pub fn roots_len(&self) -> usize {
        self.roots.len()
    }

    /// Drop every root pushed since the recorded depth (i.e. shrink to `n`).
    /// The paired teardown for a `let n = heap.roots_len(); … heap.push_root(v);
    /// … heap.truncate_roots(n);` region.
    pub fn truncate_roots(&mut self, n: usize) {
        self.roots.truncate(n);
    }

    /// Root `v` for the duration of a collection-bearing region, **skipping the
    /// operand-stack push only when `v` is truly fixed** (an atom or a `PRELUDE`
    /// handle). A `RUNTIME` handle *does* take a slot: it is immovable under the
    /// LOCAL collector but the runtime compactor ([`runtime_collect`]) evacuates
    /// it, and only the operand stack is rewritten there — so an inlined RUNTIME
    /// root would go stale across a compaction (the slab-OOB / corruption class,
    /// `docs/known-issues.md`). Returns a [`Root`] token to read back with
    /// [`read_root`](Self::read_root) after any nested eval. Teardown is the
    /// shared `truncate_roots(base)` — it drops exactly the slots pushed,
    /// regardless of how many were skipped.
    ///
    /// [`runtime_collect`]: Self::runtime_collect
    #[inline]
    pub fn root(&mut self, v: Value) -> Root {
        if needs_root_slot(v) {
            let i = self.roots.len();
            self.roots.push(v);
            Root::Slot(i)
        } else {
            Root::Stable(v)
        }
    }

    /// Read back a [`Root`] (the relocated handle if it took a slot, else the
    /// inline immovable value).
    #[inline]
    pub fn read_root(&self, r: Root) -> Value {
        match r {
            Root::Stable(v) => v,
            Root::Slot(i) => self.roots[i],
        }
    }

    /// Advance an in-place cursor (e.g. a cons spine) to `v`, reusing the same
    /// slot if the cursor is rooted. The region is invariant along a *promoted*
    /// cons chain (a RUNTIME pair's cdr is RUNTIME, a PRELUDE pair's cdr is
    /// PRELUDE), so a `Stable` cursor's successor is normally immovable too and
    /// stays inline — no per-iteration slot growth. A `Stable` cursor whose
    /// successor *is* movable (e.g. a `(cons x runtime-list)` LOCAL pair tailing
    /// into shared code, walked from the other side) falls back to a real root
    /// rather than risk a dangling handle — costs nothing on the common path
    /// (`root` of an immovable value never pushes).
    #[inline]
    pub fn advance_root(&mut self, r: Root, v: Value) -> Root {
        match r {
            Root::Slot(i) => {
                self.roots[i] = v;
                Root::Slot(i)
            }
            Root::Stable(_) => self.root(v),
        }
    }

    /// The [`EnvId`] counterpart of [`root`](Self::root): a LOCAL **or** RUNTIME
    /// frame takes a slot (the LOCAL collector relocates the former, the runtime
    /// compactor [`runtime_collect`](Self::runtime_collect) evacuates the latter,
    /// ADR-076), while the [`EnvId::GLOBAL`] sentinel and immutable PRELUDE frames
    /// stay inline. An inlined RUNTIME frame would be invisible to the runtime
    /// compaction's `env_roots` rewrite and go stale. Read back with
    /// [`read_root_env`](Self::read_root_env).
    #[inline]
    pub fn root_env(&mut self, e: EnvId) -> EnvRoot {
        if e != EnvId::GLOBAL && (e.region() == LOCAL || e.region() == RUNTIME) {
            let i = self.env_roots.len();
            self.env_roots.push(e);
            EnvRoot::Slot(i)
        } else {
            EnvRoot::Stable(e)
        }
    }

    /// Read back an [`EnvRoot`] (the relocated frame if it took a slot, else the
    /// inline immovable env).
    #[inline]
    pub fn read_root_env(&self, r: EnvRoot) -> EnvId {
        match r {
            EnvRoot::Stable(e) => e,
            EnvRoot::Slot(i) => self.env_roots[i],
        }
    }

    // ----- env operand stack (ADR-061) ----------------------------------------
    // The `EnvId` half of the operand stack: an eval frame's `scope`/`env` held
    // across a nested `eval` lives here so a collection at *any* depth relocates
    // it. Mirrors the value-root API above.

    /// Push an env onto the env-root stack; survives any GC until the matching
    /// [`truncate_env_roots`](Self::truncate_env_roots).
    pub fn push_env_root(&mut self, e: EnvId) {
        self.env_roots.push(e);
    }

    /// Current env-root depth, for a balanced
    /// `truncate_env_roots(env_roots_len())` guard.
    pub fn env_roots_len(&self) -> usize {
        self.env_roots.len()
    }

    /// The relocated handle of the `i`th env root (read back after a nested eval
    /// that may have collected).
    pub fn env_root_at(&self, i: usize) -> EnvId {
        self.env_roots[i]
    }

    /// Shrink the env-root stack to `n` (teardown paired with `push_env_root`).
    pub fn truncate_env_roots(&mut self, n: usize) {
        self.env_roots.truncate(n);
    }

    /// Run `f` within a root-stack checkpoint: both `roots` and `env_roots` are
    /// restored to their entry depths on return, whether `f` succeeds or fails.
    ///
    /// This replaces the recurring manual save/restore pattern:
    ///
    /// ```text
    /// let vb = heap.roots_len();
    /// let eb = heap.env_roots_len();
    /// // ... push roots ...
    /// match eval(...) {
    ///     Err(e) => { heap.truncate_roots(vb); heap.truncate_env_roots(eb); return Err(e); }
    ///     Ok(v) => { ... }
    /// }
    /// heap.truncate_roots(vb);
    /// heap.truncate_env_roots(eb);
    /// ```
    ///
    /// with a single `heap.root_scope(|heap| { ... })`.  Use `?` inside the
    /// closure for early exits — cleanup is still guaranteed.
    #[inline]
    pub fn root_scope<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<R, crate::error::LispError>,
    ) -> Result<R, crate::error::LispError> {
        let vb = self.roots_len();
        let eb = self.env_roots_len();
        let result = f(self);
        self.truncate_roots(vb);
        self.truncate_env_roots(eb);
        result
    }

    // ===== GC trigger, RUNTIME compaction, and statistics =======================

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
    pub fn trim_parked(&mut self) -> usize {
        // The gate runs on EVERY park, so it is three loads and a compare — see
        // `park_trim_probe`. Only capacity accumulated *since the last trim* counts; an
        // absolute threshold fails in both directions (`PARK_TRIM_GROWTH_SLOTS`).
        let probe = park_trim_probe(&self.local);
        if probe.saturating_sub(self.park_trim_mark) < PARK_TRIM_GROWTH_SLOTS {
            return 0;
        }
        let before = slab_capacity_bytes(&self.local) + slab_capacity_bytes(&self.old);
        self.collect(&mut [], &mut []);
        shrink_slabs(&mut self.local);
        shrink_slabs(&mut self.old);
        self.roots.shrink_to_fit();
        self.env_roots.shrink_to_fit();
        let after = slab_capacity_bytes(&self.local) + slab_capacity_bytes(&self.old);
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
    fn note_proc_limit(&mut self) {
        if let Some(limit) = self.proc_mem_limit {
            let live = slab_bytes(&self.local) + slab_bytes(&self.old);
            if live > limit {
                self.proc_limit_hit = Some(live);
            }
        }
    }

    /// GC observability counters (Tier-1; `docs/memory-review.md` §7), as a
    /// `(runs, copied, reclaimed)` triple of cumulative figures since process
    /// start: collections performed, LOCAL objects relocated, LOCAL objects
    /// dropped. Backs the `(gc-stats)` builtin. Counts both Stage-B safepoint
    /// collections and bare [`flush`](Self::flush) calls (they share [`arena_flip`]).
    pub fn gc_counters(&self) -> (u64, u64, u64) {
        (self.gc_runs, self.gc_copied, self.gc_reclaimed)
    }

    /// GC pause durations `(total_ns, max_ns, last_ns)` — the timing tier's
    /// per-process figures (cumulative wall time in collections, worst single
    /// pause, most recent pause). Backs `(gc-stats)`'s `:pause-*-us` keys.
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
    /// no-arg `(gc-trace)` query.
    pub fn gc_trace(&self) -> bool {
        self.gc_trace
    }

    /// Turn per-collection GC trace logging on/off for this process (each
    /// minor/major collection then prints a one-line stderr summary). Backs
    /// `(gc-trace on/off)`.
    pub fn set_gc_trace(&mut self, on: bool) {
        self.gc_trace = on;
    }

    // ----- the tracing GC ------------------------------------------------------
    //
    // A generational, moving **copy collector** over the LOCAL heap only
    // (ADR-054; `docs/memory-review.md`). A *minor* collection either tenures
    // the nursery's survivors into the old generation or semi-space-flips the
    // nursery in place; a *major* compacts the old generation when it has
    // doubled. Survivors are relocated into fresh slabs and the dead dropped
    // wholesale — no slot is ever reused in place. Roots are:
    // `extra_roots`/`extra_envs` (the caller — usually the eval safepoint —
    // supplies `expr`/`env` here), the explicit root stack [`Self::roots`],
    // the operand-stack env half [`Self::env_roots`], the write-barrier
    // [`Self::remembered`] set (minor only), and the dynamic-binding stack
    // [`Self::dynamics`]. The PRELUDE and RUNTIME regions are never traced
    // into (they hold no LOCAL refs, by the promotion invariant), so the walk
    // stays bounded by *this* process's working set.

    // ===== Collection — minor (LOCAL nursery → old) =============================

    /// **Stage B — automatic copying collection at the eval safepoint** (ADR-054;
    /// `docs/memory-review.md`). Fired by `eval::eval` when `gc_due()` *and* we are
    /// the outermost eval (`gc_block_depth() == 1`), so the only live LOCAL handles
    /// are the ones reachable from the roots below — see the safepoint's
    /// rooting-completeness argument. A semi-space copy via [`arena_flip`]: relocate
    /// every LOCAL object reachable from `extra_roots` (the eval's `expr`),
    /// `extra_envs` (its `env`), the dynamic stack, and the explicit root stack into
    /// fresh slabs; drop the rest; bump the generation epoch so any handle held
    /// across this without being re-rooted trips the tripwire at its next deref.
    ///
    /// Because it MOVES survivors, the caller **must** use the relocated handles
    /// written back into `extra_roots`/`extra_envs`. Recomputes the adaptive
    /// threshold so the next collection fires when the live set doubles (amortized
    /// O(1) copying per allocation — standard semi-space; the threshold is the
    /// slow/stable dial, `BROOD_GC_STRESS=1` ⇒ every safepoint). No-op while GC is
    /// disabled (the builder heap during prelude construction). Shares all of its
    /// machinery — and the no-slot-reuse safety — with the [`flush`](Self::flush) helper.
    // (stall_guard defined at module scope, below)
    pub fn collect(&mut self, extra_roots: &mut [Value], extra_envs: &mut [EnvId]) {
        // Pause-duration accounting (the observability timing tier): time the
        // whole collection and fold it into the per-process totals `(gc-stats)`
        // reports. Only recorded when a collection actually ran (`gc_runs`
        // moved) — a gated no-op call isn't a pause. Two `Instant` reads per
        // collection: noise against the collection itself.
        let runs_before = self.gc_runs;
        let t0 = std::time::Instant::now();
        self.collect_inner(extra_roots, extra_envs);
        if self.gc_runs != runs_before {
            let ns = t0.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            self.gc_ns_total = self.gc_ns_total.saturating_add(ns);
            self.gc_ns_max = self.gc_ns_max.max(ns);
            self.gc_ns_last = ns;
            // System-monitor GC event (the observability event stream). Emitted
            // *after* the collection completes — the heap is consistent and the
            // event build/deliver touches only Rust data, never this heap. The
            // subscriber's own collections are excluded inside emit_gc (its
            // event traffic would otherwise re-trigger itself forever).
            if crate::process::sysmon::armed() {
                if let Some(pid) = crate::process::current_pid() {
                    crate::process::sysmon::emit_gc(
                        pid,
                        ns,
                        self.gc_runs,
                        self.local_live_count() as u64,
                    );
                }
            }
        }
    }

    fn collect_inner(&mut self, extra_roots: &mut [Value], extra_envs: &mut [EnvId]) {
        // Stall trace (BROOD_STALL_MS=<n>): log if this minor collection takes ≥ n ms — to
        // pinpoint a gameplay lag spike. Works in release; zero cost unless the env is set.
        let _sg = stall_guard("minor-gc");
        // GC trace (BROOD_GC_TRACE=1): log each minor collection's working set.
        #[cfg(debug_assertions)]
        {
            static GC_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *GC_TRACE.get_or_init(|| {
                std::env::var("BROOD_GC_TRACE").is_ok_and(|v| v != "0" && !v.is_empty())
            }) {
                eprintln!(
                    "[gc-trace] collect: nursery pairs={} vecs={} strs={} envs={} closures={} | old pairs={} vecs={}",
                    self.local.pairs.len(),
                    self.local.vectors.len(),
                    self.local.strings.len(),
                    self.local.envs.len(),
                    self.local.closures.len(),
                    self.old.pairs.len(),
                    self.old.vectors.len(),
                );
            }
        }
        if !self.gc_enabled {
            return;
        }
        // DEBUG (bug #2): scan every roots slot for an OOB/garbage handle at each collection —
        // catches WHICH slot holds garbage and WHEN (the collection right after it's written),
        // independent of where it's later deref'd. Gated by BROOD_GC_VERIFY.
        #[cfg(debug_assertions)]
        if Self::gc_verify_enabled() {
            let n = self.roots.len();
            for i in 0..n {
                let v = self.roots[i];
                if let Some((kind, idx, len)) = self.dbg_value_oob(v) {
                    eprintln!(
                        "[roots-garbage] roots[{i}] = OOB {kind} idx={idx} slab_len={len} \
                         (roots_len={n}) jit_native_depth={} arm='{}'",
                        self.jit_native_depth,
                        crate::core::value::symbol_name_opt(self.jit_dbg_fn).unwrap_or("<none>"),
                    );
                }
            }
        }
        // `BROOD_GC_VERIFY=1` (debug only): before flipping, walk the whole
        // reachable LOCAL graph and assert every handle is in-bounds and
        // current-epoch. Catches a *stored* stale handle (a missed root whose
        // value was written into a heap cell) right here — with the root→…→cell
        // path — instead of letting it surface far away as an OOB index or a
        // `promote` stack overflow. See `verify_local_graph`.
        if Self::gc_verify_enabled() {
            self.verify_local_graph(extra_roots, extra_envs);
        }
        // Generational: a *minor* collection either tenures the nursery's
        // survivors into the old gen (when the nursery grew past `min_tenure` —
        // real allocation pressure, so survivors are probably long-lived) or does
        // a young semi-space flip (survivors stay young) when this is a premature
        // collection. The flip is what keeps `BROOD_GC_STRESS` (a minor at every
        // safepoint) from tenuring transient garbage. Either way it reclaims dead
        // nursery objects and never recopies the tenured old gen.
        let tenure = self.local_live_count() >= min_tenure();
        self.minor_collect(tenure, extra_roots, extra_envs);
        // Next minor fires when the *young* gen reaches `gc_threshold`. Scale it with
        // the **total** live set (young + old), not just young: a tenuring build moves
        // its survivors to the old gen, leaving young ≈ 0, so a young-only `live*2`
        // collapsed to the floor and re-collected every floor-worth of allocations —
        // O(n/floor) minor collects (and majors) while building one large structure.
        // Counting old-gen live lets a process with a big live set earn a
        // proportionally bigger nursery budget (memory bounded to ~a small multiple of
        // live), so large-structure builds collect O(log n) times; a small-live churny
        // process (e.g. a `spawn` fan-out worker) still sits at the floor. (2026-07-01)
        // Capped at NURSERY_MAX: `should_collect` fires a minor when *young* reaches
        // `gc_threshold`, so without a ceiling a process with a large live old gen that
        // then *churns* transient young garbage would buffer ~2×old worth of it before
        // collecting — young memory ballooning proportional to the old gen. The cap
        // bounds that transient buffer while staying well above real build working sets
        // (a lone process's floor is 64K; the cap is 8M ≈ a few hundred MB of nursery).
        let live_total = self.local_live_count() + self.old_live_count();
        self.gc_threshold =
            std::cmp::max(gc_floor(), live_total.saturating_mul(2).min(NURSERY_MAX));
        // Escalate to a *major* (compact the old generation) only when it has grown
        // MAJOR_GROWTH× since the last major — so majors stay rare while minors keep
        // the nursery bounded. Grown 2×→4× (2026-07-01): during a large-structure
        // build the old gen is nearly all-live, so a major copies the whole growing
        // list and reclaims almost nothing; a larger factor makes those wasteful
        // full-list compactions far rarer (fewer, at geometrically spaced sizes),
        // trading retained-garbage memory for copy throughput.
        if self.old_live_count() >= self.major_threshold {
            self.major_collect(extra_roots, extra_envs);
            self.major_threshold = std::cmp::max(
                major_floor(),
                self.old_live_count().saturating_mul(major_growth()),
            );
        }
    }

    /// Live objects in the **old generation** (`Σ old.slab.len()`). Old is
    /// append-only between major collections, so the slab lengths *are* the
    /// live count. Drives the major-collection threshold.
    pub fn old_live_count(&self) -> usize {
        slab_live_count(&self.old)
    }

    /// A **minor collection**. `tenure` selects the destination of the nursery's
    /// survivors:
    /// - `true` (allocation pressure crossed `min_tenure`): survivors are copied
    ///   into the **old** generation (tenured) — old objects are left in place,
    ///   never recopied, which is the generational win.
    /// - `false` (a premature/stress collection): survivors are copied into a
    ///   **fresh nursery** (a young semi-space flip) and stay young, so transient
    ///   garbage never reaches the old gen.
    ///
    /// Either way the dead nursery objects are reclaimed by dropping the source
    /// nursery whole, and the nursery epoch is bumped (stale young handles trip the
    /// tripwire). Roots, dynamics, the operand stack, and the write-barrier
    /// remembered set are relocated/rewritten in place.
    /// Relocate every GC root through `fwd`, from `src` into `dest`: the caller's
    /// `value_roots`/`env_roots` (the eval frame's `expr`/`env`), this process's
    /// dynamic-binding stack, and the operand stack (`roots` + `env_roots`). The
    /// single place the GC root set is enumerated — minor and major collection
    /// share it so the two can't drift (a divergent root set would be a
    /// use-after-GC bug). `dest` is a *local* `Slabs` (never a `self` field) so the
    /// `&mut self` for the stacks doesn't alias it.
    fn flush_roots(
        &mut self,
        src: &Slabs,
        dest: &mut Slabs,
        fwd: &mut FlushForward,
        value_roots: &mut [Value],
        env_roots: &mut [EnvId],
    ) {
        for v in value_roots.iter_mut() {
            *v = flush_value(src, dest, fwd, *v);
        }
        for e in env_roots.iter_mut() {
            *e = flush_env(src, dest, fwd, *e);
        }
        for (_, v) in self.dynamics.iter_mut() {
            *v = flush_value(src, dest, fwd, *v);
        }
        #[cfg(feature = "dev-tools")]
        if let Some(v) = &mut self.trace_context {
            *v = flush_value(src, dest, fwd, *v);
        }
        for v in self.roots.iter_mut() {
            *v = flush_value(src, dest, fwd, *v);
        }
        let mut er = std::mem::take(&mut self.env_roots);
        for e in er.iter_mut() {
            *e = flush_env(src, dest, fwd, *e);
        }
        self.env_roots = er;
    }

    fn minor_collect(&mut self, tenure: bool, value_roots: &mut [Value], env_roots: &mut [EnvId]) {
        let before_young = self.local_live_count();
        let old_before = self.old_live_count();
        self.local_epoch = self.local_epoch.wrapping_add(1);
        let young = std::mem::take(&mut self.local);
        // Tenure: append survivors to the old gen (take it out, append, put back).
        // Flip: survivors go to a fresh nursery that becomes the new `local`.
        let (mut dest, epoch, dest_old) = if tenure {
            (std::mem::take(&mut self.old), self.old_epoch, true)
        } else {
            // Flip: seed the fresh nursery with the outgoing one's capacity so
            // neither the survivor copy nor the next cycle's allocations re-pay
            // the Vec-doubling ladder (see `Slabs::with_capacity_like`).
            (Slabs::with_capacity_like(&young), self.local_epoch, false)
        };
        let mut fwd = FlushForward::for_source(&young);
        fwd.epoch = epoch;
        fwd.src_old = false; // copy nursery objects
        fwd.dest_old = dest_old;
        self.flush_roots(&young, &mut dest, &mut fwd, value_roots, env_roots);
        // Write barrier: an old frame that gained a young binding (`env_define`
        // after a mid-bind tenure) holds an OLD->YOUNG edge not reachable from the
        // normal roots. Its frame lives in `dest` while tenuring (we took the old
        // gen into `dest`) or in `self.old` while flipping (old untouched). Flush
        // each such var into `dest` and write it back.
        let remembered = std::mem::take(&mut self.remembered);
        for &e in &remembered {
            let n = if tenure {
                dest.envs[e.index()].vars.len()
            } else {
                self.old.envs[e.index()].vars.len()
            };
            for i in 0..n {
                let (s, v) = if tenure {
                    dest.envs[e.index()].vars[i]
                } else {
                    self.old.envs[e.index()].vars[i]
                };
                let nv = flush_value(&young, &mut dest, &mut fwd, v);
                if tenure {
                    dest.envs[e.index()].vars[i] = (s, nv);
                } else {
                    self.old.envs[e.index()].vars[i] = (s, nv);
                }
            }
        }
        // Tenuring resolves those edges to old->old (survivors are now old): drop
        // the set. A flip keeps survivors young, so the old->young edges persist —
        // retain the set (the frames didn't move) for the next collection.
        if !tenure {
            self.remembered = remembered;
        }
        // form_pos re-key: a surviving nursery pair moves to its new slot with the
        // destination's age bit (old when tenuring, young when flipping); dead
        // nursery entries drop; existing OLD entries are untouched (old didn't move
        // in a minor).
        let new_age_bit: u64 = if tenure { 1 << 32 } else { 0 };
        let old_form_pos = std::mem::take(&mut self.form_pos);
        for (key, pos) in old_form_pos {
            if (key >> 32) & 1 == 1 {
                self.form_pos.insert(key, pos);
            } else if let Some(new_idx) = fwd.pairs.lookup(key as u32) {
                self.form_pos.insert((new_idx as u64) | new_age_bit, pos);
            }
        }
        // Install the relocated space. Tenure: `dest` is the grown old gen; the
        // nursery restarts empty but with the outgoing nursery's capacity (same
        // doubling-ladder rationale as the flip path). Flip: `dest` is the fresh
        // nursery; the old gen was untouched.
        if tenure {
            self.old = dest;
            self.local = Slabs::with_capacity_like(&young);
        } else {
            self.local = dest;
        }
        let survivors = if tenure {
            self.old_live_count().saturating_sub(old_before)
        } else {
            self.local_live_count()
        };
        self.gc_runs = self.gc_runs.saturating_add(1);
        self.gc_copied = self.gc_copied.saturating_add(survivors as u64);
        self.gc_reclaimed = self
            .gc_reclaimed
            .saturating_add(before_young.saturating_sub(survivors) as u64);
        self.note_proc_limit();
        if self.gc_trace {
            eprintln!(
                "[gc] minor {}: {} nursery objects, {} {}, {} reclaimed",
                if tenure { "tenure" } else { "flip" },
                before_young,
                survivors,
                if tenure { "tenured" } else { "kept young" },
                before_young.saturating_sub(survivors),
            );
        }
        // `young` drops here, reclaiming every nursery object that didn't survive.
    }

    /// A **major collection**: compact the old generation (a semi-space copy of
    /// `old` into fresh `old` slabs, dropping dead tenured objects). The preceding
    /// minor may have been a flip (not a tenure), so the nursery may be non-empty;
    /// `flush_nursery_old_refs` handles the resulting nursery→old edges. Bumps the
    /// old epoch.
    fn major_collect(&mut self, value_roots: &mut [Value], env_roots: &mut [EnvId]) {
        let before_old = self.old_live_count();
        self.old_epoch = self.old_epoch.wrapping_add(1);
        let old_src = std::mem::take(&mut self.old);
        let mut dest = Slabs::default();
        let mut fwd = FlushForward::for_source(&old_src);
        fwd.epoch = self.old_epoch;
        fwd.src_old = true; // copy old-gen objects
        fwd.dest_old = true; // into the fresh old space
        self.flush_roots(&old_src, &mut dest, &mut fwd, value_roots, env_roots);
        // If the preceding minor was a flip (not a tenure) the nursery is
        // non-empty.  `flush_roots` updated handles in `self.roots` and
        // `self.env_roots`, but OLD handles *inside* nursery objects were
        // silently skipped by `flush_value`/`flush_env` (they gate on
        // `fwd.copies`, which is false for nursery objects during a major).
        // Rewrite those stale OLD handles in-place now, while `old_src` is
        // still live.
        flush_nursery_old_refs(&mut self.local, &old_src, &mut dest, &mut fwd);
        // Write barrier across a major. After a *tenure* minor `remembered` is
        // empty (the minor cleared it). But `collect()` can run a major right
        // after a *flip* minor, and a flip RETAINS `remembered` — old EnvIds for
        // frames that gained a young binding, pointing into the pre-compaction old
        // gen (the old->young edges persist; see `minor_collect`). This major just
        // relocated those frames into fresh slabs and bumped `old_epoch`, so every
        // retained entry is now a stale index *and* a stale epoch. Rewrite each
        // through the env forwarding table (`fwd.envs`, populated by `flush_roots`)
        // and drop any whose frame wasn't copied — it was unreachable, so the major
        // reclaimed it. Skipping this leaves the next `minor_collect` indexing
        // `self.old.envs[e.index()]` with a stale handle and no bounds/epoch check
        // (and `BROOD_GC_VERIFY`'s remembered walk uses a safe `.get()`, so it
        // never flags it) — a silent use-after-GC.
        if !self.remembered.is_empty() {
            let remembered = std::mem::take(&mut self.remembered);
            self.remembered = remembered
                .into_iter()
                .filter_map(|e| {
                    fwd.envs
                        .lookup(e.index() as u32)
                        .map(|n| fwd.mint_env(n as usize))
                })
                .collect();
        }
        let old_form_pos = std::mem::take(&mut self.form_pos);
        for (key, pos) in old_form_pos {
            if (key >> 32) & 1 == 1 {
                if let Some(new_idx) = fwd.pairs.lookup(key as u32) {
                    self.form_pos.insert((new_idx as u64) | (1 << 32), pos);
                }
            }
        }
        self.old = dest;
        let survivors = self.old_live_count();
        self.gc_runs = self.gc_runs.saturating_add(1);
        self.gc_copied = self.gc_copied.saturating_add(survivors as u64);
        self.gc_reclaimed = self
            .gc_reclaimed
            .saturating_add(before_old.saturating_sub(survivors) as u64);
        if self.gc_trace {
            eprintln!(
                "[gc] major: {} old objects, {} survived, {} reclaimed",
                before_old,
                survivors,
                before_old.saturating_sub(survivors),
            );
        }
        // `old_src` drops here, releasing the pre-compaction old slabs.
    }

    /// Is the `BROOD_GC_VERIFY` heap-verifier armed? Read once. Available in release too
    /// (gated by the env flag) so a stored-stale-handle (bug #2 class) can be caught in a
    /// normal `--release` binary without a debug-assertions rebuild — O(live) per collection
    /// only when the flag is set.
    fn gc_verify_enabled() -> bool {
        use std::sync::OnceLock;
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("BROOD_GC_VERIFY").is_some())
    }

    /// Debug heap verifier (`BROOD_GC_VERIFY`). Walk every LOCAL handle reachable
    /// from the supplied roots + the explicit root / env-root / dynamic stacks and
    /// assert each is (a) in-bounds for its slab and (b) stamped with the current
    /// epoch. Between collections every *live* LOCAL handle must be current-epoch
    /// (survivors are re-minted at the current epoch on each flip, new allocations
    /// use it), so a reachable handle from an older epoch means it was held across
    /// an earlier collection without being re-rooted and then **stored into the
    /// live graph** — the use-after-GC class the per-deref tripwire misses because
    /// the bad handle is written, not dereferenced. Panics with the
    /// root→…→containing-cell path so the offending structure (hence the missed
    /// rooting site) is obvious. O(live); only runs under the env flag. Available in
    /// release (gated by `gc_verify_enabled`) — see its note.
    fn verify_local_graph(&self, extra_roots: &[Value], extra_envs: &[EnvId]) {
        // Allocation-light: the worklist carries only Copy handles plus the raw
        // handle of the containing cell (`parent`, `0` = a root). No per-node
        // `String` paths — this runs at *every* safepoint under GC_STRESS, so it
        // must not itself churn the heap. On a hit we panic with the bad handle and
        // its immediate container, which (with the offending op's `expr`) pinpoints
        // the missed-rooting site.
        enum W {
            V(Value, u64),
            E(EnvId, u64),
        }
        // Generational: a LOCAL handle is checked against its own generation's
        // epoch + slab length (nursery via `is_old()==false`, old otherwise). The
        // seen-sets are `[young, old]` bool vecs per kind (O(1) mark, not a
        // `HashSet` — this runs every collection under GC_VERIFY, so it must not be
        // the bottleneck on a large live graph). We do *not* assert the no-old→young
        // invariant here — the write-barrier `remembered` set legitimately carries
        // transient old→young edges between a tenure-mid-bind and the next minor —
        // only that every reachable handle is in-bounds and current for its gen.
        // Truncated like `check_epoch_aged` — see `epoch_in_gen_width`.
        let young_ep = Self::epoch_in_gen_width(self.local_epoch);
        let old_ep = Self::epoch_in_gen_width(self.old_epoch);
        let mut seen_pair = [
            vec![false; self.local.pairs.len()],
            vec![false; self.old.pairs.len()],
        ];
        let mut seen_vec = [
            vec![false; self.local.vectors.len()],
            vec![false; self.old.vectors.len()],
        ];
        let mut seen_map = [
            vec![false; self.local.maps.len()],
            vec![false; self.old.maps.len()],
        ];
        let mut seen_clo = [
            vec![false; self.local.closures.len()],
            vec![false; self.old.closures.len()],
        ];
        let mut seen_env = [
            vec![false; self.local.envs.len()],
            vec![false; self.old.envs.len()],
        ];
        let mut work: Vec<W> = Vec::new();
        for &v in extra_roots {
            work.push(W::V(v, 0));
        }
        for &e in extra_envs {
            work.push(W::E(e, 0));
        }
        for &v in &self.roots {
            work.push(W::V(v, 0));
        }
        for &e in &self.env_roots {
            work.push(W::E(e, 0));
        }
        for &(_, v) in &self.dynamics {
            work.push(W::V(v, 0));
        }
        #[cfg(feature = "dev-tools")]
        if let Some(v) = self.trace_context {
            work.push(W::V(v, 0));
        }
        // The write-barrier `remembered` old frames are the *only* mutable old
        // objects (they gained young bindings after tenuring). Seed their bindings
        // as roots so a stale handle stored there is still checked, even though the
        // walk below doesn't recurse into old-gen internals (see the `is_old`
        // guards): old objects are immutable after promotion, so re-walking them
        // every collection is redundant work — that redundancy is what made
        // GC_VERIFY O(old) per collection and timed out the large-structure tests.
        for &e in &self.remembered {
            if e.is_old() {
                if let Some(frame) = self.old.envs.get(e.index()) {
                    for &(_, v) in &frame.vars {
                        work.push(W::V(v, e.0));
                    }
                }
            }
        }
        let bad =
            |kind: &str, is_old: bool, gen: u32, idx: usize, len: usize, parent: u64, raw: u64| {
                let (ep, space) = if is_old {
                    (old_ep, "OLD")
                } else {
                    (young_ep, "nursery")
                };
                assert!(
                    idx < len,
                    "GC-VERIFY: stored stale {kind} handle OUT OF BOUNDS ({space} slot {idx} \
                 ≥ slab len {len}); handle {raw:#x} held in container {parent:#x}. \
                 A handle was kept across a collection without re-rooting, then \
                 written into the live graph — use-after-GC.",
                );
                assert!(
                gen == ep,
                "GC-VERIFY: stored stale {kind} handle from epoch {gen}, {space} generation is \
                 now epoch {ep} (slot {idx}, handle {raw:#x}); held in container \
                 {parent:#x}. That cell holds a handle kept across a collection \
                 without re-rooting — use-after-GC at the op that built it.",
            );
            };
        // Routed slab views: young vs old by the handle's age bit.
        while let Some(w) = work.pop() {
            match w {
                W::V(v, parent) => match v.unpack() {
                    ValueRef::Pair(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "pair",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.pairs.len(),
                            parent,
                            id.0,
                        );
                        if !id.is_old()
                            && !std::mem::replace(
                                &mut seen_pair[id.is_old() as usize][id.index()],
                                true,
                            )
                        {
                            let (a, b) = slabs.pairs[id.index()];
                            work.push(W::V(a, id.0));
                            work.push(W::V(b, id.0));
                        }
                    }
                    ValueRef::Vector(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "vector",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.vectors.len(),
                            parent,
                            id.0,
                        );
                        if !id.is_old()
                            && !std::mem::replace(
                                &mut seen_vec[id.is_old() as usize][id.index()],
                                true,
                            )
                        {
                            for &el in slabs.vectors[id.index()].iter() {
                                work.push(W::V(el, id.0));
                            }
                        }
                    }
                    // A range's backing vector holds only ints — validate the
                    // handle itself (bounds + epoch), nothing to descend into.
                    ValueRef::Range(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "range",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.vectors.len(),
                            parent,
                            id.0,
                        );
                    }
                    // A seq-view's backing `[source xform]` holds heap values, so
                    // validate the handle then descend into them — same as a
                    // vector (it shares the vectors slab, so it dedups via
                    // `seen_vec`).
                    ValueRef::SeqView(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "seq-view",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.vectors.len(),
                            parent,
                            id.0,
                        );
                        if !id.is_old()
                            && !std::mem::replace(
                                &mut seen_vec[id.is_old() as usize][id.index()],
                                true,
                            )
                        {
                            for &el in slabs.vectors[id.index()].iter() {
                                work.push(W::V(el, id.0));
                            }
                        }
                    }
                    ValueRef::Map(id) | ValueRef::Set(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "map",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.maps.len(),
                            parent,
                            id.0,
                        );
                        if !id.is_old()
                            && !std::mem::replace(
                                &mut seen_map[id.is_old() as usize][id.index()],
                                true,
                            )
                        {
                            let node = &slabs.maps[id.index()];
                            for &(mk, mv) in &node.data {
                                work.push(W::V(mk, id.0));
                                work.push(W::V(mv, id.0));
                            }
                            for &c in &node.children {
                                work.push(W::V(Value::map(c), id.0));
                            }
                        }
                    }
                    ValueRef::Str(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "string",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.strings.len(),
                            parent,
                            id.0,
                        );
                    }
                    ValueRef::BigInt(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "bigint",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.bigints.len(),
                            parent,
                            id.0,
                        );
                    }
                    ValueRef::Decimal(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "decimal",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.decimals.len(),
                            parent,
                            id.0,
                        );
                    }
                    ValueRef::Bytes(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "bytes",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.bytes.len(),
                            parent,
                            id.0,
                        );
                    }
                    ValueRef::Rope(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "rope",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.ropes.len(),
                            parent,
                            id.0,
                        );
                    }
                    ValueRef::Fn(id) | ValueRef::Macro(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad(
                            "closure",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.closures.len(),
                            parent,
                            id.0,
                        );
                        if !id.is_old()
                            && !std::mem::replace(
                                &mut seen_clo[id.is_old() as usize][id.index()],
                                true,
                            )
                        {
                            let cl = &slabs.closures[id.index()];
                            for arm in cl.arms.iter() {
                                for &f in &arm.body {
                                    work.push(W::V(f, id.0));
                                }
                                for &(_, d) in &arm.optionals {
                                    work.push(W::V(d, id.0));
                                }
                            }
                            if let Some(e) = cl.env {
                                work.push(W::E(e, id.0));
                            }
                        }
                    }
                    _ => {}
                },
                W::E(e, parent) => {
                    if e == EnvId::GLOBAL || e.region() != LOCAL {
                        continue;
                    }
                    let slabs = if e.is_old() { &self.old } else { &self.local };
                    bad(
                        "env",
                        e.is_old(),
                        e.generation(),
                        e.index(),
                        slabs.envs.len(),
                        parent,
                        e.0,
                    );
                    if !e.is_old()
                        && !std::mem::replace(&mut seen_env[e.is_old() as usize][e.index()], true)
                    {
                        let frame = &slabs.envs[e.index()];
                        if let Some(p) = frame.parent {
                            work.push(W::E(p, e.0));
                        }
                        for &(_, val) in &frame.vars {
                            work.push(W::V(val, e.0));
                        }
                    }
                }
            }
        }
    }

    /// The relocated handle of the `i`th explicit root (see [`push_root`]). Read
    /// back by the form-loops in `Interp::eval_str`/`eval_source` after each form:
    /// a collection during form `i` relocates the LOCAL forms `i+1..` that those
    /// loops pushed as roots, so their own `Vec` copies are stale — this returns
    /// the current handle from the (relocated) root stack instead.
    ///
    /// [`push_root`]: Self::push_root
    pub fn root_at(&self, i: usize) -> Value {
        self.roots[i]
    }

    /// Overwrite the operand-stack slot at `i` (the VM uses this to write a
    /// computed `let` binding into its frame slot — ADR-076 Stage 2). The slot is
    /// already a tracked root, so the value is relocated by `arena_flip` like any
    /// other; writing it is a plain `Vec` store.
    pub fn set_root_at(&mut self, i: usize, v: Value) {
        self.roots[i] = v;
    }
}

// ----- heap flush (arena flip / Phase 2) -----------------------------------
//
// The standalone deep-copy that backs [`Heap::flush`]. Free functions so the
// recursion borrows `&old` immutably and `&mut new` mutably without tangling
// with the `Heap`'s `&mut self`. Cycles are handled with a per-slab
// forwarding table: when a node is visited, we reserve a placeholder slot
// in `new` and record `old_idx → new_idx` before recursing into its
// children — a second hit on the same old handle returns the placeholder
// instead of re-traversing.

/// Forwarding table for one slab kind: **source slab index → destination slab index**.
///
/// A dense `Vec` rather than the `HashMap<u32, u32>` this replaced. The keys are slab
/// indices — already dense, already bounded by the source slab's length — so hashing them
/// was pure overhead, and it was the dominant cost of collection: every copied object paid
/// a SipHash probe plus an insert, and the insert rehashed as the table grew into the
/// hundreds of thousands. Measured on the benchmark suite's `sort` row (375k-cell build +
/// sort, 2026-07-28): 4 collections copying 946k objects spent **95.7 ms of the row's
/// 158 ms** in GC pause — 101 ns per copied object, ~300 cycles to move 48 bytes. An array
/// index is a few of those cycles.
///
/// `NONE` marks "not yet copied". Sized from the source slab up front (see
/// [`FlushForward::for_source`]) so the common path is a bounds-checked load and a store,
/// with `set` growing defensively only if an index ever lands past the end.
#[derive(Default)]
struct FwdTable {
    to: Vec<u32>,
}

impl FwdTable {
    /// Sentinel for an entry that has not been copied yet. A real destination index can
    /// never reach `u32::MAX` — the slab would have to hold 4 G objects first.
    const NONE: u32 = u32::MAX;

    /// A table covering `len` source slots, all unset.
    fn with_len(len: usize) -> Self {
        Self {
            to: vec![Self::NONE; len],
        }
    }

    /// The destination index `src` was copied to, or `None` if it has not been copied.
    #[inline]
    fn lookup(&self, src: u32) -> Option<u32> {
        match self.to.get(src as usize) {
            Some(&d) if d != Self::NONE => Some(d),
            _ => None,
        }
    }

    /// Record that source index `src` now lives at destination index `dst`.
    #[inline]
    fn set(&mut self, src: u32, dst: u32) {
        let i = src as usize;
        if i >= self.to.len() {
            // Only reachable if a handle points past the source slab's length, which the
            // region/age guards should already exclude — grow rather than panic, and keep
            // the growth amortized so a run of them can't go quadratic.
            self.to.resize((i + 1).max(self.to.len() * 2), Self::NONE);
        }
        self.to[i] = dst;
    }
}

#[derive(Default)]
struct FlushForward {
    /// The generation epoch to stamp into every survivor handle minted into the
    /// destination slabs. Carried here rather than threaded through every
    /// `flush_*` signature.
    epoch: u32,
    /// Which generation the *source* objects being copied live in: `false` =
    /// nursery (a minor or legacy whole-heap flush), `true` = old (a major
    /// compaction). A `flush_*` copies a LOCAL handle only when its age matches;
    /// the other generation (and PRELUDE/RUNTIME) is left untouched.
    src_old: bool,
    /// Whether minted destination handles are tagged **old** (`local_old_gen`).
    /// `true` for the generational paths (minor promotes nursery→old, major
    /// compacts old→old); `false` only for the legacy single-space `flush()` test
    /// helper, which stays nursery→nursery.
    dest_old: bool,
    pairs: FwdTable,
    vectors: FwdTable,
    maps: FwdTable,
    strings: FwdTable,
    bigints: FwdTable,
    decimals: FwdTable,
    bytes: FwdTable,
    ropes: FwdTable,
    closures: FwdTable,
    envs: FwdTable,
}

impl FlushForward {
    /// A forwarding set sized for `src` — the generation this collection copies *out of*.
    ///
    /// Every forwarding key is an index into one of `src`'s slabs, so each table is
    /// allocated at exactly that slab's length and indexed directly. Sizing up front costs
    /// one `memset` per non-empty slab and removes hashing from the copy path entirely.
    fn for_source(src: &Slabs) -> Self {
        Self {
            epoch: 0,
            src_old: false,
            dest_old: false,
            pairs: FwdTable::with_len(src.pairs.len()),
            vectors: FwdTable::with_len(src.vectors.len()),
            maps: FwdTable::with_len(src.maps.len()),
            strings: FwdTable::with_len(src.strings.len()),
            bigints: FwdTable::with_len(src.bigints.len()),
            decimals: FwdTable::with_len(src.decimals.len()),
            bytes: FwdTable::with_len(src.bytes.len()),
            ropes: FwdTable::with_len(src.ropes.len()),
            closures: FwdTable::with_len(src.closures.len()),
            envs: FwdTable::with_len(src.envs.len()),
        }
    }

    /// Does a `flush_*` copy this LOCAL handle? Only if its generation age matches
    /// the source space being collected; the other generation / shared regions are
    /// left in place.
    #[inline]
    fn copies(&self, region: u8, is_old: bool) -> bool {
        region == LOCAL && is_old == self.src_old
    }
}

/// Generate a `FlushForward::mint_*` that mints a destination handle of type `$id`,
/// tagged old or young by `dest_old` and stamped with the dest `epoch`. One per
/// handle kind — they differ only in the `Id` type.
macro_rules! mint_fn {
    ($name:ident, $id:ty) => {
        impl FlushForward {
            #[inline]
            fn $name(&self, idx: usize) -> $id {
                if self.dest_old {
                    <$id>::local_old_gen(idx, self.epoch)
                } else {
                    <$id>::local_gen(idx, self.epoch)
                }
            }
        }
    };
}
mint_fn!(mint_pair, PairId);
mint_fn!(mint_vector, VecId);
mint_fn!(mint_map, MapId);
mint_fn!(mint_string, StrId);
mint_fn!(mint_bigint, BigIntId);
mint_fn!(mint_decimal, DecimalId);
mint_fn!(mint_bytes, BytesId);
mint_fn!(mint_rope, RopeId);
mint_fn!(mint_closure, ClosureId);
mint_fn!(mint_env, EnvId);

/// Cold diagnostic for the GC copy phase: a LOCAL handle reachable from the GC
/// roots whose `index()` is past the **source** slab it would be copied from.
/// [`FlushForward::copies`] admits a handle by region + generation-age but *not*
/// by slab bound, so a stale (use-after-GC), foreign, or mis-tagged handle that
/// slips into the root set indexes the source slab out of bounds here. Rather
/// than the bare `Vec` slice panic — an opaque `index out of bounds` with no
/// provenance, and `<unknown>` frames in a release backtrace — name the handle
/// directly: kind, region, age, epoch, index, slab length, and which space this
/// pass collects. See the GC slab-OOB investigation (`docs/known-issues.md` KI-2,
/// `docs/concurrency-v2.md`).
#[cold]
#[inline(never)]
fn flush_oob(
    kind: &str,
    region: u8,
    is_old: bool,
    epoch: u32,
    idx: usize,
    len: usize,
    src_old: bool,
) -> ! {
    panic!(
        "GC flush: {kind} handle indexes the source slab out of bounds — \
         region={region} age={age} epoch={epoch} index={idx} slab_len={len}, \
         collecting {space}. A handle reachable from the GC roots is not a live \
         this-pass object (missed rooting / use-after-GC / foreign handle). \
         Re-run with BROOD_GC_VERIFY=1 for the root→cell path.",
        age = if is_old { "old" } else { "young" },
        space = if src_old {
            "old-gen (major)"
        } else {
            "nursery (minor)"
        },
    );
}

/// Bounds-check a source-slab index during a flush, returning the index or
/// calling [`flush_oob`] with the handle's full provenance. Used in place of a
/// bare `slab[id.index()]` at every `flush_*` source access (the handle types
/// share `index`/`region`/`is_old`/`generation` but no trait, hence a macro).
macro_rules! flush_bound {
    ($slab:expr, $id:expr, $fwd:expr, $kind:literal) => {{
        let idx = $id.index();
        let len = $slab.len();
        if idx >= len {
            flush_oob(
                $kind,
                $id.region(),
                $id.is_old(),
                $id.generation(),
                idx,
                len,
                $fwd.src_old,
            );
        }
        idx
    }};
}

/// Gameplay-lag diagnostic. `BROOD_STALL_MS=<n>`: anything wrapped in a `stall_guard`
/// that runs ≥ n ms logs `[stall] <label> Nms` to stderr. Release-capable; zero cost
/// (no `Instant`) unless the env is set. Used to pinpoint a long pause (GC vs compaction
/// vs elsewhere) in a live session that can't be driven headless.
pub(crate) fn stall_threshold_ms() -> Option<u128> {
    static MS: std::sync::OnceLock<Option<u128>> = std::sync::OnceLock::new();
    *MS.get_or_init(|| {
        std::env::var("BROOD_STALL_MS")
            .ok()
            .and_then(|v| v.parse().ok())
    })
}

pub(crate) struct StallGuard {
    label: &'static str,
    pid: Option<u64>,
    t0: std::time::Instant,
    ms: u128,
}
impl Drop for StallGuard {
    fn drop(&mut self) {
        let el = self.t0.elapsed().as_millis();
        if el >= self.ms {
            match self.pid {
                Some(p) => eprintln!("[stall] {} (pid {}) took {}ms", self.label, p, el),
                None => eprintln!("[stall] {} took {}ms", self.label, el),
            }
        }
    }
}
/// A pid-less stall guard for a non-process work span (GC compaction/minor
/// collection, and the GUI paint path). The `heap::stall_guard` re-export is
/// gated on the `gui` feature since only `gui.rs` reaches it that way.
pub(crate) fn stall_guard(label: &'static str) -> Option<StallGuard> {
    stall_threshold_ms().map(|ms| StallGuard {
        label,
        pid: None,
        t0: std::time::Instant::now(),
        ms,
    })
}
pub(crate) fn stall_guard_pid(label: &'static str, pid: u64) -> Option<StallGuard> {
    stall_threshold_ms().map(|ms| StallGuard {
        label,
        pid: Some(pid),
        t0: std::time::Instant::now(),
        ms,
    })
}

fn flush_value(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, v: Value) -> Value {
    // Deep-car-nesting guard — see `WALKER_RED_ZONE`. The GC copies live values
    // at every collection, so a deep value must survive the walk regardless of
    // how much native stack the collecting thread has left.
    stacker::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
        flush_value_grown(old, new, fwd, v)
    })
}

fn flush_value_grown(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, v: Value) -> Value {
    match v.unpack() {
        ValueRef::Pair(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::pair(flush_pair(old, new, fwd, id))
        }
        ValueRef::Vector(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::vector(flush_vector(old, new, fwd, id))
        }
        // A range is backed by a `[lo hi step]` vector — forward it exactly like
        // a vector, keeping the `Range` wrapper.
        ValueRef::Range(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::range(flush_vector(old, new, fwd, id))
        }
        // A seq-view is backed by a `[source xform]` vector — `flush_vector`
        // recurses into the elements (forwarding the source + transducer), so
        // forward it like a vector and keep the `SeqView` wrapper.
        ValueRef::SeqView(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::seqview(flush_vector(old, new, fwd, id))
        }
        ValueRef::Map(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::map(flush_map(old, new, fwd, id))
        }
        // A set is backed by the same CHAMP storage as a map — forward it via
        // `flush_map` and keep the `Set` wrapper (mirrors the `SeqView` case above).
        ValueRef::Set(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::set(flush_map(old, new, fwd, id))
        }
        ValueRef::Str(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::str_(flush_string(old, new, fwd, id))
        }
        ValueRef::BigInt(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::bigint(flush_bigint(old, new, fwd, id))
        }
        ValueRef::Decimal(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::decimal(flush_decimal(old, new, fwd, id))
        }
        ValueRef::Bytes(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::bytes(flush_bytes(old, new, fwd, id))
        }
        ValueRef::Rope(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::rope(flush_rope(old, new, fwd, id))
        }
        ValueRef::Fn(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::func(flush_closure(old, new, fwd, id))
        }
        ValueRef::Macro(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::macro_(flush_closure(old, new, fwd, id))
        }
        // Atoms, shared (PRELUDE/RUNTIME), and LOCAL handles of the *other*
        // generation are left unchanged (no copy this pass).
        _ => v,
    }
}

fn flush_pair(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: PairId) -> PairId {
    if let Some(new_idx) = fwd.pairs.lookup(id.index() as u32) {
        return fwd.mint_pair(new_idx as usize);
    }
    // Walk the cdr spine **iteratively** so a long proper list doesn't recurse its
    // length deep (a `(cons …)` chain of 100k would overflow the native stack —
    // the same reason `promote_list` is iterative). Recursion is bounded to
    // element *nesting* via `flush_value` on each car, in phase 2.
    //
    // Phase 1: reserve a fresh slot for every not-yet-copied LOCAL pair along the
    // spine (so cycles/shared tails through any car resolve to the placeholder),
    // and flush the spine's terminal (a non-pair tail, or the handle a shared/
    // already-copied cell joins).
    let mut spine: Vec<(usize, Value)> = Vec::new(); // (new slot, original car)
    let mut cur = Value::pair(id);
    let tail = loop {
        match cur.unpack() {
            ValueRef::Pair(p) if fwd.copies(p.region(), p.is_old()) => {
                let key = p.index() as u32;
                if let Some(n) = fwd.pairs.lookup(key) {
                    break Value::pair(fwd.mint_pair(n as usize));
                }
                let (car, cdr) = old.pairs[flush_bound!(old.pairs, p, fwd, "pair")];
                let new_idx = new.pairs.len();
                new.pairs.push((Value::nil(), Value::nil()));
                fwd.pairs.set(key, new_idx as u32);
                spine.push((new_idx, car));
                cur = cdr;
            }
            // Nil / atom / dotted non-pair tail / PRELUDE/RUNTIME pair: flush it
            // (cheap, no spine recursion) and stop.
            other => break flush_value(old, new, fwd, other),
        }
    };
    // Phase 2: flush each car and wire the cdrs, walking the spine in reverse so
    // each cell's cdr is the already-built next handle. Car flushes see the full
    // spine in `fwd`, so a car cycling back into the list resolves correctly.
    let mut next = tail;
    for &(new_idx, car) in spine.iter().rev() {
        let new_car = flush_value(old, new, fwd, car);
        new.pairs[new_idx] = (new_car, next);
        next = Value::pair(fwd.mint_pair(new_idx));
    }
    match next.unpack() {
        ValueRef::Pair(pid) => pid,
        _ => unreachable!("the spine always has at least the head pair"),
    }
}

fn flush_vector(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: VecId) -> VecId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.vectors.lookup(key) {
        return fwd.mint_vector(new_idx as usize);
    }
    // Reserve the destination slot and record the forwarding *before* flushing
    // elements (so shared/repeated references to this vector resolve to the
    // placeholder), then build the survivor in place — inlining without a temp
    // `Vec` for the common small case (`from_flushed`). The source slab is read
    // element-by-element (`Value` is `Copy`), keeping `old`'s borrow immutable
    // while `new`/`fwd` are borrowed mutably by `flush_value`.
    let src_idx = flush_bound!(old.vectors, id, fwd, "vector");
    let n = old.vectors[src_idx].len();
    let new_idx = new.vectors.len();
    new.vectors.push(VecStore::Inline {
        len: 0,
        items: [Value::nil(); INLINE_VEC_CAP],
    });
    fwd.vectors.set(key, new_idx as u32);
    let store = VecStore::from_flushed(n, |i| {
        let x = old.vectors[src_idx][i];
        flush_value(old, new, fwd, x)
    });
    new.vectors[new_idx] = store;
    fwd.mint_vector(new_idx)
}

fn flush_string(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: StrId) -> StrId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.strings.lookup(key) {
        return fwd.mint_string(new_idx as usize);
    }
    // Clone by variant. `Shared(arc)` becomes `Arc::clone` (+1 ref); the old
    // slab's drop right after `flush` returns will then -1, leaving the
    // blob's refcount net unchanged across a flush. Survivors keep the same
    // `SharedBlob` identity (no byte copy); non-surviving Shared slots
    // simply drop their old `Arc` and free the blob if they were the last
    // reference.
    let entry = match &old.strings[flush_bound!(old.strings, id, fwd, "string")] {
        LocalString::Inline(s) => LocalString::Inline(s.clone()),
        LocalString::Shared(arc) => LocalString::Shared(Arc::clone(arc)),
    };
    let new_idx = new.strings.len();
    new.strings.push(entry);
    fwd.strings.set(key, new_idx as u32);
    fwd.mint_string(new_idx)
}

fn flush_bigint(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: BigIntId) -> BigIntId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.bigints.lookup(key) {
        return fwd.mint_bigint(new_idx as usize);
    }
    // A leaf: clone the value's digits into the new slab (the old slab drops
    // right after `flush`). Same shape as `flush_string`'s inline branch.
    let n = old.bigints[flush_bound!(old.bigints, id, fwd, "bigint")].clone();
    let new_idx = new.bigints.len();
    new.bigints.push(n);
    fwd.bigints.set(key, new_idx as u32);
    fwd.mint_bigint(new_idx)
}

/// Flush a LOCAL decimal (mirrors [`flush_bigint`]). A leaf — clone the value into
/// the new slab (the old slab drops right after `flush`).
fn flush_decimal(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: DecimalId) -> DecimalId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.decimals.lookup(key) {
        return fwd.mint_decimal(new_idx as usize);
    }
    let n = old.decimals[flush_bound!(old.decimals, id, fwd, "decimal")].clone();
    let new_idx = new.decimals.len();
    new.decimals.push(n);
    fwd.decimals.set(key, new_idx as u32);
    fwd.mint_decimal(new_idx)
}

/// Flush a LOCAL bytes value (mirrors [`flush_bigint`]). A byte-clean leaf —
/// clone the `Arc<SharedBlob>` (a refcount bump, not a byte copy) into the new slab.
fn flush_bytes(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: BytesId) -> BytesId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.bytes.lookup(key) {
        return fwd.mint_bytes(new_idx as usize);
    }
    let b = old.bytes[flush_bound!(old.bytes, id, fwd, "bytes")].clone();
    let new_idx = new.bytes.len();
    new.bytes.push(b);
    fwd.bytes.set(key, new_idx as u32);
    fwd.mint_bytes(new_idx)
}

fn flush_rope(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: RopeId) -> RopeId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.ropes.lookup(key) {
        return fwd.mint_rope(new_idx as usize);
    }
    // `ropey::Rope::clone` is a cheap `Arc`-node bump (no byte copy); the old
    // slab drops right after `flush`, leaving the surviving rope's internal
    // refcounts net-unchanged — same structural sharing as `flush_string`.
    let rope = old.ropes[flush_bound!(old.ropes, id, fwd, "rope")].clone();
    let new_idx = new.ropes.len();
    new.ropes.push(rope);
    fwd.ropes.set(key, new_idx as u32);
    fwd.mint_rope(new_idx)
}

fn flush_map(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: MapId) -> MapId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.maps.lookup(key) {
        return fwd.mint_map(new_idx as usize);
    }
    // Snapshot just the scalar/copy fields + arrays we need to walk.
    let (size, data_map, node_map, is_collision, data_snapshot, children_snapshot): (
        u32,
        u16,
        u16,
        bool,
        SmallVec<[(Value, Value); 4]>,
        SmallVec<[MapId; 4]>,
    ) = {
        let node = &old.maps[flush_bound!(old.maps, id, fwd, "map")];
        (
            node.size,
            node.data_map,
            node.node_map,
            node.is_collision,
            node.data.iter().copied().collect(),
            node.children.iter().copied().collect(),
        )
    };
    let new_idx = new.maps.len();
    new.maps.push(MapNode::default());
    fwd.maps.set(key, new_idx as u32);
    let new_children: SmallVec<[MapId; 4]> = children_snapshot
        .iter()
        .map(|&c| {
            // Age-aware, like every other flush edge: a CHAMP trie built
            // incrementally shares child nodes across a tenure boundary, so a
            // child can be in the *other* generation than the node being copied.
            // Only recurse into a child of the generation this pass is collecting;
            // a child of the other age (or PRELUDE/RUNTIME) is left as-is.
            if fwd.copies(c.region(), c.is_old()) {
                flush_map(old, new, fwd, c)
            } else {
                c
            }
        })
        .collect();
    let new_data: SmallVec<[(Value, Value); 4]> = data_snapshot
        .iter()
        .map(|&(k, v)| (flush_value(old, new, fwd, k), flush_value(old, new, fwd, v)))
        .collect();
    new.maps[new_idx] = MapNode {
        size,
        data_map,
        node_map,
        is_collision,
        data: new_data,
        children: new_children,
    };
    fwd.mint_map(new_idx)
}

fn flush_closure(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: ClosureId) -> ClosureId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.closures.lookup(key) {
        return fwd.mint_closure(new_idx as usize);
    }
    let cl = old.closures[flush_bound!(old.closures, id, fwd, "closure")].clone();
    let new_idx = new.closures.len();
    new.closures.push(Closure::default());
    fwd.closures.set(key, new_idx as u32);
    let arms = cl
        .arms
        .iter()
        .map(|arm| ClosureArm {
            params: arm.params.clone(),
            optionals: arm
                .optionals
                .iter()
                .map(|&(s, d)| (s, flush_value(old, new, fwd, d)))
                .collect(),
            rest: arm.rest,
            body: arm
                .body
                .iter()
                .map(|&f| flush_value(old, new, fwd, f))
                .collect(),
            // Region-independent (symbol head + index map) — carry it verbatim.
            passthrough: arm.passthrough.clone(),
        })
        .collect();
    let env = cl.env.map(|e| flush_env(old, new, fwd, e));
    new.closures[new_idx] = Closure {
        name: cl.name,
        arms,
        doc: cl.doc,
        env,
    };
    fwd.mint_closure(new_idx)
}

fn flush_env(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, env: EnvId) -> EnvId {
    if env == EnvId::GLOBAL || !fwd.copies(env.region(), env.is_old()) {
        return env;
    }
    let key = env.index() as u32;
    if let Some(new_idx) = fwd.envs.lookup(key) {
        return fwd.mint_env(new_idx as usize);
    }
    let (parent_snapshot, vars_snapshot): (Option<EnvId>, EnvVars) = {
        let frame = &old.envs[flush_bound!(old.envs, env, fwd, "env")];
        (frame.parent, frame.vars.iter().copied().collect())
    };
    let new_idx = new.envs.len();
    new.envs.push(EnvFrame {
        vars: SmallVec::new(),
        parent: None,
    });
    fwd.envs.set(key, new_idx as u32);
    let parent = parent_snapshot.map(|p| flush_env(old, new, fwd, p));
    let vars: EnvVars = vars_snapshot
        .iter()
        .map(|&(s, v)| (s, flush_value(old, new, fwd, v)))
        .collect();
    new.envs[new_idx] = EnvFrame { vars, parent };
    fwd.mint_env(new_idx)
}

/// During a **major collect**, rewrite the OLD handles that live *inside* nursery
/// objects.  `flush_roots` already updated every handle stored directly in
/// `Heap::roots` / `Heap::env_roots`, but any handle that was *inside* a nursery
/// object was silently skipped — `flush_value` and `flush_env` gate on
/// `fwd.copies(region, is_old)` and a nursery object doesn't qualify.
///
/// When the minor that preceded the major was a *flip* (not a tenure) the nursery
/// is non-empty, so those skipped handles are real: a nursery closure's `env`
/// field, a nursery env-frame's vars, a nursery map's data entries or child
/// sub-nodes can all point into the pre-compaction old slab, which is now gone.
///
/// This pass walks every nursery slab slot in-place and rewrites OLD handles
/// through `fwd`.  If an OLD handle wasn't reached by `flush_roots` (it was only
/// referenced from a dead nursery object) it is copied to `dest` here —
/// conservative but correct: the minor that follows will discard the dead nursery
/// referrer, and the next major will then reclaim the briefly-retained old object.
fn flush_nursery_old_refs(
    nursery: &mut Slabs,
    old_src: &Slabs,
    dest: &mut Slabs,
    fwd: &mut FlushForward,
) {
    // EnvFrames: vars and parent chain.
    for i in 0..nursery.envs.len() {
        let n = nursery.envs[i].vars.len();
        for j in 0..n {
            let v = nursery.envs[i].vars[j].1;
            nursery.envs[i].vars[j].1 = flush_value(old_src, dest, fwd, v);
        }
        if let Some(parent) = nursery.envs[i].parent {
            if fwd.copies(parent.region(), parent.is_old()) {
                nursery.envs[i].parent = Some(flush_env(old_src, dest, fwd, parent));
            }
        }
    }
    // Closures: captured env, per-arm optional defaults, per-arm body literals.
    for i in 0..nursery.closures.len() {
        if let Some(env) = nursery.closures[i].env {
            if fwd.copies(env.region(), env.is_old()) {
                nursery.closures[i].env = Some(flush_env(old_src, dest, fwd, env));
            }
        }
        // A *shared* arms comes only from the RUNTIME-keyed template cache, so every
        // handle it holds is RUNTIME — which a minor collection never relocates. So
        // there is nothing to flush and `get_mut` correctly skips it (no un-sharing
        // clone on the hot minor-GC path). Only a *unique* arms can hold LOCAL
        // handles that this collection moved, and those we rewrite in place.
        if let Some(arms) = std::sync::Arc::get_mut(&mut nursery.closures[i].arms) {
            for arm in arms.iter_mut() {
                for (_, d) in arm.optionals.iter_mut() {
                    *d = flush_value(old_src, dest, fwd, *d);
                }
                for f in arm.body.iter_mut() {
                    *f = flush_value(old_src, dest, fwd, *f);
                }
            }
        }
    }
    // MapNodes: inline key/value data entries and child sub-node handles.
    for i in 0..nursery.maps.len() {
        let n = nursery.maps[i].data.len();
        for j in 0..n {
            let (k, v) = nursery.maps[i].data[j];
            nursery.maps[i].data[j] = (
                flush_value(old_src, dest, fwd, k),
                flush_value(old_src, dest, fwd, v),
            );
        }
        let m = nursery.maps[i].children.len();
        for j in 0..m {
            let c = nursery.maps[i].children[j];
            if fwd.copies(c.region(), c.is_old()) {
                nursery.maps[i].children[j] = flush_map(old_src, dest, fwd, c);
            }
        }
    }
    // Cons pairs.
    for i in 0..nursery.pairs.len() {
        let (car, cdr) = nursery.pairs[i];
        nursery.pairs[i] = (
            flush_value(old_src, dest, fwd, car),
            flush_value(old_src, dest, fwd, cdr),
        );
    }
    // Vectors (also backing store for `Value::Range` and `Value::SeqView`).
    for i in 0..nursery.vectors.len() {
        let n = nursery.vectors[i].len();
        for j in 0..n {
            let v = nursery.vectors[i][j];
            nursery.vectors[i][j] = flush_value(old_src, dest, fwd, v);
        }
    }
}

#[cfg(test)]
mod gen_handle_tests {
    use super::*;
    use crate::core::value::Value;

    /// The generational-handle tripwire fires at the bad deref. A LOCAL handle
    /// held across an arena flip (`flush`) without being passed through as a root
    /// carries a stale generation epoch; dereferencing it must panic *here* with
    /// a "use-after-GC" message — not a far-away out-of-bounds index. Debug-only
    /// check; `cargo test` builds with `debug_assertions` on. See
    /// `docs/memory-review.md`.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "use-after-GC")]
    fn stale_handle_after_flip_panics() {
        let mut h = Heap::new();
        let id = match h.alloc_pair(Value::int(1), Value::int(2)).unpack() {
            ValueRef::Pair(id) => id,
            _ => unreachable!(),
        };
        // Flush with no roots: the pair isn't relocated, and the epoch bumps.
        h.flush(&mut []);
        // `id` was minted in the previous epoch → stale → tripwire.
        let _ = h.pair(id);
    }

    /// The mirror case: a handle passed through `flush` as a root is relocated
    /// and re-stamped with the new epoch, so it stays valid (no false positive).
    #[test]
    fn flushed_root_handle_stays_valid() {
        let mut h = Heap::new();
        let mut roots = [h.alloc_pair(Value::int(1), Value::int(2))];
        h.flush(&mut roots);
        let (car, _) = match roots[0].unpack() {
            ValueRef::Pair(id) => h.pair(id),
            _ => unreachable!(),
        };
        assert!(matches!(car.unpack(), ValueRef::Int(1)));
    }

    /// Regression (`docs/kernel-audit-2026-06-03.md` #1): a **major collection
    /// that follows a *flip* minor** must rewrite the write-barrier `remembered`
    /// set through the env forwarding table. A flip *retains* `remembered` (the
    /// old->young edges persist); the subsequent major then relocates those old
    /// frames and bumps `old_epoch`, leaving every retained entry a stale index
    /// *and* a stale epoch. The next minor indexes `self.old.envs[e.index()]`
    /// directly — no bounds/epoch check — so a stale entry is a silent
    /// use-after-GC: an OOB `Vec` panic when the compacted old gen is smaller, a
    /// wrong-frame read otherwise. (`BROOD_GC_VERIFY` misses it too — its
    /// remembered walk uses a safe `.get()`.)
    ///
    /// We drive the full `tenure -> mid-bind env_define -> flip -> major -> minor`
    /// interleave with the remembered frame at a HIGH old-gen index that the major
    /// then compacts away, so a stale index would be out of bounds. Without the
    /// fix the post-major assertions fail (stale epoch / OOB index); with it they
    /// hold and the trailing minor derefs the rewritten handle cleanly.
    #[test]
    fn major_after_flip_rewrites_remembered_set() {
        let mut h = Heap::new();
        let x = crate::core::value::intern("x");
        // Inflate the nursery with several frames, all rooted; the one we keep is
        // LAST so it tenures to the highest old-gen index.
        let mut envs: Vec<EnvId> = (0..8).map(|_| h.new_env(None)).collect();
        let keep_pos = envs.len() - 1;
        // Tenure them all into the old generation.
        h.minor_collect(true, &mut [], &mut envs);
        let keep = envs[keep_pos];
        assert!(keep.is_old(), "frame should be tenured into the old gen");
        let high_index = keep.index();
        assert!(
            high_index > 0,
            "kept frame should sit at a non-zero old index"
        );
        // Mid-bind define: store a *young* value into the tenured frame, recording
        // an old->young edge in `remembered`.
        let young = h.alloc_pair(Value::int(1), Value::int(2));
        h.env_define(keep, x, young);
        assert_eq!(
            h.remembered.len(),
            1,
            "the old->young edge should be remembered"
        );
        // Keep only `keep` rooted, so the upcoming major reclaims the 7 inflation
        // frames and *shrinks* the old gen below `high_index`.
        let mut roots = vec![keep];
        // A *flip* minor (tenure=false): retains `remembered`, leaves old untouched.
        h.minor_collect(false, &mut [], &mut roots);
        assert_eq!(h.remembered.len(), 1, "a flip must retain remembered");
        // The major: compacts old (drops the 7 dead frames), bumps `old_epoch`.
        h.major_collect(&mut [], &mut roots);
        let keep = roots[0];
        // With the fix, `remembered` was rewritten through the forwarding table:
        // current epoch, in bounds, pointing at the surviving frame. Without it,
        // these carry the stale pre-major index/epoch.
        assert_eq!(h.remembered.len(), 1);
        let r = h.remembered[0];
        assert!(r.is_old());
        assert_eq!(
            r.generation(),
            h.old_epoch,
            "remembered entry kept a stale epoch after the major (use-after-GC)"
        );
        assert!(
            r.index() < h.old.envs.len(),
            "remembered entry is out of bounds after the major (use-after-GC): \
             index {} >= old.envs.len() {}",
            r.index(),
            h.old.envs.len(),
        );
        assert_eq!(
            r.index(),
            keep.index(),
            "remembered should point at the surviving kept frame"
        );
        // The deref path the bug corrupts: a subsequent minor reads
        // `self.old.envs[r.index()]`. Must not panic; the young binding survives.
        h.minor_collect(false, &mut [], &mut roots);
        let keep = roots[0];
        assert!(keep.is_old());
        let bound = h.old.envs[keep.index()]
            .vars
            .iter()
            .find(|(s, _)| *s == x)
            .map(|(_, v)| *v);
        assert!(
            matches!(bound.map(Value::unpack), Some(ValueRef::Pair(_))),
            "the remembered young binding was lost or corrupted: {bound:?}"
        );
    }

    /// Repeated `env_define`s into the *same* tenured frame must not grow the
    /// write-barrier `remembered` set (kernel audit, perf #3): one entry per
    /// distinct old frame is all the minor's rewrite walk needs, and without
    /// the de-dup a long binding loop on a tenured frame grows the set — and
    /// every subsequent minor's walk — without bound until the next tenure.
    #[test]
    fn remembered_set_dedups_repeated_binds() {
        let mut h = Heap::new();
        let mut envs: Vec<EnvId> = vec![h.new_env(None)];
        h.minor_collect(true, &mut [], &mut envs);
        let frame = envs[0];
        assert!(frame.is_old(), "frame should be tenured into the old gen");
        for i in 0..64 {
            let sym = crate::core::value::intern(&format!("x{i}"));
            let young = h.alloc_pair(Value::int(i), Value::int(i));
            h.env_define(frame, sym, young);
        }
        assert_eq!(
            h.remembered.len(),
            1,
            "64 binds into one tenured frame must remember it once, not 64 times"
        );
        // The single entry still carries every young edge through a minor.
        let mut roots = vec![frame];
        h.minor_collect(false, &mut [], &mut roots);
        let frame = roots[0];
        assert_eq!(h.old.envs[frame.index()].vars.len(), 64);
    }

    // ── flush_nursery_old_refs regression tests ──────────────────────────────
    //
    // Pattern common to every test below:
    //   1. Inflate old space with N decoy objects + 1 "keeper" (tenured together,
    //      keeper is last → highest old index).
    //   2. Drop decoy roots; keeper is now only referenced from a NURSERY object.
    //   3. Flip minor: nursery object survives; decoys remain unreachable in old.
    //   4. Major: old compacts. Decoys are freed; keeper moves to index 0.
    //      Without the fix, the nursery object still holds the pre-major index
    //      (e.g., 20) which is now out-of-bounds (len=1) — silent use-after-GC.
    //      With the fix, flush_nursery_old_refs rewrites it to index 0.
    //   5. Assert the nursery object's inner handle is in-bounds and reads correctly.

    /// Tenure N strings + a keeper; drop decoys; return (inflated old space
    /// keeper Value).  The keeper is pushed last so it sits at the highest index.
    fn inflate_old_with_keeper_string(h: &mut Heap, n: usize, content: &str) -> Value {
        let mut roots: Vec<Value> = (0..n)
            .map(|i| h.alloc_string(&format!("decoy-{i}")))
            .collect();
        roots.push(h.alloc_string(content));
        h.minor_collect(true, &mut roots, &mut []);
        // Return only the keeper (last); caller holds no decoy roots.
        roots.pop().unwrap()
    }

    /// Nursery env frame with an OLD string var is rewritten after flip+major.
    /// Hatch crash pattern: session env frame held a stale old StrId → OOB.
    #[test]
    fn nursery_env_old_string_var_rewritten() {
        let mut h = Heap::new();
        let sym = crate::core::value::intern("s");
        let keeper = inflate_old_with_keeper_string(&mut h, 20, "hello-env-var");
        let pre_major_idx = match keeper.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => unreachable!(),
        };

        let nursery_env = h.new_env(None);
        h.env_define(nursery_env, sym, keeper);
        // Only the nursery env keeps the keeper alive.
        let mut er = vec![nursery_env];
        h.minor_collect(false, &mut [], &mut er); // flip
        let _nursery_env = er[0];
        h.major_collect(&mut [], &mut er); // compact — decoys freed, keeper moves
        let nursery_env = er[0];

        let bound = h.local.envs[nursery_env.index()]
            .vars
            .iter()
            .find(|(s, _)| *s == sym)
            .map(|(_, v)| *v)
            .expect("binding must survive");
        let new_idx = match bound.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => panic!("expected Str"),
        };
        assert!(
            new_idx < h.old.strings.len(),
            "string index {new_idx} OOB (len {}); pre-major index was {pre_major_idx}",
            h.old.strings.len()
        );
        assert_eq!(h.old.strings[new_idx].as_str(), "hello-env-var");
    }

    /// Nursery env frame with an OLD pair var is rewritten after flip+major.
    #[test]
    fn nursery_env_old_pair_var_rewritten() {
        let mut h = Heap::new();
        let sym = crate::core::value::intern("p");
        // Inflate: 20 decoy pairs + keeper pair (1, 2).
        let mut roots: Vec<Value> = (0..20)
            .map(|i| h.alloc_pair(Value::int(i), Value::int(i)))
            .collect();
        roots.push(h.alloc_pair(Value::int(99), Value::int(100)));
        h.minor_collect(true, &mut roots, &mut []);
        let keeper_pair = roots.pop().unwrap();
        let pre_major_idx = match keeper_pair.unpack() {
            ValueRef::Pair(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => unreachable!(),
        };
        drop(roots);

        let nursery_env = h.new_env(None);
        h.env_define(nursery_env, sym, keeper_pair);
        let mut er = vec![nursery_env];
        h.minor_collect(false, &mut [], &mut er);
        let _nursery_env = er[0];
        h.major_collect(&mut [], &mut er);
        let nursery_env = er[0];

        let bound = h.local.envs[nursery_env.index()]
            .vars
            .iter()
            .find(|(s, _)| *s == sym)
            .map(|(_, v)| *v)
            .unwrap();
        let new_idx = match bound.unpack() {
            ValueRef::Pair(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => panic!("expected Pair"),
        };
        assert!(
            new_idx < h.old.pairs.len(),
            "pair index {new_idx} OOB (len {}); pre-major was {pre_major_idx}",
            h.old.pairs.len()
        );
        let (car, _) = h.old.pairs[new_idx];
        assert!(matches!(car.unpack(), ValueRef::Int(99)));
    }

    /// Nursery env frame with an OLD closure var is rewritten after flip+major.
    #[test]
    fn nursery_env_old_closure_var_rewritten() {
        use crate::core::value::Closure;
        let mut h = Heap::new();
        let sym = crate::core::value::intern("f");
        // Build a tiny closure and inflate old space around it.
        let cl = Closure::single(None, vec![], vec![], None, vec![], None, None);
        let cl_id = h.alloc_closure(cl);
        let cl_val = Value::func(cl_id);
        // 20 decoy closures to push keeper to a high index.
        let mut roots: Vec<Value> = (0..20)
            .map(|_| {
                let dc = Closure::single(None, vec![], vec![], None, vec![], None, None);
                Value::func(h.alloc_closure(dc))
            })
            .collect();
        roots.push(cl_val);
        h.minor_collect(true, &mut roots, &mut []);
        let keeper_fn = roots.pop().unwrap();
        let pre_major_idx = match keeper_fn.unpack() {
            ValueRef::Fn(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => unreachable!(),
        };
        drop(roots);

        let nursery_env = h.new_env(None);
        h.env_define(nursery_env, sym, keeper_fn);
        let mut er = vec![nursery_env];
        h.minor_collect(false, &mut [], &mut er);
        let _nursery_env = er[0];
        h.major_collect(&mut [], &mut er);
        let nursery_env = er[0];

        let bound = h.local.envs[nursery_env.index()]
            .vars
            .iter()
            .find(|(s, _)| *s == sym)
            .map(|(_, v)| *v)
            .unwrap();
        let new_idx = match bound.unpack() {
            ValueRef::Fn(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => panic!("expected Fn"),
        };
        assert!(
            new_idx < h.old.closures.len(),
            "closure index {new_idx} OOB (len {}); pre-major was {pre_major_idx}",
            h.old.closures.len()
        );
    }

    /// Nursery closure whose `env` field points to an OLD env is rewritten.
    /// Pong crash pattern: nursery badge-ops closure had stale OLD env → wrong
    /// env frame looked up → captured `throb` returned nil.
    #[test]
    fn nursery_closure_old_env_rewritten() {
        use crate::core::value::Closure;
        let mut h = Heap::new();
        let sym = crate::core::value::intern("throb");
        // Build the "token-ops" env that captures throb=3.14.
        // Inflate: 20 decoy envs + the keeper env (all tenured).
        let mut decoy_envs: Vec<EnvId> = (0..20).map(|_| h.new_env(None)).collect();
        let keeper_env = h.new_env(None);
        h.env_define(keeper_env, sym, Value::float(1.25));
        decoy_envs.push(keeper_env);
        h.minor_collect(true, &mut [], &mut decoy_envs);
        let keeper_env_old = decoy_envs.pop().unwrap();
        assert!(keeper_env_old.is_old());
        let pre_major_env_idx = keeper_env_old.index();
        drop(decoy_envs); // decoys now unreachable

        // Create a nursery closure that captures the OLD env (the "badge-ops" closure).
        let cl = Closure::single(
            None,
            vec![],
            vec![],
            None,
            vec![],
            None,
            Some(keeper_env_old),
        );
        let cl_id = h.alloc_closure(cl);
        assert!(!cl_id.is_old(), "closure must be nursery");

        let mut roots = [Value::func(cl_id)];
        h.minor_collect(false, &mut roots, &mut []); // flip
        h.major_collect(&mut roots, &mut []); // compact — 20 decoy envs freed

        let new_cl_id = match roots[0].unpack() {
            ValueRef::Fn(id) => id,
            _ => unreachable!(),
        };
        let env_id = h.local.closures[new_cl_id.index()]
            .env
            .expect("closure must have an env");

        assert!(
            env_id.is_old(),
            "closure env must still be old-gen after major"
        );
        assert!(
            env_id.index() < h.old.envs.len(),
            "closure env index {} OOB (len {}); pre-major was {pre_major_env_idx}",
            env_id.index(),
            h.old.envs.len()
        );
        // The env lookup chain must work: env_get returns throb = 3.14.
        let val = h.env_get(env_id, sym).expect("throb must be findable");
        assert!(
            matches!(val.unpack(), ValueRef::Float(f) if (f - 1.25).abs() < 1e-9),
            "throb should be 3.14, got {val:?}"
        );
    }

    /// Nursery closure whose per-arm optional-default Value is an OLD string.
    #[test]
    fn nursery_closure_old_optional_default_rewritten() {
        use crate::core::value::{Closure, ClosureArm};
        let mut h = Heap::new();
        let keeper = inflate_old_with_keeper_string(&mut h, 20, "default-str");
        let pre_idx = match keeper.unpack() {
            ValueRef::Str(id) => id.index(),
            _ => unreachable!(),
        };

        let opt_sym = crate::core::value::intern("opt");
        let cl = Closure {
            name: None,
            arms: vec![ClosureArm {
                params: vec![],
                optionals: vec![(opt_sym, keeper)],
                rest: None,
                body: vec![],
                passthrough: None,
            }]
            .into(),
            doc: None,
            env: None,
        };
        let cl_id = h.alloc_closure(cl);
        let mut roots = [Value::func(cl_id)];
        h.minor_collect(false, &mut roots, &mut []);
        h.major_collect(&mut roots, &mut []);

        let new_cl_id = match roots[0].unpack() {
            ValueRef::Fn(id) => id,
            _ => unreachable!(),
        };
        let opt_val = h.local.closures[new_cl_id.index()].arms[0].optionals[0].1;
        let new_idx = match opt_val.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => panic!("expected Str"),
        };
        assert!(
            new_idx < h.old.strings.len(),
            "optional default index {new_idx} OOB (len {}); pre-major was {pre_idx}",
            h.old.strings.len()
        );
        assert_eq!(h.old.strings[new_idx].as_str(), "default-str");
    }

    /// Nursery closure whose arm body contains an OLD string literal is rewritten.
    #[test]
    fn nursery_closure_old_body_literal_rewritten() {
        use crate::core::value::{Closure, ClosureArm};
        let mut h = Heap::new();
        let keeper = inflate_old_with_keeper_string(&mut h, 20, "body-literal");
        let pre_idx = match keeper.unpack() {
            ValueRef::Str(id) => id.index(),
            _ => unreachable!(),
        };

        let cl = Closure {
            name: None,
            arms: vec![ClosureArm {
                params: vec![],
                optionals: vec![],
                rest: None,
                body: vec![keeper], // OLD string in body
                passthrough: None,
            }]
            .into(),
            doc: None,
            env: None,
        };
        let cl_id = h.alloc_closure(cl);
        let mut roots = [Value::func(cl_id)];
        h.minor_collect(false, &mut roots, &mut []);
        h.major_collect(&mut roots, &mut []);

        let new_cl_id = match roots[0].unpack() {
            ValueRef::Fn(id) => id,
            _ => unreachable!(),
        };
        let body_val = h.local.closures[new_cl_id.index()].arms[0].body[0];
        let new_idx = match body_val.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => panic!("expected Str"),
        };
        assert!(
            new_idx < h.old.strings.len(),
            "body literal index {new_idx} OOB (len {}); pre-major was {pre_idx}",
            h.old.strings.len()
        );
        assert_eq!(h.old.strings[new_idx].as_str(), "body-literal");
    }

    /// Nursery map (CHAMP root) with an OLD string as a data value is rewritten.
    /// The map is accessed via map_get after the major; it must not panic/return None.
    #[test]
    fn nursery_map_old_data_value_rewritten() {
        let mut h = Heap::new();
        let keeper = inflate_old_with_keeper_string(&mut h, 20, "map-value");
        let pre_idx = match keeper.unpack() {
            ValueRef::Str(id) => id.index(),
            _ => unreachable!(),
        };
        let key = crate::core::value::sym("k");

        let empty_id = match h.alloc_empty_map().unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!(),
        };
        let mut roots = [h.map_assoc(empty_id, key, keeper)];
        h.minor_collect(false, &mut roots, &mut []); // flip — map stays nursery
        h.major_collect(&mut roots, &mut []); // compact — keeper only via map

        let map_id = match roots[0].unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!(),
        };
        let result = h.map_get(map_id, key).expect("key must survive flip+major");
        let new_idx = match result.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => panic!("expected Str, got {result:?}"),
        };
        assert!(
            new_idx < h.old.strings.len(),
            "map value index {new_idx} OOB (len {}); pre-major was {pre_idx}",
            h.old.strings.len()
        );
        assert_eq!(h.old.strings[new_idx].as_str(), "map-value");
    }

    /// Nursery pair with an OLD string car is rewritten after flip+major.
    #[test]
    fn nursery_pair_old_car_rewritten() {
        let mut h = Heap::new();
        let keeper = inflate_old_with_keeper_string(&mut h, 20, "pair-car");
        let pre_idx = match keeper.unpack() {
            ValueRef::Str(id) => id.index(),
            _ => unreachable!(),
        };

        // Nursery pair: car = OLD string, cdr = nil.
        let mut roots = [h.alloc_pair(keeper, Value::nil())];
        h.minor_collect(false, &mut roots, &mut []);
        h.major_collect(&mut roots, &mut []);

        let (car, _) = match roots[0].unpack() {
            ValueRef::Pair(id) => {
                assert!(!id.is_old(), "pair must still be nursery after flip");
                h.local.pairs[id.index()]
            }
            _ => unreachable!(),
        };
        let new_idx = match car.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => panic!("expected Str"),
        };
        assert!(
            new_idx < h.old.strings.len(),
            "pair car index {new_idx} OOB (len {}); pre-major was {pre_idx}",
            h.old.strings.len()
        );
        assert_eq!(h.old.strings[new_idx].as_str(), "pair-car");
    }

    /// Nursery vector with an OLD string element is rewritten after flip+major.
    #[test]
    fn nursery_vector_old_elem_rewritten() {
        let mut h = Heap::new();
        let keeper = inflate_old_with_keeper_string(&mut h, 20, "vec-elem");
        let pre_idx = match keeper.unpack() {
            ValueRef::Str(id) => id.index(),
            _ => unreachable!(),
        };

        let mut roots = [h.alloc_vector(vec![keeper])];
        h.minor_collect(false, &mut roots, &mut []);
        h.major_collect(&mut roots, &mut []);

        let vec_id = match roots[0].unpack() {
            ValueRef::Vector(id) => id,
            _ => unreachable!(),
        };
        let elem = h.local.vectors[vec_id.index()][0];
        let new_idx = match elem.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old());
                id.index()
            }
            _ => panic!("expected Str"),
        };
        assert!(
            new_idx < h.old.strings.len(),
            "vector elem index {new_idx} OOB (len {}); pre-major was {pre_idx}",
            h.old.strings.len()
        );
        assert_eq!(h.old.strings[new_idx].as_str(), "vec-elem");
    }

    /// Nursery env with an OLD parent env: the parent pointer is rewritten.
    /// Ensures the env lookup chain stays intact after flip+major.
    #[test]
    fn nursery_env_old_parent_rewritten() {
        let mut h = Heap::new();
        let sym = crate::core::value::intern("v");
        // Inflate: 20 decoy envs + keeper parent env (all tenured).
        let mut decoy_envs: Vec<EnvId> = (0..20).map(|_| h.new_env(None)).collect();
        let parent_env = h.new_env(None);
        h.env_define(parent_env, sym, Value::int(42));
        decoy_envs.push(parent_env);
        h.minor_collect(true, &mut [], &mut decoy_envs);
        let parent_old = decoy_envs.pop().unwrap();
        assert!(parent_old.is_old());
        let pre_idx = parent_old.index();
        drop(decoy_envs);

        // Nursery child env whose parent is the OLD env.
        let child_env = h.new_env(Some(parent_old));
        let mut er = vec![child_env];
        h.minor_collect(false, &mut [], &mut er);
        let _child_env = er[0];
        h.major_collect(&mut [], &mut er);
        let child_env = er[0];

        // Parent pointer in the nursery child must be rewritten.
        let parent_ptr = h.local.envs[child_env.index()]
            .parent
            .expect("child must have parent");
        assert!(
            parent_ptr.is_old(),
            "parent must still be old-gen after major"
        );
        assert!(
            parent_ptr.index() < h.old.envs.len(),
            "parent index {} OOB (len {}); pre-major was {pre_idx}",
            parent_ptr.index(),
            h.old.envs.len()
        );
        // The env lookup must traverse the parent chain correctly.
        let val = h
            .env_get(child_env, sym)
            .expect("v must be findable via parent");
        assert!(matches!(val.unpack(), ValueRef::Int(42)));
    }

    /// Nursery map whose root node has an OLD child sub-node (structural sharing
    /// from `assoc` on an OLD map): the child pointer is rewritten.
    #[test]
    fn nursery_map_old_child_node_rewritten() {
        let mut h = Heap::new();
        // Build a map large enough for the trie to have child sub-nodes.
        // We use integer keys 0..32 to force at least one level of branching.
        let sym_key = crate::core::value::sym("new-key");
        let keeper_str = inflate_old_with_keeper_string(&mut h, 8, "map-child-keeper");
        // Build a base map with 32 entries, tenure it.
        let empty_id = match h.alloc_empty_map().unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!(),
        };
        let mut base = Value::map(empty_id);
        for i in 0..32i64 {
            let kid = match base.unpack() {
                ValueRef::Map(id) => id,
                _ => unreachable!(),
            };
            base = h.map_assoc(kid, Value::int(i), Value::int(i * 10));
        }
        let mut roots = [base];
        h.minor_collect(true, &mut roots, &mut []); // tenure base map into old
        let old_base_map = roots[0];
        // `assoc` on the OLD map creates a nursery root that shares OLD child nodes.
        let old_map_id = match old_base_map.unpack() {
            ValueRef::Map(id) => {
                assert!(id.is_old());
                id
            }
            _ => unreachable!(),
        };
        let nursery_map = h.map_assoc(old_map_id, sym_key, keeper_str);
        // Flip minor: drop old_base_map root; only nursery map keeps base alive.
        let mut roots = [nursery_map];
        h.minor_collect(false, &mut roots, &mut []);
        h.major_collect(&mut roots, &mut []);

        // map_get on the surviving nursery map must traverse correctly after the major.
        let final_id = match roots[0].unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!(),
        };
        // The newly-added key must resolve.
        let val = h
            .map_get(final_id, sym_key)
            .expect("new-key must survive flip+major");
        match val.unpack() {
            ValueRef::Str(id) => {
                assert!(id.is_old());
                assert!(id.index() < h.old.strings.len(), "string OOB after major");
                assert_eq!(h.old.strings[id.index()].as_str(), "map-child-keeper");
            }
            _ => panic!("expected Str"),
        }
        // An original key from the shared sub-tree must also resolve.
        let orig = h
            .map_get(final_id, Value::int(0))
            .expect("key 0 must survive");
        assert!(matches!(orig.unpack(), ValueRef::Int(0)));
    }

    /// Multiple nursery objects across all five affected types hold OLD references
    /// to the same keeper string; all must be rewritten in one major pass.
    #[test]
    fn all_nursery_types_old_refs_rewritten_together() {
        use crate::core::value::{Closure, ClosureArm};
        let mut h = Heap::new();
        let sym = crate::core::value::intern("k");
        let keeper = inflate_old_with_keeper_string(&mut h, 20, "shared-keeper");
        let pre_idx = match keeper.unpack() {
            ValueRef::Str(id) => id.index(),
            _ => unreachable!(),
        };

        // Five nursery objects each referencing the same OLD keeper.
        let nursery_env = h.new_env(None);
        h.env_define(nursery_env, sym, keeper);

        let cl = Closure {
            name: None,
            arms: vec![ClosureArm {
                params: vec![],
                optionals: vec![(sym, keeper)],
                rest: None,
                body: vec![keeper],
                passthrough: None,
            }]
            .into(),
            doc: None,
            env: None,
        };
        let cl_val = Value::func(h.alloc_closure(cl));

        let empty_id = match h.alloc_empty_map().unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!(),
        };
        let map_val = h.map_assoc(empty_id, crate::core::value::sym("m"), keeper);
        let pair_val = h.alloc_pair(keeper, Value::nil());
        let vec_val = h.alloc_vector(vec![keeper]);

        let mut vr = [cl_val, map_val, pair_val, vec_val];
        let mut er = vec![nursery_env];
        h.minor_collect(false, &mut vr, &mut er);
        h.major_collect(&mut vr, &mut er);

        let check_str = |val: Value, label: &str| {
            let new_idx = match val.unpack() {
                ValueRef::Str(id) => {
                    assert!(id.is_old(), "{label}: must be old-gen");
                    id.index()
                }
                _ => panic!("{label}: expected Str"),
            };
            assert!(
                new_idx < h.old.strings.len(),
                "{label}: index {new_idx} OOB (len {}); pre-major was {pre_idx}",
                h.old.strings.len()
            );
            assert_eq!(
                h.old.strings[new_idx].as_str(),
                "shared-keeper",
                "{label}: wrong content"
            );
        };

        // Env var.
        let nursery_env = er[0];
        let env_val = h.local.envs[nursery_env.index()]
            .vars
            .iter()
            .find(|(s, _)| *s == sym)
            .map(|(_, v)| *v)
            .unwrap();
        check_str(env_val, "env var");

        // Closure optional default + body literal.
        let new_cl_id = match vr[0].unpack() {
            ValueRef::Fn(id) => id,
            _ => unreachable!(),
        };
        let cl = &h.local.closures[new_cl_id.index()];
        check_str(cl.arms[0].optionals[0].1, "closure optional");
        check_str(cl.arms[0].body[0], "closure body");

        // Map value.
        let map_id = match vr[1].unpack() {
            ValueRef::Map(id) => id,
            _ => unreachable!(),
        };
        let map_result = h.map_get(map_id, crate::core::value::sym("m")).unwrap();
        check_str(map_result, "map value");

        // Pair car.
        let pair_id = match vr[2].unpack() {
            ValueRef::Pair(id) => id,
            _ => unreachable!(),
        };
        let (car, _) = h.local.pairs[pair_id.index()];
        check_str(car, "pair car");

        // Vector element.
        let vec_id = match vr[3].unpack() {
            ValueRef::Vector(id) => id,
            _ => unreachable!(),
        };
        check_str(h.local.vectors[vec_id.index()][0], "vector elem");
    }
}
