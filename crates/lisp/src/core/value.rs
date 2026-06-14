//! The core value type, [`Value`], plus the handle types that address the
//! per-process [`Heap`](crate::core::heap::Heap).
//!
//! After the step-2 migration (see `docs/memory-model.md`), `Value` is `Copy`:
//! its heap variants are small integer **handles** into a `Heap`, not `Rc`
//! pointers. Reading or allocating a heap object goes through the `Heap`. The
//! payoff: a `Heap` is plain `Vec`s of data, so it is `Send` — a process can be
//! moved between scheduler threads — and it gives us one place to do GC.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, RwLock};

use smallvec::SmallVec;

use crate::core::heap::Heap;
use crate::error::LispResult;

/// An interned symbol name (a `u32` id; the spelling lives in a global table).
pub type Symbol = u32;

// The process-wide symbol table, split so reads never take a lock:
//
// - `NAMES` (id -> spelling) is **append-only and never mutated**, so it's a
//   lock-free `boxcar::Vec`: any thread reads `NAMES[id]` without locking, and
//   pushed entries never move (stable refs) — the same structure the shared
//   RUNTIME code region uses. The hot readers go through here (`symbol_name` in
//   the printer, `symbol_is` in the compile-pass walk), so symbol spelling and
//   comparison no longer serialise every scheduler thread through one mutex.
// - `IDS` (spelling -> id) is read and extended only by `intern`, so it stays
//   behind a `Mutex`; the lock is held across the `NAMES` push so the two tables
//   agree on each new id (two threads can't mint different ids for one name).
//
// Symbol ids are consistent across scheduler threads — a prerequisite for
// sending symbols between process heaps.
static NAMES: LazyLock<boxcar::Vec<String>> = LazyLock::new(boxcar::Vec::new);
static IDS: LazyLock<Mutex<HashMap<String, Symbol>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

// Recover from a poisoned `IDS` lock rather than letting one panicking thread
// wedge symbol interning everywhere (the tables are append-only, so a recovered
// guard is consistent).
fn ids() -> MutexGuard<'static, HashMap<String, Symbol>> {
    IDS.lock().unwrap_or_else(|e| e.into_inner())
}

thread_local! {
    // Per-thread name→id cache in front of the global `IDS` mutex. Symbol ids are
    // append-only and globally consistent, so a cached id is valid forever; the
    // global tables stay the sole *minter* of new ids. This keeps the hot path —
    // re-interning an already-known name, which is the overwhelmingly common case
    // (call heads, keywords, record fields) — a thread-local lookup with no global
    // lock. Before this, every intern (even a hit) took the `IDS` mutex, which
    // serialized parallel workers: under allocation-heavy fan-out the interner lock
    // was the dominant contention point (~10% in `lock_contended` plus the futex
    // waits behind it), not the heap allocator. Bounded memory: the symbol set is
    // finite, so each thread caches at most every name it ever interns.
    static CACHE: RefCell<HashMap<String, Symbol>> = RefCell::new(HashMap::new());
}

pub fn intern(name: &str) -> Symbol {
    if let Some(id) = CACHE.with(|c| c.borrow().get(name).copied()) {
        return id;
    }
    let id = intern_global(name);
    CACHE.with(|c| c.borrow_mut().insert(name.to_string(), id));
    id
}

// The global intern: mint or fetch the id under the `IDS` mutex. Reached only on a
// thread-local cache miss (first time *this thread* sees `name`).
fn intern_global(name: &str) -> Symbol {
    let mut ids = ids();
    if let Some(&id) = ids.get(name) {
        return id;
    }
    // A new name: its index in the append-only `NAMES` vec *is* its id. Pushing
    // while holding the `IDS` lock keeps a single writer, so ids stay dense and
    // the two tables never disagree. `NAMES` and `IDS` each own their own copy of
    // the name (the `clone` below), so a new name costs two `String` allocations
    // — only on the cold intern-a-new-name path.
    let owned = name.to_string();
    let id = NAMES.push(owned.clone()) as Symbol;
    ids.insert(owned, id);
    id
}

pub fn symbol_name(sym: Symbol) -> String {
    NAMES.get(sym as usize).expect("interned symbol id").clone()
}

/// Borrowed spelling of `sym` — a `&'static str` straight into the append-only,
/// never-freed `NAMES` table (stable refs, so it's valid for the life of the
/// process). Unlike [`symbol_name`] it allocates **nothing**: use it for
/// transient inspection — compare, `contains`/`starts_with`, push into a buffer,
/// `format!` — which is the hot-path shape in the printer and the compile/macro
/// walk. Reach for [`symbol_name`] only when an owned `String` must outlive the
/// table (stored in a `Value`, returned across an API boundary, collected).
pub fn symbol_name_ref(sym: Symbol) -> &'static str {
    NAMES
        .get(sym as usize)
        .expect("interned symbol id")
        .as_str()
}

/// Look up an existing interned symbol without inserting one. Returns `None` if
/// the name has never been interned in this process. For cold-path checks (e.g.
/// `dist::connect`'s pre-dial de-dup) that don't want to grow the interner with
/// a name that may never be used as a value.
pub fn intern_existing(name: &str) -> Option<Symbol> {
    ids().get(name).copied()
}

