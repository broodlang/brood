//! The core data type of the language: [`Value`].
//!
//! Everything the reader produces, the evaluator manipulates, and the printer
//! renders is a `Value`. Lists are built from cons [`Value::Pair`]s terminated
//! by [`Value::Nil`] — this keeps the language homoiconic (code is data), which
//! is what will later let the editor rewrite itself at runtime.
//!
//! ## Memory model (v0.1)
//!
//! Heap values are held behind [`Rc`] and environments use `RefCell` for
//! interior mutability. This is the simplest thing that works. Its known
//! limitation is that reference cycles (e.g. a closure capturing an environment
//! that points back at it) will leak. That is acceptable for a REPL and the
//! early milestones; a tracing GC (`gc-arena`) is planned before editor
//! sessions become long-lived. All heap construction goes through the helpers
//! in this module so that migration touches one place.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::env::Env;
use crate::error::{LispError, LispResult};

/// An interned symbol name. Comparing symbols is a `u32` compare; the spelling
/// lives in a thread-local table (see [`intern`] / [`symbol_name`]).
pub type Symbol = u32;

thread_local! {
    static INTERNER: RefCell<Interner> = RefCell::new(Interner::default());
}

#[derive(Default)]
struct Interner {
    ids: HashMap<String, Symbol>,
    names: Vec<String>,
}

/// Intern a name, returning a stable [`Symbol`] id for it.
pub fn intern(name: &str) -> Symbol {
    INTERNER.with(|cell| {
        let mut i = cell.borrow_mut();
        if let Some(&id) = i.ids.get(name) {
            return id;
        }
        let id = i.names.len() as Symbol;
        i.names.push(name.to_string());
        i.ids.insert(name.to_string(), id);
        id
    })
}

/// Recover the spelling of an interned [`Symbol`].
pub fn symbol_name(sym: Symbol) -> String {
    INTERNER.with(|cell| cell.borrow().names[sym as usize].clone())
}

#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Sym(Symbol),
    Keyword(Symbol),
    /// A cons cell. Proper lists are chains of pairs ending in `Nil`.
    Pair(Rc<(Value, Value)>),
    /// A vector literal, `[a b c]`. Also used as a function's parameter list.
    Vector(Rc<Vec<Value>>),
    /// A closure: code plus the lexical environment captured at definition.
    Fn(Rc<Closure>),
    /// A builtin implemented in Rust.
    Native(Rc<NativeFn>),
}

/// A user-defined function (a `fn`/`lambda`). Captures its defining environment
/// for lexical scoping.
pub struct Closure {
    pub name: Option<Symbol>,
    pub params: Vec<Symbol>,
    /// The name bound to the remaining args when the parameter list uses `& rest`.
    pub rest: Option<Symbol>,
    pub body: Vec<Value>,
    pub env: Rc<Env>,
}

/// Signature of a builtin function. Receives already-evaluated arguments and
/// the call-site environment (needed by builtins like `eval`/`load`/`apply`).
pub type NativeFnPtr = fn(&[Value], &Rc<Env>) -> LispResult;

pub struct NativeFn {
    pub name: String,
    pub func: NativeFnPtr,
}

// ----- constructors (the single chokepoint for heap allocation) -----

pub fn sym(name: &str) -> Value {
    Value::Sym(intern(name))
}
pub fn kw(name: &str) -> Value {
    Value::Keyword(intern(name))
}
pub fn str_val(s: &str) -> Value {
    Value::Str(Rc::from(s))
}
pub fn cons(head: Value, tail: Value) -> Value {
    Value::Pair(Rc::new((head, tail)))
}

/// Build a proper list from a vector of items.
pub fn list(items: Vec<Value>) -> Value {
    let mut acc = Value::Nil;
    for item in items.into_iter().rev() {
        acc = cons(item, acc);
    }
    acc
}

/// Collect a proper list into a `Vec`. Errors on an improper (dotted) list.
pub fn list_to_vec(v: &Value) -> Result<Vec<Value>, LispError> {
    let mut out = Vec::new();
    let mut cur = v.clone();
    loop {
        match cur {
            Value::Nil => return Ok(out),
            Value::Pair(p) => {
                out.push(p.0.clone());
                cur = p.1.clone();
            }
            _ => return Err(LispError::type_err("improper list")),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Nil, Nil) => true,
            (Bool(a), Bool(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            // Compare floats by bit pattern so equality is reflexive (NaN == NaN).
            (Float(a), Float(b)) => a.to_bits() == b.to_bits(),
            (Str(a), Str(b)) => a == b,
            (Sym(a), Sym(b)) => a == b,
            (Keyword(a), Keyword(b)) => a == b,
            (Pair(a), Pair(b)) => a.0 == b.0 && a.1 == b.1,
            (Vector(a), Vector(b)) => a == b,
            // Functions compare by identity.
            (Fn(a), Fn(b)) => Rc::ptr_eq(a, b),
            (Native(a), Native(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", crate::printer::print(self))
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", crate::printer::print(self))
    }
}
