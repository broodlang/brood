//! `textDocument/completion` (+ `completionItem/resolve`): name candidates at
//! the cursor. Three sources, inner-shadows-outer: **locals** visible at the
//! cursor (from the CST scope walker), the **special forms / core macros** (which
//! aren't in the global table — they're evaluator syntax, so completion would
//! otherwise never offer `if`/`let`/`fn`/`def`…), and the interpreter's
//! **globals** (prelude + builtins). The client does prefix filtering, so we
//! offer the whole visible set.
//!
//! Items ship label + kind only; the signature and docstring are filled in by
//! [`resolve`] when the client asks (`completionItem/resolve`), so building the
//! list stays cheap (no introspection eval per candidate).

use std::collections::HashSet;

use brood::core::value;
use brood::error::Span;
use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::reader;
use brood::syntax::scope::{BindingKind, ScopeTree};
use brood::types::check;
use brood::Interp;
use lsp_types::{
    CompletionItem, CompletionItemKind, Documentation, InsertTextFormat, MarkupContent, MarkupKind,
};

use brood::introspect;

use crate::semantic_tokens::SPECIAL_FORMS;

/// Candidates visible at byte `offset`. `tree` is the document's scope analysis
/// (already built by the caller); `text` is the document source, used to read its
/// namespace + `(:use …)` imports so imported names are offered **bare** (ADR-065
/// §6). The client does prefix filtering, so we offer the whole visible set.
pub fn completions(
    interp: &mut Interp,
    tree: &ScopeTree,
    cst: &Node,
    text: &str,
    offset: u32,
    snippet_support: bool,
) -> Vec<CompletionItem> {
    // Module-name context: inside a `(:use …)`/`(:alias …)` clause the only
    // sensible candidates are loadable modules — offer those alone (a generic
    // `+`/`if`/local would be noise there).
    if in_module_name_position(cst, offset, text) {
        return introspect::loadable_modules(interp)
            .into_iter()
            .map(|m| item(m, CompletionItemKind::MODULE))
            .collect();
    }

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    // Record-field candidates (the ROADMAP "type-directed record-field completion"):
    // at a map-**key** position whose map argument the checker types as a record
    // shape, offer that record's field names as `:keyword` items ahead of the
    // generic sets. Additive — the generic candidates still follow (they don't
    // prefix-match a `:` anyway), and any miss (unknown type, unparseable buffer)
    // degrades to offering nothing extra, never a wrong list.
    if let Some(context) = record_key_context(cst, offset, text) {
        items.extend(record_field_items(interp, text, &context));
    }

    // Inside `(impl Ability …)`, offer the ability's ops first (so the snippet-y
    // METHOD item shadows the generic global of the same name) — you get exactly the
    // ops you must implement, each as a ready-to-fill method skeleton.
    if let Some(proto) = enclosing_impl(cst, offset, text) {
        // Is the method's own `(` already typed (cursor sits inside `(op…`), or are
        // we directly in the impl form? If the innermost list is the impl form
        // itself, the skeleton must supply the wrapping parens.
        let paren_open = innermost_list(cst, offset)
            .and_then(|list| list.forms().next())
            .is_some_and(|head| head.text(text) != "impl");
        for (name, arity) in introspect::protocol_ops(interp, &proto) {
            if seen.insert(name.clone()) {
                let mut it = item(name.clone(), CompletionItemKind::METHOD);
                it.detail = Some(format!(
                    "{} op ({} arg{})",
                    proto,
                    arity,
                    if arity == 1 { "" } else { "s" }
                ));
                it.insert_text = Some(impl_method_skeleton(
                    &name,
                    arity,
                    paren_open,
                    snippet_support,
                ));
                if snippet_support {
                    it.insert_text_format = Some(InsertTextFormat::SNIPPET);
                }
                items.push(it);
            }
        }
    }

    // Locals (and document-level defs) first — they shadow same-named globals.
    // (A namespaced file's own defs are document-level globals here, so they're
    // already offered bare by this path.)
    for b in tree.names_in_scope(offset) {
        if seen.insert(b.name.clone()) {
            items.push(item(
                b.name.clone(),
                match b.kind {
                    BindingKind::Local => CompletionItemKind::VARIABLE,
                    BindingKind::Global => CompletionItemKind::FUNCTION,
                },
            ));
        }
    }
    // Special forms / core macros (evaluator syntax — not in the global table).
    // One shared list with the semantic-token classifier, so they can't drift.
    for &kw in SPECIAL_FORMS {
        if seen.insert(kw.to_string()) {
            items.push(item(kw.to_string(), CompletionItemKind::KEYWORD));
        }
    }
    // `(:use …)`-imported names, offered **bare** with the qualified global stashed
    // in `data` so `resolve` can fetch its signature/doc (a bare import isn't a
    // global under its short name, so it'd otherwise be missing from the list).
    for (bare, qualified) in introspect::file_imports(interp, text) {
        if seen.insert(bare.clone()) {
            let mut it = item(bare, CompletionItemKind::FUNCTION);
            it.data = Some(serde_json::Value::String(qualified));
            items.push(it);
        }
    }
    // Then the interpreter's globals (prelude + builtins + every `mod/name` for
    // explicit qualified completion).
    for name in introspect::global_names(interp) {
        // `%`-prefixed names are the kernel primitives and the prelude's internals
        // (ADR-250 moved 203 of the latter behind the prefix, taking `%` globals from
        // 313 to 521). They are not vocabulary a user should be offered — half the
        // completion list would be plumbing — and `apropos`/`doc-search` filter them
        // for the same reason.
        if name.starts_with('%') {
            continue;
        }
        if seen.insert(name.clone()) {
            items.push(item(name, CompletionItemKind::FUNCTION));
        }
    }
    items
}

