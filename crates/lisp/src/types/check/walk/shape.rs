//! The syntax-shape readers the whole checker shares: which heads are special, what a
//! `fn` form's params and defaults are, a `let`'s bindings, a pattern's binders, a
//! `do` body, a list's items. Pure readers — nothing here warns.

use super::*;

/// `symbol_name(s)` is a `String` allocation; we only need the spelling on
/// the rare *error* paths (unbound / arity / type-disjoint). Wrap as a
/// no-arg helper so the hot path (the whole `is_local` / `is_syntactic` /
/// `is_globally_bound` / `curated_sig` short-circuit) skips it entirely.
#[inline]
pub(super) fn name_of(s: Symbol) -> String {
    value::symbol_name(s)
}

/// Is `s` a **gensym temporary** — a `<prefix>__<digits>` name minted by macro
/// expansion (`value::gensym`)? Such a binding is compiler-introduced, so the
/// lints that only want *surface* (user-written) names — the unused-let-binding
/// lint and the broadened dead-clause lint (ADR-131) — exempt it: warning on a
/// name the user can't rename is noise. (A rare hand-written `x__1` is only a
/// missed warning, which these lints already tolerate.)
pub(in crate::types::check) fn is_gensym_sym(s: Symbol) -> bool {
    name_of(s)
        .rsplit_once("__")
        .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// True when `head` is a function-literal head — `fn` or its synonym `lambda`.
/// Both spell the same special form (`lambda` Just Works, see the evaluator), and
/// both survive macro expansion as their original head, so every reader of a `fn`
/// shape (here, [`guards::lambda_ret`], and `protocol`'s arity reader) must accept
/// the two. Single source of truth so they can't drift.
pub(in crate::types::check) fn is_fn_head(head: Symbol) -> bool {
    value::symbol_is(head, kw::FN)
}

/// Conservative reachability scan: does `sym` appear as a `Value::Sym`
/// *anywhere* in `form` — recursively, including in binder positions and
/// inside `quote`? Used by the unused-`let`-binding lint. False negatives are
/// acceptable (a shadowed reference counted as "used"); zero false positives.
pub(in crate::types::check) fn sym_appears_in(heap: &Heap, form: Value, sym: Symbol) -> bool {
    // A worklist, not recursion: it recursed on the CDR too, so a long flat list was as
    // deep as it was long, and a deep `let` body — the one shape the deep-form tests
    // did not build — overflowed the stack.
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        match v {
            Value::Sym(s) if s == sym => return true,
            Value::Pair(pid) => {
                let (car, cdr) = heap.pair(pid);
                work.push(cdr);
                work.push(car);
            }
            Value::Vector(vid) => work.extend(heap.vector(vid).iter().copied()),
            // Map literals (`{:k v …}`) are heap maps, not pairs — scan both keys
            // and values, or a binding used only inside a `{…}` (very common: the
            // editor's minibuffer specs, `{:start s :end e}` edit forms) is falsely
            // reported unused, breaking the "false negatives only" invariant.
            Value::Map(mid) => {
                for &(k, val) in heap.map_entries(mid).iter() {
                    work.push(k);
                    work.push(val);
                }
            }
            _ => {}
        }
    }
    false
}

/// Collect every `Value::Sym` that appears anywhere in `form` — recursively,
/// including binder positions. Used by the unused-`:use` and unused-private-`defn`
/// lints to build the full reference set of a file in one pass.
pub(super) fn collect_syms_into(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    // Deep-form stack safety — same stacker remedy as the walkers above.
    stacker::maybe_grow(64 * 1024, 1024 * 1024, || {
        collect_syms_into_inner(heap, form, out)
    })
}

pub(super) fn collect_syms_into_inner(heap: &Heap, form: Value, out: &mut HashSet<Symbol>) {
    match form {
        Value::Sym(s) => {
            out.insert(s);
        }
        Value::Pair(pid) => {
            let (car, cdr) = heap.pair(pid);
            collect_syms_into(heap, car, out);
            collect_syms_into(heap, cdr, out);
        }
        Value::Vector(vid) => {
            for &v in heap.vector(vid).iter() {
                collect_syms_into(heap, v, out);
            }
        }
        Value::Map(mid) => {
            for (k, v) in heap.map_entries(mid) {
                collect_syms_into(heap, k, out);
                collect_syms_into(heap, v, out);
            }
        }
        _ => {}
    }
}

/// Collect every symbol that appears anywhere in `forms`.
pub(in crate::types::check) fn collect_all_syms(heap: &Heap, forms: &[Value]) -> HashSet<Symbol> {
    let mut out = HashSet::new();
    for &form in forms {
        collect_syms_into(heap, form, &mut out);
    }
    out
}

