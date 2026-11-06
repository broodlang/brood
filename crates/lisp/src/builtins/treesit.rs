//! The tree-sitter primitives: parse foreign-language text with a compiled grammar and
//! walk the resulting tree (`std/editor/treesit.blsp` is the policy). The mechanism is
//! `crate::host::treesit`; without the `treesit` feature every call reports so at runtime.

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::{LispError, LispResult};

use super::args::{arg, expect_int, expect_string, expect_string_ref};

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
    // maps — 9,561 of them for a 22 KB source file — and every consumer that only wants
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
        Arity::range(3, 4),
        Sig::with_optional(vec![string, kw, int], vec![any], vec_ty),
        &["source", "lang", "offset", "&optional", "all?"],
        "The named children of the deepest node STRICTLY containing char `offset`, each as {:kind :start :end :named :container} without its own children — the sibling list every structural motion needs (forward-sexp is the next one starting at or after point). Strict containment is what makes a point at a node's start belong to its parent, so forward-sexp steps over the form at point rather than into it. With all? true, the ANONYMOUS children too — the parent's own tokens, in source order among the named ones, each :named false with the token's text as its :kind (`:{`, `:do`) — so a bracket matcher reads a delimiter pair in the middle of its parent (an HCL block's braces after its labels) from the list instead of scanning the text between named children. Same grammar rules and errors as %tree-sitter-parse.",
        tree_sitter_kids);
    primitives.def(
        "%tree-sitter-errors",
        Arity::exact(2),
        Sig::new(vec![string, kw], vec_ty),
        &["source", "lang"],
        "Where the parse came apart: the ERROR and MISSING nodes as {:kind :start :end :missing? :text}, outermost first, [] for a clean parse. A MISSING node is a token the grammar expected and inserted (its :kind IS that token, and it is zero-width); an ERROR node spans text the parser could not place. The distinction a live evaluator needs — text someone is still typing must not be reported as a mistake — which %tree-sitter-chain's :broken flag cannot make, since it says only that the tree has an error somewhere. Capped at 32 nodes, each :text at 40 chars. Same grammar rules and errors as %tree-sitter-parse.",
        tree_sitter_errors);
    primitives.def(
        "%tree-sitter-spans",
        Arity::range(4, 5),
        Sig::with_optional(vec![string, kw, list_ty.union(vec_ty).union(map_ty), any], vec![any], vec_ty),
        &["source", "lang", "table", "keywords?", "&optional", "injections"],
        "The fontify query: [start end reported] for the OUTERMOST nodes the face `table` colours, in source order, not descending into one that matched. `table` is a list/vector of kind keywords (each reported as itself), or a map from SELECTOR to what a span reports: a selector is a kind keyword, or a context map {:kind :parent :field :nth :text :after} — the node's kind, its parent (a kind, or a selector map the parent must satisfy in turn, at most 4 maps deep), the field it fills in the parent (tree-sitter's field name), its index among the parent's named children, its exact source text (a string or a set of strings) and a {:kind :text} map the named sibling right before it must satisfy (ADR-614) — whose given constraints must all hold. When several match, the most specific wins: the node's own constraints first (:after over :text over :field over :parent over :nth over :kind), then each ancestor selector's, then the narrower :text set, so any context entry beats the plain kind (ADR-486, ADR-487). With keywords? true, an anonymous token beginning with a letter is also reported, as kind :__keyword__ — the cross-language keyword rule, which no grammar names; keywords? may instead be a set of the rules wanted, #{:keywords :punctuation}, where :punctuation reports an anonymous token made only of brackets, commas and semicolons as :__punctuation__ (ADR-614). Which kinds get which face stays with the caller; what this avoids is projecting every node into a map to read its kind and discard the rest, which is what a windowed fontify did on every keystroke. INJECTIONS (ADR-489): an optional {:rules [rule …] :faces {lang table}} naming the nodes whose text is another language's document — a rule is {:content selector :lang :bash} or {:content selector :lang-from kind :aliases {word lang}} (the language read from the first word of the content's sibling, or child, of that kind) — whose spans, from that language's :faces table, replace the host's over the content; a parent's matching children are ONE document (included ranges, joined by the line breaks between them), a rule's optional :exclude selector cuts the content's descendants it selects out of the document so they keep the host's colours (ADR-496), and a document whose language has no table or no grammar keeps the host's colours.",
        tree_sitter_spans);
    // Tree HANDLES (ADR-443): a whole document's parse tree, held across edits and
    // reparsed incrementally, queried over a range — what an editor's view keeps per
    // buffer instead of re-parsing a window of text on every keystroke.
    primitives.def(
        "%tree-sitter-tree",
        Arity::exact(2),
        Sig::new(vec![rope.union(string), kw], ref_ty),
        &["text", "lang"],
        "Parse a whole document — a rope (read chunk by chunk, never copied) or a string — with the tree-sitter grammar named by keyword lang, and return a tree HANDLE: an opaque ref naming the tree and the text it was parsed from. A handle is immutable: %tree-sitter-tree-reparse returns a new one and leaves this one answering as before. The handle belongs to the calling process (ADR-495): it is held until that process frees it (%tree-sitter-tree-free — do, for one you have replaced) or exits, and no other process can take it away. A gone handle reads as nil from every query and reparsing from it is a full parse. Sendable by value and readable from any process; a task that parses off the loop hands the tree back with %tree-sitter-tree-give before replying, or it dies with the task. Same grammar rules and errors as %tree-sitter-parse.",
        tree_sitter_tree);
    primitives.def(
        "%tree-sitter-tree-reparse",
        Arity::exact(4),
        Sig::new(vec![ref_ty, rope.union(string), kw, any], ref_ty),
        &["tree", "text", "lang", "edits"],
        "A NEW tree handle for text, reparsed incrementally from tree's tree (tree-sitter's edit + reparse, ~1 ms where a fresh parse of a large file is hundreds). edits says what changed since tree's text, in CHAR offsets: one [start old-end new-end] (chars [start, old-end) of the old text became [start, new-end) of the new — a splice [lo hi repl] is [lo hi (+ lo (count repl))]), a sequence of them applied in order, or nil to derive the edit by comparing the two texts (common prefix and suffix, chunk by chunk — cheap, and exact for any single contiguous change). An edit whose lengths do not fit the two texts is an error; one that fits them but names the wrong place gives a wrong tree, so pass nil when unsure. A gone handle, another lang, or a grammar reloaded since is a full parse. tree stays valid; the new handle belongs to the calling process, whoever owns tree.",
        tree_sitter_tree_reparse);
    primitives.def(
        "%tree-sitter-tree-spans",
        Arity::range(5, 6),
        Sig::with_optional(vec![ref_ty, list_ty.union(vec_ty).union(map_ty), any, int, int], vec![any], vec_or_nil),
        &["tree", "table", "keywords?", "from", "to", "&optional", "injections"],
        "The fontify query over a held tree, for chars [from, to) of its text only: [start end reported] for the OUTERMOST nodes the face table colours (as in %tree-sitter-spans, context selectors included) that reach into the range, in source order, each clipped to the range — so a string opened above it reports its visible part, coloured as a string, because the tree is the whole document's. Walks only the nodes that reach the range. keywords? as in %tree-sitter-spans (an anonymous token beginning with a letter is :__keyword__; a set #{:keywords :punctuation} adds :__punctuation__). Absolute char offsets. nil when the handle is gone (freed, or its owner exited): open a fresh tree. injections as in %tree-sitter-spans: the embedded documents reaching the range are parsed for this call (no injected tree is held), each whole when it fits 32 KB and cut to the range otherwise.",
        tree_sitter_tree_spans);
    primitives.def(
        "%tree-sitter-tree-changes",
        Arity::range(1, 2),
        Sig::with_optional(vec![ref_ty], vec![any], any),
        &["tree", "&optional", "injections"],
        "Where tree's colouring may differ from the tree it was reparsed from: a sorted vector of disjoint char ranges [start end] of its text — the edited text itself plus tree-sitter's changed ranges (a quote typed above the screen recolours everything after it, and this says so). [] when the text did not change; true for a tree from a fresh parse (everything may differ); nil when the handle is gone. What a span cache invalidates by, instead of throwing every band away on any edit. With injections (as in %tree-sitter-spans), each range is widened over the embedded documents it reaches, whole: an edit inside a fence can recolour the fence below it, which the host tree cannot know.",
        tree_sitter_tree_changes);
    primitives.def(
        "%tree-sitter-tree-free",
        Arity::exact(1),
        Sig::new(vec![ref_ty], bool_ty),
        &["tree"],
        "Drop a tree handle now, instead of when its owner exits; true when it was still held. Every later query on it answers nil.",
        tree_sitter_tree_free);
    primitives.def(
        "%tree-sitter-load-grammar",
        Arity::exact(2),
        Sig::new(vec![string, kw], int),
        &["path", "lang"],
        "Load a tree-sitter grammar from the shared library at `path` and register it under the language keyword `lang`, so tree-sitter-parse can use it like a built-in grammar; returns its ABI version. This is how a language the runtime was not built with gets parsed at all — the kernel deliberately knows no path convention, so the caller names the library and an application keeps its own grammar directory. A grammar loaded this way overrides a bundled one of the same name. Errors (never crashes) on a missing file, a library exporting no tree_sitter_<lang>, or a grammar built for an incompatible tree-sitter ABI; it is native code, though, so load only from a directory the user controls. Feature `treesit`.",
        tree_sitter_load_grammar);
    primitives.def(
        "%tree-sitter-languages",
        Arity::exact(0),
        Sig::new(vec![], vec_ty),
        &[],
        "The languages this runtime can parse right now, as a sorted vector of keywords: every grammar loaded with %tree-sitter-load-grammar plus those built in. What an editor asks before offering to fetch a grammar, or telling you a file's mode has none — instead of parsing something and reading the error. [] in a build without feature `treesit`.",
        tree_sitter_languages);
}

