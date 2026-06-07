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
/// is asserted separately in `basic.rs`. `vm = true` is the bytecode VM (the sole VM
/// executor since ADR-100 Stage 5); `vm = false` is the tree-walker (the reference).
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

/// Assert the bytecode VM agrees with the reference tree-walker on `src`.
fn agree(src: &str) {
    let tw = eval_on(src, false);
    let vm = eval_on(src, true);
    assert_eq!(
        tw, vm,
        "engine divergence on:\n  {src}\n  tree-walker: {tw:?}\n  bytecode VM: {vm:?}"
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
    // let (sequential) / letrec / cond / when
    "(let (a 1 b 2) (+ a b))",
    "(let (a 1 b (+ a 10)) (* a b))",
    "(letrec (ev? (fn (n) (if (= n 0) true (od? (- n 1)))) od? (fn (n) (if (= n 0) false (ev? (- n 1))))) (ev? 10))",
    // let-self-ref and letrec-self-ref closure send — both engines must agree on
    // rejection. The divergence (VM accepted, TW rejected let-self-ref) was the
    // blind spot that motivated this differential entry. Also exercise the call path
    // (let-self-ref recursion must work identically on both engines).
    "(defn mk-let () (let (f (fn (n) (if (= n 0) :done (f (- n 1))))) f)) (let (me (self)) (try (do (send me (mk-let)) :ACCEPT) (catch _ :REJECT)))",
    "(defn mk-letrec () (letrec (f (fn (n) (if (= n 0) :done (f (- n 1))))) f)) (let (me (self)) (try (do (send me (mk-letrec)) :ACCEPT) (catch _ :REJECT)))",
    "(defn mk-let () (let (f (fn (n) (if (= n 0) :done (f (- n 1))))) f)) ((mk-let) 10)",
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
    // try/catch — body and handler thunks route through apply_engine (builtins
    // `try_catch`). Both the success and the error path must agree across engines.
    "(defn safe-div (a b) (try (/ a b) (catch _ :div-zero))) [(safe-div 10 2) (safe-div 1 0)]",
    "(try (+ 1 2) (catch _ :err))",            // success path: no throw
    "(try (/ 1 0) (catch e (get e :kind)))",   // error path: handler invoked
    // binding (dynamic scope) — body thunk routes through apply_engine
    "(defdyn *dv* 10) (defn get-dv () *dv*) [(get-dv) (binding (*dv* 42) (get-dv)) (get-dv)]",
    // isolate — thunk routes through apply_engine; visible def is rolled back
    "(defn iso-work () (def _iso-x 99) _iso-x) (%isolate iso-work)",
    // apply unfolding in dispatch (ADR-096 follow-on): the VM now unfolds
    // `(apply f args)` inline, re-dispatching the real callee through the VM.
    // Both engines must agree on results and the O(1)-stack tail property.
    "(defn f (n) (if (= n 0) :done (apply f (list (- n 1))))) (f 5000)", // tail via apply
    "(apply + (list 1 2 3 4 5))",                                         // basic splice
    "(apply list 1 2 (list 3 4))",                                        // prefix + splice
    "(apply apply (list + (list 1 2 3)))",                                // nested apply
    "(defn g (a b) (+ a b)) (apply g (list 10 20))",                     // RUNTIME callee
    // bytecode stepping engine (ADR-100 Stage 1): call-free helper bodies lower to a
    // chunk and run on the bytecode loop when called. These exercise its node set —
    // arithmetic, if-nesting, let, vector/map build, first/rest, the fallback/error
    // paths, and the epoch-guard re-resolve after redefining an inlined operator.
    "(defn sq (x) (* x x)) (map sq (range 1 7))",
    "(defn classify (n) (if (< n 0) :neg (if (= n 0) :zero :pos))) (map classify (list -3 0 8))",
    "(defn pick (x) (let (a (* x 2) b (+ a 1)) [a b {:k a :v b}])) (pick 10)",
    "(defn hd (xs) (first xs)) (defn tl (xs) (rest xs)) [(hd [10 20 30]) (tl '(1 2 3)) (hd '())]",
    "(defn boom (x) (first x)) (boom 5)",          // Prim1 fallback → type error, both engines
    "(defn dz (a b) (/ a b)) (dz 1 0)",            // Prim2 fallback (Div) → div-by-zero
    "(defn add1 (x) (+ x 1)) (def + (fn (a b) (* a b))) (add1 5)", // redefine + → guard fallback
    "(defn cz (a b) (cons a b)) (cz 1 (list 2 3))",                // Prim2 Cons inline
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
