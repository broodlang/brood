//! `textDocument/codeAction` — quick-fixes off the diagnostics we already
//! publish.
//!
//! Off an `unbound symbol: foo` finding we offer these fixes:
//! - **"did you mean?"** — replace `foo` with the closest known name (a global,
//!   special form, or in-scope local within a small edit distance). Preferred.
//! - **Auto-import** — when a *bare* `foo` is exported as `mod/foo` by some
//!   module: **"Import `foo` from `mod`"** adds a `(:use mod)` clause to the
//!   `defmodule` header (the modern, statically-analyzable way — the editor
//!   writes the explicit import for you, no runtime autoload), and **"Qualify as
//!   `mod/foo`"** rewrites the reference in place. One pair per providing module,
//!   so a name two modules export (`sexp`/`editor/treesit`) offers a choice
//!   instead of guessing.
//! - **"Create function `foo`"** — when `foo` is a call head: insert a stub
//!   `(defn foo (a b …) nil)` with arity matched to the call site (the TDD case).
//!
//! The unbound-finding's range already narrows to the offending token (see
//! `refine_diagnostic_range` in `main.rs`), so a replace edits exactly that span.
//!
//! Pure name/CST analysis: candidates and provider discovery come from the
//! introspection surface (`global_names` / `module_file`) + the CST scope walker,
//! never from running the buffer. `global_names` reads the LSP's live image, which
//! `project/setup-tooling-image` has loaded every project source into (ADR-031),
//! so a bare name's providers are found workspace-wide — embedded std *and*
//! project modules alike — without a separate cross-file scan.

use std::collections::HashMap;

use brood::error::Span;
use brood::introspect;
use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::scope::ScopeTree;
use brood::Interp;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, Range, TextEdit, Uri,
    WorkspaceEdit,
};

use crate::line_index::LineIndex;
use crate::semantic_tokens::SPECIAL_FORMS;

/// The prefix the advisory checker uses for an unbound-name finding. We key off
/// it to recover the offending name (the diagnostic range already points at the
/// token, so we don't re-scan the source).
const UNBOUND_PREFIX: &str = "unbound symbol: ";

/// Build the quick-fixes for the diagnostics in `context_diagnostics` (the
/// subset the client says overlap the requested range). Only unbound-symbol
/// findings produce actions today.
pub fn code_actions(
    interp: &mut Interp,
    uri: &Uri,
    root: &Node,
    src: &str,
    scope: &ScopeTree,
    line_index: &LineIndex,
    offset_of: impl Fn(Range) -> u32,
    context_diagnostics: &[Diagnostic],
    at: u32,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    // Not diagnostic-driven: offered wherever the cursor is inside a `defn` the
    // checker has inferred a signature for and the author hasn't declared one.
    if let Some(action) = declare_sig_action(interp, uri, root, src, line_index, at) {
        actions.push(action);
    }
    for diag in context_diagnostics {
        let Some(rest) = diag.message.strip_prefix(UNBOUND_PREFIX) else {
            continue;
        };
        // The bare identifier: the message may carry a trailing " — hint" for a
        // foreign-construct name, which isn't part of the symbol.
        let name = rest.split_whitespace().next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let offset = offset_of(diag.range);
        let suggestions = suggestions(interp, scope, offset, name);
        for suggestion in &suggestions {
            actions.push(did_you_mean(uri, diag, suggestion));
        }
        // Auto-import: a *bare* name that some module exports as `mod/name` — offer
        // to add `(:use mod)` and/or qualify. Preferred only when there's exactly
        // one provider and no closer typo fix (never two preferred per diagnostic).
        if !name.contains('/') {
            let providers = import_providers(interp, root, src, name);
            let import_preferred = suggestions.is_empty() && providers.len() == 1;
            for module in &providers {
                actions.extend(add_use_actions(
                    uri,
                    root,
                    src,
                    line_index,
                    diag,
                    name,
                    module,
                    import_preferred,
                ));
            }
        }
        // Create a stub `defn` for an unbound name used as a call head.
        if let Some(action) = create_defn_action(uri, root, src, line_index, diag, offset, name) {
            actions.push(action);
        }
    }
    actions
}

