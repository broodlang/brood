//! `textDocument/hover`: documentation for the symbol under the cursor. Resolve
//! it against the document's scopes, then render by what it binds to:
//! - a **local** (param / `let`) → a short "local binding" note;
//! - a **document-level `def`** → its signature + docstring, read from the CST;
//! - anything **free** (a prelude or builtin name) → its arglist + docstring from
//!   the interpreter's introspection primitives.
//!
//! No user code runs — see `docs/lsp.md`.

use brood::Interp;
use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::reader;
use brood::syntax::scope::{BindingKind, Resolution, ScopeTree};
use brood::types::check;
use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use crate::defs;
use crate::line_index::LineIndex;
use crate::module_ref;
use brood::introspect;

/// Hover for the symbol at byte `offset`. `root`/`tree`/`index` are the document's
/// already-built CST, scope analysis, and line index (the caller parses once).
pub fn hover(
    interp: &mut Interp,
    text: &str,
    root: &Node,
    tree: &ScopeTree,
    index: &LineIndex,
    offset: u32,
) -> Option<Hover> {
    let node = root.symbol_at(offset)?;
    let name = node.text(text);

    // A `defmodule` clause target — the module name in `(:use …)`/`(:alias …)` — binds
    // nothing in the buffer, so it resolves `Free` below and renders nothing useful.
    // Handle it first, off the module docs.
    if let Some(module) = module_ref::clause_module_at(root, text, offset) {
        return render_module(interp, module).map(|value| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(index.range(text, node.span)),
        });
    }

    let mut markdown = match tree.resolve_at(root, text, offset) {
        Resolution::Defined {
            kind: BindingKind::Local,
            ..
        } => format!("```brood\n{name}\n```\n\nlocal binding"),
        Resolution::Defined {
            def,
            kind: BindingKind::Global,
        } => defs::find_def(root, text, def)
            .map(|d| render_def(&d))
            .unwrap_or_else(|| code(name)),
        Resolution::Free => {
            // Resolve against this file's namespace + imports (ADR-065 §4) so a
            // bare imported name or a qualified `observer/observe` finds its docs.
            let resolved = introspect::resolve_in_source(interp, text, name);
            let (sig, doc) = introspect::signature(interp, &resolved);
            // The checker's TYPE signature (declared/curated/inferred) for a resolvable
            // name — shown under the arglist. Only for free names: they're stably loaded
            // (builtins/prelude/imports), whereas a buffer def isn't loaded, so its sig
            // would be stale/absent.
            let ty = introspect::type_signature(interp, &resolved);
            // Stability facts (ADR-283) go under the signature: a deprecation names its
            // replacement, a beta mark its reason. Completion carries the same notes.
            let notes = [
                introspect::deprecation(interp, &resolved),
                introspect::beta(interp, &resolved),
            ];
            render_global(name, sig, ty, &notes, doc)?
        }
        Resolution::NotASymbol => return None,
    };
    // How a call reads the argument under the cursor — precise (by inclusion) or dynamic
    // (by overlap), the half of the type relation a reader cannot see from the types
    // alone (docs/types.md § "Which reading a value gets").
    if let Some(reading) = argument_reading(interp, text, root, offset) {
        markdown.push_str(&format!("\n\n**as an argument here:** {reading}"));
    }

    let range = index.range(text, node.span);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(range),
    })
}

/// The forms whose sub-lists are binders or clauses, not calls: a symbol in one of their
/// slots is not an argument, so no argument reading is offered there.
const NOT_CALLS: &[&str] = &[
    "let",
    "letrec",
    "fn",
    "defn",
    "defn-",
    "defmacro",
    "def",
    "def-",
    "defonce",
    "match",
    "case",
    "cond",
    "condp",
    "for",
    "doseq",
    "fold-for",
    "when-let",
    "if-let",
    "when-some",
    "if-some",
    "receive",
    "with",
    "binding",
    "try",
    "catch",
    "quote",
    "quasiquote",
    "comment",
    "defmodule",
    "sig",
    "sig!",
    "deftype",
    "defrecord",
    "defability",
    "impl",
];

