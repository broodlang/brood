//! Macro-hygiene lint: warn when a `defmacro` template introduces a binder
//! that can **capture** code spliced in from the macro's arguments.
//!
//! Brood macros are unhygienic by default *except* for opt-in auto-gensym (a
//! binder written `tmp#` is rewritten to a fresh symbol by the quasiquote
//! expander). So a binder introduced with a *plain literal* symbol — no `#`
//! suffix, no `~gensym` — lives in the same namespace as the caller's code. The
//! classic bug:
//!
//! ```text
//! (defmacro time (expr)
//!   `(let (start (now) v ~expr)        ; `start` is a literal binder…
//!      [v (- (now) start)]))           ; …and ~expr (caller code) is in its scope
//! ```
//!
//! If the caller writes `(time (+ start 1))`, the body's `start` no longer
//! refers to the caller's `start` — the template captured it. The fix is a fresh,
//! uncapturable binder: either the auto-gensym shorthand `start#`, or an explicit
//! `~(gensym "start")` (the latter is what `and`/`or`/`bench`/`is`/… in the
//! prelude do; either silences this lint).
//!
//! ## The firing condition (kept tight to honour the no-false-positive contract)
//!
//! We warn only when **both** hold for a `let`/`fn` binder inside a quasiquote
//! template:
//!   1. the binder is a *plain literal* symbol — written bare, e.g. `start`. A
//!      gensym'd binder reads as `(unquote g)` (you write `~g`), which is not a
//!      symbol, so it never trips this; nor does an unquoted caller-supplied
//!      binder name (`~evar` in `try`); nor does a `#`-suffixed binder (`start#`),
//!      which the expander auto-gensyms.
//!   2. a macro **parameter** is spliced (`~p` / `~@p`) somewhere in that
//!      binder's scope — i.e. caller code actually lands where the binder is
//!      visible. A macro that binds a private temp it never exposes caller code
//!      to (no splice in scope) is not flagged.
//!
//! Both conditions are syntactic and run over the *unexpanded* source, so this
//! pass reads the raw top-level forms (before `check_file`'s macroexpansion).
//! Audited against the whole `std/` tree: every existing macro gensyms or
//! unquotes its binders, so this lint fires zero false positives there. The one
//! shape it would flag that *could* be intentional — an anaphoric macro that
//! deliberately binds a name for the caller's spliced code to see (`it` in an
//! `aif`) — does not exist in this codebase; if one is ever written, gensym is
//! not the fix and the lint should grow an opt-out then (ADR-024: advisory,
//! never gating).

use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Value};
use crate::error::Pos;

/// Scan one raw (un-expanded) top-level form for `defmacro`s and emit a hygiene
/// warning per capturing template binder. Reads only — no allocation, so it
/// needs no GC rooting beyond what the caller already holds.
pub fn check_macro_hygiene(heap: &Heap, form: Value, out: &mut Vec<(Option<Pos>, String)>) {
    let items = match proper_list(heap, form) {
        Some(items) => items,
        None => return,
    };
    if let Some(&Value::Sym(head)) = items.first() {
        // Don't descend into quoted data: a `(defmacro …)` under `quote` /
        // `quasiquote` is data being *built*, not a macro definition here.
        if value::symbol_is(head, kw::QUOTE) || value::symbol_is(head, kw::QUASIQUOTE) {
            return;
        }
        if value::symbol_is(head, kw::DEFMACRO) && items.len() >= 3 {
            analyze_defmacro(heap, &items, form, out);
            // fall through: a nested `defmacro` in the body is still worth scanning.
        }
    }
    for &it in &items {
        check_macro_hygiene(heap, it, out);
    }
}

/// `items` is `[defmacro NAME PARAMS BODY…]`. Collect the parameter names, then
/// scan each body form for quasiquote templates and analyze their binders.
fn analyze_defmacro(
    heap: &Heap,
    items: &[Value],
    macro_form: Value,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let name = match items.get(1) {
        Some(&Value::Sym(s)) => value::symbol_name(s),
        _ => return,
    };
    let mut params: Vec<String> = Vec::new();
    collect_param_names(heap, items[2], &mut params);
    // Each remaining item is a body form (the expansion-time code + the template).
    for &body_form in &items[3..] {
        find_templates(heap, body_form, &name, &params, macro_form, out);
    }
}

/// Walk macro-body code looking for `(quasiquote TPL)`; analyze each template's
/// binders. We do not descend into `(unquote …)` (that's expansion-time code,
/// not template structure) or nested `(quasiquote …)` (v0.1 doesn't level-track
/// quasiquote — staying out keeps us conservative).
fn find_templates(
    heap: &Heap,
    form: Value,
    macro_name: &str,
    params: &[String],
    macro_form: Value,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let items = match proper_list(heap, form) {
        Some(items) => items,
        None => return,
    };
    if let Some(&Value::Sym(head)) = items.first() {
        if value::symbol_is(head, kw::QUASIQUOTE) {
            if let Some(&tpl) = items.get(1) {
                analyze_template(heap, tpl, macro_name, params, macro_form, out);
            }
            return;
        }
    }
    for &it in &items {
        find_templates(heap, it, macro_name, params, macro_form, out);
    }
}

