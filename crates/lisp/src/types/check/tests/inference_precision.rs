//! Where inference is exact and where it defers: the division residue, ratio arithmetic, element types, literal conditions, and the positions expanded code carries.

use super::*;

#[test]
fn integer_division_yields_int_or_ratio() {
    // `/` is the one contagious operator that is not int-closed, and the rule used to fall
    // through to `None` — so the checker made NO claim about `(/ x 2)`, the most ordinary
    // arithmetic expression there is. Brood's division is exact: an `int` when it divides
    // evenly, a `ratio` when it does not, never a float. Declaring `float` over it is
    // therefore provably wrong, and so is feeding it somewhere non-numeric.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig c (int -> float))\n\
         (defn c (x) (/ x 2))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("declared return type float") && w.contains("int | ratio")),
        "{ws:?}"
    );
}

// The decidable half of the division residue. `(/ x 2)` is genuinely `int | ratio` and
// stays so — but four shapes the roadmap listed as blockers for ever flagging that residue
// are not undecidable at all, they were merely unread: a literal ±1 divisor, and operands
// whose int-literal sets the checker already carries (ADR-117). Narrow first, flag second.
#[test]
fn a_decidable_division_types_exactly() {
    // every operand a known int literal: fold it
    assert_eq!(ty_str("(/ 6 3)"), "int");
    assert_eq!(ty_str("(/ 5 2)"), "ratio");
    assert_eq!(ty_str("(/ 6 3 2)"), "int");
    assert_eq!(ty_str("(/ 6 4 2)"), "ratio");
    // unary `/` is the reciprocal, so its single operand is the DIVISOR
    assert_eq!(ty_str("(/ 2)"), "ratio");
    assert_eq!(ty_str("(/ 1)"), "int");
    // a literal ±1 divisor keeps the numerator's kind, whatever the numerator is
    assert_eq!(ty_str("(let (x (+ 1 2)) (/ x 1))"), "int");
    assert_eq!(ty_str("(let (x (+ 1 2)) (/ x -1))"), "int");
    assert_eq!(ty_str("(/ 3/2 1)"), "ratio");
    // …and the literal itself is NOT carried through: `(/ 6 -1)` is -6, not 6
    assert_eq!(ty_str("(/ 6 -1)"), "int");
    // what stays deferred: an unknown numerator over a divisor that is not ±1
    assert_eq!(ty_str("(let (x (+ 1 2)) (/ x 2))"), "ratio"); // `x` is exactly 3 now
                                                              // a literal SET that lands on both kinds is exactly `int | ratio` — the answer the
                                                              // caller already gives, so the fold stops rather than walking the rest of it
    assert_eq!(ty_str("(/ (if (os/env \"X\") 6 5) 2)"), "int | ratio");
    // a zero divisor RAISES (E0040), so the checker declines rather than typing an
    // expression that cannot produce a value
    assert_eq!(ty_str("(/ 6 0)"), "int | ratio");
    // float contagion is still checked first
    assert_eq!(ty_str("(/ 6.0 3)"), "float");
}

// What the narrowing is FOR, in both directions: the bug it can now name, and the two
// correct programs it stops holding an unprovable union over.
#[test]
fn narrowing_division_names_a_bug_and_clears_two_correct_programs() {
    // `/` is exact, not integer division — the mistake a newcomer brings from another
    // language. `(/ 5 2)` is 5/2, and declaring `int` over it is now provably wrong.
    let ws = file_warnings("(defmodule t)\n(sig c (int -> int))\n(defn c (x) (/ 5 2))");
    assert!(
        ws.iter()
            .any(|w| w.contains("declared return type int") && w.contains("ratio")),
        "{ws:?}"
    );
    // …while the decidable-int cases stop being merely-wider and go silent
    let ws = file_warnings("(defmodule t)\n(sig d (int -> int))\n(defn d (x) (/ x 1))");
    assert!(ws.is_empty(), "{ws:?}");
    let ws = file_warnings("(defmodule t)\n(sig e (int -> int))\n(defn e (x) (/ 6 3))");
    assert!(ws.is_empty(), "{ws:?}");
    // the genuinely undecidable one stays silent too — this is the residue, unchanged
    let ws = file_warnings("(defmodule t)\n(sig f (int -> int))\n(defn f (x) (/ x 2))");
    assert!(ws.is_empty(), "{ws:?}");
}

// The two `seqable` members that carried no element type. Both are decided by the KIND
// rather than by a refinement, which is why neither had one: a `bytes` is a sequence of
// octets, and a map walks as its `[key value]` entries (a two-element VECTOR — checked
// against the runtime, not assumed).
#[test]
fn bytes_and_map_entries_carry_their_element_types() {
    assert_eq!(
        ty_str("(first (string/->bytes \"ab\"))"),
        "nil | int[0..255]"
    ); // an octet
    assert_eq!(
        ty_str("(map (string/->bytes \"ab\") inc)"),
        "nil | list<int[1..256]>"
    );
    // a closed record states its keys, so its entries are exact
    assert_eq!(ty_str("(first {:a 1 :b 2})"), "(tuple :a | :b, 1 | 2)");
    assert_eq!(ty_str("(nth (first {:a 1 :b 2}) 0)"), ":a | :b");
    assert_eq!(
        ty_str("(map {:a 1 :b 2} (fn (kv) (first kv)))"),
        "list<:a | :b>"
    );
    // `{}` has no entries, and says so: no key type inhabits it, and it is not
    // provably non-empty either
    assert_eq!(ty_str("(first {})"), "nil | vector<never>");
    // A vector literal is a TUPLE, whose arity is part of its type — so it is provably
    // non-empty, the same length fact a `list<T>` carries, and `first` drops the `nil`.
    assert_eq!(ty_str("(first [1 2])"), "1");
}

// `nil` is the EMPTY case, and a union with one does not stop carrying the shape it was
// unioned with — so "this has a first element" has to be read off a type that is ONLY that
// collection, not off the shape alone. Reading the shape alone made `(first (if p {:a 1}
// nil))` answer `(tuple :a, 1)` and silently drop the `nil` every caller has to handle.
#[test]
fn a_nil_union_is_not_provably_non_empty() {
    assert_eq!(
        ty_str("(first (if (bound? 'x) {:a 1} nil))"),
        "nil | vector<:a | 1>"
    );
    assert_eq!(ty_str("(first (if (bound? 'x) [1 2] nil))"), "1 | 2 | nil");
    // …while the collection on its own still carries the fact
    assert_eq!(ty_str("(first {:a 1})"), "(tuple :a, 1)");
    assert_eq!(ty_str("(first [1 2])"), "1");
}

