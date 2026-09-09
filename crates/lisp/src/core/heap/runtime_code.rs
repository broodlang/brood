//! The runtime's shared code region — what a runtime's processes hold in common.
//!
//! [`RuntimeCode`] is the mutable half of a runtime: the code `def`'d at run time (in the
//! append-only RUNTIME `CodeSlabs`, two generations of them per ADR-091) plus the global
//! bindings table every process consults on every global reference. All of a runtime's
//! inner processes share one behind an `Arc`, which is what makes a `def` visible to a
//! *running* process on its next lookup — and what keeps separate runtimes (nodes)
//! independent, since each has its own.
//!
//! Also here: the `Symbol`-keyed map machinery that table is built from ([`SymbolHasher`]
//! and friends), the name-registry vocabulary a `def` records ([`RegistryOp`],
//! [`NameMeta`], [`SourceLoc`]), [`GlobalsSnapshot`] for `%isolate`'s roll-back, and
//! [`GenPin`], the guard that keeps a generation alive while a handle into it is in play.

use super::*;

/// A fast hasher for `Symbol` (`u32`) keys. The globals table is consulted on
/// every global reference (every operator / prelude call), and the default
/// SipHash is overkill — and notably slow to finalize — for a single `u32`.
/// FxHash-style: one wrapping multiply per key. `write_u32` is the only path that
/// runs for a `Symbol`, and multiplying by an odd constant is a bijection, so
/// distinct symbols never collide.
#[derive(Default)]
pub struct SymbolHasher(u64);