/// Fill in an item's signature (`detail`) and docstring (`documentation`) — what
/// `completionItem/resolve` is for. Looked up by label against the interpreter's
/// introspection; a local (or anything with neither) is returned unchanged.
pub fn resolve(interp: &mut Interp, mut item: CompletionItem) -> CompletionItem {
    // A bare imported item carries its qualified global in `data` (its label isn't
    // a global under its short name); everything else looks up by label.
    let lookup = item
        .data
        .as_ref()
        .and_then(|d| d.as_str())
        .unwrap_or(&item.label)
        .to_string();
    let (sig, doc) = introspect::signature(interp, &lookup);
    if let Some(sig) = sig {
        item.detail = Some(sig);
    }
    if let Some(doc) = doc {
        item.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: doc,
        }));
    }
    item
}

fn item(label: String, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label,
        kind: Some(kind),
        ..Default::default()
    }
}

/// The text inserted when an ability op is picked inside `(impl …)`: the method
/// skeleton `(op [self …] body)`, so the user fills in the body rather than
/// retyping the shape. With `snippet` (the client declared `snippetSupport`), the
/// params and body are tabstops (`${1:self}` … `$0`); otherwise a plain skeleton
/// with an empty body (no `$`-syntax a non-snippet client would insert literally).
/// `paren_open` = the method's own `(` is already typed, so the wrapping parens are
/// omitted (the first arg is conventionally `self`; later args are `arg2`, `arg3`).
fn impl_method_skeleton(name: &str, arity: usize, paren_open: bool, snippet: bool) -> String {
    let (open, close) = if paren_open { ("", "") } else { ("(", ")") };
    let param = |i: usize| -> String {
        let bare = if i == 0 {
            "self".to_string()
        } else {
            format!("arg{}", i + 1)
        };
        if snippet {
            format!("${{{}:{}}}", i + 1, bare)
        } else {
            bare
        }
    };
    let params = (0..arity).map(param).collect::<Vec<_>>().join(" ");
    let body = if snippet { "$0" } else { "" };
    format!("{open}{name} [{params}] {body}{close}")
}

/// If byte `offset` falls inside an `(impl Ability …)` form, the ability name
/// `Ability`. Walks the CST for the innermost enclosing `impl` list (they don't
/// nest, so the first found while descending is it).
fn enclosing_impl(node: &Node, offset: u32, src: &str) -> Option<String> {
    // Inclusive at the end: while typing, the cursor sits *after* the last char —
    // `offset == span.end` of the still-unclosed `(impl …` — and we want to count
    // as inside it (`Span::contains` is end-exclusive).
    if offset < node.span.start || offset > node.span.end {
        return None;
    }
    for child in &node.children {
        if let Some(p) = enclosing_impl(child, offset, src) {
            return Some(p);
        }
    }
    if node.kind == NodeKind::List {
        let mut forms = node.forms();
        if forms.next().map(|n| n.text(src)) == Some("impl") {
            return forms.next().map(|n| n.text(src).to_string());
        }
    }
    None
}

