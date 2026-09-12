//! Declared-vs-curated precision (2026-08-28): a declaration beats the curated table, and the std corpus stays warning-free under both.

use super::*;

/// A declared `(sig …)` in `std/` must never be **less precise** than the curated
/// signature it shadows.
///
/// A declaration is authoritative — `annot.rs` reads it ahead of primitive / curated /
/// inferred — so widening one silently switches off every finding that rested on the
/// curated version. Nothing warns and nothing fails: during the ADR-276/277 adoption
/// rounds `string/capitalize` shipped as `(string -> any)` over a curated
/// `(string -> string)`, which disabled the `(+ 1 (string/capitalize "x"))` finding.
/// `type_check_catalog` caught that one only because that exact expression happened to be
/// in the catalog — a shadowed name nobody had written an example for would have gone
/// unchecked in silence. This gate is structural instead: it needs no example per name.
///
/// **Returns only.** A declaration that *narrows* a parameter (`math/even?` declares
/// `int` over a curated `number`) is a tightening, not a loss — it can only produce a
/// warning the curated sig would have missed, and `nest check`'s zero-warning gate over
/// `std/` + `tests/` is where that surfaces. Widening a *return* is the direction that
/// loses checking silently, so that is the direction asserted.
#[test]
fn no_declared_std_sig_widens_its_curated_signature() {
    let interp = crate::Interp::new();
    let std_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../std");

    let mut files = Vec::new();
    blsp_files(&std_root, &mut files);
    files.sort();
    assert!(
        files.len() > 40,
        "only {} .blsp files found under {} — the walk is broken, so a green result here \
         would mean nothing",
        files.len(),
        std_root.display()
    );

    let mut compared = 0usize;
    let mut violations: Vec<String> = Vec::new();

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let module = declared_module_name(&src);
        let mut heap = crate::core::heap::Heap::with_regions(
            interp.heap.prelude_arc(),
            interp.heap.runtime_arc(),
        );
        heap.set_global(crate::core::value::EnvId::GLOBAL);
        let Ok(forms) = crate::syntax::reader::read_all(&mut heap, &src) else {
            continue;
        };

        for form in &forms {
            let Some((name, declared)) = super::annot::parse_sig_decl(&heap, *form) else {
                continue;
            };
            let bare = crate::core::value::symbol_name(name);
            let qualified = match &module {
                Some(m) if !bare.contains('/') => format!("{m}/{bare}"),
                _ => bare.clone(),
            };
            let Some(curated) = super::sigs::curated_sig(crate::core::value::intern(&qualified))
            else {
                continue;
            };
            compared += 1;
            if !declared.ret.is_subtype(&curated.ret) {
                violations.push(format!(
                    "  {qualified} ({}): declared return `{}` is WIDER than the curated `{}`",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    declared.ret,
                    curated.ret
                ));
            }
        }
    }

    // The gate must not be satisfiable by doing nothing (docs/handoff.md: "a gate whose
    // pass condition can be satisfied by doing nothing is worse than no gate, because it
    // is believed"). `curated_sig` holds ~35 names and `std/` declares a sig for ten of
    // them; if qualification or the sig walk breaks, `compared` collapses to 0 and this
    // fires instead of reporting success.
    assert!(
        compared >= 8,
        "only {compared} declared/curated collisions were inspected — the sig walk or the \
         module qualification broke, and a green result here would mean nothing"
    );
    assert!(
        violations.is_empty(),
        "{} declared std sig(s) are less precise than the curated signature they shadow \
         (this silently disables lints — see the doc comment):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

#[test]
fn an_arrow_parameter_has_an_exact_arity() {
    // ADR-273 made an arrow parameter describe the call's TYPES. Its shape was still
    // unchecked, which is half a contract — and the arity half is the certain one: the
    // caller had to supply a one-argument function to satisfy `(int -> string)`, so
    // calling it with two arguments always raises.
    let w = file_warnings(
        r#"
        (sig apply-it ((int -> string) -> any))
        (defn apply-it (f) (f 1 2))
        "#,
    );
    assert!(
        w.iter().any(|s| s.contains("expected 1 argument, got 2")),
        "too many arguments through an arrow parameter: {w:?}"
    );

    let w = file_warnings(
        r#"
        (sig apply-it ((int -> string) -> any))
        (defn apply-it (f) (f))
        "#,
    );
    assert!(
        w.iter().any(|s| s.contains("expected 1 argument, got 0")),
        "too few arguments through an arrow parameter: {w:?}"
    );

    // `&optional` widens to a range and `&` to unbounded — the same mapping a declared
    // sig gets, because it is the same question asked of the same shape.
    for src in [
        "(sig f ((int -> string) -> any))\n(defn f (g) (g 1))",
        "(sig f ((int &optional int -> string) -> any))\n(defn f (g) (g 1 2))",
        "(sig f ((int &optional int -> string) -> any))\n(defn f (g) (g 1))",
        "(sig f ((int & int -> string) -> any))\n(defn f (g) (g 1 2 3))",
        // A bare `fn` carries no arrow, so it says nothing about arity.
        "(sig f (fn -> any))\n(defn f (g) (g 1 2 3))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter()
                .all(|s| !s.contains(": expected") || !s.contains(", got")),
            "correct call flagged for `{src}`: {w:?}"
        );
    }
}