// An OPEN record may carry keys nothing declares, so "these are the entries" does not
// hold for one — the same gate every other closed-shape rule needs. It is also the gate
// that keeps a NOMINAL record out: a record is modelled open, and one may implement
// Seqable, in which case it walks as whatever that impl yields rather than as entries
// (`tests/queue_test.blsp` maps over a queue — the checker gate caught this, not a
// unit test, which is why the rule is narrower than the first cut).
#[test]
fn an_open_record_yields_no_entry_type() {
    // closed: the entry's value position is exactly the declared field type
    let ws = file_warnings(
        "(defmodule t)\n(sig g ((record :name string) -> int))\n(defn g (m) (nth (first m) 1))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("declared return type int") && w.contains("string")),
        "{ws:?}"
    );
    // open: the undeclared keys could be anything, so there is nothing to report
    let ws = file_warnings(
        "(defmodule t)\n(sig h ((record &open :name string) -> int))\n(defn h (m) (nth (first m) 1))",
    );
    assert!(ws.is_empty(), "{ws:?}");
}

// Ratios close over `+ - *` exactly as ints do: `(+ 1/2 1/2)` is 1 and `(* 2 1/2)` is 1.
// The case used to defer to `+`'s declared signature, which is widened to `number | map`
// for `Num` records — sound, and noise as the answer to an all-numeric expression.
#[test]
fn ring_arithmetic_over_ints_and_ratios_yields_int_or_ratio() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig c (int -> float))\n\
         (defn c (x) (* x 1/2))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("declared return type float") && w.contains("int | ratio")),
        "{ws:?}"
    );
    // …and it stays deferred in the merely-wider direction, like division does.
    let ws = file_warnings("(defmodule t)\n(sig d (int -> int))\n(defn d (x) (/ x 1/2))");
    assert!(ws.is_empty(), "{ws:?}");
    // a float operand still wins: contagion is checked first
    let ws = file_warnings("(defmodule t)\n(sig e (int -> int))\n(defn e (x) (+ x 1/2 0.5))");
    assert!(ws.iter().any(|w| w.contains("float")), "{ws:?}");
}

// A ratio SHIFTED by whole numbers is exactly a ratio — no `int` arm. `Ratio` is demoted to
// `Int` on construction when its denominator is 1 (`core::value`), so no ratio is integral,
// and `n ± p/q` is `(nq ± p)/q`, whose denominator is still `q`. The old answer, `int | ratio`,
// was sound and useless: it is the answer `(+ 1 1/2)` — which is 3/2 and cannot be anything
// else — got from a live evaluator asking the checker what it had just computed.
#[test]
fn adding_whole_numbers_to_one_ratio_is_exactly_a_ratio() {
    assert_eq!(ty_str("(+ 1 1/2)"), "ratio");
    assert_eq!(ty_str("(- 1 1/2)"), "ratio");
    assert_eq!(ty_str("(+ 1 2 1/2)"), "ratio");
    assert_eq!(ty_str("(inc 1/2)"), "ratio");
    assert_eq!(ty_str("(- 1/2)"), "ratio");
    // Two operands can each carry a denominator, and then they can cancel: `(+ 1/2 1/2)`
    // is the int 1. Multiplication cancels one against a whole number too — `(* 2 1/2)`.
    assert_eq!(ty_str("(+ 1/2 1/2)"), "int | ratio");
    assert_eq!(ty_str("(* 2 1/2)"), "int | ratio");
    assert_eq!(ty_str("(/ 1 1/2)"), "int | ratio");
    // and the rules above it still win in order: float contagion first, then int closure
    assert_eq!(ty_str("(+ 1.0 1/2)"), "float");
    assert_eq!(ty_str("(+ 1 2)"), "3"); // int closure, and the interval arithmetic makes it exact
                                        // A declared `int` over it is now provably wrong, where before it was merely wider.
    let ws = file_warnings("(defmodule t)\n(sig f (int -> int))\n(defn f (x) (+ x 1/2))");
    assert!(
        ws.iter()
            .any(|w| w.contains("declared return type int") && w.contains("ratio")),
        "{ws:?}"
    );
}

// Sound-but-uninformative answers an expression corpus turned up, each replaced by the
// exact type the value provably has. Every rule here is a tightening in the safe
// direction: the old answer contained the new one.
#[test]
fn precision_rules_give_the_exact_type_where_it_is_provable() {
    for (src, want) in [
        // ratios close over the ring, and `/` is exact over them
        ("(+ 1 2 1/2)", "ratio"),
        ("(- 1/2 1/2)", "int | ratio"),
        ("(/ 3/2 3)", "int | ratio"),
        // a decimal operand: every operand a number, nothing narrower provable — but never
        // the declared `number | map` (the `Num`-record widening) for numeric operands
        ("(+ 1 1.5M)", "number"),
        // a nil tail contributes no elements
        ("(cons 1 '())", "(list 1)"),
        ("(cons 1 nil)", "(list 1)"),
        // a quoted list is data with its elements in view — each in its position
        ("'(1 2)", "(list 1, 2)"),
        ("(vec '(1 2))", "vector<1 | 2>"),
        // a range is a range of integers, carrying its bounds' interval (never reaching 5)
        ("(range 5)", "list<int[0..4]>[5]"),
        // a numeric operator as a callback / a fold / spread — the same closure rules
        // a two-element vector literal is a TUPLE, whose arity is part of its type — so
        // it is provably non-empty and `map` keeps that (the `nil` arm is the empty case)
        ("(map [1 2] inc)", "list<int[2..3]>[2]"),
        ("(reduce [1 2] +)", "int"),
        ("(reduce [1 2] 0 +)", "int"),
        ("(apply + [1 2])", "int"),
        ("(reduce [1 2] 0.5 +)", "float"),
        // reshapers with no signature at all used to be `any`
        ("(vec [1 2])", "vector<1 | 2>"),
        ("(into [] (list 1))", "vector<1>[1]"),
        // …and onto a vector the length is the target's plus the source's, with an
        // unknown element too: the length is a fact about the input
        ("(into [0] (range 3))", "vector<int[0..2]>[4]"),
        (
            "(into [] (map (range 1000) (fn (i) (list i))))",
            "vector<(list int[0..999])>[1000]",
        ),
        ("(into {} [[:a 1]])", "map"),
        // a non-negative MASK bounds the conjunction whatever the other operand is
        ("(bit/and 7 1)", "int[0..1]"),
        ("(bit/and (- 7 20) 3)", "int[0..3]"),
        ("(bit/and (- 0 7) (- 0 1))", "int"),
        // a computed index whose interval fits a shape reads the positions it can name
        ("(nth [10 20] (bit/and 7 1))", "10 | 20"),
        ("(nth [10 20 30] (bit/and 7 1))", "10 | 20"),
        ("(nth (into [] (range 1000)) (bit/and 7 1))", "int[0..999]"),
        ("(conj [1] 2)", "vector<1 | 2>[2]"),
        ("(conj (list 1) 2)", "list<1 | 2>"),
        ("(conj #{1} 2)", "set<1 | 2>"),
        ("(merge {:a 1} {:b 2})", "{a: 1, b: 2}"),
        // control flow and application that had no type
        ("(try 1 (catch e 2))", "1 | 2"),
        ("(try 1 \"a\" (catch e \"b\"))", "\"a\" | \"b\""),
        ("((fn (x) (str x)) 1)", "string"),
        ("(->string :a)", "string"),
        ("(string/split \"a b\" \" \")", "list<string>"),
    ] {
        assert_eq!(ty_str(src), want, "{src}");
    }
}

