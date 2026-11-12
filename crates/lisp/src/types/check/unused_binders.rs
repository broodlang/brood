//! The unused-binder lints, run on the un-expanded tree: a pattern clause's or an anonymous
//! `fn`'s binder its body never reads, a misplaced `:when` inside a head list, and unused
//! binders at every other binding form (`let` destructuring, loops, `match`/`receive`
//! arms, `catch`, `as->`), which expansion would otherwise hide behind synthetic lets.

use super::*;

/// **Unused pattern-binder lint**, run on the UN-EXPANDED tree.
///
/// A pattern clause's binder that its own body never reads is dead, and in a fold
/// callback it is worse than dead: `(fn ((acc "+") "+") ((acc num) (string/->number num)))`
/// ignores the accumulator in both arms, so the `reduce` never folds and the result is
/// whatever the last element produced. Nothing caught this. The unused-binding lint only
/// ever looked at `let`, and by the time the checker's main walk runs, a pattern clause
/// has been lowered to `%match` — where its binders become SYNTHETIC lets, which that lint
/// deliberately exempts (removing that exemption fires 69 times across std/ + tests/, so
/// it is load-bearing). Hence a surface pass: here the clause is still as written.
///
/// Per CLAUSE, because each clause has its own binder list, so `_`-prefixing one that is
/// deliberately ignored costs nothing — the same escape the `let` lint uses. A whole-fn
/// rule would be unsound: one arm may legitimately ignore what another arm reads.
pub(super) fn lint_unused_pattern_binders(heap: &Heap, form: Value, out: &mut Vec<Finding>) {
    lint_binders_walk(heap, form, true, out);
}

/// `lint_this` says whether *this* form, if it is a `fn`, is anonymous — it is cleared
/// only for the `fn` that is a definition's value. Children always reset it to true.
///
/// The distinction has to be made STRUCTURALLY, not by which tree we happen to be handed:
/// `nest check FILE` runs this over un-expanded forms (where a `defn` is still a `defn`
/// and never looked like a `fn`), while `check-string-structured` — the editor/LSP path —
/// runs it over expanded ones (where `defn` has already become `(def name (fn …))`). The
/// first version relied on that accident and so warned in the editor but not on the
/// command line for the same source, which is worse than either answer alone.
fn lint_binders_walk(heap: &Heap, root: Value, root_lint: bool, out: &mut Vec<Finding>) {
    // An explicit work stack, not recursion: this walks arbitrary user source, and the
    // checker is expected to survive deeply nested forms (`checker_survives_a_deep_let_body`
    // overflows a debug-build stack on the recursive version).
    let mut work = vec![(root, root_lint)];
    while let Some((v, lint_this)) = work.pop() {
        match v {
            Value::Vector(id) => {
                work.extend(heap.vector(id).iter().map(|&e| (e, true)));
            }
            // A map or set literal evaluates its keys and values, so a `fn` written there
            // (`{:on-complete (fn (mm text) mm)}`, the editor's callback specs) is as
            // anonymous as one in a vector and is linted the same.
            Value::Map(id) => {
                for &(key, entry) in heap.map_entries(id).iter() {
                    work.push((key, true));
                    work.push((entry, true));
                }
            }
            Value::Set(id) => work.extend(heap.map_entries(id).iter().map(|&(key, _)| (key, true))),
            Value::Pair(_) => {
                let Some(items) = list_items(heap, v) else {
                    continue;
                };
                // `Value` has no `PartialEq`; the definition's value is positionally items[2].
                let mut definition_value: Option<usize> = None;
                let mut binders_at: Option<(usize, BinderList)> = None;
                if let Some(&Value::Sym(head)) = items.first() {
                    binders_at = binder_list(heap, head, &items);
                    // Quoted/quasiquoted subtrees are data, and `comment` never runs.
                    if value::symbol_is(head, kw::QUOTE)
                        || value::symbol_is(head, kw::QUASIQUOTE)
                        || value::symbol_is(head, kw::COMMENT)
                    {
                        continue;
                    }
                    if lint_this && walk::is_fn_head(head) {
                        lint_fn_pattern_clauses(heap, &items, out);
                    }
                    lint_binding_form(heap, head, v, &items, out);
                    // `(def name (fn …))` — `defn`'s expansion. That `fn`'s parameters are
                    // the definition's PUBLISHED arglist (introspection, LSP hover, `doc`),
                    // where an unused one is frequently the contract: an ability impl, a
                    // callback of fixed shape, a dispatch arm. Only genuinely anonymous
                    // `fn`s are linted.
                    if value::symbol_is(head, kw::DEF) && items.len() >= 3 {
                        definition_value = Some(2);
                    }
                }
                for (i, it) in items.into_iter().enumerate() {
                    match binders_at {
                        Some((at, kind)) if at == i => work.extend(
                            binder_list_expressions(heap, it, kind)
                                .into_iter()
                                .map(|expression| (expression, true)),
                        ),
                        _ => work.push((it, definition_value != Some(i))),
                    }
                }
            }
            _ => {}
        }
    }
}

