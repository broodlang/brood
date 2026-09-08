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

use arc_swap::{ArcSwap, Guard};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use smallvec::SmallVec;

use crate::core::blob::{SharedBlob, SHARED_BLOB_THRESHOLD};
use crate::core::keywords as kw;
use crate::core::map_champ::{self, MapNode, MAX_DEPTH};
use crate::core::value::{
    BigIntId, BytesId, Closure, ClosureArm, ClosureId, ClosureTemplate, DecimalId, EnvId, MapId,
    NativeFn, NativeId, PairId, Passthrough, RatioId, RopeId, StrId, Symbol, Value, ValueRef,
    VecId, LOCAL, PRELUDE, RUNTIME,
};
use crate::error::LispError;

/// Generate a `&self` accessor that resolves a handle to a shared reference by
/// region: the LOCAL/PRELUDE slab is indexed directly; the append-only RUNTIME
/// slab via `boxcar::Vec::get` (stable refs, lock-free). The three uniform
/// all-three-region reference accessors share this; `pair` (returns by value)
/// and the region-restricted `native`/`env_frame` stay hand-written.
macro_rules! region_ref {
    ($name:ident, $id:ty, $field:ident, $t:ty, $what:literal) => {
        pub fn $name(&self, id: $id) -> SlabRef<'_, $t> {
            match id.region() {
                LOCAL if id.is_old() => {
                    #[cfg(debug_assertions)]
                    self.check_epoch_aged(
                        true,
                        id.generation(),
                        id.index(),
                        stringify!($name),
                        id.0,
                    );
                    SlabRef::direct(&self.old().$field[id.index()])
                }
                LOCAL => {
                    #[cfg(debug_assertions)]
                    self.check_epoch_aged(
                        false,
                        id.generation(),
                        id.index(),
                        stringify!($name),
                        id.0,
                    );
                    SlabRef::direct(&self.local.$field[id.index()])
                }
                PRELUDE => SlabRef::direct(&self.prelude.slabs.$field[id.index()]),
                RUNTIME => {
                    let pin = self.code_gen_pinned(id.code_gen());
                    let r: &$t = pin.$field.get(id.index()).expect($what);
                    let ptr = r as *const $t;
                    // SAFETY: `ptr` points into `pin`'s CodeSlabs (stable `boxcar`
                    // address), kept alive by the `Arc` moved into the `SlabRef`.
                    unsafe { SlabRef::pinned(pin, ptr) }
                }
                _ => unreachable!("invalid handle region"),
            }
        }
    };
}

/// Emit the use-after-GC tripwire for **one LOCAL match arm** of a hand-written
/// accessor — the generational `check_epoch_aged`. Factors the byte-for-byte-identical
/// preamble the `pair`/`string`/`closure`/`rope`/`bigint` accessors each copy-pasted;
/// `region_ref!` already inlines the same check for the uniform reference accessors.
/// `$name` is the accessor name (for the epoch "what" string); `$h` the handle
/// expression (`id.index()`/`id.0`/`id.generation()`).
///
/// Two forms select the aged flag: `old` → aged, `nursery` → nursery. (env_frame stays
/// hand-written — its message carries extra docs prose and binds `env`, not `id`.)
macro_rules! local_gc_check {
    (old, $self:ident, $h:expr, $name:literal) => {
        #[cfg(debug_assertions)]
        $self.check_epoch_aged(true, $h.generation(), $h.index(), $name, $h.0);
    };
    (nursery, $self:ident, $h:expr, $name:literal) => {
        #[cfg(debug_assertions)]
        $self.check_epoch_aged(false, $h.generation(), $h.index(), $name, $h.0);
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
/// A movable handle's identity — `(kind, index, region)`. `None` for an atom, which
/// has no heap identity and never needs copying. Used by
/// [`Heap::localize_for_freeze`] to collapse shared structure and to tell "already
/// LOCAL, unchanged" from "copied".
fn handle_key(v: Value) -> Option<(u8, u32, u8)> {
    let (kind, idx, reg) = match v.unpack() {
        ValueRef::Pair(id) => (0u8, id.index() as u32, id.region()),
        ValueRef::Vector(id) => (1, id.index() as u32, id.region()),
        ValueRef::Range(id) => (2, id.index() as u32, id.region()),
        ValueRef::SeqView(id) => (3, id.index() as u32, id.region()),
        ValueRef::Map(id) => (4, id.index() as u32, id.region()),
        ValueRef::Set(id) => (5, id.index() as u32, id.region()),
        ValueRef::Failure(id) => (24, id.index() as u32, id.region()),
        ValueRef::Str(id) => (6, id.index() as u32, id.region()),
        ValueRef::BigInt(id) => (7, id.index() as u32, id.region()),
        ValueRef::Decimal(id) => (8, id.index() as u32, id.region()),
        ValueRef::Ratio(id) => (13, id.index() as u32, id.region()),
        ValueRef::Bytes(id) => (9, id.index() as u32, id.region()),
        ValueRef::Fn(id) => (10, id.index() as u32, id.region()),
        ValueRef::Macro(id) => (11, id.index() as u32, id.region()),
        ValueRef::Rope(id) => (12, id.index() as u32, id.region()),
        _ => return None,
    };
    Some((kind, idx, reg))
}

/// Re-tag a **LOCAL** handle as PRELUDE, preserving its slab index.
///
/// Only LOCAL: a re-tag is an index-preserving bit flip, which is valid exactly
/// because the builder's slabs *become* the prelude region. Applying it to a
/// RUNTIME (or already-PRELUDE) handle would keep the index and change the region,
/// pointing at an unrelated object in a different slab — that was KI-12. The VM
/// promotes its constant-pool literals into RUNTIME, so a prelude global built by
/// compiled code (`(def *load-path* (list "."))`) held a LOCAL pair whose car was a
/// RUNTIME string; re-tagging it yielded PRELUDE `Str@60`, some unrelated
/// docstring. Non-LOCAL values are copied into the builder's slabs *before* this
/// runs — see [`Heap::localize_for_freeze`].
fn to_prelude(v: Value) -> Value {
    // **LOCAL only.** Reachable structure is copied LOCAL beforehand
    // ([`Heap::localize_for_freeze`]), so a non-LOCAL handle reaching here belongs to
    // unreachable boot garbage — the slab sweep visits every cell, dead ones
    // included. Leave those alone: nothing can read them, and flipping them is
    // precisely what corrupted a live global (KI-12).
    if !matches!(handle_key(v), None | Some((_, _, LOCAL))) {
        return v;
    }
    match v.unpack() {
        ValueRef::Pair(id) => Value::pair(PairId::prelude(id.index())),
        ValueRef::Vector(id) => Value::vector(VecId::prelude(id.index())),
        ValueRef::Range(id) => Value::range(VecId::prelude(id.index())),
        ValueRef::SeqView(id) => Value::seqview(VecId::prelude(id.index())),
        ValueRef::Map(id) => Value::map(MapId::prelude(id.index())),
        ValueRef::Set(id) => Value::set(MapId::prelude(id.index())),
        ValueRef::Failure(id) => Value::failure(MapId::prelude(id.index())),
        ValueRef::Str(id) => Value::str_(StrId::prelude(id.index())),
        ValueRef::BigInt(id) => Value::bigint(BigIntId::prelude(id.index())),
        ValueRef::Decimal(id) => Value::decimal(DecimalId::prelude(id.index())),
        ValueRef::Ratio(id) => Value::ratio(RatioId::prelude(id.index())),
        // A `bytes` handle used to fall through the `other` arm below, keeping its
        // LOCAL tag — so a `#b"…"` literal reaching a prelude global would resolve
        // in the wiped builder heap after the freeze. No prelude form produces one
        // today (the bit-syntax matcher only mentions them in comments), so this is
        // latent, but silence was the wrong default for a region re-tag: every kind
        // is either flipped or explicitly guarded (see `Rope`). Noticed while
        // investigating KI-12.
        ValueRef::Bytes(id) => Value::bytes(BytesId::prelude(id.index())),
        ValueRef::Fn(id) => Value::func(ClosureId::prelude(id.index())),
        ValueRef::Macro(id) => Value::macro_(ClosureId::prelude(id.index())),
        ValueRef::Native(id) => Value::native(NativeId::prelude(id.index())),
        // The prelude is pure Brood (no rope literals), so a rope can never
        // exist at freeze time. Guard the invariant rather than silently
        // re-tagging a LOCAL handle into PRELUDE.
        ValueRef::Rope(_) => unreachable!("a Rope cannot appear in the prelude region"),
        other => other,
    }
}

/// The most elements [`Heap::range_to_vec`] will realise from a lazy range before
/// refusing with a catchable error.
///
/// A range is O(1) however wide, so the count is genuinely unbounded — up to `i64::MAX`
/// — while realising it is bounded by memory. There is no "right" number here, only a
/// line past which the answer is certainly *no*: 64 Mi elements is a 512 MB `Vec` and,
/// once consed, ~1 GB of pairs, which already crosses the default soft memory limit. Set
/// high enough that nothing a program can actually complete is refused, and low enough
/// that the refusal arrives as an error instead of an allocator abort.
pub(crate) const MAX_REALISED_RANGE: i64 = 1 << 26;

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
    /// The prelude's own `(sig …)` declarations (`%register-sig` during the build), keyed
    /// like [`RuntimeCode::declared_sigs`] and holding the type-expression as a PRELUDE
    /// value. Read by [`Heap::declared_sig_value`] AFTER the runtime table, so a user
    /// redeclaration wins. Before this existed the build heap's table was simply dropped
    /// at the freeze (the runtime starts with an empty one), so a prelude sig such as
    /// `(sig %path-last-slash (string int -> int))` was invisible to every caller and the
    /// checker fell back to the body's inferred `ordered` — weaker advice, silently.
    declared_sigs: SymbolMap<Value>,
    /// The registries the PRELUDE build wrote (`%registry-update!` during the build —
    /// `defmulti num/add` alone touches `*multi-algebra*` and `*multi-ret*`). The build heap's name set lived in its `RuntimeCode`, which the
    /// freeze discards, so a fresh runtime's [`Heap::registry_names`] listed only what THIS
    /// process had written since boot and omitted every prelude-declared registry. A
    /// startup-image install that snapshots "the registries" by that list to merge them back
    /// after loading the root section could therefore not protect `num/add`'s declaration,
    /// and an imaged `nest test` failed to load any file with a `defmethod num/add` (KI-89).
    /// Carried here and unioned in, like `declared_sigs`.
    registry_names: Vec<Symbol>,
    /// Every global the prelude bound at the freeze — its own definitions, the natives
    /// (registered during the build), the registries it seeds. Fixed per binary, identical
    /// in every process. Behind `%prelude-global?`, which `stdimage/build` uses to tell a
    /// root global that is always present from one a std module defines (KI-112).
    binding_names: HashSet<Symbol>,
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
    bigints: usize,
    decimals: usize,
    ratios: usize,
    bytes: usize,
    ropes: usize,
    closures: usize,
    envs: usize,
    // The `local_epoch` the checkpoint was taken in. A collection between the
    // checkpoint and its `reset_local_to` bumps the epoch and rewrites the
    // nursery (a flip compacts survivors into fresh slabs; a tenure empties it),
    // so the slab lengths above no longer describe the live nursery — truncating
    // to them would strand the survivors the collector just kept. `reset_local_to`
    // compares this against the current epoch and skips the truncation on a
    // mismatch (the collection already reclaimed the dead). See its body.
    epoch: u32,
    // No `natives` field: a live runtime never allocates a native into its LOCAL
    // heap (they're registered once during the prelude build, then frozen into
    // PRELUDE). If that ever changes, add a field here and truncate it below.
}

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
type ClosureTemplateMap =
    HashMap<PairId, (Arc<ClosureTemplate>, u32), std::hash::BuildHasherDefault<SymbolHasher>>;

/// The [`Heap::lookup_const_closure`] cache map: a capture-free `(fn …)` literal's
/// `fn_rest` [`PairId`] → the **promoted RUNTIME closure handle** built for it once.
type ConstClosureMap = HashMap<PairId, Value, std::hash::BuildHasherDefault<SymbolHasher>>;

/// `BROOD_REG_TRACE=1` — name every registry write (pid, registry, op, first key) and
/// every globals restore on stderr. The tool for attributing a leaked or orphaned
/// registration to its writer (KI-89's class: WHO registered this id, and did a restore
/// land between its read and its write?). One cached bool when off.
fn reg_trace_enabled() -> bool {
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
    gens: [ArcSwap<CodeSlabs>; 2],
    /// Index (0 or 1) of the current code generation — read at `promote`/collect,
    /// never on the hot read path (the handle carries its own generation).
    current_gen: AtomicUsize,
    /// A process-wide-unique tag for this runtime instance — a plain `u64` the
    /// background JIT compiler keys its per-runtime publish cache by. Carried
    /// inside compile work items INSTEAD of the runtime `Arc` (or a `Weak`):
    /// either would park a reference in the queue and break the single-process
    /// RUNTIME compactor's `Arc::get_mut` uniqueness gate. Distinct per runtime,
    /// so shared native code never leaks across independent runtimes. (Read only
    /// by the JIT publish path — dead in a no-`jit` build, but kept unconditional
    /// so the struct layout and construction don't fork on the feature.)
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    runtime_tag: u64,
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
    gen_inflight: [AtomicUsize; 2],
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
    gen_version: AtomicU64,
    /// The global bindings (prelude + user `def`s). Read on every global lookup,
    /// written on `def` (the only mutation). The values point into PRELUDE or RUNTIME.
    globals: RwLock<SymbolMap<Value>>,
    /// Per-global **rebinding generation**: the value of `version` at each name's most
    /// recent `def`. Answers "is the binding I wrote still the one that is bound?" —
    /// what a temporary rebinding (`debug/trace-fn`'s wrapper, `nest test --cover`'s
    /// shim) must ask before *restoring*, or it overwrites a redefinition the user made
    /// in between. Comparing handles cannot answer it (a `def` promotes the closure, so
    /// the bound handle is not the local one); a generation can. Read by
    /// `%global-generation`.
    global_generations: RwLock<SymbolMap<u64>>,
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
    registry_lock: Mutex<HashSet<Symbol>>,
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
    sealed: RwLock<std::collections::HashSet<Symbol>>,
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
    private: RwLock<std::collections::HashSet<Symbol>>,
    /// **Stability metadata** per global (ADR-283): when a name appeared, whether it is
    /// deprecated and what replaces it, whether it is beta. Recorded by the `%register-meta`
    /// primitive a `(meta …)` form emits, and cleared by `env_define` on any redefinition —
    /// the same rule privacy follows, and for the same reason: a `def` that rebinds a name
    /// mid-run must not leave the OLD name's "deprecated" fact attached to the new one
    /// (ADR-013's late binding applies to the facts about code, not only to the code).
    meta: RwLock<SymbolMap<NameMeta>>,
    /// Monotonic version of `globals`, bumped on every binding change (`def`
    /// rebind, `restore_globals`). Per-process global **inline caches**
    /// (`Heap::global_ic`) stamp the version they resolved at and re-resolve only
    /// when it has moved — so a steady-state global read is an atomic load + a
    /// local hash hit instead of taking the shared `RwLock`. Late-binding stays
    /// exact: any `def` makes every stamped cache entry stale at once. `Relaxed`
    /// is sufficient — a global value is an immovable PRELUDE/RUNTIME handle, so
    /// there's no data it gates publication of; the counter only has to *change*.
    version: AtomicU64,
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
    code_epoch: AtomicU64,
    /// Where each global was *defined* — file + form position, recorded at load
    /// time before macroexpansion (ADR-031). Lives here, beside `globals`, so it
    /// is shared across a runtime's processes and updated by a redefinition, the
    /// same as the bindings it describes. Read by `(source-location 'name)`; the
    /// image-query foundation for cross-file goto-definition.
    def_sites: RwLock<HashMap<Symbol, SourceLoc>>,
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
    positions: RwLock<HashMap<u64, FormPos>>,
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
    jit_code_cache: RwLock<HashMap<(u64, u16), (usize, u64)>>,
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
    shared_closures: RwLock<HashMap<u64, (u64, Arc<crate::eval::compile::CompiledClosure>)>>,
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
    jit_inline_cache: RwLock<HashMap<(u64, u16), (usize, u64)>>,
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
    declared_sigs: RwLock<SymbolMap<Value>>,
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
    drain_active: AtomicBool,
    drain_gen: AtomicUsize,
    drain_epoch: AtomicU64,
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
    drain_acked: AtomicU64,
    /// RUNTIME-churn dirty bit: set true whenever a closure is minted into the
    /// current code generation (`promote_closure` — i.e. every `def`/`spawn`/
    /// hot-reload `promote`). The eval safepoint reads it to decide whether the
    /// (relatively costly) `rt_gc_due` probe — an `ArcSwap` load + a closure count
    /// — is worth running: the RUNTIME region only grows on a mint, which never
    /// happens inside a hot compute loop, so a def-free loop (`fib`, `reduce`,
    /// `apply`) skips the probe entirely. Cleared once the safepoint has run the
    /// probe. A plain relaxed `bool`: a read keeps the cache line Shared across
    /// worker cores (no invalidation), a mint writes it once.
    rt_dirty: AtomicBool,
    drain_acks: RwLock<HashMap<u64, u64>>,
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
    free_epoch: AtomicU64,
    /// **RUNTIME collector — Stage 4 (single-flight aging, ADR-091).** Held for the
    /// duration of an `age + migrate_live_globals + begin_gen_drain` sequence so at
    /// most one process ages at a time. Two processes racing the safepoint could both
    /// observe the other slot empty and both run the migration, double-copying the
    /// live image into the new generation (wasteful, and the second's reconcile would
    /// mostly no-op). A plain CAS gate ([`Heap::begin_aging`]/[`Heap::end_aging`]) —
    /// the loser skips this safepoint and retries at the next one.
    aging: AtomicBool,
    /// **RUNTIME collector — Stage 4 (aging counter, ADR-091).** Bumped by
    /// [`Heap::age_runtime`]; surfaced via [`Heap::runtime_aged_count`] so a test can
    /// confirm the multi-generation collector aged, even when a full free is timing-
    /// dependent.
    aged_count: AtomicU64,
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
    promote_lock: RwLock<()>,
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
    saved: SymbolMap<Value>,
    /// The `rt_collect_block` depth this snapshot established (post-increment). Restore
    /// asserts the live depth still matches — catching an out-of-order (non-LIFO) restore,
    /// which would release the wrong scope's suppression.
    block_depth: u32,
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
    runtime: Arc<RuntimeCode>,
    gen: usize,
}

impl GenPin {
    fn new(runtime: Arc<RuntimeCode>, gen: usize) -> Self {
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
    fn cur_gen(&self) -> usize {
        self.current_gen.load(Ordering::Relaxed)
    }
    /// A guard on the current code generation's slabs — the target of `promote`/`def`
    /// and the region the single-process compactor operates on. Derefs to
    /// `&CodeSlabs`; hold it (don't re-call) across a multi-step read so the slab
    /// can't be freed mid-use.
    #[inline]
    fn cur_code(&self) -> Guard<Arc<CodeSlabs>> {
        self.gens[self.cur_gen()].load()
    }
    // Append a value into the *current* code generation and mint a handle tagged
    // with that generation, so a read later resolves the right slab (2-generation
    // collector, ADR-091). Centralised so every RUNTIME mint is gen-tagged the same
    // way — the push slab and the handle's `code_gen` can never disagree.
    #[inline]
    fn push_str(&self, v: String) -> StrId {
        let g = self.cur_gen();
        StrId::runtime_gen(self.gens[g].load().strings.push(LocalString::inline(v)), g)
    }
    #[inline]
    fn push_bigint(&self, v: num_bigint::BigInt) -> BigIntId {
        let g = self.cur_gen();
        BigIntId::runtime_gen(self.gens[g].load().bigints.push(v), g)
    }
    #[inline]
    fn push_decimal(&self, v: bigdecimal::BigDecimal) -> DecimalId {
        let g = self.cur_gen();
        DecimalId::runtime_gen(self.gens[g].load().decimals.push(v), g)
    }
    #[inline]
    fn push_ratio(&self, v: num_rational::BigRational) -> RatioId {
        let g = self.cur_gen();
        RatioId::runtime_gen(self.gens[g].load().ratios.push(v), g)
    }
    #[inline]
    fn push_bytes(&self, v: Arc<SharedBlob>) -> BytesId {
        let g = self.cur_gen();
        BytesId::runtime_gen(self.gens[g].load().bytes.push(v), g)
    }
    #[inline]
    fn push_rope(&self, v: ropey::Rope) -> RopeId {
        let g = self.cur_gen();
        RopeId::runtime_gen(self.gens[g].load().ropes.push(v), g)
    }
    #[inline]
    fn push_vec(&self, v: VecStore) -> VecId {
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
    fn globals_read(&self) -> RwLockReadGuard<'_, SymbolMap<Value>> {
        self.globals.read().unwrap_or_else(|e| e.into_inner())
    }
    fn globals_write(&self) -> RwLockWriteGuard<'_, SymbolMap<Value>> {
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
    fn is_sealed(&self, sym: Symbol) -> bool {
        !crate::core::value::is_dynamic(sym)
            && self
                .sealed
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .contains(&sym)
    }
    /// Reserve `sym` — called for each name an embedded std module defines as it
    /// loads, so the module's own surface becomes reserved once it exists.
    fn seal(&self, sym: Symbol) {
        self.sealed
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sym);
    }

    /// Record `sym` (a qualified global name) as module-private — called by the
    /// `%mark-private` primitive that a `defn-`/`def-` emits (after its `def`).
    /// Idempotent insert. See [`RuntimeCode::private`].
    fn mark_private(&self, sym: Symbol) {
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
    fn unmark_private(&self, sym: Symbol) {
        self.private
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&sym);
    }
    /// Replace the whole private set (the `%isolate` restore — see
    /// [`Heap::restore_private_names`]). Set-level because the caller cannot enumerate
    /// what the isolated thunk marked; one write-lock swap, not a diff.
    fn restore_private(&self, names: Vec<Symbol>) {
        *self.private.write().unwrap_or_else(|e| e.into_inner()) = names.into_iter().collect();
    }
    /// Is `sym` recorded module-private? The authoritative (and, since ADR-146 step 2,
    /// the *only*) half of [`Heap::is_private`].
    /// Record `sym`'s stability metadata, replacing whatever was there. See [`NameMeta`].
    fn set_meta(&self, sym: Symbol, meta: NameMeta) {
        self.meta
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sym, meta);
    }
    /// Drop `sym`'s metadata — called from `env_define` on every global definition, so a
    /// redefined name does not inherit the old one's `:deprecated`/`:beta` facts. A
    /// `(meta …)` form re-records immediately, exactly as `%mark-private` does for privacy.
    fn clear_meta(&self, sym: Symbol) {
        self.meta
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&sym);
    }
    /// `sym`'s recorded metadata, if any.
    fn meta_of(&self, sym: Symbol) -> Option<NameMeta> {
        self.meta
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&sym)
            .cloned()
    }

    fn is_private_recorded(&self, sym: Symbol) -> bool {
        self.private
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&sym)
    }

    /// As `globals_read`/`globals_write`, for the def-site table (same
    /// poison-recovery rationale — entries are owned data, never structurally
    /// corrupting on a panicked writer).
    fn def_sites_read(&self) -> RwLockReadGuard<'_, HashMap<Symbol, SourceLoc>> {
        self.def_sites.read().unwrap_or_else(|e| e.into_inner())
    }
    /// RUNTIME-form source position + file by `(code_gen, slab index)`, or `None`.
    /// See [`Self::positions`].
    fn position_of(&self, idx: usize, code_gen: usize) -> Option<FormPos> {
        self.positions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&rt_pos_key(idx, code_gen))
            .cloned()
    }
    /// Record a RUNTIME-form source position + file (called by `promote`). See [`Self::positions`].
    fn set_position(&self, idx: usize, code_gen: usize, entry: FormPos) {
        self.positions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(rt_pos_key(idx, code_gen), entry);
    }

    fn def_sites_write(&self) -> RwLockWriteGuard<'_, HashMap<Symbol, SourceLoc>> {
        self.def_sites.write().unwrap_or_else(|e| e.into_inner())
    }
}

