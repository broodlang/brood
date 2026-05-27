//! End-to-end tests for the v0.1 language: read a string, evaluate it, and
//! check the printed result. These double as executable documentation of what
//! the language can currently do.

use brood::Interp;

/// Evaluate `src` in a fresh interpreter and return the printed result.
fn run(src: &str) -> String {
    let mut interp = Interp::new();
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
    // A literal prints insertion-ordered; assoc/dissoc return fresh maps.
    assert_eq!(run("{:a 1 :b 2}"), "{:a 1, :b 2}");
    assert_eq!(run("(get {:a 1 :b 2} :b)"), "2");
    assert_eq!(run("(get {:a 1} :z 99)"), "99");
    assert_eq!(run("(assoc {:a 1} :b 2)"), "{:a 1, :b 2}");
    // assoc does not mutate the original binding.
    assert_eq!(run("(def m {:a 1}) (assoc m :b 2) m"), "{:a 1}");
    assert_eq!(run("(dissoc {:a 1 :b 2 :c 3} :b)"), "{:a 1, :c 3}");
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
    // assoc updates in place (keeps position); a new key appends.
    assert_eq!(run("(keys (assoc {:a 1 :b 2} :a 9))"), "(:a :b)");
    assert_eq!(run("(keys (assoc {:a 1} :b 2))"), "(:a :b)");
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
    assert_eq!(loc(&mut interp, "(source-location 'foo)"), format!("[\"{p}\" 1 1]"));
    assert_eq!(loc(&mut interp, "(source-location 'bar)"), format!("[\"{p}\" 2 1]"));
    // A prelude global has no recorded site; nor does an unknown name.
    assert_eq!(loc(&mut interp, "(source-location 'map)"), "nil");
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
    // a built-in error is caught as its message string
    assert_eq!(run("(try (/ 1 0) (catch e (string? e)))"), "true");
    // error: raise a formatted message
    assert_eq!(run("(try (error \"nope: \" 5) (catch e e))"), "\"nope: 5\"");
    // try with no catch clause is just a do
    assert_eq!(run("(try 1 2 3)"), "3");
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

/// A runtime error is tagged with the enclosing top-level form's start line.
#[test]
fn runtime_errors_carry_top_level_form_position() {
    let mut interp = Interp::new();
    // First form is fine; the unbound reference is in the form starting line 3.
    let err = interp.eval_source("(+ 1 2)\n\n(+ 1 nope)\n").unwrap_err();
    let pos = err.pos.expect("runtime error should have a position");
    assert_eq!(pos.line, 3);
}

/// `eval_str` (no file context) leaves the position unset for callers that
/// don't want location tagging, e.g. the REPL.
#[test]
fn eval_str_leaves_position_unset() {
    let mut interp = Interp::new();
    let err = interp.eval_str("(+ 1 nope)").unwrap_err();
    assert!(err.pos.is_none());
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
    let mut interp = Interp::new();
    assert!(interp.eval_str("(mod -9223372036854775808 -1)").is_err());
    assert!(interp.eval_str("(rem -9223372036854775808 -1)").is_err());
    // `/` falls through to the float path rather than erroring.
    assert!(matches!(
        interp.eval_str("(/ -9223372036854775808 -1)"),
        Ok(brood::core::value::Value::Float(_))
    ));
    // Ordinary integer division/modulo unaffected.
    assert_eq!(run("(/ 12 3)"), "4");
    assert_eq!(run("(/ 7 2)"), "3.5");
    assert_eq!(run("(mod -7 3)"), "2");
    assert_eq!(run("(rem -7 3)"), "-1");
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
    assert_eq!(run("(defdyn *d* 0) (list (binding (*d* 7) *d*) *d*)"), "(7 0)");
    // Resolved at call time against the caller's binding, not at definition.
    assert_eq!(
        run("(defdyn *d* 0) (defn rd () *d*) (binding (*d* 42) (rd))"),
        "42"
    );
    // Nested bindings stack; inner wins.
    assert_eq!(run("(defdyn *d* 0) (binding (*d* 1) (binding (*d* 2) *d*))"), "2");
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
    assert!(interp.eval_str("(binding (not-dynamic 1) not-dynamic)").is_err());
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
