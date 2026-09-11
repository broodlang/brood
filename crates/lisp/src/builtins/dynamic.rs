//! Dynamic variables — the kernel for `defdyn`/`binding`; the surface macros are in the
//! prelude. A dynamic variable's *value* resolves through the per-process binding stack
//! in the `Heap` (see `Heap::env_get`), so reads need no primitive here — only the
//! declaration, the scoped rebind, and the predicate.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};
use crate::eval::compile::apply_engine;

use super::numeric::{arg, expect_symbol};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // dynamic variables (the `defdyn`/`binding` surface is Brood — see prelude)
    primitives.def(
        "%declare-dynamic",
        Arity::exact(1),
        Sig::new(vec![sym], nil_ty),
        &[],
        "",
        declare_dynamic,
    );
    // `%binding`'s first arg is the *list/vector of names*, second is the
    // *list/vector of values*, third is the thunk — the macro `binding` emits
    // these as `(quote (*a* *b* …))` + `[v1 v2 …]` + `(fn () …)`.
    primitives.def(
        "%binding",
        Arity::exact(3),
        Sig::new(vec![seq, seq, callable], any),
        &[],
        "",
        binding,
    );
    primitives.def(
        "reflect/dynamic?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["x"],
        "Whether x is a symbol declared dynamic with defdyn. Quote it: (reflect/dynamic? '*foo*).\n\n    (reflect/dynamic? 'map)   → false",
        dynamic_p,
    );
}

/// `(%declare-dynamic 'name)` — mark a symbol as a dynamic variable, so
/// `binding` will accept it (and `reflect/dynamic?` reports it). `defdyn` expands to
/// this plus a plain `def` of the default value.
pub(super) fn declare_dynamic(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let sym = expect_symbol(heap, "%declare-dynamic", arg(args, 0))?;
    value::mark_dynamic(sym);
    Ok(Value::symbol(sym))
}

/// `(reflect/dynamic? x)` — true when `x` is a symbol declared dynamic with `defdyn`.
/// A non-symbol is simply not dynamic (no error), so it composes in predicates.
pub(super) fn dynamic_p(args: &[Value], _: EnvId, _: &mut Heap) -> LispResult {
    Ok(Value::boolean(
        matches!(arg(args, 0), Value::Sym(s) if value::is_dynamic(s)),
    ))
}

/// `(%binding syms vals thunk)` — run `thunk` (no args) with each dynamic var in
/// `syms` bound to the matching value in `vals` for the dynamic extent of the
/// call, restoring the previous bindings on return *or* error. `syms` (a quoted
/// list) and `vals` (a vector) are equal-length sequences built by the `binding`
/// macro — both emitted as unshadowable literals, so a local rebinding of `list`
/// can't break the form. Every name must be declared dynamic (else it's almost
/// certainly a typo — a plain global won't track the rebind). The bindings live
/// in this process's heap, so they don't reach a `spawn`ed child.
pub(super) fn binding(args: &[Value], env: EnvId, heap: &mut Heap) -> LispResult {
    let syms = heap.seq_items(arg(args, 0))?;
    let vals = heap.seq_items(arg(args, 1))?;
    let thunk = arg(args, 2);
    // Validate every name up front, before pushing anything — so a bad `binding`
    // leaves the dynamic stack untouched rather than half-pushed.
    let mut names = Vec::with_capacity(syms.len());
    for s in &syms {
        let sym = expect_symbol(heap, "binding", *s)?;
        if !value::is_dynamic(sym) {
            return Err(LispError::runtime(format!(
                "binding: {} is not a dynamic variable (declare it with defdyn)",
                value::symbol_name(sym)
            )));
        }
        names.push(sym);
    }
    // Matched lengths are a guarantee of the `binding` MACRO, not of this primitive —
    // a raw `(%binding '(*x*) [] thunk)` used to read the missing value as `nil` via
    // `arg`'s default and bind that silently. Reject the shape instead: like the
    // not-dynamic check above, this raises before anything is pushed, so the dynamic
    // stack is untouched.
    if syms.len() != vals.len() {
        return Err(LispError::type_err(format!(
            "binding: {} name(s) but {} value(s) — each name needs exactly one value",
            syms.len(),
            vals.len()
        )));
    }
    for (&sym, &val) in names.iter().zip(vals.iter()) {
        heap.push_dynamic(sym, val);
    }
    let result = apply_engine(heap, thunk, &[], env);
    for _ in 0..names.len() {
        heap.pop_dynamic();
    }
    result
}