/// What a form's binder list holds besides its binders.
#[derive(Clone, Copy)]
enum BinderList {
    /// `(pattern expr pattern expr …)`: every second element is an expression.
    Bindings,
    /// A `fn` parameter list: only its `&optional` defaults are expressions.
    Parameters,
}

/// Where a form keeps its binding or parameter list, which the walk must not read as a
/// call: its first element is a binder, not an operator. `(let (match (get m :a) …) …)` is
/// not a `match` form, nor is `(fn (catch e) …)` a `catch`, and walking either as one
/// reported the expression after the name as an unused binder.
fn binder_list(heap: &Heap, head: Symbol, items: &[Value]) -> Option<(usize, BinderList)> {
    match value::symbol_name_ref(head) {
        "let" | "letrec" | "with" | "if-let" | "when-let" | "if-some" | "when-some" | "doseq"
        | "for" | "fold-for" => Some((1, BinderList::Bindings)),
        _ if walk::is_fn_head(head) => {
            let at = if matches!(items.get(1), Some(Value::Str(_))) && items.len() > 2 {
                2
            } else {
                1
            };
            let forms = items.get(at..)?;
            let is_parameter_list = matches!(forms.first(), Some(Value::Pair(_)))
                && !forms
                    .iter()
                    .all(|&form| crate::eval::macros::is_clause(heap, form));
            is_parameter_list.then_some((at, BinderList::Parameters))
        }
        _ => None,
    }
}

/// The expressions inside a binder list, which the walk does read.
fn binder_list_expressions(heap: &Heap, list: Value, kind: BinderList) -> Vec<Value> {
    match kind {
        BinderList::Bindings => heap
            .list_or_vector_items(list)
            .unwrap_or_default()
            .into_iter()
            .skip(1)
            .step_by(2)
            .collect(),
        BinderList::Parameters => optional_defaults(heap, list),
    }
}

