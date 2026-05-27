//! The per-process heap, plus the shared **code** region.
//!
//! A `Value`'s heap variants are integer handles whose high bit (`SHARED_BIT`,
//! see `value.rs`) says which region they live in:
//!
//! - **local** — the per-process `Heap`: everything the process allocates at
//!   runtime (cons cells, vectors, strings, call-frame env scopes). Plain
//!   `Vec`s, mutated through `&mut Heap`, so the whole `Heap` is `Send`.
//! - **shared** — a [`SharedCode`] region (behind `Arc`) holding code shared
//!   across processes. *Currently empty* (step 1 of `docs/shared-code.md`): all
//!   allocation is local and every read routes to the local slabs, so behaviour
//!   is identical to before — this just lays the routing in place.
//!
//! No GC yet (the arena only grows).

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::LispError;
use crate::value::{
    Closure, ClosureId, EnvId, NativeFn, NativeId, PairId, StrId, Symbol, VecId, Value,
};

struct EnvFrame {
    vars: HashMap<Symbol, Value>,
    parent: Option<EnvId>,
}

/// Re-tag a value's handle from the local region to the shared region (same
/// slab index, region bit set). Atoms are unchanged.
fn to_shared(v: Value) -> Value {
    match v {
        Value::Pair(id) => Value::Pair(PairId::shared(id.index())),
        Value::Vector(id) => Value::Vector(VecId::shared(id.index())),
        Value::Str(id) => Value::Str(StrId::shared(id.index())),
        Value::Fn(id) => Value::Fn(ClosureId::shared(id.index())),
        Value::Macro(id) => Value::Macro(ClosureId::shared(id.index())),
        Value::Native(id) => Value::Native(NativeId::shared(id.index())),
        other => other,
    }
}

/// The slabs holding heap objects. Used for both the local heap and the shared
/// code region.
#[derive(Default)]
struct Slabs {
    pairs: Vec<(Value, Value)>,
    vectors: Vec<Vec<Value>>,
    strings: Vec<String>,
    closures: Vec<Closure>,
    natives: Vec<NativeFn>,
    envs: Vec<EnvFrame>,
}

/// The shared, read-only code region (closures, code values, the global env,
/// natives). Cloned by `Arc` into every process. Empty until step 2.
#[derive(Default)]
pub struct SharedCode {
    slabs: Slabs,
}

pub struct Heap {
    local: Slabs,
    code: Arc<SharedCode>,
    /// This process's global (parent-less) environment. A closure that captured
    /// the global env (`Closure.env == None`) resolves to this at call time.
    global: EnvId,
}

impl Heap {
    pub fn new() -> Self {
        Heap { local: Slabs::default(), code: Arc::default(), global: EnvId::local(0) }
    }

    /// A fresh process heap sharing the given code region (empty local slabs).
    pub fn with_code(code: Arc<SharedCode>) -> Self {
        Heap { local: Slabs::default(), code, global: EnvId::local(0) }
    }

    /// Consume this (builder) heap: move everything it allocated into a shared
    /// code region — re-tagging every handle from the local region to the shared
    /// region — and return that region plus the global env's bindings
    /// (`symbol -> shared value`) used to seed each process's global env.
    ///
    /// Env frames are dropped: shared (top-level) closures capture the global
    /// env symbolically (`env == None`), so nothing references a shared frame.
    pub fn freeze_as_shared_code(self, root: EnvId) -> (SharedCode, Vec<(Symbol, Value)>) {
        let bindings: Vec<(Symbol, Value)> = self.local.envs[root.index()]
            .vars
            .iter()
            .map(|(&s, &v)| (s, to_shared(v)))
            .collect();

        let mut slabs = self.local;
        for p in &mut slabs.pairs {
            p.0 = to_shared(p.0);
            p.1 = to_shared(p.1);
        }
        for vec in &mut slabs.vectors {
            for x in vec.iter_mut() {
                *x = to_shared(*x);
            }
        }
        for c in &mut slabs.closures {
            for f in c.body.iter_mut() {
                *f = to_shared(*f);
            }
            for (_, d) in c.optionals.iter_mut() {
                *d = to_shared(*d);
            }
            debug_assert!(c.env.is_none(), "shared closures must capture the global env");
        }
        slabs.envs = Vec::new(); // the shared region has no env frames

        (SharedCode { slabs }, bindings)
    }

    /// Record this process's global environment (call once, after creating it).
    pub fn set_global(&mut self, env: EnvId) {
        self.global = env;
    }

    /// This process's global environment.
    pub fn global(&self) -> EnvId {
        self.global
    }

    /// True if `env` is a global (parent-less) environment frame.
    pub fn is_global(&self, env: EnvId) -> bool {
        self.env_frame(env).parent.is_none()
    }

    /// The slabs a handle points into.
    fn slabs(&self, shared: bool) -> &Slabs {
        if shared {
            &self.code.slabs
        } else {
            &self.local
        }
    }

    // ----- allocation (always into the local heap) -----

