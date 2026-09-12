//! Element types through the sequence combinators: first/last/nth, `and`/`or` guards, map/filter, reduce/fold, `get` with a default, extrema, and inference termination.

use super::*;

// ---- element types flow through first/last/nth (ADR-078 slice 2) ----

#[test]
fn first_of_a_string_vector_is_not_a_number() {
    // `(first ["a" "b"])` : string | nil — disjoint from number → flagged.
    let w = warnings(r#"(+ 1 (first ["a" "b"]))"#);
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("\"a\"")),
        "expected a number/string mismatch from the element type: {w:?}"
    );
}

#[test]
fn first_of_an_int_vector_is_a_number() {
    // `(first [10 20])` : int | nil — overlaps number → no warning.
    let w = warnings("(+ 1 (first [10 20]))");
    assert!(
        w.iter().all(|s| !s.contains("expects number")),
        "an int element must not warn against +: {w:?}"
    );
}

#[test]
fn list_constructor_carries_its_element_type() {
    // `(list "a" "b")` : list<string>, so `(first …)` is string|nil.
    let w = warnings(r#"(+ 1 (first (list "a" "b")))"#);
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("\"a\"")),
        "(list …) element type should flow to first: {w:?}"
    );
}

#[test]
fn heterogeneous_or_unknown_elements_do_not_warn() {
    // Mixed elements → int|string element; first → int|string|nil, which
    // overlaps number → no false positive.
    let w = warnings(r#"(+ 1 (first [1 "a"]))"#);
    assert!(
        w.iter().all(|s| !s.contains("expects number")),
        "a heterogeneous element type must not warn: {w:?}"
    );
    // first of an unknown (variable) sequence → unknown → no warning.
    let w = warnings("(fn (xs) (+ 1 (first xs)))");
    assert!(
        w.iter().all(|s| !s.contains("expects number")),
        "an unknown sequence must not warn: {w:?}"
    );
}

// ---- `and`-guard narrowing in an `if` test (the match-lowering fix) ----

#[test]
fn and_guard_narrows_in_the_then_branch() {
    // `(and (int? x) …)` as an `if` test must narrow `x` to int in the then
    // branch — so a use that would mismatch the *original* type is suppressed
    // (here `x` is a string, narrowed to never → the `+` use is unreachable).
    let w = warnings_expanded(r#"(let (x "s") (if (and (int? x) true) (+ x 1) 0))"#);
    assert!(
        w.iter().all(|s| !s.contains("expects number")),
        "an `and` guard should narrow x in the then branch: {w:?}"
    );
}

#[test]
fn matching_a_list_against_a_vector_pattern_is_not_flagged() {
    // The match compiler lowers a vector pattern to
    // `(if (and (vector? m) (= (%vector-length m) 2)) (… (%vector-ref m i) …) …)`.
    // With `(list 1 2)` now typed `list<int>`, the guarded `vector-ref` must
    // not be flagged — the `and` guard narrows `m` to a vector (→ never here).
    let w = warnings_expanded("(match (list 1 2) ([a b] :vec) (_ :not-vec))");
    assert!(
        w.iter()
            .all(|s| !s.contains("vector-ref") && !s.contains("vector-length")),
        "a list matched against a vector pattern must not warn: {w:?}"
    );
}

#[test]
fn and_guard_does_not_narrow_the_else_branch() {
    // A falsy `(and (vector? m) …)` does NOT imply `m` isn't a vector — a
    // *later* conjunct may have failed. So the else-branch must keep `m`'s
    // full type; flagging a vector op there would be a false positive.
    let w = warnings_expanded(
        "(fn (m) (if (and (vector? m) (%eq (%vector-length m) 2)) \
                         (%vector-ref m 0) (%vector-ref m 0)))",
    );
    assert!(
        w.iter().all(|s| !s.contains("vector-ref")),
        "the else-branch of an `and` guard must not be narrowed: {w:?}"
    );
    // The then-branch still narrows (sanity: the guard didn't go silent).
    let w = warnings_expanded(r#"(fn (m) (if (and (int? m) true) (string/length m) 0))"#);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "the then-branch should still narrow m to int: {w:?}"
    );
}

#[test]
fn or_guard_does_not_falsely_narrow() {
    // `or` must NOT narrow from its first operand (a truthy `or` implies
    // nothing about it). `(or (int? x) true)` is always true, so the then
    // branch keeps `x`'s full (string) type — and a genuine misuse there is
    // still seen. (Guards against the `and`-fix over-reaching into `or`.)
    let w = warnings_expanded(r#"(let (x "s") (if (or (int? x) true) (string/length x) 0))"#);
    assert!(
        w.iter().all(|s| !s.contains("expects")),
        "a correct use under an `or` guard must not warn: {w:?}"
    );
}

// ---- parametric HOF result types — map / filter (ADR-078, Option B) ----

#[test]
fn map_result_flows_the_callback_return() {
    // `(map (list 1 2 3) inc)` : list<number>, so `(first …)` is number|nil —
    // disjoint from string → string-length flags it.
    let w = warnings("(string/length (first (map (list 1 2 3) inc)))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "map's element type (number) should flow to first: {w:?}"
    );
    // ...and a numeric sink is fine (number overlaps).
    let w = warnings("(+ 1 (first (map (list 1 2 3) inc)))");
    assert!(
        w.iter().all(|s| !s.contains("expects")),
        "a number element must not warn against +: {w:?}"
    );
}

#[test]
fn filter_preserves_the_element_type() {
    // `(filter (list 1 2 3) even?)` : list<int> — element type unchanged.
    let w = warnings("(string/length (first (filter (list 1 2 3) even?)))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "filter should preserve the int element type: {w:?}"
    );
}

#[test]
fn element_type_flows_through_more_combinators() {
    // Structured-types extension: second/third/rest/but-last/distinct/dedupe/
    // take-last/drop-last/remove/keep/interpose/range all flow the element type,
    // so a downstream string-vs-number mismatch is caught. Each must warn here.
    for src in [
        r#"(+ 1 (second ["a" "b"]))"#,
        r#"(+ 1 (first (rest ["a" "b"])))"#,
        r#"(+ 1 (first (but-last ["a" "b"])))"#,
        r#"(+ 1 (first (distinct ["a" "b"])))"#,
        r#"(+ 1 (first (seq/dedupe ["a" "b"])))"#,
        r#"(+ 1 (first (seq/remove ["a" "b"] (fn (x) false))))"#,
        r#"(+ 1 (first (seq/take-last ["a" "b"] 1)))"#,
        r#"(+ 1 (first (seq/keep ["a" "b"] (fn (x) x))))"#,
        "(string/length (first (range 5)))",
    ] {
        let w = warnings(src);
        assert!(
            w.iter()
                .any(|s| s.contains("number") || s.contains("string")),
            "expected an element-type mismatch for {src}: {w:?}"
        );
    }
    // Negative controls — a valid element type must NOT warn.
    for src in [
        "(+ 1 (second [10 20]))",
        "(+ 1 (first (rest [10 20])))",
        // interpose unions the separator: int|string includes int → valid for +.
        r#"(+ 1 (first (seq/interpose "z" [1 2])))"#,
    ] {
        let w = warnings(src);
        assert!(
            w.iter().all(|s| !s.contains("expects number")),
            "a valid element type must not warn for {src}: {w:?}"
        );
    }
}

#[test]
fn identity_lambda_preserves_element_type() {
    // `(map (list 1 2 3) (fn (x) x))` : list<int> — the lambda returns its
    // argument, so B = the element type A.
    let w = warnings("(string/length (first (map (list 1 2 3) (fn (x) x))))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "an identity callback should preserve the element type: {w:?}"
    );
}

#[test]
fn map_filter_do_not_refine_when_uncertain() {
    // Unknown callback (a local) → no refinement → no warning.
    let w = warnings("(fn (g) (string/length (first (map (list 1 2 3) g))))");
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an unknown callback must not refine the result: {w:?}"
    );
    // Identity callback + unknown collection → B depends on the (unknown)
    // element type → no refinement.
    let w = warnings("(fn (xs) (string/length (first (map xs (fn (x) x)))))");
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an identity callback over an unknown collection must not refine: {w:?}"
    );
    // A branchy lambda body over elements whose truthiness is unknown → the union of
    // both branches, `1 | "a"`, which is not provably a string → reported by `⊆` (precise).
    // Over `(list 1 2 3)` the else-branch is DEAD (an int is never falsy — `Ctx::is_dead`),
    // so the result is exactly `1`, and `string/length` of it is a genuine misuse.
    let w = warnings(r#"(string/length (first (map (list 1 2 3) (fn (x) (if x 1 "a")))))"#);
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains("got 1")),
        "a dead else-branch must not widen the result: {w:?}"
    );
}

