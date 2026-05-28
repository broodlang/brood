//! The per-process data heap, plus the two shared regions: the immutable
//! **prelude** and a runtime's mutable, shared **code** region.
//!
//! A `Value`'s heap variants are integer handles whose two high bits (the
//! *region*, see `value.rs`) say where they live:
//!
//! - **LOCAL** — the per-process [`Heap`]: everything a process allocates at
//!   runtime (cons cells, vectors, strings, call-frame env scopes). Plain
//!   `Vec`s, mutated through `&mut Heap`, so the whole `Heap` is `Send`. Has a
//!   per-slab **free list** so the tracing [`collect`](Self::collect) can reclaim
//!   dead slots; `alloc_*` pop the free list before extending the slab.
//! - **PRELUDE** — a [`SharedCode`] region (behind `Arc`) holding the prelude +
//!   builtins. Built once, frozen, shared read-only by every runtime.
//! - **RUNTIME** — a [`RuntimeCode`] region (behind `Arc`) holding a runtime's
//!   `def`'d code and its global bindings. **Mutable and shared** by all of a
//!   runtime's inner (spawned) processes, so a redefinition is visible to a
//!   running process on its next global lookup (Erlang-style hot reload). The
//!   code slabs are append-only (old code is never moved or freed, so in-flight
//!   calls keep running it); the global bindings are a `RwLock<HashMap>`.
//!
//! GC is **per-process, single-threaded, non-moving mark-sweep** (ADR-035, see
//! `docs/memory-model.md`). Handles are stable across collection — a live
//! object's slab slot is never moved — so a Rust local holding a rooted handle
//! stays valid. PRELUDE and RUNTIME are not swept (they hold no LOCAL refs, by
//! the promotion invariant — see [`promote`](Self::promote)); the collector
//! only touches LOCAL. Roots are gathered explicitly at the outermost-eval
//! safepoint (see the `GC_BLOCK` discipline in `process.rs`).

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use smallvec::SmallVec;

use crate::core::value::{
    Closure, ClosureId, EnvId, MapId, NativeFn, NativeId, PairId, StrId, Symbol, Value, VecId,
    LOCAL, PRELUDE, RUNTIME,
};
use crate::error::LispError;

/// Generate a `&self` accessor that resolves a handle to a shared reference by
/// region: the LOCAL/PRELUDE slab is indexed directly; the append-only RUNTIME
/// slab via `boxcar::Vec::get` (stable refs, lock-free). The three uniform
/// all-three-region reference accessors share this; `pair` (returns by value)
/// and the region-restricted `native`/`env_frame` stay hand-written.
macro_rules! region_ref {
    ($name:ident, $id:ty, $field:ident, $ret:ty, $what:literal) => {
        pub fn $name(&self, id: $id) -> $ret {
            match id.region() {
                LOCAL => &self.local.$field[id.index()],
                PRELUDE => &self.prelude.slabs.$field[id.index()],
                RUNTIME => self.runtime.code.$field.get(id.index()).expect($what),
                _ => unreachable!("invalid handle region"),
            }
        }
    };
}

/// Inline storage for an env frame's bindings. A frame holds a handful (function
/// params, a `let`'s names), so keeping them inline avoids a heap allocation per
/// call / `let` — which the byte-counting global allocator would otherwise tax
/// with atomics on the hot path. Spills to the heap past the inline capacity.
type EnvVars = SmallVec<[(Symbol, Value); 4]>;

struct EnvFrame {
    // A small association list, not a `HashMap`: frames hold a handful of
    // bindings (function params, a `let`'s names), and they're immutable after
    // their bind phase (ADR-026 — no `set!`), so a build-once / scan-to-read
    // vector is lighter than hashing and wins at these sizes. Lookups scan from
    // the end so a later binding shadows an earlier one of the same name
    // (sequential `let`).
    vars: EnvVars,
    parent: Option<EnvId>,
}

/// Lower bound on the GC threshold (live LOCAL objects), so tiny heaps don't
/// thrash by collecting between every few allocations. Overridden by the
/// `BROOD_GC_STRESS` env var (set to `1` to collect at every safepoint — a
/// debug aid that flushes out rooting bugs by maximising free-list churn).
///
/// Read once on first use and cached — env vars don't change mid-run, and the
/// safepoint hits this every collection.
fn gc_floor() -> usize {
    static FLOOR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FLOOR.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            0
        } else {
            // 64 KB of cons cells worth (~3000 entries) is well above per-call
            // working sets but trivial vs the GBs a long-running process leaks.
            64 * 1024
        }
    })
}

/// Re-tag a value's handle from the local region to the immutable **prelude**
/// region (same slab index, region bits set). Atoms are unchanged.
fn to_prelude(v: Value) -> Value {
    match v {
        Value::Pair(id) => Value::Pair(PairId::prelude(id.index())),
        Value::Vector(id) => Value::Vector(VecId::prelude(id.index())),
        Value::Map(id) => Value::Map(MapId::prelude(id.index())),
        Value::Str(id) => Value::Str(StrId::prelude(id.index())),
        Value::Fn(id) => Value::Fn(ClosureId::prelude(id.index())),
        Value::Macro(id) => Value::Macro(ClosureId::prelude(id.index())),
        Value::Native(id) => Value::Native(NativeId::prelude(id.index())),
        other => other,
    }
}

/// The slabs holding heap objects in the LOCAL data heap and the PRELUDE region.
#[derive(Default)]
struct Slabs {
    pairs: Vec<(Value, Value)>,
    vectors: Vec<Vec<Value>>,
    /// Maps as insertion-ordered key/value association vectors (no duplicate
    /// keys — `assoc` replaces in place). Small and immutable, so a `Vec` scanned
    /// by structural equality is enough; a HAMT can replace it later with no
    /// surface change.
    maps: Vec<Vec<(Value, Value)>>,
    strings: Vec<String>,
    closures: Vec<Closure>,
    natives: Vec<NativeFn>,
    envs: Vec<EnvFrame>,
}

/// Per-slab free lists for the LOCAL heap: indices of dead slots reclaimed by
/// [`Heap::collect`] that the next [`Heap::alloc_pair`] (etc.) reuses before
/// extending the slab. Empty for the PRELUDE/RUNTIME regions (those are
/// append-only / frozen). No `natives` list — natives are only allocated during
/// the prelude build (then frozen into PRELUDE), so the LOCAL natives slab
/// stays empty at runtime and isn't swept.
#[derive(Default)]
struct FreeLists {
    pairs: Vec<u32>,
    vectors: Vec<u32>,
    maps: Vec<u32>,
    strings: Vec<u32>,
    closures: Vec<u32>,
    envs: Vec<u32>,
}

impl FreeLists {
    fn clear(&mut self) {
        self.pairs.clear();
        self.vectors.clear();
        self.maps.clear();
        self.strings.clear();
        self.closures.clear();
        self.envs.clear();
    }

    /// Drop free-list entries pointing into the *truncated* region (≥ each cap).
    /// Called after [`Heap::reset_local_to`] truncates the slabs so we don't try
    /// to reuse indices that no longer exist.
    fn purge_above(&mut self, cp: &LocalCheckpoint) {
        self.pairs.retain(|&i| (i as usize) < cp.pairs);
        self.vectors.retain(|&i| (i as usize) < cp.vectors);
        self.maps.retain(|&i| (i as usize) < cp.maps);
        self.strings.retain(|&i| (i as usize) < cp.strings);
        self.closures.retain(|&i| (i as usize) < cp.closures);
        self.envs.retain(|&i| (i as usize) < cp.envs);
    }
}

