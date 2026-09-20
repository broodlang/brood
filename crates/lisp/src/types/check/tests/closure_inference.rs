//! Step 3: sigs sourced from `NativeFn`, and closure inference — straight-line wrappers, fixpoints, recursion, multi-arity and variadic returns.

use super::*;

#[test]
fn primitive_sigs_are_read_from_native_fn() {
    // The point of Step 3: there is no parallel `primitive_sig` table.
    // The sig the checker uses for `string-length` *is* the one declared
    // next to its `Arity` in `builtins.rs`. If we ever drop the sig field
    // (or set it wrong), this catches it.
    let interp = crate::Interp::new();
    let sig = primitive_sig(&interp.heap, crate::core::value::intern("string/length"))
        .expect("string-length is a primitive");
    assert_eq!(sig.params, vec![Ty::of(Tag::Str)]);
    assert_eq!(sig.ret, Ty::of(Tag::Int));
    // The "no useful info" lane: a variadic any-arg primitive (str) returns
    // a Sig that param-overlaps every input, so it never warns.
    let any_sig =
        primitive_sig(&interp.heap, crate::core::value::intern("str")).expect("str is a primitive");
    assert_eq!(any_sig.rest, Some(Ty::ANY));
}

#[test]
fn file_defn_shadowing_a_builtin_wins_over_its_signature() {
    // A file's own `defn %bytes->list` supersedes the `%bytes->list` builtin (ADR-123: a def
    // always wins) — the checker must not type its calls with the builtin's
    // list-returning signature. This exact shape (the bintree bench, which
    // spelled it `check` before that builtin moved to `reflect/check`)
    // produced "+: argument 2 expects number, got list" plus a phantom arity
    // from the builtin's 1-arg Arity.
    let w = file_warnings(
        "(defn %bytes->list (node) (if (nil? node) 1 (+ 1 (%bytes->list (nth node 0)))))\n\
             (io/puts (%bytes->list nil))",
    );
    assert!(
        !w.iter().any(|s| s.contains("expects")),
        "stale builtin signature leaked into a shadowed call: {:?}",
        w
    );
    // Arity from the stale builtin must not leak either: the builtin `%bytes->list`
    // is 1-ary, the file's redefinition is 2-ary.
    let w = file_warnings("(defn %bytes->list (a b) (+ a b))\n(io/puts (%bytes->list 1 2))");
    assert!(
        !w.iter().any(|s| s.contains("argument")),
        "stale builtin arity leaked into a shadowed call: {:?}",
        w
    );
    // No over-suppression: the real builtin (not redefined) still warns.
    let w = file_warnings("(io/puts (+ 1 (%bytes->list (bytes 1))))");
    assert!(
        w.iter().any(|s| s.contains("expects number")),
        "the un-shadowed builtin's signature should still warn: {:?}",
        w
    );
}

#[test]
fn infers_a_straight_line_wrapper() {
    // (defn bump (x) (+ x 1)) → x : number (from +'s rest type). Not named `inc`:
    // this fixture is *evaluated*, and a shipped name is reserved (ADR-166).
    // So `(bump :k)` is a provable misuse.
    let w = check_with_defs(&["(defn bump (x) (+ x 1))"], "(bump :k)");
    assert!(
        w.iter().any(|s| s.contains("bump") && s.contains("number")),
        "expected a `bump :k` warning, got {:?}",
        w
    );
}

#[test]
fn inferred_return_type_propagates() {
    // (defn bump (x) (+ x 1)) returns the number `+` returns; feeding it into
    // `string-length` (wants string) is a provable misuse. (Not `inc` — the fixture
    // is evaluated, and shipped names are reserved, ADR-166.)
    let w = check_with_defs(&["(defn bump (x) (+ x 1))"], "(string/length (bump 1))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "expected a `string-length` warning, got {:?}",
        w
    );
}

#[test]
fn inferred_params_intersect_across_positions() {
    // (defn add (x y) (+ x y)) — both x and y at + positions → number.
    let w = check_with_defs(&["(defn add (x y) (+ x y))"], "(add \"a\" 2)");
    assert!(w.iter().any(|s| s.contains("add")), "got {:?}", w);
}

#[test]
fn same_file_caller_checked_against_inferred_return() {
    // The file being checked isn't loaded, so this exercises Pass 2.8's form-based inference:
    // `dbl` is inferred (same-file) to return a number, so `(string/length (dbl 5))` is caught
    // — a same-file caller now gets the checking a loaded-function caller already did.
    let w = file_warnings(
        "(defmodule t)\n(defn dbl (x) (+ x 1))\n(defn bad () (string/length (dbl 5)))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "same-file inferred return should flow to a caller: {w:?}"
    );
}

#[test]
fn same_file_forward_reference_resolves_via_fixpoint() {
    // Caller defined BEFORE callee — the bounded fixpoint still resolves `later`'s return.
    let w = file_warnings("(defmodule t)\n(defn bad () (+ 1 (later 1)))\n(defn later (x) (str x))");
    assert!(
        w.iter().any(|s| s.contains('+') && s.contains("number")),
        "a forward reference should resolve in the fixpoint: {w:?}"
    );
}

#[test]
fn same_file_reassigned_global_return_stays_dynamic() {
    // SOUNDNESS: a lazily-initialized global (nil default, reassigned to a table) must make
    // the returning function's return *dynamic*, not the stale `nil` — else a table use of
    // the result would false-flag. Guards Pass 2.8 against the earmuffed / reassigned-global
    // imprecision.
    let w = file_warnings(
        "(defmodule t)\n(def *g* nil)\n(defn getg () (when (nil? *g*) (def *g* (%table))) *g*)\n(defn u () (%table-get (getg) :k))",
    );
    assert!(
        !w.iter()
            .any(|s| s.contains("%table-get") && s.contains("argument")),
        "a reassigned global's return must stay dynamic (no false positive): {w:?}"
    );
}