#[test]
fn a_structural_complement_is_sayable_and_checks() {
    // ADR-263 made `(not T)` sayable; ADR-268 made it exact for literals. A STRUCTURAL
    // `T` still widened to its tag — `¬(vector int)` was `any`, so the annotation parsed
    // and then checked nothing. ADR-288 makes it exact, so it finally rejects.
    let w = file_warnings(
        r#"
        (sig not-int-vec ((not (vector int)) -> any))
        (defn not-int-vec (v) v)
        (defn wrong () (not-int-vec [1 2 3]))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("not-int-vec") && s.contains("argument 1")),
        "a vector of ints is not in `(not (vector int))`: {w:?}"
    );

    // …and a value that genuinely is in the complement passes.
    let w = file_warnings(
        r#"
        (sig not-int-vec ((not (vector int)) -> any))
        (defn not-int-vec (v) v)
        (defn fine () (not-int-vec "hello"))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("not-int-vec")),
        "a string IS in `(not (vector int))`: {w:?}"
    );

    // The same for a tuple shape, intersected with `any` — the `(and …)` spelling.
    let w = file_warnings(
        r#"
        (sig no-pair ((and any (not (tuple int int))) -> any))
        (defn no-pair (t) t)
        (defn wrong () (no-pair [1 2]))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("no-pair") && s.contains("argument 1")),
        "an int pair is not in `(not (tuple int int))`: {w:?}"
    );
}

// `list<A> ∩ list<B>` is empty when `A` and `B` are disjoint — an argument list whose
// elements can never be strings is a bug, not "consistent" with `list<string>`. The
// empty list is `nil` in Brood, not a pair, so `list<T>` is the NON-empty list and the
// intersection of two non-empty lists over disjoint elements is genuinely uninhabited.
#[test]
fn lists_over_disjoint_element_types_do_not_intersect() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig want-strs ((list string) -> int))\n\
         (defn want-strs (xs) 0)\n\
         (defn call () (want-strs (list [:a 1])))",
    );
    assert!(ws.iter().any(|w| w.contains("want-strs")), "{ws:?}");
    // …while a list that MAY hold strings stays consistent (gradual: overlap suffices).
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig want-strs ((list string) -> int))\n\
         (defn want-strs (xs) 0)\n\
         (defn call (x) (want-strs (list x)))",
    );
    assert!(ws.is_empty(), "{ws:?}");
}

// A type variable inside `or`/`and`: `(or ?A nil)` binds `?A` to the argument MINUS the
// concrete alternatives, so the result type is the argument with `nil` carved off.
#[test]
fn a_type_variable_inside_or_binds_to_the_rest_of_the_argument() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig or-default ((or ?A nil) ?A -> ?A))\n\
         (defn or-default (x d) (if (nil? x) d x))\n\
         (sig g (int -> string))\n\
         (defn g (n) (or-default n 1))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("declared return type string") && w.contains("int")),
        "{ws:?}"
    );
}

