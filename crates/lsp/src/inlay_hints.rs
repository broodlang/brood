//! `textDocument/inlayHint` — parameter-name hints at call sites, and the
//! **effective type** of each function this buffer defines.
//!
//! For a call `(f a b)` where `f` is a known function, render its parameter
//! names inline before each argument: `(f` `from:`​`a` `to:`​`b`​`)`. The names
//! come from the introspection surface (`arglist`), the same source signature
//! help reads — so a hint matches the function's real signature and never runs
//! the buffer.
//!
//! Conservative by design (an *incorrect* hint is worse than none):
//! - only the leading **required** params are labelled; at the first `&optional`
//!   / `&` rest marker we stop, because `arglist` drops `(opt default)` groups
//!   and the positional mapping would drift past that point;
//! - a head that resolves to a **local** is skipped (we'd otherwise show an
//!   unrelated global's params);
//! - special forms / unknown names yield no `arglist`, so they're skipped.

use std::collections::HashMap;

use brood::introspect;
use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::reader;
use brood::syntax::scope::{BindingKind, Resolution, ScopeTree};
use brood::types::check::{file_signatures, FnSignature};
use brood::Interp;
use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel};

use crate::line_index::LineIndex;

/// Parameter-name hints for every resolvable call whose argument falls inside
/// `range` (the editor's visible region). `range` is given as byte offsets.
pub fn inlay_hints(
    interp: &mut Interp,
    root: &Node,
    text: &str,
    scope: &ScopeTree,
    index: &LineIndex,
    range: (u32, u32),
) -> Vec<InlayHint> {
    let mut out = Vec::new();
    // Memoize `arglist` per name within one request — a hot file repeats heads.
    let mut cache: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let sigs = buffer_signatures(interp, text);
    walk(
        interp, root, root, text, scope, index, range, &mut cache, &sigs, &mut out,
    );
    out
}

/// The effective signature of every function this buffer defines, by name.
///
/// The buffer is not loaded — that is the whole reason hover cannot answer this — so
/// the answer comes from the checker's form-based inference, which is what
/// `file_signatures` exposes. Wrapped in an arena checkpoint like the diagnostics
/// path: the forms are LOCAL and reclaimed, so the server's heap doesn't grow per
/// keystroke.
fn buffer_signatures(interp: &mut Interp, text: &str) -> HashMap<String, FnSignature> {
    let cp = interp.heap.checkpoint();
    let mut out = HashMap::new();
    if let Ok(forms) = reader::read_all(&mut interp.heap, text) {
        for sig in file_signatures(&mut interp.heap, &forms) {
            out.insert(sig.name.clone(), sig);
        }
    }
    interp.heap.reset_local_to(cp);
    out
}

