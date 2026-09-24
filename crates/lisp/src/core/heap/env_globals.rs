//! The environment chain and the global bindings — child of heap.
//!
//! Two halves of *name resolution*, in the order a lookup walks them. First the LOCAL
//! environment chain: real frames are always LOCAL, and the chain bottoms out at the
//! sentinel [`EnvId::GLOBAL`]. Then the shared global table under `runtime.globals` —
//! `def` and its rebinding generation, the per-process global-read cache and the epoch a
//! JIT'd guard loads, the registry ops (`registry_update`/`_cas`/`_member`), dynamic-variable
//! binding stacks, `sig` declarations, and the globals snapshot/restore that `%isolate`
//! rolls back. It closes with the ADR-119 Phase-2 dependency recorder, which is per-process
//! precisely because it observes globals. Split out of `heap.rs` on 2026-09-07 (handoff
//! item 1): a `use super::*` child, so it reaches `Heap`'s private fields exactly as before.

use super::*;
use crate::core::registries as reg;

impl Heap {
    // ===== Environment chain ====================================================
    //
    // Real env frames are always LOCAL. The global scope is the sentinel
    // [`EnvId::GLOBAL`], which routes to the shared `runtime.globals` table; a
    // top-level frame's parent chain bottoms out there. (During prelude *build*
    // the global is instead a real local root frame with no parent.)

