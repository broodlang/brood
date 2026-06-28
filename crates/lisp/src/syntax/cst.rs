//! A lossless, span-carrying concrete syntax tree (CST) for **tooling** — the
//! language server, and later a formatter. It is the deliberate counterpart to
//! [`reader`](super::reader): the reader turns text into evaluable `Value`s and
//! *rejects* malformed input; this turns text into a tree of [`Node`]s and
//! *tolerates* it — `parse` always returns a tree, so a half-typed buffer still
//! navigates. See `docs/lsp.md` and ADR-025.
//!
//! Properties that the rest of tooling relies on:
//! - **Heap-free.** Nodes own only their kind, [`Span`], and children — no
//!   `Heap`, no `Value` — so a server holds many documents cheaply and `Send`s
//!   them between threads. A token's decoded value is sliced from the source on
//!   demand ([`Node::text`]).
//! - **Total / error-tolerant.** Unbalanced delimiters and unterminated strings
//!   become [`NodeKind::Error`] nodes; parsing resumes after them.
//! - **Lossless.** Trivia (whitespace, comments) are nodes, and every byte of
//!   the input lies within the root's span, so `root.text(src) == src`.
//! - **Shared token rules.** What counts as an atom / number / keyword comes
//!   from [`atom`](super::atom), the same module the reader uses, so the two
//!   parsers can't disagree on tokens.

use crate::error::Span;
use crate::syntax::atom::{self, AtomKind};
use crate::syntax::scanner::Scanner;

/// The kind of a CST node. Tokens (leaves) carry no children; the rest nest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// The whole file: a sequence of forms interleaved with trivia.
    Root,
    List,   // ( … )
    Vector, // [ … ]
    Map,    // { … } — a map literal (alternating key/value forms)
    // Reader macros, kept *as written* (not lowered to `(quote x)` …) so the
    // tree mirrors the source. Each wraps its target form as a child.
    Quote,   // 'x
    Quasi,   // `x
    Unquote, // ~x
    Splice,  // ~@x
    // Atom tokens.
    Symbol,
    Keyword,
    Int,
    Float,
    Str,
    Bool,
    Nil,
    // Trivia — present so the tree is lossless / round-trippable.
    Whitespace,
    Comment,
    /// An unparseable run (a stray close delimiter, an unterminated string, or a
    /// missing close — a zero-width marker at the point one was expected).
    Error,
}

impl NodeKind {
    /// Whitespace and comments: present for losslessness, skipped by analysis.
    pub fn is_trivia(self) -> bool {
        matches!(self, NodeKind::Whitespace | NodeKind::Comment)
    }
}

/// One node of the CST. A leaf has no children; its text is `span.slice(src)`.
#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub span: Span,
    pub children: Vec<Node>,
}

impl Node {
    /// The exact source text this node covers.
    pub fn text<'s>(&self, src: &'s str) -> &'s str {
        self.span.slice(src)
    }

    /// The innermost node whose span contains byte offset `at`. This is the
    /// "what is under the cursor?" primitive behind hover / goto / completion
    /// context / semantic tokens. Returns `None` only if `at` is outside this
    /// node entirely.
    pub fn node_at(&self, at: u32) -> Option<&Node> {
        if !self.span.contains(at) {
            return None;
        }
        for child in &self.children {
            if let Some(inner) = child.node_at(at) {
                return Some(inner);
            }
        }
        Some(self)
    }

    /// This node's children with trivia removed — the structural sub-forms.
    pub fn forms(&self) -> impl Iterator<Item = &Node> {
        self.children.iter().filter(|c| !c.kind.is_trivia())
    }
}

/// Parse `src` into a lossless CST. Never fails: malformed input is recorded as
/// [`NodeKind::Error`] nodes and parsing continues.
pub fn parse(src: &str) -> Node {
    let src_len = src.len();
    let mut cst = Cst {
        s: Scanner::new(src),
        depth: 0,
    };
    let mut children = Vec::new();
    while cst.s.peek().is_some() {
        children.push(cst.trivia_or_form());
    }
    Node {
        kind: NodeKind::Root,
        span: Span::new(0, src_len),
        children,
    }
}

/// Bound on nesting depth. Past this, an opening delimiter becomes an `Error`
/// node spanning the unparsed body — the CST is still total, just refuses to
/// build a million-deep tree (which would overflow the native Rust stack on
/// this parser and on every downstream walk in `scope.rs` / `check.rs`).
const MAX_DEPTH: u32 = 256;

struct Cst<'a> {
    s: Scanner<'a>,
    depth: u32,
}

impl<'a> Cst<'a> {
    fn span_from(&self, start: usize) -> Span {
        Span::new(start, self.s.pos())
    }