#[test]
fn infers_a_tail_recursive_function_return_from_its_base_case() {
    // A self-recursive call in a branch position contributes ⊥ to the return union, so
    // `count-down`'s return infers from its base case `:done` (keyword) — feeding it to
    // `string-length` (wants string) is then a provable misuse. Before this, the self-call
    // made the return uninferrable and the misuse went uncaught.
    let w = check_with_defs(
        &["(defn count-down (n) (if (<= n 0) :done (count-down (- n 1))))"],
        "(string/length (count-down 5))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "a recursive fn's base-case return should flow to its caller: {w:?}"
    );
}

#[test]
fn an_accumulator_loop_returns_what_the_accumulator_grows_into() {
    // `acc` is an unconstrained parameter, so the FLAT inference says `-> any`; the
    // call-site fixpoint (`sigs::specialize_recursive`) reads the recursive call passing
    // `(+ acc (first xs))` and settles on `int[0..]` — the elements are `1 | 2` (the
    // self-call sits in the else of `(empty? xs)`, so `(first xs)` is never nil there) and
    // the accumulator's ascent `0`, `0 | 1 | 2`, … widens to its infinity (ADR-350).
    // `(sum-acc (list 1 2) 0)` IS `3`, and a string function on it is a real finding, not
    // a false positive. (This test used to pin the opposite, when the fixpoint did not
    // exist and declining was the sound answer.)
    let w = check_with_defs(
        &["(defn sum-acc (xs acc) (if (empty? xs) acc (sum-acc (rest xs) (+ acc (first xs)))))"],
        "(string/length (sum-acc (list 1 2) 0))",
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length: argument 1 expects string, got int[0..]")),
        "{w:?}"
    );
    // SOUNDNESS: a seed the call site does not know keeps the result unknown — the
    // fixpoint over-approximates from `inputs`, and an unknown input stays unknown.
    let w = check_with_defs(
        &["(defn sum-acc (xs acc) (if (empty? xs) acc (sum-acc (rest xs) (+ acc (first xs)))))"],
        "(defn use-it (k) (string/length (sum-acc (list 1 2) k)))",
    );
    assert!(!w.iter().any(|s| s.contains("string/length")), "{w:?}");
}

#[test]
fn infers_a_multi_arity_return_as_the_union_of_its_arms() {
    // A multi-arity closure has no single param signature, but its return is the union of
    // each arm's tail — here `:one | :two`. Feeding that to `string-length` (wants string)
    // is a provable misuse. (Before, a multi-arity closure was skipped entirely.)
    let w = check_with_defs(
        &["(defn describe ((x) :one) ((x y) :two))"],
        "(string/length (describe 5))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "a multi-arity fn's union return should flow: {w:?}"
    );
}

#[test]
fn infers_a_variadic_return() {
    // A rest-param closure was skipped before; now its return (`(str a)` → string) flows, so
    // feeding it to `+` (wants a number) is caught.
    let w = check_with_defs(&["(defn joiner (a & xs) (str a))"], "(+ 1 (joiner \"x\"))");
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("number")),
        "a variadic fn's return should flow: {w:?}"
    );
}

#[test]
fn complex_closure_return_only_keeps_arity_checking() {
    // The return-only sig is params-less, but arity is checked independently (`arity_of`),
    // so a wrong-arity call to the multi-arity fn is still flagged — no regression.
    let w = check_with_defs(
        &["(defn describe ((x) :one) ((x y) :two))"],
        "(describe 1 2 3)",
    );
    assert!(
        w.iter()
            .any(|s| s.contains("describe") && s.contains("arg")),
        "arity checking must survive return-only inference: {w:?}"
    );
}

#[test]
fn does_not_infer_through_branches_or_lets() {
    // A body with `if`/complex `let` is *not* a single straight-line expression
    // — inference must skip it, leaving the closure untyped (no warning).
    // (A plain let-alias `(let (y x) call)` IS inferred — see below.)
    let w = check_with_defs(&["(defn maybe (x) (if (int? x) (+ x 1) x))"], "(maybe :k)");
    assert!(
        w.is_empty(),
        "if-branching bodies must not infer (so no warning): {:?}",
        w
    );
}

#[test]
fn infers_through_let_alias() {
    // `(let (y x) call)` where y is just a rename of closure param x:
    // the body is still one straight-line call — inference should work.
    let w = check_with_defs(
        &["(defn double (x) (let (y x) (* y 2)))"],
        "(string/length (double 3))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "let-alias wrapper should not block infer_sig: {:?}",
        w
    );
    // The param type is also inferred: `y` at number position → x : number.
    let w = check_with_defs(&["(defn double (x) (let (y x) (* y 2)))"], "(double :k)");
    assert!(
        w.iter()
            .any(|s| s.contains("double") && s.contains("number")),
        "let-alias: param type should propagate from callee: {:?}",
        w
    );
    // A non-param let (binding a computed value) isn't peeled by the precise
    // *parameter*-inferring tier — but the sound **return-only** tier still
    // infers `wrap`'s result as `number` (`wrap 3` = 8), so a real misuse of
    // that result is caught (`(string/length 8)` genuinely errors at runtime).
    let w = check_with_defs(
        &["(defn wrap (x) (let (y (+ x 1)) (* y 2)))"],
        "(string/length (wrap 3))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "return-only inference should type wrap's result as number: {:?}",
        w
    );
    // …and the *parameter* IS inferred here too: `(+ x 1)` is a `let`-binding RHS,
    // which always executes when `wrap` is called (it dominates the body), so `x`
    // genuinely must be a number — `(wrap :k)` errors at runtime. The unconditional-
    // demand tier (`collect_param_demands`) catches it. (A *guarded* use — `(+ x 1)`
    // inside an `if`/`cond`/`and`-tail — would stay unconstrained; see
    // `param_inference_skips_guarded_uses`.)
    let w = check_with_defs(&["(defn wrap (x) (let (y (+ x 1)) (* y 2)))"], "(wrap :k)");
    assert!(
        w.iter().any(|s| s.contains("wrap") && s.contains("number")),
        "let-RHS is an unconditional demand: param should be inferred number: {:?}",
        w
    );
}