/// The immutable, read-only prelude region (closures, code values, the
/// builtins). Built once, then shared by `Arc` into every runtime.
#[derive(Default)]
pub struct SharedCode {
    slabs: Slabs,
    /// Where each prelude global was defined — `name → (cache-file, pos)`,
    /// recorded once during the prelude build (the file is the materialized
    /// `prelude.blsp` copy; see `lib.rs`). Immutable like the rest of this
    /// region, and consulted by [`Heap::def_site`] *after* the runtime table so
    /// a user redefinition of a prelude name still wins. Powers cross-file
    /// goto-definition into the standard library (ADR-031, docs/lsp.md).
    def_sites: HashMap<Symbol, SourceLoc>,
}

/// A snapshot of the LOCAL heap's sizes, taken at a top-level boundary. Passing
/// it back to [`Heap::reset_local_to`] reclaims everything allocated since (see
/// there for the safety contract). This is the arena-reset reclamation strategy
/// (`docs/memory-model.md`): at a quiescent point the LOCAL heap holds nothing
/// live but the form's result, because globals live in PRELUDE/RUNTIME and never
/// point into LOCAL.
#[derive(Clone, Copy)]
pub struct LocalCheckpoint {
    pairs: usize,
    vectors: usize,
    maps: usize,
    strings: usize,
    closures: usize,
    envs: usize,
    // No `natives` field: a live runtime never allocates a native into its LOCAL
    // heap (they're registered once during the prelude build, then frozen into
    // PRELUDE). If that ever changes, add a field here and truncate it below.
}

/// Append-only code slabs for the shared RUNTIME region. `boxcar::Vec` gives
/// lock-free reads that return stable references (existing elements never move
/// or free as the vector grows), so process threads read closure bodies without
/// locking while another process `def`s new code.
#[derive(Default)]
struct CodeSlabs {
    pairs: boxcar::Vec<(Value, Value)>,
    vectors: boxcar::Vec<Vec<Value>>,
    maps: boxcar::Vec<Vec<(Value, Value)>>,
    strings: boxcar::Vec<String>,
    closures: boxcar::Vec<Closure>,
    /// Captured environments of promoted closures. A closure defined *inside a
    /// function call* (not at top level) closes over a local scope; promoting it
    /// for sharing copies that scope here so it resolves in any process. Frozen
    /// once promoted (read-only), so append-only is sound.
    envs: boxcar::Vec<EnvFrame>,
}

/// A runtime's mutable, shared code region: the code `def`'d at runtime plus the
/// global bindings table. All of a runtime's inner processes share one of these
/// (via `Arc::clone`), which is what makes a `def` propagate to them — and what
/// keeps separate runtimes (nodes) independent (each has its own).
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
    fn write(&mut self, bytes: &[u8]) {
        // Fallback for any non-`u32` key (none on the hot path); kept correct.
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// A `HashMap` keyed by interned `Symbol`s, using the fast [`SymbolHasher`].
pub type SymbolMap<V> = HashMap<Symbol, V, std::hash::BuildHasherDefault<SymbolHasher>>;

pub struct RuntimeCode {
    code: CodeSlabs,
    /// The global bindings (prelude + user `def`s). Read on every global lookup,
    /// written on `def` (the only mutation). The values point into PRELUDE or RUNTIME.
    globals: RwLock<SymbolMap<Value>>,
    /// Where each global was *defined* — file + form position, recorded at load
    /// time before macroexpansion (ADR-031). Lives here, beside `globals`, so it
    /// is shared across a runtime's processes and updated by a redefinition, the
    /// same as the bindings it describes. Read by `(source-location 'name)`; the
    /// image-query foundation for cross-file goto-definition.
    def_sites: RwLock<HashMap<Symbol, SourceLoc>>,
}

/// Where a global was defined: the file, and the start position of its
/// `def`/`defn`/`defmacro` form. Captured pre-macroexpansion so `defn`/`defmacro`
/// definitions are located accurately (ADR-031).
#[derive(Clone)]
pub struct SourceLoc {
    pub file: String,
    pub pos: crate::error::Pos,
}

impl Default for RuntimeCode {
    fn default() -> Self {
        RuntimeCode {
            code: CodeSlabs::default(),
            globals: RwLock::new(SymbolMap::default()),
            def_sites: RwLock::new(HashMap::new()),
        }
    }
}

impl RuntimeCode {
    /// A fresh runtime whose global table is seeded with the prelude bindings
    /// (`symbol -> prelude value`). The code slabs start empty — user `def`s
    /// append to them. Inner processes share this whole thing via `Arc`.
    pub fn seeded(bindings: &[(Symbol, Value)]) -> Self {
        let mut globals = SymbolMap::with_capacity_and_hasher(bindings.len(), Default::default());
        for &(s, v) in bindings {
            globals.insert(s, v);
        }
        RuntimeCode {
            code: CodeSlabs::default(),
            globals: RwLock::new(globals),
            def_sites: RwLock::new(HashMap::new()),
        }
    }

    /// Read/write the global table, recovering from a poisoned lock instead of
    /// propagating the panic. The values are `Copy` handles and writers only
    /// `insert`/replace, so a writer that panicked left the map structurally
    /// sound — recovering keeps one bad process from wedging every other one
    /// that later looks up or defines a global.
    fn globals_read(&self) -> RwLockReadGuard<'_, SymbolMap<Value>> {
        self.globals.read().unwrap_or_else(|e| e.into_inner())
    }
    fn globals_write(&self) -> RwLockWriteGuard<'_, SymbolMap<Value>> {
        self.globals.write().unwrap_or_else(|e| e.into_inner())
    }

    /// As `globals_read`/`globals_write`, for the def-site table (same
    /// poison-recovery rationale — entries are owned data, never structurally
    /// corrupting on a panicked writer).
    fn def_sites_read(&self) -> RwLockReadGuard<'_, HashMap<Symbol, SourceLoc>> {
        self.def_sites.read().unwrap_or_else(|e| e.into_inner())
    }
    fn def_sites_write(&self) -> RwLockWriteGuard<'_, HashMap<Symbol, SourceLoc>> {
        self.def_sites.write().unwrap_or_else(|e| e.into_inner())
    }
}