/// **"Declare this signature"** — write the checker's inferred signature for the
/// `defn` under the cursor as a real `(sig …)` line above it.
///
/// The inlay hint (`inlay_hints.rs`) shows the same signature; this is the half that
/// makes it durable. Once written the sig is *authoritative* — read ahead of inference
/// at every call site, validated against the definition (ADR-259), and read by the
/// reversed-args gate — so the action turns a passive display into the adoption path
/// for a language whose std carries 34 declarations over 2828 definitions.
///
/// Declines rather than guesses, in three cases: a function that already has a `(sig
/// …)` (nothing to add), one the checker inferred nothing useful about (`(any…) ->
/// any` is not worth writing down), and one whose type cannot be written faithfully in
/// the grammar (`Ty::to_source` returns `None` — a quick-fix that writes a *different*
/// type than the one it showed would be worse than none).
fn declare_sig_action(
    interp: &mut Interp,
    uri: &Uri,
    root: &Node,
    src: &str,
    line_index: &LineIndex,
    at: u32,
) -> Option<CodeActionOrCommand> {
    let defn = innermost_defn(root, src, at)?;
    let name = defn.forms().nth(1)?.text(src).to_string();
    let sig = buffer_signature(interp, src, &name)?;
    if sig.declared {
        return None;
    }
    let uninformative = sig.sig.params.iter().all(|p| p.is_any()) && sig.sig.ret.is_any();
    if uninformative {
        return None;
    }
    let rendered = sig.sig.to_source()?;
    // Insert on its own line, indented like the `defn` it describes.
    let line_start = line_index.position(src, defn.span.start);
    let indent: String = src[line_index.offset(src, lsp_types::Position::new(line_start.line, 0))
        as usize..defn.span.start as usize]
        .chars()
        .take_while(|c| c.is_whitespace() && *c != '\n')
        .collect();
    let insert = Range::new(
        lsp_types::Position::new(line_start.line, 0),
        lsp_types::Position::new(line_start.line, 0),
    );
    let edit = TextEdit {
        range: insert,
        new_text: format!("{indent}(sig {name} {rendered})\n"),
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Declare signature: (sig {name} {rendered})"),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// The innermost `(defn …)` / `(defn- …)` list containing byte offset `at`.
fn innermost_defn<'a>(root: &'a Node, src: &str, at: u32) -> Option<&'a Node> {
    let mut found: Option<&Node> = None;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if at < node.span.start || at > node.span.end {
            continue;
        }
        if node.kind == NodeKind::List {
            if let Some(head) = node.forms().next() {
                let name = head.text(src);
                if name == "defn" || name == "defn-" {
                    found = Some(node);
                }
            }
        }
        stack.extend(node.children.iter());
    }
    found
}

/// The checker's effective signature for `name` in this buffer — the same call the
/// inlay hints make, so the action and the hint can never disagree.
fn buffer_signature(
    interp: &mut Interp,
    src: &str,
    name: &str,
) -> Option<brood::types::check::FnSignature> {
    let cp = interp.heap.checkpoint();
    let forms = brood::syntax::reader::read_all(&mut interp.heap, src).ok();
    let found = forms.and_then(|forms| {
        brood::types::check::file_signatures(&mut interp.heap, &forms)
            .into_iter()
            .find(|s| s.name == name)
    });
    interp.heap.reset_local_to(cp);
    found
}