#[test]
fn param_inference_from_unconditional_positions() {
    // A parameter passed *directly* to a known-sig callee in a position that always
    // runs is inferred, even when the top-level body isn't a single call.

    // (a) Nested call argument: `(+ x 1)` is an argument to `f` (both always run),
    // so `x : number` — a keyword arg genuinely errors.
    let w = check_with_defs(&["(defn g (x) (list (+ x 1)))"], "(g :k)");
    assert!(
        w.iter().any(|s| s.contains("g") && s.contains("number")),
        "nested-call arg is an unconditional demand: {:?}",
        w
    );

    // (b) `do` form: every form runs; the last demands `x : number`.
    let w = check_with_defs(&["(defn h (x) (do 1 (+ x 1)))"], "(h :k)");
    assert!(
        w.iter().any(|s| s.contains("h") && s.contains("number")),
        "a do-form body is unconditional: {:?}",
        w
    );

    // (c) A demand through an unknown/user callee's argument still fires (the demand
    // comes from the inner *known* callee, not the outer unknown one).
    let w = check_with_defs(
        &["(defn user-sink (v) v)", "(defn k (x) (user-sink (* x 2)))"],
        "(k :k)",
    );
    assert!(
        w.iter().any(|s| s.contains("k") && s.contains("number")),
        "demand flows from the inner known callee: {:?}",
        w
    );
}

#[test]
fn param_inference_skips_guarded_uses() {
    // The soundness guard: a param used only inside a branch / guard / short-circuit
    // tail is NOT constrained — those positions don't always execute, so a
    // differently-typed argument must never warn.

    // (a) `if` branch (classic type-test guard): `x` is number only in the then-arm.
    let w = check_with_defs(&["(defn f (x) (if (number? x) (+ x 1) x))"], "(f :k)");
    assert!(
        w.is_empty(),
        "guarded (if-branch) use must not constrain: {:?}",
        w
    );

    // (b) `and`/`or` tail: only the first operand is unconditional.
    let w = check_with_defs(&["(defn f (x) (or (cached? x) (+ x 1)))"], "(f :k)");
    assert!(w.is_empty(), "or-tail use must not constrain: {:?}", w);

    // (c) `when` body is conditional on its test.
    let w = check_with_defs(&["(defn f (x) (when (ready?) (+ x 1)))"], "(f :k)");
    assert!(w.is_empty(), "when-body use must not constrain: {:?}", w);

    // (d) `try` body deliberately exercises failures — never constrain from it.
    let w = check_with_defs(&["(defn f (x) (try (+ x 1) (catch _ 0)))"], "(f :k)");
    assert!(w.is_empty(), "try-body use must not constrain: {:?}", w);
}

#[test]
fn param_inference_respects_shadowing() {
    // An inner `let` that rebinds the parameter's name hides it: the `(+ x 1)` here
    // refers to the let's `x` (a fresh unknown value), NOT the parameter, so the
    // parameter stays unconstrained and `(f :k)` must not warn.
    let w = check_with_defs(&["(defn f (x) (let (x (something)) (+ x 1)))"], "(f :k)");
    assert!(
        w.is_empty(),
        "a shadowing let binder must exclude the param from demand collection: {:?}",
        w
    );
}

#[test]
fn earmuffed_global_types_as_unknown_not_its_default() {
    // A `*earmuffed*` global is dynamic by convention — declared with a `nil` default
    // but reassigned at runtime (e.g. `*project-root*`) — so the checker must NOT pin
    // it to `nil` and flag a string-demanding use. (Regression guard for the false
    // positive the sound param-inference tier would otherwise surface at, e.g.,
    // `(path-join *project-root* rel)` after its `nil?` guard.)
    let w = check_with_defs(&["(def *root* nil)"], "(string/length *root*)");
    assert!(
        w.is_empty(),
        "earmuffed global must type as unknown, not its default nil: {:?}",
        w
    );
    // A non-earmuffed global is still pinned to its value (unchanged behaviour): a
    // real disjoint use is still caught.
    let w = check_with_defs(&["(def plain-root nil)"], "(string/length plain-root)");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "a plain (non-earmuffed) global is still typed by its value: {:?}",
        w
    );
}

#[test]
fn return_only_inference_is_sound() {
    // The return type of a branchy/multi-step body is inferred (sound: it's a
    // union of the possible results), so misusing the *result* is caught…
    let w = check_with_defs(
        &["(defn pick (c) (if c 1 2))"],
        "(string/length (pick true))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "a numeric-returning branchy body's result misuse must warn: {w:?}"
    );
    // …but a parameter used as a number only *inside a guard* must NOT be
    // inferred as number — that's the guarded-use false positive full param
    // inference would create. `(g "x")` is valid (returns 0), so no warning.
    let w = check_with_defs(&["(defn g (x) (if (number? x) (+ x 1) 0))"], "(g \"x\")");
    assert!(
        w.is_empty(),
        "a guarded numeric use must not infer the parameter as number: {w:?}"
    );
    // A union result that *overlaps* the sink must not warn (int | string fed
    // to `+` — the int arm overlaps `number`).
    let w = check_with_defs(&["(defn u (c) (if c 1 \"s\"))"], "(+ 1 (u true))");
    assert!(
        w.is_empty(),
        "a result overlapping the expected type must not warn: {w:?}"
    );
    // Recursion terminates (the re-entry guard) and stays sound — no hang,
    // no spurious warning on a valid use.
    let w = check_with_defs(
        &["(defn rfac (n) (if (< n 1) 1 (* n (rfac (- n 1)))))"],
        "(+ 1 (rfac 5))",
    );
    assert!(
        w.iter().all(|s| !s.contains("expects")),
        "recursive-body return inference must stay sound: {w:?}"
    );
}

#[test]
fn does_not_infer_through_recursion() {
    // A self-recursive call has no fixed sig to read from — must skip,
    // even though the body is structurally a single call.
    let w = check_with_defs(&["(defn go (x) (go x))"], "(go :k)");
    assert!(w.is_empty(), "recursive defns must not infer: {:?}", w);
}

#[test]
fn skips_inference_for_variadic_or_optional_closures() {
    // A variadic-tail closure isn't a "fixed-arity straight-line" — skip.
    let w = check_with_defs(&["(defn vlist (& xs) (first xs))"], "(vlist 1 2 3)");
    assert!(w.is_empty(), "variadic defns must not infer: {:?}", w);
}

