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

use brood::introspect;
use brood::syntax::scope::ScopeTree;
use brood::Interp;
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, Range, TextEdit, Uri,
    WorkspaceEdit,
};

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
}