// A value that can be a `failure`, reaching a position that cannot take one, is reported
// in BOTH modes — the converse of ADR-315's impossible-`failure?` lint. That one names a
// guard that cannot fire; this one names a failure nothing guards, which is the direction
// deferred.md recorded as needing an effect system. It does not: the union already says
// which functions can fail, so what was missing was the reporting rule.
#[test]
fn an_unguarded_failure_is_reported_in_both_modes() {
    let producer = "(defmodule t)\n(sig p (string -> (or string failure)))\n(defn p (s) s)\n";
    let flows_into_a_native = format!("{producer}(defn q (s) (string/length (p s)))");
    for strict in [false, true] {
        let ws = file_warnings_mode(&flows_into_a_native, strict);
        assert!(
            ws.iter()
                .any(|w| w.contains("expects string, got string | failure")),
            "strict={strict}: {ws:?}"
        );
    }
    // …a user function's declared domain, and a declared return, the same way
    let into_a_defn =
        format!("{producer}(sig r (string -> int))\n(defn r (s) 1)\n(defn q (s) (r (p s)))");
    assert!(
        file_warnings_mode(&into_a_defn, false)
            .iter()
            .any(|w| w.contains("string | failure")),
        "a user function's domain excludes a failure too"
    );
    let declared_return = format!("{producer}(sig q (string -> string))\n(defn q (s) (p s))");
    assert!(
        file_warnings_mode(&declared_return, false)
            .iter()
            .any(|w| w.contains("declared return type string") && w.contains("failure")),
        "a declared return excludes a failure too"
    );
}

// A guard that DIVERGES narrows everything after it. Brood has no early return — no
// `return`, no `guard` — so "refuse and stop" is spelled `(when bad (error …))`, and it is
// everywhere. Without this rule the value stays as wide as it was declared, and the only way
// to silence the callers is to declare the producer `-> any`, which is how a whole family of
// std signatures came to say nothing.
#[test]
fn a_diverging_guard_narrows_the_rest_of_the_body() {
    let base = "(defmodule t)\n(sig p (string -> (or nil string)))\n(defn p (s) s)\n\
                (sig q (string -> int))\n(defn q (s) 1)\n";
    // `when` — the diverging arm is the THEN, so the sequel gets the else-scope
    let guarded =
        format!("{base}(defn f (s) (let (r (p s)) (when (nil? r) (error \"no\")) (q r)))");
    assert!(file_warnings_mode(&guarded, true).is_empty(), "{guarded}");
    // `unless` — the diverging arm is the ELSE, so the sequel gets the then-scope
    let unless = format!("{base}(defn f (s) (let (r (p s)) (unless r (error \"no\")) (q r)))");
    assert!(file_warnings_mode(&unless, true).is_empty(), "{unless}");
    // …and it reaches the INFERRED RETURN, not just the walk: a function that guards and
    // then answers the value must advertise the narrowed type, or its callers are reported
    // instead of it.
    let via_return = format!(
        "{base}(defn root (s) (let (r (p s)) (when (nil? r) (error \"no\")) r))\n\
         (defn use-it (s) (q (root s)))"
    );
    assert!(
        file_warnings_mode(&via_return, true).is_empty(),
        "{via_return}"
    );

    // What must STILL warn, or the rule is just a hole: a guard that does not diverge
    // proves nothing about the sequel.
    let non_diverging = format!("{base}(defn f (s) (let (r (p s)) (when (nil? r) 0) (q r)))");
    assert!(
        file_warnings_mode(&non_diverging, true)
            .iter()
            .any(|w| w.contains("nil | string")),
        "a `when` that falls through proves nothing: {non_diverging}"
    );
}