// ---- a `let`-bound `fn` literal constrains the calls it heads ----
// The literal's arrow type is `(any… -> R)` on purpose (see `sigs::let_bound_lambda_sig`
// for the contravariance argument), so its parameter DOMAINS travel as a per-name fact
// instead: `(g "x")` under `(let (g (fn (b) (+ 1 b))) …)` is the same finding a same-file
// `defn` would give. Sabotage-verified: without the `let_fn_sig` lookup in `walk.rs` the
// first two cases pass in silence.

#[test]
fn a_let_bound_lambda_checks_its_arguments_against_its_domain() {
    let ws = file_warnings("(defn f () (let (g (fn (b) (+ 1 b))) (g \"x\")))");
    assert!(
        ws.iter()
            .any(|w| w.contains("g: argument 1 expects number, got \"x\"")),
        "{ws:?}"
    );
    // A guarded use is credited only within its guard — the domain rule, not a demand.
    let ws = file_warnings("(defn f () (let (g (fn (b) (if (int? b) (+ 1 b) b))) (g \"x\")))");
    assert!(ws.is_empty(), "{ws:?}");
    // The arity comes with it, even when the result cannot be typed.
    let ws = file_warnings("(defn f (h) (let (g (fn (b) (h b))) (g 1 2)))");
    assert!(
        ws.iter()
            .any(|w| w.contains("g: expected 1 argument, got 2")),
        "{ws:?}"
    );
}

#[test]
fn a_let_bound_lambda_domain_is_scoped_and_never_its_arrow() {
    // A rebinding of the name shadows the fact.
    let ws = file_warnings("(defn f (k) (let (g (fn (b) (+ 1 b))) (let (g k) (g \"x\"))))");
    assert!(ws.is_empty(), "{ws:?}");
    // The literal HANDED ON keeps its `(any -> R)` arrow: an inferred callback slot's
    // `any` means unknown, and a `(number -> number)` there would be a false positive.
    let ws = file_warnings(
        "(defn each-of (xs f) (map xs f))\n\
         (defn f (xs) (let (g (fn (b) (+ 1 b))) (each-of xs g)))",
    );
    assert!(ws.is_empty(), "{ws:?}");
    let ws = file_warnings("(defn f (xs) (let (g (fn (b) (+ 1 b))) (map xs g)))");
    assert!(ws.is_empty(), "{ws:?}");
}

// ---- the self-recursive fixpoint (`sigs::specialize_recursive`) ----
// Every case is a FILE check (Pass 2.8 + call-site specialization), which is the path a
// project's own accumulator loops take. Sabotage-verified: with the fixpoint replaced by
// the old decline, the first three go silent.

#[test]
fn a_tail_recursive_accumulator_is_typed_at_its_call_site() {
    // `(number any -> any)` flat; `int` at this call.
    let ws = file_warnings(
        "(defn sum-to (i acc) (if (= i 0) acc (sum-to (- i 1) (+ acc i))))\n\
         (defn use-it () (string/length (sum-to 10 0)))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("string/length: argument 1 expects string, got int")),
        "{ws:?}"
    );
    // A list builder: `never` → `list<int>` → stable. The element is what the else branch
    // of `(= i 0)` knows of `i` — its caller-derived `int`, minus the literal.
    let ws = file_warnings(
        "(defn build (i acc) (if (= i 0) acc (build (- i 1) (cons i acc))))\n\
         (defn use-it () (+ 1 (build 3 '())))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("+: argument 2 expects number, got nil | list<(int and (not 0))>")),
        "{ws:?}"
    );
    // A self-call in a NESTED position (`cons` of the recursive result): the classic map.
    let ws = file_warnings(
        "(defn my-map (xs f) (if (nil? xs) nil (cons (f (first xs)) (my-map (rest xs) f))))\n\
         (defn use-it () (string/length (first (my-map '(1 2 3) inc))))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("string/length: argument 1 expects string, got")),
        "{ws:?}"
    );
}

#[test]
fn the_recursive_fixpoint_keeps_the_correct_calls_silent() {
    // Every one of these is right, and each exercises a widening the fixpoint must make.
    let ws = file_warnings(
        "(defn sum-to (i acc) (if (= i 0) acc (sum-to (- i 1) (+ acc i))))\n\
         (defn build (i acc) (if (= i 0) acc (build (- i 1) (cons i acc))))\n\
         (defn my-map (xs f) (if (nil? xs) nil (cons (f (first xs)) (my-map (rest xs) f))))\n\
         (defn grow (i acc) (if (= i 0) acc (grow (- i 1) (if (= (mod i 2) 1) (cons \"s\" acc) (cons i acc)))))\n\
         (defn ok1 () (+ 1 (sum-to 10 0)))\n\
         (defn ok2 () (+ 1 (sum-to 10 0.5)))\n\
         (defn ok3 () (count (build 3 '())))\n\
         (defn ok4 () (map (my-map '(1 2 3) inc) (fn (n) (+ n 1))))\n\
         (defn ok5 () (each (grow 4 '()) (fn (x) (if (string? x) (string/length x) (+ x 1)))))\n\
         (defn ok6 (k) (string/length (sum-to 10 k)))",
    );
    // `my-map` earns the (correct) non-tail-recursion lint; only type findings matter here.
    let ws: Vec<_> = ws.into_iter().filter(|w| w.contains("expects")).collect();
    assert!(ws.is_empty(), "{ws:?}");
}

