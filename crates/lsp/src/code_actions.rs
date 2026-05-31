//! `textDocument/codeAction` — quick-fixes off the diagnostics we already
//! publish.
//!
//! Today's one action: **"did you mean?"** for an unbound symbol. When the
//! advisory checker flags `unbound symbol: foo`, we offer to replace `foo` with
//! the closest known name — a global (prelude/builtin/project def), a special
//! form, or a local in scope at that point — within a small edit distance. The
//! diagnostic's range already narrows to the offending token (see
//! `refine_diagnostic_range` in `main.rs`), so the fix edits exactly that span.
//!
//! Pure name analysis: candidates come from the introspection surface
//! (`global_names`) + the CST scope walker, never from running the buffer.

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
    scope: &ScopeTree,
    offset_of: impl Fn(Range) -> u32,
    context_diagnostics: &[Diagnostic],
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    for diag in context_diagnostics {
        let Some(name) = diag.message.strip_prefix(UNBOUND_PREFIX) else {
            continue;
        };
        let name = name.trim();
        let offset = offset_of(diag.range);
        for suggestion in suggestions(interp, scope, offset, name) {
            actions.push(did_you_mean(uri, diag, &suggestion));
        }
    }
    actions
}

/// Quick-fixes that come from the file's *structure* rather than a published
/// diagnostic: today, "remove a seemingly-unused `(require 'mod)`". Offered for
/// every standalone top-level require whose form overlaps the requested
/// `[req_start, req_end)` byte range and whose module is never referenced by a
/// qualified `mod/…` name anywhere in the file.
///
/// **Conservative by construction.** Any textual `mod/` occurrence keeps the
/// require (so a *used* module is never flagged — false negatives only), and we
/// touch only a lone `(require 'mod)` (a `(:use mod)` clause inside a
/// `defmodule` header is a different construct, and `(require 'a 'b)` is left
/// alone). A require kept purely for load side effects can't be detected
/// statically, so this is a **non-preferred** suggestion the user opts into, not
/// an auto-fix — matching `docs/lsp.md`'s planned "remove unused require".
pub fn unused_require_actions(
    uri: &Uri,
    root: &Node,
    src: &str,
    line_index: &LineIndex,
    req_start: u32,
    req_end: u32,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    for form in root.forms() {
        // Only requires overlapping the requested range (LSP asks per cursor /
        // selection; don't surface a far-away file's removal).
        if form.span.end <= req_start || form.span.start >= req_end {
            continue;
        }
        let Some(module) = require_module(form, src) else {
            continue;
        };
        // Used iff a qualified `mod/…` reference appears anywhere in the file.
        // The require form itself holds `'mod`, not `mod/`, so it never matches.
        if src.contains(&format!("{module}/")) {
            continue;
        }
        let edit = TextEdit {
            range: line_delete_range(src, form.span, line_index),
            new_text: String::new(),
        };
        let mut changes = HashMap::new();
        changes.insert(uri.clone(), vec![edit]);
        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Remove seemingly-unused `(require '{module})`"),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            ..Default::default()
        }));
    }
    actions
}

/// The module name of a standalone `(require 'mod)` form, or `None` if `form`
/// isn't one. Requires exactly one argument that is a quoted symbol — a
/// multi-arg `(require 'a 'b)` or a computed require yields `None` (left alone).
fn require_module<'s>(form: &Node, src: &'s str) -> Option<&'s str> {
    if form.kind != NodeKind::List {
        return None;
    }
    let mut forms = form.forms();
    let head = forms.next()?;
    if head.kind != NodeKind::Symbol || head.text(src) != "require" {
        return None;
    }
    let arg = forms.next()?;
    if forms.next().is_some() {
        return None; // more than one argument
    }
    if arg.kind != NodeKind::Quote {
        return None;
    }
    let inner = arg.forms().next()?;
    (inner.kind == NodeKind::Symbol).then(|| inner.text(src))
}