/// The set of global observations one `check-file-deps` made (ADR-119 Phase 2),
/// accumulated in [`Heap::check_dep_rec`] while the check runs. The checker's
/// `obs_*` wrappers push into it; `types::check::deps` turns it into the file's
/// serializable dep-keys and fingerprint. Plain data — no layering dependency on
/// the checker.
#[derive(Default)]
pub(crate) struct CheckDepRec {
    /// Global symbols whose binding/arity/sig the check observed.
    pub(crate) syms: std::collections::HashSet<Symbol>,
    /// `mod/` prefixes whose known-ness the check queried.
    pub(crate) known_ns: std::collections::HashSet<String>,
    /// `mod/` prefixes whose export set the check read (`:use` resolution).
    pub(crate) exports: std::collections::HashSet<String>,
    /// Globals DEFINED in this file (its own def-names). Excluded from the dep-keys:
    /// a file's dependency on its own globals is already covered by its own mtime, so
    /// storing them would just bloat the manifest (a 1000-def file otherwise records
    /// ~1000 self-names — the 64MB-manifest bug).
    pub(crate) own: std::collections::HashSet<Symbol>,
    /// Whether the check consulted the `*protocols*` table.
    pub(crate) protocols: bool,
}

/// **Loader / checker / namespace state — cold for a worker process** (ADR-175 follow-up,
/// 2026-07-29). Every field here is used by the process that *loads modules or runs the
/// checker*; a spawned green process that only runs Brood code never touches any of them.
/// Held behind `Option<Box<ColdHeap>>` on the `Heap` so a worker pays 8 bytes instead of
/// ~320, which matters because `Box<Process>` sits near a mimalloc size-class boundary:
/// measured, +320 B of `Process` costs **+640 B of RSS per process**, so the same move
/// downward is worth a class.
///
/// Allocated lazily by [`Heap::cold_mut`] on first write; readers go through
/// What a bare name in the `(:use …)` import table resolves to (ADR-235). One
/// contributing module → `One`; two or more `(:use …)`d modules exporting the same
/// bare name → `Ambiguous`, which is not an error until the name is actually referenced
/// bare, at which point the resolver reports the candidates. This lazy shape is what
/// lets `(:use a) (:use b)` coexist when the caller only touches non-overlapping names.
#[derive(Clone, Debug)]
pub enum ImportEntry {
    /// Imported from exactly one module — the qualified global to resolve to.
    One(Symbol),
    /// Contributed by two or more modules — the sorted candidate qualified globals.
    Ambiguous(Vec<Symbol>),
}

/// [`Heap::cold`] and treat `None` as empty, which is exactly right — an absent
/// `ColdHeap` means "this process has loaded nothing and checked nothing".
#[derive(Default)]

pub(crate) struct ColdHeap {
    /// Nesting depth of an embedded-module load (ADR-166). See `Heap::in_module_load`.
    pub(crate) module_load_depth: u32,
    /// Source position of LOCAL list forms, keyed by [`form_pos_key`].
    pub(crate) form_pos: HashMap<u64, FormPos>,
    /// The file currently being `load`ed, exposed via `(current-file)`.
    pub(crate) current_file: Option<String>,
    /// `current_file` pre-shared as an `Arc<str>`, kept in step by
    /// [`Heap::set_current_file`]. Every recorded form position stores the file it came
    /// from, and `set_form_pos` used to build that with `Arc::from(&str)` — a fresh
    /// allocation *and* a copy of the path, once per list form read. Loading 1000
    /// thousand-line modules reads ~1.15M list forms, so that was ~1.15M allocations of
    /// the same handful of strings. Cloning this instead is a refcount bump.
    pub(crate) current_file_arc: Option<Arc<str>>,
    /// The namespace being compiled (`defmodule`).
    pub(crate) compile_ns: Option<Symbol>,
    /// Names the current namespace defines — the *active* forward-ref set, which
    /// [`Heap::activate_ns_region`] switches to the module `%in-ns` opens.
    pub(crate) ns_known_names: HashSet<Symbol>,
    /// Forward-ref pre-scan **per module region** (ADR-223): a file may declare more
    /// than one `(defmodule …)`, and each opens a region whose bare def-heads live under
    /// its module key here. `%in-ns` activates the current region's set into
    /// `ns_known_names`, so a bare reference qualifies only against the module it is
    /// actually inside — the mechanism that lets several modules share one file. Empty on
    /// the sticky REPL path (no whole-file pre-scan), where `ns_assume_own` covers instead.
    pub(crate) ns_known_by_module: HashMap<Symbol, HashSet<Symbol>>,
    /// Compiling a form that has **no** whole-file pre-scan behind it (a runtime
    /// `eval`), so `ns_known_names` cannot answer "will this namespace define it?".
    pub(crate) ns_assume_own: bool,
    /// `(:use …)` import map for the namespace being compiled: bare name → what it
    /// resolves to. A name imported from one module is `One(qualified)`; a name two or
    /// more `(:use …)`d modules both export is `Ambiguous([…])` — not an error at import
    /// time (ADR-235), only if the bare name is actually *used*, at which point the
    /// resolver names the candidates. Rides the same per-file save/restore as the rest.
    pub(crate) imports: HashMap<Symbol, ImportEntry>,
    /// Package-rooted namespaces (ADR-070): while loading a *dependency* `foo`, its
    /// local name is the active prefix, so a module the file declares `(defmodule b)`
    /// roots to `foo/b` and its intra-package `(:use b)`/`(:alias b …)` targets root
    /// too. `None` outside a dep load (the root project / std stay unrooted, short).
    pub(crate) package_prefix: Option<Symbol>,
    /// The short module names the active package provides — the set that decides
    /// whether a referenced module name is *intra-package* (root it) or external
    /// (a std/other-dep name, leave it bare). Empty when `package_prefix` is `None`.
    pub(crate) package_modules: HashSet<Symbol>,
}

/// Checker-only heap state, lazily boxed off [`Heap`] (see [`Heap::check`]). None of it
/// is touched by running Brood code — only by `nest check` / `check-file-deps`.
#[derive(Default)]
pub(crate) struct CheckHeap {
    /// `mod/` prefix → its public `(bare, qualified)` export pairs, keyed by the global
    /// count so a `def` invalidates it. Lets a whole-project check build the index in ONE
    /// pass instead of rescanning every global per file (O(files²)).
    exports: Option<(
        usize,
        std::sync::Arc<std::collections::HashMap<String, Vec<(Symbol, Symbol)>>>,
    )>,
    /// The set of `mod/` namespace prefixes in the loaded image, count-keyed and shared.
    known_ns: Option<(usize, std::sync::Arc<std::collections::HashSet<String>>)>,
    /// The in-flight incremental-check dependency record (ADR-119).
    dep_rec: Option<CheckDepRec>,
}

impl Heap {
    /// The old-generation slabs. Callers must already hold an OLD handle, whose existence
    /// implies a promotion allocated this — see [`Heap::old`](Self::old)'s field docs.
    #[inline]
    fn old(&self) -> &Slabs {
        self.old
            .as_deref()
            .expect("an OLD handle implies the old generation was allocated")
    }
    /// The old-generation slabs for mutation, allocating on first promotion.
    #[inline]
    fn old_mut(&mut self) -> &mut Slabs {
        self.old.get_or_insert_with(Box::default)
    }
    /// The old generation if this process ever promoted — for aggregate walks (capacity
    /// sums, GC scans) that must tolerate its absence rather than assume it.
    #[inline]
    fn old_opt(&self) -> Option<&Slabs> {
        self.old.as_deref()
    }
}