    pub(super) fn env_frame(&self, env: EnvId) -> SlabRef<'_, EnvFrame> {
        // `EnvId::GLOBAL` is a sentinel (region bits `0b11`) — there is no
        // frame to return; the global scope routes through
        // `runtime.globals_read()` instead. Callers MUST short-circuit
        // GLOBAL before reaching here (every walker does — see `env_get`).
        // A clear assert when that invariant slips, rather
        // than the `_ => unreachable!()` arm catching it via the
        // undefined-region byte.
        assert!(
            env != EnvId::GLOBAL,
            "env_frame called with EnvId::GLOBAL — global scope has no frame; \
             use env_get / globals_read instead",
        );
        match env.region() {
            LOCAL if env.is_old() => {
                #[cfg(debug_assertions)]
                self.check_epoch_aged(true, env.generation(), env.index(), "env_frame", env.0);
                SlabRef::direct(&self.old().envs[env.index()])
            }
            LOCAL => {
                #[cfg(debug_assertions)]
                self.check_epoch_aged(false, env.generation(), env.index(), "env_frame", env.0);
                SlabRef::direct(&self.local.envs[env.index()])
            }
            RUNTIME => self.rt_slab_ref(env.code_gen(), |c| {
                c.envs
                    .get(env.index())
                    .expect("runtime env frame")
                    .get()
                    .expect("runtime env read before promote filled its slot")
            }),
            _ => unreachable!("env frames live only in the local or runtime region"),
        }
    }

    /// A captured frame's parent link and a borrow of its bindings — no copy.
    /// Used to *serialize* a closure's captured environment into a `Message`
    /// (cross-process / cross-node), mirroring what [`Self::promote_env`] reads
    /// to share it within a runtime. `EnvId::GLOBAL` has no frame (it routes to
    /// the shared global table), so the walk stops there — globals resolve on
    /// the receiver, never travel. The borrow is tied to `&self` (the LOCAL slab
    /// or the stable-ref RUNTIME boxcar), so callers walk a chain without cloning.
    pub fn env_frame_ref(&self, env: EnvId) -> (Option<EnvId>, SlabRef<'_, [(Symbol, Value)]>) {
        let frame = self.env_frame(env);
        let parent = frame.parent;
        (parent, frame.map(|f| f.vars.as_slice()))
    }

    /// The name `env`'s *immediate* frame binds to `val`, if any — used by the VM's
    /// self-call optimization to recognise a `letrec` self-recursive closure: its
    /// captured frame binds its own name to itself (the `MakeClosure` self-name
    /// `env_define`). Scans the frame only (not parents), newest binding first (so
    /// the self-binding, pushed last, wins). `None` when nothing in the frame is
    /// `val` — e.g. a global-capturing closure, which resolves its name via the
    /// global table rather than a captured binding.
    pub fn env_frame_self_name(&self, env: EnvId, id: ClosureId) -> Option<Symbol> {
        let (_, vars) = self.env_frame_ref(env);
        vars.iter()
            .rev()
            .find(|(_, v)| matches!(v.unpack(), ValueRef::Fn(fid) if fid == id))
            .map(|(s, _)| *s)
    }

    pub fn new_env(&mut self, parent: Option<EnvId>) -> EnvId {
        let idx = self.local.envs.len();
        self.local.envs.push(EnvFrame {
            vars: EnvVars::new(),
            parent,
        });
        EnvId::local_gen(idx, self.local_epoch)
    }

    pub fn env_get(&self, env: EnvId, sym: Symbol) -> Option<Value> {
        crate::perf_bump!(env_get);
        let mut cur = Some(env);
        while let Some(e) = cur {
            crate::perf_bump!(env_hops);
            if e == EnvId::GLOBAL {
                // A dynamic var resolves to its innermost active `binding`, if
                // any, before the shared global default. The stack is empty
                // unless a `binding` is in scope, so this costs nothing on the
                // ordinary path; when active it shadows only at the global level
                // (dynamic vars are never lexically bound).
                if !self.dynamics.is_empty() {
                    if let Some(&(_, v)) = self.dynamics.iter().rev().find(|&&(s, _)| s == sym) {
                        return Some(v);
                    }
                }
                return self.global_lookup_cached(sym);
            }
            let frame = self.env_frame(e);
            // Scan from the end: a later binding shadows an earlier same-named one.
            if let Some(&(_, v)) = frame.vars.iter().rev().find(|&&(s, _)| s == sym) {
                return Some(v);
            }
            cur = frame.parent;
        }
        None
    }

    /// Read the `k`-th captured lexical (`#3` lexical addressing). Fast path: when the
    /// captured env is a **flat frame** whose `vars[k]` is exactly `name` — the VM-built
    /// closure's snapshot, the common case — return it by direct index, no symbol scan or
    /// chain walk. Fallback: a chained / tree-walker env (or a shadowed misalignment)
    /// resolves by name through [`env_get`] (correct, the old cost). `name` makes the fast
    /// path self-verifying, so the two engines stay in lockstep (the differential gate).
    #[inline]
    pub fn capture_value(&self, env: EnvId, k: usize, name: Symbol) -> Value {
        if env != EnvId::GLOBAL {
            let frame = self.env_frame(env);
            if let Some(&(s, v)) = frame.vars.get(k) {
                // Fast path only if `vars[k]` is `name` AND is its **last** binding in
                // this frame — i.e. exactly what `env_get`'s reverse scan returns. A
                // `letrec` env binds a name twice (a nil placeholder + the wired value),
                // so a bare forward index would read the placeholder; the no-later-dup
                // check rejects that and falls through to `env_get` (correct for both).
                if s == name && !frame.vars[k + 1..].iter().any(|&(s2, _)| s2 == name) {
                    return v;
                }
            }
        }
        self.env_get(env, name).unwrap_or(Value::nil())
    }

    /// The distinct lexical names bound along `env`'s frame chain, innermost-first,
    /// stopping at the global scope (whose names are runtime globals, not lexicals).
    /// Used by the compiling VM (ADR-076 §2c): a nested `(fn …)` must snapshot the
    /// enclosing lexical environment it closes over, so the compiler asks which
    /// names that env actually binds. The set is a static property of the closure's
    /// definition site (every instance of the same source closure binds the same
    /// names), so it's safe to derive once and bake into the cached body.
    pub fn env_chain_names(&self, env: EnvId) -> Vec<Symbol> {
        let mut names: Vec<Symbol> = Vec::new();
        let mut cur = env;
        let mut depth = 0;
        while cur != EnvId::GLOBAL && (cur.region() == LOCAL || cur.region() == RUNTIME) {
            let frame = self.env_frame(cur);
            for &(s, _) in frame.vars.iter() {
                if !names.contains(&s) {
                    names.push(s);
                }
            }
            match frame.parent {
                Some(p) => cur = p,
                None => break,
            }
            depth += 1;
            if depth > 10_000 {
                break; // safety belt — env chains shouldn't be this deep
            }
        }
        names
    }

    /// Resolve a name in the shared global table, going through this process's
    /// [`global_ic`](Self::global_ic) inline cache. On a version match the cached
    /// (immovable PRELUDE/RUNTIME) handle is returned without touching the
    /// `RwLock`; otherwise the locked table is read and the entry re-stamped.
    /// Only reached after the local chain and dynamics have missed, so it never
    /// shadows a lexical or dynamic binding. An *unbound* name isn't cached (so it
    /// resolves the moment it's later `def`'d).
    #[inline]
    fn global_lookup_cached(&self, sym: Symbol) -> Option<Value> {
        let cur = self.runtime.version.load(Ordering::Relaxed);
        if let Some(&(ver, val)) = self.global_ic.borrow().get(&sym) {
            if ver == cur {
                return Some(val);
            }
        }
        // KI-135 / ADR-344: a name this process's OPEN module load has defined but not yet
        // published — checked BEFORE the table, because a load may REBIND a live name (the
        // image branch's `(def- *std-regs* …)` tables exist live as nil, and the loader must
        // read its own value, or its dependency edges read as none and nothing it needs
        // loads). Only a process with a frame open pays the probe; every other process's
        // lookup falls straight through to the table. Cached like a table hit: a staged
        // rebind and the publish both bump `version`.
        if let Some(val) = self.staged_lookup(sym) {
            self.global_ic.borrow_mut().insert(sym, (cur, val));
            return Some(val);
        }
        if let Some(val) = self.runtime.globals_read().get(&sym).copied() {
            self.global_ic.borrow_mut().insert(sym, (cur, val));
            return Some(val);
        }
        // ADR-070: an intra-package qualified reference resolves through its rooted name
        // (`commands/cmd-open` → `bedit/commands/cmd-open`). Only on the MISS path, so an
        // ordinary hit pays nothing; `root_qualified_ref` memoizes the symbol→symbol answer, and
        // the resolved value is cached under the ORIGINAL symbol, so a hot reference costs
        // the same as any other after the first lookup.
        let rooted = self.root_qualified_ref(sym)?;
        let val = self.runtime.globals_read().get(&rooted).copied();
        if let Some(val) = val {
            self.global_ic.borrow_mut().insert(sym, (cur, val));
        }
        val
    }

    // ===== Global bindings (RUNTIME) ============================================

    /// The current global-binding **epoch** — bumped on every `def`/`defmacro`
    /// (and hot-reload) via `runtime.version`. The compiling VM stamps it into a
    /// `Node::Prim2`'s inline-op guard at compile time and re-validates against it
    /// at run time, so a primitive baked inline (`+` → inline `i64` add) self-heals
    /// to the general call path the moment the operator is redefined. Mirrors the
    /// version `global_lookup_cached` already keys the symbol inline-cache on.
    pub fn global_epoch(&self) -> u64 {
        self.runtime.code_epoch.load(Ordering::Relaxed)
    }

    /// Address of the global-epoch counter (`runtime.version`), so JIT'd code can read the
    /// epoch with a **raw load** instead of a `brood_rt_global_epoch` FFI *call* on every loop
    /// back-edge / linked call (the call was ~20% of a hoisted-global loop like `loop`). The
    /// counter is an `AtomicU64` living in the `Arc<RuntimeCode>` — a stable address for the
    /// process (`runtime_collect` mutates `version` in place via `Arc::get_mut`, never replaces
    /// the `Arc`), so a JIT'd arm fetches this once at entry and loads through it each iteration.
    /// A plain `u64` load matches the `Relaxed` atomic load (a plain `mov` on the host); the
    /// guard only needs to *eventually* observe a concurrent `def`'s bump, which it does.
    /// It is a formal data race in the abstract model (a plain load vs the writers'
    /// `fetch_add(Relaxed)`), but a benign one on every supported target — and not even
    /// ThreadSanitizer-observable, since TSan instruments rustc-compiled code, not the
    /// Cranelift-JIT'd machine code that performs this load. An atomic op here would buy
    /// nothing on these targets and reinstate the FFI cost, so the plain load stays.
    #[cfg(feature = "jit")]
    pub(crate) fn global_epoch_ptr(&self) -> *const u64 {
        &self.runtime.code_epoch as *const AtomicU64 as *const u64
    }

    /// The rebinding generation of global `sym`: strictly increasing across `def`s of
    /// that name, 0 for a name never `def`'d in this runtime (a prelude binding, or an
    /// unbound one). See `RuntimeCode::global_generations`.
    pub fn global_generation(&self, sym: Symbol) -> u64 {
        self.runtime
            .global_generations
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&sym)
            .copied()
            .unwrap_or(0)
    }

    /// Is `sym` bound in the global table (prelude + user `def`s)? An authoritative,
    /// non-racy read of `runtime.globals` (which is seeded with the prelude). Used by
    /// the unbound-symbol diagnostic to tell a *spuriously*-unbound known global (the
    /// fan-out race) apart from a genuinely-undefined name (a typo) — so the
    /// scheduler-race hint only fires for the former.
    pub fn global_defined(&self, sym: Symbol) -> bool {
        self.runtime.globals_read().get(&sym).is_some()
    }

    /// Is `sym` **reserved** — a name the language itself ships (prelude, builtin, or
    /// embedded std module)? A global `def` of one is refused (ADR-166); the caller
    /// raises, because that is where a user-facing error belongs. Always false while
    /// this process is loading an embedded module, which is the one context allowed to
    /// (re)define its own surface.
    pub fn is_reserved_global(&self, sym: Symbol) -> bool {
        !self.in_module_load() && self.runtime.is_sealed(sym)
    }

    /// Reserve `sym`, so a later user `def` of it is refused. Called for every global
    /// an embedded module defines while loading.
    pub fn reserve_global(&self, sym: Symbol) {
        self.runtime.seal(sym);
    }

    /// Force `sym`'s declared contract (`sig!`, ADR-381/382): enforced whatever the mode
    /// and at every call, its own module's included. Runtime-level, so a globals restore
    /// does not roll it back.
    pub fn force_contract(&self, sym: Symbol) {
        self.runtime.force_contract(sym);
    }

    /// Was `sym`'s contract forced by a `sig!`?
    pub fn is_contract_forced(&self, sym: Symbol) -> bool {
        self.runtime.is_contract_forced(sym)
    }

    /// True while this process is loading an embedded std module.
    pub fn in_module_load(&self) -> bool {
        self.cold().is_some_and(|c| c.module_load_depth > 0)
    }

    /// Enter/leave an embedded-module load. Paired by `%load-module-source`, which
    /// decrements even when the load throws — a leaked exemption would silently
    /// un-reserve the language.
    pub fn enter_module_load(&mut self) {
        self.cold_mut().module_load_depth += 1;
    }
    pub fn leave_module_load(&mut self) {
        let d = &mut self.cold_mut().module_load_depth;
        *d = d.saturating_sub(1);
    }

    /// KI-134/KI-135: is this process inside a `%with-load-journal` — a `require-one` load —
    /// and not in the one place a load-time write must NOT be staged (a registry update's own
    /// whole-map write-back, a restore's replay)? While true, a global define or registry
    /// update lands in the innermost open [`LoadStage`] rather than the shared table.
    pub fn in_journalled_load(&self) -> bool {
        self.cold()
            .is_some_and(|c| !c.load_stages.is_empty() && !c.journal_suppressed)
    }

    /// Open a module load: push a staging frame (ADR-344). Everything the load defines or
    /// registers goes into it; [`Self::publish_module_load`] installs it, or
    /// [`Self::discard_module_load`] drops it when the load throws. RUNTIME compaction is
    /// held off for the frame's life, as `snapshot_globals` holds it off for a snapshot: the
    /// frame holds promoted handles that are not yet on the shared graph.
    pub fn enter_journalled_load(&mut self, direct: bool) {
        self.begin_rt_collect_block();
        // A `load` reached from inside a `require`'s frame IS that require's load — the
        // module is a runtime-wide fact whichever primitive read the file — so only an
        // outermost frame can be direct.
        let outermost = self.cold().is_none_or(|c| c.load_stages.is_empty());
        self.cold_mut().load_stages.push(LoadStage {
            direct: direct && outermost,
            ..LoadStage::default()
        });
    }

    /// A load threw: drop its frame. Nothing it defined or registered ever reaches the
    /// shared table — a broken module leaves no half-module behind.
    pub fn discard_module_load(&mut self) {
        if self.cold_mut().load_stages.pop().is_some() {
            // The loader's own lookups cached the staged values (`global_lookup_cached` keys
            // its cache on `version`), and an arm it ran may have baked one in: both must
            // re-read now that the frame is gone.
            self.runtime.version.fetch_add(1, Ordering::Relaxed);
            self.runtime.code_epoch.fetch_add(1, Ordering::Relaxed);
            self.end_rt_collect_block();
        }
    }

    /// A name defined by one of this process's open loads, innermost first.
    fn staged_lookup(&self, sym: Symbol) -> Option<Value> {
        let cold = self.cold()?;
        cold.load_stages
            .iter()
            .rev()
            .find_map(|st| st.bindings.get(&sym).copied())
    }

    /// The index of the innermost open load that defined `sym`, if any.
    fn staged_frame_of(&self, sym: Symbol) -> Option<usize> {
        let cold = self.cold()?;
        cold.load_stages
            .iter()
            .rposition(|st| st.bindings.contains_key(&sym))
    }

    /// Every value an open load of this process holds unpublished — collector roots for
    /// exactly the window they are not on the shared graph.
    pub(crate) fn staged_values(&self) -> impl Iterator<Item = Value> + '_ {
        self.cold()
            .into_iter()
            .flat_map(|c| c.load_stages.iter())
            .flat_map(|st| st.bindings.values().copied())
    }

    /// Is `sym` one of the registries loads coordinate THROUGH, written live even while a
    /// load is staged? Which ones is policy — the prelude's `*live-registries*` set, defined
    /// beside them (`std/prelude/tools.blsp`) — so it is read from the table here, never
    /// staged and never cached: it is a prelude binding and does not change.
    fn is_live_coordination_registry(&self, sym: Symbol) -> bool {
        static LIVE: std::sync::OnceLock<Symbol> = std::sync::OnceLock::new();
        let live = *LIVE.get_or_init(|| crate::core::value::intern(reg::LIVE_REGISTRIES));
        let set = self.runtime.globals_read().get(&live).copied();
        match set.map(|v| v.unpack()) {
            Some(ValueRef::Set(id)) => self.map_get(id, Value::Sym(sym)).is_some(),
            _ => false,
        }
    }

    /// A load completed: publish its frame (ADR-344). Under `registry_lock` — no registry
    /// read-modify-write interleaves — every staged registry op is re-applied to the LIVE
    /// table (a concurrent process may have registered into the same registry meanwhile),
    /// every value is re-homed into the current generation under the promote guard, and the
    /// whole set lands under ONE write of the globals table: a reader sees the module
    /// entirely or not at all. The writes are journalled for the KI-134 replay here, at the
    /// moment they land, with the same before/after check `env_define` makes.
    pub fn publish_module_load(&mut self) {
        let Some(stage) = self.cold_mut().load_stages.pop() else {
            return;
        };
        let direct = stage.direct;
        let rt = self.runtime.clone();
        let _registry = rt.registry_lock.lock().unwrap_or_else(|e| e.into_inner());
        // Resolve every write to the value it will bind. A registry op reads the LIVE
        // registry (the journal shape KI-134 settled on: replay the OPERATION, never a whole
        // map). `promote` takes the promote guard per value and must not nest inside the
        // guard held below, so this pass runs first.
        let mut pending: Vec<(Symbol, Value, LoadWrite)> = Vec::with_capacity(stage.writes.len());
        let mut staged_regs: std::collections::HashMap<Symbol, Value> =
            std::collections::HashMap::new();
        for w in stage.writes {
            match w {
                LoadWrite::Define { sym, val } => {
                    // A registry op later in this load builds on the map this load bound
                    // (`(defonce *reg* {})`, a swap, then an op — in any order).
                    staged_regs.insert(sym, val);
                    pending.push((sym, val, LoadWrite::Define { sym, val }));
                }
                LoadWrite::Registry { sym, op, path, val } => {
                    // Later ops on one registry within this load build on the earlier ones'
                    // result, not on the live value again.
                    let cur = staged_regs
                        .get(&sym)
                        .copied()
                        .or_else(|| self.runtime.globals_read().get(&sym).copied())
                        .unwrap_or(Value::nil());
                    if let Some(next) = self.registry_next(cur, op, &path, val) {
                        let next = self.promote(next);
                        staged_regs.insert(sym, next);
                        pending.push((sym, next, LoadWrite::Registry { sym, op, path, val }));
                    }
                }
            }
        }
        let pre = {
            let j = self
                .runtime
                .load_journal
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            j.outstanding > 0
        };
        if pre {
            for (_, _, w) in &pending {
                self.journal_load_write(w.clone(), direct);
            }
        }
        let mut rebind = false;
        {
            let _promote_guard = self
                .runtime
                .promote_lock
                .read()
                .unwrap_or_else(|e| e.into_inner());
            #[cfg(test)]
            super::table_swap_probe::begin(&self.runtime.version, &self.runtime.code_epoch);
            let mut table = self.runtime.globals_write();
            for (sym, val, _) in &pending {
                let shared = self.rehome_to_current_locked(*val);
                if table.insert(*sym, shared).is_some() {
                    rebind = true;
                }
            }
            // Bump `version` BEFORE the table lock drops (KI-193). Every process's global
            // inline cache is keyed on it, so a reader that takes the table after this
            // publish must also see the new version. Bumped after the unlock, a reader could
            // see the module's names in the table (`bound?` reads it directly) and then
            // serve `*features*` from its cache at the old version: the module visible and
            // not yet provided, which is exactly what ADR-344 exists to rule out. The
            // generations lock is only ever taken on its own, so nesting it here cannot
            // invert an order.
            let mut generations = self
                .runtime
                .global_generations
                .write()
                .unwrap_or_else(|e| e.into_inner());
            let generation = self.runtime.version.fetch_add(1, Ordering::Relaxed) + 1;
            for (sym, _, _) in &pending {
                generations.insert(*sym, generation);
            }
            self.features_audit_table(&table, "module publish");
            // The VM's global-read cache and the JIT key on `code_epoch`, not `version`, and
            // a load REBINDS at least `*features*`: the same window, through the other cache.
            if rebind {
                self.runtime.code_epoch.fetch_add(1, Ordering::Relaxed);
            }
        }
        #[cfg(test)]
        super::table_swap_probe::end(&self.runtime.version, &self.runtime.code_epoch);
        if !pre {
            // A snapshot may have begun between the check above and the insert; its clone
            // may predate these writes. A replayed define is idempotent, so the rare double
            // entry costs nothing.
            for (_, _, w) in &pending {
                if !self.journal_load_write(w.clone(), direct) {
                    break;
                }
            }
        }
        self.end_rt_collect_block();
    }

    /// Append `w` to the runtime's load journal — only while a snapshot is outstanding, i.e.
    /// while there is a restore that could discard the write; returns whether it did. One
    /// step under the journal's lock, so it cannot interleave with a snapshot's
    /// begin-and-clear or a restore's replay-and-clear (see `LoadJournal`).
    fn journal_load_write(&self, w: LoadWrite, direct: bool) -> bool {
        let mut j = self
            .runtime
            .load_journal
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if j.outstanding == 0 {
            return false;
        }
        let seq = j.next_seq;
        j.next_seq += 1;
        if let Some(t) = Self::global_trace_target() {
            let hit = match &w {
                LoadWrite::Define { sym, .. } | LoadWrite::Registry { sym, .. } => *sym == t,
            };
            if hit {
                eprintln!(
                    "[global] pid={:?} scope={} JOURNAL write for {} seq={} outstanding={} entries={}",
                    crate::process::current_pid(),
                    crate::process::self_isolate_scope(),
                    crate::core::value::symbol_name(t),
                    seq,
                    j.outstanding,
                    j.entries.len()
                );
            }
        }
        j.entries
            .push((seq, crate::process::self_isolate_scope(), direct, w));
        true
    }

    /// **Atomically** update a global that holds a registry (KI-22).
    ///
    /// Every load-time registry — `*impls*`, `*features*`, `*abilities*`, `*methods*`,
    /// `*record-ids*`, … — is one global holding a whole map or list, and Brood updates it
    /// as `(def *X* (assoc *X* …))`. That reads, computes and writes as three separate
    /// steps, so two processes registering at the same time each read the old value and each
    /// write their own successor: the later write silently drops the earlier one. Measured
    /// **218 of 500** concurrent registrations lost, after which the op dispatched to
    /// `:default` — a wrong answer, not a crash, and `impl` is hot-reloadable by design.
    ///
    /// The whole read-modify-write happens here, under `registry_lock`, so it is atomic by
    /// construction: no CAS (and so no ABA question), no retry loop, no spinning, and no
    /// callback into Brood while a lock is held. Two earlier in-language attempts failed
    /// exactly there — optimistic retry cannot close the read-write window, and a ticket lock
    /// either burns CPU busy-waiting or desynchronises when a bounded wait times out.
    ///
    /// `op` selects the update; `path` is `[k]` or `[k1 k2]` (nested one level, for
    /// `*impls*`/`*methods*`, whose shape is `ability -> id -> fn`):
    /// - `:assoc` — set `path` to `val`, creating the intermediate map if absent.
    /// - `:assoc-new` — the same, but only when `path` is currently **absent**. The
    ///   presence test has to be inside the lock too: a derived method mirror that checks
    ///   "absent?" outside it can clobber an authored impl registered in between.
    /// - `:dissoc` — remove `path` (one key).
    /// - `:cons-new` — prepend `val` to a list-valued global unless it is already a member.
    ///   Was `provide`'s op; `*features*` became a set-shaped map in ADR-216 (the membership
    ///   test has to be O(1)), so this op has no in-tree caller today. Kept as the generic
    ///   list-registry update — it is the only atomic one for a list-valued global.
    ///
    /// Returns true when the registry was written, false when the op declined (`:assoc-new`
    /// onto a present key, `:cons-new` of an existing member) — so Brood can still report
    /// "already there" without a second, racy read.
    ///
    /// `from` names the module making the write, for the KI-136 writer journal; `None`
    /// means "the namespace being compiled", which is the module whose LOAD is running the
    /// registration — a replay from an image passes the module it is replaying, since the
    /// namespace at that moment is whoever `require`d it.
    pub fn registry_update(
        &mut self,
        env: EnvId,
        sym: Symbol,
        op: RegistryOp,
        path: &[Value],
        val: Value,
        from: Option<Symbol>,
    ) -> bool {
        // Clone the Arc so the guard borrows a LOCAL, leaving `&mut self` free for the map
        // ops between the read and the write. Recover from a poisoned lock rather than
        // propagate: a panicking registrar leaves the registry structurally sound (values
        // are immutable), and wedging every later registration would be worse.
        let rt = self.runtime.clone();
        let mut guard = rt.registry_lock.lock().unwrap_or_else(|e| e.into_inner());
        let wrote = self.registry_apply(env, sym, op, path, val, true);
        if !wrote {
            return false;
        }
        // Only on the write path: a declined op leaves the registry untouched, and a name
        // that was never written has nothing for an image to carry.
        guard.insert(sym);
        // The writer journal (KI-136): who owns this entry now. A `:dissoc` retires it; any
        // other write records the writing module, or clears the entry for a root write.
        {
            let key = (sym, self.registry_path_key(path));
            let mut writers = rt
                .registry_writers
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match (op, from.or_else(|| self.compile_ns())) {
                (RegistryOp::Dissoc, _) | (_, None) => {
                    writers.remove(&key);
                }
                (_, Some(writer)) => {
                    writers.insert(key, writer);
                }
            }
        }
        if reg_trace_enabled() && crate::core::value::symbol_name(sym) == reg::RECORD_IDS {
            let k1 = path.first().copied().unwrap_or(Value::nil());
            // Ancestry chain (up to 4 hops), so a leaked writer can be attributed to the
            // unit/driver that spawned it even after intermediates exited.
            let mut chain = String::new();
            let mut cur = crate::process::current_pid();
            for _ in 0..4 {
                match cur {
                    Some(p) => {
                        chain.push_str(&format!("{p}<-"));
                        cur = crate::process::parent_of(p);
                    }
                    None => break,
                }
            }
            // The `%isolate` ownership stamp beside the chain. The chain alone could not
            // settle KI-89: a walk that reaches the runner does not say whether the hop it
            // arrives through was spawned INSIDE the isolate that is restoring, which is
            // the whole question. `owner` is that answer directly.
            eprintln!(
                "[reg] {} {:?} {} chain={} owner={}",
                crate::core::value::symbol_name(sym),
                op,
                crate::syntax::printer::print(self, k1),
                chain,
                crate::process::current_pid()
                    .map(crate::process::isolate_owner_of)
                    .unwrap_or(0),
            );
        }
        true
    }

    /// The body of [`Self::registry_update`] — the read-modify-write itself, with
    /// `registry_lock` HELD BY THE CALLER. Split out so a restore can re-apply a journalled
    /// registry operation to the restored table under the lock it already holds (KI-134);
    /// `journal` is false there, since the entry being replayed IS the journal. Returns
    /// whether the registry was written (false when the op declined).

    /// The value registry global `sym` takes after `op` at `path` with `val`, given its
    /// current value `cur` — the read-modify-write's MODIFY, with no read and no write, so
    /// that [`Self::registry_apply`] can run it against the live binding and a restore's
    /// journal replay (KI-134) against the table it is rebuilding. `None` when the op
    /// declines (`:assoc-new` onto a present key, `:cons-new` of a member, a `:dissoc` with
    /// nothing to remove). Allocates the new map LOCALLY; the caller promotes it.
    fn registry_next(
        &mut self,
        cur: Value,
        op: RegistryOp,
        path: &[Value],
        val: Value,
    ) -> Option<Value> {
        let k1 = path.first().copied().unwrap_or(Value::nil());
        let next = match op {
            RegistryOp::ConsNew => {
                if self.list_contains(cur, val) {
                    return None;
                }
                self.alloc_pair(val, cur)
            }
            RegistryOp::AppendNew => {
                if self.list_contains(cur, val) {
                    return None;
                }
                let mut items = self.list_to_vec(cur).unwrap_or_default();
                items.push(val);
                self.list(items)
            }
            RegistryOp::Merge => {
                let ValueRef::Map(patch) = val.unpack() else {
                    return None;
                };
                let outer = match cur.unpack() {
                    ValueRef::Map(id) => id,
                    _ => match self.alloc_empty_map().unpack() {
                        ValueRef::Map(id) => id,
                        _ => return None,
                    },
                };
                let mut merged = match self.map_get(outer, k1) {
                    Some(v) => match v.unpack() {
                        ValueRef::Map(_) => v,
                        _ => return None,
                    },
                    None => self.alloc_empty_map(),
                };
                for (k, v) in self.map_entries(patch) {
                    let ValueRef::Map(id) = merged.unpack() else {
                        return None;
                    };
                    merged = self.map_assoc(id, k, v);
                }
                self.map_assoc(outer, k1, merged)
            }
            RegistryOp::Dissoc => match cur.unpack() {
                ValueRef::Map(id) => {
                    if path.len() >= 2 {
                        // NESTED dissoc, symmetric with `:assoc`'s two-key path: remove `k2`
                        // from the inner map at `k1`, leaving that map (and every sibling
                        // key) in place. Without this, a two-key `:dissoc` silently used
                        // only `k1` and removed the WHOLE inner map — which is how
                        // `unregister-impl`, retracting one id of `[ability op]`, destroyed
                        // every impl of that op including the language's `:default`.
                        let k2 = path[1];
                        match self.map_get(id, k1).map(|v| v.unpack()) {
                            Some(ValueRef::Map(inner)) => {
                                let inner_next = self.map_dissoc(inner, k2);
                                self.map_assoc(id, k1, inner_next)
                            }
                            // no inner map at `k1`: nothing to remove
                            _ => return None,
                        }
                    } else {
                        self.map_dissoc(id, k1)
                    }
                }
                _ => return None,
            },
            RegistryOp::Assoc | RegistryOp::AssocNew => {
                let outer = match cur.unpack() {
                    ValueRef::Map(id) => id,
                    // An uninitialised registry (nil) starts as an empty map rather than
                    // failing — the same shape `(or *X* {})` had at the call sites.
                    _ => match self.alloc_empty_map().unpack() {
                        ValueRef::Map(id) => id,
                        _ => unreachable!("alloc_empty_map returns a map"),
                    },
                };
                if path.len() >= 2 {
                    let k2 = path[1];
                    let inner_cur = self.map_get(outer, k1);
                    let inner_id = match inner_cur.map(|v| v.unpack()) {
                        Some(ValueRef::Map(id)) => id,
                        _ => match self.alloc_empty_map().unpack() {
                            ValueRef::Map(id) => id,
                            _ => unreachable!("alloc_empty_map returns a map"),
                        },
                    };
                    if op == RegistryOp::AssocNew && self.map_get(inner_id, k2).is_some() {
                        return None;
                    }
                    let inner = self.map_assoc(inner_id, k2, val);
                    // (Allocation cannot invalidate `outer` — collection in this runtime happens
                    // only at eval safepoints, never inside `map_assoc`.)
                    self.map_assoc(outer, k1, inner)
                } else {
                    if op == RegistryOp::AssocNew && self.map_get(outer, k1).is_some() {
                        return None;
                    }
                    self.map_assoc(outer, k1, val)
                }
            }
        };
        Some(next)
    }

    fn registry_apply(
        &mut self,
        env: EnvId,
        sym: Symbol,
        op: RegistryOp,
        path: &[Value],
        val: Value,
        journal: bool,
    ) -> bool {
        // `def` binds at `env_root(env)`, which is NOT always `EnvId::GLOBAL`: during prelude
        // load the root is a bootstrap env whose bindings later seed the shared runtime. A
        // write straight to the globals table there is silently dropped (it cost the prelude
        // its own `Display`/`Inspect` impls). Read and write the same place `def` would.
        let root = self.env_root(env);
        // This process's open load's staged result for `sym` first (a second op on a live
        // registry within one load builds on the first's), then the table — the order
        // `global_lookup_cached` reads in, so the loader's own view is consistent.
        let cur = self
            .staged_lookup(sym)
            .or_else(|| self.env_get(env, sym))
            .unwrap_or(Value::nil());
        let Some(next) = self.registry_next(cur, op, path, val) else {
            return false;
        };
        // KI-135 / ADR-344: while a module load is open, the OPERATION is staged (with its
        // keys and value promoted, so the frame holds RUNTIME handles) and the resulting map
        // is staged as this process's view of the registry. `publish_module_load` re-applies
        // the op to the live registry — never the map, which would clobber what a concurrent
        // process registered meanwhile — and journals it for the KI-134 replay. The two
        // registries loads coordinate through stay live (`is_live_coordination_registry`).
        if journal && self.in_journalled_load() && !self.is_live_coordination_registry(sym) {
            let path: Vec<Value> = path.iter().map(|v| self.promote(*v)).collect();
            let val = self.promote(val);
            let next = self.promote(next);
            // Into the frame that DEFINED the registry when one of this process's open loads
            // did (a module defining a registry, then requiring the submodules that register
            // into it): the owner publishes the whole binding, so the op must be its.
            let owner = self.staged_frame_of(sym);
            let stages = &mut self.cold_mut().load_stages;
            let idx = owner.unwrap_or(stages.len() - 1);
            let st = &mut stages[idx];
            st.writes.push(LoadWrite::Registry { sym, op, path, val });
            st.bindings.insert(sym, next);
            self.runtime.version.fetch_add(1, Ordering::Relaxed);
            // And `code_epoch`, as the staged DEFINE branch of `env_define` does for a
            // rebind: a registry op is always a rebind of a live name, and a compiled arm
            // that read the registry through a `GlobalIc` before this op keys that cache on
            // the epoch, not on `version`. Without the bump `%register-method`'s
            // `(contains? *multi-algebra* mname)` served the map from the FIRST `defmulti`
            // of a file to every later `defmethod` — "no `(defmulti mm-cmp …)` is in
            // scope" for the second multimethod of `multimethod_test.blsp`, deterministic,
            // once KI-170 put a directly loaded file in this frame (a `require`d module of
            // that shape had the same latent bug).
            self.runtime.code_epoch.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        let was_private = self.runtime.is_private_recorded(sym);
        // The whole-map write-back goes to the live table (a live coordination registry, or
        // no load open); `journal_suppressed` keeps `env_define` from staging or journalling
        // a define whose OPERATION is the real record.
        self.features_audit_write(sym, path, next);
        let prev_suppressed = std::mem::replace(&mut self.cold_mut().journal_suppressed, true);
        self.env_define(root, sym, next);
        self.cold_mut().journal_suppressed = prev_suppressed;
        if was_private {
            self.runtime.mark_private(sym);
        }
        true
    }

    /// Does registry global `sym` (a map) contain `key`, read from the SHARED globals table
    /// **bypassing the per-process inline cache**? `env_get`/`global_lookup_cached` gate their
    /// cache on `runtime.version`, loaded `Relaxed`, so a concurrent registrar's just-committed
    /// entry can be momentarily invisible to another process's cached read. That is fine for a
    /// hot lookup, but WRONG for `require`'s load-once guard: a requirer whose `*features*` read
    /// missed a racing loader's `provide` would reload an already-loaded module (the co-located
    /// secondary double-load, ADR-225). A direct `globals_read` synchronises with the writer's
    /// `globals_write`, so it never observes a stale "absent".
    pub fn registry_member(&self, sym: Symbol, key: Value) -> bool {
        match self.runtime.globals_read().get(&sym).map(|v| v.unpack()) {
            Some(ValueRef::Map(id)) => self.map_get(id, key).is_some(),
            _ => false,
        }
    }

    /// `(%registry-cas! 'sym old new)` — compare-and-swap a registry global under the same
    /// lock as [`Self::registry_update`]. Rebinds `sym` to `new` and returns true **only if**
    /// its current value still equals `old`; otherwise leaves it alone and returns false, so
    /// the caller can recompute against the value that won and retry.
    ///
    /// This is the general form of `registry_update`. That one has to name every shape it
    /// supports as an op (`:assoc`, `:cons-new`, …), which covers a registry whose update is
    /// one map/list operation and nothing else. The registries in `std/` are not all like
    /// that: `face-set` merges into the *existing* entry, `attach` strips an id across every
    /// bucket before consing onto one, `register-repl-command` filters by name-overlap and
    /// appends. Expressing those as ops would mean a Rust op per shape — and the transform
    /// itself is policy, which belongs in Brood. A CAS lets the transform stay an ordinary
    /// Brood function (`registry-swap!` in the prelude retries around it) while the
    /// read-decide-write stays indivisible.
    ///
    /// Equality is structural, so an ABA against an equal-valued registry is indistinguishable
    /// — and harmless: the retry would recompute the same answer.
    pub fn registry_cas(&mut self, env: EnvId, sym: Symbol, old: Value, new: Value) -> bool {
        let rt = self.runtime.clone();
        let mut guard = rt.registry_lock.lock().unwrap_or_else(|e| e.into_inner());
        // Read through the chain and write at the root, exactly as `def` does (see
        // `registry_update`: the root is NOT always `EnvId::GLOBAL` during prelude load).
        // Matching `def` is what makes a `defdyn` registry safe to convert — an active
        // `binding` shadows the root write for both spellings identically.
        let root = self.env_root(env);
        // Compare against the view the caller READ: `global_lookup_cached` reaches this
        // process's staged binding before the table (ADR-344), so a live registry rebound in
        // an open load, read staged and compared live, would never match — `%registry-swap!`
        // retried forever (that is how `repl`'s `*require-edges*` edge went missing).
        let cur = self
            .staged_lookup(sym)
            .or_else(|| self.env_get(env, sym))
            .unwrap_or(Value::nil());
        if !self.equal(cur, old) {
            return false;
        }
        // Save and restore privacy across `env_define`, exactly as `registry_update` does and
        // for the same reason: `env_define` calls `unmark_private`, which is right for a real
        // `def` (editing `def-` → `def` and reloading must publish the name) and wrong for an
        // in-place registry UPDATE, where no def form changed and nothing re-marks afterwards.
        //
        // `registry_update` grew this guard and its sibling here did not — the two funnels are
        // documented as a pair four lines up, and the fix went into one of them. So a `def-`'d
        // registry reached through `%swap-registry!` rather than `%registry-update!` still
        // turned public on its first write: `*require-edges*` read `reflect/private? = true` at
        // boot and false after the first `require` recorded an edge, which is every program.
        // Found by ADR-320's journal differential, because the name is absent from
        // `(reflect/global-names)` and no per-global gate could see it.
        let was_private = self.runtime.is_private_recorded(sym);
        // ADR-344: a registry this process's OPEN load defined (`(defonce *reg* {})` then a
        // swap in the same module) is not on the shared table yet — the new map replaces the
        // staged binding in the frame that owns it, and publishes with the module. Written
        // live instead, the frame's `Define` would overwrite it at publish.
        if self.in_journalled_load() {
            if let Some(owner) = self.staged_frame_of(sym) {
                self.global_trace(sym, "swap->staged", Some(new));
                let shared = self.promote(new);
                let st = &mut self.cold_mut().load_stages[owner];
                st.writes.push(LoadWrite::Define { sym, val: shared });
                st.bindings.insert(sym, shared);
                self.runtime.version.fetch_add(1, Ordering::Relaxed);
                self.runtime.code_epoch.fetch_add(1, Ordering::Relaxed);
                guard.insert(sym);
                return true;
            }
        }
        // Otherwise LIVE, never staged or journalled: this funnel writes a WHOLE map, and a
        // whole map cannot be re-applied — staged, it would publish an older copy over what a
        // nested load or a concurrent process added meanwhile; replayed after an isolate, it
        // would clobber what other processes wrote since. The live registries that reach here
        // are bookkeeping whose transform is policy in Brood (`registry-swap!`); a registration
        // that must survive an isolate goes through `%registry-update!`'s ops instead.
        self.global_trace(sym, "swap->LIVE", Some(new));
        let prev_suppressed = std::mem::replace(&mut self.cold_mut().journal_suppressed, true);
        self.env_define(root, sym, new);
        self.cold_mut().journal_suppressed = prev_suppressed;
        if was_private {
            self.runtime.mark_private(sym);
        }
        guard.insert(sym);
        true
    }

    /// Every global a registry update has written in this runtime — the derived answer to
    /// "which globals does *loading* mutate rather than create?".
    ///
    /// A startup image is built from the `(reflect/global-names)` diff across a load (ADR-218), which
    /// by construction cannot see a global that already existed and was only updated. Those
    /// were named by hand and the list went stale three times, silently: `declared_sigs`
    /// weakened the checker, seven ability/multimethod registries governed dispatch with no
    /// error when lost, and `*method-from*` stopped cross-module `defmethod` conflicts being
    /// reported. Both funnels record here instead, so the set is a consequence of the writes
    /// rather than of anyone's memory. Which of them an image should *carry* stays policy, in
    /// `std/tool/project.blsp`.
    ///
    /// Sorted by spelling, like `(reflect/global-names)` — a set iterates in hash order, and a
    /// caller diffing two runs or asserting on the set wants neither that nor interner order.
    /// Record `names` as registries without writing them — the prelude-image loader's
    /// counterpart of the insert `registry_update` does on every write. A materialised
    /// prelude never calls `%registry-update!`, so the set the source boot built by
    /// evaluating `(defmulti num/add :commutative)` and friends has to be carried in the
    /// image and re-marked here; `freeze_as_shared_code` then reads it into
    /// `SharedCode::registry_names` exactly as it does after a source boot. Without this the
    /// imaged boot's set was 10 names to the source boot's 12 — missing `*multi-algebra*` and
    /// `*multi-ret*`, the two only the prelude itself writes — and `project-registry-snapshot`
    /// stopped protecting them across a section load (KI-106, the KI-89 mechanism again).
    pub fn mark_registry_names(&self, names: &[Symbol]) {
        let mut guard = self
            .runtime
            .registry_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.extend(names.iter().copied());
    }

    /// The journal key for a registry write's path: each key printed, space-joined — the
    /// same rendering `registry_writer` asks with, so a Brood caller never has to
    /// reproduce it.
    fn registry_path_key(&self, path: &[Value]) -> String {
        path.iter()
            .map(|&k| crate::syntax::printer::print(self, k))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The module whose load wrote registry `sym`'s entry at `path` (KI-136), or `None`
    /// for an entry nobody owns — a prelude or root write, or one a `:dissoc` retired.
    pub fn registry_writer(&self, sym: Symbol, path: &[Value]) -> Option<Symbol> {
        let key = (sym, self.registry_path_key(path));
        self.runtime
            .registry_writers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .copied()
    }

    pub fn registry_names(&self) -> Vec<Symbol> {
        let mut names: Vec<Symbol> = self
            .runtime
            .registry_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect();
        // …plus the ones the prelude build wrote, which no process re-writes (see the
        // field's note on `SharedCode`).
        names.extend(self.prelude.registry_names.iter().copied());
        names.sort_by_cached_key(|&s| crate::core::value::symbol_name(s));
        names.dedup();
        names
    }

    /// Structural `member?` over a proper list — the `:cons-new` presence test, kept inside
    /// [`Self::registry_update`]'s lock so `provide` cannot double-add under a race.
    fn list_contains(&self, list: Value, needle: Value) -> bool {
        let mut cur = list;
        while let ValueRef::Pair(id) = cur.unpack() {
            let (head, tail) = {
                let p = self.pair(id);
                (p.0, p.1)
            };
            if self.equal(head, needle) {
                return true;
            }
            cur = tail;
        }
        false
    }

    /// `BROOD_TRACE_GLOBAL=<name>`: the global whose every root write and every isolate
    /// restore is narrated to stderr (`[global] …`), with the writer's pid and isolate scope
    /// and which funnel it took (define / staged swap / live swap / restore). Read once.
    /// The KI-141 tool: a registry that is full in one test file and empty in the next has a
    /// timeline nothing else prints — `BROOD_REG_TRACE` watches `*record-ids*` alone.
    fn global_trace_target() -> Option<Symbol> {
        static TARGET: std::sync::OnceLock<Option<Symbol>> = std::sync::OnceLock::new();
        *TARGET.get_or_init(|| {
            std::env::var("BROOD_TRACE_GLOBAL")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| crate::core::value::intern(&s))
        })
    }

    fn global_trace(&self, sym: Symbol, what: &str, val: Option<Value>) {
        if Self::global_trace_target() != Some(sym) {
            return;
        }
        let shown = match val {
            Some(v) => {
                let text = crate::syntax::printer::print(self, v);
                let n: String = text.chars().take(120).collect();
                format!(" {n}")
            }
            None => String::from(" <absent>"),
        };
        eprintln!(
            "[global] pid={:?} scope={} {} {}{}",
            crate::process::current_pid(),
            crate::process::self_isolate_scope(),
            what,
            crate::core::value::symbol_name(sym),
            shown
        );
    }

    pub fn env_define(&mut self, env: EnvId, sym: Symbol, val: Value) {
        self.global_trace(
            sym,
            if env == EnvId::GLOBAL {
                "define"
            } else {
                "define(non-root)"
            },
            Some(val),
        );
        if env == EnvId::GLOBAL {
            // Privacy (ADR-146) is declared by the def FORM, not the name: clear any
            // prior private mark on every global def (before the dedup early-return,
            // so it fires even on an identical-closure reload). A `defn-`/`def-`
            // re-marks right after via `%mark-private`; a plain `defn`/`def` leaves it
            // public — so editing `defn-` → `defn` and hot-reloading makes the name
            // public. The prelude's privates are seeded in `seeded`, not here.
            self.runtime.unmark_private(sym);
            // …and its stability facts, for the same reason (ADR-283): a redefinition must
            // not inherit the old name's `:deprecated`.
            self.runtime.clear_meta(sym);
            // Dedup an unchanged hot-reload redefinition (Stage 5): if `sym` is
            // already bound to a closure structurally identical to `val`, keep the
            // existing (already-promoted) binding rather than append a duplicate
            // into the append-only RUNTIME region. Bounds the leak for the common
            // save-without-change / formatter-churn path; any *real* edit differs
            // structurally and falls through to the normal promote+rebind.
            let existing = self.runtime.globals_read().get(&sym).copied();
            if let Some(old) = existing {
                let unchanged = match (old.unpack(), val.unpack()) {
                    (ValueRef::Fn(o), ValueRef::Fn(n)) => self.closures_structurally_equal(o, n),
                    (ValueRef::Macro(o), ValueRef::Macro(n)) => {
                        self.closures_structurally_equal(o, n)
                    }
                    _ => false,
                };
                if unchanged {
                    return;
                }
            }
            // Global code/data is shared across inner processes, so promote it into the
            // shared RUNTIME region before binding, re-homing a value promote left in a
            // *non-current* generation (an already-RUNTIME handle passes through
            // unchanged) into the current one — so a `def` can never re-pin a draining
            // generation through the shared globals table (ADR-091 Stage 5 soundness).
            //
            // Promote, re-home and the table insert all happen under ONE `promote_lock`
            // read guard: an aging flip between the re-home and the store would either
            // strand this binding on a generation about to be freed or let migration's
            // reconcile revert it. See `promote_rehome_publish`.
            // KI-135 / ADR-344: an OPEN module load binds into its staging frame, never the
            // shared table; `publish_module_load` installs the frame whole. The loader's own
            // lookups reach it through `staged_lookup`; a rebind of a staged (or live) name
            // bumps `version` so this process's inline cache re-reads, and `code_epoch` so
            // an arm that baked the old value in re-validates.
            if self.in_journalled_load() {
                let shared = self.promote(val);
                let was_live = self.runtime.globals_read().contains_key(&sym);
                let was_staged = {
                    let st = self.cold_mut().load_stages.last_mut().expect("open load");
                    st.writes.push(LoadWrite::Define { sym, val: shared });
                    st.bindings.insert(sym, shared).is_some()
                };
                {
                    let mut generations = self
                        .runtime
                        .global_generations
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    let generation = self.runtime.version.fetch_add(1, Ordering::Relaxed) + 1;
                    generations.insert(sym, generation);
                }
                if was_live || was_staged {
                    self.runtime.code_epoch.fetch_add(1, Ordering::Relaxed);
                }
                return;
            }
            let journal = self.in_journalled_load();
            let rebind = self.promote_rehome_publish(val, |h, shared| {
                // Test probe: assert (from inside the window) that the publish really is
                // covered by the read guard. `try_write` fails iff a read guard is held —
                // and this thread holds it, so it must fail. Compiled out entirely
                // otherwise; see `heap::def_publish_probe`.
                #[cfg(test)]
                def_publish_probe::observe(&h.runtime.promote_lock);
                // KI-134: a module-load write is journalled BEFORE it lands, so a restore
                // that swaps the table between the two finds it in the journal; the second
                // attempt after the insert covers a snapshot that began between the first
                // attempt (nothing outstanding, so nothing journalled) and the insert — its
                // clone may predate the write. A replayed define is idempotent, so the rare
                // double entry costs nothing.
                let pre =
                    journal && h.journal_load_write(LoadWrite::Define { sym, val: shared }, false);
                let was = {
                    let mut table = h.runtime.globals_write();
                    let was = table.insert(sym, shared).is_some();
                    h.features_audit_define(sym, &table);
                    was
                };
                if journal && !pre {
                    h.journal_load_write(LoadWrite::Define { sym, val: shared }, false);
                }
                was
            });
            // Invalidate every process's global inline cache (late binding), and stamp
            // this name with the new version — its rebinding generation.
            // The bump happens under the generations lock so two racing `def`s of one
            // name stamp it in the order they took the counter.
            {
                let mut generations = self
                    .runtime
                    .global_generations
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                let generation = self.runtime.version.fetch_add(1, Ordering::Relaxed) + 1;
                generations.insert(sym, generation);
            }
            // The JIT's code epoch moves only on a REBIND (ADR-217). Binding a name for
            // the first time cannot invalidate compiled code — no arm can have baked in
            // a binding that did not exist when it compiled (see `code_epoch`'s doc) —
            // and bumping here made a bulk load re-tier every JIT'd arm per `def`.
            if rebind {
                self.runtime.code_epoch.fetch_add(1, Ordering::Relaxed);
            }
        } else if env.is_old() {
            // The frame was tenured (a minor collection promoted it while it was
            // still being bound — e.g. a collection during a `let` rhs eval). Mutate
            // it in the old space and remember it: this push can create an
            // OLD->YOUNG edge (`val` is a fresh nursery value), which the next minor
            // collection must trace and rewrite, since it otherwise never scans old.
            // De-dup: repeated binds into the same tenured frame (a long `let`
            // body, a binding loop) would otherwise re-push it every time,
            // growing `remembered` — and the minor's rewrite walk — without
            // bound until the next tenure clears it. The linear scan is fine:
            // deduped, the set holds one entry per *distinct* old frame mutated
            // since the last minor, which is tiny.
            self.old_mut().envs[env.index()].vars.push((sym, val));
            if !self.remembered.contains(&env) {
                self.remembered.push(env);
            }
        } else {
            self.local.envs[env.index()].vars.push((sym, val));
        }
    }

    // ----- dynamic-variable bindings (the `binding` form) -----

    /// Push a dynamic binding of `sym` to `val` (the innermost wins on lookup).
    /// Paired with [`Heap::pop_dynamic`] by the `%binding` primitive, which pops
    /// exactly what it pushed when its body returns — even on error.
    pub fn push_dynamic(&mut self, sym: Symbol, val: Value) {
        self.dynamics.push((sym, val));
    }

    /// Pop the most recent dynamic binding (the matching unwind of `push_dynamic`).
    pub fn pop_dynamic(&mut self) {
        self.dynamics.pop();
    }

    /// The current (innermost) value of dynamic variable `sym`, if a `binding` for
    /// it is active — the read side of [`push_dynamic`]. An empty stack (the common
    /// case) costs one `is_empty` check. Used by `spawn` to inherit a propagating
    /// causal context (`*trace-context*`) into a child without an explicit hand-off.
    pub fn current_dynamic(&self, sym: Symbol) -> Option<Value> {
        if self.dynamics.is_empty() {
            return None;
        }
        self.dynamics
            .iter()
            .rev()
            .find(|&&(s, _)| s == sym)
            .map(|&(_, v)| v)
    }

    /// The debugger's durable per-process trace context (ADR-174), or `None`. A
    /// settable slot (unlike a `binding`): `spawn` copies it into a child, `send`
    /// ships it, `receive` overwrites it on pop. GC-traced with [`dynamics`].
    #[cfg(feature = "dev-tools")]
    pub fn trace_context(&self) -> Option<Value> {
        self.trace_context
    }

    /// Set (or clear) the durable per-process trace context. `own` marks it as this
    /// process's own context (propagated by `spawn`) versus one adopted from a message
    /// (not propagated). The value must be a promoted/LOCAL handle valid in this heap;
    /// it is then traced like a root.
    #[cfg(feature = "dev-tools")]
    pub fn set_trace_context(&mut self, v: Option<Value>, own: bool) {
        self.trace_context = v;
        self.trace_context_own = own;
    }

    /// Whether the current [`trace_context`] is the process's OWN (propagate on spawn),
    /// not merely adopted from a message.
    #[cfg(feature = "dev-tools")]
    pub fn trace_context_own(&self) -> bool {
        self.trace_context_own
    }

    /// Snapshot the runtime's global bindings (`symbol -> value`). Cheap: the
    /// values are `Copy` handles. Pair with [`Heap::restore_globals`] to run code
    /// against a *private copy* of the globals — mutations to the live table can
    /// then be rolled back (this is what the `%isolate` primitive does for
    /// `:isolated` tests). Only meaningful when no other process is writing the
    /// table concurrently.
    pub fn snapshot_globals(&self) -> GlobalsSnapshot {
        // The snapshot holds raw RUNTIME handles off the graph; suppress RUNTIME compaction
        // until the paired `restore_globals` reinstalls (or discards) it, so a relocation
        // can't strand those handles (KI-6). Structural — every caller of the protocol is
        // covered, not just `%isolate`. The `#[must_use]` guard + by-value restore make
        // forgetting-to-restore a compiler warning and double-restore impossible.
        self.begin_rt_collect_block();
        // KI-134: register this snapshot BEFORE reading the table, so a module-load write
        // that lands after the clone is journalled (`env_define` checks the journal on both
        // sides of its insert). The first outstanding snapshot starts the journal fresh —
        // with nothing outstanding, nothing before it can need replaying — and one lock
        // covers the count, the clear and the mark together (see `LoadJournal`).
        let journal_mark = {
            let mut j = self
                .runtime
                .load_journal
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if j.outstanding == 0 {
                j.entries.clear();
            }
            j.outstanding += 1;
            if Self::global_trace_target().is_some() {
                eprintln!(
                    "[global] pid={:?} scope={} SNAPSHOT journal: mark={} outstanding={} entries={}",
                    crate::process::current_pid(),
                    crate::process::self_isolate_scope(),
                    j.next_seq,
                    j.outstanding,
                    j.entries.len()
                );
            }
            j.next_seq
        };
        GlobalsSnapshot {
            saved: self.runtime.globals_read().clone(),
            block_depth: self.rt_collect_block.get(),
            journal_mark,
        }
    }

    /// Every symbol currently bound in the global table (prelude + user `def`s).
    /// For tooling/introspection — `(reflect/global-names)` feeds completion and
    /// workspace-symbol queries (see `docs/lsp.md`). Returns just the keys, so
    /// no `Value`s are cloned.
    pub fn global_symbols(&self) -> Vec<Symbol> {
        self.runtime.globals_read().keys().copied().collect()
    }

    /// The public exports of module `prefix` (a `mod/` segment, trailing slash included) as
    /// `(bare, qualified)` symbol pairs — the set `(:use mod)` refers. Public = a *direct*
    /// `mod/name` global whose bare tail is non-empty and is not private
    /// (matching `%refer`'s scan). Cached + count-keyed (see
    /// [`module_exports_cache`](Self::module_exports_cache)) so the checker's per-file
    /// `(:use …)` resolution builds the index by ONE pass over the globals instead of
    /// rescanning every global per file (O(files²)). Empty when the module isn't loaded (no
    /// `mod/*` globals) — the checker then `require`s it first. Checker use only.
    pub fn module_public_exports(&self, prefix: &str) -> Vec<(Symbol, Symbol)> {
        let count = self.runtime.globals_read().len();
        let fresh = matches!(self.check.borrow().as_ref()
            .and_then(|c| c.exports.as_ref()), Some((c, _)) if *c == count);
        if !fresh {
            let mut map: std::collections::HashMap<String, Vec<(Symbol, Symbol)>> =
                std::collections::HashMap::new();
            for g in self.global_symbols() {
                let name = crate::core::value::symbol_name(g);
                if let Some(slash) = name.rfind('/') {
                    let bare = &name[slash + 1..];
                    // `g` is a live enumerated global, so `is_private` (the recorded
                    // fact) is exact here; the module is definitionally loaded.
                    if !bare.is_empty() && !self.is_private(g) {
                        let bare_sym = crate::core::value::intern(bare);
                        map.entry(name[..=slash].to_string())
                            .or_default()
                            .push((bare_sym, g));
                    }
                }
            }
            self.check_mut().exports = Some((count, std::sync::Arc::new(map)));
        }
        self.check
            .borrow()
            .as_ref()
            .and_then(|c| c.exports.as_ref())
            .and_then(|(_, m)| m.get(prefix).cloned())
            .unwrap_or_default()
    }

    /// The set of `mod/` namespace prefixes present in the loaded image (the checker's
    /// `known_ns`), cached + shared as an `Arc` (see [`known_ns_cache`](Self::known_ns_cache)).
    /// Count-keyed like [`module_public_exports`](Self::module_public_exports) — checker-only,
    /// sound because a whole-project check does no `def`s per file, so an O(1) `Arc` clone on
    /// all but the first file (was an O(globals) scan per file → O(files²)).
    pub fn known_ns_prefixes(&self) -> std::sync::Arc<std::collections::HashSet<String>> {
        let count = self.runtime.globals_read().len();
        if let Some((c, arc)) = self
            .check
            .borrow()
            .as_ref()
            .and_then(|c| c.known_ns.as_ref())
        {
            if *c == count {
                return std::sync::Arc::clone(arc);
            }
        }
        let mut set = std::collections::HashSet::new();
        for sym in self.global_symbols() {
            let name = crate::core::value::symbol_name(sym);
            if let Some(slash) = name.rfind('/') {
                set.insert(name[..=slash].to_string());
            }
        }
        let arc = std::sync::Arc::new(set);
        self.check_mut().known_ns = Some((count, std::sync::Arc::clone(&arc)));
        arc
    }

    // ── Phase-2 incremental-check dependency recorder (ADR-119) ───────────────
    // Per-process (this heap travels with the green process), so `check-file-deps`
    // can run concurrently across the worker pool without clobbering. See the
    // `check_dep_rec` field and `types::check::deps`.

    /// Start recording global observations into a fresh record on this heap.
    pub(crate) fn begin_check_dep_record(&self) {
        self.check_mut().dep_rec = Some(CheckDepRec::default());
    }
    /// Drain and return the recorded observations (clearing the recorder).
    pub(crate) fn take_check_dep_record(&self) -> Option<CheckDepRec> {
        self.check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.take())
    }
    /// Record an observed global symbol (binding/arity/sig).
    pub(crate) fn rec_check_dep_sym(&self, sym: Symbol) {
        // A gensym is a macro-expansion temporary, never a global: its counter differs
        // in every process, so recording one made the dependency fingerprint
        // process-specific — `nest check` and `nest run` could never share a verdict
        // (found 2026-08-30: 1 982 keys per bedit file, the differing ones all `and__N`).
        if crate::core::value::is_gensym(sym) {
            return;
        }
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            d.syms.insert(sym);
        }
    }
    /// Record a queried `mod/` known-namespace prefix.
    pub(crate) fn rec_check_dep_ns(&self, prefix: &str) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            if !d.known_ns.contains(prefix) {
                d.known_ns.insert(prefix.to_string());
            }
        }
    }
    /// Record a `mod/` prefix whose export set was read.
    pub(crate) fn rec_check_dep_exports(&self, prefix: &str) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            if !d.exports.contains(prefix) {
                d.exports.insert(prefix.to_string());
            }
        }
    }
    /// Record that the `*protocols*` table was consulted.
    pub(crate) fn rec_check_dep_protocols(&self) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            d.protocols = true;
        }
    }
    /// Record a global DEFINED in the file being checked (excluded from dep-keys).
    pub(crate) fn rec_check_dep_own(&self, sym: Symbol) {
        if let Some(d) = self
            .check
            .borrow_mut()
            .as_mut()
            .and_then(|c| c.dep_rec.as_mut())
        {
            d.own.insert(sym);
        }
    }

    /// Register a user-declared `(sig name type)` signature: `sym` is the
    /// module-qualified global symbol (the same key `def` would produce), `type_value`
    /// the raw type-expression form (e.g. `(int -> int)`). The value is `promote`d into
    /// the shared RUNTIME region first — the store is shared across the runtime's
    /// processes (`Arc`) and must outlive the LOCAL heap, exactly like a global. Read
    /// by the checker via [`Heap::declared_sig_value`]. Idempotent: a re-`def`/reload
    /// just overwrites. (No `version` bump — declared sigs aren't consulted by the
    /// per-process global inline cache.)
    pub fn set_declared_sig(&mut self, sym: Symbol, type_value: Value) {
        // Promote + re-home + store under one `promote_lock` read guard, for the same
        // reason as a global `def` (`declared_sigs` is a shared root the drain scans, so
        // a handle stored here against a generation that flipped in between would re-pin
        // a draining generation — ADR-091 Stage 5). See `promote_rehome_publish`.
        self.promote_rehome_publish(type_value, |h, shared| {
            h.runtime
                .declared_sigs
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .insert(sym, shared);
            h.runtime
                .declared_sigs_version
                .fetch_add(1, std::sync::atomic::Ordering::Release);
        });
    }

    /// The qualified key of the ONE alias (`deftype`) whose name ends in `/name` — the
    /// contract's fallback for a bare alias name checked away from where it was declared
    /// (KI-165) — or `None` when no alias, or two, answer. `is_alias` decides whether a
    /// store entry is an alias (the `%type` marker is the builtins layer's vocabulary).
    ///
    /// Memoised per process, keyed on the store's version: this is asked per checked VALUE
    /// under contracts, and the scan it answers with walks every declared signature in the
    /// runtime — 76 M instructions of a 1.3 G callgrind run were this scan, three times per
    /// `conj` (2026-09-21). A `deftype` (a store insert) bumps the version and drops the
    /// memo. Keys only, never handles: a compaction relocates the store's values, and the
    /// caller re-reads the form through the live store by this key.
    pub fn alias_key_by_suffix(
        &self,
        name: Symbol,
        is_alias: impl Fn(&Heap, Symbol) -> bool,
    ) -> Option<Symbol> {
        let version = self
            .runtime
            .declared_sigs_version
            .load(std::sync::atomic::Ordering::Acquire);
        {
            let cache = self.type_alias_cache.borrow();
            if cache.0 == version {
                if let Some(hit) = cache.1.get(&name) {
                    return *hit;
                }
            }
        }
        let suffix = format!("/{}", crate::core::value::symbol_name(name));
        let mut found = None;
        for (key, _) in self.declared_sigs_everywhere() {
            if !crate::core::value::symbol_name(key).ends_with(&suffix) {
                continue;
            }
            if is_alias(self, key) {
                if found.is_some() {
                    found = None;
                    break;
                }
                found = Some(key);
            }
        }
        let mut cache = self.type_alias_cache.borrow_mut();
        if cache.0 != version {
            cache.0 = version;
            cache.1.clear();
        }
        cache.1.insert(name, found);
        found
    }

    /// Every `(sig …)` declared so far, as `(qualified-name, type-expression)`.
    ///
    /// Exists for the startup image (ADR-218): declared sigs live here rather than in the
    /// globals table, so an image that snapshots only globals silently loses them, and the
    /// checker quietly falls back to inferring from the body — `expects int` becomes
    /// `expects number | map`. Weaker advice, no error, which is the worst shape of bug.
    pub fn declared_sigs_snapshot(&self) -> Vec<(Symbol, Value)> {
        self.runtime
            .declared_sigs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect()
    }

    /// Every declaration in BOTH stores — the prelude image's and the runtime's — as
    /// `(qualified-name, type-expression)`. What the checker's type-alias table is built
    /// from (`protocol::type_alias_table`): an alias declared in the prelude or std is in
    /// the prelude store, one from a loaded module in the runtime's, and a bare alias
    /// name has to be resolved against all of them.
    pub fn declared_sigs_everywhere(&self) -> Vec<(Symbol, Value)> {
        let mut out: Vec<(Symbol, Value)> = self
            .prelude
            .declared_sigs
            .iter()
            .map(|(k, v)| (*k, *v))
            .collect();
        out.extend(self.declared_sigs_snapshot());
        out
    }

    /// The raw type-expression `Value` a `(sig …)` declared for the qualified global
    /// `sym`, or `None`. The checker (`sig_of`) parses it to a signature and gives it
    /// precedence over primitive/curated/inferred sigs. See [`Heap::set_declared_sig`].
    pub fn declared_sig_value(&self, sym: Symbol) -> Option<Value> {
        self.runtime
            .declared_sigs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&sym)
            .copied()
            .or_else(|| self.prelude.declared_sigs.get(&sym).copied())
    }

    /// Restore the runtime's global bindings from a [`Heap::snapshot_globals`]
    /// snapshot, discarding every `def` made since it was taken — so a name `def`'d
    /// since the snapshot becomes unbound again, and a rebound name returns to its
    /// earlier value. The `def`'d code the bindings referenced is now unreachable and
    /// is reclaimed by the next RUNTIME compaction (which this call re-enables — see
    /// [`rt_collect_block`](Self::rt_collect_block) — after `snapshot_globals` suppressed it).
    ///
    /// `discard_scope` (KI-134, ADR-339): `None` replays every module load journalled since
    /// the snapshot over the restored table — `%isolate`'s contract. `Some(scope)` is the
    /// scratch-world contract (`%isolate-discard-loads`, which the stdlib image builder probes
    /// each module under): the loads written under isolate `scope` — this isolate's own thunk
    /// and whatever it spawned — are dropped from the journal and NOT replayed, while loads
    /// other processes made in the same window are replayed exactly as before, so a scratch
    /// probe cannot roll a bystander's module back either.
    ///
    /// Returns the names the replay re-bound — the module loads that survived — so the
    /// caller can keep the side facts a define carries beside its binding (the `defn-`
    /// privacy mark, the `defdyn` mark) for exactly those names when it restores its own
    /// snapshots of them. Without that a kept module came back with every private name
    /// public: the surface audit read forty `defn-` helpers as undocumented public API.
    pub fn restore_globals(
        &mut self,
        snapshot: GlobalsSnapshot,
        discard_scope: Option<u64>,
        own_scope: u64,
    ) -> Vec<Symbol> {
        // LIFO check: the live suppression depth must still equal what this snapshot set,
        // or snapshots were restored out of order and we'd release the wrong scope's
        // suppression (re-exposing an outer snapshot to KI-6). The newtype already rules
        // out restore-without-snapshot and double-restore; this catches reordering.
        debug_assert_eq!(
            self.rt_collect_block.get(),
            snapshot.block_depth,
            "restore_globals out of order — globals snapshots must be restored LIFO"
        );
        // Serialize with the registry read-modify-write (`registry_update`/`registry_cas`,
        // KI-22's lock) — KI-89. Those RMWs hold `registry_lock` from their read of the
        // registry global to their write; this swap did NOT take it, so a concurrent
        // registration could read `cur` before the swap and `env_define` its successor
        // after it — writing back a map computed from the PRE-restore table, i.e.
        // resurrecting every accumulated `*record-ids*`/`*impls*`/`*features*` entry
        // wholesale while the ordinary bindings beside them stayed rolled back. That
        // asymmetry (ids registered, constructors unbound) is exactly KI-89's orphaned
        // record ids, and one hit is sticky: the resurrected entries sit inside every
        // later snapshot. Measured before this lock: 1994 of 2000 isolate cycles against
        // a registering bystander resurrected the rolled-back entry. Under the lock a
        // racing RMW either completes first (and is wiped — the isolation contract) or
        // starts after (and reads the restored table). Lock order is registry_lock →
        // globals lock here and in every RMW, so no inversion.
        // Clone the Arc so the guard borrows a LOCAL, leaving `&mut self` free for the
        // journal replay below (the same shape `registry_update` uses).
        let rt = self.runtime.clone();
        let _registry = rt.registry_lock.lock().unwrap_or_else(|e| e.into_inner());
        if reg_trace_enabled() {
            eprintln!(
                "[reg] pid={:?} RESTORE scope={}",
                crate::process::current_pid(),
                crate::process::self_isolate_scope(),
            );
        }
        // KI-134 (ADR-339): the snapshot rolls back every write since it was taken,
        // including a MODULE LOAD's — this process's own first-use load inside the window,
        // or a concurrent process's — whose bindings and registrations other processes may
        // be using right now. Isolation rolls back what a test did, not what a load did: the
        // journalled load writes are replayed over the snapshot. A define is re-bound as it
        // was; a registry write is re-applied as the OPERATION it was, so the entries the
        // isolate itself registered stay rolled back (see `LoadWrite`).
        //
        // Replayed into a PRIVATE copy first, then installed with the ONE swap below. Readers
        // are not under this lock: swapping the bare snapshot in and replaying into the live
        // table left a window in which a worker's `conj` dispatched against an `*impls*`
        // table its record was no longer in — 4 of 12 three-file repro runs, the same
        // symptom this exists to remove. Copied out under the journal lock, applied after it
        // (the apply allocates and takes `promote_lock`); still under `registry_lock`, so no
        // registry read-modify-write interleaves with the rebuild.
        let mut table = snapshot.saved;
        let mut rebound: Vec<Symbol> = Vec::new();
        if let Some(t) = Self::global_trace_target() {
            let live = self.runtime.globals_read().get(&t).copied();
            self.global_trace(t, "restore: live value before", live);
            self.global_trace(t, "restore: snapshot value", table.get(&t).copied());
        }
        let replay: Vec<LoadWrite> = {
            let mut j = self
                .runtime
                .load_journal
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mark = snapshot.journal_mark;
            let mine = |scope: u64| discard_scope == Some(scope);
            // A DIRECT load frame's writes (KI-170) belong to the isolate that loaded the
            // file: replayed for everyone else's restore, so a bystander cannot roll the
            // loader's module back mid-use, and discarded — dropped from the journal too, or
            // a later bystander restore would resurrect them — by the loader's own. Without
            // this, the scoped test runner's per-file `%isolate` replayed the file's own
            // `defmodule` defs over its restore, and every test file's top-level def leaked
            // into the next (`nest::named_files_scoped`, `bare_names_test` — red on
            // `c8c58c41`).
            let own_direct = |scope: u64, direct: bool| direct && scope == own_scope;
            if let Some(t) = Self::global_trace_target() {
                let hits: Vec<String> = j
                    .entries
                    .iter()
                    .filter(|(_, _, _, w)| match w {
                        LoadWrite::Define { sym, .. } | LoadWrite::Registry { sym, .. } => {
                            *sym == t
                        }
                    })
                    .map(|(seq, scope, direct, _)| {
                        format!("seq={seq}/scope={scope}/direct={direct}")
                    })
                    .collect();
                eprintln!(
                    "[global] pid={:?} scope={} RESTORE journal: mark={} outstanding={} entries={} discard={:?} target-entries=[{}]",
                    crate::process::current_pid(),
                    crate::process::self_isolate_scope(),
                    mark,
                    j.outstanding,
                    j.entries.len(),
                    discard_scope,
                    hits.join(" ")
                );
            }
            let writes = j
                .entries
                .iter()
                .filter(|(seq, scope, direct, _)| {
                    *seq >= mark && !mine(*scope) && !own_direct(*scope, *direct)
                })
                .map(|(_, _, _, w)| w.clone())
                .collect();
            if discard_scope.is_some() {
                // The scratch window's own loads are gone for good: nobody may replay them.
                j.entries
                    .retain(|(seq, scope, _, _)| !(*seq >= mark && mine(*scope)));
            }
            // This isolate's own direct loads are over with its run: nobody may replay them.
            j.entries
                .retain(|(seq, scope, direct, _)| !(*seq >= mark && own_direct(*scope, *direct)));
            j.outstanding = j.outstanding.saturating_sub(1);
            if j.outstanding == 0 {
                j.entries.clear();
            }
            writes
        };
        if !replay.is_empty() {
            if reg_trace_enabled() {
                eprintln!(
                    "[reg] pid={:?} RESTORE replays {} module-load write(s)",
                    crate::process::current_pid(),
                    replay.len()
                );
            }
            for w in replay {
                match w {
                    LoadWrite::Define { sym, val } => {
                        // `val` is already a RUNTIME handle; the publish re-homes it into the
                        // current generation under the guard `env_define` uses.
                        self.promote_rehome_publish(val, |_h, shared| {
                            table.insert(sym, shared).is_some()
                        });
                        rebound.push(sym);
                    }
                    LoadWrite::Registry { sym, op, path, val } => {
                        let cur = table.get(&sym).copied().unwrap_or(Value::nil());
                        if let Some(next) = self.registry_next(cur, op, &path, val) {
                            self.promote_rehome_publish(next, |_h, shared| {
                                table.insert(sym, shared).is_some()
                            });
                        }
                    }
                }
            }
        }
        if let Some(t) = Self::global_trace_target() {
            self.global_trace(t, "restore: value after replay", table.get(&t).copied());
        }
        // Wholesale table swap — invalidate every stamped global inline cache. This one
        // bumps the code epoch too (ADR-217): a restore can *replace or remove* bindings
        // a compiled arm baked in, so it is a rebind in every sense that matters.
        //
        // The bumps happen while the write guard is still held (KI-193). Bumped after the
        // unlock, a reader could take the restored table (a module's names gone) and then
        // serve `*features*` from its cache at the old version, still listing the module:
        // `require-one` short-circuits on it, the `:use` imports nothing, and every bare use
        // in that file dies `unbound symbol`. That is the `[refer] imported NOTHING` shape.
        #[cfg(test)]
        super::table_swap_probe::begin(&self.runtime.version, &self.runtime.code_epoch);
        {
            let mut globals = self.runtime.globals_write();
            *globals = table;
            self.features_audit_table(&globals, "isolate restore");
            self.runtime.version.fetch_add(1, Ordering::Relaxed);
            self.runtime.code_epoch.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(test)]
        super::table_swap_probe::end(&self.runtime.version, &self.runtime.code_epoch);
        // Release the compaction suppression `snapshot_globals` took: the snapshot is no
        // longer outstanding, so a relocation can no longer strand it (KI-6).
        self.end_rt_collect_block();
        rebound
    }

    /// Walk to the global scope at the bottom of the frame chain.
    pub fn env_root(&self, env: EnvId) -> EnvId {
        let mut cur = env;
        loop {
            if cur == EnvId::GLOBAL {
                return EnvId::GLOBAL;
            }
            match self.env_frame(cur).parent {
                Some(p) => cur = p,
                None => return cur, // the prelude builder's local root
            }
        }
    }
}
