//! `textDocument/semanticTokens/full`: a classified token stream so the editor
//! colours Brood by *meaning* (a call vs. a local vs. a special form), not by
//! regex. Reads the tooling CST plus the scope tree — the same substrate as
//! every other feature — so the classification agrees with hover/goto: a symbol
//! that resolves to a local is a `variable`, a `def`-family head is a `keyword`,
//! a definition's name carries the `definition` modifier, and so on.
//!
//! The token legend is fixed (declared in `main`'s capabilities and mirrored by
//! the `T_*` / `M_*` indices here); tokens are emitted in source order and
//! delta-encoded as the protocol requires. Tokens never cross a line — a
//! multi-line string is split into one token per line.

use brood::syntax::cst::{Node, NodeKind};
use brood::syntax::scope::{BindingKind, Resolution, ScopeTree};
use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensLegend,
};

use crate::line_index::LineIndex;

// Token-type indices into [`legend`]'s `token_types`. Keep in lockstep with it.
const T_KEYWORD: u32 = 0;
const T_FUNCTION: u32 = 1;
const T_VARIABLE: u32 = 2;
const T_STRING: u32 = 3;
const T_NUMBER: u32 = 4;
const T_COMMENT: u32 = 5;
const T_ENUM_MEMBER: u32 = 6;
// Token-modifier bits into [`legend`]'s `token_modifiers`.
const M_DEFINITION: u32 = 1 << 0;

/// The token legend the server advertises and every `data` triple indexes into.
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::KEYWORD,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::COMMENT,
            SemanticTokenType::ENUM_MEMBER,
        ],
        token_modifiers: vec![SemanticTokenModifier::DEFINITION],
    }
}

/// Special forms and the core control/binding macros — coloured as keywords when
/// they head a form. Mirrors `brood.el`'s `brood-special-forms` plus the
/// `def`-family heads (handled via [`is_def_head`] for the *name*, but the head
/// word itself reads as a keyword here).
const KEYWORDS: &[&str] = &[
    "if", "do", "def", "fn", "lambda", "let", "let*", "letrec", "quote", "quasiquote", "defmacro",
    "defn", "defdyn", "defmodule", "when", "unless", "cond", "and", "or", "match", "match*", "try",
    "catch", "throw", "receive", "binding", "dolist", "doseq", "dotimes", "for", "->", "->>",
];

/// All semantic tokens for the document, delta-encoded.
pub fn semantic_tokens(text: &str, root: &Node, tree: &ScopeTree, index: &LineIndex) -> SemanticTokens {
    let mut raws: Vec<Raw> = Vec::new();
    walk(root, text, tree, Role::Normal, index, &mut raws);
    // Source order normally falls out of the depth-first walk, but multi-line
    // splits and sigil skips make it worth a defensive sort before delta-coding.
    raws.sort_by_key(|r| (r.line, r.start));
    SemanticTokens {
        result_id: None,
        data: delta_encode(&raws),
    }
}

/// A token before delta-encoding: absolute line + UTF-16 start column, UTF-16
/// length, and its legend indices.
struct Raw {
    line: u32,
    start: u32,
    len: u32,
    ttype: u32,
    tmods: u32,
}

/// How a node sits in its parent form — drives symbol classification.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// The first form of a `List` (the operator / callee).
    Head,
    /// The name a `def`-family form binds (its second form).
    DefName,
    /// Anything else.
    Normal,
}

fn walk(node: &Node, src: &str, tree: &ScopeTree, role: Role, index: &LineIndex, out: &mut Vec<Raw>) {
    match node.kind {
        NodeKind::List => {
            let def_head = head_sym(node, src).map(is_def_head).unwrap_or(false);
            let mut form_i = 0usize;
            for child in &node.children {
                if child.kind.is_trivia() {
                    walk(child, src, tree, Role::Normal, index, out);
                    continue;
                }
                let r = match form_i {
                    0 => Role::Head,
                    1 if def_head => Role::DefName,
                    _ => Role::Normal,
                };
                walk(child, src, tree, r, index, out);
                form_i += 1;
            }
        }
        // Reader-macro wrappers: the sigil has no token; recurse into the target.
        NodeKind::Root
        | NodeKind::Vector
        | NodeKind::Map
        | NodeKind::Quote
        | NodeKind::Quasi
        | NodeKind::Unquote
        | NodeKind::Splice => {
            for child in &node.children {
                walk(child, src, tree, Role::Normal, index, out);
            }
        }
        NodeKind::Symbol => push_symbol(node, src, tree, role, index, out),
        NodeKind::Keyword => emit(node, src, index, T_ENUM_MEMBER, 0, out),
        NodeKind::Str => emit(node, src, index, T_STRING, 0, out),
        NodeKind::Int | NodeKind::Float => emit(node, src, index, T_NUMBER, 0, out),
        NodeKind::Bool | NodeKind::Nil => emit(node, src, index, T_KEYWORD, 0, out),
        NodeKind::Comment => emit(node, src, index, T_COMMENT, 0, out),
        NodeKind::Whitespace | NodeKind::Error => {}
    }
}

