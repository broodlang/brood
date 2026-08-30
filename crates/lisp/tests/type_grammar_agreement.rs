//! The type grammar is implemented **twice**, and this makes the two agree.
//!
//! A `(sig …)` is read by the Rust checker (`types::check::annot::parse_type` plus the
//! lattice relations) and, under `sig!` / `BROOD_CONTRACTS=1`, by the Brood runtime
//! (`type-matches?` in `std/prelude/core.blsp`). Both answer the same question — *does
//! this value inhabit this type* — through completely separate code, and nothing made
//! them answer it the same way.
//!
//! That gap is not hypothetical. ADR-264 made a record **closed**, which had to land in
//! both: the checker's `RecordShape.rest` and the runtime's extra-key scan. For a few
//! hours the tree carried one half of it — the contract rejected an undeclared key while
//! `nest check` said nothing about the same declaration. A drift like that is invisible
//! to every other gate, because each implementation is internally consistent.
//!
//! The property asserted: for a literal value, whose *static* type the checker knows
//! exactly, `type-matches?` agrees with `is_subtype`. Literals are the whole of it —
//! anything whose static type is an over-approximation would legitimately differ, and
//! the point is to catch drift, not to re-litigate approximation.
//!
//! **Arrows are excluded, deliberately.** A `fn` literal's static type is bare `fn` (no
//! arrow refinement is inferrable from a value), so `(int -> int)` is statically not a
//! supertype of it while the runtime — which cannot inspect a closure's domain — accepts
//! any function. Both are right; the disagreement is structural, not drift.

use brood::types::check::annot::parse_type;
use brood::types::check::expr_ty_of;
use brood::Interp;

/// `(type-expr, value-expr, expected)` — `expected` is what BOTH implementations must say.
const CASES: &[(&str, &str, bool)] = &[
    // base types
    ("int", "5", true),
    ("int", "\"s\"", false),
    ("string", "\"s\"", true),
    ("number", "1.5", true),
    ("number", "5", true),
    ("nil", "nil", true),
    ("bool", "true", true),
    ("any", "5", true),
    ("keyword", ":k", true),
    // unions and intersections
    ("(or int string)", "5", true),
    ("(or int string)", "\"s\"", true),
    ("(or int string)", ":k", false),
    ("(and number int)", "5", true),
    ("(and number int)", "1.5", false),
    // the complement (ADR-263)
    ("(not nil)", "5", true),
    ("(not nil)", "nil", false),
    // Negative LITERAL atoms — the complement of a literal, which the lattice now holds
    // exactly (an equality guard's else branch produces these). The runtime matcher has
    // to agree, or a `sig!` contract and the static check disagree about the same type.
    ("(not :ok)", ":err", true),
    ("(not :ok)", ":ok", false),
    ("(not :ok)", "5", true),
    ("(not 5)", "6", true),
    ("(not 5)", "5", false),
    ("(and keyword (not :ok))", ":err", true),
    ("(and keyword (not :ok))", ":ok", false),
    ("(and keyword (not :ok))", "5", false),
    ("(and any (not nil))", "\"s\"", true),
    // literal singletons
    (":ok", ":ok", true),
    (":ok", ":no", false),
    ("(or :ok :err)", ":err", true),
    ("5", "5", true),
    ("5", "6", false),
    ("true", "true", true),
    ("\"GET\"", "\"GET\"", true),
    ("\"GET\"", "\"POST\"", false),
    // element-typed sequences
    ("(vector int)", "[1 2]", true),
    ("(vector int)", "[1 \"s\"]", false),
    ("(vector int)", "[]", true),
    ("(list int)", "(list 1 2)", true),
    // tuples (ADR-128)
    ("(tuple int string)", "[1 \"s\"]", true),
    ("(tuple int string)", "[\"s\" 1]", false),
    ("(tuple int string)", "[1]", false),
    ("(tuple)", "[]", true),
    // maps
    ("(map int keyword)", "{:a 1}", true),
    ("(map int keyword)", "{:a \"s\"}", false),
    // records — CLOSED by default (ADR-264), which is the half that drifted
    ("(record :a int)", "{:a 1}", true),
    ("(record :a int)", "{:a \"s\"}", false),
    ("(record :a int)", "{}", false),
    ("(record :a int)", "{:a 1 :b 2}", false),
    ("(record &open :a int)", "{:a 1 :b 2}", true),
    ("(record &open :a int)", "{:b 2}", false),
    ("(record :a (optional int))", "{}", true),
    ("(record :a (optional int))", "{:a 1}", true),
    ("(record :a (optional int))", "{:a \"s\"}", false),
    ("(record :a int :b string)", "{:a 1 :b \"s\"}", true),
    // nested: a record's field type is itself closed
    ("(record :a (record :b int))", "{:a {:b 1}}", true),
    ("(record :a (record :b int))", "{:a {:b 1 :c 2}}", false),
];

#[test]
fn the_checker_and_the_runtime_contract_agree() {
    let mut interp = Interp::new();
    let mut drift = Vec::new();
    for (ty_src, val_src, expected) in CASES {
        // The runtime's answer, through the same primitive a `sig!` contract calls.
        let runtime = interp
            .eval_str(&format!("(type-matches? '{ty_src} {val_src})"))
            .unwrap_or_else(|e| panic!("`(type-matches? '{ty_src} {val_src})`: {e:?}"));
        let runtime = !matches!(runtime, brood::core::value::Value::Nil)
            && !matches!(runtime, brood::core::value::Value::Bool(false));

        // The checker's answer: the value's exact static type against the parsed type.
        let ty_form = brood::syntax::reader::read_one(&mut interp.heap, ty_src)
            .unwrap_or_else(|e| panic!("`{ty_src}` does not read: {e:?}"));
        let ty = parse_type(&interp.heap, ty_form)
            .unwrap_or_else(|| panic!("`{ty_src}` is not a type expression"));
        let val_form = brood::syntax::reader::read_one(&mut interp.heap, val_src)
            .unwrap_or_else(|e| panic!("`{val_src}` does not read: {e:?}"));
        let val_ty = expr_ty_of(&interp.heap, val_form)
            .unwrap_or_else(|| panic!("no static type for the literal `{val_src}`"));
        let checker = val_ty.is_subtype(&ty);

        if runtime != *expected || checker != *expected {
            drift.push(format!(
                "`{ty_src}` vs `{val_src}`: expected {expected}, runtime said {runtime}, \
                 checker said {checker} (static type `{val_ty}`)"
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "the two implementations of the type grammar disagree, or with the expectation:\n{}",
        drift.join("\n")
    );
}
