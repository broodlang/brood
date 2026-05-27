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
    assert_send::<brood::heap::Heap>();
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
    assert_eq!(run("(def adder (fn [a] (fn [b] (+ a b)))) ((adder 3) 4)"), "7");
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
fn defn_defines_functions() {
    assert_eq!(run("(defn sq [x] (* x x)) (sq 6)"), "36");
    assert_eq!(run("(defn add3 [a b c] (+ a b c)) (add3 1 2 3)"), "6");
    // defn is itself written in Brood; it expands to (def name (fn ...)).
    assert_eq!(run("(macroexpand-1 '(defn f [x] (+ x 1)))"), "(def f (fn [x] (+ x 1)))");
}

#[test]
fn params_may_be_a_list_or_vector() {
    assert_eq!(run("(defn sq (x) (* x x)) (sq 7)"), "49");      // list params
    assert_eq!(run("(defn sq2 [x] (* x x)) (sq2 8)"), "64");    // vector params
    assert_eq!(run("((fn (a b) (+ a b)) 2 3)"), "5");
    assert_eq!(run("(defn rest-args (& xs) xs) (rest-args 1 2 3)"), "(1 2 3)");
}

#[test]
fn optional_params() {
    let g = "(defn greet (name &optional (greeting \"hi\")) (str greeting \", \" name))";
    assert_eq!(run(&format!("{} (greet \"Ada\")", g)), "\"hi, Ada\"");
    assert_eq!(run(&format!("{} (greet \"Ada\" \"yo\")", g)), "\"yo, Ada\"");
    // a default may reference an earlier parameter (left-to-right binding)
    assert_eq!(run("(defn rect (w &optional (h w)) (* w h)) (rect 5)"), "25");
    assert_eq!(run("(defn rect (w &optional (h w)) (* w h)) (rect 5 3)"), "15");
    // a bare optional defaults to nil
    assert_eq!(run("(defn f (a &optional b) (list a b)) (f 1)"), "(1 nil)");
    // optionals work on a raw fn, not just defn
    assert_eq!(run("((fn (a &optional (b 10)) (+ a b)) 5)"), "15");
    // optionals compose with & rest
    assert_eq!(run("(defn f (a &optional (b 2) & more) (list a b more)) (f 1)"), "(1 2 nil)");
    assert_eq!(run("(defn f (a &optional (b 2) & more) (list a b more)) (f 1 9 8 7)"), "(1 9 (8 7))");
}

#[test]
fn optional_params_arity() {
    let mut interp = Interp::new();
    // too many args, no rest to absorb them
    assert!(interp.eval_str("(defn f (a &optional b) a) (f 1 2 3)").is_err());
    // too few required
    assert!(interp.eval_str("(defn f (a b &optional c) a) (f 1)").is_err());
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
fn throw_and_catch() {
    // a thrown value is rebound by catch
    assert_eq!(run("(try (throw 42) (catch e e))"), "42");
    assert_eq!(run("(try (throw :boom) (catch e (str \"caught \" e)))"), "\"caught :boom\"");
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