impl Heap {
    /// The checker state, allocating it on first use. Callers hold `&self` (see
    /// [`Heap::check`]), so this returns a guard rather than a reference.
    fn check_mut(&self) -> std::cell::RefMut<'_, Box<CheckHeap>> {
        let mut b = self.check.borrow_mut();
        if b.is_none() {
            *b = Some(Box::default());
        }
        std::cell::RefMut::map(b, |o| o.as_mut().expect("just filled"))
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
    /// The old generation, lazily boxed. Empty for almost every process — measured at
    /// **7 of 300,000** on `spawn-live`, because a process only populates it by surviving a
    /// minor collection, and a short-lived worker never collects at all. Inline it is 264 B
    /// (eleven `Vec` headers) on every `Heap`, which is inline in `Box<Process>`; boxed it
    /// is 8. Reads go through [`Heap::old`], which may only be called when an OLD handle
    /// exists — and an OLD handle can only exist if a promotion allocated this.
    old: Option<Box<Slabs>>,
    prelude: Arc<SharedCode>,
    runtime: Arc<RuntimeCode>,
    /// Nesting depth of an **embedded-module load** in this process (ADR-166). While
    /// non-zero, a global `def` of a reserved name is permitted *and* reserves the
    /// name — that is how a std module's own surface (`set/union`, `path/join`)
    /// becomes reserved once it exists, and how re-loading one stays idempotent
    /// (`require--await` deliberately re-evaluates a module whose loader died).
    /// Per-process, so two processes loading different modules never see each
    /// other's exemption; incremented and decremented by `%load-module-source`,
    /// which restores it even when the load throws.

    /// **Per-process pinned-read cache for the RUNTIME generations.** A RUNTIME handle
    /// deref must pin `gens[g]`'s `Arc<CodeSlabs>` (so a concurrent Stage-4 free can't drop
    /// it mid-read), but taking a fresh `ArcSwap::load` guard per deref dominated
    /// global-data-heavy hot loops. Instead each slot caches the last-loaded `Arc` plus the
    /// [`RuntimeCode::gen_version`] it was loaded at; [`code_gen_pinned`](Self::code_gen_pinned)
    /// clones the cached `Arc` (a single refcount bump) when the version is unchanged and
    /// only `load_full`s on a real generation replacement (Stage-4 free / compaction store —
    /// rare). `RefCell`/`Cell`: the `Heap` is single-threaded (one worker owns a process at a
    /// time). `gen_cache_ver` starts at `u64::MAX` so the first read always populates.
    gen_cache: [RefCell<Option<Arc<CodeSlabs>>>; 2],
    gen_cache_ver: [Cell<u64>; 2],
    /// **Per-process parse cache for `(fn …)` literals**, keyed by the `MakeClosure`
    /// site's `fn_rest` AST handle. Building a closure re-parses its param
    /// lists/optionals/doc and walks the (RUNTIME) body cons list on every creation —
    /// pure waste in a closure-in-a-loop (a `receive` matcher, a per-frame callback),
    /// since the parse is a function of the fixed AST. This memoises the parsed
    /// [`ClosureTemplate`], so creation drops to cloning the arm `Vec` out of it + env
    /// attach — the re-parse, RUNTIME-AST walk, and pass-through analysis are gone. (The
    /// per-instance arm-`Vec` clone/drop is the remaining cost; sharing the arms via an
    /// `Arc<[ClosureArm]>` in `Closure` would remove it too, but touches the GC's in-place
    /// arm rewrite — deferred.) Invalidated exactly like [`gen_cache`](Self::gen_cache): the arms hold RUNTIME
    /// AST handles, which move only on a `gen_version` bump (Stage-4 free / compaction),
    /// so a version change clears the whole map (`closure_tpl_ver` starts at `u64::MAX`
    /// so the first use populates). `RefCell`: the `Heap` is single-threaded (one worker
    /// owns a process at a time); `Arc` (not `Rc`) so the `Heap` stays `Send` across the
    /// worker migration a process undergoes. Uses [`SymbolHasher`] — a `PairId` hashes as a
    /// single `u64`, so the lookup (once per closure creation) takes its bijective
    /// `write_u64` fast path instead of stock `SipHash`.
    closure_tpl_cache: RefCell<ClosureTemplateMap>,
    closure_tpl_ver: Cell<u64>,
    /// **Capture-free closure constant cache.** A `(fn …)` literal with no lexical captures
    /// and no self-name is a *constant* — its `env` is [`EnvId::GLOBAL`], so it late-binds
    /// globals but captures nothing, and every evaluation would otherwise rebuild an
    /// identical closure and (for a `spawn` thunk) re-`promote` it into the RUNTIME region,
    /// piling up garbage the collector must reclaim. This memoises the closure built **once**
    /// and promoted to a stable RUNTIME handle, so re-evaluating the literal returns the same
    /// handle — no alloc, no re-promote (`(spawn (worker))` in a fan-out drops ~7×). Keyed and
    /// invalidated exactly like [`closure_tpl_cache`](Self::closure_tpl_cache): the handle is a
    /// RUNTIME value that moves only on a `gen_version` bump, so a version change clears the
    /// map (`closure_const_ver` starts at `u64::MAX` so the first use populates).
    closure_const_cache: RefCell<ConstClosureMap>,
    closure_const_ver: Cell<u64>,
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

    /// The file currently being `load`ed, exposed via `(current-file)`. Saved and
    /// restored around each load so nested loads don't clobber the outer file.

    /// The namespace currently being compiled into (ADR-065). `None` = root (the
    /// prelude, plain code, and the REPL until an `(ns …)` form runs). Set by the
    /// `(ns foo)` form via the `%in-ns` primitive; read by the resolver pass
    /// (`eval::macros::resolve`) to qualify definition heads and free references to
    /// `foo/name`. Per-process compile state — NOT a shared global, which would race
    /// across green processes (`RuntimeCode` is shared). File/module loaders save +
    /// reset this to root per file (so a `require`d file starts at root); the REPL
    /// driver leaves it sticky across entries.

    /// Names the current-namespace file will define (its top-level `def`/`defmacro`
    /// heads), pre-scanned when an `(ns …)` form runs so the resolver can qualify a
    /// *forward* reference (`bar` used before `foo/bar` is defined) — without it,
    /// such a reference would silently stay bare (order-dependent miscompile). Bare
    /// symbols only; consulted alongside the live global table. Cleared/repopulated
    /// per file by the loader.

    /// Names the current file `(:use …)`-imported: bare name → qualified global
    /// (`describe` → `test/describe`). Populated by `%refer` when the `(ns …)`
    /// header runs; consulted by the resolver after the current namespace and
    /// before root fall-through. Per-file like `ns_known_names` — reset/restored
    /// by the loaders so imports never leak across files (ADR-065 inc-2).

    /// This process's dynamic-variable binding stack (the `binding` form). Each
    /// `binding` pushes its `(symbol, value)` pairs and pops them when its body
    /// returns (even on error); a read of a dynamic var consults this — latest
    /// binding wins — before the shared global table (see [`Heap::env_get`]).
    /// Per-process and not shared: a `spawn`ed child starts with an empty stack,
    /// so dynamic bindings never cross to another process (data isn't shared).
    /// Empty whenever no `binding` is active — so it's free on the common path
    /// and holds no LOCAL handles across a top-level arena reset.
    dynamics: Vec<(Symbol, Value)>,
    /// The debugger's durable per-process causal context (ADR-174 send-level slice):
    /// a settable slot that, unlike a `binding` on [`dynamics`], survives across
    /// `receive` and migration (so a long-lived server adopts the sender's context
    /// per message). GC-traced exactly where `dynamics` is. `#[cfg(dev-tools)]` — a
    /// lean release has no such field, so the whole send-level path compiles out.
    #[cfg(feature = "dev-tools")]
    trace_context: Option<Value>,
    /// Whether [`trace_context`] is the process's OWN context (set by `with-debugger`
    /// / `span`) — which `spawn` propagates to children — versus one merely ADOPTED
    /// from a received message, which is used to handle that message but must NOT
    /// propagate onward (else an adopted context leaks transitively through unrelated
    /// spawns). Meaningful only when `trace_context` is `Some`.
    #[cfg(feature = "dev-tools")]
    trace_context_own: bool,
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
    /// Memoized `mod/name` → `prefix/mod/name` rooting for intra-package *qualified
    /// references* (ADR-070). A miss in the global table falls back to the rooted name
    /// (see [`root_qualified_ref`](Self::root_qualified_ref)); this caches the symbol→symbol answer —
    /// including the negative one (`None` = not intra-package, don't retry) — so the
    /// fallback costs one hash lookup rather than a `format!` + intern per reference.
    /// Keyed only by symbol, which is safe because [`set_package_context`] clears it:
    /// the mapping is a property of the *active* package context, and that's the one
    /// place the context changes. `RefCell` because `env_get` is `&self`; per-process.
    rooted_ref_ic: RefCell<SymbolMap<Option<Symbol>>>,
    /// Cached `mod/` prefix → that module's public exports (`(bare, qualified)` pairs).
    /// Used ONLY by the advisory whole-project checker's direct import setup
    /// (`types::check::setup_check_imports`): a whole-project check resolves every file's
    /// `(:use …)` clauses, and enumerating a module's exports by scanning all globals per
    /// file was O(files²). Keyed by the global symbol **count**, which is safe here because
    /// the checker's per-file loop performs NO `def`s (it sets up imports without evaling
    /// the header) and runs no `%isolate` rollback mid-loop, so the global set — hence the
    /// count — is stable across the loop. (NOT used by runtime `%refer`, where `%isolate`
    /// rollback could otherwise collide counts.) Per-process; built once per check.
    /// Cached set of `mod/` namespace prefixes the loaded image knows — the checker's
    /// `known_ns` (decides whether an unresolved *qualified* name is a real unbound ref or
    /// one in an unloaded module). Same rationale + count-keying + checker-only soundness as
    /// [`module_exports_cache`](Self::module_exports_cache): rebuilding it by scanning all
    /// globals per file was the residual O(files²) after the header-eval redesign.

    /// Phase-2 incremental-check dependency recorder (ADR-119). `Some` only while a
    /// `check-file-deps` runs *on this process*; the advisory checker's `obs_*`
    /// wrappers record every global observation into it. Living on the **heap** (not
    /// a thread-local) makes it per-process — a green process owns its heap and it
    /// migrates *with* the process — so dep-capture can run in parallel across the
    /// worker pool without two concurrent checks (or a mid-check migration/preempt)
    /// clobbering each other's record. Off (`None`) for all normal eval; the record
    /// borrow on the hot per-symbol observation path is a single `RefCell` check.