/// The argument the symbol at `offset` is — itself (`(f x)`, cursor on `x`) or the head of
/// a nested call that is one (`(f (g y))`, cursor on `g`) — read the way the call's
/// argument check reads it ([`check::arg_gradual_at`]). `None` when the symbol is not in an
/// argument slot, the buffer does not read, or the checker cannot type it.
fn argument_reading(interp: &mut Interp, text: &str, root: &Node, offset: u32) -> Option<String> {
    // The chain of nodes from the root down to the symbol.
    let mut path: Vec<&Node> = Vec::new();
    let mut node = root;
    loop {
        path.push(node);
        let Some(child) = node.children.iter().find(|c| c.span.contains(offset)) else {
            break;
        };
        node = child;
    }
    let sym = *path.last()?;
    if sym.kind != NodeKind::Symbol {
        return None;
    }
    let index_in = |list: &Node, item: &Node| list.forms().position(|f| std::ptr::eq(f, item));
    let parent = path.get(path.len().checked_sub(2)?).copied()?;
    if parent.kind != NodeKind::List {
        return None;
    }
    let (call, arg_index) = match index_in(parent, sym)? {
        0 => {
            let grand = path.get(path.len().checked_sub(3)?).copied()?;
            if grand.kind != NodeKind::List {
                return None;
            }
            (grand, index_in(grand, parent)?)
        }
        index => (parent, index),
    };
    if arg_index == 0 {
        return None;
    }
    let head = call.head_symbol(text)?;
    if NOT_CALLS.contains(&head) {
        return None;
    }
    // The call form's opening paren in 1-based reader coordinates (columns count chars).
    let opening = call.span.start as usize;
    let line = text[..opening].bytes().filter(|&b| b == b'\n').count() as u32 + 1;
    let line_start = text[..opening].rfind('\n').map_or(0, |i| i + 1);
    let col = text[line_start..opening].chars().count() as u32 + 1;
    let checkpoint = interp.heap.checkpoint();
    let reading = reader::read_all_positioned(&mut interp.heap, text)
        .ok()
        .and_then(|positioned| {
            let forms: Vec<_> = positioned.into_iter().map(|(form, _)| form).collect();
            check::arg_gradual_at(&mut interp.heap, &forms, line, col, arg_index)
        })
        .map(|gradual| check::describe_reading(&gradual, brood::types::strict_checking()));
    interp.heap.reset_local_to(checkpoint);
    reading
}

fn code(text: &str) -> String {
    format!("```brood\n{text}\n```")
}

fn render_def(d: &defs::Def) -> String {
    let mut s = code(&d.signature());
    if let Some(doc) = d.doc {
        s.push_str("\n\n");
        s.push_str(doc);
    }
    s
}

/// Render a free (prelude/builtin) name: its arglist, then the checker's type signature
/// (when known), then its docstring. With none of the three there's nothing useful to
/// show, so the popup is suppressed (`None`).
fn render_global(
    name: &str,
    sig: Option<String>,
    ty: Option<String>,
    notes: &[Option<String>],
    doc: Option<String>,
) -> Option<String> {
    if sig.is_none() && ty.is_none() && doc.is_none() {
        return None;
    }
    let mut s = code(sig.as_deref().unwrap_or(name));
    if let Some(ty) = ty {
        s.push_str(&format!("\n\n`{ty}`"));
    }
    for note in notes.iter().flatten() {
        s.push_str(&format!("\n\n**{note}**"));
    }
    if let Some(doc) = doc {
        s.push_str("\n\n");
        s.push_str(&doc);
    }
    Some(s)
}