// ---- reduce / fold result types (slice 2) ----

#[test]
fn reduce_result_is_the_accumulator_type() {
    // `(reduce (list 1 2 3) 0 +)` : number (init int ∪ +'s number return) —
    // disjoint from string → flagged.
    let w = warnings("(string/length (reduce (list 1 2 3) 0 +))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "reduce's accumulator type should flow out: {w:?}"
    );
    // ...and a numeric sink is fine.
    let w = warnings("(+ 1 (reduce (list 1 2 3) 0 +))");
    assert!(
        w.iter().all(|s| !s.contains("expects")),
        "a numeric reduce result must not warn against +: {w:?}"
    );
}

#[test]
fn fold_with_a_lambda_callback_types_the_result() {
    // `(fold … 0 (fn (acc x) (+ acc x)))` : number — the 2-arg callback's
    // return (number) joined with the init (int).
    let w = warnings("(string/length (fold (list 1 2 3) 0 (fn (acc x) (+ acc x))))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "fold should type the accumulator from a lambda callback: {w:?}"
    );
}

#[test]
fn reduce_fold_bail_when_init_or_callback_unknown() {
    // Unknown callback (local) → flat, no warning.
    let w = warnings("(fn (g) (string/length (reduce (list 1 2 3) 0 g)))");
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an unknown reduce callback must not refine: {w:?}"
    );
    // Unknown init type (a fn param) → flat, no warning.
    let w = warnings("(fn (init) (string/length (reduce (list 1 2 3) init +)))");
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an unknown init must not refine the reduce result: {w:?}"
    );
}