/// The structural queries over a held tree (ADR-467) — registered LAST from `builtins::register`,
/// not beside their siblings above: registration order feeds the intern table (see
/// `syntax_scan::register`).
pub(super) fn register_tree_queries(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // `%tree-sitter-chain` and `%tree-sitter-kids` without re-parsing the text they are
    // handed: a caller that keeps the document's tree incrementally (an editor's view) asks it.
    primitives.def(
        "%tree-sitter-tree-chain",
        Arity::exact(2),
        Sig::new(vec![ref_ty, int], vec_or_nil),
        &["tree", "offset"],
        "%tree-sitter-chain over a held tree: the nodes containing char offset of the tree's text, outermost first, each as {:kind :start :end :named :container} without its children, the outermost carrying :broken true when the tree has an error anywhere. Parses nothing — O(depth) over the tree an incremental reparse keeps. nil when the handle is gone (freed, or its owner exited): ask the text-keyed query, or open a fresh tree.",
        tree_sitter_tree_chain,
    );
    primitives.def(
        "%tree-sitter-tree-kids",
        Arity::range(2, 3),
        Sig::with_optional(vec![ref_ty, int], vec![any], vec_or_nil),
        &["tree", "offset", "&optional", "all?"],
        "%tree-sitter-kids over a held tree: the named children of the deepest node STRICTLY containing char offset, each without its own children — with all? true, its anonymous token children too, as in %tree-sitter-kids. Parses nothing. nil when the handle is gone.",
        tree_sitter_tree_kids,
    );
    primitives.def(
        "%tree-sitter-tree-alive?",
        Arity::exact(1),
        Sig::new(vec![ref_ty], bool_ty),
        &["tree"],
        "Is the tree handle still held — not freed, its owner not exited? Builds nothing — the check a holder makes per event instead of asking %tree-sitter-tree-changes for a vector it throws away. false in a build without feature `treesit`.",
        tree_sitter_tree_alive,
    );
}