/// Modules that export a **public** `name` as `mod/name`, for the auto-import
/// menu. Read from the live image's global table (`global_names`) — the LSP has
/// every project source loaded (ADR-031), so this finds providers workspace-wide,
/// embedded std and project modules alike. Filtered so the offer is never wrong:
/// the file's **own** namespace is dropped (you don't import your own module), any
/// module the header **already imports** is dropped (it wouldn't be unbound), and a
/// **private** provider (a `defn-`/`def-` name, judged by `is_private`, ADR-146) is
/// dropped — `(:use mod)` never refers it, so it would still be unbound. Sorted +
/// deduped for a stable menu.
fn import_providers(interp: &mut Interp, root: &Node, src: &str, name: &str) -> Vec<String> {
    let own = header_ns(root, src);
    let imported = header_imported_modules(root, src);
    let suffix = format!("/{name}");
    let names = introspect::global_names(interp);
    let mut mods: Vec<String> = names
        .into_iter()
        // A private target can't be referred bare by `(:use)`, so never suggest it.
        .filter(|g| !interp.heap.is_private(brood::core::value::intern(g)))
        .filter_map(|g| g.strip_suffix(&suffix).map(str::to_string))
        // A non-empty module (not a leading `/name` root escape). The `--` module-path
        // guard is the separate private-*module* heuristic, kept as in `eval/mod.rs`.
        .filter(|m| !m.is_empty() && !m.contains("--"))
        .filter(|m| own.as_deref() != Some(m.as_str()))
        .filter(|m| !imported.iter().any(|i| i == m))
        .collect();
    mods.sort();
    mods.dedup();
    mods
}

/// The auto-import fixes for one provider `module` of a bare `name`:
/// - **"Import `name` from `(:use module)`"** — add a `(:use module)` clause to
///   the `defmodule` header (offered only when there *is* a header to edit).
/// - **"Qualify as `module/name`"** — rewrite the reference in place (always;
///   the only option in a header-less file).
///
/// `import_preferred` marks the import as the one-keystroke default — set by the
/// caller only when this is the sole provider and no closer typo fix competes.
fn add_use_actions(
    uri: &Uri,
    root: &Node,
    src: &str,
    line_index: &LineIndex,
    diag: &Diagnostic,
    name: &str,
    module: &str,
    import_preferred: bool,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    if let Some((at, text)) = insert_use_clause(root, src, module) {
        let range = line_index.range(src, Span { start: at, end: at });
        actions.push(quickfix(
            uri,
            format!("Import `{name}` from `(:use {module})`"),
            range,
            text,
            Some(diag),
            import_preferred,
        ));
    }
    actions.push(quickfix(
        uri,
        format!("Qualify as `{module}/{name}`"),
        diag.range,
        format!("{module}/{name}"),
        Some(diag),
        false,
    ));
    actions
}

/// Where and what to insert to add `(:use module)` to the leading `(defmodule …)`
/// header, or `None` when there's no header to edit. A new clause is grouped after
/// the **last existing** `(:use …)`/`(:alias …)`/… clause, on its own line at that
/// clause's indentation; a header with no clauses gets the use inline just before
/// its closing paren. The layout mirrors the header's own shape, so the edit reads
/// like hand-written code either way.
fn insert_use_clause(root: &Node, src: &str, module: &str) -> Option<(u32, String)> {
    let first = root.forms().next()?;
    if !is_head(first, src, "defmodule") {
        return None;
    }
    let last_clause = first
        .forms()
        .filter(|c| c.kind == NodeKind::List && clause_keyword(c, src).is_some())
        .last();
    if let Some(clause) = last_clause {
        let indent = line_indent(src, clause.span.start);
        return Some((clause.span.end, format!("\n{indent}(:use {module})")));
    }
    // No clauses: `(defmodule name)` / `(defmodule name "doc")` → insert inline
    // before the closing paren (span.end is one past `)`).
    let close = first.span.end.saturating_sub(1);
    Some((close, format!(" (:use {module})")))
}

/// The namespace a leading `(defmodule ns …)` declares, or `None` (no header, or a
/// non-symbol name).
fn header_ns(root: &Node, src: &str) -> Option<String> {
    let first = root.forms().next()?;
    if !is_head(first, src, "defmodule") {
        return None;
    }
    let mut forms = first.forms();
    forms.next()?; // `defmodule`
    let name = forms.next()?;
    (name.kind == NodeKind::Symbol).then(|| name.text(src).to_string())
}