/// The per-clause half of [`lint_unused_pattern_binders`], for an anonymous `fn` in either
/// spelling. `(fn (acc x) acc)` and `(fn ((acc x) acc))` are the same function, so they get
/// the same answer: the single-arm form is one clause whose head is the parameter list.
fn lint_fn_pattern_clauses(heap: &Heap, items: &[Value], out: &mut Vec<Finding>) {
    let forms = match items.get(1..) {
        Some([Value::Str(_), rest @ ..]) if !rest.is_empty() => rest,
        Some(rest) => rest,
        None => return,
    };
    if forms.is_empty() {
        return;
    }
    if !forms
        .iter()
        .all(|&c| crate::eval::macros::is_clause(heap, c))
    {
        // `(fn (params…) body…)`: the parameter list is the one clause head, and a head of
        // only symbols is an arity clause, so its binders are parameters.
        if let Some((&params @ (Value::Pair(_) | Value::Nil), body)) = forms.split_first() {
            let noun = match list_items(heap, params) {
                Some(names) if names.iter().all(|p| matches!(p, Value::Sym(_))) => {
                    "unused parameter"
                }
                _ => "unused pattern binding",
            };
            lint_clause_binders(heap, params, params, body, noun, out);
        }
        return;
    }
    let heads: Vec<Value> = forms
        .iter()
        .filter_map(|&c| list_items(heap, c)?.first().copied())
        .collect();
    if heads.len() != forms.len() {
        return;
    }
    // `:when` INSIDE the head list. It is clause syntax and belongs after the list —
    // `((acc num) :when (string? num) body…)` — so a head list containing it silently
    // becomes two extra parameters, the arity stops matching, and every call dies with a
    // `{:kind :match :context :fn …}` error that names the clauses rather than the mistake. There is
    // nothing else it can plausibly be: a head list holds patterns, and a pattern matching
    // the literal keyword `:when` in a parameter position is not a thing anyone writes.
    for (&clause, &head) in forms.iter().zip(&heads) {
        let Some(ps) = list_items(heap, head) else {
            continue;
        };
        if ps
            .iter()
            .any(|p| matches!(p, Value::Keyword(k) if value::symbol_is(*k, "when")))
            && let Some(pos) = heap.form_pos_only(clause)
        {
            out.push(Finding::new(
                Lint::Suspicious,
                Some(pos),
                concat!(
                    "`:when` belongs after the head list, not inside it — ",
                    "write `((a b) :when (guard a) body…)`. Inside, it is two more ",
                    "parameters, so the clause stops matching and every call fails ",
                    "with a match error",
                )
                .to_string(),
            ));
        }
    }
    // A clause whose head holds a non-symbol is a PATTERN clause (the whole `fn` is then
    // lowered to `%match`); one of only symbols is an arity clause (a native multi-arity
    // arm). Both bind per-clause names, so both are linted — the wording differs so the
    // reader knows which construct is meant. It is decided once for the whole `fn`, NOT
    // per clause, because the LOWERING is per fn: one pattern head anywhere sends the
    // entire dispatch through `%match`, so every clause's binders are then pattern
    // binders, including a head that happens to be all symbols. Naming that one a
    // "parameter" would describe a construct the program does not have.
    let any_pattern = heads.iter().any(|&h| {
        list_items(heap, h).is_some_and(|ps| ps.iter().any(|p| !matches!(p, Value::Sym(_))))
    });
    let noun = if any_pattern {
        "unused pattern binding"
    } else {
        "unused parameter"
    };
    for &clause in forms {
        let Some(citems) = list_items(heap, clause) else {
            continue;
        };
        let (Some(&plist), body) = (citems.first(), &citems[1..]) else {
            continue;
        };
        lint_clause_binders(heap, clause, plist, body, noun, out);
    }
}

/// Warn on each binder of the head `plist` that `body` never reads, at `located`'s
/// position (the clause, or the parameter list of a single-arm `fn`).
fn lint_clause_binders(
    heap: &Heap,
    located: Value,
    plist: Value,
    body: &[Value],
    noun: &str,
    out: &mut Vec<Finding>,
) {
    if body.is_empty() || heap.is_synthetic(located) {
        return;
    }
    let Some(pos) = heap.form_pos_only(located) else {
        return;
    };
    let mut binders = Vec::new();
    collect_clause_binders(heap, plist, &mut binders);
    // An `&optional` default is an expression evaluated with the earlier parameters in
    // scope — `(fn (a &optional (b (fn () a))) …)` reads `a` there — so it counts as a use.
    let defaults = optional_defaults(heap, plist);
    for b in binders {
        let name = value::symbol_name(b);
        // The `_`-prefix ignore convention, and real gensym temporaries — via the
        // interner's own predicate, not a `contains("__")` guess, which also swallowed
        // a legitimate `my__thing`.
        if name.starts_with('_') || walk::is_gensym_sym(b) {
            continue;
        }
        // The same shadowing exemption the sibling unused-`let` lint applies: you do
        // not accidentally name a binder after a builtin, so a shadow left unused is a
        // deliberate scope-isolation test, and `_`-prefixing cannot express it (that
        // changes the name being shadowed).
        if sigs::is_globally_bound(heap, b) || sigs::has_curated_sig(b) {
            continue;
        }
        if !body
            .iter()
            .chain(&defaults)
            .any(|&f| walk::sym_appears_in(heap, f, b))
        {
            out.push(Finding::new(
                Lint::UnusedBinding,
                Some(pos),
                format!("{noun}: {name}"),
            ));
        }
    }
}