/// What the walk does at a head symbol. `Generic` is the fall-through for any
/// head that isn't one of the recognised special forms / skip-body markers —
/// the walk treats it as a normal call (resolves sig + arity, checks for
/// unbound). One [`SymbolMap`] lookup decides: pre-consolidation each call
/// allocated a `String` via `value::symbol_name` just to feed a chain of
/// `matches!(name.as_str(), "if" | …)` plus `skips_body(&name)` — that was
/// the hot allocation the review flagged. (`eval.rs` uses the same
/// `SymbolMap` pattern on its own loop.)
#[derive(Clone, Copy)]
pub(in crate::types::check) enum SpecialHead {
    /// `quote` / `comment` — return without descending. These hold *syntax*, not
    /// evaluated code, so nothing inside them is a reference.
    SkipBody,
    /// `quasiquote` (ADR-260) — a template is data, *except* for its `~` / `~@` escapes, which
    /// are ordinary code evaluated at expansion time in the macro's own scope. Descend
    /// to those and check them; everything else in the template is quoted. Skipping
    /// the whole form (as this did until the reach gate went in) is the same silent
    /// coverage boundary as KI-67 (`try`) and KI-70 (container literals): a rename
    /// that kills a name used inside `~(…)` left every gate green.
    Quasiquote,
    /// `try` / `%try` / `error-of` / `assert-error` — descend, but with every
    /// lint except `unbound` suppressed (KI-67). These forms deliberately
    /// exercise failures, so `(error-of (first 5))` must stay silent about the
    /// type misuse — that is the whole point of the form. An **unbound symbol**
    /// is a different class: it is never the failure under test unless the
    /// author says so, and skipping the body entirely meant a rename could
    /// leave a call site dead inside a `try` with every gate green. A test that
    /// really does assert on an unbound name opts out with
    /// `(check-allow :unbound …)`.
    ErrorTesting,
    If,
    /// `let` / `let*` — sequential bind, no pre-binding.
    Let,
    /// `letrec` — pre-bind every name before walking RHSs (mutual recursion).
    Letrec,
    /// `fn` / `lambda` — open a fresh scope with the params bound.
    Fn,
    /// `def` — `name` is a binder, value is an expression.
    Def,
    /// `defn` / `defmacro` — same shape as `fn`/`lambda` plus a binder name.
    Defn,
}

pub(in crate::types::check) static SPECIAL_HEAD: LazyLock<SymbolMap<SpecialHead>> =
    LazyLock::new(|| {
        use SpecialHead::*;
        [
            (kw::QUOTE, SkipBody),
            (kw::QUASIQUOTE, Quasiquote),
            (kw::TRY, ErrorTesting),
            (kw::ERROR_OF, ErrorTesting),
            (kw::ASSERT_ERROR, ErrorTesting),
            (kw::TRY_PRIM, ErrorTesting),
            // `(comment …)` expands to `nil` — its body is never evaluated, so
            // checking it would flag names that intentionally don't resolve (a
            // sketched call, a snippet from another project). The whole point of the
            // form is to hold code that doesn't run.
            (kw::COMMENT, SkipBody),
            (kw::IF, If),
            (kw::LET, Let),
            (kw::LETREC, Letrec),
            (kw::FN, Fn),
            (kw::DEF, Def),
            (kw::DEFN, Defn),
            (kw::DEFMACRO, Defn),
        ]
        .into_iter()
        .map(|(n, k)| (value::intern(n), k))
        .collect()
    });

/// Is `form` a `(%make-macro …)` combination — the value a `defmacro` lowers to
/// once expanded? Recognises a file-local macro definition in the expanded tree
/// (where the `defmacro` head is gone, replaced by `(def name (%make-macro …))`).
pub(super) fn is_make_macro_form(heap: &Heap, form: Value) -> bool {
    matches!(list_items(heap, form).as_deref(),
        Some([Value::Sym(h), ..]) if value::symbol_is(*h, "%make-macro"))
}

/// Does the value form of a `def` resolve to a **variadic** `fn`/`lambda` — one
/// whose parameter list (in any arm of a multi-arity fn) contains a `&` rest
/// marker? Reads the `(def name (fn …))` shape `defn` expands to; `false` for a
/// non-`fn` value or a fixed-arity one.
pub(super) fn def_value_is_variadic(heap: &Heap, value_form: Value) -> bool {
    let Some(items) = fn_form_items(heap, value_form) else {
        return false;
    };
    // items = [fn, params-or-arm, body…]. A multi-arity fn has clause *lists*
    // (`((a) …) ((a & b) …)`); a single-arity fn has the param list directly.
    let rest = &items[1..];
    let rest = match rest.first() {
        // Peel a leading docstring for the single-arity shape.
        Some(Value::Str(_)) if rest.len() > 1 => &rest[1..],
        _ => rest,
    };
    rest.iter().any(|&part| part_has_rest(heap, part))
}

