//! Syntactic diagnostics, read straight off the tooling CST. This is the whole
//! of Tier 0: the [`cst`](brood::syntax::cst) parse is *total*, recording every
//! malformed run as a [`NodeKind::Error`] node, so a diagnostic is just a walk
//! that collects those nodes and names what went wrong. No evaluation, no type
//! checking — it works on a half-typed buffer that cannot yet run.
//!
//! Output is deliberately LSP-agnostic ([`SynDiagnostic`] carries a byte
//! [`Span`] and a message); `main` projects spans to ranges through the
//! [`LineIndex`](crate::line_index). That keeps this unit-testable against the
//! CST alone.

use brood::error::Span;
use brood::syntax::cst::{Node, NodeKind};

/// A located syntactic problem: a byte range and a human message. Severity is
/// always "error" at this tier (an `Error` node means the text doesn't parse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynDiagnostic {
    pub span: Span,
    pub message: String,
}

/// Collect every syntactic diagnostic in `root` (the parse of `src`).
pub fn collect(root: &Node, src: &str) -> Vec<SynDiagnostic> {
    let mut out = Vec::new();
    walk(root, src, &mut out);
    out
}

fn walk(node: &Node, src: &str, out: &mut Vec<SynDiagnostic>) {
    if node.kind == NodeKind::Error {
        out.push(SynDiagnostic {
            span: node.span,
            message: describe(node, src),
        });
    }
    for child in &node.children {
        walk(child, src, out);
    }
}

/// Name an `Error` node from its text. The CST's recovery rules (see
/// `cst.rs`) produce exactly three shapes, which we distinguish by their span:
/// a zero-width marker (missing close), a lone close delimiter (stray close),
/// or a run beginning with `"` (unterminated string).
fn describe(node: &Node, src: &str) -> String {
    let text = node.span.slice(src);
    match text.chars().next() {
        // Zero-width marker emitted at EOF inside an unclosed `(`/`[`/`{`.
        None => "unexpected end of input: unclosed delimiter".to_string(),
        Some(')') | Some(']') | Some('}') if text.chars().count() == 1 => {
            format!("unmatched `{text}`")
        }
        Some('"') => "unterminated string literal".to_string(),
        _ => "syntax error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::cst;

    fn messages(src: &str) -> Vec<String> {
        collect(&cst::parse(src), src)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn clean_source_has_no_diagnostics() {
        assert!(messages("(defn sq (x) (* x x))").is_empty());
    }

    #[test]
    fn flags_stray_close() {
        assert_eq!(messages("(a) )"), vec!["unmatched `)`".to_string()]);
    }

    #[test]
    fn flags_unterminated_string() {
        // Top-level so the string is the only malformed run. (Inside a list, an
        // unterminated string also swallows the close, so the list reports a
        // second, separate "unclosed delimiter" — also correct.)
        assert_eq!(
            messages("\"oops"),
            vec!["unterminated string literal".to_string()]
        );
    }

    #[test]
    fn flags_unclosed_delimiter() {
        let msgs = messages("(foo (bar ");
        assert!(
            msgs.iter().all(|m| m.contains("unclosed delimiter")),
            "got {msgs:?}"
        );
        assert!(!msgs.is_empty());
    }

    #[test]
    fn diagnostic_span_points_at_the_offending_text() {
        let src = "(a) ]";
        let diags = collect(&cst::parse(src), src);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].span.slice(src), "]");
    }
}