impl std::hash::Hasher for SymbolHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.0 = (self.0 ^ i as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    #[inline]
    fn write_u64(&mut self, i: u64) {
        // The hot path for a `VmCacheKey` (its handle `.0`): same odd-multiply
        // bijection as `write_u32`, so distinct handles never collide.
        self.0 = (self.0 ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write(&mut self, bytes: &[u8]) {
        // Fallback for any non-`u32` key (none on the hot path); kept correct.
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// A `HashMap` keyed by interned `Symbol`s, using the fast [`SymbolHasher`].
pub type SymbolMap<V> = HashMap<Symbol, V, std::hash::BuildHasherDefault<SymbolHasher>>;

/// A `HashMap` keyed by [`VmCacheKey`], using the fast [`SymbolHasher`] (its
/// manual `Hash` writes a single `u64`, so it takes the `write_u64` fast path).
/// The compiling VM hits this on **every closure call** (`compiled_for`), so the
/// stock `SipHash` was pure per-call overhead (perf #2).
pub type VmCacheMap<V> = HashMap<VmCacheKey, V, std::hash::BuildHasherDefault<SymbolHasher>>;

/// The [`Heap::lookup_closure_template`] cache map: `fn_rest` [`PairId`] → parsed
/// [`ClosureTemplate`] plus a **sighting count**, on the fast [`SymbolHasher`] (a `PairId`
/// writes one `u64`). The count is what gates the const-closure promote — see
/// [`Heap::lookup_closure_template`].
pub(super) type ClosureTemplateMap =
    HashMap<PairId, (Arc<ClosureTemplate>, u32), std::hash::BuildHasherDefault<SymbolHasher>>;

/// The [`Heap::lookup_const_closure`] cache map: a capture-free `(fn …)` literal's
/// `fn_rest` [`PairId`] → the **promoted RUNTIME closure handle** built for it once.
pub(super) type ConstClosureMap =
    HashMap<PairId, Value, std::hash::BuildHasherDefault<SymbolHasher>>;

/// `BROOD_REG_TRACE=1` — name every registry write (pid, registry, op, first key) and
/// every globals restore on stderr. The tool for attributing a leaked or orphaned
/// registration to its writer (KI-89's class: WHO registered this id, and did a restore
/// land between its read and its write?). One cached bool when off.
pub(super) fn reg_trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_REG_TRACE").is_some())
}

/// Which update [`Heap::registry_update`] performs. See that method for why the whole
/// read-modify-write has to happen inside one kernel call (KI-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryOp {
    /// Set `path` to the value, creating the intermediate map if needed.
    Assoc,
    /// Set `path` only if it is currently absent.
    AssocNew,
    /// Remove a one-key `path`.
    Dissoc,
    /// Prepend to a list-valued global unless already a member.
    ConsNew,
}

/// What a `(meta name …)` form records about a global (ADR-283): three independent facts,
/// each optional, each spent in a different place. `since` is documentation only; `deprecated`
/// drives an advisory checker diagnostic naming `use_instead`; `beta` warns that a surface is
/// not settled. Held as owned data rather than a heap `Value` so it is independent of any one
/// process's heap, the way `Sig` is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameMeta {
    /// The version this name first appeared in.
    pub since: Option<String>,
    /// The version it was deprecated in.
    pub deprecated: Option<String>,
    /// What to use instead — the half that makes a deprecation actionable rather than
    /// merely discouraging.
    pub use_instead: Option<Symbol>,
    /// Why this surface is not settled yet.
    pub beta: Option<String>,
}

/// A runtime's mutable, shared code region: the code `def`'d at runtime plus the
/// global bindings table. All of a runtime's inner processes share one of these
/// (via `Arc::clone`), which is what makes a `def` propagate to them — and what
/// keeps separate runtimes (nodes) independent (each has its own).
pub struct RuntimeCode {
    /// The **two** code generations (ADR-091 Erlang-style 2-generation collector).
    /// New code (`def`/`promote`) lands in `gens[current_gen]`; the *other* slot
    /// holds the previous generation's still-referenced code during a migration
    /// (freed once no live process references it). A RUNTIME handle self-describes
    /// its generation ([`code_gen`](crate::core::value::PairId::code_gen)), so a
    /// read resolves `gens[handle.code_gen()]` — no shared read on the hot path.
    /// Until aging is wired, `current_gen` stays `0` and `gens[1]` is empty, so this
    /// behaves exactly like the former single `code: CodeSlabs`.
    ///
    /// Each slot is an [`ArcSwap`] so a drained generation can be **freed while the
    /// runtime is shared** (ADR-091 Stage 4): [`Heap::free_runtime_gen`] stores a
    /// fresh empty `CodeSlabs`, and the old `Arc` drops once the last reader releases
    /// its [`Guard`]. Reads stay lock-free (`gens[g].load()`); appends push into the
    /// loaded slab's `boxcar` in place (visible to every holder of that `Arc`), so a
    /// store only ever happens on a free — never on the `def`/`promote` hot path.
    pub(super) gens: [ArcSwap<CodeSlabs>; 2],
    /// Index (0 or 1) of the current code generation — read at `promote`/collect,
    /// never on the hot read path (the handle carries its own generation).
    pub(super) current_gen: AtomicUsize,
    /// A process-wide-unique tag for this runtime instance — a plain `u64` the
    /// background JIT compiler keys its per-runtime publish cache by. Carried
    /// inside compile work items INSTEAD of the runtime `Arc` (or a `Weak`):
    /// either would park a reference in the queue and break the single-process
    /// RUNTIME compactor's `Arc::get_mut` uniqueness gate. Distinct per runtime,
    /// so shared native code never leaks across independent runtimes. (Read only
    /// by the JIT publish path — dead in a no-`jit` build, but kept unconditional
    /// so the struct layout and construction don't fork on the feature.)
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(super) runtime_tag: u64,
    /// Per-generation count of **in-flight shared-closure messages** — a queued
    /// `Message::FnShared` holding a RUNTIME handle into that generation.
    ///
    /// Why a counter rather than the reachability probe. A shared handle that has *landed*
    /// in a receiver's LOCAL heap is already sound: the drain's Phase 2 walks the whole
    /// local heap, so `runtime_gen_referenced` sees it (this is ADR-194's argument for the
    /// L1 path). A handle **still queued** is in no heap and no process's roots, so nothing
    /// walks it — and it cannot be found by extending the probe either, because
    /// `report_gen_liveness` caches a process's clean ack for the whole epoch on the
    /// explicit grounds that *"an old-gen handle can never arrive by message (messages
    /// deep-copy)"*. This counter restores that guarantee from the other side: while a
    /// generation has messages in flight against it, `free_runtime_gen` refuses. The pin is
    /// released by `GenPin`'s `Drop`, so every path that discards a message — a dead target,
    /// a dropped mailbox, a routing failure — releases it without a manual decrement.
    pub(super) gen_inflight: [AtomicUsize; 2],
    /// Monotonic version of the `gens` **`Arc` identities**, bumped only when a slot's
    /// `Arc<CodeSlabs>` is *replaced* — a Stage-4 free or a compaction store, both rare
    /// (never on the `def`/`promote`/append hot path, which mutates a loaded slab's
    /// `boxcar` in place without swapping the `Arc`). It gates the per-process pinned
    /// read cache ([`Heap::code_gen_pinned`]): a RUNTIME deref clones the *cached* `Arc`
    /// when this version is unchanged, avoiding the `ArcSwap::load` hybrid-strategy cost
    /// that dominated global-data-heavy hot loops. An aging flip changes `current_gen`
    /// but not either slot's `Arc`, so it deliberately does **not** bump this — a cached
    /// pin stays valid across it (a handle carries its own generation index). `Relaxed`
    /// suffices: the cache re-`load_full`s on any change, which republishes the `Arc`.
    pub(super) gen_version: AtomicU64,
    /// The global bindings (prelude + user `def`s). Read on every global lookup,
    /// written on `def` (the only mutation). The values point into PRELUDE or RUNTIME.
    pub(super) globals: RwLock<SymbolMap<Value>>,
    /// Per-global **rebinding generation**: the value of `version` at each name's most
    /// recent `def`. Answers "is the binding I wrote still the one that is bound?" —
    /// what a temporary rebinding (`debug/trace-fn`'s wrapper, `nest test --cover`'s
    /// shim) must ask before *restoring*, or it overwrites a redefinition the user made
    /// in between. Comparing handles cannot answer it (a `def` promotes the closure, so
    /// the bound handle is not the local one); a generation can. Read by
    /// `%global-generation`.
    pub(super) global_generations: RwLock<SymbolMap<u64>>,
    /// Serialises a **registry update** — the read-modify-write of a global that holds a
    /// whole registry map (`*impls*`, `*features*`, `*abilities*`, … — see
    /// [`Heap::registry_update`]). `def` itself is atomic, but `(def *X* (assoc *X* …))` is
    /// three steps in the language, and two processes registering at once each read the old
    /// map and each write their own successor, so the later write silently drops the
    /// earlier one (KI-22: ~40% of concurrent registrations lost). This lock lets the whole
    /// sequence happen inside ONE kernel call.
    ///
    /// Separate from `globals` on purpose: the update needs `&mut Heap` for the map ops
    /// between the read and the write, which it could not do while holding a guard borrowed
    /// from `self`. Nothing acquires this while holding the `globals` lock, so there is no
    /// ordering hazard. Registration is a load-time/hot-reload event, so the contention is
    /// nil and holding it briefly on the worker thread is free.
    ///
    /// It guards a *value*, not `()`: the set of globals a registry update has actually
    /// written, which [`Heap::registry_names`] reports. A registry is precisely a global that
    /// loading MUTATES rather than creates, so the `(reflect/global-names)` diff a startup image is
    /// built from cannot see it — and a registry left out of the image is lost with no error
    /// (ADR-218). Naming them by hand went stale three times; this set is derived from the writes
    /// themselves, so a registry added later is carried without anyone remembering to.
    /// Recorded on the write path only, under the lock already held, so it costs one
    /// `HashSet` insert per registration and nothing at all per lookup.
    pub(super) registry_lock: Mutex<HashSet<Symbol>>,
    /// **Reserved** names — everything the language itself ships, which a user `def`
    /// may not rebind (ADR-166). Seeded with every symbol bound at runtime-seed time
    /// (the prelude's 443 definitions plus every Rust builtin), and extended with each
    /// name an *embedded* std module defines as it loads. The rule the boundary
    /// encodes: **if it shipped inside the `brood` binary it is reserved; if you or a
    /// package author wrote it, it is yours** — so hot-reloading your own code, and a
    /// dependency's, is untouched, which is all the live-editing story ever needed.
    ///
    /// Read only when a global `def` runs (rare), so a `HashSet` probe costs nothing
    /// on any hot path. Shared through the runtime `Arc`, so every inner process sees
    /// one reserved set.
    pub(super) sealed: RwLock<std::collections::HashSet<Symbol>>,
    /// Module-private globals (ADR-146): the qualified [`Symbol`] of every global
    /// defined with `defn-`/`def-`. Privacy is a **recorded fact** declared by the
    /// def FORM, not derived from the name — the name is clean (no `--` marker), so
    /// this set is the ONLY authority and `is_private` MUST consult it for every
    /// name (there is no "name without `--`" fast-negative; adding one would silently
    /// make every clean private public). Populated by the `%mark-private` primitive a
    /// `defn-`/`def-` emits (and `unmark_private` on any plain def, so privacy tracks
    /// the latest def form across hot reload); the prelude's privates are seeded from
    /// the builder heap in [`RuntimeCode::seeded`] (the prelude is inserted, not
    /// re-`eval`ed). Shared through the runtime `Arc`, so every inner process sees one
    /// set.
    pub(super) private: RwLock<std::collections::HashSet<Symbol>>,
    /// **Stability metadata** per global (ADR-283): when a name appeared, whether it is
    /// deprecated and what replaces it, whether it is beta. Recorded by the `%register-meta`
    /// primitive a `(meta …)` form emits, and cleared by `env_define` on any redefinition —
    /// the same rule privacy follows, and for the same reason: a `def` that rebinds a name
    /// mid-run must not leave the OLD name's "deprecated" fact attached to the new one
    /// (ADR-013's late binding applies to the facts about code, not only to the code).
    pub(super) meta: RwLock<SymbolMap<NameMeta>>,
    /// Monotonic version of `globals`, bumped on every binding change (`def`
    /// rebind, `restore_globals`). Per-process global **inline caches**
    /// (`Heap::global_ic`) stamp the version they resolved at and re-resolve only
    /// when it has moved — so a steady-state global read is an atomic load + a
    /// local hash hit instead of taking the shared `RwLock`. Late-binding stays
    /// exact: any `def` makes every stamped cache entry stale at once. `Relaxed`
    /// is sufficient — a global value is an immovable PRELUDE/RUNTIME handle, so
    /// there's no data it gates publication of; the counter only has to *change*.
    pub(super) version: AtomicU64,
    /// Monotonic **code** epoch — [`Heap::global_epoch`], the one the JIT guards on
    /// (ADR-217). Bumped by everything `version` is bumped by *except a `def` that
    /// binds a name for the FIRST time*, which cannot invalidate compiled code:
    ///
    /// - an inlined prim requires `resolve_prim`'s `env_get(global, head)?` to have
    ///   succeeded, so its head was already bound at compile time;
    /// - an entry-hoisted global that is unbound *deopts* rather than baking a value,
    ///   and the hoist re-resolves on every activation anyway;
    /// - `env_get` resolves a global by a single flat symbol lookup — namespace
    ///   resolution happened before compilation — so a new binding of some *other*
    ///   symbol can never redirect a symbol an arm already holds.
    ///
    /// Every dependency a compiled arm can have therefore already existed when it
    /// compiled, and any change to one is a **rebind**, which does bump this.
    ///
    /// Split from `version` because the two have opposite cost profiles: re-resolving
    /// a stale global IC is a hash hit, while a stale `compile_epoch` throws away
    /// native code and re-tiers the arm. Sharing one counter meant a bulk load —
    /// which is nothing but first-time `def`s — invalidated every JIT'd arm ~100
    /// times per module: 12 distinct arms re-lowered 2294 times each over a
    /// 4000-module load, and the JIT came out a net 43% *loss* against no JIT at all.
    /// `version` keeps its exact old meaning for the inline caches.
    pub(super) code_epoch: AtomicU64,
    /// Where each global was *defined* — file + form position, recorded at load
    /// time before macroexpansion (ADR-031). Lives here, beside `globals`, so it
    /// is shared across a runtime's processes and updated by a redefinition, the
    /// same as the bindings it describes. Read by `(source-location 'name)`; the
    /// image-query foundation for cross-file goto-definition.
    pub(super) def_sites: RwLock<HashMap<Symbol, SourceLoc>>,
    /// Source positions of RUNTIME *list forms*, keyed by [`rt_pos_key`] — the pair's
    /// **`(code_gen, slab index)`** — the RUNTIME counterpart of the per-heap LOCAL
    /// [`Heap::form_pos`] map. The reader stamps positions on LOCAL pairs; `promote`
    /// carries them here when a form is frozen into RUNTIME (a `defn` body, or a
    /// top-level inline lambda baked for VM-compilation), so `(form-pos …)` still
    /// resolves and a position survives a cross-node send (`Message::List`). Shared
    /// across the runtime's processes via `Arc`.
    ///
    /// **The generation is part of the key** (ADR-091). The two generations share one
    /// index space, so a bare slab index conflates gen-0 #5 with gen-1 #5: once aging is
    /// active, a `def` into the fresh generation silently overwrote the recorded position
    /// of a live form in the retained one, and `(form-pos …)` / an error message on gen-0
    /// code reported a stranger's line. The same conflation also made
    /// [`Heap::free_runtime_gen_locked`]'s reclamation invisible here — a freed
    /// generation's leftovers aliased newly-minted pairs at reused indices and
    /// accumulated forever. Keying by generation fixes the first and lets the free purge
    /// exactly its own entries (the KI-7/KI-8 class).
    pub(super) positions: RwLock<HashMap<u64, FormPos>>,
    /// Shared JIT native-code cache (ADR-101, the spawn lever): maps a simple
    /// fixed-arity RUNTIME/PRELUDE closure arm's `(closure_id, argc)` key (see
    /// `CompiledArm::share_key`) to its compiled native code as
    /// `(code_ptr_as_usize, compile_epoch)`. The first process to JIT such an arm
    /// publishes here; every other process of this runtime installs the pointer
    /// directly (epoch-checked) instead of re-tiering + recompiling its own copy — so
    /// a hot shared function (`fib` under `spawn`) compiles to native ONCE, not once
    /// per process (the spawn-14× cause). The code lives in the process-lifetime
    /// GLOBAL_JIT module (never freed or moved), so the raw pointer is valid across
    /// processes/threads; the `compile_epoch` is checked against `version` (this
    /// struct's `global_epoch`) on install, so a `def` or RUNTIME compaction — both
    /// bump `version` — invalidates every entry without a sweep. Stored as `usize`
    /// because a raw code pointer isn't `Send`/`Sync`; reconstituted on read. Empty
    /// unless the JIT runs.
    pub(super) jit_code_cache: RwLock<HashMap<(u64, u16), (usize, u64)>>,
    /// **Shared compiled-closure cache** (ADR-175 Phase B — the BEAM module-area move):
    /// PRELUDE closure handle bits → the compiled closure, shared by every process of
    /// this runtime. Before this, each green process compiled its own copy of every
    /// prelude function it called (~18 KB per distinct callee per process — the
    /// spawn-live 4.5 GB cause). Eligibility is strict (see `compiled_arm_for`):
    /// PRELUDE-region key (never freed/recycled, so no ADR-091 free-epoch discipline
    /// needed here) and **immortal** arms (no RUNTIME-region handle anywhere, so
    /// `runtime_collect`'s per-process rewrite never touches them — a shared arm
    /// rewritten by two processes would double-forward its handles). Publish is
    /// idempotent: every process compiles the identical closure from the same shared
    /// AST, so last-writer-wins is safe. `BROOD_NO_SHARED_ARMS=1` bypasses (ADR-175's
    /// off-switch). Arm site ids are arm-relative (Phase A), so a shared arm's ICs
    /// work in every process, each against its own block.
    /// Value is `(free_epoch_at_compile, closure)`. The stamp is read **before** the
    /// publisher compiles and validated against the live `free_epoch` on lookup, so a
    /// closure compiled against a generation that was freed mid-compile can never be
    /// installed by anyone (ADR-091: a freed slot is reused with bit-identical
    /// `(gen, index)` handles, which is exactly what the per-process `vm_cache` guards
    /// with `sync_free_epoch`).
    pub(super) shared_closures:
        RwLock<HashMap<u64, (u64, Arc<crate::eval::compile::CompiledClosure>)>>,
    /// Companion to `jit_code_cache` for the two-stage-tiering **inlined** upgrade
    /// (the deferred, self-inlined body). Same `(closure_id, argc)` key and
    /// `(code_ptr, compile_epoch)` value, but a separate map because a slot holds
    /// either the small native (that cache) or the inlined native (this one), never
    /// both. Sharing the inlined native across a runtime's processes — exactly as the
    /// small native already is — means ONE inlined compile serves every process instead
    /// of each of N spawned workers compiling (and, for a short fan-out like `pfib`,
    /// finishing before) its own copy; the inlined win then lands for short parallel
    /// bursts too. Safe because `inline_nslots` is deterministic for a given bytecode
    /// (so a peer sizes its own frame correctly on install) and the epoch guard flushes
    /// it on `def`/compaction just like the small-native cache. See
    /// [`Heap::jit_inline_lookup`] / [`Heap::jit_inline_publish`].
    pub(super) jit_inline_cache: RwLock<HashMap<(u64, u16), (usize, u64)>>,
    /// User-declared `(sig name type)` signatures, keyed by the **module-qualified**
    /// global `Symbol` (the same key `def` produces for `name`) and holding the raw
    /// type-expression as a promoted RUNTIME `Value` (e.g. the `(int -> int)` form).
    /// Registered by the `%register-sig` primitive when a `(sig …)` form evaluates,
    /// so a declared sig is visible to the checker's `sig_of` *first* — ahead of
    /// primitive/curated/inferred — both intra-module (the call resolves to the
    /// qualified name the file-local ctx misses) and cross-module (`nest check`
    /// loads the whole project image, so b's sig is present when a's caller is
    /// checked). The stored value is a `Value`, not a `types::Sig`: the `core`
    /// layer must not depend on `types` (the checker parses it on read). Shared
    /// across the runtime's processes via `Arc`, like `globals`.
    pub(super) declared_sigs: RwLock<SymbolMap<Value>>,
    /// **RUNTIME collector — Stage 3b (cooperative drain coordination, ADR-091).**
    /// When an aged-out generation is being reclaimed, each of the runtime's
    /// processes cooperatively reports — at its safepoint / before parking —
    /// whether it still references the draining generation
    /// ([`Heap::runtime_gen_referenced`]). The old generation is dead (Stage 4 may
    /// free it) only once *every* live process has reported clean for the current
    /// drain epoch. Shared across the runtime's processes via `Arc`, like `globals`.
    ///
    /// `drain_active` is `false` when no drain is in progress (the always-case until
    /// Stage 4 arms one, so the whole mechanism is inert by default). `drain_gen` is
    /// the generation being reclaimed. `drain_epoch` is **strictly monotonic** (a new
    /// drain bumps it and clears `drain_acks`), so a stale ack from a previous drain
    /// can never be mistaken for a current-epoch one. `drain_acks` maps a process's
    /// pid → the epoch it last reported *clean* for; a process still referencing the
    /// draining generation has no current-epoch entry, so it pins the generation.
    pub(super) drain_active: AtomicBool,
    pub(super) drain_gen: AtomicUsize,
    pub(super) drain_epoch: AtomicU64,
    /// **O(1) drain-completion gate (ADR-091).** A running count of *distinct*
    /// processes that have reported clean for the current drain epoch (reset to 0
    /// by `begin_gen_drain`, bumped once per new ack in `report_gen_liveness`). The
    /// process layer's `old_gen_drained` compares it to the live-process count as a
    /// cheap gate: while `drain_acked < live` some process still pins the generation,
    /// so it skips the O(live-process) parked-liveness registry scan + mailbox-lock
    /// sweep entirely — the whole cost of a lingering drain (a `spawn` fan-out where
    /// every child's body pins the draining gen made this ~300× at scale). It only
    /// grows within an epoch (an acked process that later exits is not decremented),
    /// which is sound: the count can only *over*-report completion, and the actual
    /// free is still gated by the authoritative `gen_drained` scan below the gate —
    /// so a stale count can at worst run the scan a bit early (never free early).
    pub(super) drain_acked: AtomicU64,
    /// RUNTIME-churn dirty bit: set true whenever a closure is minted into the
    /// current code generation (`promote_closure` — i.e. every `def`/`spawn`/
    /// hot-reload `promote`). The eval safepoint reads it to decide whether the
    /// (relatively costly) `rt_gc_due` probe — an `ArcSwap` load + a closure count
    /// — is worth running: the RUNTIME region only grows on a mint, which never
    /// happens inside a hot compute loop, so a def-free loop (`fib`, `reduce`,
    /// `apply`) skips the probe entirely. Cleared once the safepoint has run the
    /// probe. A plain relaxed `bool`: a read keeps the cache line Shared across
    /// worker cores (no invalidation), a mint writes it once.
    pub(super) rt_dirty: AtomicBool,
    pub(super) drain_acks: RwLock<HashMap<u64, u64>>,
    /// **RUNTIME collector — Stage 4 (free-generation epoch, ADR-091).** Bumped each
    /// time a generation is freed ([`Heap::free_runtime_gen`]). A freed slot is later
    /// reused by aging, minting handles with bit-identical `(gen, index)` to the freed
    /// ones — so a per-process `vm_cache` entry (keyed on the closure handle bits, not
    /// version-stamped) could otherwise alias *old* compiled code onto *new* code. Each
    /// process compares this against its own [`Heap::seen_free_epoch`] on the
    /// `vm_cache` read path and clears its `vm_cache` once when it advances. The
    /// version-stamped caches (`global_ic`, the call/global ICs, the shared JIT caches)
    /// self-invalidate on the `version` bump a free also does, so only `vm_cache` needs
    /// this. Relaxed: it only has to *change* (a lazy one-shot cache clear, no data
    /// publication gated on it — the freed slab is already unreachable by the drain).
    pub(super) free_epoch: AtomicU64,
    /// **RUNTIME collector — Stage 4 (single-flight aging, ADR-091).** Held for the
    /// duration of an `age + migrate_live_globals + begin_gen_drain` sequence so at
    /// most one process ages at a time. Two processes racing the safepoint could both
    /// observe the other slot empty and both run the migration, double-copying the
    /// live image into the new generation (wasteful, and the second's reconcile would
    /// mostly no-op). A plain CAS gate ([`Heap::begin_aging`]/[`Heap::end_aging`]) —
    /// the loser skips this safepoint and retries at the next one.
    pub(super) aging: AtomicBool,
    /// **RUNTIME collector — Stage 4 (aging counter, ADR-091).** Bumped by
    /// [`Heap::age_runtime`]; surfaced via [`Heap::runtime_aged_count`] so a test can
    /// confirm the multi-generation collector aged, even when a full free is timing-
    /// dependent.
    pub(super) aged_count: AtomicU64,
    /// **RUNTIME collector — Stage 4 (promote⇄age mutual exclusion, ADR-091).** A
    /// generation flip ([`Heap::age_runtime`]) must not interleave with an in-flight
    /// [`Heap::promote`] on another process: promote reserves a slot in the current
    /// generation and then fills it, re-reading `cur_code()` — if aging flipped
    /// `current_gen` in between, the fill would target the *wrong* generation's slab
    /// (a panic or cross-generation-split closure). Promotion holds this **read** lock
    /// (many concurrent promotes are fine — they append to a lock-free `boxcar`);
    /// aging holds the **write** lock, so the flip waits for every in-flight promote to
    /// finish and no promote ever spans it. Uncontended on the default single-generation
    /// path (nothing ever ages), so it's a bare read-lock acquire per `def`/`spawn`.
    pub(super) promote_lock: RwLock<()>,
}

/// Where a global was defined: the file, and the start position of its
/// `def`/`defn`/`defmacro` form. Captured pre-macroexpansion so `defn`/`defmacro`
/// definitions are located accurately (ADR-031).
#[derive(Clone, Debug)]
pub struct SourceLoc {
    pub file: String,
    pub pos: crate::error::Pos,
}

/// A rolled-back-on-restore snapshot of the runtime globals, plus the RUNTIME-compaction
/// suppression it holds (KI-6). Constructed **only** by [`Heap::snapshot_globals`] — and
/// the sole argument type [`Heap::restore_globals`] accepts — so the snapshot↔restore
/// protocol can't be misused: a restore can't run without a paired snapshot (no way to
/// forge one), and `restore_globals` takes it *by value* so the same snapshot can't be
/// restored twice. `#[must_use]`: dropping a snapshot without restoring it leaves the
/// globals mutated AND compaction suppressed, so the compiler flags an ignored one.
#[must_use = "a globals snapshot must be handed to heap.restore_globals — dropping it \
              leaves the globals table mutated and RUNTIME compaction suppressed (KI-6)"]
pub struct GlobalsSnapshot {
    pub(super) saved: SymbolMap<Value>,
    /// The `rt_collect_block` depth this snapshot established (post-increment). Restore
    /// asserts the live depth still matches — catching an out-of-order (non-LIFO) restore,
    /// which would release the wrong scope's suppression.
    pub(super) block_depth: u32,
}

/// An RAII pin on one RUNTIME generation, held by an in-flight `Message::FnShared` so the
/// generation cannot be freed while a shared handle into it is queued but not yet landed in
/// any heap (see [`RuntimeCode::gen_inflight`]).
///
/// Deliberately RAII rather than a manual increment/decrement pair. A message is dropped on
/// several paths that are easy to miss — an unknown or dead target, a mailbox torn down, a
/// routing failure — and a *leaked* pin is the worst possible failure here: the generation is
/// never reclaimed, so the region grows without bound, silently, which is the very class of
/// bug this whole change exists to fix. Making the release structural means the compiler
/// enforces it instead of a reviewer.
pub struct GenPin {
    pub(super) runtime: Arc<RuntimeCode>,
    pub(super) gen: usize,
}

impl GenPin {
    pub(super) fn new(runtime: Arc<RuntimeCode>, gen: usize) -> Self {
        runtime.gen_inflight[gen].fetch_add(1, Ordering::AcqRel);
        GenPin { runtime, gen }
    }
}

impl Clone for GenPin {
    fn clone(&self) -> Self {
        GenPin::new(Arc::clone(&self.runtime), self.gen)
    }
}

impl Drop for GenPin {
    fn drop(&mut self) {
        self.runtime.gen_inflight[self.gen].fetch_sub(1, Ordering::AcqRel);
    }
}

impl std::fmt::Debug for GenPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GenPin(gen={})", self.gen)
    }
}