// ---- `get` with a default: the absence case is the default, not nil ----

#[test]
fn get_with_a_default_replaces_the_absence_nil_by_the_default_type() {
    let interp = crate::Interp::new();
    let ty = |src: &str| {
        use super::infer::expr_ty;
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let form = reader::read_one(&mut heap, src).expect("parse");
        expr_ty(&heap, form, &Ctx::default())
    };
    let int = Ty::of(Tag::Int);
    let nil = Ty::of(Tag::Nil);
    // A map literal declares `:a`; `:b` is absent, so it is exactly the default.
    let t = ty("(get {:a 1} :a 0)").expect("typed");
    assert!(t.is_subtype(&int) && !t.is_subtype(&nil), "{t}");
    let t = ty("(get {:a 1} :b 0)").expect("typed");
    assert!(t.is_subtype(&int) && t.is_disjoint(&nil), "{t}");
    // Without a default the absent key is nil.
    let t = ty("(get {:a 1} :b)").expect("typed");
    assert!(t.is_subtype(&nil), "{t}");
    // A default whose type is unknown keeps the two-argument reading (admits nil).
    let t = ty("(get {:a 1} :a unknown-thing)").expect("typed");
    assert!(t.is_subtype(&int), "{t}");
}

// ---- inferred returns narrow their branches; and/or are short-circuit exact ----