#[test]
fn the_recursive_fixpoint_declines_rather_than_under_approximate() {
    // A parameter that grows on every round — a list nesting one level deeper per
    // call — never converges as written; the call-site fixpoint declines to the flat
    // answer, and the caller-derived one WIDENS (`Ty::widened_below`) to `1 | pair`, which
    // is sound and exact enough: `(nest 3 1)` is a list, so `string/length` on it is a
    // true finding and `+` (which `1` satisfies) is not.
    let ws = file_warnings(
        "(defn nest (i acc) (if (= i 0) acc (nest (- i 1) (list acc))))\n\
         (defn use-a () (string/length (nest 3 1)))\n\
         (defn use-b () (+ 1 (nest 3 1)))",
    );
    assert_eq!(
        ws,
        vec!["string/length: argument 1 expects string, got 1 | pair ((nest 3 1))".to_string()],
        "{ws:?}"
    );
    // Two arms fit the call's arity (a `:when` overload): declined, flat answer.
    let ws = file_warnings(
        "(defn pick ((i acc) :when (= i 0) acc) ((i acc) (pick (- i 1) (+ acc i))))\n\
         (defn use-it () (string/length (pick 3 0)))",
    );
    assert!(!ws.iter().any(|w| w.contains("string/length")), "{ws:?}");
}

#[test]
fn a_self_call_argument_is_typed_in_its_enclosing_scope() {
    // `j2` is a `let` binder on the way down to the self-call. Typed under the parameters
    // alone it was unbound, `j` went unknown, and `(inc j)` read `number` (bedit's
    // `git-scan-rows`); the exact `int` below is what the scoped site walk earns. The
    // `:else` keyword (a plain truthy test to `cond`) is the spelling that showed it.
    let ws = file_warnings(
        "(defn- scan (rows j dir n)\n\
           (if (= n 0)\n\
             (inc j)\n\
             (let (j2 (+ j dir))\n\
               (cond\n\
                 (< j2 0) (inc j)\n\
                 (>= j2 (count rows)) (inc j)\n\
                 (nil? (nth rows j2)) (scan rows j2 dir n)\n\
                 :else (scan rows j2 dir (dec n))))))\n\
         (defn use-it (rows) (string/length (scan rows 3 1 1)))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("string/length: argument 1 expects string, got int")),
        "{ws:?}"
    );
}

// ---- Pass 2.9: caller-derived parameter types (ADR-341) ----
// The union of what a function's call sites in its file hand each parameter binds it in the
// walk of its body and in the return its same-file callers read — the mechanism that lets a
// leaf `sig` be enough. A fact about this file's calls, so public and private alike.
// Sabotage-verified: with `set_derived_params` never called the first two cases go silent
// and the third loses its `int`.

#[test]
fn a_private_function_is_walked_under_what_its_callers_pass() {
    // Strict: `(+ i 1)` under a demand alone reads `number`; under the callers it is `int`.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn- bump (i) (want-int (+ i 1)))\n\
         (defn pub (xs) (+ (bump 3) (bump (count xs))))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    // …and a caller that hands a string over is reported INSIDE the body too: every
    // in-file call would raise there.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (defn- bump (i) (+ i 1))\n\
         (defn pub () (bump \"s\"))",
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("+: argument 1 expects number, got \"s\" (i)")),
        "{ws:?}"
    );
}

/// A function whose only sites are its own self-calls has no base for the derivation:
/// the least fixpoint seeds every parameter at ⊥ and a self-call's arguments are typed
/// under those parameters, so it derived ⊥ throughout, its body read as dead code, and
/// `(string/length 5)` inside it was never reported — while the same body with one
/// outside caller, or with no self-call at all, was. Such a function is site-less: its
/// parameters are unknown, and its body is walked like any other.
#[test]
fn a_function_reached_only_through_itself_is_not_derived() {
    for shape in [
        // the lint in the self-call's own argument
        "(defn w (xs acc) (if (empty? xs) acc (w (rest xs) (+ acc (string/length 5)))))",
        // …beside the self-call in the recursive branch
        "(defn w (xs) (if (empty? xs) 0 (do (string/length 5) (w (rest xs)))))",
        // …and private, where a base case that nobody calls is the same shape
        "(defmodule t)\n(defn- w (xs) (if (empty? xs) 0 (do (string/length 5) (w (rest xs)))))",
    ] {
        let ws = file_warnings(shape);
        assert!(
            ws.iter()
                .any(|w| w.contains("string/length: argument 1 expects string, got 5")),
            "{shape}: {ws:?}"
        );
    }
    // The derivation itself is untouched where a real caller seeds it.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (defn- w (xs acc) (if (empty? xs) acc (w (rest xs) (+ acc 1))))\n\
         (defn pub () (w (list 1 2) 0))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
}

/// A quoted datum handed straight to `pr-str`/`str` is text: nothing can call what it
/// names. Every assertion macro expands to `(pr-str (quote (assert= … (drive 3000 0))))`
/// for its failure message, and reading `drive` there as an ESCAPE excluded every private
/// function under a test from derivation — a driver called only from its tests derived
/// from its own recursion alone, and reported `number` on `(- i 1)`. A quoted datum
/// anywhere else is still an escape: it may be `eval`ed.
#[test]
fn a_quoted_datum_printed_is_not_an_escape() {
    let driver = "(defmodule t)\n\
         (defn- drive (i acc) (if (= i 0) acc (drive (- i 1) (+ acc (bit/and i 1)))))\n";
    let ws = file_warnings_mode(
        &format!("{driver}(defn run () (str (pr-str (quote (drive 3000 0))) (drive 3000 0)))"),
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    let ws = file_warnings_mode(
        &format!("{driver}(defn run () (list (quote (drive 3000 0)) (drive 3000 0)))"),
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("drive: argument 1 expects int, got number")),
        "quoted elsewhere, the name escapes and the driver is not derived — {ws:?}"
    );
}

#[test]
fn a_private_functions_return_is_read_under_its_callers() {
    // Ten-deep in `json`, three-deep here: the index stays `int` through the chain and
    // the public function's return is re-read over it.
    let sigs = signatures(
        "(defmodule t)\n\
         (defn- step3 (i) [i (+ i 1)])\n\
         (defn- step2 (i) (let ([a b] (step3 (+ i 1))) [a b]))\n\
         (defn- step1 (i) (step2 (+ i 1)))\n\
         (defn pub (s) (step1 (string/length s)))",
    );
    let sig_of = |name: &str| {
        sigs.iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, s, _)| s.clone())
            .unwrap_or_else(|| panic!("{name}: no signature in {sigs:?}"))
    };
    assert_eq!(sig_of("t/step3"), "(number) -> (tuple int, int)");
    assert_eq!(sig_of("t/pub"), "(string) -> (tuple int, int)");
}