    /// Explicit GC root stack — the evaluator's **operand stack** (ADR-061).
    /// Every LOCAL [`Value`] an eval frame still needs *after* a nested `eval`
    /// (its accumulated `argv`, literal accumulators, `callee`, the `call_form`,
    /// the cons-spine cursor) is pushed here for the duration of that call, then
    /// re-read via [`root_at`](Self::root_at) afterwards (the copying collector
    /// relocates these in place). This is what lets the safepoint collect at
    /// **any** eval depth, not just the outermost — see `docs/memory-model.md`.
    /// Also used by `eval_str`/`eval_source` for the unevaluated forms vector.
    /// Empty between top-level forms.
    /// **Delivered-message slots (ADR-177 / L1).** When a `send` finds this process
    /// *parked*, it copies the value straight into this heap — skipping the wire-format
    /// `Message` round trip entirely — and parks the result here; the envelope in the
    /// mailbox carries the slot index. A traced root set, flushed in place by `collect`
    /// exactly like [`Self::roots`], because a queued message can sit through any number
    /// of the receiver's collections before a selective `receive` gets to it.
    ///
    /// It is a **slot table, not a stack**: `roots` is the operand stack and is truncated
    /// from ~109 sites (every frame pop), so a long-lived value cannot live there. A
    /// consumed slot is recorded on the table's free list and reused, so the table stays
    /// as small as the process's peak *undelivered* Local message count — normally 0 or 1.
    ///
    /// Boxed and lazily allocated: inline it is 24+ bytes on every `Heap`, and a `Heap`
    /// is inline in `Box<Process>`, where bytes cost about 2:1 in RSS via mimalloc's size
    /// classes — measured at `spawn` +5.9% for the inline `Vec`. `None` until this process
    /// is actually handed a fast-path message, which most processes never are.
    msg_roots: Option<Box<MsgRoots>>,
    /// Loader/checker/namespace state — see [`ColdHeap`]. `None` until a module load,
    /// namespace compile or checker run needs it, so a plain worker process never
    /// allocates it (worth a mimalloc size class on `Box<Process>`).
    cold: Option<Box<ColdHeap>>,
    /// Checker caches. These stay on `Heap` rather than moving into [`ColdHeap`]
    /// because they are filled through `&self` (the checker's read paths), which
    /// cannot lazily allocate the boxed cold state. ~96 bytes; the six fields that
    /// *are* in `ColdHeap` were enough to drop `Box<Process>` a size class.
    /// Checker-only state — see [`CheckHeap`]. Lazily boxed and `None` until a check
    /// actually runs, because it is **288 bytes** inline (`check_dep_rec` alone is 208,
    /// four `HashSet`s) on every `Heap`, and a `Heap` is inline in `Box<Process>`. A
    /// spawned worker process never checks anything, so it never pays for this. Behind a
    /// `RefCell` rather than a plain `Option<Box<_>>` (the shape [`ColdHeap`] uses)
    /// because every one of these is filled through `&self` — which is exactly why M1
    /// left them inline.
    check: RefCell<Option<Box<CheckHeap>>>,
    /// The GC root stack. A [`roots_buf::RootsBuf`], not a `Vec<Value>`, so its
    /// (ptr, len, cap) header sits at fixed offsets for the JIT (§7.5); same semantics.
    roots: roots_buf::RootsBuf,
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
    /// [`park_trim_probe`] as of this process's last park-time trim — the baseline
    /// [`Heap::trim_parked`] measures growth against, in slab elements.
    park_trim_mark: usize,
    /// Adaptive **RUNTIME**-collection trigger: the eval safepoint reclaims the shared
    /// code region once the RUNTIME closure count crosses this — in-place compaction when
    /// this heap uniquely owns the runtime, else a step of the 2-generation collector.
    /// Recomputed after each reclaim as `max(RT_GC_FLOOR, 2 * live)`, so the collector
    /// re-enters only as the region grows rather than bailing every safepoint.
    /// `usize::MAX` while [`gc_enabled`] is false. See [`rt_gc_floor`] and
    /// [`maybe_runtime_collect`](Self::maybe_runtime_collect).
    rt_gc_threshold: usize,
    /// A per-heap override of [`rt_gc_floor`] — the process-wide floor is a `OnceLock`
    /// read from `BROOD_RT_GC_FLOOR` exactly once, so a test that lowered it through the
    /// environment lowered it for every test that ran after it in the same process.
    /// `runtime_collector`'s three promotion-count tests read 231 promoted closures for
    /// 3000 redefs whenever a floor-setting test happened to run first (KI-86). A test
    /// that wants a low floor says so on ITS heap ([`Heap::set_rt_gc_floor`]).
    rt_gc_floor_override: Option<usize>,
    /// GC switch. `false` during the prelude *build* (`Heap::new`), `true` for
    /// real process heaps (`Heap::with_regions`); also forced `false` when the
    /// prelude `SharedCode` `Arc` is the default (empty) one, since a missing
    /// prelude means a freshly-built builder heap that's about to freeze.
    gc_enabled: bool,
    /// Re-entrant suppression of RUNTIME-region compaction while a **globals snapshot
    /// is outstanding**. [`snapshot_globals`] clones the global table — a
    /// `SymbolMap<Value>` of raw RUNTIME handles — off the graph and hands it back for a
    /// later [`restore_globals`] (the `%isolate` protocol). A compaction between the two
    /// would relocate those handles, leaving the snapshot pointing at recycled slots — so
    /// the restore reinstalls stale handles and unrelated globals silently misdispatch
    /// (KI-6). `snapshot_globals` increments this and `restore_globals` decrements it, so
    /// the invariant "no compaction while a snapshot is live" holds *structurally* — any
    /// caller of the snapshot/restore protocol is covered, not just `%isolate`.
    /// [`runtime_collect_with`] bails while it's >0 (the choke point for both the auto
    /// safepoint path — via [`rt_gc_due`] — and a manual `(%)`); the
    /// isolate's `def`s become garbage at restore and are reclaimed by the next safepoint.
    /// `Cell` so the `&self` snapshot/restore can bump it; a counter (not a bool) so nested
    /// snapshots compose. [`rt_gc_due`]: Self::rt_gc_due
    rt_collect_block: std::cell::Cell<u32>,
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
    /// path (binds finish in the nursery). Env-frame binding (late binding / `def`
    /// rebinding, ADR-013) is the **only** data mutation the collector must track;
    /// every Lisp value is immutable, so the minor flip can safely rely on the
    /// invariant that old never points to young everywhere else.
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
    /// tested arena-flip helper), which share that path. Read out via `(%)`.
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
    /// GC **pause durations** (the observability timing tier, ROADMAP survey
    /// gap #4 — counts alone can't answer "is GC why this frame stuttered").
    /// Cumulative / max / most-recent collection wall time in nanoseconds,
    /// measured around [`collect`](Self::collect)'s body (covers both the
    /// legacy flip and the generational path). Timing cost is two `Instant`
    /// reads per *collection* — noise against the µs–ms the collection itself
    /// takes. Surfaced by `(%)` as `:pause-total-us` / `:pause-max-us` /
    /// `:pause-last-us`.
    gc_ns_total: u64,
    gc_ns_max: u64,
    gc_ns_last: u64,
    /// Per-process heap limit (bytes), the BEAM `max_heap_size` analogue — set by
    /// this process on itself via `(proc/flag :max-heap n)`, `None` = unlimited
    /// (the default; the ADR-043 global soft/hard cap is separate). Checked
    /// **after** each collection against the *surviving* footprint (nursery +
    /// old gen), so transient garbage a collection reclaims never trips it.
    proc_mem_limit: Option<usize>,
    /// Sticky post-collection flag: `Some(live_bytes)` when the last collection
    /// left the heap over `proc_mem_limit`. Probed (and cleared) at the eval/VM
    /// safepoints, which raise a catchable error **in this process only** — the
    /// per-process isolation the global hard cap (whole-OS-process abort) lacks.
    proc_limit_hit: Option<usize>,
    /// `(proc/flag :send-errors on)` — when set, a `send` whose target *node*
    /// is unknown/disconnected raises a catchable `:noconnection` error instead
    /// of silently dropping the message (the dist self-healing seam). Default
    /// off: Erlang's silent-send semantics.
    proc_send_errors: bool,
    /// Per-process GC **trace** switch (`(gc-trace on/off)`, defaulted from
    /// `BROOD_GC_TRACE`). When set, each minor/major collection prints a one-line
    /// summary to stderr — a Tier-1 observability aid for tests/benchmarks (the
    /// numbers `(%)` reports as cumulative totals, but per collection as
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
    vm_cache: RefCell<VmCacheMap<vm_cache::VmCacheEntry>>,
    /// The [`RuntimeCode::free_epoch`] this process last synced its [`Self::vm_cache`]
    /// to (ADR-091 Stage 4). When the shared free-epoch advances (a generation was
    /// freed and its slot may be reused with bit-identical handles), the `vm_cache`
    /// read path clears the cache once and updates this — so a stale compiled body
    /// can't alias a reused handle. Cheap: one relaxed atomic load + compare per
    /// closure-call cache lookup, a full clear only on the rare free.
    seen_free_epoch: Cell<u64>,
    /// **RUNTIME collector — Stage 4 (drain free-attempt throttle, ADR-091).** A
    /// per-process tick rate-limiting how often this process runs the multigen drain
    /// **free-attempt** ([`crate::process::free_drained_gen`] → the O·live-process
    /// `report_parked_liveness` whole-registry scan) at its safepoint. While a drain is
    /// armed the threshold is held low so every safepoint re-enters the collector; when
    /// the drain can't yet complete — a long-lived process still runs old-generation
    /// code, so the generation stays pinned — that means the O(live-process) registry
    /// scan runs on *every* safepoint of *every* worker purely to re-discover "still not
    /// drained" (measured: 800 k scans / 20 M mailbox locks on a 30-round repro, ~6×
    /// the default runtime). Throttling the free-attempt to 1/[`RT_DRAIN_SCAN_STRIDE`]
    /// cuts that ~stride-fold; the free is still attempted regularly (no lost wakeup) as
    /// long as any process reaches a safepoint, and every process's O(1) drain
    /// self-report still runs every frame so acks stay current. A plain `Cell` (the
    /// `Heap` is single-threaded), so the throttle adds no atomic or cross-core traffic.
    rt_drain_tick: Cell<u32>,
    /// **RUNTIME collector — Stage 3c (local clean-ack cache, ADR-091).** The drain
    /// epoch this process last reported *clean* for (0 = none). While a drain stays
    /// armed — which it does for the whole run whenever a long-lived process pins the
    /// draining generation (a top-level test-runner loop still executing old-gen code;
    /// Erlang has the same local-call limitation) — every process's safepoint calls
    /// `report_gen_liveness`, and a process that already acked clean would re-take the
    /// shared `drain_acks` *read* lock every frame just to re-confirm its ack. That
    /// per-frame lock, across every worker for the whole run, is the residual cost once
    /// the scan and the dirty write are handled (the `rounds`-shape ~6× overhead). This
    /// `Cell` short-circuits it: once clean for epoch E, the process is clean for E by
    /// the clean-stays-clean invariant, so a `Cell` read + compare replaces the lock. A
    /// fresh drain bumps the epoch (≠ the cached value) so the process re-reports. Plain
    /// `Cell`: the `Heap` is single-threaded.
    acked_drain_epoch: Cell<u64>,
    /// Per-heap safepoint tick throttling the drain self-report to 1/[`DRAIN_REPORT_STRIDE`]
    /// (see the const). Plain `Cell` (the `Heap` is single-threaded), read/written with no
    /// shared atomic so a throttled frame is nearly free; a miscount only shifts *when* a
    /// report fires, never its correctness. Reset by [`begin_gen_drain`](Self::begin_gen_drain)
    /// on the arming process so its first report is prompt.
    drain_report_tick: Cell<u32>,
    /// **Phase-2 dirty re-validation throttle** for the drain self-report. When the private
    /// probe finds this process dirty via Phase 2 (a RUNTIME handle embedded in its LOCAL
    /// heap data — see `runtime_gen_referenced_private`), it records the drain epoch here and
    /// re-runs that O(heap) walk only every [`P2_REVALIDATE_STRIDE`] safepoints, reporting a
    /// cheap stale-dirty verdict in between. Reset to `u64::MAX` (an epoch that never matches)
    /// when the probe next finds it clean. `p2_dirty_tick` counts safepoints within the epoch.
    /// Plain `Cell`s: the `Heap` is single-threaded.
    p2_dirty_epoch: Cell<u64>,
    p2_dirty_tick: Cell<u32>,
    /// **Phase-1 dirty re-validation throttle**, the deep-recursion counterpart of the
    /// Phase-2 pair above. Armed only while this process's Phase-1 seed exceeds
    /// [`P1_LARGE_SEED`] — a `roots` stack that has grown with recursion depth — so a
    /// shallow process is never throttled and keeps acking on its very next safepoint.
    /// See [`P1_REVALIDATE_STRIDE`].
    p1_dirty_epoch: Cell<u64>,
    p1_dirty_tick: Cell<u32>,
    /// **Receive-mark** (ADR-195): the most recent `(ref)` this process minted, paired with
    /// its mailbox's arrival sequence at that instant. A `receive` whose every clause pins
    /// that ref can start its scan at the first message with `seq >= mark`, because a
    /// message enqueued *before* the ref existed cannot possibly carry it — turning a
    /// backlogged selective receive from O(backlog) into a binary search.
    ///
    /// One entry, deliberately: it covers `(let (r (ref)) (send …) (receive ([:reply ^r v]
    /// …)))`, which is every synchronous call in the language. A nested call evicts it and
    /// the outer receive simply scans from the front — slower, never wrong.
    recv_mark: Cell<(u64, u64)>,
    /// The compiled arms **currently executing** on this process's stack — a stack
    /// pushed by `compile::vm_apply` (and the top-level `run`) on entry, the top
    /// updated on a tail-call into a different arm, popped on return. `runtime_collect`
    /// walks these after evacuating the RUNTIME region and rewrites the movable
    /// handles their node trees embed (`Const`/`MakeClosure` literals): they're the
    /// one RUNTIME-handle holder the root walk can't reach — the `Arc`'d node tree is
    /// off the GC root graph, and `exec_node` holds it by `&Node`, so the `Arc` can't
    /// be swapped for a relocated copy. (The non-live arms in `vm_cache` are just
    /// cleared and rebuilt lazily; only the live ones need fixup.) Empty unless the VM
    /// is running a body. See ADR-076 / `docs/known-issues.md`.
    live_vm_arms: Vec<Arc<crate::eval::compile::ArmHandle>>,
    /// Call-site inline caches (ADR-096). Indexed by the `site` id a compiled
    /// `Node::Call` with a global-symbol callee carries; each entry caches that
    /// site's most recent resolution — the callee value, and (for a VM-eligible
    /// non-passthrough closure callee) its compiled arm + captured env — stamped
    /// with the global epoch it was resolved at. A probe validates
    /// `(sym, argc, epoch)`, so a `def` rebind, a `restore_globals` swap, or a
    /// RUNTIME compaction (all bump `runtime.version`) invalidates every entry
    /// without a sweep; sym+argc are re-checked so a recycled site id after
    /// [`Heap::runtime_collect`] clears the table can never alias a different
    /// call site into a wrong hit. Per-process (`RefCell`, like `vm_cache`); a
    /// site is allocated at compile time ([`Heap::vm_site_alloc`]), so ids are
    /// only as dense as the code this process actually compiled.
    /// See [`Heap::note_jit_deopt_reason`] — the JIT's last deopt reason id.
    /// Written only by the JIT runtime; `Heap` is built the same way either way, so the
    /// field stays and only the unused-warning is waived in a no-JIT build.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_deopt_reason: std::cell::Cell<u32>,
    vm_call_ics: RefCell<Vec<Option<CallIcEntry>>>,
    /// Next IC-block base to hand out for [`Self::vm_call_ics`] / [`Self::vm_global_ics`]
    /// (M2b, 2026-08-30). `vm_arm_block` used to take `base = table.len()` and
    /// `resize_with(base + nsites)` — materialising an `Option<CallIcEntry>` slot for
    /// every site of every ACTIVATED arm, entered or not, which for a spawn-once
    /// process is nearly all of them. Bases now come from these counters and the
    /// tables grow **on publish** (`vm_call_ic_put` / `vm_global_ic_put`), the same
    /// lazy-by-its-only-writer pattern the `vm_fast_links` mirror proved on
    /// 2026-08-18. Every reader already tolerates a short table (`.get(abs)` — a
    /// missing slot reads exactly like an unpublished one). Reset to 0 in lockstep
    /// with the table clears + `arm_ic_blocks`, preserving ADR-096's site-id
    /// recycling semantics exactly (the sym/argc guards on every probe exist for it).
    next_ic_base: std::cell::Cell<u32>,
    next_gic_base: std::cell::Cell<u32>,
    /// Depth of live tree-walker→VM re-entries (`eval`'s closure application routing a
    /// VM-eligible callee through `vm_apply` — see `tw_vm_route` in `eval/mod.rs`).
    /// Each re-entry is a real Rust frame, so unbounded routing would turn a
    /// mixed-eligibility mutual TAIL loop (VM-eligible `f` ↔ ineligible `g`) — which
    /// today runs flat because the tree-walker absorbs both sides in its `'tail` loop —
    /// into unbounded native recursion. Past [`crate::eval::TW_REENTRY_BUDGET`] the
    /// router stands down and the call tree-walks exactly as before, so the PTC
    /// invariant (`tail_calls_do_not_overflow`) survives every shape; the budget only
    /// bounds how much VM speed a pathological bounce pattern can buy.
    pub(crate) tw_reentry_depth: std::cell::Cell<u32>,
    /// **IR-readable mirror** of the fast-link memo (Track B / Technique A): a flat,
    /// `#[repr(C)]` side table indexed by the same call-site id as [`Self::vm_call_ics`],
    /// so JIT'd code can read a site's `(epoch, code, nslots, env)` with a raw load + an
    /// epoch compare — no `RefCell` borrow, no `Vec<Option<…>>` niche, no `Cell` (none of
    /// which are safe to touch from Cranelift IR). It is the same data as a
    /// [`CallIcEntry::fast`] memo, written in lockstep by [`Self::vm_call_ic_fast_link`].
    /// A slot is **valid** only when `epoch == global_epoch()`; a `def`/compaction bumps
    /// the epoch (so a stale or recycled slot misses the IR guard and falls to the slow
    /// path), and the table is cleared in lockstep with `vm_call_ics` on a
    /// [`Self::runtime_collect`].
    ///
    /// **Allocated lazily, by its only writer.** It is NOT grown in lockstep with
    /// `vm_call_ics` — `Heap::fastlink_slot_grown` grows it on the first *publish* into
    /// this process, so a process that never JIT-links a site never allocates it at all.
    /// Every reader already tolerated that: the VM probe uses `.get(abs)`, the publish
    /// paths `.get_mut(abs)`, and the IR bounds-checks `site < len` against the length
    /// from `brood_rt_fastlink_base`, which it re-fetches after each Brood→Brood call
    /// because a cold nested call may grow and realloc this table. A missing slot reads
    /// exactly like an unpublished one. Measured on `spawn-live` 2026-08-18: **99.8% of unit
    /// processes now allocate 0 bytes here**, worth 48 B per call site entered — ~193 B while
    /// parked in `receive` (the state that sets peak memory) and ~672 B for a process that has
    /// run its whole body.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    vm_fast_links: RefCell<Vec<FastLink>>,
    /// DEBUG ONLY: per-call-site source position, recorded at compile time and indexed
    /// by the same site id as [`Self::vm_call_ics`]. Lets a crash map a runtime call site
    /// back to its `.blsp` file:line (call-site ids are positional + reset on
    /// `runtime_collect`, so this is grown/cleared in lockstep). For diagnosing the JIT
    /// stale-operand bug — see `dbg_site_pos` / `dbg_set_site_pos`.
    #[cfg(debug_assertions)]
    dbg_site_pos: RefCell<Vec<Option<(crate::error::Pos, Option<Arc<str>>)>>>,
    /// Global-read inline caches (ADR-096) — the value-position counterpart of
    /// [`Self::vm_call_ics`], indexed by a compiled `Node::GlobalIc`'s site id.
    /// Same lifecycle: allocated at compile time, validated by (sym, epoch),
    /// cleared wholesale on a RUNTIME compaction.
    vm_global_ics: RefCell<Vec<Option<GlobalIcEntry>>>,
    /// Per-arm IC block registry (ADR-175 Phase A): [`CompiledArm::uid`] → the
    /// `(call-site base, global-site base)` this process allocated for that arm in
    /// the tables above. Blocks are contiguous, lazily allocated on first activation
    /// ([`Heap::vm_arm_block`]), and never individually freed — a `runtime_collect`
    /// table clear drops the whole map in lockstep with the tables.
    arm_ic_blocks: RefCell<std::collections::HashMap<u64, (u32, u32)>>,
    /// The **currently executing arm's** IC block bases (call sites / global sites).
    /// Set by the VM/JIT drivers at every arm transition; every site-indexed IC
    /// method resolves `base + arm-relative site` through these. Plain `Cell`s: the
    /// Heap is single-threaded (one worker owns a process at a time).
    cur_ic_base: Cell<u32>,
    cur_gic_base: Cell<u32>,
    /// Ability-dispatch inline cache (ADR-172 §7), keyed by an op's `[ability op]` symbol
    /// pair packed into a `u64`, on the fast [`SymbolHasher`]. Per process, like the other
    /// ICs; validated by (`id`, `global_epoch`) so it self-heals on any `def *impls*` /
    /// compaction. See [`Self::vm_dispatch`].
    dispatch_ics:
        RefCell<HashMap<u64, DispatchIcEntry, std::hash::BuildHasherDefault<SymbolHasher>>>,
    /// JIT execution state, per process. These were thread-locals; moved onto the heap so
    /// (a) they travel with a process that migrates worker threads — notably `jit_force_vm`,
    /// which must stay set across a yield during an over-deep VM drain — and (b) each access
    /// is a plain field load rather than a TLS lookup (the linked-call hot path touches them
    /// ~4× per call). Only meaningful while a JIT'd arm is on the stack.
    ///
    /// The executing JIT'd arm's env (its compiled `fn(heap, base)` carries none, but a
    /// Brood→Brood call needs it to resolve a free-global callee). Save/restored around each
    /// native-arm entry ([`jit_tier`]) so re-entry nests correctly.
    // These four are read only from JIT-gated code paths, so a non-jit build
    // (e.g. `brood-lsp`) sees them as dead. Keep them (they're written by the
    // shared initializers) and silence the lint only when jit is off.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_call_env: EnvRoot,
    /// Native-to-native call recursion depth — bounds the native stack (which `MAX_BC_FRAMES`
    /// doesn't), draining deeper recursion onto the VM instead of overflowing.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_native_depth: u32,
    /// **Absolute stack address below which a JIT'd arm must not run** (KI-14). Every
    /// compiled arm's prologue loads this and deopts to the VM if its own frame sits
    /// below it, so deep recursion drains into the bounded heap-frame loop instead of
    /// running the native stack into its guard page — an abort `try`/`catch` cannot see
    /// and no supervisor can restart.
    ///
    /// The pre-existing guards (`jit_native_depth` + the `stacker` headroom probe) sit on
    /// the *dispatch* paths, so they only bound recursion that goes through a fast link.
    /// A JSON parse 100 000 levels deep proved a path that reaches none of them: on the
    /// root thread the depth cap fired at 1500, while in a spawned green process the probe
    /// was never even called and the worker died. Checking in the prologue is the one place
    /// every native frame must pass, whatever route created it.
    ///
    /// Written by the three native entry points (`jit_tier`, `jit_run_fast_link`, the
    /// i64-worker wrapper) from the *live* remaining stack, so it is correct on the root
    /// thread and on any worker regardless of their differing stack bases. `0` disables
    /// the check (the probe couldn't read the stack — fail open, as the old code did).
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_stack_limit: usize,
    /// Set while draining an over-deep native-recursion subtree on the VM ([`jit_tier`]
    /// reads it and declines to run native, keeping the recursion in the bounded heap-frame
    /// loop).
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_force_vm: bool,
    /// Diagnostic only (`BROOD_JIT_VERIFY`/staged-stale): the symbol name of the JIT'd arm
    /// currently executing native code (`u32::MAX` = none/unknown). Set on each native entry
    /// and restored after, so when that arm stages a stale handle for a sub-call the report
    /// can name the *caller* arm (the one holding the stale handle), not just the callee.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_dbg_fn: u32,
    /// An error parked by a JIT runtime callback (the C ABI can't return a `Value` *and* an
    /// error); the arm returns the error outcome and [`vm_run_bc`] takes this to propagate.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_pending_error: Option<crate::error::LispError>,
    /// Suspend-host attribution (see `jit_suspend_feedback`). When THIS process's
    /// `receive` dirty-blocks its worker (the §7.4 carve-out: a native frame between the
    /// body driver and the receive means no state capture, so the worker thread waits on
    /// the condvar), the mailbox records WHICH native activation was innermost-alive:
    /// `blocked_under_gateway = cur_native_gateway`. Each JIT gateway stamps a fresh
    /// `native_gateway_seq` into `cur_native_gateway` around its invoke (save/restore,
    /// like `jit_dbg_fn`), and after the invoke latches its arm `BAILED` iff the recorded
    /// token is ITS OWN — so only the arm that actually enclosed the blocking receive is
    /// latched, never a native that merely ran later in the same quantum (in a gen-server,
    /// that would be the hot post-receive handler). `0` = none. A block with no live JIT
    /// gateway (a Rust-native shape, `map`/`try` callbacks) records 0 and latches nothing.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))] // only JIT gateways bump it
    pub(crate) native_gateway_seq: u64,
    pub(crate) cur_native_gateway: u64,
    pub(crate) blocked_under_gateway: u64,
    /// Overflow sentinel for the unboxed-`i64` fast path (the register calling convention for
    /// int-only recursive arms). That path carries args/results as raw `i64` in registers and
    /// uses overflow-checked arithmetic; on an overflow (or a non-`Int` at the boxed entry) it
    /// sets this and unwinds, and the boxed wrapper deopts to the VM — which recomputes with
    /// BigInt, keeping the JIT bit-identical to the VM. A plain `bool` (per-process heap, only
    /// this process's native code touches it); the JIT loads/stores it through a stable pointer
    /// fetched once at arm entry (`brood_rt_i64_overflow_ptr`).
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_i64_overflow: bool,
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

/// Bump-only allocation: append to the slab, return the new index. The shared
/// shape behind `alloc_pair`, `alloc_vector`, `alloc_map`, `alloc_closure`
/// (and the rest). Indices grow monotonically per process — **no slot is ever
/// reused in place**, which is what makes a stale handle detectable (the
/// epoch tripwire) instead of silently aliasing fresh data. Slab `len()` is
/// bounded not by a free list but by collections relocating survivors into
/// fresh slabs and dropping the old slabs wholesale.
macro_rules! alloc_slot {
    ($self:expr, $field:ident, $value:expr) => {{
        $crate::perf_bump!(alloc);
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

/// The [`RuntimeCode::positions`] key for a RUNTIME pair: its slab index plus its
/// **code generation**. The RUNTIME twin of [`form_pos_key`] (which packs the LOCAL
/// age bit at the same offset), and for the same reason: the two RUNTIME generations
/// share one index space, so a bare index conflates them (see `positions`). Slab
/// indices are bounded by `GEN_SHIFT` bits, so the low 32 bits always suffice.
/// What the two position tables hold for a list form: where it is, which file, and whether
/// the EXPANDER built it (a macro's result, a pattern-binder desugar) rather than the reader.
/// One record for both tables — LOCAL (`ColdHeap::form_pos`) and RUNTIME
/// (`RuntimeCode::positions`) — so `promote` carries the whole fact across, not a subset:
/// the synthetic mark once lived in a side set beside the LOCAL table, and a promoted form
/// silently lost it (the checker then warned on generated `let`s it should have exempted).
#[derive(Clone, Debug)]
pub(crate) struct FormPos {
    pub(crate) pos: crate::error::Pos,
    pub(crate) file: Option<Arc<str>>,
    pub(crate) synthetic: bool,
}

#[inline]
pub(crate) fn rt_pos_key(idx: usize, code_gen: usize) -> u64 {
    debug_assert!(code_gen < 2, "RUNTIME code generation must be 0 or 1");
    (idx as u64) | (((code_gen & 1) as u64) << 32)
}

/// True iff `v` is a LOCAL heap object the copying collector relocates — the set
/// `push_value`/`flush_value` move in place during a LOCAL (nursery/major)
/// collection. Atoms (`Int`, `Sym`, `Pid`, …) and shared-region
/// (`PRELUDE`/`RUNTIME`) handles are never touched by the LOCAL collector.
///
/// **Not** the rooting predicate: a RUNTIME handle is immovable under the LOCAL
/// collector but *is* evacuated by the runtime compactor, so it still needs an
/// operand-stack slot — see [`needs_root_slot`], which [`Heap::root`] uses.
#[inline]
pub fn is_movable(v: Value) -> bool {
    match v.unpack() {
        ValueRef::Pair(id) => id.region() == LOCAL,
        ValueRef::Vector(id) => id.region() == LOCAL,
        ValueRef::Range(id) => id.region() == LOCAL,
        ValueRef::SeqView(id) => id.region() == LOCAL,
        ValueRef::Map(id) => id.region() == LOCAL,
        ValueRef::Set(id) => id.region() == LOCAL,
        ValueRef::Failure(id) => id.region() == LOCAL,
        ValueRef::Str(id) => id.region() == LOCAL,
        ValueRef::BigInt(id) => id.region() == LOCAL,
        ValueRef::Decimal(id) => id.region() == LOCAL,
        ValueRef::Ratio(id) => id.region() == LOCAL,
        ValueRef::Bytes(id) => id.region() == LOCAL,
        ValueRef::Rope(id) => id.region() == LOCAL,
        ValueRef::Fn(id) | ValueRef::Macro(id) => id.region() == LOCAL,
        _ => false,
    }
}

/// True iff a handle to `v` held across a collection safepoint must take an
/// operand-stack slot to be rewritten — because **some** collector relocates it:
/// a LOCAL object (the copying collector moves it) **or** a RUNTIME object (the
/// runtime compactor [`Heap::runtime_collect`] evacuates the shared code region,
/// ADR-076). Only atoms and the immutable PRELUDE region are truly fixed and may
/// stay inline as a [`Root::Stable`].
///
/// This is the superset [`Heap::root`] gates on; [`is_movable`] is the narrower
/// LOCAL-only set. The distinction matters because a RUNTIME constant held inline
/// (e.g. a `let` body or a `do` spine cursor in hot-reloaded/REPL code) would be
/// invisible to `runtime_collect`'s root rewrite and go stale across a
/// compaction — the slab-OOB / silent-corruption class in `docs/known-issues.md`.
#[inline]
pub fn needs_root_slot(v: Value) -> bool {
    let shared = |r| r == LOCAL || r == RUNTIME;
    match v.unpack() {
        ValueRef::Pair(id) => shared(id.region()),
        ValueRef::Vector(id) => shared(id.region()),
        ValueRef::Range(id) => shared(id.region()),
        ValueRef::SeqView(id) => shared(id.region()),
        ValueRef::Map(id) => shared(id.region()),
        ValueRef::Set(id) => shared(id.region()),
        ValueRef::Failure(id) => shared(id.region()),
        ValueRef::Str(id) => shared(id.region()),
        ValueRef::BigInt(id) => shared(id.region()),
        ValueRef::Decimal(id) => shared(id.region()),
        ValueRef::Ratio(id) => shared(id.region()),
        ValueRef::Bytes(id) => shared(id.region()),
        ValueRef::Rope(id) => shared(id.region()),
        ValueRef::Fn(id) | ValueRef::Macro(id) => shared(id.region()),
        _ => false,
    }
}

/// A rooted value handle from [`Heap::root`]: either a truly-fixed value kept
/// inline (no operand-stack slot) or the index of a slot a collector rewrites.
/// Read back with [`Heap::read_root`] after any potential collection. Running
/// prelude code or handling atoms pays no `Vec` churn; LOCAL handles **and**
/// RUNTIME handles (which `runtime_collect` evacuates) take a slot — see
/// [`needs_root_slot`].
#[derive(Clone, Copy)]
pub enum Root {
    /// A truly-fixed value (atom or `PRELUDE` handle); the inline copy stays
    /// valid across any collection. RUNTIME handles do **not** use this — they
    /// take a `Slot`, since the runtime compactor relocates them.
    Stable(Value),
    /// A relocatable value (LOCAL or RUNTIME) parked at this operand-root-stack
    /// index; rewritten in place by whichever collector moves it.
    Slot(usize),
}

/// The [`EnvId`] counterpart of [`Root`] — see [`Heap::root_env`]. The
/// [`EnvId::GLOBAL`] sentinel and immutable PRELUDE frames stay inline; a LOCAL
/// **or** RUNTIME frame takes a slot (the latter is evacuated by the runtime
/// compactor, so it must be rewritten there).
///
/// `#[repr(C, u8)]` because the JIT's inline fast-frame path (§7.5,
/// `BROOD_XCALL=1`) saves/restores [`Heap::jit_call_env`] as two opaque words
/// and constructs `Stable(EnvId::GLOBAL)` by storing `(0, u64::MAX)` — the
/// defined layout (tag `u8` at 0, payload at 8, 16 bytes total) is what makes
/// those stores a valid value. Pinned by `env_root_layout_is_pinned`.
#[derive(Clone, Copy)]
#[repr(C, u8)]
pub enum EnvRoot {
    Stable(EnvId),
    Slot(usize),
}

/// Byte offsets of the `Heap` fields the JIT's inline fast-frame path (§7.5,
/// `BROOD_XCALL=1`) reads and writes directly from emitted code — the call ceremony
/// `jit_run_fast_link` otherwise performs in Rust. Computed with `offset_of!` at lower
/// time inside the same binary that runs the emitted code, so they are exact by
/// construction; a struct reorder recomputes them on the next build.
#[cfg(feature = "jit")]
#[derive(Clone, Copy)]
pub(crate) struct JitCeremonyOffsets {
    /// `Heap.roots` — a [`roots_buf::RootsBuf`], whose own header is (ptr, len, cap)
    /// at +0/+8/+16 (pinned by `RootsBuf::header_offsets`).
    pub roots: usize,
    pub jit_call_env: usize,
    pub jit_native_depth: usize,
    pub jit_force_vm: usize,
    pub jit_dbg_fn: usize,
    /// `Cell<u32>` — `repr(transparent)`, so a raw u32 load/store at the offset is the
    /// same access `set_ic_bases` makes.
    pub cur_ic_base: usize,
    pub cur_gic_base: usize,
    pub native_gateway_seq: usize,
    pub cur_native_gateway: usize,
    pub blocked_under_gateway: usize,
}

#[cfg(feature = "jit")]
pub(crate) fn jit_ceremony_offsets() -> JitCeremonyOffsets {
    // The two-word EnvRoot save/restore (and the (0, MAX) = Stable(GLOBAL) store) needs
    // exactly this size; a grown EnvRoot must revisit the emission.
    const { assert!(std::mem::size_of::<EnvRoot>() == 16) };
    JitCeremonyOffsets {
        roots: std::mem::offset_of!(Heap, roots),
        jit_call_env: std::mem::offset_of!(Heap, jit_call_env),
        jit_native_depth: std::mem::offset_of!(Heap, jit_native_depth),
        jit_force_vm: std::mem::offset_of!(Heap, jit_force_vm),
        jit_dbg_fn: std::mem::offset_of!(Heap, jit_dbg_fn),
        cur_ic_base: std::mem::offset_of!(Heap, cur_ic_base),
        cur_gic_base: std::mem::offset_of!(Heap, cur_gic_base),
        native_gateway_seq: std::mem::offset_of!(Heap, native_gateway_seq),
        cur_native_gateway: std::mem::offset_of!(Heap, cur_native_gateway),
        blocked_under_gateway: std::mem::offset_of!(Heap, blocked_under_gateway),
    }
}

#[cfg(all(test, feature = "jit"))]
mod env_root_layout_tests {
    use super::*;

    #[test]
    fn env_root_layout_is_pinned() {
        // The inline path stores (0u64, u64::MAX) for Stable(GLOBAL): tag byte 0 selects
        // the first variant under repr(C, u8), the payload word sits at +8.
        assert_eq!(std::mem::size_of::<EnvRoot>(), 16);
        let words = [0u64, u64::MAX];
        // SAFETY: fully-initialized bytes forming a valid repr(C, u8) value (tag 0 =
        // Stable, payload = EnvId(u64::MAX) = GLOBAL).
        let er: EnvRoot = unsafe { std::mem::transmute::<[u64; 2], EnvRoot>(words) };
        assert!(matches!(er, EnvRoot::Stable(e) if e.0 == u64::MAX));
        let slot = [1u64, 42u64];
        // SAFETY: tag 1 = Slot, payload 42.
        let er2: EnvRoot = unsafe { std::mem::transmute::<[u64; 2], EnvRoot>(slot) };
        assert!(matches!(er2, EnvRoot::Slot(42)));
    }
}

// ===== Construction and shared-region management ================================

mod env_globals;
mod equality;
/// Side facts — what a definition RECORDS about a name rather than binds to it (ADR-320).
mod facts;
pub use facts::{Fact, FactKind};
mod freeze;
mod gc;
mod gc_runtime;
/// The LOCAL string representation: the slab entry, its cached char count, and the
/// side tables that keep char↔byte conversion linear.
mod local_string;
mod map_ops;
mod positions;
mod promote;
mod roots_buf;
/// The slab substrate: `VecStore`, the LOCAL/PRELUDE `Slabs`, the append-only
/// RUNTIME `CodeSlabs`, and the `SlabRef` borrow shim every accessor returns.
mod slabs;
mod vm_cache;
// `stall_guard` is used by the RUNTIME compactor (`gc_runtime`) and the GUI paint
// path, so it's re-exported unconditionally; `stall_guard_pid` by the scheduler.
pub(crate) use self::gc::{stall_guard, stall_guard_pid, stall_threshold_ms};
// The GC tuning knobs live beside the collector; the heap and its other children read
// them when sizing a nursery, a drain stride or a walker's stack.
use self::gc::{
    gc_floor, gc_trace_default, major_floor, rt_gc_floor, DRAIN_REPORT_STRIDE, P1_LARGE_SEED,
    P1_REVALIDATE_STRIDE, P2_REVALIDATE_STRIDE, RT_DRAIN_SCAN_STRIDE, WALKER_RED_ZONE,
    WALKER_STACK_CHUNK,
};
// The live-process gauge is a GC input (it divides `gc_floor` among the live processes),
// but the scheduler is its only writer — so it keeps its `crate::core::heap` path.
pub use self::gc::{live_process_count, live_process_dec, live_process_inc};
use self::local_string::{LocalString, StrData};
use self::slabs::{
    park_trim_probe, shrink_slabs, slab_bytes, slab_capacity_bytes, slab_live_count, CodeSlabs,
    Slabs, PARK_TRIM_GROWTH_SLOTS,
};
pub(crate) use self::slabs::{SlabRef, VecStore, INLINE_VEC_CAP};
pub(crate) use self::vm_cache::{
    CallIcEntry, DispatchIcEntry, FastLink, GlobalIcEntry, VmCacheKey,
};

/// The parked-message slot table ([`Heap::msg_roots`]): the traced slots plus an
/// explicit free list.
///
/// Freeness is tracked **out of band, never by slot content**: `nil` is a legal message
/// value, so a content sentinel cannot work — when `Value::Nil` *was* the tombstone, an
/// L1-delivered `nil` message wrote a slot indistinguishable from a free one, the next
/// delivery reused it, and two queued envelopes then read one slot (the receiver saw the
/// second message where `nil` belonged and `nil` where the second message belonged —
/// silent duplication + loss, guarded by `tests/receive_consume_test.blsp`). The free
/// list also makes [`Heap::msg_root_add`] O(1) instead of an O(live-slots) scan, which
/// mattered because that add runs under the receiver's mailbox lock on every L1 send.
///
/// A freed slot's `Value` is still overwritten to `nil` so the GC (which traces every
/// slot) never keeps a consumed message alive.
#[derive(Default)]
pub struct MsgRoots {
    slots: Vec<Value>,
    free: Vec<u32>,
}

impl Heap {
    /// Park a message value copied into this heap, returning its slot index. Reuses a
    /// freed slot when one is available so a steady request/response process never grows
    /// the table past one entry.
    pub fn msg_root_add(&mut self, v: Value) -> u32 {
        let table = self.msg_roots.get_or_insert_with(Box::default);
        if let Some(i) = table.free.pop() {
            table.slots[i as usize] = v;
            return i;
        }
        table.slots.push(v);
        (table.slots.len() - 1) as u32
    }

    /// Take the value out of slot `i`, freeing it for reuse. Returns `nil` for an
    /// out-of-range index, which cannot happen for an envelope this heap produced.
    pub fn msg_root_take(&mut self, i: u32) -> Value {
        let Some(t) = self.msg_roots.as_mut() else {
            return Value::nil();
        };
        let Some(slot) = t.slots.get_mut(i as usize) else {
            return Value::nil();
        };
        // Clear the slot so the traced table doesn't keep the consumed value alive.
        let v = std::mem::replace(slot, Value::nil());
        debug_assert!(
            !t.free.contains(&i),
            "msg_root_take: slot {i} freed twice — two envelopes aliasing one slot"
        );
        t.free.push(i);
        v
    }

    /// Read slot `i` without clearing it — the peek-in-place scan path, where a
    /// candidate that fails to match must stay queued with its slot intact.
    pub fn msg_root_peek(&self, i: u32) -> Value {
        self.msg_roots
            .as_ref()
            .and_then(|t| t.slots.get(i as usize))
            .copied()
            .unwrap_or(Value::nil())
    }
}

impl Heap {
    /// The cold loader/checker state, if this process has ever needed it. `None` is the
    /// normal case for a worker and means "empty" for every reader.
    #[inline]
    /// Live sizes of the four per-process inline-cache tables, for costing the
    /// green-process floor (`FRONTIER.md` lever 1 puts the IC tables at 896 B/process,
    /// the largest single attributed item). Returns, per table, `(len, capacity, bytes)`
    /// with bytes derived from live CAPACITY and the real element size, so a `Vec` grown
    /// past its contents reports the memory it actually holds. Read by `(%)`.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn ic_table_stats(&self) -> ([(usize, usize, usize); 4], usize) {
        use std::mem::size_of;
        let calls = self.vm_call_ics.borrow();
        let links = self.vm_fast_links.borrow();
        let globals = self.vm_global_ics.borrow();
        let blocks = self.arm_ic_blocks.borrow();
        let call_sz = size_of::<Option<crate::core::heap::vm_cache::CallIcEntry>>();
        let link_sz = size_of::<crate::core::heap::vm_cache::FastLink>();
        let gl_sz = size_of::<Option<crate::core::heap::vm_cache::GlobalIcEntry>>();
        // hashbrown: one flat (K,V) slot array plus a control byte each.
        let blk_sz = size_of::<(u64, (u32, u32))>() + 1;
        (
            [
                (calls.len(), calls.capacity(), calls.capacity() * call_sz),
                (links.len(), links.capacity(), links.capacity() * link_sz),
                (
                    globals.len(),
                    globals.capacity(),
                    globals.capacity() * gl_sz,
                ),
                (blocks.len(), blocks.capacity(), blocks.capacity() * blk_sz),
            ],
            call_sz,
        )
    }

    /// Reason id recorded by the JIT at its most recent type-deopt, written by
    /// `brood_rt_note_deopt` from the (cold) shared deopt block. `0` = none yet.
    ///
    /// Exists because a deopt previously told you only *where it resumed* — the last
    /// checkpoint — and an arm can have a dozen guards after that point. KI-49 sat at
    /// "one of five guards" for exactly this reason.
    #[cfg(feature = "jit")]
    pub(crate) fn note_jit_deopt_reason(&self, reason: u32) {
        self.jit_deopt_reason.set(reason);
    }

    /// The reason id from the most recent JIT type-deopt (see `note_jit_deopt_reason`).
    #[cfg(feature = "jit")]
    pub(crate) fn jit_deopt_reason(&self) -> u32 {
        self.jit_deopt_reason.get()
    }

    /// Entry counts of the two source-position side tables — the LOCAL
    /// [`Heap::form_pos`] map and the shared RUNTIME [`RuntimeCode::positions`] map.
    /// Measurement surface for the position-table cost (they were 169 MB of a 933 MB
    /// 1000-module load, and 24% of load time, on 2026-08-06); read by `(%)`.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn pos_table_stats(&self) -> (usize, usize, usize, usize) {
        let (local, local_cap) = self
            .cold()
            .map_or((0, 0), |c| (c.form_pos.len(), c.form_pos.capacity()));
        let rt = self
            .runtime
            .positions
            .read()
            .unwrap_or_else(|e| e.into_inner());
        (local, local_cap, rt.len(), rt.capacity())
    }

    /// Bytes the two position tables' *own* storage occupies, derived from live
    /// capacity rather than guessed: hashbrown lays out `(K, V)` slots in one flat
    /// array with a control byte each, so a table costs `capacity * (size_of::<(K,V)>() + 1)`.
    /// Excludes the `Arc<str>` filenames the values point at (shared, counted once).
    #[cfg(feature = "dev-tools")]
    pub(crate) fn pos_table_bytes(&self) -> (usize, usize) {
        let (_, local_cap, _, rt_cap) = self.pos_table_stats();
        let local_slot = std::mem::size_of::<(u64, (crate::error::Pos, Option<Arc<str>>))>() + 1;
        let rt_slot = std::mem::size_of::<(u64, (crate::error::Pos, Option<Arc<str>>))>() + 1;
        (local_cap * local_slot, rt_cap * rt_slot)
    }

    fn cold(&self) -> Option<&ColdHeap> {
        self.cold.as_deref()
    }

    /// The cold state, allocating it on first use. Only write paths call this — a module
    /// load, a `defmodule` compile, or a checker run.
    #[inline]
    fn cold_mut(&mut self) -> &mut ColdHeap {
        self.cold.get_or_insert_with(Default::default)
    }
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
            old: None,
            gen_cache: [RefCell::new(None), RefCell::new(None)],
            gen_cache_ver: [Cell::new(u64::MAX), Cell::new(u64::MAX)],
            closure_tpl_cache: RefCell::new(ClosureTemplateMap::default()),
            closure_tpl_ver: Cell::new(u64::MAX),
            closure_const_cache: RefCell::new(ConstClosureMap::default()),
            closure_const_ver: Cell::new(u64::MAX),
            prelude: Arc::default(),
            runtime: Arc::default(),
            global: EnvId::local(0),
            dynamics: Vec::new(),
            #[cfg(feature = "dev-tools")]
            trace_context: None,
            #[cfg(feature = "dev-tools")]
            trace_context_own: false,
            global_ic: RefCell::new(SymbolMap::default()),
            rooted_ref_ic: RefCell::new(SymbolMap::default()),
            msg_roots: None,
            cold: None,
            check: RefCell::new(None),
            roots: roots_buf::RootsBuf::new(),
            env_roots: Vec::new(),
            gc_threshold: usize::MAX,
            park_trim_mark: 0,
            rt_gc_threshold: usize::MAX,
            rt_gc_floor_override: None,
            gc_enabled: false,
            rt_collect_block: std::cell::Cell::new(0),
            local_epoch: 0,
            remembered: Vec::new(),
            old_epoch: 0,
            major_threshold: usize::MAX,
            gc_runs: 0,
            gc_copied: 0,
            gc_reclaimed: 0,
            gc_ns_total: 0,
            gc_ns_max: 0,
            gc_ns_last: 0,
            proc_mem_limit: None,
            proc_limit_hit: None,
            proc_send_errors: false,
            gc_trace: gc_trace_default(),
            vm_cache: RefCell::new(VmCacheMap::default()),
            seen_free_epoch: Cell::new(0),
            rt_drain_tick: Cell::new(0),
            acked_drain_epoch: Cell::new(0),
            drain_report_tick: Cell::new(0),
            p2_dirty_epoch: Cell::new(u64::MAX),
            p2_dirty_tick: Cell::new(0),
            p1_dirty_epoch: Cell::new(u64::MAX),
            p1_dirty_tick: Cell::new(0),
            recv_mark: Cell::new((0, 0)),
            live_vm_arms: Vec::new(),
            jit_deopt_reason: std::cell::Cell::new(0),
            vm_call_ics: RefCell::new(Vec::new()),
            next_ic_base: std::cell::Cell::new(0),
            next_gic_base: std::cell::Cell::new(0),
            tw_reentry_depth: std::cell::Cell::new(0),
            vm_fast_links: RefCell::new(Vec::new()),
            #[cfg(debug_assertions)]
            dbg_site_pos: RefCell::new(Vec::new()),
            vm_global_ics: RefCell::new(Vec::new()),
            arm_ic_blocks: RefCell::new(std::collections::HashMap::new()),
            cur_ic_base: Cell::new(0),
            cur_gic_base: Cell::new(0),
            dispatch_ics: RefCell::new(HashMap::default()),
            jit_call_env: EnvRoot::Stable(EnvId::GLOBAL),
            jit_native_depth: 0,
            jit_stack_limit: 0,
            jit_force_vm: false,
            jit_dbg_fn: u32::MAX,
            jit_pending_error: None,
            native_gateway_seq: 0,
            cur_native_gateway: 0,
            blocked_under_gateway: 0,
            jit_i64_overflow: false,
        }
    }

