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
fn recursive_inference_defers_when_the_base_case_is_unknown() {
    // SOUNDNESS: an accumulator-returning recursion (`acc` is an unconstrained param → the
    // base case is unknown) must infer an unknown return, never a spuriously-narrow one — so
    // a caller using its result in any way is NOT false-flagged.
    let w = check_with_defs(
        &["(defn sum-acc (xs acc) (if (empty? xs) acc (sum-acc (rest xs) (+ acc (first xs)))))"],
        "(string/length (sum-acc (list 1 2) 0))",
    );
    assert!(
        !w.iter().any(|s| s.contains("string/length")),
        "an unknown (param) base case must defer, not false-flag: {w:?}"
    );
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