/// The default expressions of a head list's `&optional` slots, `(pattern default)`.
fn optional_defaults(heap: &Heap, plist: Value) -> Vec<Value> {
    let mut defaults = Vec::new();
    let mut optional = false;
    for item in list_items(heap, plist).unwrap_or_default() {
        match item {
            Value::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => optional = true,
            Value::Sym(s) if value::symbol_is(s, kw::AMP) => optional = false,
            _ if optional => {
                if let Some(&[_, default]) = list_items(heap, item).as_deref() {
                    defaults.push(default);
                }
            }
            _ => {}
        }
    }
    defaults
}

/// Binder symbols of a `fn` parameter list (a clause head).
///
/// A typed parameter (ADR-420) binds its name, not its spelling: `position:int` binds
/// `position`, `name: T` binds `name` and `T` is a type, and everything after `->` is the
/// result type. Read as written, every typed parameter was a binder nothing could read.
/// After `&optional`, a `(pattern default)` slot binds its pattern's names; the default is
/// an expression (ADR-619). Each parameter is itself a pattern ([`collect_pattern_binders`]).
fn collect_clause_binders(heap: &Heap, plist: Value, out: &mut Vec<Symbol>) {
    let Some(items) = heap.list_or_vector_items(plist) else {
        collect_pattern_binders(heap, plist, out);
        return;
    };
    let mut optional = false;
    let mut items = items.into_iter();
    while let Some(item) = items.next() {
        if let Value::Sym(s) = item {
            if value::symbol_is(s, crate::syntax::typed::ARROW) {
                break;
            }
            if value::symbol_is(s, kw::AMP_OPTIONAL) {
                optional = true;
                continue;
            }
            if value::symbol_is(s, kw::AMP) {
                optional = false;
                continue;
            }
            match crate::syntax::typed::split(value::symbol_name_ref(s)) {
                Some(crate::syntax::typed::TypedName::Next(name)) => {
                    out.push(value::intern(name));
                    items.next();
                }
                Some(typed) => out.push(value::intern(typed.name())),
                None => collect_pattern_binders(heap, item, out),
            }
            continue;
        }
        match list_items(heap, item).as_deref() {
            Some(&[slot, _default]) if optional => collect_pattern_binders(heap, slot, out),
            _ => collect_pattern_binders(heap, item, out),
        }
    }
}

/// The names a pattern binds, by the one pattern grammar (`docs/language.md` § Pattern
/// matching; the CST twin is `syntax::scope`'s `collect_pattern_binders`). A bare symbol
/// binds; a literal, a quoted symbol `'s` and a pin `^x` (`(%pin x)`) do not — a pin
/// COMPARES against an outer binding, so flagging it would false-positive on every pinned
/// `receive` clause (`[:down ^ref _ reason]`). `and`/`or`/`bytes` are operators over
/// sub-patterns, `(record name {…})` names a type, and a map pattern's `:or` holds
/// default values, not patterns.
fn collect_pattern_binders(heap: &Heap, pattern: Value, out: &mut Vec<Symbol>) {
    match pattern {
        Value::Sym(s) => {
            // `_` is collected like any binder; the `_`-prefix exemption silences it
            if !(value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_OPTIONAL)) {
                out.push(s);
            }
        }
        Value::Vector(id) => {
            collect_sequence_binders(heap, heap.vector(id), out);
        }
        Value::Pair(_) => {
            let items = list_items(heap, pattern).unwrap_or_default();
            let skip = match items.first() {
                Some(&Value::Sym(h)) if value::symbol_is(h, kw::QUOTE) => return,
                Some(&Value::Sym(h)) if value::symbol_is(h, "%pin") => return,
                Some(&Value::Sym(h))
                    if value::symbol_is(h, "or")
                        || value::symbol_is(h, "and")
                        || value::symbol_is(h, "bytes") =>
                {
                    1
                }
                Some(&Value::Sym(h)) if value::symbol_is(h, "record") => 2,
                _ => return collect_sequence_binders(heap, &items, out),
            };
            for &item in items.iter().skip(skip) {
                collect_pattern_binders(heap, item, out);
            }
        }
        Value::Map(id) => {
            for &(key, entry) in heap.map_entries(id).iter() {
                if !matches!(key, Value::Keyword(k) if value::symbol_is(k, "or")) {
                    collect_pattern_binders(heap, entry, out);
                }
            }
        }
        _ => {}
    }
}