/// `(%tree-sitter-languages)` — the parseable languages; `crate::host::treesit::languages`.
pub(super) fn tree_sitter_languages(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    crate::host::treesit::languages(heap)
}

/// `(tree-sitter-parse source lang)` — parse a foreign language into the same
/// positioned-CST node shape as `parse-source-positioned`. Mechanism lives in
/// `crate::host::treesit` (feature-gated); this just unwraps the args. See §C.
pub(super) fn tree_sitter_parse(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%tree-sitter-parse", arg(args, 0))?;
    let lang = match arg(args, 1) {
        Value::Keyword(s) => value::symbol_name(s),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%tree-sitter-parse",
                "keyword",
                v,
            ));
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
    let src = expect_string(heap, "%tree-sitter-chain", arg(args, 0))?;
    let lang = ts_lang(heap, "%tree-sitter-chain", arg(args, 1))?;
    let offset = expect_int(heap, "%tree-sitter-chain", arg(args, 2))?;
    crate::host::treesit::chain(heap, &src, &lang, offset)
}

pub(super) fn tree_sitter_kids(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%tree-sitter-kids", arg(args, 0))?;
    let lang = ts_lang(heap, "%tree-sitter-kids", arg(args, 1))?;
    let offset = expect_int(heap, "%tree-sitter-kids", arg(args, 2))?;
    let all = !matches!(arg(args, 3), Value::Nil | Value::Bool(false));
    crate::host::treesit::kids(heap, &src, &lang, offset, all)
}

pub(super) fn tree_sitter_errors(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%tree-sitter-errors", arg(args, 0))?;
    let lang = ts_lang(heap, "%tree-sitter-errors", arg(args, 1))?;
    crate::host::treesit::errors(heap, &src, &lang)
}

pub(super) fn tree_sitter_spans(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let src = expect_string(heap, "%tree-sitter-spans", arg(args, 0))?;
    let lang = ts_lang(heap, "%tree-sitter-spans", arg(args, 1))?;
    let table = ts_table(heap, "%tree-sitter-spans", arg(args, 2))?;
    let tokens = ts_tokens(heap, "%tree-sitter-spans", arg(args, 3))?;
    let injections = ts_injections(heap, "%tree-sitter-spans", arg(args, 4))?;
    crate::host::treesit::spans(heap, &src, &lang, &table, tokens, injections.as_ref())
}

/// `(tree-sitter-load-grammar path lang)` — load a grammar shared library at
/// runtime and register it under `lang`. Mechanism in `crate::host::treesit`
/// (feature-gated); this just unwraps the args.
pub(super) fn tree_sitter_load_grammar(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%tree-sitter-load-grammar", arg(args, 0))?;
    let lang = match arg(args, 1) {
        Value::Keyword(s) => value::symbol_name(s),
        v => {
            return Err(LispError::wrong_type(
                heap,
                "%tree-sitter-load-grammar",
                "keyword",
                v,
            ));
        }
    };
    crate::host::treesit::load_grammar(&path, &lang)
}

// ---- tree handles (ADR-443) ----------------------------------------------------------

/// The keyword list a span query names, as strings.
fn ts_kinds(heap: &Heap, who: &'static str, v: Value) -> Result<Vec<String>, LispError> {
    let mut kinds: Vec<String> = Vec::new();
    for k in heap.seq_items(v)? {
        match k {
            Value::Keyword(s) => kinds.push(value::symbol_name(s)),
            other => return Err(LispError::wrong_type(heap, who, "keyword", other)),
        }
    }
    Ok(kinds)
}