// The two shapes the `tests/` strict sweep of 2026-09-17 closed: a vector built by `into`
// from a counted source is exactly that long whatever its elements (an untyped callback
// used to drop the length with the element), and a `:keys`/`:or` binder is the default
// where the key is absent, never `nil` — the pattern lowers to `(get m k default)`, the one
// lookup the `get` rule reads exactly (the `(if (contains? …) …)` around it read `40 | nil`).
#[test]
fn a_counted_build_and_an_or_default_are_present() {
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig f ((int -> bytes) -> int))\n\
         (defn f (mk) (let (xs (into [] (map (range 1000) mk))) (bytes/at (nth xs 999) 2)))\n\
         (defn g () (let ({:keys [a b] :or {b 40}} {:a 2}) (+ a b)))\n\
         (sig h ((map keyword int) -> int))\n\
         (defn h (m) (let ({:keys [b] :or {b 40}} m) (+ 1 b)))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    // …and an `:or` default does not paper over a PRESENT nil: the key is there, its value
    // is nil, and `get` answers it.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (defn g () (let ({:keys [b] :or {b 40}} {:b nil}) (+ 1 b)))",
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("+: argument 2 expects number, got nil")),
        "{ws:?}"
    );
}

// The expanded path too: `try` becomes `(%try (fn () a b) (fn (e) h))`, and a multi-form
// thunk used to be untypeable — so a checked file saw nothing where the unexpanded query did.
#[test]
fn a_multi_form_try_types_through_its_expansion() {
    let ws = file_warnings(
        "(sig s-int (int -> int))\n(defn s-int (x) x)\n(defn f () (s-int (try 1 \"a\" (catch e \"b\"))))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("expects int") && w.contains("\"a\"")),
        "{ws:?}"
    );
}

// …and the rules must NOT claim more than they can prove: the operands that defeat each
// closure keep the wider answer.
#[test]
fn precision_rules_defer_where_nothing_narrower_is_provable() {
    for (src, want) in [
        ("(+ 1 2.5)", "float"),           // contagion wins over the ring
        ("(quot 7/2 2)", "<unknown>"),    // quot is int-only: a ratio operand defers
        ("(vec (identity 1))", "vector"), // unknown elements → a bare vector
        ("(range 0 1 0.5)", "list"),      // a non-int argument: only range's own inferred type
    ] {
        assert_eq!(ty_str(src), want, "{src}");
    }
}

// A `defrecord` constructor's signature used to declare every undeclared field `any`, so
// `(pt 1 2)` was the bare id type and `(:x (pt 1 2))` was `any` — a record you had just
// built with `1` in it could not be typed at all. The signature now carries a type variable
// per undeclared field (`?x`), bound at each call.
#[test]
fn a_record_constructor_call_carries_its_argument_types_in_its_fields() {
    let src = "(defrecord pt (x y))\n(list (:x (pt 1 2)) (:y (pt 1 \"s\")))";
    let x = arg_ty_of(src, "(list", 1).expect("typed");
    let y = arg_ty_of(src, "(list", 2).expect("typed");
    assert_eq!(x.to_string(), "1");
    assert_eq!(y.to_string(), "\"s\"");
    // …and a declared field type is still a contract, not a variable
    let ws = file_warnings("(defrecord money ((amount int) cur))\n(defn m () (money \"s\" :usd))");
    assert!(
        ws.iter().any(|w| w.contains("money") && w.contains("int")),
        "{ws:?}"
    );
}

// The design behind that guarantee (ADR-297): synthetic code is located at the form it was
// expanded from. A `match`'s expansion ends in a `throw` the reader never saw; after
// expansion it carries the `match`'s own position — and is marked synthetic, so a lint that
// must speak only about the user's text (the unused-`let` exemption) can still tell.
#[test]
fn expanded_code_carries_the_position_of_the_form_it_came_from_and_is_marked_synthetic() {
    let mut interp = crate::Interp::new();
    let src = "(defn f (x) (match x (:a 1)))";
    let (form, _) = reader::read_all_positioned(&mut interp.heap, src)
        .expect("parse")
        .into_iter()
        .next()
        .expect("one form");
    let env = interp.heap.global();
    let expanded =
        crate::eval::macros::macroexpand_all(&mut interp.heap, form, env).expect("expands");
    // find a `throw` anywhere in the expansion
    fn find_throw(heap: &crate::core::heap::Heap, v: Value) -> Option<Value> {
        let items = super::walk::list_items(heap, v)?;
        if matches!(items.first(), Some(Value::Sym(s)) if value::symbol_is(*s, "throw")) {
            return Some(v);
        }
        items.iter().find_map(|&i| find_throw(heap, i))
    }
    let throw = find_throw(&interp.heap, expanded).expect("a match expands to a throw");
    assert!(
        interp.heap.form_pos_only(throw).is_some(),
        "the throw has a position"
    );
    assert!(interp.heap.is_synthetic(throw), "…and is marked synthetic");
    // the defn itself was read, so it is NOT synthetic, and neither is the user's `x`
    assert!(!interp.heap.is_synthetic(expanded));
}

// Under `(defmodule …)` every form is rebuilt for namespace rooting, and a rebuild must carry
// the synthetic mark with the position — it once copied the position alone, and every
// destructured name in every module of a project read as an "unused let binding".
#[test]
fn a_generated_let_stays_exempt_from_the_unused_lint_inside_a_module() {
    for src in [
        "(defmodule zz)\n(defn h3 (v) (let ([a b] v) a))",
        "(defmodule zz)\n(defn h2 (r) (let ([x y w h] r s 1) (+ w h)))",
        "(defmodule zz)\n(defn h1 (m) (match m ([:ok v] 1) (_ 2)))",
    ] {
        let ws = file_warnings(src);
        assert!(
            !ws.iter().any(|w| w.contains("unused let binding")),
            "{src}: {ws:?}"
        );
    }
    // …while a plain unused binding the user wrote is still reported, module or not
    let ws = file_warnings("(defmodule zz)\n(defn g (v) (let (x 1) 2))");
    assert!(
        ws.iter().any(|w| w.contains("unused let binding: x")),
        "{ws:?}"
    );
}