impl Heap {
    /// Pin the generation `id` lives in, for as long as the returned guard is held. Returns
    /// `None` for a non-RUNTIME handle (PRELUDE is never freed; LOCAL is not shareable).
    pub fn pin_gen_of(&self, id: crate::core::value::ClosureId) -> Option<GenPin> {
        if id.region() != RUNTIME {
            return None;
        }
        Some(GenPin::new(Arc::clone(&self.runtime), id.code_gen()))
    }

    /// Are there shared-closure messages in flight against generation `gen`?
    pub fn gen_has_inflight(&self, gen: usize) -> bool {
        self.runtime.gen_inflight[gen].load(Ordering::Acquire) != 0
    }

    /// Forget this process's cached "clean" drain ack, forcing it to re-walk on its next
    /// safepoint.
    ///
    /// `report_gen_liveness` caches the ack for a whole epoch, justified by "an old-gen
    /// handle can never arrive by message (messages deep-copy)". Materialising a
    /// `Message::FnShared` breaks exactly that: this heap may now hold a handle into the
    /// draining generation, and a stale clean ack would let the collector free it. Called
    /// only on that path, so the fan-out drain cost the caching was introduced to fix is
    /// unchanged for every other message.
    pub fn rearm_drain_ack(&self) {
        // `0` is the "never acked" sentinel the constructors use; a real epoch is >= 1.
        self.acked_drain_epoch.set(0);
    }
}

