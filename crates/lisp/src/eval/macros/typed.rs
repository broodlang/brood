//! Typed parameters (ADR-420) in the expander: `(defn area (w:number h:number -> number) …)`.
//!
//! The types come out of the parameter list HERE, in the compile pass every engine shares, so
//! the tree-walker, the VM, the JIT, hygiene, resolve and the checker's walk only ever see
//! plain binders — nothing downstream can bind a variable literally named `w:number`. What the
//! types declare survives as the `(%register-sig 'NAME '(T… -> R))` a named definition emits
//! beside itself: the checker reads it as a declared signature, and the contracts enforce it
//! when armed. An anonymous `fn` has no name, so its declaration rides beside it instead —
//! `(%typed-fn '(T… -> R) (fn PLAIN …))`, an identity the compiler drops (`typed_fn_lambda`)
//! and the checker reads as the lambda's signature. An unannotated parameter or a missing
//! `->` is the placeholder `_`, which the checker reads as "not declared" — the parameter is
//! unconstrained and the result keeps its inferred type — so annotating one parameter never
//! costs the inference of the rest.
//!
//! Not supported, and refused with an error rather than mis-bound: a type inside a
//! destructuring pattern, a typed binder in `let`/`letrec`, and types on a multi-clause `fn`
//! (declare those with `(sig f (and …))`). The parameter-text rules are
//! `crate::syntax::typed`'s, shared with the readers of unexpanded source.

use super::*;
use crate::syntax::typed::{self as syntax_typed, TypedName};

/// The placeholder for a position the parameter list does not type.
pub(crate) const UNDECLARED: &str = "_";

/// A symbol's typed split, or `None` for anything else.
fn typed_name(v: Value) -> Option<TypedName<'static>> {
    match v.unpack() {
        ValueRef::Sym(s) => syntax_typed::split(value::symbol_name_ref(s)),
        _ => None,
    }
}

fn is_sym(v: Value, name: &str) -> bool {
    matches!(v.unpack(), ValueRef::Sym(s) if value::symbol_is(s, name))
}

/// Does `v` (a pattern) mention a typed symbol or the `->` marker anywhere inside it? A walk
/// with an explicit work list, not recursion: it runs on every parameter list of a `fn` or
/// `let` BEFORE the pattern compiler's depth cap can refuse one, and a pattern built at
/// runtime can nest past any native stack — a 100 000-level vector overflowed this walk
/// and aborted the runtime, where the cap meant to raise "nested deeper than 64".
fn mentions_types(heap: &Heap, v: Value) -> bool {
    let mut work = vec![v];
    while let Some(v) = work.pop() {
        if typed_name(v).is_some() || is_sym(v, syntax_typed::ARROW) {
            return true;
        }
        match v.unpack() {
            ValueRef::Pair(_) | ValueRef::Vector(_) => {
                if let Some(items) = heap.list_or_vector_items(v) {
                    work.extend(items);
                }
            }
            ValueRef::Map(id) => {
                for (k, val) in heap.map_entries(id) {
                    work.push(k);
                    work.push(val);
                }
            }
            _ => {}
        }
    }
    false
}