// Generated code sits on the most specific honest line: a `match` clause's expansion holds
// the clause's own body (a form the user wrote), so an unreachable-clause warning lands on
// the CLAUSE, not on the `match`. (Item 1 of the 2026-08-29 review.)
#[test]
fn a_warning_inside_a_match_clause_points_at_the_clause() {
    let src = "(defn dead (x)\n  (match x\n    (:a (inc 1))\n    (:a (inc 2))\n    (_ 3)))";
    let mut interp = crate::Interp::new();
    let forms = reader::read_all_positioned(&mut interp.heap, src)
        .expect("parse")
        .into_iter()
        .map(|(f, _)| f)
        .collect::<Vec<_>>();
    let ws = super::check_file(&mut interp.heap, &forms);
    let (pos, msg) = ws
        .iter()
        .find(|(_, m)| m.contains("unreachable clause"))
        .expect("the duplicate :a clause is reported");
    let pos = pos.expect("positioned");
    assert!(
        pos.line >= 3,
        "expected a clause line (≥3), got line {} for {msg}",
        pos.line
    );
}

// Every warning must be somewhere: a lint over the macro-EXPANDED tree (match exhaustiveness,
// an unreachable clause, an argument inside a destructuring `let`) reported at a pair the
// reader never positioned, and printed as `file: warning: …` with nothing to jump to.
#[test]
fn every_warning_carries_a_position() {
    let src = "(sig s-str (string -> string))\n(defn s-str (x) x)\n\
               (sig e1 ((or :a :b :c) -> int))\n(defn e1 (x) (match x (:a 1) (:b 2)))\n\
               (defn dead (x) (match x (:a 1) (:a 2) (_ 3)))\n\
               (defn p1 (v) (let ([a b] v) (s-str (+ a b))))";
    let mut interp = crate::Interp::new();
    let forms = reader::read_all_positioned(&mut interp.heap, src)
        .expect("parse")
        .into_iter()
        .map(|(f, _)| f)
        .collect::<Vec<_>>();
    let ws = super::check_file(&mut interp.heap, &forms);
    assert!(ws.len() >= 3, "{ws:?}");
    for (pos, msg) in &ws {
        assert!(pos.is_some(), "positionless warning: {msg}");
    }
}

// A literal condition selects its branch: `(if true 1 "a")` is `1`, and passing it where an
// int is wanted is right, not a warning. (`(if c 1 "a")` for an unknown `c` stays `1 | "a"`.)
#[test]
fn a_literal_if_condition_folds_to_its_branch() {
    assert_eq!(ty_str("(if true 1 \"a\")"), "1");
    assert_eq!(ty_str("(if nil 1 \"a\")"), "\"a\"");
    assert_eq!(ty_str("(if false 1)"), "nil");
    let ws = file_warnings(
        "(sig s-int (int -> int))\n(defn s-int (x) x)\n(defn l1 () (s-int (if true 1 \"a\")))",
    );
    assert!(ws.is_empty(), "{ws:?}");
}

#[test]
fn integer_division_does_not_flag_the_merely_wider_case() {
    // The residue that stays deferred (ADR-011): a body of `int | ratio` DECLARED `int` is
    // right whenever the numerator is even, and proving that needs range analysis. Adding
    // the division rule must not smuggle in the false positive that deferral exists to
    // avoid — nor flag `number`, which `int | ratio` genuinely is.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig a (int -> int))\n\
         (defn a (x) (/ x 2))\n\
         (sig b (int -> number))\n\
         (defn b (x) (/ x 2))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("declared return type")),
        "{ws:?}"
    );
}

#[test]
fn division_with_a_float_operand_is_still_contagious() {
    // The pre-existing contagion rule must win over the new one: one float operand makes the
    // result a float, not `int | ratio`.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig f (int -> float))\n\
         (defn f (x) (/ x 2.0))\n\
         (sig g (int -> int))\n\
         (defn g (x) (/ x 2.0))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("g:") && w.contains("yields float")),
        "the int declaration over a float body must warn — {ws:?}"
    );
    assert!(
        !ws.iter().any(|w| w.contains("t/f:")),
        "the float declaration is correct — {ws:?}"
    );
}

// A vector literal keeps its arity whatever its elements are: `[row col]` over untyped
// params is `(tuple any any)`, which a `(tuple int int)` parameter accepts (the unknown
// slots read gradually) and a 3-tuple parameter rejects. It used to fall back to a bare
// `vector` on one unknown element, which threw the arity away with it.
#[test]
fn a_vector_literal_keeps_its_arity_over_unknown_elements() {
    assert_eq!(ty_str("(fn (r c) [r c])"), "(any, any) -> (tuple any, any)");
    let src = "\
         (defmodule t)\n\
         (sig at ((tuple int int) -> int))\n\
         (defn at (p) (first p))\n\
         (sig at3 ((tuple int int int) -> int))\n\
         (defn at3 (p) (first p))\n\
         (defn ok (row col) (at [row col]))\n\
         (defn bad (row col) (at3 [row col]))";
    let strict = file_warnings_mode(src, true);
    assert!(!strict.iter().any(|w| w.contains("t/at:")), "{strict:?}");
    assert!(
        strict
            .iter()
            .any(|w| w
                .contains("t/at3: argument 1 expects (tuple int, int, int), got (tuple any, any)")),
        "{strict:?}"
    );
}

// `(assoc x :k v)` on an unknown `x` is a `map`, not `vector | map`: a keyword key on a
// vector raises, so a keyword-keyed call that returns at all returns a map. An int key
// keeps the honest `vector | map`. This is every `(assoc (step m) :k v)` in an editor's
// `model -> model` chain, and the strict finding at each of them.
#[test]
fn assoc_with_keyword_keys_on_an_unknown_receiver_is_a_map() {
    assert_eq!(ty_str("(fn (x) (assoc x :k 1))"), "(any) -> map");
    assert_eq!(ty_str("(fn (x) (assoc x 0 1))"), "(any) -> vector | map");
    let src = "\
         (defmodule t)\n\
         (defn step (m) (assoc m :n 1))\n\
         (sig f (map -> map))\n\
         (defn f (m) (assoc (step m) :k 2))";
    let strict = file_warnings_mode(src, true);
    assert!(strict.is_empty(), "{strict:?}");
}