/// The binders of a list or vector pattern's elements. After `&optional` a slot is
/// `(pattern default)` (or a bare name), and the default is an expression, not a pattern:
/// `(_a &optional (b (do (io/write "d") 0)))` binds `b` and nothing named `do`.
fn collect_sequence_binders(heap: &Heap, items: &[Value], out: &mut Vec<Symbol>) {
    let mut optional = false;
    for &item in items {
        match item {
            Value::Sym(s) if value::symbol_is(s, kw::AMP_OPTIONAL) => optional = true,
            Value::Sym(s) if value::symbol_is(s, kw::AMP) => optional = false,
            _ => match list_items(heap, item).as_deref() {
                Some(&[slot, _default]) if optional => collect_pattern_binders(heap, slot, out),
                _ => collect_pattern_binders(heap, item, out),
            },
        }
    }
}

/// How many times `sym` occurs in `pattern`. A name written twice in one pattern is an
/// equality constraint (`[x x]`), and a `bytes` segment's size may name an earlier
/// segment's binder — either way the pattern itself reads it.
fn occurrences_in(heap: &Heap, pattern: Value, sym: Symbol) -> usize {
    match pattern {
        Value::Sym(s) if s == sym => 1,
        Value::Vector(id) => heap
            .vector(id)
            .to_vec()
            .into_iter()
            .map(|item| occurrences_in(heap, item, sym))
            .sum(),
        Value::Pair(_) => list_items(heap, pattern)
            .unwrap_or_default()
            .into_iter()
            .map(|item| occurrences_in(heap, item, sym))
            .sum(),
        Value::Map(id) => heap
            .map_entries(id)
            .iter()
            .map(|&(key, entry)| occurrences_in(heap, key, sym) + occurrences_in(heap, entry, sym))
            .sum(),
        _ => 0,
    }
}

/// **Unused binders at every other binding form**, on the un-expanded tree. Each of these
/// lowers to `%match` or to synthetic `let`s, which the main walk's unused-`let` lint
/// exempts by design, so before this a binder nothing read was reported only when it was
/// a plain symbol in a `let`/`letrec`. Two of the cases are bugs, not tidiness:
/// `(fold-for (acc 0 x xs) x)` never folds, and a `match` arm that binds a field and then
/// reads the whole value has usually read the wrong name.
///
/// `let`/`letrec` lint only their destructuring patterns here: a symbol binder there is
/// the main lint's, and reporting it twice would be noise.
fn lint_binding_form(
    heap: &Heap,
    head: Symbol,
    form: Value,
    items: &[Value],
    out: &mut Vec<Finding>,
) {
    if heap.is_synthetic(form) {
        return;
    }
    let name = value::symbol_name_ref(head);
    match name {
        "let" | "letrec" | "with" | "if-let" | "when-let" | "if-some" | "when-some" => {
            let noun = "unused let binding";
            lint_sequential_binders(heap, form, items, name == "letrec", name, noun, out);
        }
        "doseq" | "for" | "fold-for" => {
            lint_sequential_binders(heap, form, items, false, name, "unused loop binding", out);
        }
        "match" | "receive" => {
            let clauses = if name == "match" {
                items.get(2..)
            } else {
                items.get(1..)
            };
            for &clause in clauses.unwrap_or_default() {
                let Some(clause_items) = list_items(heap, clause) else {
                    continue;
                };
                let Some((&pattern, scope)) = clause_items.split_first() else {
                    continue;
                };
                // `receive`'s timeout clause, `(after ms body…)`, binds nothing.
                if name == "receive"
                    && matches!(pattern, Value::Sym(s) if value::symbol_is(s, "after"))
                {
                    continue;
                }
                report_unused_binders(heap, clause, pattern, scope, "unused pattern binding", out);
            }
        }
        "catch" => {
            if let Some((&pattern, scope)) = items.get(1..).and_then(|rest| rest.split_first()) {
                report_unused_binders(heap, form, pattern, scope, "unused catch binding", out);
            }
        }
        "as->" => {
            if let (Some(&pattern), Some(scope)) = (items.get(2), items.get(3..)) {
                report_unused_binders(heap, form, pattern, scope, "unused let binding", out);
            }
        }
        _ => {}
    }
}