/// A span query's face table (ADR-486, ADR-487): a sequence of kind keywords (each reported
/// as itself), or a map whose keys are SELECTORS and whose values are what a span reports. A
/// selector is a kind keyword, or a map naming the node's context:
///
/// ```text
/// {:kind :identifier :parent :block :nth 0}   ; the identifier first among a block's named children
/// {:field :function}                          ; whatever fills a parent's `function` field
/// {:kind :identifier :text #{"def" "if"}}     ; an identifier spelt `def` or `if`
/// {:kind :field_identifier
///  :parent {:kind :field_expression :field :function}}   ; a method call's name
/// {:kind :arguments :after {:text "def"}}     ; what follows a `def` head: a grammar's definition form
/// ```
///
/// `:kind` and `:field` are keywords, `:nth` a non-negative int, `:text` a string or a
/// non-empty set of strings, `:after` a map of `:kind` and `:text` alone that the named sibling
/// right before the node must satisfy (ADR-614), and `:parent` a kind keyword or a selector
/// map the parent must satisfy, nested at most [`MAX_SELECTOR_DEPTH`](crate::host::treesit::MAX_SELECTOR_DEPTH)
/// maps deep. At least one constraint is given, and nothing else is accepted, so a misspelt
/// constraint fails at the call rather than quietly matching everything.
fn ts_table(
    heap: &Heap,
    who: &'static str,
    v: Value,
) -> Result<crate::host::treesit::SpanTable, LispError> {
    use crate::host::treesit::{Selector, SpanRule, SpanTable};
    let Value::Map(id) = v else {
        return Ok(SpanTable::of_kinds(ts_kinds(heap, who, v)?));
    };
    let mut rules = Vec::new();
    for (selector, reported) in heap.map_entries(id) {
        let selector = match selector {
            Value::Keyword(kind) => Selector::of_kind(value::symbol_name(kind)),
            Value::Map(selector_id) => ts_selector(heap, who, selector_id, 1)?,
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    who,
                    "a node kind keyword or a {:kind :parent :field :nth :text} selector map",
                    other,
                ));
            }
        };
        rules.push(SpanRule { selector, reported });
    }
    Ok(SpanTable::new(rules))
}

/// One context selector of a face table, `depth` maps deep (the key itself is 1).
fn ts_selector(
    heap: &Heap,
    who: &'static str,
    selector_map: value::MapId,
    depth: usize,
) -> Result<crate::host::treesit::Selector, LispError> {
    use crate::host::treesit::{MAX_SELECTOR_DEPTH, Selector, TextMatch};
    let mut selector = Selector::default();
    let named = |constraint: &str, v: Value| match v {
        Value::Keyword(name) => Ok(value::symbol_name(name)),
        other => Err(LispError::wrong_type(
            heap,
            who,
            &format!("a keyword for selector {constraint}"),
            other,
        )),
    };
    for (key, v) in heap.map_entries(selector_map) {
        let constraint = match key {
            Value::Keyword(k) => value::symbol_name(k),
            other => return Err(LispError::wrong_type(heap, who, "a selector key", other)),
        };
        match constraint.as_str() {
            "kind" => selector.kind = Some(named(":kind", v)?),
            "field" => selector.field = Some(named(":field", v)?),
            "parent" => {
                let parent = match v {
                    Value::Keyword(kind) => Selector::of_kind(value::symbol_name(kind)),
                    Value::Map(parent_map) if depth < MAX_SELECTOR_DEPTH => {
                        ts_selector(heap, who, parent_map, depth + 1)?
                    }
                    Value::Map(_) => {
                        return Err(LispError::runtime(format!(
                            "{who}: a face-table selector nests :parent maps at most {MAX_SELECTOR_DEPTH} deep"
                        )));
                    }
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            who,
                            "a kind keyword or a selector map for selector :parent",
                            other,
                        ));
                    }
                };
                selector.parent = Some(Box::new(parent));
            }
            "nth" => {
                let n = expect_int(heap, who, v)?;
                selector.nth = Some(usize::try_from(n).map_err(|_| {
                    LispError::runtime(format!(
                        "{who}: a selector's :nth is never negative, got {n}"
                    ))
                })?);
            }
            "text" => {
                let texts = match v {
                    Value::Str(_) => vec![expect_string(heap, who, v)?],
                    Value::Set(set_id) => heap
                        .set_elems(set_id)
                        .into_iter()
                        .map(|element| match element {
                            Value::Str(_) => expect_string(heap, who, element),
                            other => Err(LispError::wrong_type(
                                heap,
                                who,
                                "a string in selector :text",
                                other,
                            )),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            who,
                            "a string or a set of strings for selector :text",
                            other,
                        ));
                    }
                };
                if texts.is_empty() {
                    return Err(LispError::runtime(format!(
                        "{who}: a selector's :text set #{{}} would match no node"
                    )));
                }
                selector.text = Some(TextMatch::new(texts));
            }
            "after" => {
                // the named sibling before the node: a kind and/or a text, nothing about
                // where IT sits (ADR-614)
                let Value::Map(after_map) = v else {
                    return Err(LispError::wrong_type(
                        heap,
                        who,
                        "a {:kind :text} selector map for selector :after",
                        v,
                    ));
                };
                let after = ts_selector(heap, who, after_map, depth)?;
                if after.parent.is_some()
                    || after.field.is_some()
                    || after.nth.is_some()
                    || after.after.is_some()
                {
                    return Err(LispError::runtime(format!(
                        "{who}: a selector's :after names the sibling's :kind and :text only"
                    )));
                }
                selector.after = Some(Box::new(after));
            }
            other => {
                return Err(LispError::runtime(format!(
                    "{who}: a face-table selector takes :kind :parent :field :nth :text :after, not :{other}"
                )));
            }
        }
    }
    if selector.is_empty() {
        return Err(LispError::runtime(format!(
            "{who}: an empty face-table selector {{}} would colour every node"
        )));
    }
    Ok(selector)
}

