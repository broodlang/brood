//! Introspection queries answered by the server's `Interp`. The server holds one
//! interpreter loaded with the prelude + builtins; it does **not** evaluate the
//! open document (see `docs/lsp.md`), so these surface the *language's* globals —
//! completion candidates, and the arglist/docstring of a prelude or builtin name
//! under the cursor. They run by `eval`ing the introspection primitives
//! (`global-names` / `arglist` / `doc`, ADR-025), never user code.

use brood::core::value::{self, Value};
use brood::Interp;

/// Every global the interpreter knows (prelude + builtins), sorted by spelling
/// (`global-names` sorts) — the completion candidate pool.
pub fn global_names(interp: &mut Interp) -> Vec<String> {
    // The result is a list of *every* global (hundreds of symbols), and this
    // runs on every completion keystroke. `eval_str` keeps its result in LOCAL,
    // so reclaim it once we've copied the names into owned `String`s — otherwise
    // a long server session leaks a fresh list per request. (Safe: `symbol_name`
    // reads the interner, not LOCAL, so `names` holds no LOCAL handle.)
    let cp = interp.heap.checkpoint();
    let names = match interp.eval_str("(global-names)") {
        Ok(v) => interp
            .heap
            .list_to_vec(v)
            .map(|items| {
                items
                    .into_iter()
                    .filter_map(|x| match x {
                        Value::Sym(s) => Some(value::symbol_name(s)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    interp.heap.reset_local_to(cp);
    names
}

/// The `(signature, docstring)` of a global `name`, via `(list (arglist NAME)
/// (doc NAME))`. `name` is a CST symbol token, so it can't contain a delimiter,
/// comment, or quote char (see [`is_delimiter`]) — interpolating it can't escape
/// the expression. Returns `(None, None)` when `name` is unbound (eval errors)
/// or names something without an arglist/doc.
///
/// [`is_delimiter`]: brood::syntax::atom::is_delimiter
pub fn signature(interp: &mut Interp, name: &str) -> (Option<String>, Option<String>) {
    // As in `global_names`: reclaim the LOCAL allocations this eval leaves behind
    // once the signature/doc have been copied into owned `String`s.
    let cp = interp.heap.checkpoint();
    let out = match interp.eval_str(&format!("(list (arglist {name}) (doc {name}))")) {
        Ok(v) => match interp.heap.list_to_vec(v) {
            Ok(parts) => {
                let sig = parts.first().and_then(|&a| render_arglist(interp, name, a));
                let doc = match parts.get(1) {
                    Some(&Value::Str(id)) => Some(interp.heap.string(id).to_string()),
                    _ => None,
                };
                (sig, doc)
            }
            Err(_) => (None, None),
        },
        Err(_) => (None, None),
    };
    interp.heap.reset_local_to(cp);
    out
}

/// The raw parameter tokens of a global function/macro `name` — the names *and*
/// the `&optional` / `&` markers, in source order (e.g. `["a", "&optional", "b",
/// "&", "rest"]`). `None` when `name` is unbound or has no params (a builtin or a
/// zero-arg fn — indistinguishable here, as in [`signature`]). For signature help.
pub fn arglist_tokens(interp: &mut Interp, name: &str) -> Option<Vec<String>> {
    let cp = interp.heap.checkpoint();
    let out = match interp.eval_str(&format!("(arglist {name})")) {
        Ok(v) => interp.heap.list_to_vec(v).ok().map(|items| {
            items
                .into_iter()
                .filter_map(|x| match x {
                    Value::Sym(s) => Some(value::symbol_name(s)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        }),
        Err(_) => None,
    };
    interp.heap.reset_local_to(cp);
    out.filter(|v| !v.is_empty())
}

/// Render an `(arglist f)` result (a list of parameter symbols, or `nil`) as a
/// signature line `(name p1 p2 …)`. An empty result (a builtin, a non-fn, *or* a
/// zero-arg fn) yields `None`: we can't tell a zero-arg fn from a non-fn here, so
/// we show no signature rather than a misleading one.
fn render_arglist(interp: &Interp, name: &str, arglist: Value) -> Option<String> {
    let items = interp.heap.list_to_vec(arglist).ok()?;
    if items.is_empty() {
        return None;
    }
    let mut s = format!("({name}");
    for it in items {
        if let Value::Sym(p) = it {
            s.push(' ');
            s.push_str(&value::symbol_name(p));
        }
    }
    s.push(')');
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_names_includes_prelude_and_builtins() {
        let mut interp = Interp::new();
        let names = global_names(&mut interp);
        assert!(names.contains(&"map".to_string()), "prelude `map` missing");
        assert!(names.contains(&"+".to_string()), "builtin `+` missing");
        // `global-names` sorts, so the pool is in deterministic order.
        assert!(names.windows(2).all(|w| w[0] <= w[1]), "not sorted");
    }

    #[test]
    fn signature_of_a_prelude_fn_renders_its_params() {
        let mut interp = Interp::new();
        // `map` takes a function and a list; whatever its param names, the
        // signature must start `(map ` and list at least one parameter.
        let (sig, _doc) = signature(&mut interp, "map");
        let sig = sig.expect("map should have a signature");
        assert!(sig.starts_with("(map "), "got {sig:?}");
    }

    #[test]
    fn signature_of_an_unbound_name_is_empty() {
        let mut interp = Interp::new();
        assert_eq!(
            signature(&mut interp, "no-such-global-xyzzy"),
            (None, None)
        );
    }

    #[test]
    fn repeated_queries_stay_correct_after_local_reset() {
        // Each query resets the LOCAL heap to reclaim its result (so a long server
        // session doesn't leak a list per keystroke). This must not clobber
        // interned/prelude state: results stay identical across many calls.
        let mut interp = Interp::new();
        let first = global_names(&mut interp);
        let first_sig = signature(&mut interp, "map");
        for _ in 0..1000 {
            assert_eq!(global_names(&mut interp), first);
            assert_eq!(signature(&mut interp, "map"), first_sig);
        }
    }
}
