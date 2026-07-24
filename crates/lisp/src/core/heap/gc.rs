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
        let mut fwd = FlushForward::default();
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
            if let Some(&new_idx) = fwd.pairs.get(&(key as u32)) {
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

    /// Number of closures in the shared, append-only RUNTIME region. For
    /// introspection / tests of hot-reload growth (Stage 5 dedup): redefining a
    /// global to *unchanged* code must not increase this; it never decreases
    /// (append-only — old versions stay live for in-flight calls, reclaimed only
    /// by the future RUNTIME collector, docs/live-editing.md Stage 5).
    pub fn runtime_closure_count(&self) -> usize {
        self.runtime.cur_code().closures.count()
    }

    /// **RUNTIME-collector exploration (read-only, ADR-076 / `docs/runtime-collector-
    /// exploration.md`).** Count RUNTIME closures *reachable* from the live roots —
    /// the global bindings plus this process's operand stack — by marking the shared
    /// code graph transitively. The difference from [`runtime_closure_count`] is the
    /// **reclaimable** set: superseded versions a future compacting collector could
    /// free. Walks immutable RUNTIME code only; moves and frees nothing. Single-
    /// process view: a *spawned* process holding an older version would keep it live,
    /// so for a multi-process runtime this `live` is a lower bound (`reclaimable` an
    /// upper bound) — adequate for a leak estimate, not for an actual collector.
    pub fn runtime_live_closure_count(&self) -> usize {
        let mut visited: HashSet<usize> = HashSet::new();
        let mut work: Vec<Value> = Vec::new();
        // Seed: every global binding value + this process's in-flight operand roots.
        work.extend(self.runtime.globals_read().values().copied());
        work.extend(self.roots.iter().copied());
        while let Some(v) = work.pop() {
            match v.unpack() {
                ValueRef::Fn(id) | ValueRef::Macro(id) if id.region() == RUNTIME => {
                    if visited.insert(id.index()) {
                        let cl = self.closure(id);
                        for arm in cl.arms.iter() {
                            work.extend(arm.body.iter().copied());
                            work.extend(arm.optionals.iter().map(|(_, d)| *d));
                        }
                        // Captured env (a closure promoted from inside a call): walk
                        // its RUNTIME frame chain's bound values for nested closures.
                        let mut cur = cl.env;
                        while let Some(e) = cur {
                            if e == EnvId::GLOBAL || e.region() != RUNTIME {
                                break;
                            }
                            let frame = self.env_frame(e);
                            work.extend(frame.vars.iter().map(|(_, val)| *val));
                            cur = frame.parent;
                        }
                    }
                }
                ValueRef::Pair(id) if id.region() == RUNTIME => {
                    let (h, t) = self.pair(id);
                    work.push(h);
                    work.push(t);
                }
                ValueRef::Vector(id) if id.region() == RUNTIME => {
                    work.extend(self.vector(id).iter().copied());
                }
                ValueRef::Map(id) | ValueRef::Set(id) if id.region() == RUNTIME => {
                    // `fold_entries` walks the trie in place — no intermediate Vec
                    // (unlike `map_entries`), which matters on this diagnostics walk.
                    // A set shares the trie (values all `true`), so the same walk covers it.
                    self.fold_entries(id, &mut |k, val| {
                        work.push(k);
                        work.push(val);
                    });
                }
                _ => {}
            }
        }
        visited.len()
    }

    /// **RUNTIME collector — Step 2 (aging).** Start a fresh code generation: flip
    /// `current_gen` so subsequent `def`/`promote` land in the *other* slot, while
    /// the previous generation's code stays fully readable via its own handle
    /// `code_gen` bit (no rewrite — the essence of the Erlang-style 2-generation
    /// model, ADR-091). Returns whether it aged.
    ///
    /// Only ages when the target slot is **empty** — its previous generation fully
    /// reclaimed (the 2-versions-max rule), so a new gen's handle indices can never
    /// collide with a stale generation's still-live handles. A lightweight atomic
    /// flip: it needs no unique ownership (unlike compaction), so it's the
    /// multi-process reclamation primitive. Freeing the *old* generation once no
    /// live process references it is stage 4 (cooperative liveness).
    pub fn age_runtime(&self) -> bool {
        // Exclude every in-flight `promote` for the flip: the write lock waits for all
        // promote read-guards to drain, so no promote's slot reservation and fill can
        // straddle the `current_gen` change (ADR-091 — else the fill hits the wrong
        // generation's slab). Held only across the atomic flip; promotion resumes
        // immediately after, now targeting the new generation.
        let _age_guard = self
            .runtime
            .promote_lock
            .write()
            .unwrap_or_else(|e| e.into_inner());
        let other = 1 - self.runtime.cur_gen();
        if !self.runtime.gens[other].load().is_empty() {
            return false;
        }
        self.runtime
            .current_gen
            .store(other, std::sync::atomic::Ordering::Relaxed);
        self.runtime.aged_count.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// How many times this runtime has aged ([`age_runtime`](Self::age_runtime))
    /// — diagnostic, lets a test confirm the multi-generation collector's aging path
    /// fired end-to-end even when a full free is timing-dependent.
    pub fn runtime_aged_count(&self) -> u64 {
        self.runtime.aged_count.load(Ordering::Relaxed)
    }

    /// The RUNTIME code region's current generation slot (0 or 1). Diagnostic /
    /// test observation of the [`age_runtime`](Self::age_runtime) flip.
    pub fn runtime_cur_gen(&self) -> usize {
        self.runtime.cur_gen()
    }

    /// **RUNTIME collector — Stage 4 (single-flight aging gate).** Claim the exclusive
    /// right to run an `age + migrate + drain` cycle: a CAS on the shared `aging` flag.
    /// Returns `true` to the one winner; a loser skips this safepoint (the winner's
    /// cycle will reclaim). Paired with [`end_aging`](Self::end_aging).
    pub fn begin_aging(&self) -> bool {
        self.runtime
            .aging
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Release the single-flight aging gate ([`begin_aging`](Self::begin_aging)).
    pub fn end_aging(&self) {
        self.runtime.aging.store(false, Ordering::Release);
    }

    /// **RUNTIME collector — Stage 4 (live-globals migration, ADR-091).** Re-export the
    /// live globals from the just-aged-out generation `old_gen` into the current
    /// generation — the piece that makes whole-generation reclamation actually
    /// *converge*. Returns how many bindings it migrated.
    ///
    /// **Why it's needed.** [`age_runtime`](Self::age_runtime) only flips which slot new
    /// code lands in; it moves no existing binding. Because Brood's `def` is per-global
    /// (unlike an Erlang module reload, which re-exports *all* of a module's functions
    /// as a unit), a global defined once and never redefined would stay in `old_gen`
    /// forever and pin it — so the generation never drains and can never be freed. This
    /// copies each live global's RUNTIME sub-graph into the current generation and
    /// repoints the shared globals table at the copy, so `old_gen` is left holding only
    /// superseded versions plus whatever in-flight process state still references it —
    /// which drains as those calls finish (exactly Erlang's old-vs-current code).
    ///
    /// **How it stays safe under concurrency** (this runs on `&self` — the runtime is
    /// shared, no unique ownership):
    /// - It copies into the *current* generation's `boxcar` slab, which peer processes'
    ///   `def`/`promote` also append to — lock-free concurrent appends give disjoint
    ///   indices, so there's no collision.
    /// - It does **not** touch any process's private roots (stacks, captured closures);
    ///   those keep their `old_gen` handles, which stay valid because `old_gen` is
    ///   *retained* (not freed) until the Stage-3 drain says it's unreferenced.
    /// - The reconcile installs a migrated handle **only if the global still resides in
    ///   `old_gen`** — a concurrent redefinition moves the binding to the current
    ///   generation, and that newer value wins (its handle isn't in `old_gen`, so the
    ///   migrated copy is skipped and simply becomes unreferenced, reclaimed later).
    ///   This needs no value-equality: after aging, `old_gen` is frozen — the *only*
    ///   way a binding leaves it is a redefinition into the current generation.
    ///
    /// A no-op (returns 0) unless a prior [`age_runtime`](Self::age_runtime) left
    /// `old_gen != cur_gen` and `old_gen` is non-empty.
    /// **RUNTIME collector — Stage 5 soundness (ADR-091).** Re-home a value resident
    /// in a *non-current* RUNTIME generation into the current generation, deep-copying
    /// its code tree (the same flush [`migrate_live_globals`] uses). A value already in
    /// the current generation — or not a RUNTIME handle — is returned unchanged.
    ///
    /// This closes a drain hole a global `def` could otherwise open. `promote` is a
    /// no-op on an already-RUNTIME value, so `(def k v)` with `v` resident in the
    /// *draining* generation would store that stale handle straight into the shared
    /// globals table — an un-walked GC root — *after* migration moved the live globals
    /// off it, re-pinning a generation a process already reported clean for. If that
    /// process then exits, the drain union can go all-clean and free a still-referenced
    /// generation → dangling handle. Re-homing at def time keeps the invariant "no
    /// shared root points at the draining generation" intact, so the drain gate stays
    /// sound; it also stops migration's reconcile from mistaking a concurrent
    /// `(def k old-gen-value)` for a stale binding and clobbering it.
    ///
    /// Holds `promote_lock` (read) so an aging flip can't relocate the current
    /// generation between reading it and appending the copy — the same discipline
    /// `promote` uses. A no-op on the default single-generation path (every RUNTIME
    /// value is already current, so the fast-path check returns immediately).
    pub(crate) fn rehome_to_current(&self, v: Value) -> Value {
        let g = match runtime_gen_of(v) {
            Some(g) => g,
            None => return v,
        };
        if g == self.runtime.cur_gen() {
            return v;
        }
        let _guard = self
            .runtime
            .promote_lock
            .read()
            .unwrap_or_else(|e| e.into_inner());
        // Re-read the current generation under the lock: aging (which holds the write
        // lock) can't now flip it out from under the flush.
        let cur = self.runtime.cur_gen();
        if g == cur {
            return v;
        }
        let old_guard = self.runtime.gens[g].load();
        let dst_guard = self.runtime.gens[cur].load();
        let mut fwd = RuntimeForward::for_gens(g, cur);
        flush_rt_value(&old_guard, &dst_guard, &mut fwd, v)
    }

    pub fn migrate_live_globals(&self, old_gen: usize) -> usize {
        let dest_gen = self.runtime.cur_gen();
        if dest_gen == old_gen || self.runtime.gens[old_gen].load().is_empty() {
            return 0;
        }
        let old_guard = self.runtime.gens[old_gen].load();
        let dst_guard = self.runtime.gens[dest_gen].load();
        let old_slab: &CodeSlabs = &old_guard;
        let dst_slab: &CodeSlabs = &dst_guard;
        let mut fwd = RuntimeForward::for_gens(old_gen, dest_gen);

        // (a) Snapshot the shared roots still resident in `old_gen`: globals + the
        // declared `(sig …)` type-exprs. (Read lock — released before the flush.)
        let glob_snap: Vec<(Symbol, Value)> = self
            .runtime
            .globals_read()
            .iter()
            .filter(|(_, v)| value_in_gen(**v, old_gen))
            .map(|(k, v)| (*k, *v))
            .collect();
        let sig_snap: Vec<(Symbol, Value)> = self
            .runtime
            .declared_sigs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, v)| value_in_gen(**v, old_gen))
            .map(|(k, v)| (*k, *v))
            .collect();

        // (b) Trace + copy each into the current generation (off any table lock; the
        // boxcar append is concurrency-safe). Record (symbol, migrated handle).
        let glob_new: Vec<(Symbol, Value)> = glob_snap
            .iter()
            .map(|(k, v)| (*k, flush_rt_value(old_slab, dst_slab, &mut fwd, *v)))
            .collect();
        let sig_new: Vec<(Symbol, Value)> = sig_snap
            .iter()
            .map(|(k, v)| (*k, flush_rt_value(old_slab, dst_slab, &mut fwd, *v)))
            .collect();

        // (b2) Carry `(form-pos …)` entries for every forwarded pair to its new index,
        // so source positions resolve on the migrated code too (the old-gen entries stay
        // for as long as `old_gen` is retained). Diagnostics-only, mirroring compaction.
        {
            let mut p = self
                .runtime
                .positions
                .write()
                .unwrap_or_else(|e| e.into_inner());
            for (&old_idx, &new_idx) in fwd.pairs.iter() {
                if let Some(entry) = p.get(&(old_idx as usize)).cloned() {
                    p.insert(new_idx as usize, entry);
                }
            }
        }

        // (c) Reconcile under the write locks: install a migrated handle only where the
        // binding still points into `old_gen` (a concurrent redefinition wins).
        let mut migrated = 0usize;
        {
            let mut g = self.runtime.globals_write();
            for (k, new_v) in &glob_new {
                if g.get(k).is_some_and(|cur| value_in_gen(*cur, old_gen)) {
                    g.insert(*k, *new_v);
                    migrated += 1;
                }
            }
        }
        {
            let mut s = self
                .runtime
                .declared_sigs
                .write()
                .unwrap_or_else(|e| e.into_inner());
            for (k, new_v) in &sig_new {
                if s.get(k).is_some_and(|cur| value_in_gen(*cur, old_gen)) {
                    s.insert(*k, *new_v);
                }
            }
        }

        // Bump the version so every process's version-stamped caches (global_ic, the
        // call/global ICs, the shared JIT caches) re-resolve to the migrated handles;
        // clear this process's own caches eagerly (they may hold old_gen handles).
        self.runtime.version.fetch_add(1, Ordering::Relaxed);
        self.vm_cache.borrow_mut().clear();
        self.global_ic.borrow_mut().clear();
        self.vm_call_ics.borrow_mut().clear();
        self.vm_fast_links.borrow_mut().clear();
        #[cfg(debug_assertions)]
        self.dbg_site_pos.borrow_mut().clear();
        self.vm_global_ics.borrow_mut().clear();
        migrated
    }

    /// **RUNTIME collector — Stage 4 (free a drained generation).** Reclaim the
    /// aged-out generation `old_gen` — which the Stage-3 union has confirmed no live
    /// process references ([`crate::process::old_gen_drained`]) — by storing a fresh
    /// empty slab into its [`ArcSwap`] slot. The old `Arc<CodeSlabs>` drops once the
    /// last reader guard releases it (there are none, by the drain), reclaiming the
    /// whole superseded generation at once — no per-cell trace, no handle rewrite.
    /// Ends the drain. Returns `false` (a no-op) if `old_gen` is the current
    /// generation or already empty.
    ///
    /// Cross-process cache coherence:
    /// - `version` is bumped, so every process's `global_ic` / call-&-global ICs and
    ///   the shared JIT caches (all `version`/epoch-stamped) miss and re-resolve —
    ///   none can hand back a handle into the freed slab.
    /// - `free_epoch` is bumped, so every process clears its `vm_cache` (keyed on raw
    ///   handle bits, not `version`) on its next lookup — a reused slot's
    ///   bit-identical handle can't hit a stale compiled body.
    ///
    /// This process's own caches are cleared eagerly; the shared JIT caches too.
    ///
    /// SAFETY of the store: the drain guarantees no live process holds a handle into
    /// `old_gen`, so no accessor is (or will be) borrowing into its slab — the old
    /// `Arc` has no live [`SlabRef`] guard and drops without invalidating a read. (A
    /// concurrent `load()` on the slot is still memory-safe: `ArcSwap` hands out
    /// either the old or new `Arc`, both valid — the drain only rules out a *semantic*
    /// use of an old-gen handle.)
    pub fn free_runtime_gen(&self, old_gen: usize) -> bool {
        if old_gen == self.runtime.cur_gen() || self.runtime.gens[old_gen].load().is_empty() {
            return false;
        }
        // Drop the whole generation: store a fresh empty slab; the old `Arc` releases
        // when the last (already none, by the drain) reader guard does.
        self.runtime.gens[old_gen].store(Arc::new(CodeSlabs::default()));
        // Replaced `gens[old_gen]`'s `Arc` — bump the pinned-read cache version so every
        // process re-`load_full`s instead of cloning its now-stale cached `Arc` (Release,
        // after the store, so a consumer that sees the new version also sees the new slab).
        self.runtime.gen_version.fetch_add(1, Ordering::Release);
        // Invalidate the version-stamped caches (global_ic, call/global ICs, JIT)…
        self.runtime.version.fetch_add(1, Ordering::Relaxed);
        // …and the handle-keyed vm_cache (via free_epoch) across every process.
        self.runtime.free_epoch.fetch_add(1, Ordering::Relaxed);
        // Eagerly clear this process's caches (peers clear lazily on their next use).
        self.vm_cache.borrow_mut().clear();
        self.seen_free_epoch
            .set(self.runtime.free_epoch.load(Ordering::Relaxed));
        self.global_ic.borrow_mut().clear();
        self.vm_call_ics.borrow_mut().clear();
        self.vm_fast_links.borrow_mut().clear();
        #[cfg(debug_assertions)]
        self.dbg_site_pos.borrow_mut().clear();
        self.vm_global_ics.borrow_mut().clear();
        // Drop the shared JIT-code caches (the version bump already epoch-invalidated
        // them; clearing reclaims the memory and prevents a recycled id lingering).
        if let Ok(mut c) = self.runtime.jit_code_cache.write() {
            c.clear();
        }
        if let Ok(mut c) = self.runtime.jit_inline_cache.write() {
            c.clear();
        }
        // The drain is complete.
        self.end_gen_drain();
        true
    }

    /// **RUNTIME collector — Stage 3 (cooperative liveness probe).** Is generation
    /// `gen` still *referenced* by any live code, as seen from this process? Walks
    /// the shared roots (the global bindings + declared `(sig …)` type-exprs) and
    /// this process's own private roots — the operand/env stack, dynamic bindings,
    /// both LOCAL heap generations, and the live VM arms mid-execution — following
    /// RUNTIME handles transitively, and returns `true` the instant it reaches a
    /// live handle in generation `gen`. Read-only: moves and frees nothing.
    ///
    /// This is the per-process half of the Stage 3 union that decides when an
    /// aged-out old generation may be freed (Stage 4): the generation is dead only
    /// when *every* live process (and the shared globals) reports `false`. For a
    /// single-process runtime this heap sees the whole picture, so its answer is
    /// exact. A process holding an old-generation closure in a local variable,
    /// mid-call, or captured in data keeps that generation pinned — exactly
    /// Erlang's "old code lives until no process still runs it".
    ///
    /// The per-process **caches** (`vm_cache`/`global_ic`/…) are deliberately *not*
    /// scanned: they hold RUNTIME handles too, but rebuild lazily, so Stage 4 clears
    /// them when it frees a generation (as [`runtime_collect`] already does) rather
    /// than treating a cached handle as a live pin. Only the live VM arms — which
    /// are mid-execution and can't be cleared — are scanned.
    pub fn runtime_gen_referenced(&self, gen: usize) -> bool {
        self.runtime_gen_referenced_impl(gen, true)
    }

    /// The **private-only** reachability probe: like [`runtime_gen_referenced`] but
    /// *without* seeding the shared globals + `(sig …)` roots — it walks only this
    /// process's own roots, local heap, and live VM arms. Used by the drain self-report
    /// ([`Self::report_gen_liveness`]): the drain arms only after `migrate_live_globals`
    /// moved every value off the draining generation, and post-aging no shared root can
    /// come to point at it again, so the shared roots provably never reach it — including
    /// them is O(globals) cost with no effect. The only way a process can genuinely pin
    /// the generation is a handle it captured *privately* before the migration, which
    /// this probe still catches.
    fn runtime_gen_referenced_private(&self, gen: usize) -> bool {
        // An empty generation slot is trivially dead — the common case.
        if self.runtime.gens[gen].load().is_empty() {
            return false;
        }
        let mut visited: HashSet<(usize, usize)> = HashSet::new();
        let mut visited_env: HashSet<(usize, usize)> = HashSet::new();
        let mut work: Vec<Value> = Vec::new();
        let mut env_work: Vec<EnvId> = Vec::new();

        // Phase 1 (cheap: private roots + live arms) always runs — a process that
        // *becomes* clean by dropping a root re-reports clean at its very next safepoint.
        if self.seed_phase1_and_walk(
            gen,
            false,
            &mut work,
            &mut env_work,
            &mut visited,
            &mut visited_env,
        ) {
            return true;
        }

        // Phase 2 (expensive: the whole LOCAL heap) is **throttled once it has found this
        // process dirty for the current drain epoch**. A process pinned by a RUNTIME handle
        // embedded in its LOCAL data (e.g. a large live message backlog carrying a
        // closure-as-data) is dirty until that data dies; without throttling it re-walks its
        // entire O(heap) graph on *every* safepoint for the whole epoch — quadratic, and the
        // dominant cost of a `spawn` fan-out under a lingering drain (a ~300× regression:
        // the root, dirty via Phase 2 over a growing 65k-cell heap, re-walked it tens of
        // thousands of times in one epoch). Re-validating only every `P2_REVALIDATE_STRIDE`
        // safepoints bounds that to 1/stride. Sound: a stale-dirty verdict only *delays*
        // drain completion (never fabricates a clean ack), and a process that becomes clean
        // re-validates within a stride. Only Phase-2 dirtiness arms the throttle — a process
        // dirty via Phase 1 (running old-gen code) never sets it, so the cheap re-check above
        // still reports its transition to clean immediately (the drain-completion tests rely
        // on that promptness). The epoch key resets the throttle when a new drain arms.
        let epoch = self.runtime.drain_epoch.load(Ordering::Relaxed);
        if self.p2_dirty_epoch.get() == epoch {
            let t = self.p2_dirty_tick.get().wrapping_add(1);
            self.p2_dirty_tick.set(t);
            if !t.is_multiple_of(P2_REVALIDATE_STRIDE) {
                return true; // stale-dirty: skip the O(heap) re-walk this safepoint
            }
        }
        let dirty = self.seed_phase2_and_walk(
            gen,
            &mut work,
            &mut env_work,
            &mut visited,
            &mut visited_env,
        );
        if dirty {
            // Arm / keep the re-validation throttle for this epoch.
            if self.p2_dirty_epoch.get() != epoch {
                self.p2_dirty_epoch.set(epoch);
                self.p2_dirty_tick.set(0);
            }
        } else {
            // Clean via Phase 2 — disarm so a later re-dirty re-walks at once. (Belt-and-
            // braces: the caller acks a clean process, and the ack `Cell` then short-circuits
            // this whole probe for the rest of the epoch.)
            self.p2_dirty_epoch.set(u64::MAX);
        }
        dirty
    }

    /// The authoritative reachability probe used by the drain-completion / free path
    /// ([`runtime_gen_referenced`]): both phases, seeding the shared globals + `(sig …)`
    /// roots too, and **never throttled** — it decides an actual free, so it always walks.
    fn runtime_gen_referenced_impl(&self, gen: usize, include_shared: bool) -> bool {
        // An empty generation slot is trivially dead — the common case (normal runs
        // never age, so `gens[1]` stays empty and this short-circuits).
        if self.runtime.gens[gen].load().is_empty() {
            return false;
        }
        // `visited` keys on (gen, index): the two generations share one index space,
        // so a bare slab index would conflate gen-0 #5 with gen-1 #5.
        let mut visited: HashSet<(usize, usize)> = HashSet::new();
        let mut visited_env: HashSet<(usize, usize)> = HashSet::new();
        let mut work: Vec<Value> = Vec::new();
        let mut env_work: Vec<EnvId> = Vec::new();
        self.seed_phase1_and_walk(
            gen,
            include_shared,
            &mut work,
            &mut env_work,
            &mut visited,
            &mut visited_env,
        ) || self.seed_phase2_and_walk(
            gen,
            &mut work,
            &mut env_work,
            &mut visited,
            &mut visited_env,
        )
    }

    /// **Phase 1** of the RUNTIME-generation reachability probe — the CHEAP roots, walked
    /// to fixpoint with an early exit. The private roots and live VM arms are O(process
    /// stack + arm count); the local heap ([`seed_phase2_and_walk`]) is O(heap size). A
    /// drain's overwhelmingly common pin is a process *running* old-gen code — its live arm
    /// sits here — so checking these first lets a pinning process's per-safepoint report
    /// short-circuit without paying the O(heap) seed at all. The two-batch split is
    /// semantics-preserving: same seed set, same transitive rule; the shared `visited` sets
    /// carry across so Phase 2 never re-walks Phase 1's graph.
    fn seed_phase1_and_walk(
        &self,
        gen: usize,
        include_shared: bool,
        work: &mut Vec<Value>,
        env_work: &mut Vec<EnvId>,
        visited: &mut HashSet<(usize, usize)>,
        visited_env: &mut HashSet<(usize, usize)>,
    ) -> bool {
        // --- Shared roots: globals + declared `(sig …)` type-exprs. Skipped by the
        // private probe (the drain self-report) — see `runtime_gen_referenced_private`. ---
        if include_shared {
            work.extend(self.runtime.globals_read().values().copied());
            work.extend(
                self.runtime
                    .declared_sigs
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .values()
                    .copied(),
            );
        }
        // --- This process's private roots (operand/env stack, dynamics). ---
        work.extend(self.roots.iter().copied());
        env_work.extend(self.env_roots.iter().copied());
        work.extend(self.dynamics.iter().map(|(_, v)| *v));
        // --- Live VM arms mid-execution: RUNTIME literals baked into Const/MakeClosure
        // (the one holder off the GC root graph, mirroring compaction's step 3b). Read
        // them via the arm-handle visitor, returning each value unchanged. ---
        let live_arms = self.live_vm_arms.clone();
        for arm in &live_arms {
            crate::eval::compile::rewrite_arm_handles(arm, &mut |v| {
                work.push(v);
                v
            });
        }
        self.walk_reaches_gen(gen, work, env_work, visited, visited_env)
    }

    /// **Phase 2** of the RUNTIME-generation reachability probe — the LOCAL heap (both
    /// generations): immutable data can embed a captured RUNTIME closure handle, the one
    /// place the shared-root walk can't reach. Only worth running when the cheap roots
    /// (Phase 1) didn't already pin the gen. Every cell is seeded directly (a LOCAL handle
    /// is never gen-tagged RUNTIME, so the walk only ever follows RUNTIME sub-handles).
    fn seed_phase2_and_walk(
        &self,
        gen: usize,
        work: &mut Vec<Value>,
        env_work: &mut Vec<EnvId>,
        visited: &mut HashSet<(usize, usize)>,
        visited_env: &mut HashSet<(usize, usize)>,
    ) -> bool {
        for slabs in [&self.local, &self.old] {
            for (a, b) in slabs.pairs.iter() {
                work.push(*a);
                work.push(*b);
            }
            for vec in slabs.vectors.iter() {
                work.extend(vec.iter().copied());
            }
            for node in slabs.maps.iter() {
                for (k, v) in node.data.iter() {
                    work.push(*k);
                    work.push(*v);
                }
            }
            for cl in slabs.closures.iter() {
                for arm in cl.arms.iter() {
                    work.extend(arm.body.iter().copied());
                    work.extend(arm.optionals.iter().map(|(_, d)| *d));
                }
                if let Some(e) = cl.env {
                    env_work.push(e);
                }
            }
            for fr in slabs.envs.iter() {
                work.extend(fr.vars.iter().map(|(_, v)| *v));
                if let Some(p) = fr.parent {
                    env_work.push(p);
                }
            }
        }
        self.walk_reaches_gen(gen, work, env_work, visited, visited_env)
    }

    /// Drive the transitive reachability walk over the seeded `work`/`env_work` lists to
    /// fixpoint, following every RUNTIME sub-handle; return `true` the instant a handle in
    /// generation `gen` is seen. Shared by both phases of `runtime_gen_referenced_impl`
    /// (the `visited` sets carry across phases so Phase 2 never re-walks Phase 1's graph).
    fn walk_reaches_gen(
        &self,
        gen: usize,
        work: &mut Vec<Value>,
        env_work: &mut Vec<EnvId>,
        visited: &mut HashSet<(usize, usize)>,
        visited_env: &mut HashSet<(usize, usize)>,
    ) -> bool {
        // --- Transitive walk. Detect generation `gen`; follow every RUNTIME sub-handle
        // (a gen-current closure can embed a gen-`gen` handle in a body/env). ---
        loop {
            while let Some(env) = env_work.pop() {
                if env == EnvId::GLOBAL || env.region() != RUNTIME {
                    continue;
                }
                if env.code_gen() == gen {
                    return true;
                }
                if visited_env.insert((env.code_gen(), env.index())) {
                    let frame = self.env_frame(env);
                    work.extend(frame.vars.iter().map(|(_, v)| *v));
                    if let Some(p) = frame.parent {
                        env_work.push(p);
                    }
                }
            }
            let Some(v) = work.pop() else { break };
            match v.unpack() {
                ValueRef::Fn(id) | ValueRef::Macro(id) if id.region() == RUNTIME => {
                    if id.code_gen() == gen {
                        return true;
                    }
                    if visited.insert((id.code_gen(), id.index())) {
                        let cl = self.closure(id);
                        for arm in cl.arms.iter() {
                            work.extend(arm.body.iter().copied());
                            work.extend(arm.optionals.iter().map(|(_, d)| *d));
                        }
                        if let Some(e) = cl.env {
                            env_work.push(e);
                        }
                    }
                }
                ValueRef::Pair(id) if id.region() == RUNTIME => {
                    if id.code_gen() == gen {
                        return true;
                    }
                    if visited.insert((id.code_gen(), id.index())) {
                        let (h, t) = self.pair(id);
                        work.push(h);
                        work.push(t);
                    }
                }
                ValueRef::Vector(id) if id.region() == RUNTIME => {
                    if id.code_gen() == gen {
                        return true;
                    }
                    if visited.insert((id.code_gen(), id.index())) {
                        work.extend(self.vector(id).iter().copied());
                    }
                }
                ValueRef::Map(id) | ValueRef::Set(id) if id.region() == RUNTIME => {
                    if id.code_gen() == gen {
                        return true;
                    }
                    if visited.insert((id.code_gen(), id.index())) {
                        self.fold_entries(id, &mut |k, val| {
                            work.push(k);
                            work.push(val);
                        });
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// **RUNTIME collector — Stage 3b (begin a cooperative drain).** Arm a drain of
    /// the aged-out generation `old_gen`: bump the strictly-monotonic drain epoch
    /// (so no ack from a prior drain can count) and clear the ack table, so every
    /// live process must re-report clean before the generation is considered dead.
    /// Returns the new epoch. Shared via the runtime `Arc`, so any process observes
    /// the drain at its next [`report_gen_liveness`](Self::report_gen_liveness).
    ///
    /// Ordered so a concurrent reader sees a consistent drain: `drain_gen` and the
    /// epoch (and the ack clear) are published *before* `drain_active` flips true.
    pub fn begin_gen_drain(&self, old_gen: usize) -> u64 {
        let rt = &self.runtime;
        rt.drain_gen.store(old_gen, Ordering::Relaxed);
        let epoch = rt.drain_epoch.fetch_add(1, Ordering::Relaxed) + 1;
        {
            // Clear the ack table AND reset the O(1) completion counter to 0 under the
            // SAME write lock, so a concurrent `report_gen_liveness` insert+increment
            // (also under this lock) can't interleave between the two and leave a live
            // ack uncounted — an undercount would hold the gate shut and leak the gen.
            let mut acks = rt.drain_acks.write().unwrap_or_else(|e| e.into_inner());
            acks.clear();
            rt.drain_acked.store(0, Ordering::Relaxed);
        }
        rt.drain_active.store(true, Ordering::Release);
        // Arm the arming process's self-report to fire on its very next safepoint (rather than
        // up to a stride later), so a drain it starts completes promptly — the drain-completion
        // path drives progress off this process. Other processes fire within a stride, which is
        // ample for a lingering fan-out. `wrapping_sub(1)` so the next `drain_report_due` tick
        // lands on a stride boundary.
        self.drain_report_tick
            .set(DRAIN_REPORT_STRIDE.wrapping_sub(1));
        epoch
    }

    /// **RUNTIME collector — Stage 3b (end a drain).** Disarm the current drain
    /// (Stage 4 calls this once the generation is freed, or to abandon a drain). The
    /// epoch is left monotonically advanced; the ack table is cleared. After this,
    /// [`report_gen_liveness`](Self::report_gen_liveness) is inert again.
    pub fn end_gen_drain(&self) {
        let rt = &self.runtime;
        rt.drain_active.store(false, Ordering::Release);
        rt.drain_acks
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// Is a cooperative generation drain currently armed? A single relaxed atomic
    /// load — the cheap gate the eval safepoint checks before reporting (Stage 3c),
    /// so the always-case (no drain) costs almost nothing.
    #[inline]
    pub fn drain_active(&self) -> bool {
        self.runtime.drain_active.load(Ordering::Acquire)
    }

    /// The O(1) drain-completion gate's ack count (see the `drain_acked` field) —
    /// distinct processes that have reported clean for the current drain epoch. The
    /// process layer compares it against the live-process count to skip the
    /// authoritative parked scan while the drain clearly can't be complete. Relaxed:
    /// a racy read only mistimes the scan, never the free.
    #[inline]
    pub fn drain_acked_count(&self) -> u64 {
        self.runtime.drain_acked.load(Ordering::Relaxed)
    }

    /// Account a process exit against the O(1) drain-completion gate: if the exiting
    /// process had acked the current epoch, drop its ack so `drain_acked` keeps
    /// meaning "distinct *live* processes that reported clean". Without this the count
    /// would drift above the live set under churn (many short-lived clean processes)
    /// and force the authoritative scan on every check. A no-op when no drain is armed.
    /// Sound regardless: the count only gates *when* the scan runs, never the free.
    pub fn drain_note_exit(&self, pid: u64) {
        let rt = &self.runtime;
        if !rt.drain_active.load(Ordering::Acquire) {
            return;
        }
        let epoch = rt.drain_epoch.load(Ordering::Relaxed);
        let mut acks = rt.drain_acks.write().unwrap_or_else(|e| e.into_inner());
        if acks.remove(&pid) == Some(epoch) {
            rt.drain_acked.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Has the RUNTIME region grown (a closure minted via `promote_closure`) since
    /// the eval safepoint last ran its `rt_gc_due` probe? A single relaxed load —
    /// the cheap gate that lets a def-free hot loop skip the probe entirely.
    #[inline]
    pub fn rt_dirty(&self) -> bool {
        self.runtime.rt_dirty.load(Ordering::Relaxed)
    }

    /// Clear the RUNTIME-churn dirty bit — called by the safepoint right before it
    /// runs the `rt_gc_due` probe, so the next mint re-arms it. Only ever stored on
    /// the (rare) dirty path, keeping the flag's cache line Shared in steady state.
    #[inline]
    pub fn rt_dirty_clear(&self) {
        self.runtime.rt_dirty.store(false, Ordering::Relaxed);
    }

    /// The generation currently being drained (meaningful only while
    /// [`drain_active`](Self::drain_active)).
    pub fn drain_gen(&self) -> usize {
        self.runtime.drain_gen.load(Ordering::Relaxed)
    }

    /// **RUNTIME collector — Stage 3b (this process's cooperative report).** If a
    /// drain is armed, probe whether *this* process still references the draining
    /// generation ([`runtime_gen_referenced`](Self::runtime_gen_referenced)) and
    /// record the result under `pid`: a **clean** process acks the current epoch; a
    /// process that still references the generation drops its ack, so it pins the
    /// generation until it reports clean at a later safepoint. A no-op when no drain
    /// is armed. Called at the eval safepoint and just before a process parks
    /// (Stage 3c wires those); safe to call from any of the runtime's processes
    /// (each writes only its own pid's entry, under the shared lock).
    /// Should this safepoint run its drain self-report? Advances the per-heap tick and returns
    /// true once every [`DRAIN_REPORT_STRIDE`] frames — a `Cell` read/write, **no shared
    /// atomic**, so a throttled frame is nearly free. See the const for why every-frame
    /// reporting dominated a lingering-drain fan-out and why throttling is sound. The arming
    /// process resets this tick in [`begin_gen_drain`](Self::begin_gen_drain) so completion
    /// stays prompt. The caller ([`crate::process::report_drain_liveness`]) has already
    /// established a drain is armed.
    #[inline]
    pub fn drain_report_due(&self) -> bool {
        let t = self.drain_report_tick.get().wrapping_add(1);
        self.drain_report_tick.set(t);
        t.is_multiple_of(DRAIN_REPORT_STRIDE)
    }

    pub fn report_gen_liveness(&self, pid: u64) {
        let rt = &self.runtime;
        if !rt.drain_active.load(Ordering::Acquire) {
            return;
        }
        let epoch = rt.drain_epoch.load(Ordering::Relaxed);
        // Already reported clean for this epoch? Skip the re-walk. Sound by the
        // no-new-refs invariant: once a process is clean of the draining generation,
        // it *stays* clean — post-aging all `def`/`promote` land in the current
        // generation, globals were migrated off `old_gen` before the drain armed, and
        // an old-gen handle can never arrive by message (messages deep-copy, promoting
        // closures into the receiver's current generation). So a clean ack needn't be
        // re-earned each safepoint — this bounds a process to one liveness walk per drain.
        //
        // A single **local** `Cell` read, no lock. It subsumes the shared `drain_acks`
        // table: this heap's `Cell` is set on *every* ack of `epoch` — whether by this
        // process's own safepoint OR by the parked-process inspector calling this on the
        // heap's behalf while it was suspended (the inspector runs on the quiescent parked
        // heap and sets the same `Cell`). So `drain_acks[pid] == epoch` ⟺ this `Cell` ==
        // epoch; reading the shared table here would be pure redundant work — and, taken on
        // *every* safepoint of *every* process that pins the draining generation until it
        // exits (a `spawn` worker whose body lives in the drained generation), that
        // per-frame `drain_acks` **read lock** — contended across the whole worker pool —
        // was what dominated a fan-out drain (the multi-process spawn regression). Dropping
        // it leaves only the private probe below, which is O(this process's own state).
        if self.acked_drain_epoch.get() == epoch {
            return;
        }
        let gen = rt.drain_gen.load(Ordering::Relaxed);
        // Use the **private** reachability probe (this process's own roots / local heap /
        // live arms only), NOT the full one that also seeds every shared global + `(sig)`.
        // The drain armed only *after* `migrate_live_globals` moved every value off the
        // draining generation, and post-aging no global can come to point at it again
        // (the `gen_drained` soundness note), so the shared roots provably never reach the
        // drained generation — seeding them is pure cost. That cost is O(globals): re-run
        // by *every* still-pinning process on *every* safepoint over a large accumulated
        // global set, it is the whole-registry work that livelocked a many-process drain
        // (the multigen suite hang). The private probe is O(this process's own
        // state), so a pinning process's per-safepoint report is cheap and no throttle is
        // needed — and it still catches the only way a process can actually pin the
        // generation: a handle it captured privately before the migration.
        let clean = !self.runtime_gen_referenced_private(gen);
        if clean {
            {
                let mut acks = rt.drain_acks.write().unwrap_or_else(|e| e.into_inner());
                // Count this ack toward the O(1) completion gate exactly once per epoch.
                // Under the write lock so a concurrent self-report / parked-inspector for
                // the same pid can't double-count. `insert` returns the prior value: a new
                // clean ack (prior != this epoch) bumps `drain_acked`.
                if acks.insert(pid, epoch) != Some(epoch) {
                    rt.drain_acked.fetch_add(1, Ordering::Relaxed);
                }
            }
            // Cache the clean ack locally so subsequent frames short-circuit at the
            // `Cell` check above without re-taking the `drain_acks` read lock.
            self.acked_drain_epoch.set(epoch);
        }
        // Dirty → take no write lock. A process reaching here holds no current-epoch ack
        // (the fast path above returns if it did) and `begin_gen_drain` cleared the table
        // at epoch start, so there is nothing to remove — simply not acking already pins
        // the generation. The former `acks.remove(&pid)` here was a no-op that still took
        // the `drain_acks` *write* lock on every safepoint of every still-pinning process:
        // P-way writer serialization (a churny multi-process drain). Sound by clean-stays-clean
        // (ADR-091): post-migrate no process can newly acquire an old-gen handle, so a
        // process that ever acked clean never becomes dirty again and needs no removal; a
        // fresh drain bumps the epoch and clears the table, so no stale ack survives.
    }

    /// **RUNTIME collector — Stage 3b (the union answer).** Is the draining
    /// generation dead — i.e. has *every* currently-live process reported clean for
    /// the current drain epoch? The caller supplies the live-pid set (the process
    /// layer reads it from the scheduler registry; keeping the enumeration out of
    /// `core` preserves the layering). Returns `false` if no drain is armed. Once
    /// this is `true`, Stage 4 may free the generation (after clearing the per-process
    /// caches that hold — but never pin — its handles).
    ///
    /// Soundness: a process acks clean only when its probe (which includes the shared
    /// globals) sees no reference to the generation. After aging, new code only ever
    /// lands in the *current* generation, so a global can never come to point at the
    /// drained one again — a clean ack therefore stays valid, and a process cannot
    /// re-acquire a reference except by spawning a child (which enters the live set
    /// un-acked, keeping the answer `false` until it too reports clean).
    pub fn gen_drained(&self, live_pids: &[u64]) -> bool {
        let rt = &self.runtime;
        if !rt.drain_active.load(Ordering::Acquire) {
            return false;
        }
        let epoch = rt.drain_epoch.load(Ordering::Relaxed);
        let acks = rt.drain_acks.read().unwrap_or_else(|e| e.into_inner());
        live_pids.iter().all(|pid| acks.get(pid) == Some(&epoch))
    }

    /// **RUNTIME collector — Step 2a (out-of-place evacuation).** Trace the live
    /// RUNTIME code reachable from the global bindings + this process's operand roots
    /// and *copy* it into a fresh [`CodeSlabs`], returning `(new_slabs, forwarding)`.
    /// Installs nothing and mutates nothing live — purely an evacuation preview, the
    /// safe foundation for the single-process in-place compaction swap it feeds. (The
    /// *shared*-runtime case is reclaimed by the 2-generation collector instead —
    /// ADR-091 — not by a stop-the-world.) Single-process root view.
    fn runtime_evacuate(&self) -> (CodeSlabs, RuntimeForward) {
        let new = CodeSlabs::default();
        // Same-generation compaction: copy from the current gen back into itself.
        let cg = self.runtime.cur_gen();
        let mut fwd = RuntimeForward::for_gens(cg, cg);
        let mut roots: Vec<Value> = self.runtime.globals_read().values().copied().collect();
        roots.extend(self.roots.iter().copied());
        let cur = self.runtime.cur_code();
        for r in roots {
            flush_rt_value(&cur, &new, &mut fwd, r);
        }
        (new, fwd)
    }

    /// Test/diagnostic hook for Step 2a: `(total RUNTIME closures, live closures in
    /// the evacuated region, evacuated-region verifier passed)`. `live` should equal
    /// [`runtime_live_closure_count`](Self::runtime_live_closure_count) and the
    /// verifier ([`verify_rt_slabs`]) confirms no handle dangles outside the new,
    /// compacted region.
    pub fn runtime_evacuate_check(&self) -> (usize, usize, bool) {
        let total = self.runtime.cur_code().closures.count();
        let (new, _fwd) = self.runtime_evacuate();
        let live = new.closures.count();
        let ok = verify_rt_slabs(&new);
        (total, live, ok)
    }

    /// **RUNTIME collector — Step 2b (in-place compaction).** Reclaim superseded
    /// RUNTIME code by compacting the region to its live set and rewriting every
    /// reference to it. Returns `Some((before, after))` closure counts, or `None` if
    /// it couldn't run (see the gate). `docs/runtime-collector-exploration.md`.
    ///
    /// **Safety gate — `Arc::get_mut`.** Runs only when *this* heap uniquely owns the
    /// runtime `Arc` — i.e. no other process/thread can be reading the code region
    /// concurrently. That makes it sound without any stop-the-world; when the runtime is
    /// shared it returns `None` (the 2-generation collector reclaims that case instead —
    /// ADR-091). The eval safepoint calls it automatically once churn
    /// crosses [`rt_gc_threshold`](Self::rt_gc_threshold)
    /// ([`maybe_runtime_collect`](Self::maybe_runtime_collect)); the
    /// `(runtime-collect)` builtin is the explicit/force form.
    ///
    /// One pass, evacuate-and-rewrite: every RUNTIME handle in the globals, both
    /// LOCAL generations (`local`+`old`), the operand/env roots, and the dynamic-
    /// binding stack is replaced with its evacuated index ([`flush_rt_value`] copies
    /// the sub-graph into a fresh `CodeSlabs` on first sight). The per-process
    /// `vm_cache` (keys + cached bodies are RUNTIME handles) and `global_ic` (cached
    /// RUNTIME values) are *cleared* — both rebuild lazily. A debug `verify_rt_slabs`
    /// asserts the result has no dangling handle.
    pub fn runtime_collect(&mut self) -> Option<(usize, usize)> {
        self.runtime_collect_with(&mut [], &mut [])
    }

    /// As [`runtime_collect`](Self::runtime_collect), but also rewrites RUNTIME
    /// handles in the caller-supplied `extra_roots`/`extra_envs`. The automatic
    /// safepoint path ([`maybe_runtime_collect`](Self::maybe_runtime_collect))
    /// passes the eval loop's live `expr`/`env` — the one pair that, at the
    /// loop-top safepoint, isn't yet on the operand stack (`roots`/`env_roots`),
    /// exactly mirroring how the LOCAL collect relocates `expr`/`env`. With empty
    /// slices it is precisely `runtime_collect`.
    pub(crate) fn runtime_collect_with(
        &mut self,
        extra_roots: &mut [Value],
        extra_envs: &mut [EnvId],
    ) -> Option<(usize, usize)> {
        // Bail while a globals snapshot is outstanding (KI-6): relocating RUNTIME handles
        // now would strand the off-graph snapshot `restore_globals` will reinstall. The
        // single choke point for BOTH the auto safepoint path (via `rt_gc_due`) and a
        // manual `(runtime-collect)`, so an explicit collect inside `%isolate` is safe too.
        if self.rt_collect_block.get() > 0 {
            return None;
        }
        // Single-process compaction only runs in generation 0 (the pre-aging /
        // reclaimed-back-to-0 state). After aging (`current_gen == 1`) the
        // reclamation strategy is whole-generation freeing (the multi-process
        // path), not compaction — and the compactor's `flush_rt_*` mint gen-0
        // handles, which would be wrong for a gen-1 region. Bail (a safe no-op).
        if self.runtime.cur_gen() != 0 {
            return None;
        }
        // Bail unless we uniquely own the runtime region (no concurrent readers).
        Arc::get_mut(&mut self.runtime)?;
        // Stall trace (BROOD_STALL_MS): RUNTIME-region compaction is a prime gameplay-lag
        // suspect (it copies the whole live shared-code region). Log if it's slow.
        let _sg = stall_guard("runtime-compact");
        let before = self.runtime.cur_code().closures.count();
        // Compact the *current* generation in place (single-process). Swap it out
        // (owned `Arc`) so we can read it while mutating self's LOCAL slabs without a
        // borrow conflict; the slot holds a fresh empty slab meanwhile. Sound because
        // we uniquely own the runtime (the `Arc::get_mut` gate above), so no other
        // process can be reading the swapped-out slab.
        let cur = self.runtime.cur_gen();
        let old_code = self.runtime.gens[cur].swap(Arc::new(CodeSlabs::default()));
        let new = CodeSlabs::default();
        // In-place compaction copies the current generation back into itself.
        let mut fwd = RuntimeForward::for_gens(cur, cur);

        // 1. Globals (the primary roots).
        {
            let rt = Arc::get_mut(&mut self.runtime).unwrap();
            let mut g = rt.globals.write().unwrap_or_else(|e| e.into_inner());
            for v in g.values_mut() {
                *v = flush_rt_value(&old_code, &new, &mut fwd, *v);
            }
        }
        // 1b. Declared `(sig …)` type-expressions — also promoted RUNTIME handles held
        // off the graph (a `SymbolMap<Value>` beside `globals`). Without this rewrite a
        // compaction relocates the type-expr out from under the stored handle, so the
        // checker's `sig_of` later reads a garbage form (e.g. `(int -> int)` → `(i 1)`).
        // Evacuated exactly like a global. See [`RuntimeCode::declared_sigs`].
        {
            let rt = Arc::get_mut(&mut self.runtime).unwrap();
            let mut s = rt.declared_sigs.write().unwrap_or_else(|e| e.into_inner());
            for v in s.values_mut() {
                *v = flush_rt_value(&old_code, &new, &mut fwd, *v);
            }
        }
        // 2. This process's roots, env roots, and dynamic bindings.
        for v in self.roots.iter_mut() {
            *v = flush_rt_value(&old_code, &new, &mut fwd, *v);
        }
        for e in self.env_roots.iter_mut() {
            *e = flush_rt_env(&old_code, &new, &mut fwd, *e);
        }
        for (_, v) in self.dynamics.iter_mut() {
            *v = flush_rt_value(&old_code, &new, &mut fwd, *v);
        }
        // ...and the caller's extra live roots (the auto path's `expr`/`env`).
        for v in extra_roots.iter_mut() {
            *v = flush_rt_value(&old_code, &new, &mut fwd, *v);
        }
        for e in extra_envs.iter_mut() {
            *e = flush_rt_env(&old_code, &new, &mut fwd, *e);
        }
        // 3. The LOCAL heap (both generations) — any slot may embed a RUNTIME handle.
        rewrite_local_rt_handles(&mut self.local, &old_code, &new, &mut fwd);
        rewrite_local_rt_handles(&mut self.old, &old_code, &new, &mut fwd);
        // 3b. The LIVE compiled-VM arms on this process's execution stack. Their
        // `Arc`'d node trees are off the GC root graph (and held by `&Node` during
        // execution, so the `Arc` can't be swapped), but they embed promoted RUNTIME
        // handles in their `Const`/`MakeClosure` literals — the one holder the root
        // walk above can't reach. Rewrite them in place with the SAME `fwd`, so a
        // literal shared with an already-evacuated global maps to the same new index,
        // and a literal reachable ONLY from a live arm is evacuated here on first
        // sight. Clone the registry (cheap `Arc` bumps) to drop the `&self` borrow
        // before the `&old_code`/`&mut fwd` closure. This is what lets the safepoint
        // compact even while the VM is mid-call (no deferral, region stays bounded).
        let live_arms = self.live_vm_arms.clone();
        for arm in &live_arms {
            crate::eval::compile::rewrite_arm_handles(arm, &mut |v| {
                flush_rt_value(&old_code, &new, &mut fwd, v)
            });
        }
        // 3c. Remap RUNTIME form-position keys through the same forwarding. `positions`
        // is keyed by raw pair slab index and lives off the graph, so — like
        // `declared_sigs` — a relocation strands its keys on recycled pairs; without this
        // `(form-pos …)` / source-location diagnostics return a stranger's position (or
        // none) after a compaction. `fwd.pairs` is fully populated by the walks above.
        // Diagnostics-only, but the same stale-after-relocation class. Entries whose pair
        // didn't survive are dropped.
        {
            let rt = Arc::get_mut(&mut self.runtime).unwrap();
            let mut p = rt.positions.write().unwrap_or_else(|e| e.into_inner());
            let old = std::mem::take(&mut *p);
            for (old_idx, val) in old {
                if let Some(&new_idx) = fwd.pairs.get(&(old_idx as u32)) {
                    p.insert(new_idx as usize, val);
                }
            }
        }
        // 4. Drop the per-process caches that key on / hold RUNTIME handles. (The live
        // arms above are NOT cleared — they're mid-execution; only the lookup cache is.)
        // The call-site IC table resets wholesale (sites are re-allocated when the
        // cleared `vm_cache` recompiles its node trees); a still-live arm's old site
        // id then either falls out of range (ignored) or aliases a fresh site, which
        // the probe's sym+argc+epoch validation makes harmless (ADR-096).
        self.vm_cache.borrow_mut().clear();
        self.global_ic.borrow_mut().clear();
        self.vm_call_ics.borrow_mut().clear();
        // Clear the IR-readable mirror in lockstep (recycled sites get a fresh slot; a
        // live arm's now-out-of-range site id is caught by the JIT's `site < len` guard).
        self.vm_fast_links.borrow_mut().clear();
        #[cfg(debug_assertions)]
        self.dbg_site_pos.borrow_mut().clear();
        self.vm_global_ics.borrow_mut().clear();

        debug_assert!(
            verify_rt_slabs(&new),
            "runtime_collect left a dangling RUNTIME handle"
        );

        // 5. Install the compacted region; bump the version so any IC stamp is stale.
        {
            let rt = Arc::get_mut(&mut self.runtime).unwrap();
            rt.gens[cur].store(Arc::new(new));
            // Replaced `gens[cur]`'s `Arc` (compaction) — bump the pinned-read cache version
            // so the next read reloads instead of cloning the stale pre-compaction `Arc`.
            rt.gen_version.fetch_add(1, Ordering::Release);
            rt.version.fetch_add(1, Ordering::Relaxed);
            // Drop the shared JIT-code cache: compaction rewrites closure ids, so its
            // `(id, argc)` keys no longer denote the same closures. The version bump
            // already makes every entry epoch-stale (never installed), but clearing
            // reclaims it and prevents a recycled id from lingering.
            if let Ok(c) = rt.jit_code_cache.get_mut() {
                c.clear();
            }
            // Same for the shared inlined-native cache (companion to the above).
            if let Ok(c) = rt.jit_inline_cache.get_mut() {
                c.clear();
            }
        }
        let after = self.runtime.cur_code().closures.count();
        Some((before, after))
    }

    /// Should the next safepoint attempt a **RUNTIME** compaction? True once the
    /// RUNTIME closure count crosses the adaptive [`rt_gc_threshold`]
    /// (`max(RT_GC_FLOOR, 2 * live)`). Cheap: a `boxcar` length read + a compare.
    /// Gated on [`gc_enabled`] so builder heaps never auto-collect.
    ///
    /// [`rt_gc_threshold`]: Self::rt_gc_threshold
    /// [`gc_enabled`]: Self::gc_enabled
    #[inline]
    pub fn rt_gc_due(&self) -> bool {
        self.gc_enabled
            && self.rt_collect_block.get() == 0
            && self.runtime.cur_code().closures.count() >= self.rt_gc_threshold
    }

    /// Enter a window in which RUNTIME compaction is suppressed (see
    /// [`rt_collect_block`](Self::rt_collect_block)). Called by [`Self::snapshot_globals`]
    /// so an outstanding globals snapshot can't be relocated out from under it; re-entrant,
    /// paired with [`Self::end_rt_collect_block`] (from [`Self::restore_globals`]).
    pub(crate) fn begin_rt_collect_block(&self) {
        self.rt_collect_block
            .set(self.rt_collect_block.get().saturating_add(1));
    }

    /// Leave a [`Self::begin_rt_collect_block`] window.
    pub(crate) fn end_rt_collect_block(&self) {
        self.rt_collect_block
            .set(self.rt_collect_block.get().saturating_sub(1));
    }

    /// Test/diagnostic hook: turn the automatic safepoint RUNTIME collection on or
    /// off for this heap. Disabling raises the threshold to `usize::MAX` so churn
    /// accumulates, letting a test exercise the *manual* `runtime_collect` /
    /// evacuation paths in isolation (otherwise, under `BROOD_GC_STRESS`'s low
    /// floor, the auto-trigger would compact the churn away mid-loop).
    pub fn set_rt_auto_collect(&mut self, on: bool) {
        self.rt_gc_threshold = if on { rt_gc_floor() } else { usize::MAX };
    }

    /// Reclaim the RUNTIME region at the eval safepoint (the shared-code analog of the
    /// LOCAL [`collect`](Self::collect)). Two complementary reclaimers, chosen by runtime
    /// ownership: when this heap **uniquely owns** the runtime `Arc` (single-process /
    /// quiescent), [`runtime_collect_with`](Self::runtime_collect_with) compacts in place
    /// (sound without stop-the-world); when the runtime is **shared** with live processes
    /// it can't compact, so the 2-generation collector drives instead
    /// ([`advance_runtime_multigen`](Self::advance_runtime_multigen) — age/migrate/drain/
    /// free, ADR-091).
    ///
    /// Either way the adaptive threshold is reset so this isn't re-attempted every
    /// safepoint: after a real compaction, to `max(RT_GC_FLOOR, 2 * live)` (the next runs
    /// once the live set roughly doubles via fresh churn); on the shared path, to
    /// `2 * count` between generational cycles, or `count` (re-enter promptly) while a
    /// drain is in flight and waiting to free. `extra_roots`/`extra_envs` carry the eval
    /// loop's live `expr`/`env` to be rewritten alongside the rooted set.
    pub fn maybe_runtime_collect(&mut self, extra_roots: &mut [Value], extra_envs: &mut [EnvId]) {
        // A multi-generation drain in flight takes priority and must be advanced
        // (freed once every live process reports clean) **regardless of runtime
        // ownership** — a drain armed while the runtime was shared has to still
        // complete if it later goes quiescent (otherwise the aged-out generation would
        // leak, never freed). Compaction can't run on a generation being drained anyway.
        if self.drain_active() {
            // Throttle the free-attempt (the O·live-process `report_parked_liveness`
            // registry scan) to 1/stride of this process's safepoints. A drain that can't
            // yet complete otherwise re-runs that whole-registry scan on every safepoint of
            // every worker purely to re-discover "still not drained" — the dominant cost
            // once the self-report walk is cheap (measured: ~800 k scans / 20 M mailbox
            // locks / 30-round repro). Every process's O(1) self-report still runs every
            // frame, so acks stay current and the free is still attempted every `stride`
            // frames as long as any process reaches a safepoint (no lost wakeup).
            let t = self.rt_drain_tick.get().wrapping_add(1);
            self.rt_drain_tick.set(t);
            if t.is_multiple_of(RT_DRAIN_SCAN_STRIDE) {
                self.advance_runtime_multigen();
            }
            // Back off the threshold even while the drain is still armed, rather than
            // pinning it to `count`. Pinning made `rt_gc_due` true every frame, so every
            // safepoint re-entered this branch and paid the `cur_code()` `ArcSwap` loads —
            // for the whole run, whenever a drain lingers because a long-lived process
            // pins the generation (which it does across a churny workload; Erlang's
            // local-call code-pinning limitation). With the exponential back-off the
            // collector is re-entered only at region-growth doublings; the O(1) drain
            // self-report still runs every frame (so acks stay current) and the free is
            // still attempted at each doubling (no lost wakeup — completion never needs
            // the free, and a completable drain frees at the next doubling).
            let count = self.runtime.cur_code().closures.count();
            self.rt_gc_threshold = rt_gc_floor().max(count.saturating_mul(2));
            return;
        }
        match self.runtime_collect_with(extra_roots, extra_envs) {
            Some((_before, after)) => {
                self.rt_gc_threshold = rt_gc_floor().max(after.saturating_mul(2));
            }
            None => {
                // Single-process compaction couldn't run (the runtime is shared), so
                // advance the multi-generation collector's state machine instead — age +
                // migrate + drain + free the aged-out generation. This is the shared-
                // runtime reclaimer (compaction handles the single-process case above).
                self.advance_runtime_multigen();
                let count = self.runtime.cur_code().closures.count();
                self.rt_gc_threshold = if self.drain_active() {
                    // A drain is in flight — re-enter at the next safepoint to free it
                    // promptly once every live process has reported clean.
                    count.max(rt_gc_floor())
                } else {
                    self.rt_gc_threshold
                        .max(count.saturating_mul(2))
                        .max(rt_gc_floor())
                };
            }
        }
    }

    /// **RUNTIME collector — Stage 4 (multi-process reclamation state machine, ADR-091).**
    /// One step of whole-generation reclamation, driven at the RUNTIME safepoint when
    /// in-place compaction can't run (the runtime is shared). Idempotent and cheap when
    /// there's nothing to do.
    ///
    /// The state machine (at most one action per call):
    /// - **Drain in flight** → try to free the draining generation. Each live process
    ///   reports clean at its own safepoint (Stage 3c); once the union is clean,
    ///   [`free_drained_gen`](crate::process::free_drained_gen) reclaims the slot and
    ///   ends the drain. Until then, wait.
    /// - **Idle, and the other slot is empty** (its previous generation already freed) →
    ///   start a cycle: [`age_runtime`](Self::age_runtime) flips into it,
    ///   [`migrate_live_globals`](Self::migrate_live_globals) re-exports the live globals
    ///   so the vacated generation holds only superseded + in-flight code, then
    ///   [`begin_gen_drain`](Self::begin_gen_drain) arms its drain. Single-flight via
    ///   [`begin_aging`](Self::begin_aging) — a losing racer simply waits.
    /// - **Idle, but the other slot is still occupied** (its previous generation not yet
    ///   freed) → wait. This is the 2-versions-max back-pressure: at most two live
    ///   generations exist at once.
    ///
    /// Ordering is load-bearing: migrate **before** arming the drain, so by the time any
    /// process reports for the new drain epoch the globals already point into the current
    /// generation (nothing can newly acquire an `old_gen` reference — the invariant the
    /// clean-stays-clean report optimization rests on).
    pub fn advance_runtime_multigen(&self) {
        if self.drain_active() {
            crate::process::free_drained_gen(self);
            return;
        }
        let other = 1 - self.runtime.cur_gen();
        if !self.runtime.gens[other].load().is_empty() {
            return; // previous generation not yet freed — 2-versions-max back-pressure
        }
        if !self.begin_aging() {
            return; // another process is running a cycle
        }
        let old = self.runtime.cur_gen();
        if self.age_runtime() {
            self.migrate_live_globals(old);
            self.begin_gen_drain(old);
        }
        self.end_aging();
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
        let mut fwd = FlushForward::default();
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
            } else if let Some(&new_idx) = fwd.pairs.get(&(key as u32)) {
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
        let mut fwd = FlushForward::default();
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
                        .get(&(e.index() as u32))
                        .map(|&n| fwd.mint_env(n as usize))
                })
                .collect();
        }
        let old_form_pos = std::mem::take(&mut self.form_pos);
        for (key, pos) in old_form_pos {
            if (key >> 32) & 1 == 1 {
                if let Some(&new_idx) = fwd.pairs.get(&(key as u32)) {
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
    pairs: HashMap<u32, u32>,
    vectors: HashMap<u32, u32>,
    maps: HashMap<u32, u32>,
    strings: HashMap<u32, u32>,
    bigints: HashMap<u32, u32>,
    decimals: HashMap<u32, u32>,
    bytes: HashMap<u32, u32>,
    ropes: HashMap<u32, u32>,
    closures: HashMap<u32, u32>,
    envs: HashMap<u32, u32>,
}

impl FlushForward {
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
    if let Some(&new_idx) = fwd.pairs.get(&(id.index() as u32)) {
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
                if let Some(&n) = fwd.pairs.get(&key) {
                    break Value::pair(fwd.mint_pair(n as usize));
                }
                let (car, cdr) = old.pairs[flush_bound!(old.pairs, p, fwd, "pair")];
                let new_idx = new.pairs.len();
                new.pairs.push((Value::nil(), Value::nil()));
                fwd.pairs.insert(key, new_idx as u32);
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
    if let Some(&new_idx) = fwd.vectors.get(&key) {
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
    fwd.vectors.insert(key, new_idx as u32);
    let store = VecStore::from_flushed(n, |i| {
        let x = old.vectors[src_idx][i];
        flush_value(old, new, fwd, x)
    });
    new.vectors[new_idx] = store;
    fwd.mint_vector(new_idx)
}

fn flush_string(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: StrId) -> StrId {
    let key = id.index() as u32;
    if let Some(&new_idx) = fwd.strings.get(&key) {
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
    fwd.strings.insert(key, new_idx as u32);
    fwd.mint_string(new_idx)
}

fn flush_bigint(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: BigIntId) -> BigIntId {
    let key = id.index() as u32;
    if let Some(&new_idx) = fwd.bigints.get(&key) {
        return fwd.mint_bigint(new_idx as usize);
    }
    // A leaf: clone the value's digits into the new slab (the old slab drops
    // right after `flush`). Same shape as `flush_string`'s inline branch.
    let n = old.bigints[flush_bound!(old.bigints, id, fwd, "bigint")].clone();
    let new_idx = new.bigints.len();
    new.bigints.push(n);
    fwd.bigints.insert(key, new_idx as u32);
    fwd.mint_bigint(new_idx)
}

/// Flush a LOCAL decimal (mirrors [`flush_bigint`]). A leaf — clone the value into
/// the new slab (the old slab drops right after `flush`).
fn flush_decimal(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: DecimalId) -> DecimalId {
    let key = id.index() as u32;
    if let Some(&new_idx) = fwd.decimals.get(&key) {
        return fwd.mint_decimal(new_idx as usize);
    }
    let n = old.decimals[flush_bound!(old.decimals, id, fwd, "decimal")].clone();
    let new_idx = new.decimals.len();
    new.decimals.push(n);
    fwd.decimals.insert(key, new_idx as u32);
    fwd.mint_decimal(new_idx)
}

/// Flush a LOCAL bytes value (mirrors [`flush_bigint`]). A byte-clean leaf —
/// clone the `Arc<SharedBlob>` (a refcount bump, not a byte copy) into the new slab.
fn flush_bytes(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: BytesId) -> BytesId {
    let key = id.index() as u32;
    if let Some(&new_idx) = fwd.bytes.get(&key) {
        return fwd.mint_bytes(new_idx as usize);
    }
    let b = old.bytes[flush_bound!(old.bytes, id, fwd, "bytes")].clone();
    let new_idx = new.bytes.len();
    new.bytes.push(b);
    fwd.bytes.insert(key, new_idx as u32);
    fwd.mint_bytes(new_idx)
}

fn flush_rope(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: RopeId) -> RopeId {
    let key = id.index() as u32;
    if let Some(&new_idx) = fwd.ropes.get(&key) {
        return fwd.mint_rope(new_idx as usize);
    }
    // `ropey::Rope::clone` is a cheap `Arc`-node bump (no byte copy); the old
    // slab drops right after `flush`, leaving the surviving rope's internal
    // refcounts net-unchanged — same structural sharing as `flush_string`.
    let rope = old.ropes[flush_bound!(old.ropes, id, fwd, "rope")].clone();
    let new_idx = new.ropes.len();
    new.ropes.push(rope);
    fwd.ropes.insert(key, new_idx as u32);
    fwd.mint_rope(new_idx)
}

fn flush_map(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: MapId) -> MapId {
    let key = id.index() as u32;
    if let Some(&new_idx) = fwd.maps.get(&key) {
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
    fwd.maps.insert(key, new_idx as u32);
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
    if let Some(&new_idx) = fwd.closures.get(&key) {
        return fwd.mint_closure(new_idx as usize);
    }
    let cl = old.closures[flush_bound!(old.closures, id, fwd, "closure")].clone();
    let new_idx = new.closures.len();
    new.closures.push(Closure::default());
    fwd.closures.insert(key, new_idx as u32);
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
    if let Some(&new_idx) = fwd.envs.get(&key) {
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
    fwd.envs.insert(key, new_idx as u32);
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

// ===================== RUNTIME-region compaction (ADR-076 follow-up) =====================
//
// The compacting collector for the shared RUNTIME code region (the one open GC
// item — `docs/runtime-collector-exploration.md`). This is **Step 2a: the
// out-of-place evacuation core** — it traces the live RUNTIME code reachable from a
// root set and *copies* it into a fresh `CodeSlabs`, building an old→new forwarding
// map, exactly mirroring the LOCAL GC's `flush_*` but over `CodeSlabs` (`boxcar`,
// `OnceLock` closures/envs) and RUNTIME handles. It **installs nothing** — it feeds
// the single-process in-place compaction swap. (Reclamation on a *shared* runtime is
// handled by the 2-generation collector — ADR-091 — rather than a stop-the-world.)
// Out-of-place means it cannot corrupt the live region; it's the safe, testable
// algorithmic foundation.

/// Old→new RUNTIME index maps, one per slab kind (the RUNTIME counterpart of
/// [`FlushForward`] — but no generation epoch: RUNTIME handles are region+index).
#[derive(Default)]
struct RuntimeForward {
    pairs: HashMap<u32, u32>,
    vectors: HashMap<u32, u32>,
    maps: HashMap<u32, u32>,
    strings: HashMap<u32, u32>,
    bigints: HashMap<u32, u32>,
    decimals: HashMap<u32, u32>,
    bytes: HashMap<u32, u32>,
    ropes: HashMap<u32, u32>,
    closures: HashMap<u32, u32>,
    envs: HashMap<u32, u32>,
    /// The generation the flush copies **from**: only RUNTIME handles tagged with
    /// this generation are forwarded; handles in the *other* generation (and PRELUDE)
    /// pass through untouched. For same-generation compaction `old_gen == dest_gen`;
    /// for ADR-091 **global migration** they differ (copy the live image from the
    /// aged-out generation into the current one). See [`RuntimeForward::for_gens`].
    old_gen: usize,
    /// The generation the copied handles are minted **into** — every new slab index
    /// is tagged with this via `Id::runtime_gen(idx, dest_gen)`, so a cross-generation
    /// migration produces correctly-tagged handles (not gen-0 as the bare
    /// `Id::runtime` constructor would).
    dest_gen: usize,
}

impl RuntimeForward {
    /// A forwarding map that copies RUNTIME handles from generation `old_gen` into
    /// `dest_gen` (equal for in-place compaction; different for global migration).
    fn for_gens(old_gen: usize, dest_gen: usize) -> Self {
        RuntimeForward {
            old_gen,
            dest_gen,
            ..RuntimeForward::default()
        }
    }
}

/// Is `v` a RUNTIME handle resident in generation `gen`? (A leaf test on the top-level
/// handle — not transitive.) Used by [`Heap::migrate_live_globals`] to snapshot the
/// globals still living in the aged-out generation and to detect a concurrent
/// redefinition (which moves the binding to the current generation) at reconcile.
fn value_in_gen(v: Value, gen: usize) -> bool {
    runtime_gen_of(v) == Some(gen)
}

/// Which RUNTIME generation a value's handle lives in, or `None` if it isn't a
/// RUNTIME handle (a LOCAL/PRELUDE handle, or an immediate). The companion to
/// [`value_in_gen`] that returns *which* generation rather than testing one — used
/// by [`Heap::rehome_to_current`] to decide whether a value needs re-homing.
fn runtime_gen_of(v: Value) -> Option<usize> {
    let in_rt = |region: u8, gen: usize| (region == RUNTIME).then_some(gen);
    match v.unpack() {
        ValueRef::Pair(id) => in_rt(id.region(), id.code_gen()),
        ValueRef::Vector(id) | ValueRef::Range(id) | ValueRef::SeqView(id) => {
            in_rt(id.region(), id.code_gen())
        }
        ValueRef::Map(id) | ValueRef::Set(id) => in_rt(id.region(), id.code_gen()),
        ValueRef::Str(id) => in_rt(id.region(), id.code_gen()),
        ValueRef::BigInt(id) => in_rt(id.region(), id.code_gen()),
        ValueRef::Decimal(id) => in_rt(id.region(), id.code_gen()),
        ValueRef::Bytes(id) => in_rt(id.region(), id.code_gen()),
        ValueRef::Rope(id) => in_rt(id.region(), id.code_gen()),
        ValueRef::Fn(id) | ValueRef::Macro(id) => in_rt(id.region(), id.code_gen()),
        _ => None,
    }
}

/// Copy a value's RUNTIME sub-graph into `new`, returning the value with its RUNTIME
/// handles rewritten to their new indices. Non-RUNTIME values (atoms, LOCAL,
/// PRELUDE) are returned unchanged — only the runtime region moves.
fn flush_rt_value(old: &CodeSlabs, new: &CodeSlabs, fwd: &mut RuntimeForward, v: Value) -> Value {
    // Only forward handles resident in the *source* generation; a handle already in
    // the destination generation (or PRELUDE, whose `region() != RUNTIME`) is left
    // untouched — load-bearing for cross-generation migration, where a live global's
    // graph can already straddle both generations.
    let g = fwd.old_gen;
    match v.unpack() {
        ValueRef::Pair(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::pair(flush_rt_pair(old, new, fwd, id))
        }
        ValueRef::Vector(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::vector(flush_rt_vector(old, new, fwd, id))
        }
        // A range's backing `[lo hi step]` vector moves like any other vector;
        // keep the `Range` wrapper on the forwarded handle.
        ValueRef::Range(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::range(flush_rt_vector(old, new, fwd, id))
        }
        // Like a range, a seq-view's backing vector moves under a runtime
        // compaction; `flush_rt_vector` forwards its elements. Keep the wrapper.
        ValueRef::SeqView(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::seqview(flush_rt_vector(old, new, fwd, id))
        }
        ValueRef::Map(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::map(flush_rt_map(old, new, fwd, id))
        }
        // A set shares the CHAMP storage — forward its trie like a map under a
        // RUNTIME compaction and keep the `Set` wrapper (mirrors `SeqView` above).
        ValueRef::Set(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::set(flush_rt_map(old, new, fwd, id))
        }
        ValueRef::Str(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::str_(flush_rt_string(old, new, fwd, id))
        }
        ValueRef::BigInt(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::bigint(flush_rt_bigint(old, new, fwd, id))
        }
        ValueRef::Decimal(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::decimal(flush_rt_decimal(old, new, fwd, id))
        }
        ValueRef::Bytes(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::bytes(flush_rt_bytes(old, new, fwd, id))
        }
        ValueRef::Rope(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::rope(flush_rt_rope(old, new, fwd, id))
        }
        ValueRef::Fn(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::func(flush_rt_closure(old, new, fwd, id))
        }
        ValueRef::Macro(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::macro_(flush_rt_closure(old, new, fwd, id))
        }
        _ => v,
    }
}

fn flush_rt_pair(old: &CodeSlabs, new: &CodeSlabs, fwd: &mut RuntimeForward, id: PairId) -> PairId {
    let (key, src_gen, dest) = (id.index() as u32, fwd.old_gen, fwd.dest_gen);
    if let Some(&n) = fwd.pairs.get(&key) {
        return PairId::runtime_gen(n as usize, dest);
    }
    // `boxcar` is append-only (no write-back), so we can't reserve-then-fill. RUNTIME
    // code lists are **immutable and acyclic** — no cons cycle is constructible — so
    // flush the car/cdr *first*, then push the finished cell once. Sharing (a DAG)
    // is handled by the `fwd` check on revisit; a true cycle would only arise
    // through a closure, and `flush_rt_closure` breaks that with `OnceLock`.
    //
    // Walk the cdr spine **iteratively** (mirroring the LOCAL `flush_pair`): a
    // pathological 100k-element quoted literal promoted to RUNTIME would otherwise
    // recurse its length deep and blow the native stack at `runtime_collect` —
    // now reachable since RT compaction runs at auto-safepoints (ADR-091).
    // Recursion stays bounded to element *nesting* via `flush_rt_value` on each
    // car. Append-only means we can't pre-reserve a slot, so we record (key, car)
    // along the spine and push the finished cells in reverse, wiring each cdr to
    // the already-built next handle.
    let mut spine: Vec<(u32, Value)> = Vec::new(); // (forward-map key, original car)
    let mut cur_id = id;
    let tail = loop {
        let (h, t) = *old.pairs.get(cur_id.index()).expect("rt pair");
        spine.push((cur_id.index() as u32, h));
        match t.unpack() {
            // Another not-yet-copied RUNTIME pair: continue the spine. A
            // shared/already-copied cell resolves via the `fwd` check below
            // (handled by `flush_rt_value` on the terminal), so we only extend
            // the spine for fresh cells.
            ValueRef::Pair(p)
                if p.region() == RUNTIME
                    && p.code_gen() == src_gen
                    && !fwd.pairs.contains_key(&(p.index() as u32)) =>
            {
                cur_id = p;
            }
            // Nil / atom / dotted tail / other-generation or shared-or-copied RUNTIME
            // pair: flush it (cheap, no spine recursion) and stop.
            other => break flush_rt_value(old, new, fwd, other),
        }
    };
    // Build the cells in reverse so each cdr is the already-pushed next handle.
    // Car flushes run after the whole spine is known; sharing through a car
    // resolves via `fwd` once that car's cell is pushed.
    let mut next = tail;
    for (key, car) in spine.into_iter().rev() {
        let new_car = flush_rt_value(old, new, fwd, car);
        let new_idx = new.pairs.push((new_car, next));
        fwd.pairs.insert(key, new_idx as u32);
        next = Value::pair(PairId::runtime_gen(new_idx, dest));
    }
    match next.unpack() {
        ValueRef::Pair(pid) => pid,
        _ => unreachable!("the spine always has at least the head pair"),
    }
}

fn flush_rt_vector(old: &CodeSlabs, new: &CodeSlabs, fwd: &mut RuntimeForward, id: VecId) -> VecId {
    let (key, dest) = (id.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.vectors.get(&key) {
        return VecId::runtime_gen(n as usize, dest);
    }
    let src = old.vectors.get(id.index()).expect("rt vector");
    let n = src.len();
    // Build in place — no temp `Vec` for the inline case. RUNTIME vectors are
    // acyclic, so recording the forwarding after the build (as the pair path
    // here also does) is safe. `src` and `flush_rt_value` share `old`'s
    // immutable borrow; `new` is append-only via interior mutability.
    let store = VecStore::from_flushed(n, |i| flush_rt_value(old, new, fwd, src[i]));
    let new_idx = new.vectors.push(store);
    fwd.vectors.insert(key, new_idx as u32);
    VecId::runtime_gen(new_idx, dest)
}

fn flush_rt_string(old: &CodeSlabs, new: &CodeSlabs, fwd: &mut RuntimeForward, id: StrId) -> StrId {
    let (key, dest) = (id.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.strings.get(&key) {
        return StrId::runtime_gen(n as usize, dest);
    }
    let s = old.strings.get(id.index()).expect("rt string").clone();
    let new_idx = new.strings.push(s);
    fwd.strings.insert(key, new_idx as u32);
    StrId::runtime_gen(new_idx, dest)
}

fn flush_rt_bigint(
    old: &CodeSlabs,
    new: &CodeSlabs,
    fwd: &mut RuntimeForward,
    id: BigIntId,
) -> BigIntId {
    let (key, dest) = (id.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.bigints.get(&key) {
        return BigIntId::runtime_gen(n as usize, dest);
    }
    let v = old.bigints.get(id.index()).expect("rt bigint").clone();
    let new_idx = new.bigints.push(v);
    fwd.bigints.insert(key, new_idx as u32);
    BigIntId::runtime_gen(new_idx, dest)
}

/// Flush a RUNTIME decimal during a runtime-region compaction (mirrors
/// [`flush_rt_bigint`]). A leaf — clone the value into the new region.
fn flush_rt_decimal(
    old: &CodeSlabs,
    new: &CodeSlabs,
    fwd: &mut RuntimeForward,
    id: DecimalId,
) -> DecimalId {
    let (key, dest) = (id.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.decimals.get(&key) {
        return DecimalId::runtime_gen(n as usize, dest);
    }
    let v = old.decimals.get(id.index()).expect("rt decimal").clone();
    let new_idx = new.decimals.push(v);
    fwd.decimals.insert(key, new_idx as u32);
    DecimalId::runtime_gen(new_idx, dest)
}

/// Flush a RUNTIME bytes value during a runtime-region compaction (mirrors
/// [`flush_rt_bigint`]). Byte-clean — clone the `Arc<SharedBlob>` into the new region.
fn flush_rt_bytes(
    old: &CodeSlabs,
    new: &CodeSlabs,
    fwd: &mut RuntimeForward,
    id: BytesId,
) -> BytesId {
    let (key, dest) = (id.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.bytes.get(&key) {
        return BytesId::runtime_gen(n as usize, dest);
    }
    let v = old.bytes.get(id.index()).expect("rt bytes").clone();
    let new_idx = new.bytes.push(v);
    fwd.bytes.insert(key, new_idx as u32);
    BytesId::runtime_gen(new_idx, dest)
}

fn flush_rt_rope(old: &CodeSlabs, new: &CodeSlabs, fwd: &mut RuntimeForward, id: RopeId) -> RopeId {
    let (key, dest) = (id.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.ropes.get(&key) {
        return RopeId::runtime_gen(n as usize, dest);
    }
    let r = old.ropes.get(id.index()).expect("rt rope").clone();
    let new_idx = new.ropes.push(r);
    fwd.ropes.insert(key, new_idx as u32);
    RopeId::runtime_gen(new_idx, dest)
}

fn flush_rt_map(old: &CodeSlabs, new: &CodeSlabs, fwd: &mut RuntimeForward, id: MapId) -> MapId {
    let (key, src_gen, dest) = (id.index() as u32, fwd.old_gen, fwd.dest_gen);
    if let Some(&n) = fwd.maps.get(&key) {
        return MapId::runtime_gen(n as usize, dest);
    }
    let node = old.maps.get(id.index()).expect("rt map");
    let (size, data_map, node_map, is_collision) =
        (node.size, node.data_map, node.node_map, node.is_collision);
    let data_snapshot: SmallVec<[(Value, Value); 4]> = node.data.iter().copied().collect();
    let children_snapshot: SmallVec<[MapId; 4]> = node.children.iter().copied().collect();
    // CHAMP nodes form a trie (acyclic): flush children + data first, push once.
    let new_children: SmallVec<[MapId; 4]> = children_snapshot
        .iter()
        .map(|&c| {
            if c.region() == RUNTIME && c.code_gen() == src_gen {
                flush_rt_map(old, new, fwd, c)
            } else {
                c
            }
        })
        .collect();
    let new_data: SmallVec<[(Value, Value); 4]> = data_snapshot
        .iter()
        .map(|&(k, v)| {
            (
                flush_rt_value(old, new, fwd, k),
                flush_rt_value(old, new, fwd, v),
            )
        })
        .collect();
    let new_idx = new.maps.push(MapNode {
        size,
        data_map,
        node_map,
        is_collision,
        data: new_data,
        children: new_children,
    });
    fwd.maps.insert(key, new_idx as u32);
    MapId::runtime_gen(new_idx, dest)
}

fn flush_rt_closure(
    old: &CodeSlabs,
    new: &CodeSlabs,
    fwd: &mut RuntimeForward,
    id: ClosureId,
) -> ClosureId {
    let (key, dest) = (id.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.closures.get(&key) {
        return ClosureId::runtime_gen(n as usize, dest);
    }
    let cl = old
        .closures
        .get(id.index())
        .expect("rt closure slot")
        .get()
        .expect("rt closure set")
        .clone();
    // Reserve-then-fill (OnceLock) breaks cyclic closures (a closure whose captured
    // env binds the closure itself), exactly as `promote_closure` does.
    let new_idx = new.closures.push(OnceLock::new());
    fwd.closures.insert(key, new_idx as u32);
    let arms = cl
        .arms
        .iter()
        .map(|arm| ClosureArm {
            params: arm.params.clone(),
            optionals: arm
                .optionals
                .iter()
                .map(|&(s, d)| (s, flush_rt_value(old, new, fwd, d)))
                .collect(),
            rest: arm.rest,
            body: arm
                .body
                .iter()
                .map(|&f| flush_rt_value(old, new, fwd, f))
                .collect(),
            passthrough: arm.passthrough.clone(),
        })
        .collect();
    let env = cl.env.map(|e| flush_rt_env(old, new, fwd, e));
    let _ = new.closures.get(new_idx).unwrap().set(Closure {
        name: cl.name,
        arms,
        doc: cl.doc,
        env,
    });
    ClosureId::runtime_gen(new_idx, dest)
}

fn flush_rt_env(old: &CodeSlabs, new: &CodeSlabs, fwd: &mut RuntimeForward, env: EnvId) -> EnvId {
    // Leave the global env, PRELUDE frames, and frames already in the destination
    // generation untouched — only source-generation frames are copied.
    if env == EnvId::GLOBAL || env.region() != RUNTIME || env.code_gen() != fwd.old_gen {
        return env;
    }
    let (key, dest) = (env.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.envs.get(&key) {
        return EnvId::runtime_gen(n as usize, dest);
    }
    let (parent, vars_snapshot): (Option<EnvId>, EnvVars) = {
        let frame = old
            .envs
            .get(env.index())
            .expect("rt env slot")
            .get()
            .expect("rt env set");
        (frame.parent, frame.vars.iter().copied().collect())
    };
    let new_idx = new.envs.push(OnceLock::new());
    fwd.envs.insert(key, new_idx as u32);
    let new_parent = parent.map(|p| flush_rt_env(old, new, fwd, p));
    let vars: EnvVars = vars_snapshot
        .iter()
        .map(|&(s, v)| (s, flush_rt_value(old, new, fwd, v)))
        .collect();
    let _ = new.envs.get(new_idx).unwrap().set(EnvFrame {
        vars,
        parent: new_parent,
    });
    EnvId::runtime_gen(new_idx, dest)
}

/// Verify an evacuated `CodeSlabs`: every RUNTIME handle it contains must point
/// *within* the new region (`index < that slab's len`). A handle still pointing at
/// an old index (≥ the new, smaller length) means the evacuation missed a rewrite —
/// the exact failure mode a moving collector must never ship. (In-bounds is a
/// necessary soundness check; the redef test additionally pins the live *count*.)
fn verify_rt_slabs(s: &CodeSlabs) -> bool {
    let (np, nv, nm, ns, nb, nd, nby, nr, nc, ne) = (
        s.pairs.count(),
        s.vectors.count(),
        s.maps.count(),
        s.strings.count(),
        s.bigints.count(),
        s.decimals.count(),
        s.bytes.count(),
        s.ropes.count(),
        s.closures.count(),
        s.envs.count(),
    );
    let ok = |v: Value| -> bool {
        match v.unpack() {
            ValueRef::Pair(id) if id.region() == RUNTIME => id.index() < np,
            ValueRef::Vector(id) | ValueRef::Range(id) | ValueRef::SeqView(id)
                if id.region() == RUNTIME =>
            {
                id.index() < nv
            }
            ValueRef::Map(id) | ValueRef::Set(id) if id.region() == RUNTIME => id.index() < nm,
            ValueRef::Str(id) if id.region() == RUNTIME => id.index() < ns,
            ValueRef::BigInt(id) if id.region() == RUNTIME => id.index() < nb,
            ValueRef::Decimal(id) if id.region() == RUNTIME => id.index() < nd,
            ValueRef::Bytes(id) if id.region() == RUNTIME => id.index() < nby,
            ValueRef::Rope(id) if id.region() == RUNTIME => id.index() < nr,
            ValueRef::Fn(id) | ValueRef::Macro(id) if id.region() == RUNTIME => id.index() < nc,
            _ => true,
        }
    };
    let env_ok = |e: EnvId| e == EnvId::GLOBAL || e.region() != RUNTIME || e.index() < ne;
    for i in 0..np {
        let (h, t) = *s.pairs.get(i).unwrap();
        if !ok(h) || !ok(t) {
            return false;
        }
    }
    for i in 0..nv {
        if !s.vectors.get(i).unwrap().iter().all(|&x| ok(x)) {
            return false;
        }
    }
    for i in 0..nm {
        let node = s.maps.get(i).unwrap();
        if !node.data.iter().all(|&(k, v)| ok(k) && ok(v)) {
            return false;
        }
        if !node
            .children
            .iter()
            .all(|c| c.region() != RUNTIME || c.index() < nm)
        {
            return false;
        }
    }
    for i in 0..nc {
        if let Some(cl) = s.closures.get(i).unwrap().get() {
            for arm in cl.arms.iter() {
                if !arm.body.iter().all(|&f| ok(f)) || !arm.optionals.iter().all(|&(_, d)| ok(d)) {
                    return false;
                }
            }
            if let Some(e) = cl.env {
                if !env_ok(e) {
                    return false;
                }
            }
        }
    }
    for i in 0..ne {
        if let Some(fr) = s.envs.get(i).unwrap().get() {
            if !fr.vars.iter().all(|&(_, v)| ok(v)) {
                return false;
            }
            if let Some(p) = fr.parent {
                if !env_ok(p) {
                    return false;
                }
            }
        }
    }
    true
}

/// Rewrite every RUNTIME handle held in a LOCAL [`Slabs`] (one generation) to its
/// evacuated index, evacuating it from `old` into `new` on first sight (`fwd`
/// memoizes). Used by [`Heap::runtime_collect`] for both `local` and `old`. Visiting
/// every slot directly is what makes the one-pass evacuate-and-rewrite correct: any
/// RUNTIME handle anywhere is reached without graph-walking LOCAL structure (LOCAL
/// handles are left as-is — only the RUNTIME handles *inside* each slot move).
/// Strings/ropes/natives hold no handles; LOCAL map children are LOCAL (same-region
/// CHAMP), never RUNTIME, so only map *data* is rewritten.
fn rewrite_local_rt_handles(
    s: &mut Slabs,
    old: &CodeSlabs,
    new: &CodeSlabs,
    fwd: &mut RuntimeForward,
) {
    for (a, b) in s.pairs.iter_mut() {
        *a = flush_rt_value(old, new, fwd, *a);
        *b = flush_rt_value(old, new, fwd, *b);
    }
    for vec in s.vectors.iter_mut() {
        for x in vec.iter_mut() {
            *x = flush_rt_value(old, new, fwd, *x);
        }
    }
    for node in s.maps.iter_mut() {
        for (k, v) in node.data.iter_mut() {
            *k = flush_rt_value(old, new, fwd, *k);
            *v = flush_rt_value(old, new, fwd, *v);
        }
    }
    for cl in s.closures.iter_mut() {
        // Compaction relocates RUNTIME code, so a closure's arm handles — which point
        // *into* RUNTIME whenever they're shared (the template cache) — must be
        // rewritten. `make_mut` rewrites a unique arms in place and clones a shared one
        // (un-sharing that closure). Unlike the minor-flush hot path this is fine: a
        // RUNTIME compaction is rare (def-churn only), and the template cache is
        // invalidated by the same `gen_version` bump, so fresh closures re-share after.
        for arm in std::sync::Arc::make_mut(&mut cl.arms).iter_mut() {
            for f in arm.body.iter_mut() {
                *f = flush_rt_value(old, new, fwd, *f);
            }
            for (_, d) in arm.optionals.iter_mut() {
                *d = flush_rt_value(old, new, fwd, *d);
            }
        }
        if let Some(e) = cl.env {
            cl.env = Some(flush_rt_env(old, new, fwd, e));
        }
    }
    for fr in s.envs.iter_mut() {
        for (_, v) in fr.vars.iter_mut() {
            *v = flush_rt_value(old, new, fwd, *v);
        }
        if let Some(p) = fr.parent {
            fr.parent = Some(flush_rt_env(old, new, fwd, p));
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