/// A document's text as a rope: a rope as itself (a reference count, no copy), a string
/// copied into one.
fn ts_text(heap: &Heap, who: &'static str, v: Value) -> Result<ropey::Rope, LispError> {
    match v {
        Value::Rope(id) => Ok(heap.rope(id).clone()),
        Value::Str(_) => Ok(ropey::Rope::from_str(expect_string_ref(heap, who, v)?)),
        other => Err(LispError::wrong_type(heap, who, "rope or string", other)),
    }
}

/// A tree handle: the `Ref` `%tree-sitter-tree` returned.
fn ts_handle(heap: &Heap, who: &'static str, v: Value) -> Result<u64, LispError> {
    match v {
        Value::Ref(id) => Ok(id),
        other => Err(LispError::wrong_type(heap, who, "tree handle (ref)", other)),
    }
}

/// A non-negative int, as a char offset.
fn ts_offset(heap: &Heap, who: &'static str, v: Value) -> Result<usize, LispError> {
    let n = expect_int(heap, who, v)?;
    usize::try_from(n).map_err(|_| {
        LispError::runtime(format!("{who}: an edit offset is never negative, got {n}"))
    })
}

/// The edits a reparse is told: nil (derive them), one `[start old-end new-end]`, or a
/// sequence of them applied in order.
fn ts_edits(
    heap: &Heap,
    who: &'static str,
    v: Value,
) -> Result<Option<Vec<(usize, usize, usize)>>, LispError> {
    if matches!(v, Value::Nil) {
        return Ok(None);
    }
    let items = heap.seq_items(v)?;
    let triple = |heap: &Heap, item: Value| -> Result<(usize, usize, usize), LispError> {
        let parts = heap.seq_items(item)?;
        if parts.len() != 3 {
            return Err(LispError::runtime(format!(
                "{who}: an edit is [start old-end new-end], got {} items",
                parts.len()
            )));
        }
        Ok((
            ts_offset(heap, who, parts[0])?,
            ts_offset(heap, who, parts[1])?,
            ts_offset(heap, who, parts[2])?,
        ))
    };
    if items
        .first()
        .is_some_and(|first| matches!(first, Value::Int(_)))
    {
        return Ok(Some(vec![triple(heap, v)?]));
    }
    let edits = items
        .into_iter()
        .map(|item| triple(heap, item))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(edits))
}

pub(super) fn tree_sitter_tree(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let text = ts_text(heap, "%tree-sitter-tree", arg(args, 0))?;
    let lang = ts_lang(heap, "%tree-sitter-tree", arg(args, 1))?;
    let owner = crate::process::self_pid();
    Ok(Value::Ref(crate::host::treesit::tree_open(
        text, &lang, owner,
    )?))
}

pub(super) fn tree_sitter_tree_reparse(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%tree-sitter-tree-reparse";
    let old = ts_handle(heap, who, arg(args, 0))?;
    let text = ts_text(heap, who, arg(args, 1))?;
    let lang = ts_lang(heap, who, arg(args, 2))?;
    let edits = ts_edits(heap, who, arg(args, 3))?;
    Ok(Value::Ref(crate::host::treesit::tree_reparse(
        old,
        text,
        &lang,
        edits,
        crate::process::self_pid(),
    )?))
}

pub(super) fn tree_sitter_tree_spans(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%tree-sitter-tree-spans";
    let handle = ts_handle(heap, who, arg(args, 0))?;
    let table = ts_table(heap, who, arg(args, 1))?;
    let tokens = ts_tokens(heap, who, arg(args, 2))?;
    let from = expect_int(heap, who, arg(args, 3))?;
    let to = expect_int(heap, who, arg(args, 4))?;
    let injections = ts_injections(heap, who, arg(args, 5))?;
    crate::host::treesit::tree_spans(heap, handle, &table, tokens, from, to, injections.as_ref())
}

pub(super) fn tree_sitter_tree_changes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%tree-sitter-tree-changes";
    let handle = ts_handle(heap, who, arg(args, 0))?;
    let injections = ts_injections(heap, who, arg(args, 1))?;
    crate::host::treesit::tree_changes(heap, handle, injections.as_ref())
}

pub(super) fn tree_sitter_tree_free(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let handle = ts_handle(heap, "%tree-sitter-tree-free", arg(args, 0))?;
    Ok(Value::boolean(crate::host::treesit::tree_free(handle)))
}

pub(super) fn tree_sitter_tree_chain(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%tree-sitter-tree-chain";
    let handle = ts_handle(heap, who, arg(args, 0))?;
    let offset = expect_int(heap, who, arg(args, 1))?;
    crate::host::treesit::tree_chain(heap, handle, offset)
}

pub(super) fn tree_sitter_tree_kids(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%tree-sitter-tree-kids";
    let handle = ts_handle(heap, who, arg(args, 0))?;
    let offset = expect_int(heap, who, arg(args, 1))?;
    let all = !matches!(arg(args, 2), Value::Nil | Value::Bool(false));
    crate::host::treesit::tree_kids(heap, handle, offset, all)
}

pub(super) fn tree_sitter_tree_alive(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let handle = ts_handle(heap, "%tree-sitter-tree-alive?", arg(args, 0))?;
    Ok(Value::boolean(crate::host::treesit::tree_alive(handle)))
}

