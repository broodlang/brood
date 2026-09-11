//! The tree-sitter primitives: parse foreign-language text with a compiled grammar and
//! walk the resulting tree (`std/editor/treesit.blsp` is the policy). The mechanism is
//! `crate::host::treesit`; without the `treesit` feature every call reports so at runtime.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::numeric::{arg, expect_int, expect_string};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // Foreign-language CST via tree-sitter (feature "treesit"), in the SAME node
    // shape as `parse-source-positioned` so std/sexp + the editor modes navigate
    // it unchanged. Always registered; errors if built without the feature. §C.
    primitives.def(
        "%tree-sitter-parse",
        Arity::exact(2),
        Sig::new(vec![string, kw], map_ty),
        &["source", "lang"],
        "Parse source (a string) with the tree-sitter grammar named by keyword lang into a positioned CST — the SAME node-map shape as parse-source-positioned (`{:kind :start :end :named}`; leaves add :text, nodes with children add :kids), :kind a keyword of the tree-sitter node type and :named false for anonymous tokens (keywords/punctuation). Char offsets, so std/sexp + the editor's fontify navigate it unchanged. The generic mechanism is in the default build, but the kernel ships NO language grammar — a grammar is opt-in (e.g. --features treesit-ruby, or treesit-grammars for all). Errors if the named language's grammar isn't built in, or if the runtime was built without --features treesit.",
        tree_sitter_parse);
    // The positional queries. `%tree-sitter-parse` projects the WHOLE tree into Brood
    // maps — 9,561 of them for a 22 KB Elixir file — and every consumer that only wants
    // to know what encloses a point then walks that to read about ten. These answer in
    // O(depth) without building the rest.
    primitives.def(
        "%tree-sitter-chain",
        Arity::exact(3),
        Sig::new(vec![string, kw, int], vec_ty),
        &["source", "lang", "offset"],
        "The nodes containing char `offset`, outermost first, each as {:kind :start :end :named :container} WITHOUT its children. The enclosing-context query an indenter or backward-up-list wants, answered in O(depth) instead of by projecting the whole tree and walking it. Same grammar rules and errors as %tree-sitter-parse.",
        tree_sitter_chain);
    primitives.def(
        "%tree-sitter-kids",
        Arity::exact(3),
        Sig::new(vec![string, kw, int], vec_ty),
        &["source", "lang", "offset"],
        "The named children of the deepest node STRICTLY containing char `offset`, each as {:kind :start :end :named :container} without its own children — the sibling list every structural motion needs (forward-sexp is the next one starting at or after point). Strict containment is what makes a point at a node's start belong to its parent, so forward-sexp steps over the form at point rather than into it. Same grammar rules and errors as %tree-sitter-parse.",
        tree_sitter_kids);
    primitives.def(
        "%tree-sitter-spans",
        Arity::exact(4),
        Sig::new(vec![string, kw, list_ty.union(vec_ty), any], vec_ty),
        &["source", "lang", "kinds", "keywords?"],
        "The fontify query: [start end kind] for the OUTERMOST nodes whose :kind is in `kinds` (a list/vector of keywords), in source order, not descending into one that matched. With keywords? true, an anonymous token beginning with a letter is also reported, as kind :__keyword__ — the cross-language keyword rule, which no grammar names. Which kinds get which face stays with the caller; what this avoids is projecting every node into a map to read its kind and discard the rest, which is what a windowed fontify did on every keystroke.",
        tree_sitter_spans);
    primitives.def(
        "%tree-sitter-load-grammar",
        Arity::exact(2),
        Sig::new(vec![string, kw], int),
        &["path", "lang"],
        "Load a tree-sitter grammar from the shared library at `path` and register it under the language keyword `lang`, so tree-sitter-parse can use it like a built-in grammar; returns its ABI version. This is how a language the runtime was not built with gets parsed at all — the kernel deliberately knows no path convention, so the caller names the library and an application keeps its own grammar directory. A grammar loaded this way overrides a bundled one of the same name. Errors (never crashes) on a missing file, a library exporting no tree_sitter_<lang>, or a grammar built for an incompatible tree-sitter ABI; it is native code, though, so load only from a directory the user controls. Feature `treesit`.",
        tree_sitter_load_grammar);
}

/// `(tree-sitter-parse source lang)` — parse a foreign language into the same
/// positioned-CST node shape as `parse-source-positioned`. Mechanism lives in
/// `crate::host::treesit` (feature-gated); this just unwraps the args. See §C.
pub(super) fn tree_sitter_parse(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "tree-sitter-parse", arg(args, 0))?;
    let lang = match arg(args, 1) {
        Value::Keyword(s) => value::symbol_name(s),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "tree-sitter-parse",
                "keyword",
                v,
            ))
        }
    };
    crate::host::treesit::parse(heap, &src, &lang)
}

/// `(tree-sitter-chain source lang offset)` / `(tree-sitter-kids source lang offset)` —
/// the two positional queries. Mechanism in `crate::host::treesit`; these unwrap the args.
fn ts_lang(heap: &Heap, who: &'static str, v: Value) -> Result<String, LispError> {
    match v {
        Value::Keyword(s) => Ok(value::symbol_name(s)),
        other => Err(LispError::wrong_type(heap, who, "keyword", other)),
    }
}

pub(super) fn tree_sitter_chain(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "tree-sitter-chain", arg(args, 0))?;
    let lang = ts_lang(heap, "tree-sitter-chain", arg(args, 1))?;
    let offset = expect_int(heap, "tree-sitter-chain", arg(args, 2))?;
    crate::host::treesit::chain(heap, &src, &lang, offset)
}

pub(super) fn tree_sitter_kids(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "tree-sitter-kids", arg(args, 0))?;
    let lang = ts_lang(heap, "tree-sitter-kids", arg(args, 1))?;
    let offset = expect_int(heap, "tree-sitter-kids", arg(args, 2))?;
    crate::host::treesit::kids(heap, &src, &lang, offset)
}

pub(super) fn tree_sitter_spans(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "tree-sitter-spans", arg(args, 0))?;
    let lang = ts_lang(heap, "tree-sitter-spans", arg(args, 1))?;
    let mut kinds: Vec<String> = Vec::new();
    for k in heap.seq_items(arg(args, 2))? {
        match k {
            Value::Keyword(s) => kinds.push(value::symbol_name(s)),
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    "tree-sitter-spans",
                    "keyword",
                    other,
                ))
            }
        }
    }
    let keywords = !matches!(arg(args, 3), Value::Nil | Value::Bool(false));
    crate::host::treesit::spans(heap, &src, &lang, kinds, keywords)
}

/// `(tree-sitter-load-grammar path lang)` — load a grammar shared library at
/// runtime and register it under `lang`. Mechanism in `crate::host::treesit`
/// (feature-gated); this just unwraps the args.
pub(super) fn tree_sitter_load_grammar(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "tree-sitter-load-grammar", arg(args, 0))?;
    let lang = match arg(args, 1) {
        Value::Keyword(s) => value::symbol_name(s),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "tree-sitter-load-grammar",
                "keyword",
                v,
            ))
        }
    };
    crate::host::treesit::load_grammar(&path, &lang)
}
