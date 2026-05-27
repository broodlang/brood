//! The core value type, [`Value`], plus the handle types that address the
//! per-process [`Heap`](crate::core::heap::Heap).
//!
//! After the step-2 migration (see `docs/memory-model.md`), `Value` is `Copy`:
//! its heap variants are small integer **handles** into a `Heap`, not `Rc`
//! pointers. Reading or allocating a heap object goes through the `Heap`. The
//! payoff: a `Heap` is plain `Vec`s of data, so it is `Send` — a process can be
//! moved between scheduler threads — and it gives us one place to do GC.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

use crate::core::heap::Heap;
use crate::error::LispResult;

/// An interned symbol name (a `u32` id; the spelling lives in a global table).
pub type Symbol = u32;

// Global (process-wide) interner so symbol ids are consistent across scheduler
// threads — a prerequisite for sending symbols between process heaps.
static INTERNER: LazyLock<Mutex<Interner>> = LazyLock::new(|| Mutex::new(Interner::default()));

#[derive(Default)]
struct Interner {
    ids: HashMap<String, Symbol>,
    names: Vec<String>,
}

// The interner is read-mostly and lives for the whole process; recover from a
// poisoned lock rather than letting one panicking thread wedge symbol lookup
// everywhere (the table is append-only, so a recovered guard is consistent).
fn interner() -> MutexGuard<'static, Interner> {
    INTERNER.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn intern(name: &str) -> Symbol {
    let mut i = interner();
    if let Some(&id) = i.ids.get(name) {
        return id;
    }
    let id = i.names.len() as Symbol;
    i.names.push(name.to_string());
    i.ids.insert(name.to_string(), id);
    id
}

pub fn symbol_name(sym: Symbol) -> String {
    interner().names[sym as usize].clone()
}

/// Does `sym`'s spelling equal `name`? Locks once and compares in place — no
/// `String` allocation, unlike `symbol_name(s) == name`. For the hot compares
/// against fixed words (`&optional`, `quasiquote`, the compile-pass walk).
pub fn symbol_is(sym: Symbol, name: &str) -> bool {
    interner().names[sym as usize] == name
}

/// The first character of `sym`'s spelling, if any — to recognise the `&`-marker
/// family without allocating the whole name first.
pub fn symbol_first_char(sym: Symbol) -> Option<char> {
    interner().names[sym as usize].chars().next()
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
            /// A handle into the local (per-process) data heap.
            #[inline]
            pub fn local(index: usize) -> Self {
                $name(index as u32)
            }
            /// A handle into the immutable shared prelude region.
            #[inline]
            pub fn prelude(index: usize) -> Self {
                $name(index as u32 | ((PRELUDE as u32) << REGION_SHIFT))
            }
            /// A handle into the runtime's mutable shared code region.
            #[inline]
            pub fn runtime(index: usize) -> Self {
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
    }
}

/// A user-defined function. Captures its defining environment (an [`EnvId`]) for
/// lexical scoping.
#[derive(Clone)]
pub struct Closure {
    pub name: Option<Symbol>,
    pub params: Vec<Symbol>,
    pub optionals: Vec<(Symbol, Value)>,
    pub rest: Option<Symbol>,
    pub body: Vec<Value>,
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