/// True when byte `offset` sits where a **module name** belongs: the module slot
/// of a `(:use …)`/`(:alias …)` clause (after the keyword, before any `:only`/`:as`
/// marker). End-inclusive, so a cursor typing at the end of a still-open form
/// counts as inside it.
fn in_module_name_position(node: &Node, offset: u32, src: &str) -> bool {
    let Some(list) = innermost_list(node, offset) else {
        return false;
    };
    let mut forms = list.forms();
    let Some(head) = forms.next() else {
        return false;
    };
    // `(:use mod …)` / `(:alias mod …)` — after the keyword, before a later
    // `:only`/`:as`/`:refer` marker (so completing inside `:only [..]` doesn't
    // offer modules).
    if head.kind == NodeKind::Keyword && matches!(head.text(src), ":use" | ":alias") {
        offset > head.span.end
            && !list
                .forms()
                .skip(1)
                .any(|f| f.kind == NodeKind::Keyword && f.span.end <= offset)
    } else {
        false
    }
}

/// A cursor position where a **record field keyword** belongs: the key slot of a
/// `(get M …)` / `(assoc M …)` / `(update M …)` / `(dissoc M …)` / `(contains? M …)`
/// call, or the head of a keyword-accessor call `(:key M)`. Carries what the
/// checker query needs: the call form's opening byte offset (the reader records a
/// list's `Pos` there), the item index of the map expression, and the span of the
/// partially-typed key token — blanked before reading, since a lone `:` doesn't
/// parse and the key's spelling is irrelevant to the map argument's type.
struct RecordKeyContext {
    call_open: u32,
    map_arg_index: usize,
    key_token: Option<Span>,
}

fn record_key_context(node: &Node, offset: u32, src: &str) -> Option<RecordKeyContext> {
    let list = innermost_list(node, offset)?;
    let forms: Vec<&Node> = list.forms().collect();
    let head = forms.first()?;
    // The argument slot the cursor occupies: the first form the cursor hasn't
    // passed the end of (typing inside / at the end of it — end-inclusive, like
    // `enclosing_impl`), or one past the last form when typing a fresh one.
    let slot = forms
        .iter()
        .position(|f| offset <= f.span.end)
        .unwrap_or(forms.len());
    let (key_position, map_arg_index) = match head.kind {
        NodeKind::Symbol => {
            let at_key = match head.text(src) {
                "get" | "update" | "contains?" => slot == 2,
                // `(assoc m k v k v …)`: keys sit at the even slots.
                "assoc" => slot >= 2 && slot % 2 == 0,
                "dissoc" => slot >= 2,
                _ => false,
            };
            (at_key, 1)
        }
        // `(:key m)` — completing the accessor keyword itself; the map must
        // already be there to have a type. The head gets blanked below, so in
        // the read form the map lands at item 0.
        NodeKind::Keyword => (slot == 0 && forms.len() >= 2, 0),
        _ => (false, 1),
    };
    if !key_position {
        return None;
    }
    // Only offer while the key slot is still keyword-shaped: empty (about to
    // type), a keyword token, an in-progress scrap the CST couldn't classify,
    // or the lone `:` just typed (which classifies as a *symbol* — atom.rs:
    // "a bare `:` is a symbol, not an empty keyword"). Any other symbol is a
    // *computed* key (`(get p k)`) — the generic candidate paths' job — and any
    // other complete form isn't a field key.
    let key_token = match forms.get(slot) {
        None => None,
        Some(n)
            if matches!(n.kind, NodeKind::Keyword | NodeKind::Error)
                || (n.kind == NodeKind::Symbol && n.text(src).starts_with(':')) =>
        {
            Some(n.span)
        }
        Some(_) => return None,
    };
    Some(RecordKeyContext {
        call_open: list.span.start,
        map_arg_index,
        key_token,
    })
}