/// The edit range that removes the form at `span`. When the form is alone on its
/// line(s) (only whitespace before its start and after its end), expand to delete
/// the whole physical line including indentation and the trailing newline;
/// otherwise delete exactly the form so neighbouring code on the same line is
/// untouched.
fn line_delete_range(src: &str, span: Span, li: &LineIndex) -> Range {
    let (start, end) = (span.start as usize, span.end as usize);
    let line_start = src[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let pre_ws = src[line_start..start]
        .chars()
        .all(|c| c == ' ' || c == '\t');
    let line_end = src[end..]
        .find('\n')
        .map(|i| end + i + 1)
        .unwrap_or(src.len());
    let post_ws = src[end..line_end]
        .chars()
        .all(|c| c == ' ' || c == '\t' || c == '\n');
    let (s, e) = if pre_ws && post_ws {
        (line_start as u32, line_end as u32)
    } else {
        (span.start, span.end)
    };
    Range {
        start: li.position(src, s),
        end: li.position(src, e),
    }
}

/// One "Replace with `X`" quick-fix targeting the diagnostic's range.
fn did_you_mean(uri: &Uri, diag: &Diagnostic, suggestion: &str) -> CodeActionOrCommand {
    let edit = TextEdit {
        range: diag.range,
        new_text: suggestion.to_string(),
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Replace with `{suggestion}`"),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diag.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        // A quick-fix that resolves the diagnostic the user is looking at — mark
        // it preferred so a single keystroke applies the top suggestion.
        is_preferred: Some(true),
        ..Default::default()
    })
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

    /// Run `unused_require_actions` over the whole file (request range = whole
    /// doc), returning the action titles.
    fn unused_titles(src: &str) -> Vec<String> {
        let root = brood::syntax::cst::parse(src);
        let li = LineIndex::new(src);
        unused_require_actions(&uri(), &root, src, &li, 0, src.len() as u32)
            .into_iter()
            .map(|a| match a {
                CodeActionOrCommand::CodeAction(ca) => ca.title,
                CodeActionOrCommand::Command(c) => c.title,
            })
            .collect()
    }

    #[test]
    fn flags_a_require_with_no_qualified_use() {
        let titles = unused_titles("(require 'format)\n(defn f (x) x)\n");
        assert_eq!(titles, vec!["Remove seemingly-unused `(require 'format)`"]);
    }

    #[test]
    fn keeps_a_require_whose_module_is_used_qualified() {
        let titles = unused_titles("(require 'format)\n(defn f (x) (format/format-source x))\n");
        assert!(titles.is_empty(), "got: {titles:?}");
    }

    #[test]
    fn leaves_a_multi_arg_require_alone() {
        // Not a lone `(require 'mod)` — conservative, don't offer.
        let titles = unused_titles("(require 'a 'b)\n");
        assert!(titles.is_empty(), "got: {titles:?}");
    }

    #[test]
    fn only_offers_requires_overlapping_the_request_range() {
        // Two unused requires on separate lines; ask only over line 0's range.
        let src = "(require 'aaa)\n(require 'bbb)\n";
        let root = brood::syntax::cst::parse(src);
        let li = LineIndex::new(src);
        // Byte range of just the first line (the first require).
        let acts = unused_require_actions(&uri(), &root, src, &li, 0, 5);
        assert_eq!(acts.len(), 1);
        let title = match &acts[0] {
            CodeActionOrCommand::CodeAction(ca) => &ca.title,
            _ => panic!(),
        };
        assert!(title.contains("aaa"), "got: {title}");
    }

    #[test]
    fn delete_edit_removes_the_whole_line() {
        let src = "(require 'format)\n(defn f (x) x)\n";
        let root = brood::syntax::cst::parse(src);
        let li = LineIndex::new(src);
        let acts = unused_require_actions(&uri(), &root, src, &li, 0, src.len() as u32);
        let CodeActionOrCommand::CodeAction(ca) = &acts[0] else {
            panic!()
        };
        let edit = &ca.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri()][0];
        // Whole first line (0,0)..(1,0): applying it leaves the defn intact.
        assert_eq!(edit.range.start, lsp_types::Position::new(0, 0));
        assert_eq!(edit.range.end, lsp_types::Position::new(1, 0));
        assert_eq!(edit.new_text, "");
    }
}
