//! Lexical environments.
//!
//! An [`Env`] is a frame of variable bindings plus an optional parent. Closures
//! capture the `Env` in effect where they were defined, and symbol lookup walks
//! the parent chain — that chain *is* lexical scoping. The root frame holds the
//! global definitions (builtins, prelude, and everything `def`'d at top level);
//! because it is mutable, redefining a global at runtime is what gives the
//! system its "edit yourself while running" property.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::value::{Symbol, Value};

pub struct Env {
    vars: RefCell<HashMap<Symbol, Value>>,
    parent: Option<Rc<Env>>,
}

impl Env {
    /// Create a fresh global (parent-less) environment.
    pub fn new_root() -> Rc<Env> {
        Rc::new(Env { vars: RefCell::new(HashMap::new()), parent: None })
    }

    /// Create a child scope nested inside `parent`.
    pub fn child(parent: &Rc<Env>) -> Rc<Env> {
        Rc::new(Env { vars: RefCell::new(HashMap::new()), parent: Some(parent.clone()) })
    }

    /// Walk to the global (root) environment.
    pub fn root(this: &Rc<Env>) -> Rc<Env> {
        let mut cur = this.clone();
        loop {
            let parent = cur.parent.clone();
            match parent {
                Some(p) => cur = p,
                None => return cur,
            }
        }
    }

    /// Look up `sym`, searching this frame then outward through parents.
    pub fn get(&self, sym: Symbol) -> Option<Value> {
        if let Some(v) = self.vars.borrow().get(&sym) {
            return Some(v.clone());
        }
        match &self.parent {
            Some(p) => p.get(sym),
            None => None,
        }
    }

    /// Mutate the nearest existing binding for `sym`. Returns `false` if no
    /// binding exists anywhere in the chain (so `set!` can report an error).
    pub fn set_existing(&self, sym: Symbol, val: Value) -> bool {
        if self.vars.borrow().contains_key(&sym) {
            self.vars.borrow_mut().insert(sym, val);
            return true;
        }
        match &self.parent {
            Some(p) => p.set_existing(sym, val),
            None => false,
        }
    }

    /// Define (or overwrite) a binding in *this* frame.
    pub fn define(&self, sym: Symbol, val: Value) {
        self.vars.borrow_mut().insert(sym, val);
    }
}