/// True if `part` — either a single-arity parameter list (`(a & b)`) or a
/// multi-arity clause (`((a & b) body…)`) — introduces a `&` rest parameter.
/// Checks the form as a param list, and if its first element is itself a list
/// (the clause shape), checks that nested param list too.
pub(super) fn part_has_rest(heap: &Heap, part: Value) -> bool {
    if params_have_rest(heap, part) {
        return true;
    }
    // Multi-arity clause: ((params) body…) — look at the inner param list.
    match list_items(heap, part) {
        Some(items) => items
            .first()
            .is_some_and(|&inner| params_have_rest(heap, inner)),
        None => false,
    }
}

/// The arity a `fn`/`lambda` **definition** admits, read off its own parameter
/// list(s) — the same fact [`sigs::arity_of`](crate::types::check::sigs::arity_of) reads from a
/// *loaded* closure, recovered from the form for a function defined in the file
/// being checked (which is never loaded — see [`Ctx::file_arity`](crate::types::check::ctx::Ctx)).
///
/// Multi-arm closures collapse to the interval **hull** (smallest min, largest max),
/// exactly as `arity_of` does: sound (it over-accepts a gap between two arms) and
/// never a false positive. `None` when the form isn't a `fn`, or when a parameter
/// list isn't a readable list/vector — the caller then leaves the call unchecked,
/// which is the pre-existing behaviour.
pub(in crate::types::check) fn fn_form_arity(heap: &Heap, value_form: Value) -> Option<Arity> {
    let items = fn_form_items(heap, value_form)?;
    let forms = &items[1..];
    // A leading docstring is not a parameter list.
    let forms = match forms.first() {
        Some(Value::Str(_)) if forms.len() > 1 => &forms[1..],
        _ => forms,
    };
    if crate::eval::macros::fn_is_arity_multi_clause(heap, &items) {
        let mut hull: Option<Arity> = None;
        for &clause in forms {
            let plist = *list_items(heap, clause)?.first()?;
            let a = params_arity(heap, plist)?;
            hull = Some(match hull {
                None => a,
                Some(h) => Arity {
                    min: h.min.min(a.min),
                    max: match (h.max, a.max) {
                        (Some(x), Some(y)) => Some(x.max(y)),
                        _ => None,
                    },
                },
            });
        }
        return hull;
    }
    params_arity(heap, *forms.first()?)
}

/// The arity one parameter list admits: the required binders before any marker,
/// widened by `&optional` (a range) and by `&`/`&rest` (unbounded). Mirrors the
/// closure `Arm::min_arity`/`max_arity` the runtime computes from the same list.
/// `None` for a form that isn't a parameter list at all (so the caller stays silent
/// rather than guessing).
pub(super) fn params_arity(heap: &Heap, params: Value) -> Option<Arity> {
    let items = match params {
        Value::Vector(id) => heap.vector(id).to_vec(),
        // `()` reads as `nil` — a real, empty parameter list.
        Value::Nil => Vec::new(),
        Value::Pair(_) => list_items(heap, params)?,
        _ => return None,
    };
    let mut required = 0usize;
    let mut optional = 0usize;
    let mut seen_optional = false;
    for item in items {
        if let Value::Sym(s) = item {
            if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST) {
                // Everything from here on is collected into one rest binder.
                return Some(Arity::at_least(required));
            }
            if value::symbol_is(s, kw::AMP_OPTIONAL) {
                seen_optional = true;
                continue;
            }
        }
        if seen_optional {
            optional += 1;
        } else {
            required += 1;
        }
    }
    Some(if seen_optional {
        Arity::range(required, required + optional)
    } else {
        Arity::exact(required)
    })
}

/// True if the parameter-list form `params` contains a `&` (or `&rest`) marker —
/// i.e. the function it belongs to is variadic. A vector or list param list is
/// accepted; a non-list form (e.g. a docstring) yields `false`.
pub(super) fn params_have_rest(heap: &Heap, params: Value) -> bool {
    let items = match params {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => match list_items(heap, params) {
            Some(v) => v,
            None => return false,
        },
        _ => return false,
    };
    items.iter().any(|p| {
        matches!(p, &Value::Sym(s)
            if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST))
    })
}

