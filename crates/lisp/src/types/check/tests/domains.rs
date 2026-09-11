//! Domains: `(not T)`, arrow-typed parameters at the call site, callback compatibility, parameter domains credited within a guard, multi-arm functions, unions of shapes, closed records.

use super::*;

// ---- `(not T)` — the complement, sayable at last ----

#[test]
fn not_type_in_a_sig_rejects_a_member_of_the_negated_set() {
    let ws = file_warnings("(sig f ((not nil) -> int))\n(defn f (x) 0)\n(defn g () (f nil))");
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
    // …and admits everything else.
    let ws = file_warnings("(sig f ((not nil) -> int))\n(defn f (x) 0)\n(defn g () (f 5))");
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
}

#[test]
fn not_type_composes_with_and_and_or() {
    // The idiom the lattice could always compute and the grammar could not say.
    let ws = file_warnings(
        "(sig f ((and number (not float)) -> int))\n(defn f (x) 0)\n(defn g () (f 1.5))",
    );
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
    let ws = file_warnings(
        "(sig f ((and number (not float)) -> int))\n(defn f (x) 0)\n(defn g () (f 1))",
    );
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
}

#[test]
fn a_small_complement_renders_as_not_rather_than_a_tag_dump() {
    // `expects string, got nil | bool | number | symbol | keyword | pair | vector | fn
    // | macro | native | map | ref | pid | rope | socket | subprocess | table | bytes |
    // set` was a real diagnostic — the else-branch of a `(string? x)` guard.
    assert_eq!(Ty::of(Tag::Str).negate().to_string(), "(not string)");
    assert_eq!(
        Ty::of(Tag::Str)
            .union(Ty::of(Tag::Nil))
            .negate()
            .to_string(),
        "(not (nil | string))"
    );
    // An ordinary wide union is still a union — the rendering only fires for a
    // genuinely small complement.
    assert_eq!(
        Ty::of(Tag::Int).union(Ty::of(Tag::Str)).to_string(),
        "int | string"
    );
}

// ---- arrow-typed parameters are enforced at the call site ----
// A `sig`-declared higher-order parameter was annotated and then not checked:
// `(sig g ((int -> int) -> int))` accepted `string/length` in silence.

// A lambda LITERAL was never checked as a callback: `callback_sig` answered `None` for
// one, so the parameter and result rules both skipped it and `(g (fn (x) (str x)))`
// against `((int -> int) -> int)` was silent. The literal is now typed under the arrow's
// own domain, and the existing result-disjointness rule catches it.
#[test]
fn a_lambda_callback_whose_result_is_disjoint_is_flagged() {
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n(defn c () (g (fn (x) (str x))))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("callback whose result is used as int") && w.contains("string")),
        "{ws:?}"
    );
}

// …and the reason the body is typed under the DECLARED domain rather than as a free
// `(any -> R)` arrow compared by `⊆`: with `x` unknown `(+ x 1)` is `number`, which is
// not a subtype of `int` — a false positive on a perfectly valid callback. Under `x :
// int` it is `int`, and identity is whatever it was handed.
#[test]
fn a_lambda_callback_with_a_merely_wider_result_is_not_flagged() {
    for body in ["(+ x 1)", "x", "(if (> x 0) x 0)"] {
        let ws = file_warnings(&format!(
            "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n(defn c () (g (fn (x) {body})))"
        ));
        assert!(
            !ws.iter().any(|w| w.contains("callback whose result")),
            "{body}: {ws:?}"
        );
    }
}