// `update` / `assoc-in` / `update-in` keep a record shape the way `assoc` and `dissoc`
// do: the named field becomes unknown, the others keep their types, and on an unknown
// receiver with a keyword key the answer is `map`.
#[test]
fn update_and_assoc_in_keep_a_record_shape() {
    assert_eq!(ty_str("(fn (x) (update x :k inc))"), "(any) -> map");
    assert_eq!(ty_str("(fn (x) (assoc-in x [:a :b] 1))"), "(any) -> map");
    let src = "\
         (defmodule t)\n\
         (deftype st (record &open :n int :name string))\n\
         (sig bump (st -> st))\n\
         (defn bump (s) (update s :n inc))\n\
         (sig nest (st -> st))\n\
         (defn nest (s) (assoc-in s [:meta :seen] true))\n\
         (sig deep (st -> st))\n\
         (defn deep (s) (update-in s [:meta :count] inc))\n\
         (sig name-of (st -> string))\n\
         (defn name-of (s) (:name (update s :n inc)))";
    let strict = file_warnings_mode(src, true);
    assert!(strict.is_empty(), "{strict:?}");
}

// An extremum over an operand the checker cannot type is the UNKNOWN, not the sig's
// `ordered`: `(math/max 1 s)` IS `s` or `1`, and the same `s` that passes into an `int`
// parameter untouched must not come back positively `ordered` for having been clamped.
// Arithmetic is different — `(dec s)` computes a new value that is positively a `number`.
#[test]
fn an_extremum_over_an_unknown_operand_is_the_unknown() {
    let src = "\
         (defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (n) n)\n\
         (defn clamp (s) (want-int (math/max 1 s)))\n\
         (defn shift (s) (want-int (dec s)))\n\
         (defn known (a b) (want-int (math/max 1.5 2)))";
    let strict = file_warnings_mode(src, true);
    assert!(
        !strict
            .iter()
            .any(|w| w.contains("t/want-int") && w.contains("ordered")),
        "{strict:?}"
    );
    assert!(
        strict
            .iter()
            .any(|w| w.contains("t/want-int: argument 1 expects int, got number")),
        "{strict:?}"
    );
    assert!(
        strict.iter().any(
            |w| w.contains("t/want-int: argument 1 expects int, got 2 | float")
                || w.contains("t/want-int: argument 1 expects int, got float | 2")
        ),
        "{strict:?}"
    );
}

// A DYNAMIC key on a KNOWN map receiver still answers a map — `(update m (:key spec) …)`
// on a record is a map (which field changed is unknown, so the shape goes; the map-ness
// does not), never `update`'s own `vector | map`.
#[test]
fn a_dynamic_key_on_a_known_map_keeps_it_a_map() {
    assert_eq!(ty_str("(fn (k) (update {:a 1} k inc))"), "(any) -> map");
    assert_eq!(
        ty_str("(fn (k) (assoc {:a 1} k 2))"),
        "(any) -> map<any, 1 | 2>"
    );
    assert_eq!(
        ty_str("(fn (m k) (update m k inc))"),
        "(any, any) -> vector | map"
    );
}

// ---- list shapes (2026-09-13): `(list a b)` is a positional list, not `list<A | B>` ----

#[test]
fn a_list_call_and_a_quoted_list_are_positional_shapes() {
    assert_eq!(ty_str("(list 1 \"s\")"), "(list 1, \"s\")");
    // (a quoted string reads through the heap-free `of_value`: its tag, not its content)
    assert_eq!(ty_str("'(1 \"s\")"), "(list 1, string)");
    assert_eq!(ty_str("(list)"), "nil");
    // Reading them: `first`/`second`/`nth` by position, `rest` the tail shape, `count`
    // the arity, `cons` one longer — and `first` of a two-shape is exactly the first,
    // never `nil | …`.
    assert_eq!(ty_str("(first (list {:a 1} '(x)))"), "{a: 1}");
    assert_eq!(ty_str("(second (list {:a 1} '(x)))"), "(list symbol)");
    assert_eq!(ty_str("(nth (list 1 \"s\") 1)"), "\"s\"");
    assert_eq!(ty_str("(nth (list 1 \"s\") 5)"), "nil");
    assert_eq!(ty_str("(rest (list 1 \"s\"))"), "(list \"s\")");
    assert_eq!(ty_str("(rest (list 1))"), "nil");
    assert_eq!(ty_str("(rest [])"), "nil");
    assert_eq!(ty_str("(but-last [1])"), "nil");
    assert_eq!(ty_str("(count (list 1 \"s\"))"), "2");
    assert_eq!(ty_str("(cons :k (list 1 \"s\"))"), "(list :k, 1, \"s\")");
    assert_eq!(ty_str("(cons :k nil)"), "(list :k)");
    // Destructuring binds by position, and `& rest` to the remaining positions.
    assert_eq!(ty_str("(let ((a b) (list 1 \"s\")) b)"), "\"s\"");
    assert_eq!(
        ty_str("(let ((a & more) (list 1 \"s\" :k)) more)"),
        "(list \"s\", :k)"
    );
    // The strict finding this was built for: a two-element list built as `(list m '(…))`
    // read `pair | map` at `first`.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (defn lm-quoted (acc) (list acc '(%map-get acc 1)))\n\
         (defn use-it () (%map-get (first (lm-quoted {})) :a))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
}

// ---- lengths and indices (ADR-350) --------------------------------------------------

/// A comparison on a variable's `count` is a fact about the collection's LENGTH, and a
/// positional read within that length is present: `(nth words 1)` under `(>= n 4)` is a
/// string, not `nil | string`.
#[test]
fn a_count_comparison_bounds_the_collection_length() {
    let src = "(defmodule t)\n\
         (sig want-str (string -> int))\n\
         (defn want-str (s) 1)\n\
         (sig f ((list string) -> any))\n\
         (defn f (words) (let (n (count words)) (if (>= n 4) (want-str (nth words 1)) 0)))\n\
         (sig g ((list string) -> any))\n\
         (defn g (words) (if (= (count words) 3) (want-str (nth words 1)) 0))\n\
         (sig h ((list string) -> any))\n\
         (defn h (words) (if (= (count words) 3) 0 (want-str (nth words 2))))";
    let ws = file_warnings_mode(src, true);
    // `f` and `g` are clean; `h` reads position 2 of a list whose length is NOT 3.
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(ws[0].contains("nil | string ((nth words 2))"), "{ws:?}");
}

/// `(not (empty? xs))` proves a length of at least one, so `(first xs)` is an element;
/// the then-branch of `(empty? xs)` proves nothing about `first` (it is nil).
#[test]
fn a_non_empty_guard_makes_first_present() {
    let src = "(defmodule t)\n\
         (sig want-map (map -> int))\n\
         (defn want-map (m) 1)\n\
         (sig f ((list map) -> any))\n\
         (defn f (ms) (if (not (empty? ms)) (want-map (first ms)) 0))\n\
         (sig g ((list map) -> any))\n\
         (defn g (ms) (if (empty? ms) (want-map (first ms)) 0))";
    let ws = file_warnings_mode(src, true);
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(ws[0].contains("got nil ((first ms))"), "{ws:?}");
}

