//! A local binding shadows a macro (ADR-734).
//!
//! The runtime always looked a call head up in the lexical environment first, so a local
//! named `count` was called in place of the root `count`. The compile pass did not: it
//! expanded every head that named a macro, so `(let (when f) (when x))` silently ran the
//! macro and ignored the local. This module gives the compile pass the scope the runtime
//! has. The expander walks binding forms from the outside in, so by the time it reaches a
//! body it has seen every binder above it — including the ones a macro introduced, because
//! that macro was expanded before its result was walked. A head bound in that scope is a
//! call, never a macro.
//!
//! The evaluator's own forms (`if`, `let`, `fn`, …) are not names: the evaluator dispatches
//! on their spelling before any lookup, so a binder spelled like one could never be called.
//! Binding one is refused at compile time, naming it.
//!
//! The scope is per thread and per expansion: [`ExpansionScope::enter`] starts every
//! top-level expansion empty and puts the enclosing one back when it ends, so an expansion
//! nested inside another (a macro body that loads a module, a lazily compiled arm) never
//! sees the outer one's locals.

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Symbol, Value, ValueRef};
use crate::error::LispError;
use std::cell::RefCell;

thread_local! {
    /// The lexical binders in scope at the point the expander is walking, innermost last.
    static LOCAL_NAMES: RefCell<Vec<Symbol>> = const { RefCell::new(Vec::new()) };
}

/// One expansion's scope: empty while it lives, the enclosing expansion's restored when it
/// drops (on an error too).
pub(crate) struct ExpansionScope {
    saved: Vec<Symbol>,
}

impl ExpansionScope {
    pub(crate) fn enter() -> Self {
        ExpansionScope {
            saved: LOCAL_NAMES.with(|names| std::mem::take(&mut *names.borrow_mut())),
        }
    }
}

impl Drop for ExpansionScope {
    fn drop(&mut self) {
        let saved = std::mem::take(&mut self.saved);
        LOCAL_NAMES.with(|names| *names.borrow_mut() = saved);
    }
}

/// Is `name` bound by a lexical binder enclosing the form being expanded?
pub(super) fn is_local(name: Symbol) -> bool {
    LOCAL_NAMES.with(|names| names.borrow().contains(&name))
}

/// The current depth of the scope, to [`release`] back to when a binding form's body is done.
pub(super) fn mark() -> usize {
    LOCAL_NAMES.with(|names| names.borrow().len())
}

/// Leave the scope a binding form entered at `depth` ([`mark`]).
pub(super) fn release(depth: usize) {
    LOCAL_NAMES.with(|names| names.borrow_mut().truncate(depth));
}

/// Bring `name` into scope, or refuse it when it is spelled like one of the evaluator's own
/// forms — which the evaluator reads before any lookup, so such a binding could never be
/// called and would only hide.
pub(super) fn bind(name: Symbol) -> Result<(), LispError> {
    if crate::eval::is_special_form(name) {
        let spelling = value::symbol_name(name);
        return Err(LispError::runtime(format!(
            "`{spelling}` is one of the evaluator's own forms and cannot be bound as a name"
        ))
        .with_hint(format!(
            "rename the binding — `{spelling}` always means the form, so a local spelled \
             that way could never be called. The evaluator's forms are `def`, `do`, `fn`, \
             `if`, `let`, `letrec`, `quote` and `quasiquote`; every other name, a macro's \
             included, can be bound and shadows it inside the binding"
        )));
    }
    LOCAL_NAMES.with(|names| names.borrow_mut().push(name));
    Ok(())
}

/// Bring a `let`/`letrec` binding target into scope when it is a plain symbol (a pattern
/// target was lowered before the expander got here).
pub(super) fn bind_target(target: Value) -> Result<(), LispError> {
    if let ValueRef::Sym(name) = target.unpack() {
        bind(name)?;
    }
    Ok(())
}

/// Bring every name a `fn` parameter list binds into scope: the required parameters, each
/// `&optional` one (bare, or the head of its `(name default)` pair) and the `&` rest one.
/// The markers themselves bind nothing. A malformed list binds what it can; the evaluator
/// reports the malformation.
pub(super) fn bind_params(heap: &Heap, params: Value) -> Result<(), LispError> {
    let Some(items) = heap.list_items(params) else {
        return Ok(());
    };
    let mut optional = false;
    for item in items {
        match item.unpack() {
            ValueRef::Sym(name) if value::symbol_is(name, kw::AMP_OPTIONAL) => optional = true,
            ValueRef::Sym(name) if value::symbol_is(name, kw::AMP) => {}
            ValueRef::Sym(name) => bind(name)?,
            // Only an `&optional` parameter is a list that names a binder, `(name default)`;
            // a list anywhere else is a pattern the lowering has not taken apart (`'x` is
            // `(quote x)`), and binds nothing here.
            ValueRef::Pair(_) if optional => {
                if let Some(ValueRef::Sym(name)) = heap
                    .list_items(item)
                    .and_then(|parts| parts.first().map(|part| part.unpack()))
                {
                    bind(name)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