// A FALSE POSITIVE the lattice produced: `(tuple 0) ∪ pair` — a fold whose init is `[0]`
// and whose step conses — merged into one term whose `elem_ty` reported the TUPLE's
// elements for the whole thing, pair member included. `first` then typed as `0`, and
// passing it where a string is wanted warned, while the runtime value was the string
// "t". The invariant is "never false-positives", so this must stay silent.
#[test]
fn a_tuple_shape_does_not_leak_onto_a_pair_member_through_first() {
    let ws = file_warnings(
        "(sig takes-str (string -> any))\n(defn takes-str (s) s)\n\
         (defn fp () (takes-str (first (fold [\"t\"] [0] (fn (a x) (cons x a))))))",
    );
    assert!(ws.is_empty(), "{ws:?}");
}

#[test]
fn a_callback_that_cannot_accept_what_it_is_handed_is_flagged() {
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n(defn c () (g string/length))",
    );
    assert!(
        ws.iter().any(|w| w.contains(
            "g: argument 1 is a callback handed int at position 1, but string/length takes string there"
        )),
        "{ws:?}"
    );
}

#[test]
fn a_callback_whose_parameter_merely_widens_is_silent() {
    // `math/abs` takes `number`, which overlaps `int` — not provably wrong, so
    // silent. Disjointness, never subtyping: the no-false-positive rule.
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n(defn c () (g math/abs))",
    );
    assert!(!ws.iter().any(|w| w.contains("callback handed")), "{ws:?}");
}

#[test]
fn a_callback_whose_result_cannot_be_used_is_flagged() {
    // Comparing results by *subtyping* would false-positive at every call site (an
    // over-approximated return is not a subtype of a specific one). Disjointness is
    // sound in the same way the parameter direction is: the inferred return is a
    // superset of the truth, so if the superset shares nothing with what the caller
    // does with it, neither does the truth.
    let ws = file_warnings(
        "(sig g ((int -> string) -> int))\n(defn g (f) 0)\n(defn h (n) (+ n 1))\n(defn c () (g h))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("callback whose result is used as string")
                && w.contains("h returns")),
        "{ws:?}"
    );
}

#[test]
fn a_callback_whose_result_merely_widens_is_silent() {
    // The over-approximation must not warn: `number` overlaps `int`, and an unknown
    // return (`any`) overlaps everything.
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) 0)\n(defn h (n) (+ n 1))\n(defn c () (g h))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("callback whose result")),
        "{ws:?}"
    );
    let ws = file_warnings(
        "(sig g ((int -> string) -> int))\n(defn g (f) 0)\n(defn h (n) n)\n(defn c () (g h))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("callback whose result")),
        "{ws:?}"
    );
}

#[test]
fn a_same_file_callback_is_checked_from_its_inferred_signature() {
    // An inferred parameter demand is a *superset* of what the function really
    // accepts, so disjoint-from-the-superset is disjoint from the truth.
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n\
         (defn cb (s) (string/length s))\n(defn c () (g cb))",
    );
    assert!(
        ws.iter().any(|w| w.contains("but cb takes string there")),
        "{ws:?}"
    );
}

#[test]
fn a_permissive_higher_order_stdlib_callback_stays_silent() {
    // `map`'s curated arrow is `(any) -> any`, which is disjoint from nothing.
    let ws = file_warnings("(defn c () (map [1 2 3] string/length))");
    assert!(!ws.iter().any(|w| w.contains("callback handed")), "{ws:?}");
}

// ---- parameter DOMAINS: a branch's demand, credited within its guard ----
// The old rule credited only unconditional demands, so the ordinary shape of Brood
// code — a body that branches on what its argument is — constrained nothing at all.