/// `(quote SYM)` → the symbol's name; used to read `register-impl`'s quoted ability / op.
pub(super) fn quoted_sym_name(heap: &Heap, v: Value) -> Option<String> {
    let items = list_items(heap, v)?;
    if items.len() == 2 && matches!(items[0], Value::Sym(s) if value::symbol_is(s, kw::QUOTE)) {
        match items[1] {
            Value::Sym(s) | Value::Keyword(s) => Some(value::symbol_name(s)),
            _ => None,
        }
    } else {
        None
    }
}

/// The bare (last `/`-segment) name of a symbol head — `ability/register-impl` and a
/// bare `%register-impl` both read as `"%register-impl"`.
pub(super) fn head_name(sym: Symbol) -> String {
    let full = value::symbol_name(sym);
    full.rsplit('/').next().unwrap_or(&full).to_string()
}

/// The items of `form` when it is an `(fn …)` form, else `None` —
/// so `check_def` can recognise the `(def name (fn …))` shape that `defn`
/// expands to.
/// Is this `def`'s value a `fn` form — i.e. does the name denote a *function*, so that
/// a signature is the right thing to say about it?
pub(in crate::types::check) fn is_fn_value_form(heap: &Heap, form: Value) -> bool {
    fn_form_items(heap, form).is_some()
}

pub(in crate::types::check) fn fn_form_items(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    let items = list_items(heap, form)?;
    match items.first()? {
        &Value::Sym(s) if is_fn_head(s) => Some(items),
        _ => None,
    }
}

/// The set of parameter-binder symbols introduced by a `fn`/`defn`/`defmacro`
/// parameter list. Handles the three Brood shapes uniformly:
///
/// - positional: `(x y z)` → `{x, y, z}`
/// - optional:   `(x &optional (y 0))` → `{x, y}`
/// - rest:       `(x & ys)` → `{x, ys}`
///
/// `&` / `&optional` themselves are markers, not binders, so they're filtered
/// out. The result is *just* what would be in scope — used to seed `Ctx`
/// without false-flagging the inner body's references.
/// Does this parameter list end in a `& rest` tail? (The `&`/`&rest` marker; a
/// bare-symbol binder follows it.) Used to seed the rest binder as `list<elem>`.
pub(super) fn params_form_has_rest(heap: &Heap, form: Value) -> bool {
    let items = match form {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, form).unwrap_or_default(),
        _ => return false,
    };
    items.iter().any(|&it| {
        matches!(it, Value::Sym(s) if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST))
    })
}

/// Per binder of `form` (aligned with [`fn_params`]), the `&optional` default expression
/// — `(name default)` — or `None` for a plain parameter / an optional without one.
pub(in crate::types::check) fn fn_param_defaults(heap: &Heap, form: Value) -> Vec<Option<Value>> {
    let items = match form {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, form).unwrap_or_default(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::Sym(s) => {
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST)
                {
                    continue;
                }
                out.push(None);
            }
            Value::Pair(_) | Value::Vector(_) => {
                let inner = match item {
                    Value::Vector(id) => heap.vector(id).to_vec(),
                    _ => list_items(heap, item).unwrap_or_default(),
                };
                if let Some(Value::Sym(_)) = inner.first() {
                    out.push(inner.get(1).copied());
                }
            }
            _ => {}
        }
    }
    out
}

pub(in crate::types::check) fn fn_params(heap: &Heap, form: Value) -> Vec<Symbol> {
    let items = match form {
        Value::Vector(id) => heap.vector(id).to_vec(),
        Value::Nil | Value::Pair(_) => list_items(heap, form).unwrap_or_default(),
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::Sym(s) => {
                // Lock-free `symbol_is` to filter the parameter-list markers
                // — three name compares without ever allocating the spelling.
                if value::symbol_is(s, kw::AMP)
                    || value::symbol_is(s, kw::AMP_OPTIONAL)
                    || value::symbol_is(s, kw::AMP_REST)
                {
                    continue;
                }
                out.push(s);
            }
            // `&optional` defaults: `(name default)` — the binder is at [0].
            Value::Pair(_) | Value::Vector(_) => {
                let inner = match item {
                    Value::Vector(id) => heap.vector(id).to_vec(),
                    _ => list_items(heap, item).unwrap_or_default(),
                };
                if let Some(&Value::Sym(s)) = inner.first() {
                    out.push(s);
                }
            }
            _ => {}
        }
    }
    out
}

