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
    assert_eq!(
        run("(try (/ 1 0) (catch e (get e :code)))"),
        "\"E0040\""
    );
    // Hint rides along when the raise site set one — `with_hint("…")`.
    assert_eq!(
        run("(try (/ 1 0) (catch e (string? (get e :hint))))"),
        "true"
    );
    // E0041 integer overflow — checked-arithmetic raises on `%add` etc. when
    // the result wouldn't fit i64. `(* i64::MAX 2)` is the easiest trigger.
    assert_eq!(
        run("(try (* 9223372036854775807 2) (catch e (get e :code)))"),
        "\"E0041\""
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
    assert_eq!(run("(try (no-such-fn) (catch e (get e :kind)))"), ":unbound");
    assert_eq!(
        run("(try (no-such-fn) (catch e (get e :code)))"),
        "\"E0010\""
    );
    // Type errors carry E0030; the message preserves the structured detail
    // (`wrong_type` includes "expected <kind>, got <kind> (<value>)").
    assert_eq!(
        run("(try (first 5) (catch e (get e :code)))"),
        "\"E0030\""
    );
    assert_eq!(
        run("(try (first 5) (catch e (get e :kind)))"),
        ":type"
    );
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

#[test]
fn unbound_in_root_thread_has_no_scheduler_hint() {
    // Negative case: the root thread (REPL / file runner / nest mcp
    // dispatcher) is *not* a green process, so the hint stays nil.
    assert_eq!(run("(try (no-such-fn) (catch e (get e :hint)))"), "nil");
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
    assert!(line.starts_with(&format!("{}:3:1: type error:", p_str)),
            "unexpected diagnostic: {}", line);

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
    let pos = err.pos.expect("eval_str should still tag the innermost pos");
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

/// A pathological deeply-nested input is rejected as a parse error rather
/// than overflowing the native Rust stack. Guards the depth caps added to
/// `reader.rs`, `cst.rs`, `printer.rs`, `eval/macros.rs`, and the wire codec
/// — any one of those reverting would either error here or abort the test
/// runner.
#[test]
fn parser_rejects_deeply_nested_input_instead_of_overflowing() {
    let mut interp = brood::Interp::new();
    let src: String = "(".repeat(5000);
    let err = interp.eval_str(&src).expect_err("must reject deep input");
    let msg = format!("{}", err);
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
fn reader_rejects_out_of_range_integer_literal() {
    let mut interp = brood::Interp::new();
    let err = interp
        .eval_str("9223372036854775808")
        .expect_err("must reject");
    let msg = format!("{}", err);
    assert!(
        msg.contains("integer literal out of range"),
        "expected an integer-range parse error, got: {}",
        msg
    );
    // The float-shaped sibling still reads as float (here, +inf).
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
        err.to_string().contains("spawn:")
            && err.to_string().contains("expected"),
        "should be a spawn shape error, got: {}",
        err
    );
}

/// Supervisor recovery (ADR-039): a worker spawned with `(supervise …)`
/// catches an uncaught throw at the process boundary and re-invokes the
/// *most recent tail call* with the **same args** — so a long-running
/// stateful loop survives a bad iteration. We exercise that by having a
/// tail-recursive worker that throws on iteration N=0, then count how
/// many times it reports reaching that iteration: once for the first
/// arrival + once per retry, up to the **restart intensity** (Erlang's
/// default of 3 retries within any 5 s window — `*supervise-max-restarts*`
/// / `*supervise-max-window-ms*` in the prelude). Plain `(spawn …)`
/// without `supervise` is still let-it-crash; this test wraps in
/// `supervise` explicitly.
///
/// Verifies, end-to-end:
/// 1. `record_resume` captures `(callee, argv)` on every tail-call dispatch
///    (we see argv=[0] re-used by the supervisor, not [3] or [2]).
/// 2. The supervisor catches a throw escaping the eval and loops.
/// 3. Restart intensity actually fires (we see exactly 3 retries within
///    the window, not unbounded; the 4th throw exceeds intensity).
#[test]
fn supervisor_retries_last_iteration_with_same_args() {
    let mut interp = Interp::new();
    interp
        .eval_str(
            "(def *sup-recovery-parent* (self))
             (defn sup-recovery-worker (n)
               (send *sup-recovery-parent* (vector :iter n))
               (if (= n 0) (throw \"boom\"))
               (sup-recovery-worker (- n 1)))
             (supervise (sup-recovery-worker 3))",
        )
        .expect("setup");

    // Collect messages with a generous overall timeout: backoff
    // (1+2+4 ms) keeps all 3 retries inside the 5 s window so the test
    // observes the cap firing rather than timing out.
    let mut iters: Vec<i64> = Vec::new();
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(10) {
        let got = interp
            .eval_str("(receive (v v) (after 2000 nil))")
            .expect("receive");
        let s = interp.print(got);
        if s == "nil" {
            break;
        }
        // s looks like "[:iter 3]" — parse the trailing integer.
        let n: i64 = s
            .trim_end_matches(']')
            .rsplit(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap_or_else(|_| panic!("unparseable iter msg: {s}"));
        iters.push(n);
    }
    // Expected with default intensity (3 in 5 s):
    //   first descent: 3, 2, 1, 0  — 1 zero
    //   retries 1, 2, 3 each see argv=[0]: + 3 zeros
    //   4th throw exceeds intensity → give up.
    // Total: [3, 2, 1, 0, 0, 0, 0] — 4 zeros.
    assert_eq!(
        iters.iter().take(4).copied().collect::<Vec<_>>(),
        vec![3, 2, 1, 0],
        "expected the worker's first descent 3→0, got {iters:?}"
    );
    let zeros = iters.iter().filter(|&&n| n == 0).count();
    assert_eq!(
        zeros, 4,
        "supervisor should fire 3 retries (Erlang default intensity) + the original arrival at n=0; got {zeros} zeros (full trace: {iters:?})"
    );
}

/// Supervisor + hot reload (ADR-039 × ADR-013). A worker throws on every
/// iteration; the parent `def`s a fixed version between throws; the
/// supervisor re-resolves the function name on its next retry and picks up
/// the fix. Without name-based re-resolution, the supervisor would call the
/// old throwing closure handle forever (up to the restart budget) — the
/// whole point of integrating supervision with hot reload.
#[test]
fn supervisor_picks_up_hot_reloaded_definition_on_retry() {
    let mut interp = Interp::new();
    // Use the primitive `%spawn-supervised` directly so we can hand it a
    // generous intensity (100 in 60 s). The default 3/5 s would fire ~10
    // ms after the spawn (each retry takes ~30 ms of sleep + a few ms of
    // backoff), well before the test thread sleeps 200 ms and `def`s the
    // fix.
    interp
        .eval_str(
            "(def *hr-parent* (self))
             (defn hr-worker (n)
               (do (sleep 30) (throw \"buggy\")))
             (%spawn-supervised (fn () (hr-worker 0)) 100 60000)",
        )
        .expect("setup");

    // Let the supervisor catch the buggy version a few times. The first
    // ~10 ms total of backoff (1+2+4+8 = 15 ms) ensures at least a couple
    // of catches before our `def` lands.
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Hot-reload the worker: now it sends a heartbeat instead of throwing.
    interp
        .eval_str(
            "(defn hr-worker (n)
               (do (send *hr-parent* [:hr-beat n])
                   (sleep 30)
                   (hr-worker (+ n 1))))",
        )
        .expect("redef");

    // The supervisor's next retry calls the new closure (resolved by name).
    // Read two beats — the first proves the fix took, the second proves the
    // re-resolved closure tail-recurses normally (it's the *new* fn, not the
    // captured handle).
    let first = interp
        .eval_str("(receive ([:hr-beat n] (vector :beat n)) (after 3000 :timeout))")
        .expect("receive 1");
    let second = interp
        .eval_str("(receive ([:hr-beat n] (vector :beat n)) (after 3000 :timeout))")
        .expect("receive 2");
    assert_eq!(
        interp.print(first),
        "[:beat 0]",
        "first beat should arrive once the fix is live"
    );
    assert_eq!(
        interp.print(second),
        "[:beat 1]",
        "second beat proves the new closure tail-recurses normally"
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