#[allow(clippy::too_many_arguments)]
fn walk(
    interp: &mut Interp,
    root: &Node,
    node: &Node,
    text: &str,
    scope: &ScopeTree,
    index: &LineIndex,
    range: (u32, u32),
    cache: &mut HashMap<String, Option<Vec<String>>>,
    sigs: &HashMap<String, FnSignature>,
    out: &mut Vec<InlayHint>,
) {
    // Prune whole subtrees outside the requested (visible) range: their hints
    // would all be filtered out anyway, and each call head we *don't* visit is an
    // `arglist` eval we don't run.
    if node.span.end < range.0 || node.span.start > range.1 {
        return;
    }
    if node.kind == NodeKind::List {
        if let Some(head) = node.forms().next() {
            if head.kind == NodeKind::Symbol {
                let name = head.text(text);
                if name == "defn" || name == "defn-" {
                    hint_for_defn(node, text, index, range, sigs, out);
                } else {
                    hints_for_call(
                        interp, root, node, head, text, scope, index, range, cache, out,
                    );
                }
            }
        }
    }
    for child in &node.children {
        walk(
            interp, root, child, text, scope, index, range, cache, sigs, out,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn hints_for_call(
    interp: &mut Interp,
    root: &Node,
    call: &Node,
    head: &Node,
    text: &str,
    scope: &ScopeTree,
    index: &LineIndex,
    range: (u32, u32),
    cache: &mut HashMap<String, Option<Vec<String>>>,
    out: &mut Vec<InlayHint>,
) {
    let name = head.text(text);
    // Where the head's parameter names come from — the same three-way split
    // signature help makes (`signature.rs`), and for the same reason: the *live
    // image* is not the authority on a name this buffer redefines.
    //
    //  - a **local** head isn't a global at all → no hints;
    //  - a head defined in **this buffer** takes its params from the buffer's own
    //    CST def. Reading `arglist` for the raw name here was a live bug: a file
    //    that defines its own `(defn map (a b c) …)` got the *prelude* `map`'s
    //    parameter names painted over its arguments — a confidently wrong hint,
    //    which the module docs call out as worse than none;
    //  - a **free** head is resolved through this file's namespace + `(:use …)`
    //    imports before asking the image, so a bare imported name gets hints too
    //    (`arglist` on the unqualified name would just miss).
    let resolution = scope.resolve_at(root, text, head.span.start);
    if let Resolution::Defined {
        kind: BindingKind::Local,
        ..
    } = resolution
    {
        return;
    }
    let in_buffer_def = match resolution {
        Resolution::Defined {
            def,
            kind: BindingKind::Global,
        } => Some(def),
        _ => None,
    };
    let params = cache.entry(name.to_string()).or_insert_with(|| {
        let tokens = match in_buffer_def {
            Some(def) => crate::defs::find_def(root, text, def)
                .map(|d| d.params.iter().map(|p| (*p).to_string()).collect()),
            None => {
                let resolved = introspect::resolve_in_source(interp, text, name);
                introspect::arglist_tokens(interp, &resolved)
            }
        };
        leading_required(tokens)
    });
    let Some(params) = params else { return };

    // The args are the call's forms after the head; label as many as we have
    // leading required params for.
    for (arg, pname) in call.forms().skip(1).zip(params.iter()) {
        let start = arg.span.start;
        if start < range.0 || start >= range.1 {
            continue;
        }
        out.push(InlayHint {
            position: index.position(text, start),
            label: InlayHintLabel::String(format!("{pname}:")),
            kind: Some(InlayHintKind::PARAMETER),
            text_edits: None,
            tooltip: None,
            padding_left: None,
            padding_right: Some(true),
            data: None,
        });
    }
}

/// The **effective type** of a `defn`, rendered after its parameter list.
///
/// Subtle by omission — three things are deliberately not hinted, because a hint on
/// every function is wallpaper and a wrong one is worse than none:
///
/// - a function carrying a hand-written `(sig …)`: the signature is already on screen
///   one line up, and this hint's job is to answer "what does the checker *infer*?";
/// - an uninformative signature (`(any …) -> any`), which is most functions in a file
///   that hasn't been annotated and says nothing a reader doesn't see;
/// - anything the checker declined to infer at all.
///
/// When only the return is known the label is just `→ T`; when the parameters are
/// known it reads as the arrow you would write in a `sig`.
fn hint_for_defn(
    call: &Node,
    text: &str,
    index: &LineIndex,
    range: (u32, u32),
    sigs: &HashMap<String, FnSignature>,
    out: &mut Vec<InlayHint>,
) {
    let mut forms = call.forms().skip(1); // past `defn`
    let Some(name_node) = forms.next() else {
        return;
    };
    let Some(sig) = sigs.get(name_node.text(text)) else {
        return;
    };
    if sig.declared {
        return;
    }
    let Some(label) = render_effective(&sig.sig) else {
        return;
    };
    // The parameter list is the next form — or, for a multi-clause definition, the
    // first *clause*, whose own first form is the list. Anchoring to it puts the hint
    // where a reader's eye already is, and where a `sig` would describe.
    let Some(params) = forms.next() else {
        return;
    };
    let params = match params.forms().next() {
        Some(inner) if inner.kind == NodeKind::List => inner,
        _ => params,
    };
    let at = params.span.end;
    if at < range.0 || at >= range.1 {
        return;
    }
    out.push(InlayHint {
        position: index.position(text, at),
        label: InlayHintLabel::String(label),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: Some(lsp_types::InlayHintTooltip::String(
            "inferred by the checker — no `sig` declared".to_string(),
        )),
        padding_left: Some(true),
        padding_right: None,
        data: None,
    });
}

/// The label for an inferred signature, or `None` when it says nothing worth showing.
fn render_effective(sig: &brood::types::Sig) -> Option<String> {
    let params_known = sig.params.iter().any(|p| !p.is_any());
    let ret_known = !sig.ret.is_any();
    match (params_known, ret_known) {
        (false, false) => None,
        (false, true) => Some(format!("→ {}", sig.ret)),
        _ => Some(format!("{sig}")),
    }
}

/// The leading required parameter names, stopping at the first `&optional` / `&`
/// marker. `None` (no hints) when there's no arglist or it has no plain params.
fn leading_required(tokens: Option<Vec<String>>) -> Option<Vec<String>> {
    let tokens = tokens?;
    let plain: Vec<String> = tokens
        .into_iter()
        .take_while(|t| !t.starts_with('&'))
        .collect();
    (!plain.is_empty()).then_some(plain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn hints(src: &str) -> Vec<InlayHint> {
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        inlay_hints(
            &mut interp,
            &root,
            src,
            &tree,
            &index,
            (0, src.len() as u32),
        )
    }

    fn labels(hs: &[InlayHint]) -> Vec<String> {
        hs.iter()
            .map(|h| match &h.label {
                InlayHintLabel::String(s) => s.clone(),
                _ => "<parts>".to_string(),
            })
            .collect()
    }

    #[test]
    fn labels_args_with_prelude_param_names() {
        // `cons` is a builtin `(cons x xs)`; hints name the two args.
        let hs = hints("(cons 1 (list 2))");
        let ls = labels(&hs);
        assert!(ls.contains(&"x:".to_string()), "got: {ls:?}");
        assert!(ls.contains(&"xs:".to_string()), "got: {ls:?}");
    }

    #[test]
    fn stops_at_optional_or_rest_marker() {
        // Only the required leading params of a variadic are labelled.
        let plain = leading_required(Some(vec![
            "a".into(),
            "b".into(),
            "&".into(),
            "rest".into(),
        ]));
        assert_eq!(plain, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn local_head_is_not_hinted() {
        // `f` is a let-bound local, not the global it might otherwise resolve to.
        let hs = hints("(let (f (fn (x) x)) (f 1))");
        assert!(
            labels(&hs).iter().all(|l| l != "x:"),
            "got: {:?}",
            labels(&hs)
        );
    }

    #[test]
    fn unknown_head_yields_nothing() {
        assert!(hints("(no-such-fn 1 2)").is_empty());
    }

    // ---- effective-type hints on `defn` ----

    #[test]
    fn a_defn_gets_the_type_the_checker_inferred() {
        // The buffer is never loaded, so this can only come from form-based inference.
        let ls = labels(&hints("(defn f (s) (string/length s))"));
        assert!(
            ls.iter().any(|l| l.contains("string") && l.contains("->")),
            "got: {ls:?}"
        );
    }

    #[test]
    fn a_declared_signature_is_not_repeated() {
        // It is already on screen one line up; the hint answers what the checker
        // *inferred*, which is only interesting when nothing was said.
        let ls = labels(&hints("(sig f (int -> string))\n(defn f (n) \"x\")"));
        assert!(ls.iter().all(|l| !l.contains("->")), "got: {ls:?}");
    }

    #[test]
    fn an_uninformative_signature_is_not_hinted() {
        // `(any) -> any` on every function is wallpaper.
        let ls = labels(&hints("(defn f (x) x)"));
        assert!(ls.iter().all(|l| !l.contains("->")), "got: {ls:?}");
    }

    #[test]
    fn a_known_return_alone_reads_as_an_arrow() {
        let ls = labels(&hints("(defn f (x) (string/length \"lit\"))"));
        assert!(ls.iter().any(|l| l.starts_with("→ ")), "got: {ls:?}");
    }

    #[test]
    fn the_hint_sits_at_the_end_of_the_parameter_list() {
        let src = "(defn f (s) (string/length s))";
        let hs = hints(src);
        let hint = hs
            .iter()
            .find(|h| matches!(&h.label, InlayHintLabel::String(l) if l.contains("->")))
            .expect("a type hint");
        // `(defn f (s)` — the character after the closing paren of the param list.
        assert_eq!(
            hint.position.character as usize,
            src.find(") (").unwrap() + 1
        );
    }
}
