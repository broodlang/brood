//! The root set a collection starts from: the explicit root stack (`push_root` /
//! `root` / `read_root` / `advance_root`), the env operand stack (ADR-061), and
//! `root_scope`. A LOCAL handle held across a safepoint must live here or it is stale
//! after the next flip — see `docs/` on use-after-GC.

use super::*;

impl Heap {
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

    /// Reserve `n` fresh slots at the top of `roots` and return a pointer to the first —
    /// the JIT's per-call argument staging, written **in place** by the caller's own stores.
    ///
    /// Replaces the store-to-stack-slot-then-[`push_roots_n`] pair: the JIT used to write
    /// each argument's three words into a Cranelift stack slot and then copy the block onto
    /// `roots`, which LBR attribution put at **4.4% of `bintree`** in the copy alone, plus
    /// the stores feeding it. Handing back the destination lets the same stores land where
    /// the value is needed, so the copy stops existing rather than getting cheaper — the one
    /// shape of change that has actually moved this row (docs/compute-frontier.md §2j).
    ///
    /// # Safety
    /// The returned slots are **live roots holding uninitialised memory** until the caller
    /// stores real `Value`s into all `n` of them. Nothing may allocate, collect, or otherwise
    /// walk `roots` in that window — the caller must emit stores and nothing else. This is
    /// the same discipline the out-pointer ABIs run under ([`crate::jit::JitArmFn`],
    /// `brood_rt_cons`), and it holds here for the same reason: the JIT emits pure stores
    /// between this call and the call that consumes them. Under `debug_assertions` the slots
    /// are nil-filled first, so a *missing* store shows up as a `nil` argument — a wrong
    /// answer the tests catch — instead of as garbage with a valid-looking tag.
    #[cfg(feature = "jit")]
    pub(crate) unsafe fn push_roots_room(&mut self, n: usize) -> *mut Value {
        let old = self.roots.len();
        self.roots.reserve(n);
        let dst = self.roots.as_mut_ptr().add(old);
        #[cfg(debug_assertions)]
        std::ptr::write_bytes(dst, 0, n);
        self.roots.set_len(old + n);
        dst
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
        self.old_opt()
            .map_or(std::ptr::null(), |o| o.pairs.as_ptr() as *const u8)
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
        self.old_opt()
            .map_or(std::ptr::null(), |o| o.vectors.as_ptr() as *const u8)
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
}
