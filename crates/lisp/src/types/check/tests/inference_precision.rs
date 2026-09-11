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
    assert_eq!(ty_str("(let (x (+ 1 2)) (/ x 2))"), "int | ratio");
    // a literal SET that lands on both kinds is exactly `int | ratio` — the answer the
    // caller already gives, so the fold stops rather than walking the rest of it
    assert_eq!(ty_str("(/ (first (list 6 5)) 2)"), "int | ratio");
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
    assert_eq!(ty_str("(first (string/->bytes \"ab\"))"), "nil | int");
    assert_eq!(
        ty_str("(map (string/->bytes \"ab\") inc)"),
        "nil | list<int>"
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
    assert_eq!(ty_str("(+ 1 2)"), "int");
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
        ("(cons 1 '())", "list<1>"),
        ("(cons 1 nil)", "list<1>"),
        // a quoted list is data with its elements in view
        ("'(1 2)", "list<1 | 2>"),
        ("(vec '(1 2))", "vector<1 | 2>"),
        // a range is a range of integers
        ("(range 5)", "list<int>"),
        // a numeric operator as a callback / a fold / spread — the same closure rules
        // a two-element vector literal is a TUPLE, whose arity is part of its type — so
        // it is provably non-empty and `map` keeps that (the `nil` arm is the empty case)
        ("(map [1 2] inc)", "list<int>"),
        ("(reduce [1 2] +)", "int"),
        ("(reduce [1 2] 0 +)", "int"),
        ("(apply + [1 2])", "int"),
        ("(reduce [1 2] 0.5 +)", "float"),
        // reshapers with no signature at all used to be `any`
        ("(vec [1 2])", "vector<1 | 2>"),
        ("(into [] (list 1))", "vector<1>"),
        ("(into {} [[:a 1]])", "map"),
        ("(conj [1] 2)", "vector<1 | 2>"),
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
