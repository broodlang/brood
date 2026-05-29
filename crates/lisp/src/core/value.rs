//! The core value type, [`Value`], plus the handle types that address the
//! per-process [`Heap`](crate::core::heap::Heap).
//!
//! After the step-2 migration (see `docs/memory-model.md`), `Value` is `Copy`:
//! its heap variants are small integer **handles** into a `Heap`, not `Rc`
//! pointers. Reading or allocating a heap object goes through the `Heap`. The
//! payoff: a `Heap` is plain `Vec`s of data, so it is `Send` — a process can be
//! moved between scheduler threads — and it gives us one place to do GC.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, RwLock};

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

pub fn intern(name: &str) -> Symbol {
    let mut ids = ids();
    if let Some(&id) = ids.get(name) {
        return id;
    }
    // A new name: its index in the append-only `NAMES` vec *is* its id. Pushing
    // while holding the `IDS` lock keeps a single writer, so ids stay dense and
    // the two tables never disagree. One allocation, not two — `NAMES` and
    // `IDS` share the same `String` (cloned once from `name: &str` here).
    let owned = name.to_string();
    let id = NAMES.push(owned.clone()) as Symbol;
    ids.insert(owned, id);
    id
}

pub fn symbol_name(sym: Symbol) -> String {
    NAMES.get(sym as usize).expect("interned symbol id").clone()
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

/// A handle's two high bits tag which heap **region** it addresses; the low 30
/// bits are the slab index (≈1 billion objects per region — ample). See
/// `docs/shared-code.md`.
///
/// - [`LOCAL`] — the process's own data heap (mutable, per-process).
/// - [`PRELUDE`] — the immutable prelude/builtins, shared by *all* runtimes.
/// - [`RUNTIME`] — a runtime's mutable, append-only code region, shared by all
///   of that runtime's inner (spawned) processes. This is where `def`'d code
///   and the global bindings live, so an update is visible to running processes.
pub const REGION_SHIFT: u32 = 30;
pub const INDEX_MASK: u32 = (1 << REGION_SHIFT) - 1;
pub const LOCAL: u8 = 0b00;
pub const PRELUDE: u8 = 0b01;
pub const RUNTIME: u8 = 0b10;

macro_rules! handle {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub struct $name(pub u32);
        impl $name {
            /// A handle into the local (per-process) data heap. The
            /// debug-assert catches the silent-aliasing case where a slab
            /// grows past `2^30` and the high bits start colliding with the
            /// region tag.
            #[inline]
            pub fn local(index: usize) -> Self {
                debug_assert!(
                    index < (1 << REGION_SHIFT),
                    "handle index {} would alias the region tag bits",
                    index,
                );
                $name(index as u32)
            }
            /// A handle into the immutable shared prelude region.
            #[inline]
            pub fn prelude(index: usize) -> Self {
                debug_assert!(
                    index < (1 << REGION_SHIFT),
                    "prelude handle index {} would alias the region tag bits",
                    index,
                );
                $name(index as u32 | ((PRELUDE as u32) << REGION_SHIFT))
            }
            /// A handle into the runtime's mutable shared code region.
            #[inline]
            pub fn runtime(index: usize) -> Self {
                debug_assert!(
                    index < (1 << REGION_SHIFT),
                    "runtime handle index {} would alias the region tag bits",
                    index,
                );
                $name(index as u32 | ((RUNTIME as u32) << REGION_SHIFT))
            }
            /// Which region this handle addresses ([`LOCAL`]/[`PRELUDE`]/[`RUNTIME`]).
            #[inline]
            pub fn region(self) -> u8 {
                (self.0 >> REGION_SHIFT) as u8
            }
            /// The slab index, with the region tag masked off.
            #[inline]
            pub fn index(self) -> usize {
                (self.0 & INDEX_MASK) as usize
            }
        }
    };
}
handle!(PairId);
handle!(VecId);
handle!(StrId);
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
    /// **Encoding.** `u32::MAX` sets both region bits to `0b11`, an otherwise
    /// undefined region — `LOCAL` / `PRELUDE` / `RUNTIME` are `0b00` / `0b01`
    /// / `0b10`. This is the marker `Heap::env_frame` and the env walkers
    /// short-circuit on (`env == EnvId::GLOBAL`) before touching the region
    /// dispatch; a stray dispatch on a GLOBAL panics with a clear message
    /// (see `Heap::env_frame`), not the `_ => unreachable!()` fall-through.
    pub const GLOBAL: EnvId = EnvId(u32::MAX);
}

/// A Brood value. `Copy`: primitives inline, heap objects as handles.
#[derive(Clone, Copy, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
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
        }
    }
}

/// The runtime [`Tag`] of `v` — the canonical discriminant of [`Value`]. The one
/// place the value-to-tag mapping lives.
pub fn tag(v: Value) -> Tag {
    match v {
        Value::Nil => Tag::Nil,
        Value::Bool(_) => Tag::Bool,
        Value::Int(_) => Tag::Int,
        Value::Float(_) => Tag::Float,
        Value::Sym(_) => Tag::Sym,
        Value::Keyword(_) => Tag::Keyword,
        Value::Str(_) => Tag::Str,
        Value::Pair(_) => Tag::Pair,
        Value::Vector(_) => Tag::Vector,
        Value::Fn(_) => Tag::Fn,
        Value::Macro(_) => Tag::Macro,
        Value::Native(_) => Tag::Native,
        Value::Map(_) => Tag::Map,
        Value::Ref(_) => Tag::Ref,
        Value::Pid { .. } => Tag::Pid,
        Value::Rope(_) => Tag::Rope,
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
