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