#[test]
fn a_private_function_that_escapes_or_has_unseen_callers_stays_unknown() {
    // Handed to `map` as a value: its arguments are whatever the combinator passes.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn- bump (i) (want-int (+ i 1)))\n\
         (defn pub (xs) (+ (bump 3) (first (map xs bump))))",
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("want-int: argument 1 expects int, got number")),
        "{ws:?}"
    );
    // Quoted (an `apply`, an `eval`, a registry): the same.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn- bump (i) (want-int (+ i 1)))\n\
         (defn pub () (+ (bump 3) (count '(bump))))",
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("want-int: argument 1 expects int, got number")),
        "{ws:?}"
    );
    // A PUBLIC function derives too: the type is a fact about this file's calls, and a
    // caller in another file reads the demand-based loaded inference as before.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn bump (i) (want-int (+ i 1)))\n\
         (defn pub () (bump 3))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
}

#[test]
fn a_function_handed_to_a_combinator_is_called_with_what_it_promises() {
    // `(map xs bump)` calls `bump` with `xs`'s elements: a site of the element type, not
    // an escape. Strict: `(+ i 1)` under the callers is `int`.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn- bump (i) (want-int (+ i 1)))\n\
         (defn pub (xs) (+ (bump 3) (count (map (map xs (fn (s) (string/length s))) bump))))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    // …and an element the body cannot take is reported inside it, as a direct call is.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (defn- bump (i) (+ i 1))\n\
         (defn pub () (map [\"a\" \"b\"] bump))",
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("+: argument 1 expects number, got \"a\" | \"b\" (i)")),
        "{ws:?}"
    );
    // A fold hands its accumulator and an element; the accumulator is the fold's own
    // type, a joint fixpoint with the callback's return.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn- add (acc x) (want-int (+ acc (string/length x))))\n\
         (defn pub (xs) (fold xs 0 add))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    // A callee with a declared arrow hands what the arrow says.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (sig each2 ((int int -> any) -> any))\n\
         (defn each2 (f) (f 1 2))\n\
         (defn- add (a b) (want-int (+ a b)))\n\
         (defn pub () (each2 add))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    // Handed somewhere with no promise (`apply`): still an escape, hence unknown.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn- bump (i) (want-int (+ i 1)))\n\
         (defn pub (xs) (+ (bump 3) (apply bump xs)))",
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("want-int: argument 1 expects int, got number")),
        "{ws:?}"
    );
}
#[test]
fn a_call_site_under_a_stored_guard_is_narrowed_like_the_walk() {
    // `and` stores each conjunct in a temporary — `(let (g (int? y)) (if g …))` — and the
    // site collector binds a `let` by the walk's one rule (`let_bind_scope`), so the guard
    // alias narrows `y` at the site: `days-in-month` is handed `int`, not `nil | int`.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig leap? (int -> bool))\n\
         (defn leap? (y) (= (math/rem y 4) 0))\n\
         (defn days-in (y m) (if (= m 2) (if (leap? y) 29 28) 30))\n\
         (defn parse (s) (let (y (string/->number s) m 2) (and (int? y) (> m 0) (days-in y m))))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
}

