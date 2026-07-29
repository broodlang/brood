//! VM body cache + inline caches (child of heap).
use super::*;

/// One global-read inline-cache entry (ADR-096): the value a compiled
/// `Node::GlobalIc` read from the shared global table, stamped with the epoch.
/// Same validation discipline as [`CallIcEntry`] (sym + epoch), same
/// immovability invariant on `value`.
pub struct GlobalIcEntry {
    pub sym: Symbol,
    pub epoch: u64,
    pub value: Value,
}

/// One ability-dispatch inline-cache entry (ADR-172 §7). Held in [`Heap::dispatch_ics`],
/// keyed by an op's `[ability op]` symbol pair packed into a `u64`; memoises the impl
/// `fn` resolved for the LAST-seen dispatch `id`, validated by (`id`, `epoch`). `callee`
/// is immovable — it lives in `*impls*`, which `register-impl` promotes to RUNTIME by
/// `def` — and a reload (`def *impls*`) or a RUNTIME compaction bumps `global_epoch`, so
/// a stale entry misses and re-resolves. Semantically a pure memo of `impl-for`, invisible
/// to the language.
pub struct DispatchIcEntry {
    pub id: Symbol,
    pub epoch: u64,
    pub callee: Value,
}

/// One call-site inline-cache entry — see the [`Heap::vm_call_ics`] field doc.
/// `callee` is always an immovable (PRELUDE/RUNTIME/atom) value: global bindings
/// are promoted by `env_define`, so the LOCAL collector never relocates it; the
/// runtime compactor would, but it bumps the epoch this entry is validated
/// against, so a stale handle is never read.
pub struct CallIcEntry {
    pub sym: Symbol,
    pub argc: u32,
    pub epoch: u64,
    pub callee: Value,
    /// The VM fast path: the callee's compiled arm for `argc` + its captured env,
    /// when the callee resolved to a non-passthrough VM-eligible closure.
    pub arm: Option<(Arc<crate::eval::compile::CompiledArm>, EnvId)>,
    /// The callee arm's IC block in **this process** (resolved once at install via
    /// [`Heap::vm_arm_block`]), so entering the callee on an IC hit sets the cursors
    /// without a registry lookup on the hot path. Meaningless when `arm` is `None`.
    pub callee_bases: (u32, u32),
    /// Cached JIT fast-link result `(code_ptr_as_usize, nslots, env)` — the validated
    /// output of [`Heap::vm_call_ic_fast_link`], memoised so the hot recursive call
    /// skips the per-call atomic loads (`jit_code`/`compile_epoch`) + arm-shape checks.
    /// Populated lazily once the arm is installed + fast-linkable, and only valid at this
    /// entry's `epoch` (a `def` bumps the epoch → the entry is re-resolved fresh via
    /// [`Heap::vm_call_ic_put`], which clears this). Within an epoch an installed arm's
    /// code pointer is stable (a recompile bumps the epoch), so the cache can't go stale.
    pub fast: std::cell::Cell<Option<(usize, usize, EnvId)>>,
}

/// IR-readable mirror of one call site's fast-link, indexed by site id in
/// [`Heap::vm_fast_links`]. `#[repr(C)]` with no niche so JIT'd code can load each
/// field at a fixed offset (epoch at +0, code at +8, …). A `code` of 0 marks an
/// empty/invalidated slot; a slot is only honoured when `epoch == global_epoch()`,
/// which the IR checks before reading the rest. `code` is the native entry pointer as
/// a `u64` (not a `*const u8`) — like [`CallIcEntry::fast`], so the table stays
/// `Send`/`Sync` for a process that migrates worker threads; the IR loads it as a
/// pointer. `env` is an [`EnvId`]'s raw word.
#[repr(C)]
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "jit"), allow(dead_code))]
pub struct FastLink {
    pub epoch: u64,
    pub code: u64,
    pub env: u64,
    pub nslots: u32,
    /// The IC entry's callee symbol and arity this slot was resolved for. The IR's
    /// fast path checks them against the call site's *baked* `head`/`argc` (alongside
    /// the epoch guard) before honouring the slot — without this, a call-site id reused
    /// across a [`Self::runtime_collect`] table clear (ADR-096) lets one arm read a
    /// fast-link another arm populated for a *different* callee, then jump into the wrong
    /// native code with the wrong arity (a SIGSEGV in release). The IC probe paths already
    /// validate sym+argc+epoch; mirroring them here closes the same hole on the raw-load
    /// fast path. `u32::MAX` head/`0` argc in an [`Self::EMPTY`] slot match nothing real.
    pub sym: u32,
    pub argc: u32,
    pub _pad: u32,
}