/// Walk a quasiquote template. At each `let`/`fn` binder form, flag a literal
/// symbol binder when a macro parameter is spliced into that binder's scope.
fn analyze_template(
    heap: &Heap,
    tpl: Value,
    macro_name: &str,
    params: &[String],
    macro_form: Value,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let items = match proper_list(heap, tpl) {
        Some(items) => items,
        None => return,
    };
    if let Some(&Value::Sym(head)) = items.first() {
        // `(unquote E)` / `(unquote-splicing E)`: a hole filled with
        // expansion-time code, not template-introduced binders — and a nested
        // `(quote …)`/`(quasiquote …)` is data. Don't hunt binders inside any.
        if value::symbol_is(head, kw::UNQUOTE)
            || value::symbol_is(head, kw::UNQUOTE_SPLICING)
            || value::symbol_is(head, kw::QUOTE)
            || value::symbol_is(head, kw::QUASIQUOTE)
        {
            return;
        }

        let is_let = value::symbol_is(head, kw::LET) || value::symbol_is(head, kw::LET_STAR);
        let is_fn = value::symbol_is(head, kw::FN);
        if is_let && items.len() >= 2 {
            // `(let (b0 v0 b1 v1 …) body…)`. Brood `let` is sequential, so binder
            // `bj`'s scope is the *later* bindings' value expressions plus the
            // body — NOT its own value `vj` (evaluated before `bj` is bound) and
            // not earlier values. So a splice in `vk` (k>j) or the body is what
            // `bj` could capture.
            if let Some(pairs) = proper_list(heap, items[1]) {
                let body = &items[2..];
                let nbind = pairs.len() / 2;
                for j in 0..nbind {
                    let mut scope: Vec<Value> = (j + 1..nbind).map(|k| pairs[2 * k + 1]).collect();
                    scope.extend_from_slice(body);
                    flag_if_captures(
                        heap,
                        pairs[2 * j],
                        &scope,
                        macro_name,
                        params,
                        tpl,
                        macro_form,
                        out,
                    );
                }
            }
        } else if is_fn && items.len() >= 2 {
            // `(fn (p…) body…)` — parameters are bound together; each is visible
            // in the body, so the body is every parameter's scope.
            if let Some(ps) = proper_list(heap, items[1]) {
                let body = &items[2..];
                for &p in &ps {
                    flag_if_captures(heap, p, body, macro_name, params, tpl, macro_form, out);
                }
            }
        }
    }
    // Recurse into every child so nested templates' binders are also analyzed.
    for &it in &items {
        analyze_template(heap, it, macro_name, params, macro_form, out);
    }
}

/// Emit a warning when `binder` is a literal symbol (not `_`/`&…`/an unquoted
/// hole) and a macro parameter is spliced into `scope`.
#[allow(clippy::too_many_arguments)]
fn flag_if_captures(
    heap: &Heap,
    binder: Value,
    scope: &[Value],
    macro_name: &str,
    params: &[String],
    tpl: Value,
    macro_form: Value,
    out: &mut Vec<(Option<Pos>, String)>,
) {
    let s = match binder {
        Value::Sym(s) => s,
        _ => return, // `~g` (gensym) / `~evar` (caller's name) read as a pair — never a literal binder
    };
    let bname = value::symbol_name(s);
    if bname == "_" || is_amp_marker(&bname) {
        return;
    }
    // A `#`-suffixed binder (`tmp#`) is rewritten to a fresh gensym by the
    // quasiquote expander (auto-gensym, `eval::macros::maybe_autogensym`), so it
    // is uncapturable by construction — not a literal binder. Don't flag it.
    if bname.len() > 1 && bname.ends_with('#') {
        return;
    }
    if !scope_splices_param(heap, scope, params) {
        return;
    }
    let pos = heap.form_pos(tpl).or_else(|| heap.form_pos(macro_form));
    out.push((
        pos,
        format!(
            "macro `{macro_name}` binds `{bname}` in a template that splices a parameter into \
             its scope — `{bname}` can capture references in the spliced code. Give the binder a \
             `#` suffix (`{bname}#`) for an auto-gensym, or bind `~(gensym \"{bname}\")`."
        ),
    ));
}

/// True when some subtree of `forms` is `(unquote E)` / `(unquote-splicing E)`
/// whose `E` mentions one of the macro's parameters — i.e. caller-supplied code
/// is spliced here.
fn scope_splices_param(heap: &Heap, forms: &[Value], params: &[String]) -> bool {
    forms.iter().any(|&f| splice_of_param(heap, f, params))
}