#[test]
fn a_falsy_or_refutes_every_disjunct_in_the_else_branch() {
    // `(or A (nil? root) C)` false ⇒ `root` is not `nil` — each biconditional disjunct
    // narrows its own variable, no shared variable needed.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-str (string -> string))\n\
         (defn want-str (s) s)\n\
         (defn pub (root files flag) (if (or flag (nil? root) (empty? files)) 0 (want-str root)))\n\
         (defn go () (pub (os/env \"HOME\") '(1) false))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
    // …and a `then_only` disjunct (an `and`) proves nothing of its variable when falsy.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn pub (x) (if (or (and (string? x) (> (string/length x) 3)) false) 0 (want-int x)))\n\
         (defn go () (pub (if (> (count (os/args)) 0) \"s\" 1)))",
        true,
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("want-int: argument 1 expects int, got")),
        "{ws:?}"
    );
}

#[test]
fn a_call_site_is_read_in_the_scope_the_walk_sees() {
    // A guard on the way to the call narrows the argument (`(and j (f j))` binds a
    // temporary aliased to `j`), and a `let` binder types it — so the callee's `i` is
    // `int`, never `nil | int`.
    let ws = file_warnings_mode(
        "(defmodule t)\n\
         (sig want-int (int -> int))\n\
         (defn want-int (x) x)\n\
         (defn- next-of (i) (want-int (+ i 1)))\n\
         (defn- maybe (i) (if (> i 0) i nil))\n\
         (sig pub (int -> any))\n\
         (defn pub (n) (let (j (maybe n)) (and j (let (k (next-of j)) (next-of k)))))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
}

// ---- recursive types from the fixpoint (ADR-349) ----

#[test]
fn a_recursive_value_type_folds_into_a_rec_instead_of_nesting_forever() {
    // A JSON-shaped decoder: a value is nil, a number, or a vector of values. The
    // return used to nest one level deeper per round until the widening cut it at a
    // depth; it is now the recursive type, exactly, and a caller reads it through the
    // binder — the element of the vector arm is the value type again.
    let sigs = signatures(
        "(defmodule t)\n\
         (defn- val (s i) (if (= (nth s i) \"[\") (arr s (+ i 1) []) (if (= (nth s i) \"n\") nil 1)))\n\
         (defn- arr (s i acc) (if (= (nth s i) \"]\") acc (arr s (+ i 1) (conj acc (val s i)))))\n\
         (defn decode (s) (val s 0))\n\
         (defn use-it (s) (let (v (decode s)) (if (vector? v) (first v) v)))",
    );
    let sig_of = |name: &str| {
        sigs.iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, s, _)| s.clone())
            .unwrap_or_else(|| panic!("{name}: no signature in {sigs:?}"))
    };
    assert_eq!(
        sig_of("t/decode"),
        "(seqable) -> (rec X 1 | nil | vector<X>)"
    );
    // `(first v)` on the vector arm is a value again (or nil for the empty vector), and
    // the union with the other arms reads as the recursive type plus its own parts.
    let use_it = sig_of("t/use-it");
    assert!(use_it.contains("(rec X 1 | nil | vector<X>)"), "{use_it}");
}

// ---- a `let`-bound `fn` literal's parameters are DERIVED from its callers ----
// The same derivation a module-private `defn` gets (ADR-341), scoped to the binding: the
// later bindings and the body are every form the name is visible in, so a parameter is the
// union of what those sites hand it — a direct call's argument, or what a combinator
// promises (`(map (range …) row-op)` hands an int). Before this the parameters were
// unknown, `(+ y k)` read `number`, and its use as an `int` was a strict finding — fifteen
// of bedit's, every one a local helper over an index — while the same literal written
// inline in the `map` was typed from the element.

#[test]
fn a_let_bound_lambda_derives_its_parameters_from_its_callers() {
    // Handed to a combinator: the element type.
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (sig takes-int (int -> int))\n\
         (defn takes-int (n) (inc n))\n\
         (sig lam (int -> list))\n\
         (defn lam (y)\n\
           (let (row-op (fn (k) (takes-int (+ y k))))\n\
             (map (range 0 3) row-op)))",
        true,
    );
    assert!(ws.is_empty(), "the combinator hands `k` an int — {ws:?}");
    // Called directly: the argument's type, and the result under it feeds the return check.
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (sig direct (int -> int))\n\
         (defn direct (y)\n\
           (let (f (fn (k) (+ y k)))\n\
             (f 2)))",
        true,
    );
    assert!(ws.is_empty(), "`k` is 2, so the body is an int — {ws:?}");
    assert_eq!(ty_str("(let (f (fn (k) (+ 1 k))) (f 2))"), "3");
    // A genuine mismatch is now visible through the derivation.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig takes-int (int -> int))\n\
         (defn takes-int (n) (inc n))\n\
         (defn wrong (y)\n\
           (let (f (fn (k) (takes-int k)))\n\
             (f \"s\")))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("takes-int: argument 1 expects int, got \"s\"")),
        "{ws:?}"
    );
}

#[test]
fn a_let_bound_lambda_that_escapes_derives_nothing() {
    // Handed somewhere with no promise about what it will be called with: unknown, and
    // the body's `(+ y k)` stays the honest `number` it always was — no finding either way.
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (sig keep (int -> any))\n\
         (defn keep (y)\n\
           (let (f (fn (k) (+ y k)))\n\
             (spawn (fn () (f 1)))\n\
             f))",
        true,
    );
    assert!(!ws.iter().any(|w| w.contains("never")), "{ws:?}");
    assert_eq!(
        ty_str("(let (f (fn (k) (+ 1 k))) [f])"),
        "(tuple (any) -> number)"
    );
    // A defensive guard over a derived parameter is not "never true": the callers that
    // would exercise it are not here yet (the private-`defn` rule).
    let ws = file_warnings("(defn f () (let (g (fn (b) (if (int? b) (+ 1 b) b))) (g \"x\")))");
    assert!(ws.is_empty(), "{ws:?}");
}

// A combinator handed a `let`-bound literal reads the literal's derived result: `(map xs f)`
// is `list<R>`, not a bare `list`. `callback_ret` declined every lexical local; a local whose
// TYPE is an arrow — the derived literal, or a parameter declared `(int -> string)` — has a
// result it can answer with.
#[test]
fn a_combinator_reads_a_let_bound_lambdas_derived_result() {
    assert_eq!(
        ty_str("(let (f (fn (k) (+ 1 k))) (map [1 2] f))"),
        "list<int[2..3]>[2]"
    );
    assert_eq!(
        ty_str("(let (f (fn (k) (str k))) (map [1 2] f))"),
        "list<string>[2]"
    );
    // A declared arrow parameter answers the same way.
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (sig apply-all ((int -> string) -> (list string)))\n\
         (defn apply-all (f) (map [1 2 3] f))",
        true,
    );
    assert!(ws.is_empty(), "{ws:?}");
}

// The derivation reaches through a `let`-bound literal in BOTH directions: the walk binds
// the literal's parameters from its callers (above), and the site collector — what a
// same-file function's parameters are derived from (ADR-341) — walks the literal's body
// under the same types. Before this the collector bound the literal's parameters to
// nothing, so a private helper called from inside one derived `number` from `(+ l dir)`
// where the walk saw `int`: bedit's `ed-blank-line?` from the `step` of `ed-blank-run`.
#[test]
fn a_site_inside_a_let_bound_lambda_hands_the_derived_type() {
    let program = |arg: &str| {
        format!(
            "\
             (defmodule t)\n\
             (defn- blank? (text line) (= \"\" (string/trim (string/substring text line))))\n\
             (sig run (string int int -> int))\n\
             (defn run (text line dir)\n\
               (let (n (string/length text)\n\
                     step (fn (l)\n\
                            (let (next (+ l dir))\n\
                              (if (and (>= next 0) (< next n) (blank? text next)) (step next) l))))\n\
                 (if (blank? text {arg}) (step line) line)))"
        )
    };
    let ws = file_warnings_mode(&program("line"), true);
    assert!(
        ws.is_empty(),
        "`next` is an int under the derived `l` — {ws:?}"
    );
    // The helper's parameter is still DERIVED (not declared): a wrong caller shows.
    let ws = file_warnings_mode(&program("\"s\""), true);
    assert!(
        ws.iter()
            .any(|w| w.contains("string/substring") && w.contains("string")),
        "{ws:?}"
    );
}

// Three derivation gaps bedit paid for with a `sig` on a derived function (2026-09-16),
// each a place the site collector or the return inference read less than the walk knew.

