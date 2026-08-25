//! Top-level definitions read straight off the tooling CST: the model behind
//! `documentSymbol` and the document side of `hover`. A pure walk over the
//! root's direct `def` / `defn` / `defmacro` / `defrecord` / `defability` forms —
//! no evaluation, so it works on a buffer the server never runs (and couldn't,
//! mid-edit). Mirrors the `def`-family handling in
//! [`scope`](brood::syntax::scope), but keeps the richer surface (params,
//! docstring) the outline and hover want. A `defrecord`'s name doubles as its
//! constructor (so its fields become the signature params); a `defability`
//! surfaces as an interface. The globals those macros *synthesize* (accessors,
//! op dispatchers) aren't visible here — they're navigable through the runtime
//! def-site table instead (`source-location`, populated on load; see
//! [`crate::definition`]).

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
    /// `(defrecord name (fields) …)` — a record; the name doubles as the
    /// constructor, so its `params` are the fields.
    Record,
    /// `(defability Name (ops…) …)` — an ability (a generic-function interface).
    Ability,
}

impl DefKind {
    /// The defining keyword, for a signature line / hover header.
    pub fn keyword(self) -> &'static str {
        match self {
            DefKind::Var => "def",
            DefKind::Fn => "defn",
            DefKind::Macro => "defmacro",
            DefKind::Record => "defrecord",
            DefKind::Ability => "defability",
        }
    }

    /// Whether a signature renders with a parenthesized parameter list (a
    /// callable) rather than as a bare name.
    fn is_callable(self) -> bool {
        matches!(self, DefKind::Fn | DefKind::Macro | DefKind::Record)
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
        if !self.kind.is_callable() {
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

/// Every top-level `def`/`defn`/`defmacro` in document order — the file outline.
pub fn top_level<'s>(root: &Node, src: &'s str) -> Vec<Def<'s>> {
    root.forms().filter_map(|f| parse_def(f, src)).collect()
}

/// Find the definition whose name token is exactly `name_span`, searching at any
/// depth. Unlike [`top_level`], this recurses: a `def` nested in a `do`/`when`
/// still defines a *global* (def is global wherever it appears — see
/// [`scope`](brood::syntax::scope)), so hover must locate it even when it isn't a
/// direct child of the root.
pub fn find_def<'s>(node: &Node, src: &'s str, name_span: Span) -> Option<Def<'s>> {
    if let Some(d) = parse_def(node, src) {
        if d.name_span == name_span {
            return Some(d);
        }
    }
    node.children
        .iter()
        .find_map(|c| find_def(c, src, name_span))
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
        "defrecord" => DefKind::Record,
        "defability" => DefKind::Ability,
        _ => return None,
    };
    let name = forms.next()?;
    if name.kind != NodeKind::Symbol {
        return None; // e.g. `(def (destructure) …)` — deferred, not a plain name
    }

    // Params: the fields (Record) or the arglist (Fn/Macro); none for a Var or an
    // Ability (whose name isn't itself callable). Docstring: only Fn/Macro carry a
    // leading-string doc in the position we read — a Record's third form is its
    // field list, not a docstring.
    let (params, doc) = if kind.is_callable() {
        let params = forms
            .next()
            .filter(|p| matches!(p.kind, NodeKind::List | NodeKind::Vector))
            .map(|p| p.forms().map(|n| n.text(src)).collect())
            .unwrap_or_default();
        let doc = if matches!(kind, DefKind::Fn | DefKind::Macro) {
            // A leading string with more body after it (a lone string is a return value).
            let body: Vec<&Node> = forms.collect();
            match body.as_slice() {
                [first, _, ..] if first.kind == NodeKind::Str => {
                    Some(str_contents(first.text(src)))
                }
                _ => None,
            }
        } else {
            None
        };
        (params, doc)
    } else {
        (Vec::new(), None)
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
    fn recognizes_defrecord_as_a_struct_whose_fields_are_the_constructor_params() {
        let ds = defs("(defrecord point (x y))");
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].kind, DefKind::Record);
        assert_eq!(ds[0].name, "point");
        assert_eq!(ds[0].params, vec!["x", "y"]);
        // The name doubles as the constructor, so it renders as a callable.
        assert_eq!(ds[0].signature(), "(point x y)");
    }

    #[test]
    fn defrecord_derives_and_typed_fields_dont_leak_into_params() {
        // `:derives …` opts come after the field list and must not be read as fields.
        let ds = defs("(defrecord point (x y) :derives [Fields])");
        assert_eq!(ds[0].params, vec!["x", "y"]);
    }

    #[test]
    fn recognizes_defability_as_an_interface_by_name() {
        let ds = defs("(defability Shape :sealed [circle] (area [self] :-> float))");
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].kind, DefKind::Ability);
        assert_eq!(ds[0].name, "Shape");
        // The ability name isn't itself callable — it renders as a bare name.
        assert!(ds[0].params.is_empty());
        assert_eq!(ds[0].signature(), "Shape");
    }

    #[test]
    fn ignores_non_definitions() {
        assert!(defs("(io/puts \"hi\") 42").is_empty());
    }

    #[test]
    fn find_def_locates_a_nested_def() {
        // `helper` is defined inside a `do`, so it isn't a top-level form — but it
        // is still a global, and `find_def` must locate it by its name span.
        let src = "(do (defn helper (x) x))";
        let root: &'static Node = Box::leak(Box::new(cst::parse(src)));
        let src: &'static str = Box::leak(src.to_string().into_boxed_str());
        assert!(top_level(root, src).is_empty(), "not a top-level form");
        let name_span = Span::new(
            src.find("helper").unwrap(),
            src.find("helper").unwrap() + "helper".len(),
        );
        let d = find_def(root, src, name_span).expect("nested def found");
        assert_eq!(d.name, "helper");
        assert_eq!(d.signature(), "(helper x)");
    }
}