/// The record-field completion items for a detected key position: parse the
/// (blanked + balanced) buffer with the strict reader, ask the checker for the
/// map argument's inferred type at the call site, and turn a record shape's
/// fields into `:keyword` items. Empty on any miss.
///
/// Wrapped in a heap checkpoint like `typecheck_diagnostics` — the parsed forms
/// are LOCAL and reclaimed after the query, so completion doesn't grow the
/// interpreter's heap per keystroke.
fn record_field_items(
    interp: &mut Interp,
    text: &str,
    context: &RecordKeyContext,
) -> Vec<CompletionItem> {
    // Blank the partial key token byte-for-byte with spaces, so every other
    // offset in the buffer — including the call's opening paren — stays put.
    let mut source = text.to_string();
    if let Some(span) = context.key_token {
        source.replace_range(
            span.start as usize..span.end as usize,
            &" ".repeat((span.end - span.start) as usize),
        );
    }
    close_open_delimiters(&mut source);
    // The call form's opening paren in 1-based reader coordinates (columns count
    // *chars* within the line, matching the scanner's `pos_at`).
    let opening = context.call_open as usize;
    let line = source[..opening].bytes().filter(|&b| b == b'\n').count() as u32 + 1;
    let line_start = source[..opening].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = source[line_start..opening].chars().count() as u32 + 1;

    let checkpoint = interp.heap.checkpoint();
    let mut items = Vec::new();
    if let Ok(positioned) = reader::read_all_positioned(&mut interp.heap, &source) {
        let forms: Vec<_> = positioned.into_iter().map(|(f, _)| f).collect();
        let ty = check::arg_ty_at(&mut interp.heap, &forms, line, col, context.map_arg_index);
        if let Some(fields) = ty.as_ref().and_then(|t| t.record_fields()) {
            for (&sym, (field_ty, _required)) in fields {
                let name = value::symbol_name(sym);
                if name == "__id__" {
                    continue; // the record's internal identity tag, not a user field
                }
                let mut it = item(format!(":{name}"), CompletionItemKind::FIELD);
                it.detail = Some(field_ty.to_string());
                items.push(it);
            }
        }
    }
    interp.heap.reset_local_to(checkpoint);
    items
}

/// Append the closers a mid-edit buffer is missing (`(get p :` → `(get p :)`),
/// so the strict reader can parse what the tolerant CST already navigates.
/// Tracks strings (with escapes) and `;` line comments so a delimiter inside
/// either doesn't count; an unterminated string is closed first, and a trailing
/// line comment gets a newline so the closers don't land inside it. A stray
/// close delimiter just pops whatever is open — the read will fail and the
/// caller degrades to no candidates.
fn close_open_delimiters(source: &mut String) {
    #[derive(PartialEq)]
    enum State {
        Code,
        Str,
        StrEscape,
        Comment,
    }
    let mut state = State::Code;
    let mut open: Vec<char> = Vec::new();
    for c in source.chars() {
        state = match state {
            State::Str => match c {
                '\\' => State::StrEscape,
                '"' => State::Code,
                _ => State::Str,
            },
            State::StrEscape => State::Str,
            State::Comment => {
                if c == '\n' {
                    State::Code
                } else {
                    State::Comment
                }
            }
            State::Code => match c {
                '"' => State::Str,
                ';' => State::Comment,
                '(' => {
                    open.push(')');
                    State::Code
                }
                '[' => {
                    open.push(']');
                    State::Code
                }
                '{' => {
                    open.push('}');
                    State::Code
                }
                ')' | ']' | '}' => {
                    open.pop();
                    State::Code
                }
                _ => State::Code,
            },
        };
    }
    match state {
        State::Str | State::StrEscape => source.push('"'),
        State::Comment => source.push('\n'),
        State::Code => {}
    }
    while let Some(close) = open.pop() {
        source.push(close);
    }
}

