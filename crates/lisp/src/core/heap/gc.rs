//! GC: the tracing collector itself — `collect`, the minor and major passes, the heap
//! verifier — with its roots, accounting, flush, stall guard and tuning as child modules
//! (child of heap; every child reaches `Heap`'s private fields via `use super::*`).
use super::*;

mod accounting;
mod flush;
mod roots;
mod stall;
#[cfg(test)]
mod tests;
mod tuning;

use flush::*;
pub(crate) use stall::{stall_guard, stall_guard_pid, stall_threshold_ms};
pub(in crate::core::heap) use tuning::*;

impl Heap {
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
        // whole collection and fold it into the per-process totals `(dev/gc-stats)`
        // reports. Only recorded when a collection actually ran (`gc_runs`
        // moved) — a gated no-op call isn't a pause. Two `Instant` reads per
        // collection: noise against the collection itself.
        let runs_before = self.gc_runs;
        let t0 = web_time::Instant::now();
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
                    self.old_opt().map_or(0, |o| o.pairs.len()),
                    self.old_opt().map_or(0, |o| o.vectors.len()),
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
        self.gc_threshold = std::cmp::max(
            gc_floor(),
            (((live_total as f64) * Self::gc_growth()) as usize).min(NURSERY_MAX),
        );
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
        self.old_opt().map_or(0, slab_live_count)
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
        // Delivered-message slots (L1) — same reasoning as the nursery flush above: a
        // queued Local message outlives arbitrarily many collections.
        for v in self.msg_roots.iter_mut().flat_map(|t| t.slots.iter_mut()) {
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
            (
                self.old.take().map(|b| *b).unwrap_or_default(),
                self.old_epoch,
                true,
            )
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
                self.old().envs[e.index()].vars.len()
            };
            for i in 0..n {
                let (s, v) = if tenure {
                    dest.envs[e.index()].vars[i]
                } else {
                    self.old().envs[e.index()].vars[i]
                };
                let nv = flush_value(&young, &mut dest, &mut fwd, v);
                if tenure {
                    dest.envs[e.index()].vars[i] = (s, nv);
                } else {
                    self.old_mut().envs[e.index()].vars[i] = (s, nv);
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
        if self.cold.is_some() {
            // An OLD entry keeps its exact key — old doesn't move in a minor — so only the
            // young survivors need re-keying. Taking the whole map and reinserting every
            // entry re-hashed and re-allocated the tenured ones on *every* collection,
            // which is O(all positions recorded so far) per minor rather than O(nursery).
            // Retain in place, and reinsert only the entries whose slab index moved.
            let mut moved: Vec<(u64, crate::core::heap::FormPos)> = Vec::new();
            let cold = self.cold_mut();
            cold.form_pos.retain(|&key, pos| {
                if (key >> 32) & 1 == 1 {
                    true
                } else if let Some(new_idx) = fwd.pairs.lookup(key as u32) {
                    moved.push(((new_idx as u64) | new_age_bit, pos.clone()));
                    false
                } else {
                    false
                }
            });
            for (k, p) in moved {
                cold.form_pos.insert(k, p);
            }
            // `retain` above drops dead entries but never releases their slots, and
            // `HashMap` never shrinks itself — so without this the map keeps its
            // HIGH-WATER capacity for the whole process life. Measured 2026-08-20 on a
            // 38-module stdlib load: 8 000 live entries sitting in 25 156 slots, i.e.
            // 830 KB where 265 KB was live.
            //
            // Shrinking has to stay RARE, because rebuilding this map is exactly the
            // O(all positions recorded so far) cost the retain-in-place above exists to
            // avoid. Hence hysteresis: only when capacity exceeds `SHRINK_RATIO x` the
            // live count, and only down to `HEADROOM x` it — so after a shrink the map
            // must grow by `SHRINK_RATIO / HEADROOM` again before another can trigger,
            // and a steady-state heap never shrinks twice for the same entries.
            const SHRINK_FLOOR: usize = 1024;
            const SHRINK_RATIO: usize = 3;
            const HEADROOM: usize = 2;
            let (len, cap) = (cold.form_pos.len(), cold.form_pos.capacity());
            if cap > SHRINK_FLOOR && cap > SHRINK_RATIO.saturating_mul(len.max(1)) {
                cold.form_pos
                    .shrink_to(len.saturating_mul(HEADROOM).max(SHRINK_FLOOR));
            }
        }
        // Install the relocated space. Tenure: `dest` is the grown old gen; the
        // nursery restarts empty but with the outgoing nursery's capacity (same
        // doubling-ladder rationale as the flip path). Flip: `dest` is the fresh
        // nursery; the old gen was untouched.
        if tenure {
            self.old = Some(Box::new(dest));
            // The nursery restarts EMPTY on a tenure — every survivor was moved into the
            // old gen — so reserving the *outgoing* nursery's full length holds a
            // peak-sized allocation the next cycle may never touch. That is the least
            // justified of the two `with_capacity_like` uses: on the flip path below,
            // `dest` genuinely holds the survivors, so its capacity is in use. `sort`
            // builds a 375k-cell list and peaks at 191 MB against .NET's 30 MB and
            // Ruby's 25 MB, and this reservation is a large part of the difference.
            // `BROOD_GC_TENURE_RESERVE=1` restores the old behaviour for an A/B.
            self.local = if std::env::var_os("BROOD_GC_TENURE_RESERVE").is_some() {
                Slabs::with_capacity_like(&young)
            } else {
                Slabs::default()
            };
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
        let old_src = self.old.take().map(|b| *b).unwrap_or_default();
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
        // `self.old().envs[e.index()]` with a stale handle and no bounds/epoch check
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
        // form_pos re-key across a major: only the OLD generation moved, so an old entry
        // is re-keyed through the forwarding table (dropped if its pair died) and a
        // NURSERY entry is kept exactly as it is.
        //
        // This used to `take` the whole map and reinsert only the entries whose age bit
        // was set — silently discarding every young entry, i.e. the recorded position of
        // every still-live *nursery* form, which a major does not move. A load that
        // allocates heavily (flip minor, survivors stay young) followed by a major thus
        // lost the positions of the forms it had just read: error messages, `(form-pos …)`
        // and the test framework's line lookups all went blank for them. The minor path
        // (`minor_collect`) has always retained in place correctly; this mirrors it, which
        // also drops the O(all positions recorded so far) rehash the `take` cost per major.
        if self.cold.is_some() {
            let mut moved: Vec<(u64, crate::core::heap::FormPos)> = Vec::new();
            let cold = self.cold_mut();
            cold.form_pos.retain(|&key, pos| {
                if (key >> 32) & 1 == 0 {
                    true // a nursery entry — a major leaves the nursery in place
                } else if let Some(new_idx) = fwd.pairs.lookup(key as u32) {
                    moved.push(((new_idx as u64) | (1 << 32), pos.clone()));
                    false
                } else {
                    false // the tenured pair was reclaimed by this major
                }
            });
            for (k, p) in moved {
                cold.form_pos.insert(k, p);
            }
        }
        self.old = Some(Box::new(dest));
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
            vec![false; self.old_opt().map_or(0, |o| o.pairs.len())],
        ];
        let mut seen_vec = [
            vec![false; self.local.vectors.len()],
            vec![false; self.old_opt().map_or(0, |o| o.vectors.len())],
        ];
        let mut seen_map = [
            vec![false; self.local.maps.len()],
            vec![false; self.old_opt().map_or(0, |o| o.maps.len())],
        ];
        let mut seen_clo = [
            vec![false; self.local.closures.len()],
            vec![false; self.old_opt().map_or(0, |o| o.closures.len())],
        ];
        let mut seen_env = [
            vec![false; self.local.envs.len()],
            vec![false; self.old_opt().map_or(0, |o| o.envs.len())],
        ];
        let mut work: Vec<W> = Vec::new();
        for &v in extra_roots {
            work.push(W::V(v, 0));
        }
        for &e in extra_envs {
            work.push(W::E(e, 0));
        }
        // Delivered-message slots (L1) are a root set too — a stale handle parked in
        // one would otherwise surface far away, at the `receive` that finally pops it.
        for &v in self.msg_roots.iter().flat_map(|t| t.slots.iter()) {
            work.push(W::V(v, 0));
        }
        for &v in self.roots.iter() {
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
                if let Some(frame) = self.old_opt().and_then(|o| o.envs.get(e.index())) {
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                    ValueRef::Map(id) | ValueRef::Set(id) | ValueRef::Failure(id)
                        if id.region() == LOCAL =>
                    {
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                    ValueRef::Ratio(id) if id.region() == LOCAL => {
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
                        bad(
                            "ratio",
                            id.is_old(),
                            id.generation(),
                            id.index(),
                            slabs.ratios.len(),
                            parent,
                            id.0,
                        );
                    }
                    ValueRef::Bytes(id) if id.region() == LOCAL => {
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                        let Some(slabs) = (if id.is_old() {
                            self.old_opt()
                        } else {
                            Some(&self.local)
                        }) else {
                            continue;
                        };
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
                    let slabs = if e.is_old() { self.old() } else { &self.local };
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