pub struct Heap {
    local: Slabs,
    /// Reclaimed-but-not-yet-reused LOCAL slots. Grown by [`Heap::collect`]'s
    /// sweep, drained by `alloc_*` before extending the slab. PRELUDE/RUNTIME
    /// (append-only) have no equivalent.
    local_free: FreeLists,
    prelude: Arc<SharedCode>,
    runtime: Arc<RuntimeCode>,
    /// This process's global scope. For a real runtime this is [`EnvId::GLOBAL`]
    /// (routing to `runtime.globals`); for the prelude *builder* it's a real
    /// local root frame (so the prelude can be evaluated, then frozen).
    global: EnvId,
    /// Source position of LOCAL list forms, keyed by pair slab index, recorded
    /// by the reader. Queried via `(form-pos …)` (e.g. by the test macros, which
    /// look up a form's line *before* it expands). LOCAL-only and dropped on
    /// reset, since it is read-time metadata for the source being loaded.
    form_pos: HashMap<usize, crate::error::Pos>,
    /// The file currently being `load`ed, exposed via `(current-file)`. Saved and
    /// restored around each load so nested loads don't clobber the outer file.
    current_file: Option<String>,
    /// This process's dynamic-variable binding stack (the `binding` form). Each
    /// `binding` pushes its `(symbol, value)` pairs and pops them when its body
    /// returns (even on error); a read of a dynamic var consults this — latest
    /// binding wins — before the shared global table (see [`Heap::env_get`]).
    /// Per-process and not shared: a `spawn`ed child starts with an empty stack,
    /// so dynamic bindings never cross to another process (data isn't shared).
    /// Empty whenever no `binding` is active — so it's free on the common path
    /// and holds no LOCAL handles across a top-level arena reset.
    dynamics: Vec<(Symbol, Value)>,
    /// Explicit GC root stack: any LOCAL [`Value`] alive across a possible GC
    /// safepoint that isn't already reachable from `env`/`expr`/`dynamics` lives
    /// here. In practice this is one site — `eval_str`/`eval_source` push the
    /// unevaluated forms vector here for the duration of the per-form eval (the
    /// only depth-0-reachable transient surface, by the `GC_BLOCK==1` invariant
    /// — see `docs/memory-model.md`). Empty on the hot path.
    roots: Vec<Value>,
    /// Adaptive GC trigger: collect when the LOCAL live-object count crosses
    /// this. Recomputed after each [`collect`](Self::collect) as
    /// `max(GC_FLOOR, 2 * live)`. `usize::MAX` while [`gc_enabled`] is false
    /// (prelude build) so the safepoint check is a single compare with no GC.
    ///
    /// [`gc_enabled`]: Self::gc_enabled
    gc_threshold: usize,
    /// GC switch. `false` during the prelude *build* (`Heap::new`), `true` for
    /// real process heaps (`Heap::with_regions`); also forced `false` when the
    /// prelude `SharedCode` `Arc` is the default (empty) one, since a missing
    /// prelude means a freshly-built builder heap that's about to freeze.
    gc_enabled: bool,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

/// Pop a free-list slot if one is waiting, otherwise extend the slab. The
/// shared shape behind every `replace-wholesale` allocator: `alloc_pair`,
/// `alloc_vector`, `alloc_map`, `alloc_closure`. Returns the chosen slot
/// index (usize). Pre-consolidation each of those was four lines of the
/// same `if let Some(idx) = … pop() { … } else { … push() }` shape; the
/// macro is that shape in one place. (`alloc_string` and `new_env` reuse
/// the slot's inner buffer instead and stay hand-written.)
macro_rules! alloc_slot {
    ($self:expr, $field:ident, $value:expr) => {{
        if let Some(idx) = $self.local_free.$field.pop() {
            $self.local.$field[idx as usize] = $value;
            idx as usize
        } else {
            let idx = $self.local.$field.len();
            $self.local.$field.push($value);
            idx
        }
    }};
}

impl Heap {
    /// A bare heap with empty shared regions — used to *build* the prelude
    /// before freezing it. Real runtimes use [`Heap::with_regions`]. GC is
    /// disabled here (the prelude is built once, then frozen — collection would
    /// be wasted work and could complicate `freeze_as_shared_code` if it left
    /// holes mid-build).
    pub fn new() -> Self {
        Heap {
            local: Slabs::default(),
            local_free: FreeLists::default(),
            prelude: Arc::default(),
            runtime: Arc::default(),
            global: EnvId::local(0),
            form_pos: HashMap::new(),
            current_file: None,
            dynamics: Vec::new(),
            roots: Vec::new(),
            gc_threshold: usize::MAX,
            gc_enabled: false,
        }
    }

    /// A fresh process heap sharing the given prelude + runtime regions (empty
    /// local slabs). Spawned inner processes pass the *same* `runtime` Arc as
    /// their parent, so they see its global bindings and its later `def`s.
    pub fn with_regions(prelude: Arc<SharedCode>, runtime: Arc<RuntimeCode>) -> Self {
        Heap {
            local: Slabs::default(),
            local_free: FreeLists::default(),
            prelude,
            runtime,
            global: EnvId::local(0),
            form_pos: HashMap::new(),
            current_file: None,
            dynamics: Vec::new(),
            roots: Vec::new(),
            gc_threshold: gc_floor(),
            gc_enabled: true,
        }
    }

    /// Clone the Arc to this heap's prelude region (for spawning a child).
    pub fn prelude_arc(&self) -> Arc<SharedCode> {
        Arc::clone(&self.prelude)
    }

    /// Clone the Arc to this runtime's shared code region (for spawning a child
    /// that shares this runtime's live globals).
    pub fn runtime_arc(&self) -> Arc<RuntimeCode> {
        Arc::clone(&self.runtime)
    }

    /// Consume this (builder) heap: move everything it allocated into a frozen
    /// [`SharedCode`] (PRELUDE) region — re-tagging every handle local→prelude —
    /// and return that region plus the global env's bindings
    /// (`symbol -> prelude value`) used to seed each runtime's global table.
    ///
    /// Env frames are dropped: shared (top-level) closures capture the global
    /// env symbolically (`env == None`), so nothing references a frame.
    /// GC is disabled in a builder heap (`Heap::new` sets `gc_enabled = false`),
    /// so the slabs have no holes here — indices are dense and stable across
    /// the local→prelude re-tag.
    pub fn freeze_as_shared_code(self, root: EnvId) -> (SharedCode, Vec<(Symbol, Value)>) {
        let bindings: Vec<(Symbol, Value)> = self.local.envs[root.index()]
            .vars
            .iter()
            .map(|&(s, v)| (s, to_prelude(v)))
            .collect();

        let mut slabs = self.local;
        for p in &mut slabs.pairs {
            p.0 = to_prelude(p.0);
            p.1 = to_prelude(p.1);
        }
        for vec in &mut slabs.vectors {
            for x in vec.iter_mut() {
                *x = to_prelude(*x);
            }
        }
        for map in &mut slabs.maps {
            for (k, v) in map.iter_mut() {
                *k = to_prelude(*k);
                *v = to_prelude(*v);
            }
        }
        for c in &mut slabs.closures {
            for f in c.body.iter_mut() {
                *f = to_prelude(*f);
            }
            for (_, d) in c.optionals.iter_mut() {
                *d = to_prelude(*d);
            }
            debug_assert!(
                c.env.is_none(),
                "shared closures must capture the global env"
            );
        }
        slabs.envs = Vec::new(); // the prelude region has no env frames

        // Move the def-sites the builder recorded (via `note_definition` while
        // loading the prelude) into the immutable region. They describe prelude
        // globals, never change, and shouldn't be re-recorded per runtime.
        let def_sites = std::mem::take(&mut *self.runtime.def_sites_write());

        (SharedCode { slabs, def_sites }, bindings)
    }

    /// Record this process's global scope (call once, after creating it).
    pub fn set_global(&mut self, env: EnvId) {
        self.global = env;
    }

    /// This process's global scope.
    pub fn global(&self) -> EnvId {
        self.global
    }