fn push_symbol(node: &Node, src: &str, tree: &ScopeTree, role: Role, index: &LineIndex, out: &mut Vec<Raw>) {
    let name = node.text(src);
    let (ttype, tmods) = if role == Role::DefName {
        // The name being defined.
        (T_FUNCTION, M_DEFINITION)
    } else if role == Role::Head && KEYWORDS.contains(&name) {
        (T_KEYWORD, 0)
    } else {
        match tree.resolve(node.span.start, name) {
            Resolution::Defined { kind: BindingKind::Local, .. } => (T_VARIABLE, 0),
            Resolution::Defined { kind: BindingKind::Global, .. } => (T_FUNCTION, 0),
            // A free name in head position is a call; elsewhere treat as a value.
            Resolution::Free if role == Role::Head => (T_FUNCTION, 0),
            _ => (T_VARIABLE, 0),
        }
    };
    emit(node, src, index, ttype, tmods, out);
}

/// The head symbol's text of a `List`, or `None`.
fn head_sym<'s>(node: &Node, src: &'s str) -> Option<&'s str> {
    let first = node.forms().next()?;
    (first.kind == NodeKind::Symbol).then(|| first.text(src))
}

/// Whether `head` introduces a named definition (so its second form is a name).
/// Any `def…` operator longer than `def` itself, plus bare `def` — mirrors the
/// indentation rule in `brood.el`.
fn is_def_head(head: &str) -> bool {
    head == "def" || head.starts_with("def")
}

/// Emit a token for `node`'s span, split so no token crosses a line (the
/// protocol forbids it — only multi-line strings hit this in practice).
fn emit(node: &Node, src: &str, index: &LineIndex, ttype: u32, tmods: u32, out: &mut Vec<Raw>) {
    let slice = &src[node.span.start as usize..node.span.end as usize];
    let mut byte = node.span.start;
    for part in slice.split('\n') {
        if !part.is_empty() {
            let pos = index.position(src, byte);
            let len: u32 = part.chars().map(|c| c.len_utf16() as u32).sum();
            out.push(Raw {
                line: pos.line,
                start: pos.character,
                len,
                ttype,
                tmods,
            });
        }
        byte += part.len() as u32 + 1; // + the '\n' that `split` removed
    }
}

/// Delta-encode absolute tokens into the protocol's relative triples.
fn delta_encode(raws: &[Raw]) -> Vec<SemanticToken> {
    let mut data = Vec::with_capacity(raws.len());
    let (mut pl, mut pc) = (0u32, 0u32);
    for r in raws {
        let delta_line = r.line - pl;
        let delta_start = if delta_line == 0 { r.start - pc } else { r.start };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length: r.len,
            token_type: r.ttype,
            token_modifiers_bitset: r.tmods,
        });
        pl = r.line;
        pc = r.start;
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::{cst, scope};

    /// Decode the delta stream back to `(line, col, len, type, mods)` tuples.
    fn tokens(src: &str) -> Vec<(u32, u32, u32, u32, u32)> {
        let root = cst::parse(src);
        let tree = scope::analyze(&root, src);
        let index = LineIndex::new(src);
        let st = semantic_tokens(src, &root, &tree, &index);
        let (mut line, mut col) = (0u32, 0u32);
        st.data
            .iter()
            .map(|t| {
                if t.delta_line != 0 {
                    line += t.delta_line;
                    col = t.delta_start;
                } else {
                    col += t.delta_start;
                }
                (line, col, t.length, t.token_type, t.token_modifiers_bitset)
            })
            .collect()
    }

    #[test]
    fn classifies_a_defn() {
        // (defn f (x) "doc" (+ x x))
        let toks = tokens("(defn f (x) \"d\" (+ x x))");
        // `defn` keyword at col 1
        assert!(toks.contains(&(0, 1, 4, T_KEYWORD, 0)), "defn keyword: {toks:?}");
        // `f` is a definition name (function + definition modifier) at col 6
        assert!(toks.contains(&(0, 6, 1, T_FUNCTION, M_DEFINITION)), "f def: {toks:?}");
        // the docstring is a string token
        assert!(toks.iter().any(|t| t.3 == T_STRING), "string: {toks:?}");
        // `+` heads a call → function; `x` is a local → variable
        assert!(toks.iter().any(|t| t.3 == T_FUNCTION && t.2 == 1), "+ fn: {toks:?}");
        assert!(toks.iter().any(|t| t.3 == T_VARIABLE), "local x: {toks:?}");
    }

    #[test]
    fn keyword_and_number_and_comment() {
        let toks = tokens("; hi\n(f :k 42)");
        assert!(toks.iter().any(|t| t.3 == T_COMMENT), "comment: {toks:?}");
        assert!(toks.iter().any(|t| t.3 == T_ENUM_MEMBER), "keyword :k: {toks:?}");
        assert!(toks.iter().any(|t| t.3 == T_NUMBER && t.2 == 2), "number 42: {toks:?}");
    }

    #[test]
    fn a_multiline_string_splits_per_line() {
        // One string spanning two lines → two string tokens, never one that
        // crosses the line boundary.
        let toks = tokens("\"a\nbc\"");
        let strs: Vec<_> = toks.iter().filter(|t| t.3 == T_STRING).collect();
        assert_eq!(strs.len(), 2, "{toks:?}");
        assert_eq!(strs[0].0, 0);
        assert_eq!(strs[1].0, 1);
    }
}
