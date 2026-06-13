//! End-to-end tests for the v0.1 language: read a string, evaluate it, and
//! check the printed result. These double as executable documentation of what
//! the language can currently do.

use brood::Interp;
use std::sync::LazyLock;

/// Process-wide memory backstop for this test binary (ADR-043). None of these
/// end-to-end tests is a runaway, but `cargo test` runs each `tests/*.rs` file
/// as its own process and only `suite.rs` armed a ceiling — so this binary had
/// none. Forcing this once (idempotent; the limit is a process-wide atomic) means
/// an *accidental* future runaway here trips a clean `E0043` instead of OOMing
/// the host. An explicit `BROOD_MEM_LIMIT` still wins (it's applied first inside
/// `init_limits_with_default`). Forced from `run()` and from each test that builds
/// its own `Interp`; the limit is process-wide, so the first force covers the
/// rest of the parallel run too.
static MEM_GUARD: LazyLock<()> = LazyLock::new(|| {
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
});

/// Build a fresh interpreter with the [`MEM_GUARD`] memory ceiling installed.
fn fresh_interp() -> Interp {
    LazyLock::force(&MEM_GUARD);
    Interp::new()
}

/// Evaluate `src` in a fresh interpreter and return the printed result.
fn run(src: &str) -> String {
    let mut interp = fresh_interp();
    let value = interp.eval_str(src).expect("evaluation failed");
    interp.print(value)
}

/// The step-2 goal: a process heap is `Send` (movable across scheduler threads).
#[test]
fn heap_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<brood::core::heap::Heap>();
}

#[test]
fn arithmetic() {
    assert_eq!(run("(+ 1 2)"), "3");
    assert_eq!(run("(* 2 3 4)"), "24");
    assert_eq!(run("(- 10 3 2)"), "5");
    assert_eq!(run("(- 5)"), "-5");
    assert_eq!(run("(/ 12 3)"), "4");
    assert_eq!(run("(/ 7 2)"), "3.5");
    assert_eq!(run("(mod 7 3)"), "1");
}

#[test]
fn nested_arithmetic() {
    assert_eq!(run("(+ 1 (* 2 3))"), "7");
    assert_eq!(run("(+ (* 2 3) (- 10 4))"), "12");
}

#[test]
fn def_and_reference() {
    assert_eq!(run("(def x 10) (+ x 5)"), "15");
}

#[test]
fn closures_capture_lexically() {
    assert_eq!(
        run("(def adder (fn [a] (fn [b] (+ a b)))) ((adder 3) 4)"),
        "7"
    );
}

#[test]
fn let_is_sequential() {
    assert_eq!(run("(let [a 1 b (+ a 1)] (+ a b))"), "3"); // vector bindings
    assert_eq!(run("(let (a 1 b (+ a 1)) (+ a b))"), "3"); // list bindings (idiomatic)
}

#[test]
fn conditionals() {
    assert_eq!(run("(if (< 1 2) :yes :no)"), ":yes");
    assert_eq!(run("(if (> 1 2) :yes :no)"), ":no");
    assert_eq!(run("(cond (= 1 2) :a (= 1 1) :b else :c)"), ":b");
    assert_eq!(run("(cond (= 1 2) :a else :fallback)"), ":fallback");
    assert_eq!(run("(when true 1 2 3)"), "3");
    assert_eq!(run("(unless false :ok)"), ":ok");
}

#[test]
fn logic_short_circuits() {
    assert_eq!(run("(and 1 2 3)"), "3");
    assert_eq!(run("(and 1 false 3)"), "false");
    assert_eq!(run("(or false nil :found)"), ":found");
}

#[test]
fn lists_and_sequences() {
    assert_eq!(run("(list 1 2 3)"), "(1 2 3)");
    assert_eq!(run("(cons 0 (list 1 2))"), "(0 1 2)");
    assert_eq!(run("(first (list 1 2 3))"), "1");
    assert_eq!(run("(rest (list 1 2 3))"), "(2 3)");
    assert_eq!(run("(count (list 1 2 3 4))"), "4");
    assert_eq!(run("(reverse (list 1 2 3))"), "(3 2 1)");
    assert_eq!(run("(append (list 1 2) (list 3 4))"), "(1 2 3 4)");
}

#[test]
fn vectors_evaluate_elements() {
    assert_eq!(run("[1 (+ 1 1) 3]"), "[1 2 3]");
}

#[test]
fn maps_are_immutable_values() {
    // ADR-040 (CHAMP): print order is hash-driven, not insertion order.
    // The only printed form we assert exactly is a single-entry map; the
    // round-trip in the `=` line is the real shape-correctness check.
    assert_eq!(run("(get {:a 1 :b 2} :b)"), "2");
    assert_eq!(run("(get {:a 1} :z 99)"), "99");
    assert_eq!(run("(get (assoc {:a 1} :b 2) :b)"), "2");
    // assoc does not mutate the original binding.
    assert_eq!(run("(def m {:a 1}) (assoc m :b 2) m"), "{:a 1}");
    // dissoc result equals the expected map regardless of print order.
    assert_eq!(run("(= (dissoc {:a 1 :b 2 :c 3} :b) {:a 1 :c 3})"), "true");
    assert_eq!(run("(count {:a 1 :b 2 :c 3})"), "3");
    // equality is order-independent; any value is a structurally-compared key.
    assert_eq!(run("(= {:a 1 :b 2} {:b 2 :a 1})"), "true");
    assert_eq!(run("(get {[1 2] :v} [1 2])"), ":v");
    assert_eq!(run("(type-of {})"), ":map");
}

#[test]
fn maps_structural_keys_and_equality() {
    // Any value is a key, compared structurally (strings, vectors, maps, ints).
    assert_eq!(run("(get {\"x\" 1 \"y\" 2} \"y\")"), "2");
    assert_eq!(run("(get {{:a 1} :found} {:a 1})"), ":found");
    assert_eq!(run("(contains? {{:a 1} :found} {:a 2})"), "false");
    // int and float keys are distinct (consistent with `=`).
    assert_eq!(run("(get {1 :int} 1.0)"), "nil");
    // A stored falsy value is not absence: get returns it, contains? is true.
    assert_eq!(run("(get {:a false} :a 99)"), "false");
    assert_eq!(run("(contains? {:a nil} :a)"), "true");
    // Equality: order-independent, value-sensitive, depth-sensitive.
    assert_eq!(run("(= {} {})"), "true");
    assert_eq!(run("(= {:a 1} {:a 1 :b 2})"), "false");
    assert_eq!(run("(= {:a {:b [1 2]}} {:a {:b [1 2]}})"), "true");
    assert_eq!(run("(= {:a {:b [1 2]}} {:a {:b [1 3]}})"), "false");
    // ADR-040 (CHAMP): iteration order is hash-driven, not insertion.
    // The key *set* survives assoc; assert via frequencies-as-map equality.
    assert_eq!(
        run("(= (frequencies (keys (assoc {:a 1 :b 2} :a 9))) {:a 1 :b 1})"),
        "true"
    );
    assert_eq!(
        run("(= (frequencies (keys (assoc {:a 1} :b 2))) {:a 1 :b 1})"),
        "true"
    );
}

#[test]
fn maps_round_trip_through_reader() {
    // pr-str's readable form reads + evals back to an equal map.
    let src = "(def m {:a 1 :b [2 3] :c \"x\" :d {:nested true}}) \
               (= m (eval (read-string (pr-str m))))";
    assert_eq!(run(src), "true");
}

/// Maps `def`'d at top level are promoted into the shared RUNTIME region and
/// survive the per-form LOCAL arena reset (ADR-016) — many sequential top-level
/// forms must not corrupt earlier maps.
#[test]
fn maps_survive_arena_reset() {
    let src = "
        (def a {:x 1})
        (def b (assoc a :y 2))
        (def c (assoc b :z 3))
        (def d (dissoc c :x))
        (list (get a :x) (get b :y) (get c :z) (contains? d :x) (count c))
    ";
    assert_eq!(run(src), "(1 2 3 false 3)");
}

#[test]
fn higher_order() {
    assert_eq!(run("(map inc (list 1 2 3))"), "(2 3 4)");
    assert_eq!(run("(filter positive? (list -1 2 -3 4))"), "(2 4)");
    assert_eq!(run("(reduce + 0 (list 1 2 3 4))"), "10");
    assert_eq!(run("(apply + (list 1 2 3))"), "6");
}

#[test]
fn prelude_helpers() {
    assert_eq!(run("(inc 41)"), "42");
    assert_eq!(run("(sum (list 1 2 3 4))"), "10");
    assert_eq!(run("(max 3 7)"), "7");
    assert_eq!(run("(abs -9)"), "9");
}

#[test]
fn strings() {
    assert_eq!(run("(str \"a\" \"b\" \"c\")"), "\"abc\"");
    assert_eq!(run("(str \"n=\" 42)"), "\"n=42\"");
    assert_eq!(run("(count \"hello\")"), "5");
}

/// The string-library kernel primitives (the Brood layer over them is covered by
/// the in-language suite, `tests/strings_test.blsp`).
#[test]
fn string_kernel() {
    assert_eq!(run("(upper \"abc\")"), "\"ABC\"");
    assert_eq!(run("(lower \"ABC\")"), "\"abc\"");
    assert_eq!(run("(upper \"ß\")"), "\"SS\""); // Unicode case folding
    assert_eq!(run("(string->number \"42\")"), "42");
    assert_eq!(run("(string->number \"3.5\")"), "3.5");
    assert_eq!(run("(string->number \"3abc\")"), "nil"); // strict parse, not read-string
    assert_eq!(run("(string->number \"\")"), "nil");
}

/// The headline property: deep tail recursion uses O(1) Rust stack, so it must
/// not overflow. 100,000 frames would blow the stack without tail calls; we keep
/// the count here (rather than millions) because arithmetic is now defined in
/// Brood itself and is correspondingly slower than a native loop.
#[test]
fn tail_calls_do_not_overflow() {
    let src = "
        (def sum-to
          (fn [n acc]
            (if (= n 0) acc (sum-to (- n 1) (+ acc n)))))
        (sum-to 100000 0)
    ";
    assert_eq!(run(src), "5000050000");
}

