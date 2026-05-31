//! Differential engine test (ADR-076 §7): every expression in the corpus is
//! evaluated through **both** engines — the tree-walker and the compiling VM — in a
//! fresh `Interp` each, and the results must be identical (same printed value, or
//! both errors with the same message). This is the standing regression guard that
//! the VM never diverges from the reference tree-walker semantics — the safety net
//! under which VM coverage can be widened (variadic arms, etc.).
//!
//! The engine is pinned per-eval via `compile::set_forced_engine`, which overrides
//! the `BROOD_VM` env / build default (so this test is independent of how the suite
//! is being run). A *fresh* interpreter per (expr, engine) keeps side effects
//! (`def`, `print`, `spawn`) from leaking across the two runs.

use std::sync::LazyLock;

use brood::eval::compile::set_forced_engine;
use brood::Interp;

static MEM_GUARD: LazyLock<()> = LazyLock::new(|| {
    brood::core::alloc::init_limits_with_default(
        brood::core::alloc::TEST_DEFAULT_HARD,
        brood::core::alloc::TEST_DEFAULT_SOFT,
    );
});

/// Evaluate `src` in a fresh interpreter pinned to one engine. `Ok(printed)` or
/// `Err(message)` — the message alone (engine-independent), not the position, which
/// is asserted separately in `basic.rs`.
fn eval_on(src: &str, vm: bool) -> Result<String, String> {
    LazyLock::force(&MEM_GUARD);
    set_forced_engine(Some(vm));
    let mut interp = Interp::new();
    let out = match interp.eval_str(src) {
        Ok(v) => Ok(interp.print(v)),
        Err(e) => Err(e.message),
    };
    set_forced_engine(None);
    out
}

/// Assert both engines agree on `src`.
fn agree(src: &str) {
    let tw = eval_on(src, false);
    let vm = eval_on(src, true);
    assert_eq!(
        tw, vm,
        "engine divergence on:\n  {src}\n  tree-walker: {tw:?}\n  vm:          {vm:?}"
    );
}

/// The corpus — each entry is a self-contained program. Grouped by feature so a
/// failure points at the area. Add to this whenever the VM grows new coverage.
const CORPUS: &[&str] = &[
    // arithmetic / comparison / passthrough ops
    "(+ 1 2 3)",
    "(* 2 (- 10 3) 4)",
    "(< 1 2 3)",
    "(= 2 2 2)",
    "(and (< 1 2) (= 2 2) 7)",
    "(or false nil 5)",
    "(if (< 3 2) :a :b)",
    // let / let* / letrec / cond / when
    "(let (a 1 b 2) (+ a b))",
    "(let* (a 1 b (+ a 10)) (* a b))",
    "(letrec (ev? (fn (n) (if (= n 0) true (od? (- n 1)))) od? (fn (n) (if (= n 0) false (ev? (- n 1))))) (ev? 10))",
    "(cond false :a (= 1 1) :b else :c)",
    "(when (< 1 2) (+ 1 1) (* 3 3))",
    // macros in a fn body — the VM must defer a closure whose body still holds an
    // *unexpanded* (forward-referenced / lazily-expanded) macro, or it would compile
    // the macro's argument syntax as ordinary calls (the prelude `sleep`→`receive`
    // regression). `earlym` is defined before its use (expanded → VM-runs); `fwm`
    // after (forward ref → must defer to the tree-walker).
    "(defmacro earlym (x) `(+ ~x 1)) (defn ue (n) (earlym n)) (ue 5)",
    "(defn uf (n) (fwm n)) (defmacro fwm (x) `(* ~x 3)) (uf 7)",
    // a forward-referenced macro whose argument is itself macro syntax (pin/unquote)
    "(defn uw (n) (wrapm (~n))) (defmacro wrapm (x) `(quote ~x)) (uw 9)",
    // recursion + tail loops
    "(def fib (fn (n) (if (< n 2) n (+ (fib (- n 1)) (fib (- n 2)))))) (fib 15)",
    "(defn down (i acc) (if (= i 0) acc (down (- i 1) (+ acc 1)))) (down 50000 0)",
    // local-capturing closures (Stage 2c)
    "(defn make-adder (n) (fn (x) (+ x n))) ((make-adder 5) 37)",
    "(defn adder3 (a) (fn (b) (let (s (+ a b)) (fn (c) (+ s c))))) (((adder3 1) 2) 3)",
    "(defn drive (f acc i n) (if (= i n) acc (drive f (f acc) (+ i 1) n))) (drive (make-adder 1) 0 0 1000)",
    // higher-order + threading
    "(map (fn (x) (* x x)) (range 1 6))",
    "(filter even? (range 1 11))",
    "(reduce + 0 (range 1 101))",
    "(-> 5 (+ 3) (* 2))",
    "(->> (range 1 6) (map (fn (x) (* x x))) (reduce + 0))",
    // multi-arity
    "(defn g ((x) x) ((x y) (+ x y))) [(g 7) (g 3 4)]",
    // variadic call (and a variadic user fn)
    "(defn vsum (& xs) (reduce + 0 xs)) (vsum 1 2 3 4 5)",
    "(defn vsum (& xs) (reduce + 0 xs)) (vsum)",        // empty rest → ()
    "(defn greet (name & rest) [name (count rest)]) (greet :a :b :c)",
    "(defn greet (name & rest) [name rest]) (greet :a)", // rest is the empty list
    // &optional (nil default) — provided and missing
    "(defn opt (a &optional b c) [a b c]) [(opt 1) (opt 1 2) (opt 1 2 3)]",
    // &optional with REAL defaults — a default referencing an earlier param, a
    // later default referencing an earlier optional, a default with a `let`, and the
    // provided-arg case (default not evaluated). VM compiles these now (was deferred).
    "(defn rd (a &optional (b (+ a 1))) (+ a b)) [(rd 10) (rd 10 5)]",
    "(defn rd2 (a &optional (b (* a 2)) (c (+ b 1))) [a b c]) [(rd2 3) (rd2 3 100) (rd2 3 100 200)]",
    "(defn rd3 (a &optional (b (let (x 5) (+ a x)))) b) [(rd3 10) (rd3 10 0)]",
    // real-default &optional + & rest in one arm
    "(defn rdm (a &optional (b (* a 10)) & rest) [a b rest]) [(rdm 1) (rdm 1 2) (rdm 1 2 3 4)]",
    // &optional + & rest together
    "(defn mix (a &optional b & rest) [a b rest]) [(mix 1) (mix 1 2) (mix 1 2 3 4)]",
    // multi-arity WITH a variadic arm — the selection must match select_arm exactly
    // (fixed arms win for their exact arity; the rest arm catches the overflow)
    "(defn ma ((x) :one) ((x y) :two) ((x y & r) [:many (count r)])) [(ma 1) (ma 1 2) (ma 1 2 3 4)]",
    // a variadic helper driven in a tail loop (rest list rebuilt each call)
    "(defn vmax (& xs) (reduce (fn (a b) (if (< a b) b a)) (first xs) (rest xs))) (vmax 3 9 2 7 1)",
    // pattern-dispatch fns (lower to match* whose no-match arm is `(throw [:match-error
    // (quote ctx) m (quote pats)])`): the VM now compiles `quote` + vector/map literals,
    // so these run on the VM instead of deferring. Recursive + non-total + the throw path.
    "(defn pfib ((0) 0) ((1) 1) ((n) (+ (pfib (- n 1)) (pfib (- n 2))))) (pfib 12)",
    "(defn pf ((0) :zero) ((1 2) :one-two)) [(pf 0) (pf 1 2)]",
    "(defn pf ((0) :zero)) (pf 9)", // no clause matches → match-error throw (both engines)
    // quoted data + vector/map literals built inside a compiled body (non-constant
    // elements, so they're build-nodes, not folded Consts)
    "(defn qd (x) (list (quote a) x '(n e s t) [:v x] {:k x :q (quote s)})) (qd 7)",
    // a match expression whose clauses include a guard, list-destructure, and wildcard
    "(defn cl (x) (match x (n :when (< n 0) :neg) (0 :z) ((a b) (+ a b)) (_ :o))) [(cl -3) (cl 0) (cl (list 4 5)) (cl 9)]",
    // data structures
    "{:a 1 :b (+ 1 1)}",
    "(get (assoc {:x 1} :y 2) :y)",
    "[1 2 (+ 1 2) (* 2 2)]",
    "(first (rest [10 20 30]))",
    "(str \"a\" \"b\" \"c\")",
    // error cases — both engines must fail the same way
    "(first 5)",
    "(nope-undefined-fn 1)",
    "(/ 1 0)",
    "(+ 1 \"x\")",
    "(1 2 3)",   // calling a non-function — both engines: "cannot call non-function: 1"
];