// A guard narrows THROUGH a deterministic parser, so re-evaluating it in the branch is the
// value the guard just tested. Without this, `(if (failure? (parse s)) d (parse s))` — the
// spelling that does not bind — inferred `… | failure`, and ADR-316 then reported that arm
// at every call site: a false positive on a function that provably cannot fail, which is
// the one class this checker is not allowed to have.
#[test]
fn a_guard_narrows_through_a_deterministic_parser() {
    let calc = "(defmodule t)\n\
                (defn calc (expr) (if (failure? (string/->number expr)) 0 (string/->number expr)))\n";
    // the value flows out and is used as a number — silent, because it cannot be a failure
    let used = format!("{calc}(defn use-it (s) (+ 1 (calc s)))");
    assert!(file_warnings_mode(&used, false).is_empty(), "{used}");
    // …and a `number` return may be declared over it
    let declared = "(defmodule t)\n(sig calc (string -> number))\n\
         (defn calc (expr) (if (failure? (string/->number expr)) 0 (string/->number expr)))";
    assert!(file_warnings_mode(declared, false).is_empty(), "{declared}");

    // What must NOT be narrowed away, or the fix would just be a hole in the lint:
    // an unguarded parse still reports…
    let unguarded = "(defmodule t)\n(defn calc (expr) (+ 1 (string/->number expr)))";
    assert!(
        file_warnings_mode(unguarded, false)
            .iter()
            .any(|w| w.contains("number | failure")),
        "an unguarded parse must still report"
    );
    // …and a guard on a DIFFERENT argument narrows nothing, since the path is keyed by its
    // base symbol — `(parse a)` and `(parse b)` are two paths, not one.
    let other_arg = "(defmodule t)\n\
         (defn calc (a b) (if (failure? (string/->number a)) 0 (+ 1 (string/->number b))))";
    assert!(
        file_warnings_mode(other_arg, false)
            .iter()
            .any(|w| w.contains("number | failure")),
        "a guard on another argument must not narrow this one"
    );
}

// The three shapes that must stay SILENT, which is what keeps the rule from being a
// strictness change in disguise.
#[test]
fn a_handled_or_merely_unknown_failure_is_not_reported() {
    let producer = "(defmodule t)\n(sig p (string -> (or string failure)))\n(defn p (s) s)\n";
    // 1. guarded with `failure?` — the whole point of the value being one
    let guarded =
        format!("{producer}(defn q (s) (let (v (p s)) (if (failure? v) 0 (string/length v))))");
    assert!(file_warnings_mode(&guarded, false).is_empty(), "{guarded}");
    // 2. a position that ACCEPTS a failure: `failure?` and `error-message` take any, and
    //    `=` compares anything — a test asserting a failure came back must stay silent
    let accepted =
        format!("{producer}(defn q (s) (failure? (p s)))\n(defn r (s) (error-message (p s)))");
    assert!(
        file_warnings_mode(&accepted, false).is_empty(),
        "{accepted}"
    );
    // 3. the language's OWN failure mechanisms (ADR-315). A lint that fired on `ok->` and
    //    `with` would be fighting the two idioms the answer to it is written in.
    let mechanisms = format!(
        "{producer}(defn q (s) (ok-> s (p) (string/length)))\n\
         (defn r (s) (with (v (p s)) (string/length v)))"
    );
    assert!(
        file_warnings_mode(&mechanisms, false).is_empty(),
        "{mechanisms}"
    );
    // 4. a position that carries a failure rather than consuming it — `=`, a collection it
    //    is stored in, a message, `str`, and simply returning it. ADR-315 left storage
    //    deliberately silent: what it added was somewhere to SAY stop, not a guess.
    let carried = format!(
        "{producer}(defn q (s) (= (p s) \"x\"))\n(defn r (s) (conj (list) (p s)))\n\
         (defn u (s) [(p s)])\n(defn v (s) (str (p s)))\n(defn w (s) (p s))"
    );
    assert!(file_warnings_mode(&carried, false).is_empty(), "{carried}");
    // 5. an UNANNOTATED value. `any` admits a failure the way it admits everything; a
    //    bound known only by exclusion says nothing positive, so neither may be read as
    //    "this can fail" — that would fire on every unannotated parameter in the language.
    let unknown = "(defmodule t)\n(defn q (x) (string/length x))\n                   (defn r (x) (when x (string/length x)))";
    assert!(file_warnings_mode(unknown, false).is_empty(), "{unknown}");
}

// Strict mode (`nest check --strict`): a dynamic value with a precise bound is checked by
// inclusion. `(h x)` is `dynamic(number)` — consistent with `int` by overlap (the default,
// reload-safe reading), rejected strictly. A bare `dynamic()` (`any`) stays consistent in
// both modes: strictness sharpens what is known, it never invents a verdict.
#[test]
fn strict_mode_checks_a_precise_dynamic_bound_by_inclusion() {
    let src = "\
         (defmodule t)\n\
         (sig f (int -> int))\n\
         (defn f (x) x)\n\
         (sig h (int -> number))\n\
         (defn h (x) x)\n\
         (defn g (x) (f (h x)))\n\
         (defn k (y) (f (undeclared-thing y)))";
    let lax = file_warnings_mode(src, false);
    assert!(lax.iter().all(|w| !w.contains("argument 1")), "{lax:?}");
    let strict = file_warnings_mode(src, true);
    assert!(
        strict
            .iter()
            .any(|w| w.contains("t/f: argument 1 expects int, got number")),
        "{strict:?}"
    );
    assert_eq!(
        strict.iter().filter(|w| w.contains("argument 1")).count(),
        1,
        "{strict:?}"
    );
}