/// The next [`RuntimeCode::runtime_tag`] — a process-wide monotonic counter.
fn next_runtime_tag() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Default for RuntimeCode {
    fn default() -> Self {
        RuntimeCode {
            gens: [
                ArcSwap::from_pointee(CodeSlabs::default()),
                ArcSwap::from_pointee(CodeSlabs::default()),
            ],
            current_gen: AtomicUsize::new(0),
            runtime_tag: next_runtime_tag(),
            gen_inflight: [AtomicUsize::new(0), AtomicUsize::new(0)],
            gen_version: AtomicU64::new(0),
            globals: RwLock::new(SymbolMap::default()),
            global_generations: RwLock::new(SymbolMap::default()),
            meta: RwLock::new(SymbolMap::default()),
            registry_lock: Mutex::new(HashSet::new()),
            // A default (un-seeded) runtime reserves nothing — the prelude hasn't run.
            sealed: RwLock::new(std::collections::HashSet::new()),
            // Likewise no private names until the prelude has been seeded.
            private: RwLock::new(std::collections::HashSet::new()),
            version: AtomicU64::new(0),
            code_epoch: AtomicU64::new(0),
            def_sites: RwLock::new(HashMap::new()),
            positions: RwLock::new(HashMap::new()),
            jit_code_cache: RwLock::new(HashMap::new()),
            shared_closures: RwLock::new(HashMap::new()),
            jit_inline_cache: RwLock::new(HashMap::new()),
            declared_sigs: RwLock::new(SymbolMap::default()),
            drain_active: AtomicBool::new(false),
            rt_dirty: AtomicBool::new(true),
            drain_gen: AtomicUsize::new(0),
            drain_epoch: AtomicU64::new(0),
            drain_acked: AtomicU64::new(0),
            drain_acks: RwLock::new(HashMap::new()),
            free_epoch: AtomicU64::new(0),
            aging: AtomicBool::new(false),
            aged_count: AtomicU64::new(0),
            promote_lock: RwLock::new(()),
        }
    }
}