#[test]
fn engines_agree_on_corpus() {
    // Run on a large, explicitly-sized stack (like tests/suite.rs and the `brood`
    // binaries): some corpus entries are pattern-dispatch fns whose `match*`
    // expansion is a large nested-`if` tree, and macroexpand/compile recurse over it
    // deeper than libtest's small default thread stack would allow.
    std::thread::Builder::new()
        .stack_size(brood::process::CORO_STACK_BYTES)
        .spawn(|| {
            for &src in CORPUS {
                agree(src);
            }
        })
        .expect("spawn differential corpus thread")
        .join()
        .expect("differential corpus thread panicked");
}

/// Regression: a closure body that holds a macro **defined after** it (a forward
/// reference) keeps the macro call *unexpanded* — `macroexpand_all` couldn't expand
/// it at definition time, and it's expanded lazily at eval. The VM must **defer**
/// such a closure to the tree-walker, not compile the raw macro call (which would
/// treat the macro's argument syntax — pin patterns, `~`-unquotes — as ordinary
/// calls → "unbound symbol: unquote" / "cannot call non-function"). This was a
/// latent VM bug for user closures, and the prelude's `sleep`→`receive` (sleep is
/// defined before receive) hit it the moment prelude closures became VM-eligible.
#[test]
fn vm_defers_unexpanded_forward_referenced_macro() {
    // forward ref: `fwm` defined after `uf` → must defer, still produce 21.
    assert_eq!(
        eval_on("(defn uf (n) (fwm n)) (defmacro fwm (x) `(* ~x 3)) (uf 7)", true),
        Ok("21".to_string()),
    );
    // a forward macro whose argument is itself macro syntax (the `~`-pin shape that
    // produced the original "unbound unquote").
    assert_eq!(
        eval_on("(defn uw (n) (pm (~n))) (defmacro pm (x) `(quote ~x)) (uw 9)", true),
        eval_on("(defn uw (n) (pm (~n))) (defmacro pm (x) `(quote ~x)) (uw 9)", false),
    );
    // a macro defined *before* still VM-compiles (we didn't over-defer).
    assert_eq!(
        eval_on("(defmacro em (x) `(+ ~x 1)) (defn ue (n) (em n)) (ue 41)", true),
        Ok("42".to_string()),
    );
}