// Sets carry an element type, exactly as vectors and lists do (`set<E>`): the literal,
// `conj`/`into` onto one, and `(set T)` in a signature — including one with a type
// variable inside. `#{}` is `set<never>`, which every `set<T>` admits.
#[test]
fn sets_carry_their_element_type() {
    assert_eq!(ty_str("#{1 2}"), "set<1 | 2>");
    assert_eq!(ty_str("#{}"), "set<never>");
    assert_eq!(ty_str("(conj #{1} :a)"), "set<:a | 1>");
    assert_eq!(ty_str("(into #{1} (list :a))"), "set<:a | 1>");
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig want-ints ((set int) -> int))\n\
         (defn want-ints (xs) 0)\n\
         (defn ok () (want-ints #{}))\n\
         (defn bad () (want-ints #{:a}))",
    );
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(
        ws[0].contains("want-ints") && ws[0].contains("set<int>"),
        "{ws:?}"
    );
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (sig pick ((set ?A) -> ?A))\n\
         (defn pick (xs) (first xs))\n\
         (sig g (int -> string))\n\
         (defn g (n) (pick #{1 2}))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("declared return type string") && w.contains("1 | 2")),
        "{ws:?}"
    );
}

// An operator's domain is what the multimethod registry covers (ADR-299): a number, plus
// exactly the records `num/*` (or `compare-to`) have methods for. With none loaded, `+`
// accepts `number` and nothing else — no more `number | map`, which read as "a map can be
// added" when it meant "a record with a `num/add` method can".
#[test]
fn an_operator_accepts_a_number_or_exactly_the_records_its_multimethod_covers() {
    let ws = file_warnings("(defmodule t)\n(sig g (string -> int))\n(defn g (s) (+ 1 s))");
    assert!(
        ws.iter()
            .any(|w| w.contains("+: argument 2 expects number, got string")),
        "{ws:?}"
    );
    let ws = file_warnings("(defmodule t)\n(sig g (string -> bool))\n(defn g (s) (< 1 s))");
    assert!(
        ws.iter()
            .any(|w| w.contains("<: argument 2 expects number, got string")),
        "{ws:?}"
    );
    // …and a record with a `num/add` method is in the domain, BY NAME.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord usd (cents))\n\
         (defmethod num/add [usd usd] (a b) (usd (+ (get a :cents) (get b :cents))))\n\
         (sig g (string -> int))\n\
         (defn g (s) (+ (usd 1) s))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("+: argument 2 expects number | t/usd, got string")),
        "{ws:?}"
    );
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord usd (cents))\n\
         (defmethod num/add [usd usd] (a b) (usd (+ (get a :cents) (get b :cents))))\n\
         (defn total (a b) (+ a b))",
    );
    assert!(ws.is_empty(), "{ws:?}");
}

// The sig spelling of a domain that names records (ADR-299): a nominal shape is spelled by
// its id — the name a `sig` takes — and the numeric half as `number`, one flat `(or …)`; a
// set carries its element type in that spelling too. This is what `--suggest-sigs` prints,
// so it must be something a reader would paste.
#[test]
fn a_record_domain_is_spelled_by_name_in_a_suggested_sig() {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        value::intern("__id__"),
        (Ty::keyword_lit(value::intern("t/usd")), true),
    );
    let usd = Ty::record_of_open(fields);
    assert_eq!(usd.to_source().as_deref(), Some("t/usd"));
    // an inferred field refinement is dropped from the spelling — the name is the sig
    let mut refined = std::collections::BTreeMap::new();
    refined.insert(
        value::intern("__id__"),
        (Ty::keyword_lit(value::intern("t/usd")), true),
    );
    refined.insert(value::intern("cents"), (Ty::of(Tag::Int), true));
    assert_eq!(
        Ty::record_of_open(refined).to_source().as_deref(),
        Some("t/usd")
    );
    assert_eq!(
        Ty::NUMBER.union(usd).to_source().as_deref(),
        Some("(or number t/usd)")
    );
    assert_eq!(
        Ty::set_of(Ty::of(Tag::Int)).to_source().as_deref(),
        Some("(set int)")
    );
}

