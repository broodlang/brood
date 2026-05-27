//! The core value type, [`Value`], plus the handle types that address the
//! per-process [`Heap`](crate::heap::Heap).
//!
//! After the step-2 migration (see `docs/memory-model.md`), `Value` is `Copy`:
//! its heap variants are small integer **handles** into a `Heap`, not `Rc`
//! pointers. Reading or allocating a heap object goes through the `Heap`. The
//! payoff: a `Heap` is plain `Vec`s of data, so it is `Send` — a process can be
//! moved between scheduler threads — and it gives us one place to do GC.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::error::LispResult;
use crate::heap::Heap;

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

pub fn intern(name: &str) -> Symbol {
    let mut i = INTERNER.lock().unwrap();
    if let Some(&id) = i.ids.get(name) {
        return id;
    }
    let id = i.names.len() as Symbol;
    i.names.push(name.to_string());
    i.ids.insert(name.to_string(), id);
    id
}

pub fn symbol_name(sym: Symbol) -> String {
    INTERNER.lock().unwrap().names[sym as usize].clone()
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
    /// A closure (`fn`).
    Fn(ClosureId),
    /// A macro — same `Closure` storage, invoked on unevaluated forms.
    Macro(ClosureId),
    /// A builtin implemented in Rust.
    Native(NativeId),
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
    /// The captured lexical environment. `None` means the **global** env —
    /// resolved per-process at call time, so a (shared) top-level closure works
    /// in any process. `Some(id)` is a specific local enclosing scope.
    pub env: Option<EnvId>,
}

/// Signature of a builtin: already-evaluated args, the call-site environment,
/// and the heap (to read/allocate values and call back into `eval`).
pub type NativeFnPtr = fn(&[Value], EnvId, &mut Heap) -> LispResult;

pub struct NativeFn {
    pub name: String,
    pub func: NativeFnPtr,
}

// ----- handle-free constructors (interned; no heap needed) -----

pub fn sym(name: &str) -> Value {
    Value::Sym(intern(name))
}

pub fn kw(name: &str) -> Value {
    Value::Keyword(intern(name))
}