    pub fn alloc_pair(&mut self, head: Value, tail: Value) -> Value {
        let idx = self.local.pairs.len();
        self.local.pairs.push((head, tail));
        Value::Pair(PairId::local(idx))
    }

    pub fn alloc_vector(&mut self, items: Vec<Value>) -> Value {
        let idx = self.local.vectors.len();
        self.local.vectors.push(items);
        Value::Vector(VecId::local(idx))
    }

    pub fn alloc_string(&mut self, s: &str) -> Value {
        let idx = self.local.strings.len();
        self.local.strings.push(s.to_string());
        Value::Str(StrId::local(idx))
    }

    pub fn alloc_closure(&mut self, c: Closure) -> ClosureId {
        let idx = self.local.closures.len();
        self.local.closures.push(c);
        ClosureId::local(idx)
    }

    pub fn alloc_native(&mut self, f: NativeFn) -> Value {
        let idx = self.local.natives.len();
        self.local.natives.push(f);
        Value::Native(NativeId::local(idx))
    }

    /// Build a proper list from a vector of items.
    pub fn list(&mut self, items: Vec<Value>) -> Value {
        let mut acc = Value::Nil;
        for item in items.into_iter().rev() {
            acc = self.alloc_pair(item, acc);
        }
        acc
    }

    // ----- access (dispatch on the handle's region) -----

    pub fn pair(&self, id: PairId) -> (Value, Value) {
        self.slabs(id.is_shared()).pairs[id.index()]
    }
    pub fn car(&self, id: PairId) -> Value {
        self.pair(id).0
    }
    pub fn cdr(&self, id: PairId) -> Value {
        self.pair(id).1
    }
    pub fn vector(&self, id: VecId) -> &[Value] {
        &self.slabs(id.is_shared()).vectors[id.index()]
    }
    pub fn string(&self, id: StrId) -> &str {
        &self.slabs(id.is_shared()).strings[id.index()]
    }
    pub fn closure(&self, id: ClosureId) -> &Closure {
        &self.slabs(id.is_shared()).closures[id.index()]
    }
    pub fn native(&self, id: NativeId) -> &NativeFn {
        &self.slabs(id.is_shared()).natives[id.index()]
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
    pub fn equal(&self, a: Value, b: Value) -> bool {
        use Value::*;
        match (a, b) {
            (Nil, Nil) => true,
            (Bool(x), Bool(y)) => x == y,
            (Int(x), Int(y)) => x == y,
            (Float(x), Float(y)) => x.to_bits() == y.to_bits(),
            (Sym(x), Sym(y)) => x == y,
            (Keyword(x), Keyword(y)) => x == y,
            (Str(x), Str(y)) => self.string(x) == self.string(y),
            (Pair(x), Pair(y)) => {
                let (a0, a1) = self.pair(x);
                let (b0, b1) = self.pair(y);
                self.equal(a0, b0) && self.equal(a1, b1)
            }
            (Vector(x), Vector(y)) => {
                let xs = self.vector(x);
                let ys = self.vector(y);
                xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(&p, &q)| self.equal(p, q))
            }
            (Fn(x), Fn(y)) => x == y,
            (Macro(x), Macro(y)) => x == y,
            (Native(x), Native(y)) => x == y,
            _ => false,
        }
    }

    // ----- environments -----

    fn env_frame(&self, env: EnvId) -> &EnvFrame {
        &self.slabs(env.is_shared()).envs[env.index()]
    }

    pub fn new_env(&mut self, parent: Option<EnvId>) -> EnvId {
        let idx = self.local.envs.len();
        self.local.envs.push(EnvFrame { vars: HashMap::new(), parent });
        EnvId::local(idx)
    }

    pub fn env_get(&self, env: EnvId, sym: Symbol) -> Option<Value> {
        let mut cur = Some(env);
        while let Some(e) = cur {
            let frame = self.env_frame(e);
            if let Some(v) = frame.vars.get(&sym) {
                return Some(*v);
            }
            cur = frame.parent;
        }
        None
    }

    pub fn env_define(&mut self, env: EnvId, sym: Symbol, val: Value) {
        // Allocation/definition targets the local heap. (Defining into the shared
        // global env arrives with the mutable shared region — step 4.)
        self.local.envs[env.index()].vars.insert(sym, val);
    }

    /// Mutate the nearest existing binding; returns false if none exists.
    pub fn env_set(&mut self, env: EnvId, sym: Symbol, val: Value) -> bool {
        let mut cur = Some(env);
        while let Some(e) = cur {
            if self.env_frame(e).vars.contains_key(&sym) {
                if e.is_shared() {
                    // Mutating a shared global binding comes with step 4.
                    return false;
                }
                self.local.envs[e.index()].vars.insert(sym, val);
                return true;
            }
            cur = self.env_frame(e).parent;
        }
        false
    }

    /// Walk to the global (parent-less) environment.
    pub fn env_root(&self, env: EnvId) -> EnvId {
        let mut cur = env;
        loop {
            match self.env_frame(cur).parent {
                Some(p) => cur = p,
                None => return cur,
            }
        }
    }
}