    /// True if `env` is this process's global scope.
    pub fn is_global(&self, env: EnvId) -> bool {
        env == self.global
    }

    /// Snapshot the LOCAL heap's current sizes (for arena-reset reclamation).
    pub fn checkpoint(&self) -> LocalCheckpoint {
        LocalCheckpoint {
            pairs: self.local.pairs.len(),
            vectors: self.local.vectors.len(),
            maps: self.local.maps.len(),
            strings: self.local.strings.len(),
            closures: self.local.closures.len(),
            envs: self.local.envs.len(),
        }
    }

    /// Reclaim everything allocated into the LOCAL heap since `cp`, by truncating
    /// the slabs back to it.
    ///
    /// **Safety contract (logical, not `unsafe`):** call this only at a top-level
    /// boundary — when the evaluator has fully returned and no value reachable
    /// from here on holds a LOCAL handle at or past `cp`. Globals live in the
    /// PRELUDE/RUNTIME regions and never point into LOCAL (a top-level `def`
    /// *promotes* its value out), so they're always safe; the only thing that can
    /// still be live is the *result* of the form just evaluated — consume or
    /// promote it before resetting. Resetting mid-evaluation would strand the
    /// in-flight computation's values and corrupt later reads.
    pub fn reset_local_to(&mut self, cp: LocalCheckpoint) {
        self.local.pairs.truncate(cp.pairs);
        self.local.vectors.truncate(cp.vectors);
        self.local.maps.truncate(cp.maps);
        self.local.strings.truncate(cp.strings);
        self.local.closures.truncate(cp.closures);
        self.local.envs.truncate(cp.envs);
        // Drop position metadata for the pairs just reclaimed (indices reused).
        if !self.form_pos.is_empty() {
            self.form_pos.retain(|&i, _| i < cp.pairs);
        }
        // Drop free-list entries pointing into the truncated tail — those slots
        // no longer exist. Entries below the cap remain valid (holes inside the
        // surviving prefix that a later `alloc_*` can still reuse).
        self.local_free.purge_above(&cp);
        // The threshold is relative to live count; reclamation here is so cheap
        // that we let the next `gc_due` check recompute against the smaller heap.
    }

    // ----- source-position metadata (editor tooling; see docs/tooling.md) -----

    /// Record the source position of a LOCAL list form (no-op for atoms and
    /// forms in the shared regions). Called by the reader as it builds lists.
    pub fn set_form_pos(&mut self, v: Value, pos: crate::error::Pos) {
        if let Value::Pair(id) = v {
            if id.region() == crate::core::value::LOCAL {
                self.form_pos.insert(id.index(), pos);
            }
        }
    }

    /// The recorded source position of a form, if it is a LOCAL list with one.
    pub fn form_pos(&self, v: Value) -> Option<crate::error::Pos> {
        if let Value::Pair(id) = v {
            if id.region() == crate::core::value::LOCAL {
                return self.form_pos.get(&id.index()).copied();
            }
        }
        None
    }

    /// Set the file currently being loaded, returning the previous value so the
    /// caller can restore it (loads nest).
    pub fn set_current_file(&mut self, file: Option<String>) -> Option<String> {
        std::mem::replace(&mut self.current_file, file)
    }

    /// The file currently being loaded, exposed to Brood via `(current-file)`.
    pub fn current_file(&self) -> Option<&str> {
        self.current_file.as_deref()
    }

    // ----- definition sites (cross-file xref; ADR-031, docs/lsp.md) -----

    /// If `form` is a top-level `def`/`defn`/`defmacro`, record its name's source
    /// location (the [`current_file`] + `pos`). Called by the file loaders on each
    /// *un-expanded* top-level form — before macroexpansion, so `defn`/`defmacro`
    /// (which lower to `def`) are still recognisable by their head and their span
    /// is intact. A no-op when no file is set (e.g. the REPL) or the form isn't a
    /// definition.
    ///
    /// [`current_file`]: Self::current_file
    pub fn note_definition(&mut self, form: Value, pos: crate::error::Pos) {
        let Some(file) = self.current_file.clone() else {
            return;
        };
        if let Some(name) = self.def_form_name(form) {
            self.runtime
                .def_sites_write()
                .insert(name, SourceLoc { file, pos });
        }
    }

    /// The name a top-level `def`/`defn`/`defmacro` form binds, reading the head
    /// and first argument from the *un-expanded* form. `None` for anything else
    /// (including `(def (pattern) …)`, which has no plain name — deferred).
    fn def_form_name(&self, form: Value) -> Option<Symbol> {
        let Value::Pair(p) = form else { return None };
        let Value::Sym(head) = self.car(p) else {
            return None;
        };
        if !matches!(
            crate::core::value::symbol_name(head).as_str(),
            "def" | "defn" | "defmacro"
        ) {
            return None;
        }
        let Value::Pair(rest) = self.cdr(p) else {
            return None;
        };
        match self.car(rest) {
            Value::Sym(name) => Some(name),
            _ => None,
        }
    }

    /// Where `name`'s global definition was loaded from, if recorded. Backs
    /// `(source-location 'name)`. The runtime table (user/project `def`s) takes
    /// precedence over the immutable prelude table, so redefining a prelude name
    /// reports the user's site, not the standard library's.
    pub fn def_site(&self, name: Symbol) -> Option<SourceLoc> {
        self.runtime
            .def_sites_read()
            .get(&name)
            .cloned()
            .or_else(|| self.prelude.def_sites.get(&name).cloned())
    }

    // ----- allocation (always into the local heap) -----
    //
    // Each allocator pops a [`FreeLists`] entry (a slot the GC reclaimed and
    // overwrites in place) before extending the slab — so the slab's `len()`
    // stays bounded by the high-water live count, not the lifetime allocation
    // total. Atomic w.r.t. the slab's `Vec`: a free index is always < current
    // `len`, so writing in place is well-defined.
    //
    // The four `replace-wholesale` allocators (pair/vector/map/closure) share
    // the same pop-or-push shape; the [`alloc_slot!`] macro is that shape in
    // one place. `alloc_string` / `new_env` differ — they *reuse* the slot's
    // inner buffer (String capacity, EnvVars inline storage) rather than
    // replacing wholesale — so they stay hand-written.

    pub fn alloc_pair(&mut self, head: Value, tail: Value) -> Value {
        let idx = alloc_slot!(self, pairs, (head, tail));
        Value::Pair(PairId::local(idx))
    }

    pub fn alloc_vector(&mut self, items: Vec<Value>) -> Value {
        let idx = alloc_slot!(self, vectors, items);
        Value::Vector(VecId::local(idx))
    }

    /// Allocate a map from already-canonical entries (insertion order, no
    /// duplicate keys). The map operations below build the entry vector — keyed
    /// by structural equality — and hand it here.
    pub fn alloc_map(&mut self, entries: Vec<(Value, Value)>) -> Value {
        let idx = alloc_slot!(self, maps, entries);
        Value::Map(MapId::local(idx))
    }

    // ----- map operations (immutable: each returns a fresh map) -----

    /// The value `key` maps to, by structural equality, or `None` if absent.
    pub fn map_get(&self, id: MapId, key: Value) -> Option<Value> {
        self.map(id)
            .iter()
            .find(|(k, _)| self.equal(*k, key))
            .map(|(_, v)| *v)
    }