    /// A fresh process heap sharing the given prelude + runtime regions (empty
    /// local slabs). Spawned inner processes pass the *same* `runtime` Arc as
    /// their parent, so they see its global bindings and its later `def`s.
    pub fn with_regions(prelude: Arc<SharedCode>, runtime: Arc<RuntimeCode>) -> Self {
        Heap {
            local: Slabs::default(),
            old: None,
            gen_cache: [RefCell::new(None), RefCell::new(None)],
            gen_cache_ver: [Cell::new(u64::MAX), Cell::new(u64::MAX)],
            closure_tpl_cache: RefCell::new(ClosureTemplateMap::default()),
            closure_tpl_ver: Cell::new(u64::MAX),
            closure_const_cache: RefCell::new(ConstClosureMap::default()),
            closure_const_ver: Cell::new(u64::MAX),
            prelude,
            runtime,
            global: EnvId::local(0),
            dynamics: Vec::new(),
            #[cfg(feature = "dev-tools")]
            trace_context: None,
            #[cfg(feature = "dev-tools")]
            trace_context_own: false,
            global_ic: RefCell::new(SymbolMap::default()),
            rooted_ref_ic: RefCell::new(SymbolMap::default()),
            msg_roots: None,
            cold: None,
            check: RefCell::new(None),
            roots: roots_buf::RootsBuf::new(),
            env_roots: Vec::new(),
            gc_threshold: gc_floor(),
            park_trim_mark: 0,
            rt_gc_threshold: rt_gc_floor(),
            rt_gc_floor_override: None,
            gc_enabled: true,
            rt_collect_block: std::cell::Cell::new(0),
            local_epoch: 0,
            remembered: Vec::new(),
            old_epoch: 0,
            major_threshold: major_floor(),
            gc_runs: 0,
            gc_copied: 0,
            gc_reclaimed: 0,
            gc_ns_total: 0,
            gc_ns_max: 0,
            gc_ns_last: 0,
            proc_mem_limit: None,
            proc_limit_hit: None,
            proc_send_errors: false,
            gc_trace: gc_trace_default(),
            vm_cache: RefCell::new(VmCacheMap::default()),
            seen_free_epoch: Cell::new(0),
            rt_drain_tick: Cell::new(0),
            acked_drain_epoch: Cell::new(0),
            drain_report_tick: Cell::new(0),
            p2_dirty_epoch: Cell::new(u64::MAX),
            p2_dirty_tick: Cell::new(0),
            p1_dirty_epoch: Cell::new(u64::MAX),
            p1_dirty_tick: Cell::new(0),
            recv_mark: Cell::new((0, 0)),
            live_vm_arms: Vec::new(),
            jit_deopt_reason: std::cell::Cell::new(0),
            vm_call_ics: RefCell::new(Vec::new()),
            next_ic_base: std::cell::Cell::new(0),
            next_gic_base: std::cell::Cell::new(0),
            tw_reentry_depth: std::cell::Cell::new(0),
            vm_fast_links: RefCell::new(Vec::new()),
            #[cfg(debug_assertions)]
            dbg_site_pos: RefCell::new(Vec::new()),
            vm_global_ics: RefCell::new(Vec::new()),
            arm_ic_blocks: RefCell::new(std::collections::HashMap::new()),
            cur_ic_base: Cell::new(0),
            cur_gic_base: Cell::new(0),
            dispatch_ics: RefCell::new(HashMap::default()),
            jit_call_env: EnvRoot::Stable(EnvId::GLOBAL),
            jit_native_depth: 0,
            jit_stack_limit: 0,
            jit_force_vm: false,
            jit_dbg_fn: u32::MAX,
            jit_pending_error: None,
            native_gateway_seq: 0,
            cur_native_gateway: 0,
            blocked_under_gateway: 0,
            jit_i64_overflow: false,
        }
    }

