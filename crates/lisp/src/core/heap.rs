//! The per-process data heap, plus the two shared regions: the immutable
//! **prelude** and a runtime's mutable, shared **code** region.
//!
//! A `Value`'s heap variants are integer handles whose two high bits (the
//! *region*, see `value.rs`) say where they live:
//!
//! - **LOCAL** — the per-process [`Heap`]: everything a process allocates at
//!   runtime (cons cells, vectors, strings, call-frame env scopes). Plain
//!   `Vec`s, mutated through `&mut Heap`, so the whole `Heap` is `Send`.
//!   Bump-allocated into a **nursery**; survivors are relocated by the copying
//!   collector (see below), never freed in place, so handle slots are never
//!   reused.
//! - **PRELUDE** — a [`SharedCode`] region (behind `Arc`) holding the prelude +
//!   builtins. Built once, frozen, shared read-only by every runtime.
//! - **RUNTIME** — a [`RuntimeCode`] region (behind `Arc`) holding a runtime's
//!   `def`'d code and its global bindings. **Mutable and shared** by all of a
//!   runtime's inner (spawned) processes, so a redefinition is visible to a
//!   running process on its next global lookup (Erlang-style hot reload). The
//!   code slabs are append-only (old code is never moved or freed, so in-flight
//!   calls keep running it); the global bindings are a `RwLock<HashMap>`.
//!
//! GC is **per-process, single-threaded, generational semi-space copying**
//! (ADR-055/061/072, see `docs/memory-model.md` and `docs/memory-review.md`). The
//! LOCAL heap is a **nursery** + a tenured **old** generation; a *minor*
//! collection ([`collect`](Self::collect) → [`minor_collect`](Self::minor_collect))
//! copies the nursery's survivors (tenuring or flipping) and drops the rest, a
//! rare *major* compacts old. Because survivors **move**, a handle held across a
//! collection without being re-rooted goes stale — so the evaluator keeps its
//! in-flight LOCAL handles on an explicit operand stack ([`roots`](Self::roots) +
//! [`env_roots`](Self::env_roots)) that the collector relocates in place, letting
//! it collect at **any** eval depth; a generation epoch on every handle (ADR-054)
//! trips a precise debug tripwire on a stale deref. PRELUDE and RUNTIME are never
//! traced (they hold no LOCAL refs, by the promotion invariant — see
//! [`promote`](Self::promote)); the collector only touches LOCAL.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use smallvec::SmallVec;

use crate::core::blob::{SharedBlob, SHARED_BLOB_THRESHOLD};
use crate::core::map_champ::{self, MapNode, MAX_DEPTH};
use crate::core::value::{
    Closure, ClosureArm, ClosureId, EnvId, MapId, NativeFn, NativeId, PairId, Passthrough, RopeId,
    StrId, Symbol, Value, VecId, LOCAL, PRELUDE, RUNTIME,
};
use crate::error::LispError;

/// A LOCAL (and transitively PRELUDE-builder) string slab entry. Small strings
/// stay inline; strings of [`SHARED_BLOB_THRESHOLD`] bytes or more route through
/// an `Arc<SharedBlob>` so cross-process sends bump a refcount instead of
/// deep-copying the bytes (see `core/blob.rs`).
///
/// PRELUDE itself contains no `Shared` entries — `freeze_as_shared_code`
/// inline-extracts any builder-time Shared blobs into `Inline(String)` before
/// freezing, keeping the cross-runtime PRELUDE region independent of any
/// runtime-scoped `Arc<SharedBlob>`.
enum LocalString {
    Inline(String),
    Shared(Arc<SharedBlob>),
}

impl Default for LocalString {
    fn default() -> Self {
        LocalString::Inline(String::new())
    }
}

impl LocalString {
    fn as_str(&self) -> &str {
        match self {
            LocalString::Inline(s) => s.as_str(),
            // SAFETY: `SharedBlob::new` is the only constructor and takes
            // `&[u8]` from a `&str`'s `as_bytes()` (see [`Heap::alloc_string`]).
            // Blobs are immutable after construction. The wire decoder
            // (`get_str` in `dist::wire`) validates UTF-8 on entry before
            // allocating, so a cross-node payload satisfies the invariant
            // too. In debug builds an extra `from_utf8` round-trip catches
            // a missed entry-point — the unchecked read only ships in
            // release.
            #[cfg(not(debug_assertions))]
            LocalString::Shared(b) => unsafe { std::str::from_utf8_unchecked(b.as_bytes()) },
            #[cfg(debug_assertions)]
            LocalString::Shared(b) => {
                std::str::from_utf8(b.as_bytes()).expect("shared blob bytes are valid UTF-8")
            }
        }
    }
}

/// Generate a `&self` accessor that resolves a handle to a shared reference by
/// region: the LOCAL/PRELUDE slab is indexed directly; the append-only RUNTIME
/// slab via `boxcar::Vec::get` (stable refs, lock-free). The three uniform
/// all-three-region reference accessors share this; `pair` (returns by value)
/// and the region-restricted `native`/`env_frame` stay hand-written.
macro_rules! region_ref {
    ($name:ident, $id:ty, $field:ident, $ret:ty, $what:literal) => {
        pub fn $name(&self, id: $id) -> $ret {
            match id.region() {
                LOCAL if id.is_old() => {
                    #[cfg(debug_assertions)]
                    self.check_epoch_aged(true, id.generation(), id.index(), stringify!($name), id.0);
                    &self.old.$field[id.index()]
                }
                LOCAL => {
                    #[cfg(debug_assertions)]
                    debug_assert!(
                        !PoisonBits::is(&self.poison.$field, id.index()),
                        "use-after-GC: {}() on freed nursery {} slot {} (handle {:#x}).",
                        stringify!($name),
                        stringify!($field),
                        id.index(),
                        id.0
                    );
                    #[cfg(debug_assertions)]
                    self.check_epoch_aged(false, id.generation(), id.index(), stringify!($name), id.0);
                    &self.local.$field[id.index()]
                }
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
/// Tag ranks for `value_cmp`'s heterogeneous fallback. The order is mostly
/// aesthetic — what matters is that it's *fixed* so a heterogeneous sort is
/// reproducible. Numbers come first (most common), then strings/keywords/
/// symbols (text), then collections, then everything else.
fn tag_rank(v: Value) -> u8 {
    match v {
        Value::Nil => 0,
        Value::Bool(_) => 1,
        Value::Int(_) | Value::Float(_) => 2,
        Value::Str(_) => 3,
        Value::Keyword(_) => 4,
        Value::Sym(_) => 5,
        Value::Pair(_) => 6,
        Value::Vector(_) => 7,
        Value::Map(_) => 8,
        Value::Fn(_) => 9,
        Value::Native(_) => 10,
        Value::Macro(_) => 11,
        Value::Ref(_) => 12,
        Value::Pid { .. } => 13,
        Value::Rope(_) => 14,
        Value::Socket(_) => 15,
    }
}

/// Opt-in (`BROOD_ENV_DEBUG=1`) for the legacy poison-based env-chain
/// diagnostics. Off by default: they run per eval / per symbol and walk the env
/// chain, so leaving them always-on made debug builds pathologically slow — and
/// they're superseded by the generational-handle tripwire (ADR-054). Kept as an
/// on-demand tool. Debug-only.
#[cfg(debug_assertions)]
fn env_chain_debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_ENV_DEBUG").is_some())
}

/// Parse a GC threshold override (an *object count*, with an optional `K`/`M`
/// suffix — `64K` = 65536, `1M` = 1048576) from env var `key`. `None` if unset;
/// a malformed value warns and is ignored (so the caller's default stands).
/// Mirrors the `BROOD_MEM_LIMIT` size-parse style in `core/alloc.rs`, but counts
/// objects rather than bytes.
fn gc_count_env(key: &str) -> Option<usize> {
    let v = std::env::var(key).ok()?;
    let s = v.trim();
    let (num, mult) = match s.chars().last() {
        Some(c @ ('K' | 'k')) => (&s[..s.len() - c.len_utf8()], 1024usize),
        Some(c @ ('M' | 'm')) => (&s[..s.len() - c.len_utf8()], 1024 * 1024),
        _ => (s, 1usize),
    };
    match num.trim().parse::<usize>() {
        Ok(n) => n.checked_mul(mult),
        Err(_) => {
            eprintln!("[gc] ignoring malformed {key}={v:?} (try e.g. 65536 or 64K)");
            None
        }
    }
}

fn gc_floor() -> usize {
    static FLOOR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FLOOR.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            0
        } else {
            // 64 KB of cons cells worth (~3000 entries) is well above per-call
            // working sets but trivial vs the GBs a long-running process leaks.
            // Overridable for tuning via `BROOD_GC_FLOOR` (object count, K/M ok).
            gc_count_env("BROOD_GC_FLOOR").unwrap_or(64 * 1024)
        }
    })
}

/// Live old-gen object count below which a **major** collection never fires —
/// the old-gen counterpart of [`gc_floor`]. Crucially this is **not** zeroed by
/// `BROOD_GC_STRESS`: stress makes *minor* collection fire at every safepoint
/// (its purpose), but a major every safepoint would recompact the whole old
/// generation on an incremental large-structure build — O(n²). Keeping a nonzero
/// floor makes majors periodic under stress (still exercised) and rare in normal
/// operation (the old gen grows to a few MB before a compaction reclaims tenured
/// garbage, so live tenured data isn't recopied often).
fn major_floor() -> usize {
    static FLOOR: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *FLOOR.get_or_init(|| {
        if std::env::var_os("BROOD_GC_STRESS").is_some() {
            8192
        } else {
            // Overridable for tuning via `BROOD_GC_MAJOR` (object count, K/M ok).
            gc_count_env("BROOD_GC_MAJOR").unwrap_or(256 * 1024)
        }
    })
}

/// Nursery-pressure threshold (live object count) at or above which a minor
/// collection **tenures** survivors into the old generation; below it the minor
/// does a young **semi-space flip** (survivors stay in a fresh nursery) instead.
/// This is the *aging* policy: an object tenures only when it survives a
/// collection that followed real allocation pressure — never a premature one.
/// Stress-independent (unlike [`gc_floor`]) so that `BROOD_GC_STRESS=1`, which
/// fires a minor at *every* safepoint with a tiny nursery, always flips and so
/// never tenures transient garbage (which would otherwise bloat the old gen and
/// make majors recopy it — the adversarial-under-stress regression). A
/// long-lived structure still tenures once the nursery genuinely grows past this.
fn min_tenure() -> usize {
    static T: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    // Overridable for tuning via `BROOD_GC_TENURE` (object count, K/M ok).
    *T.get_or_init(|| gc_count_env("BROOD_GC_TENURE").unwrap_or(16 * 1024))
}

/// Default for the per-process GC **trace** flag, from the `BROOD_GC_TRACE` env
/// var (set it to trace the whole run — including the root process, which the
/// `(gc-trace …)` builtin can't reach before user code runs). Read once and
/// cached; `(gc-trace on/off)` overrides it per process at runtime.
fn gc_trace_default() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_GC_TRACE").is_some())
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
        // The prelude is pure Brood (no rope literals), so a rope can never
        // exist at freeze time. Guard the invariant rather than silently
        // re-tagging a LOCAL handle into PRELUDE.
        Value::Rope(_) => unreachable!("a Rope cannot appear in the prelude region"),
        other => other,
    }
}

/// The slabs holding heap objects in the LOCAL data heap and the PRELUDE region.
#[derive(Default)]
struct Slabs {
    pairs: Vec<(Value, Value)>,
    vectors: Vec<Vec<Value>>,
    /// Maps as a flat slab of CHAMP nodes (ADR-040). Each [`MapNode`] is
    /// either a branch (two bitmaps + packed data/children arrays) or a
    /// max-depth collision leaf. The handle in `Value::Map(MapId)` points
    /// at the trie's *root* node; child sub-nodes live in the same slab,
    /// referenced by `MapId`. The root is the only entry-point — internal
    /// nodes are reachable only through the trie itself.
    maps: Vec<MapNode>,
    strings: Vec<LocalString>,
    /// Text ropes (ADR-045). A `ropey::Rope` is itself `Arc`-shared internally,
    /// so this slab owns one cheap handle per live rope; cloning for an edit
    /// bumps refcounts, not bytes. Always inline (no SharedBlob split — ropes
    /// don't cross processes, so there's no cross-heap aliasing to optimise).
    ropes: Vec<ropey::Rope>,
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
    ropes: Vec<u32>,
    closures: Vec<u32>,
    envs: Vec<u32>,
}

impl FreeLists {
    fn clear(&mut self) {
        self.pairs.clear();
        self.vectors.clear();
        self.maps.clear();
        self.strings.clear();
        self.ropes.clear();
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
        self.ropes.retain(|&i| (i as usize) < cp.ropes);
        self.closures.retain(|&i| (i as usize) < cp.closures);
        self.envs.retain(|&i| (i as usize) < cp.envs);
    }
}

/// Use-after-GC tripwire bits, one per LOCAL slot in each slab. **Debug-only**:
/// the field on `Heap` is `#[cfg(debug_assertions)]`, and every accessor that
/// consults this drops out entirely in release. Set by [`Heap::sweep`] when a
/// slot is freed, cleared by `new_env` / the `alloc_slot!` reuse paths when a
/// slot is taken back out of the free list. A `debug_assert!` in each handle
/// accessor checks the bit so a *use of a dangling handle* panics at the
/// instant of the bad deref — pointing the backtrace at the actual offender,
/// not at the eventual symptom (e.g. an "unbound symbol" arising later when
/// the reclaimed env's parent chain is read).
#[cfg(debug_assertions)]
#[derive(Default)]
struct PoisonBits {
    pairs: Vec<bool>,
    vectors: Vec<bool>,
    maps: Vec<bool>,
    strings: Vec<bool>,
    ropes: Vec<bool>,
    closures: Vec<bool>,
    envs: Vec<bool>,
}