    /// A fresh map with `key` bound to `val`: replaces the value if `key` is
    /// already present (keeping its position), otherwise appends.
    pub fn map_assoc(&mut self, id: MapId, key: Value, val: Value) -> Value {
        let mut entries = self.map(id).to_vec();
        match entries.iter_mut().find(|(k, _)| self.equal(*k, key)) {
            Some(slot) => slot.1 = val,
            None => entries.push((key, val)),
        }
        self.alloc_map(entries)
    }

    /// A fresh map with `key` removed (a no-op clone if it was absent).
    pub fn map_dissoc(&mut self, id: MapId, key: Value) -> Value {
        let entries: Vec<(Value, Value)> = self
            .map(id)
            .iter()
            .filter(|(k, _)| !self.equal(*k, key))
            .copied()
            .collect();
        self.alloc_map(entries)
    }

    /// Build a canonical map from raw `(key, value)` pairs, applying last-wins
    /// deduplication by structural equality (for map literals and `hash-map`).
    pub fn map_from_pairs(&mut self, pairs: Vec<(Value, Value)>) -> Value {
        let mut entries: Vec<(Value, Value)> = Vec::with_capacity(pairs.len());
        for (k, v) in pairs {
            match entries.iter_mut().find(|(ek, _)| self.equal(*ek, k)) {
                Some(slot) => slot.1 = v,
                None => entries.push((k, v)),
            }
        }
        self.alloc_map(entries)
    }

    pub fn alloc_string(&mut self, s: &str) -> Value {
        if let Some(idx) = self.local_free.strings.pop() {
            // Reuse the slot's `String` buffer by replacing its contents — saves
            // the existing capacity if `s` fits.
            let slot = &mut self.local.strings[idx as usize];
            slot.clear();
            slot.push_str(s);
            return Value::Str(StrId::local(idx as usize));
        }
        let idx = self.local.strings.len();
        self.local.strings.push(s.to_string());
        Value::Str(StrId::local(idx))
    }

    pub fn alloc_closure(&mut self, c: Closure) -> ClosureId {
        let idx = alloc_slot!(self, closures, c);
        ClosureId::local(idx)
    }

    pub fn alloc_native(&mut self, f: NativeFn) -> Value {
        // Natives are only allocated during the prelude build (then frozen into
        // PRELUDE); the LOCAL natives slab stays empty at runtime and isn't
        // swept, so there's no free list to consult.
        let idx = self.local.natives.len();
        self.local.natives.push(f);
        Value::Native(NativeId::local(idx))
    }

    /// Build a proper list from a vector of items.
    pub fn list(&mut self, items: Vec<Value>) -> Value {
        self.list_with_tail(items, Value::Nil)
    }

    /// Build a list of `items` ending in `tail`. A `Nil` tail gives a proper
    /// list; any other tail gives an improper (dotted) list, e.g. `(1 2 . 3)`.
    pub fn list_with_tail(&mut self, items: Vec<Value>, tail: Value) -> Value {
        let mut acc = tail;
        for item in items.into_iter().rev() {
            acc = self.alloc_pair(item, acc);
        }
        acc
    }

    /// Build a proper list from a slice — no intermediate `Vec`. For the hot path
    /// where the items already live in a buffer, notably a `& rest` parameter's
    /// trailing args (every variadic call, which includes all the arithmetic and
    /// comparison operators).
    pub fn list_from_slice(&mut self, items: &[Value]) -> Value {
        let mut acc = Value::Nil;
        for &item in items.iter().rev() {
            acc = self.alloc_pair(item, acc);
        }
        acc
    }

    // ----- promotion: copy code from LOCAL into the shared RUNTIME region -----

    /// Deep-copy a value's reachable structure from the local heap into the
    /// shared RUNTIME region, returning a handle valid in every inner process.
    /// `def` of a global runs this so the bound code/data is shareable;
    /// `spawn` runs it on the target function. Atoms and already-shared values
    /// (PRELUDE/RUNTIME) are returned unchanged — no copy.
    ///
    /// Appends only (never mutates existing shared code), so a redefinition adds
    /// a new version while in-flight calls keep running the old one.
    pub fn promote(&self, v: Value) -> Value {
        match v {
            Value::Str(id) if id.region() == LOCAL => {
                let s = self.string(id).to_string();
                Value::Str(StrId::runtime(self.runtime.code.strings.push(s)))
            }
            Value::Pair(id) if id.region() == LOCAL => self.promote_list(id),
            Value::Vector(id) if id.region() == LOCAL => {
                let items: Vec<Value> = self
                    .vector(id)
                    .to_vec()
                    .into_iter()
                    .map(|x| self.promote(x))
                    .collect();
                Value::Vector(VecId::runtime(self.runtime.code.vectors.push(items)))
            }
            Value::Map(id) if id.region() == LOCAL => {
                let entries: Vec<(Value, Value)> = self
                    .map(id)
                    .to_vec()
                    .into_iter()
                    .map(|(k, v)| (self.promote(k), self.promote(v)))
                    .collect();
                Value::Map(MapId::runtime(self.runtime.code.maps.push(entries)))
            }
            Value::Fn(id) if id.region() == LOCAL => Value::Fn(self.promote_closure(id)),
            Value::Macro(id) if id.region() == LOCAL => Value::Macro(self.promote_closure(id)),
            // Atoms, and values already in PRELUDE/RUNTIME, need no copy.
            _ => v,
        }
    }

    /// Promote a local cons-chain. Walks the `cdr` spine *iteratively* so a long
    /// list doesn't recurse its length deep (which overflowed the native stack);
    /// recursion is bounded by element nesting via `promote` on each `car`.
    /// Stops at the first already-shared cell or non-pair tail, preserving both
    /// improper (dotted) lists and existing structure sharing.
    fn promote_list(&self, first: PairId) -> Value {
        let mut heads = Vec::new();
        let mut cur = Value::Pair(first);
        let tail = loop {
            match cur {
                Value::Pair(id) if id.region() == LOCAL => {
                    let (head, next) = self.pair(id);
                    heads.push(self.promote(head));
                    cur = next;
                }
                other => break self.promote(other),
            }
        };
        let mut acc = tail;
        for head in heads.into_iter().rev() {
            acc = Value::Pair(PairId::runtime(self.runtime.code.pairs.push((head, acc))));
        }
        acc
    }

    fn promote_closure(&self, id: ClosureId) -> ClosureId {
        let cl = self.closure(id).clone();
        let body = cl.body.iter().map(|&f| self.promote(f)).collect();
        let optionals = cl
            .optionals
            .iter()
            .map(|&(s, d)| (s, self.promote(d)))
            .collect();
        // A top-level closure captures the global env (`None`) and is fully
        // shareable as-is. A closure that captured a *local* scope has its scope
        // promoted too, so it resolves its free variables in any process.
        let env = cl.env.map(|e| self.promote_env(e));
        let promoted = Closure {
            name: cl.name,
            params: cl.params,
            optionals,
            rest: cl.rest,
            body,
            doc: cl.doc,
            env,
        };
        ClosureId::runtime(self.runtime.code.closures.push(promoted))
    }