    /// Clone the Arc to this heap's prelude region (for spawning a child).
    pub fn prelude_arc(&self) -> Arc<SharedCode> {
        Arc::clone(&self.prelude)
    }

    /// Is `s` one of the globals the prelude bound at the freeze? See
    /// [`SharedCode::binding_names`].
    pub fn is_prelude_global(&self, s: Symbol) -> bool {
        self.prelude.binding_names.contains(&s)
    }

    /// Clone the Arc to this runtime's shared code region (for spawning a child
    /// that shares this runtime's live globals).
    /// This heap's runtime-instance tag — see [`RuntimeCode::runtime_tag`]. Unconditional
    /// (it was `jit`-only) because the messaging path needs it to decide whether a target
    /// process shares this runtime, and therefore whether a RUNTIME handle may cross to it.
    pub(crate) fn runtime_tag(&self) -> u64 {
        self.runtime.runtime_tag
    }

    pub fn runtime_arc(&self) -> Arc<RuntimeCode> {
        Arc::clone(&self.runtime)
    }

    /// Whether `other` is a heap of the **same runtime** — i.e. the two processes
    /// read one shared RUNTIME code region through the same `Arc`, so a handle into
    /// it is meaningful in both. Pointer comparison, no `Arc` clone.
    ///
    /// This is the precondition for handing a promoted (shared-region) handle from
    /// one process to another instead of deep-copying: `spawn` relies on it
    /// implicitly (parent and child share the region by construction), and the
    /// local-send closure fast path checks it explicitly, because the process
    /// REGISTRY is global — two `Interp`s in one OS process (a test harness, an
    /// embedder) have *different* regions, and a handle must never cross that line.
    /// Record that `ref_id` was minted when this process's mailbox was at arrival
    /// sequence `seq` — see [`Heap::recv_mark`].
    pub fn set_recv_mark(&self, ref_id: u64, seq: u64) {
        self.recv_mark.set((ref_id, seq));
    }