/// Does `sym`'s spelling equal `name`? A lock-free read + in-place compare — no
/// `String` allocation, unlike `symbol_name(s) == name`. For the hot compares
/// against fixed words (`&optional`, `quasiquote`, the compile-pass walk).
pub fn symbol_is(sym: Symbol, name: &str) -> bool {
    NAMES
        .get(sym as usize)
        .expect("interned symbol id")
        .as_str()
        == name
}

/// The first character of `sym`'s spelling, if any — to recognise the `&`-marker
/// family without allocating the whole name first.
pub fn symbol_first_char(sym: Symbol) -> Option<char> {
    NAMES
        .get(sym as usize)
        .expect("interned symbol id")
        .chars()
        .next()
}

// ----- dynamic-variable registry ---------------------------------------------
//
// Which symbols are *dynamic variables* (declared by `defdyn`). A monotonic,
// process-wide declaration fact — like interning, not per-runtime state — so it
// lives in a `static` rather than the runtime's global table. Reads never touch
// this set (a dynamic value resolves through the per-process binding stack in
// `Heap`); it exists only so `binding` can reject an undeclared var and so
// `dynamic?` can report. See `docs/language.md` (Dynamic variables).

static DYNAMICS: LazyLock<RwLock<HashSet<Symbol>>> = LazyLock::new(|| RwLock::new(HashSet::new()));

/// Mark `sym` as a dynamic variable (idempotent). Called by `defdyn`.
pub fn mark_dynamic(sym: Symbol) {
    DYNAMICS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .insert(sym);
}

/// Has `sym` been declared dynamic with `defdyn`?
pub fn is_dynamic(sym: Symbol) -> bool {
    DYNAMICS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&sym)
}

// ----- handles into the Heap -----

/// A handle is a packed `u64`: the two top bits tag the heap **region**, the
/// next 30 bits are a **generation** stamp, and the low 32 bits are the slab
/// index (≈4 billion objects per region — ample). See `docs/shared-code.md`.
///
/// - [`LOCAL`] — the process's own data heap (mutable, per-process).
/// - [`PRELUDE`] — the immutable prelude/builtins, shared by *all* runtimes.
/// - [`RUNTIME`] — a runtime's mutable, append-only code region, shared by all
///   of that runtime's inner (spawned) processes. This is where `def`'d code
///   and the global bindings live, so an update is visible to running processes.
///
/// **Generation stamp (ADR-054 / `docs/memory-review.md`).** A LOCAL handle
/// carries the heap's *epoch* at the moment it was minted. Every per-process
/// arena flip (the automatic copying collector [`Heap::collect`], or the
/// [`Heap::flush`] helper) bumps that
/// epoch and re-mints the survivors, so a handle held across a flip without
/// being re-rooted carries a *stale* epoch. The LOCAL accessors in `heap.rs`
/// `debug_assert!` the stamp matches the current epoch, turning use-after-flip
/// (a moved object) into a precise panic **at the bad deref** instead of a
/// far-away out-of-bounds index or a silent wrong-slot read. PRELUDE/RUNTIME
/// handles never move, so their stamp is always 0 and is not checked. The width
/// is free: `Value` already carries 8-byte payloads (`Int`/`Float`/`Ref`), so a
/// `u64` handle doesn't grow it. **Equality/hashing ignore the stamp** — two
/// handles to the same region+index are the same object regardless of epoch.
pub const REGION_SHIFT: u32 = 62;
/// Bit 61: the **generation age** of a LOCAL handle — `0` = young (nursery), `1` =
/// old (tenured). Stolen from the top of the old 30-bit gen field (now 29 bits),
/// so the generational collector can tell, from the handle alone, which LOCAL
/// space a slot lives in — without a boundary scan that a *stale* handle could
/// fool. Meaningless (always 0) for PRELUDE/RUNTIME, which don't move.
pub const AGE_SHIFT: u32 = 61;
pub const GEN_SHIFT: u32 = 32;
/// Low 32 bits: the slab index.
pub const INDEX_MASK: u64 = (1u64 << GEN_SHIFT) - 1;
/// 29 bits between the index and the age bit: the generation stamp (epoch). One
/// bit narrower than before to make room for [`AGE_SHIFT`]; 2^29 epochs is ample
/// for a *debug-only* stale-deref tripwire (a collision needs that many flips of
/// one space between a handle's mint and its stale use).
pub const GEN_MASK: u64 = (1u64 << (AGE_SHIFT - GEN_SHIFT)) - 1;
/// The age bit, pre-shifted — OR'd into an old (tenured) LOCAL handle.
pub const AGE_OLD: u64 = 1u64 << AGE_SHIFT;
pub const LOCAL: u8 = 0b00;
pub const PRELUDE: u8 = 0b01;
pub const RUNTIME: u8 = 0b10;
/// The two region bits `0b11` are **reserved as un-mintable**: [`EnvId::GLOBAL`]
/// is `u64::MAX`, whose region field is `0b11`, so it round-trips through
/// `canonical()` as a distinct sentinel that no real handle can collide with.
/// Every handle constructor debug-asserts its region stays below this — a guard
/// against a future fourth region quietly stealing `0b11` and aliasing GLOBAL.
pub const REGION_RESERVED: u8 = 0b11;