impl FastLink {
    /// An empty slot: `code == 0` and an epoch (`u64::MAX`) no real `global_epoch`
    /// reaches, so the IR's `epoch == global_epoch()` guard misses it either way.
    const EMPTY: FastLink = FastLink {
        epoch: u64::MAX,
        code: 0,
        env: 0,
        nslots: 0,
        sym: u32::MAX,
        argc: 0,
        _pad: 0,
    };
}

/// A key into the compiling-VM body cache ([`Heap::vm_cache_arm`]). Two stable
/// handle spaces are namespaced apart (ADR-076 §2c): a top-level closure is keyed
/// by its own RUNTIME [`ClosureId`] handle; a local-capturing closure is keyed by
/// the immovable **body-code handle** its (recycled LOCAL) `ClosureId` points at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VmCacheKey {
    /// A top-level / promoted RUNTIME closure, keyed by its closure-handle `.0`.
    Runtime(u64),
    /// A local-capturing closure, keyed by the `.0` of its first body form's
    /// (RUNTIME-stable) handle — the closure handle itself is unstable across GC.
    LocalBody(u64),
}

impl std::hash::Hash for VmCacheKey {
    /// Hash to a single `u64` (variant bit folded into the handle), so the fast
    /// [`SymbolHasher::write_u64`] path runs instead of the derived multi-write.
    /// The two handle spaces are already disjoint by construction, but the variant
    /// fold keeps the key total even if that ever changed.
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        let (tag, v) = match *self {
            VmCacheKey::Runtime(x) => (0u64, x),
            VmCacheKey::LocalBody(x) => (1u64, x),
        };
        h.write_u64(v.rotate_left(1) ^ tag);
    }
}

impl Heap {
    // ===== Compiling-VM body cache and inline caches (ADR-076/096) =============

    /// **RUNTIME collector — Stage 4.** Clear this process's [`Self::vm_cache`] once
    /// if a generation was freed since it was last synced. A freed slot is reused by
    /// aging with bit-identical `(gen, index)` handles, and `vm_cache` keys on those
    /// bits (not `version`), so a stale compiled body could otherwise be served for a
    /// *new* closure. The version-stamped caches self-heal on the `version` bump a
    /// free also does; only `vm_cache` needs this one-shot clear. Called on the
    /// `vm_cache` read path — one relaxed load + compare in the common (no-free) case.
    #[inline]
    fn sync_free_epoch(&self) {
        let cur = self.runtime.free_epoch.load(Ordering::Relaxed);
        if self.seen_free_epoch.get() != cur {
            self.vm_cache.borrow_mut().clear();
            self.seen_free_epoch.set(cur);
        }
    }

    /// Look up the compiled body for closure key `k` (see [`VmCacheKey`]) and resolve
    /// straight to the `argc` arm under the cache borrow, cloning **only** that
    /// `Arc<CompiledArm>` — never the whole
    /// `CompiledClosure`. The compiling VM's per-call hot path (`compiled_arm_for`)
    /// uses this so each closure call pays one arm clone instead of a transient
    /// `CompiledClosure` clone + an arm clone. Outer `None` = key absent (a cache
    /// miss to compile); `Some(None)` = present but no VM arm for `argc` (defer to
    /// the tree-walker); `Some(Some(arm))` = the compiled arm.
    pub fn vm_cache_arm(
        &self,
        k: VmCacheKey,
        argc: usize,
    ) -> Option<Option<Arc<crate::eval::compile::CompiledArm>>> {
        self.sync_free_epoch();
        self.vm_cache
            .borrow()
            .get(&k)
            .map(|cc| cc.as_ref().and_then(|cc| cc.arm_for(argc).cloned()))
    }

    /// ADR-175 Phase B off-switch: `BROOD_NO_SHARED_ARMS=1` makes every process
    /// compile privately, exactly as before the shared cache. The A/B / bisect lever.
    pub fn shared_arms_disabled() -> bool {
        static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *OFF.get_or_init(|| std::env::var_os("BROOD_NO_SHARED_ARMS").is_some())
    }