/// The innermost `List` whose span contains `offset` (end-inclusive), or `None`.
fn innermost_list(node: &Node, offset: u32) -> Option<&Node> {
    if offset < node.span.start || offset > node.span.end {
        return None;
    }
    for child in &node.children {
        if let Some(inner) = innermost_list(child, offset) {
            return Some(inner);
        }
    }
    (node.kind == NodeKind::List).then_some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    fn labels_at(src: &str, needle: &str) -> Vec<String> {
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.find(needle).unwrap() as u32;
        completions(&mut interp, &tree, &root, src, at, true)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    #[test]
    fn offers_locals_keywords_and_globals() {
        let labels = labels_at("(defn f (x) (+ x 1))", "x 1");
        assert!(labels.contains(&"x".to_string()), "local missing");
        assert!(labels.contains(&"f".to_string()), "doc def missing");
        assert!(labels.contains(&"+".to_string()), "global missing");
        assert!(labels.contains(&"let".to_string()), "special form missing");
    }

    #[test]
    fn a_local_appears_once_even_if_it_shadows() {
        let labels = labels_at("(defn map2 (map) map)", "map)");
        assert_eq!(
            labels.iter().filter(|l| *l == "map").count(),
            1,
            "shadowing local should be de-duped: {labels:?}"
        );
    }

    #[test]
    fn offers_use_imported_names_bare() {
        // In a `(:use set)` file, `union` (a `set` export) is offered **bare**,
        // carrying its qualified target in `data` for resolve.
        let mut interp = Interp::new();
        let src = "(defmodule app (:use set))\n(uni";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.rfind("uni").unwrap() as u32;
        let items = completions(&mut interp, &tree, &root, src, at, true);
        let union = items
            .iter()
            .find(|i| i.label == "union")
            .expect("bare `union` offered");
        assert_eq!(
            union.data.as_ref().and_then(|d| d.as_str()),
            Some("set/union"),
            "data should carry the qualified target"
        );
        // and resolve uses that data to fetch the real signature.
        let r = resolve(&mut interp, union.clone());
        assert!(
            r.detail.unwrap_or_default().contains("union"),
            "resolved signature"
        );
    }

    #[test]
    fn resolve_attaches_a_signature_and_doc_for_a_global() {
        let mut interp = Interp::new();
        let resolved = resolve(
            &mut interp,
            item("map".into(), CompletionItemKind::FUNCTION),
        );
        assert!(resolved.detail.unwrap().contains("(map "), "signature");
        assert!(resolved.documentation.is_some(), "doc");
    }

    #[test]
    fn module_position_detection() {
        // :use / :alias slot → module position; the keyword itself and an :only
        // operand → not.
        let chk = |src: &str, needle: &str| {
            let root = cst::parse(src);
            let at = src.find(needle).unwrap() as u32;
            in_module_name_position(&root, at, src)
        };
        assert!(chk("(defmodule a (:use foo))", "foo"));
        assert!(chk("(defmodule a (:alias foo))", "foo"));
        assert!(!chk("(defmodule a (:use foo))", ":use")); // on the keyword
        assert!(!chk("(defmodule a (:use foo :only [bar]))", "bar")); // past :only
        assert!(!chk("(+ 1 2)", "+")); // ordinary call
    }

    // This is the only test that reads the *default* `*load-path*`, which made it the
    // canary for KI-12: a frozen-prelude handle bug left that path holding an
    // unrelated object, so no module was ever found on it. Fixed 2026-07-26
    // (`Heap::localize_for_freeze`); keep this test as the regression guard.
    #[test]
    fn completes_module_names_in_use() {
        // A module on the load-path is offered inside `(:use …)`, and the generic
        // globals are suppressed there.
        let dir = std::env::temp_dir().join(format!("brood_modcomp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("greeter.blsp"), "(defmodule greeter)\n").unwrap();
        let mut interp = Interp::new();
        interp
            .eval_str(&format!(
                "(def *load-path* (cons \"{}\" *load-path*))",
                dir.display()
            ))
            .unwrap();

        // One case since ADR-229 removed the `require`-form cases this loop also covered
        // (clippy rejects a loop over a single element under -D warnings).
        let src = "(defmodule app (:use ";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.len() as u32;
        let labels: Vec<String> = completions(&mut interp, &tree, &root, src, at, true)
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert!(
            labels.contains(&"greeter".to_string()),
            "module missing in {src:?}: {labels:?}"
        );
        assert!(
            !labels.contains(&"+".to_string()),
            "generic global leaked into {src:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn offers_ability_ops_inside_impl() {
        // Seed the ability registry directly (defability isn't loaded in a bare interp).
        let mut interp = Interp::new();
        interp
            .eval_str("(def *abilities* (assoc {} 'Encode (list (list 'encode '[v]))))")
            .unwrap();
        let src = "(impl Encode :int (enc";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.len() as u32; // cursor at end, inside the method form
        let items = completions(&mut interp, &tree, &root, src, at, true);
        let enc = items
            .iter()
            .find(|i| i.label == "encode")
            .expect("op `encode` offered inside (impl Encode …)");
        assert_eq!(
            enc.kind,
            Some(CompletionItemKind::METHOD),
            "tagged as an ability op"
        );
        assert!(
            enc.detail.as_deref().unwrap_or("").contains("Encode op"),
            "{:?}",
            enc.detail
        );
        // A fillable method skeleton is inserted: params + a body tabstop. The
        // cursor sits inside the already-typed `(`, so no wrapping parens.
        assert_eq!(enc.insert_text.as_deref(), Some("encode [${1:self}] $0"));
        assert_eq!(enc.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    /// Full-path harness for the record-field candidates: completions at the end
    /// of `src`, filtered to the FIELD-kind items.
    fn field_labels_at_end(src: &str) -> Vec<String> {
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let items = completions(&mut interp, &tree, &root, src, src.len() as u32, true);
        items
            .into_iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
            .map(|i| i.label)
            .collect()
    }

    #[test]
    fn offers_record_fields_for_a_direct_ctor_argument() {
        // Mid-edit buffer: unclosed call, lone `:` — the roadmap's motivating case.
        let labels = field_labels_at_end("(defrecord point (x y))\n(get (point 1 2) :");
        assert!(labels.contains(&":x".to_string()), "{labels:?}");
        assert!(labels.contains(&":y".to_string()), "{labels:?}");
        assert!(
            !labels.iter().any(|l| l.contains("__id__")),
            "the identity tag is not a user field: {labels:?}"
        );
    }

    #[test]
    fn offers_record_fields_for_a_let_bound_record() {
        // `p` is a bare symbol — only the checker's scope walk knows its type.
        let labels = field_labels_at_end(
            "(defrecord point (x y))\n(defn f () (let (p (point 1 2)) (assoc p :",
        );
        assert!(labels.contains(&":x".to_string()), "{labels:?}");
    }

    #[test]
    fn record_fields_carry_their_declared_type() {
        let mut interp = Interp::new();
        let src = "(defrecord point ((x int) (y int)))\n(get (point 1 2) :";
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let items = completions(&mut interp, &tree, &root, src, src.len() as u32, true);
        let x = items
            .iter()
            .find(|i| i.label == ":x")
            .expect("field offered");
        assert_eq!(x.detail.as_deref(), Some("int"));
    }

    #[test]
    fn offers_record_fields_for_the_keyword_accessor_head() {
        // `(:x p)` — completing the accessor keyword itself.
        let src = "(defrecord point (x y))\n(defn f () (let (p (point 1 2)) (:x p)))";
        let mut interp = Interp::new();
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let at = src.rfind(":x").unwrap() as u32 + 2; // cursor right after `:x`
        let labels: Vec<String> = completions(&mut interp, &tree, &root, src, at, true)
            .into_iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
            .map(|i| i.label)
            .collect();
        assert!(labels.contains(&":y".to_string()), "{labels:?}");
    }

    #[test]
    fn record_fields_stay_out_of_the_wrong_slots() {
        // A value position, a computed (symbol) key, an untyped map argument, and
        // a plain non-map call each offer no field items.
        for src in [
            "(defrecord point (x y))\n(assoc (point 1 2) :x ", // value slot
            "(defrecord point (x y))\n(get (point 1 2) k",     // computed key
            "(defn f (p) (get p :",                            // untyped argument
            "(defrecord point (x y))\n(+ 1 ",                  // not a map op
        ] {
            let labels = field_labels_at_end(src);
            assert!(labels.is_empty(), "{src:?} leaked {labels:?}");
        }
    }

    #[test]
    fn close_open_delimiters_tracks_strings_and_comments() {
        let case = |src: &str, want: &str| {
            let mut s = src.to_string();
            close_open_delimiters(&mut s);
            assert_eq!(s, want, "for {src:?}");
        };
        case("(get p :", "(get p :)");
        case("(f [1 {", "(f [1 {}])");
        case("(f \"a ( b", "(f \"a ( b\")");
        case("(f \"a \\\" (", "(f \"a \\\" (\")");
        case("(f ; comment (", "(f ; comment (\n)");
        case("(f)", "(f)");
        case("(f))", "(f))"); // stray close: left for the reader to reject
    }

    #[test]
    fn impl_method_skeleton_covers_parens_and_snippet_modes() {
        // snippet + method `(` already open → tabstops, no wrapping parens.
        assert_eq!(
            impl_method_skeleton("area", 1, true, true),
            "area [${1:self}] $0"
        );
        // snippet + directly in the impl form → supply the wrapping parens.
        assert_eq!(
            impl_method_skeleton("area", 1, false, true),
            "(area [${1:self}] $0)"
        );
        // no snippet support → a plain skeleton, empty body, never literal `$`.
        assert_eq!(
            impl_method_skeleton("cmp", 2, false, false),
            "(cmp [self arg2] )"
        );
        assert_eq!(
            impl_method_skeleton("cmp", 2, true, false),
            "cmp [self arg2] "
        );
    }
}
