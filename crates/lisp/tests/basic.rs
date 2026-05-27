//! End-to-end tests for the v0.1 language: read a string, evaluate it, and
//! check the printed result. These double as executable documentation of what
//! the language can currently do.

use mylisp::{printer, Interp};

/// Evaluate `src` in a fresh interpreter and return the printed result.
fn run(src: &str) -> String {
    let interp = Interp::new();
    printer::print(&interp.eval_str(src).expect("evaluation failed"))
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
    assert_eq!(run("(let [a 1 b (+ a 1)] (+ a b))"), "3");
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

/// The headline property: deep tail recursion must not overflow the Rust stack.
#[test]
fn tail_calls_do_not_overflow() {
    let src = "
        (def sum-to
          (fn [n acc]
            (if (= n 0) acc (sum-to (- n 1) (+ acc n)))))
        (sum-to 1000000 0)
    ";
    assert_eq!(run(src), "500000500000");
}

/// The foundation for editing the editor on the fly: redefining a function in
/// the live global environment changes behaviour immediately.
#[test]
fn live_redefinition() {
    let interp = Interp::new();
    interp.eval_str("(def greet (fn [] :v1))").unwrap();
    assert_eq!(printer::print(&interp.eval_str("(greet)").unwrap()), ":v1");
    interp.eval_str("(def greet (fn [] :v2))").unwrap();
    assert_eq!(printer::print(&interp.eval_str("(greet)").unwrap()), ":v2");
}

/// `eval` + `read-string` let the language run code it builds at runtime.
#[test]
fn eval_and_read_string() {
    assert_eq!(run("(eval (read-string \"(+ 40 2)\"))"), "42");
}

#[test]
fn errors_are_reported() {
    let interp = Interp::new();
    assert!(interp.eval_str("(+ 1 nope)").is_err());
    assert!(interp.eval_str("(this-is-not-defined)").is_err());
    assert!(interp.eval_str("(/ 1 0)").is_err());
}