#[test]
fn an_inferred_return_sees_the_truthy_half_of_an_or_default() {
    // `(or E -1)` where E is `number | failure` yields `number | failure`: a failure is
    // TRUTHY, so `or` hands it back rather than silently defaulting it away. The inferred
    // return (`expr_ty`, not the walk) used to union both branches of the expansion's `if`
    // under the unnarrowed scope and report `nil | number`.
    let interp = crate::Interp::new();
    let ret = |src: &str| {
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let form = reader::read_one(&mut heap, src).expect("parse");
        let expanded = crate::eval::macros::macroexpand_all(&mut heap, form, interp.root).unwrap();
        super::infer::expr_ty(&heap, expanded, &Ctx::default())
            .and_then(|t| t.as_arrow().map(|s| s.ret.clone()))
    };
    let nil = Ty::of(Tag::Nil);
    let number = Ty::NUMBER;
    // The expanded `(let (g E) (if g g -1))`.
    let t = ret("(fn (s) (or (string/->number s) -1))").expect("typed");
    assert!(
        t.is_subtype(&number.clone().union(Ty::of(Tag::Failure))) && t.is_disjoint(&nil),
        "{t}"
    );
    // A predicate guard narrows the same way.
    let t = ret("(fn (s) (let (g (string/->number s)) (if (int? g) g -1)))").expect("typed");
    assert!(t.is_subtype(&Ty::of(Tag::Int)), "{t}");
    // The SURFACE `or` (a fragment that is not expanded) is short-circuit exact too, and
    // `and` symmetrically: only the falsy slice of a non-last operand can be the value.
    let surface = |src: &str| {
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let form = reader::read_one(&mut heap, src).expect("parse");
        super::infer::expr_ty(&heap, form, &Ctx::default())
    };
    // The SURFACE `or` is short-circuit exact too: it yields the first TRUTHY operand,
    // and a failure is truthy — so a failing parse is the result, never the default.
    let t = surface("(or (string/->number \"1\") -1)").expect("typed");
    assert!(
        t.is_subtype(&number.clone().union(Ty::of(Tag::Failure))) && t.is_disjoint(&nil),
        "{t}"
    );
    // `and` yields the first falsy operand or the last one. A failure is truthy, so it
    // never short-circuits here: the result is the last operand, a string.
    let t = surface("(and (string/->number \"1\") \"yes\")").expect("typed");
    assert!(t.is_subtype(&Ty::of(Tag::Str)), "{t}");
}

// ---- an extremum returns one of its operands ----

#[test]
fn an_extremum_is_typed_as_the_union_of_its_operands() {
    use super::infer::expr_ty;
    // `math/max` over two ints is an int, so it feeds an int-only parameter without a
    // `--strict` complaint that `ordered ⊄ int`; over an int and a float it is `int | float`.
    let interp = crate::Interp::new();
    let ty = |src: &str| {
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let form = reader::read_one(&mut heap, src).expect("parse");
        expr_ty(&heap, form, &Ctx::default())
    };
    let int = Ty::of(Tag::Int);
    let float = Ty::of(Tag::Float);
    // Literal operands keep their literal sets (`{1, 2}` — max of 1 and 2 IS one of them).
    let t = ty("(math/max 1 2)").expect("typed");
    assert!(t.is_subtype(&int), "{t}");
    let t = ty("(math/min 1 2.5)").expect("typed");
    assert!(
        t.is_subtype(&int.clone().union(float.clone())) && !t.is_subtype(&int),
        "{t}"
    );
    let t = ty("(math/min 1)").expect("typed");
    assert!(t.is_subtype(&int), "{t}");
    // An unknown operand defers to the declared sig (`ordered`) — never narrower than the
    // truth.
    let t = ty("(math/max 1 x)").expect("the sig still types it");
    assert!(!t.is_subtype(&int), "{t}");
}

// ---- inference terminates on a mutually recursive call graph ----

/// Two loaded functions that call each other, each referencing the partner TWICE. The
/// cycle guard refuses the first re-entry; the regression was that the refusal itself
/// released the in-flight mark (`InferGuard::enter` built-and-dropped a guard on the
/// refusal path), so the second reference re-entered the cycle and the pair nested
/// without bound — every level a fresh stack segment, until memory ran out (54 GB on a
/// `nest run`; three 19 GB test processes here). Now: both sigs resolve, and quickly.
#[test]
fn mutually_recursive_loaded_functions_infer_in_bounded_time() {
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(defn mutual-a (x) (list (mutual-b x) (mutual-b (string/length x))))")
        .unwrap();
    interp
        .eval_str("(defn mutual-b (x) (list (mutual-a x) (mutual-a (string/length x))))")
        .unwrap();
    let started = std::time::Instant::now();
    let a = super::sigs::sig_of(&interp.heap, value::intern("mutual-a"));
    let b = super::sigs::sig_of(&interp.heap, value::intern("mutual-b"));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "inference diverged"
    );
    // The cycle refusal leaves the partner's return unknown, but `string/length`'s own
    // demand (a primitive: its domain needs no inference) still pins the parameter.
    for (name, sig) in [("mutual-a", a), ("mutual-b", b)] {
        let sig = sig.unwrap_or_else(|| panic!("{name}: no sig"));
        assert_eq!(sig.params.len(), 1, "{name}: {sig}");
        assert!(sig.params[0].is_subtype(&Ty::of(Tag::Str)), "{name}: {sig}");
    }
    // …and a call site is still checked against it.
    let form = reader::read_one(&mut interp.heap, "(mutual-a 5)").expect("parse");
    let ws = check_form(&interp.heap, form);
    assert!(
        ws.iter().any(|w| w.contains("mutual-a")),
        "expected an argument warning for mutual-a, got {ws:?}"
    );
}