/// Declaring a grammar without loading it (ADR-481) — registered LAST from
/// `builtins::register`, for the intern-table order reason `register_tree_queries` gives.
pub(super) fn register_grammar_declarations(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%tree-sitter-declare-grammar",
        Arity::exact(2),
        Sig::new(vec![string, kw], bool_ty),
        &["path", "lang"],
        "Declare that the tree-sitter grammar for the language keyword `lang` is the shared library at `path`, WITHOUT loading it: the first parse, structural query or tree handle that names `lang` loads it, once, and every later one uses it. Costs a table entry, so an application can declare every grammar it has at startup and dlopen only those its files use. A later declaration of the same name replaces this one; declaring a language that is already loaded reloads it from `path` at its next use (the swap %tree-sitter-load-grammar makes). A load that fails is not retried until the language is declared again: the parse raises the load's message when nothing else parses `lang`, and otherwise keeps the grammar it had; %tree-sitter-grammar-status reports the failure either way. Returns true; errors in a build without feature `treesit`.",
        tree_sitter_declare_grammar,
    );
    primitives.def(
        "%tree-sitter-grammar-status",
        Arity::exact(1),
        Sig::new(vec![kw], any),
        &["lang"],
        "Where the grammar for the language keyword `lang` stands, without loading anything: :declared (declared by %tree-sitter-declare-grammar, not used yet), :loaded (loaded at runtime), :built-in, a failure value carrying the load's message when the last declaration did not load, or nil when this runtime has no grammar for it.",
        tree_sitter_grammar_status,
    );
}

/// `(%tree-sitter-declare-grammar path lang)` — `crate::host::treesit::declare_grammar`.
pub(super) fn tree_sitter_declare_grammar(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%tree-sitter-declare-grammar", arg(args, 0))?;
    let lang = ts_lang(heap, "%tree-sitter-declare-grammar", arg(args, 1))?;
    crate::host::treesit::declare_grammar(&path, &lang)
}

/// `(%tree-sitter-grammar-status lang)` — `crate::host::treesit::grammar_status`.
pub(super) fn tree_sitter_grammar_status(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let lang = ts_lang(heap, "%tree-sitter-grammar-status", arg(args, 0))?;
    crate::host::treesit::grammar_status(heap, &lang)
}

// ---- injections: another language's document inside this one (ADR-489) ----------------

/// Colouring given ranges of a text as one embedded document — registered LAST from
/// `builtins::register`, for the intern-table order reason `register_tree_queries` gives.
pub(super) fn register_injections(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%tree-sitter-range-spans",
        Arity::exact(5),
        Sig::new(
            vec![rope.union(string), kw, list_ty.union(vec_ty), list_ty.union(vec_ty).union(map_ty), any],
            vec_or_nil,
        ),
        &["text", "lang", "ranges", "table", "keywords?"],
        "The fontify query over PART of a text: the char `ranges` ([start end] each) of `text` — a rope (read in place) or a string — parsed as ONE document in the tree-sitter language `lang` (tree-sitter's included ranges, so a heredoc split by interpolations parses as one), and [start end reported] for the outermost nodes the face `table` colours (as in %tree-sitter-spans), in the OUTER text's char offsets, each clipped to the ranges, in source order. What a host without a tree (a line lexer: a Markdown fence, a Dockerfile RUN line) calls to colour embedded code in its own language. nil when no grammar for `lang` loads — an embedded language's grammar is optional, so this never raises for one. Ranges are sorted and merged; past 256 ranges or 128 KB the rest are left uncoloured, so a call is bounded whatever it is handed (ADR-489).",
        tree_sitter_range_spans,
    );
}

/// `(%tree-sitter-range-spans text lang ranges table keywords?)` —
/// `crate::host::treesit::range_spans`.
pub(super) fn tree_sitter_range_spans(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    use crate::host::treesit::RangeText;
    let who = "%tree-sitter-range-spans";
    let text = match arg(args, 0) {
        Value::Rope(id) => RangeText::Rope(heap.rope(id).clone()),
        Value::Str(_) => RangeText::Flat(expect_string(heap, who, arg(args, 0))?),
        other => return Err(LispError::wrong_type(heap, who, "rope or string", other)),
    };
    let lang = ts_lang(heap, who, arg(args, 1))?;
    let mut ranges = Vec::new();
    for range in heap.seq_items(arg(args, 2))? {
        let parts = heap.seq_items(range)?;
        if parts.len() != 2 {
            return Err(LispError::runtime(format!(
                "{who}: a range is [start end], got {} items",
                parts.len()
            )));
        }
        let start = ts_offset(heap, who, parts[0])?;
        let end = ts_offset(heap, who, parts[1])?;
        ranges.push((start, end.max(start)));
    }
    let table = ts_table(heap, who, arg(args, 3))?;
    let tokens = ts_tokens(heap, who, arg(args, 4))?;
    crate::host::treesit::range_spans(heap, text, &lang, &ranges, &table, tokens)
}

