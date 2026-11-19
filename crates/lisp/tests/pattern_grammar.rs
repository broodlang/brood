//! The Rust reading of the pattern grammar (`syntax::pattern`) and the prelude's pattern
//! compiler (`std/prelude/match.blsp`) describe one language. These hold them together: the
//! heads the compiler recognises are exactly `ListForm::NAMED`, and for a corpus covering
//! every form, the names Rust says a pattern binds are the names the compiler binds.
//!
//! A new pattern form fails here until both sides know it, and then fails to compile at
//! every Rust reader that matches `ListForm` until each handles it.

use brood::Interp;
use brood::syntax::pattern::{ListForm, Pattern};

/// The heads `match.blsp`'s shape predicates test for: every
/// `(defn- %match-…? (p) (and (pair? p) (= (first p) 'HEAD)))`.
fn compiler_heads() -> Vec<String> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../std/prelude/match.blsp");
    let source = std::fs::read_to_string(path).expect("read match.blsp");
    let lines: Vec<&str> = source.lines().collect();
    let mut heads = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let Some(predicate) = line.strip_prefix("(defn- %match-") else {
            continue;
        };
        // The formatter may break a short definition after its parameter list.
        let next = match lines.get(index + 1) {
            Some(next) if next.starts_with(' ') => next.trim(),
            _ => "",
        };
        let predicate = format!("{predicate} {next}");
        let Some((name, body)) = predicate.split_once(' ') else {
            continue;
        };
        if !name.ends_with('?') {
            continue;
        }
        for argument in ["p", "pat"] {
            let test = format!("(and (pair? {argument}) (= (first {argument}) '");
            if let Some(rest) = body.split_once(&test).map(|(_, rest)| rest) {
                let head: String = rest.chars().take_while(|c| *c != ')').collect();
                heads.push(head);
            }
        }
    }
    heads.sort();
    heads
}

#[test]
fn the_compiler_recognises_exactly_the_named_heads() {
    let mut named: Vec<String> = ListForm::NAMED
        .iter()
        .map(|(head, _)| head.to_string())
        .collect();
    named.sort();
    let found = compiler_heads();
    assert!(
        found.len() >= 8,
        "the scan found too few heads to be reading the file: {found:?}"
    );
    assert_eq!(
        found, named,
        "match.blsp's pattern heads and ListForm::NAMED differ: add the form to both, and \
         handle it at every reader that matches ListForm"
    );
}

/// Every form, with the shapes inside each that bind differently.
const CORPUS: &[&str] = &[
    "_",
    "x",
    "42",
    "\"s\"",
    ":k",
    "nil",
    "'sym",
    "^outer",
    "(a b c)",
    "(a b & rest)",
    "(a &optional b (c 0) & rest)",
    "(a &optional ((b c) nil))",
    "((a b) [c d] {:keys [e]})",
    "[a b c]",
    "[a [b c] _]",
    "[x x]",
    "{:keys [a b] :or {a 1}}",
    "{:k v :keys [w]}",
    "{:k [a b]}",
    "(or [:a x] [:b x])",
    "(or 1 2)",
    "(and whole {:keys [a]})",
    "(and [a b] all)",
    "(record point)",
    "(record point {:keys [x y]})",
    "(record geo/circle {:r radius})",
    "(bytes a b)",
    "(bytes 0 _ len (payload len) & tail)",
    "(bytes (n :u16) (_ :u32) (m :i8-le) #b\"ab\" & _)",
    "(str \"GET \" path)",
    "(str stem \".blsp\")",
    "(str \"<\" (or \"a\" \"b\") \">\")",
    "(str \"exact\")",
    "(str \"v\" (and all tail))",
    "(str ^separator rest)",
    "(str \"#\" ^expected)",
    "(unquote x)",
];

#[test]
fn the_corpus_covers_every_form() {
    let mut interp = Interp::new();
    let mut covered = Vec::new();
    for source in CORPUS {
        let pattern = read(&mut interp, source);
        if let Some(items) = interp.heap.list_items(pattern) {
            covered.push(ListForm::of_list(&items));
        }
    }
    for form in ListForm::NAMED
        .iter()
        .map(|&(_, form)| form)
        .chain([ListForm::List])
    {
        // `(not …)` binds differently in the two readers (the compiler refuses it before it
        // binds anything), so it is pinned by its own test rather than compared.
        if form == ListForm::Not {
            continue;
        }
        assert!(
            covered.contains(&form),
            "no corpus pattern exercises {form:?}"
        );
    }
}

#[test]
fn rust_binders_are_the_compilers_pattern_vars() {
    let mut interp = Interp::new();
    for source in CORPUS {
        let pattern = read(&mut interp, source);
        let mut rust: Vec<String> = Pattern::parse(&interp.heap, pattern)
            .binders()
            .into_iter()
            .map(brood::core::value::symbol_name)
            .collect();
        rust.sort();
        let joined = interp
            .eval_str(&format!(
                "(string/join (sort (map (%pattern-vars '{source}) ->string)) \" \")"
            ))
            .unwrap_or_else(|error| panic!("%pattern-vars of {source}: {error:?}"));
        let brood::core::value::Value::Str(id) = joined else {
            panic!("%pattern-vars of {source} did not join to a string");
        };
        let brood_names = interp.heap.string(id).to_string();
        let rust_names = rust.join(" ");
        assert_eq!(rust_names, brood_names, "the binders of {source}");
    }
}

#[test]
fn a_form_the_compiler_refuses_binds_nothing() {
    let mut interp = Interp::new();
    for source in [
        "(not x)",
        "[a & rest]",
        "[a &optional b]",
        "{:as whole}",
        "(record p {} junk)",
        "(str key \"=\" value)",
        "(str \"GET \" & path)",
    ] {
        let pattern = read(&mut interp, source);
        let parsed = Pattern::parse(&interp.heap, pattern);
        assert!(
            matches!(parsed, Pattern::Refused(_)),
            "{source} parsed as {parsed:?}"
        );
        let compiled = interp.eval_str(&format!("(match nil ({source} 1) (_ 0))"));
        assert!(compiled.is_err(), "the compiler accepted {source}");
    }
}

fn read(interp: &mut Interp, source: &str) -> brood::core::value::Value {
    let forms = brood::syntax::reader::read_all(&mut interp.heap, source).expect("read");
    *forms.first().expect("one form")
}