/// An index proven below the count and at least zero reads an element, full stop.
#[test]
fn an_index_bounded_by_the_count_is_in_range() {
    let src = "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (n) 1)\n\
         (sig f ((vector int) (int 0 _) -> any))\n\
         (defn f (xs i) (if (< i (count xs)) (want-int (nth xs i)) 0))\n\
         (sig g ((vector int) int -> any))\n\
         (defn g (xs i) (if (< i (count xs)) (want-int (nth xs i)) 0))";
    let ws = file_warnings_mode(src, true);
    // `g`'s index may be negative, and a negative `nth` is nil.
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(ws[0].contains("nil | int ((nth xs i))"), "{ws:?}");
}

/// C13 / ADR-367 (2026-09-17) — why there is no float interval, pinned so a later attempt
/// has to face it. Floats here are NOT totally ordered: NaN is reachable (`(- inf inf)`,
/// `(* inf 0.0)`, `(/ inf inf)`, with `inf` itself from `(* 1.0e200 1.0e200)`, which does
/// not raise), and `(< nan 1.0)` and `(>= nan 1.0)` are BOTH false. So the else-branch rule
/// the int interval rests on — `¬(L < R) ⟹ L ≥ R` — does not hold for a float.
///
/// The current design is sound by NOT PARTICIPATING: `int_guard_ty` narrows to "an int
/// within the range, or not an int at all", so a float — NaN included — survives both
/// branches. Break that (narrow to the bare interval) and the second case below starts
/// claiming `int`, which is the unsound shortcut a float interval would invite.
#[test]
fn a_float_comparison_narrows_nothing_because_nan_fails_both() {
    // The else-branch of a float comparison proves nothing, so a float body is a float.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig a (float -> float))\n\
         (defn a (x) (if (< x 1.0) 0.0 x))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    // Over `number`, the else-branch must NOT be read as "then it is an int ≥ 1": the
    // value may be a float (or a NaN) that failed the comparison for another reason.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig b (number -> int))\n\
         (defn b (x) (if (< x 1) 0 x))",
        true,
    );
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(
        ws[0].contains("float"),
        "the else-branch must keep the non-int members: {ws:?}"
    );
    // …while the same comparison over a declared INT does narrow — the sound case the
    // interval exists for, and the one this must not cost.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig c (int -> int))\n\
         (defn c (x) (if (< x 1) 0 x))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    // And an int interval refuses a float outright — `(int 0 10)` is not "a number in
    // 0..10", which is the confusion a float interval would have to resolve.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig d ((int 0 10) -> (int 0 10)))\n\
         (defn d (x) x)\n\
         (defn e () (d 5.0))",
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("expects int[0..10], got float")),
        "{ws:?}"
    );
}

/// C12 (2026-09-17) — the index a SCAN writes: `(nth s (+ i 1))` under `(< (+ i 1) n)`.
/// The shape the corpora actually contain (`std/json.blsp` ×4, `std/ansi.blsp` ×2,
/// `std/url.blsp`), and before this a guard over `(+ i 1)` produced no facts at all — the
/// comparison was discarded whole, so it narrowed nothing and bounded nothing.
///
/// The fact is `i + k < (count xs)`, keyed on the base local and the offset, and a proved
/// offset covers every SMALLER one (`i + b ≤ i + k < n`) — which is what reads
/// `std/json.blsp`'s `\uXXXX` scan, guarded at `+10` and reading `+4`/`+5`.
#[test]
fn a_computed_index_is_bounded_by_a_guard_on_the_same_expression() {
    let src = "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (n) 1)\n\
         (sig a ((vector int) (int 0 _) -> any))\n\
         (defn a (s i) (if (< (+ i 1) (count s)) (want-int (nth s (+ i 1))) 0))\n\
         (sig b ((vector int) (int 0 _) -> any))\n\
         (defn b (s i) (let (n (count s)) (if (< (inc i) n) (want-int (nth s (inc i))) 0)))\n\
         (sig c ((vector int) (int 0 _) -> any))\n\
         (defn c (s i) (let (n (count s)) (if (<= (+ i 10) n) (want-int (nth s (+ i 4))) 0)))";
    assert!(file_warnings_mode(src, true).is_empty(), "{src}");
}

/// …and the two directions that must still warn, which are what keep the rule sound: a
/// read PAST the offset the guard proved, and a guard on a DIFFERENT collection.
#[test]
fn a_computed_index_past_its_guard_is_not_bounded() {
    let src = "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (n) 1)\n\
         (sig d ((vector int) (int 0 _) -> any))\n\
         (defn d (s i) (if (< (+ i 1) (count s)) (want-int (nth s (+ i 2))) 0))\n\
         (sig e ((vector int) (vector int) (int 0 _) -> any))\n\
         (defn e (s t i) (if (< (+ i 1) (count t)) (want-int (nth s (+ i 1))) 0))";
    let ws = file_warnings_mode(src, true);
    assert_eq!(ws.len(), 2, "{ws:?}");
    assert!(
        ws.iter().all(|w| w.contains("got nil | int")),
        "both reads may run off the end: {ws:?}"
    );
    // A non-strict guard at offset 0 says nothing — `i ≤ n` is not `i < n`.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (n) 1)\n\
         (sig f ((vector int) (int 0 _) -> any))\n\
         (defn f (s i) (let (n (count s)) (if (<= i n) (want-int (nth s i)) 0)))",
        true,
    );
    assert_eq!(ws.len(), 1, "{ws:?}");
}