macro_rules! handle {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug)]
        pub struct $name(pub u64);
        impl $name {
            /// A LOCAL handle with no generation stamp (epoch 0) — the prelude
            /// build and any caller that doesn't track epochs. Runtime
            /// allocations use [`local_gen`](Self::local_gen) to stamp the
            /// heap's current epoch.
            #[inline]
            pub fn local(index: usize) -> Self {
                Self::local_gen(index, 0)
            }
            /// A LOCAL handle stamped with generation `gen` (the allocating
            /// heap's current epoch). The debug-assert catches a slab growing
            /// past `2^32` (where the index would collide with the gen bits).
            #[inline]
            pub fn local_gen(index: usize, gen: u32) -> Self {
                debug_assert!(
                    index < (1usize << GEN_SHIFT),
                    "handle index {} overflows the 32-bit index field",
                    index,
                );
                let h = $name((index as u64) | (((gen as u64) & GEN_MASK) << GEN_SHIFT));
                debug_assert!(
                    h.region() < REGION_RESERVED,
                    "region 0b11 is reserved for EnvId::GLOBAL"
                );
                h
            }
            /// A LOCAL handle in the **old (tenured) generation**, stamped with the
            /// old-space epoch `gen`. Same index space as the nursery but the
            /// [`AGE_OLD`] bit routes accessors / `check_epoch` to the old slabs.
            #[inline]
            pub fn local_old_gen(index: usize, gen: u32) -> Self {
                debug_assert!(
                    index < (1usize << GEN_SHIFT),
                    "handle index {} overflows the 32-bit index field",
                    index,
                );
                let h = $name((index as u64) | (((gen as u64) & GEN_MASK) << GEN_SHIFT) | AGE_OLD);
                debug_assert!(
                    h.region() < REGION_RESERVED,
                    "region 0b11 is reserved for EnvId::GLOBAL"
                );
                h
            }
            /// A handle into the immutable shared prelude region (no generation).
            #[inline]
            pub fn prelude(index: usize) -> Self {
                debug_assert!(
                    index < (1usize << GEN_SHIFT),
                    "prelude index {} overflows",
                    index
                );
                debug_assert!(
                    PRELUDE < REGION_RESERVED,
                    "region 0b11 is reserved for EnvId::GLOBAL"
                );
                $name((index as u64) | ((PRELUDE as u64) << REGION_SHIFT))
            }
            /// A handle into the runtime's mutable shared code region (no generation).
            #[inline]
            pub fn runtime(index: usize) -> Self {
                debug_assert!(
                    index < (1usize << GEN_SHIFT),
                    "runtime index {} overflows",
                    index
                );
                debug_assert!(
                    RUNTIME < REGION_RESERVED,
                    "region 0b11 is reserved for EnvId::GLOBAL"
                );
                $name((index as u64) | ((RUNTIME as u64) << REGION_SHIFT))
            }
            /// Which region this handle addresses ([`LOCAL`]/[`PRELUDE`]/[`RUNTIME`]).
            #[inline]
            pub fn region(self) -> u8 {
                (self.0 >> REGION_SHIFT) as u8
            }
            /// The slab index, with the region tag and generation masked off.
            #[inline]
            pub fn index(self) -> usize {
                (self.0 & INDEX_MASK) as usize
            }
            /// The generation stamp (the heap epoch this LOCAL handle was minted
            /// in; 0 for PRELUDE/RUNTIME). Checked by the LOCAL accessors.
            #[inline]
            pub fn generation(self) -> u32 {
                ((self.0 >> GEN_SHIFT) & GEN_MASK) as u32
            }
            /// Whether a LOCAL handle addresses the **old (tenured)** generation
            /// ([`AGE_OLD`]). Only meaningful when `region() == LOCAL`; always
            /// `false` for PRELUDE/RUNTIME (their age bit is 0).
            #[inline]
            pub fn is_old(self) -> bool {
                (self.0 >> AGE_SHIFT) & 1 == 1
            }
            /// Region + index with the generation cleared — the identity used for
            /// equality and hashing, so a handle compares equal to itself across
            /// epochs (same object) while the stamp still flags stale *derefs*.
            #[inline]
            fn canonical(self) -> u64 {
                self.0 & !(GEN_MASK << GEN_SHIFT)
            }
        }
        impl PartialEq for $name {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                self.canonical() == other.canonical()
            }
        }
        impl Eq for $name {}
        impl ::core::hash::Hash for $name {
            #[inline]
            fn hash<H: ::core::hash::Hasher>(&self, state: &mut H) {
                self.canonical().hash(state);
            }
        }
    };
}
handle!(PairId);
handle!(VecId);
handle!(StrId);
handle!(BigIntId);
handle!(TransientId);
handle!(RopeId);
handle!(ClosureId);
handle!(NativeId);
handle!(MapId);
handle!(EnvId);