    /// Look up the runtime-shared compiled closure for PRELUDE closure-handle `bits`
    /// (ADR-175 Phase B). One read-lock on first touch per process per closure; after
    /// that the process's own `vm_cache` serves every call.
    pub fn shared_closure_lookup(
        &self,
        bits: u64,
    ) -> Option<Arc<crate::eval::compile::CompiledClosure>> {
        self.runtime
            .shared_closures
            .read()
            .ok()?
            .get(&bits)
            .cloned()
    }

    /// Publish a compiled closure to the runtime-shared cache. Idempotent — all
    /// publishers compiled the identical closure from the same shared AST.
    pub fn shared_closure_publish(
        &self,
        bits: u64,
        cc: Arc<crate::eval::compile::CompiledClosure>,
    ) {
        if let Ok(mut m) = self.runtime.shared_closures.write() {
            m.insert(bits, cc);
        }
    }

    /// Record the compile result for closure key `k` (eligible body or `None`).
    pub fn vm_cache_put(
        &self,
        k: VmCacheKey,
        v: Option<Arc<crate::eval::compile::CompiledClosure>>,
    ) {
        self.vm_cache.borrow_mut().insert(k, v);
    }

    /// Resolve **this process's IC block** for `arm` (ADR-175 Phase A): the pair of
    /// contiguous base offsets — `(call-site base, global-site base)` — under which the
    /// arm's arm-relative site ids index this process's IC tables. Lazily allocated on
    /// first activation: the tables grow by `nsites`/`ngsites` and the block is
    /// remembered under the arm's process-independent [`CompiledArm::uid`], so a shared
    /// arm gets one block per process that actually runs it (and a process that never
    /// runs it pays nothing). The compiling process is not special — it resolves a
    /// block exactly like an installer would.
    pub fn vm_arm_block(&self, arm: &crate::eval::compile::CompiledArm) -> (u32, u32) {
        if arm.nsites == 0 && arm.ngsites == 0 {
            return (0, 0); // site-free arm: any base works, never indexed
        }
        if let Some(&b) = self.arm_ic_blocks.borrow().get(&arm.uid) {
            return b;
        }
        let base = {
            let mut t = self.vm_call_ics.borrow_mut();
            let base = t.len();
            let new_len = base + arm.nsites as usize;
            t.resize_with(new_len, || None);
            // Grow the IR-readable mirror in lockstep, so `vm_fast_links[base + site]`
            // is always in range for any site `vm_call_ics` knows about.
            self.vm_fast_links
                .borrow_mut()
                .resize(new_len, FastLink::EMPTY);
            base as u32
        };
        #[cfg(debug_assertions)]
        {
            // Copy the arm's compile-time site positions into the absolute debug table.
            let mut dbg = self.dbg_site_pos.borrow_mut();
            dbg.resize(base as usize + arm.nsites as usize, None);
            for (i, p) in arm.site_pos.iter().enumerate() {
                if i < arm.nsites as usize {
                    dbg[base as usize + i] = p.clone();
                }
            }
        }
        let gbase = {
            let mut t = self.vm_global_ics.borrow_mut();
            let gbase = t.len();
            let new_len = gbase + arm.ngsites as usize;
            t.resize_with(new_len, || None);
            gbase as u32
        };
        self.arm_ic_blocks
            .borrow_mut()
            .insert(arm.uid, (base, gbase));
        (base, gbase)
    }

    /// The current activation's IC block bases — set by the drivers at every arm
    /// transition (call/tail/return/resume/native entry), read by every site-indexed
    /// method below. Returns the previous pair, for save/restore around a transition.
    #[inline]
    pub fn set_ic_bases(&self, bases: (u32, u32)) -> (u32, u32) {
        let old = (self.cur_ic_base.get(), self.cur_gic_base.get());
        self.cur_ic_base.set(bases.0);
        self.cur_gic_base.set(bases.1);
        old
    }

    /// The current activation's IC block bases (see [`Self::set_ic_bases`]).
    #[inline]
    pub fn ic_bases(&self) -> (u32, u32) {
        (self.cur_ic_base.get(), self.cur_gic_base.get())
    }