#[cfg(debug_assertions)]
impl PoisonBits {
    /// Is `idx` currently poisoned? Out-of-range answers `false` — a slot we
    /// never sized for can't have been freed by sweep.
    fn is(bits: &[bool], idx: usize) -> bool {
        bits.get(idx).copied().unwrap_or(false)
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
    ropes: usize,
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
    maps: boxcar::Vec<MapNode>,
    strings: boxcar::Vec<String>,
    /// Ropes `def`'d into a global (shared read-only across this runtime's
    /// processes). A `ropey::Rope` is `Send + Sync` and immutable-by-construction
    /// here (every edit makes a fresh LOCAL rope), so sharing one by handle is
    /// sound. Append-only like the rest of this region.
    ropes: boxcar::Vec<ropey::Rope>,
    /// `OnceLock`-wrapped so `promote` can **reserve a slot, then fill it** — the
    /// append-only `boxcar` can't write-back the way the GC's mutable slabs do, so
    /// a *cyclic* promote (a closure whose captured scope binds the closure itself,
    /// e.g. `(let (g (fn () g)) g)` or mutually-recursive `letrec` closures) would
    /// otherwise recurse forever → SIGSEGV. Reserve-then-fill lets the recursion
    /// resolve the back-edge to the reserved handle. Each cell is set exactly once
    /// before the handle is ever published, so reads (`get().unwrap()`) never race.
    closures: boxcar::Vec<OnceLock<Closure>>,
    /// Captured environments of promoted closures. A closure defined *inside a
    /// function call* (not at top level) closes over a local scope; promoting it
    /// for sharing copies that scope here so it resolves in any process. Frozen
    /// once promoted (read-only), so append-only is sound. `OnceLock`-wrapped for
    /// the same reserve-then-fill cycle break as `closures` above.
    envs: boxcar::Vec<OnceLock<EnvFrame>>,
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
    /// Monotonic version of `globals`, bumped on every binding change (`def`
    /// rebind, `restore_globals`). Per-process global **inline caches**
    /// (`Heap::global_ic`) stamp the version they resolved at and re-resolve only
    /// when it has moved — so a steady-state global read is an atomic load + a
    /// local hash hit instead of taking the shared `RwLock`. Late-binding stays
    /// exact: any `def` makes every stamped cache entry stale at once. `Relaxed`
    /// is sufficient — a global value is an immovable PRELUDE/RUNTIME handle, so
    /// there's no data it gates publication of; the counter only has to *change*.
    version: AtomicU64,
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
            version: AtomicU64::new(0),
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
            version: AtomicU64::new(0),
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
    /// The **nursery** (young generation): every `alloc_*` bumps into here, so it
    /// holds the freshly-allocated, mostly-short-lived objects. A *minor*
    /// collection ([`minor_collect`](Self::minor_collect)) copies its survivors
    /// into [`old`](Self::old) and drops the rest whole. Kept named `local` because
    /// it's the allocation hot path and the common case for an accessor.
    local: Slabs,
    /// The **old (tenured) generation**: objects that survived a minor collection,
    /// addressed by LOCAL handles with the [`AGE_OLD`](crate::core::value::AGE_OLD)
    /// bit set. Grows by append on each minor collection (cheap — old objects are
    /// never recopied); reclaimed only by a *major* collection
    /// ([`major_collect`](Self::major_collect)), which compacts it. Because Brood
    /// data is immutable, an old object can never come to point at a young one, so
    /// the old generation is **not a root set for a minor collection** — no write
    /// barrier, no remembered set.
    old: Slabs,
    /// Reclaimed-but-not-yet-reused LOCAL slots. Grown by [`Heap::collect`]'s
    /// sweep, drained by `alloc_*` before extending the slab. PRELUDE/RUNTIME
    /// (append-only) have no equivalent.
    local_free: FreeLists,
    /// Debug-build use-after-GC tripwire: a bit per LOCAL slot that's set when
    /// sweep frees the slot and cleared when an `alloc_*` / `new_env` reuses
    /// it. Every handle accessor (`pair`, `vector`, `closure`, `env_frame`, …)
    /// `debug_assert!`s its slot isn't poisoned, so a dangling handle panics
    /// at the *moment of use* with a backtrace pointing at the offender —
    /// instead of returning silently-stale data that surfaces as an "unbound
    /// symbol" or wrong-arity error many call frames later
    /// (`docs/claude-demo-findings.md` § Scheduler race). Skipped in release
    /// (`#[cfg(debug_assertions)]`) so there's zero hot-path cost shipped.
    #[cfg(debug_assertions)]
    poison: PoisonBits,
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
    /// Keyed by [`form_pos_key`] — the pair's slab index packed with its
    /// generation age bit, so a nursery pair and an old pair at the same slab
    /// index don't collide (the two LOCAL spaces share an index range).
    form_pos: HashMap<u64, crate::error::Pos>,
    /// The file currently being `load`ed, exposed via `(current-file)`. Saved and
    /// restored around each load so nested loads don't clobber the outer file.
    current_file: Option<String>,
    /// The namespace currently being compiled into (ADR-065). `None` = root (the
    /// prelude, plain code, and the REPL until an `(ns …)` form runs). Set by the
    /// `(ns foo)` form via the `%in-ns` primitive; read by the resolver pass
    /// (`eval::macros::resolve`) to qualify definition heads and free references to
    /// `foo/name`. Per-process compile state — NOT a shared global, which would race
    /// across green processes (`RuntimeCode` is shared). File/module loaders save +
    /// reset this to root per file (so a `require`d file starts at root); the REPL
    /// driver leaves it sticky across entries.
    compile_ns: Option<Symbol>,
    /// Names the current-namespace file will define (its top-level `def`/`defmacro`
    /// heads), pre-scanned when an `(ns …)` form runs so the resolver can qualify a
    /// *forward* reference (`bar` used before `foo/bar` is defined) — without it,
    /// such a reference would silently stay bare (order-dependent miscompile). Bare
    /// symbols only; consulted alongside the live global table. Cleared/repopulated
    /// per file by the loader.
    ns_known_names: HashSet<Symbol>,
    /// Names the current file `(:use …)`-imported: bare name → qualified global
    /// (`describe` → `test/describe`). Populated by `%refer` when the `(ns …)`
    /// header runs; consulted by the resolver after the current namespace and
    /// before root fall-through. Per-file like `ns_known_names` — reset/restored
    /// by the loaders so imports never leak across files (ADR-065 inc-2).
    imports: HashMap<Symbol, Symbol>,
    /// This process's dynamic-variable binding stack (the `binding` form). Each
    /// `binding` pushes its `(symbol, value)` pairs and pops them when its body
    /// returns (even on error); a read of a dynamic var consults this — latest
    /// binding wins — before the shared global table (see [`Heap::env_get`]).
    /// Per-process and not shared: a `spawn`ed child starts with an empty stack,
    /// so dynamic bindings never cross to another process (data isn't shared).
    /// Empty whenever no `binding` is active — so it's free on the common path
    /// and holds no LOCAL handles across a top-level arena reset.
    dynamics: Vec<(Symbol, Value)>,
    /// Per-process **global inline cache** (perf): `symbol -> (runtime version,
    /// resolved value)`. Consulted by [`env_get`](Self::env_get) only after the
    /// local env chain misses *and* no dynamic binding shadows the name — i.e.
    /// exactly where a lookup would otherwise take the shared `RwLock` on
    /// `runtime.globals`. On a version match it returns the cached handle with no
    /// lock; a stale entry (a `def` bumped `runtime.version`) falls through to the
    /// locked table and re-stamps. Cached values are always immovable
    /// PRELUDE/RUNTIME handles (globals are `promote`d before binding), so an entry
    /// survives a local GC untouched and needs no rooting. `RefCell` because
    /// `env_get` is `&self`; per-process, so never shared across threads.
    global_ic: RefCell<SymbolMap<(u64, Value)>>,
    /// Explicit GC root stack — the evaluator's **operand stack** (ADR-061).
    /// Every LOCAL [`Value`] an eval frame still needs *after* a nested `eval`
    /// (its accumulated `argv`, literal accumulators, `callee`, the `call_form`,
    /// the cons-spine cursor) is pushed here for the duration of that call, then
    /// re-read via [`root_at`](Self::root_at) afterwards (the copying collector
    /// relocates these in place). This is what lets the safepoint collect at
    /// **any** eval depth, not just the outermost — see `docs/memory-model.md`.
    /// Also used by `eval_str`/`eval_source` for the unevaluated forms vector.
    /// Empty between top-level forms.
    roots: Vec<Value>,
    /// The env half of the operand stack (ADR-061): LOCAL [`EnvId`]s an eval
    /// frame still needs across a nested `eval` (its `scope`/`env`). Relocated in
    /// place by [`arena_flip`](Self::arena_flip) alongside `roots`; re-read via
    /// [`env_root_at`](Self::env_root_at). Separate stack because an `EnvId`
    /// isn't a `Value`. Empty between top-level forms.
    env_roots: Vec<EnvId>,
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
    /// The LOCAL **generation epoch** — stamped into every LOCAL handle minted
    /// (the `local_gen` in `alloc_*`), and bumped on every arena flip
    /// ([`arena_flip`](Self::arena_flip), shared by `flush`/`collect`) so the
    /// survivors are re-minted with the new value and any handle held across the
    /// flip without being re-rooted keeps the old one. A debug-only deref check
    /// in the LOCAL accessors compares `handle.generation()` against this and
    /// panics at the bad deref. Per-heap (not per-slot): the bump allocator never
    /// reuses a slot, so a whole-arena flip is the only LOCAL-invalidating event.
    /// See `docs/memory-review.md`.
    local_epoch: u32,
    /// **Write-barrier remembered set.** Old-generation env frames mutated by
    /// [`env_define`](Self::env_define) since the last minor collection — the only
    /// way an old object can come to reference a young one (a frame promoted while
    /// still mid-bind, e.g. a collection during a `let` rhs eval, then bound
    /// further). A minor collection scans these as extra roots and rewrites their
    /// bindings to the promoted handles, then clears the set. Empty on the common
    /// path (binds finish in the nursery). Brood's immutability means env-frame
    /// binding is the sole data mutation, so this is the language's one barrier site.
    remembered: Vec<EnvId>,
    /// The **old-generation** epoch — stamped into tenured handles
    /// (`local_old_gen`) and bumped only by a *major* collection (which moves old
    /// objects). A minor collection leaves old objects in place, so it does **not**
    /// bump this — old handles stay valid across minor GCs. Routed to by the
    /// LOCAL accessors when `handle.is_old()`. See [`local_epoch`](Self::local_epoch)
    /// for the nursery counterpart.
    old_epoch: u32,
    /// Live old-generation object count after the last collection; a *major*
    /// collection is triggered when `old` grows past `2×` this (recomputed each
    /// major), so major GCs stay rare while minors keep the nursery bounded.
    major_threshold: usize,
    /// GC observability counters (Tier-1; `docs/memory-review.md` §7). Bumped by
    /// every [`arena_flip`](Self::arena_flip) — so they count both the automatic
    /// Stage-B safepoint collections and any bare [`flush`](Self::flush) (the
    /// tested arena-flip helper), which share that path. Read out via `(gc-stats)`.
    /// Per-heap (per Brood process), reset
    /// to zero only at process start; survive arena flips (the flip writes them,
    /// it doesn't clear them). `u64` so a long-lived server loop can't wrap them.
    /// `gc_runs` = collections performed; `gc_copied` = cumulative survivors
    /// relocated; `gc_reclaimed` = cumulative objects dropped (live-before minus
    /// survivors). These are *counts of LOCAL objects*, not bytes — the cheap,
    /// traversal-free figure (cf. [`local_bytes`](Self::local_bytes) for a byte
    /// estimate).
    gc_runs: u64,
    gc_copied: u64,
    gc_reclaimed: u64,
    /// Per-process GC **trace** switch (`(gc-trace on/off)`, defaulted from
    /// `BROOD_GC_TRACE`). When set, each minor/major collection prints a one-line
    /// summary to stderr — a Tier-1 observability aid for tests/benchmarks (the
    /// numbers `(gc-stats)` reports as cumulative totals, but per collection as
    /// they happen). Per-process like every other heap field: a spawned child
    /// starts from the `BROOD_GC_TRACE` default, not the parent's setting.
    gc_trace: bool,
    /// Compiling-VM body cache (ADR-076, `BROOD_VM`). Maps a closure handle's raw
    /// bits to its compiled single-arm body, or `None` if the closure isn't
    /// VM-eligible (so we don't re-attempt). Per-process (a `RefCell`, like
    /// `global_ic`). The key is **namespaced** (`VmCacheKey`) because two stable
    /// handle spaces are mixed: a top-level RUNTIME closure is keyed by its own
    /// closure-handle `.0`, while a local-capturing closure (Stage 2c) is keyed by
    /// its **body-code handle** — the closure's `ClosureId` is a LOCAL handle whose
    /// index is recycled after GC, so it can't be a stable key, but the body forms
    /// it points at live in the immovable RUNTIME code region (ADR-076 §2c(a)). The
    /// two spaces share the same numeric range, so the `u8` tag keeps them apart. A
    /// `def` rebind promotes a *new* closure (new handle → new key), so a stale
    /// entry is simply never looked up again. Empty unless `BROOD_VM` is on. `Arc`
    /// so the trampoline can hold the compiled body across a call without borrowing
    /// the cache.
    vm_cache: RefCell<HashMap<VmCacheKey, Option<Arc<crate::eval::compile::CompiledClosure>>>>,
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
// Bump-only allocator (post-supervisor-strip): indices grow monotonically
// per process, no free-list reuse, no mark-sweep. The per-process heap is
// dropped wholesale at process exit; long-running receive loops will (next
// phase) flip the arena on receive. Stale-handle bugs become impossible
// because slots are never reused.
macro_rules! alloc_slot {
    ($self:expr, $field:ident, $value:expr) => {{
        let idx = $self.local.$field.len();
        $self.local.$field.push($value);
        idx
    }};
}

/// The `form_pos` map key for a LOCAL pair: its slab index packed with the
/// generation age bit (bit 32). Nursery and old pairs share one slab-index range,
/// so the age bit keeps their source-position entries from colliding.
#[inline]
fn form_pos_key(id: PairId) -> u64 {
    (id.index() as u64) | ((id.is_old() as u64) << 32)
}

/// True iff `v` is a LOCAL heap object the copying collector relocates — the
/// only kind that must be rooted across a collection safepoint. Atoms (`Int`,
/// `Sym`, `Pid`, …) and shared-region (`PRELUDE`/`RUNTIME`) handles never move,
/// so a copy held across a safepoint stays valid and needs no operand-stack
/// slot. Mirrors exactly the set `push_value`/`flush_value` relocate.
#[inline]
pub fn is_movable(v: Value) -> bool {
    match v {
        Value::Pair(id) => id.region() == LOCAL,
        Value::Vector(id) => id.region() == LOCAL,
        Value::Map(id) => id.region() == LOCAL,
        Value::Str(id) => id.region() == LOCAL,
        Value::Rope(id) => id.region() == LOCAL,
        Value::Fn(id) | Value::Macro(id) => id.region() == LOCAL,
        _ => false,
    }
}

/// A rooted value handle from [`Heap::root`]: either an immovable value kept
/// inline (no operand-stack slot) or the index of a LOCAL slot the collector
/// relocates. Read back with [`Heap::read_root`] after any potential collection.
/// The region check means running compiled/promoted (RUNTIME) or prelude code —
/// the hot path — pays no `Vec` churn, only genuinely LOCAL transients do.
#[derive(Clone, Copy)]
pub enum Root {
    /// An immovable value (atom or `PRELUDE`/`RUNTIME` handle); the inline copy
    /// stays valid across collections.
    Stable(Value),
    /// A movable LOCAL value parked at this operand-root-stack index.
    Slot(usize),
}

/// The [`EnvId`] counterpart of [`Root`] — see [`Heap::root_env`]. The
/// [`EnvId::GLOBAL`] sentinel and RUNTIME-promoted frames are immovable and kept
/// inline; only a LOCAL frame takes a slot.
#[derive(Clone, Copy)]
pub enum EnvRoot {
    Stable(EnvId),
    Slot(usize),
}

/// A key into the compiling-VM body cache ([`Heap::vm_cache_get`]). Two stable
/// handle spaces are namespaced apart (ADR-076 §2c): a top-level closure is keyed
/// by its own RUNTIME [`ClosureId`] handle; a local-capturing closure is keyed by
/// the immovable **body-code handle** its (recycled LOCAL) `ClosureId` points at.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum VmCacheKey {
    /// A top-level / promoted RUNTIME closure, keyed by its closure-handle `.0`.
    Runtime(u64),
    /// A local-capturing closure, keyed by the `.0` of its first body form's
    /// (RUNTIME-stable) handle — the closure handle itself is unstable across GC.
    LocalBody(u64),
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
            old: Slabs::default(),
            local_free: FreeLists::default(),
            #[cfg(debug_assertions)]
            poison: PoisonBits::default(),
            prelude: Arc::default(),
            runtime: Arc::default(),
            global: EnvId::local(0),
            form_pos: HashMap::new(),
            current_file: None,
            compile_ns: None,
            ns_known_names: HashSet::new(),
            imports: HashMap::new(),
            dynamics: Vec::new(),
            global_ic: RefCell::new(SymbolMap::default()),
            roots: Vec::new(),
            env_roots: Vec::new(),
            gc_threshold: usize::MAX,
            gc_enabled: false,
            local_epoch: 0,
            remembered: Vec::new(),
            old_epoch: 0,
            major_threshold: usize::MAX,
            gc_runs: 0,
            gc_copied: 0,
            gc_reclaimed: 0,
            gc_trace: gc_trace_default(),
            vm_cache: RefCell::new(HashMap::new()),
        }
    }

    /// A fresh process heap sharing the given prelude + runtime regions (empty
    /// local slabs). Spawned inner processes pass the *same* `runtime` Arc as
    /// their parent, so they see its global bindings and its later `def`s.
    pub fn with_regions(prelude: Arc<SharedCode>, runtime: Arc<RuntimeCode>) -> Self {
        Heap {
            local: Slabs::default(),
            old: Slabs::default(),
            local_free: FreeLists::default(),
            #[cfg(debug_assertions)]
            poison: PoisonBits::default(),
            prelude,
            runtime,
            global: EnvId::local(0),
            form_pos: HashMap::new(),
            current_file: None,
            compile_ns: None,
            ns_known_names: HashSet::new(),
            imports: HashMap::new(),
            dynamics: Vec::new(),
            global_ic: RefCell::new(SymbolMap::default()),
            roots: Vec::new(),
            env_roots: Vec::new(),
            gc_threshold: gc_floor(),
            gc_enabled: true,
            local_epoch: 0,
            remembered: Vec::new(),
            old_epoch: 0,
            major_threshold: major_floor(),
            gc_runs: 0,
            gc_copied: 0,
            gc_reclaimed: 0,
            gc_trace: gc_trace_default(),
            vm_cache: RefCell::new(HashMap::new()),
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
        debug_assert!(
            slabs.ropes.is_empty(),
            "a Rope cannot appear in the prelude — it is pure Brood with no rope literals",
        );
        // Inline-extract any `Shared` string entries the builder created
        // (~9 prelude docstrings exceed `SHARED_BLOB_THRESHOLD` at the time
        // of writing). PRELUDE is shared `Arc<SharedCode>` across runtimes;
        // `Arc<SharedBlob>` is per-runtime, so leaving them as `Shared` here
        // would entangle their lifetimes. The blob's `Arc` drops as the old
        // `LocalString::Shared` is overwritten — freeing the blob if no other
        // handle remains (none does, at freeze time).
        for entry in slabs.strings.iter_mut() {
            if let LocalString::Shared(arc) = entry {
                let bytes: Vec<u8> = arc.as_bytes().to_vec();
                *entry = LocalString::Inline(
                    String::from_utf8(bytes).expect("prelude blob is valid UTF-8"),
                );
            }
        }
        for p in &mut slabs.pairs {
            p.0 = to_prelude(p.0);
            p.1 = to_prelude(p.1);
        }
        for vec in &mut slabs.vectors {
            for x in vec.iter_mut() {
                *x = to_prelude(*x);
            }
        }
        for map_node in &mut slabs.maps {
            // Re-tag every (k, v) inside the trie node — child `MapId`s
            // need their region bits flipped to PRELUDE too.
            for (k, v) in map_node.data.iter_mut() {
                *k = to_prelude(*k);
                *v = to_prelude(*v);
            }
            for child in map_node.children.iter_mut() {
                *child = MapId::prelude(child.index());
            }
        }
        for c in &mut slabs.closures {
            for arm in c.arms.iter_mut() {
                for f in arm.body.iter_mut() {
                    *f = to_prelude(*f);
                }
                for (_, d) in arm.optionals.iter_mut() {
                    *d = to_prelude(*d);
                }
            }
            // Hard assert (not debug_assert!) — `slabs.envs` is wiped below,
            // so a closure capturing a non-None env would survive into the
            // frozen prelude with a dangling env handle, and the first call
            // would silently index past the empty slab. We want the same
            // failure in release: a clear panic at freeze time, not corrupt
            // state at runtime. The message names the closure so the prelude
            // line that produced it is easy to find.
            assert!(
                c.env.is_none(),
                "shared closures must capture the global env (closure {:?} \
                 has env={:?}); the prelude tried to freeze a closure with a \
                 captured local frame — most likely a `defn`/`def` whose body \
                 closes over a let-bound name instead of a global",
                c.name.map(crate::core::value::symbol_name),
                c.env,
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
            ropes: self.local.ropes.len(),
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
        self.local.ropes.truncate(cp.ropes);
        self.local.closures.truncate(cp.closures);
        self.local.envs.truncate(cp.envs);
        // Drop position metadata for the pairs just reclaimed (indices reused).
        // Keys pack the age bit at bit 32; this checkpoint path is nursery-only,
        // so compare the low-32 slab index against the checkpoint length.
        if !self.form_pos.is_empty() {
            self.form_pos.retain(|&k, _| (k as u32 as usize) < cp.pairs);
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
                self.form_pos.insert(form_pos_key(id), pos);
            }
        }
    }

    /// The recorded source position of a form, if it is a LOCAL list with one.
    pub fn form_pos(&self, v: Value) -> Option<crate::error::Pos> {
        if let Value::Pair(id) = v {
            if id.region() == crate::core::value::LOCAL {
                return self.form_pos.get(&form_pos_key(id)).copied();
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

    // ----- current namespace (ADR-065) -----

    /// Set the namespace being compiled into (`None` = root), returning the prior
    /// value so the caller can restore it. File/module loaders save + reset to
    /// `None` per file; the `%in-ns` primitive sets it from an `(ns …)` form.
    pub fn set_compile_ns(&mut self, ns: Option<Symbol>) -> Option<Symbol> {
        std::mem::replace(&mut self.compile_ns, ns)
    }

    /// The namespace currently being compiled into, or `None` at root.
    pub fn compile_ns(&self) -> Option<Symbol> {
        self.compile_ns
    }

    /// Record the bare names the current-namespace file will define, so the
    /// resolver can qualify forward references. Returns the prior set so the
    /// caller can restore it (loads nest).
    pub fn set_ns_known_names(&mut self, names: HashSet<Symbol>) -> HashSet<Symbol> {
        std::mem::replace(&mut self.ns_known_names, names)
    }

    /// Is `sym` (a bare name) known to be defined in the current namespace's file?
    pub fn ns_knows_name(&self, sym: Symbol) -> bool {
        self.ns_known_names.contains(&sym)
    }

    /// Record one more bare name as defined in the current namespace's file. Used
    /// by the resolver when it qualifies a `def` head whose name the up-front
    /// forward-ref scan missed — a name produced by a *macro* expansion (e.g.
    /// `defprocess` → `(def counter …)`), which `scan_def_names` can't see in the
    /// raw form. Registering it before the def's body is resolved lets self-
    /// references (the recursion in `counter`'s loop) qualify to the same name.
    pub fn add_ns_known_name(&mut self, sym: Symbol) {
        self.ns_known_names.insert(sym);
    }

    /// Replace the current file's `(:use …)` import table, returning the prior one
    /// so the caller can restore it (loads nest). Maps bare → qualified.
    pub fn set_imports(&mut self, imports: HashMap<Symbol, Symbol>) -> HashMap<Symbol, Symbol> {
        std::mem::replace(&mut self.imports, imports)
    }

    /// Add one imported binding (bare name → qualified global). Used by `%refer`.
    pub fn add_import(&mut self, bare: Symbol, qualified: Symbol) {
        self.imports.insert(bare, qualified);
    }

    /// The qualified global a bare name was `(:use …)`-imported to, if any.
    pub fn import_of(&self, bare: Symbol) -> Option<Symbol> {
        self.imports.get(&bare).copied()
    }

    /// Every `(bare, qualified)` import pair in the current file's table — for the
    /// LSP to offer imported names as bare completion candidates (ADR-065 §6).
    pub fn imported_pairs(&self) -> Vec<(Symbol, Symbol)> {
        self.imports.iter().map(|(&b, &q)| (b, q)).collect()
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
            // Qualify the recorded name to the current namespace (ADR-065) so the
            // def-site key matches the global the resolver will actually define
            // (`foo/name`); a no-op at root or for an already-qualified name.
            Value::Sym(name) => Some(match self.compile_ns {
                Some(ns) => crate::eval::macros::qualify_name(&crate::core::value::symbol_name(ns), name),
                None => name,
            }),
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
        Value::Pair(PairId::local_gen(idx, self.local_epoch))
    }

    pub fn alloc_vector(&mut self, items: Vec<Value>) -> Value {
        let idx = alloc_slot!(self, vectors, items);
        Value::Vector(VecId::local_gen(idx, self.local_epoch))
    }

    // ===== map operations (ADR-040: CHAMP — see `core/map_champ.rs`) =====
    //
    // Every op returns a fresh `Value::Map` handle; the trie is path-copied
    // from root to the touched leaf, with the rest structurally shared.
    // None of these mutate any existing `MapNode` — the slab is append-only
    // from the language's point of view, which is what makes RUNTIME/PRELUDE
    // maps safely shareable across processes.

    /// Allocate a fresh empty map — a single root `MapNode` with no
    /// entries. Used by `(hash-map)` with no args and as the starting
    /// point for `map_from_pairs`.
    pub fn alloc_empty_map(&mut self) -> Value {
        let idx = alloc_slot!(self, maps, MapNode::default());
        Value::Map(MapId::local_gen(idx, self.local_epoch))
    }

    /// The value `key` maps to, by structural equality, or `None` if absent.
    /// O(log₁₆ N) — one 4-bit hash slice + one bitmap test per trie level.
    pub fn map_get(&self, id: MapId, key: Value) -> Option<Value> {
        let hash = self.hash_value(key);
        self.champ_get(id, key, hash, 0)
    }

    fn champ_get(&self, id: MapId, key: Value, hash: u64, depth: u32) -> Option<Value> {
        let node = self.map_node(id);
        if node.is_collision {
            return node
                .data
                .iter()
                .find(|(k, _)| self.equal(*k, key))
                .map(|(_, v)| *v);
        }
        let slot = map_champ::slot_at(hash, depth);
        let bit = map_champ::slot_mask(slot);
        if node.data_map & bit != 0 {
            let i = map_champ::rank(node.data_map, slot);
            let (k, v) = node.data[i];
            if self.equal(k, key) {
                Some(v)
            } else {
                None
            }
        } else if node.node_map & bit != 0 {
            let j = map_champ::rank(node.node_map, slot);
            self.champ_get(node.children[j], key, hash, depth + 1)
        } else {
            None
        }
    }

    /// A fresh map with `key` bound to `val` — replaces or inserts in
    /// O(log₁₆ N). Path-copies only the nodes from root to the touched
    /// leaf; every other node is structurally shared with the input map.
    pub fn map_assoc(&mut self, id: MapId, key: Value, val: Value) -> Value {
        let hash = self.hash_value(key);
        let new_root = self.champ_assoc(id, key, val, hash, 0);
        Value::Map(new_root)
    }

    fn champ_assoc(&mut self, id: MapId, key: Value, val: Value, hash: u64, depth: u32) -> MapId {
        // Snapshot the node fields we need — releases the immutable borrow
        // on `self` before we go allocating new slots.
        let node = self.map_node(id);
        let is_collision = node.is_collision;
        let data_map = node.data_map;
        let node_map = node.node_map;

        if is_collision {
            // At max depth — all entries share the full hash. Linear scan
            // by `equal`.
            let data = node.data.clone();
            let pos = data.iter().position(|(k, _)| self.equal(*k, key));
            let (new_data, delta) = match pos {
                Some(i) => {
                    let mut d = data;
                    d[i].1 = val;
                    (d, 0i64)
                }
                None => {
                    let mut d = data;
                    d.push((key, val));
                    (d, 1i64)
                }
            };
            let new_size = node.size as i64 + delta;
            let new_node = MapNode {
                size: new_size as u32,
                data_map: 0,
                node_map: 0,
                is_collision: true,
                data: new_data,
                children: SmallVec::new(),
            };
            return self.alloc_map_node(new_node);
        }

        let slot = map_champ::slot_at(hash, depth);
        let bit = map_champ::slot_mask(slot);

        // Case 1: slot already holds an inline (k, v) entry.
        if data_map & bit != 0 {
            let i = map_champ::rank(data_map, slot);
            let (existing_k, existing_v) = node.data[i];
            if self.equal(existing_k, key) {
                // Overwrite. If the value is identical by `equal`, we could
                // return id unchanged — but assoc's contract is "returns a
                // fresh map", and callers can dedup themselves if they care.
                let mut new_data = node.data.clone();
                new_data[i].1 = val;
                let new_node = MapNode {
                    size: node.size,
                    data_map,
                    node_map,
                    is_collision: false,
                    data: new_data,
                    children: node.children.clone(),
                };
                return self.alloc_map_node(new_node);
            }
            // Different key hashed to same slot. Split: turn this inline
            // entry into a child sub-node holding both pairs.
            let other_hash = self.hash_value(existing_k);
            let child_id = self.champ_split(
                existing_k,
                existing_v,
                other_hash,
                key,
                val,
                hash,
                depth + 1,
            );
            let node = self.map_node(id); // re-borrow after the recursive alloc
            let new_data_map = data_map ^ bit;
            let new_node_map = node_map | bit;
            let mut new_data = node.data.clone();
            new_data.remove(i);
            let child_pos = map_champ::rank(new_node_map, slot);
            let mut new_children = node.children.clone();
            new_children.insert(child_pos, child_id);
            let new_node = MapNode {
                size: node.size + 1,
                data_map: new_data_map,
                node_map: new_node_map,
                is_collision: false,
                data: new_data,
                children: new_children,
            };
            return self.alloc_map_node(new_node);
        }

        // Case 2: slot holds a child sub-node — recurse, then patch the
        // child handle.
        if node_map & bit != 0 {
            let j = map_champ::rank(node_map, slot);
            let old_child = node.children[j];
            let old_child_size = self.map_node(old_child).size;
            let new_child = self.champ_assoc(old_child, key, val, hash, depth + 1);
            let new_child_size = self.map_node(new_child).size;
            let node = self.map_node(id);
            let mut new_children = node.children.clone();
            new_children[j] = new_child;
            let new_node = MapNode {
                size: node.size + new_child_size - old_child_size,
                data_map,
                node_map,
                is_collision: false,
                data: node.data.clone(),
                children: new_children,
            };
            return self.alloc_map_node(new_node);
        }

        // Case 3: empty slot — insert a fresh inline entry.
        let new_data_map = data_map | bit;
        let new_data_pos = map_champ::rank(new_data_map, slot);
        let mut new_data = node.data.clone();
        new_data.insert(new_data_pos, (key, val));
        let new_node = MapNode {
            size: node.size + 1,
            data_map: new_data_map,
            node_map,
            is_collision: false,
            data: new_data,
            children: node.children.clone(),
        };
        self.alloc_map_node(new_node)
    }

    /// Build a sub-node holding two entries with different keys but
    /// possibly the same slot at `depth`. Recursively descends until
    /// the two keys' hash slices diverge (or until [`MAX_DEPTH`], where
    /// it spawns a collision leaf). Used by `champ_assoc`'s split case.
    //
    // 8 args: two (k, v, h) triples + depth + &mut self. Bundling the
    // triples into a struct adds noise for an internal-only helper called
    // from one site.
    #[allow(clippy::too_many_arguments)]
    fn champ_split(
        &mut self,
        k1: Value,
        v1: Value,
        h1: u64,
        k2: Value,
        v2: Value,
        h2: u64,
        depth: u32,
    ) -> MapId {
        if depth >= MAX_DEPTH {
            // Hash exhausted — both keys hash identically. Collision leaf.
            let mut data = SmallVec::<[(Value, Value); 4]>::new();
            data.push((k1, v1));
            data.push((k2, v2));
            return self.alloc_map_node(MapNode {
                size: 2,
                data_map: 0,
                node_map: 0,
                is_collision: true,
                data,
                children: SmallVec::new(),
            });
        }
        let s1 = map_champ::slot_at(h1, depth);
        let s2 = map_champ::slot_at(h2, depth);
        if s1 == s2 {
            // Still colliding at this level — recurse.
            let child = self.champ_split(k1, v1, h1, k2, v2, h2, depth + 1);
            let bit = map_champ::slot_mask(s1);
            let mut children = SmallVec::<[MapId; 4]>::new();
            children.push(child);
            return self.alloc_map_node(MapNode {
                size: 2,
                data_map: 0,
                node_map: bit,
                is_collision: false,
                data: SmallVec::new(),
                children,
            });
        }
        // Diverged: two inline entries in the new node, ordered by slot.
        let (lo_slot, lo_kv, hi_slot, hi_kv) = if s1 < s2 {
            (s1, (k1, v1), s2, (k2, v2))
        } else {
            (s2, (k2, v2), s1, (k1, v1))
        };
        let data_map = map_champ::slot_mask(lo_slot) | map_champ::slot_mask(hi_slot);
        let mut data = SmallVec::<[(Value, Value); 4]>::new();
        data.push(lo_kv);
        data.push(hi_kv);
        self.alloc_map_node(MapNode {
            size: 2,
            data_map,
            node_map: 0,
            is_collision: false,
            data,
            children: SmallVec::new(),
        })
    }

    /// A fresh map with `key` removed; a clone of the same shape if
    /// `key` was absent. Path-copies the affected branch; collapses
    /// singleton sub-trees into the parent's inline data (the CHAMP
    /// canonicalisation rule that keeps the tree shallow).
    pub fn map_dissoc(&mut self, id: MapId, key: Value) -> Value {
        let hash = self.hash_value(key);
        let new_root = self.champ_dissoc(id, key, hash, 0);
        Value::Map(new_root)
    }

    fn champ_dissoc(&mut self, id: MapId, key: Value, hash: u64, depth: u32) -> MapId {
        let node = self.map_node(id);
        let is_collision = node.is_collision;

        if is_collision {
            let pos = node.data.iter().position(|(k, _)| self.equal(*k, key));
            let Some(i) = pos else {
                return self.clone_map_node(id);
            };
            let mut new_data = node.data.clone();
            new_data.remove(i);
            return self.alloc_map_node(MapNode {
                size: node.size - 1,
                data_map: 0,
                node_map: 0,
                is_collision: true,
                data: new_data,
                children: SmallVec::new(),
            });
        }

        let slot = map_champ::slot_at(hash, depth);
        let bit = map_champ::slot_mask(slot);
        let data_map = node.data_map;
        let node_map = node.node_map;

        // Case 1: inline entry at this slot.
        if data_map & bit != 0 {
            let i = map_champ::rank(data_map, slot);
            if !self.equal(node.data[i].0, key) {
                return self.clone_map_node(id); // key absent
            }
            let new_data_map = data_map ^ bit;
            let mut new_data = node.data.clone();
            new_data.remove(i);
            return self.alloc_map_node(MapNode {
                size: node.size - 1,
                data_map: new_data_map,
                node_map,
                is_collision: false,
                data: new_data,
                children: node.children.clone(),
            });
        }

        // Case 2: child sub-node at this slot — recurse and patch.
        if node_map & bit != 0 {
            let j = map_champ::rank(node_map, slot);
            let old_child = node.children[j];
            let old_child_size = self.map_node(old_child).size;
            let new_child = self.champ_dissoc(old_child, key, hash, depth + 1);
            let new_child_node = self.map_node(new_child);
            let new_child_size = new_child_node.size;
            if new_child_size == old_child_size {
                // No change (key was absent below).
                return self.clone_map_node(id);
            }
            // Promote: if the child shrunk to a singleton (one entry, no
            // children — branch *or* collision leaf), inline it here.
            // Collision leaves are legitimate singletons: the surviving
            // entry's hash still routes through this slot at this depth,
            // so inlining is safe and keeps the trie shallow.
            if new_child_node.is_singleton() {
                let (kk, vv) = new_child_node.data[0];
                let node = self.map_node(id);
                let new_node_map = node_map ^ bit;
                let new_data_map = data_map | bit;
                let mut new_children = node.children.clone();
                new_children.remove(j);
                let new_data_pos = map_champ::rank(new_data_map, slot);
                let mut new_data = node.data.clone();
                new_data.insert(new_data_pos, (kk, vv));
                return self.alloc_map_node(MapNode {
                    size: node.size - 1,
                    data_map: new_data_map,
                    node_map: new_node_map,
                    is_collision: false,
                    data: new_data,
                    children: new_children,
                });
            }
            // If the child is now empty entirely, drop the reference.
            if new_child_node.is_empty() {
                let node = self.map_node(id);
                let new_node_map = node_map ^ bit;
                let mut new_children = node.children.clone();
                new_children.remove(j);
                return self.alloc_map_node(MapNode {
                    size: node.size - 1,
                    data_map,
                    node_map: new_node_map,
                    is_collision: false,
                    data: node.data.clone(),
                    children: new_children,
                });
            }
            // Otherwise just swap the child handle.
            let node = self.map_node(id);
            let mut new_children = node.children.clone();
            new_children[j] = new_child;
            return self.alloc_map_node(MapNode {
                size: node.size - old_child_size + new_child_size,
                data_map,
                node_map,
                is_collision: false,
                data: node.data.clone(),
                children: new_children,
            });
        }

        // Case 3: empty slot — key absent.
        self.clone_map_node(id)
    }

    /// Build a canonical map from raw `(key, value)` pairs, applying
    /// last-wins de-dup by structural equality. Used by the `{ }` literal
    /// reader path and `(hash-map …)`. Folds `assoc` over a fresh empty
    /// root — O(N log N) overall, in line with CHAMP's per-op cost.
    pub fn map_from_pairs(&mut self, pairs: Vec<(Value, Value)>) -> Value {
        let mut current = match self.alloc_empty_map() {
            Value::Map(id) => id,
            _ => unreachable!("alloc_empty_map returns Value::Map"),
        };
        for (k, v) in pairs {
            let next = match self.map_assoc(current, k, v) {
                Value::Map(id) => id,
                _ => unreachable!("map_assoc returns Value::Map"),
            };
            current = next;
        }
        Value::Map(current)
    }

    /// All entries in the map, walked depth-first through the trie.
    /// Order is deterministic per shape (slot-index ascending at each
    /// level, then collision-leaf order) but is **not** insertion order
    /// — ADR-040's one contract change vs ADR-030. Callers that need an
    /// ordered set should sort the result.
    pub fn map_entries(&self, id: MapId) -> Vec<(Value, Value)> {
        let mut out = Vec::with_capacity(self.map_size(id));
        self.collect_entries_into(id, &mut out);
        out
    }

    fn collect_entries_into(&self, id: MapId, out: &mut Vec<(Value, Value)>) {
        let node = self.map_node(id);
        for &kv in &node.data {
            out.push(kv);
        }
        if !node.is_collision {
            // children are in slot-ascending order — that's our traversal.
            for &child in &node.children {
                self.collect_entries_into(child, out);
            }
        }
    }

    /// Walk every entry in the map, calling `f(k, v)` on each. Borrow-friendly
    /// alternative to `map_entries` when the caller doesn't need a Vec — used by
    /// `hash_value_into` where allocating per call would be wasteful.
    pub fn fold_entries(&self, id: MapId, f: &mut dyn FnMut(Value, Value)) {
        let node = self.map_node(id);
        for &(k, v) in &node.data {
            f(k, v);
        }
        if !node.is_collision {
            for &child in &node.children {
                self.fold_entries(child, f);
            }
        }
    }

    /// Number of entries in the map. O(1) — every node tracks the size
    /// of its own subtree, so the root's `size` is the answer.
    pub fn map_size(&self, id: MapId) -> usize {
        self.map_node(id).size as usize
    }

    /// True if `id` resolves to a map with `key` as one of its keys (so
    /// `(contains? m k)` distinguishes a stored `nil`/`false` from absence
    /// — both are valid stored values, only "not bound" returns false here).
    /// Same cost as `map_get`; we delegate rather than duplicate the trie
    /// walk.
    pub fn map_contains(&self, id: MapId, key: Value) -> bool {
        self.map_get(id, key).is_some()
    }

    /// Allocate a new map node — the path-copy primitive every assoc /
    /// dissoc step ends with. Returns the `MapId` (not a `Value`) so
    /// internal callers can stitch children together before wrapping the
    /// root in `Value::Map`.
    fn alloc_map_node(&mut self, node: MapNode) -> MapId {
        let idx = alloc_slot!(self, maps, node);
        MapId::local_gen(idx, self.local_epoch)
    }

    /// A fresh root `MapNode` slot holding the same shape as `id`. The
    /// child handles are reused (structural sharing extends one level
    /// out from the root), so this is `O(branching)`, not deep. Used by
    /// `dissoc` when the key was absent — the surface contract is
    /// "every op returns a fresh map handle", and an unconditional
    /// root clone keeps that honest without touching the unchanged
    /// subtree.
    fn clone_map_node(&mut self, id: MapId) -> MapId {
        let node = self.map_node(id);
        let cloned = MapNode {
            size: node.size,
            data_map: node.data_map,
            node_map: node.node_map,
            is_collision: node.is_collision,
            data: node.data.clone(),
            children: node.children.clone(),
        };
        self.alloc_map_node(cloned)
    }

    /// The single chokepoint for materialising a `Value::Str` into LOCAL. Routes
    /// by size: strings of [`SHARED_BLOB_THRESHOLD`] bytes or more allocate an
    /// `Arc<SharedBlob>` so a later cross-process send can ship a handle
    /// instead of copying the bytes; smaller strings stay inline because
    /// atomic-refcount traffic dominates the per-byte memcpy at small sizes.
    /// Every `String -> Value::Str` path must come through here — don't add a
    /// second allocator that bypasses the threshold.
    pub fn alloc_string(&mut self, s: &str) -> Value {
        let entry = if s.len() >= SHARED_BLOB_THRESHOLD {
            LocalString::Shared(SharedBlob::new(s.as_bytes()))
        } else {
            LocalString::Inline(s.to_string())
        };
        let idx = self.local.strings.len();
        self.local.strings.push(entry);
        Value::Str(StrId::local_gen(idx, self.local_epoch))
    }

    /// Materialise a `Value::Rope` into LOCAL from an owned `ropey::Rope`
    /// (ADR-045). Bump-only like the other allocators; the rope's internal
    /// `Arc` nodes mean this stores one cheap handle, not a byte copy.
    pub fn alloc_rope(&mut self, r: ropey::Rope) -> Value {
        let idx = self.local.ropes.len();
        self.local.ropes.push(r);
        Value::Rope(RopeId::local_gen(idx, self.local_epoch))
    }

    /// Resolve a rope handle to its `&ropey::Rope`. LOCAL slots are the common
    /// case; RUNTIME holds a rope `def`'d to a global (shared read-only across
    /// the runtime's processes). There is no PRELUDE rope (see `to_prelude`).
    pub fn rope(&self, id: RopeId) -> &ropey::Rope {
        match id.region() {
            LOCAL if id.is_old() => {
                #[cfg(debug_assertions)]
                self.check_epoch_aged(true, id.generation(), id.index(), "rope", id.0);
                &self.old.ropes[id.index()]
            }
            LOCAL => {
                #[cfg(debug_assertions)]
                debug_assert!(
                    !PoisonBits::is(&self.poison.ropes, id.index()),
                    "use-after-GC: rope() on freed nursery ropes slot {} (handle {:#x}).",
                    id.index(),
                    id.0
                );
                #[cfg(debug_assertions)]
                self.check_epoch_aged(false, id.generation(), id.index(), "rope", id.0);
                &self.local.ropes[id.index()]
            }
            RUNTIME => self
                .runtime
                .code
                .ropes
                .get(id.index())
                .expect("runtime rope handle"),
            _ => unreachable!("Rope handles live only in LOCAL or RUNTIME"),
        }
    }

    /// Install a pre-existing `Arc<SharedBlob>` as a new LOCAL string slot.
    /// Used by the receive path ([`crate::process::message::from_message`]):
    /// the sender already bumped the refcount via `Arc::clone` for the
    /// `Message`, so installing it here is just slot bookkeeping — no copy.
    pub(crate) fn alloc_string_from_shared(&mut self, blob: Arc<SharedBlob>) -> Value {
        let idx = self.local.strings.len();
        self.local.strings.push(LocalString::Shared(blob));
        Value::Str(StrId::local_gen(idx, self.local_epoch))
    }

    /// A LOCAL string slot, routed to the nursery or old generation by the
    /// handle's age bit. Caller must have checked `id.region() == LOCAL`. Not
    /// debug-gated — the production `local_shared_blob` path uses it too.
    fn string_slot(&self, id: StrId) -> &LocalString {
        if id.is_old() {
            &self.old.strings[id.index()]
        } else {
            &self.local.strings[id.index()]
        }
    }

    /// Debug-only: the underlying `SharedBlob` address for a LOCAL Shared
    /// string, used by the `%blob-ptr` primitive for identity assertions in
    /// cross-process tests. `None` for an inline string or a non-LOCAL handle.
    /// Does **not** clone the `Arc`, so the read leaves the refcount
    /// untouched. Honours the GC poison bitmap — a use-after-flush trips an
    /// assertion at the call site, the same as every other LOCAL accessor.
    #[cfg(debug_assertions)]
    pub(crate) fn local_shared_blob_ptr(&self, id: StrId) -> Option<*const SharedBlob> {
        if id.region() != LOCAL {
            return None;
        }
        if !id.is_old() {
            debug_assert!(
                !PoisonBits::is(&self.poison.strings, id.index()),
                "use-after-GC: local_shared_blob_ptr() on freed nursery strings slot {} (handle {:#x}).",
                id.index(),
                id.0
            );
        }
        self.check_epoch_aged(id.is_old(), id.generation(), id.index(), "local_shared_blob_ptr", id.0);
        match self.string_slot(id) {
            LocalString::Shared(arc) => Some(Arc::as_ptr(arc)),
            LocalString::Inline(_) => None,
        }
    }

    /// Debug-only: the current `Arc::strong_count` for a LOCAL Shared string.
    /// Used by `%blob-strong-count` for leak-check assertions; like
    /// [`Self::local_shared_blob_ptr`] this does not bump the count, so the
    /// reading caller doesn't itself perturb the value it's checking.
    /// Honours the poison bitmap.
    #[cfg(debug_assertions)]
    pub(crate) fn local_shared_blob_strong_count(&self, id: StrId) -> Option<usize> {
        if id.region() != LOCAL {
            return None;
        }
        if !id.is_old() {
            debug_assert!(
                !PoisonBits::is(&self.poison.strings, id.index()),
                "use-after-GC: local_shared_blob_strong_count() on freed nursery strings slot {} \
                 (handle {:#x}).",
                id.index(),
                id.0
            );
        }
        self.check_epoch_aged(
            id.is_old(),
            id.generation(),
            id.index(),
            "local_shared_blob_strong_count",
            id.0,
        );
        match self.string_slot(id) {
            LocalString::Shared(arc) => Some(Arc::strong_count(arc)),
            LocalString::Inline(_) => None,
        }
    }

    /// If `id` is a LOCAL `Shared` string, return a cloned `Arc<SharedBlob>`
    /// (atomic incr, no byte copy). Otherwise return `None` so the caller
    /// falls back to the byte-copying [`Self::string`] path. Used by
    /// [`crate::process::message::to_message`] to ship big strings between
    /// processes without copying.
    pub(crate) fn local_shared_blob(&self, id: StrId) -> Option<Arc<SharedBlob>> {
        if id.region() != LOCAL {
            return None;
        }
        #[cfg(debug_assertions)]
        if !id.is_old() {
            debug_assert!(
                !PoisonBits::is(&self.poison.strings, id.index()),
                "use-after-GC: local_shared_blob() on freed nursery strings slot {} (handle {:#x}).",
                id.index(),
                id.0
            );
        }
        #[cfg(debug_assertions)]
        self.check_epoch_aged(id.is_old(), id.generation(), id.index(), "local_shared_blob", id.0);
        match self.string_slot(id) {
            LocalString::Shared(arc) => Some(Arc::clone(arc)),
            LocalString::Inline(_) => None,
        }
    }

    pub fn alloc_closure(&mut self, mut c: Closure) -> ClosureId {
        // Precompute each arm's thin-wrapper redirect once, here at the single
        // closure-construction choke point — every LOCAL closure (`fn`/`defn`,
        // and a message-rebuilt one) flows through here. promote/freeze copy the
        // result verbatim, so it never has to be re-derived per call (see
        // `eval::passthrough_arm` and `ClosureArm::passthrough`).
        for arm in &mut c.arms {
            if arm.passthrough.is_none() {
                arm.passthrough = self.compute_passthrough(arm);
            }
        }
        let idx = alloc_slot!(self, closures, c);
        ClosureId::local_gen(idx, self.local_epoch)
    }

    /// Analyse whether `arm` is a pure pass-through wrapper — a single body form
    /// `(head p_i p_j …)` with no `&optional`/`&` rest, `head` an ordinary
    /// function reference (not a special form, not one of the arm's own params),
    /// and every argument one of the arm's parameters used directly. Returns the
    /// forwarding `(head, map)` if so. A pure function of the immutable arm, run
    /// once at allocation; mirrors the predicate `eval::passthrough_arm` used to
    /// recompute on every call.
    fn compute_passthrough(&self, arm: &ClosureArm) -> Option<Passthrough> {
        if !arm.optionals.is_empty() || arm.rest.is_some() || arm.body.len() != 1 {
            return None;
        }
        let (head, mut rest) = match arm.body[0] {
            Value::Pair(p) => self.pair(p),
            _ => return None,
        };
        let head_sym = match head {
            Value::Sym(s) => s,
            _ => return None,
        };
        if crate::eval::is_special_form(head_sym) || arm.params.iter().any(|&p| p == head_sym) {
            return None;
        }
        let mut map: SmallVec<[usize; 4]> = SmallVec::new();
        loop {
            match rest {
                Value::Nil => break,
                Value::Pair(p) => {
                    let (a, next) = self.pair(p);
                    let asym = match a {
                        Value::Sym(s) => s,
                        _ => return None, // a literal / nested call — not a pure forward
                    };
                    map.push(arm.params.iter().position(|&p| p == asym)?);
                    rest = next;
                }
                _ => return None, // improper arg list
            }
        }
        Some(Passthrough { head, map })
    }

    pub fn alloc_native(&mut self, f: NativeFn) -> Value {
        // Natives are only allocated during the prelude build (then frozen into
        // PRELUDE); the LOCAL natives slab stays empty at runtime and isn't
        // swept, so there's no free list to consult.
        let idx = self.local.natives.len();
        self.local.natives.push(f);
        Value::Native(NativeId::local_gen(idx, self.local_epoch))
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
        let mut fwd = PromoteForward::default();
        self.promote_in(v, &mut fwd)
    }

    /// The recursive core of [`promote`](Self::promote), threading a forwarding
    /// table so a *cyclic* graph (a closure capturing its own binding scope)
    /// terminates: closures and envs reserve their RUNTIME slot and register it in
    /// `fwd` *before* recursing, so the back-edge resolves to the reserved handle
    /// instead of recursing forever. The table also collapses shared (DAG)
    /// closures/envs to one RUNTIME copy. Pairs/vectors/maps/strings/ropes are
    /// acyclic by construction (immutable, built bottom-up), so they aren't
    /// forwarded — they just recurse through `fwd` to reach any closures inside.
    fn promote_in(&self, v: Value, fwd: &mut PromoteForward) -> Value {
        match v {
            Value::Str(id) if id.region() == LOCAL => {
                let s = self.string(id).to_string();
                Value::Str(StrId::runtime(self.runtime.code.strings.push(s)))
            }
            Value::Rope(id) if id.region() == LOCAL => {
                // Cheap `Arc`-node clone into the shared region; the rope is
                // immutable, so sibling processes read it concurrently.
                let r = self.rope(id).clone();
                Value::Rope(RopeId::runtime(self.runtime.code.ropes.push(r)))
            }
            Value::Pair(id) if id.region() == LOCAL => self.promote_list(id, fwd),
            Value::Vector(id) if id.region() == LOCAL => {
                let items: Vec<Value> = self
                    .vector(id)
                    .to_vec()
                    .into_iter()
                    .map(|x| self.promote_in(x, fwd))
                    .collect();
                Value::Vector(VecId::runtime(self.runtime.code.vectors.push(items)))
            }
            Value::Map(id) if id.region() == LOCAL => {
                // Recursively promote the trie depth-first. Children are
                // promoted before their parent so the parent's `children`
                // array can be wired to the freshly-allocated RUNTIME
                // sub-node handles.
                Value::Map(self.promote_map_node(id, fwd))
            }
            Value::Fn(id) if id.region() == LOCAL => Value::Fn(self.promote_closure(id, fwd)),
            Value::Macro(id) if id.region() == LOCAL => Value::Macro(self.promote_closure(id, fwd)),
            // Atoms, and values already in PRELUDE/RUNTIME, need no copy.
            _ => v,
        }
    }

    /// Promote a local cons-chain. Walks the `cdr` spine *iteratively* so a long
    /// list doesn't recurse its length deep (which overflowed the native stack);
    /// recursion is bounded by element nesting via `promote_in` on each `car`.
    /// Stops at the first already-shared cell or non-pair tail, preserving both
    /// improper (dotted) lists and existing structure sharing.
    fn promote_list(&self, first: PairId, fwd: &mut PromoteForward) -> Value {
        let mut heads = Vec::new();
        let mut cur = Value::Pair(first);
        let tail = loop {
            match cur {
                Value::Pair(id) if id.region() == LOCAL => {
                    let (head, next) = self.pair(id);
                    heads.push(self.promote_in(head, fwd));
                    cur = next;
                }
                other => break self.promote_in(other, fwd),
            }
        };
        let mut acc = tail;
        for head in heads.into_iter().rev() {
            acc = Value::Pair(PairId::runtime(self.runtime.code.pairs.push((head, acc))));
        }
        acc
    }

    /// Promote a LOCAL CHAMP trie into the shared RUNTIME region. Walks
    /// depth-first: child sub-nodes are promoted before their parent so
    /// the parent's `children` array references the new RUNTIME handles.
    /// Every `(k, v)` entry is promoted recursively (matches `promote_in`
    /// on vectors / lists). The result is a brand-new trie in RUNTIME;
    /// the original LOCAL trie is left untouched (it'll be GC'd when its
    /// last reference goes).
    fn promote_map_node(&self, id: MapId, fwd: &mut PromoteForward) -> MapId {
        let node = self.map_node(id);
        // Promote children first (bottom-up) so the new RUNTIME node can
        // be built with the new child handles in one push.
        let new_children: SmallVec<[MapId; 4]> = node
            .children
            .iter()
            .map(|&c| match c.region() {
                LOCAL => self.promote_map_node(c, fwd),
                _ => c, // already shared
            })
            .collect();
        let new_data: SmallVec<[(Value, Value); 4]> = node
            .data
            .iter()
            .map(|&(k, v)| (self.promote_in(k, fwd), self.promote_in(v, fwd)))
            .collect();
        let promoted = MapNode {
            size: node.size,
            data_map: node.data_map,
            node_map: node.node_map,
            is_collision: node.is_collision,
            data: new_data,
            children: new_children,
        };
        MapId::runtime(self.runtime.code.maps.push(promoted))
    }

    fn promote_closure(&self, id: ClosureId, fwd: &mut PromoteForward) -> ClosureId {
        // Already promoted on this walk? Return the shared handle (cycle break +
        // DAG-sharing collapse). Keyed on LOCAL slot index.
        let key = id.index() as u32;
        if let Some(&existing) = fwd.closures.get(&key) {
            return existing;
        }
        // Reserve the RUNTIME slot *first* and register it, so a reference back to
        // this closure reached while promoting its captured scope resolves here
        // rather than recursing forever (e.g. `(let (g (fn () g)) g)`).
        let new_idx = self.runtime.code.closures.push(OnceLock::new());
        let runtime_id = ClosureId::runtime(new_idx);
        fwd.closures.insert(key, runtime_id);
        let cl = self.closure(id).clone();
        // Promote every arm's body forms and `&optional` defaults into the shared
        // region (param symbols and `&` rest are interned/copy, so they ride along).
        let arms = cl
            .arms
            .iter()
            .map(|arm| ClosureArm {
                params: arm.params.clone(),
                optionals: arm
                    .optionals
                    .iter()
                    .map(|&(s, d)| (s, self.promote_in(d, fwd)))
                    .collect(),
                rest: arm.rest,
                body: arm.body.iter().map(|&f| self.promote_in(f, fwd)).collect(),
                // The forwarding head is an interned symbol and the map is plain
                // indices, so the analysis is region-independent — copy it verbatim.
                passthrough: arm.passthrough.clone(),
            })
            .collect();
        // A top-level closure captures the global env (`None`) and is fully
        // shareable as-is. A closure that captured a *local* scope has its scope
        // promoted too, so it resolves its free variables in any process.
        let env = cl.env.map(|e| self.promote_env(e, fwd));
        let promoted = Closure {
            name: cl.name,
            arms,
            doc: cl.doc,
            env,
        };
        // Fill the reserved slot exactly once. The handle isn't published (bound
        // in a global / shipped to a process) until `promote` returns, so nothing
        // can observe the cell before this set.
        self.runtime
            .code
            .closures
            .get(new_idx)
            .expect("reserved closure slot")
            .set(promoted)
            .ok()
            .expect("promote: closure slot filled exactly once");
        runtime_id
    }

    /// Deep-copy an environment frame chain from LOCAL into the shared RUNTIME
    /// region, promoting each bound value. Stops at the global scope (the shared
    /// sentinel). Already-shared (RUNTIME) frames are returned unchanged. Reserves
    /// its slot before recursing (same cycle break as [`promote_closure`]).
    fn promote_env(&self, env: EnvId, fwd: &mut PromoteForward) -> EnvId {
        if env == EnvId::GLOBAL || env.region() == RUNTIME {
            return env;
        }
        let key = env.index() as u32;
        if let Some(&existing) = fwd.envs.get(&key) {
            return existing;
        }
        let new_idx = self.runtime.code.envs.push(OnceLock::new());
        let runtime_id = EnvId::runtime(new_idx);
        fwd.envs.insert(key, runtime_id);
        // Snapshot the frame, then promote its parent and values (no borrow held).
        let (parent, bindings): (Option<EnvId>, Vec<(Symbol, Value)>) = {
            let frame = self.env_frame(env);
            (
                frame.parent,
                frame.vars.iter().map(|&(s, v)| (s, v)).collect(),
            )
        };
        let parent = parent.map(|p| self.promote_env(p, fwd));
        let vars = bindings
            .into_iter()
            .map(|(s, v)| (s, self.promote_in(v, fwd)))
            .collect();
        self.runtime
            .code
            .envs
            .get(new_idx)
            .expect("reserved env slot")
            .set(EnvFrame { vars, parent })
            .ok()
            .expect("promote: env slot filled exactly once");
        runtime_id
    }

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
    /// drops — so it cannot resurrect the slot-aliasing scheduler race that
    /// disabled the old mark-sweep (`collect_old`).
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
        self.local_free.clear();
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
        #[cfg(debug_assertions)]
        {
            self.poison.pairs.clear();
            self.poison.vectors.clear();
            self.poison.maps.clear();
            self.poison.strings.clear();
            self.poison.ropes.clear();
            self.poison.closures.clear();
            self.poison.envs.clear();
        }
        // GC observability (Tier-1). After the flip the free lists are cleared,
        // so `local_live_count()` is exactly the survivor count. Saturating so a
        // pathological wrap can't panic on the collector hot path.
        let survivors = self.local_live_count();
        self.gc_runs = self.gc_runs.saturating_add(1);
        self.gc_copied = self.gc_copied.saturating_add(survivors as u64);
        self.gc_reclaimed = self
            .gc_reclaimed
            .saturating_add(before.saturating_sub(survivors) as u64);
        // `old` drops here, releasing every LOCAL slot the previous iteration
        // ever allocated.
    }

    // ----- access (dispatch on the handle's region) -----

    /// Generation-aware epoch tripwire. Young (`is_old == false`) handles are
    /// checked against the nursery epoch (bumped by every collection); old handles
    /// against the old-generation epoch (bumped only by a major collection, since a
    /// minor leaves old objects in place). A mismatch means a handle was held
    /// across a collection that moved its space without being re-rooted. Only the
    /// debug-gated accessors call it, so it's `cfg(debug_assertions)` too (no
    /// release dead-code).
    #[cfg(debug_assertions)]
    fn check_epoch_aged(&self, is_old: bool, gen: u32, index: usize, what: &str, raw: u64) {
        let (expected, space) = if is_old {
            (self.old_epoch, "OLD")
        } else {
            (self.local_epoch, "nursery")
        };
        debug_assert!(
            gen == expected,
            "use-after-GC: {} handle ({} slot {}) is from epoch {}, but that generation is \
             now epoch {} — a handle held across a collection without being re-rooted \
             (handle {:#x}).",
            what,
            space,
            index,
            gen,
            expected,
            raw,
        );
    }

    pub fn pair(&self, id: PairId) -> (Value, Value) {
        match id.region() {
            LOCAL if id.is_old() => {
                #[cfg(debug_assertions)]
                self.check_epoch_aged(true, id.generation(), id.index(), "pair", id.0);
                self.old.pairs[id.index()]
            }
            LOCAL => {
                #[cfg(debug_assertions)]
                debug_assert!(
                    !PoisonBits::is(&self.poison.pairs, id.index()),
                    "use-after-GC: pair() on freed nursery pair slot {} \
                     (handle {:#x}).",
                    id.index(),
                    id.0
                );
                #[cfg(debug_assertions)]
                self.check_epoch_aged(false, id.generation(), id.index(), "pair", id.0);
                self.local.pairs[id.index()]
            }
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
    region_ref!(map_node, MapId, maps, &MapNode, "runtime map node");

    /// Resolve a string handle to a `&str`. Hand-written (not via the
    /// `region_ref!` macro) because LOCAL slots are `LocalString` enum
    /// variants that need a match to extract their bytes, while PRELUDE and
    /// RUNTIME store plain `String` (PRELUDE is inline-extracted at freeze;
    /// RUNTIME is append-only via `boxcar::Vec<String>` for stable refs).
    pub fn string(&self, id: StrId) -> &str {
        match id.region() {
            LOCAL if id.is_old() => {
                #[cfg(debug_assertions)]
                self.check_epoch_aged(true, id.generation(), id.index(), "string", id.0);
                self.old.strings[id.index()].as_str()
            }
            LOCAL => {
                #[cfg(debug_assertions)]
                debug_assert!(
                    !PoisonBits::is(&self.poison.strings, id.index()),
                    "use-after-GC: string() on freed nursery strings slot {} (handle {:#x}).",
                    id.index(),
                    id.0
                );
                #[cfg(debug_assertions)]
                self.check_epoch_aged(false, id.generation(), id.index(), "string", id.0);
                self.local.strings[id.index()].as_str()
            }
            // PRELUDE's `Slabs::strings` is also `Vec<LocalString>` because
            // it shares the `Slabs` shape, but `freeze_as_shared_code`
            // inline-extracts any `Shared` entries — every prelude slot is
            // `Inline`. `as_str` works either way.
            PRELUDE => self.prelude.slabs.strings[id.index()].as_str(),
            RUNTIME => self
                .runtime
                .code
                .strings
                .get(id.index())
                .expect("runtime string handle"),
            _ => unreachable!("invalid handle region"),
        }
    }

    /// Resolve a closure handle to its `&Closure`. Hand-written (not via
    /// `region_ref!`) because the RUNTIME slab wraps each entry in a `OnceLock`
    /// (reserve-then-fill cycle break, see `CodeSlabs::closures`); the cell is
    /// always filled before its handle is published, so `get()` is infallible in
    /// practice.
    pub fn closure(&self, id: ClosureId) -> &Closure {
        match id.region() {
            LOCAL if id.is_old() => {
                #[cfg(debug_assertions)]
                self.check_epoch_aged(true, id.generation(), id.index(), "closure", id.0);
                &self.old.closures[id.index()]
            }
            LOCAL => {
                #[cfg(debug_assertions)]
                debug_assert!(
                    !PoisonBits::is(&self.poison.closures, id.index()),
                    "use-after-GC: closure() on freed nursery closures slot {} (handle {:#x}).",
                    id.index(),
                    id.0
                );
                #[cfg(debug_assertions)]
                self.check_epoch_aged(false, id.generation(), id.index(), "closure", id.0);
                &self.local.closures[id.index()]
            }
            PRELUDE => &self.prelude.slabs.closures[id.index()],
            RUNTIME => self
                .runtime
                .code
                .closures
                .get(id.index())
                .expect("runtime closure handle")
                .get()
                .expect("runtime closure read before promote filled its slot"),
            _ => unreachable!("invalid handle region"),
        }
    }

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

    /// A `u64` hash of `v` consistent with [`Heap::equal`]: two values that
    /// `equal` agrees on must hash to the same number. Used by the CHAMP map
    /// (ADR-040) to drive trie navigation — top 4 bits pick the root slot,
    /// next 4 the child, …
    ///
    /// Subtle bits the consistency proof rides on:
    /// - `Float(0.0)` and `Float(-0.0)` hash the same (they compare equal).
    /// - `NaN` ≠ `NaN` per IEEE-754, so two `NaN` keys won't be `equal` and
    ///   needn't hash the same — but a single canonical bit pattern still
    ///   keeps the trie well-typed; pick `u64::MAX` so any NaN routes to one
    ///   leaf where it'll fail the `equal` check anyway.
    /// - Maps are insertion-order-independent: the hash XORs each entry's
    ///   `(k, v)` hash so order doesn't matter (XOR is commutative).
    /// - Pair / Vector hashes feed children into a `DefaultHasher` so
    ///   structure matters; lists with the same `equal` shape hash the same
    ///   regardless of which `Cons` cells they're built from.
    /// - Region bits in handles are ignored — `hash_value` works on
    ///   *structure*, so a LOCAL pair and its PRELUDE-retagged twin land at
    ///   the same key.
    pub fn hash_value(&self, v: Value) -> u64 {
        use std::hash::Hasher;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.hash_value_into(v, &mut h);
        h.finish()
    }

    fn hash_value_into<H: std::hash::Hasher>(&self, v: Value, h: &mut H) {
        use std::hash::{Hash, Hasher};
        // A leading byte tags the variant so a `Sym(0)` and an `Int(0)` never
        // collide on the *exact* same hash by accident.
        match v {
            Value::Nil => 0u8.hash(h),
            Value::Bool(b) => {
                1u8.hash(h);
                b.hash(h);
            }
            Value::Int(i) => {
                2u8.hash(h);
                i.hash(h);
            }
            Value::Float(f) => {
                3u8.hash(h);
                if f.is_nan() {
                    u64::MAX.hash(h);
                } else if f == 0.0 {
                    // 0.0 and -0.0 compare equal; canonicalise to +0.0 bits.
                    0u64.hash(h);
                } else {
                    f.to_bits().hash(h);
                }
            }
            Value::Sym(s) => {
                4u8.hash(h);
                s.hash(h);
            }
            Value::Keyword(s) => {
                5u8.hash(h);
                s.hash(h);
            }
            Value::Str(id) => {
                6u8.hash(h);
                self.string(id).hash(h);
            }
            Value::Pair(id) => {
                7u8.hash(h);
                // Walk the cdr spine iteratively (matches `equal`'s loop).
                let mut cur = id;
                loop {
                    let (car, cdr) = self.pair(cur);
                    self.hash_value_into(car, h);
                    match cdr {
                        Value::Pair(next) => cur = next,
                        other => {
                            // Marker so a 1-pair `(a . b)` doesn't hash the
                            // same as a 2-pair `(a b)` (whose cdr ends Nil).
                            0xFFu8.hash(h);
                            self.hash_value_into(other, h);
                            break;
                        }
                    }
                }
            }
            Value::Vector(id) => {
                8u8.hash(h);
                let xs = self.vector(id);
                (xs.len() as u64).hash(h);
                for &x in xs {
                    self.hash_value_into(x, h);
                }
            }
            Value::Map(id) => {
                9u8.hash(h);
                // Order-insensitive: XOR each entry's hash into an
                // accumulator (XOR is commutative — works regardless of
                // CHAMP trie shape). Mix in size so `{}` ≠ `{a a}` even
                // if the per-entry hash ever conspired to 0.
                let mut acc: u64 = 0;
                let size = self.map_size(id);
                self.fold_entries(id, &mut |k, vv| {
                    let mut sub = std::collections::hash_map::DefaultHasher::new();
                    self.hash_value_into(k, &mut sub);
                    self.hash_value_into(vv, &mut sub);
                    acc ^= sub.finish();
                });
                (size as u64).hash(h);
                acc.hash(h);
            }
            Value::Fn(id) => {
                10u8.hash(h);
                id.0.hash(h);
            }
            Value::Macro(id) => {
                11u8.hash(h);
                id.0.hash(h);
            }
            Value::Native(id) => {
                12u8.hash(h);
                id.0.hash(h);
            }
            Value::Ref(id) => {
                13u8.hash(h);
                id.hash(h);
            }
            Value::Pid { node, id } => {
                14u8.hash(h);
                node.hash(h);
                id.hash(h);
            }
            Value::Rope(id) => {
                15u8.hash(h);
                // Hash by text content so two ropes with equal text hash equal,
                // consistent with `equal` below. Materialise the whole string:
                // hashing chunk-by-chunk would frame each chunk (str's Hash adds
                // a terminator), so equal text under different chunk boundaries
                // could hash differently — breaking the equal⇒same-hash contract.
                // Only paid when a rope is actually used as a map key (rare).
                self.rope(id).to_string().hash(h);
            }
            Value::Socket(id) => {
                16u8.hash(h);
                id.hash(h);
            }
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
            // Maps: CHAMP is *canonical* under structural equality, so two
            // equal maps have identical trie shapes — same `data_map` /
            // `node_map` / `is_collision` bits at every node. Recurse
            // structurally; collision leaves fall back to set-equality on
            // their entries (their internal order isn't canonical).
            (Map(x), Map(y)) => self.map_equal(x, y),
            (Fn(x), Fn(y)) => x == y,
            (Macro(x), Macro(y)) => x == y,
            (Native(x), Native(y)) => x == y,
            (Ref(x), Ref(y)) => x == y,
            // Pids are equal by node identity + local id (same process, anywhere).
            (Pid { node: n1, id: i1 }, Pid { node: n2, id: i2 }) => n1 == n2 && i1 == i2,
            // Ropes compare by text content (ropey's PartialEq walks chunks; no
            // full materialisation). Distinct handles to equal text are `=`.
            (Rope(x), Rope(y)) => self.rope(x) == self.rope(y),
            // Sockets are identity values — equal iff the same registry handle.
            (Socket(x), Socket(y)) => x == y,
            _ => false,
        }
    }

    /// Structural equality between two closures — used *only* to dedup a
    /// hot-reload redefinition that didn't actually change the code (a
    /// save-without-change, or `nest format` rewriting the whole file) so it
    /// doesn't append a duplicate into the append-only RUNTIME region
    /// (docs/live-editing.md Stage 5). Deliberately **conservative**: it bails
    /// (returns `false`) on any closure that captured a *local* scope
    /// (`env.is_some()`), handling only the common top-level case where `env`
    /// resolves to the global per-process. Soundness rests on the asymmetry — a
    /// false "not equal" merely keeps today's behaviour (append, i.e. the leak),
    /// while a false "equal" would skip a real redefinition; identical params,
    /// body, optionals, rest, name and doc with no captured scope means the two
    /// closures are behaviourally identical, so "equal" is never false-positive.
    fn closures_structurally_equal(&self, a: ClosureId, b: ClosureId) -> bool {
        let ca = self.closure(a);
        let cb = self.closure(b);
        if ca.env.is_some() || cb.env.is_some() {
            return false;
        }
        ca.name == cb.name
            && ca.doc == cb.doc
            && ca.arms.len() == cb.arms.len()
            && ca.arms.iter().zip(cb.arms.iter()).all(|(aa, ab)| {
                aa.params == ab.params
                    && aa.rest == ab.rest
                    && aa.optionals.len() == ab.optionals.len()
                    && aa.body.len() == ab.body.len()
                    && aa
                        .optionals
                        .iter()
                        .zip(ab.optionals.iter())
                        .all(|((sa, da), (sb, db))| sa == sb && self.equal(*da, *db))
                    && aa
                        .body
                        .iter()
                        .zip(ab.body.iter())
                        .all(|(&x, &y)| self.equal(x, y))
            })
    }

    /// Equality between two CHAMP maps — canonical-form recursion. Two
    /// equal maps have the same node shape (same bitmaps, same children
    /// in slot order), so a structural walk bails on the first mismatch.
    /// Collision leaves fall back to set-equality on their entries (their
    /// internal order isn't canonical — two equally-content collision
    /// leaves can hold their entries in different positions).
    fn map_equal(&self, x: MapId, y: MapId) -> bool {
        let nx = self.map_node(x);
        let ny = self.map_node(y);
        if nx.size != ny.size {
            return false;
        }
        if nx.is_collision != ny.is_collision {
            return false;
        }
        if nx.is_collision {
            // Set-equality on entries. Collision leaves are tiny (entries
            // share the full 64-bit hash — astronomically rare), so O(n²)
            // is fine.
            if nx.data.len() != ny.data.len() {
                return false;
            }
            return nx.data.iter().all(|(k, v)| {
                ny.data
                    .iter()
                    .any(|(k2, v2)| self.equal(*k, *k2) && self.equal(*v, *v2))
            });
        }
        // Branch: same bitmaps → same slot occupancy → same shapes.
        if nx.data_map != ny.data_map || nx.node_map != ny.node_map {
            return false;
        }
        for ((k1, v1), (k2, v2)) in nx.data.iter().zip(ny.data.iter()) {
            if !self.equal(*k1, *k2) || !self.equal(*v1, *v2) {
                return false;
            }
        }
        for (&c1, &c2) in nx.children.iter().zip(ny.children.iter()) {
            if !self.map_equal(c1, c2) {
                return false;
            }
        }
        true
    }

    /// A total structural ordering for `(sort coll)`'s non-numeric fallback.
    /// **Not** Brood-visible as `<`/`compare` — that's a separate decision; this
    /// is just enough to give the sort builtin a defined order on heterogeneous
    /// values without throwing.
    ///
    /// Within a kind, ordering is the natural one: ints by `<`, floats by IEEE,
    /// mixed numerics by promotion (same compromise as `prim_lt`); strings/
    /// symbols/keywords by their text; pairs/vectors lexicographically;
    /// `nil` < `false` < `true`. Across kinds we use a fixed tag order
    /// (`tag_rank`) so a heterogeneous list still has *some* total order — the
    /// alternative is the current "throws on a vector" trap. Maps, fns,
    /// natives, macros, refs, pids fall through to a tag-rank compare (sorting
    /// them by content isn't well-defined here).
    pub fn value_cmp(&self, a: Value, b: Value) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        use Value::*;
        match (a, b) {
            (Nil, Nil) => Ordering::Equal,
            (Bool(x), Bool(y)) => x.cmp(&y),
            (Int(x), Int(y)) => x.cmp(&y),
            (Float(x), Float(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            (Int(x), Float(y)) => (x as f64).partial_cmp(&y).unwrap_or(Ordering::Equal),
            (Float(x), Int(y)) => x.partial_cmp(&(y as f64)).unwrap_or(Ordering::Equal),
            (Str(x), Str(y)) => self.string(x).cmp(self.string(y)),
            // Symbols/keywords sort by spelling so it's stable and human-meaningful.
            (Sym(x), Sym(y)) | (Keyword(x), Keyword(y)) => {
                crate::core::value::symbol_name(x).cmp(&crate::core::value::symbol_name(y))
            }
            (Vector(x), Vector(y)) => {
                let xs: Vec<Value> = self.vector(x).to_vec();
                let ys: Vec<Value> = self.vector(y).to_vec();
                for (xv, yv) in xs.iter().zip(ys.iter()) {
                    match self.value_cmp(*xv, *yv) {
                        Ordering::Equal => continue,
                        o => return o,
                    }
                }
                xs.len().cmp(&ys.len())
            }
            // Lists: walk the cons spine like equal(). Empty list < non-empty.
            (Nil, Pair(_)) => Ordering::Less,
            (Pair(_), Nil) => Ordering::Greater,
            (Pair(x), Pair(y)) => {
                let (mut x, mut y) = (x, y);
                loop {
                    let (a0, a1) = self.pair(x);
                    let (b0, b1) = self.pair(y);
                    match self.value_cmp(a0, b0) {
                        Ordering::Equal => {}
                        o => return o,
                    }
                    match (a1, b1) {
                        (Pair(nx), Pair(ny)) => {
                            x = nx;
                            y = ny;
                        }
                        _ => return self.value_cmp(a1, b1),
                    }
                }
            }
            _ => tag_rank(a).cmp(&tag_rank(b)),
        }
    }

    // ----- environments -----
    //
    // Real env frames are always LOCAL. The global scope is the sentinel
    // [`EnvId::GLOBAL`], which routes to the shared `runtime.globals` table; a
    // top-level frame's parent chain bottoms out there. (During prelude *build*
    // the global is instead a real local root frame with no parent.)

    /// True if `env` points at a LOCAL env slot that the sweep has poisoned
    /// (i.e. a freed slot whose handle leaked past GC). Debug-only entry
    /// point for the use-after-GC chase in [`crate::eval`]; in release the
    /// `poison` field doesn't exist, so the method is `#[cfg]`-gated too —
    /// every call site is `#[cfg(debug_assertions)]`-gated to match.
    ///
    /// **Opt-in** (`BROOD_ENV_DEBUG=1`): superseded by the generational-handle
    /// tripwire (ADR-054), which catches use-after-GC precisely at the deref. Off
    /// by default because it (and [`debug_walk_env_chain`]) run per eval / per
    /// symbol and walk the env chain — pathologically slow always-on. Kept as an
    /// on-demand tool. [`debug_walk_env_chain`]: Self::debug_walk_env_chain
    #[cfg(debug_assertions)]
    pub fn env_is_poisoned(&self, env: EnvId) -> bool {
        env_chain_debug()
            && env != EnvId::GLOBAL
            && env.region() == LOCAL
            && PoisonBits::is(&self.poison.envs, env.index())
    }

    /// Walk the parent chain from `env` looking up `_sym`, logging at the
    /// first poisoned link. Helps localise *which* frame in a lookup chain
    /// is the use-after-GC offender. Debug-only; no-op in release.
    #[cfg(debug_assertions)]
    pub fn debug_walk_env_chain(&self, env: EnvId, _sym: Symbol) {
        if !env_chain_debug() || !crate::process::in_green_process() {
            return;
        }
        let mut cur = env;
        let mut depth = 0u32;
        while cur != EnvId::GLOBAL {
            if cur.region() == LOCAL && PoisonBits::is(&self.poison.envs, cur.index()) {
                eprintln!(
                    "[panic-context] env chain hit POISONED frame at depth {} env={:#x}",
                    depth, cur.0
                );
                return;
            }
            match self.local.envs.get(cur.index()) {
                Some(frame) => match frame.parent {
                    Some(p) => cur = p,
                    None => return,
                },
                None => return,
            }
            depth += 1;
            if depth > 10_000 {
                return; // safety belt — env chains shouldn't be this deep
            }
        }
    }

    fn env_frame(&self, env: EnvId) -> &EnvFrame {
        // `EnvId::GLOBAL` is a sentinel (region bits `0b11`) — there is no
        // frame to return; the global scope routes through
        // `runtime.globals_read()` instead. Callers MUST short-circuit
        // GLOBAL before reaching here (every walker does — see `env_get`
        // line 1086). A clear assert when that invariant slips, rather
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
                &self.old.envs[env.index()]
            }
            LOCAL => {
                #[cfg(debug_assertions)]
                debug_assert!(
                    !PoisonBits::is(&self.poison.envs, env.index()),
                    "use-after-GC: env_frame on freed nursery env slot {} \
                     (handle {:#x}). Sweep poisoned this slot; some caller \
                     held the EnvId across a GC safepoint without rooting it. \
                     See docs/claude-demo-findings.md § Scheduler race.",
                    env.index(),
                    env.0
                );
                #[cfg(debug_assertions)]
                self.check_epoch_aged(false, env.generation(), env.index(), "env_frame", env.0);
                &self.local.envs[env.index()]
            }
            RUNTIME => self
                .runtime
                .code
                .envs
                .get(env.index())
                .expect("runtime env frame")
                .get()
                .expect("runtime env read before promote filled its slot"),
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
        let idx = self.local.envs.len();
        self.local.envs.push(EnvFrame {
            vars: EnvVars::new(),
            parent,
        });
        EnvId::local_gen(idx, self.local_epoch)
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
        let val = self.runtime.globals_read().get(&sym).copied();
        if let Some(val) = val {
            self.global_ic.borrow_mut().insert(sym, (cur, val));
        }
        val
    }

    pub fn env_define(&mut self, env: EnvId, sym: Symbol, val: Value) {
        if env == EnvId::GLOBAL {
            // Dedup an unchanged hot-reload redefinition (Stage 5): if `sym` is
            // already bound to a closure structurally identical to `val`, keep the
            // existing (already-promoted) binding rather than append a duplicate
            // into the append-only RUNTIME region. Bounds the leak for the common
            // save-without-change / formatter-churn path; any *real* edit differs
            // structurally and falls through to the normal promote+rebind.
            let existing = self.runtime.globals_read().get(&sym).copied();
            if let Some(old) = existing {
                let unchanged = match (old, val) {
                    (Value::Fn(o), Value::Fn(n)) => self.closures_structurally_equal(o, n),
                    (Value::Macro(o), Value::Macro(n)) => self.closures_structurally_equal(o, n),
                    _ => false,
                };
                if unchanged {
                    return;
                }
            }
            // Global code/data is shared across inner processes, so promote it
            // into the shared RUNTIME region before binding.
            let shared = self.promote(val);
            self.runtime.globals_write().insert(sym, shared);
            // Invalidate every process's global inline cache (late binding).
            self.runtime.version.fetch_add(1, Ordering::Relaxed);
        } else if env.is_old() {
            // The frame was tenured (a minor collection promoted it while it was
            // still being bound — e.g. a collection during a `let` rhs eval). Mutate
            // it in the old space and remember it: this push can create an
            // OLD->YOUNG edge (`val` is a fresh nursery value), which the next minor
            // collection must trace and rewrite, since it otherwise never scans old.
            self.old.envs[env.index()].vars.push((sym, val));
            self.remembered.push(env);
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
        // Wholesale table swap — invalidate every stamped global inline cache.
        self.runtime.version.fetch_add(1, Ordering::Relaxed);
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

    /// Overwrite the `i`th explicit root in place (operand-stack slot update —
    /// e.g. advancing a rooted cons-spine cursor between argument evals). Paired
    /// with [`root_at`](Self::root_at) for read-back.
    pub fn set_root(&mut self, i: usize, v: Value) {
        self.roots[i] = v;
    }

    /// Root `v` for the duration of a collection-bearing region, **skipping the
    /// operand-stack push when `v` is immovable** (an atom, or a `PRELUDE`/
    /// `RUNTIME` handle — the common case when running compiled/promoted code).
    /// Returns a [`Root`] token to read back with [`read_root`](Self::read_root)
    /// after any nested eval. The cheaper, region-aware sibling of
    /// [`push_root`](Self::push_root): on the hot path it pays nothing. Teardown
    /// is still the shared `truncate_roots(base)` — it drops exactly the LOCAL
    /// slots this region pushed, regardless of how many were skipped.
    #[inline]
    pub fn root(&mut self, v: Value) -> Root {
        if is_movable(v) {
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

    /// The [`EnvId`] counterpart of [`root`](Self::root): roots a frame only if
    /// it's a movable LOCAL frame, keeping the [`EnvId::GLOBAL`] sentinel and
    /// RUNTIME-promoted frames inline. Read back with
    /// [`read_root_env`](Self::read_root_env).
    #[inline]
    pub fn root_env(&mut self, e: EnvId) -> EnvRoot {
        if e != EnvId::GLOBAL && e.region() == LOCAL {
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

    // ----- GC trigger / introspection -----------------------------------------

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
        self.runtime.code.closures.count()
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
            + self.local.ropes.len()
            + self.local.closures.len()
            + self.local.envs.len();
        let free = self.local_free.pairs.len()
            + self.local_free.vectors.len()
            + self.local_free.maps.len()
            + self.local_free.strings.len()
            + self.local_free.ropes.len()
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

    /// An estimate of this process's LOCAL heap footprint in **bytes** — the
    /// occupied slab entries weighted by element size (`len * size_of` per slab).
    /// Cheap (no traversal); counts the slab arrays themselves, not nested/shared
    /// content (inner vectors, string bytes, `Arc`-shared ropes), so it's a
    /// comparative figure for an observer, not an exact RSS. Bump-allocated, so it
    /// reflects allocation since the last arena reset / collection. Backs
    /// `process-info`'s `:memory` (published on `receive`).
    pub fn local_bytes(&self) -> usize {
        use std::mem::size_of;
        let s = &self.local;
        s.pairs.len() * size_of::<(Value, Value)>()
            + s.vectors.len() * size_of::<Vec<Value>>()
            + s.maps.len() * size_of::<MapNode>()
            + s.strings.len() * size_of::<LocalString>()
            + s.ropes.len() * size_of::<ropey::Rope>()
            + s.closures.len() * size_of::<Closure>()
            + s.natives.len() * size_of::<NativeFn>()
            + s.envs.len() * size_of::<EnvFrame>()
    }

    /// GC observability counters (Tier-1; `docs/memory-review.md` §7), as a
    /// `(runs, copied, reclaimed)` triple of cumulative figures since process
    /// start: collections performed, LOCAL objects relocated, LOCAL objects
    /// dropped. Backs the `(gc-stats)` builtin. Counts both Stage-B safepoint
    /// collections and bare [`flush`](Self::flush) calls (they share [`arena_flip`]).
    pub fn gc_counters(&self) -> (u64, u64, u64) {
        (self.gc_runs, self.gc_copied, self.gc_reclaimed)
    }

    /// The current adaptive GC threshold (LOCAL live-object count that triggers
    /// the next safepoint collection). The slow/stable dial — exposed so an
    /// observer can see how close the heap is to its next collection.
    pub fn gc_threshold(&self) -> usize {
        self.gc_threshold
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

    // ----- compiling-VM body cache (ADR-076; see `eval::compile`) -----

    /// The cached compile result for closure key `k` (see [`VmCacheKey`]):
    /// `None` = not cached yet; `Some(None)` = cached as ineligible; `Some(Some(a))`
    /// = the compiled body. `&self` (interior-mutable `RefCell`), so the VM can
    /// consult it on the read-only hot path.
    pub fn vm_cache_get(
        &self,
        k: VmCacheKey,
    ) -> Option<Option<Arc<crate::eval::compile::CompiledClosure>>> {
        self.vm_cache.borrow().get(&k).cloned()
    }

    /// Record the compile result for closure key `k` (eligible body or `None`).
    pub fn vm_cache_put(&self, k: VmCacheKey, v: Option<Arc<crate::eval::compile::CompiledClosure>>) {
        self.vm_cache.borrow_mut().insert(k, v);
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
    pub fn collect(&mut self, extra_roots: &mut [Value], extra_envs: &mut [EnvId]) {
        if !self.gc_enabled {
            return;
        }
        // `BROOD_GC_VERIFY=1` (debug only): before flipping, walk the whole
        // reachable LOCAL graph and assert every handle is in-bounds and
        // current-epoch. Catches a *stored* stale handle (a missed root whose
        // value was written into a heap cell) right here — with the root→…→cell
        // path — instead of letting it surface far away as an OOB index or a
        // `promote` stack overflow. See `verify_local_graph`.
        #[cfg(debug_assertions)]
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
        self.gc_threshold = std::cmp::max(gc_floor(), self.local_live_count().saturating_mul(2));
        // Escalate to a *major* (compact the old generation) only when it has
        // doubled since the last major — so majors stay rare while minors keep the
        // nursery bounded.
        if self.old_live_count() >= self.major_threshold {
            self.major_collect(extra_roots, extra_envs);
            self.major_threshold =
                std::cmp::max(major_floor(), self.old_live_count().saturating_mul(2));
        }
    }

    /// Live objects in the **old generation** (`Σ old.slab.len()`). Old has no
    /// free list — it's append-only between major collections — so the slab
    /// lengths *are* the live count. Drives the major-collection threshold.
    pub fn old_live_count(&self) -> usize {
        self.old.pairs.len()
            + self.old.vectors.len()
            + self.old.maps.len()
            + self.old.strings.len()
            + self.old.ropes.len()
            + self.old.closures.len()
            + self.old.envs.len()
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
            (Slabs::default(), self.local_epoch, false)
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
        self.local_free.clear();
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
        #[cfg(debug_assertions)]
        {
            self.poison.pairs.clear();
            self.poison.vectors.clear();
            self.poison.maps.clear();
            self.poison.strings.clear();
            self.poison.ropes.clear();
            self.poison.closures.clear();
            self.poison.envs.clear();
        }
        // Install the relocated space. Tenure: `dest` is the grown old gen; the
        // nursery stays the empty Slabs left by the take. Flip: `dest` is the fresh
        // nursery; the old gen was untouched.
        if tenure {
            self.old = dest;
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
    /// `old` into fresh `old` slabs, dropping dead tenured objects). Assumes a
    /// minor has just run, so the nursery is empty and everything live is in old
    /// and reachable from the roots. Bumps the old epoch.
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
        // `remembered` is empty (the minor cleared it; no binding has run since).
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

    /// Is the `BROOD_GC_VERIFY` heap-verifier armed? Read once. Debug only.
    #[cfg(debug_assertions)]
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
    /// rooting site) is obvious. O(live); only runs under the env flag.
    #[cfg(debug_assertions)]
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
        let young_ep = self.local_epoch;
        let old_ep = self.old_epoch;
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
        let bad = |kind: &str, is_old: bool, gen: u32, idx: usize, len: usize, parent: u64, raw: u64| {
            let (ep, space) = if is_old { (old_ep, "OLD") } else { (young_ep, "nursery") };
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
                W::V(v, parent) => match v {
                    Value::Pair(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad("pair", id.is_old(), id.generation(), id.index(), slabs.pairs.len(), parent, id.0);
                        if !id.is_old() && !std::mem::replace(&mut seen_pair[id.is_old() as usize][id.index()], true) {
                            let (a, b) = slabs.pairs[id.index()];
                            work.push(W::V(a, id.0));
                            work.push(W::V(b, id.0));
                        }
                    }
                    Value::Vector(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad("vector", id.is_old(), id.generation(), id.index(), slabs.vectors.len(), parent, id.0);
                        if !id.is_old() && !std::mem::replace(&mut seen_vec[id.is_old() as usize][id.index()], true) {
                            for &el in &slabs.vectors[id.index()] {
                                work.push(W::V(el, id.0));
                            }
                        }
                    }
                    Value::Map(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad("map", id.is_old(), id.generation(), id.index(), slabs.maps.len(), parent, id.0);
                        if !id.is_old() && !std::mem::replace(&mut seen_map[id.is_old() as usize][id.index()], true) {
                            let node = &slabs.maps[id.index()];
                            for &(mk, mv) in &node.data {
                                work.push(W::V(mk, id.0));
                                work.push(W::V(mv, id.0));
                            }
                            for &c in &node.children {
                                work.push(W::V(Value::Map(c), id.0));
                            }
                        }
                    }
                    Value::Str(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad("string", id.is_old(), id.generation(), id.index(), slabs.strings.len(), parent, id.0);
                    }
                    Value::Rope(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad("rope", id.is_old(), id.generation(), id.index(), slabs.ropes.len(), parent, id.0);
                    }
                    Value::Fn(id) | Value::Macro(id) if id.region() == LOCAL => {
                        let slabs = if id.is_old() { &self.old } else { &self.local };
                        bad("closure", id.is_old(), id.generation(), id.index(), slabs.closures.len(), parent, id.0);
                        if !id.is_old() && !std::mem::replace(&mut seen_clo[id.is_old() as usize][id.index()], true) {
                            let cl = &slabs.closures[id.index()];
                            for arm in &cl.arms {
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
                    bad("env", e.is_old(), e.generation(), e.index(), slabs.envs.len(), parent, e.0);
                    if !e.is_old() && !std::mem::replace(&mut seen_env[e.is_old() as usize][e.index()], true) {
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

    #[allow(dead_code)]
    fn collect_old(&mut self, extra_roots: &[Value], extra_envs: &[EnvId]) {
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
                    // CHAMP node: trace every inline entry's (k, v) and
                    // every child sub-node handle. Children are LOCAL
                    // `MapId`s — push them via the normal Map traceitem.
                    let node = &self.local.maps[idx];
                    for &(k, v) in &node.data {
                        push_value(work, k);
                        push_value(work, v);
                    }
                    for &c in &node.children {
                        if c.region() == LOCAL {
                            work.push(TraceItem::Map(c.index()));
                        }
                    }
                }
            }
            TraceItem::Str(idx) => {
                // No children, but mark it so it survives sweep.
                marks.mark_string(idx);
            }
            TraceItem::Rope(idx) => {
                // A rope is an opaque leaf (no Value children); just mark it.
                marks.mark_rope(idx);
            }
            TraceItem::Closure(idx) => {
                if marks.mark_closure(idx) {
                    let cl = &self.local.closures[idx];
                    for arm in &cl.arms {
                        for &f in &arm.body {
                            push_value(work, f);
                        }
                        for &(_, d) in &arm.optionals {
                            push_value(work, d);
                        }
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
        // Reset the use-after-GC tripwire: poisoned[i] starts equal to "slot
        // i was just freed" — set inside each loop below. Live slots clear to
        // false; reused-then-freed slots flip true. Debug builds only — the
        // `poison` field doesn't exist in release.
        #[cfg(debug_assertions)]
        {
            self.poison.pairs.clear();
            self.poison.pairs.resize(self.local.pairs.len(), false);
            self.poison.vectors.clear();
            self.poison.vectors.resize(self.local.vectors.len(), false);
            self.poison.maps.clear();
            self.poison.maps.resize(self.local.maps.len(), false);
            self.poison.strings.clear();
            self.poison.strings.resize(self.local.strings.len(), false);
            self.poison.ropes.clear();
            self.poison.ropes.resize(self.local.ropes.len(), false);
            self.poison.closures.clear();
            self.poison
                .closures
                .resize(self.local.closures.len(), false);
            self.poison.envs.clear();
            self.poison.envs.resize(self.local.envs.len(), false);
        }

        for i in 0..self.local.pairs.len() {
            if !marks.is_pair_marked(i) {
                self.local_free.pairs.push(i as u32);
                // form_pos is keyed by (age,index); this nursery sweep drops the
                // young (age 0) entry since the slot will be reused.
                self.form_pos.remove(&(i as u64));
                #[cfg(debug_assertions)]
                {
                    self.poison.pairs[i] = true;
                }
            }
        }
        for i in 0..self.local.vectors.len() {
            if !marks.is_vector_marked(i) {
                self.local_free.vectors.push(i as u32);
                // Release the dead `Vec<Value>`'s buffer; alloc_vector replaces
                // the slot wholesale on reuse, so we don't need an empty marker.
                self.local.vectors[i] = Vec::new();
                #[cfg(debug_assertions)]
                {
                    self.poison.vectors[i] = true;
                }
            }
        }
        for i in 0..self.local.maps.len() {
            if !marks.is_map_marked(i) {
                self.local_free.maps.push(i as u32);
                self.local.maps[i] = MapNode::default();
                #[cfg(debug_assertions)]
                {
                    self.poison.maps[i] = true;
                }
            }
        }
        for i in 0..self.local.strings.len() {
            if !marks.is_string_marked(i) {
                self.local_free.strings.push(i as u32);
                // Release the slot's owned buffer / `Arc<SharedBlob>` ref;
                // alloc_string replaces wholesale on reuse. `Default` for
                // `LocalString` is `Inline(String::new())`, so a dead `Shared`
                // slot also decrements its refcount via the drop here — if
                // it was the last handle, the blob is freed.
                self.local.strings[i] = LocalString::default();
                #[cfg(debug_assertions)]
                {
                    self.poison.strings[i] = true;
                }
            }
        }
        for i in 0..self.local.ropes.len() {
            if !marks.is_rope_marked(i) {
                self.local_free.ropes.push(i as u32);
                // Replace with an empty rope so the old one's `Arc` nodes drop
                // (freeing them if this was the last reference).
                self.local.ropes[i] = ropey::Rope::new();
                #[cfg(debug_assertions)]
                {
                    self.poison.ropes[i] = true;
                }
            }
        }
        for i in 0..self.local.closures.len() {
            if !marks.is_closure_marked(i) {
                self.local_free.closures.push(i as u32);
                // Replace with a default so the `Vec`s inside drop. `Closure`
                // derives `Default`, so adding a field to it doesn't risk a
                // sweep-bug from a missed initialiser here.
                self.local.closures[i] = Closure::default();
                #[cfg(debug_assertions)]
                {
                    self.poison.closures[i] = true;
                }
            }
        }
        for i in 0..self.local.envs.len() {
            if !marks.is_env_marked(i) {
                self.local_free.envs.push(i as u32);
                let slot = &mut self.local.envs[i];
                slot.vars.clear();
                slot.parent = None;
                #[cfg(debug_assertions)]
                {
                    self.poison.envs[i] = true;
                }
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
    Rope(usize),
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
        Value::Rope(id) if id.region() == LOCAL => work.push(TraceItem::Rope(id.index())),
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
    ropes: Vec<bool>,
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
            ropes: vec![false; local.ropes.len()],
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
    ropes => mark_rope, is_rope_marked,
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

// ----- heap flush (arena flip / Phase 2) -----------------------------------
//
// The standalone deep-copy that backs [`Heap::flush`]. Free functions so the
// recursion borrows `&old` immutably and `&mut new` mutably without tangling
// with the `Heap`'s `&mut self`. Cycles are handled with a per-slab
// forwarding table: when a node is visited, we reserve a placeholder slot
// in `new` and record `old_idx → new_idx` before recursing into its
// children — a second hit on the same old handle returns the placeholder
// instead of re-traversing.

/// Forwarding table for [`Heap::promote`]: LOCAL slot index → the RUNTIME handle
/// it was promoted to, for the two handle kinds that can form a cycle (a closure
/// capturing its own binding scope). Lets a cyclic graph terminate — the back-edge
/// resolves to the already-reserved RUNTIME handle — and collapses a shared (DAG)
/// closure/env to one RUNTIME copy. Pairs/vectors/maps are acyclic by construction
/// so they need no forwarding (they'd only ever be a finite tree to re-copy).
#[derive(Default)]
struct PromoteForward {
    closures: HashMap<u32, ClosureId>,
    envs: HashMap<u32, EnvId>,
}

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
mint_fn!(mint_rope, RopeId);
mint_fn!(mint_closure, ClosureId);
mint_fn!(mint_env, EnvId);

fn flush_value(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, v: Value) -> Value {
    match v {
        Value::Pair(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::Pair(flush_pair(old, new, fwd, id))
        }
        Value::Vector(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::Vector(flush_vector(old, new, fwd, id))
        }
        Value::Map(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::Map(flush_map(old, new, fwd, id))
        }
        Value::Str(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::Str(flush_string(old, new, fwd, id))
        }
        Value::Rope(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::Rope(flush_rope(old, new, fwd, id))
        }
        Value::Fn(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::Fn(flush_closure(old, new, fwd, id))
        }
        Value::Macro(id) if fwd.copies(id.region(), id.is_old()) => {
            Value::Macro(flush_closure(old, new, fwd, id))
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
    let mut cur = Value::Pair(id);
    let tail = loop {
        match cur {
            Value::Pair(p) if fwd.copies(p.region(), p.is_old()) => {
                let key = p.index() as u32;
                if let Some(&n) = fwd.pairs.get(&key) {
                    break Value::Pair(fwd.mint_pair(n as usize));
                }
                let (car, cdr) = old.pairs[p.index()];
                let new_idx = new.pairs.len();
                new.pairs.push((Value::Nil, Value::Nil));
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
        next = Value::Pair(fwd.mint_pair(new_idx));
    }
    match next {
        Value::Pair(pid) => pid,
        _ => unreachable!("the spine always has at least the head pair"),
    }
}

fn flush_vector(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: VecId) -> VecId {
    let key = id.index() as u32;
    if let Some(&new_idx) = fwd.vectors.get(&key) {
        return fwd.mint_vector(new_idx as usize);
    }
    let items: Vec<Value> = old.vectors[id.index()].clone();
    let new_idx = new.vectors.len();
    new.vectors.push(Vec::new());
    fwd.vectors.insert(key, new_idx as u32);
    let copied: Vec<Value> = items
        .into_iter()
        .map(|x| flush_value(old, new, fwd, x))
        .collect();
    new.vectors[new_idx] = copied;
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
    let entry = match &old.strings[id.index()] {
        LocalString::Inline(s) => LocalString::Inline(s.clone()),
        LocalString::Shared(arc) => LocalString::Shared(Arc::clone(arc)),
    };
    let new_idx = new.strings.len();
    new.strings.push(entry);
    fwd.strings.insert(key, new_idx as u32);
    fwd.mint_string(new_idx)
}

fn flush_rope(old: &Slabs, new: &mut Slabs, fwd: &mut FlushForward, id: RopeId) -> RopeId {
    let key = id.index() as u32;
    if let Some(&new_idx) = fwd.ropes.get(&key) {
        return fwd.mint_rope(new_idx as usize);
    }
    // `ropey::Rope::clone` is a cheap `Arc`-node bump (no byte copy); the old
    // slab drops right after `flush`, leaving the surviving rope's internal
    // refcounts net-unchanged — same structural sharing as `flush_string`.
    let rope = old.ropes[id.index()].clone();
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
        let node = &old.maps[id.index()];
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
    let cl = old.closures[id.index()].clone();
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
        let frame = &old.envs[env.index()];
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
    #[should_panic(expected = "use-after-GC")]
    fn stale_handle_after_flip_panics() {
        let mut h = Heap::new();
        let id = match h.alloc_pair(Value::Int(1), Value::Int(2)) {
            Value::Pair(id) => id,
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
        let mut roots = [h.alloc_pair(Value::Int(1), Value::Int(2))];
        h.flush(&mut roots);
        let (car, _) = match roots[0] {
            Value::Pair(id) => h.pair(id),
            _ => unreachable!(),
        };
        assert!(matches!(car, Value::Int(1)));
    }
}