/// The foundation for editing the editor on the fly: redefining a function in
/// the live global environment changes behaviour immediately.
#[test]
fn live_redefinition() {
    let mut interp = Interp::new();
    interp.eval_str("(def greet (fn () :v1))").unwrap();
    let v = interp.eval_str("(greet)").unwrap();
    assert_eq!(interp.print(v), ":v1");
    interp.eval_str("(def greet (fn () :v2))").unwrap();
    let v = interp.eval_str("(greet)").unwrap();
    assert_eq!(interp.print(v), ":v2");
}

/// `defonce` initialises a binding once; re-evaluating the same form — which is
/// exactly what a hot reload (`reload-defs`/`load`) does on every save — is a
/// no-op, so top-level state and singletons survive the reload instead of being
/// reset / re-created. (docs/live-editing.md Stage 1.)
#[test]
fn defonce_preserves_state_across_reload() {
    let mut interp = fresh_interp();
    // First definition binds.
    let v = interp.eval_str("(defonce s 41)").unwrap();
    assert_eq!(interp.print(v), "s");
    let v = interp.eval_str("s").unwrap();
    assert_eq!(interp.print(v), "41");
    // The running program changes the value.
    interp.eval_str("(def s 99)").unwrap();
    // A reload re-evaluates the original `defonce` form: it must NOT reset to 41.
    interp.eval_str("(defonce s 41)").unwrap();
    let v = interp.eval_str("s").unwrap();
    assert_eq!(interp.print(v), "99");
    // `bound?` is false for an unbound symbol, so the first `defonce` is safe.
    let v = interp.eval_str("(bound? 'never-bound)").unwrap();
    assert_eq!(interp.print(v), "false");
}

/// `reload-defs` re-evaluates definitions but skips side-effecting top-level
/// calls — even a call whose name starts with "def" (it resolves to a `Fn`, not
/// a definer macro). (docs/live-editing.md Stage 2.)
#[test]
fn reload_defs_applies_definitions_and_skips_calls() {
    let mut interp = fresh_interp();
    // A sentinel we watch for unwanted side effects from skipped calls.
    interp.eval_str("(def side :untouched)").unwrap();

    let mut path = std::env::temp_dir();
    path.push(format!("brood-reload-{}.blsp", std::process::id()));
    let src = "(def rx 1)\n\
               (defn rf () 2)\n\
               (defn default-thing () (def side :ran-default))\n\
               (defn run-it () (def side :ran-call))\n\
               (default-thing)\n\
               (run-it)\n";
    std::fs::write(&path, src).unwrap();
    let p = path.to_string_lossy().replace('\\', "\\\\");
    interp
        .eval_str(&format!("(reload-defs \"{}\")", p))
        .unwrap();

    // Definitions applied:
    let v = interp.eval_str("rx").unwrap();
    assert_eq!(interp.print(v), "1");
    let v = interp.eval_str("(rf)").unwrap();
    assert_eq!(interp.print(v), "2");
    // Both top-level *calls* were skipped — including `(default-thing)`, whose
    // name starts with "def" but is a function call, not a definition.
    let v = interp.eval_str("side").unwrap();
    assert_eq!(interp.print(v), ":untouched");

    std::fs::remove_file(&path).ok();
}

/// Redefining a global to *unchanged* code (a save-without-change, or `nest
/// format` rewriting the file) is deduped — it doesn't append a duplicate to the
/// append-only RUNTIME region. A real change still appends. (Stage 5.)
#[test]
fn unchanged_redefinition_is_deduped() {
    let mut interp = fresh_interp();
    interp.eval_str("(def f (fn (x) (+ x 1)))").unwrap();
    let after_first = interp.heap.runtime_closure_count();

    // Re-defining the *same* code must not grow the RUNTIME region…
    interp.eval_str("(def f (fn (x) (+ x 1)))").unwrap();
    assert_eq!(
        interp.heap.runtime_closure_count(),
        after_first,
        "identical redefinition should be deduped"
    );

    // …while a genuine change appends a new version (and is live immediately).
    interp.eval_str("(def f (fn (x) (+ x 2)))").unwrap();
    assert!(
        interp.heap.runtime_closure_count() > after_first,
        "changed redefinition should append a new version"
    );
    let v = interp.eval_str("(f 10)").unwrap();
    assert_eq!(interp.print(v), "12");
}

/// `eval` + `read-string` let the language run code it builds at runtime.
#[test]
fn eval_and_read_string() {
    assert_eq!(run("(eval (read-string \"(+ 40 2)\"))"), "42");
}