/// A parameter list with its types taken out: the plain list and the signature it declares,
/// `(T… -> R)` with `_` for each position left untyped. `None` when the list carries no type
/// at all — the common case, which the expander leaves exactly as written.
pub(super) fn strip_param_list(
    heap: &mut Heap,
    who: &str,
    params: Value,
) -> Result<Option<(Value, Value)>, LispError> {
    let Some(items) = heap.list_or_vector_items(params) else {
        return Ok(None);
    };
    if !matches!(params.unpack(), ValueRef::Pair(_)) {
        return Ok(None);
    }
    if !items.iter().any(|&i| mentions_types(heap, i)) {
        return Ok(None);
    }
    let undeclared = value::sym(UNDECLARED);
    let mut plain: Vec<Value> = Vec::with_capacity(items.len());
    let mut types: Vec<Value> = Vec::with_capacity(items.len() + 2);
    let mut ret = undeclared;
    let mut i = 0;
    while i < items.len() {
        let p = items[i];
        if is_sym(p, syntax_typed::ARROW) {
            match items.get(i + 1) {
                Some(&r) if i + 2 == items.len() => {
                    ret = r;
                    break;
                }
                Some(_) => {
                    return Err(LispError::runtime(format!(
                        "{who}: `-> TYPE` must end the parameter list, got {}",
                        crate::syntax::printer::print(heap, params)
                    )));
                }
                None => {
                    return Err(LispError::runtime(format!(
                        "{who}: `->` in a parameter list needs a result type after it"
                    )));
                }
            }
        }
        match typed_name(p) {
            Some(TypedName::Inline(name, ty)) => {
                plain.push(value::sym(name));
                types.push(value::sym(ty));
                i += 1;
            }
            Some(TypedName::Next(name)) => {
                let Some(&ty) = items.get(i + 1) else {
                    return Err(LispError::runtime(format!(
                        "{who}: parameter `{name}:` needs a type after it, like `{name}: (list int)`"
                    )));
                };
                plain.push(value::sym(name));
                types.push(ty);
                i += 2;
            }
            None => {
                if is_sym(p, kw::AMP_OPTIONAL) || is_sym(p, kw::AMP) || is_sym(p, kw::AMP_REST) {
                    plain.push(p);
                    types.push(if is_sym(p, kw::AMP_REST) {
                        value::sym(kw::AMP)
                    } else {
                        p
                    });
                    i += 1;
                    continue;
                }
                // `(name:type default)` — an `&optional` slot with a default.
                if matches!(p.unpack(), ValueRef::Pair(_))
                    && let Some(slot) = heap.list_or_vector_items(p)
                    && let Some(&head) = slot.first()
                    && let Some(TypedName::Inline(name, ty)) = typed_name(head)
                    && !slot[1..].iter().any(|&d| mentions_types(heap, d))
                {
                    let mut rebuilt = vec![value::sym(name)];
                    rebuilt.extend_from_slice(&slot[1..]);
                    plain.push(heap.list(rebuilt));
                    types.push(value::sym(ty));
                    i += 1;
                    continue;
                }
                if mentions_types(heap, p) {
                    return Err(LispError::runtime(format!(
                        "{who}: a type inside a destructuring pattern is not supported — got {}; \
                         bind the parameter to a name and type that instead",
                        crate::syntax::printer::print(heap, p)
                    )));
                }
                plain.push(p);
                types.push(undeclared);
                i += 1;
            }
        }
    }
    types.push(value::sym(syntax_typed::ARROW));
    types.push(ret);
    let plain = heap.list(plain);
    let sig = heap.list(types);
    Ok(Some((plain, sig)))
}

/// The signature a typed parameter list declares, `(T… -> R)`, or `None` for an untyped (or
/// malformed) one — for the readers of raw source that need the declaration without
/// expanding the definition (the std signature index).
pub(crate) fn typed_param_sig(heap: &mut Heap, params: Value) -> Option<Value> {
    strip_param_list(heap, "defn", params)
        .ok()
        .flatten()
        .map(|(_, sig)| sig)
}