    /// DEBUG ONLY: look up a call site's recorded source position as `file:line:col`.
    /// `site` is arm-relative (the current activation's block is added, like every
    /// site-indexed path).
    #[cfg(debug_assertions)]
    pub fn dbg_site_loc(&self, site: u32) -> String {
        match self
            .dbg_site_pos
            .borrow()
            .get((self.cur_ic_base.get() + site) as usize)
            .and_then(|o| o.clone())
        {
            Some((p, file)) => format!("{}:{}:{}", file.as_deref().unwrap_or("?"), p.line, p.col),
            None => format!("<site {site}: no recorded pos>"),
        }
    }

    /// Probe global-read site `site`: a hit requires the same symbol and the
    /// current epoch (see [`Self::vm_call_ic_probe`] for the validation story).
    #[inline]
    pub fn vm_global_ic_probe(&self, site: u32, sym: Symbol, epoch: u64) -> Option<Value> {
        let t = self.vm_global_ics.borrow();
        let e = t.get((self.cur_gic_base.get() + site) as usize)?.as_ref()?;
        if e.sym == sym && e.epoch == epoch {
            Some(e.value)
        } else {
            None
        }
    }

    /// Install global-read site `site`'s entry. Same skip rules as
    /// [`Self::vm_call_ic_put`]: out-of-range sites and movable (builder-heap)
    /// values are ignored.
    pub fn vm_global_ic_put(&self, site: u32, sym: Symbol, epoch: u64, value: Value) {
        if is_movable(value) {
            return;
        }
        let mut t = self.vm_global_ics.borrow_mut();
        if let Some(slot) = t.get_mut((self.cur_gic_base.get() + site) as usize) {
            *slot = Some(GlobalIcEntry { sym, epoch, value });
        }
    }

    /// Ability dispatch with a per-op inline cache (ADR-172 §7) — the `%dispatch`
    /// primitive's engine. Given the current `*impls*` map (passed by the op, so the
    /// kernel stays decoupled from the global's name), the op's `[ability op]` key
    /// vector, and the first argument's dispatch `id` keyword, return the impl `fn` (or
    /// `nil` when none, so the op raises `no-impl`). A HIT — same op-key, same id, current
    /// `global_epoch` — returns the cached fn without touching `*impls*`; a MISS resolves
    /// `impls[op-key][id]` (falling back to `:default`), caches it, and returns it. The
    /// result is always identical to the pure `impl-for`: `*impls*` is immutable within an
    /// epoch, and any `def *impls*` / compaction bumps the epoch so the entry misses.
    pub fn vm_dispatch(&self, impls: Value, op_key: Value, id: Value) -> Value {
        // op-key is the constant 2-symbol vector `[ability op]`; id is a keyword. If either
        // isn't the shape `defability` emits, skip the cache and resolve purely.
        let (key, id_sym) = match (self.dispatch_key(op_key), id) {
            (Some(k), Value::Keyword(s)) => (k, s),
            _ => return self.dispatch_resolve(impls, op_key, id),
        };
        let epoch = self.global_epoch();
        if let Some(e) = self.dispatch_ics.borrow().get(&key) {
            if e.epoch == epoch && e.id == id_sym {
                return e.callee;
            }
        }
        let callee = self.dispatch_resolve(impls, op_key, id);
        // Cache only an immovable fn (mirrors `vm_global_ic_put`): a resolved impl lives in
        // the RUNTIME-promoted `*impls*`, so this holds; the guard is belt-and-braces.
        if !is_movable(callee) {
            self.dispatch_ics.borrow_mut().insert(
                key,
                DispatchIcEntry {
                    id: id_sym,
                    epoch,
                    callee,
                },
            );
        }
        callee
    }

    /// Pack an op-key `[ability op]` vector's two interned symbols into a `u64` cache key,
    /// or `None` if it isn't a 2-symbol vector.
    fn dispatch_key(&self, op_key: Value) -> Option<u64> {
        let v = match op_key {
            Value::Vector(id) => self.vector(id),
            _ => return None,
        };
        match (v.first(), v.get(1)) {
            (Some(&Value::Sym(a)), Some(&Value::Sym(b))) if v.len() == 2 => {
                Some(((a as u64) << 32) | (b as u64))
            }
            _ => None,
        }
    }

    /// The pure resolution `impl-for` does: `impls[op-key][id]`, else `[:default]`, else nil.
    fn dispatch_resolve(&self, impls: Value, op_key: Value, id: Value) -> Value {
        let impls_id = match impls {
            Value::Map(m) => m,
            _ => return Value::nil(),
        };
        let methods_id = match self.map_get(impls_id, op_key) {
            Some(Value::Map(m)) => m,
            _ => return Value::nil(),
        };
        if let Some(f) = self.map_get(methods_id, id) {
            return f;
        }
        self.map_get(
            methods_id,
            Value::Keyword(crate::core::value::intern("default")),
        )
        .unwrap_or_else(Value::nil)
    }

