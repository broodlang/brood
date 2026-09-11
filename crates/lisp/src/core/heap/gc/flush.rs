//! The heap flush (arena flip / Phase 2): a standalone deep copy of the live LOCAL graph
//! into fresh slabs. Free functions so the recursion borrows `&old` immutably and `&mut
//! new` mutably without tangling with the `Heap`'s `&mut self`; cycles are handled with a
//! per-slab forwarding table.

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
        // Delivered-message slots (L1): a message copied straight into this heap by a
        // sender can sit queued through any number of collections before a selective
        // `receive` reaches it, so the slots relocate in place like the operand stack.
        // Empty for any process that has never been sent a Local message.
        for v in self.msg_roots.iter_mut().flat_map(|t| t.slots.iter_mut()) {
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
        // Source positions are loader state: absent for a worker process, in which case
        // there is nothing to remap and we skip the walk entirely (see `ColdHeap`).
        if self.cold.is_some() {
            let old_form_pos = std::mem::take(&mut self.cold_mut().form_pos);
            for (key, pos) in old_form_pos {
                if let Some(new_idx) = fwd.pairs.lookup(key as u32) {
                    self.cold_mut().form_pos.insert(new_idx as u64, pos);
                }
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
}

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
pub(super) struct FwdTable {
    to: Vec<u32>,
}

impl FwdTable {
    /// Sentinel for an entry that has not been copied yet. A real destination index can
    /// never reach `u32::MAX` — the slab would have to hold 4 G objects first.
    const NONE: u32 = u32::MAX;

    /// A table covering `len` source slots, all unset.
    pub(super) fn with_len(len: usize) -> Self {
        Self {
            to: vec![Self::NONE; len],
        }
    }

    /// The destination index `src` was copied to, or `None` if it has not been copied.
    #[inline]
    pub(super) fn lookup(&self, src: u32) -> Option<u32> {
        match self.to.get(src as usize) {
            Some(&d) if d != Self::NONE => Some(d),
            _ => None,
        }
    }

    /// Record that source index `src` now lives at destination index `dst`.
    #[inline]
    pub(super) fn set(&mut self, src: u32, dst: u32) {
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
pub(super) struct FlushForward {
    /// The generation epoch to stamp into every survivor handle minted into the
    /// destination slabs. Carried here rather than threaded through every
    /// `flush_*` signature.
    pub(super) epoch: u32,
    /// Which generation the *source* objects being copied live in: `false` =
    /// nursery (a minor or legacy whole-heap flush), `true` = old (a major
    /// compaction). A `flush_*` copies a LOCAL handle only when its age matches;
    /// the other generation (and PRELUDE/RUNTIME) is left untouched.
    pub(super) src_old: bool,
    /// Whether minted destination handles are tagged **old** (`local_old_gen`).
    /// `true` for the generational paths (minor promotes nursery→old, major
    /// compacts old→old); `false` only for the legacy single-space `flush()` test
    /// helper, which stays nursery→nursery.
    pub(super) dest_old: bool,
    pub(super) pairs: FwdTable,
    pub(super) vectors: FwdTable,
    pub(super) maps: FwdTable,
    pub(super) strings: FwdTable,
    pub(super) bigints: FwdTable,
    pub(super) decimals: FwdTable,
    pub(super) ratios: FwdTable,
    pub(super) bytes: FwdTable,
    pub(super) ropes: FwdTable,
    pub(super) closures: FwdTable,
    pub(super) envs: FwdTable,
}

impl FlushForward {
    /// A forwarding set sized for `src` — the generation this collection copies *out of*.
    ///
    /// Every forwarding key is an index into one of `src`'s slabs, so each table is
    /// allocated at exactly that slab's length and indexed directly. Sizing up front costs
    /// one `memset` per non-empty slab and removes hashing from the copy path entirely.
    pub(super) fn for_source(src: &Slabs) -> Self {
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
            ratios: FwdTable::with_len(src.ratios.len()),
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
    pub(super) fn copies(&self, region: u8, is_old: bool) -> bool {
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
            pub(super) fn $name(&self, idx: usize) -> $id {
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
mint_fn!(mint_ratio, RatioId);
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
pub(super) fn flush_oob(
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

pub(super) fn flush_value(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, v: Value) -> Value {
    // Deep-car-nesting guard — see `WALKER_RED_ZONE`. The GC copies live values
    // at every collection, so a deep value must survive the walk regardless of
    // how much native stack the collecting thread has left.
    stacker::maybe_grow(WALKER_RED_ZONE, WALKER_STACK_CHUNK, || {
        flush_value_grown(old, new, fwd, v)
    })
}

pub(super) fn flush_value_grown(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    v: Value,
) -> Value {
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
        // Map, set and failure share one CHAMP store — forward the trie once and
        // re-wrap in whichever kind came in (`champ_rewrap`). One arm replaces the
        // pair of near-identical Map/Set arms this used to carry.
        ValueRef::Map(id) | ValueRef::Set(id) | ValueRef::Failure(id)
            if fwd.copies(id.region(), id.is_old()) =>
        {
            crate::core::value::champ_rewrap(v, flush_map(old, new, fwd, id))
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
        ValueRef::Ratio(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::ratio(flush_ratio(old, new, fwd, id))
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

pub(super) fn flush_pair(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: PairId,
) -> PairId {
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

pub(super) fn flush_vector(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: VecId,
) -> VecId {
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

pub(super) fn flush_string(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: StrId,
) -> StrId {
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
    // `Clone` keeps the cached char length with the entry — an Inline clones its
    // `String`, a Shared just bumps the blob's `Arc` (no byte copy), as before.
    let entry = old.strings[flush_bound!(old.strings, id, fwd, "string")].clone();
    let new_idx = new.strings.len();
    new.strings.push(entry);
    fwd.strings.set(key, new_idx as u32);
    fwd.mint_string(new_idx)
}

pub(super) fn flush_bigint(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: BigIntId,
) -> BigIntId {
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
pub(super) fn flush_decimal(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: DecimalId,
) -> DecimalId {
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

/// Flush a LOCAL ratio (mirrors [`flush_decimal`]). A leaf — clone the value into
/// the new slab (the old slab drops right after `flush`).
pub(super) fn flush_ratio(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: RatioId,
) -> RatioId {
    let key = id.index() as u32;
    if let Some(new_idx) = fwd.ratios.lookup(key) {
        return fwd.mint_ratio(new_idx as usize);
    }
    let n = old.ratios[flush_bound!(old.ratios, id, fwd, "ratio")].clone();
    let new_idx = new.ratios.len();
    new.ratios.push(n);
    fwd.ratios.set(key, new_idx as u32);
    fwd.mint_ratio(new_idx)
}

/// Flush a LOCAL bytes value (mirrors [`flush_bigint`]). A byte-clean leaf —
/// clone the `Arc<SharedBlob>` (a refcount bump, not a byte copy) into the new slab.
pub(super) fn flush_bytes(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: BytesId,
) -> BytesId {
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

pub(super) fn flush_rope(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: RopeId,
) -> RopeId {
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

pub(super) fn flush_map(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: MapId) -> MapId {
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

pub(super) fn flush_closure(
    old: &Slabs,
    new: &mut Slabs,
    fwd: &mut FlushForward,
    id: ClosureId,
) -> ClosureId {
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

pub(super) fn flush_env(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, env: EnvId) -> EnvId {
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
pub(super) fn flush_nursery_old_refs(
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