/// Which anonymous tokens a span query reports though its table names none (ADR-614): nil or
/// false none; a set of `:keywords` and `:punctuation`; any other value the keyword rule alone,
/// which is what the flag meant before punctuation could be asked for.
fn ts_tokens(
    heap: &Heap,
    who: &'static str,
    v: Value,
) -> Result<crate::host::treesit::TokenRules, LispError> {
    use crate::host::treesit::TokenRules;
    match v {
        Value::Nil | Value::Bool(false) => Ok(TokenRules::default()),
        Value::Set(set_id) => {
            let mut rules = TokenRules::default();
            for element in heap.set_elems(set_id) {
                match element {
                    Value::Keyword(name) if value::symbol_name(name) == "keywords" => {
                        rules.keywords = true
                    }
                    Value::Keyword(name) if value::symbol_name(name) == "punctuation" => {
                        rules.punctuation = true
                    }
                    other => {
                        return Err(LispError::wrong_type(
                            heap,
                            who,
                            "a token rule, :keywords or :punctuation",
                            other,
                        ));
                    }
                }
            }
            Ok(rules)
        }
        _ => Ok(TokenRules::KEYWORDS),
    }
}

/// A span query's injections (ADR-489), or None for nil:
///
/// ```text
/// {:rules [{:content {:kind :quoted_content :parent :sigil}
///           :lang-from :sigil_name :aliases {"H" :heex}}
///          {:content {:kind :template_literal :parent :heredoc_template} :lang :bash}]
///  :faces {:heex {…face table…} :bash {…}}}
/// ```
///
/// A rule's `:content` is a face-table selector (a kind keyword or a selector map); it names
/// its language with `:lang` (a keyword) or `:lang-from` (a node kind keyword) plus optional
/// `:aliases` (string word → language keyword), never both. `:faces` maps each injected
/// language to its face table. Anything else is refused, as a face table's misspelt key is.
fn ts_injections(
    heap: &Heap,
    who: &'static str,
    v: Value,
) -> Result<Option<crate::host::treesit::Injections>, LispError> {
    use crate::host::treesit::{InjectedLanguage, InjectionRule, Injections, Selector};
    let Value::Map(id) = v else {
        return match v {
            Value::Nil => Ok(None),
            other => Err(LispError::wrong_type(
                heap,
                who,
                "nil or an injection spec {:rules [...] :faces {...}}",
                other,
            )),
        };
    };
    let keyword_name = |heap: &Heap, what: &str, v: Value| match v {
        Value::Keyword(name) => Ok(value::symbol_name(name)),
        other => Err(LispError::wrong_type(
            heap,
            who,
            &format!("a keyword for {what}"),
            other,
        )),
    };
    let mut injections = Injections::default();
    for (key, entry) in heap.map_entries(id) {
        match keyword_name(heap, "an injection spec key", key)?.as_str() {
            "rules" => {
                for rule in heap.seq_items(entry)? {
                    let Value::Map(rule_id) = rule else {
                        return Err(LispError::wrong_type(
                            heap,
                            who,
                            "an injection rule map",
                            rule,
                        ));
                    };
                    let mut content: Option<Selector> = None;
                    let mut exclude: Option<Selector> = None;
                    let mut fixed: Option<String> = None;
                    let mut from: Option<String> = None;
                    let mut aliases = std::collections::HashMap::new();
                    let rule_selector = |heap: &Heap, what: &str, v: Value| match v {
                        Value::Keyword(kind) => Ok(Selector::of_kind(value::symbol_name(kind))),
                        Value::Map(selector_id) => ts_selector(heap, who, selector_id, 1),
                        other => Err(LispError::wrong_type(
                            heap,
                            who,
                            &format!("a node kind keyword or a selector map for {what}"),
                            other,
                        )),
                    };
                    for (rule_key, rule_value) in heap.map_entries(rule_id) {
                        match keyword_name(heap, "an injection rule key", rule_key)?.as_str() {
                            "content" => {
                                content = Some(rule_selector(heap, ":content", rule_value)?)
                            }
                            "exclude" => {
                                exclude = Some(rule_selector(heap, ":exclude", rule_value)?)
                            }
                            "lang" => fixed = Some(keyword_name(heap, ":lang", rule_value)?),
                            "lang-from" => {
                                from = Some(keyword_name(heap, ":lang-from", rule_value)?)
                            }
                            "aliases" => {
                                let Value::Map(aliases_id) = rule_value else {
                                    return Err(LispError::wrong_type(
                                        heap,
                                        who,
                                        "a map from word to language for :aliases",
                                        rule_value,
                                    ));
                                };
                                for (word, lang) in heap.map_entries(aliases_id) {
                                    let word = match word {
                                        Value::Str(_) => expect_string(heap, who, word)?,
                                        other => {
                                            return Err(LispError::wrong_type(
                                                heap,
                                                who,
                                                "a string word in :aliases",
                                                other,
                                            ));
                                        }
                                    };
                                    aliases.insert(
                                        word,
                                        keyword_name(heap, "an :aliases language", lang)?,
                                    );
                                }
                            }
                            other => {
                                return Err(LispError::runtime(format!(
                                    "{who}: an injection rule takes :content :exclude :lang :lang-from :aliases, not :{other}"
                                )));
                            }
                        }
                    }
                    let Some(content) = content else {
                        return Err(LispError::runtime(format!(
                            "{who}: an injection rule names its :content selector"
                        )));
                    };
                    let language = match (fixed, from) {
                        (Some(lang), None) if aliases.is_empty() => InjectedLanguage::Fixed(lang),
                        (None, Some(kind)) => InjectedLanguage::From { kind, aliases },
                        _ => {
                            return Err(LispError::runtime(format!(
                                "{who}: an injection rule names its language with :lang, or with \
                                 :lang-from (and optional :aliases) — exactly one"
                            )));
                        }
                    };
                    injections.rules.push(InjectionRule {
                        content,
                        exclude,
                        language,
                    });
                }
            }
            "faces" => {
                let Value::Map(faces_id) = entry else {
                    return Err(LispError::wrong_type(
                        heap,
                        who,
                        "a map from language to face table for :faces",
                        entry,
                    ));
                };
                for (lang, table) in heap.map_entries(faces_id) {
                    let lang = keyword_name(heap, "a :faces language", lang)?;
                    injections.tables.insert(lang, ts_table(heap, who, table)?);
                }
            }
            other => {
                return Err(LispError::runtime(format!(
                    "{who}: an injection spec takes :rules and :faces, not :{other}"
                )));
            }
        }
    }
    Ok(Some(injections))
}