/// The binders of a destructuring pattern with the type each takes from a value of
/// `rhs_ty`: a flat `[a b c]` / `(a b c)` over a `(vector T)` / `(list T)` binds each name
/// to `T` — a short value is a match ERROR at runtime, never a nil-fill, so the element is
/// exactly `T` — and over a tuple to that position's type; a `& rest` binder collects a
/// `list<T>`. `_` binds nothing. Anything the position can't pin (a nested pattern, a
/// literal constraint beside it, an unknown RHS) binds to `None` — in scope, unknown —
/// which is what every binder got before this existed, when `(let ([x y w h] rect) (- w 1))`
/// read `w` as unknown under a `(vector int)` sig.
pub(in crate::types::check) fn pattern_bindings(
    heap: &Heap,
    pat: Value,
    rhs_ty: Option<&Ty>,
) -> Vec<(Symbol, Option<Ty>)> {
    let names = pattern_syms(heap, pat);
    let Some(items) = bindings(heap, pat) else {
        return names.into_iter().map(|s| (s, None)).collect();
    };
    let flat = items.iter().all(|it| matches!(it, Value::Sym(_)));
    let elem = rhs_ty.and_then(|t| t.elem_ty());
    let tuple = rhs_ty.and_then(|t| t.tuple_elems().cloned());
    if !flat || (elem.is_none() && tuple.is_none()) {
        return names.into_iter().map(|s| (s, None)).collect();
    }
    let mut out = Vec::new();
    let mut rest_next = false;
    let mut position = 0usize;
    for it in items {
        let Value::Sym(s) = it else { continue };
        let nm = name_of(s);
        if nm == "&" {
            rest_next = true;
            continue;
        }
        if rest_next {
            out.push((s, elem.clone().map(Ty::list_of)));
            rest_next = false;
            continue;
        }
        let ty = match &tuple {
            Some(elems) => elems.get(position).cloned(),
            None => elem.clone(),
        };
        position += 1;
        if nm != "_" {
            out.push((s, ty));
        }
    }
    out
}

/// The binder symbols of a destructuring pattern (`(a b)`, `[a b & rest]`,
/// nested `((a b) c)`) — every `Value::Sym` leaf except the `&` rest marker and
/// the `_` wildcard, which bind nothing. Literals (ints/keywords/strings) are
/// match constraints, not binders, so they're skipped. Used to put a pattern-let's
/// names in scope for the unbound-symbol check (a precise per-position type isn't
/// available, so each is bound to `None`).
pub(super) fn pattern_syms(heap: &Heap, pat: Value) -> Vec<Symbol> {
    let mut out = Vec::new();
    collect_pattern_syms(heap, pat, &mut out);
    out
}

pub(super) fn collect_pattern_syms(heap: &Heap, pat: Value, out: &mut Vec<Symbol>) {
    match pat {
        Value::Sym(s) => {
            let nm = name_of(s);
            if nm != "&" && nm != "_" {
                out.push(s);
            }
        }
        Value::Pair(_) | Value::Vector(_) => {
            if let Some(items) = bindings(heap, pat) {
                for it in items {
                    collect_pattern_syms(heap, it, out);
                }
            }
        }
        _ => {}
    }
}

/// Parse a `let` bindings form — accepts both `(name val name val …)` lists
/// and `[name val name val …]` vectors, the two shapes the reader emits.
pub(super) fn bindings(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    match form {
        Value::Vector(id) => Some(heap.vector(id).to_vec()),
        Value::Nil | Value::Pair(_) => list_items(heap, form),
        _ => None,
    }
}

/// The elements of a proper list, or `None` for an improper list / non-list.
/// `pub(super)` because `sigs` (`infer_sig`) and `guards` (`guard_assertion`,
/// `expr_ty`) all need to peel a list head off a call form.
/// The body forms of a `(do …)`, or `None` for anything else. `defn-`/`def-` expand to
/// one, so a walk over top-level forms that does not look inside a `do` cannot see a
/// module-private definition at all.
pub(in crate::types::check) fn do_body(heap: &Heap, form: Value) -> Option<Vec<Value>> {
    let items = list_items(heap, form)?;
    let head = items.first()?;
    if !matches!(head, Value::Sym(s) if value::symbol_is(*s, "do")) {
        return None;
    }
    Some(items[1..].to_vec())
}

pub(in crate::types::check) fn list_items(heap: &Heap, mut v: Value) -> Option<Vec<Value>> {
    let mut out = Vec::new();
    loop {
        match v {
            Value::Nil => return Some(out),
            Value::Pair(p) => {
                let (head, tail) = heap.pair(p);
                out.push(head);
                v = tail;
            }
            _ => return None,
        }
    }
}