impl EnvId {
    /// The runtime's global scope. Not a real frame — a sentinel that the
    /// environment routines special-case to the shared global bindings table
    /// (`RuntimeCode::globals`). A local frame chain bottoms out here, and a
    /// top-level closure captures it symbolically (`Closure.env == None`), so a
    /// shared closure resolves globals against whichever process runs it.
    ///
    /// **Encoding.** `u64::MAX` sets both region bits to `0b11`, an otherwise
    /// undefined region — `LOCAL` / `PRELUDE` / `RUNTIME` are `0b00` / `0b01`
    /// / `0b10`. This is the marker `Heap::env_frame` and the env walkers
    /// short-circuit on (`env == EnvId::GLOBAL`) before touching the region
    /// dispatch; a stray dispatch on a GLOBAL panics with a clear message
    /// (see `Heap::env_frame`), not the `_ => unreachable!()` fall-through.
    /// (`u64::MAX` survives the gen-masked equality — its region+index bits are
    /// all-ones, which no real handle produces.)
    pub const GLOBAL: EnvId = EnvId(u64::MAX);
}

/// A Brood value. `Copy`: primitives inline, heap objects as handles.
///
/// `#[repr(C, u8)]` is **load-bearing for the JIT** (ADR-101 / `docs/jit-stage1.md` §2):
/// it pins a stable tagged-union layout — a `u8` discriminant at offset 0 (in
/// *declaration order*: `Nil`=0, `Bool`=1, `Int`=2, …) and the payload 8-aligned at
/// offset 8 — so JIT'd code can read a `Value` out of a `Heap::roots` slot, tag-check
/// it, and extract the `i64`/`f64` without a runtime callback. It grows `Value` to 24
/// bytes, which `docs/value-repr.md` measured perf-neutral for the operand stack. The
/// layout is pinned by `value_layout_is_stable_for_the_jit` — treat a change as an ABI
/// break the JIT codegen must follow.
#[derive(Clone, Copy, Debug)]
#[repr(C, u8)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    /// An arbitrary-precision integer that does **not** fit in `i64` — a heap
    /// leaf handle into the per-process `bigints` slab (mirrors [`Value::Str`]).
    /// The **normalize invariant**: a `BigInt` is *always* strictly outside
    /// `[i64::MIN, i64::MAX]`; any operation that produces one in range returns
    /// a `Value::Int` instead (see `Heap::int_from_bigint`). Consequence: an
    /// `Int` and a `BigInt` are never numerically equal (disjoint ranges).
    /// Transparently an integer to the language — [`tag`] maps it to
    /// [`Tag::Int`], so `int?`/`number?`/`type-of` all treat it as `int`.
    BigInt(BigIntId),
    Float(f64),
    Sym(Symbol),
    Keyword(Symbol),
    Str(StrId),
    /// A text **rope** — the editor's buffer storage (ADR-045). Backed by a
    /// `ropey::Rope` (an `Arc`-shared B-tree); immutable like every Brood value,
    /// so every editing primitive (`rope-insert`/`rope-delete`) returns a *fresh*
    /// rope that structurally shares the unchanged parts. Process-local: a rope
    /// lives in exactly one process's heap and never crosses in a message — its
    /// content moves as a string via `rope->string` (mirrors how a `Pid` is the
    /// handle, not the process). The one heap object kind that wraps a Rust
    /// crate's structure rather than being built from `Value`s.
    Rope(RopeId),
    /// A cons cell. Proper lists are pairs chained to a final `Nil`.
    Pair(PairId),
    Vector(VecId),
    /// A lazy integer range `lo..hi` by `step`, backed by a 3-element
    /// `[lo hi step]` vector (so it rides the `Vector` GC / region / forwarding
    /// machinery for free). It is never materialised: `fold` / `reduce` / `sum`
    /// / `count` iterate it in a counted loop with zero allocation, while every
    /// other op realises it to a list on demand. [`tag`] reports it as a
    /// [`Tag::Pair`] so the type system and `type-of` treat it exactly like the
    /// list it stands in for. Constructed only by `%range`, which returns `Nil`
    /// for an empty range — so a `Range` always has at least one element.
    Range(VecId),
    /// An immutable map (`{ }`): key→value associations. Insertion-ordered; keys
    /// compared by structural equality, so any value can be a key. Every
    /// operation (`assoc`/`dissoc`) returns a *fresh* map (ADR-026 immutability).
    Map(MapId),
    /// A closure (`fn`).
    Fn(ClosureId),
    /// A macro — same `Closure` storage, invoked on unevaluated forms.
    Macro(ClosureId),
    /// A builtin implemented in Rust.
    Native(NativeId),
    /// A unique, opaque reference token from `(ref)` — a fresh monotonic id, the
    /// only way to make one. Distinct from `Int` so a reply tagged with a ref can
    /// never be confused with a pid or a user integer (Erlang's `make_ref`). Sent
    /// by value across processes; compared by identity.
    Ref(u64),
    /// A process identifier, carrying **node identity** (`node`, an interned node
    /// name) alongside the process-local id. A *local* pid carries this node's
    /// name; a *remote* pid (received from a peer) carries the peer's. The same
    /// value addresses a process whether it's here or across a node link —
    /// `send` dispatches on `node` (see `crate::dist`). Compared by value.
    Pid {
        node: Symbol,
        id: u64,
    },
    /// A TCP socket — an id into the global socket registry (`crate::net`). Like a
    /// `Pid`/`Ref` it is a scalar handle, not a heap object: the GC never traces or
    /// moves it, and it carries a live OS resource. Process-local mechanism (the
    /// owning process drives it via the non-blocking `tcp-*` primitives); **never**
    /// sent across processes. The TLS counterpart reuses this same handle.
    Socket(u64),
    /// A child process — an id into the global subprocess registry
    /// (`crate::proc`). Like a `Socket` it is a scalar handle, not a heap object:
    /// the GC never traces or moves it, and it carries a live OS resource (a spawned
    /// process plus its stdin pipe and stdout/stderr reader threads). Process-local
    /// mechanism (the owning process drives it via the `proc-*` primitives and
    /// receives its output as mailbox messages); **never** sent across processes.
    Subprocess(u64),
    /// An **in-memory table** (Brood's ETS, ADR-107) — an id into the global table
    /// registry (`crate::table`). Like a `Socket`/`Subprocess` it is a scalar handle,
    /// not a heap object: the GC never traces or moves it. **Unlike** them it is
    /// *shared, not process-local* — the handle is sent by value across processes and
    /// every copy indexes the same shared store (the way a `Pid` names one shared
    /// process). The store holds deep clones in `Message` form, so it is genuine
    /// mutable state behind primitives (`table-*`), never a mutable `Value`. Local to
    /// this runtime (the registry is global to the OS process); not node-portable.
    Table(u64),
    /// A **transient map** — Clojure's `(transient m)` / `assoc!` / `persistent!`
    /// fast-building handle into the per-process `transients` slab. A heap object
    /// holding a [`crate::core::heap::TransientCell`]: a mutable `root` (a `Map`),
    /// a build **watermark**, the LOCAL **epoch** the watermark is valid in, and a
    /// `live` flag. Identity-mutable (unlike every other Value): `assoc!`/`dissoc!`
    /// rewrite the cell in place and return *the same handle*, mutating nodes the
    /// transient owns rather than path-copying. Process-local — never frozen into
    /// PRELUDE/RUNTIME, never sent across processes. `type-of` is `:transient`; it
    /// is **not** a `map?`. See `Heap::transient` and the epoch guard.
    Transient(TransientId),
}

