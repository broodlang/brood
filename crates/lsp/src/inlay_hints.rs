//! `textDocument/inlayHint` — parameter-name hints at call sites.
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
use brood::syntax::scope::{BindingKind, Resolution, ScopeTree};
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
    walk(
        interp, root, text, scope, index, range, &mut cache, &mut out,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn walk(
    interp: &mut Interp,
    node: &Node,
    text: &str,
    scope: &ScopeTree,
    index: &LineIndex,
    range: (u32, u32),
    cache: &mut HashMap<String, Option<Vec<String>>>,
    out: &mut Vec<InlayHint>,
) {
    if node.kind == NodeKind::List {
        if let Some(head) = node.forms().next() {
            if head.kind == NodeKind::Symbol {
                hints_for_call(interp, node, head, text, scope, index, range, cache, out);
            }
        }
    }
    for child in &node.children {
        walk(interp, child, text, scope, index, range, cache, out);
    }
}

#[allow(clippy::too_many_arguments)]
fn hints_for_call(
    interp: &mut Interp,
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
    // A locally-bound head isn't the global we'd introspect — skip it.
    if let Resolution::Defined {
        kind: BindingKind::Local,
        ..
    } = scope.resolve_at(call, text, head.span.start)
    {
        return;
    }

    let params = cache
        .entry(name.to_string())
        .or_insert_with(|| leading_required(introspect::arglist_tokens(interp, name)));
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
}