fn splice_of_param(heap: &Heap, form: Value, params: &[String]) -> bool {
    let items = match proper_list(heap, form) {
        Some(items) => items,
        None => return false,
    };
    if let Some(&Value::Sym(head)) = items.first() {
        if value::symbol_is(head, kw::UNQUOTE) || value::symbol_is(head, kw::UNQUOTE_SPLICING) {
            // The unquoted expression: does it reference a macro parameter?
            return items
                .get(1)
                .is_some_and(|&e| mentions_param(heap, e, params));
        }
    }
    items.iter().any(|&it| splice_of_param(heap, it, params))
}

/// True if any symbol anywhere in `form` names one of `params`.
fn mentions_param(heap: &Heap, form: Value, params: &[String]) -> bool {
    match form {
        Value::Sym(s) => {
            let n = value::symbol_name(s);
            params.contains(&n)
        }
        Value::Pair(_) => match proper_list(heap, form) {
            Some(items) => items.iter().any(|&it| mentions_param(heap, it, params)),
            None => false,
        },
        _ => false,
    }
}

/// Collect the symbol names from a macro parameter list, skipping `&`-markers
/// and `_`. One level deep — macro params are a flat list in practice.
fn collect_param_names(heap: &Heap, plist: Value, out: &mut Vec<String>) {
    if let Some(items) = proper_list(heap, plist) {
        for it in items {
            if let Value::Sym(s) = it {
                let n = value::symbol_name(s);
                if n != "_" && !is_amp_marker(&n) {
                    out.push(n);
                }
            }
        }
    }
}

/// `&`, `&optional`, `&rest`, `&key` — the rest/marker symbols, never real binders.
fn is_amp_marker(name: &str) -> bool {
    name.starts_with('&')
}

/// `Some(items)` for a proper list, `None` for a non-pair or improper list.
fn proper_list(heap: &Heap, v: Value) -> Option<Vec<Value>> {
    match v {
        Value::Pair(_) => heap.list_to_vec(v).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::reader;
    use crate::Interp;

    fn hygiene_warnings(src: &str) -> Vec<String> {
        let mut interp = Interp::new();
        let form = reader::read_one(&mut interp.heap, src).expect("parse");
        let mut out = Vec::new();
        check_macro_hygiene(&interp.heap, form, &mut out);
        out.into_iter().map(|(_, m)| m).collect()
    }

    #[test]
    fn flags_literal_binder_capturing_a_spliced_param() {
        // `start` bound literally; `~expr` (a param) spliced into its scope.
        let ws = hygiene_warnings("(defmacro time (expr) `(let (start (now) v ~expr) [v start]))");
        assert!(
            ws.iter().any(|w| w.contains("time") && w.contains("start")),
            "expected a capture warning for `start`, got {ws:?}"
        );
    }

    #[test]
    fn flags_fn_param_binder_capturing_a_splice() {
        let ws = hygiene_warnings("(defmacro m (body) `(map (fn (acc) ~body) xs))");
        assert!(
            ws.iter().any(|w| w.contains("acc")),
            "expected a capture warning for the fn param `acc`, got {ws:?}"
        );
    }

    #[test]
    fn gensym_binder_is_not_flagged() {
        // The prelude `and`/`or` shape: binder is `~g`, never a literal symbol.
        assert!(hygiene_warnings(
            "(defmacro safe (x) (let (g (gensym)) `(let (~g ~x) (if ~g ~g 0))))"
        )
        .is_empty());
    }

    #[test]
    fn autogensym_binder_is_not_flagged() {
        // `r#` is auto-gensym'd by the quasiquote expander, so it's uncapturable
        // even though `~a`/`~b` (params) are spliced into its scope.
        assert!(
            hygiene_warnings("(defmacro my-or (a b) `(let (r# ~a) (if r# r# ~b)))").is_empty(),
            "a `#`-suffixed (auto-gensym) binder must not be flagged"
        );
    }

    #[test]
    fn unquoted_caller_binder_is_not_flagged() {
        // `try`'s shape: the binder is the caller's chosen name, unquoted.
        assert!(hygiene_warnings(
            "(defmacro mytry (evar & body) `(%try (fn () 1) (fn (~evar) ~@body)))"
        )
        .is_empty());
    }

    #[test]
    fn literal_binder_without_a_splice_in_scope_is_not_flagged() {
        // `tmp` is a private temp; the only splice (`~x`) is OUTSIDE tmp's scope
        // (sequenced before the `let`), so it can't be captured — don't flag.
        assert!(
            hygiene_warnings("(defmacro m (x) `(do ~x (let (tmp 1) (+ tmp 2))))").is_empty(),
            "splice is outside tmp's scope — must not flag"
        );
        // No splice at all: a fully self-contained template never captures.
        assert!(hygiene_warnings("(defmacro m (x) `(let (tmp 1) (+ tmp 2)))").is_empty());
    }

    #[test]
    fn non_macro_forms_are_ignored() {
        assert!(hygiene_warnings("(defn f (x) (let (start x) start))").is_empty());
        assert!(hygiene_warnings("(let (start 1) start)").is_empty());
    }
}