    fn leaf(&self, kind: NodeKind, start: usize) -> Node {
        Node {
            kind,
            span: self.span_from(start),
            children: Vec::new(),
        }
    }

    /// One run of trivia, or one form. (Trivia stays in the tree — lossless.)
    fn trivia_or_form(&mut self) -> Node {
        match self.s.peek() {
            Some(c) if atom::is_trivia_ws(c) => self.trivia(false),
            Some(';') => self.trivia(true),
            _ => self.form(),
        }
    }

    /// Consume a maximal run of whitespace (`,` counts) or a single `;` comment
    /// to end-of-line. We don't reuse [`Scanner::skip_trivia`] (which consumes
    /// whitespace *and* comments uninterrupted) because the CST keeps them as
    /// separate nodes for losslessness.
    fn trivia(&mut self, comment: bool) -> Node {
        let start = self.s.pos();
        if comment {
            self.s.skip_line_comment();
            self.leaf(NodeKind::Comment, start)
        } else {
            while matches!(self.s.peek(), Some(c) if atom::is_trivia_ws(c)) {
                self.s.bump();
            }
            self.leaf(NodeKind::Whitespace, start)
        }
    }

    fn form(&mut self) -> Node {
        let start = self.s.pos();
        match self.s.peek() {
            Some('(') => self.seq(NodeKind::List, ')', start),
            Some('[') => self.seq(NodeKind::Vector, ']', start),
            Some('{') => self.seq(NodeKind::Map, '}', start),
            Some('\'') => {
                self.s.bump();
                self.wrap(NodeKind::Quote, start)
            }
            Some('`') => {
                self.s.bump();
                self.wrap(NodeKind::Quasi, start)
            }
            Some('~') => {
                self.s.bump();
                let kind = if self.s.peek() == Some('@') {
                    self.s.bump();
                    NodeKind::Splice
                } else {
                    NodeKind::Unquote
                };
                self.wrap(kind, start)
            }
            Some('"') => self.string(start),
            Some('#') => self.hash(start),
            // A stray close delimiter is an error token; resume after it.
            Some(')') | Some(']') | Some('}') => {
                self.s.bump();
                self.leaf(NodeKind::Error, start)
            }
            Some(_) => self.atom(start),
            // Called only when `peek()` is `Some`, so this is unreachable in
            // practice; produce a zero-width error rather than panicking.
            None => self.leaf(NodeKind::Error, start),
        }
    }

    /// A delimited sequence. Recovers: a stray inner close is handled by `form`;
    /// at EOF without our close, emit a zero-width `Error` child marking where
    /// the close was expected, then stop (the node's children stay navigable).
    ///
    /// Past [`MAX_DEPTH`], skip the body without recursing — emit a single
    /// `Error` child so the tree stays total but the parser doesn't grow the
    /// native stack with the source.
    fn seq(&mut self, kind: NodeKind, close: char, start: usize) -> Node {
        self.s.bump(); // opening delimiter
        if self.depth >= MAX_DEPTH {
            let err_start = self.s.pos();
            self.skip_to_balanced_close(close);
            // The Error covers what wasn't parsed; the outer node still spans
            // through the close (if we found one), just like a normal `seq`.
            // `skip_to_balanced_close` leaves `pos` *past* the close delimiter
            // when it found one, but *at* EOF when it ran out of input. So the
            // Error's end is:
            //   - found a close → `pos - 1`, excluding the close byte (which
            //     belongs to the outer node, not the unparsed body);
            //   - hit EOF → `pos` unchanged, since there is no close byte to
            //     exclude (subtracting here would trim the last real byte).
            // The `.max(err_start)` keeps the span non-empty for an empty body.
            let err_end = if self.s.at_end() {
                self.s.pos()
            } else {
                self.s.pos().saturating_sub(1)
            };
            let err = Node {
                kind: NodeKind::Error,
                span: Span::new(err_start, err_end.max(err_start)),
                children: Vec::new(),
            };
            return Node {
                kind,
                span: self.span_from(start),
                children: vec![err],
            };
        }
        self.depth += 1;
        let mut children = Vec::new();
        loop {
            match self.s.peek() {
                None => {
                    children.push(self.leaf(NodeKind::Error, self.s.pos())); // missing close
                    break;
                }
                Some(c) if c == close => {
                    self.s.bump();
                    break;
                }
                _ => children.push(self.trivia_or_form()),
            }
        }
        self.depth -= 1;
        Node {
            kind,
            span: self.span_from(start),
            children,
        }
    }