#[test]
fn slurp_round_trips_a_file() {
    // `slurp` is the read counterpart of `spit`: write a file, read it back, get
    // the same bytes. Used by the doc tooling to inspect a module's source.
    let mut path = std::env::temp_dir();
    path.push(format!("brood-slurp-{}.tmp", std::process::id()));
    let path = path.to_string_lossy().replace('\\', "\\\\");
    let src = format!(
        "(spit \"{p}\" \"hello\\n\") (= (slurp \"{p}\") \"hello\\n\")",
        p = path
    );
    assert_eq!(run(&src), "true");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn source_location_records_def_sites_from_a_loaded_file() {
    // ADR-031: loading a file records where each top-level def/defn/defmacro was
    // defined, so `(source-location 'name)` can answer cross-file goto-definition.
    let mut path = std::env::temp_dir();
    path.push(format!("brood-srcloc-{}.blsp", std::process::id()));
    std::fs::write(&path, "(defn foo (x) (* x 2))\n(def bar 10)\n").unwrap();
    let p = path.to_string_lossy().replace('\\', "\\\\");

    let mut interp = Interp::new();
    interp.eval_str(&format!("(load \"{p}\")")).expect("load");
    let loc = |interp: &mut Interp, s: &str| {
        let v = interp_eval(interp, s);
        interp.print(v)
    };

    // `defn` (a macro lowering to `def`) is still located: the site is captured
    // pre-expansion. Both names point at their form's line, column 1.
    assert_eq!(
        loc(&mut interp, "(source-location 'foo)"),
        format!("[\"{p}\" 1 1]")
    );
    assert_eq!(
        loc(&mut interp, "(source-location 'bar)"),
        format!("[\"{p}\" 2 1]")
    );
    // A prelude `defn` (`map`) does have a recorded site — it points at the
    // materialised prelude cache copy on disk (ADR-031 step 4; see
    // `introspect::source_location_resolves_prelude_fns_but_not_builtins_or_unbound`).
    // A Rust primitive (`cons`) has no Brood source, and an unknown name has
    // no global at all, so both still yield `nil`.
    let map_loc = loc(&mut interp, "(source-location 'map)");
    assert!(
        map_loc.contains("prelude.blsp"),
        "expected prelude `map` to resolve to its cache copy, got {map_loc}"
    );
    assert_eq!(loc(&mut interp, "(source-location 'cons)"), "nil");
    assert_eq!(loc(&mut interp, "(source-location 'no-such-xyz)"), "nil");

    let _ = std::fs::remove_file(&path);
}

/// Eval `src` against an existing interpreter (so state from a prior `load`
/// persists), returning the value.
fn interp_eval(interp: &mut Interp, src: &str) -> brood::core::value::Value {
    interp.eval_str(src).expect("evaluation failed")
}

#[test]
fn slurp_of_a_missing_file_errors() {
    Interp::new()
        .eval_str("(slurp \"/no/such/brood/file.blsp\")")
        .expect_err("slurp of a missing path should error");
}

#[test]
fn type_of_reports_the_runtime_tag() {
    assert_eq!(run("(type-of 1)"), ":int");
    assert_eq!(run("(type-of 1.5)"), ":float");
    assert_eq!(run("(type-of \"s\")"), ":string");
    assert_eq!(run("(type-of 'x)"), ":symbol");
    assert_eq!(run("(type-of :k)"), ":keyword");
    assert_eq!(run("(type-of nil)"), ":nil");
    assert_eq!(run("(type-of true)"), ":bool");
    assert_eq!(run("(type-of (list 1))"), ":pair");
    assert_eq!(run("(type-of [1])"), ":vector");
    assert_eq!(run("(type-of inc)"), ":fn"); // a Brood closure
    assert_eq!(run("(type-of %add)"), ":native"); // a Rust builtin
}

#[test]
fn type_errors_are_self_identifying() {
    // The op, the wanted type, and the offending value's tag + form all appear.
    let err = |src: &str| {
        Interp::new()
            .eval_str(src)
            .expect_err("expected a type error")
            .to_string()
    };
    assert_eq!(
        err("(+ 1 \"x\")"),
        "type error: %add: expected number, got string (\"x\")"
    );
    assert_eq!(
        err("(first 5)"),
        "type error: first: expected list or vector, got int (5)"
    );
    assert_eq!(
        err("(string-length :k)"),
        "type error: string-length: expected string, got keyword (:k)"
    );
}

#[test]
fn native_arity_is_enforced_centrally() {
    let err = |src: &str| {
        Interp::new()
            .eval_str(src)
            .expect_err("expected an arity error")
            .to_string()
    };
    // Too few, too many, and a variadic minimum — all caught before the builtin runs.
    assert_eq!(
        err("(type-of)"),
        "arity error: type-of: expected 1 argument, got 0"
    );
    assert_eq!(
        err("(cons 1)"),
        "arity error: cons: expected 2 arguments, got 1"
    );
    assert_eq!(
        err("(now 1 2)"),
        "arity error: now: expected 0 arguments, got 2"
    );
    assert_eq!(
        err("(apply +)"),
        "arity error: apply: expected at least 2 arguments, got 1"
    );
    // The same gate applies when a builtin is reached through `apply`.
    assert_eq!(
        err("(apply cons (list 1))"),
        "arity error: cons: expected 2 arguments, got 1"
    );
}

#[test]
fn check_builtin_flags_provable_misuse() {
    // The advisory checker, end to end through the language. Provable primitive
    // misuse yields a warning; correct or not-statically-known code yields none.
    assert!(run("(check '(first 5))").contains("first: argument 1 expects"));
    assert!(run("(check '(string-length :k))").contains("string-length"));
    assert_eq!(run("(check '(first (list 1 2)))"), "nil"); // arg type unknown → no warning
    assert_eq!(run("(check '(+ 1 2))"), "nil"); // closure, not a primitive
                                                // It is advisory — it never raises, even on the misuse it reports.
    assert_eq!(run("(do (check '(first 5)) :ok)"), ":ok");
}

#[test]
fn defn_defines_functions() {
    assert_eq!(run("(defn sq [x] (* x x)) (sq 6)"), "36");
    assert_eq!(run("(defn add3 [a b c] (+ a b c)) (add3 1 2 3)"), "6");
    // defn is itself written in Brood; it expands to (def name (fn ...)).
    assert_eq!(
        run("(macroexpand-1 '(defn f [x] (+ x 1)))"),
        "(def f (fn [x] (+ x 1)))"
    );
}

#[test]
fn params_may_be_a_list_or_vector() {
    assert_eq!(run("(defn sq (x) (* x x)) (sq 7)"), "49"); // list params
    assert_eq!(run("(defn sq2 [x] (* x x)) (sq2 8)"), "64"); // vector params
    assert_eq!(run("((fn (a b) (+ a b)) 2 3)"), "5");
    assert_eq!(
        run("(defn rest-args (& xs) xs) (rest-args 1 2 3)"),
        "(1 2 3)"
    );
}

#[test]
fn optional_params() {
    let g = "(defn greet (name &optional (greeting \"hi\")) (str greeting \", \" name))";
    assert_eq!(run(&format!("{} (greet \"Ada\")", g)), "\"hi, Ada\"");
    assert_eq!(run(&format!("{} (greet \"Ada\" \"yo\")", g)), "\"yo, Ada\"");
    // a default may reference an earlier parameter (left-to-right binding)
    assert_eq!(
        run("(defn rect (w &optional (h w)) (* w h)) (rect 5)"),
        "25"
    );
    assert_eq!(
        run("(defn rect (w &optional (h w)) (* w h)) (rect 5 3)"),
        "15"
    );
    // a bare optional defaults to nil
    assert_eq!(run("(defn f (a &optional b) (list a b)) (f 1)"), "(1 nil)");
    // optionals work on a raw fn, not just defn
    assert_eq!(run("((fn (a &optional (b 10)) (+ a b)) 5)"), "15");
    // optionals compose with & rest
    assert_eq!(
        run("(defn f (a &optional (b 2) & more) (list a b more)) (f 1)"),
        "(1 2 nil)"
    );
    assert_eq!(
        run("(defn f (a &optional (b 2) & more) (list a b more)) (f 1 9 8 7)"),
        "(1 9 (8 7))"
    );
}

#[test]
fn optional_params_arity() {
    let mut interp = Interp::new();
    // too many args, no rest to absorb them
    assert!(interp
        .eval_str("(defn f (a &optional b) a) (f 1 2 3)")
        .is_err());
    // too few required
    assert!(interp
        .eval_str("(defn f (a b &optional c) a) (f 1)")
        .is_err());
    // an unknown marker is rejected, not silently treated as a param name
    assert!(interp.eval_str("(defn f (a &key b) a)").is_err());
}

#[test]
fn user_macros_and_quasiquote() {
    let when_macro = "(defmacro my-when [c & body] `(if ~c (do ~@body) nil))";
    assert_eq!(run(&format!("{} (my-when true 1 2 3)", when_macro)), "3");
    assert_eq!(run(&format!("{} (my-when false 1 2 3)", when_macro)), "nil");
    // quasiquote with unquote and unquote-splicing
    assert_eq!(run("`(1 ~(+ 1 1) ~@(list 3 4) 5)"), "(1 2 3 4 5)");
    assert_eq!(run("(def x 10) `(a ~x b)"), "(a 10 b)");
}

/// The kernel expander's fixpoint loop is bounded (kernel audit): a macro
/// whose expansion *grows* every round never reaches a fixpoint and must
/// produce a clean error — not hard-hang the expander. (A macro expanding to a
/// structurally identical call IS a fixpoint and terminates normally; the
/// growing shape is the runaway.) This drives the Rust `macros::macroexpand`
/// via the compile pass on a direct invocation; the prelude's Brood-level
/// `macroexpand` fn has its own cap, tested in `tests/macroexpand_test.blsp`.
#[test]
fn runaway_macro_expansion_errors_instead_of_hanging() {
    let mut interp = Interp::new();
    interp
        .eval_str("(defmacro grow-loop (x) `(grow-loop (~x)))")
        .expect("defining the macro is fine");
    let err = interp
        .eval_str("(grow-loop 1)")
        .expect_err("invoking it must error, not spin");
    assert!(
        err.to_string().contains("fixpoint"),
        "expected the fixpoint-cap error, got: {err}"
    );
}

/// `(quote a b)` is malformed — exactly one argument is quoted. The tree-walker
/// rejects it with an arity error; the closure-compiling VM (the default engine)
/// must agree rather than silently dropping the extra(s) and compiling `(quote a)`
/// to a constant. Cover both the direct top-level form (tree-walker entry) and a
/// form inside a compiled closure body (so the VM's quote arm is exercised — it
/// now defers the whole closure to the tree-walker on a bad arity).
#[test]
fn quote_arity_is_enforced_on_both_engines() {
    let err = |src: &str| {
        fresh_interp()
            .eval_str(src)
            .expect_err("(quote a b) must error, not drop the tail")
            .to_string()
    };
    // Direct: handled by the tree-walker entry.
    assert!(
        err("(quote 1 2)").contains("quote"),
        "expected a quote arity error, got: {}",
        err("(quote 1 2)")
    );
    // Inside a compiled closure body: forces the VM's quote arm to defer.
    assert!(
        err("(defn q () (quote 1 2)) (q)").contains("quote"),
        "VM-compiled (quote 1 2) must error too, got: {}",
        err("(defn q () (quote 1 2)) (q)")
    );
    // Sanity: the well-formed single-argument form still works.
    assert_eq!(run("(quote 1)"), "1");
    assert_eq!(run("(defn q () (quote hi)) (q)"), "hi");
}

/// A top-level `~@` (unquote-splicing with nothing to splice *into* — outside any
/// list/vector template position) is malformed. It must raise a clear error, not
/// silently mis-build `(list 'unquote-splicing xs)`.
#[test]
fn top_level_unquote_splicing_errors() {
    let err = fresh_interp()
        .eval_str("`~@(list 1 2 3)")
        .expect_err("top-level ~@ must error")
        .to_string();
    assert!(
        err.contains("unquote-splicing"),
        "expected an unquote-splicing context error, got: {err}"
    );
    // The well-formed splice (inside a list template) still works.
    assert_eq!(run("`(~@(list 1 2 3))"), "(1 2 3)");
}

#[test]
fn threading_macros() {
    // (-> 5 (- 1) (* 2)) => (* (- 5 1) 2) => 8
    assert_eq!(run("(-> 5 (- 1) (* 2))"), "8");
    // (->> (list 1 2 3) (map inc)) => (map inc (list 1 2 3)) => (2 3 4)
    assert_eq!(run("(->> (list 1 2 3) (map inc))"), "(2 3 4)");
}

#[test]
fn match_dispatches_on_patterns() {
    // literal / case dispatch
    assert_eq!(run("(match 2 (1 :one) (2 :two) (_ :other))"), ":two");
    // tagged vectors are the tuple idiom; the same literal builds and matches
    assert_eq!(run("(match [:ok 42] ([:ok v] v) ([:err e] e))"), "42");
    // list destructure with a & rest tail
    assert_eq!(
        run("(match (list 1 2 3) ((h & t) (list h t)))"),
        "(1 (2 3))"
    );
    // nested patterns compose
    assert_eq!(run("(match [:add [1 2]] ([:add [a b]] (+ a b)))"), "3");
    // guards
    assert_eq!(run("(match 7 (n :when (> n 0) :pos) (_ :nonpos))"), ":pos");
    // non-linear: a repeated variable is an equality constraint
    assert_eq!(run("(match [3 3] ([x x] :eq) (_ :ne))"), ":eq");
    assert_eq!(run("(match [3 4] ([x x] :eq) (_ :ne))"), ":ne");
    // match a known value: keyword tag, quoted symbol, and a pin
    assert_eq!(run("(match 'foo ('foo :y) (_ :n))"), ":y");
    assert_eq!(run("(let (k :ok) (match [:ok 9] ([~k v] v) (_ :n)))"), "9");
}

#[test]
fn match_no_clause_raises_structured_value() {
    // A no-match crashes with a structured, catchable value:
    // [:match-error <context> <value> <patterns-tried>].
    assert_eq!(
        run("(try (match 42 ([:ok v] v))
               (catch e (match e ([:match-error ctx val pats] (list ctx val)) (_ :other))))"),
        "(:match 42)"
    );
    let mut interp = Interp::new();
    assert!(interp.eval_str("(match 42 ([:ok v] v))").is_err());
    // compile-time checks fire while the macro expands
    assert!(interp.eval_str("(match 1 (x :always) (2 :dead))").is_err()); // unreachable
    assert!(interp.eval_str("(match (list 1) ((a & b c) :x))").is_err()); // malformed &
}

/// The compile pass expands `match` once at definition, so a `match` in tail
/// position is both TCO-safe (no overflow) and runs at plain-`if` speed (it is
/// not re-expanded per call). The rigorous 100,000-frame guard is the if-based
/// `tail_calls_do_not_overflow`; here we just confirm a match loop doesn't grow
/// the stack.
#[test]
fn match_in_tail_position_does_not_overflow() {
    let src = "
        (defn count-down (n) (match n (0 :done) (_ (count-down (- n 1)))))
        (count-down 50000)
    ";
    assert_eq!(run(src), ":done");
}

#[test]
fn destructuring_let() {
    // a binding target may be a pattern — a refutable bind (Brood's `=`)
    assert_eq!(run("(let ([:ok v] [:ok 42]) v)"), "42");
    assert_eq!(run("(let ((a b c) (list 1 2 3)) (+ a b c))"), "6");
    // sequential, freely mixed with plain-symbol binds
    assert_eq!(
        run("(let (x 1 [a b] [10 20] y (+ x a)) (list x a b y))"),
        "(1 10 20 11)"
    );
    // a non-match raises (handle it with `match` instead if you don't want a crash)
    let mut interp = Interp::new();
    assert!(interp.eval_str("(let ([:ok v] [:err 1]) v)").is_err());
}

#[test]
fn fn_clauses_and_pattern_params() {
    // multi-clause dispatch — the canonical Erlang shape
    assert_eq!(
        run("(defn fac ((0) 1) ((n) (* n (fac (- n 1))))) (fac 5)"),
        "120"
    );
    // single-clause fn with a tuple-destructured parameter
    assert_eq!(run("((fn ([x y]) (* x y)) [3 4])"), "12");
    assert_eq!(run("((fn (a [x y]) (+ a x y)) 1 [2 3])"), "6");
    // pattern params coexist with &optional
    assert_eq!(
        run("(defn box (a [x y] &optional (c 10)) (+ a x y c)) (box 1 [2 3])"),
        "16"
    );
    // no clause matched is a runtime error
    let mut interp = Interp::new();
    assert!(interp
        .eval_str("(defn only-ok (([:ok v]) v)) (only-ok [:err 1])")
        .is_err());
}

#[test]
fn throw_and_catch() {
    // a thrown value is rebound by catch
    assert_eq!(run("(try (throw 42) (catch e e))"), "42");
    assert_eq!(
        run("(try (throw :boom) (catch e (str \"caught \" e)))"),
        "\"caught :boom\""
    );
    // no throw: the body's value is returned
    assert_eq!(run("(try (+ 1 2) (catch e e))"), "3");
    // A built-in error is caught as a **structured map** (the llm-native
    // §4 contract — agents and Brood code branch on `:kind` / `:code`
    // instead of grepping a string). User throws (`(throw v)`) still
    // come back verbatim — only kernel-raised errors get the wrapper.
    assert_eq!(run("(try (/ 1 0) (catch e (map? e)))"), "true");
    assert_eq!(run("(try (/ 1 0) (catch e (get e :kind)))"), ":runtime");
    assert_eq!(run("(try (/ 1 0) (catch e (get e :code)))"), "\"E0040\"");
    // Hint rides along when the raise site set one — `with_hint("…")`.
    assert_eq!(
        run("(try (/ 1 0) (catch e (string? (get e :hint))))"),
        "true"
    );
    // Integer overflow no longer raises (E0041 is gone): arithmetic past i64
    // auto-promotes to a bignum (ADR bignums). `(* i64::MAX 2)` is the exact
    // big value, returned without throwing.
    assert_eq!(
        run("(try (* 9223372036854775807 2) (catch e e))"),
        "18446744073709551614"
    );
    // E0042 index out of range — vector-ref off the end.
    assert_eq!(
        run("(try (vector-ref [1 2 3] 7) (catch e (get e :code)))"),
        "\"E0042\""
    );
    // Same code for `substring` (different surface, same family).
    assert_eq!(
        run("(try (substring \"hi\" 0 99) (catch e (get e :code)))"),
        "\"E0042\""
    );
    // E0050 file IO — slurp of a path that doesn't exist.
    assert_eq!(
        run("(try (slurp \"/does/not/exist/anywhere\") (catch e (get e :code)))"),
        "\"E0050\""
    );
    assert_eq!(
        run("(try (no-such-fn) (catch e (get e :kind)))"),
        ":unbound"
    );
    assert_eq!(
        run("(try (no-such-fn) (catch e (get e :code)))"),
        "\"E0010\""
    );
    // Type errors carry E0030; the message preserves the structured detail
    // (`wrong_type` includes "expected <kind>, got <kind> (<value>)").
    assert_eq!(run("(try (first 5) (catch e (get e :code)))"), "\"E0030\"");
    assert_eq!(run("(try (first 5) (catch e (get e :kind)))"), ":type");
    // Arity errors carry E0020. `((fn (x) x))` calls the unary fn with zero
    // args — the kernel's arity check fires.
    assert_eq!(
        run("(try ((fn (x) x)) (catch e (get e :code)))"),
        "\"E0020\""
    );
    // The :message field is always a string, even for kernel errors.
    assert_eq!(
        run("(try (/ 1 0) (catch e (string? (get e :message))))"),
        "true"
    );
    // error: raise a formatted message — user-throw, no :code
    assert_eq!(run("(try (error \"nope: \" 5) (catch e e))"), "\"nope: 5\"");
    // try with no catch clause is just a do
    assert_eq!(run("(try 1 2 3)"), "3");
}

/// The scheduler-race hint fires for unbound errors raised inside a *green*
/// process (the under-load failure mode `docs/claude-demo-findings.md`
/// flagged). The root thread doesn't get the hint — that's the contract
/// `eval::unbound_error` enforces via `process::in_green_process()`.
#[test]
fn scheduler_race_hint_attaches_to_unbound_in_green_processes() {
    // `spawn` takes an *expression* (ADR-033), so the body is the form to
    // evaluate in the new process — not a `(fn () …)`-wrapped thunk.
    let src = r#"
        (let (me (self))
          (spawn (send me (try (no-such-fn) (catch e e))))
          (let (msg (receive))
            (string? (get msg :hint))))
    "#;
    assert_eq!(run(src), "true");
}

/// A *qualified* unbound miss (`mod/name`) inside a green process does NOT get
/// the scheduler-race hint: a qualified miss is never the prelude-lookup race
/// (which only loses *bare* internal names like `fold`/`acc`/`%eq`). This is the
/// cross-node sent-closure case — a `send`-ed closure's free global that exists
/// only on the sending node arrives `unbound symbol: <ns>/<name>` on the
/// receiver, and the race story would misdirect. `eval::unbound_error`.
#[test]
fn qualified_unbound_in_green_process_has_no_scheduler_hint() {
    let src = r#"
        (let (me (self))
          (spawn (send me (try (some-mod/no-such-fn) (catch e (get e :hint)))))
          (receive))
    "#;
    // No scheduler-race hint, and no namespace hint (no `some-mod/no-such-fn`
    // is defined), so `:hint` is nil.
    assert_eq!(run(src), "nil");
}

#[test]
fn unbound_in_root_thread_has_no_scheduler_hint() {
    // Negative case: the root thread (REPL / file runner / nest mcp
    // dispatcher) is *not* a green process, so the hint stays nil.
    // (`no-such-fn` has no `mod/no-such-fn`, so the namespace hint below
    // doesn't fire either.)
    assert_eq!(run("(try (no-such-fn) (catch e (get e :hint)))"), "nil");
}

/// A construct from another Lisp that Brood doesn't have gets a "the Brood way"
/// hint — at runtime (the caught error's `:hint`) and at write-time (the advisory
/// checker), both via the shared `eval::foreign_construct_hint`.
#[test]
fn foreign_constructs_hint_at_the_brood_way() {
    // Runtime: hint rides the caught error.
    assert!(run("(try (set! x 1) (catch e (get e :hint)))").contains("immutable"));
    assert!(run("(try (loop 1) (catch e (get e :hint)))").contains("tail-recursive"));
    // Write-time: `check` appends the same guidance to the unbound warning.
    assert!(run("(check '(swap! a 1))").contains("atoms"));
    // A name Brood *does* provide (it aliases `car` → `first`) gets no foreign
    // hint — it simply runs.
    assert_eq!(run("(try (car (list 1)) (catch e e))"), "1");
}

/// A bare name that exists only as `mod/name` — because `(require 'mod)` loaded
/// the module but didn't refer it — gets a `(:use mod)` fix-it hint. This is the
/// most common post-ADR-065 mistake for code (and LLMs) written against the old
/// flat-namespace model. `eval::unbound_namespace_hint`.
#[test]
fn unbound_bare_name_suggests_use_of_its_namespace() {
    let mut interp = Interp::new();
    interp.eval_str("(require 'set)").unwrap(); // defines set/union, set/set, …
    let r = interp
        .eval_str("(try (union {} {}) (catch e (get e :hint)))")
        .unwrap();
    let printed = interp.print(r);
    assert!(printed.contains("(:use set)"), "{printed}");
    assert!(printed.contains("set/union"), "{printed}");
}

/// The same fix-it for a *hierarchical* module (ADR-085): a bare name whose only
/// global is `gui/window/draw` suggests `(:use gui/window)` — the multi-segment
/// module name must survive the hint's filter (it used to drop anything with a
/// `/`). `eval::unbound_namespace_hint`.
#[test]
fn unbound_bare_name_suggests_use_of_a_hierarchical_namespace() {
    let mut interp = Interp::new();
    interp
        .eval_str("(%load-string \"(defmodule gui/window) (defn draw (x) x)\")")
        .unwrap();
    let r = interp
        .eval_str("(try (draw 1) (catch e (get e :hint)))")
        .unwrap();
    let printed = interp.print(r);
    assert!(printed.contains("(:use gui/window)"), "{printed}");
    assert!(printed.contains("gui/window/draw"), "{printed}");
}

/// Parse errors caught by `try`/`catch` carry the `:line` and `:col` the
/// reader recorded — agents can highlight the bad span without parsing the
/// message string. (The kernel raises before the source is bound, so `:file`
/// is absent — that field comes from `load` / the file runner.)
#[test]
fn parse_errors_carry_position_in_catch_map() {
    let mut interp = Interp::new();
    let r = interp
        .eval_str("(try (eval-string \"(unclosed\") (catch e [(get e :kind) (get e :line)]))")
        .unwrap();
    let printed = interp.print(r);
    assert!(printed.contains(":parse"), "{printed}");
    // Some positive line number — the reader caught the unclosed delimiter
    // at a known position. Exact value depends on the reader's recovery, so
    // pin only "non-nil non-zero integer present".
    assert!(
        printed.contains("1") || printed.contains("2"),
        "expected a line number in {printed}"
    );
}

#[test]
fn uncaught_throw_propagates() {
    let mut interp = Interp::new();
    assert!(interp.eval_str("(throw 1)").is_err());
    assert!(interp.eval_str("(error \"boom\")").is_err());
}

#[test]
fn errors_are_reported() {
    let mut interp = Interp::new();
    assert!(interp.eval_str("(+ 1 nope)").is_err());
    assert!(interp.eval_str("(this-is-not-defined)").is_err());
    assert!(interp.eval_str("(/ 1 0)").is_err());
}

/// The cross-process extension of `live_redefinition`, and the whole point of
/// the shared-runtime model: a long-running *spawned* process shares the
/// runtime's live code, so redefining a function it calls changes its behaviour
/// on the next request — without restarting it (the web-server / Erlang
/// hot-code-swap scenario; see docs/shared-code.md).
#[test]
fn spawned_process_picks_up_redefinition() {
    let src = r#"
        (def handler (fn (x) (* x 10)))
        ;; a tiny request/reply loop, like a long-running server
        (def server
          (fn ()
            (let (msg (receive))
              (send (first msg) (handler (first (rest msg))))
              (server))))
        (def srv  (spawn (server)))
        (def call (fn (x) (send srv (list (self) x)) (receive)))
        (def before (call 5))             ; 5 * 10 = 50
        (def handler (fn (x) (+ x 100)))  ; hot-reload the handler in place
        (def after (call 5))              ; 5 + 100 = 105 — on the SAME running server
        (list before after)
    "#;
    assert_eq!(run(src), "(50 105)");
}

/// `%isolate` runs a thunk against a private copy of the global bindings: a
/// `def` it makes takes effect *inside* the thunk but is rolled back when
/// it returns. This is what gives `:isolated` tests true state isolation — a
/// test's definitions can't leak to any other test (see std/test.blsp).
#[test]
fn isolate_rolls_back_global_defs() {
    let src = r#"
        (def before 1)
        ;; Inside the thunk: rebind an existing global and define a new one.
        (def saw (%isolate (fn () (def before 2) (def new-one 3) (list before new-one))))
        ;; saw shows the defs took effect inside; after, `before` is back to 1 and
        ;; `new-one` is unbound again — the isolated mutations were discarded.
        (list saw before (try new-one (catch e :unbound)))
    "#;
    assert_eq!(run(src), "((2 3) 1 :unbound)");
}

// --- editor-parseable error positions (docs/tooling.md) ----------------------

/// A parse error carries the reader's precise line:col.
#[test]
fn parse_errors_carry_precise_position() {
    let mut interp = Interp::new();
    // Stray ')' on line 2, column 3.
    let err = interp.eval_source("(+ 1 2)\n  )\n").unwrap_err();
    let pos = err.pos.expect("parse error should have a position");
    assert_eq!((pos.line, pos.col), (2, 3));
}

/// A runtime error from a primitive is tagged with the *innermost* combination
/// that triggered it — not just the enclosing top-level form's line. The eval
/// loop's `or_form_pos` shape preserves the most precise known position
/// (innermost wins).
#[test]
fn runtime_errors_carry_innermost_form_position() {
    let mut interp = Interp::new();
    // First form is fine; `(first 5)` on line 5 col 3 is where the error fires.
    let err = interp
        .eval_source("(+ 1 2)\n(def x 1)\n(def y 2)\n(do\n  (first 5))\n")
        .unwrap_err();
    let pos = err.pos.expect("runtime error should have a position");
    assert_eq!((pos.line, pos.col), (5, 3));
}

/// A primitive misuse inside a `let` RHS lands on the RHS's line, not the let's.
#[test]
fn runtime_error_inside_let_rhs_points_at_rhs() {
    let mut interp = Interp::new();
    // The let opens on line 1; `(first 99)` is on line 3 col 9.
    let err = interp
        .eval_source("(let (a 1\n      b 2\n      c (first 99))\n  c)\n")
        .unwrap_err();
    let pos = err.pos.expect("runtime error should have a position");
    assert_eq!((pos.line, pos.col), (3, 9));
}

/// A primitive misuse inside an `if` test lands on the test's line, not the if's.
#[test]
fn runtime_error_inside_if_test_points_at_test() {
    let mut interp = Interp::new();
    // The if opens on line 1; the test `(+ 1 "x")` is on line 2 col 3.
    let err = interp
        .eval_source("(if\n  (+ 1 \"x\")\n  :then\n  :else)\n")
        .unwrap_err();
    let pos = err.pos.expect("runtime error should have a position");
    assert_eq!((pos.line, pos.col), (2, 3));
}

/// An unbound symbol at the head of a *tail-position* call (the last form
/// of a `do`/`let`/`letrec` body, reached via `expr = last; continue 'tail`)
/// must still carry the call's position. The unbound error fires inside the
/// symbol-head branch of the eval loop — a path that exits via raw `?` to the
/// eval frame's caller, so no enclosing `or_form_pos` ever sees it. Regression
/// guard for the explicit `or_form_pos(call_form)` on that branch.
#[test]
fn unbound_head_in_tail_position_carries_call_pos() {
    let mut interp = Interp::new();
    // `(nope-fn)` is the tail of a `do` whose top-level form opens on line 1;
    // before the fix this reported line 1, now it reports line 3 col 3.
    let err = interp
        .eval_source("(do\n  (println :a)\n  (nope-fn))\n")
        .unwrap_err();
    let pos = err.pos.expect("unbound error should have a position");
    assert_eq!((pos.line, pos.col), (3, 3));
}

/// Macroexpand rebuilds list forms (the compile pass walks the whole tree);
/// the rebuilt forms must carry the original's position so a diagnostic from
/// inside an expanded form still points at the source line. Regression:
/// before this carry-through, every inner position was lost to expansion and
/// the diagnostic fell back to the top-level form's start.
#[test]
fn position_survives_macroexpansion() {
    let mut interp = Interp::new();
    // `when` is a prelude macro that expands to `if`+`do`. The misuse is on
    // line 3 col 5 — inside the body of the `when`, which gets rebuilt by the
    // compile pass.
    let err = interp
        .eval_source("(def x 1)\n(when true\n    (first 5))\n")
        .unwrap_err();
    let pos = err.pos.expect("runtime error should have a position");
    assert_eq!((pos.line, pos.col), (3, 5));
}

/// The GNU `[FILE:]LINE:COL: kind error: msg` formatter — what editors parse.
/// File + pos come from `load`; pos refinement comes from the eval loop. End
/// to end: we should be able to point a tool at a `.blsp` file and get a
/// jumpable diagnostic.
#[test]
fn located_diagnostic_carries_file_line_col() {
    let mut path = std::env::temp_dir();
    path.push(format!("brood-loc-{}.blsp", std::process::id()));
    std::fs::write(&path, "(def x 1)\n(def y 2)\n(first 99)\n").unwrap();
    let p_str = path.to_string_lossy().to_string();

    let err = Interp::new()
        .eval_str(&format!("(load {:?})", p_str))
        .unwrap_err();
    let line = err.located();
    // PATH:3:1: type error: first: ...
    assert!(
        line.starts_with(&format!("{}:3:1: type error:", p_str)),
        "unexpected diagnostic: {}",
        line
    );

    let _ = std::fs::remove_file(&path);
}

/// `eval_str` (no file context) still attaches the innermost form's position
/// to the error — it just leaves `file` unset. Useful for the REPL: a
/// multi-line input still gets a `LINE:COL:` prefix on the diagnostic.
#[test]
fn eval_str_attaches_position_no_file() {
    let mut interp = Interp::new();
    // Multi-line input; misuse is on line 3 col 3.
    let err = interp
        .eval_str("(do\n  (println :a)\n  (first 5))")
        .unwrap_err();
    let pos = err
        .pos
        .expect("eval_str should still tag the innermost pos");
    assert_eq!((pos.line, pos.col), (3, 3));
    assert!(err.file.is_none(), "eval_str must not invent a file");
}

// ---- regression tests for the correctness/robustness fixes ----

/// `<` must compare integers exactly. Coercing to f64 first would collapse
/// values past 2^53 onto the same float (these two differ by 1).
#[test]
fn int_comparison_is_exact_past_2_pow_53() {
    assert_eq!(run("(< 9007199254740992 9007199254740993)"), "true");
    assert_eq!(run("(< 9007199254740993 9007199254740992)"), "false");
    assert_eq!(run("(< 1 2)"), "true");
    assert_eq!(run("(< 2.5 3)"), "true");
}

/// `mod`/`rem`/`/` on the one overflowing integer case (`i64::MIN` by `-1`) must
/// return a clean error or a float — never panic-abort the interpreter.
#[test]
fn integer_overflow_does_not_panic() {
    // The `i64::MIN op -1` edge cases no longer overflow-error: rem/mod are
    // mathematically 0, and quot promotes to the bignum `9223372036854775808`
    // (ADR bignums — integer arithmetic auto-promotes rather than trapping).
    assert_eq!(run("(mod -9223372036854775808 -1)"), "0");
    assert_eq!(run("(rem -9223372036854775808 -1)"), "0");
    assert_eq!(run("(quot -9223372036854775808 -1)"), "9223372036854775808");
    // `/` keeps its i64 fast path: the `i64::MIN / -1` overflow falls through to
    // the float path (the Int/Int arm doesn't promote — only the explicit bignum
    // operand case does), so this stays a `Float` as before.
    assert!(matches!(
        Interp::new().eval_str("(/ -9223372036854775808 -1)"),
        Ok(brood::core::value::Value::Float(_))
    ));
    // Ordinary integer division/modulo unaffected.
    assert_eq!(run("(/ 12 3)"), "4");
    assert_eq!(run("(/ 7 2)"), "3.5");
    assert_eq!(run("(mod -7 3)"), "2");
    assert_eq!(run("(rem -7 3)"), "-1");
}

/// `bit-count` (population count) over the two's-complement bit pattern.
#[test]
fn bit_count_counts_set_bits() {
    assert_eq!(run("(bit-count 0)"), "0");
    assert_eq!(run("(bit-count 7)"), "3");
    assert_eq!(run("(bit-count 255)"), "8");
    assert_eq!(run("(bit-count (bit-shift-left 1 40))"), "1");
    // A negative integer counts its sign bits: -1 is all 64 bits set.
    assert_eq!(run("(bit-count -1)"), "64");
}

/// `bit-positions` — the ascending 0-based indices of set bits, O(popcount),
/// across i64 and bignum.
#[test]
fn bit_positions_lists_set_bits() {
    assert_eq!(run("(bit-positions 0)"), "[]");
    assert_eq!(run("(bit-positions 6)"), "[1 2]");
    assert_eq!(run("(bit-positions 255)"), "[0 1 2 3 4 5 6 7]");
    // Bignum: a bit set past the i64 range is found at its true index.
    assert_eq!(
        run("(bit-positions (bit-or (bit-shift-left 1 200) (bit-shift-left 1 5)))"),
        "[5 200]"
    );
    // Inverse of bit-count: same number of positions as set bits.
    assert_eq!(run("(count (bit-positions (bit-shift-left 1 200)))"), "1");
}

/// `=` on floats uses IEEE value equality, not bitwise: `-0.0 = 0.0` is true.
#[test]
fn float_equality_is_ieee() {
    assert_eq!(run("(= 0.0 -0.0)"), "true");
    assert_eq!(run("(= 1.5 1.5)"), "true");
    assert_eq!(run("(= 1.5 2.5)"), "false");
}

/// `def` of a long list must not overflow: promotion into the shared region
/// walks the cons spine iteratively, not `length`-deep recursion.
#[test]
fn def_of_long_list_does_not_overflow() {
    let src = "(def build (fn (n acc) (if (= n 0) acc (build (- n 1) (cons n acc)))))\
               (def big (build 200000 nil)) (first big)";
    assert_eq!(run(src), "1");
}

/// Structural `=` on long lists must not overflow: it walks the spine
/// iteratively. Also confirm it still discriminates.
#[test]
fn equal_on_long_lists_does_not_overflow() {
    let build = "(def build (fn (n acc) (if (= n 0) acc (build (- n 1) (cons n acc)))))";
    assert_eq!(
        run(&format!(
            "{build} (let (a (build 200000 nil) b (build 200000 nil)) (= a b))"
        )),
        "true"
    );
    assert_eq!(
        run(&format!(
            "{build} (let (a (build 5 nil) b (build 6 nil)) (= a b))"
        )),
        "false"
    );
}

/// The printer emits dotted notation for improper lists; the reader must read it
/// back (round-trip), while a lone `.` stays distinct from atoms like `.5`.
#[test]
fn dotted_pairs_round_trip() {
    assert_eq!(run(r#"(pr-str (cons 1 2))"#), r#""(1 . 2)""#);
    assert_eq!(run(r#"(pr-str (cons 1 (cons 2 3)))"#), r#""(1 2 . 3)""#);
    assert_eq!(run(r#"(first (read-string "(1 2 . 3)"))"#), "1");
    assert_eq!(run(r#"(rest (read-string "(1 . 2)"))"#), "2");
    assert_eq!(run(r#"(read-string "(1 2 3)")"#), "(1 2 3)"); // proper list unaffected
    assert_eq!(run("(first (list .5 6))"), "0.5"); // `.5` is a float, not a separator

    let mut interp = Interp::new();
    assert!(interp.eval_str(r#"(read-string "( . 3)")"#).is_err());
    assert!(interp.eval_str(r#"(read-string "(1 . )")"#).is_err());
    assert!(interp.eval_str(r#"(read-string "(1 . 2 3)")"#).is_err());
}

/// Dynamic variables (`defdyn`/`binding`): default, dynamic shadowing through a
/// function call, restoration, and the unwind on error.
#[test]
fn dynamic_variables() {
    assert_eq!(run("(defdyn *d* 0) *d*"), "0");
    // `binding` shadows for the dynamic extent, then restores.
    assert_eq!(
        run("(defdyn *d* 0) (list (binding (*d* 7) *d*) *d*)"),
        "(7 0)"
    );
    // Resolved at call time against the caller's binding, not at definition.
    assert_eq!(
        run("(defdyn *d* 0) (defn rd () *d*) (binding (*d* 42) (rd))"),
        "42"
    );
    // Nested bindings stack; inner wins.
    assert_eq!(
        run("(defdyn *d* 0) (binding (*d* 1) (binding (*d* 2) *d*))"),
        "2"
    );
    // The binding is unwound even when the body throws.
    assert_eq!(
        run("(defdyn *d* 0) (try (binding (*d* 5) (throw \"x\")) (catch e nil)) *d*"),
        "0"
    );
    // `dynamic?` reports declared dynamics only.
    assert_eq!(
        run("(defdyn *d* 0) (list (dynamic? '*d*) (dynamic? 'rd) (dynamic? 42))"),
        "(true false false)"
    );
}

/// `binding` on a variable that was never declared dynamic is an error — the
/// usual cause is a typo, and silently rebinding a plain global would mislead.
#[test]
fn binding_undeclared_is_an_error() {
    let mut interp = Interp::new();
    assert!(interp
        .eval_str("(binding (not-dynamic 1) not-dynamic)")
        .is_err());
}

/// A pathological deeply-nested input is rejected as a parse error rather
/// than overflowing the native Rust stack. Guards the depth caps added to
/// `reader.rs`, `cst.rs`, `printer.rs`, `eval/macros.rs`, and the wire codec
/// — any one of those reverting would either error here or abort the test
/// runner.
#[test]
fn parser_rejects_deeply_nested_input_instead_of_overflowing() {
    // Run on a thread with a realistic stack. The parser caps nesting at 256
    // levels (the property under test — it returns a depth-cap error instead of
    // recursing unbounded), but unwinding even 256 large *debug* frames needs
    // more than nextest's ~2 MB default test-thread stack, so a debug `make test`
    // would abort before the cap's error could surface. Release frames are small
    // enough that it passes either way; the real runtime runs eval on a generous
    // stack too. This test asserts graceful rejection, not minimal stack use.
    let msg = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let mut interp = brood::Interp::new();
            let src: String = "(".repeat(5000);
            let err = interp.eval_str(&src).expect_err("must reject deep input");
            format!("{}", err)
        })
        .expect("spawn deep-parse thread")
        .join()
        .expect("deep-parse thread must not overflow the stack");
    assert!(
        msg.contains("nested too deeply"),
        "expected a depth-cap parse error, got: {}",
        msg
    );
}

/// `floor` on `NaN`/`±inf`/out-of-`i64`-range values is a runtime error,
/// not a silent saturating cast. Pre-fix, `(floor (* 1e308 1e308))` returned
/// `i64::MAX` and `(floor (/ 0.0 0.0))` returned `0`.
#[test]
fn floor_rejects_non_finite_and_out_of_range() {
    let mut interp = brood::Interp::new();
    assert!(interp.eval_str("(floor (* 1e308 1e308))").is_err());
    assert!(interp.eval_str("(floor (/ 0.0 0.0))").is_err());
    // In-range floats still work.
    let v = interp.eval_str("(floor 3.7)").unwrap();
    assert_eq!(interp.print(v), "3");
    let v = interp.eval_str("(floor -3.2)").unwrap();
    assert_eq!(interp.print(v), "-4");
}

/// An integer-shaped literal that doesn't fit in `i64` is a parse error,
/// not a silent fall-through to `Float`. Pre-fix, `9223372036854775808`
/// quietly read as `9.22e18` and any downstream type-check or arithmetic
/// silently used the rounded float.
#[test]
fn reader_promotes_out_of_range_integer_literal_to_bignum() {
    // An integer literal outside i64 range now PROMOTES to an arbitrary-precision
    // bignum, reading back as its exact value — not rejected, and not silently
    // rounded to a float (the pre-bignum bug, where `9223372036854775808` quietly
    // read as `9.22e18`).
    assert_eq!(run("9223372036854775808"), "9223372036854775808");
    // The float-shaped sibling still reads as float (here, +inf).
    let mut interp = brood::Interp::new();
    assert!(interp.eval_str("1e1000").is_ok());
}

/// Tail-recursion via `apply` doesn't blow the Rust stack. Pre-fix, an
/// `(apply f …)` recursing on itself grew the Rust stack ~4 frames per
/// level because `apply` → `apply_closure` → `eval(last)` recursed through
/// native code rather than trampolining; with the `apply_with_tco` loop in
/// `eval/mod.rs`, the recursion stays O(1) on the Rust stack.
#[test]
fn apply_tail_recursion_does_not_overflow() {
    let src = "
        (def loop-apply
          (fn [n acc]
            (if (= n 0) acc (apply loop-apply (list (- n 1) (+ acc n))))))
        (loop-apply 100000 0)
    ";
    assert_eq!(run(src), "5000050000");
}

/// Named-spawn (ADR-039 step 1): `(spawn :name expr)` is idempotent on
/// `name`. A second call while the first process is alive returns the
/// existing pid and **does not evaluate** the new thunk.
///
/// **Name is unique per test** (`:named-spawn-idempotent-test`) because the
/// `NAMES` table is a process-wide static shared across all tests in this
/// binary — collisions with sibling tests would race.
#[test]
fn named_spawn_is_idempotent_while_alive() {
    let mut interp = Interp::new();
    interp
        .eval_str(
            "(defn nsi-loop () (receive (after 60000 nil)) (nsi-loop))
             (def p1 (spawn :named-spawn-idempotent-test (nsi-loop)))
             (def p2 (spawn :named-spawn-idempotent-test
                            (throw \"second thunk should NOT run\")))",
        )
        .expect("spawn calls should succeed");
    // The two pids are the same value — the second spawn was a no-op.
    let same = interp.eval_str("(= p1 p2)").expect("eq check");
    assert_eq!(interp.print(same), "true");
}

/// When a named process dies, its name is reaped — a subsequent
/// `(spawn :name …)` spawns fresh (does not reuse the dead pid).
/// Exercises `dist::unregister_dead_pid` wired into `process::deregister`.
#[test]
fn named_spawn_respawns_after_death() {
    let mut interp = Interp::new();
    // First named spawn — process exits immediately (body is `nil`).
    let p1 = interp
        .eval_str("(spawn :named-spawn-respawn-test nil)")
        .expect("first spawn ok");
    let p1_str = interp.print(p1);
    // Wait for the scheduler to run + deregister to fire.
    std::thread::sleep(std::time::Duration::from_millis(50));
    // The name has been reaped — `whereis` returns nil.
    let where_ = interp
        .eval_str("(whereis :named-spawn-respawn-test)")
        .expect("whereis");
    assert_eq!(
        interp.print(where_),
        "nil",
        "name should be reaped after process death"
    );
    // A second spawn under the same name creates a fresh process — different pid.
    let p2 = interp
        .eval_str("(spawn :named-spawn-respawn-test nil)")
        .expect("second spawn ok");
    let p2_str = interp.print(p2);
    assert_ne!(
        p1_str, p2_str,
        "fresh spawn after reap must produce a distinct pid (got both as {p1_str})"
    );
}

/// `(spawn a b c)` with three args is a macroexpand-time error — the macro
/// should refuse extra args rather than silently dropping them.
#[test]
fn spawn_with_three_args_is_a_macro_error() {
    let err = Interp::new()
        .eval_str("(spawn :a :b :c)")
        .expect_err("three-arg spawn must error");
    assert!(
        err.to_string().contains("spawn:") && err.to_string().contains("expected"),
        "should be a spawn shape error, got: {}",
        err
    );
}

/// `gensym` must mint process-wide-unique symbols, including across threads — a
/// thread-local counter would reset per worker and collide.
#[test]
fn gensym_is_unique_across_threads() {
    use std::collections::HashSet;
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                (0..2000)
                    .map(|_| match brood::core::value::gensym("g") {
                        brood::core::value::Value::Sym(s) => s,
                        _ => unreachable!(),
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut seen = HashSet::new();
    for h in handles {
        for sym in h.join().unwrap() {
            assert!(
                seen.insert(sym),
                "gensym produced a duplicate across threads"
            );
        }
    }
}

/// The test registry (`*units*` in `std/test.blsp`) must be resettable so a
/// long-lived image — the `nest mcp` hot-reload session (ADR-013) — that loads
/// the same test file twice doesn't double-count. `reset-units!` clears it; the
/// project test runner calls it before (re)loading test files. Here we simulate
/// the reload directly: registering a test twice inflates the count, and
/// `reset-units!` before re-registering restores a count of one.
#[test]
fn reset_units_prevents_reload_double_count() {
    let mut interp = Interp::new();
    interp.eval_str("(require 'test)").expect("require test");
    // Simulate a test file loaded twice into one image: two registrations.
    interp
        .eval_str(
            r#"(test/reset-units!) (test/test "t" (test/is true)) (test/test "t" (test/is true))"#,
        )
        .expect("register twice");
    let doubled = interp
        .eval_str("(get (test/run-tests-structured) :total)")
        .expect("run twice-registered");
    assert_eq!(
        interp.print(doubled),
        "2",
        "two registrations should report two tests"
    );
    // The fix: reset before the (re)load clears the stale registrations.
    interp
        .eval_str(r#"(test/reset-units!) (test/test "t" (test/is true))"#)
        .expect("reset then register once");
    let single = interp
        .eval_str("(get (test/run-tests-structured) :total)")
        .expect("run once-registered");
    assert_eq!(
        interp.print(single),
        "1",
        "reset should leave exactly one test"
    );
}

// ----- arbitrary-precision integers (bignums) -------------------------------
//
// Brood ints are i64 in the common case; a value outside the i64 range is a
// heap `BigInt` that auto-promotes on overflow and demotes the moment a result
// fits again (the normalize invariant). Bignums are transparently integers:
// `int?`/`number?`/`type-of` all treat them as `int`. See ADR (bignums).

#[test]
fn bignum_overflow_promotes() {
    // 10^12 * 10^12 = 10^24, well past i64::MAX (~9.2*10^18).
    assert_eq!(
        run("(* 1000000000000 1000000000000)"),
        "1000000000000000000000000"
    );
    // Still an integer to the language.
    assert_eq!(run("(int? (* 1000000000000 1000000000000))"), "true");
    assert_eq!(run("(number? (* 1000000000000 1000000000000))"), "true");
    assert_eq!(run("(type-of (* 1000000000000 1000000000000))"), ":int");
}

#[test]
fn bignum_demotes_when_result_fits() {
    // A computation that goes big then comes back in range returns an `Int`.
    assert_eq!(
        run("(- (* 1000000000000 1000000000000) (* 1000000000000 1000000000000))"),
        "0"
    );
    assert_eq!(
        run("(int? (- (* 1000000000000 1000000000000) (* 1000000000000 1000000000000)))"),
        "true"
    );
    // 2^100 / 2^100 demotes to 1.
    assert_eq!(
        run("(quot (bit-shift-left 1 100) (bit-shift-left 1 100))"),
        "1"
    );
}

#[test]
fn bignum_literal_roundtrips() {
    // A 60-digit literal parses to a bignum and prints back identically.
    let lit = "123456789012345678901234567890123456789012345678901234567890";
    assert_eq!(run(lit), lit);
    assert_eq!(run(&format!("(int? {lit})")), "true");
    // Negative over-range literal.
    assert_eq!(
        run("-99999999999999999999999999999"),
        "-99999999999999999999999999999"
    );
}

#[test]
fn bignum_shifts_unbounded() {
    // (bit-shift-left 1 200) is 1 followed by zeros — a 61-digit number.
    let s = run("(bit-shift-left 1 200)");
    assert_eq!(
        s,
        "1606938044258990275541962092341162602522202993782792835301376"
    );
    assert_eq!(run("(bit-count (bit-shift-left 1 200))"), "1");
    assert_eq!(run("(int? (bit-shift-left 1 200))"), "true");
    // A right shift undoes it.
    assert_eq!(run("(bit-shift-right (bit-shift-left 1 200) 200)"), "1");
    assert_eq!(
        run("(bit-shift-right (bit-shift-left 1 200) 100)"),
        run("(bit-shift-left 1 100)")
    );
    // Negative shift is still an error.
    assert!(fresh_interp().eval_str("(bit-shift-left 1 -1)").is_err());
}

#[test]
fn bignum_bitwise() {
    // AND of a value with itself round-trips.
    assert_eq!(
        run("(bit-and (bit-shift-left 1 200) (bit-shift-left 1 200))"),
        run("(bit-shift-left 1 200)")
    );
    // OR/XOR of disjoint high bits.
    assert_eq!(
        run("(= (bit-or (bit-shift-left 1 200) (bit-shift-left 1 100)) (+ (bit-shift-left 1 200) (bit-shift-left 1 100)))"),
        "true"
    );
    assert_eq!(
        run("(bit-xor (bit-shift-left 1 200) (bit-shift-left 1 200))"),
        "0"
    );
}

#[test]
fn bignum_quot_rem_mod() {
    // (quot 2^200 2^100) == 2^100.
    assert_eq!(
        run("(quot (bit-shift-left 1 200) (bit-shift-left 1 100))"),
        run("(bit-shift-left 1 100)")
    );
    // rem divides evenly here.
    assert_eq!(
        run("(rem (bit-shift-left 1 200) (bit-shift-left 1 100))"),
        "0"
    );
    // mod composes (prelude) over rem/+/-.
    assert_eq!(
        run("(mod (+ (bit-shift-left 1 200) 7) (bit-shift-left 1 100))"),
        "7"
    );
}

#[test]
fn bignum_comparisons() {
    // BigInt vs Int: a big positive is greater than any i64.
    assert_eq!(
        run("(> (bit-shift-left 1 200) 9223372036854775807)"),
        "true"
    );
    assert_eq!(
        run("(< (- 0 (bit-shift-left 1 200)) -9223372036854775808)"),
        "true"
    );
    // BigInt vs BigInt.
    assert_eq!(
        run("(< (bit-shift-left 1 100) (bit-shift-left 1 200))"),
        "true"
    );
    assert_eq!(
        run("(= (bit-shift-left 1 200) (bit-shift-left 1 200))"),
        "true"
    );
    // Int and BigInt are never equal (disjoint ranges).
    assert_eq!(run("(= 1 (bit-shift-left 1 200))"), "false");
    // <= boundary.
    assert_eq!(
        run("(<= (bit-shift-left 1 200) (bit-shift-left 1 200))"),
        "true"
    );
}

#[test]
fn bignum_equal_as_map_key() {
    // Two independently-computed equal bignums must be the same map key
    // (equal => same hash).
    assert_eq!(
        run("(get (assoc {} (* 1000000000000 1000000000000) :v) (* 1000000000000 1000000000000))"),
        ":v"
    );
}

#[test]
fn bignum_mixed_float() {
    // A float operand keeps the float path; the bignum coerces.
    assert_eq!(run("(int? (+ (bit-shift-left 1 200) 0.0))"), "false");
    assert_eq!(run("(> (+ (bit-shift-left 1 200) 0.0) 1.0)"), "true");
}

// ---- transient maps (Clojure's transient/assoc!/persistent!) ----

#[test]
fn transient_basic_build() {
    // The headline example: build with assoc!, then persistent!. The transient
    // build yields the *canonical* CHAMP shape — byte-identical to the persistent
    // literal (trie order, not insertion order).
    assert_eq!(
        run("(persistent! (assoc! (assoc! (transient {}) :a 1) :b 2))"),
        run("{:a 1 :b 2}")
    );
    assert_eq!(
        run("(= (persistent! (assoc! (assoc! (transient {}) :a 1) :b 2)) {:a 1 :b 2})"),
        "true"
    );
}

#[test]
fn transient_equals_persistent_fold() {
    // A transient build of N entries equals the persistent assoc fold.
    let n = 500;
    let transient = run(&format!(
        "(persistent! (reduce (fn (t i) (assoc! t i i)) (transient {{}}) (range {n})))"
    ));
    let persistent = run(&format!(
        "(reduce (fn (m i) (assoc m i i)) {{}} (range {n}))"
    ));
    assert_eq!(transient, persistent);
    // ...and round-tripping through transient is the identity on contents.
    assert_eq!(run(&format!("(= (persistent! (reduce (fn (t i) (assoc! t i i)) (transient {{}}) (range {n}))) (reduce (fn (m i) (assoc m i i)) {{}} (range {n})))")), "true");
}

#[test]
fn transient_seeded_from_existing_map() {
    // transient over a non-empty map: pre-watermark nodes are path-copied once,
    // the original stays immutable.
    assert_eq!(
        run("(let (m {:a 1}) (let (t (transient m)) (assoc! t :b 2) (= (persistent! t) {:a 1 :b 2})))"),
        "true"
    );
    // The seed map is untouched.
    assert_eq!(
        run("(let (m {:a 1}) (let (t (transient m)) (assoc! t :b 2) (persistent! t) m))"),
        "{:a 1}"
    );
}

#[test]
fn transient_dissoc_bang() {
    assert_eq!(
        run("(persistent! (dissoc! (assoc! (assoc! (transient {}) :a 1) :b 2) :a))"),
        "{:b 2}"
    );
}

#[test]
fn transient_lookups_on_live_transient() {
    // get / count / contains? work on a live transient (Clojure-style).
    assert_eq!(run("(get (assoc! (transient {}) :a 1) :a)"), "1");
    assert_eq!(
        run("(get (assoc! (transient {}) :a 1) :missing :dflt)"),
        ":dflt"
    );
    assert_eq!(
        run("(count (assoc! (assoc! (transient {}) :a 1) :b 2))"),
        "2"
    );
    assert_eq!(run("(contains? (assoc! (transient {}) :a 1) :a)"), "true");
    assert_eq!(run("(contains? (assoc! (transient {}) :a 1) :b)"), "false");
}

#[test]
fn transient_predicate_and_type_of() {
    assert_eq!(run("(transient? (transient {}))"), "true");
    assert_eq!(run("(transient? {})"), "false");
    assert_eq!(run("(transient? 1)"), "false");
    assert_eq!(run("(type-of (transient {}))"), ":transient");
    // A transient is NOT a map.
    assert_eq!(run("(map? (transient {}))"), "false");
    // ...and a persistent! result IS a map.
    assert_eq!(run("(map? (persistent! (transient {})))"), "true");
}

#[test]
fn assoc_bang_after_persistent_errors() {
    // Use a `let`-bound transient (a transient is process-local; you can't `def`
    // one into a shared global) — persist it, then the next op must error.
    let mut interp = fresh_interp();
    assert!(
        interp
            .eval_str("(let (t (transient {})) (persistent! t) (assoc! t :a 1))")
            .is_err(),
        "assoc! after persistent! must error"
    );
    assert!(
        interp
            .eval_str("(let (t (transient {})) (persistent! t) (dissoc! t :a))")
            .is_err(),
        "dissoc! after persistent! must error"
    );
    assert!(
        interp
            .eval_str("(let (t (transient {})) (persistent! t) (persistent! t))")
            .is_err(),
        "persistent! twice must error"
    );
}

#[test]
fn transient_assoc_returns_same_handle() {
    // assoc! is identity-mutable: it returns the same transient it was given.
    assert_eq!(
        run("(let (t (transient {})) (= t (assoc! t :a 1)))"),
        "true"
    );
}

// ----- ADR-096: VM inline caches (call-site / global-read / prim guards) -----
//
// The semantics these guard are late binding (a `def` must be visible at the
// very next call through an already-hot compiled call site) and dynamic-var
// shadowing (a `binding` must never be bypassed by a cached resolution). The
// differential harness covers breadth; these are the targeted edges.

/// A hot compiled call site (the IC has been hit repeatedly inside one
/// top-level form) must re-resolve after a `def` — the epoch guard, exercised
/// *within* a single compiled body, not across top-level forms.
#[test]
fn call_site_ic_sees_redefinition_within_a_form() {
    let mut interp = fresh_interp();
    interp.eval_str("(def f (fn () :old))").unwrap();
    // `loop-f` hammers the site so the IC installs, then the body redefines `f`
    // (via `eval`, a def during the loop) and calls the SAME site again.
    let v = interp
        .eval_str(
            "(def loop-f
               (fn [n]
                 (if (= n 0)
                   (do (eval '(def f (fn () :new))) (f))
                   (do (f) (loop-f (- n 1))))))
             (loop-f 50)",
        )
        .unwrap();
    assert_eq!(interp.print(v), ":new");
}

/// A dynamic symbol is never cached by the call-site or global-read IC: a
/// `binding` re-binds it without bumping the global epoch, so a cached
/// resolution would silently bypass the binding.
#[test]
fn ics_never_bypass_a_dynamic_binding() {
    let mut interp = fresh_interp();
    interp.eval_str("(defdyn dynf (fn () :default))").unwrap();
    interp
        .eval_str("(def call-dynf (fn [n acc] (if (= n 0) acc (call-dynf (- n 1) (dynf)))))")
        .unwrap();
    // Hammer the site outside any binding (IC would love to cache it)...
    let v = interp.eval_str("(call-dynf 50 nil)").unwrap();
    assert_eq!(interp.print(v), ":default");
    // ...then the same hot site under a `binding` must see the shadow value.
    let v = interp
        .eval_str("(binding [dynf (fn () :shadowed)] (call-dynf 50 nil))")
        .unwrap();
    assert_eq!(interp.print(v), ":shadowed");
    // And back outside, the default again (no stale shadow cached).
    let v = interp.eval_str("(call-dynf 50 nil)").unwrap();
    assert_eq!(interp.print(v), ":default");
}

/// The `Prim1` (`first`/`rest`) epoch guard: a redefinition of the operator is
/// seen by an already-compiled body on its next call.
#[test]
fn prim1_guard_sees_redefinition() {
    let mut interp = fresh_interp();
    interp
        .eval_str("(def use-first (fn [xs] (first xs)))")
        .unwrap();
    let v = interp.eval_str("(use-first (list 1 2))").unwrap();
    assert_eq!(interp.print(v), "1");
    interp.eval_str("(def first (fn [x] :redefined))").unwrap();
    let v = interp.eval_str("(use-first (list 1 2))").unwrap();
    assert_eq!(interp.print(v), ":redefined");
}

// ----- ADR-096 round 2: direct letrec self-recursion runs on the VM -----
//
// A `letrec` binder whose RHS is a `(fn …)` that calls itself used to make the
// whole enclosing closure ineligible (a value snapshot can't capture an
// in-progress binder), so the `defseq` family — `map`/`filter`/`mapcat`/
// `remove`/`keep` — and every hand-written local loop deferred to the
// tree-walker. The fix binds the closure's own name to itself in its captured
// env at build time (the tree-walker's late-bind). These guard the semantics;
// the differential harness compares the two engines across the whole suite.

/// A hand-written self-recursive `letrec` loop accumulates correctly on the VM.
#[test]
fn letrec_self_recursion_accumulates() {
    assert_eq!(
        run("(letrec (sumto (fn (n acc) (if (= n 0) acc (sumto (- n 1) (+ acc n))))) (sumto 100 0))"),
        "5050"
    );
}

/// The `defseq` ops (which expand to a self-recursive `letrec` loop) produce the
/// same results they always did, now that they compile rather than defer.
#[test]
fn defseq_ops_run_correctly() {
    assert_eq!(run("(map inc (range 5))"), "(1 2 3 4 5)");
    assert_eq!(run("(filter even? (range 10))"), "(0 2 4 6 8)");
    assert_eq!(
        run("(mapcat (fn (x) (list x x)) (range 3))"),
        "(0 0 1 1 2 2)"
    );
    assert_eq!(run("(remove even? (range 6))"), "(1 3 5)");
    assert_eq!(
        run("(keep (fn (x) (if (even? x) (* x 10) nil)) (range 6))"),
        "(0 20 40)"
    );
}

/// Each call builds a *fresh* self-recursive closure with its own captured env;
/// the recursive call must resolve through that instance's env, never a cached
/// resolution from another instance (the call-site IC stays disengaged for a
/// local-capturing frame). Two `make-counter` closures with different bases run
/// interleaved and must not cross-contaminate.
#[test]
fn letrec_self_recursion_is_per_instance() {
    // `count-down` returns a self-recursive accumulator closing over `base`; two
    // instances with different bases summed over the same depth must differ by
    // exactly depth*(b2-b1) — proving each saw its own `step`, not a sibling's.
    let prog = "
      (defn make-summer (base)
        (letrec (step (fn (n acc) (if (= n 0) acc (step (- n 1) (+ acc base n)))))
          step))
      (let (a (make-summer 0) b (make-summer 1000))
        (list (a 10 0) (b 10 0)))";
    // a: sum 1..10 = 55 ; b: 55 + 10*1000 = 10055
    assert_eq!(run(prog), "(55 10055)");
}

/// Mutual local recursion is *not* covered by the direct-self path and still
/// defers — but must remain correct.
#[test]
fn letrec_mutual_recursion_still_correct() {
    assert_eq!(
        run("(letrec (ev? (fn (n) (if (= n 0) true (od? (- n 1))))
                      od? (fn (n) (if (= n 0) false (ev? (- n 1)))))
               (list (ev? 10) (od? 7)))"),
        "(true true)"
    );
}

/// A binder bound to a *call that returns a fn* (not directly a `fn`) must not
/// be misclassified as self-recursive: the binder's value is the call result.
#[test]
fn letrec_non_fn_rhs_not_misclassified_as_self() {
    // `g` returns an identity fn; `h`'s value is `(g)`'s result, which captures
    // nothing recursive. If the compiler wrongly bound `h` to the inner fn, the
    // body's `(h 41)` would misbehave; correct is 42.
    assert_eq!(
        run("(letrec (g (fn () (fn (x) (inc x))) h (g)) (h 41))"),
        "42"
    );
}