/// Hover for a `(:use foo)` / `(:alias foo)` module name: a `(module foo)` header
/// plus the docstring its `defmodule` declared (when the module is loaded). Always
/// renders the header so the popup confirms what the name refers to.
fn render_module(interp: &mut Interp, name: &str) -> Option<String> {
    let mut s = code(&format!("(module {name})"));
    if let Some(doc) = introspect::module_doc(interp, name) {
        s.push_str("\n\n");
        s.push_str(&doc);
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn hover_at(src: &str, needle: &str) -> Option<String> {
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find(needle).unwrap() as u32;
        hover(&mut interp, src, &root, &tree, &index, at).map(|h| match h.contents {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup hover"),
        })
    }

    #[test]
    fn hover_says_how_a_call_reads_its_argument() {
        // A declared parameter is precise: a call reads it by inclusion, which is why a
        // `number` handed to an `int` parameter is reported. A call result is dynamic: read
        // by overlap, so the same call with `(maybe x)` is silent. The hover says which.
        let src = "(defn takes-int (n:int -> int) n)\n\
                   (defn maybe (n:int -> (or int bool)) (if (= n 0) false n))\n\
                   (defn outer (x:number) (takes-int x))\n\
                   (defn outer2 (y:int) (takes-int (maybe y)))\n";
        let at_x = src.rfind("takes-int x").unwrap() + "takes-int ".len();
        let precise = hover_at(src, &src[at_x..]).expect("hover on x");
        assert!(
            precise.contains("as an argument here:") && precise.contains("precise"),
            "a declared parameter reads as precise: {precise}"
        );
        let dynamic = hover_at(src, "maybe y").expect("hover on maybe");
        assert!(
            dynamic.contains("as an argument here:") && dynamic.contains("dynamic"),
            "a call result reads as dynamic: {dynamic}"
        );
        // Not an argument: a binder in a parameter list gets no reading.
        let binder = hover_at(src, "x:number").unwrap_or_default();
        assert!(!binder.contains("as an argument here"), "{binder}");
    }

    #[test]
    fn hover_names_a_deprecation_and_its_replacement() {
        // The prelude deprecates `not=` in favour of `not` (ADR-283).
        let md = hover_at("(not= 1 2)", "not=").expect("hover on not=");
        assert!(
            md.contains("deprecated since") && md.contains("`not`"),
            "hover must carry the deprecation and its replacement: {md}"
        );
    }

    #[test]
    fn hover_names_a_beta_mark_and_its_reason() {
        let src = "(wobbly 1)";
        let mut interp = Interp::new();
        interp
            .eval_str(
                "(defn wobbly (x) \"A test surface.\" x) (meta wobbly :beta \"still settling\")",
            )
            .expect("define the beta name");
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let md = match hover(&mut interp, src, &root, &tree, &index, 1)
            .expect("hover on wobbly")
            .contents
        {
            HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup hover"),
        };
        assert!(
            md.contains("beta — still settling"),
            "hover must carry the beta reason: {md}"
        );
    }

    #[test]
    fn hovers_a_prelude_function_with_signature() {
        let md = hover_at("(map xs f)", "map").expect("hover on map");
        assert!(md.contains("(map "), "signature missing: {md:?}");
    }

    #[test]
    fn hovers_a_document_def_with_its_docstring() {
        let src = "(defn sq (x) \"square it\" (* x x)) (sq 3)";
        // hover on the *call site* — it resolves to the document def.
        let md = hover_at(src, "sq 3").expect("hover on sq call");
        assert!(md.contains("(sq x)"), "signature missing: {md:?}");
        assert!(md.contains("square it"), "docstring missing: {md:?}");
    }

    #[test]
    fn hovers_a_local_as_a_local_binding() {
        let md = hover_at("(defn f (x) (+ x 1))", "x 1").expect("hover on x");
        assert!(md.contains("local binding"), "{md:?}");
    }

    #[test]
    fn hovers_a_nested_def_by_finding_it_at_any_depth() {
        // `helper` is global despite being nested in a `do`; hover must still
        // render its signature, not fall back to the bare name.
        let src = "(do (defn helper (x) (* x 2))) (helper 3)";
        let md = hover_at(src, "helper 3").expect("hover on helper call");
        assert!(md.contains("(helper x)"), "signature missing: {md:?}");
    }

    #[test]
    fn hovers_a_primitive_with_signature_and_doc() {
        // Builtins now carry params + a docstring (PRIMITIVE_DOCS), so hover on a
        // primitive renders both, like a Brood function.
        let md = hover_at("(cons 1 xs)", "cons").expect("hover on cons");
        assert!(md.contains("(cons x xs)"), "signature missing: {md:?}");
        assert!(md.contains("pair"), "doc missing: {md:?}");
    }

    #[test]
    fn hover_shows_the_checker_type_signature_of_a_builtin() {
        // Beyond the arglist + docstring, hover now surfaces the checker's TYPE signature
        // (an arrow) for a resolvable name — `map`'s domain/result.
        let md = hover_at("(map xs f)", "map").expect("hover on map");
        assert!(
            md.contains("->"),
            "expected a type-signature arrow in the hover: {md:?}"
        );
    }

    #[test]
    fn no_hover_on_a_literal() {
        assert!(hover_at("(+ 1 2)", "1").is_none());
    }

    #[test]
    fn hovers_a_use_module_name_with_its_docstring() {
        // Load a module with a docstring, then hover its name in a `(:use …)` clause.
        let dir = std::env::temp_dir().join(format!("brood_use_hover_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("greeter.blsp"),
            "(defmodule greeter \"says hello\")\n",
        )
        .unwrap();

        let mut interp = Interp::new();
        interp
            .eval_str(&format!(
                "(def *load-path* (cons \"{}\" *load-path*))",
                dir.display()
            ))
            .expect("extend load-path");
        interp
            .eval_str("(require-one 'greeter)")
            .expect("load greeter");

        let src = "(defmodule app (:use greeter))";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let at = src.find("greeter").unwrap() as u32;
        let md = hover(&mut interp, src, &root, &tree, &index, at)
            .map(|h| match h.contents {
                HoverContents::Markup(m) => m.value,
                _ => panic!("expected markup"),
            })
            .expect("hover on the :use module name");
        assert!(md.contains("(module greeter)"), "header missing: {md:?}");
        assert!(md.contains("says hello"), "docstring missing: {md:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