impl RuntimeCode {
    /// The current code generation's index (0 or 1). Where new code lands.
    #[inline]
    pub(super) fn cur_gen(&self) -> usize {
        self.current_gen.load(Ordering::Relaxed)
    }
    /// A guard on the current code generation's slabs — the target of `promote`/`def`
    /// and the region the single-process compactor operates on. Derefs to
    /// `&CodeSlabs`; hold it (don't re-call) across a multi-step read so the slab
    /// can't be freed mid-use.
    #[inline]
    pub(super) fn cur_code(&self) -> Guard<Arc<CodeSlabs>> {
        self.gens[self.cur_gen()].load()
    }
    // Append a value into the *current* code generation and mint a handle tagged
    // with that generation, so a read later resolves the right slab (2-generation
    // collector, ADR-091). Centralised so every RUNTIME mint is gen-tagged the same
    // way — the push slab and the handle's `code_gen` can never disagree.
    #[inline]
    pub(super) fn push_str(&self, v: String) -> StrId {
        let g = self.cur_gen();
        StrId::runtime_gen(self.gens[g].load().strings.push(LocalString::inline(v)), g)
    }
    #[inline]
    pub(super) fn push_bigint(&self, v: num_bigint::BigInt) -> BigIntId {
        let g = self.cur_gen();
        BigIntId::runtime_gen(self.gens[g].load().bigints.push(v), g)
    }
    #[inline]
    pub(super) fn push_decimal(&self, v: bigdecimal::BigDecimal) -> DecimalId {
        let g = self.cur_gen();
        DecimalId::runtime_gen(self.gens[g].load().decimals.push(v), g)
    }
    #[inline]
    pub(super) fn push_ratio(&self, v: num_rational::BigRational) -> RatioId {
        let g = self.cur_gen();
        RatioId::runtime_gen(self.gens[g].load().ratios.push(v), g)
    }
    #[inline]
    pub(super) fn push_bytes(&self, v: Arc<SharedBlob>) -> BytesId {
        let g = self.cur_gen();
        BytesId::runtime_gen(self.gens[g].load().bytes.push(v), g)
    }
    #[inline]
    pub(super) fn push_rope(&self, v: ropey::Rope) -> RopeId {
        let g = self.cur_gen();
        RopeId::runtime_gen(self.gens[g].load().ropes.push(v), g)
    }
    #[inline]
    pub(super) fn push_vec(&self, v: VecStore) -> VecId {
        let g = self.cur_gen();
        VecId::runtime_gen(self.gens[g].load().vectors.push(v), g)
    }
    /// A fresh runtime whose global table is seeded with the prelude bindings
    /// (`symbol -> prelude value`). The code slabs start empty — user `def`s
    /// append to them. Inner processes share this whole thing via `Arc`.
    pub fn seeded(
        bindings: &[(Symbol, Value)],
        prelude_private: &[Symbol],
        prelude_meta: &[(Symbol, NameMeta)],
    ) -> Self {
        let mut globals = SymbolMap::with_capacity_and_hasher(bindings.len(), Default::default());
        for &(s, v) in bindings {
            globals.insert(s, v);
        }
        RuntimeCode {
            gens: [
                ArcSwap::from_pointee(CodeSlabs::default()),
                ArcSwap::from_pointee(CodeSlabs::default()),
            ],
            current_gen: AtomicUsize::new(0),
            runtime_tag: next_runtime_tag(),
            gen_inflight: [AtomicUsize::new(0), AtomicUsize::new(0)],
            gen_version: AtomicU64::new(0),
            // The prelude's stability metadata, threaded in for exactly the reason its
            // privacy set is: `seeded` INSERTS the prelude's bindings rather than
            // re-evaluating them, so the `%register-meta` a `(meta …)` emits never fires
            // in a live runtime and the facts would be silently absent — which is how
            // `not=`'s deprecation first came out invisible in every process but the
            // builder heap.
            meta: RwLock::new(prelude_meta.iter().cloned().collect()),
            // Reserved at seed time: every shipped **function**, macro and builtin.
            // Deliberately NOT the prelude's data globals — `*features*`,
            // `*load-path*`, `*module-docs*`, `*reload-diagnostics*` are registries
            // that prelude functions rebind with `def` at runtime (Brood's one
            // mutation), so `require`/`defmodule`/`provide` would break if they were
            // reserved. The rule is exactly "a shipped FUNCTION can't be redefined";
            // shipped mutable state stays rebindable, which is how it works at all.
            sealed: RwLock::new(
                bindings
                    .iter()
                    .filter(|(_, v)| {
                        matches!(
                            v.unpack(),
                            ValueRef::Fn(_) | ValueRef::Macro(_) | ValueRef::Native(_)
                        )
                    })
                    .map(|&(s, _)| s)
                    .collect(),
            ),
            // The prelude's own module-private names (ADR-146). `seeded` *inserts*
            // the bindings (it does not re-`eval` them, so `%mark-private` never
            // fires for a prelude name in the live runtime), and privacy is no
            // longer derivable from the clean name — so the set is collected when
            // the prelude is built (every `defn-`/`def-` head recorded in the
            // builder heap's runtime) and threaded in here, the same way the
            // bindings themselves are.
            private: RwLock::new(prelude_private.iter().copied().collect()),
            globals: RwLock::new(globals),
            global_generations: RwLock::new(SymbolMap::default()),
            registry_lock: Mutex::new(HashSet::new()),
            version: AtomicU64::new(0),
            code_epoch: AtomicU64::new(0),
            def_sites: RwLock::new(HashMap::new()),
            positions: RwLock::new(HashMap::new()),
            jit_code_cache: RwLock::new(HashMap::new()),
            shared_closures: RwLock::new(HashMap::new()),
            jit_inline_cache: RwLock::new(HashMap::new()),
            declared_sigs: RwLock::new(SymbolMap::default()),
            drain_active: AtomicBool::new(false),
            rt_dirty: AtomicBool::new(true),
            drain_gen: AtomicUsize::new(0),
            drain_epoch: AtomicU64::new(0),
            drain_acked: AtomicU64::new(0),
            drain_acks: RwLock::new(HashMap::new()),
            free_epoch: AtomicU64::new(0),
            aging: AtomicBool::new(false),
            aged_count: AtomicU64::new(0),
            promote_lock: RwLock::new(()),
        }
    }