    /// Flat scan to the matching close delimiter, used when nesting is past
    /// [`MAX_DEPTH`] — tracks delimiter balance at the byte level (not by
    /// recursive descent) and skips `"…"` strings so a `"` inside a string
    /// doesn't fool the counter. Always terminates (advances `pos` by at
    /// least one byte per iteration or stops at EOF).
    fn skip_to_balanced_close(&mut self, close: char) {
        let mut depth = 1i32; // we've already consumed our opener
        while let Some(c) = self.s.peek() {
            match c {
                '(' | '[' | '{' => {
                    self.s.bump();
                    depth += 1;
                }
                ')' | ']' | '}' => {
                    self.s.bump();
                    depth -= 1;
                    if depth == 0 && c == close {
                        return;
                    }
                    if depth <= 0 {
                        return; // a mismatched close — let the outer parser see it
                    }
                }
                '"' => {
                    self.s.bump(); // opening quote
                    let _ = self.s.scan_string_body(None);
                }
                ';' => self.s.skip_line_comment(),
                _ => {
                    self.s.bump();
                }
            }
        }
    }

    /// A reader-macro wrapper (`'` `` ` `` `~` `~@`): the sigil is already
    /// consumed; attach any interior trivia and the one target form as children.
    /// A dangling sigil (EOF or a close delimiter follows) yields a childless
    /// node — an incomplete form the LSP can flag, without derailing the parse.
    ///
    /// Past [`MAX_DEPTH`], the wrapper is left childless rather than recursing
    /// into another `form`, matching the [`seq`] depth-cap behaviour.
    fn wrap(&mut self, kind: NodeKind, start: usize) -> Node {
        let mut children = Vec::new();
        // interior trivia, kept (lossless): `' x`, `` ` ;c\n x``
        while matches!(self.s.peek(), Some(c) if atom::is_trivia_ws(c) || c == ';') {
            children.push(self.trivia_or_form());
        }
        if self.depth >= MAX_DEPTH {
            return Node {
                kind,
                span: self.span_from(start),
                children,
            };
        }
        self.depth += 1;
        match self.s.peek() {
            Some(c) if c != ')' && c != ']' && c != '}' => children.push(self.form()),
            _ => {} // dangling sigil — recover, leaving the wrapper childless
        }
        self.depth -= 1;
        Node {
            kind,
            span: self.span_from(start),
            children,
        }
    }

    /// A `"…"` string. An unterminated string (EOF before the close quote)
    /// becomes an `Error` node spanning to EOF, since `Node` carries no
    /// "recovered" sub-state — `Error` is how syntactic diagnostics find it.
    ///
    /// A `#`-dispatched form. `#b"…"` is a bytes literal — scanned like a string
    /// so the leaf span covers the whole `#b"…"` (the formatter preserves it
    /// verbatim; it highlights as a string token). Any other `#x` is a tolerant
    /// error token, like every CST error.
    fn hash(&mut self, start: usize) -> Node {
        if self.s.starts_with("#b\"") {
            self.s.bump(); // '#'
            self.s.bump(); // 'b'
            self.string(start)
        } else {
            // `#` is an ordinary atom character (e.g. the symbol `#q`).
            self.atom(start)
        }
    }

    /// Uses [`Scanner::scan_string_body`] with `out: None` — same body-walk
    /// the reader uses, just without decoding (the CST keeps content as the
    /// source span; readers slice it on demand).
    fn string(&mut self, start: usize) -> Node {
        self.s.bump(); // opening quote
        match self.s.scan_string_body(None) {
            crate::syntax::scanner::StringScan::Closed => self.leaf(NodeKind::Str, start),
            crate::syntax::scanner::StringScan::Unterminated => self.leaf(NodeKind::Error, start),
            // The body was scanned through its close quote, so the span covers
            // the whole string — record an `Error` node and carry on with the
            // rest of the buffer (tolerant, like every CST error).
            crate::syntax::scanner::StringScan::BadEscape { .. } => {
                self.leaf(NodeKind::Error, start)
            }
        }
    }

