//! The walk survives what a program can throw at it: pathologically deep forms, deep recursion shapes, depth-capped bodies.

use super::*;

#[test]
fn checker_survives_pathologically_deep_forms() {
    // Host-panic hardening: a deeply-nested-but-legal form —
    // the same class as the kernel's 60k-deep-value tests — must be
    // *checked*, not blow the checker's native stack. check_into grows the
    // stack in heap-backed segments (stacker), so this returns normally.
    let mut interp = crate::Interp::new();
    let identity = crate::core::value::intern("identity");
    let mut form = Value::Int(1);
    for _ in 0..30_000 {
        let tail = interp.heap.alloc_pair(form, Value::Nil);
        form = interp.heap.alloc_pair(Value::Sym(identity), tail);
    }
    // No assertion on the warning list's content — the property under test
    // is "returns instead of crashing the host".
    let _ = check_file(&mut interp.heap, &[form]);

    // A deep `(do (do … (%register-sig …)))` chain exercises the OTHER
    // recursive walker, `collect_register_sig_forms` (the one the initial
    // hardening pass missed): it must grow the stack too, not SIGSEGV.
    let do_sym = crate::core::value::intern("do");
    let mut deep_do = Value::Nil; // an empty innermost `(do)`
    for _ in 0..30_000 {
        let inner = interp.heap.alloc_pair(Value::Sym(do_sym), deep_do);
        let tail = interp.heap.alloc_pair(inner, Value::Nil);
        deep_do = tail;
    }
    let top = interp.heap.alloc_pair(Value::Sym(do_sym), deep_do);
    let _ = check_file(&mut interp.heap, &[top]);
}

// Four recursion shapes that run INSIDE `expr_ty`'s 1 MB stack segment (or beside it)
// and must grow the stack themselves. One test per shape so a
// regression names itself. The two chains `expr_ty` re-asks at every level are 8k
// deep (a chain of n is O(n²) there) — still several segments' worth of frames when
// the wrap is removed; the other two are the kernel's 30k.

#[test]
fn checker_survives_a_deep_access_path_chain() {
    // `guards::path_of` — asked at `expr_ty`'s FIRST level, for every `first`.
    let mut interp = crate::Interp::new();
    let x = Value::Sym(crate::core::value::intern("x"));
    let firsts = nest_form(&mut interp, "first", 8_000, x);
    let _ = check_file(&mut interp.heap, &[firsts]);
}

#[test]
fn checker_survives_a_deep_quoted_datum() {
    // `quoted_datum_ty` recurses into itself, never back through the depth cap.
    let mut interp = crate::Interp::new();
    let datum = nest_form(&mut interp, "list", 30_000, Value::Int(1));
    let quoted = reader::read_one(&mut interp.heap, "(def deep-datum nil)").unwrap();
    let quote_sym = crate::core::value::intern("quote");
    let qtail = interp.heap.alloc_pair(datum, Value::Nil);
    let qform = interp.heap.alloc_pair(Value::Sym(quote_sym), qtail);
    let def_items = items_of(&interp, quoted);
    let def_form = mk_list(&mut interp, &[def_items[0], def_items[1], qform]);
    let _ = check_file(&mut interp.heap, &[def_form]);
}

#[test]
fn checker_survives_a_deep_let_body() {
    // `sym_appears_in` (the unused-binding lint) recursed on the cdr too.
    let mut interp = crate::Interp::new();
    let x = Value::Sym(crate::core::value::intern("x"));
    let body = nest_form(&mut interp, "identity", 30_000, x);
    let let_form = reader::read_one(&mut interp.heap, "(let (x 1) nil)").unwrap();
    let let_items = items_of(&interp, let_form);
    let let_deep = mk_list(&mut interp, &[let_items[0], let_items[1], body]);
    let _ = check_file(&mut interp.heap, &[let_deep]);
}

#[test]
fn checker_survives_a_deep_negated_guard() {
    // `path_guard_assertion` on `(not (not …))`, from `check_if`.
    let mut interp = crate::Interp::new();
    let pred = reader::read_one(&mut interp.heap, "(int? (get r :age))").unwrap();
    let nots = nest_form(&mut interp, "not", 8_000, pred);
    let if_form = reader::read_one(&mut interp.heap, "(defn f (r) (if nil 1 2))").unwrap();
    let if_items = items_of(&interp, if_form);
    let inner = items_of(&interp, if_items[3]);
    let deep_if = mk_list(&mut interp, &[inner[0], nots, inner[2], inner[3]]);
    let deep_fn = mk_list(
        &mut interp,
        &[if_items[0], if_items[1], if_items[2], deep_if],
    );
    let _ = check_file(&mut interp.heap, &[deep_fn]);
}