    /// Read/write the global table, recovering from a poisoned lock instead of
    /// propagating the panic. The values are `Copy` handles and writers only
    /// `insert`/replace, so a writer that panicked left the map structurally
    /// sound — recovering keeps one bad process from wedging every other one
    /// that later looks up or defines a global.
    pub(super) fn globals_read(&self) -> RwLockReadGuard<'_, SymbolMap<Value>> {
        self.globals.read().unwrap_or_else(|e| e.into_inner())
    }
    pub(super) fn globals_write(&self) -> RwLockWriteGuard<'_, SymbolMap<Value>> {
        self.globals.write().unwrap_or_else(|e| e.into_inner())
    }
    /// Is `sym` a reserved (language-shipped) name? See [`RuntimeCode::sealed`].
    ///
    /// A **dynamic variable is never reserved**, whatever it holds. `defdyn` (or
    /// `%declare-dynamic`) *declares a name rebindable* — that is the entire meaning
    /// of the declaration — so reserving one would contradict it. This matters
    /// concretely for `*out*`/`*err*`: an output port IS a function
    /// (`(fn (s) …)`), so the function-valued test would otherwise reserve them and
    /// make a permanent output redirect impossible, leaving only the scoped
    /// `binding` form. The check lives here rather than in the seed filter so it also
    /// covers a `defdyn` inside an embedded module, and so a name declared dynamic
    /// *after* the seed is exempt too.
    pub(super) fn is_sealed(&self, sym: Symbol) -> bool {
        !crate::core::value::is_dynamic(sym)
            && self
                .sealed
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&sym)
    }
    /// Reserve `sym` — called for each name an embedded std module defines as it
    /// loads, so the module's own surface becomes reserved once it exists.
    pub(super) fn seal(&self, sym: Symbol) {
        self.sealed
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sym);
    }

    /// Record `sym` (a qualified global name) as module-private — called by the
    /// `%mark-private` primitive that a `defn-`/`def-` emits (after its `def`).
    /// Idempotent insert. See [`RuntimeCode::private`].
    pub(super) fn mark_private(&self, sym: Symbol) {
        self.private
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sym);
    }
    /// Clear any private mark on `sym` — called from `env_define` on EVERY global
    /// definition, so a name redefined public (an author editing `defn-` → `defn`
    /// and hot-reloading) stops being private. A `defn-`/`def-` re-marks immediately
    /// via `%mark-private`, which runs after the `def`; a plain `defn`/`def` does
    /// not, leaving the name public. So privacy always tracks the latest def form.
    pub(super) fn unmark_private(&self, sym: Symbol) {
        self.private
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&sym);
    }
    /// Replace the whole private set (the `%isolate` restore — see
    /// [`Heap::restore_private_names`]). Set-level because the caller cannot enumerate
    /// what the isolated thunk marked; one write-lock swap, not a diff.
    pub(super) fn restore_private(&self, names: Vec<Symbol>) {
        *self.private.write().unwrap_or_else(|e| e.into_inner()) = names.into_iter().collect();
    }
    /// Is `sym` recorded module-private? The authoritative (and, since ADR-146 step 2,
    /// the *only*) half of [`Heap::is_private`].
    /// Record `sym`'s stability metadata, replacing whatever was there. See [`NameMeta`].
    pub(super) fn set_meta(&self, sym: Symbol, meta: NameMeta) {
        self.meta
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sym, meta);
    }
    /// Drop `sym`'s metadata — called from `env_define` on every global definition, so a
    /// redefined name does not inherit the old one's `:deprecated`/`:beta` facts. A
    /// `(meta …)` form re-records immediately, exactly as `%mark-private` does for privacy.
    pub(super) fn clear_meta(&self, sym: Symbol) {
        self.meta
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&sym);
    }
    /// `sym`'s recorded metadata, if any.
    pub(super) fn meta_of(&self, sym: Symbol) -> Option<NameMeta> {
        self.meta
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&sym)
            .cloned()
    }

    pub(super) fn is_private_recorded(&self, sym: Symbol) -> bool {
        self.private
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&sym)
    }

    /// As `globals_read`/`globals_write`, for the def-site table (same
    /// poison-recovery rationale — entries are owned data, never structurally
    /// corrupting on a panicked writer).
    pub(super) fn def_sites_read(&self) -> RwLockReadGuard<'_, HashMap<Symbol, SourceLoc>> {
        self.def_sites.read().unwrap_or_else(|e| e.into_inner())
    }
    /// RUNTIME-form source position + file by `(code_gen, slab index)`, or `None`.
    /// See [`Self::positions`].
    pub(super) fn position_of(&self, idx: usize, code_gen: usize) -> Option<FormPos> {
        self.positions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&rt_pos_key(idx, code_gen))
            .cloned()
    }
    /// Record a RUNTIME-form source position + file (called by `promote`). See [`Self::positions`].
    pub(super) fn set_position(&self, idx: usize, code_gen: usize, entry: FormPos) {
        self.positions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(rt_pos_key(idx, code_gen), entry);
    }

    pub(super) fn def_sites_write(&self) -> RwLockWriteGuard<'_, HashMap<Symbol, SourceLoc>> {
        self.def_sites.write().unwrap_or_else(|e| e.into_inner())
    }
}