#[test]
fn the_domain_walk_survives_a_pathologically_deep_body() {
    // The sibling of `checker_survives_pathologically_deep_forms`, for the passes that
    // walk a function's BODY: a body this deep can only arrive by construction (the
    // reader caps nesting at 256), which is exactly what a macro expansion can produce.
    // The property under test is "returns instead of crashing the host".
    //
    // It found one on its first run, and not in the pass it was written for: the
    // non-tail-recursion lint's `walk` recursed unguarded, so a deep body inside a
    // `(def n (fn …))` aborted the process — the 2026-07-23 host-panic pass hardened
    // that lint's entry point and not the recursion that descends the body.
    let interp = crate::Interp::new();
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let identity = crate::core::value::intern("identity");
    let x = crate::core::value::intern("x");
    // (identity (identity … x))
    let mut body = Value::Sym(x);
    for _ in 0..20_000 {
        let tail = heap.alloc_pair(body, Value::Nil);
        body = heap.alloc_pair(Value::Sym(identity), tail);
    }
    // (def deep (fn (x) <body>)) — the shape Pass 2.8 infers a parameter domain from.
    let params = heap.alloc_pair(Value::Sym(x), Value::Nil);
    let fn_tail = heap.alloc_pair(body, Value::Nil);
    let fn_parts = heap.alloc_pair(params, fn_tail);
    let fn_form = heap.alloc_pair(Value::Sym(crate::core::value::intern("fn")), fn_parts);
    let def_tail = heap.alloc_pair(fn_form, Value::Nil);
    let def_parts = heap.alloc_pair(Value::Sym(crate::core::value::intern("deep")), def_tail);
    let def_form = heap.alloc_pair(Value::Sym(crate::core::value::intern("def")), def_parts);
    let _ = check_file(&mut heap, &[def_form]);
}