    /// Deep-copy an environment frame chain from LOCAL into the shared RUNTIME
    /// region, promoting each bound value. Stops at the global scope (the shared
    /// sentinel). Already-shared (RUNTIME) frames are returned unchanged.
    fn promote_env(&self, env: EnvId) -> EnvId {
        if env == EnvId::GLOBAL || env.region() == RUNTIME {
            return env;
        }
        // Snapshot the frame, then promote its parent and values (no borrow held).
        let (parent, bindings): (Option<EnvId>, Vec<(Symbol, Value)>) = {
            let frame = self.env_frame(env);
            (
                frame.parent,
                frame.vars.iter().map(|&(s, v)| (s, v)).collect(),
            )
        };
        let parent = parent.map(|p| self.promote_env(p));
        let vars = bindings
            .into_iter()
            .map(|(s, v)| (s, self.promote(v)))
            .collect();
        EnvId::runtime(self.runtime.code.envs.push(EnvFrame { vars, parent }))
    }

    // ----- access (dispatch on the handle's region) -----

    pub fn pair(&self, id: PairId) -> (Value, Value) {
        match id.region() {
            LOCAL => self.local.pairs[id.index()],
            PRELUDE => self.prelude.slabs.pairs[id.index()],
            RUNTIME => *self
                .runtime
                .code
                .pairs
                .get(id.index())
                .expect("runtime pair handle"),
            _ => unreachable!("invalid handle region"),
        }
    }
    pub fn car(&self, id: PairId) -> Value {
        self.pair(id).0
    }
    pub fn cdr(&self, id: PairId) -> Value {
        self.pair(id).1
    }
    region_ref!(vector, VecId, vectors, &[Value], "runtime vector handle");
    region_ref!(map, MapId, maps, &[(Value, Value)], "runtime map handle");
    region_ref!(string, StrId, strings, &str, "runtime string handle");
    region_ref!(
        closure,
        ClosureId,
        closures,
        &Closure,
        "runtime closure handle"
    );

    pub fn native(&self, id: NativeId) -> &NativeFn {
        match id.region() {
            LOCAL => &self.local.natives[id.index()],
            PRELUDE => &self.prelude.slabs.natives[id.index()],
            _ => unreachable!("natives live only in the local or prelude region"),
        }
    }

    /// Collect a proper list into a `Vec`. Errors on an improper (dotted) list.
    pub fn list_to_vec(&self, v: Value) -> Result<Vec<Value>, LispError> {
        let mut out = Vec::new();
        let mut cur = v;
        loop {
            match cur {
                Value::Nil => return Ok(out),
                Value::Pair(p) => {
                    let (head, tail) = self.pair(p);
                    out.push(head);
                    cur = tail;
                }
                _ => return Err(LispError::type_err("improper list")),
            }
        }
    }

    /// Treat a list or vector as a sequence of items.
    pub fn seq_items(&self, v: Value) -> Result<Vec<Value>, LispError> {
        match v {
            Value::Nil => Ok(Vec::new()),
            Value::Pair(_) => self.list_to_vec(v),
            Value::Vector(id) => Ok(self.vector(id).to_vec()),
            _ => Err(LispError::type_err("expected a list or vector")),
        }
    }

    /// Structural equality (the basis of `=`). Functions/macros/natives compare
    /// by identity (same handle).
    ///
    /// Floats compare by IEEE value, so `-0.0 = 0.0` is true and `nan = nan` is
    /// false — the least-surprising arithmetic semantics (not bitwise equality).
    pub fn equal(&self, a: Value, b: Value) -> bool {
        use Value::*;
        match (a, b) {
            (Nil, Nil) => true,
            (Bool(x), Bool(y)) => x == y,
            (Int(x), Int(y)) => x == y,
            (Float(x), Float(y)) => x == y,
            (Sym(x), Sym(y)) => x == y,
            (Keyword(x), Keyword(y)) => x == y,
            (Str(x), Str(y)) => self.string(x) == self.string(y),
            // Walk the `cdr` spine iteratively so comparing long lists doesn't
            // recurse their length deep; recursion stays bounded by `car` nesting.
            (Pair(x), Pair(y)) => {
                let (mut x, mut y) = (x, y);
                loop {
                    let (a0, a1) = self.pair(x);
                    let (b0, b1) = self.pair(y);
                    if !self.equal(a0, b0) {
                        break false;
                    }
                    match (a1, b1) {
                        (Pair(nx), Pair(ny)) => {
                            x = nx;
                            y = ny;
                        }
                        _ => break self.equal(a1, b1),
                    }
                }
            }
            (Vector(x), Vector(y)) => {
                let xs = self.vector(x);
                let ys = self.vector(y);
                xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(&p, &q)| self.equal(p, q))
            }
            // Maps are equal when they hold the same key→value associations,
            // independent of insertion order: same size, and every key in `x`
            // maps to an equal value in `y` (keys themselves found by `equal`).
            (Map(x), Map(y)) => {
                let xs = self.map(x);
                xs.len() == self.map(y).len()
                    && xs
                        .iter()
                        .all(|(k, v)| self.map_get(y, *k).is_some_and(|w| self.equal(*v, w)))
            }
            (Fn(x), Fn(y)) => x == y,
            (Macro(x), Macro(y)) => x == y,
            (Native(x), Native(y)) => x == y,
            (Ref(x), Ref(y)) => x == y,
            // Pids are equal by node identity + local id (same process, anywhere).
            (Pid { node: n1, id: i1 }, Pid { node: n2, id: i2 }) => n1 == n2 && i1 == i2,
            _ => false,
        }
    }

    // ----- environments -----
    //
    // Real env frames are always LOCAL. The global scope is the sentinel
    // [`EnvId::GLOBAL`], which routes to the shared `runtime.globals` table; a
    // top-level frame's parent chain bottoms out there. (During prelude *build*
    // the global is instead a real local root frame with no parent.)

