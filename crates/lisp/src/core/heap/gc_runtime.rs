//! The RUNTIME code-region collector — the shared, append-only code region's
//! two-generation aging + single-process compaction, node-liveness drain, and
//! live-globals migration (ADR-091). A child of `heap`, split out of `gc.rs`
//! (which keeps the per-process LOCAL nursery/old collector): these methods and
//! free helpers only touch the RUNTIME region and are independent of the LOCAL
//! collector, so they live on their own. Reaches `Heap`'s private items via
//! `use super::*`, exactly like `gc.rs`.
use super::*;

impl Heap {
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
        // ADR-175 Phase A: the per-arm block registry indexes the tables just cleared,
        // so it clears in lockstep. Live arms re-resolve fresh blocks on their next
        // activation; the currently-executing arm's stale cursors are benign (probes
        // bounds-check → miss, and every entry validates sym+argc+epoch).
        self.arm_ic_blocks.borrow_mut().clear();
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
    /// `epoch` is the `drain_epoch` the caller validated the drain at. The free is
    /// **single-flighted through the same gate as aging** and re-validates that epoch
    /// while holding it — see the race this closes, below.
    pub fn free_runtime_gen(&self, old_gen: usize, epoch: u64) -> bool {
        // The destructive store below replaces a whole generation, so it must not run
        // concurrently with an aging cycle (which flips `current_gen` and migrates the live
        // image into the other slot) nor with a second freer.
        //
        // Before this, `free_drained_gen` validated the drain and then read `drain_gen()`
        // *separately*, and the free itself ran outside `begin_aging`'s CAS. Two processes
        // could both pass the validation and both enter; the first frees and ends the drain,
        // a third then ages — flipping `current_gen` to the just-freed slot and migrating
        // every live global into it — and the second, still holding its stale view, stores an
        // empty slab over **the current generation**. Every migrated global's handle then
        // points into an empty `boxcar`: `expect("runtime closure handle")`, or a recycled
        // slot read. The shorter variant needs no double-entry at all, only the split read:
        // validate for gen 0, someone else frees and re-arms a drain of gen 1, then
        // `drain_gen()` returns 1 and this frees a generation that is neither drained nor
        // dead. A milder outcome of the same race — a late `end_gen_drain` clearing a newly
        // armed drain — wedges aging permanently behind the 2-versions-max back-pressure,
        // so the region grows without bound.
        //
        // Reachable by default in any multi-process program once the shared closure count
        // crosses `rt_gc_floor` (4096): a `spawn` fan-out with a capturing thunk promotes one
        // RUNTIME closure per spawn, and every process arrives at this path in the same window.
        if !self.begin_aging() {
            return false; // an aging cycle or another freer owns the region right now
        }
        // Two callers, two liveness proofs. The cooperative drain (`free_drained_gen`)
        // arms one and must own *this* epoch and generation; the single-process path frees
        // after `runtime_gen_referenced` says no, with no drain armed at all. Accept either,
        // and refuse when a drain is armed that this call does not own — that is exactly the
        // stale-view case.
        let drain_armed = self.runtime.drain_active.load(Ordering::Acquire);
        let owns_drain = drain_armed
            && self.runtime.drain_epoch.load(Ordering::Relaxed) == epoch
            && self.runtime.drain_gen.load(Ordering::Relaxed) == old_gen;
        let ok = (owns_drain || !drain_armed)
            && old_gen != self.runtime.cur_gen()
            && !self.runtime.gens[old_gen].load().is_empty();
        if !ok {
            self.end_aging();
            return false;
        }
        let out = self.free_runtime_gen_locked(old_gen, owns_drain);
        self.end_aging();
        out
    }