/// Handing a held tree to another process (ADR-495) — registered LAST from
/// `builtins::register`, for the intern-table order reason `register_tree_queries` gives.
pub(super) fn register_tree_ownership(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%tree-sitter-tree-give",
        Arity::exact(2),
        Sig::new(vec![ref_ty, pid_ty], bool_ty),
        &["tree", "pid"],
        "Hand the tree handle tree to the LOCAL process pid: from now on it is pid's, freed when pid frees it or dies, where it was the caller's. A handle belongs to the process that opened it (%tree-sitter-tree, %tree-sitter-tree-reparse) and is freed when that process exits, so a task that parses off the loop gives its tree to the process it reports to BEFORE replying — else the tree dies with the task. true when pid holds it now; false when the handle was already gone, or pid has died (the tree dies with it). An error when the caller does not own the handle — only the owner decides where it goes — or pid is on another node. false in a build without feature `treesit`.",
        tree_sitter_tree_give,
    );
    primitives.def(
        "%tree-sitter-owned-trees",
        Arity::exact(0),
        Sig::new(vec![], vec_ty),
        &[],
        "Every tree handle the calling process owns, as a vector of refs by ascending id: those it opened or reparsed and has not freed or given away, and those given to it. What a holder sweeps against the handles it still references, freeing the rest — a handle it lost track of (opened by a turn that was then rolled back) is otherwise held until the process exits (ADR-495). [] in a build without feature `treesit`.",
        tree_sitter_owned_trees,
    );
}

/// `(%tree-sitter-tree-give tree pid)` — `crate::host::treesit::tree_give`.
pub(super) fn tree_sitter_tree_give(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let who = "%tree-sitter-tree-give";
    let handle = ts_handle(heap, who, arg(args, 0))?;
    let to = match arg(args, 1) {
        Value::Pid { node, id } if crate::dist::is_local(node) => id,
        Value::Pid { .. } => {
            return Err(LispError::runtime(format!(
                "{who}: a tree lives in this runtime and cannot be given to a process on \
                 another node"
            )));
        }
        other => return Err(LispError::wrong_type(heap, who, "pid", other)),
    };
    match crate::host::treesit::tree_give(handle, crate::process::self_pid(), to) {
        Ok(held) => Ok(Value::boolean(held)),
        Err(crate::host::treesit::GiveRefused::NotOwner) => Err(LispError::runtime(format!(
            "{who}: this process does not own the tree — only its owner can give it away"
        ))),
    }
}

/// `(%tree-sitter-owned-trees)` — `crate::host::treesit::owned_trees` of the caller.
pub(super) fn tree_sitter_owned_trees(_: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let owned = crate::host::treesit::owned_trees(crate::process::self_pid())
        .into_iter()
        .map(Value::Ref)
        .collect();
    Ok(heap.alloc_vector(owned))
}

// ---- what a loaded grammar contains (ADR-583) ----------------------------------------

/// Asking a grammar for its vocabulary — registered LAST from `builtins::register`, for the
/// intern-table order reason `register_tree_queries` gives.
pub(super) fn register_grammar_info(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    primitives.def(
        "%tree-sitter-grammar-info",
        Arity::exact(1),
        Sig::new(vec![kw], map_ty),
        &["lang"],
        "What the grammar for the language keyword `lang` contains, as {:lang :abi :version :name :source :loaded-via :named :anonymous :fields}: :named and :anonymous the visible node kinds as keyword sets (aliases included; hidden rules and supertypes are never a node's kind), :fields the field names as a keyword set, :abi the grammar's tree-sitter ABI, :version its own \"major.minor.patch\" and :name its name (both nil before ABI 15), :source the library path it was loaded from or :built-in, :loaded-via how it was opened (:beside or :cache — a copy named for its content, so a library rebuilt in place loads as the new build — or :in-place when no copy could be made, and a library rebuilt at that path may then stay the old one until restart; nil when built in). The vocabulary a face table, an indent spec or an injection rule may name — what makes a grammar update that renamed a node detectable (ADR-583). Loads a declared grammar, as a parse would; errors when the runtime has no grammar for `lang`, or in a build without feature `treesit`.",
        tree_sitter_grammar_info,
    );
}

/// `(%tree-sitter-grammar-info lang)` — `crate::host::treesit::grammar_info`.
pub(super) fn tree_sitter_grammar_info(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let lang = ts_lang(heap, "%tree-sitter-grammar-info", arg(args, 0))?;
    crate::host::treesit::grammar_info(heap, &lang)
}