/// The runtime type tags — the discriminant of [`Value`] made first-class, so it
/// can be named (`type-of`), reported in self-identifying type errors, and used
/// as the base of the (future, advisory) inference lattice. This *is* Brood's
/// entire type universe; the language has no other types. Names mirror the
/// `int?`/`string?`/… predicates (`Sym` → `symbol`, `Str` → `string`).
///
/// `#[repr(u8)]` is load-bearing: `Tag as u8` is the bit position of this tag in
/// a [`crate::types::Ty`] set, so the *declaration order is the lattice bit
/// order*. Adding a variant just extends the universe; reordering renumbers the
/// bits (harmless — `Ty` values aren't persisted — but keep `types::ALL_TAGS` in
/// the same order, which a test checks).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tag {
    Nil,
    Bool,
    Int,
    Float,
    Sym,
    Keyword,
    Str,
    Pair,
    Vector,
    Fn,
    Macro,
    Native,
    Map,
    Ref,
    Pid,
    Rope,
    Socket,
    Subprocess,
    Transient,
    Table,
}

impl Tag {
    /// The canonical name — the `type-of` keyword spelling and the word used in
    /// type-error messages.
    pub fn name(self) -> &'static str {
        match self {
            Tag::Nil => "nil",
            Tag::Bool => "bool",
            Tag::Int => "int",
            Tag::Float => "float",
            Tag::Sym => "symbol",
            Tag::Keyword => "keyword",
            Tag::Str => "string",
            Tag::Pair => "pair",
            Tag::Vector => "vector",
            Tag::Fn => "fn",
            Tag::Macro => "macro",
            Tag::Native => "native",
            Tag::Map => "map",
            Tag::Ref => "ref",
            Tag::Pid => "pid",
            Tag::Rope => "rope",
            Tag::Socket => "socket",
            Tag::Subprocess => "subprocess",
            Tag::Transient => "transient",
            Tag::Table => "table",
        }
    }

    /// The `type-of` keyword *id* for this tag, interned **once** and cached.
    /// `type-of` (and the seq predicates built on it — `pair?`/`empty?`/… hit it
    /// per element) used to re-intern the tag name on every call; on map/list-heavy
    /// code that was essentially the entire `intern` cost (~98% of all interns were
    /// a tag name like `"pair"`). Indexed by the `#[repr(u8)]` discriminant.
    pub fn keyword(self) -> Symbol {
        static KW: LazyLock<[Symbol; 20]> = LazyLock::new(|| {
            const TAGS: [Tag; 20] = [
                Tag::Nil, Tag::Bool, Tag::Int, Tag::Float, Tag::Sym, Tag::Keyword,
                Tag::Str, Tag::Pair, Tag::Vector, Tag::Fn, Tag::Macro, Tag::Native,
                Tag::Map, Tag::Ref, Tag::Pid, Tag::Rope, Tag::Socket, Tag::Subprocess,
                Tag::Transient, Tag::Table,
            ];
            let mut out = [0u32; 20];
            for t in TAGS {
                out[t as usize] = intern(t.name());
            }
            out
        });
        KW[self as usize]
    }
}