/// The `(pattern expr pattern expr …)` binding list of `items[1]`, each pattern visible to
/// every later element and the body (`items[2..]`), and for `letrec` to the earlier ones
/// too. A keyword in a pattern position is a clause (`:when guard`, `:into coll`) whose
/// form is an expression, read like any other.
fn lint_sequential_binders(
    heap: &Heap,
    form: Value,
    items: &[Value],
    letrec: bool,
    name: &str,
    noun: &str,
    out: &mut Vec<Finding>,
) {
    let (Some(&binds), Some(body)) = (items.get(1), items.get(2..)) else {
        return;
    };
    let Some(binds) = heap.list_or_vector_items(binds) else {
        return;
    };
    // An all-symbol `let`/`letrec` stays a plain let the main lint reads. One destructured
    // pattern lowers the WHOLE list to synthetic lets it exempts, the plain names included.
    let symbols_are_the_main_lints = (name == "let" || name == "letrec")
        && binds
            .iter()
            .step_by(2)
            .all(|pattern| matches!(pattern, Value::Sym(_)));
    let mut position = 0;
    while position < binds.len() {
        let pattern = binds[position];
        let is_clause = matches!(pattern, Value::Keyword(_));
        let is_main_lints = symbols_are_the_main_lints && matches!(pattern, Value::Sym(_));
        if !is_clause && !is_main_lints {
            let earlier = if letrec { &binds[..position] } else { &[][..] };
            let later = binds.get(position + 2..).unwrap_or_default();
            let scope: Vec<Value> = earlier.iter().chain(later).chain(body).copied().collect();
            report_unused_binders(heap, form, pattern, &scope, noun, out);
        }
        position += 2;
    }
}

/// Warn on each binder of `pattern` that `scope` never reads, at `pattern`'s position or,
/// for a bare symbol (which has none), `located`'s. The exemptions are the other unused
/// lints': the `_` prefix, gensyms, a name that shadows a global (a deliberate isolation
/// test, which `_` cannot express), and a name the pattern itself reads.
fn report_unused_binders(
    heap: &Heap,
    located: Value,
    pattern: Value,
    scope: &[Value],
    noun: &str,
    out: &mut Vec<Finding>,
) {
    let Some(pos) = heap
        .form_pos_only(pattern)
        .or_else(|| heap.form_pos_only(located))
    else {
        return;
    };
    let mut binders = Vec::new();
    collect_pattern_binders(heap, pattern, &mut binders);
    for binder in binders {
        let spelling = value::symbol_name_ref(binder);
        if spelling.starts_with('_')
            || walk::is_gensym_sym(binder)
            || sigs::is_globally_bound(heap, binder)
            || sigs::has_curated_sig(binder)
            || occurrences_in(heap, pattern, binder) > 1
        {
            continue;
        }
        if !scope.iter().any(|&f| walk::sym_appears_in(heap, f, binder)) {
            out.push(Finding::new(
                Lint::UnusedBinding,
                Some(pos),
                format!("{noun}: {spelling}"),
            ));
        }
    }
}