    /// The arrival sequence to start a scan at for a receive pinned on `ref_id`, or
    /// `None` when that ref is not the one we last minted (so we must scan from the front).
    pub fn recv_mark_for(&self, ref_id: u64) -> Option<u64> {
        let (id, seq) = self.recv_mark.get();
        (id == ref_id).then_some(seq)
    }

    pub fn shares_runtime_with(&self, other: &Heap) -> bool {
        Arc::ptr_eq(&self.runtime, &other.runtime)
    }

    // ===== Process global scope =================================================

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
            bigints: self.local.bigints.len(),
            decimals: self.local.decimals.len(),
            ratios: self.local.ratios.len(),
            bytes: self.local.bytes.len(),
            ropes: self.local.ropes.len(),
            closures: self.local.closures.len(),
            envs: self.local.envs.len(),
            epoch: self.local_epoch,
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
    ///
    /// **Collection-safety.** If a collection fired between [`checkpoint`] and
    /// here, it already compacted the nursery (a flip rewrote the slabs; a tenure
    /// emptied them) and bumped [`local_epoch`](Self::local_epoch), so `cp`'s slab
    /// lengths no longer describe the live nursery. Truncating to them would
    /// **strand the survivors the collector just kept** (the demonstrated GC
    /// slab-OOB crash: a wide-bignum eval forced a flip, then the stale-length
    /// truncate cut live objects loose). On an epoch mismatch this is a no-op — the
    /// collection has already reclaimed the dead, and the next `gc_due` reclaims
    /// this form's now-garbage survivors. Only the no-collection fast path (epoch
    /// unchanged: a pure bump-allocated region) actually truncates.
    pub fn reset_local_to(&mut self, cp: LocalCheckpoint) {
        if self.local_epoch != cp.epoch {
            return;
        }
        self.local.pairs.truncate(cp.pairs);
        self.local.vectors.truncate(cp.vectors);
        self.local.maps.truncate(cp.maps);
        self.local.strings.truncate(cp.strings);
        self.local.bigints.truncate(cp.bigints);
        self.local.decimals.truncate(cp.decimals);
        self.local.ratios.truncate(cp.ratios);
        self.local.bytes.truncate(cp.bytes);
        self.local.ropes.truncate(cp.ropes);
        self.local.closures.truncate(cp.closures);
        self.local.envs.truncate(cp.envs);
        // Drop position metadata for the pairs just reclaimed (indices reused).
        // Keys pack the age bit at bit 32; this checkpoint path is nursery-only,
        // so compare the low-32 slab index against the checkpoint length.
        if self.cold.as_ref().is_some_and(|c| !c.form_pos.is_empty()) {
            self.cold_mut()
                .form_pos
                .retain(|&k, _| (k as u32 as usize) < cp.pairs);
        }
        // The threshold is relative to live count; reclamation here is so cheap
        // that we let the next `gc_due` check recompute against the smaller heap.
    }

    // ===== Runtime-GC floor knobs ===============================================

    /// The runtime-region GC floor this heap uses: its own override, else the process-wide
    /// [`rt_gc_floor`].
    pub(crate) fn floor(&self) -> usize {
        self.rt_gc_floor_override.unwrap_or_else(rt_gc_floor)
    }

    /// Set THIS heap's runtime-GC floor — for a test that needs a compaction to trip on a
    /// small churn. Takes effect immediately: the threshold becomes the floor (as it is at
    /// construction), and every later re-arm reads the override. Replaces setting
    /// `BROOD_RT_GC_FLOOR` in a test, which — being read once per process — leaked into
    /// every test scheduled after it (KI-86).
    pub fn set_rt_gc_floor(&mut self, floor: usize) {
        self.rt_gc_floor_override = Some(floor);
        if self.rt_gc_threshold != usize::MAX {
            self.rt_gc_threshold = floor;
        }
    }

    // ===== Allocation — LOCAL slab =============================================
    //
    // Every allocator bump-appends to its LOCAL slab (the [`alloc_slot!`]
    // macro is that shape in one place; `alloc_string` / `new_env` stay
    // hand-written for their extra bookkeeping). Slots are never reused in
    // place; the slab's `len()` is kept bounded by the copying collector
    // relocating survivors into fresh slabs and dropping the rest.

    pub fn alloc_pair(&mut self, head: Value, tail: Value) -> Value {
        let idx = alloc_slot!(self, pairs, (head, tail));
        Value::pair(PairId::local_gen(idx, self.local_epoch))
    }

    pub fn alloc_vector(&mut self, items: Vec<Value>) -> Value {
        let idx = alloc_slot!(self, vectors, VecStore::from_vec(items));
        Value::vector(VecId::local_gen(idx, self.local_epoch))
    }

    /// Allocate a 2-element vector directly from its elements — a bump-push of an
    /// inline [`VecStore`] with **no temporary `Vec`** (hence no `malloc`). The
    /// JIT's `MakeVector(2)` runtime helper (`brood_rt_make_vector2`) uses this
    /// so the overwhelmingly common 2-tuple allocation (e.g. every `bintree`
    /// node) is as cheap as a `cons`. Larger literals still go through
    /// [`alloc_vector`].
    pub fn alloc_vector2(&mut self, a: Value, b: Value) -> Value {
        let idx = alloc_slot!(
            self,
            vectors,
            VecStore::Inline {
                len: 2,
                items: [a, b],
            }
        );
        Value::vector(VecId::local_gen(idx, self.local_epoch))
    }

    /// Allocate a 2-element vector whose elements the **caller** writes, in place: returns
    /// the fresh handle and a pointer to the slot's `items`.
    ///
    /// The counterpart of [`alloc_vector2`](Self::alloc_vector2) for JIT-compiled code, and
    /// the reason it exists is the ABI. `brood_rt_make_vector2` took the two elements as
    /// **six `i64` words**; SysV passes six arguments in registers and this call also needs
    /// `heap` and `out`, so four words spilled to the caller's stack and the callee loaded
    /// them straight back. Those two loads were **66% of `brood_rt_make_vector2`**, itself
    /// 7.9% of `bintree` — the same store-to-load-across-a-call shape as the return value in
    /// §2h. Handing back the destination lets the arm's own stores land in the slab.
    ///
    /// # Safety
    /// The returned slot's elements are **live heap data holding whatever `Value::Nil`
    /// leaves there** until the caller writes both. Nothing may allocate or collect in that
    /// window. `Nil` rather than uninitialised memory deliberately: this slot is reachable
    /// from the returned handle, so a missed store must degrade to a wrong *value* the tests
    /// can catch, never to a garbage word the GC would trace.
    #[cfg(feature = "jit")]
    pub(crate) fn alloc_vector2_room(&mut self) -> (Value, *mut Value) {
        let idx = alloc_slot!(
            self,
            vectors,
            VecStore::Inline {
                len: 2,
                items: [Value::nil(), Value::nil()],
            }
        );
        let ptr = match &mut self.local.vectors[idx] {
            VecStore::Inline { items, .. } => items.as_mut_ptr(),
            VecStore::Spill { .. } => unreachable!("just pushed an Inline"),
        };
        (Value::vector(VecId::local_gen(idx, self.local_epoch)), ptr)
    }

    /// Allocate a lazy integer range `lo..hi` by `step`. Returns `Nil` when the
    /// range is empty (so a `Value::Range` always has ≥1 element), otherwise a
    /// `Value::Range` backed by a 3-element `[lo hi step]` vector. `step` must be
    /// non-zero (the caller — `%range` — enforces it).
    pub fn alloc_range(&mut self, lo: i64, hi: i64, step: i64) -> Value {
        let empty = if step > 0 { lo >= hi } else { hi >= lo };
        if empty {
            return Value::nil();
        }
        let idx = alloc_slot!(
            self,
            vectors,
            VecStore::from_vec(vec![Value::int(lo), Value::int(hi), Value::int(step)])
        );
        Value::range(VecId::local_gen(idx, self.local_epoch))
    }

    /// Allocate a lazy **seq-view** backed by a 2-element `[source xform]`
    /// vector. `source` is the underlying collection, `xform` a transducer
    /// composing every pending `map`/`filter`/`keep`/`remove` stage. Rides the
    /// vector slab exactly like [`Heap::alloc_range`], but its backing holds heap
    /// values (not just ints), so GC promote/flush/verify recurse into them.
    pub fn alloc_seqview(&mut self, source: Value, xform: Value) -> Value {
        let idx = alloc_slot!(self, vectors, VecStore::from_vec(vec![source, xform]));
        Value::seqview(VecId::local_gen(idx, self.local_epoch))
    }

    /// The `(source, xform)` of a seq-view handle's backing `[source xform]`
    /// vector.
    pub fn seqview_parts(&self, id: VecId) -> (Value, Value) {
        let v = self.vector(id);
        (v[0], v[1])
    }

    /// The `(lo, hi, step)` of a range handle's backing `[lo hi step]` vector.
    pub fn range_parts(&self, id: VecId) -> (i64, i64, i64) {
        let v = self.vector(id);
        let int = |x: Value| x.as_int().unwrap_or(0);
        (int(v[0]), int(v[1]), int(v[2]))
    }

    /// The number of elements a range yields. O(1).
    pub fn range_len(&self, id: VecId) -> i64 {
        let (lo, hi, step) = self.range_parts(id);
        // step is non-zero and the range is non-empty by construction. Compute in
        // i128: a wide range (e.g. i64::MIN..i64::MAX) overflows an i64 span even
        // though its element count is meaningful. Saturate on the way back — a range
        // longer than i64::MAX can't be materialised anyway.
        let (lo, hi, step) = (lo as i128, hi as i128, step as i128);
        let span = if step > 0 { hi - lo } else { lo - hi };
        let mag = step.abs();
        (((span + mag - 1) / mag).min(i64::MAX as i128)) as i64
    }

    /// Materialise a range's elements into a `Vec<Value>` of `Int`s — the slow
    /// path behind realising a range to a list / vector.
    ///
    /// **Fallible**, because a range is lazy and its element count is unbounded: `(range
    /// 0 9223372036854775807)` is a legal O(1) value, and realising it is not. It used to
    /// pre-size with the saturated [`range_len`], so `Vec::with_capacity(i64::MAX)`
    /// exceeded `isize::MAX` and hit the `capacity overflow` **panic** — instantly, with
    /// no large allocation, and un-catchable by Brood's `try`/`catch`, so it killed the
    /// worker rather than failing the expression. Reachable straight from
    /// `(seq …)`/`(reverse …)`/`(nth …)` on a wide range. Capping the reservation alone
    /// would only trade the panic for a slower allocator abort, so anything past
    /// [`MAX_REALISED_RANGE`] is refused as a clean, catchable error instead.
    pub fn range_to_vec(&self, id: VecId) -> Result<Vec<Value>, LispError> {
        let (lo, hi, step) = self.range_parts(id);
        let n = self.range_len(id).max(0);
        if n > MAX_REALISED_RANGE {
            return Err(LispError::runtime(format!(
                "range too large to realise: {n} elements (limit {MAX_REALISED_RANGE})"
            ))
            .with_hint(
                "a range is lazy — consume it with `take`/`fold`/`map` instead of \
                 realising it with `seq`/`reverse`/`vec`",
            ));
        }
        let mut out = Vec::with_capacity(n as usize);
        let mut i = lo;
        while if step > 0 { i < hi } else { i > hi } {
            out.push(Value::int(i));
            // The final step near i64::MIN/MAX would overflow; the loop is done anyway.
            i = match i.checked_add(step) {
                Some(v) => v,
                None => break,
            };
        }
        Ok(out)
    }

    // ----- access (dispatch on the handle's region) -----

    /// A heap epoch counter truncated to the handle GEN field's width. A
    /// handle's `generation()` is its mint-time epoch masked to `GEN_MASK`,
    /// while the heap's `local_epoch`/`old_epoch` counters are full u32s — so
    /// every stale-handle comparison must truncate the expected side
    /// identically, or after 2^29 collections of one heap every *valid*
    /// handle would "mismatch" (kernel audit; astronomically rare, but the
    /// tripwire must not be the thing that cries wolf). The one definition
    /// shared by [`check_epoch_aged`](Self::check_epoch_aged) and the
    /// `BROOD_GC_VERIFY` walker, so the two detectors can't drift.
    // Available in release too: `dbg_value_stale` (used by the runtime BROOD_JIT_VERIFY
    // staged-stale scan) calls it. Pure arithmetic — zero cost unless called.
    #[allow(dead_code)]
    fn epoch_in_gen_width(epoch: u32) -> u32 {
        epoch & (crate::core::value::GEN_MASK as u32)
    }

