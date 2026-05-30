//! `textDocument/signatureHelp`: while you type a call's arguments, show the
//! callee's parameter list with the argument you're on highlighted. Find the
//! enclosing call form at the cursor, take its head symbol's parameters — from
//! the CST def if it's defined in this document ([`defs`](crate::defs)),
//! otherwise from the interpreter ([`introspect`](brood::introspect)) for a
//! prelude/builtin — and compute the active argument from the cursor's position
//! among the argument forms. No user code runs.

use brood::introspect;
use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::scope::{BindingKind, Resolution, ScopeTree};
use brood::Interp;
use lsp_types::{ParameterInformation, ParameterLabel, SignatureHelp, SignatureInformation};

use crate::defs;

pub fn signature_help(
    interp: &mut Interp,
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    offset: u32,
) -> Option<SignatureHelp> {
    let list = enclosing_list(root, offset)?;
    let head = list.forms().next()?;
    if head.kind != NodeKind::Symbol {
        return None; // `((f) …)` etc. — no callable name to describe
    }
    let name = head.text(text);

    // The callee's parameter tokens (names + `&optional`/`&` markers): from the
    // CST def if `name` is defined in this file, else from the interpreter.
    let raw: Vec<String> = match tree.resolve_at(root, text, head.span.start) {
        Resolution::Defined {
            def,
            kind: BindingKind::Global,
        } => defs::find_def(root, text, def).map(|d| owned(&d.params))?,
        // Resolve against this file's namespace + imports (ADR-065 §4) so a call
        // to a bare imported fn or a qualified `mod/fn` finds its parameters.
        _ => {
            let resolved = introspect::resolve_in_source(interp, text, name);
            introspect::arglist_tokens(interp, &resolved)?
        }
    };

    let slots = slots(&raw);
    if slots.is_empty() {
        return None; // a builtin / zero-arg fn — nothing useful to highlight
    }

    let args: Vec<&Node> = list.forms().skip(1).collect();
    // Clamp into range so something is always highlighted (extra args beyond a
    // fixed arity, or a `& rest` tail, land on the last slot).
    let active = active_param(&args, offset).min(slots.len() - 1) as u32;

    let label = format!("({} {})", name, raw.join(" "));
    let parameters = slots
        .into_iter()
        .map(|s| ParameterInformation {
            label: ParameterLabel::Simple(s),
            documentation: None,
        })
        .collect();

    let signature = SignatureInformation {
        label,
        documentation: None,
        parameters: Some(parameters),
        active_parameter: Some(active),
    };
    Some(SignatureHelp {
        signatures: vec![signature],
        active_signature: Some(0),
        active_parameter: Some(active),
    })
}

/// The innermost `List` node whose span contains `offset` — the call we're typing
/// arguments into. Containment is **inclusive of the span end** (unlike
/// `node_at`): signature help fires while you type, when the cursor sits at the
/// very end of an unclosed `(map ` (offset == EOF == the recovered span's end),
/// which a half-open check would miss.
fn enclosing_list(node: &Node, offset: u32) -> Option<&Node> {
    let contains = |n: &Node| n.span.start <= offset && offset <= n.span.end;
    let mut best = (node.kind == NodeKind::List && contains(node)).then_some(node);
    for child in &node.children {
        if contains(child) {
            if let Some(inner) = enclosing_list(child, offset) {
                best = Some(inner);
            }
        }
    }
    best
}

/// The bindable parameter slots from raw arglist tokens: drop the `&optional` /
/// `&` markers, and reduce an `(name default)` optional group to its name. The
/// active-argument index points into *this* list, not the raw tokens.
fn slots(raw: &[String]) -> Vec<String> {
    raw.iter()
        .filter_map(|p| {
            if p == "&optional" || p == "&" {
                None
            } else if let Some(inner) = p.strip_prefix('(') {
                // `(b 1)` → `b`
                let name = inner
                    .split(|c: char| c.is_whitespace() || c == ')')
                    .next()
                    .unwrap_or("");
                (!name.is_empty()).then(|| name.to_string())
            } else {
                Some(p.clone())
            }
        })
        .collect()
}

/// Which argument the cursor is on: the index of the arg form containing `offset`
/// (inclusive of its end, so editing at the end of an arg counts as that arg),
/// or — when the cursor sits in the gap between args — the count of args already
/// completed before it.
fn active_param(args: &[&Node], offset: u32) -> usize {
    if let Some(i) = args
        .iter()
        .position(|a| a.span.start <= offset && offset <= a.span.end)
    {
        return i;
    }
    args.iter().filter(|a| a.span.end <= offset).count()
}

fn owned(params: &[&str]) -> Vec<String> {
    params.iter().map(|s| (*s).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    /// Run signature help with the cursor at the *end* of `cursor` (its first
    /// occurrence in `src`), returning `(label, active_parameter)`.
    fn help_at(src: &str, cursor: &str) -> Option<(String, u32)> {
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = (src.find(cursor).unwrap() + cursor.len()) as u32;
        signature_help(&mut interp, src, &root, &tree, at).map(|h| {
            let s = &h.signatures[0];
            (s.label.clone(), s.active_parameter.unwrap())
        })
    }

    #[test]
    fn highlights_the_active_argument_of_a_prelude_call() {
        // Cursor after `map ` → on the first parameter.
        let (label, active) = help_at("(map ", "map ").expect("help on map");
        assert!(label.starts_with("(map "), "{label:?}");
        assert_eq!(active, 0);
    }

    #[test]
    fn advances_active_parameter_as_args_are_filled() {
        // `(reduce f init |coll)` — cursor in the gap before the 3rd arg.
        let src = "(reduce f init coll)";
        // after "init " (the gap) → third parameter (index 2)
        let (_label, active) = help_at(src, "init ").expect("help on reduce");
        assert_eq!(active, 2);
    }

    #[test]
    fn works_for_a_document_defined_function() {
        let src = "(defn add3 (a b c) (+ a b c))\n(add3 1 ";
        let (label, active) = help_at(src, "add3 1 ").expect("help on add3");
        assert_eq!(label, "(add3 a b c)");
        assert_eq!(active, 1); // after `1 ` → second parameter
    }

    #[test]
    fn drops_optional_and_rest_markers_from_the_slots() {
        // `&optional` / `&` appear in the label but are not selectable slots.
        let src = "(defn f (a &optional (b 1) & cs) a)\n(f ";
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.rfind("(f ").unwrap() as u32 + 3;
        let h = signature_help(&mut interp, src, &root, &tree, at).unwrap();
        let names: Vec<&str> = h.signatures[0]
            .parameters
            .as_ref()
            .unwrap()
            .iter()
            .map(|p| match &p.label {
                ParameterLabel::Simple(s) => s.as_str(),
                _ => panic!("expected simple label"),
            })
            .collect();
        assert_eq!(names, vec!["a", "b", "cs"]);
        assert_eq!(h.signatures[0].label, "(f a &optional (b 1) & cs)");
    }

    #[test]
    fn no_help_outside_a_call() {
        assert!(help_at("(map f) ", ") ").is_none());
    }
}