#[test]
fn a_branch_union_domain_rejects_what_no_branch_admits() {
    let ws = file_warnings(
        "(defn f (x) (if (string? x) (string/length x) (+ x 1)))\n(defn c () (f :kw))",
    );
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_branch_union_domain_admits_what_either_branch_admits() {
    // Both members of the union must stay silent — this is the false-positive class
    // the unconditional-demand rule was protecting against.
    for arg in ["\"s\"", "5"] {
        let ws = file_warnings(&format!(
            "(defn f (x) (if (string? x) (string/length x) (+ x 1)))\n(defn c () (f {arg}))"
        ));
        assert!(
            !ws.iter().any(|w| w.contains("f: argument 1")),
            "arg {arg}: {ws:?}"
        );
    }
}

#[test]
fn a_guard_that_proves_nothing_leaves_the_branch_unconstrained() {
    // `(if b …)` on an unrelated variable: whichever branch runs, one of the two
    // demands must hold — but neither alone may constrain.
    let ws = file_warnings(
        "(defn f (b x) (if b (string/length x) (+ x 1)))\n(defn c () (f true \"s\"))",
    );
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
    // …and a value no branch admits is still caught.
    let ws =
        file_warnings("(defn f (b x) (if b (string/length x) (+ x 1)))\n(defn c () (f true :kw))");
    assert!(
        ws.iter().any(|w| w.contains("f: argument 2 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_when_body_does_not_constrain_what_the_test_does_not_reach() {
    // `(when test body)` runs `body` only sometimes, so an argument the body would
    // reject is not provably wrong — unless the test itself pins the argument.
    let ws = file_warnings("(defn f (b x) (when b (string/length x)))\n(defn c () (f true 5))");
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
}

#[test]
fn a_match_domain_comes_from_its_clause_patterns() {
    // Every clause's pattern is a guard; the no-clause-matched branch raises, so its
    // domain is `never` and the clauses' patterns add up to the function's domain.
    let ws = file_warnings("(defn f (x) (match x ((:ok v) v) ((:error e) e)))\n(defn c () (f 5))");
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_destructuring_head_constrains_the_argument() {
    let ws = file_warnings("(defn f ([a b]) (+ a b))\n(defn c () (f 5))");
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_clause_guard_constrains_the_argument() {
    let ws = file_warnings("(defn f (x) :when (string? x) (string/length x))\n(defn c () (f 5))");
    assert!(
        ws.iter()
            .any(|w| w.contains("f: argument 1 expects string")),
        "{ws:?}"
    );
}

#[test]
fn an_unexpanded_macro_body_does_not_leak_a_demand() {
    // A prelude closure keeps its body *as written*, so the walk meets `(cond …)`,
    // `(when …)` and user macros verbatim. Reading those as ordinary calls — every
    // operand evaluated — made `type-matches?` demand a `seqable` first argument,
    // because `(first t)` sat in a clause body. The whole-file gate caught it; these
    // pin the two rules that fixed it.
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(defn tm (t v) (cond (nil? t) (nil? v) (pair? t) (first t) else true))")
        .expect("def");
    let sig = super::sigs::sig_of(&interp.heap, crate::core::value::intern("tm"))
        .expect("a sig is inferred");
    assert_eq!(
        sig.params.first().map(Ty::to_string),
        Some("any".to_string()),
        "a `cond` clause body must not constrain unconditionally"
    );
}

// ---- multi-arm functions: every clause is a signature ----
// A multi-arm closure has no single `Sig`, so its callers' arguments went entirely
// unchecked. Each arm has one, and a call no arity-relevant arm accepts is a
// provable error — the same rule ADR-116's declared overloads already used.

#[test]
fn a_call_no_clause_of_a_multi_arity_function_accepts_is_flagged() {
    let ws = file_warnings("(defn f ((x) (string/length x)) ((x y) (+ x y)))\n(defn c () (f 5))");
    assert!(
        ws.iter()
            .any(|w| w.contains("f: no clause accepts these arguments")),
        "{ws:?}"
    );
    // The arity that fits a different clause stays silent.
    let ws = file_warnings("(defn f ((x) (string/length x)) ((x y) (+ x y)))\n(defn c () (f 1 2))");
    assert!(
        !ws.iter().any(|w| w.contains("no clause accepts")),
        "{ws:?}"
    );
}

#[test]
fn guarded_clauses_give_a_function_its_domain() {
    // `:when` guards (ADR-226) lower to a single variadic `fn` over `match*`, so the
    // clauses only exist in the un-expanded form — which is where this reads them.
    let ws = file_warnings(
        "(defn g ((x) :when (string? x) (string/length x)) ((x) :when (int? x) (+ x 1)))\n\
         (defn c () (g :kw))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("g: no clause accepts these arguments")
                && w.contains("(string), (int)")),
        "{ws:?}"
    );
    // Both admitted types stay silent.
    for arg in ["\"s\"", "5"] {
        let ws = file_warnings(&format!(
            "(defn g ((x) :when (string? x) (string/length x)) ((x) :when (int? x) (+ x 1)))\n\
             (defn c () (g {arg}))"
        ));
        assert!(
            !ws.iter().any(|w| w.contains("no clause accepts")),
            "arg {arg}: {ws:?}"
        );
    }
}

#[test]
fn an_unguarded_final_clause_keeps_a_multi_clause_call_silent() {
    // A clause that admits anything is the catch-all every dispatch-style function
    // ends with; its domain is `any`, so no call can be ruled out.
    let ws = file_warnings(
        "(defn g ((x) :when (string? x) (string/length x)) ((x) x))\n(defn c () (g :kw))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("no clause accepts")),
        "{ws:?}"
    );
}

// ---- a union of shapes is checkable at a call site (ADR-262) ----

#[test]
fn a_union_of_tuple_shapes_rejects_a_member_of_neither() {
    let ws = file_warnings(
        "(sig f ((or (tuple int) (tuple string)) -> any))\n(defn f (t) t)\n(defn c () (f [true]))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("expects (tuple int) | (tuple string)")),
        "{ws:?}"
    );
    // …and admits either alternative.
    for arg in ["[1]", "[\"s\"]"] {
        let ws = file_warnings(&format!(
            "(sig f ((or (tuple int) (tuple string)) -> any))\n(defn f (t) t)\n(defn c () (f {arg}))"
        ));
        assert!(
            !ws.iter().any(|w| w.contains("f: argument")),
            "{arg}: {ws:?}"
        );
    }
}

#[test]
fn a_union_of_record_shapes_rejects_a_map_matching_neither() {
    let ws = file_warnings(
        "(sig f ((or (record :a int) (record :b int)) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:zzz 1}))",
    );
    assert!(
        ws.iter().any(|w| w.contains("expects {a: int} | {b: int}")),
        "{ws:?}"
    );
    let ws = file_warnings(
        "(sig f ((or (record :a int) (record :b int)) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:a 1}))",
    );
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
}

// ---- closed records (ADR-264) ----

#[test]
fn a_closed_record_rejects_an_undeclared_key() {
    let ws = file_warnings(
        "(sig f ((record :name string) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:name \"Ada\" :extra :k}))",
    );
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn an_open_record_admits_undeclared_keys() {
    let ws = file_warnings(
        "(sig f ((record &open :name string) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:name \"Ada\" :extra :k}))",
    );
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
    // …and still enforces what it does declare.
    let ws = file_warnings(
        "(sig f ((record &open :name string) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:name 42}))",
    );
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_field_read_through_a_tagged_union_resolves() {
    // The payoff. Each term answers for `:ok` — `int` in the first, `nil` in the
    // second (closed: the key is absent) — so the union answers `int | nil`.
    let ws = file_warnings(
        "(sig f ((or (record :ok int) (record :error string)) -> any))\n\
         (defn f (r) (string/length (get r :ok)))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("string/length") && w.contains("int")),
        "{ws:?}"
    );
    // An open alternative says nothing about the key, so the union says nothing.
    let ws = file_warnings(
        "(sig f ((or (record :ok int) (record &open :error string)) -> any))\n\
         (defn f (r) (string/length (get r :ok)))",
    );
    assert!(!ws.iter().any(|w| w.contains("string/length")), "{ws:?}");
}

#[test]
fn a_defrecord_accessor_takes_any_record_carrying_its_field() {
    // The accessor sig is `&open` by construction: a real value carries `:__id__` and
    // every sibling field, so a closed one-field shape would describe nothing.
    let ws =
        file_warnings("(defrecord point ((x int) (y int)))\n(defn c () (point-x (point 1 2)))");
    assert!(!ws.iter().any(|w| w.contains("point-x")), "{ws:?}");
}

#[test]
fn a_bare_local_test_narrows_by_truthiness() {
    // `(if v …)` is itself a guard: only `nil` and `false` are falsy, so the
    // then-branch has `v` as neither. This is what `if-let`/`when-let` expand to, and
    // without it a closed literal's `nil` read as a false positive there.
    let ws = warnings("(let (v (get {:x 10} :y)) (if v (inc v) :none))");
    assert!(!ws.iter().any(|w| w.contains("inc")), "{ws:?}");
    // **Biconditional**, now that `¬{false}` is exactly `{true}`: the else-branch has
    // `v` falsy, so a use that needs a number there is a real error and is caught.
    // (While the truthy type was only approximable as `not nil`, this had to stay
    // one-sided — its complement, `nil`, is not implied by a false test.)
    let ws = warnings("(fn (x) (let (v (if (int? x) 1 nil)) (if v :ok (inc v))))");
    assert!(
        ws.iter().any(|w| w.contains("inc") && w.contains("nil")),
        "{ws:?}"
    );
    // …and `(not v)` must NOT read as "v is nil": that inversion reported live code
    // as dead when the guard was two-sided.
    // (The condition is a real predicate, not a literal: `(if true …)` now folds to its
    // then-branch, so `s` would be exactly `true`, `v` exactly `false`, and the then-branch
    // of `(if v 1 2)` REALLY dead — which the lint is right to say.)
    let ws = warnings_expanded(
        "(fn (x) (let (s (if (int? x) true false)) (let (v (not s)) (if v 1 2))))",
    );
    assert!(!ws.iter().any(|w| w.contains("unreachable")), "{ws:?}");
}