/// The runtime [`Tag`] of `v` — the canonical discriminant of [`Value`]. The one
/// place the value-to-tag mapping lives.
pub fn tag(v: Value) -> Tag {
    match v {
        Value::Nil => Tag::Nil,
        Value::Bool(_) => Tag::Bool,
        Value::Int(_) => Tag::Int,
        // A BigInt is transparently an integer — no lattice change (ADR bignums).
        Value::BigInt(_) => Tag::Int,
        Value::Float(_) => Tag::Float,
        Value::Sym(_) => Tag::Sym,
        Value::Keyword(_) => Tag::Keyword,
        Value::Str(_) => Tag::Str,
        Value::Pair(_) => Tag::Pair,
        // A range stands in for the list it would materialise to — report it as a
        // pair so `type-of`, the type lattice, and every `pair?`/list check treat
        // it identically. (The fold-family fast path matches `Value::Range`
        // directly in Rust, not via this tag.)
        Value::Range(_) => Tag::Pair,
        Value::Vector(_) => Tag::Vector,
        Value::Fn(_) => Tag::Fn,
        Value::Macro(_) => Tag::Macro,
        Value::Native(_) => Tag::Native,
        Value::Map(_) => Tag::Map,
        Value::Ref(_) => Tag::Ref,
        Value::Pid { .. } => Tag::Pid,
        Value::Rope(_) => Tag::Rope,
        Value::Socket(_) => Tag::Socket,
        Value::Subprocess(_) => Tag::Subprocess,
        Value::Transient(_) => Tag::Transient,
        Value::Table(_) => Tag::Table,
    }
}

/// One arity-clause of a [`Closure`]: a parameter list plus the body run when the
/// call's argument count selects this arm. A single-arity `fn`/`defn` has exactly
/// one arm; a **multi-arity** one (e.g. `(fn (() 0) ((a) a) ((a b) (%add a b))
/// ((a b & more) …))`) has one arm per clause, dispatched by argument count in
/// `bind_params` (Clojure-style — each fixed arm binds its params *directly*, no
/// rest-list, so the common small-arity call is cheap). Only *arity* clauses —
/// plain symbol params plus optional `&optional`/`&` rest — become arms; clauses
/// with literal/destructuring *patterns* (e.g. `((3 _) …)`) are lowered to the
/// `match*` engine instead (see `eval::macros::lower_fn`).
#[derive(Clone, Default)]
pub struct ClosureArm {
    pub params: Vec<Symbol>,
    pub optionals: Vec<(Symbol, Value)>,
    pub rest: Option<Symbol>,
    pub body: Vec<Value>,
    /// Precomputed thin-wrapper analysis (perf). `Some` when this arm is a pure
    /// pass-through — no `&optional`/`&` rest and a single body form
    /// `(head p_i p_j …)` whose arguments are all the arm's own parameters used
    /// directly — so a call can redirect straight to `head` on the already-bound
    /// `argv`, skipping the scope alloc + param bind + body walk. Computed once at
    /// closure-allocation time (`Heap::alloc_closure`) and carried verbatim across
    /// promote/freeze/message copies, since it's a pure function of the immutable
    /// arm. `None` for any arm that isn't a redirectable wrapper. This is what
    /// keeps the prelude operator wrappers (`(+ a b)` → `(%add a b)`) cheap without
    /// re-deriving the forwarding map on every call (see `eval::passthrough_arm`).
    pub passthrough: Option<Passthrough>,
}

/// A resolved thin-wrapper redirect for a [`ClosureArm`] — see
/// [`ClosureArm::passthrough`]. `head` is the inner call's head (always a
/// `Value::Sym`, so it is region-independent and copies verbatim across
/// promote/freeze/message); `map[k]` is the `argv` index that the inner call's
/// `k`th argument forwards.
#[derive(Clone)]
pub struct Passthrough {
    pub head: Value,
    pub map: SmallVec<[usize; 4]>,
}

impl ClosureArm {
    /// Smallest argument count this arm accepts.
    pub fn min_arity(&self) -> usize {
        self.params.len()
    }
    /// Largest argument count this arm accepts (`None` = unbounded, has `&` rest).
    pub fn max_arity(&self) -> Option<usize> {
        if self.rest.is_some() {
            None
        } else {
            Some(self.params.len() + self.optionals.len())
        }
    }
    /// Does this arm accept a call of `argc` arguments?
    pub fn accepts(&self, argc: usize) -> bool {
        argc >= self.min_arity() && self.max_arity().map_or(true, |m| argc <= m)
    }
}

/// A user-defined function. Captures its defining environment (an [`EnvId`]) for
/// lexical scoping.
#[derive(Clone, Default)]
pub struct Closure {
    pub name: Option<Symbol>,
    /// Arity clauses, dispatched by argument count (always ≥ 1). A single-arity
    /// function has one arm; see [`ClosureArm`].
    pub arms: Vec<ClosureArm>,
    /// The docstring: a leading string literal in the `fn`/`defn` body, when
    /// more body follows it (a lone string is the return value, not docs — the
    /// CL/Elisp rule). Read by `(doc f)`; powers hover / signature help. See
    /// ADR-025 / `docs/lsp.md`.
    pub doc: Option<String>,
    /// The captured lexical environment. `None` means the **global** env —
    /// resolved per-process at call time, so a (shared) top-level closure works
    /// in any process. `Some(id)` is a specific local enclosing scope.
    pub env: Option<EnvId>,
}