#[test]
fn a_depth_capped_body_walk_is_not_memoized_as_the_callees_signature() {
    // `g`'s inferred signature must not be whatever `expr_ty` answered the FIRST time
    // anyone asked: asked from deep inside another walk, the depth cap trips inside `g`'s
    // body, `g` reads `(string -> any)`, and memoizing that hides the misuse below. A
    // callee's body is a fresh question (`infer::with_fresh_depth`).
    let mut interp = crate::Interp::new();
    // A body Tier-2 inference has to WALK (a single direct call would be read off the
    // callee's signature without `expr_ty`, and never meet the cap), with a rest
    // parameter so call-site specialization — which skips such arms — cannot re-derive
    // the return at the misuse and paper over a poisoned signature.
    interp
        .eval_str("(defn g (s & more) (if s (%string-length s) 0))")
        .expect("def g");
    let if_sym = Value::Sym(crate::core::value::intern("if"));
    let call = reader::read_one(&mut interp.heap, "(g \"a\")").unwrap();
    let mut forms = Vec::new();
    // `(if true X 1)` wrappers — control flow, which `expr_ty` descends into (a plain
    // call wrapper would not be asked from the inside). Every depth from 100 to 130, so
    // one of them lands `g`'s body walk exactly on the cap whatever the walker's own
    // frame accounting is.
    for depth in (100..=130).rev() {
        let mut form = call;
        for _ in 0..depth {
            let t3 = interp.heap.alloc_pair(Value::Int(1), Value::Nil);
            let t2 = interp.heap.alloc_pair(form, t3);
            let t1 = interp.heap.alloc_pair(Value::Bool(true), t2);
            form = interp.heap.alloc_pair(if_sym, t1);
        }
        // `(def dN <chain>)`: global value inference types every `def`'s right-hand side, which is
        // what carries the ask down to `(g "a")` — a bare top-level form is walked, not
        // typed from the outside in.
        let name = Value::Sym(crate::core::value::intern(&format!("d{depth}")));
        let t2 = interp.heap.alloc_pair(form, Value::Nil);
        let t1 = interp.heap.alloc_pair(name, t2);
        forms.push(
            interp
                .heap
                .alloc_pair(Value::Sym(crate::core::value::intern("def")), t1),
        );
    }
    let misuse =
        reader::read_one(&mut interp.heap, "(defn h () (%string-length (g \"a\")))").unwrap();
    forms.push(misuse);
    let ws: Vec<String> = check_file(&mut interp.heap, &forms)
        .into_iter()
        .map(|(_, m)| m)
        .collect();
    assert!(
        ws.iter()
            .any(|w| w.contains("%string-length") && w.contains("int")),
        "g's int return must survive a depth-capped first ask: {ws:?}"
    );
}

