//! Top-level definitions read straight off the tooling CST: the model behind
//! `documentSymbol` and the document side of `hover`. A pure walk over the
//! root's direct `def` / `defn` / `defmacro` forms — no evaluation, so it works
//! on a buffer the server never runs (and couldn't, mid-edit). Mirrors the
//! `def`-family handling in [`scope`](brood::syntax::scope), but keeps the
//! richer surface (params, docstring) the outline and hover want.

use brood::error::Span;
use brood::syntax::cst::{Node, NodeKind};

/// Which `def`-family form introduced a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefKind {
    /// `(def name value)` — a value binding.
    Var,
    /// `(defn name (params) …)` — a function.
    Fn,
    /// `(defmacro name (params) …)` — a macro.
    Macro,
}

impl DefKind {
    /// The defining keyword, for a signature line / hover header.
    pub fn keyword(self) -> &'static str {
        match self {
            DefKind::Var => "def",
            DefKind::Fn => "defn",
            DefKind::Macro => "defmacro",
        }
    }
}

/// One top-level definition.
pub struct Def<'s> {
    pub kind: DefKind,
    pub name: &'s str,
    /// Span of the name token — where goto-definition lands, and the outline's
    /// selection range.
    pub name_span: Span,
    /// Span of the whole form — the outline's full range.
    pub full_span: Span,
    /// Parameter tokens as written (incl. `&optional` markers and `(opt def)`
    /// groups). Empty for a `Var`.
    pub params: Vec<&'s str>,
    /// A leading-string docstring, when the body has one *and* more body follows
    /// it (a lone string is the return value — the CL/Elisp rule the closure
    /// `doc` field also uses).
    pub doc: Option<&'s str>,
}

impl Def<'_> {
    /// A one-line signature for hover / outline detail: `(name p1 p2)` for a
    /// fn/macro, or just `name` for a var.
    pub fn signature(&self) -> String {
        if self.kind == DefKind::Var {
            return self.name.to_string();
        }
        let mut s = String::from("(");
        s.push_str(self.name);
        for p in &self.params {
            s.push(' ');
            s.push_str(p);
        }
        s.push(')');
        s
    }
}

/// Every top-level `def`/`defn`/`defmacro` in document order.
pub fn top_level<'s>(root: &Node, src: &'s str) -> Vec<Def<'s>> {
    root.forms().filter_map(|f| parse_def(f, src)).collect()
}

/// Read one top-level form as a definition, or `None` if it isn't one.
fn parse_def<'s>(form: &Node, src: &'s str) -> Option<Def<'s>> {
    if form.kind != NodeKind::List {
        return None;
    }
    let mut forms = form.forms();
    let head = forms.next()?;
    let kind = match (head.kind == NodeKind::Symbol).then(|| head.text(src))? {
        "def" => DefKind::Var,
        "defn" => DefKind::Fn,
        "defmacro" => DefKind::Macro,
        _ => return None,
    };
    let name = forms.next()?;
    if name.kind != NodeKind::Symbol {
        return None; // e.g. `(def (destructure) …)` — deferred, not a plain name
    }

    let (params, doc) = if kind == DefKind::Var {
        (Vec::new(), None)
    } else {
        let params = forms
            .next()
            .filter(|p| matches!(p.kind, NodeKind::List | NodeKind::Vector))
            .map(|p| p.forms().map(|n| n.text(src)).collect())
            .unwrap_or_default();
        // Docstring: a leading string with more body after it.
        let body: Vec<&Node> = forms.collect();
        let doc = match body.as_slice() {
            [first, _, ..] if first.kind == NodeKind::Str => Some(str_contents(first.text(src))),
            _ => None,
        };
        (params, doc)
    };

    Some(Def {
        kind,
        name: name.text(src),
        name_span: name.span,
        full_span: form.span,
        params,
        doc,
    })
}

/// Strip the surrounding quotes off a string token's source. We show the raw
/// inner text for hover rather than decoding escapes — good enough for display.
fn str_contents(tok: &str) -> &str {
    tok.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(tok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brood::syntax::cst;

    fn defs(src: &str) -> Vec<Def<'_>> {
        // Leak the parse so the borrowed `Def`s outlive this helper in a test.
        let root: &'static Node = Box::leak(Box::new(cst::parse(src)));
        let src: &'static str = Box::leak(src.to_string().into_boxed_str());
        top_level(root, src)
    }

    #[test]
    fn extracts_defn_with_params_and_doc() {
        let ds = defs("(defn sq (x) \"square it\" (* x x))");
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].kind, DefKind::Fn);
        assert_eq!(ds[0].name, "sq");
        assert_eq!(ds[0].params, vec!["x"]);
        assert_eq!(ds[0].doc, Some("square it"));
        assert_eq!(ds[0].signature(), "(sq x)");
    }

    #[test]
    fn lone_string_body_is_a_return_value_not_a_docstring() {
        // `(defn name (x) "hi")` — the string is the return value, not docs.
        let ds = defs("(defn greet (x) \"hi\")");
        assert_eq!(ds[0].doc, None);
    }

    #[test]
    fn def_is_a_var_with_no_params() {
        let ds = defs("(def pi 3.14)");
        assert_eq!(ds[0].kind, DefKind::Var);
        assert_eq!(ds[0].signature(), "pi");
        assert!(ds[0].params.is_empty());
    }

    #[test]
    fn keeps_optional_and_rest_markers_in_signature() {
        let ds = defs("(defn f (a &optional (b 1) & cs) a)");
        assert_eq!(ds[0].signature(), "(f a &optional (b 1) & cs)");
    }

    #[test]
    fn ignores_non_definitions() {
        assert!(defs("(println \"hi\") 42").is_empty());
    }
}