    /// An atom: consume to the next delimiter, then classify the token with the
    /// shared [`atom`] rules so the kind matches what the reader would produce.
    fn atom(&mut self, start: usize) -> Node {
        let token = self.s.read_atom();
        let kind = match atom::classify(token) {
            AtomKind::Nil => NodeKind::Nil,
            AtomKind::Bool(_) => NodeKind::Bool,
            AtomKind::Int(_) => NodeKind::Int,
            AtomKind::Float(_) => NodeKind::Float,
            AtomKind::Keyword => NodeKind::Keyword,
            AtomKind::Symbol => NodeKind::Symbol,
            // An integer-shaped token that overflows `i64` — the reader
            // rejects this as a parse error; in the tooling tree it's an
            // `Error` token so the LSP flags it the same way.
            AtomKind::IntOverflow => NodeKind::Error,
        };
        self.leaf(kind, start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flatten the leaves (childless nodes) left-to-right.
    fn leaves<'t>(n: &'t Node, out: &mut Vec<&'t Node>) {
        if n.children.is_empty() {
            out.push(n);
        } else {
            for c in &n.children {
                leaves(c, out);
            }
        }
    }

    #[test]
    fn root_span_covers_whole_input_and_round_trips() {
        let src = "(defn sq (x)\n  \"doc\"  ; comment\n  (* x x))\n";
        let root = parse(src);
        // The losslessness guarantee: the root spans every byte, so the source
        // is always recoverable by slicing. (Delimiters live in their parent's
        // span rather than as separate tokens, so leaves don't *tile* the input
        // — consumers slice from source, they don't concatenate leaves.)
        assert_eq!(root.text(src), src, "root must cover every byte");
        // Leaves are still ordered and disjoint within the root.
        let mut ls = Vec::new();
        leaves(&root, &mut ls);
        let mut prev = 0u32;
        for leaf in ls {
            assert!(leaf.span.start >= prev, "leaves are in source order");
            assert!(leaf.span.end <= src.len() as u32);
            prev = leaf.span.end;
        }
    }

    #[test]
    fn node_at_finds_the_symbol_under_the_cursor() {
        let src = "(foo bar baz)";
        let root = parse(src);
        let at = src.find("bar").unwrap() as u32 + 1; // inside "bar"
        let n = root.node_at(at).expect("a node under the cursor");
        assert_eq!(n.kind, NodeKind::Symbol);
        assert_eq!(n.text(src), "bar");
    }

    #[test]
    fn classifies_atoms_like_the_reader() {
        let kinds: Vec<NodeKind> = parse("1 2.5 :kw foo nil true \"s\"")
            .forms()
            .map(|n| n.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                NodeKind::Int,
                NodeKind::Float,
                NodeKind::Keyword,
                NodeKind::Symbol,
                NodeKind::Nil,
                NodeKind::Bool,
                NodeKind::Str,
            ]
        );
    }

    #[test]
    fn keeps_quote_sugar_as_written() {
        let root = parse("'(a b) `c ~d ~@e");
        let kinds: Vec<NodeKind> = root.forms().map(|n| n.kind).collect();
        assert_eq!(
            kinds,
            vec![
                NodeKind::Quote,
                NodeKind::Quasi,
                NodeKind::Unquote,
                NodeKind::Splice
            ]
        );
        // The quote wraps the list as its (only) structural child.
        let quote = root.forms().next().unwrap();
        assert_eq!(quote.forms().next().unwrap().kind, NodeKind::List);
    }

    #[test]
    fn comments_and_whitespace_are_kept_as_trivia() {
        let root = parse("a ; hi\nb");
        assert!(root
            .children
            .iter()
            .any(|c| c.kind == NodeKind::Comment && c.text("a ; hi\nb") == "; hi\n"));
    }

    #[test]
    fn recovers_from_unclosed_list() {
        // Always returns a tree; the List is present with a trailing error marker.
        let src = "(foo (bar ";
        let root = parse(src);
        assert_eq!(root.text(src), src);
        let outer = root.forms().next().unwrap();
        assert_eq!(outer.kind, NodeKind::List);
        // somewhere inside there is an Error marker for the missing close(s)
        let mut ls = Vec::new();
        leaves(&root, &mut ls);
        assert!(ls.iter().any(|n| n.kind == NodeKind::Error));
    }

    #[test]
    fn recovers_from_stray_close_and_unterminated_string() {
        let stray = parse(")");
        assert_eq!(stray.forms().next().unwrap().kind, NodeKind::Error);

        let unterminated = parse("\"oops");
        assert_eq!(unterminated.forms().next().unwrap().kind, NodeKind::Error);
    }

    #[test]
    fn map_literals_parse_for_tooling() {
        // Eval rejects `{ }` today, but the tooling tree accepts it so a buffer
        // mid-edit still navigates (and to anticipate the planned map literals).
        let root = parse("{:a 1 :b 2}");
        assert_eq!(root.forms().next().unwrap().kind, NodeKind::Map);
    }

    #[test]
    fn handles_multibyte_input_with_byte_spans() {
        let src = "(λ \"café\")"; // multi-byte chars before and inside
        let root = parse(src);
        assert_eq!(root.text(src), src);
        let list = root.forms().next().unwrap();
        let inner: Vec<NodeKind> = list.forms().map(|n| n.kind).collect();
        assert_eq!(inner, vec![NodeKind::Symbol, NodeKind::Str]);
    }
}
