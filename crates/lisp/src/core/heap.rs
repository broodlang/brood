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
//!   running process on its next global lookup (live code reload). The
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

use crate::core::blob::{SHARED_BLOB_THRESHOLD, SharedBlob};
use crate::core::keywords as kw;
use crate::core::map_champ::{self, MAX_DEPTH, MapNode};
use crate::core::value::{
    BigIntId, BytesId, Closure, ClosureArm, ClosureId, ClosureTemplate, DecimalId, EnvId, LOCAL,
    MapId, NativeFn, NativeId, PRELUDE, PairId, Passthrough, RUNTIME, RatioId, RopeId, StrId,
    Symbol, Value, ValueRef, VecId,
};
use crate::error::LispError;

/// Generate a `&self` accessor that resolves a handle to a shared reference by
/// region: the LOCAL/PRELUDE slab is indexed directly; the append-only RUNTIME
/// slab via `boxcar::Vec::get` (stable refs, lock-free). The three uniform
/// all-three-region reference accessors share this; `pair` (returns by value)
/// and the region-restricted `native`/`env_frame` stay hand-written.
macro_rules! region_ref {
    ($(#[$attribute:meta])* $name:ident, $id:ty, $field:ident, $t:ty, $what:literal) => {
        $(#[$attribute])*
        pub fn $name(&self, id: $id) -> &$t {
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
                    &self.old().$field[id.index()]
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
                    &self.local.$field[id.index()]
                }
                PRELUDE => &self.prelude.slabs.$field[id.index()],
                RUNTIME => self
                    .code_gen_ref(id.code_gen())
                    .$field
                    .get(id.index())
                    .expect($what),
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

/// A process's dynamic-binding stack (see `Heap::dynamics`) and, beside it, how many entries
/// each symbol has on it. A global read scans the stack only for a name that is on it:
/// scanning for every global read made N nested `binding`s cost N per read, and a deep
/// recursion through `binding` quadratic. Boxed and allocated by the first `binding`, so a
/// process that never binds carries one word — the `Vec` this replaced was three, and a
/// `Heap` is inline in every `Box<Process>` (`process_floor.rs`).
#[derive(Default)]
pub(crate) struct Dynamics(Option<Box<DynamicStack>>);

#[derive(Default)]
pub(crate) struct DynamicStack {
    stack: Vec<(Symbol, Value)>,
    index: SymbolMap<u32>,
}

impl Dynamics {
    pub(crate) fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |d| d.stack.len())
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, (Symbol, Value)> {
        self.0.as_ref().map_or([].iter(), |d| d.stack.iter())
    }

    pub(crate) fn iter_mut(&mut self) -> std::slice::IterMut<'_, (Symbol, Value)> {
        self.0
            .as_mut()
            .map_or([].iter_mut(), |d| d.stack.iter_mut())
    }

    pub(crate) fn push(&mut self, sym: Symbol, val: Value) {
        let d = self.0.get_or_insert_with(Default::default);
        d.stack.push((sym, val));
        *d.index.entry(sym).or_insert(0) += 1;
    }

    /// Drop every binding pushed since the stack was `len` long.
    pub(crate) fn truncate(&mut self, len: usize) {
        let Some(d) = self.0.as_mut() else { return };
        if len >= d.stack.len() {
            return;
        }
        for &(sym, _) in &d.stack[len..] {
            if let Some(n) = d.index.get_mut(&sym) {
                *n -= 1;
                if *n == 0 {
                    d.index.remove(&sym);
                }
            }
        }
        d.stack.truncate(len);
    }

    /// `sym`'s innermost binding, scanning only when `sym` is on the stack at all.
    #[inline]
    pub(crate) fn lookup(&self, sym: Symbol) -> Option<Value> {
        let d = self.0.as_ref()?;
        if !d.index.contains_key(&sym) {
            return None;
        }
        d.stack
            .iter()
            .rev()
            .find(|&&(s, _)| s == sym)
            .map(|&(_, v)| v)
    }

    #[cfg(test)]
    pub(crate) fn binds(&self, sym: Symbol) -> bool {
        self.0.as_ref().is_some_and(|d| d.index.contains_key(&sym))
    }
}

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

/// Live **green-process** gauge: incremented when a process is spawned,
/// decremented when it is torn down (the scheduler calls [`live_process_inc`] /
/// [`live_process_dec`]). The root thread is *not* counted — `gc_floor`'s
/// `.max(1)` covers it. Kept here in `core` so the GC can read it without a
/// `core → process` dependency; `process` is the only writer.
static LIVE_PROCESSES: AtomicUsize = AtomicUsize::new(0);

/// Note a newly spawned green process (scheduler `spawn`).
pub fn live_process_inc() {
    LIVE_PROCESSES.fetch_add(1, Ordering::Relaxed);
}

/// Note a torn-down green process (scheduler `deregister`).
pub fn live_process_dec() {
    LIVE_PROCESSES.fetch_sub(1, Ordering::Relaxed);
}

/// Current count of live green processes (excludes the root).
pub fn live_process_count() -> usize {
    LIVE_PROCESSES.load(Ordering::Relaxed)
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

/// The per-file `(:use …)` import table: bare name → [`ImportEntry`], plus whether any key
/// may be a module ALIAS or an internals GRANT. An `(:alias mod :as short)` rides this table
/// under the slash-suffixed key `short/`, and a `(:use-internals mod)` under
/// `/internals/mod`, so the resolver and the privacy check would otherwise format and intern
/// one of those keys for every qualified reference to ask a question whose answer is almost
/// always "none". The facts live in the table, not beside it, so every save/restore that
/// swaps the table (`Heap::set_imports`) carries them along unchanged.
///
/// Both are supersets: `may_alias` is set by any key spelled with a trailing `/`, which every
/// alias key has (a `(:shadow /)` key, `/shadow//`, sets it too — the cost of that is only
/// the lookup it would have skipped), and `may_grant` by any key spelled `/internals/…`.
/// Entries are never removed from a table, so neither flag has to clear; a fresh table
/// starts with both false.
#[derive(Clone, Debug, Default)]
pub struct ImportTable {
    entries: HashMap<Symbol, ImportEntry>,
    may_alias: bool,
    may_grant: bool,
}

impl ImportTable {
    /// Note what `key`'s spelling says it might be.
    fn note_key(&mut self, key: Symbol) {
        let spelled = crate::core::value::symbol_name_ref(key);
        self.may_alias |= spelled.ends_with('/');
        self.may_grant |= spelled.starts_with("/internals/");
    }

    /// Add or replace `key`'s entry.
    pub fn insert(&mut self, key: Symbol, entry: ImportEntry) {
        self.note_key(key);
        self.entries.insert(key, entry);
    }

    /// `key`'s entry, if it has one.
    pub fn get(&self, key: Symbol) -> Option<&ImportEntry> {
        self.entries.get(&key)
    }

    /// `key`'s entry, inserted as `default` when it has none.
    pub fn entry_or_insert(&mut self, key: Symbol, default: ImportEntry) -> &mut ImportEntry {
        self.note_key(key);
        self.entries.entry(key).or_insert(default)
    }

    /// Every `(key, entry)`, in the map's iteration order.
    pub fn iter(&self) -> impl Iterator<Item = (&Symbol, &ImportEntry)> {
        self.entries.iter()
    }

    /// Could any key be a module alias? False means no `prefix/` lookup can hit.
    pub fn may_have_aliases(&self) -> bool {
        self.may_alias
    }

    /// Could any key be an internals grant? False means no `/internals/…` lookup can hit.
    pub fn may_grant_internals(&self) -> bool {
        self.may_grant
    }
}

impl FromIterator<(Symbol, ImportEntry)> for ImportTable {
    fn from_iter<I: IntoIterator<Item = (Symbol, ImportEntry)>>(items: I) -> Self {
        let mut table = ImportTable::default();
        for (key, entry) in items {
            table.insert(key, entry);
        }
        table
    }
}

/// **Loader / checker / namespace state — cold for a worker process.** Every field here
/// is used by the process that *loads modules or runs the checker*; a spawned green
/// process that only runs Brood code never touches any of them. Held behind
/// `Option<Box<ColdHeap>>` on the `Heap` so a worker pays 8 bytes instead of ~320, which
/// matters because `Box<Process>` sits near a mimalloc size-class boundary.
///
/// Allocated lazily by [`Heap::cold_mut`] on first write; readers go through
/// [`Heap::cold`] and treat `None` as empty, which is exactly right — an absent
/// `ColdHeap` means "this process has loaded nothing and checked nothing".
#[derive(Default)]
pub(crate) struct ColdHeap {
    /// Nesting depth of an embedded-module load (ADR-166). See `Heap::in_module_load`.
    pub(crate) module_load_depth: u32,
    /// The module loads this process has OPEN, innermost last (KI-135, ADR-344). While a
    /// frame is open, this process's global defines and registry updates land in it instead
    /// of the shared table, its own lookups read it BEFORE the table, and
    /// `publish_module_load` installs the whole frame under one write of the table.
    pub(crate) load_stages: Vec<LoadStage>,
    /// Set while a registry update writes its result back through `env_define`, and while a
    /// restore replays the journal: the journal holds the OPERATION, so the whole-map define
    /// underneath it must not be journalled as well.
    pub(crate) journal_suppressed: bool,
    /// Source position of LOCAL list forms, keyed by [`form_pos_key`].
    pub(crate) form_pos: HashMap<u64, FormPos>,
    /// The file currently being `load`ed, exposed via `(current-file)`.
    pub(crate) current_file: Option<String>,
    /// `current_file` pre-shared as an `Arc<str>`, kept in step by
    /// [`Heap::set_current_file`]. Every recorded form position stores the file it came
    /// from; cloning this is a refcount bump, where `Arc::from(&str)` would allocate and
    /// copy the path once per list form read.
    pub(crate) current_file_arc: Option<Arc<str>>,
    /// The namespace being compiled (`defmodule`).
    pub(crate) compile_ns: Option<Symbol>,
    /// The file a `reflect/load` is reading when that file declares no module — its
    /// absolute path, interned — which a registry write made while no namespace is open
    /// credits (ADR-582). `None` outside such a load, and for a file that has a
    /// `defmodule` (its writes credit the module).
    pub(crate) root_load_file: Option<Symbol>,
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
    pub(crate) imports: ImportTable,
    /// Package-rooted namespaces (ADR-070): while loading a *dependency* `foo`, its
    /// local name is the active prefix, so a module the file declares `(defmodule b)`
    /// roots to `foo/b` and its intra-package `(:use b)`/`(:alias b …)` targets root
    /// too. `None` outside a dep load (the root project / std stay unrooted, short).
    pub(crate) package_prefix: Option<Symbol>,
    /// The short module names the active package provides — the set that decides
    /// whether a referenced module name is *intra-package* (root it) or external
    /// (a std/other-dep name, leave it bare). Empty when `package_prefix` is `None`.
    pub(crate) package_modules: HashSet<Symbol>,
    /// Memoized `mod/name` → `prefix/mod/name` rooting for intra-package *qualified
    /// references* (ADR-070). A miss in the global table falls back to the rooted name
    /// (see [`Heap::root_qualified_ref`]); this caches the symbol→symbol answer — including
    /// the negative one (`None` = not intra-package, don't retry) — so the fallback costs
    /// one lookup rather than a `format!` + intern per reference. Keyed only by symbol,
    /// which is safe because [`Heap::set_package_context`] clears it: the mapping is a
    /// property of the *active* package context, and that's the one place the context
    /// changes. Here rather than on [`Heap`] because outside a package there is nothing to
    /// root and nothing to memoise. `RefCell` because `env_get` is `&self`.
    pub(crate) rooted_ref_ic: RefCell<
        SmallMap<Symbol, Option<Symbol>, std::hash::BuildHasherDefault<runtime_code::SymbolHasher>>,
    >,
}

/// How many caught user errors [`HeapExtras::caught`] keeps: a handler that catches
/// another error while cleaning up before its rethrow does not lose the first.
const CAUGHT_KEPT: usize = 4;

/// One `defmemo` store of one process ([`HeapExtras::memo_stores`], ADR-670).
pub(crate) struct MemoStore {
    /// The memoised definition's qualified name.
    pub(crate) slot: Symbol,
    /// The definition's version — a symbol minted each time the `defmemo` form runs, so a
    /// redefinition finds its predecessor's entries stale rather than reusing them.
    pub(crate) version: Symbol,
    /// The `[key value]` entries, newest first: a LOCAL (or shared) list, traced as a root.
    pub(crate) entries: Value,
}

/// [`Heap::extras`]: the per-process fields almost no process writes, boxed at the first
/// write so a process that never collects or sets a flag pays one pointer for them.
#[derive(Default)]
pub(crate) struct HeapExtras {
    /// A per-heap override of [`rt_gc_floor`] — the process-wide floor is a `OnceLock`
    /// read from `BROOD_RT_GC_FLOOR` exactly once, so a test that lowered it through the
    /// environment lowered it for every test that ran after it in the same process.
    /// `runtime_collector`'s three promotion-count tests read 231 promoted closures for
    /// 3000 redefs whenever a floor-setting test happened to run first (KI-86). A test
    /// that wants a low floor says so on ITS heap ([`Heap::set_rt_gc_floor`]).
    pub(crate) rt_gc_floor_override: Option<usize>,

    pub(crate) gc_copied: u64,

    pub(crate) gc_reclaimed: u64,

    /// GC **pause durations** (the observability timing tier, ROADMAP survey
    /// gap #4 — counts alone can't answer "is GC why this frame stuttered").
    /// Cumulative / max / most-recent collection wall time in nanoseconds,
    /// measured around [`collect`](Heap::collect)'s body (covers both the
    /// legacy flip and the generational path). Timing cost is two `Instant`
    /// reads per *collection* — noise against the µs–ms the collection itself
    /// takes. Surfaced by `(%)` as `:pause-total-us` / `:pause-max-us` /
    /// `:pause-last-us`.
    pub(crate) gc_ns_total: u64,

    pub(crate) gc_ns_max: u64,

    pub(crate) gc_ns_last: u64,

    /// The byte half of the **major** trigger (ADR-509): the buffer bytes the old generation
    /// holds — measured by each major over what it copied, grown by each tenure by what it
    /// appended — and the budget the last major left, `major_growth × what it kept` (0 before
    /// the first: the floor alone). A major is due once the first reaches
    /// `max(MAJOR_BYTE_FLOOR, budget)`. Here rather than on [`Heap`] because only a
    /// collection reads or writes them, and a collection boxes these extras anyway.
    pub(crate) old_buffer_bytes: usize,

    pub(crate) major_buffer_budget: usize,

    /// Per-process heap limit (bytes) — set by
    /// this process on itself via `(proc/flag :max-heap n)`, `None` = unlimited
    /// (the default; the ADR-043 global soft/hard cap is separate). Checked
    /// **after** each collection against the *surviving* footprint (nursery +
    /// old gen), so transient garbage a collection reclaims never trips it.
    pub(crate) proc_mem_limit: Option<usize>,

    /// Sticky post-collection flag: `Some(live_bytes)` when the last collection
    /// left the heap over `proc_mem_limit`. Probed (and cleared) at the eval/VM
    /// safepoints, which raise a catchable error **in this process only** — the
    /// per-process isolation the global hard cap (whole-OS-process abort) lacks.
    pub(crate) proc_limit_hit: Option<usize>,

    /// `(proc/flag :send-errors on)` — when set, a `send` whose target *node*
    /// is unknown/disconnected raises a catchable `:noconnection` error instead
    /// of silently dropping the message (the dist self-healing seam). Default
    /// off: a send to an unreachable node is silent.
    pub(crate) proc_send_errors: bool,

    /// `(proc/flag :no-break on)` — a breakpoint (`std/debug` `break`) passes this process
    /// through instead of parking it. For infrastructure that other processes wait on — a
    /// buffer process, a registry — where a parked caller would freeze whoever asked it
    /// something, and nothing could ever resume it. Default off.
    pub(crate) proc_no_break: bool,

    /// `(proc/flag :responsive mode)` — how this process treats a native call that waits on
    /// the outside world: ordinary (the default), reported to it after the wait, or refused
    /// with `E0048` (ADR-517).
    pub(crate) proc_responsive: crate::process::responsive::Responsive,

    /// Per-process GC **trace** switch (`(gc-trace on/off)`, defaulted from
    /// `BROOD_GC_TRACE`). When set, each minor/major collection prints a one-line
    /// summary to stderr — a Tier-1 observability aid for tests/benchmarks (the
    /// numbers `(%)` reports as cumulative totals, but per collection as
    /// they happen). Per-process like every other heap field: a spawned child
    /// starts from the `BROOD_GC_TRACE` default, not the parent's setting.
    /// `None` until set: the `BROOD_GC_TRACE` default, read lazily.
    pub(crate) gc_trace: Option<bool>,
    /// The debugger's durable per-process causal context (ADR-174 send-level slice):
    /// a settable slot that, unlike a `binding` on [`Heap::dynamics`], survives across
    /// `receive` and migration (so a long-lived server adopts the sender's context
    /// per message). GC-traced exactly where `dynamics` is. `#[cfg(dev-tools)]` — a
    /// lean release has no such field, so the whole send-level path compiles out.
    #[cfg(feature = "dev-tools")]
    pub(crate) trace_context: Option<Value>,
    /// Whether [`Self::trace_context`] is the process's OWN context (set by `with-debugger`
    /// / `span`) — which `spawn` propagates to children — versus one merely ADOPTED
    /// from a received message, which is used to handle that message but must NOT
    /// propagate onward (else an adopted context leaks transitively through unrelated
    /// spawns). Meaningful only when `trace_context` is `Some`.
    #[cfg(feature = "dev-tools")]
    pub(crate) trace_context_own: bool,
    /// The tables this process created (`table/new`), which die with it unless a global
    /// pinned them (`core::table`, "Lifetime"). Pruned of dropped and pinned ones when it
    /// doubles, so a loop that creates and drops tables keeps it short.
    pub(crate) owned_tables: Vec<u64>,
    /// The `owned_tables` length that triggers the next prune.
    pub(crate) owned_tables_prune_at: usize,
    /// The user errors this process caught most recently, oldest first (KI-291). A `catch`
    /// binds a user throw's bare payload, which has nowhere to carry the position and trace
    /// the error collected; without this a rethrow (`finally`, a cleanup then `(throw e)`)
    /// raised it afresh from the rethrow site, and a module's `(error …)` at load was
    /// reported at the outermost requirer's header. `throw` of a recorded payload takes its
    /// origin back ([`Heap::take_caught_origin`]). Only LOCAL heap payloads — objects this
    /// process minted, never a shared literal or global — matched by handle: the record is
    /// GC-traced where `dynamics` is, so no other value can come to share the handle while
    /// it is kept.
    pub(crate) caught: Vec<crate::error::CaughtOrigin>,
    /// This process's `defmemo` stores (ADR-670): one per memoised definition this process
    /// has called, its remembered `[key value]` entries kept IN THIS HEAP — never promoted —
    /// so a remembered key is the very cell the caller passed and `=` on the next call with
    /// it answers by identity, without a walk. GC-traced where `dynamics` is.
    pub(crate) memo_stores: Vec<MemoStore>,
    /// The top-level form scan's safepoint tables for the last few ropes this process
    /// scanned, most recent first (ADR-488). A string's table is cached on the string, but
    /// an editor's document is a new rope per keystroke; keeping the recent ones lets the
    /// next rope's table be carried across the edit instead of rebuilt. The heap does not
    /// interpret the table — `builtins::syntax_scan` downcasts it. Holds no handles, so
    /// nothing here is traced.
    pub(crate) form_scan_ropes: Vec<(ropey::Rope, Arc<dyn std::any::Any + Send + Sync>)>,
    /// The call a builtin handed back to the engine that called it (ADR-505): its callee and
    /// arguments, set by [`Heap::redirect`] and taken by that engine at once, before anything
    /// allocates. Never held across a collection, so not traced.
    pub(crate) pending_redirect: Option<(Value, SmallVec<[Value; 4]>)>,
    /// The drain epoch this process last copied its data out for
    /// ([`Heap::rehome_drain_pins`], ADR-720): at most once a drain, so data that stays
    /// pinned another way (a current-generation cell embedding an old handle, shared and not
    /// this process's to rewrite) falls back to the throttled re-walk. Here, not on the heap:
    /// only a process that pinned a drain ever writes it.
    pub(crate) rehomed_epoch: Option<u64>,
}

/// [`Heap::type_alias_cache`]'s contents: `(declared_sigs_version, name → alias key)`.
type TypeAliasCache = RefCell<(u64, HashMap<Symbol, Option<Symbol>>)>;

/// Checker-only heap state, lazily boxed off [`Heap`] (see [`Heap::check`]). None of it
/// is touched by running Brood code — only by `nest check` / `check-file-deps`.
#[derive(Default)]
pub(crate) struct CheckHeap {
    /// `mod/` prefix → its public `(bare, qualified)` export pairs, keyed by the globals
    /// shape (count and removals, `Heap::globals_shape`) so a `def` or a `reflect/undef`
    /// invalidates it. Lets a whole-project check build the index in ONE pass instead of
    /// rescanning every global per file (O(files²)).
    exports: Option<(
        (usize, u64),
        std::sync::Arc<std::collections::HashMap<String, Vec<(Symbol, Symbol)>>>,
    )>,
    /// The set of `mod/` namespace prefixes in the loaded image, shape-keyed and shared.
    known_ns: Option<(
        (usize, u64),
        std::sync::Arc<std::collections::HashSet<String>>,
    )>,
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
    fn check_mut(&self) -> std::cell::RefMut<'_, CheckHeap> {
        self.check.get_or_init(Box::default).borrow_mut()
    }
}

/// What an entry of [`Heap::vm_handlers`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HandlerKind {
    /// An inline `try` (ADR-400): catches every error that is not a control signal.
    Try,
    /// An interrupt REGION (ADR-491): a `(proc/interruptible ceiling thunk)` call the
    /// driver runs as a frame of its own rather than a native call that runs the thunk
    /// nested, so a `receive` in the thunk parks the process instead of blocking its worker.
    /// The thunk's frame returning to `base` closes it and answers `[:ok value]`.
    /// `window`: the depth of the interrupt window it opened (ADR-493) — it closes that
    /// window however it ends, and catches an interrupt raised FOR it, answering
    /// `[:interrupted e]` at `ip` with the value pushed at `sp`; any other error, an
    /// interrupt raised for an outer window among them, passes through it. `tail`:
    /// the call was in tail position, so its answer RETURNS from the `base` frame.
    Region { window: u32, tail: bool },
}

/// One open inline `try` (ADR-400) or interrupt region (ADR-491) — see
/// [`Heap::vm_handlers`]. Everything the bytecode driver needs to unwind to the frame that
/// opened it and run its handler there.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VmHandler {
    pub(crate) kind: HandlerKind,
    /// The `roots` index of the frame that executed the `TryEnter`. Frames above it in the
    /// same driver have strictly larger bases, so this names the frame uniquely.
    pub(crate) base: usize,
    /// `roots` length at the `TryEnter`: the operand-stack height the handler starts at.
    pub(crate) sp: usize,
    /// `env_roots` / `live_vm_arms` lengths at the `TryEnter`, truncated back to on a catch.
    pub(crate) env_len: usize,
    pub(crate) arms_len: usize,
    /// The handler code's ip in the frame's chunk.
    pub(crate) ip: usize,
    /// The absolute `roots` index of the slot the caught value is bound to.
    pub(crate) bind: usize,
    /// The dynamic stack's length at the `TryEnter`: a `binding` the error skipped the exit
    /// of is undone by truncating back to it (ADR-402).
    pub(crate) dyn_len: usize,
    /// A REGION only: the index in its driver's frame stack of the frame that opened it — the
    /// frame the thunk returns to. Bases cannot say it: a frame with no slots and an empty
    /// operand stack has the same base as the frame it calls.
    pub(crate) caller: usize,
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
    /// data is immutable, an old object can only come to point at a young one through a
    /// binding rebound in a tenured env frame — tracked by [`remembered`](Self::remembered)
    /// — so the old generation is otherwise **not a root set for a minor collection**.
    ///
    /// Lazily boxed: empty for almost every process (a process only populates it by
    /// surviving a minor collection, and a short-lived worker never collects at all), and
    /// inline it would be 264 B on every `Heap`. Reads go through [`Heap::old`], which may only be called when an OLD handle
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

    /// **Per-process read cache for the RUNTIME generations.** A RUNTIME handle deref must
    /// keep `gens[g]`'s `Arc<CodeSlabs>` alive (so a concurrent Stage-4 free can't drop it
    /// mid-read), but taking a fresh `ArcSwap::load` guard per deref dominated
    /// global-data-heavy hot loops, and so did an `Arc` clone per deref. Instead each slot
    /// caches the last-loaded `Arc` plus the [`RuntimeCode::gen_version`] it was loaded at;
    /// [`code_gen_ref`](Self::code_gen_ref) borrows the cached `Arc`'s target when the version
    /// is unchanged and only `load_full`s on a real generation replacement (Stage-4 free /
    /// compaction store — rare), retiring the replaced `Arc` until `&mut self`. `RefCell`/`Cell`: the `Heap` is single-threaded (one worker owns a process at a
    /// time). `gen_cache_ver` starts at `u64::MAX` so the first read always populates.
    gen_cache: [RefCell<Option<Arc<CodeSlabs>>>; 2],
    /// Generation `Arc`s the cache replaced, kept alive until `&mut self` — what makes
    /// [`code_gen_ref`](Self::code_gen_ref)'s borrow sound. Pushed only on a generation
    /// replacement (a Stage-4 free or a compaction store), so it stays tiny.
    gen_cache_retired: RefCell<slabs::LazyVec<Arc<CodeSlabs>>>,
    gen_cache_ver: [Cell<u64>; 2],
    /// The address of each cached generation's `CodeSlabs` (the target of `gen_cache[g]`),
    /// set with `gen_cache_ver[g]`, for native code's inline RUNTIME vector read. That read
    /// trusts it only while `gen_cache_ver[g]` equals the live [`Self::rt_gen_version`];
    /// otherwise it takes the callback, whose `code_gen_ref` refreshes both. `usize`, not a
    /// pointer, so `Heap` stays `Send`.
    #[cfg(feature = "jit")]
    gen_slabs_addr: [Cell<usize>; 2],
    /// The address of this heap's [`RuntimeCode::gen_version`] — stable, since `runtime` is
    /// never reassigned — so native code can read the live version without a call.
    #[cfg(feature = "jit")]
    rt_gen_version: usize,
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
    /// The contract's alias-by-suffix memo — `(declared_sigs_version, name → alias key)`;
    /// see [`Heap::alias_key_by_suffix`]. Keys only, so a relocation cannot strand it.
    /// Boxed at its first use: only a process running contracts ever reads it, and inline
    /// it was 64 bytes on every process.
    type_alias_cache: std::cell::OnceCell<Box<TypeAliasCache>>,
    /// The runtime contract's verdict memo (ADR-480): `(type, value) → verdict` by the
    /// identity of both, so an armed `sig` re-checking an unchanged value pays one lookup.
    /// Cleared or pruned by everything that can make a handle name another object — see
    /// `type_verdicts.rs`. Boxed at its first use, like `type_alias_cache`: only a process
    /// running contracts reads it.
    type_verdicts: std::cell::OnceCell<Box<RefCell<TypeVerdictCache>>>,
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
    dynamics: Dynamics,
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
    global_ic: RefCell<
        SmallMap<Symbol, (u64, Value), std::hash::BuildHasherDefault<runtime_code::SymbolHasher>>,
    >,
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
    /// classes. `None` until this process
    /// is actually handed a fast-path message, which most processes never are.
    msg_roots: Option<Box<MsgRoots>>,
    /// Loader/checker/namespace state — see [`ColdHeap`]. `None` until a module load,
    /// namespace compile or checker run needs it, so a plain worker process never
    /// allocates it (worth a mimalloc size class on `Box<Process>`).
    cold: Option<Box<ColdHeap>>,
    /// Checker-only state — see [`CheckHeap`]. Lazily boxed and `None` until a check
    /// actually runs, because it is ~288 bytes inline on every `Heap`, and a `Heap` is
    /// inline in `Box<Process>`; a spawned worker never checks anything. Behind a
    /// `RefCell` rather than a plain `Option<Box<_>>` (the shape [`ColdHeap`] uses)
    /// because it is filled through `&self` (the checker's read paths).
    check: std::cell::OnceCell<Box<RefCell<CheckHeap>>>,
    /// The GC root stack. A [`roots_buf::RootsBuf`], not a `Vec<Value>`, so its
    /// (ptr, len, cap) header sits at fixed offsets for the JIT (§7.5); same semantics.
    roots: roots_buf::RootsBuf,
    /// The env half of the operand stack (ADR-061): LOCAL [`EnvId`]s an eval
    /// frame still needs across a nested `eval` (its `scope`/`env`). Relocated in
    /// place by every collection alongside `roots`; re-read via
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
    /// How many more bytes of **fresh buffers** — spilled vectors, string text, byte blobs —
    /// the nursery may allocate before a minor collection is due, in 16-byte units
    /// ([`budget_units`]). One object can own any number of bytes, so the object count
    /// alone let a loop making large garbage vectors run to gigabytes before a collection
    /// fired (KI-307: a `conj` loop over a 60 000-element vector reached 12 GB resident in
    /// three seconds). Each fresh buffer counts down, [`gc_due`](Self::gc_due) fires at
    /// zero, and every minor refills it to `max(byte floor, growth × retained buffer
    /// bytes)` capped at `NURSERY_BYTE_MAX` — survivors are paid for, so they size the next
    /// budget instead of spending it. A countdown in one `u32` rather than a count and a
    /// threshold in two `usize`s, because this struct is per-process memory and a parked
    /// process is held to a floor (`tests/process_floor.rs`). `u32::MAX` while GC is off or
    /// an override pins the object floor: 64 GiB, never spent in practice.
    nursery_budget_left: u32,
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
    /// The nursery **generation epoch** — stamped into every nursery handle minted
    /// (the `local_gen` in `alloc_*`), and bumped on every minor collection so the
    /// survivors are re-minted with the new value and any handle held across it
    /// without being re-rooted keeps the old one. A debug-only deref check in the
    /// LOCAL accessors compares `handle.generation()` against this and panics at
    /// the bad deref. Per-heap (not per-slot): the bump allocator never reuses a
    /// slot, so a collection is the only LOCAL-invalidating event.
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
    /// every minor and major collection. Per-heap (per Brood process), reset to
    /// zero only at process start. `u64` so a long-lived server loop can't wrap them.
    /// `gc_runs` = collections performed; `gc_copied` = cumulative survivors
    /// relocated; `gc_reclaimed` = cumulative objects dropped (live-before minus
    /// survivors). These are *counts of LOCAL objects*, not bytes — the cheap,
    /// traversal-free figure (cf. [`local_bytes`](Self::local_bytes) for a byte
    /// estimate).
    gc_runs: u64,
    /// Remembered hashes of LOCAL sets and maps whose hash took real work
    /// (`equality::HASH_MEMO_MIN_WORK`), so a set nested in a set nested in a set is hashed
    /// once, not once per level above it. Stamped with `gc_runs`: a collection moves
    /// handles, so the memo empties at the first use after one. Allocated on first use —
    /// one pointer in a heap that never needs it, because every spawn pays for this
    /// struct (`process_floor`).
    pub(crate) hash_memo: std::cell::OnceCell<Box<std::cell::RefCell<HashMemo>>>,
    /// Compiling-VM body cache (ADR-076). Maps a closure handle's raw
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
    /// entry is simply never looked up again. Empty under `BROOD_TIER=0`. `Arc`
    /// so the trampoline can hold the compiled body across a call without borrowing
    /// the cache.
    vm_cache: vm_cache::VmCacheCell,
    /// The [`RuntimeCode::free_epoch`] this process last synced its [`Self::vm_cache`]
    /// to (ADR-091 Stage 4). When the shared free-epoch advances (a generation was
    /// freed and its slot may be reused with bit-identical handles), the `vm_cache`
    /// read path clears the cache once and updates this — so a stale compiled body
    /// can't alias a reused handle. Cheap: one relaxed atomic load + compare per
    /// closure-call cache lookup, a full clear only on the rare free.
    seen_free_epoch: Cell<u64>,
    /// The runtime `stale_gen` this process last synced its `vm_cache` against (ADR-366).
    seen_stale_gen: Cell<u64>,
    /// The `uid` of the arm most recently marked stale FROM THIS PROCESS (ADR-366), so a
    /// running `SelfCall` loop of that arm re-enters `exec_chunk` with the sentinel epoch and
    /// adopts the recompile at its next back-edge. One hint, not a set: a second stale arm
    /// in the same process overwrites it and simply recompiles at its next lookup instead.
    /// Read at every frame entry — a hot per-process cell, not the arm's cold flag.
    stale_arm_uid: Cell<u64>,
    /// **RUNTIME collector — Stage 4 (drain free-attempt throttle, ADR-091).** A
    /// per-process tick rate-limiting how often this process runs the multigen drain
    /// **free-attempt** ([`crate::process::free_drained_gen`] → the O·live-process
    /// `report_parked_liveness` whole-registry scan) at its safepoint. While a drain is
    /// armed the threshold is held low so every safepoint re-enters the collector; when
    /// the drain can't yet complete — a long-lived process still runs old-generation
    /// code, so the generation stays pinned — that means the O(live-process) registry
    /// scan runs on *every* safepoint of *every* worker purely to re-discover "still not
    /// drained". Throttling the free-attempt to 1/[`RT_DRAIN_SCAN_STRIDE`]
    /// cuts that ~stride-fold; the free is still attempted regularly (no lost wakeup) as
    /// long as any process reaches a safepoint, and every process's O(1) drain
    /// self-report still runs every frame so acks stay current. A plain `Cell` (the
    /// `Heap` is single-threaded), so the throttle adds no atomic or cross-core traffic.
    rt_drain_tick: Cell<u32>,
    /// **RUNTIME collector — Stage 3c (local clean-ack cache, ADR-091).** The drain
    /// epoch this process last reported *clean* for (0 = none). While a drain stays
    /// armed — which it does for the whole run whenever a long-lived process pins the
    /// draining generation (a top-level test-runner loop still executing old-gen
    /// code) — every process's safepoint calls
    /// `report_gen_liveness`, and a process that already acked clean would re-take the
    /// shared `drain_acks` *read* lock every frame just to re-confirm its ack. That
    /// per-frame lock, across every worker for the whole run, is the residual cost once
    /// the scan and the dirty write are handled (the `rounds`-shape ~6× overhead). This
    /// `Cell` short-circuits it: once clean for epoch E, the process is clean for E by
    /// the clean-stays-clean invariant, so a `Cell` read + compare replaces the lock. A
    /// fresh drain bumps the epoch (≠ the cached value) so the process re-reports. Plain
    /// `Cell`: the `Heap` is single-threaded.
    acked_drain_epoch: Cell<u64>,
    /// The pid `acked_drain_epoch` was acked under — what
    /// [`rearm_drain_ack`](Self::rearm_drain_ack) removes from the shared `drain_acks`
    /// table when a shared handle arrives after the ack (KI-188).
    acked_drain_pid: Cell<u64>,
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
    /// Phase-2 pair above: the drain epoch the probe last found this process dirty in via
    /// Phase 1, `u64::MAX` once it finds it clean. It throttles only while the Phase-1 seed
    /// exceeds [`P1_LARGE_SEED`] — a `roots` stack that has grown with recursion depth — so
    /// a shallow process keeps acking on its very next safepoint; see
    /// [`P1_REVALIDATE_STRIDE`]. With `p2_dirty_epoch` it is also what asks for a drain
    /// re-home ([`rehome_drain_pins`](Self::rehome_drain_pins)): either one at the open
    /// drain's epoch says this process holds the draining generation. No field of its own,
    /// so a process that never pins a drain pays nothing for the re-home (`process_floor`).
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
    /// off the GC root graph, and it is held by `&Node`, so the `Arc` can't
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
    /// Next IC-block base to hand out for [`Self::vm_call_ics`] / [`Self::vm_global_ics`].
    /// Bases come from these counters and the tables grow **on publish**
    /// (`vm_call_ic_put` / `vm_global_ic_put`), so an activated arm whose sites never run
    /// materialises no slots. Every reader tolerates a short table (`.get(abs)` — a
    /// missing slot reads exactly like an unpublished one). Reset to 0 in lockstep
    /// with the table clears + `arm_ic_blocks`, preserving ADR-096's site-id
    /// recycling semantics (the sym/argc guards on every probe exist for it).
    next_ic_base: std::cell::Cell<u32>,
    next_gic_base: std::cell::Cell<u32>,
    /// Depth of live tree-walker→VM re-entries (`eval`'s closure application routing a
    /// VM-eligible callee through `vm_apply` — see `tw_vm_route` in `eval.rs`).
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
    /// exactly like an unpublished one. Almost every short-lived process allocates 0 bytes
    /// here.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    vm_fast_links: RefCell<Vec<FastLink>>,
    /// The `INLINE_LANDED` count this process last cleared its fast links at (see
    /// `needs_tier_entry`): a driver entry that finds the count moved clears the links to
    /// arms with an upgrade waiting, so their next call takes the entry that swaps it in.
    #[cfg(feature = "jit")]
    pub(crate) inline_landed_seen: u64,
    /// DEBUG ONLY: per-call-site source position, recorded at compile time and indexed
    /// by the same site id as [`Self::vm_call_ics`]. Lets a crash map a runtime call site
    /// back to its `.blsp` file:line (call-site ids are positional + reset on
    /// `runtime_collect`, so this is grown/cleared in lockstep). For diagnosing the JIT
    /// stale-operand bug — see `dbg_site_pos` / `dbg_set_site_pos`. Filled only under
    /// `BROOD_JIT_VERIFY`, so a process pays for it only when someone is looking.
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
    /// table clear drops the whole map in lockstep with the tables. Read on every VM frame
    /// push, so it hashes the `uid` (a counter) with [`SymbolHasher`]'s one multiply, not
    /// SipHash: past eight arms the map is hashed, and SipHash there was ~4% of a native →
    /// interpreted → native round trip.
    arm_ic_blocks: RefCell<SmallMap<u64, (u32, u32), std::hash::BuildHasherDefault<SymbolHasher>>>,
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
        RefCell<SmallMap<u64, DispatchIcEntry, std::hash::BuildHasherDefault<SymbolHasher>>>,
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
    /// The open inline `try`s of this process's bytecode drivers, innermost last (ADR-400):
    /// pushed by `Inst::TryEnter`, popped by `Inst::TryExit` or by the driver catching an
    /// error into one. Each driver only consults the entries above the length it found on
    /// entry (its floor), so a nested driver's error can never land in an outer driver's
    /// handler without crossing the native frame between them. Plain indices into `roots`
    /// / `env_roots` / `live_vm_arms`, never handles, so the collector ignores it.
    pub(crate) vm_handlers: Vec<VmHandler>,
    /// Native activations left before the entry reduction poll settles a batch with the
    /// scheduler (ADR-401): decremented by every unwindable native activation of an arm that
    /// makes calls, so a native recursion — which has no back edge to poll at — is charged
    /// one reduction per call, as the interpreter charges it, and can be preempted.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) jit_call_budget: i32,
    /// Suspend-host attribution (see `jit_suspend_feedback`). When THIS process's
    /// `receive` dirty-blocks its worker (the §7.4 carve-out: a native frame between the
    /// body driver and the receive means no state capture, so the worker thread waits on
    /// the condvar), the mailbox records WHICH native activation was innermost-alive:
    /// `blocked_under_gateway = cur_native_gateway`. Each JIT gateway stamps a fresh
    /// `native_gateway_seq` into `cur_native_gateway` around its invoke (save/restore,
    /// like `jit_dbg_fn`), and after the invoke latches its arm `BAILED` iff the recorded
    /// token is ITS OWN — so only the arm that actually enclosed the blocking receive is
    /// latched, never a native that merely ran later in the same quantum (in a gen-server,
    /// that would be the hot post-receive handler). A latching gateway then hands the token
    /// on to the activation enclosing it (`blame_enclosing_gateway`), so every native frame
    /// under the receive latches as it exits. `0` = none. A block with no live JIT
    /// gateway (a Rust-native shape, `map`/`try` callbacks) records 0 and latches nothing.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))] // only JIT gateways bump it
    pub(crate) native_gateway_seq: u64,
    /// The local a native arm's tail call is about to call, stored by the arm just before it
    /// exits for the driver to make that call — only at a call whose head is a local, the one
    /// fact the shared tail exit cannot carry. `jit_dispatch_tail` takes it (resetting it to
    /// `u32::MAX`, none) on every tail call, so it names the local in a `cannot call
    /// non-function` from THIS call and is never left over for the next.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))] // only native code stores it
    pub(crate) jit_tail_local_head: u32,
    /// The global-read site of the unbound read a native arm's callback just raised for
    /// (`jit_resolve_global_ic`), `u32::MAX` when none. The callback has no arm to locate the
    /// read with; the arm's own error block does (`brood_rt_trace_push`), and takes it there.
    #[cfg_attr(not(feature = "jit"), allow(dead_code))] // only native code's callbacks set it
    pub(crate) jit_miss_gsite: u32,
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
    /// The address of the argument slice an engine that can take a redirect is passing the
    /// builtin it is calling right now (ADR-505), or 0. A builtin whose slow path would call
    /// Brood compares its own `args` against it ([`Heap::redirect_offered`]): equal means it
    /// is that call, and may hand the callee back instead of running it nested; anything
    /// else — a builtin another builtin called from Rust, with a slice of its own — keeps
    /// the nested call.
    redirect_offer: usize,
    /// Who a registry write this process makes OUTSIDE any load credits (ADR-596): the module
    /// (or namespace-less file) whose load spawned this process, or an ancestor's, or what a
    /// `registry/with-acting-writer` set for an extent. `spawn` hands it to the child — so a
    /// timer, a task, a worker started by a plugin registers as that plugin. `None` for a
    /// process nothing spawned under a load and nothing set: its run-time writes credit
    /// nobody, as before.
    pub(crate) acting_writer: Option<RegistryWriter>,
    /// Per-process state almost no process touches — the GC statistics, the `proc/flag`
    /// settings, a test's shared-code floor — boxed at its first write. Inline it was ~130
    /// bytes on every process; most never collect, never set a flag, and park
    /// (`process_floor.rs`). Readers treat `None` as every field at its default.
    extras: Option<Box<HeapExtras>>,
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

/// What the two position tables hold for a list form: where it is, which file, and whether
/// the EXPANDER built it (a macro's result, a pattern-binder desugar) rather than the reader.
/// One record for both tables — LOCAL (`ColdHeap::form_pos`) and RUNTIME
/// (`RuntimeCode::positions`) — so `promote` carries the whole fact across, not a subset.
#[derive(Clone, Debug)]
pub(crate) struct FormPos {
    pub(crate) pos: crate::error::Pos,
    pub(crate) file: Option<Arc<str>>,
    pub(crate) synthetic: bool,
}

/// The [`RuntimeCode::positions`] key for a RUNTIME pair: its slab index plus its
/// **code generation**. The RUNTIME twin of [`form_pos_key`] (which packs the LOCAL
/// age bit at the same offset), and for the same reason: the two RUNTIME generations
/// share one index space, so a bare index conflates them (see `positions`). Slab
/// indices are bounded by `GEN_SHIFT` bits, so the low 32 bits always suffice.
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
    v.heap_handle()
        .is_some_and(|handle| handle.region() == LOCAL)
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
    v.heap_handle()
        .is_some_and(|handle| matches!(handle.region(), LOCAL | RUNTIME))
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
/// `#[repr(C, u8)]` because the JIT's inline fast-frame path (§7.5) saves/restores [`Heap::jit_call_env`] as two opaque words
/// and constructs `Stable(EnvId::GLOBAL)` by storing `(0, u64::MAX)` — the
/// defined layout (tag `u8` at 0, payload at 8, 16 bytes total) is what makes
/// those stores a valid value. Pinned by `env_root_layout_is_pinned`.
#[derive(Clone, Copy)]
#[repr(C, u8)]
pub enum EnvRoot {
    Stable(EnvId),
    Slot(usize),
}

/// Byte offsets of the `Heap` fields the JIT's inline fast-frame path (§7.5) reads and writes directly from emitted code — the call ceremony
/// `jit_run_fast_link` otherwise performs in Rust. Computed with `offset_of!` at lower
/// time inside the same binary that runs the emitted code, so they are exact by
/// construction; a struct reorder recomputes them on the next build.
#[cfg(feature = "jit")]
#[derive(Clone, Copy)]
pub(crate) struct JitCeremonyOffsets {
    /// `Heap.roots` — a [`roots_buf::RootsBuf`], whose own header is (ptr, len, cap)
    /// at +0/+8/+16 (pinned by a `const` assertion in `roots_buf.rs`).
    pub roots: usize,
    /// The gateway-token counter — bumped per inline call so the callee's token is
    /// unique among live activations. The per-call save/restore of `jit_call_env`,
    /// `jit_native_depth`, `jit_force_vm`, `jit_dbg_fn`, the IC cursors and
    /// `cur_native_gateway` is gone (rung A1): callbacks assert them from the callee's
    /// `JitCallCtx` instead, so those offsets are no longer needed here.
    pub native_gateway_seq: usize,
    pub blocked_under_gateway: usize,
    /// `Heap::jit_call_budget` — the entry reduction poll's countdown (ADR-401).
    pub jit_call_budget: usize,
    /// `Heap::jit_tail_local_head` — a local-headed tail call's head, stored before the exit.
    pub jit_tail_local_head: usize,
}

#[cfg(feature = "jit")]
pub(crate) fn jit_ceremony_offsets() -> JitCeremonyOffsets {
    // The two-word EnvRoot save/restore (and the (0, MAX) = Stable(GLOBAL) store) needs
    // exactly this size; a grown EnvRoot must revisit the emission.
    const { assert!(std::mem::size_of::<EnvRoot>() == 16) };
    JitCeremonyOffsets {
        roots: std::mem::offset_of!(Heap, roots),
        native_gateway_seq: std::mem::offset_of!(Heap, native_gateway_seq),
        blocked_under_gateway: std::mem::offset_of!(Heap, blocked_under_gateway),
        jit_call_budget: std::mem::offset_of!(Heap, jit_call_budget),
        jit_tail_local_head: std::mem::offset_of!(Heap, jit_tail_local_head),
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
        let stable: EnvRoot = unsafe { std::mem::transmute::<[u64; 2], EnvRoot>(words) };
        assert!(matches!(stable, EnvRoot::Stable(e) if e.0 == u64::MAX));
        let slot = [1u64, 42u64];
        // SAFETY: tag 1 = Slot, payload 42.
        let slotted: EnvRoot = unsafe { std::mem::transmute::<[u64; 2], EnvRoot>(slot) };
        assert!(matches!(slotted, EnvRoot::Slot(42)));
    }
}

// ===== Construction and shared-region management ================================

mod env_globals;
mod equality;
use equality::HashMemo;
/// Side facts — what a definition RECORDS about a name rather than binds to it (ADR-320).
mod facts;
mod features_audit;
pub use facts::{Fact, FactKind};
mod freeze;
mod gc;
// The collector's tuning knobs live with it (move f); re-imported so `heap.rs` and the
// sibling children resolve them unqualified, as they did when they were defined here.
use self::gc::{
    DRAIN_REPORT_STRIDE, P1_LARGE_SEED, P1_REVALIDATE_STRIDE, P2_REVALIDATE_STRIDE,
    RT_DRAIN_SCAN_STRIDE, WALKER_RED_ZONE, WALKER_STACK_CHUNK, budget_units, gc_floor, major_floor,
    nursery_byte_floor, rt_gc_floor,
};
mod gc_runtime;
mod local_string;
use self::local_string::{LocalString, StrData};
mod map_ops;
mod positions;
mod promote;
mod roots_buf;
/// The runtime's shared code region: the `def`'d code and globals table every process of
/// a runtime holds in common, and the generation machinery around them.
mod runtime_code;
#[cfg(all(feature = "jit", test))]
pub(crate) use self::runtime_code::SUSPEND_SWEEP_FLOOR;
use self::runtime_code::{
    ClosureTemplateMap, ConstClosureMap, LoadStage, LoadWrite, MemberSlot, RegistryWriters,
    reg_trace_enabled,
};
pub use self::runtime_code::{
    GenPin, GlobalsSnapshot, NameMeta, RegistryOp, RegistryWriter, RuntimeCode, SourceLoc,
    SymbolHasher, SymbolMap, VmCacheMap,
};
mod slabs;
mod small_map;
#[cfg(feature = "jit")]
pub(crate) use self::slabs::rt_vec_entry_stride;
use self::slabs::{
    AppendBuf, BufferMark, CodeSlabs, Slabs, buffer_bytes_since, buffer_mark, owned_bytes,
    park_trim_due, park_trim_probe, shrink_slabs, slab_bytes, slab_capacity_bytes, slab_live_count,
};
pub(crate) use self::slabs::{INLINE_VEC_CAP, VecStore};
pub(crate) use self::small_map::SmallMap;
mod type_verdicts;
pub(crate) use self::type_verdicts::{KnownVerdict, TypeVerdictCache, is_base_type_name};
mod vm_cache;
/// The per-process compiled-body cache's type — see [`Heap::vm_cache`].
pub(crate) type VmCache = SmallMap<
    VmCacheKey,
    vm_cache::VmCacheEntry,
    std::hash::BuildHasherDefault<runtime_code::SymbolHasher>,
>;
// `stall_guard` is used by the RUNTIME compactor (`gc_runtime`) and the GUI paint
// path, so it's re-exported unconditionally; `stall_guard_pid` by the scheduler.
pub(crate) use self::gc::{stall_guard, stall_guard_pid, stall_threshold_ms};
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
    /// Exists because *where a deopt resumed* — the last checkpoint — does not say which
    /// of the dozen guards after that point failed (KI-49).
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
    /// Measurement surface for the position-table cost; read by `(%)`.
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

    /// The rarely-written fields, if any has been written — see [`Heap::extras`].
    #[inline]
    pub(crate) fn extras(&self) -> Option<&HeapExtras> {
        self.extras.as_deref()
    }

    /// Record that process `pid` created table `id`, so its exit drops it.
    pub(crate) fn note_owned_table(&mut self, pid: u64, id: u64) {
        let extras = self.extras_mut();
        extras.owned_tables.push(id);
        if extras.owned_tables.len() > extras.owned_tables_prune_at {
            extras
                .owned_tables
                .retain(|&t| crate::core::table::owned_by(t, pid));
            extras.owned_tables_prune_at = (extras.owned_tables.len() * 2).max(64);
        }
    }

    /// Record where the user error `error` was raised as a handler catches it, binding
    /// `caught` (KI-291, [`HeapExtras::caught`]). Keeps the newest [`CAUGHT_KEPT`]. Takes
    /// the error: every catch is done with it here, so its trace moves rather than copies.
    #[inline]
    pub(crate) fn note_caught(&mut self, error: crate::error::LispError, caught: Value) {
        // A swallowed interrupt is raised again at the next safepoint (ADR-491, sticky).
        if error.kind == crate::error::ErrorKind::Interrupted {
            crate::process::interrupt_caught();
        }
        // A contract violation binds a map rebuilt from its payload, not the payload.
        let Some(payload) = error.payload else { return };
        // Only a payload this process minted: a literal or a `def`'d sentinel lives in a
        // shared region, one object to every raise of it, so a swallowed raise would lend
        // its origin to a later, unrelated one.
        if !payload.same_heap_object(caught)
            || payload
                .heap_handle()
                .is_none_or(|h| h.region() != crate::core::value::LOCAL)
            || (error.pos.is_none() && error.file.is_none() && error.trace.is_empty())
        {
            return;
        }
        self.record_caught(error, payload);
    }

    /// [`Heap::note_caught`]'s recording half, kept off the catch path's hot loop (a loop
    /// that throws and catches an immediate never reaches it).
    #[cold]
    #[inline(never)]
    fn record_caught(&mut self, mut error: crate::error::LispError, payload: Value) {
        let origin = crate::error::CaughtOrigin {
            payload,
            pos: error.pos,
            file: error.file.take(),
            trace: std::mem::take(&mut error.trace),
        };
        let caught = &mut self.extras_mut().caught;
        caught.retain(|o| !o.payload.same_heap_object(payload));
        if caught.len() == CAUGHT_KEPT {
            caught.remove(0);
        }
        caught.push(origin);
    }

    /// Whether any caught user error's origin is recorded — `throw`'s cheap test.
    #[inline]
    pub(crate) fn has_caught_origins(&self) -> bool {
        self.extras().is_some_and(|e| !e.caught.is_empty())
    }

    /// The recorded origin of the caught payload `v`, removed — for `throw` to raise it
    /// again from where it was first raised (KI-291).
    pub(crate) fn take_caught_origin(&mut self, v: Value) -> Option<crate::error::CaughtOrigin> {
        let caught = &mut self.extras.as_deref_mut()?.caught;
        let at = caught.iter().rposition(|o| o.payload.same_heap_object(v))?;
        Some(caught.remove(at))
    }

    /// The entries this process's `defmemo` store `slot` holds (ADR-670): nil when it has
    /// none, or when `version` is given and the store was kept by another version of the
    /// definition (a redefinition starts empty).
    pub(crate) fn memo_store(&self, slot: Symbol, version: Option<Symbol>) -> Option<Value> {
        let store = self
            .extras()?
            .memo_stores
            .iter()
            .find(|store| store.slot == slot)?;
        match version {
            Some(version) if version != store.version => None,
            _ => Some(store.entries),
        }
    }

    /// Keep `entries` as this process's `defmemo` store `slot` at `version`, replacing what
    /// it held under any version. The value is held as it is — no copy — so its cells stay
    /// the caller's (ADR-670).
    pub(crate) fn set_memo_store(&mut self, slot: Symbol, version: Symbol, entries: Value) {
        let stores = &mut self.extras_mut().memo_stores;
        match stores.iter_mut().find(|store| store.slot == slot) {
            Some(store) => {
                store.version = version;
                store.entries = entries;
            }
            None => stores.push(MemoStore {
                slot,
                version,
                entries,
            }),
        }
    }

    /// The tables this process created and may still own — for its exit to drop.
    pub(crate) fn owned_tables(&self) -> &[u64] {
        self.extras().map_or(&[], |e| &e.owned_tables)
    }

    /// The rarely-written fields, allocating them on first use.
    pub(crate) fn extras_mut(&mut self) -> &mut HeapExtras {
        self.extras.get_or_insert_with(Default::default)
    }

    /// An engine that can take a redirect is about to call a builtin on `argv` (ADR-505).
    /// Returns the offer it replaces, which the engine puts back with
    /// [`Heap::withdraw_redirect`] once the builtin returns.
    #[inline]
    pub(crate) fn offer_redirect(&mut self, argv: &[Value]) -> usize {
        std::mem::replace(&mut self.redirect_offer, argv.as_ptr() as usize)
    }

    /// The builtin returned: the offer [`Heap::offer_redirect`] made is over.
    #[inline]
    pub(crate) fn withdraw_redirect(&mut self, previous: usize) {
        self.redirect_offer = previous;
    }

    /// Whether `args` is the slice an engine offered a redirect for — that is, this builtin
    /// is the call the engine made, not one another builtin made from Rust — taking the
    /// offer when it is, so nothing nested under the builtin can take it too.
    #[inline]
    pub(crate) fn redirect_offered(&mut self, args: &[Value]) -> bool {
        if self.redirect_offer != 0 && self.redirect_offer == args.as_ptr() as usize {
            self.redirect_offer = 0;
            true
        } else {
            false
        }
    }

    /// Hand `callee` on `args` back to the engine as the builtin's whole result (ADR-505):
    /// the engine calls it where the builtin was called, as an ordinary call, so a `receive`
    /// inside parks. Only after [`Heap::redirect_offered`] said yes.
    pub(crate) fn redirect(&mut self, callee: Value, args: &[Value]) -> crate::error::LispError {
        self.extras_mut().pending_redirect = Some((callee, SmallVec::from_slice(args)));
        crate::error::LispError::control(crate::error::Control::Redirect)
    }

    /// The call a builtin handed back with [`Heap::redirect`].
    pub(crate) fn take_redirect(&mut self) -> (Value, SmallVec<[Value; 4]>) {
        self.extras
            .as_mut()
            .and_then(|extras| extras.pending_redirect.take())
            .expect("a redirect signal without its call")
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
    ///
    /// Everything else is [`Heap::with_regions`]'s, by construction: the two were once
    /// written out field by field, ~90 each, differing in six.
    pub fn new() -> Self {
        Heap {
            gc_threshold: usize::MAX,
            rt_gc_threshold: usize::MAX,
            major_threshold: usize::MAX,
            nursery_budget_left: u32::MAX,
            gc_enabled: false,
            ..Heap::with_regions(Arc::default(), Arc::default())
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
            gen_cache_retired: RefCell::new(slabs::LazyVec::default()),
            gen_cache_ver: [Cell::new(u64::MAX), Cell::new(u64::MAX)],
            #[cfg(feature = "jit")]
            gen_slabs_addr: [Cell::new(0), Cell::new(0)],
            #[cfg(feature = "jit")]
            rt_gen_version: &runtime.gen_version as *const std::sync::atomic::AtomicU64 as usize,
            closure_tpl_cache: RefCell::new(ClosureTemplateMap::default()),
            closure_tpl_ver: Cell::new(u64::MAX),
            closure_const_cache: RefCell::new(ConstClosureMap::default()),
            closure_const_ver: Cell::new(u64::MAX),
            type_alias_cache: std::cell::OnceCell::new(),
            type_verdicts: std::cell::OnceCell::new(),
            prelude,
            runtime,
            global: EnvId::local(0),
            dynamics: Dynamics::default(),
            global_ic: RefCell::new(SmallMap::new()),
            msg_roots: None,
            cold: None,
            check: std::cell::OnceCell::new(),
            roots: roots_buf::RootsBuf::new(),
            env_roots: Vec::new(),
            gc_threshold: gc_floor(),
            nursery_budget_left: budget_units(nursery_byte_floor()),
            park_trim_mark: 0,
            rt_gc_threshold: rt_gc_floor(),
            gc_enabled: true,
            rt_collect_block: std::cell::Cell::new(0),
            local_epoch: 0,
            remembered: Vec::new(),
            old_epoch: 0,
            major_threshold: major_floor(),
            gc_runs: 0,
            extras: None,
            hash_memo: Default::default(),
            vm_cache: vm_cache::VmCacheCell::default(),
            seen_free_epoch: Cell::new(0),
            seen_stale_gen: Cell::new(0),
            stale_arm_uid: Cell::new(0),
            rt_drain_tick: Cell::new(0),
            acked_drain_epoch: Cell::new(0),
            acked_drain_pid: Cell::new(0),
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
            #[cfg(feature = "jit")]
            inline_landed_seen: 0,
            #[cfg(debug_assertions)]
            dbg_site_pos: RefCell::new(Vec::new()),
            vm_global_ics: RefCell::new(Vec::new()),
            arm_ic_blocks: RefCell::new(SmallMap::new()),
            cur_ic_base: Cell::new(0),
            cur_gic_base: Cell::new(0),
            dispatch_ics: RefCell::new(SmallMap::new()),
            jit_call_env: EnvRoot::Stable(EnvId::GLOBAL),
            jit_native_depth: 0,
            jit_stack_limit: 0,
            jit_force_vm: false,
            jit_dbg_fn: u32::MAX,
            jit_pending_error: None,
            vm_handlers: Vec::new(),
            jit_call_budget: 0,
            native_gateway_seq: 0,
            jit_tail_local_head: u32::MAX,
            jit_miss_gsite: u32::MAX,
            cur_native_gateway: 0,
            blocked_under_gateway: 0,
            jit_i64_overflow: false,
            redirect_offer: 0,
            acting_writer: None,
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

    /// This runtime's held image files by path (see `RuntimeCode::image_files`).
    pub(crate) fn image_files(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, Arc<crate::boot::image::ImageFile>>> {
        crate::core::sync::lock(&self.runtime.image_files)
    }

    /// Record the modules an indexed image holds and the macro names among them (see
    /// `RuntimeCode::image_modules`).
    pub(crate) fn register_image_kinds(
        &self,
        modules: impl IntoIterator<Item = Symbol>,
        macros: impl IntoIterator<Item = Symbol>,
    ) {
        crate::core::sync::write(&self.runtime.image_modules).extend(modules);
        crate::core::sync::write(&self.runtime.image_macros).extend(macros);
    }

    /// The kind index as `(modules, macros)` — what the shared boot carries into each runtime.
    pub(crate) fn image_kinds_snapshot(&self) -> (Vec<Symbol>, Vec<Symbol>) {
        (
            crate::core::sync::read(&self.runtime.image_modules)
                .iter()
                .copied()
                .collect(),
            crate::core::sync::read(&self.runtime.image_macros)
                .iter()
                .copied()
                .collect(),
        )
    }

    /// Has this runtime's kind index described `module`, from an image or its source?
    pub(crate) fn image_indexes(&self, module: Symbol) -> bool {
        crate::core::sync::read(&self.runtime.image_modules).contains(&module)
    }

    /// Does an image this runtime indexed vouch that `qualified` (in `module`) is NOT a
    /// macro? True only for a module such an image holds whose macros do not include the name.
    pub(crate) fn image_says_function(&self, module: Symbol, qualified: Symbol) -> bool {
        crate::core::sync::read(&self.runtime.image_modules).contains(&module)
            && !crate::core::sync::read(&self.runtime.image_macros).contains(&qualified)
    }

    /// This runtime's memo of modules a `require` failed to find (see
    /// `RuntimeCode::absent_modules`).
    pub(crate) fn absent_modules(&self) -> &RwLock<HashSet<Symbol>> {
        &self.runtime.absent_modules
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

    /// Reclaim everything allocated into the LOCAL nursery since `checkpoint`, by
    /// truncating the slabs back to it.
    ///
    /// **Safety contract (logical, not `unsafe`):** call this only at a top-level
    /// boundary — when the evaluator has fully returned and no value reachable
    /// from here on holds a LOCAL handle at or past `checkpoint`. Globals live in the
    /// PRELUDE/RUNTIME regions and never point into LOCAL (a top-level `def`
    /// *promotes* its value out), so they're always safe; the only thing that can
    /// still be live is the *result* of the form just evaluated — consume or
    /// promote it before resetting. Resetting mid-evaluation would strand the
    /// in-flight computation's values and corrupt later reads.
    ///
    /// **Collection-safety.** If a collection fired between [`checkpoint`] and
    /// here, it already compacted the nursery and bumped
    /// [`local_epoch`](Self::local_epoch), so the checkpoint's slab lengths no longer
    /// describe the live nursery and truncating to them would strand the survivors
    /// the collector just kept. On an epoch mismatch this is a no-op — the
    /// collection has already reclaimed the dead, and the next `gc_due` reclaims
    /// this form's now-garbage survivors.
    pub fn reset_local_to(&mut self, checkpoint: LocalCheckpoint) {
        if self.local_epoch != checkpoint.epoch {
            return;
        }
        self.local.pairs.truncate(checkpoint.pairs);
        self.local.vectors.truncate(checkpoint.vectors);
        self.local.maps.truncate(checkpoint.maps);
        self.local.strings.truncate(checkpoint.strings);
        self.local.bigints.truncate(checkpoint.bigints);
        self.local.decimals.truncate(checkpoint.decimals);
        self.local.ratios.truncate(checkpoint.ratios);
        self.local.bytes.truncate(checkpoint.bytes);
        self.local.ropes.truncate(checkpoint.ropes);
        self.local.closures.truncate(checkpoint.closures);
        self.local.envs.truncate(checkpoint.envs);
        // Drop position metadata for the nursery pairs just reclaimed (their indices
        // are reused). Keys pack the age bit at bit 32 (`form_pos_key`): only the
        // nursery slab was truncated, so an OLD pair's entry always survives.
        if self.cold.as_ref().is_some_and(|c| !c.form_pos.is_empty()) {
            self.cold_mut()
                .form_pos
                .retain(|&key, _| (key >> 32) != 0 || (key as u32 as usize) < checkpoint.pairs);
        }
        // The truncated slots are minted again at the same epoch, so a verdict about one
        // would answer for whatever is allocated there next (ADR-480).
        self.type_verdicts_forget_nursery();
        // A `defmemo` store is the one per-process root that may point into the nursery
        // across a top-level boundary (ADR-670): what the form just evaluated remembered
        // may sit in the slots reclaimed above. Forget every store — a cache, so this only
        // costs the next call a recomputation.
        if let Some(extras) = self.extras.as_deref_mut() {
            extras.memo_stores.clear();
        }
        // The threshold is relative to live count; reclamation here is so cheap
        // that we let the next `gc_due` check recompute against the smaller heap.
    }

    // ===== Runtime-GC floor knobs ===============================================

    /// The runtime-region GC floor this heap uses: its own override, else the process-wide
    /// [`rt_gc_floor`].
    pub(crate) fn floor(&self) -> usize {
        self.extras()
            .and_then(|e| e.rt_gc_floor_override)
            .unwrap_or_else(rt_gc_floor)
    }

    /// Set THIS heap's runtime-GC floor — for a test that needs a compaction to trip on a
    /// small churn. Takes effect immediately: the threshold becomes the floor (as it is at
    /// construction), and every later re-arm reads the override. Replaces setting
    /// `BROOD_RT_GC_FLOOR` in a test, which — being read once per process — leaked into
    /// every test scheduled after it (KI-86).
    pub fn set_rt_gc_floor(&mut self, floor: usize) {
        self.extras_mut().rt_gc_floor_override = Some(floor);
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
        let store = VecStore::from_vec(items);
        self.note_fresh_buffer(store.buffer_bytes());
        let idx = alloc_slot!(self, vectors, store);
        Value::vector(VecId::local_gen(idx, self.local_epoch))
    }

    /// Allocate a vector holding a copy of `items`. A vector of up to `INLINE_VEC_CAP`
    /// elements is stored inline in its slab entry, so building one from a slice needs no
    /// `Vec` at all — where `alloc_vector(items.to_vec())` mallocs a buffer only to copy
    /// it inline and free it. A longer one is copied into its spill buffer once.
    pub fn alloc_vector_from_slice(&mut self, items: &[Value]) -> Value {
        let store = VecStore::from_flushed(items.len(), |i| items[i]);
        self.note_fresh_buffer(store.buffer_bytes());
        let idx = alloc_slot!(self, vectors, store);
        Value::vector(VecId::local_gen(idx, self.local_epoch))
    }

    /// A fresh vector of `v`'s elements followed by `tail` — the append behind `conj` and
    /// `into` on a vector (ADR-511). A vector is flat, so this used to copy all of `v`, and
    /// a loop of `conj` was quadratic. Now it writes into the spare capacity of `v`'s append
    /// buffer when `v` is the newest vector over it, and copies only otherwise: exactly
    /// sized for a first append, so a one-off `into` wastes nothing, and with room for as
    /// many again when `v` was itself an append, so a chain of them is amortised O(1).
    pub fn vector_append(&mut self, id: VecId, tail: &[Value]) -> Value {
        let head_len = self.vector(id).len(); // the use-after-GC tripwire, before the slot read
        let total = head_len + tail.len();
        // Only a LOCAL vector's buffer may grow: a shared region's never takes a write.
        let (appended, appending) = {
            let store = match id.region() {
                LOCAL if id.is_old() => Some(&self.old().vectors[id.index()]),
                LOCAL => Some(&self.local.vectors[id.index()]),
                _ => None,
            };
            match store {
                Some(VecStore::Shared { len, buf, .. }) => (
                    buf.append_in_place(*len as usize, tail)
                        .then(|| VecStore::shared(buf.clone(), total)),
                    true,
                ),
                _ => (None, false),
            }
        };
        let store = match appended {
            Some(store) => store,
            None if total <= INLINE_VEC_CAP => {
                let mut items = self.vector(id).to_vec();
                items.extend_from_slice(tail);
                VecStore::from_vec(items)
            }
            None => {
                let capacity = if appending { total * 2 } else { total };
                let mut items = Vec::with_capacity(capacity);
                items.extend_from_slice(self.vector(id));
                items.extend_from_slice(tail);
                self.note_fresh_buffer(capacity * std::mem::size_of::<Value>());
                VecStore::shared(AppendBuf::new(items), total)
            }
        };
        let idx = alloc_slot!(self, vectors, store);
        Value::vector(VecId::local_gen(idx, self.local_epoch))
    }

    /// Allocate a vector of the `len` operand-stack slots from `start` — the VM's vector
    /// literal (`Inst::MakeVector`) — with no intermediate `Vec`: inline for a small one,
    /// one exact-size spill buffer otherwise. The caller drops the operands after.
    pub(crate) fn alloc_vector_from_roots(&mut self, start: usize, len: usize) -> Value {
        let store = VecStore::from_flushed(len, |k| self.root_at(start + k));
        self.note_fresh_buffer(store.buffer_bytes());
        let idx = alloc_slot!(self, vectors, store);
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
            VecStore::Spill { .. } | VecStore::Shared { .. } => {
                unreachable!("just pushed an Inline")
            }
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
        let idx = alloc_slot!(
            self,
            vectors,
            VecStore::Inline {
                len: 2,
                items: [source, xform],
            }
        );
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
        // Saturate — a range longer than i64::MAX can't be materialised anyway.
        self.range_len_exact(id).min(i64::MAX as i128) as i64
    }

    /// The EXACT number of elements a range yields, unsaturated. O(1). A wide range
    /// (`i64::MIN..i64::MAX`) has more than `i64::MAX` elements, so anything that
    /// distinguishes two ranges by length — equality, hashing — must use this, not
    /// the saturating [`range_len`](Self::range_len).
    pub fn range_len_exact(&self, id: VecId) -> i128 {
        let (lo, hi, step) = self.range_parts(id);
        // step is non-zero by construction; an empty range has a non-positive span.
        let (lo, hi, step) = (lo as i128, hi as i128, step as i128);
        let span = if step > 0 { hi - lo } else { lo - hi };
        let mag = step.abs();
        ((span + mag - 1) / mag).max(0)
    }

    /// `(nth coll i default)` on a vector with a fixnum index — the rule behind
    /// `PrimOp3::VectorRefOr`, shared by the VM and `brood_rt_vector_ref3`: the element
    /// when `i` is in range, else `default`. `None` for every other shape (bytes, a list,
    /// a record, a bad index), which the prelude `nth` decides. Never allocates.
    pub fn nth3_inline(&self, coll: Value, i: Value, default: Value) -> Option<Value> {
        let (Value::Vector(id), Value::Int(i)) = (coll, i) else {
            return None;
        };
        let v = self.vector(id);
        Some(if i >= 0 && (i as u64) < v.len() as u64 {
            v[i as usize]
        } else {
            default
        })
    }

    /// `(count v)` for the shapes whose answer is a size the heap already holds — the
    /// rule behind `PrimOp1::Count`, shared by the VM and `brood_rt_count`. `None` for
    /// everything the prelude `count` decides in Brood: a record (its `Counted` /
    /// `Seqable` impls), a seq-view, a rope or table, an improper list, a `Seqable` impl
    /// and the type error. Never allocates. Out of line, so its list walk and map probe
    /// stay out of `exec_chunk`'s body.
    #[inline(never)]
    pub fn count_inline(&self, v: Value) -> Option<i64> {
        Some(match v {
            Value::Nil => 0,
            Value::Vector(id) => self.vector(id).len() as i64,
            Value::Str(id) => self.str_metrics(id).0 as i64,
            Value::Range(id) => self.range_len(id),
            Value::Bytes(id) => self.bytes(id).as_bytes().len() as i64,
            Value::Set(id) => self.map_size(id) as i64,
            // A record (ADR-542) is its own kind and counts through the prelude's `count`,
            // which consults `Counted`/`Seqable`; a plain map needs no probe for it.
            Value::Map(id) => self.map_size(id) as i64,
            Value::Pair(mut id) => {
                let mut n = 1i64;
                loop {
                    match self.cdr(id) {
                        Value::Pair(next) => {
                            id = next;
                            n += 1;
                        }
                        Value::Nil => break n,
                        _ => return None,
                    }
                }
            }
            _ => return None,
        })
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
    fn check_epoch_aged(&self, is_old: bool, generation: u32, index: usize, what: &str, raw: u64) {
        let (expected, space) = if is_old {
            (self.old_epoch, "OLD")
        } else {
            (self.local_epoch, "nursery")
        };
        // Compare in the handle's truncated GEN width — see `epoch_in_gen_width`.
        let expected = Self::epoch_in_gen_width(expected);
        debug_assert!(
            generation == expected,
            "use-after-GC: {} handle ({} slot {}) is from epoch {}, but that generation is \
             now epoch {} — a handle held across a collection without being re-rooted \
             (handle {:#x}). [current JIT arm: '{}']",
            what,
            space,
            index,
            generation,
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
        let handle = v.heap_handle().filter(|handle| handle.region() == LOCAL)?;
        let expected = Self::epoch_in_gen_width(if handle.is_old() {
            self.old_epoch
        } else {
            self.local_epoch
        });
        (handle.generation() != expected).then_some((
            handle.kind.name(),
            handle.generation(),
            expected,
        ))
    }

    /// Is `v` a handle whose slab index is **out of bounds** for its region's slab — i.e.
    /// garbage read from a freed/wrong location (a recycled roots buffer, an unspilled
    /// register that went stale across a collection)? Catches bug-#2 garbage that
    /// `dbg_value_stale` misses (the garbage's region/epoch bits don't read as a clean
    /// LOCAL-stale handle). Returns `Some((kind, index, slab_len))` if OOB.
    pub fn dbg_value_oob(&self, v: Value) -> Option<(&'static str, usize, usize)> {
        // Only LOCAL (nursery/old) — the bug-#2 garbage is young/local; PRELUDE/RUNTIME
        // are stable boxcar slabs (different len API), skipped.
        let handle = v.heap_handle().filter(|handle| handle.region() == LOCAL)?;
        let len = if handle.is_old() {
            self.old_opt().map_or(0, |old| old.len_of(handle.kind))
        } else {
            self.local.len_of(handle.kind)
        };
        (handle.index() >= len).then_some((handle.kind.slab().name(), handle.index(), len))
    }

    // ===== Accessors — read LOCAL/PRELUDE/RUNTIME values =======================

    /// RUNTIME generation `g`'s slabs, borrowed for as long as `&self` — no refcount bump.
    ///
    /// ADR-224's reasoning, extended to every RUNTIME read: a `get` on a
    /// `def`'d map dereferences one trie node per level, and cloning the generation's `Arc`
    /// there would cost two contended atomics per level.
    ///
    /// **Why the borrow is sound on its own terms** (not merely because ADR-091 says a
    /// generation this process references is never freed): the only thing that ever drops the
    /// cached `Arc` is this function's refresh, and it does not drop it — it moves the
    /// replaced `Arc` to `gen_cache_retired`, which is emptied only by
    /// [`release_retired_gens`](Self::release_retired_gens) under `&mut self`. A `&'a CodeSlabs`
    /// handed out here borrows `&'a self`, so no `&mut self` can run while it lives, so the
    /// `Arc` owning its target is alive for all of `'a` — whatever the drain protocol does.
    /// A drain bug (KI-188's class) therefore still reads stale code, never freed memory.
    #[inline]
    fn code_gen_ref(&self, g: usize) -> &CodeSlabs {
        let ver = self.runtime.gen_version.load(Ordering::Acquire);
        if self.gen_cache_ver[g].get() != ver {
            // First read, or generation `g`'s `Arc` was replaced — reload and re-stamp.
            let fresh = self.runtime.gens[g].load_full();
            #[cfg(feature = "jit")]
            self.gen_slabs_addr[g].set(Arc::as_ptr(&fresh) as usize);
            if let Some(old) = self.gen_cache[g].borrow_mut().replace(fresh) {
                self.gen_cache_retired.borrow_mut().push(old);
            }
            self.gen_cache_ver[g].set(ver);
        }
        let ptr: *const CodeSlabs = Arc::as_ptr(
            self.gen_cache[g]
                .borrow()
                .as_ref()
                .expect("gen cache populated on the version miss above"),
        );
        // SAFETY: `ptr` is the target of the `Arc` in `gen_cache[g]`, which is only ever
        // replaced above, and a replaced `Arc` is kept in `gen_cache_retired` until
        // `release_retired_gens(&mut self)` — impossible while the returned `&self`-borrow
        // lives. So the target outlives the borrow.
        unsafe { &*ptr }
    }

    /// After the callback has read RUNTIME vector `id`, record its `boxcar` bucket's base
    /// so native code reads the next vector in that bucket inline (`CodeSlabs::note_vector`).
    #[cfg(feature = "jit")]
    pub(crate) fn note_shared_vector(&self, id: VecId) {
        if id.region() == RUNTIME {
            self.code_gen_ref(id.code_gen()).note_vector(id.index());
        }
    }

    /// Drop the generation `Arc`s the cache replaced since the last call. `&mut self` is the
    /// whole soundness argument of [`code_gen_ref`](Self::code_gen_ref): no borrow it handed
    /// out can be live here. Called at collection.
    pub(crate) fn release_retired_gens(&mut self) {
        let retired = self.gen_cache_retired.get_mut();
        if !retired.is_empty() {
            retired.truncate(0);
            retired.shrink_to_fit();
        }
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
            // deref costs no atomic refcount traffic (see `code_gen_ref`).
            RUNTIME => *self
                .code_gen_ref(id.code_gen())
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
    region_ref!(vector, VecId, vectors, [Value], "runtime vector handle");
    region_ref!(map_node, MapId, maps, MapNode, "runtime map node");

    /// Resolve a closure handle to its `&Closure`. Hand-written (not via
    /// `region_ref!`) because the RUNTIME slab wraps each entry in a `OnceLock`
    /// (reserve-then-fill cycle break, see `CodeSlabs::closures`); the cell is
    /// always filled before its handle is published, so `get()` is infallible in
    /// practice.
    pub fn closure(&self, id: ClosureId) -> &Closure {
        match id.region() {
            LOCAL if id.is_old() => {
                local_gc_check!(old, self, id, "closure");
                &self.old().closures[id.index()]
            }
            LOCAL => {
                local_gc_check!(nursery, self, id, "closure");
                &self.local.closures[id.index()]
            }
            PRELUDE => &self.prelude.slabs.closures[id.index()],
            RUNTIME => self
                .code_gen_ref(id.code_gen())
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

    /// The items of a proper list (`nil` gives none), or `None` for an atom or an improper
    /// (dotted) list. [`Heap::list_to_vec`] without the error: for a walker that asks "is this
    /// a list?" of every node, an atom answered with a boxed error was an allocation per leaf.
    pub fn list_items(&self, v: Value) -> Option<Vec<Value>> {
        let mut out = Vec::new();
        let mut cur = v;
        loop {
            match cur.unpack() {
                ValueRef::Nil => return Some(out),
                ValueRef::Pair(p) => {
                    let (head, tail) = self.pair(p);
                    out.push(head);
                    cur = tail;
                }
                _ => return None,
            }
        }
    }

    /// The items of a vector or proper list, or `None` for anything else (unlike
    /// [`Heap::seq_items`], which also accepts ranges and sets).
    pub fn list_or_vector_items(&self, v: Value) -> Option<Vec<Value>> {
        match v.unpack() {
            ValueRef::Vector(id) => Some(self.vector(id).to_vec()),
            ValueRef::Nil | ValueRef::Pair(_) => self.list_to_vec(v).ok(),
            _ => None,
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
            // No `who` here — the primitive calling it is named by `call_native` — but the
            // value is: "expected a list or vector" alone left you to find which argument.
            _ => Err(LispError::type_err(format!(
                "expected a list, vector or set, got {} ({})",
                crate::core::value::tag(v).name(),
                crate::syntax::printer::print(self, v)
            ))),
        }
    }
}

/// KI-95: `promote` must copy shared (DAG) substructure ONCE, the way the GC's
/// flush path does (`flush_pair`/`flush_vector`/`flush_map` in `gc.rs`) — not once
/// per referrer. Immutable path-copying code produces shared substructure
/// routinely, and the RUNTIME region is append-only, so per-referrer copies are a
/// leak that compounds exponentially with nesting. The counts are asserted
/// directly against the RUNTIME slabs, per the KI-95 fix-shape note.

/// Test-only probe for the "a global `def` publishes under the promote lock" invariant
/// (see [`Heap::promote_rehome_publish`]). Armed by the test, checked from inside
/// [`Heap::env_define`]'s global arm at the instant just before the globals insert, and
/// compiled out entirely in every non-test build.
#[cfg(test)]
mod def_publish_probe {
    use std::sync::RwLock;
    use std::sync::atomic::{AtomicU8, Ordering};

    pub(super) const DISARMED: u8 = 0;
    pub(super) const ARMED: u8 = 1;
    /// The publish ran with NO read guard held — the TOCTOU window is open.
    pub(super) const SAW_UNGUARDED: u8 = 2;
    /// The publish ran inside the guard — aging cannot flip underneath it.
    pub(super) const SAW_GUARDED: u8 = 3;

    pub(super) static STATE: AtomicU8 = AtomicU8::new(DISARMED);

    /// A `try_write` fails exactly while some reader holds the lock. This thread is the
    /// only candidate reader (nothing else is running), so failure ⟺ our own guard is
    /// still held across the publish.
    pub(super) fn observe(lock: &RwLock<()>) {
        if STATE.load(Ordering::Relaxed) != ARMED {
            return;
        }
        let guarded = lock.try_write().is_err();
        STATE.store(
            if guarded { SAW_GUARDED } else { SAW_UNGUARDED },
            Ordering::Relaxed,
        );
    }
}

/// KI-193: every multi-name swap of the globals table — a module load's publish, an
/// `%isolate` restore — must bump `version` BEFORE its write guard drops, because every
/// process's global inline cache is keyed on it. [`begin`](table_swap_probe::begin) reads
/// `version` just before the guard is taken, [`end`](table_swap_probe::end) just after it
/// drops; the tests below require every swap to have moved it in between.
#[cfg(test)]
pub(super) mod table_swap_probe {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// One swap: `(version, code_epoch)` before the guard, and at its unlock.
    pub(crate) type Swap = ((u64, u64), (u64, u64));

    thread_local! {
        static SWAPS: RefCell<Vec<Swap>> = const { RefCell::new(Vec::new()) };
    }

    fn read(version: &AtomicU64, epoch: &AtomicU64) -> (u64, u64) {
        (
            version.load(Ordering::Relaxed),
            epoch.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn begin(version: &AtomicU64, epoch: &AtomicU64) {
        let v = read(version, epoch);
        SWAPS.with(|s| s.borrow_mut().push((v, v)));
    }

    pub(crate) fn end(version: &AtomicU64, epoch: &AtomicU64) {
        let v = read(version, epoch);
        SWAPS.with(|s| {
            if let Some(last) = s.borrow_mut().last_mut() {
                last.1 = v;
            }
        });
    }

    /// Every swap this thread made since the last call.
    pub(crate) fn take() -> Vec<Swap> {
        SWAPS.with(|s| std::mem::take(&mut *s.borrow_mut()))
    }
}

#[cfg(test)]
mod table_swap_tests {
    use super::table_swap_probe::{self, Swap};

    /// Every swap moved `version` — and `code_epoch` too, since each swap here rebinds a
    /// name a compiled arm may have cached — before its write guard dropped.
    fn assert_swaps_bumped_under_the_lock(what: &str, swaps: &[Swap]) {
        assert!(!swaps.is_empty(), "{what}: no table swap was observed");
        for &((v0, e0), (v1, e1)) in swaps {
            assert!(
                v1 > v0 && e1 > e0,
                "{what}: the globals write guard dropped before the caches' keys moved \
                 (version {v0} -> {v1}, code_epoch {e0} -> {e1}), so a reader could pair \
                 the new table with a stale cached global (KI-193)"
            );
        }
    }

    #[test]
    fn a_module_publish_bumps_the_cache_keys_before_the_table_unlocks() {
        let mut interp = crate::Interp::new();
        // Bound first, so the publish is a REBIND — as a real load's `*features*` is.
        interp.eval_str("(def ki191-published 0)").unwrap();
        table_swap_probe::take();
        interp
            .eval_str("(%with-load-journal (fn () (def ki191-published 1)))")
            .unwrap();
        assert_swaps_bumped_under_the_lock("publish", &table_swap_probe::take());
    }

    #[test]
    fn an_isolate_restore_bumps_the_cache_keys_before_the_table_unlocks() {
        let mut interp = crate::Interp::new();
        table_swap_probe::take();
        interp
            .eval_str("(%isolate (fn () (def ki191-restored 1)))")
            .unwrap();
        assert_swaps_bumped_under_the_lock("restore", &table_swap_probe::take());
    }
}

#[cfg(test)]
mod def_atomicity_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// Regression: `(def name value)` must promote, re-home **and** install the binding
    /// under a single `promote_lock` read guard.
    ///
    /// The store publishes a shared GC root. If the lock is dropped between the re-home
    /// (which validates "this handle is in the current generation") and the insert, an
    /// aging flip + `migrate_live_globals` can complete in the gap — leaving the binding
    /// pinned to a generation that is already draining and about to be freed (a dangling
    /// global), or letting migration's reconcile mistake a fresh rebind for the stale
    /// value it snapshotted and overwrite it (a silently reverted `def`). Both are
    /// invisible until they aren't, so the invariant is asserted structurally, from
    /// inside the window itself.
    #[test]
    fn global_def_installs_the_binding_under_the_promote_lock() {
        let mut interp = crate::Interp::new();
        def_publish_probe::STATE.store(def_publish_probe::ARMED, Ordering::Relaxed);
        interp.eval_str("(def probe-atomicity 41)").expect("def");
        let state = def_publish_probe::STATE.swap(def_publish_probe::DISARMED, Ordering::Relaxed);
        assert_ne!(
            state,
            def_publish_probe::ARMED,
            "the probe never fired — `env_define`'s global arm no longer publishes \
             through `promote_rehome_publish`",
        );
        assert_eq!(
            state,
            def_publish_probe::SAW_GUARDED,
            "a global `def` installed its binding with the promote lock RELEASED: an \
             aging flip can land between the re-home and the insert (dangling global / \
             silently reverted def)",
        );
    }

    /// The companion for `declared_sigs`, the other shared root published this way.
    /// Same window, same consequence — a `(sig …)` type-expression stranded on a
    /// draining generation is read by the checker long after the generation is gone.
    #[test]
    fn declared_sig_installs_under_the_promote_lock() {
        let mut interp = crate::Interp::new();
        let heap = &interp.heap;
        let sym = crate::core::value::intern("probe-sig-atomicity");
        let observed = heap.promote_rehome_publish(Value::int(7), |h, _shared| {
            h.runtime.promote_lock.try_write().is_err()
        });
        assert!(
            observed,
            "`promote_rehome_publish` released the promote lock before running its \
             publish step — the TOCTOU window it exists to close is open",
        );
        // And the real caller still stores what it promised to.
        interp.heap.set_declared_sig(sym, Value::int(7));
        assert_eq!(
            interp.heap.declared_sig_value(sym).and_then(|v| v.as_int()),
            Some(7),
        );
    }
}

#[cfg(test)]
mod handle_debug_tests {
    use super::*;
    use crate::core::value::{BigIntId, BytesId};

    /// `dbg_value_stale` and `dbg_value_oob` (the `BROOD_JIT_VERIFY` / `BROOD_GC_VERIFY`
    /// scans) ask their question of every handle kind, through `Value::heap_handle`. Each
    /// used to keep its own switch: the stale check skipped bigints, bytes, closures and
    /// five more kinds, and the bounds check knew four slabs, so garbage of any other kind
    /// sailed through both.
    #[test]
    fn the_handle_debug_checks_see_every_kind() {
        let mut heap = Heap::new();
        let Value::BigInt(id) = heap.alloc_bigint(num_bigint::BigInt::from(1u8) << 100) else {
            panic!("a bigint allocates as a bigint handle")
        };
        let live = Heap::epoch_in_gen_width(heap.local_epoch);
        let stale = Value::BigInt(BigIntId::local_gen(id.index(), live ^ 1));
        assert_eq!(
            heap.dbg_value_stale(stale),
            Some(("bigint", live ^ 1, live))
        );
        assert_eq!(heap.dbg_value_stale(Value::BigInt(id)), None);

        let past_the_slab = Value::Bytes(BytesId::local_gen(10_000, live));
        assert_eq!(
            heap.dbg_value_oob(past_the_slab),
            Some(("bytes", 10_000, 0))
        );
    }
}

/// Byte offsets native code reads a RUNTIME vector through (`jit_lower/emit.rs`,
/// `shared_vec_slot`): the heap's cached generation versions and `CodeSlabs` addresses, the
/// live-version address, and the bucket table inside a `CodeSlabs`.
#[cfg(feature = "jit")]
pub(crate) mod rt_vec_offsets {
    use super::{Heap, slabs::CodeSlabs};
    /// `Heap::gen_cache_ver` — `[Cell<u64>; 2]`, laid out as `[u64; 2]`.
    pub(crate) const GEN_CACHE_VER: i32 = std::mem::offset_of!(Heap, gen_cache_ver) as i32;
    /// `Heap::gen_slabs_addr` — `[Cell<usize>; 2]`, laid out as `[usize; 2]`.
    pub(crate) const GEN_SLABS_ADDR: i32 = std::mem::offset_of!(Heap, gen_slabs_addr) as i32;
    /// `Heap::rt_gen_version` — the address of the runtime's live `gen_version`.
    pub(crate) const RT_GEN_VERSION: i32 = std::mem::offset_of!(Heap, rt_gen_version) as i32;
    /// `CodeSlabs::vec_buckets` — `[AtomicUsize; RT_VEC_BUCKETS]`.
    pub(crate) const VEC_BUCKETS: i32 = std::mem::offset_of!(CodeSlabs, vec_buckets) as i32;
}
