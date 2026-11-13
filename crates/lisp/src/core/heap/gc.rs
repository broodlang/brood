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
    /// rooting-completeness argument. A generational copy: a minor collection
    /// ([`minor_collect`](Self::minor_collect)) relocates every nursery object
    /// reachable from `extra_roots` (the eval's `expr`), `extra_envs` (its `env`), the
    /// dynamic stack, and the explicit root stack — tenuring them into the old
    /// generation or flipping them into a fresh nursery — and drops the rest; a
    /// [`major_collect`](Self::major_collect) follows when the old generation has
    /// grown enough. Each bumps its generation's epoch, so any handle held across this
    /// without being re-rooted trips the tripwire at its next deref.
    ///
    /// Because it MOVES survivors, the caller **must** use the relocated handles
    /// written back into `extra_roots`/`extra_envs`. Recomputes the adaptive
    /// threshold so the next collection fires when the live set doubles (amortized
    /// O(1) copying per allocation; `BROOD_GC_STRESS=1` ⇒ every safepoint). No-op
    /// while GC is disabled (the builder heap during prelude construction).
    pub fn collect(&mut self, extra_roots: &mut [Value], extra_envs: &mut [EnvId]) {
        // Pause-duration accounting (the observability timing tier): time the
        // whole collection and fold it into the per-process totals `(dev/gc-stats)`
        // reports. Only recorded when a collection actually ran (`gc_runs`
        // moved) — a gated no-op call isn't a pause. Two `Instant` reads per
        // collection: noise against the collection itself.
        // `&mut self`: no RUNTIME borrow can be live, so the generation `Arc`s the cache
        // replaced can go (see `code_gen_ref`).
        self.release_retired_gens();
        let runs_before = self.gc_runs;
        let started = web_time::Instant::now();
        self.collect_inner(extra_roots, extra_envs);
        // A RUNTIME drain this process's data pins: copy it out now, with the collection's
        // roots in hand and the garbage already gone (ADR-091, `rehome_drain_pins`). Inside
        // the timed span, so a slow one shows as collector time where the pause is measured.
        if self.rehome_asked.get().is_some() {
            self.rehome_drain_pins(extra_roots, extra_envs);
        }
        if self.gc_runs != runs_before {
            let ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            let extras = self.extras_mut();
            extras.gc_ns_total = extras.gc_ns_total.saturating_add(ns);
            extras.gc_ns_max = extras.gc_ns_max.max(ns);
            extras.gc_ns_last = ns;
            // System-monitor GC event (the observability event stream). Emitted
            // *after* the collection completes — the heap is consistent and the
            // event build/deliver touches only Rust data, never this heap. The
            // subscriber's own collections are excluded inside emit_gc (its
            // event traffic would otherwise re-trigger itself forever).
            if crate::process::sysmon::armed()
                && let Some(pid) = crate::process::current_pid()
            {
                crate::process::sysmon::emit_gc(
                    pid,
                    ns,
                    self.gc_runs,
                    self.local_live_count() as u64,
                );
            }
        }
    }

    fn collect_inner(&mut self, extra_roots: &mut [Value], extra_envs: &mut [EnvId]) {
        // Stall trace (BROOD_STALL_MS=<n>): log if this minor collection takes ≥ n ms — to
        // pinpoint a gameplay lag spike. Works in release; zero cost unless the env is set.
        let _stall = stall_guard("minor-gc");
        // GC trace (BROOD_GC_TRACE=1): log each minor collection's working set.
        #[cfg(debug_assertions)]
        {
            static GC_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if crate::diagnostics::debug_flags::is_armed(&GC_TRACE, "BROOD_GC_TRACE") {
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
        // Where the old generation's buffer slabs end before a tenure appends to them, so
        // the bytes it adds are measured over what it copied, not the whole old generation.
        let old_mark = self.old_opt().map(buffer_mark).unwrap_or_default();
        self.minor_collect(tenure, extra_roots, extra_envs);
        // The old generation's buffer bytes, kept in the extras: only a collection touches
        // them, and `collect` boxes the extras for its pause timing anyway.
        let mut old_bytes = self.extras().map_or(0, |extras| extras.old_buffer_bytes);
        // The byte budget refills from nothing spent: survivors are paid for, and keeping
        // them in the count would make a collection due at every safepoint over one large
        // live young vector. They size the budget instead, beside the old generation's bytes.
        let young_bytes = if tenure {
            old_bytes = old_bytes.saturating_add(buffer_bytes_since(self.old(), old_mark));
            0
        } else {
            buffer_bytes_since(&self.local, BufferMark::default())
        };
        let retained_bytes = young_bytes.saturating_add(old_bytes);
        self.nursery_budget_left = budget_units(std::cmp::max(
            nursery_byte_floor(),
            (((retained_bytes as f64) * Self::gc_growth()) as usize).min(NURSERY_BYTE_MAX),
        ));
        // Next minor fires when the *young* gen reaches `gc_threshold`. Scale it with
        // the **total** live set (young + old): a tenuring build leaves young ≈ 0, so a
        // young-only scale would collapse to the floor and collect O(n/floor) times
        // while building one large structure; counting old live makes that O(log n).
        // Capped at NURSERY_MAX so a process with a large old gen that churns transient
        // garbage cannot buffer young memory proportional to the old gen.
        let live_total = self.local_live_count() + self.old_live_count();
        self.gc_threshold = std::cmp::max(
            gc_floor(),
            (((live_total as f64) * Self::gc_growth()) as usize).min(NURSERY_MAX),
        );
        // Escalate to a *major* (compact the old generation) only when it has grown
        // `major_growth()`× since the last major — so majors stay rare while minors keep
        // the nursery bounded. During a large-structure build the old gen is nearly
        // all-live, so a major reclaims little; a larger factor trades retained garbage
        // for copy throughput.
        // Or when the old generation's buffers have grown as much: a few tenured objects can
        // own gigabytes of dead vector, which the object count would wait on indefinitely.
        let major_budget = self.extras().map_or(0, |extras| extras.major_buffer_budget);
        if self.old_live_count() >= self.major_threshold
            || old_bytes >= std::cmp::max(MAJOR_BYTE_FLOOR, major_budget)
        {
            self.major_collect(extra_roots, extra_envs);
            self.major_threshold = std::cmp::max(
                major_floor(),
                self.old_live_count().saturating_mul(major_growth()),
            );
            // A major copies the whole old generation, so measuring it whole costs the same.
            old_bytes = self
                .old_opt()
                .map_or(0, |old| buffer_bytes_since(old, BufferMark::default()));
            self.extras_mut().major_buffer_budget = old_bytes.saturating_mul(major_growth());
        }
        self.extras_mut().old_buffer_bytes = old_bytes;
    }

    /// Live objects in the **old generation** (`Σ old.slab.len()`). Old is
    /// append-only between major collections, so the slab lengths *are* the
    /// live count. Drives the major-collection threshold.
    pub fn old_live_count(&self) -> usize {
        self.old_opt().map_or(0, slab_live_count)
    }

    /// Relocate every GC root through `forward`, from `source` into `dest`: the caller's
    /// `value_roots`/`env_roots` (the eval frame's `expr`/`env`), this process's
    /// dynamic-binding stack, and the operand stack (`roots` + `env_roots`). The
    /// single place the GC root set is enumerated — minor and major collection
    /// share it so the two can't drift (a divergent root set would be a
    /// use-after-GC bug). `dest` is a *local* `Slabs` (never a `self` field) so the
    /// `&mut self` for the stacks doesn't alias it.
    fn flush_roots(
        &mut self,
        source: &mut Slabs,
        dest: &mut Slabs,
        forward: &mut FlushForward,
        value_roots: &mut [Value],
        env_roots: &mut [EnvId],
    ) {
        for v in value_roots.iter_mut() {
            *v = flush_value(source, dest, forward, *v);
        }
        for e in env_roots.iter_mut() {
            *e = flush_env(source, dest, forward, *e);
        }
        for (_, v) in self.dynamics.iter_mut() {
            *v = flush_value(source, dest, forward, *v);
        }
        #[cfg(feature = "dev-tools")]
        if let Some(v) = self.extras.as_mut().and_then(|e| e.trace_context.as_mut()) {
            *v = flush_value(source, dest, forward, *v);
        }
        if let Some(extras) = self.extras.as_mut() {
            for origin in extras.caught.iter_mut() {
                origin.payload = flush_value(source, dest, forward, origin.payload);
            }
            // `defmemo` stores (ADR-670): held uncopied in this heap, so they relocate here.
            for store in extras.memo_stores.iter_mut() {
                store.entries = flush_value(source, dest, forward, store.entries);
            }
        }
        for v in self.roots.iter_mut() {
            *v = flush_value(source, dest, forward, *v);
        }
        // Delivered-message slots (L1): a message copied straight into this heap by a
        // sender can sit queued through any number of collections before a selective
        // `receive` reaches it, so the slots relocate in place like the operand stack.
        for v in self
            .msg_roots
            .iter_mut()
            .flat_map(|slots| slots.slots.iter_mut())
        {
            *v = flush_value(source, dest, forward, *v);
        }
        // The env half of the operand stack (ADR-061).
        let mut operand_envs = std::mem::take(&mut self.env_roots);
        for e in operand_envs.iter_mut() {
            *e = flush_env(source, dest, forward, *e);
        }
        self.env_roots = operand_envs;
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
    fn minor_collect(&mut self, tenure: bool, value_roots: &mut [Value], env_roots: &mut [EnvId]) {
        let t_start = web_time::Instant::now();
        let before_young = self.local_live_count();
        let old_before = self.old_live_count();
        self.local_epoch = self.local_epoch.wrapping_add(1);
        // Every nursery handle is about to name something else (ADR-480).
        self.type_verdicts_forget_nursery();
        let mut young = std::mem::take(&mut self.local);
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
        let mut forward = FlushForward::for_source(&young);
        forward.epoch = epoch;
        forward.src_old = false; // copy nursery objects
        forward.dest_old = dest_old;
        self.flush_roots(&mut young, &mut dest, &mut forward, value_roots, env_roots);
        let t_roots = web_time::Instant::now();
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
                let nv = flush_value(&mut young, &mut dest, &mut forward, v);
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
        let t_remembered = web_time::Instant::now();
        let positions = self.cold.as_ref().map_or(0, |c| c.form_pos.len());
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
                } else if let Some(new_idx) = forward.pairs.lookup(key as u32) {
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
            // high-water capacity for the whole process life.
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
        let t_positions = web_time::Instant::now();
        // Install the relocated space. Tenure: `dest` is the grown old gen; the
        // nursery restarts empty but with the outgoing nursery's capacity (same
        // doubling-ladder rationale as the flip path). Flip: `dest` is the fresh
        // nursery; the old gen was untouched.
        if tenure {
            self.old = Some(Box::new(dest));
            // The nursery restarts EMPTY on a tenure — every survivor moved into the old
            // gen — so reserving the outgoing nursery's length would hold a peak-sized
            // allocation the next cycle may never touch (unlike the flip path, whose
            // `dest` really holds the survivors). `BROOD_GC_TENURE_RESERVE=1` reserves
            // anyway, for an A/B.
            static RESERVE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            self.local =
                if crate::diagnostics::debug_flags::is_set(&RESERVE, "BROOD_GC_TENURE_RESERVE") {
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
        let extras = self.extras_mut();
        extras.gc_copied = extras.gc_copied.saturating_add(survivors as u64);
        extras.gc_reclaimed = extras
            .gc_reclaimed
            .saturating_add(before_young.saturating_sub(survivors) as u64);
        self.note_proc_limit();
        if self.gc_trace() {
            let us = |a: web_time::Instant, b: web_time::Instant| b.duration_since(a).as_micros();
            eprintln!(
                "[gc] minor {}: {} nursery objects, {} {}, {} reclaimed | {}us: roots={}us \
                 remembered={}us positions={}us ({} entries) install={}us",
                if tenure { "tenure" } else { "flip" },
                before_young,
                survivors,
                if tenure { "tenured" } else { "kept young" },
                before_young.saturating_sub(survivors),
                t_start.elapsed().as_micros(),
                us(t_start, t_roots),
                us(t_roots, t_remembered),
                us(t_remembered, t_positions),
                positions,
                t_positions.elapsed().as_micros(),
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
        // Every old-generation handle is about to name something else (ADR-480).
        self.type_verdicts_forget_local();
        let mut old_src = self.old.take().map(|b| *b).unwrap_or_default();
        let mut dest = Slabs::default();
        let mut forward = FlushForward::for_source(&old_src);
        forward.epoch = self.old_epoch;
        forward.src_old = true; // copy old-gen objects
        forward.dest_old = true; // into the fresh old space
        self.flush_roots(
            &mut old_src,
            &mut dest,
            &mut forward,
            value_roots,
            env_roots,
        );
        // If the preceding minor was a flip (not a tenure) the nursery is
        // non-empty.  `flush_roots` updated handles in `self.roots` and
        // `self.env_roots`, but OLD handles *inside* nursery objects were
        // silently skipped by `flush_value`/`flush_env` (they gate on
        // `forward.copies`, which is false for nursery objects during a major).
        // Rewrite those stale OLD handles in-place now, while `old_src` is
        // still live.
        flush_nursery_old_refs(&mut self.local, &mut old_src, &mut dest, &mut forward);
        // Write barrier across a major. After a *tenure* minor `remembered` is
        // empty (the minor cleared it). But `collect()` can run a major right
        // after a *flip* minor, and a flip RETAINS `remembered` — old EnvIds for
        // frames that gained a young binding, pointing into the pre-compaction old
        // gen (the old->young edges persist; see `minor_collect`). This major just
        // relocated those frames into fresh slabs and bumped `old_epoch`, so every
        // retained entry is now a stale index *and* a stale epoch. Rewrite each
        // through the env forwarding table (`forward.envs`, populated by `flush_roots`)
        // and drop any whose frame wasn't copied — it was unreachable, so the major
        // reclaimed it. The next `minor_collect` indexes `self.old().envs[e.index()]`
        // with no bounds/epoch check, so a stale entry would be a silent use-after-GC.
        if !self.remembered.is_empty() {
            let remembered = std::mem::take(&mut self.remembered);
            self.remembered = remembered
                .into_iter()
                .filter_map(|e| {
                    forward
                        .envs
                        .lookup(e.index() as u32)
                        .map(|n| forward.mint_env(n as usize))
                })
                .collect();
        }
        // form_pos re-key across a major: only the OLD generation moved, so an old entry
        // is re-keyed through the forwarding table (dropped if its pair died) and a
        // NURSERY entry — a live form a major does not move — is kept exactly as it is.
        // Retained in place like `minor_collect`, not rebuilt.
        if self.cold.is_some() {
            let mut moved: Vec<(u64, crate::core::heap::FormPos)> = Vec::new();
            let cold = self.cold_mut();
            cold.form_pos.retain(|&key, pos| {
                if (key >> 32) & 1 == 0 {
                    true // a nursery entry — a major leaves the nursery in place
                } else if let Some(new_idx) = forward.pairs.lookup(key as u32) {
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
        let extras = self.extras_mut();
        extras.gc_copied = extras.gc_copied.saturating_add(survivors as u64);
        extras.gc_reclaimed = extras
            .gc_reclaimed
            .saturating_add(before_old.saturating_sub(survivors) as u64);
        if self.gc_trace() {
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
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        crate::diagnostics::debug_flags::is_set(&ON, "BROOD_GC_VERIFY")
    }

    /// Debug heap verifier (`BROOD_GC_VERIFY`). Walk every LOCAL handle reachable
    /// from the supplied roots + the explicit root / env-root / dynamic stacks and
    /// assert each is (a) in-bounds for its slab and (b) stamped with the current
    /// epoch of its generation. Between collections every *live* LOCAL handle must be
    /// current-epoch (survivors are re-minted at the current epoch, new allocations
    /// use it), so a reachable handle from an older epoch was held across an earlier
    /// collection without being re-rooted and then **stored into the live graph** —
    /// the use-after-GC class the per-deref tripwire misses because the bad handle is
    /// written, not dereferenced. Panics naming the bad handle and the raw handle of
    /// the cell that holds it (`0` = a root), which with the offending op pinpoints the
    /// missed rooting site. O(live nursery); only runs under the env flag, in release
    /// too (see `gc_verify_enabled`).
    ///
    /// The walk descends into NURSERY objects only: old objects are immutable after
    /// tenuring, so their handles are checked where they are reached but their
    /// contents are not re-walked every collection (that made it O(old) and timed out
    /// the large-structure tests). The one mutable old object is a `remembered` frame,
    /// whose bindings are seeded as roots. It does *not* assert no-old→young: the
    /// `remembered` set legitimately carries such edges until the next minor.
    fn verify_local_graph(&self, extra_roots: &[Value], extra_envs: &[EnvId]) {
        // The worklist carries Copy handles plus the raw handle of the containing
        // cell — no per-node paths, since this runs at every safepoint under
        // GC_STRESS and must not itself churn the heap.
        enum Pending {
            Value(Value, u64),
            Env(EnvId, u64),
        }
        fn first_visit(seen: &mut [bool], index: usize) -> bool {
            !std::mem::replace(&mut seen[index], true)
        }
        // Truncated like `check_epoch_aged` — see `epoch_in_gen_width`.
        let young_epoch = Self::epoch_in_gen_width(self.local_epoch);
        let old_epoch = Self::epoch_in_gen_width(self.old_epoch);
        let young = &self.local;
        let slabs_for = |is_old: bool| if is_old { self.old_opt() } else { Some(young) };
        // Visited marks for the nursery kinds the walk descends into.
        let mut seen_pairs = vec![false; young.pairs.len()];
        let mut seen_vectors = vec![false; young.vectors.len()];
        let mut seen_maps = vec![false; young.maps.len()];
        let mut seen_closures = vec![false; young.closures.len()];
        let mut seen_envs = vec![false; young.envs.len()];

        let mut work: Vec<Pending> = Vec::new();
        work.extend(extra_roots.iter().map(|&v| Pending::Value(v, 0)));
        work.extend(extra_envs.iter().map(|&e| Pending::Env(e, 0)));
        // Delivered-message slots (L1) are a root set too — a stale handle parked in
        // one would otherwise surface far away, at the `receive` that finally pops it.
        work.extend(
            self.msg_roots
                .iter()
                .flat_map(|slots| slots.slots.iter())
                .map(|&v| Pending::Value(v, 0)),
        );
        work.extend(self.roots.iter().map(|&v| Pending::Value(v, 0)));
        work.extend(self.env_roots.iter().map(|&e| Pending::Env(e, 0)));
        work.extend(self.dynamics.iter().map(|&(_, v)| Pending::Value(v, 0)));
        #[cfg(feature = "dev-tools")]
        if let Some(v) = self.trace_context() {
            work.push(Pending::Value(v, 0));
        }
        if let Some(extras) = self.extras() {
            work.extend(extras.caught.iter().map(|o| Pending::Value(o.payload, 0)));
            work.extend(
                extras
                    .memo_stores
                    .iter()
                    .map(|store| Pending::Value(store.entries, 0)),
            );
        }
        for &frame_id in &self.remembered {
            if frame_id.is_old()
                && let Some(frame) = self.old_opt().and_then(|o| o.envs.get(frame_id.index()))
            {
                work.extend(
                    frame
                        .vars
                        .iter()
                        .map(|&(_, v)| Pending::Value(v, frame_id.0)),
                );
            }
        }

        let check = |kind: &str,
                     is_old: bool,
                     generation: u32,
                     index: usize,
                     length: usize,
                     parent: u64,
                     raw: u64| {
            let (epoch, space) = if is_old {
                (old_epoch, "OLD")
            } else {
                (young_epoch, "nursery")
            };
            assert!(
                index < length,
                "GC-VERIFY: stored stale {kind} handle OUT OF BOUNDS ({space} slot {index} \
                 ≥ slab len {length}); handle {raw:#x} held in container {parent:#x}. \
                 A handle was kept across a collection without re-rooting, then \
                 written into the live graph — use-after-GC.",
            );
            assert!(
                generation == epoch,
                "GC-VERIFY: stored stale {kind} handle from epoch {generation}, {space} \
                 generation is now epoch {epoch} (slot {index}, handle {raw:#x}); held in \
                 container {parent:#x}. That cell holds a handle kept across a collection \
                 without re-rooting — use-after-GC at the op that built it.",
            );
        };

        while let Some(item) = work.pop() {
            let (value, parent) = match item {
                Pending::Env(env, parent) => {
                    if env == EnvId::GLOBAL || env.region() != LOCAL {
                        continue;
                    }
                    let Some(slabs) = slabs_for(env.is_old()) else {
                        continue;
                    };
                    check(
                        "env",
                        env.is_old(),
                        env.generation(),
                        env.index(),
                        slabs.envs.len(),
                        parent,
                        env.0,
                    );
                    if !env.is_old() && first_visit(&mut seen_envs, env.index()) {
                        let frame = &young.envs[env.index()];
                        if let Some(enclosing) = frame.parent {
                            work.push(Pending::Env(enclosing, env.0));
                        }
                        work.extend(frame.vars.iter().map(|&(_, v)| Pending::Value(v, env.0)));
                    }
                    continue;
                }
                Pending::Value(value, parent) => (value, parent),
            };
            // Every LOCAL heap kind: (kind name, age, epoch, index, raw handle, its slab).
            type SlabLength = fn(&Slabs) -> usize;
            let (kind, is_old, generation, index, raw, slab_length): (
                &str,
                bool,
                u32,
                usize,
                u64,
                SlabLength,
            ) = match value.unpack() {
                ValueRef::Pair(id) if id.region() == LOCAL => (
                    "pair",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.pairs.len(),
                ),
                ValueRef::Vector(id) if id.region() == LOCAL => (
                    "vector",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.vectors.len(),
                ),
                ValueRef::Range(id) if id.region() == LOCAL => (
                    "range",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.vectors.len(),
                ),
                ValueRef::SeqView(id) if id.region() == LOCAL => (
                    "seq-view",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.vectors.len(),
                ),
                ValueRef::Map(id)
                | ValueRef::Set(id)
                | ValueRef::Failure(id)
                | ValueRef::Record { fields: id, .. }
                    if id.region() == LOCAL =>
                {
                    ("map", id.is_old(), id.generation(), id.index(), id.0, |s| {
                        s.maps.len()
                    })
                }
                ValueRef::Str(id) if id.region() == LOCAL => (
                    "string",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.strings.len(),
                ),
                ValueRef::BigInt(id) if id.region() == LOCAL => (
                    "bigint",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.bigints.len(),
                ),
                ValueRef::Decimal(id) if id.region() == LOCAL => (
                    "decimal",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.decimals.len(),
                ),
                ValueRef::Ratio(id) if id.region() == LOCAL => (
                    "ratio",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.ratios.len(),
                ),
                ValueRef::Bytes(id) if id.region() == LOCAL => (
                    "bytes",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.bytes.len(),
                ),
                ValueRef::Rope(id) if id.region() == LOCAL => (
                    "rope",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.ropes.len(),
                ),
                ValueRef::Fn(id) | ValueRef::Macro(id) if id.region() == LOCAL => (
                    "closure",
                    id.is_old(),
                    id.generation(),
                    id.index(),
                    id.0,
                    |s| s.closures.len(),
                ),
                _ => continue,
            };
            let Some(slabs) = slabs_for(is_old) else {
                continue;
            };
            check(
                kind,
                is_old,
                generation,
                index,
                slab_length(slabs),
                parent,
                raw,
            );
            if is_old {
                continue;
            }
            // Descend into the nursery kinds that hold heap values. A range's backing
            // vector holds only ints; a seq-view's `[source xform]` shares the vectors
            // slab and its marks.
            match value.unpack() {
                ValueRef::Pair(_) if first_visit(&mut seen_pairs, index) => {
                    let (car, cdr) = young.pairs[index];
                    work.push(Pending::Value(car, raw));
                    work.push(Pending::Value(cdr, raw));
                }
                ValueRef::Vector(_) | ValueRef::SeqView(_)
                    if first_visit(&mut seen_vectors, index) =>
                {
                    work.extend(young.vectors[index].iter().map(|&v| Pending::Value(v, raw)));
                }
                ValueRef::Map(_)
                | ValueRef::Set(_)
                | ValueRef::Failure(_)
                | ValueRef::Record { .. }
                    if first_visit(&mut seen_maps, index) =>
                {
                    let node = &young.maps[index];
                    for &(key, entry) in &node.data {
                        work.push(Pending::Value(key, raw));
                        work.push(Pending::Value(entry, raw));
                    }
                    work.extend(
                        node.children
                            .iter()
                            .map(|&child| Pending::Value(Value::map(child), raw)),
                    );
                }
                ValueRef::Fn(_) | ValueRef::Macro(_) if first_visit(&mut seen_closures, index) => {
                    let closure = &young.closures[index];
                    for arm in closure.arms.iter() {
                        work.extend(arm.body.iter().map(|&form| Pending::Value(form, raw)));
                        work.extend(
                            arm.optionals
                                .iter()
                                .map(|&(_, default)| Pending::Value(default, raw)),
                        );
                    }
                    if let Some(env) = closure.env {
                        work.push(Pending::Env(env, raw));
                    }
                }
                _ => {}
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
    /// already a tracked root, so the value is relocated by every collection like any
    /// other; writing it is a plain `Vec` store.
    pub fn set_root_at(&mut self, i: usize, v: Value) {
        self.roots[i] = v;
    }
}