    /// The destructive half of [`free_runtime_gen`], run with the aging gate held and the
    /// drain epoch re-validated. Split out so the gate is released on every path.
    fn free_runtime_gen_locked(&self, old_gen: usize, owns_drain: bool) -> bool {
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
        // ADR-175 Phase A: the per-arm block registry indexes the tables just cleared,
        // so it clears in lockstep. Live arms re-resolve fresh blocks on their next
        // activation; the currently-executing arm's stale cursors are benign (probes
        // bounds-check → miss, and every entry validates sym+argc+epoch).
        self.arm_ic_blocks.borrow_mut().clear();
        // Drop the shared JIT-code caches (the version bump already epoch-invalidated
        // them; clearing reclaims the memory and prevents a recycled id lingering).
        if let Ok(mut c) = self.runtime.jit_code_cache.write() {
            c.clear();
        }
        if let Ok(mut c) = self.runtime.jit_inline_cache.write() {
            c.clear();
        }
        // End the drain only if this call actually owned one. Ending unconditionally was
        // the milder half of the same race: a late free could clear a drain armed by a
        // *newer* cycle, leaving `drain_active` false with the other generation non-empty —
        // which parks aging forever behind the 2-versions-max back-pressure and grows the
        // region without bound.
        if owns_drain {
            self.end_gen_drain();
        }
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

        // Both phases below are guarded by a stale-dirty throttle. The design assumed
        // Phase 1 was cheap enough to run unconditionally; that holds only while the seed
        // is small, and `roots` — the VM operand/env stack — grows with recursion depth.
        // A process 100 000 frames deep seeds ~1.7 million values per walk (KI-14).
        let epoch = self.runtime.drain_epoch.load(Ordering::Relaxed);

        // **A cached stale-dirty verdict short-circuits the WHOLE probe, Phase 1 included.**
        // This check used to sit between the two phases, which meant a process dirty via
        // Phase 2 still paid a full Phase-1 walk on every safepoint — and then threw the
        // result away, because the verdict below is `true` regardless of what Phase 1 found.
        // Pure waste, invisible while Phase 1 really was cheap. It is not cheap for a deeply
        // recursing process: `roots` grows with depth, and the KI-14 run measured 78 000
        // Phase-1 walks over a 1.7-million-entry root stack inside a *single* drain epoch,
        // which is why that run never finished. Hoisting the check changes no verdict — it
        // only skips work whose result was already discarded.
        if self.p2_dirty_epoch.get() == epoch {
            let t = self.p2_dirty_tick.get().wrapping_add(1);
            self.p2_dirty_tick.set(t);
            if !t.is_multiple_of(P2_REVALIDATE_STRIDE) {
                return true; // stale-dirty: skip both walks this safepoint
            }
        }

        // The Phase-1 throttle proper, for a process dirty *via Phase 1* — deep in
        // draining-generation code, which the hoisted Phase-2 check above does not cover.
        // Gated on seed size so a shallow process is never throttled and keeps reporting
        // its transition to clean on the very next safepoint.
        let p1_seed =
            self.roots.len() + self.env_roots.len() + self.dynamics.len() + self.live_vm_arms.len();
        let p1_large = p1_seed > P1_LARGE_SEED;
        if p1_large && self.p1_dirty_epoch.get() == epoch {
            let t = self.p1_dirty_tick.get().wrapping_add(1);
            self.p1_dirty_tick.set(t);
            if !t.is_multiple_of(P1_REVALIDATE_STRIDE) {
                return true; // stale-dirty: skip the O(depth) re-walk this safepoint
            }
        }
        if self.seed_phase1_and_walk(
            gen,
            false,
            &mut work,
            &mut env_work,
            &mut visited,
            &mut visited_env,
        ) {
            // Arm / keep the re-validation throttle for this epoch (large seeds only).
            if p1_large && self.p1_dirty_epoch.get() != epoch {
                self.p1_dirty_epoch.set(epoch);
                self.p1_dirty_tick.set(0);
            }
            return true;
        }
        // Clean via Phase 1 — disarm so a later re-dirty is caught at once.
        self.p1_dirty_epoch.set(u64::MAX);

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
        // dirty via Phase 1 (running old-gen code) sets its own, separately-gated throttle,
        // which stays disarmed for a shallow process — so such a process still reports its
        // transition to clean immediately (the drain-completion tests rely on that
        // promptness). The stale-dirty short-circuit itself is hoisted to the TOP of this
        // function; see the comment there for why.
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
        #[cfg(feature = "dev-tools")]
        work.extend(self.trace_context.iter().copied());
        // --- Live VM arms mid-execution: RUNTIME literals baked into Const/MakeClosure
        // (the one holder off the GC root graph, mirroring compaction's step 3b). Read
        // them via the arm-handle visitor, returning each value unchanged. ---
        //
        // **Visit each DISTINCT arm once.** `live_vm_arms` is a per-frame stack, so a
        // recursive function occupies one entry per *active frame* — a 100 000-deep parse
        // holds 100 000 entries that are all the same `Arc`. An arm's handle set doesn't
        // depend on which frame is running it, so walking it once per distinct arm is
        // equivalent and collapses that to a handful.
        //
        // Without the dedup this walk alone is O(depth × arm size). It is one of three
        // things that made a deep process's probe explode under KI-14; the throttles in
        // `runtime_gen_referenced_private` bound how often the probe runs, while this bounds
        // what a single run costs. (It also drops a full `Vec<Arc<_>>` clone — one atomic
        // increment per active frame — that the walk previously took for borrow reasons it
        // no longer needs.)
        let mut seen_arms: HashSet<*const crate::eval::compile::CompiledArm> = HashSet::new();
        for arm in self.live_vm_arms.iter() {
            if !seen_arms.insert(Arc::as_ptr(arm)) {
                continue;
            }
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
        for slabs in [Some(&self.local), self.old_opt()].into_iter().flatten() {
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
    /// The current drain's `(epoch, generation)` read as one snapshot — the identity a
    /// freer must carry from validation through to the destructive store, so it cannot free
    /// a generation a *newer* drain armed. See [`free_runtime_gen`](Self::free_runtime_gen).
    pub fn drain_identity(&self) -> (u64, usize) {
        let rt = &self.runtime;
        let epoch = rt.drain_epoch.load(Ordering::Relaxed);
        let gen = rt.drain_gen.load(Ordering::Relaxed);
        (epoch, gen)
    }

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
        #[cfg(feature = "dev-tools")]
        if let Some(v) = &mut self.trace_context {
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
        if let Some(o) = self.old.as_deref_mut() {
            rewrite_local_rt_handles(o, &old_code, &new, &mut fwd);
        }
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
        // ...and the RUNTIME-shared compiled closures (ADR-175). The live arms above were
        // rewritten because they are on this process's execution stack; a merely *cached*
        // shared arm is on no stack, so its embedded RUNTIME handles still address the
        // pre-compaction region. Dropping the cache makes every process recompile from the
        // (already rewritten) AST. Safe unconditionally — compaction holds `Arc::get_mut`
        // on the runtime, so this is the only process.
        self.shared_closures_clear();
        self.global_ic.borrow_mut().clear();
        self.vm_call_ics.borrow_mut().clear();
        // Clear the IR-readable mirror in lockstep (recycled sites get a fresh slot; a
        // live arm's now-out-of-range site id is caught by the JIT's `site < len` guard).
        self.vm_fast_links.borrow_mut().clear();
        #[cfg(debug_assertions)]
        self.dbg_site_pos.borrow_mut().clear();
        self.vm_global_ics.borrow_mut().clear();
        // ADR-175 Phase A: the per-arm block registry indexes the tables just cleared,
        // so it clears in lockstep. Live arms re-resolve fresh blocks on their next
        // activation; the currently-executing arm's stale cursors are benign (probes
        // bounds-check → miss, and every entry validates sym+argc+epoch).
        self.arm_ic_blocks.borrow_mut().clear();

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
    ratios: HashMap<u32, u32>,
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
        ValueRef::Ratio(id) => in_rt(id.region(), id.code_gen()),
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
    // Deep-car-nesting guard — see `WALKER_RED_ZONE`, and the identical guard on the
    // LOCAL twin `gc::flush_value`. The cdr spine below is iterative, but *car* nesting
    // still recurses `flush_rt_value` ⇄ `flush_rt_pair` one native frame per level, and a
    // deeply nested value promoted into RUNTIME (a 100 000-level JSON document under test)
    // ran the thread into its guard page — an abort, not a catchable error. RT compaction
    // fires at auto-safepoints (ADR-091), so the collecting thread's remaining stack is
    // arbitrary; grow into heap-backed segments rather than assume there is room.
    stacker::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
        flush_rt_value_grown(old, new, fwd, v)
    })
}

fn flush_rt_value_grown(
    old: &CodeSlabs,
    new: &CodeSlabs,
    fwd: &mut RuntimeForward,
    v: Value,
) -> Value {
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
        ValueRef::Ratio(id) if id.region() == RUNTIME && id.code_gen() == g => {
            Value::ratio(flush_rt_ratio(old, new, fwd, id))
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

/// Flush a RUNTIME ratio during a runtime-region compaction (mirrors
/// [`flush_rt_decimal`]). A leaf — clone the value into the new region.
fn flush_rt_ratio(
    old: &CodeSlabs,
    new: &CodeSlabs,
    fwd: &mut RuntimeForward,
    id: RatioId,
) -> RatioId {
    let (key, dest) = (id.index() as u32, fwd.dest_gen);
    if let Some(&n) = fwd.ratios.get(&key) {
        return RatioId::runtime_gen(n as usize, dest);
    }
    let v = old.ratios.get(id.index()).expect("rt ratio").clone();
    let new_idx = new.ratios.push(v);
    fwd.ratios.insert(key, new_idx as u32);
    RatioId::runtime_gen(new_idx, dest)
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
    let (np, nv, nm, ns, nb, nd, nra, nby, nr, nc, ne) = (
        s.pairs.count(),
        s.vectors.count(),
        s.maps.count(),
        s.strings.count(),
        s.bigints.count(),
        s.decimals.count(),
        s.ratios.count(),
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
            ValueRef::Ratio(id) if id.region() == RUNTIME => id.index() < nra,
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
