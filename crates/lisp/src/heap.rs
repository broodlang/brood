//! The per-process heap: an arena that owns every heap-allocated value, plus the
//! environment frames. Values address it by integer handle (see `value.rs`).
//!
//! It is deliberately plain `Vec`s of data, so the whole `Heap` is `Send` — the
//! property that lets a process move between scheduler threads later. There is
//! no garbage collection yet (the arena only grows); a per-process mark-sweep is
//! a later step (see `docs/memory-model.md`).
//!
//! Mutation goes through `&mut Heap`, so no interior mutability (`RefCell`) is
//! needed — the heap is the single owner of its objects.

use std::collections::HashMap;

use crate::error::LispError;
use crate::value::{
    Closure, ClosureId, EnvId, NativeFn, NativeId, PairId, StrId, Symbol, VecId, Value,
};

struct EnvFrame {
    vars: HashMap<Symbol, Value>,
    parent: Option<EnvId>,
}

#[derive(Default)]
pub struct Heap {
    pairs: Vec<(Value, Value)>,
    vectors: Vec<Vec<Value>>,
    strings: Vec<String>,
    closures: Vec<Closure>,
    natives: Vec<NativeFn>,
    envs: Vec<EnvFrame>,
}

impl Heap {
    pub fn new() -> Self {
        Heap::default()
    }

    // ----- allocation -----

    pub fn alloc_pair(&mut self, head: Value, tail: Value) -> Value {
        let id = self.pairs.len() as u32;
        self.pairs.push((head, tail));
        Value::Pair(PairId(id))
    }

    pub fn alloc_vector(&mut self, items: Vec<Value>) -> Value {
        let id = self.vectors.len() as u32;
        self.vectors.push(items);
        Value::Vector(VecId(id))
    }

    pub fn alloc_string(&mut self, s: &str) -> Value {
        let id = self.strings.len() as u32;
        self.strings.push(s.to_string());
        Value::Str(StrId(id))
    }

    pub fn alloc_closure(&mut self, c: Closure) -> ClosureId {
        let id = self.closures.len() as u32;
        self.closures.push(c);
        ClosureId(id)
    }

    pub fn alloc_native(&mut self, f: NativeFn) -> Value {
        let id = self.natives.len() as u32;
        self.natives.push(f);
        Value::Native(NativeId(id))
    }

    /// Build a proper list from a vector of items.
    pub fn list(&mut self, items: Vec<Value>) -> Value {
        let mut acc = Value::Nil;
        for item in items.into_iter().rev() {
            acc = self.alloc_pair(item, acc);
        }
        acc
    }

    // ----- access (handles are Copy; small reads return by value) -----

    pub fn pair(&self, id: PairId) -> (Value, Value) {
        self.pairs[id.0 as usize]
    }
    pub fn car(&self, id: PairId) -> Value {
        self.pairs[id.0 as usize].0
    }
    pub fn cdr(&self, id: PairId) -> Value {
        self.pairs[id.0 as usize].1
    }
    pub fn vector(&self, id: VecId) -> &[Value] {
        &self.vectors[id.0 as usize]
    }
    pub fn string(&self, id: StrId) -> &str {
        &self.strings[id.0 as usize]
    }
    pub fn closure(&self, id: ClosureId) -> &Closure {
        &self.closures[id.0 as usize]
    }
    pub fn native(&self, id: NativeId) -> &NativeFn {
        &self.natives[id.0 as usize]
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

    pub fn new_env(&mut self, parent: Option<EnvId>) -> EnvId {
        let id = self.envs.len() as u32;
        self.envs.push(EnvFrame { vars: HashMap::new(), parent });
        EnvId(id)
    }

    pub fn env_get(&self, env: EnvId, sym: Symbol) -> Option<Value> {
        let mut cur = Some(env);
        while let Some(e) = cur {
            let frame = &self.envs[e.0 as usize];
            if let Some(v) = frame.vars.get(&sym) {
                return Some(*v);
            }
            cur = frame.parent;
        }
        None
    }

    pub fn env_define(&mut self, env: EnvId, sym: Symbol, val: Value) {
        self.envs[env.0 as usize].vars.insert(sym, val);
    }

    /// Mutate the nearest existing binding; returns false if none exists.
    pub fn env_set(&mut self, env: EnvId, sym: Symbol, val: Value) -> bool {
        let mut cur = Some(env);
        while let Some(e) = cur {
            if self.envs[e.0 as usize].vars.contains_key(&sym) {
                self.envs[e.0 as usize].vars.insert(sym, val);
                return true;
            }
            cur = self.envs[e.0 as usize].parent;
        }
        false
    }

    /// Walk to the global (parent-less) environment.
    pub fn env_root(&self, env: EnvId) -> EnvId {
        let mut cur = env;
        loop {
            match self.envs[cur.0 as usize].parent {
                Some(p) => cur = p,
                None => return cur,
            }
        }
    }
}