impl Closure {
    /// Build a single-arity closure (the common case) from a flat param spec.
    pub fn single(
        name: Option<Symbol>,
        params: Vec<Symbol>,
        optionals: Vec<(Symbol, Value)>,
        rest: Option<Symbol>,
        body: Vec<Value>,
        doc: Option<String>,
        env: Option<EnvId>,
    ) -> Self {
        Closure {
            name,
            arms: vec![ClosureArm {
                params,
                optionals,
                rest,
                body,
                // Filled by `Heap::alloc_closure` once the closure is interned.
                passthrough: None,
            }],
            doc,
            env,
        }
    }

    /// Select the arm to run for a call of `argc` arguments. Prefers an exact
    /// fixed-arity arm (no `&` rest) over a variadic one; among matching arms,
    /// the one with the most required params (most specific). `None` if no arm
    /// accepts `argc` (an arity error). A single-arity closure always returns its
    /// sole arm when `argc` fits.
    pub fn select_arm(&self, argc: usize) -> Option<&ClosureArm> {
        self.arms
            .iter()
            .filter(|a| a.accepts(argc))
            // exact fixed match beats variadic; then most-specific (most params).
            .max_by_key(|a| (a.rest.is_none(), a.params.len()))
    }
}

/// Signature of a builtin: already-evaluated args, the call-site environment,
/// and the heap (to read/allocate values and call back into `eval`).
pub type NativeFnPtr = fn(&[Value], EnvId, &mut Heap) -> LispResult;

/// How many arguments a primitive accepts — declared once per builtin, the single
/// source of truth for the arity check the evaluator runs before every native
/// call. (Closures derive theirs from their parameter list instead.) `max: None`
/// is variadic.
#[derive(Clone, Copy, Debug)]
pub struct Arity {
    pub min: usize,
    pub max: Option<usize>,
}

impl Arity {
    /// Exactly `n` arguments.
    pub const fn exact(n: usize) -> Self {
        Arity {
            min: n,
            max: Some(n),
        }
    }
    /// `n` or more (variadic tail).
    pub const fn at_least(n: usize) -> Self {
        Arity { min: n, max: None }
    }
    /// Between `min` and `max` inclusive (e.g. an optional trailing arg).
    pub const fn range(min: usize, max: usize) -> Self {
        Arity {
            min,
            max: Some(max),
        }
    }
    /// Any number of arguments.
    pub const fn any() -> Self {
        Arity { min: 0, max: None }
    }
    /// Does this arity admit a call with `n` arguments?
    pub fn accepts(self, n: usize) -> bool {
        n >= self.min && self.max.is_none_or(|m| n <= m)
    }
}

pub struct NativeFn {
    pub name: String,
    pub arity: Arity,
    pub func: NativeFnPtr,
    /// Parameter names for hover / signature help (e.g. `["a", "b"]`, or
    /// `["&", "xs"]` for a variadic tail) — the builtin analogue of a closure's
    /// params, so `arglist` and the LSP treat primitives and Brood functions
    /// uniformly. Empty when undocumented.
    pub params: &'static [&'static str],
    /// One-line docstring shown on hover / by `(doc 'name)`. Empty when
    /// undocumented. Primitives can't carry a `defn` leading-string docstring
    /// (they're Rust), so this is their equivalent; sourced from the
    /// `PRIMITIVE_DOCS` table in `builtins.rs` (mirrors `docs/primitives.md`).
    pub doc: &'static str,
    /// The primitive's type signature — what the advisory checker reads to flag
    /// provably-wrong calls. **Required:** the compatibility-contract point #6
    /// (every primitive declares its type) is enforced *here* — there is no
    /// way to construct a `NativeFn` without one. A primitive whose args/result
    /// aren't usefully typed uses `Sig::any()`, the explicit "no useful info"
    /// signature (which still satisfies the contract). See `types/mod.rs` and
    /// `types/check.rs`.
    pub sig: crate::types::Sig,
}

// ----- handle-free constructors (interned; no heap needed) -----

pub fn sym(name: &str) -> Value {
    Value::Sym(intern(name))
}

pub fn kw(name: &str) -> Value {
    Value::Keyword(intern(name))
}

// Process-wide so the uniqueness guarantee below holds across scheduler threads:
// a green process expanding macros on a worker thread must not mint the same name
// as the root thread. (A `thread_local` counter would reset per worker and clash.)
static GENSYM_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh, interned symbol `<prefix>__<n>` for hygiene-by-convention. Shared by
/// the `gensym` builtin and the compile pass (so a macro-time temporary and a
/// pattern-lowering temporary can never collide), across all threads.
pub fn gensym(prefix: &str) -> Value {
    let n = GENSYM_COUNTER.fetch_add(1, Ordering::Relaxed);
    sym(&format!("{}__{}", prefix, n))
}