/// Module names the header already imports via `(:use m)` / `(:alias m …)` /
/// `(:use-internals m)`, so an already-imported module is never re-offered.
fn header_imported_modules(root: &Node, src: &str) -> Vec<String> {
    let mut mods = Vec::new();
    let Some(first) = root.forms().next() else {
        return mods;
    };
    if !is_head(first, src, "defmodule") {
        return mods;
    }
    for clause in first.forms() {
        if clause.kind != NodeKind::List {
            continue;
        }
        let Some(kw) = clause_keyword(clause, src) else {
            continue;
        };
        if matches!(kw, ":use" | ":alias" | ":use-internals") {
            let mut it = clause.forms();
            it.next(); // the keyword
            if let Some(m) = it.next() {
                if m.kind == NodeKind::Symbol {
                    mods.push(m.text(src).to_string());
                }
            }
        }
    }
    mods
}

/// The keyword text (`:use`, …) of a header clause `(:kw …)`, or `None` when the
/// list isn't keyword-led.
fn clause_keyword<'s>(node: &Node, src: &'s str) -> Option<&'s str> {
    let head = node.forms().next()?;
    (head.kind == NodeKind::Keyword).then(|| head.text(src))
}

/// The leading whitespace (indentation) of the line containing byte `offset`.
fn line_indent(src: &str, offset: u32) -> String {
    let offset = offset as usize;
    let line_start = src[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    src[line_start..offset]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// "Create function `name`" for an unbound symbol used as a **call head** — the
/// TDD "call it before you write it" case. Inserts a stub `(defn name (a b …) nil)`
/// at the end of the file, its parameter count matched to the call site's argument
/// count. `None` when `name` isn't a call head (an operand reference — a stub fn
/// would be the wrong fix) or is qualified (a `mod/x` name isn't a stub candidate).
/// Non-preferred.
fn create_defn_action(
    uri: &Uri,
    root: &Node,
    src: &str,
    line_index: &LineIndex,
    diag: &Diagnostic,
    offset: u32,
    name: &str,
) -> Option<CodeActionOrCommand> {
    if name.contains('/') {
        return None; // a qualified name isn't a create-defn candidate
    }
    let argc = call_head_argc(root, offset)?;
    let params = (0..argc).map(param_name).collect::<Vec<_>>().join(" ");
    // Leading blank line separates the stub from existing code; a `(do …)`-free
    // top level means appending at EOF is always valid.
    let stub = format!("\n(defn {name} ({params}) nil)\n");
    let end = src.len() as u32;
    let range = line_index.range(src, Span { start: end, end });
    Some(quickfix(
        uri,
        format!("Create function `{name}`"),
        range,
        stub,
        Some(diag),
        false,
    ))
}

/// If the unbound name at `offset` is the head of a call `(name a b …)`, the number
/// of arguments (so a created `defn` matches the call's arity); `None` when it's
/// not in head position. Walks the chain of nodes containing `offset` and finds the
/// innermost `List` whose first form *is* the symbol under the cursor.
fn call_head_argc(root: &Node, offset: u32) -> Option<usize> {
    let mut chain = Vec::new();
    chain_to(root, offset, &mut chain);
    for node in chain.iter().rev() {
        if node.kind != NodeKind::List {
            continue;
        }
        let mut forms = node.forms();
        if let Some(head) = forms.next() {
            if head.span.contains(offset) {
                return Some(forms.count());
            }
        }
    }
    None
}

/// The chain of nodes from `root` down to the innermost one containing `offset`.
fn chain_to<'a>(node: &'a Node, offset: u32, out: &mut Vec<&'a Node>) {
    out.push(node);
    for child in &node.children {
        if child.span.start <= offset && offset < child.span.end {
            chain_to(child, offset, out);
            break; // children don't overlap — at most one contains the offset
        }
    }
}

/// True when `node` is a `List` whose head symbol is `name` (`defmodule`, …).
fn is_head(node: &Node, src: &str, name: &str) -> bool {
    node.kind == NodeKind::List
        && node
            .forms()
            .next()
            .is_some_and(|h| h.kind == NodeKind::Symbol && h.text(src) == name)
}

/// The i-th generated parameter name: `a`, `b`, … `z`, then `a26`, `a27`, … so
/// large arities stay distinct and valid.
fn param_name(i: usize) -> String {
    if i < 26 {
        ((b'a' + i as u8) as char).to_string()
    } else {
        format!("a{i}")
    }
}

/// A `QUICKFIX` code action applying a single text edit on `uri`. `diag` attaches
/// the diagnostic this resolves (so the editor associates the lightbulb); set
/// `preferred` for the obvious one-keystroke fix (at most one per diagnostic).
fn quickfix(
    uri: &Uri,
    title: String,
    range: Range,
    new_text: String,
    diag: Option<&Diagnostic>,
    preferred: bool,
) -> CodeActionOrCommand {
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![TextEdit { range, new_text }]);
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: diag.map(|d| vec![d.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        is_preferred: preferred.then_some(true),
        ..Default::default()
    })
}