    /// Probe call-site `site`'s inline cache: a hit requires the same callee
    /// symbol, the same argument count, **and** the current global epoch (so any
    /// `def`/compaction since the entry was installed misses). Returns the cached
    /// callee value plus the VM fast-path payload. Sym + argc are validated (not
    /// just the epoch) so a site id recycled by [`Self::runtime_collect`]'s table
    /// clear can never serve a *different* call site a wrong resolution.
    pub fn vm_call_ic_probe(
        &self,
        site: u32,
        sym: Symbol,
        argc: u32,
        epoch: u64,
    ) -> Option<(
        Value,
        Option<(Arc<crate::eval::compile::CompiledArm>, EnvId, (u32, u32))>,
    )> {
        let t = self.vm_call_ics.borrow();
        let e = t.get((self.cur_ic_base.get() + site) as usize)?.as_ref()?;
        if e.sym == sym && e.argc == argc && e.epoch == epoch {
            Some((
                e.callee,
                e.arm
                    .as_ref()
                    .map(|(a, env)| (a.clone(), *env, e.callee_bases)),
            ))
        } else {
            None
        }
    }

    /// Fast-link probe for the JIT's native-to-native call path — like
    /// [`Self::vm_call_ic_probe`] but does the *entire* fast-link validation (sym/argc/
    /// epoch match, native code installed + epoch-current, simple arm: `nslots > 0`, no
    /// optional, no rest) inside the borrow and returns only **Copy** data: the native
    /// entry pointer, the frame `nslots`, and the captured env. **No `Arc` clone** — the
    /// one real atomic-RMW the hot recursive call (`fib` &c.) otherwise pays per call
    /// (~30M times). Returns `None` when not fast-linkable; the caller falls back to the
    /// cloning [`Self::vm_call_ic_probe`] / slow path (which also covers deopt). Mirrors
    /// `jit_dispatch_call`'s native-link guard — the two must stay in sync.
    #[cfg(feature = "jit")]
    pub fn vm_call_ic_fast_link(
        &self,
        site: u32,
        sym: Symbol,
        argc: u32,
        epoch: u64,
    ) -> Option<(*const u8, usize, EnvId)> {
        use std::sync::atomic::Ordering::Acquire;
        let t = self.vm_call_ics.borrow();
        let e = t.get((self.cur_ic_base.get() + site) as usize)?.as_ref()?;
        if e.sym != sym || e.argc != argc || e.epoch != epoch {
            return None;
        }
        // Memoised hot path: the entry matched at this epoch and the arm was already
        // validated as fast-linkable — return the cached `(code, nslots, env)` directly,
        // skipping the two atomic `Acquire` loads + the arm-shape checks below. Valid
        // because within an epoch an installed arm's code pointer is stable (a `def`
        // bumps the epoch → a fresh entry; a recompile bumps it too).
        if let Some((code, nslots, env)) = e.fast.get() {
            return Some((code as *const u8, nslots, env));
        }
        let (arm, env) = e.arm.as_ref()?;
        let code = arm.jit_code.load(Acquire);
        if code.is_null() || code == crate::jit::BAILED || code == crate::jit::QUEUED {
            return None;
        }
        if arm.nslots == 0 || arm.noptional != 0 || arm.rest_slot.is_some() {
            return None;
        }
        // A closure WITH captures can't fast-link: the fast-link / fast-frame frame setup
        // (`jit_run_fast_link`, `brood_rt_fast_frame`) places only params and nil-fills the
        // rest — it skips `push_frame`, which is where capture slots are filled from the
        // captured env. Linking one would leave its captured lexicals nil (read as nil in
        // the body). A capturing closure `def`'d globally and called via a free-global head
        // would otherwise hit this. Fall back to the native-link block / slow path, which
        // fill captures. (Top-level `defn`s have no captures, so the hot recursive case is
        // unaffected.)
        if !arm.capture_names.is_empty() {
            return None;
        }
        if arm.compile_epoch.load(Acquire) != epoch {
            return None;
        }
        // Two-stage tiering: the frame the *installed* native runs against — the inlined
        // upgrade (post-swap) needs the larger `inline_nslots`. The small→inlined swap
        // bumps the epoch, so a stale memo here (with the small `nslots`) is invalidated
        // and re-validated through the IC miss path, picking up the new active size.
        let active_ns = arm.active_nslots();
        // Fully validated + installed at this epoch — memoise for subsequent calls.
        e.fast.set(Some((code as usize, active_ns, *env)));
        // Mirror into the IR-readable flat table so the next call reaches the native code
        // straight from JIT'd code (epoch-guarded raw load) without re-entering this probe.
        // Same data, written in lockstep — [`brood_rt_fast_frame`] debug-asserts they agree.
        if let Some(slot) = self
            .vm_fast_links
            .borrow_mut()
            .get_mut((self.cur_ic_base.get() + site) as usize)
        {
            *slot = FastLink {
                epoch,
                code: code as u64,
                env: env.0,
                nslots: active_ns as u32,
                // `sym`/`argc` matched `e.sym`/`e.argc` at the top of this fn (the early
                // `return None`), so they identify exactly the callee this slot links to —
                // the IR re-checks them against its baked head/argc so a reused site id
                // (ADR-096) can never read another arm's link. See [`FastLink`].
                sym,
                argc,
                _pad: 0,
            };
        }
        Some((code as *const u8, active_ns, *env))
    }