// (1) An `&optional` function's body was walked by the collector with every parameter
// unknown — `fixed_arms_of_form` does not count it as a fixed arm, and only a `& rest`
// arm had a declared-sig fallback — so a self-recursive accumulator it called derived
// `number` from `(- limit x)` under unknowns, where the walk seeded the declared ints.
#[test]
fn a_site_inside_an_optional_function_hands_its_declared_types() {
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (defn- fit (s w)\n\
           (if (> (string/display-width s) w) (string/substring s 0 (string/width->index s w)) s))\n\
         (defn- paint (row x column limit chunks acc)\n\
           (if (or (empty? chunks) (>= x limit))\n\
             (reverse acc)\n\
             (let (s (first chunks)\n\
                   expanded (fit (string/expand-tabs s column) (- limit x))\n\
                   cells (string/display-width expanded))\n\
               (paint row (+ x cells) (+ column (string/display-width s column)) limit\n\
                 (rest chunks) (cons [row x expanded] acc)))))\n\
         (sig ops (int int list &optional int int -> list))\n\
         (defn ops (row x chunks &optional (column 0) (limit 100000))\n\
           (paint row x column limit chunks '()))",
        true,
    );
    assert!(
        ws.is_empty(),
        "`w` derives int through the optional caller — {ws:?}"
    );
}

// (2) A `let`-bound lambda's RESULT was inferred under the pre-bound (unknown) name, so
// its self-call made the whole result unknown and every caller of the binder read `any`.
// The self-call contributes ⊥ — the least fixpoint — so `(if … (step next) l)` is `l`'s.
#[test]
fn a_let_bound_lambdas_result_folds_its_self_call_away() {
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (defn- blank? (text line) (= line (string/length text)))\n\
         (defn- run (text line dir)\n\
           (let (n (string/length text)\n\
                 step (fn (l)\n\
                        (let (next (+ l dir))\n\
                          (if (and (>= next 0) (< next n) (blank? text next)) (step next) l))))\n\
             (if (blank? text line) (step line) line)))\n\
         (sig use (string -> int))\n\
         (defn use (text)\n\
           (let (first (run text 3 -1)\n\
                 last (run text 3 1))\n\
             (string/length (string/substring text (inc first) last))))",
        true,
    );
    assert!(
        ws.is_empty(),
        "`run` returns the int its `step` does — {ws:?}"
    );
    // The parameters are derived from the self-call sites too — `k` is `0` from the one
    // external site and `(inc k)` from its own, widened to `int[0..]` — and the result is
    // the base case under the guard: the first `k` past 3. Read from the external site
    // alone, `k` was the literal `0`, `(> k 3)` decided false, and the result was `never`.
    assert_eq!(
        ty_str("(let (f (fn (k) (if (> k 3) k (f (inc k))))) (f 0))"),
        "int[4..]"
    );
}

// (3) A fold accumulator built from a record literal lost its fields on the second step:
// after one step the accumulator is a UNION of two record shapes (the seed beside the
// step), `assoc` over that union answered a flat `map`, and `conj` over its `(tuple) |
// vector<string>` field read the elements of one term only. Both now distribute over the
// union, so the ascent settles with every field, and a callback literal in the collector
// is walked under the accumulator the fold promises it.
#[test]
fn a_fold_accumulator_keeps_its_fields_through_the_ascent() {
    let ws = file_warnings_mode(
        "\
         (defmodule t)\n\
         (defn- clip (s n)\n\
           (if (> (string/length s) n) (string/substring s 0 (math/max 0 n)) s))\n\
         (sig layout ((list string) int int -> map))\n\
         (defn layout (segments x w)\n\
           (fold segments\n\
             {:col x :ops []}\n\
             (fn (a s)\n\
               (let (scol (:col a)\n\
                     label (clip s (math/max 0 (- (+ x w) scol))))\n\
                 (assoc a :col (+ scol (string/length label)) :ops (conj (:ops a) label))))))",
        true,
    );
    assert!(
        ws.is_empty(),
        "`n` derives int from the accumulator's `:col` — {ws:?}"
    );
    // Seeded at 1 and stepped at least once (the list is provably non-empty): `[2, ∞)`.
    // The seed and the step are two record shapes over one key set, merged field-wise by
    // the ascent's widening; kept apart they multiplied by one alternative a round.
    assert_eq!(
        ty_str("(:col (fold '(\"a\" \"b\") {:col 1 :ops []} (fn (a s) (assoc a :col (+ (:col a) 1) :ops (conj (:ops a) s)))))"),
        "int[2..]"
    );
    assert_eq!(
        ty_str("(:ops (fold '(\"a\" \"b\") {:col 1 :ops []} (fn (a s) (assoc a :col (+ (:col a) 1) :ops (conj (:ops a) s)))))"),
        "vector<string>[1..]"
    );
}

#[test]
fn a_fold_callbacks_accumulator_is_handed_init_on_its_first_step() {
    // The fold's RESULT over a provably non-empty input leaves `init` out (the step ran);
    // the callback's accumulator is what every step is handed, and the first is handed
    // `init`. Seeding the callback from the result alone read `b` as `3 | 9 | 4` and
    // flagged the callback's own `nil?` guard as never true — a plain-mode false positive
    // (2026-09-20, found by `fold-for`'s docstring example).
    let quiet = file_warnings(
        "(defn best (xs) (fold [3 9 4] nil (fn (b x) (if (or (nil? b) (> x b)) x b))))",
    );
    assert!(
        quiet.iter().all(|w| !w.contains("never")),
        "`b` is `nil` on the first step — {quiet:?}"
    );
    // …and with a non-nil seed the guard IS dead, so the finding stays a finding.
    let dead = file_warnings(
        "(defn best (xs) (fold [3 9 4] 0 (fn (b x) (if (or (nil? b) (> x b)) x b))))",
    );
    assert!(
        dead.iter()
            .any(|w| w.contains("nil?") && w.contains("never")),
        "a `nil?` on an accumulator seeded with 0 is never true — {dead:?}"
    );
    // The no-init `reduce` seeds from the first element.
    let quiet =
        file_warnings("(defn total (xs) (reduce [1 2 3] (fn (a x) (if (int? a) (+ a x) x))))");
    assert!(
        quiet.iter().all(|w| !w.contains("never")),
        "the seed of a no-init reduce is the first element — {quiet:?}"
    );
}