/// Malformed forms, one per line: a form missing its body, its bindings, its clauses or
/// its type. The compile pass reports each; the checker must walk past it. A bare
/// `(let)`, `(letrec)`, `(match v ())` and a top-level `(check-allow)` each indexed past
/// the end of a slice, and the advisory pass died with "checker internal error".
const MALFORMED_FORMS: &str = r#"(match v ((or) 1))
(match v ((and) 1))
(case v)
(case)
(case v ())
(cond)
(cond ())
(cond (true))
(let)
(let ())
(match v)
(let (a))
(let ([a b]) a)
(letrec)
(letrec (f))
(fn)
(fn ())
(fn (()) 1)
(fn ((a b)) a)
(fn (a:) a)
(fn (-> int) 1)
(match)
(fn (a:int ->) a)
(fn (& ) 1)
(fn (&optional) 1)
(if)
(if v)
(for)
(for ())
(for (x) x)
(for (x xs :when) x)
(fold-for)
(match v ())
(fold-for (acc) acc)
(when-let)
(when-let ())
(if-let (a))
(try)
(try 1 (catch))
(try 1 (catch e))
(receive)
(receive ())
(with)
(match v (nil))
(with ())
(ok->)
(-> )
(->> v)
(doseq)
(doseq (x))
(quote)
(quasiquote)
(unquote v)
(def)
(match v (_ :when))
(sig)
(sig f)
(sig f ())
(sig f (->))
(sig f (int ->))
(check-allow)
(check-allow :type-mismatch)
(map)
(map xs)
(fold xs)
(match v (_ :when true))
(apply)
(comp)
(defrecord)
(%typed-fn)
(%typed-fn 'x)
(%typed-fn '(int -> int) 3)
(%typed-fn 'nil (fn (a) a))
(string/interp)
(string/interp "{")
(string/interp "{}")
(match v ([a b]))
(seq/sort-by xs)
(assoc)
(get)
(nth)
(nth xs)
(match v ((tuple) 1))
(match v ([& rest] rest))
(match v ([a &] a))
(match v (^ 1))
(match v ('  1))
(match v ((record) 1))
(defn)
(defn g)
(defn g ())
(defn g "doc")
(defn- g)
(defmacro)
(defmacro m)
(defmodule)
(fn (a b) )
(fn ((a)) a)
(fn [a] a)
(let [a 1] a)
(let (1 2) 3)
(let ("a" 2) 3)
(letrec ((f)) 1)
(letrec (1 2) 3)
(match v (1))
(match v ([1 2] ) )
(match v ((or 1) ) )
(case v 1)
(cond true)
(if 1 2 3 4)
(do)
(def 1 2)
(def x)
(fn (& a b) a)
(fn (&optional (a)) a)
(fn (&optional (a 1 2)) a)
(sig 1 (int -> int))
(defrecord r)
(defrecord r ())
(defrecord r (a:))
(for (x xs y) x)
(for (x xs :let) x)
(for (x xs :let (y)) y)
(fold-for (acc 0 x) acc)
(receive (1))
(receive (:after))
(receive (:after 10))
(with (a) a)
(with (a 1) )
(ok-> 1 ())
(-> 1 ())
(-> 1 (1 2))
(cond-> 1 true)
(as-> 1)
(as-> 1 x)
(comment)
(check-allow :nope 1)
(%lint-allow)
(%lint-allow :x)
(%register-sig)
(%register-sig 'f)
(%register-sig 'f '(int))
(%typed-fn '(int -> int))
(%typed-fn '(-> int) (fn () 1))
(%typed-fn '(int int -> int) (fn (a) a))
(%typed-fn '(int -> int) (fn (a b) a))
(%typed-fn '(int -> int) (fn))
(%typed-fn '(int -> int) (fn ((a) 1) ((a b) 2)))
(map xs (fn))
(map xs (fn ()))
(fold xs 0 (fn))
(fold xs 0 (fn (a)))
(seq/filter xs (fn))
(apply (fn))
(comp (fn))
(string/interp "{(}")
(string/interp "{)}")
(quote 1 2)
(get-in)
(assoc v)
(assoc v :a)
(nth xs 1 2 3)"#;

#[test]
fn the_checker_walks_past_every_malformed_form() {
    // Each context puts the form where a different pass reaches it: a body, the top
    // level (the file lints), a helper typed from its caller, a `def`, a `let`-bound
    // lambda, a combinator's callback and a `letrec` loop's base case.
    let contexts = [
        "(defn f (v xs) FORM)",
        "FORM",
        "(defn- helper (v xs) FORM) (defn caller () (helper 1 [2]))",
        "(def x FORM)",
        "(defn f (v xs) (let (g (fn (a) FORM)) (g 1)))",
        "(defn f (v xs) (map xs (fn (item) FORM)))",
        "(defn f (v xs) (letrec (go (fn (n) (if (= n 0) FORM (go (dec n))))) (go 3)))",
    ];
    let internal = |src: &str| {
        file_warnings(src)
            .into_iter()
            .find(|w| w.starts_with("checker internal error"))
    };
    for context in contexts {
        let file: String = MALFORMED_FORMS
            .lines()
            .map(|form| context.replace("FORM", form) + "\n")
            .collect();
        if internal(&file).is_none() {
            continue;
        }
        // One file aborts at its first panic; name every form that panics.
        let culprits: Vec<String> = MALFORMED_FORMS
            .lines()
            .filter_map(|form| {
                let src = context.replace("FORM", form);
                internal(&src).map(|error| format!("{src}\n    {error}"))
            })
            .collect();
        panic!("the checker died on:\n{}", culprits.join("\n"));
    }
}

#[test]
fn a_caller_derived_recursive_parameter_does_not_hang_the_check() {
    // `object`'s parameter is derived from its callers: `data`'s JSON-ish return, and a
    // reply whose error records carry that return as `data`. The derivation folds the two
    // into `μX. … | {error: … | {code: 2, data: X}}`, and asking `(map? value)` of it
    // never returned — the relation re-unrolled the type to canonicalise its own
    // unrolling. Whole-file, as `nest check` runs it; asserted on the unrolling budget
    // (which bounds the relation even then) never being reached, not on time.
    let before = crate::types::rec_budget_exhaustions();
    let ws = file_warnings(
        r#"
(sig data (any -> (or string (map string any))))
(defn data (v) v)
(defn reply (line)
  (cond (= line "a") {:error {:code 1 :data (str line)}}
    (= line "b") {:error {:code line}}
    else {:error {:code 2 :data (data line)}}))
(defn object (value) (if (map? value) value (error "no")))
(defn use-data (x) (object (data x)))
(defn use-reply (x) (object (reply x)))
"#,
    );
    assert!(
        ws.iter().all(|w| !w.starts_with("checker gave up:")),
        "{ws:?}"
    );
    let exhausted = crate::types::rec_budget_exhaustions() - before;
    assert_eq!(
        exhausted, 0,
        "the unrolling budget cut {exhausted} relations short"
    );
}

/// KI-416: the joint ascent of returns and caller-derived parameters must only rise. This
/// file's `jv` re-typed as `vector` one round and `vector<(tuple any)>` the next, for good:
/// no fixpoint in `MAX_JOINT_ROUNDS`, and the checker gave up on the whole file (b3d-mass,
/// where the same shape is a `mapv` of `mapv`s read back with `nth` of `nth`).
#[test]
fn a_return_that_does_not_retype_monotonically_still_settles() {
    let ws = file_warnings(
        r#"
(defn- jv (data) (seq/mapv [7 8] (fn (index) [index])))
(defn- influences (joints vertex) (nth (nth joints vertex) 0))
(defn top (primitive) (influences (jv (get primitive :joints)) 0))
"#,
    );
    assert!(
        ws.iter().all(|w| !w.starts_with("checker gave up:")),
        "{ws:?}"
    );
}