    /// Publish a NATIVE (builtin) callee into the IR-readable [`FastLink`] mirror:
    /// `code` = the `NativeFnPtr` bits, `nslots == u32::MAX` is the native marker the
    /// IR branches on (a Brood link's `nslots` is a real frame size, never MAX). The
    /// caller pre-validates arity for exactly this `argc`, so the IR-side trampoline
    /// needs no arity check; the epoch/sym/argc guards invalidate it exactly like a
    /// Brood link (a `def` bumps the epoch → miss → re-resolve).
    #[cfg(feature = "jit")]
    pub(crate) fn vm_fast_link_publish_native(
        &self,
        site: u32,
        sym: Symbol,
        argc: u32,
        epoch: u64,
        func: u64,
    ) {
        if let Some(slot) = self
            .vm_fast_links
            .borrow_mut()
            .get_mut((self.cur_ic_base.get() + site) as usize)
        {
            *slot = FastLink {
                epoch,
                code: func,
                env: 0,
                nslots: u32::MAX,
                sym,
                argc,
                _pad: 0,
            };
        }
    }

    /// Base pointer + length of the IR-readable [`FastLink`] mirror, for the JIT to read a
    /// call site's fast-link with a raw load (no `RefCell` borrow on the hot path). Uses
    /// [`RefCell::as_ptr`] so it never takes a borrow; valid until the table next grows
    /// (`vm_site_alloc`, only during compilation — never mid-arm-call, like `roots_base`).
    /// The JIT re-fetches it after each Brood→Brood call, exactly as it does the roots base.
    #[cfg(feature = "jit")]
    #[inline]
    pub fn vm_fast_links_base(&self) -> (*const FastLink, usize) {
        // SAFETY: single-threaded per process; nothing mutates the table between this read
        // and the IR's use of the pointer (a `def`/compaction that would clear it can't run
        // concurrently with this process executing an arm).
        let v = unsafe { &*self.vm_fast_links.as_ptr() };
        // Block-adjusted for the current activation (ADR-175 Phase A): the IR indexes
        // with the arm-relative site id, so hand it the current arm's block as if it
        // were the whole table. `min` guards a stale cursor after a `runtime_collect`
        // table clear (the IR then sees len 0 → every site misses → slow path, exactly
        // the pre-existing degradation semantics for a live arm across a clear).
        let base = (self.cur_ic_base.get() as usize).min(v.len());
        (unsafe { v.as_ptr().add(base) }, v.len() - base)
    }