/// The sequence ACCESSORS carry their domain. `second`/`third` always did; `first`,
/// `rest`, `nth` and `last` — the primitives the other two are built out of — carried
/// no signature at all, so `(nth 7 0)` and `(first "ab")` type-checked in silence and
/// failed at runtime. The domain is `seqable`, which is what their own Rust
/// `wrong_type` message names ("list, vector, set, map or bytes") — a string is not in
/// it, and neither is any scalar.
#[test]
fn the_sequence_accessors_reject_a_non_seqable_argument() {
    for (call, culprit) in [
        ("(first 7)", "first"),
        ("(rest \"ab\")", "rest"),
        ("(last :k)", "last"),
        ("(nth \"ab\" 0)", "nth"),
        ("(nth 7 0)", "nth"),
    ] {
        let ws = file_warnings(&format!("(defmodule test/mod)\n(defn f () {call})"));
        assert!(
            ws.iter()
                .any(|w| w.contains(culprit) && w.contains("seqable")),
            "expected a seqable-domain warning for {call}, got {ws:?}"
        );
    }
}

/// …and stays silent on every argument that genuinely works, which is the half that
/// decides whether the signature above is shippable. A RECORD is the load-bearing case:
/// it is a `map` to the type system and it IS indexable (through its Seqable view), so
/// a domain that excluded `map` — reality, since a plain map raises — would false-flag
/// every `(first some-record)`. Soundness over completeness: the signature deliberately
/// admits the plain map it cannot distinguish.
#[test]
fn the_sequence_accessors_stay_silent_on_what_actually_works() {
    let src = "(defmodule test/mod)\n        (defrecord point (x y))\n        (defn a (v) (first v))\n        (defn b () (first [1 2]))\n        (defn c () (first {:a 1}))\n        (defn d () (first (point 1 2)))\n        (defn e () (nth (list 1 2) 0))\n        (defn f () (nth nil 0))\n        (defn g (v) (nth v 0 :missing))";
    let ws = file_warnings(src);
    assert!(
        !ws.iter().any(|w| w.contains("seqable")),
        "no seqable warning should fire on valid accessor uses, got {ws:?}"
    );
}

// ---- a fold over a provably non-empty sequence ran its step at least once ----
// `(first (reduce (string/split s) '() (fn (acc x) (conj acc x))))` read as `nil | string`:
// the reduce joined the empty-input case (`init`, here `nil`) into its result, so `first`
// had to allow the empty list — though `string/split` never returns one (`""` splits to
// `("")`), which `list<string>` (the `pair` tag alone) already states.

#[test]
fn a_reduce_over_a_non_empty_sequence_is_its_step_result_not_the_init() {
    let sigs =
        signatures("(defn g (s) (first (reduce (string/split s) '() (fn (acc x) (conj acc x)))))");
    let (_, sig, _) = sigs
        .iter()
        .find(|(n, _, _)| n == "g")
        .unwrap_or_else(|| panic!("{sigs:?}"));
    assert!(
        !sig.contains("nil"),
        "no empty case for a split's fold: {sig}"
    );
    // …and over a sequence that MAY be empty, the init stays in — `nil` is honest there.
    let sigs = signatures(
        "(defn h () (first (reduce (filter (list 1 2) int?) '() (fn (acc x) (conj acc x)))))",
    );
    let (_, sig, _) = sigs
        .iter()
        .find(|(n, _, _)| n == "h")
        .unwrap_or_else(|| panic!("{sigs:?}"));
    assert!(
        sig.contains("nil"),
        "a maybe-empty fold keeps the empty case: {sig}"
    );
}