// …and the spelling round-trips: the suggested `(or number t/usd)` is accepted back as a
// declaration, and means the same thing (a string is rejected, a `usd` is not).
#[test]
fn a_suggested_record_domain_parses_back_as_a_sig() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord usd (cents))\n\
         (sig f ((or number t/usd) -> int))\n\
         (defn f (x) 0)\n\
         (defn ok () (f (usd 1)))\n\
         (defn bad () (f \"s\"))",
    );
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(
        ws[0].contains("t/f: argument 1 expects number | t/usd, got \"s\""),
        "{ws:?}"
    );
}

// `countable` is a name, not a six-way union: `count`'s parameter is spelled by it in a
// suggested sig, and a `sig` accepts it back.
#[test]
fn countable_is_spelled_by_name() {
    assert_eq!(Ty::COUNTABLE.to_source().as_deref(), Some("countable"));
    assert_eq!(Ty::COUNTABLE.to_string(), "countable");
    let ws = file_warnings(
        "(defmodule t)\n(sig n (countable -> int))\n(defn n (xs) (count xs))\n(defn bad () (n 5))",
    );
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(ws[0].contains("expects countable, got 5"), "{ws:?}");
}

// `%max`/`%min` (behind `math/max`/`math/min`) route records through `compare-to` exactly as
// `<` does, so they take the same registry-derived domain — and return it, since the result
// is one of the operands. No more `(-> (or map number))` on a function that picks a max.
#[test]
fn max_and_min_take_and_return_the_ordered_domain() {
    // An extremum hands back one of its operands, so literal operands keep their literal
    // set — narrower than the `ordered` domain the sig declares, and still `⊆ number`.
    assert_eq!(ty_str("(%max 1 2)"), "1 | 2");
    assert_eq!(ty_str("(math/min 3 4)"), "3 | 4");
    assert_eq!(ty_str("(math/min 3 4 5)"), "3 | 4 | 5");
    // An untyped operand makes the result the UNKNOWN — `3 ∪ ?` — not the declared domain:
    // the value IS one of the operands, and an unknown operand is unknown wherever it goes
    // (arithmetic differs: `(dec x)` is a new value, positively `number`).
    assert_eq!(ty_str("(math/min 3 x)"), "any");
    let ws = file_warnings("(defmodule t)\n(sig g (string -> int))\n(defn g (s) (%max 1 s))");
    assert!(
        ws.iter()
            .any(|w| w.contains("%max: argument 2 expects number, got string")),
        "{ws:?}"
    );
}

// The named covers (ADR-299): `ordered` is `number` plus every record `compare-to` covers,
// `numeric` the same over `num/*`. A `sig` can write them; a suggestion prints them (never
// the list, which goes stale as the registry grows); a diagnostic prints the name once two
// or more records are in the cover, and the explicit `number | t/usd` while it is one.
#[test]
fn ordered_and_numeric_are_named_covers() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord date (d))\n\
         (defrecord time (t))\n\
         (defmethod compare-to [date date] (a b) 0)\n\
         (defmethod compare-to [time time] (a b) 0)\n\
         (sig before? (ordered ordered -> bool))\n\
         (defn before? (a b) (< a b))\n\
         (defn bad () (before? \"x\" 1))",
    );
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert!(
        ws[0].contains("t/before?: argument 1 expects ordered, got \"x\""),
        "{ws:?}"
    );
    // with a single record in the cover the diagnostic stays explicit
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord date (d))\n\
         (defmethod compare-to [date date] (a b) 0)\n\
         (defn bad () (< \"x\" 1))",
    );
    assert!(
        ws.iter().any(|w| w.contains("expects number | t/date")),
        "{ws:?}"
    );
}