/// The count ALIAS reaches every consumer, not just the walk's argument checks (C12). A
/// `let` is bound in three places — the walk, inference, and the return check's
/// `gradual_of_compound` — and only the first recorded `(let (n (count xs)) …)`, so the
/// same read was clean as an argument and warned as a RETURN.
#[test]
fn a_count_alias_reaches_the_return_check_too() {
    let src = "(defmodule t)\n\
         (sig a ((list string) -> string))\n\
         (defn a (words) (let (n (count words)) (if (>= n 4) (nth words 3) \"\")))\n\
         (sig b ((vector int) (int 0 _) -> int))\n\
         (defn b (xs i) (let (n (count xs)) (if (< i n) (nth xs i) 0)))";
    assert!(file_warnings_mode(src, true).is_empty(), "{src}");
    // …and the alias still proves nothing it should not: a length of 4 is not a length of 5.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig c ((list string) -> string))\n\
         (defn c (words) (let (n (count words)) (if (>= n 4) (nth words 4) \"\")))",
        true,
    );
    assert_eq!(ws.len(), 1, "{ws:?}");
}

/// An equality guard on ONE position of a tagged tuple selects the alternative in both
/// branches — for the whole value, not only the reads of that position. This is the
/// `[:ok x] | [:error msg]` dispatch every `parse!` is built on.
#[test]
fn a_position_guard_selects_the_tagged_tuple_alternative() {
    let src = "(defmodule t)\n\
         (sig want-map (map -> int))\n\
         (defn want-map (m) 1)\n\
         (sig want-err ((tuple :error string) -> int))\n\
         (defn want-err (e) 1)\n\
         (sig use-it ((or (tuple :error string) (tuple :ok (record :a int))) -> any))\n\
         (defn use-it (a) (if (= (nth a 0) :error) (want-err a) (want-map (nth a 1))))\n\
         (defn p (s) (if (= s \"x\") [:error \"bad\"] [:ok {:a 1}]))\n\
         (defn use-p (s) (let (a (p s)) (if (= (nth a 0) :error) (want-err a) (want-map (nth a 1)))))\n\
         (defn wrong (s) (let (a (p s)) (if (= (nth a 0) :ok) (want-err a) 0)))";
    let ws = file_warnings_mode(src, true);
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(
        ws[0].contains(
            "want-err: argument 1 expects (tuple :error, string), got (tuple :ok, {a: 1})"
        ),
        "{ws:?}"
    );
}

/// A self-call is typed in the branch it sits in: the accumulator of a list walk takes
/// an ELEMENT of the list, not an element-or-nil, because the self-call is in the else of
/// `(nil? xs)`.
#[test]
fn a_recursive_accumulator_is_typed_under_its_branch_guard() {
    let src = "(defmodule t)\n\
         (defn- walk (xs acc) (if (nil? xs) (first acc) (walk (rest xs) (cons (first xs) acc))))\n\
         (sig f (& string -> int))\n\
         (defn f (& parts) (walk (seq parts) (list \"x\")))";
    let ws = file_warnings_mode(src, true);
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(ws[0].ends_with("the body yields string"), "{ws:?}");
}

/// A `when`-shaped binding is a then-only guard on its condition: `(let (src (when k
/// (lookup k))) (if src (use k) …))` reads `k` truthy where `src` is — a falsy `src` proves
/// nothing, so the else-branch is left alone.
#[test]
fn a_when_shaped_binding_guards_its_condition() {
    let src = "(defmodule t)\n\
         (sig want-str (string -> int))\n\
         (defn want-str (s) 1)\n\
         (sig lookup (string -> (or nil string)))\n\
         (defn lookup (k) nil)\n\
         (sig f ((or nil string) -> any))\n\
         (defn f (k) (let (src (when k (lookup k))) (if src (want-str k) 0)))\n\
         (sig g ((or nil string) -> any))\n\
         (defn g (k) (let (src (when k (lookup k))) (if src 0 (want-str k))))";
    let ws = file_warnings_mode(src, true);
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(
        ws[0].contains("expects string, got nil | string (k)"),
        "{ws:?}"
    );
}

/// A count relation between two PARAMETERS, established by every caller (ADR-350): the
/// loop below is handed `n = (count codes)` beside `codes` by its one external caller and
/// passes both through in its self-call, so `(nth codes i)` under `(>= i n)`'s else is an
/// element — the shape `std/regex`'s two DFA loops have, which used to carry a
/// `(check-allow :type-mismatch …)` for exactly this.
#[test]
fn a_count_relation_between_parameters_is_derived_from_the_callers() {
    let src = "(defmodule t)\n\
         (defn- walk (codes n i acc) (if (>= i n) acc (walk codes n (+ i 1) (+ acc (nth codes i)))))\n\
         (defn f (s) (let (codes (string/->codepoints s) n (count codes)) (walk codes n 0 0)))";
    assert!(file_warnings_mode(src, true).is_empty());
    assert!(
        signatures(src)
            .iter()
            .any(|(name, sig, _)| name == "t/f" && sig == "(string) -> int"),
        "{:?}",
        signatures(src)
    );
    // A second caller that passes something else for `n` breaks the relation everywhere.
    let src2 = "(defmodule t)\n\
         (defn- walk (codes n i acc) (if (>= i n) acc (walk codes n (+ i 1) (+ acc (nth codes i)))))\n\
         (defn f (s) (let (codes (string/->codepoints s) n (count codes)) (walk codes n 0 0)))\n\
         (defn g (s) (walk (string/->codepoints s) 100 0 0))";
    let ws = file_warnings_mode(src2, true);
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(ws[0].contains("nil | int ((nth codes i))"), "{ws:?}");
    // …and so does a self-call that hands `n` on beside a DIFFERENT collection.
    let src3 = "(defmodule t)\n\
         (defn- walk (codes n i acc) (if (>= i n) acc (walk (rest codes) n (+ i 1) (+ acc (nth codes i)))))\n\
         (defn f (s) (let (codes (string/->codepoints s) n (count codes)) (walk codes n 0 0)))";
    let ws = file_warnings_mode(src3, true);
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(ws[0].contains("nil | int ((nth codes i))"), "{ws:?}");
}

/// A no-init `reduce` seeds the accumulator with the first element and steps over the
/// REST: on a one-element collection it is that element, unstepped — so the seed joins
/// the result unless the length proves two or more (ADR-350). It read `int[2..]` for
/// `(reduce [1] +)`, whose value is `1`.
#[test]
fn a_no_init_reduce_keeps_its_seed_unless_two_elements_are_proven() {
    assert_eq!(ty_str("(reduce [1] (fn (a x) (+ a x)))"), "1");
    assert_eq!(
        ty_str("(let (xs (if (int? 1) [1] [1 2 3])) (reduce xs (fn (a x) (+ a x))))"),
        "int[1..]"
    );
    assert_eq!(ty_str("(reduce [1 2 3] (fn (a x) (+ a x)))"), "int[2..]");
}

/// A return that is two tuple alternatives — two branches each yielding `[model idx]`,
/// with the model reshaped in one of them — keeps BOTH shapes through Pass 2.9's
/// widening (ADR-350): a union that is not growing is not an ascent, and merging it lost
/// the tuple, so a caller's `[m1 idx]` read `int | map` for `m1`.
#[test]
fn a_stable_union_of_tuples_keeps_its_shapes_through_the_fixpoint() {
    let src = "(defmodule t)\n\
         (sig want-map (map -> int))\n\
         (defn want-map (m) 1)\n\
         (defn- ensure (m path)\n\
           (let (idx (get m path))\n\
             (if idx [m idx] [(assoc m :extra path) (dec (count (get m :buffers)))])))\n\
         (sig use-it ((record &open :buffers (vector any)) string -> int))\n\
         (defn use-it (m path) (let ([m1 idx] (ensure m path)) (want-map m1)))";
    let ws = file_warnings_mode(src, true);
    assert!(ws.is_empty(), "{ws:?}");
}

#[test]
fn a_growing_union_of_tuples_widens_to_one_shape_not_a_bare_vector() {
    // The lattice half: a fixpoint round that gained a second tuple alternative differing
    // in TWO positions widens to the position-wise hull, a tuple a destructuring can still
    // read — not the bare `vector<int | map>` the plain merge produced.
    let prev = ty_of_sig("(tuple (record :a int) int)");
    let now =
        ty_of_sig("(or (tuple (record :a int) int) (tuple (record :a int :b int) (int -1 _)))");
    let widened = now.widen_intervals_against(&prev);
    let first = widened.tuple_elem_at(0).expect("a tuple shape survives");
    assert!(first.is_subtype(&Ty::of(Tag::Map)), "{widened}");
    assert!(
        widened
            .tuple_elem_at(1)
            .is_some_and(|t| t.is_subtype(&Ty::of(Tag::Int))),
        "{widened}"
    );
}

#[test]
fn a_moving_interval_beside_a_stable_tuple_of_the_same_tags_still_widens() {
    // Two alternatives with the SAME tags — a parser state whose counter moves, beside its
    // seed — matched two previous terms by tag alone, so neither was widened and the
    // derivation ran out of rounds (`tests/jit_eq_join_test.blsp`'s `drive-advance`,
    // 2026-09-17). The previous term the current one GREW from is the one that is its
    // subtype.
    let prev = ty_of_sig("(or (tuple \"(\" (int 1 4) nil bool) (tuple \"(\" 0 nil false))");
    let now = ty_of_sig("(or (tuple \"(\" (int 1 5) nil bool) (tuple \"(\" 0 nil false))");
    let widened = now.widen_intervals_against(&prev);
    assert!(
        widened.to_string().contains("int[1..]"),
        "the moving position goes to its infinity: {widened}"
    );
    assert!(
        widened.to_string().contains("0, nil, false"),
        "the seed is untouched: {widened}"
    );
    // …and end to end: the driver derives, and its callers' arithmetic is `int`.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (defn step (st) (let ([open n head p] (first st)) (cons [open (+ n 1) head (or p (= n 2))] (rest st))))\n\
         (defn- drive (i st) (if (= i 0) st (drive (- i 1) (step st))))\n\
         (defn- other (i acc) (if (= i 0) acc (other (- i 1) (+ acc (bit/and i 1)))))\n\
         (defn run () (list (drive 4000 (list [\"(\" 0 nil false])) (other 3000 0)))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
}

// KI-140: `apply` binds the callee's type variable from the spread operands and the
// collection's element type, the way the written-out call does. `math/max` declares
// `(& ?A -> ?A)`, so `(apply math/max 1 (map xs string/length))` is an `int` — it used to
// type by `apply`'s own curated signature, which knows nothing of the callee's variable, and
// fell to the callee's flat return (`ordered`), failing a `-> int` declaration under `--strict`
// (bedit's ratchet, four findings of this shape).
#[test]
fn apply_binds_the_callee_type_variable_from_its_operands() {
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (sig widest (list -> int))\n\
         (defn widest (xs) (apply math/max 1 (map xs string/length)))\n\
         (sig widest2 (list -> int))\n\
         (defn widest2 (xs) (math/max 1 (string/length (first xs))))",
        true,
    );
    assert!(ws.is_empty(), "both bodies are ints — {ws:?}");
    assert_eq!(ty_str("(apply math/max 1 [2 3])"), "1 | 2 | 3");
    // A same-file callee with a declared variable binds through `apply` too.
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (sig pick (& ?A -> ?A))\n\
         (defn pick (& xs) (first xs))\n\
         (sig use-pick (list -> int))\n\
         (defn use-pick (xs) (apply pick 1 (map xs string/length)))",
        true,
    );
    assert!(
        ws.is_empty(),
        "`?A` is bound to `int` through the spread — {ws:?}"
    );
}

#[test]
fn apply_of_a_ring_operator_stays_in_its_closure_not_one_steps_interval() {
    // The spread's count is unknown: `(apply + 1 [2 3])` is an int, never `int[3..6]`, and
    // a float element is contagious whatever the count.
    assert_eq!(ty_str("(apply + 1 [2 3])"), "int");
    assert_eq!(ty_str("(apply + [1 2])"), "int");
    assert_eq!(ty_str("(apply * 2 [1.5])"), "float");
}

// `get-in` with a literal path reads through a declared shape the way a chain of `get`s
// does — a keyword step is the record field, any other step is a `map<K, V>`'s value.
// The offset read back from a declared `(map any int)` two levels down is `int`, so the
// arithmetic on it is int and the `range` it feeds does not warn; with the leaf declared
// `any` the same read says `number`, which is what every consumer of `get-in` used to see.
#[test]
fn get_in_with_a_literal_path_reads_the_declared_shape() {
    let program = |leaf: &str| {
        format!(
            "\
             (defmodule t)\n\
             (deftype model (record &open :hosted (optional (map int (record &open :cursors (optional (map any {leaf})))))))\n\
             (sig f (model int any -> list))\n\
             (defn f (m i who)\n\
               (let (pos (get-in m [:hosted i :cursors who]))\n\
                 (if (nil? pos) '() (range (dec pos) (+ pos 1)))))"
        )
    };
    let precise = file_warnings_mode(&program("int"), true);
    assert!(precise.is_empty(), "{precise:?}");
    let vague = file_warnings_mode(&program("any"), true);
    assert!(
        vague
            .iter()
            .any(|w| w.contains("range") && w.contains("number")),
        "{vague:?}"
    );
}

// The default form: the walk stops at the first absent key (or non-map value) and answers
// the default, so an empty map two keys deep IS the default — `1`, not `1 | nil` — while a
// present key holding nil at the last step is still nil.
#[test]
fn get_in_with_a_default_reads_the_default_for_absence_only() {
    assert_eq!(ty_str("(get-in {} [:sandbox :next-id] 1)"), "1");
    assert_eq!(ty_str("(get-in {:a {:b 2}} [:a :b] 1)"), "2");
    assert_eq!(ty_str("(get-in {:a {:b nil}} [:a :b] 1)"), "nil");
    assert_eq!(ty_str("(get-in {:a {:b 2}} [:a :c] 1)"), "1");
    assert_eq!(ty_str("(get-in {:a {:b 2}} [:a :b])"), "2");
    assert_eq!(ty_str("(get-in {:a {:b 2}} [:a :c])"), "nil");
}