    /// Generation-aware epoch tripwire. Young (`is_old == false`) handles are
    /// checked against the nursery epoch (bumped by every collection); old handles
    /// against the old-generation epoch (bumped only by a major collection, since a
    /// minor leaves old objects in place). Both sides compare truncated — see
    /// [`epoch_in_gen_width`]. A mismatch means a handle was held
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
        // Compare in the handle's truncated GEN width — see `epoch_in_gen_width`.
        let expected = Self::epoch_in_gen_width(expected);
        debug_assert!(
            gen == expected,
            "use-after-GC: {} handle ({} slot {}) is from epoch {}, but that generation is \
             now epoch {} — a handle held across a collection without being re-rooted \
             (handle {:#x}). [current JIT arm: '{}']",
            what,
            space,
            index,
            gen,
            expected,
            raw,
            crate::core::value::symbol_name_opt(self.jit_dbg_fn).unwrap_or("<none/computed>"),
        );
    }

    /// Is `v` a LOCAL handle whose generation epoch no longer matches the live epoch
    /// (stale across a collection)? Non-panicking sibling of the per-deref tripwire, for
    /// scanning staged call args. Returns `Some((kind, handle_gen, live_gen))` if stale.
    /// Available in release (gated only by the caller) so the runtime `BROOD_JIT_VERIFY`
    /// scan can run without a debug-assertions build.
    pub fn dbg_value_stale(&self, v: Value) -> Option<(&'static str, u32, u32)> {
        let (name, region, is_old, gen) = match v {
            Value::Pair(id) => ("pair", id.region(), id.is_old(), id.generation()),
            Value::Vector(id) => ("vector", id.region(), id.is_old(), id.generation()),
            Value::Map(id) => ("map", id.region(), id.is_old(), id.generation()),
            Value::Set(id) => ("set", id.region(), id.is_old(), id.generation()),
            Value::Failure(id) => ("failure", id.region(), id.is_old(), id.generation()),
            Value::Str(id) => ("string", id.region(), id.is_old(), id.generation()),
            Value::Rope(id) => ("rope", id.region(), id.is_old(), id.generation()),
            _ => return None,
        };
        if region != LOCAL {
            return None;
        }
        let expected = Self::epoch_in_gen_width(if is_old {
            self.old_epoch
        } else {
            self.local_epoch
        });
        if gen != expected {
            Some((name, gen, expected))
        } else {
            None
        }
    }

    /// Is `v` a handle whose slab index is **out of bounds** for its region's slab — i.e.
    /// garbage read from a freed/wrong location (a recycled roots buffer, an unspilled
    /// register that went stale across a collection)? Catches bug-#2 garbage that
    /// `dbg_value_stale` misses (the garbage's region/epoch bits don't read as a clean
    /// LOCAL-stale handle). Returns `Some((kind, index, slab_len))` if OOB.
    pub fn dbg_value_oob(&self, v: Value) -> Option<(&'static str, usize, usize)> {
        macro_rules! check {
            ($id:expr, $name:expr, $field:ident) => {{
                let id = $id;
                let idx = id.index();
                // Only LOCAL (nursery/old) — the bug-#2 garbage is young/local; PRELUDE/RUNTIME
                // are stable boxcar slabs (different len API), skip.
                let len = match id.region() {
                    LOCAL if id.is_old() => self.old_opt().map_or(0, |o| o.$field.len()),
                    LOCAL => self.local.$field.len(),
                    _ => return None,
                };
                if idx >= len {
                    return Some(($name, idx, len));
                }
            }};
        }
        match v {
            Value::Pair(id) => check!(id, "pair", pairs),
            Value::Vector(id) | Value::Range(id) => check!(id, "vector", vectors),
            Value::Map(id) | Value::Set(id) | Value::Failure(id) => check!(id, "map", maps),
            Value::Str(id) => check!(id, "string", strings),
            _ => {}
        }
        None
    }

    // ===== Accessors — read LOCAL/PRELUDE/RUNTIME values =======================

    /// Pin RUNTIME generation `g`'s `Arc<CodeSlabs>` for a read, via the per-process
    /// version-gated cache ([`gen_cache`](Self::gen_cache)). Returns a cheap `Arc` clone
    /// (one refcount bump) when the generation's identity is unchanged since this process
    /// last read it, and `load_full`s only on a real replacement — a Stage-4 free or a
    /// compaction store, both rare and both bumping [`RuntimeCode::gen_version`]. This
    /// replaces the per-deref `ArcSwap::load` guard whose hybrid-strategy cost dominated
    /// global-data-heavy hot loops (a `def`'d matrix element read in `matmul` derefs a
    /// RUNTIME handle ~16 M times). Soundness: the returned `Arc` pins the slab exactly as
    /// the old guard did, so a concurrent free can't drop it mid-read; and reading a stale
    /// cached `Arc` is impossible to observe wrongly — a generation is freed only once every
    /// process (this one included) has reported clean of it (ADR-091), so this process holds
    /// no live handle into a generation whose `Arc` it might still have cached.
    /// Run `f` against RUNTIME generation `g`'s slabs **without bumping its refcount**.
    ///
    /// [`code_gen_pinned`](Self::code_gen_pinned) returns an owned `Arc` so a caller can hold
    /// the generation alive across a borrow — necessary when handing out a reference, but pure
    /// overhead for a read that copies its value straight out. The clone and its matching drop
    /// are two atomic RMWs on a path that runs once per element of every `def`'d structure,
    /// and they dominated it: measured 2026-07-28, `first` on a RUNTIME pair cost **77 ns**
    /// against **1 ns** for the identical code on a LOCAL one — a 70x cliff that every global
    /// data structure fell off (`sort` walks a 375k-element `def`'d list; `matmul` derefs a
    /// `def`'d matrix ~16 M times).
    ///
    /// Soundness is unchanged: the cache still owns the `Arc`, so the generation cannot be
    /// freed while `f` borrows it. `f` must not itself take a *mutable* borrow of this same
    /// generation's cache slot — every caller is a trivial copy-out read, which cannot.
    #[inline]
    fn with_code_gen<R>(&self, g: usize, f: impl FnOnce(&CodeSlabs) -> R) -> R {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.gen_cache_ver[g].get() != ver {
            *self.gen_cache[g].borrow_mut() = Some(self.runtime.gens[g].load_full());
            self.gen_cache_ver[g].set(ver);
        }
        let cached = self.gen_cache[g].borrow();
        f(cached
            .as_ref()
            .expect("gen cache populated on the version miss above"))
    }

    #[inline]
    fn code_gen_pinned(&self, g: usize) -> Arc<CodeSlabs> {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.gen_cache_ver[g].get() != ver {
            // First read, or generation `g`'s `Arc` was replaced — reload and re-stamp.
            *self.gen_cache[g].borrow_mut() = Some(self.runtime.gens[g].load_full());
            self.gen_cache_ver[g].set(ver);
        }
        Arc::clone(
            self.gen_cache[g]
                .borrow()
                .as_ref()
                .expect("gen cache populated on the version miss above"),
        )
    }

    /// Look up the parsed [`ClosureTemplate`] for a `MakeClosure` site's `fn_rest`
    /// handle, gen-synced exactly like [`code_gen_pinned`](Self::code_gen_pinned): a
    /// `gen_version` bump (the only event that relocates the RUNTIME AST handles the
    /// arms carry) clears the whole cache first, so any hit is current-generation.
    /// `None` on a miss — the caller parses once and calls [`store_closure_template`].
    /// Returns the memoised template **and how many times this key has now been seen**
    /// (this lookup included, first parse not counted — so the first hit reports 2).
    /// The count gates [`make_closure_cached`]'s const-promote: promoting into the
    /// append-only RUNTIME region on the *second* sighting meant every spawned process
    /// that evaluated a `receive` twice promoted its matcher closure into shared code —
    /// one region entry **per process**, the exact per-operation growth
    /// `docs/runtime-frontier.md` A3 measures at 541 MB per 800k ops and rejects. A
    /// mass-spawned worker sees its matcher a handful of times; a genuinely hot literal
    /// loop crosses any small threshold in microseconds — so the count separates them
    /// where "seen before" could not.
    ///
    /// [`make_closure_cached`]: crate::eval::make_closure_cached
    pub(crate) fn lookup_closure_template(
        &self,
        key: PairId,
    ) -> Option<(Arc<ClosureTemplate>, u32)> {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.closure_tpl_ver.get() != ver {
            self.closure_tpl_cache.borrow_mut().clear();
            self.closure_tpl_ver.set(ver);
            return None;
        }
        let mut cache = self.closure_tpl_cache.borrow_mut();
        cache.get_mut(&key).map(|(tpl, seen)| {
            *seen = seen.saturating_add(1);
            (Arc::clone(tpl), *seen)
        })
    }

    /// Memoise a freshly-parsed [`ClosureTemplate`] under its `fn_rest` key. Call only
    /// right after a [`lookup_closure_template`] miss (which synced the version this
    /// creation), so the insert lands against the current generation.
    pub(crate) fn store_closure_template(&self, key: PairId, tpl: Arc<ClosureTemplate>) {
        self.closure_tpl_cache.borrow_mut().insert(key, (tpl, 1));
    }

    /// Look up the memoised **promoted RUNTIME closure** for a capture-free `(fn …)`
    /// literal's `fn_rest` key (see [`closure_const_cache`](Self::closure_const_cache)),
    /// gen-synced like [`lookup_closure_template`]: a `gen_version` bump clears the map, so
    /// any hit is a current-generation handle. `None` on a miss — the caller builds +
    /// promotes once and calls [`store_const_closure`].
    pub(crate) fn lookup_const_closure(&self, key: PairId) -> Option<Value> {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.closure_const_ver.get() != ver {
            self.closure_const_cache.borrow_mut().clear();
            self.closure_const_ver.set(ver);
            return None;
        }
        self.closure_const_cache.borrow().get(&key).copied()
    }

    /// Memoise a capture-free closure's promoted RUNTIME handle under its `fn_rest` key.
    /// Call only right after a [`lookup_const_closure`] miss (which synced the version), so
    /// the insert lands against the current generation.
    pub(crate) fn store_const_closure(&self, key: PairId, closure: Value) {
        self.closure_const_cache.borrow_mut().insert(key, closure);
    }

    pub fn pair(&self, id: PairId) -> (Value, Value) {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "pair");
                self.old().pairs[id.index()]
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "pair");
                self.local.pairs[id.index()]
            }
            PRELUDE => self.prelude.slabs.pairs[id.index()],
            // Copy-out read: borrow the generation rather than pinning it, so a pair
            // deref costs no atomic refcount traffic (see `with_code_gen`).
            RUNTIME => self.with_code_gen(id.code_gen(), |slabs| {
                *slabs.pairs.get(id.index()).expect("runtime pair handle")
            }),
            _ => unreachable!("invalid handle region"),
        }
    }
    pub fn car(&self, id: PairId) -> Value {
        self.pair(id).0
    }
    pub fn cdr(&self, id: PairId) -> Value {
        self.pair(id).1
    }
    region_ref!(vector, VecId, vectors, [Value], "runtime vector handle");
    region_ref!(map_node, MapId, maps, MapNode, "runtime map node");

    /// Build a guarded [`SlabRef`] into RUNTIME generation `g` by projecting the
    /// generation's [`CodeSlabs`] to the `&T` a hand-written accessor wants. The
    /// [`Guard`] is moved into the `SlabRef`, keeping the generation's slab alive for
    /// the borrow's lifetime — so a concurrent Stage-4 free can't drop it mid-read
    /// (ADR-091). Mirrors the `RUNTIME` arm of [`region_ref!`] for the accessors that
    /// can't use the macro (`OnceLock`/`LocalString`/`Arc` projections).
    #[inline]
    fn rt_slab_ref<T: ?Sized>(
        &self,
        g: usize,
        project: impl FnOnce(&CodeSlabs) -> &T,
    ) -> SlabRef<'_, T> {
        // A generation can be **freed concurrently** by the multi-process collector while
        // other processes run (ADR-091), so the `SlabRef` must pin `gens[g]`'s `Arc<CodeSlabs>`
        // to defer the freed `Arc`'s drop until this borrow ends. The `Arc` comes from the
        // per-process version-gated cache ([`code_gen_pinned`]) — a cheap clone when the
        // generation is unchanged — not a fresh `ArcSwap::load` guard per deref.
        let pin = self.code_gen_pinned(g);
        let ptr = project(&pin) as *const T;
        // SAFETY: `ptr` points into `pin`'s `CodeSlabs` (stable `boxcar` address);
        // the `Arc` moved into the `SlabRef` keeps that slab alive for the borrow.
        unsafe { SlabRef::pinned(pin, ptr) }
    }

    /// Resolve a string handle to a `&str`. Hand-written (not via the
    /// `region_ref!` macro) because LOCAL slots are `LocalString` enum
    /// variants that need a match to extract their bytes, while PRELUDE and
    /// RUNTIME store plain `String` (PRELUDE is inline-extracted at freeze;
    /// RUNTIME is append-only via `boxcar::Vec<String>` for stable refs).
    /// The **char** length of string `id`, and whether it is pure ASCII — both O(1),
    /// read from the count cached at construction (see [`LocalString`]). The pair is
    /// returned together because every caller that converts a char index to a byte
    /// offset needs both, and resolving the slot twice would cost more than the work.
    pub fn str_metrics(&self, id: StrId) -> (usize, bool) {
        self.with_string_slot(id, |e| (e.char_len(), e.is_ascii()))
    }

    /// Byte offset of char `ci` in string `id`, clamped to the string's end — the
    /// conversion every char-indexed string builtin needs before it can touch the UTF-8
    /// bytes. O(1) for ASCII; for non-ASCII a lookup in the slot's sparse char→byte
    /// index plus a walk bounded by one stride (which is what keeps a scan carrying a
    /// rising index linear rather than quadratic — see [`LocalString`]).
    pub fn str_char_to_byte(&self, id: StrId, ci: usize) -> usize {
        self.with_string_slot(id, |e| e.char_to_byte(ci))
    }

    /// Char index of byte offset `b` in string `id` (`b` must be a char boundary) — the
    /// return direction: a byte-level `find`/`match_indices` result converted back to
    /// the char index the language speaks. Same complexities as
    /// [`str_char_to_byte`](Self::str_char_to_byte).
    pub fn str_byte_to_char(&self, id: StrId, b: usize) -> usize {
        self.with_string_slot(id, |e| e.byte_to_char(b))
    }

    /// The higher-layer table cached against string `id`, built by `build` on first use
    /// and shared thereafter (including with the slot's GC copies). The heap does not
    /// interpret it — see [`StrAux::scan`](local_string::StrAux::scan); the caller downcasts to its own type. Callers
    /// that key a cache by string *value* belong here rather than in a map keyed by
    /// handle: a handle is only unique within a GC epoch, while this cell travels with
    /// the bytes it describes.
    pub fn str_scan_table(
        &self,
        id: StrId,
        build: impl FnOnce(&str) -> Arc<dyn std::any::Any + Send + Sync>,
    ) -> Arc<dyn std::any::Any + Send + Sync> {
        self.with_string_slot(id, |e| {
            Arc::clone(e.aux().scan.get_or_init(|| build(e.as_str())))
        })
    }

    /// Resolve a string handle to its slab entry and hand it to `f`. The
    /// region dispatch the string-metric accessors share; separate from
    /// [`string`](Self::string) because these need the `LocalString` itself (its cached
    /// count and char index), not just its bytes.
    fn with_string_slot<R>(&self, id: StrId, f: impl FnOnce(&LocalString) -> R) -> R {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "string");
                f(&self.old().strings[id.index()])
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "string");
                f(&self.local.strings[id.index()])
            }
            PRELUDE => f(&self.prelude.slabs.strings[id.index()]),
            RUNTIME => {
                let c = self
                    .runtime
                    .gens
                    .get(id.code_gen())
                    .expect("runtime string generation")
                    .load();
                f(c.strings.get(id.index()).expect("runtime string handle"))
            }
            _ => unreachable!("invalid handle region"),
        }
    }

    pub fn string(&self, id: StrId) -> SlabRef<'_, str> {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "string");
                SlabRef::direct(self.old().strings[id.index()].as_str())
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "string");
                SlabRef::direct(self.local.strings[id.index()].as_str())
            }
            // PRELUDE's `Slabs::strings` is also `Vec<LocalString>` because
            // it shares the `Slabs` shape, but `freeze_as_shared_code`
            // inline-extracts any `Shared` entries — every prelude slot is
            // `Inline`. `as_str` works either way.
            PRELUDE => SlabRef::direct(self.prelude.slabs.strings[id.index()].as_str()),
            RUNTIME => self.rt_slab_ref(id.code_gen(), |c| {
                c.strings
                    .get(id.index())
                    .expect("runtime string handle")
                    .as_str()
            }),
            _ => unreachable!("invalid handle region"),
        }
    }

    /// Resolve a closure handle to its `&Closure`. Hand-written (not via
    /// `region_ref!`) because the RUNTIME slab wraps each entry in a `OnceLock`
    /// (reserve-then-fill cycle break, see `CodeSlabs::closures`); the cell is
    /// always filled before its handle is published, so `get()` is infallible in
    /// practice.
    pub fn closure(&self, id: ClosureId) -> SlabRef<'_, Closure> {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "closure");
                SlabRef::direct(&self.old().closures[id.index()])
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "closure");
                SlabRef::direct(&self.local.closures[id.index()])
            }
            PRELUDE => SlabRef::direct(&self.prelude.slabs.closures[id.index()]),
            RUNTIME => self.rt_slab_ref(id.code_gen(), |c| {
                c.closures
                    .get(id.index())
                    .expect("runtime closure handle")
                    .get()
                    .expect("runtime closure read before promote filled its slot")
            }),
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
            match cur.unpack() {
                ValueRef::Nil => return Ok(out),
                ValueRef::Pair(p) => {
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
        match v.unpack() {
            ValueRef::Nil => Ok(Vec::new()),
            ValueRef::Pair(_) => self.list_to_vec(v),
            ValueRef::Vector(id) => Ok(self.vector(id).to_vec()),
            ValueRef::Range(id) => self.range_to_vec(id),
            // A set is a sequence of its elements — so `map`/`reduce`/`count`/`vec`/…
            // work on it (Clojure-like). Order is the CHAMP's deterministic-per-shape
            // order, matching how `#{…}` prints.
            ValueRef::Set(id) => Ok(self.set_elems(id)),
            _ => Err(LispError::type_err("expected a list or vector")),
        }
    }
}