// Strict mode reads a bound by inclusion only when the bound is POSITIVELY known. The
// `(not nil)` a `when` guard leaves on an untyped parameter says nothing about what the
// value is, so it stays consistent by overlap — else every guarded use of every untyped
// parameter in std would warn.
#[test]
fn strict_mode_keeps_the_overlap_reading_for_a_bound_that_is_only_a_subtraction() {
    let src = "\
         (defmodule t)\n\
         (defn f (xs) (when xs (first xs)))\n\
         (defn g (x) (first (or x [])))";
    let strict = file_warnings_mode(src, true);
    assert!(
        strict.iter().all(|w| !w.contains("argument 1")),
        "{strict:?}"
    );
}

// The truthy half of `(or x default)` — `any` less `nil` and `false` — is a guard's
// leftover, not a 21-tag union: it renders as `(not (nil | false))`.
#[test]
fn a_guards_truthy_leftover_renders_as_a_negation() {
    assert_eq!(Ty::truthy().to_string(), "(not (nil | false))");
    assert_eq!(
        Ty::ANY.difference(Ty::of(Tag::Nil)).to_string(),
        "(not nil)"
    );
    assert!(Ty::truthy().is_known_only_by_exclusion());
    assert!(!Ty::of(Tag::Str).is_known_only_by_exclusion());
}

// A `& rest` function's fixed parameters bind positionally like any other's, so they keep
// their demands; the rest binder's demand becomes a per-argument one. And a known callback
// hands its demands to `fold`/`reduce`'s init and collection. Together:
// `(defn foo (x y & more) (+ (fold more x +) y))` is `(number number & number -> number)`.
#[test]
fn a_rest_function_keeps_its_positional_demands_and_fold_hands_down_the_callbacks() {
    let mut interp = crate::Interp::new();
    let form = reader::read_one(&mut interp.heap, "(fn (x y & more) (+ (fold more x +) y))")
        .expect("parse");
    let demands =
        super::sigs::infer_params_from_form(&interp.heap, form, &Ctx::default()).expect("demands");
    let params: Vec<String> = demands.params.iter().map(Ty::to_string).collect();
    assert_eq!(params, vec!["number", "number"]);
    assert_eq!(
        demands.rest.map(|t| t.to_string()).as_deref(),
        Some("number")
    );
    // a call site checks each rest argument against that element demand
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defn foo (x y & more) (+ (fold more x +) y))\n\
         (defn bad () (foo 1 2 3 \"four\"))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("t/foo: argument 4 expects number, got \"four\"")),
        "{ws:?}"
    );
}

// The one length fact the lattice states — `list<T>` is non-empty (the empty list is `nil`)
// — carried through every combinator that preserves it. No `nil |` on the result of
// `append`/`map`/`sort`/`reverse`/`distinct`/`into` over a non-empty list, no `nil |` on its
// `first`/`last`, a literal `range` that cannot be empty; everything that CAN empty a
// sequence (`filter`, `rest`, `nth`, a vector input) keeps its `nil`.
#[test]
fn length_preserving_combinators_over_a_non_empty_list_drop_the_nil() {
    assert_eq!(
        ty_str("(append '(1 2 \"foo\") '(1 \"bar\"))"),
        "list<1 | 2 | string>"
    );
    assert_eq!(ty_str("(append '(1) nil)"), "list<1>");
    assert_eq!(ty_str("(map '(1 2) inc)"), "list<int>");
    assert_eq!(ty_str("(sort '(3 1))"), "list<1 | 3>");
    assert_eq!(ty_str("(reverse '(1 2))"), "list<1 | 2>");
    assert_eq!(ty_str("(first '(1 2))"), "1 | 2");
    assert_eq!(ty_str("(last '(1 2))"), "1 | 2");
    assert_eq!(ty_str("(range 5)"), "list<int>");
    assert_eq!(ty_str("(into '(1) '(2))"), "list<1 | 2>");
    // …and what may be empty keeps the nil
    assert!(ty_str("(filter '(1 2) even?)").starts_with("nil | "));
    assert!(ty_str("(rest '(1 2))").starts_with("nil | "));
    assert!(ty_str("(nth '(1 2) 5)").contains("nil"));
    assert!(ty_str("(first [1 2])").contains("nil") || ty_str("(first [1 2])") == "1");
    assert!(ty_str("(map [] inc)").starts_with("nil"));
}