    fn env_frame(&self, env: EnvId) -> &EnvFrame {
        match env.region() {
            LOCAL => &self.local.envs[env.index()],
            RUNTIME => self
                .runtime
                .code
                .envs
                .get(env.index())
                .expect("runtime env frame"),
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
    pub fn env_frame_ref(&self, env: EnvId) -> (Option<EnvId>, &[(Symbol, Value)]) {
        let frame = self.env_frame(env);
        (frame.parent, &frame.vars)
    }

    pub fn new_env(&mut self, parent: Option<EnvId>) -> EnvId {
        if let Some(idx) = self.local_free.envs.pop() {
            // Reuse the slot, dropping its old contents. `EnvVars` is a
            // `SmallVec` so this releases any spilled bindings; the inline
            // capacity stays with the slot.
            let slot = &mut self.local.envs[idx as usize];
            slot.vars.clear();
            slot.parent = parent;
            return EnvId::local(idx as usize);
        }
        let idx = self.local.envs.len();
        self.local.envs.push(EnvFrame {
            vars: EnvVars::new(),
            parent,
        });
        EnvId::local(idx)
    }

    pub fn env_get(&self, env: EnvId, sym: Symbol) -> Option<Value> {
        let mut cur = Some(env);
        while let Some(e) = cur {
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
                return self.runtime.globals_read().get(&sym).copied();
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

    pub fn env_define(&mut self, env: EnvId, sym: Symbol, val: Value) {
        if env == EnvId::GLOBAL {
            // Global code/data is shared across inner processes, so promote it
            // into the shared RUNTIME region before binding.
            let shared = self.promote(val);
            self.runtime.globals_write().insert(sym, shared);
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

    /// Snapshot the runtime's global bindings (`symbol -> value`). Cheap: the
    /// values are `Copy` handles. Pair with [`Heap::restore_globals`] to run code
    /// against a *private copy* of the globals — mutations to the live table can
    /// then be rolled back (this is what the `%isolate` primitive does for
    /// `:isolated` tests). Only meaningful when no other process is writing the
    /// table concurrently.
    pub fn snapshot_globals(&self) -> SymbolMap<Value> {
        self.runtime.globals_read().clone()
    }

    /// Every symbol currently bound in the global table (prelude + user `def`s).
    /// For tooling/introspection — `(global-names)` feeds completion and
    /// workspace-symbol queries (see `docs/lsp.md`). Returns just the keys, so
    /// no `Value`s are cloned.
    pub fn global_symbols(&self) -> Vec<Symbol> {
        self.runtime.globals_read().keys().copied().collect()
    }

    /// Restore the runtime's global bindings from a [`Heap::snapshot_globals`]
    /// snapshot, discarding every `def` made since it was taken. The
    /// append-only code slabs are *not* reclaimed (there's no GC yet), but the
    /// bindings revert — so a name `def`'d since the snapshot becomes unbound
    /// again, and a rebound name returns to its earlier value.
    pub fn restore_globals(&self, snapshot: SymbolMap<Value>) {
        *self.runtime.globals_write() = snapshot;
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

    // ----- GC root stack -------------------------------------------------------
    //
    // A small explicit root stack for the few sites (today: `eval_str` /
    // `eval_source`) that hold a `Vec<Value>` of LOCAL forms across a depth-0
    // eval call. Every other place is either already reachable from
    // `env`/`expr` at the safepoint, or sits at `GC_BLOCK > 1` where GC won't
    // fire — see `docs/memory-model.md`. Empty on the hot path.

    /// Push `v` onto the explicit root stack so it survives any GC that may run
    /// between now and the matching [`Self::truncate_roots`] (or
    /// [`Self::pop_root`]). Cheap: one `Vec` push.
    pub fn push_root(&mut self, v: Value) {
        self.roots.push(v);
    }

    /// Pop the most recently pushed root (the matching unwind of `push_root`).
    pub fn pop_root(&mut self) -> Option<Value> {
        self.roots.pop()
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

    // ----- GC trigger / introspection -----------------------------------------

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

    /// LOCAL live-object count = `Σ slab.len() − Σ free.len()` over the swept
    /// slabs. The metric the threshold tracks; also exposed for tests asserting
    /// reclamation in long-running loops.
    pub fn local_live_count(&self) -> usize {
        let total = self.local.pairs.len()
            + self.local.vectors.len()
            + self.local.maps.len()
            + self.local.strings.len()
            + self.local.closures.len()
            + self.local.envs.len();
        let free = self.local_free.pairs.len()
            + self.local_free.vectors.len()
            + self.local_free.maps.len()
            + self.local_free.strings.len()
            + self.local_free.closures.len()
            + self.local_free.envs.len();
        // `saturating_sub` rather than `total - free`: if a future bug ever
        // makes the free list outgrow the slab (sweep accounting drift, a
        // double-free, etc.) this returns 0 instead of panicking on the GC
        // safepoint hot path. A `debug_assert!` flags the invariant break in
        // tests without taking the prod runtime down.
        debug_assert!(
            total >= free,
            "free count {} exceeds slab count {}",
            free,
            total
        );
        total.saturating_sub(free)
    }

    // ----- the tracing GC ------------------------------------------------------
    //
    // Non-moving, single-threaded mark-sweep over the LOCAL heap only. Roots
    // are: `extra_roots`/`extra_envs` (the caller — usually the eval safepoint
    // — supplies `expr`/`env` here), the explicit root stack [`Self::roots`],
    // and the dynamic-binding stack [`Self::dynamics`]. The PRELUDE and RUNTIME
    // regions are never traced into (they hold no LOCAL refs, by the promotion
    // invariant), so the walk stays bounded by *this* process's working set.
    //
    // Marking is **iterative** (an explicit worklist) so a deep cons chain or
    // env-frame chain can't overflow the native stack. Sweep rebuilds the free
    // lists from scratch as `(0..len).filter(|i| !marked[i])` — equivalently,
    // any LOCAL slot present in the slab and not reached from a root.

    /// Collect garbage in the LOCAL heap. `extra_roots` / `extra_envs` are
    /// transient roots known to the caller (the evaluator passes the current
    /// `expr` and `env` here). Pre-allocated objects whose handles aren't
    /// rooted in *any* form become unreachable and are added to the free lists.
    ///
    /// **Safety contract:** every live LOCAL handle, anywhere — Rust locals,
    /// captured borrows, in-flight builtin accumulators — must be reachable
    /// from the union {`extra_roots`, `extra_envs`, [`Self::roots`],
    /// [`Self::dynamics`]}. The `GC_BLOCK == 1` discipline in `process.rs`
    /// makes this true *by construction* at the eval safepoint (no other eval
    /// or macroexpand frame is active, the eval's own loop-body locals are
    /// dead at `continue 'tail`, and the only depth-0 caller — `eval_str` —
    /// uses [`Self::push_root`] for its forms vec). Calling `collect` from
    /// anywhere else is the caller's responsibility to satisfy.
    pub fn collect(&mut self, extra_roots: &[Value], extra_envs: &[EnvId]) {
        if !self.gc_enabled {
            return;
        }
        // Sized to the LOCAL slabs only — RUNTIME/PRELUDE handles are filtered
        // out before they reach the worklist, so we never index those marks.
        let mut marks = Marks::new(&self.local);
        let mut work: Vec<TraceItem> = Vec::new();

        // Seed: the caller's transient roots.
        for &v in extra_roots {
            push_value(&mut work, v);
        }
        for &e in extra_envs {
            push_env(&mut work, e);
        }
        // The explicit root stack and the dynamic-binding stack.
        for &v in &self.roots {
            push_value(&mut work, v);
        }
        for &(_, v) in &self.dynamics {
            push_value(&mut work, v);
        }

        // Worklist mark phase. Adding a handle to `work` is a *request* to mark
        // it; the pop site checks the mark bit and only walks its children if
        // it was unmarked (so we never cycle, no quadratic re-traversal).
        while let Some(item) = work.pop() {
            self.trace_one(item, &mut marks, &mut work);
        }

        // Sweep: rebuild free lists from `(0..len) \ marked`. Clearing the slot
        // (strings/vectors/maps/closures/envs) releases the slot's owned inner
        // allocations; pairs are 16 bytes inline, so they only need the index
        // re-listed.
        self.sweep(&marks);

        // Adaptive threshold: collect again when live doubles. Floored so a
        // tiny heap doesn't thrash.
        let live = self.local_live_count();
        self.gc_threshold = std::cmp::max(gc_floor(), live.saturating_mul(2));
    }

    /// Mark one item and, if it was previously unmarked, enqueue its children.
    /// Skips PRELUDE/RUNTIME handles entirely — the promotion invariant
    /// guarantees they reach no LOCAL data, so there's nothing for us to
    /// reclaim down those edges.
    fn trace_one(&self, item: TraceItem, marks: &mut Marks, work: &mut Vec<TraceItem>) {
        match item {
            TraceItem::Pair(idx) => {
                if marks.mark_pair(idx) {
                    let (a, b) = self.local.pairs[idx];
                    push_value(work, a);
                    push_value(work, b);
                }
            }
            TraceItem::Vector(idx) => {
                if marks.mark_vector(idx) {
                    for &v in &self.local.vectors[idx] {
                        push_value(work, v);
                    }
                }
            }
            TraceItem::Map(idx) => {
                if marks.mark_map(idx) {
                    for &(k, v) in &self.local.maps[idx] {
                        push_value(work, k);
                        push_value(work, v);
                    }
                }
            }
            TraceItem::Str(idx) => {
                // No children, but mark it so it survives sweep.
                marks.mark_string(idx);
            }
            TraceItem::Closure(idx) => {
                if marks.mark_closure(idx) {
                    let cl = &self.local.closures[idx];
                    for &f in &cl.body {
                        push_value(work, f);
                    }
                    for &(_, d) in &cl.optionals {
                        push_value(work, d);
                    }
                    if let Some(env) = cl.env {
                        push_env(work, env);
                    }
                }
            }
            TraceItem::Env(idx) => {
                if marks.mark_env(idx) {
                    let frame = &self.local.envs[idx];
                    for &(_, v) in &frame.vars {
                        push_value(work, v);
                    }
                    if let Some(parent) = frame.parent {
                        push_env(work, parent);
                    }
                }
            }
        }
    }

    /// Sweep the LOCAL slabs: any unmarked slot becomes a free-list entry.
    /// Replaces the old free list (every slot present-and-unmarked is "free
    /// now," whether or not it was free before — the marks distinguish live
    /// from dead, not from previously-free).
    fn sweep(&mut self, marks: &Marks) {
        self.local_free.clear();

        for i in 0..self.local.pairs.len() {
            if !marks.is_pair_marked(i) {
                self.local_free.pairs.push(i as u32);
                // form_pos is keyed by pair index; drop the entry since the
                // slot will be reused for an unrelated pair.
                self.form_pos.remove(&i);
            }
        }
        for i in 0..self.local.vectors.len() {
            if !marks.is_vector_marked(i) {
                self.local_free.vectors.push(i as u32);
                // Release the dead `Vec<Value>`'s buffer; alloc_vector replaces
                // the slot wholesale on reuse, so we don't need an empty marker.
                self.local.vectors[i] = Vec::new();
            }
        }
        for i in 0..self.local.maps.len() {
            if !marks.is_map_marked(i) {
                self.local_free.maps.push(i as u32);
                self.local.maps[i] = Vec::new();
            }
        }
        for i in 0..self.local.strings.len() {
            if !marks.is_string_marked(i) {
                self.local_free.strings.push(i as u32);
                // Release the dead `String` buffer; alloc_string replaces.
                self.local.strings[i] = String::new();
            }
        }
        for i in 0..self.local.closures.len() {
            if !marks.is_closure_marked(i) {
                self.local_free.closures.push(i as u32);
                // Replace with a default so the `Vec`s inside drop. `Closure`
                // derives `Default`, so adding a field to it doesn't risk a
                // sweep-bug from a missed initialiser here.
                self.local.closures[i] = Closure::default();
            }
        }
        for i in 0..self.local.envs.len() {
            if !marks.is_env_marked(i) {
                self.local_free.envs.push(i as u32);
                let slot = &mut self.local.envs[i];
                slot.vars.clear();
                slot.parent = None;
            }
        }
    }
}

// ----- GC worklist + mark bits ----------------------------------------------

/// One item on the mark worklist — a LOCAL handle to walk. RUNTIME/PRELUDE
/// handles are filtered out at the `push_*` sites so they never reach here.
#[derive(Clone, Copy)]
enum TraceItem {
    Pair(usize),
    Vector(usize),
    Map(usize),
    Str(usize),
    Closure(usize),
    Env(usize),
}

/// If `v` carries a LOCAL handle, push it onto the mark worklist. Atoms and
/// shared-region values are ignored.
fn push_value(work: &mut Vec<TraceItem>, v: Value) {
    match v {
        Value::Pair(id) if id.region() == LOCAL => work.push(TraceItem::Pair(id.index())),
        Value::Vector(id) if id.region() == LOCAL => work.push(TraceItem::Vector(id.index())),
        Value::Map(id) if id.region() == LOCAL => work.push(TraceItem::Map(id.index())),
        Value::Str(id) if id.region() == LOCAL => work.push(TraceItem::Str(id.index())),
        Value::Fn(id) | Value::Macro(id) if id.region() == LOCAL => {
            work.push(TraceItem::Closure(id.index()))
        }
        _ => {}
    }
}

/// If `env` is a LOCAL frame, push it. The [`EnvId::GLOBAL`] sentinel and
/// RUNTIME-promoted frames are skipped (no LOCAL slot to mark).
fn push_env(work: &mut Vec<TraceItem>, env: EnvId) {
    if env != EnvId::GLOBAL && env.region() == LOCAL {
        work.push(TraceItem::Env(env.index()));
    }
}

/// One bit per slot in each LOCAL slab. Allocated per collection (no persistent
/// memory cost between cycles). `mark_*` returns `true` if the slot transitioned
/// from unmarked to marked, so the caller can enqueue children only once.
struct Marks {
    pairs: Vec<bool>,
    vectors: Vec<bool>,
    maps: Vec<bool>,
    strings: Vec<bool>,
    closures: Vec<bool>,
    envs: Vec<bool>,
}

impl Marks {
    fn new(local: &Slabs) -> Self {
        Marks {
            pairs: vec![false; local.pairs.len()],
            vectors: vec![false; local.vectors.len()],
            maps: vec![false; local.maps.len()],
            strings: vec![false; local.strings.len()],
            closures: vec![false; local.closures.len()],
            envs: vec![false; local.envs.len()],
        }
    }
}

// Generate `mark_X` / `is_X_marked` for each slab. Pre-consolidation these
// were twelve hand-written one-line methods that drifted on style (some used
// `.unwrap_or(false)`, some asserted in-range). The macro pins one shape: a
// `mark_X` that flips the bit and reports first-touch (so the worklist
// enqueues children only once), and an `is_X_marked` that's safe past the
// end of the bit-vector (the sweep loop indexes `local.X.len()`, but a slab
// that grew mid-mark would otherwise panic). One shape, one place.
macro_rules! mark_methods {
    ($($field:ident => $mark:ident, $is_marked:ident),+ $(,)?) => {
        impl Marks {
            $(
                fn $mark(&mut self, i: usize) -> bool { mark_one(&mut self.$field, i) }
                fn $is_marked(&self, i: usize) -> bool {
                    self.$field.get(i).copied().unwrap_or(false)
                }
            )+
        }
    };
}

mark_methods! {
    pairs => mark_pair, is_pair_marked,
    vectors => mark_vector, is_vector_marked,
    maps => mark_map, is_map_marked,
    strings => mark_string, is_string_marked,
    closures => mark_closure, is_closure_marked,
    envs => mark_env, is_env_marked,
}

#[inline]
fn mark_one(bits: &mut [bool], i: usize) -> bool {
    if bits[i] {
        false
    } else {
        bits[i] = true;
        true
    }
}