/// For a raw `(defn NAME (params…) …)` / `(defn- …)` whose parameter list carries types: the
/// name and each type form it writes, labelled — the parameter's name, or `->` for the result.
/// `None` for any other form or an untyped list. Reads only; nothing is allocated (a type
/// written `x:int` is the interned symbol `int`). For the readers of raw source that hold a
/// typed definition to what they hold a `(sig …)` to: the declaration check reports an
/// unreadable type here exactly as in a `sig` (ADR-259).
pub(crate) fn defn_param_types(heap: &Heap, form: Value) -> Option<(Symbol, Vec<(String, Value)>)> {
    let items = heap.list_items(form)?;
    let ValueRef::Sym(head) = items.first()?.unpack() else {
        return None;
    };
    if !value::symbol_is(head, "defn") && !value::symbol_is(head, "defn-") {
        return None;
    }
    let ValueRef::Sym(name) = items.get(1)?.unpack() else {
        return None;
    };
    let params = *items.get(2)?;
    if !matches!(params.unpack(), ValueRef::Pair(_)) {
        return None;
    }
    let list = heap.list_items(params)?;
    let mut out = Vec::new();
    let mut i = 0;
    while i < list.len() {
        let p = list[i];
        if is_sym(p, syntax_typed::ARROW) {
            if let Some(&ret) = list.get(i + 1) {
                out.push((syntax_typed::ARROW.to_string(), ret));
            }
            break;
        }
        let slot_head = match p.unpack() {
            ValueRef::Pair(_) => heap.list_items(p).and_then(|slot| slot.first().copied()),
            _ => Some(p),
        };
        match slot_head.and_then(typed_name) {
            Some(TypedName::Inline(n, ty)) => out.push((n.to_string(), value::sym(ty))),
            Some(TypedName::Next(n)) => {
                if let Some(&ty) = list.get(i + 1) {
                    out.push((n.to_string(), ty));
                }
                i += 1;
            }
            None => {}
        }
        i += 1;
    }
    (!out.is_empty()).then_some((name, out))
}

/// A raw parameter list of plain binders, typed or not — `(x:int y -> string)` — as each
/// binder's name, the type form it writes (`None` where untyped, or typed `_`), and the
/// result form after `->` (`None` when absent or `_`). `None` for a list holding a pattern,
/// an `&` marker or a malformed `->`: the shapes a reader of unexpanded source types only
/// when every binder is a plain name. Reads only; nothing is allocated.
pub(crate) fn plain_param_types(
    heap: &Heap,
    params: Value,
) -> Option<(Vec<Symbol>, Vec<Option<Value>>, Option<Value>)> {
    let list = heap.list_items(params)?;
    let declared = |ty: Value| (!is_sym(ty, UNDECLARED)).then_some(ty);
    let mut names = Vec::with_capacity(list.len());
    let mut types = Vec::with_capacity(list.len());
    let mut ret = None;
    let mut index = 0;
    while index < list.len() {
        let param = list[index];
        if is_sym(param, syntax_typed::ARROW) {
            if index + 2 != list.len() {
                return None;
            }
            ret = declared(list[index + 1]);
            break;
        }
        match typed_name(param) {
            Some(TypedName::Inline(name, ty)) => {
                names.push(value::intern(name));
                types.push(declared(value::sym(ty)));
            }
            Some(TypedName::Next(name)) => {
                names.push(value::intern(name));
                types.push(declared(*list.get(index + 1)?));
                index += 1;
            }
            None => {
                let ValueRef::Sym(name) = param.unpack() else {
                    return None;
                };
                if value::symbol_name_ref(name).starts_with('&') {
                    return None;
                }
                names.push(name);
                types.push(None);
            }
        }
        index += 1;
    }
    Some((names, types, ret))
}

/// Is `fn_items` (`(fn …)`'s items) a multi-clause `fn` — every form after `fn`, past an
/// optional leading docstring, a clause? The docstring is peeled as `lower_fn` peels it: a
/// documented `(defn f "doc" ((x:int) …) ((x y) …))` reached here as `(fn "doc" clause…)`,
/// was read as single-clause, and bound a variable literally named `x:int`.
fn fn_is_multi_clause(heap: &Heap, fn_items: &[Value]) -> bool {
    fn_items.len() > 1 && fn_is_arity_multi_clause(heap, fn_items)
}