/// One "Replace with `X`" quick-fix targeting the diagnostic's range — preferred,
/// so a single keystroke applies the top suggestion.
fn did_you_mean(uri: &Uri, diag: &Diagnostic, suggestion: &str) -> CodeActionOrCommand {
    quickfix(
        uri,
        format!("Replace with `{suggestion}`"),
        diag.range,
        suggestion.to_string(),
        Some(diag),
        true,
    )
}

/// Up to three known names closest to `name` by edit distance, nearest first.
/// Candidates: locals in scope here, the special forms, and every global. A
/// candidate qualifies only within a length-relative threshold, so an unrelated
/// short name (`x` for `frobnicate`) isn't offered.
fn suggestions(interp: &mut Interp, scope: &ScopeTree, offset: u32, name: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    candidates.extend(scope.names_in_scope(offset).iter().map(|b| b.name.clone()));
    candidates.extend(SPECIAL_FORMS.iter().map(|s| s.to_string()));
    candidates.extend(introspect::global_names(interp));

    // Threshold scales with the name's length: a 1-char typo on a short name, up
    // to ~1/3 of a long one. Distinct, sorted by closeness then alphabetically.
    let max_dist = (name.chars().count() / 3).max(1);
    let mut scored: Vec<(usize, String)> = candidates
        .into_iter()
        .filter(|c| c != name)
        .filter_map(|c| {
            let d = levenshtein(name, &c);
            (d <= max_dist).then_some((d, c))
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(3).map(|(_, c)| c).collect()
}

/// Classic O(m·n) Levenshtein edit distance over Unicode scalar values.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The titles a code-action request at `at` (a byte offset) offers, with no
    /// diagnostics in context — the cursor-driven path.
    fn actions_at(src: &str, at: u32) -> Vec<String> {
        let mut interp = Interp::new();
        let root = brood::syntax::cst::parse(src);
        let scope = brood::syntax::scope::analyze(&root, src);
        let li = LineIndex::new(src);
        let offset_of = |r: Range| li.offset(src, r.start);
        code_actions(
            &mut interp,
            &uri(),
            &root,
            src,
            &scope,
            &li,
            offset_of,
            &[],
            at,
        )
        .into_iter()
        .map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) => ca.title,
            CodeActionOrCommand::Command(c) => c.title,
        })
        .collect()
    }

    #[test]
    fn declares_the_inferred_signature_under_the_cursor() {
        let src = "(defn f (s) (string/length s))";
        let titles = actions_at(src, 8);
        assert!(
            titles
                .iter()
                .any(|t| t.starts_with("Declare signature: (sig f (") && t.contains("string")),
            "got: {titles:?}"
        );
    }

    #[test]
    fn declines_when_a_signature_is_already_declared() {
        let src = "(sig f (int -> string))\n(defn f (n) \"x\")";
        let at = src.find("(defn").unwrap() as u32 + 2;
        assert!(
            actions_at(src, at)
                .iter()
                .all(|t| !t.starts_with("Declare signature")),
            "a declared sig must not be offered again"
        );
    }

    #[test]
    fn declines_when_the_inference_says_nothing() {
        let src = "(defn f (x) x)";
        assert!(
            actions_at(src, 8)
                .iter()
                .all(|t| !t.starts_with("Declare signature")),
            "`(any) -> any` is not worth writing down"
        );
    }

    #[test]
    fn the_declared_edit_is_a_parseable_sig_line_above_the_defn() {
        let src = "(defn f (s) (string/length s))";
        let mut interp = Interp::new();
        let root = brood::syntax::cst::parse(src);
        let scope = brood::syntax::scope::analyze(&root, src);
        let li = LineIndex::new(src);
        let offset_of = |r: Range| li.offset(src, r.start);
        let acts = code_actions(
            &mut interp,
            &uri(),
            &root,
            src,
            &scope,
            &li,
            offset_of,
            &[],
            8,
        );
        let edit = acts
            .iter()
            .find_map(|a| match a {
                CodeActionOrCommand::CodeAction(ca)
                    if ca.title.starts_with("Declare signature") =>
                {
                    Some(ca.edit.as_ref()?.changes.as_ref()?[&uri()][0].clone())
                }
                _ => None,
            })
            .expect("a declare-signature edit");
        assert!(edit.new_text.starts_with("(sig f ("), "{edit:?}");
        assert!(edit.new_text.ends_with(")\n"), "{edit:?}");
        // Inserted at the start of the `defn`'s line, so the file still parses.
        assert_eq!(edit.range.start.line, 0);
        assert_eq!(edit.range.start.character, 0);
        let applied = format!("{}{src}", edit.new_text);
        assert!(
            brood::syntax::cst::parse(&applied)
                .children
                .iter()
                .all(|n| n.kind != NodeKind::Error),
            "the applied edit must parse: {applied}"
        );
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("reduc", "reduce"), 1); // one insertion
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn suggests_a_close_global() {
        let mut interp = Interp::new();
        // No document scope needed for a global typo; empty tree at offset 0.
        let root = brood::syntax::cst::parse("");
        let scope = brood::syntax::scope::analyze(&root, "");
        // `reduce` is a prelude global; `reduc` is one deletion away.
        let s = suggestions(&mut interp, &scope, 0, "reduc");
        assert!(s.contains(&"reduce".to_string()), "got: {s:?}");
    }

    #[test]
    fn no_suggestion_for_a_wildly_different_name() {
        let mut interp = Interp::new();
        let root = brood::syntax::cst::parse("");
        let scope = brood::syntax::scope::analyze(&root, "");
        let s = suggestions(&mut interp, &scope, 0, "zzqqxx");
        assert!(s.is_empty(), "got: {s:?}");
    }

    fn uri() -> Uri {
        use std::str::FromStr;
        Uri::from_str("file:///x.blsp").unwrap()
    }

    // ---- create-defn -------------------------------------------------------

    #[test]
    fn call_head_argc_counts_args_in_head_position_only() {
        let root = brood::syntax::cst::parse("(foo 1 2 3)");
        let at = 1; // on `foo`
        assert_eq!(call_head_argc(&root, at), Some(3));
        // On an argument, not the head → not a call head.
        let on_arg = "(foo 1 2 3)".find('1').unwrap() as u32;
        assert_eq!(call_head_argc(&root, on_arg), None);
    }

    #[test]
    fn param_names_run_past_z() {
        assert_eq!(param_name(0), "a");
        assert_eq!(param_name(25), "z");
        assert_eq!(param_name(26), "a26");
    }

    /// Build an `unbound symbol: NAME` diagnostic over `needle` in `src`, then run
    /// `code_actions` (with the given interp) and return the action titles.
    fn unbound_action_titles(
        interp: &mut Interp,
        src: &str,
        name: &str,
        needle: &str,
    ) -> Vec<String> {
        let root = brood::syntax::cst::parse(src);
        let scope = brood::syntax::scope::analyze(&root, src);
        let li = LineIndex::new(src);
        let start = src.find(needle).unwrap() as u32;
        let range = li.range(
            src,
            Span {
                start,
                end: start + needle.len() as u32,
            },
        );
        let diag = Diagnostic {
            range,
            message: format!("unbound symbol: {name}"),
            ..Default::default()
        };
        let offset_of = |r: Range| li.offset(src, r.start);
        code_actions(
            interp,
            &uri(),
            &root,
            src,
            &scope,
            &li,
            offset_of,
            &[diag],
            0,
        )
        .into_iter()
        .map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) => ca.title,
            CodeActionOrCommand::Command(c) => c.title,
        })
        .collect()
    }

    #[test]
    fn offers_create_function_for_an_unbound_call_head() {
        let mut interp = Interp::new();
        let titles =
            unbound_action_titles(&mut interp, "(frobnicate 1 2)", "frobnicate", "frobnicate");
        assert!(
            titles.iter().any(|t| t == "Create function `frobnicate`"),
            "got: {titles:?}"
        );
    }

    #[test]
    fn create_function_stub_matches_the_call_arity() {
        let mut interp = Interp::new();
        let src = "(frobnicate 1 2)";
        let root = brood::syntax::cst::parse(src);
        let scope = brood::syntax::scope::analyze(&root, src);
        let li = LineIndex::new(src);
        let start = src.find("frobnicate").unwrap() as u32;
        let range = li.range(
            src,
            Span {
                start,
                end: start + 10,
            },
        );
        let diag = Diagnostic {
            range,
            message: "unbound symbol: frobnicate".into(),
            ..Default::default()
        };
        let offset_of = |r: Range| li.offset(src, r.start);
        let acts = code_actions(
            &mut interp,
            &uri(),
            &root,
            src,
            &scope,
            &li,
            offset_of,
            &[diag],
            0,
        );
        let edit = acts
            .iter()
            .find_map(|a| match a {
                CodeActionOrCommand::CodeAction(ca) if ca.title.contains("Create function") => {
                    Some(ca.edit.as_ref()?.changes.as_ref()?[&uri()][0].clone())
                }
                _ => None,
            })
            .expect("a create-function edit");
        assert_eq!(edit.new_text, "\n(defn frobnicate (a b) nil)\n");
    }

    #[test]
    fn does_not_offer_create_function_for_an_operand() {
        let mut interp = Interp::new();
        // `frobnicate` here is an argument, not a call head — no stub offered.
        let titles = unbound_action_titles(
            &mut interp,
            "(io/puts frobnicate)",
            "frobnicate",
            "frobnicate",
        );
        assert!(
            !titles.iter().any(|t| t.contains("Create function")),
            "should not offer create-fn for an operand, got: {titles:?}"
        );
    }

    // ---- auto-import: add `(:use mod)` / qualify a bare unbound name ----------

    /// An interp whose live image exports the provider defs (each a `(defmodule …)`
    /// followed by `defn`s) — the auto-import discovery source, exactly as the LSP's
    /// project-loaded image would carry them.
    fn interp_with_provider(defs: &str) -> Interp {
        let mut interp = Interp::new();
        interp.eval_str(defs).expect("define provider(s)");
        interp
    }

    /// Run code actions for a bare unbound `name` (first occurrence in `src`) and
    /// return `src` with the "Import … from `(:use …)`" edit applied — the readable
    /// way to assert the header edit. `None` if no import action was offered.
    fn applied_import(interp: &mut Interp, src: &str, name: &str) -> Option<String> {
        let root = brood::syntax::cst::parse(src);
        let scope = brood::syntax::scope::analyze(&root, src);
        let li = LineIndex::new(src);
        let start = src.find(name)? as u32;
        let range = li.range(
            src,
            Span {
                start,
                end: start + name.len() as u32,
            },
        );
        let diag = Diagnostic {
            range,
            message: format!("unbound symbol: {name}"),
            ..Default::default()
        };
        let offset_of = |r: Range| li.offset(src, r.start);
        let acts = code_actions(
            interp,
            &uri(),
            &root,
            src,
            &scope,
            &li,
            offset_of,
            &[diag],
            0,
        );
        let edit = acts.iter().find_map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) if ca.title.starts_with("Import ") => {
                Some(ca.edit.as_ref()?.changes.as_ref()?[&uri()][0].clone())
            }
            _ => None,
        })?;
        let s = li.offset(src, edit.range.start) as usize;
        let e = li.offset(src, edit.range.end) as usize;
        Some(format!("{}{}{}", &src[..s], edit.new_text, &src[e..]))
    }

    #[test]
    fn offers_import_and_qualify_for_a_bare_name_with_a_provider() {
        let mut interp = interp_with_provider("(defmodule myprov) (defn thing (x) x)");
        let titles =
            unbound_action_titles(&mut interp, "(defmodule app)\n(thing 1)", "thing", "thing");
        assert!(
            titles
                .iter()
                .any(|t| t == "Import `thing` from `(:use myprov)`"),
            "got: {titles:?}"
        );
        assert!(
            titles.iter().any(|t| t == "Qualify as `myprov/thing`"),
            "got: {titles:?}"
        );
    }

    #[test]
    fn import_edit_inserts_use_into_a_bare_header() {
        let mut interp = interp_with_provider("(defmodule myprov) (defn thing (x) x)");
        let out = applied_import(&mut interp, "(defmodule app)\n(thing 1)", "thing").unwrap();
        assert_eq!(out, "(defmodule app (:use myprov))\n(thing 1)");
    }

    #[test]
    fn import_edit_groups_after_an_existing_use_clause() {
        let mut interp = interp_with_provider("(defmodule myprov) (defn thing (x) x)");
        let src = "(defmodule app\n  (:use test))\n(thing 1)";
        let out = applied_import(&mut interp, src, "thing").unwrap();
        assert_eq!(
            out,
            "(defmodule app\n  (:use test)\n  (:use myprov))\n(thing 1)"
        );
    }

    #[test]
    fn no_import_without_a_header_but_qualify_is_offered() {
        let mut interp = interp_with_provider("(defmodule myprov) (defn thing (x) x)");
        let titles = unbound_action_titles(&mut interp, "(thing 1)", "thing", "thing");
        assert!(
            !titles.iter().any(|t| t.starts_with("Import ")),
            "no header → no `:use` edit, got: {titles:?}"
        );
        assert!(
            titles.iter().any(|t| t == "Qualify as `myprov/thing`"),
            "got: {titles:?}"
        );
    }

    #[test]
    fn does_not_offer_import_for_an_already_used_module() {
        let mut interp = interp_with_provider("(defmodule myprov) (defn thing (x) x)");
        let src = "(defmodule app (:use myprov))\n(thing 1)";
        let titles = unbound_action_titles(&mut interp, src, "thing", "thing");
        assert!(
            !titles.iter().any(|t| t.contains("myprov")),
            "already imported → not re-offered, got: {titles:?}"
        );
    }

    #[test]
    fn does_not_offer_import_for_the_files_own_namespace() {
        // Bare `thing` resolves to this file's own `myprov/thing` — not an import.
        let mut interp = interp_with_provider("(defmodule myprov) (defn thing (x) x)");
        let src = "(defmodule myprov)\n(defn other (x) (thing x))";
        let titles = unbound_action_titles(&mut interp, src, "thing", "thing");
        assert!(
            !titles.iter().any(|t| t.contains("Import")),
            "own namespace is not an import, got: {titles:?}"
        );
    }

    #[test]
    fn multiple_providers_offer_a_choice() {
        // `shared` is exported by two modules — offer one import per provider
        // (the `sexp`/`editor/treesit` clash shape), never a silent pick.
        let mut interp = interp_with_provider(
            "(defmodule mods1) (defn shared (x) x) (defmodule mods2) (defn shared (y) y)",
        );
        let titles = unbound_action_titles(
            &mut interp,
            "(defmodule app)\n(shared 1)",
            "shared",
            "shared",
        );
        assert!(
            titles
                .iter()
                .any(|t| t == "Import `shared` from `(:use mods1)`"),
            "got: {titles:?}"
        );
        assert!(
            titles
                .iter()
                .any(|t| t == "Import `shared` from `(:use mods2)`"),
            "got: {titles:?}"
        );
    }
}