/// JIT layout constants for [`Value`] (`#[repr(C, u8)]`). JIT codegen hardcodes these
/// to read a `Value` out of a `Heap::roots` slot — the discriminant byte at offset 0,
/// the payload at [`jit_layout::PAYLOAD_OFFSET`]. Pinned by the layout test below; a
/// reorder of `Value`'s variants breaks both, intentionally (an ABI break the JIT
/// codegen must follow — see `docs/jit-stage1.md` §2).
#[cfg(any(test, feature = "jit"))]
pub(crate) mod jit_layout {
    /// The 8-aligned payload sits after the `u8` discriminant under `repr(C, u8)`.
    pub const PAYLOAD_OFFSET: usize = 8;
    /// `Value::Bool`'s discriminant (declaration order: `Nil`=0, `Bool`=1, `Int`=2). The
    /// JIT boxes a comparison result (`<`/`<=`/`=`, an `i8` 0/1) as a `Value::Bool` with
    /// this tag — a Brood comparison yields `true`/`false`, not the integers 0/1.
    pub const TAG_BOOL: u8 = 1;
    /// `Value::Int`'s discriminant (declaration order: `Nil`=0, `Bool`=1, `Int`=2).
    pub const TAG_INT: u8 = 2;
    /// `Value::Float`'s discriminant — `Int`=2, `BigInt`=3, `Float`=4 (see `TAG_PAIR`'s
    /// note on why `Value`'s tags differ from `Tag`'s). The float JIT tag-checks a slot's
    /// discriminant against this before reading its `f64` payload. Pinned by the layout test.
    pub const TAG_FLOAT: u8 = 4;
    /// `Value::Pair`'s discriminant. Note this is **not** `Tag::Pair` (7): `Value` has an
    /// extra `BigInt` after `Int` (folded into `int` by `Tag`) *and* a `Rope` before
    /// `Pair`, so `Value`'s discriminants run `… Int=2, BigInt=3, Float=4, …, Str=7,
    /// Rope=8, Pair=9`. A `car`/`cdr` tag-check compares a slot's discriminant byte
    /// against this. Pinned by the layout test.
    pub const TAG_PAIR: u8 = 9;
}

#[cfg(test)]
mod jit_layout_tests {
    use super::*;

    /// Pins the `Value` byte layout the JIT depends on. If `Value`'s variant order or
    /// `repr` changes, this fails — forcing the JIT codegen (and `jit_layout`) to be
    /// updated rather than silently miscompiling. See `docs/jit-stage1.md` §2.
    #[test]
    fn value_layout_is_stable_for_the_jit() {
        assert_eq!(
            std::mem::align_of::<Value>(),
            8,
            "Value must stay 8-aligned"
        );
        // The JIT addresses a roots slot as `roots_base + k * size_of::<Value>()` (its
        // `STRIDE`) and copies a whole handle word-by-word across all `size_of` bytes — a
        // change here is an ABI break the JIT codegen must follow. It is **not** 16: the
        // `Pid { node, id }` variant needs two payload words (the second at offset 16),
        // which a tag+payload-only copy would drop.
        assert_eq!(
            std::mem::size_of::<Value>(),
            24,
            "Value size (the JIT's STRIDE) drifted"
        );
        // `Value::Pair`'s discriminant byte must match `TAG_PAIR` (the JIT's car/cdr
        // tag-check); a variant reorder breaks it.
        let p = Value::Pair(PairId::local_gen(0, 0));
        let pbytes = unsafe {
            std::slice::from_raw_parts(
                &p as *const Value as *const u8,
                std::mem::size_of::<Value>(),
            )
        };
        assert_eq!(
            pbytes[0],
            jit_layout::TAG_PAIR,
            "Value::Pair discriminant drifted"
        );
        // `Value::Bool`'s discriminant must match `TAG_BOOL` (the JIT boxes a comparison
        // result as a Bool, not an Int); a variant reorder breaks it.
        let bt = Value::Bool(true);
        let btbytes = unsafe {
            std::slice::from_raw_parts(
                &bt as *const Value as *const u8,
                std::mem::size_of::<Value>(),
            )
        };
        assert_eq!(
            btbytes[0],
            jit_layout::TAG_BOOL,
            "Value::Bool discriminant drifted"
        );
        // `Value::Float`'s discriminant must match `TAG_FLOAT` (the float JIT's slot
        // tag-check); a variant reorder breaks it.
        let f = Value::Float(1.5);
        let fbytes = unsafe {
            std::slice::from_raw_parts(&f as *const Value as *const u8, std::mem::size_of::<Value>())
        };
        assert_eq!(
            fbytes[0],
            jit_layout::TAG_FLOAT,
            "Value::Float discriminant drifted"
        );
        let v = Value::Int(0x0123_4567_89ab_cdef_u64 as i64);
        // Read the raw bytes (no transmute size constraint).
        let bytes = unsafe {
            std::slice::from_raw_parts(
                &v as *const Value as *const u8,
                std::mem::size_of::<Value>(),
            )
        };
        assert_eq!(
            bytes[0],
            jit_layout::TAG_INT,
            "Value::Int discriminant drifted"
        );
        let off = jit_layout::PAYLOAD_OFFSET;
        let payload = i64::from_ne_bytes(bytes[off..off + 8].try_into().unwrap());
        assert_eq!(
            payload, 0x0123_4567_89ab_cdef_u64 as i64,
            "Value::Int payload not at offset {off}",
        );
    }
}