/// Refuse types on a multi-clause `fn` — every clause's parameter list is checked, so a typed
/// clause cannot slip through as a binder named `x:int`.
fn refuse_multi_clause_types(heap: &Heap, fn_items: &[Value]) -> Result<(), LispError> {
    for &clause in &fn_items[1..] {
        if let Some(clause_items) = heap.list_or_vector_items(clause)
            && let Some(&params) = clause_items.first()
            && mentions_types(heap, params)
        {
            return Err(LispError::runtime(
                "fn: typed parameters are not supported on a multi-clause fn — declare its \
                 signature with (sig NAME (and (T… -> R) …))"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// The marker an anonymous typed `fn` expands under: `(%typed-fn '(T… -> R) (fn PLAIN …))`.
/// At runtime the prelude's `%typed-fn` returns the function unchanged; the checker reads the
/// quoted signature as the lambda's declaration, as it reads `%register-sig` for a `defn`.
pub(crate) const TYPED_FN: &str = "%typed-fn";

/// The plain `(fn …)` inside a `(%typed-fn 'SIG (fn …))`, or `None` for any other form. The
/// marker is an identity at runtime, so whatever treats a `fn`-valued form specially — the
/// compiler's direct self-recursion for a `letrec`/`let` binder, the tree-walker's frame for
/// it — sees through it to the lambda: wrapped, a typed `letrec`
/// loop lost the self-recursion path and its enclosing closure fell to the tree-walker.
pub(crate) fn typed_fn_lambda(heap: &Heap, form: Value) -> Option<Value> {
    let items = heap.list_items(form)?;
    let [head, quoted, lambda] = items.as_slice() else {
        return None;
    };
    if !is_sym(*head, TYPED_FN) {
        return None;
    }
    let quote_head = heap.list_items(*quoted)?.first().copied();
    if !quote_head.is_some_and(|h| is_sym(h, kw::QUOTE)) {
        return None;
    }
    let lambda_head = heap.list_items(*lambda)?.first().copied();
    lambda_head
        .is_some_and(|h| is_sym(h, kw::FN))
        .then_some(*lambda)
}

/// `(fn PARAMS body…)` with typed parameters → `(%typed-fn '(T… -> R) (fn PLAIN body…))`. An
/// anonymous `fn` has no name to register a signature under, so the declaration rides beside
/// it: the checker types the body under it, holds the result to it and types the value as
/// that arrow, where a stripped list left the types checked by nothing. `None` when there is
/// nothing to strip.
pub(super) fn strip_fn(heap: &mut Heap, items: &[Value]) -> Result<Option<Value>, LispError> {
    if fn_is_multi_clause(heap, items) {
        refuse_multi_clause_types(heap, items)?;
        return Ok(None);
    }
    let Some(&params) = items.get(1) else {
        return Ok(None);
    };
    let Some((plain, sig)) = strip_param_list(heap, "fn", params)? else {
        return Ok(None);
    };
    let mut lambda = items.to_vec();
    lambda[1] = plain;
    let lambda = heap.list(lambda);
    let quoted_sig = heap.list(vec![value::sym(kw::QUOTE), sig]);
    Ok(Some(heap.list(vec![
        value::sym(TYPED_FN),
        quoted_sig,
        lambda,
    ])))
}

/// `(defmacro NAME PARAMS body…)` with typed parameters → plain ones. A macro's arguments
/// are forms, so its types declare nothing; they are accepted for symmetry and dropped.
pub(super) fn strip_defmacro(heap: &mut Heap, items: &[Value]) -> Result<Option<Value>, LispError> {
    let Some(&params) = items.get(2) else {
        return Ok(None);
    };
    let Some((plain, _sig)) = strip_param_list(heap, "defmacro", params)? else {
        return Ok(None);
    };
    let mut out = items.to_vec();
    out[2] = plain;
    Ok(Some(heap.list(out)))
}

/// `(def NAME (fn PARAMS body…))` with typed parameters →
/// `(do (def NAME (fn PLAIN body…)) (%register-sig 'NAME '(T… -> R)))` — the declaration
/// `(sig NAME …)` would make, through the primitive so it works anywhere, the prelude
/// included. `None` for any other `def`.
pub(super) fn typed_def(heap: &mut Heap, items: &[Value]) -> Result<Option<Value>, LispError> {
    let [_, name, rhs] = items else {
        return Ok(None);
    };
    if !matches!(name.unpack(), ValueRef::Sym(_)) {
        return Ok(None);
    }
    let Some(fn_items) = heap.list_or_vector_items(*rhs) else {
        return Ok(None);
    };
    if !matches!(rhs.unpack(), ValueRef::Pair(_))
        || !fn_items.first().is_some_and(|&h| is_sym(h, kw::FN))
    {
        return Ok(None);
    }
    if fn_is_multi_clause(heap, &fn_items) {
        refuse_multi_clause_types(heap, &fn_items)?;
        return Ok(None);
    }
    let Some(&params) = fn_items.get(1) else {
        return Ok(None);
    };
    let who = format!("defn {}", crate::syntax::printer::print(heap, *name));
    let Some((plain, sig)) = strip_param_list(heap, &who, params)? else {
        return Ok(None);
    };
    let mut new_fn = fn_items.clone();
    new_fn[1] = plain;
    let new_fn = heap.list(new_fn);
    let def = heap.list(vec![value::sym(kw::DEF), *name, new_fn]);
    let quote = value::sym(kw::QUOTE);
    let quoted_name = heap.list(vec![quote, *name]);
    let quoted_sig = heap.list(vec![quote, sig]);
    let register = heap.list(vec![value::sym("%register-sig"), quoted_name, quoted_sig]);
    Ok(Some(heap.list(vec![value::sym(kw::DO), def, register])))
}

/// Refuse a typed binder in `let`/`letrec`: the type would declare nothing, and the binder
/// would otherwise be a variable literally named `x:int`.
pub(super) fn refuse_typed_let(heap: &Heap, who: &str, items: &[Value]) -> Result<(), LispError> {
    let Some(binds) = items.get(1).and_then(|&b| heap.list_or_vector_items(b)) else {
        return Ok(());
    };
    for target in binds.iter().step_by(2) {
        if mentions_types(heap, *target) {
            return Err(LispError::runtime(format!(
                "{who}: a binding cannot carry a type ({}) — types go on function parameters",
                crate::syntax::printer::print(heap, *target)
            )));
        }
    }
    Ok(())
}

/// `(def x:int 3)` — a typed NAME on a `def`. Typed parameters (ADR-420) belong to
/// function parameter lists; on a `def` the colon spelling silently bound a global literally
/// named `x:int`, leaving `x` unbound. Refuse it, naming the two spellings that work.
pub(super) fn refuse_typed_def_name(heap: &Heap, items: &[Value]) -> Result<(), LispError> {
    let Some(&name @ Value::Sym(sym)) = items.get(1) else {
        return Ok(());
    };
    let text = value::symbol_name_ref(sym);
    let Some(typed) = crate::syntax::typed::split(text) else {
        return Ok(());
    };
    let plain = typed.name();
    let declare = match typed {
        crate::syntax::typed::TypedName::Inline(_, ty) => format!("(sig {plain} {ty})"),
        crate::syntax::typed::TypedName::Next(_) => format!("(sig {plain} TYPE)"),
    };
    Err(LispError::runtime(format!(
        "def: `{}` is a typed name, which only a function parameter can carry — write \
         (def {plain} …) and declare its type with {declare}",
        crate::syntax::printer::print(heap, name)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parameter pattern nested far past any native stack is walked without recursion:
    /// `strip_param_list` asks `mentions_types` about every parameter before the pattern
    /// compiler's depth cap can refuse it, and a recursive walk of a 100 000-level vector
    /// overflowed the stack and aborted the runtime (`robustness_limits_test`'s `fn` and `let`
    /// cases, on this machine). Run on a thread with a deliberately small stack, so the
    /// recursive walk could not pass by luck of a large one.
    #[test]
    fn a_deep_untyped_pattern_is_walked_on_a_small_stack() {
        let answer = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let mut heap = Heap::new();
                let mut deep = value::sym("x");
                for _ in 0..100_000 {
                    deep = heap.alloc_vector(vec![deep]);
                }
                let params = heap.list(vec![deep]);
                strip_param_list(&mut heap, "fn", params)
                    .expect("an untyped list strips without error")
                    .is_none()
            })
            .expect("spawn the small-stack thread")
            .join()
            .expect("the walk did not overflow");
        assert!(answer, "an untyped parameter list carries no signature");
    }
}