    /// Install (or overwrite) call-site `site`'s inline cache entry. An
    /// out-of-range site (a live arm compiled before the last
    /// [`Self::runtime_collect`] table clear) is ignored. Refuses to cache a
    /// movable (LOCAL) callee or captured env — the entry lives outside the GC
    /// root graph, so it must hold only immovable handles. A *process* heap's
    /// globals are promoted on `def`, so that never skips; the prelude *builder*
    /// heap (whose "global" env is a plain LOCAL frame of unpromoted values)
    /// simply never caches, which is correct — its handles are re-tagged
    /// wholesale by `freeze_as_shared_code`.
    pub fn vm_call_ic_put(&self, site: u32, mut entry: CallIcEntry) {
        if is_movable(entry.callee) {
            return;
        }
        if let Some((_, env)) = &entry.arm {
            if *env != EnvId::GLOBAL && env.region() == LOCAL {
                return;
            }
        }
        // Resolve the callee arm's IC block BEFORE borrowing the table below —
        // `vm_arm_block` may grow `vm_call_ics` (a nested mutable borrow otherwise).
        entry.callee_bases = match &entry.arm {
            Some((arm, _)) => self.vm_arm_block(arm),
            None => (0, 0),
        };
        let abs = (self.cur_ic_base.get() + site) as usize;
        let mut t = self.vm_call_ics.borrow_mut();
        if let Some(slot) = t.get_mut(abs) {
            *slot = Some(entry);
            // The replacing entry's `fast` memo starts empty; clear the IR-readable mirror
            // too so it never leads the IC. It is re-populated the next time
            // [`Self::vm_call_ic_fast_link`] validates the (now installed) arm.
            if let Some(fl) = self.vm_fast_links.borrow_mut().get_mut(abs) {
                *fl = FastLink::EMPTY;
            }
        }
    }

    /// Invalidate **this process's** cached call-site fast-links to callee `sym` — both the
    /// [`CallIcEntry::fast`] memo and its IR-readable [`FastLink`] mirror — so the next call
    /// re-probes [`Self::vm_call_ic_fast_link`] and picks up freshly-installed native code
    /// (and its frame size). Cheap: one pass over the (small) per-process call-IC table,
    /// touching only entries for `sym`.
    ///
    /// Used by the two-stage-tiering inline-upgrade swap ([`crate::eval::compile::jit_tier`])
    /// **instead of bumping the shared `global_epoch`**. The swap is local to this process's
    /// own [`CompiledArm`] (arms are per-process — [`Self::vm_cache_arm`]), so a global bump
    /// would needlessly invalidate every *other* process's arms: their `compile_epoch` would
    /// go stale, they'd nuke their installed code, re-tier, re-enqueue their own inline
    /// upgrade, re-swap and re-bump — a cross-process cascade that under `pfib` diverted
    /// nearly every call from the in-IR fast-link to the slow IC-dispatch path (~2× the
    /// instructions). Scoping the invalidation to this process's links to this callee keeps
    /// the upgrade's effect where it belongs and leaves peers' fast-links intact.
    #[cfg(feature = "jit")]
    pub(crate) fn invalidate_fast_links_for(&self, sym: Symbol) {
        let ics = self.vm_call_ics.borrow();
        let mut fls = self.vm_fast_links.borrow_mut();
        for (i, entry) in ics.iter().enumerate() {
            if let Some(e) = entry {
                if e.sym == sym {
                    e.fast.set(None);
                    if let Some(fl) = fls.get_mut(i) {
                        *fl = FastLink::EMPTY;
                    }
                }
            }
        }
    }

    /// Push a live compiled arm onto the execution-stack registry; returns its depth
    /// (the index to [`live_arm_truncate`](Self::live_arm_truncate) back to on return).
    /// See [`live_vm_arms`](Self::live_vm_arms).
    pub fn live_arm_push(&mut self, arm: Arc<crate::eval::compile::CompiledArm>) -> usize {
        let i = self.live_vm_arms.len();
        self.live_vm_arms.push(arm);
        i
    }

    /// Replace the arm at depth `i` — a tail call into a different arm reuses the slot
    /// rather than growing the stack (the trampoline never recurses).
    pub fn live_arm_set(&mut self, i: usize, arm: Arc<crate::eval::compile::CompiledArm>) {
        self.live_vm_arms[i] = arm;
    }

    /// Pop the registry back to depth `n` (teardown paired with `live_arm_push`,
    /// also self-healing on an error unwind).
    pub fn live_arm_truncate(&mut self, n: usize) {
        self.live_vm_arms.truncate(n);
    }

    /// Current depth of the live-arm registry — captured at the bytecode driver's
    /// entry so a multi-frame error unwind can truncate every frame's registration
    /// back in one call (`vm_run_bc`).
    pub fn live_arm_len(&self) -> usize {
        self.live_vm_arms.len()
    }
}
