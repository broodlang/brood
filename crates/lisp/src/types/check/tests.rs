use super::*;
// The submodules' items are still accessed by name in these tests —
// import them explicitly now that they're not all in this file.
use super::sigs::primitive_sig;
use crate::core::value::Tag;
use crate::syntax::reader;
use crate::types::Ty;

/// A full `Interp` — primitives + the loaded prelude. We need the prelude
/// in the global env so the new unbound-symbol diagnostic doesn't false-
/// flag every Brood-side stdlib name (`list`, `int?`, `zero?`, `inc`, …);
/// the previous primitives-only setup worked when the checker silently
/// skipped unknown callees, but Step 4's unbound check has to know what's
/// genuinely bound.
fn warnings(src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    let form = reader::read_one(&mut interp.heap, src).expect("parse");
    check_form(&interp.heap, form)
}

#[test]
fn checker_survives_pathologically_deep_forms() {
    // Host-panic hardening (2026-07-23): a deeply-nested-but-legal form —
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

/// `warnings` but with macroexpansion — what `(check 'form)` and
/// `check-file` actually do. Required to exercise post-expansion shapes
/// like `match` (a `defmacro` whose pattern compiler lowers to
/// `let`+`if`+`%eq`), threading macros, and the test-framework wrappers.
/// Like [`warnings`], but with `mods` loaded first. A bare `Interp` carries the prelude
/// only, so a module's declared `sig`s are unknown and every cross-module check passes
/// vacuously — `brood --check` on a real file auto-requires, this harness does not.
fn warnings_with(mods: &[&str], src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    for m in mods {
        interp
            .eval_str(&format!("(require-one '{m})"))
            .unwrap_or_else(|e| panic!("require {m}: {e:?}"));
    }
    let form = reader::read_one(&mut interp.heap, src).expect("parse");
    check_form(&interp.heap, form)
}

fn warnings_expanded(src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    let form = reader::read_one(&mut interp.heap, src).expect("parse");
    let form = crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
    check_form(&interp.heap, form)
}

/// Whole-file checking — what `nest check` runs. Unlike [`warnings`] (a bare
/// fragment), this enables operand / value-slot unbound checking and threads
/// file-local def names, so it exercises the strict, file-mode behaviour.
fn file_warnings(src: &str) -> Vec<String> {
    let interp = crate::Interp::new();
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
    check_file(&mut heap, &forms)
        .into_iter()
        .map(|(_, m)| m)
        .collect()
}

// ---- ability impl conformance (Pass 2.6) ----
// The conformance check (missing op / arity / undeclared) reads the *un-expanded*
// `defability`/`impl` surface forms — which are core (prelude), always available.
// (`defprotocol`/`defimpl` were retired in favour of `ability`.)

#[test]
fn ability_impl_flags_a_missing_op() {
    let ws = file_warnings("(defability P (a [x]) (b [x]))\n(impl P :int (a [x] x))");
    assert!(ws.iter().any(|w| w.contains("missing op `b`")), "{ws:?}");
}

#[test]
fn ability_impl_flags_an_arity_mismatch() {
    let ws = file_warnings("(defability P (a [x]))\n(impl P :int (a [x y] x))");
    assert!(
        ws.iter()
            .any(|w| w.contains("`a` takes 1 arg(s), this impl has 2")),
        "{ws:?}"
    );
}

#[test]
fn ability_impl_flags_an_undeclared_method() {
    let ws = file_warnings("(defability P (a [x]))\n(impl P :int (a [x] x) (z [x] x))");
    assert!(ws.iter().any(|w| w.contains("has no op `z`")), "{ws:?}");
}

#[test]
fn ability_impl_complete_is_clean() {
    let ws = file_warnings("(defability P (a [x]) (b [x]))\n(impl P :int (a [x] x) (b [x] x))");
    assert!(!ws.iter().any(|w| w.contains("missing op")), "{ws:?}");
    assert!(!ws.iter().any(|w| w.contains("has no op")), "{ws:?}");
    assert!(!ws.iter().any(|w| w.contains("takes")), "{ws:?}");
}

// ---- ability missing-impl at call sites (Slice 3) ----
// The pass runs over the EXPANDED tree; `defability`/`impl`/`defrecord` are core
// (prelude) macros, always available to expand. Identity for a literal is its `type-of`
// kind; for a `defrecord` ctor call, its nominal id.

#[test]
fn ability_flags_a_builtin_kind_with_no_impl() {
    let ws = file_warnings(
        "\
         (defability Size (size [self] :-> int))\n\
         (impl Size :int (size [n] n))\n\
         (defn bad () (size \"hi\"))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("Size: no impl of `size` for :string")),
        "{ws:?}"
    );
}

#[test]
fn ability_flags_a_record_with_no_impl() {
    // `(defmodule t)` gives `defrecord` a namespace to bake its `:t/rect` identity into
    // (check_file's `file_ns` sets the compile ns from it); the top-level require loads
    // the module so the qualified macros expand.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defability Size (size [self] :-> int))\n\
         (defrecord rect (w h))\n\
         (defn bad () (size (rect 1 2)))",
    );
    assert!(
        ws.iter().any(|w| w.contains("no impl of `size` for :")),
        "{ws:?}"
    );
}

#[test]
fn ability_is_silent_when_the_call_is_covered() {
    let ws = file_warnings(
        "\
         (defability Size (size [self] :-> int))\n\
         (impl Size :int (size [n] n))\n\
         (defn ok () (size 5))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("no impl of `size`")),
        "{ws:?}"
    );
}

#[test]
fn ability_flags_a_record_typed_variable_via_inference() {
    // `(let (r (rect 1 2)) (size r))` — the identity of a VARIABLE, caught by the
    // `check_into` inference hook: `defrecord` emits a `sig` so the constructor's
    // record-shaped return type flows to the binding, and the hook reads its `:__id__`.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defability Size (size [self] :-> int))\n\
         (defrecord rect (w h))\n\
         (defn bad () (let (r (rect 1 2)) (size r)))",
    );
    assert!(
        ws.iter().any(|w| w.contains("no impl of `size` for :")),
        "{ws:?}"
    );
}

#[test]
fn ability_inference_is_silent_when_the_variable_is_covered() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defability Size (size [self] :-> int))\n\
         (defrecord circle (r))\n\
         (impl Size t/circle (size [c] (get c :r)))\n\
         (defn ok () (let (c (circle 2)) (size c)))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("no impl of `size`")),
        "{ws:?}"
    );
}

#[test]
fn sealed_ability_flags_a_member_missing_an_impl() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defrecord rect (w h))\n\
         (defability Shape :sealed [circle rect] (area [self] :-> float))\n\
         (impl Shape t/circle (area [c] (get c :r)))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("sealed ability Shape: no impl of `area` for :t/rect")),
        "{ws:?}"
    );
}

#[test]
fn sealed_ability_bare_impl_id_qualifies_ki15() {
    // KI-15: a bare impl id (`circle`) must qualify to the record's ns (`:t/circle`),
    // matching `:sealed`, so a bare impl counts toward exhaustiveness. Before the fix it
    // registered under `:circle` and the sealed check falsely flagged the member.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defability Shape :sealed [circle] (area [self] :-> float))\n\
         (impl Shape circle (area [c] (get c :r)))",
    );
    assert!(!ws.iter().any(|w| w.contains("sealed ability")), "{ws:?}");
}

#[test]
fn sealed_ability_complete_is_silent() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defrecord rect (w h))\n\
         (defability Shape :sealed [circle rect] (area [self] :-> float))\n\
         (impl Shape t/circle (area [c] (get c :r)))\n\
         (impl Shape t/rect (area [r] (get r :w)))",
    );
    assert!(!ws.iter().any(|w| w.contains("sealed ability")), "{ws:?}");
}

// ---- typed ability ops: `:-> RET` return types (return-type flow + impl check) ----

#[test]
fn ability_op_return_type_flows_to_call_site() {
    // `size :-> int`, so `(size 5)` yields an `int`; feeding it to `string-length`
    // (which wants a string) is a provable mismatch — proving the declared return
    // flowed into inference at the op call site.
    let ws = file_warnings(
        "\
         (defability Size (size [self] :-> int))\n\
         (impl Size :int (size [n] n))\n\
         (defn bad () (string/length (size 5)))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("string/length") && w.contains("int")),
        "the op's :-> int return should flow so string-length flags it: {ws:?}"
    );
}

#[test]
fn ability_impl_return_mismatch_is_flagged() {
    // The impl body is a string literal, but `size` declares `:-> int` — a provable
    // disjointness the impl-return check catches.
    let ws = file_warnings(
        "\
         (defability Size (size [self] :-> int))\n\
         (impl Size :int (size [n] \"hi\"))",
    );
    assert!(
        ws.iter().any(|w| w
            .contains("Size/size for :int: declared return type int but the impl yields")
            && w.contains("hi")),
        "{ws:?}"
    );
}

#[test]
fn ability_impl_return_consistent_is_silent() {
    // An `int` literal body conforms; an unknown (`n`) body defers (gradual) — neither warns.
    let ws = file_warnings(
        "\
         (defability Size (size [self] :-> int))\n\
         (impl Size :int (size [n] 5))\n\
         (impl Size :float (size [n] n))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("declared return type")),
        "{ws:?}"
    );
}

#[test]
fn ability_op_any_return_imposes_no_impl_constraint() {
    // A `:-> any` op is the gradual unknown — an impl may return anything.
    let ws = file_warnings(
        "\
         (defability Blob (blob [self] :-> any))\n\
         (impl Blob :int (blob [n] \"whatever\"))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("declared return type")),
        "{ws:?}"
    );
}

// ---- open abilities as types: any ability name is a valid type (ADR-186) ----

#[test]
fn open_ability_name_resolves_as_a_permissive_type() {
    // `Display` is an OPEN prelude ability (no closed member set). Naming it in a sig no
    // longer drops the whole declaration — it resolves to a permissive `any`, so the rest of
    // the sig survives: the `-> string` return flows and `(+ 1 (render 5))` is caught.
    let w = file_warnings(
        "\
         (defmodule t)\n\
         (sig render (Display -> string))\n\
         (defn render (x) \"s\")\n\
         (defn bad () (+ 1 (render 5)))",
    );
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("number")),
        "an open ability as a param type must keep the sig alive (return flows): {w:?}"
    );
}

#[test]
fn open_ability_param_accepts_anything() {
    // The open-ability param is permissive (its safety is enforced at op call sites, not
    // here) — so any argument is fine, no false positive.
    let w = file_warnings(
        "\
         (defmodule t)\n\
         (sig render (Display -> string))\n\
         (defn render (x) \"s\")\n\
         (defn ok () (render {:a 1}))",
    );
    assert!(
        !w.iter()
            .any(|s| s.contains("render") && s.contains("argument")),
        "an open-ability param must accept anything: {w:?}"
    );
}

// ---- typed ability op parameters: `(name T)` in an op spec (ADR-180) ----

#[test]
fn ability_op_typed_param_flags_a_bad_argument() {
    // `scale`'s second param is declared `float`; passing a string is a provable mismatch —
    // the argument-side sibling of the `:-> RET` return flow.
    let ws = file_warnings(
        "\
         (defability Scale (scale [self (factor float)] :-> int))\n\
         (impl Scale :int (scale [n f] n))\n\
         (defn bad () (scale 5 \"x\"))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("Scale/scale: argument 2 expects float")),
        "{ws:?}"
    );
}

#[test]
fn ability_op_typed_param_accepts_a_good_argument() {
    // A float where a float is wanted, and an untyped position — neither warns.
    let ws = file_warnings(
        "\
         (defability Scale (scale [self (factor float)] :-> int))\n\
         (impl Scale :int (scale [n f] n))\n\
         (defn ok () (scale 5 2.5))",
    );
    assert!(!ws.iter().any(|w| w.contains("argument")), "{ws:?}");
}

#[test]
fn ability_op_untyped_params_impose_no_argument_constraint() {
    // An all-bare op spec declares no arg types — any argument is fine.
    let ws = file_warnings(
        "\
         (defability Plain (plain [self k] :-> int))\n\
         (impl Plain :int (plain [n k] n))\n\
         (defn ok () (plain 5 \"anything\"))",
    );
    assert!(!ws.iter().any(|w| w.contains("argument")), "{ws:?}");
}

#[test]
fn ability_op_typed_param_flows_into_the_impl_body() {
    // The impl param `f` inherits the op's declared `float`, so returning it where the op
    // declares `:-> int` is a provable return mismatch (caught only because the param is typed).
    let ws = file_warnings(
        "\
         (defability Scale (scale [self (factor float)] :-> int))\n\
         (impl Scale :int (scale [n f] f))",
    );
    assert!(
        ws.iter().any(|w| w
            .contains("Scale/scale for :int: declared return type int but the impl yields float")),
        "{ws:?}"
    );
}

// ---- ability-name-as-a-type: a sealed ability is the union of its members (ADR-181) ----

#[test]
fn sealed_ability_name_resolves_as_a_type_in_a_sig() {
    // `Shape` = `(or circle rect)`; a non-record int passed where `Shape` is wanted is a
    // provable mismatch — the ability name parsed as a real (finite) union type.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defrecord rect (w h))\n\
         (defability Shape :sealed [circle rect] (area [self] :-> float))\n\
         (sig total (Shape -> float))\n\
         (defn total (s) 1.0)\n\
         (defn bad () (total 5))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("total") && w.contains("argument 1")),
        "an int passed where a sealed-ability type is wanted should warn: {ws:?}"
    );
}

#[test]
fn sealed_ability_type_accepts_a_member_record() {
    // A genuine member `(circle 2)` satisfies `Shape` — no false positive (records are open,
    // so the extra `:r` field is fine).
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defrecord rect (w h))\n\
         (defability Shape :sealed [circle rect] (area [self] :-> float))\n\
         (sig total (Shape -> float))\n\
         (defn total (s) 1.0)\n\
         (defn ok () (total (circle 2)))",
    );
    assert!(
        !ws.iter()
            .any(|w| w.contains("total") && w.contains("argument 1")),
        "a member record must satisfy the ability type: {ws:?}"
    );
}

#[test]
fn sealed_ability_type_soundness_precise_paths() {
    // SOUNDNESS (no false positives on the strict `⊆` path): a **map literal** member and a
    // `Shape`-typed param are *precise* args (checked with subtyping, not disjointness), so a
    // record-in-union subtyping bug would surface here. Both must pass clean.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defrecord rect (w h))\n\
         (defability Shape :sealed [circle rect] (area [self] :-> float))\n\
         (impl Shape t/circle (area [c] (* 1.0 (get c :r))))\n\
         (impl Shape t/rect (area [r] (* 1.0 (get r :w))))\n\
         (sig total (Shape -> float))\n\
         (defn total (s) (area s))\n\
         (defn ok-literal () (total {:__id__ :t/circle :r 2}))\n\
         (sig relay (Shape -> float))\n\
         (defn relay (s) (total s))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("argument 1")),
        "neither a map-literal member nor a Shape-typed param may false-positive: {ws:?}"
    );
}

#[test]
fn sealed_ability_type_in_op_return_position() {
    // `:-> Shape` — an op whose declared return is another (sealed) ability's domain. The
    // return flows: feeding it to `string-length` (wants string) is a provable mismatch.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defrecord rect (w h))\n\
         (defability Shape :sealed [circle rect] (area [self] :-> float))\n\
         (defability Scaled (scaled [self] :-> Shape))\n\
         (impl Scaled :int (scaled [n] (circle n)))\n\
         (defn bad (x) (string/length (scaled x)))",
    );
    assert!(
        ws.iter().any(|w| w.contains("string/length")),
        "a `:-> Shape` return should flow as the member union: {ws:?}"
    );
}

#[test]
fn non_sealed_ability_name_is_not_a_type() {
    // An OPEN ability has no closed member set, so its name is *not* a type — the sig is
    // dropped (unknown type name), and no spurious warning appears from treating it as one.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defability Open (op [self] :-> int))\n\
         (sig f (Open -> int))\n\
         (defn f (s) 1)\n\
         (defn use-it () (f 5))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("argument 1")),
        "an open ability name must not resolve to a type (sig dropped): {ws:?}"
    );
}

// ---- behaviour conformance: `(:implements …)` on a module ----

#[test]
fn behaviour_flags_a_missing_callback() {
    let ws = file_warnings(
            "(defbehaviour B (render [m]) (mount [p]))\n(defmodule foo (:implements B))\n(defn render (m) m)",
        );
    assert!(
        ws.iter()
            .any(|w| w.contains("behaviour B: this module is missing `mount`")),
        "{ws:?}"
    );
}

#[test]
fn behaviour_flags_an_arity_mismatch() {
    let ws = file_warnings(
        "(defbehaviour B (render [m]))\n(defmodule foo (:implements B))\n(defn render (m extra) m)",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("`render` takes 2 arg(s), the behaviour needs 1")),
        "{ws:?}"
    );
}

#[test]
fn behaviour_complete_module_is_clean() {
    let ws = file_warnings(
        "(defbehaviour B (render [m]))\n(defmodule foo (:implements B))\n(defn render (m) m)",
    );
    // No conformance diagnostic (the bare-interp "unbound symbol: defbehaviour"
    // noise contains the substring "behaviour", so match the real messages).
    assert!(
        !ws.iter()
            .any(|w| w.contains("module is missing") || w.contains("the behaviour needs")),
        "{ws:?}"
    );
}

/// The non-tail-recursion lint (`recursion::check_recursion`) over a
/// macroexpanded form — what `check-file`'s Pass 3.5 runs.
fn recursion_warnings(src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    let form = reader::read_one(&mut interp.heap, src).expect("parse");
    let form = crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
    let mut out = Vec::new();
    recursion::check_recursion(&interp.heap, form, &mut out);
    out.into_iter().map(|(_, m)| m).collect()
}

#[test]
fn flags_non_tail_self_recursion() {
    // self-call as an argument to another call
    assert!(
        recursion_warnings("(defn fact (n) (if (= n 0) 1 (* n (fact (- n 1)))))")
            .iter()
            .any(|w| w.contains("fact") && w.contains("non-tail"))
    );
    assert!(recursion_warnings(
        "(defn sum (xs) (if (empty? xs) 0 (+ (first xs) (sum (rest xs)))))"
    )
    .iter()
    .any(|w| w.contains("sum")));
    // self-call as a let binding value
    assert!(!recursion_warnings("(defn k (n) (let (m (k (- n 1))) m))").is_empty());
    // first (tested) operand of `and`, and a `cond` test
    assert!(!recursion_warnings("(defn p (n) (and (p n) (> n 0)))").is_empty());
    assert!(!recursion_warnings("(defn g (n) (cond (g 0) :a else :b))").is_empty());
}

#[test]
fn no_warning_for_tail_recursion_or_higher_order() {
    // proper tail calls in each tail-propagating special form
    assert!(
        recursion_warnings("(defn go (n acc) (if (= n 0) acc (go (- n 1) (* acc n))))").is_empty()
    );
    assert!(recursion_warnings("(defn down (n) (when (> n 0) (down (- n 1))))").is_empty());
    assert!(recursion_warnings("(defn f (n) (cond (= n 0) :z else (f (- n 1))))").is_empty());
    assert!(recursion_warnings("(defn p (n) (and (> n 0) (p (- n 1))))").is_empty());
    assert!(recursion_warnings("(defn k (n) (let (m (- n 1)) (k m)))").is_empty());
    // a self-call inside a nested closure is a different frame — not flagged
    assert!(recursion_warnings("(defn h (xs) (map (fn (x) (h x)) xs))").is_empty());
    // non-recursive function
    assert!(recursion_warnings("(defn g (x) (+ x 1))").is_empty());
}

#[test]
fn flags_literal_misuse_of_primitives() {
    // An int literal now infers as its singleton (B0), so the diagnostic
    // names the exact value (`5`) rather than the coarse `int` tag.
    assert!(warnings("(first 5)")
        .iter()
        .any(|w| w.contains("first") && w.contains("got 5")));
    // A keyword literal now infers as its singleton type, so the diagnostic
    // names the exact value (`:k`) rather than the coarse `keyword` tag.
    assert!(warnings("(string/length :k)")
        .iter()
        .any(|w| w.contains("string/length") && w.contains(":k")));
    assert!(warnings("(%add 1 \"x\")")
        .iter()
        .any(|w| w.contains("%add")));
    assert!(warnings("(%vector-ref [1 2] :k)")
        .iter()
        .any(|w| w.contains("vector-ref")));
}

#[test]
fn no_false_positives_when_type_is_unknown_or_right() {
    assert!(warnings("(first (list 1 2))").is_empty()); // arg is a non-sig call → dynamic
    assert!(warnings("(first xs)").is_empty()); // variable → dynamic
    assert!(warnings("(first [1 2 3])").is_empty()); // vector is allowed
    assert!(warnings("(%add 1 2)").is_empty());
    assert!(warnings("(string/length \"hi\")").is_empty());
}

#[test]
fn propagates_primitive_result_types() {
    // string-length returns int; first wants a list/vector → flag the int.
    assert!(warnings("(first (string/length \"a\"))")
        .iter()
        .any(|w| w.contains("first") && w.contains("int")));
}

#[test]
fn an_any_result_is_not_a_false_positive() {
    // vector-ref's result type is `any` (unknown), so feeding it to
    // string-length (wants string) must NOT warn — `any` overlaps `string`.
    assert!(warnings("(string/length (%vector-ref [1] 0))").is_empty());
}

#[test]
fn does_not_descend_into_quote() {
    assert!(warnings("(quote (first 5))").is_empty());
}

#[test]
fn curated_closures_are_checked() {
    // `+`, `<`, `map` are Brood closures, but their curated sigs let us flag
    // provable misuse — the headline cases.
    assert!(warnings("(+ 1 \"x\")")
        .iter()
        .any(|w| w.contains('+') && w.contains("number")));
    assert!(warnings("(< 1 :k)").iter().any(|w| w.contains('<')));
    // map's first argument must be callable; an int is not.
    assert!(warnings("(map 1 xs)")
        .iter()
        .any(|w| w.contains("map") && w.contains("argument 1")));
    // Correct uses, and an unknown (variable) callable, stay silent.
    assert!(warnings("(+ 1 2)").is_empty());
    assert!(warnings("(map inc xs)").is_empty()); // inc is a variable → unknown
}

#[test]
fn sig_declaration_is_read_by_the_checker() {
    // A user (sig …) gives a branchy fn a signature the checker trusts:
    // arguments checked against the declared params.
    let w = file_warnings("(sig f (int -> int))\n(defn f (x) (if (> x 0) x (- x)))\n(f \"s\")");
    assert!(
        w.iter()
            .any(|m| m.contains("f:") && m.contains("argument 1") && m.contains("int")),
        "declared param type should flag (f \"s\"): {w:?}"
    );
    // The declared *result* flows out: f : int, string-length wants string.
    let w = file_warnings("(sig f (int -> int))\n(defn f (x) x)\n(string/length (f 3))");
    assert!(
        w.iter().any(|m| m.contains("string/length")),
        "declared result type should flag string/length: {w:?}"
    );
    // Correct uses stay silent.
    let w = file_warnings("(sig f (int -> int))\n(defn f (x) x)\n(f 3)\n(+ 1 (f 4))");
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "correct uses of a declared fn must be silent: {w:?}"
    );
}

#[test]
fn keyword_literal_types_in_a_sig_are_enforced() {
    // A parameter typed as an enumerated keyword set flags a keyword outside it.
    let w = file_warnings("(sig f ((or :a :b) -> int))\n(defn f (x) 1)\n(f :c)");
    assert!(
        w.iter()
            .any(|m| m.contains("f:") && m.contains("argument 1") && m.contains(":a | :b")),
        "a keyword outside the literal set should flag, naming it: {w:?}"
    );
    // A member of the set is fine.
    let w = file_warnings("(sig f ((or :a :b) -> int))\n(defn f (x) 1)\n(f :a)");
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a keyword in the set must be silent: {w:?}"
    );
    // The declared literal *result* flows out and is checked too.
    let w = file_warnings(
            "(sig mode (-> (or :maximized :fullscreen)))\n(defn mode () :maximized)\n(string/length (mode))",
        );
    assert!(
        w.iter().any(|m| m.contains("string/length")),
        "a keyword-literal result feeding string-length should flag: {w:?}"
    );
}

#[test]
fn sig_declaration_handles_arity_unions_and_bad_exprs() {
    // Arity comes from the declared param count for a file-local defn the
    // read-only checker can't otherwise inspect.
    let w = file_warnings("(sig g (int int -> int))\n(defn g (a b) (+ a b))\n(g 1)");
    assert!(
        w.iter().any(|m| m.contains("expected 2")),
        "declared arity should flag (g 1): {w:?}"
    );
    // Union result type: (or int nil) — feeding it to a sink that wants a
    // string is still a provable mismatch.
    let w = file_warnings("(sig h (int -> (or int nil)))\n(defn h (x) x)\n(string/length (h 1))");
    assert!(
        w.iter().any(|m| m.contains("string/length")),
        "union result (int|nil) is disjoint from string: {w:?}"
    );
    // An unparseable type-expr is dropped — never a false signal.
    let w = file_warnings("(sig k (bogus -> int))\n(defn k (x) x)\n(k \"s\")");
    assert!(
        w.iter()
            .all(|m| !m.contains("k:") || !m.contains("argument")),
        "an unrecognised type-expr must be ignored, not guessed: {w:?}"
    );
}

#[test]
fn variadic_defn_with_sig_does_not_get_a_false_arity_warning() {
    // Regression: the `(sig …)` parser only builds *fixed*-arity sigs, so a
    // sig on a **variadic** defn would record an exact arity equal to the
    // declared param count. A read-only whole-file check can't inspect the
    // real (unevaluated) closure, so it falls back to that count — and a call
    // with more args than the sig lists would falsely warn. The def site's
    // own `& rest` must suppress the sig-derived exact arity.
    let w = file_warnings("(sig f (int -> int))\n(defn f (x & rest) x)\n(f 1 2 3)");
    assert!(
        w.iter()
            .all(|m| !(m.contains("f:") && m.contains("number of arguments"))),
        "a variadic defn must not get a false arity warning: {w:?}"
    );
    // `&rest` spelling, and below the declared count is fine too.
    let w = file_warnings("(sig g (int int -> int))\n(defn g (a &rest more) a)\n(g 1 2 3 4)");
    assert!(
        w.iter()
            .all(|m| !(m.contains("g:") && m.contains("number of arguments"))),
        "&rest variadic defn must not get a false arity warning: {w:?}"
    );
    // A multi-arity fn with a variadic arm is likewise variadic.
    let w = file_warnings("(sig h (int -> int))\n(defn h ((x) x) ((x & ys) x))\n(h 1 2 3)");
    assert!(
        w.iter()
            .all(|m| !(m.contains("h:") && m.contains("number of arguments"))),
        "multi-arity variadic defn must not get a false arity warning: {w:?}"
    );
    // Control: a *fixed*-arity sig'd defn STILL gets its arity checked (the
    // fix must not over-suppress) — mirrors the case above.
    let w = file_warnings("(sig p (int int -> int))\n(defn p (a b) (+ a b))\n(p 1)");
    assert!(
        w.iter().any(|m| m.contains("expected 2")),
        "fixed-arity sig'd defn must still be arity-checked: {w:?}"
    );
}

#[test]
fn optional_sig_params_parse_and_check() {
    // `&optional` in `(sig …)` grammar — previously unsupported: the whole
    // arrow silently failed to parse (no marker recognized `&optional`,
    // so `parse_type` on that symbol returned `None`, propagating out
    // through `parse_arrow`), meaning the sig vanished with zero warning
    // at all, not just an unchecked optional slot.

    // Call-site: the optional argument's declared type is checked, same
    // as a required one.
    let w =
        file_warnings("(sig g (int &optional string -> int))\n(defn g (a &optional b) a)\n(g 1 2)");
    assert!(
        w.iter()
            .any(|m| m.contains("g: argument 2 expects string") && m.contains("got 2")),
        "an optional arg's declared type must be checked: {w:?}"
    );

    // Arity: calling with just the required arg, or with the optional
    // one supplied, is fine; one too many is an arity error.
    let w =
        file_warnings("(sig g (int &optional string -> int))\n(defn g (a &optional b) a)\n(g 1)");
    assert!(
        w.iter().all(|m| !m.contains("number of arguments")),
        "omitting an optional arg must not warn: {w:?}"
    );
    let w = file_warnings(
        r#"(sig g (int &optional string -> int))
(defn g (a &optional b) a)
(g 1 "x")"#,
    );
    assert!(
        w.iter()
            .all(|m| !m.contains("number of arguments") && !m.contains("expects string")),
        "supplying the optional arg with the right type must not warn: {w:?}"
    );
    let w = file_warnings(
        "(sig g (int &optional string -> int))\n(defn g (a &optional b) a)\n(g 1 \"x\" 2)",
    );
    assert!(
        w.iter().any(|m| m.contains("number of arguments")),
        "one arg beyond required+optional must still be an arity error: {w:?}"
    );

    // Body seeding: an optional param is widened with `nil` (it may
    // genuinely be absent), so a defensive `(nil? b)` check is never
    // mistaken for dead code the way an exact required-param contract
    // would be — but real misuse (using it unconditionally as if it
    // can't be nil) is still caught.
    let w = file_warnings(
        "(sig g (int &optional string -> int))\n\
             (defn g (a &optional b) (if (nil? b) a (+ a (string/length b))))",
    );
    assert!(
        w.is_empty(),
        "a defensive nil-check on an optional param must not warn: {w:?}"
    );
    let w =
        file_warnings("(sig g (int &optional string -> int))\n(defn g (a &optional b) (+ a b))");
    assert!(
        w.iter()
            .any(|m| m.contains("+: argument 2 expects number") && m.contains("nil | string")),
        "using an optional param unconditionally as non-nil must still warn: {w:?}"
    );

    // `&optional` combined with a trailing `&` rest, mirroring a
    // closure's full `(req &optional opt & rest)` shape.
    let w = file_warnings(
            "(sig h (int &optional string & number -> int))\n(defn h (a &optional b & c) a)\n(h 1 \"x\" true)",
        );
    assert!(
        w.iter()
            .any(|m| m.contains("h: argument 3 expects number") && m.contains("got true")),
        "a rest arg after an optional one must still be checked: {w:?}"
    );

    // Malformed order (`&` before `&optional`) is still never *misparsed* into
    // something incorrect — but since Pass 2.85 the author is told, rather than
    // the declaration vanishing silently (an annotation that is ignored when
    // wrong is a gate that cannot fail). Nothing about `k` itself is checked.
    let w = file_warnings("(sig k (int & number &optional string -> int))\n(defn k (a) a)");
    assert!(
        w.iter()
            .any(|m| m.contains("sig k: malformed function type")),
        "a malformed marker order must be reported: {w:?}"
    );
    assert!(
        w.iter().all(|m| !m.contains("k: argument")),
        "…and must not be misparsed into an argument check: {w:?}"
    );
}

#[test]
fn tuple_sig_params_parse_and_check() {
    // `(tuple T1 T2 …)` — a fixed-arity positional vector shape
    // (ADR-128). A vector *literal* infers its exact per-position types
    // (not a widened uniform element type), so a mismatched literal
    // argument is caught by the ordinary disjointness check — no new
    // machinery needed at the call site itself.
    let w = file_warnings("(sig f ((tuple int string) -> any))\n(defn f (t) t)\n(f [\"x\" 1])");
    assert!(
        w.iter()
            .any(|m| m.contains("f: argument 1 expects (tuple int, string)")
                && m.contains("got (tuple \"x\", 1)")),
        "a mismatched tuple-shaped literal argument must warn: {w:?}"
    );
    let w = file_warnings("(sig f ((tuple int string) -> any))\n(defn f (t) t)\n(f [1 \"x\"])");
    assert!(
        w.is_empty(),
        "a matching tuple-shaped literal must not warn: {w:?}"
    );

    // Different arity is disjoint too (a vector has one definite length).
    let w =
        file_warnings("(sig f ((tuple int string) -> any))\n(defn f (t) t)\n(f [1 \"x\" true])");
    assert!(
        w.iter().any(|m| m.contains("f: argument 1")),
        "a wrong-arity tuple literal must warn: {w:?}"
    );

    // Position-aware `first`/`second`/`third`/`last`/`nth` on a
    // tuple-typed param: each resolves to its *exact* position's type
    // (not the coarse union every other element access falls back to),
    // so a mismatch on the specific position used is caught.
    let w = file_warnings(
        "(sig f ((tuple int string) -> any))\n(defn f (t) (string/length (first t)))",
    );
    assert!(
        w.iter()
            .any(|m| m.contains("string/length: argument 1 expects string") && m.contains("int")),
        "first on a tuple must resolve to position 0's exact type: {w:?}"
    );
    let w = file_warnings(
        "(sig f ((tuple int string) -> any))\n(defn f (t) (string/length (second t)))",
    );
    assert!(
        w.is_empty(),
        "second on this tuple is already a string — no warning: {w:?}"
    );
    let w = file_warnings(
        "(sig f ((tuple int string) -> any))\n(defn f (t) (string/length (nth t 0)))",
    );
    assert!(
        w.iter()
            .any(|m| m.contains("string/length: argument 1 expects string") && m.contains("int")),
        "a literal-index nth on a tuple must resolve position-exactly: {w:?}"
    );
    let w = file_warnings(
        "(sig f ((tuple int string) -> any))\n(defn f (t) (string/length (nth t 1)))",
    );
    assert!(
        w.is_empty(),
        "nth at the string position must not warn: {w:?}"
    );

    // Return-type flow: a tuple-shaped return type is checked against
    // the body's inferred literal shape, same as any other declared
    // return type.
    let w = file_warnings(
        r#"(sig f (-> (tuple int string)))
(defn f () ["x" 1])"#,
    );
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type (tuple int, string)")),
        "a mismatched declared tuple return type must warn: {w:?}"
    );

    // A tuple is a subtype of the corresponding uniform vector type (every
    // element of a `tuple<int,string>` is an `int | string`) — so passing
    // a tuple-shaped literal where a plain `(vector …)` is expected must
    // not warn just because the shapes differ.
    let w = file_warnings("(sig g ((vector any) -> any))\n(defn g (v) v)\n(g [1 \"x\"])");
    assert!(
        w.is_empty(),
        "a tuple literal must satisfy a uniform vector param: {w:?}"
    );
}

#[test]
fn dead_clause_flagged_for_a_sig_typed_param() {
    // A `match` literal pattern that can't match the parameter's declared type.
    let w =
        file_warnings("(sig f (int -> keyword))\n(defn f (n) (match n (\"hi\" :s) (_ :other)))");
    assert!(
        w.iter()
            .any(|m| m.contains("unreachable clause") && m.contains("int")),
        "a string-literal clause when n : int should be dead: {w:?}"
    );
    // A `cond` predicate disjoint from the declared parameter type.
    let w = file_warnings("(sig g (int -> keyword))\n(defn g (n) (cond (string? n) :s else :o))");
    assert!(
        w.iter().any(|m| m.contains("unreachable clause")),
        "(string? n) when n : int should be dead: {w:?}"
    );
}

#[test]
fn dead_clause_silent_without_sig_or_when_compatible_or_a_literal_scrutinee() {
    // No `sig` → the parameter is untyped → never flagged (no false positive).
    assert!(
        file_warnings("(defn k (n) (match n (\"hi\" :s) (_ :o)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "no sig ⇒ no dead-clause"
    );
    // A recognised but *compatible* guard narrows, it isn't dead.
    assert!(
        file_warnings("(sig h (int -> keyword))\n(defn h (n) (cond (int? n) :i else :o))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "(int? n) when n : int must not flag"
    );
    // A literal scrutinee is not a sig-typed param — the gate excludes it (this
    // is the intentional non-match test shape that the naive lint flagged).
    assert!(
        file_warnings("(defn m () (match [1 2] ((a) :one) (_ :o)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "a literal scrutinee must never be flagged dead"
    );
}

#[test]
fn dead_clause_flagged_for_a_precise_let_local() {
    // ADR-131: the dead-clause lint now covers a *precise, surface* `let`-local,
    // not just a sig-typed param. `x` is statically `5` (a literal, precise), so
    // a `string?` clause can never run.
    let w = file_warnings("(defn f () (let (x 5) (cond (string? x) :a else :b)))");
    assert!(
        w.iter()
            .any(|m| m.contains("unreachable clause") && m.contains("string")),
        "a string? clause on a let-local typed 5 should be dead: {w:?}"
    );
    // A `match` literal pattern disjoint from the local's type is dead too.
    let w = file_warnings("(defn g () (let (x 5) (match x (\"hi\" :s) (_ :o))))");
    assert!(
        w.iter().any(|m| m.contains("unreachable clause")),
        "a string-literal pattern on a let-local typed int should be dead: {w:?}"
    );
}

#[test]
fn dead_clause_let_local_respects_precision_gensym_and_compatibility() {
    // A *compatible* guard narrows without emptying — not dead.
    assert!(
        file_warnings("(defn a () (let (x 5) (cond (int? x) :a else :b)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "(int? x) when x : int must not flag"
    );
    // A local bound to a **call result** is `dynamic` (redefinable → the type
    // could change on reload), so it's excluded — no dead-clause warning even
    // though the current-image type would narrow to `never`. Reload-safe.
    assert!(
        file_warnings("(defn h () 5)\n(defn b () (let (x (h)) (cond (string? x) :a else :b)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "a call-result local is dynamic and must not be flagged dead"
    );
    // A **gensym** temporary (macro-introduced) is exempt — warning on a name
    // the user can't rename would be noise.
    assert!(
        file_warnings("(defn c () (let (x__1 5) (cond (string? x__1) :a else :b)))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "a gensym let-local must never be flagged dead"
    );
    // Shadowing a precise local with an unknown one drops eligibility.
    assert!(
        file_warnings("(defn d (y) (let (x 5) (let (x y) (cond (string? x) :a else :b))))")
            .iter()
            .all(|m| !m.contains("unreachable")),
        "a shadowing rebind of unknown type must not be flagged dead"
    );
}

#[test]
fn curated_helper_sigs_catch_misuse() {
    // even?/odd?/abs require a number. Written QUALIFIED: they are `math/` since ADR-227,
    // and the bare spelling is now unbound without `(:use math)`. Asserting the bare form
    // here was testing a dead key — and worse, the bare entry it relied on is what
    // suppressed the unbound lint on a name that no longer exists (see `sigs.rs`).
    assert!(warnings("(math/even? \"x\")")
        .iter()
        .any(|w| w.contains("even?") && w.contains("number")));
    assert!(warnings("(math/odd? :k)")
        .iter()
        .any(|w| w.contains("odd?") && w.contains("number")));
    assert!(warnings("(math/abs :k)")
        .iter()
        .any(|w| w.contains("abs") && w.contains("number")));
    // count wants a string | map | sequence, not a number.
    assert!(warnings("(count 5)").iter().any(|w| w.contains("count")));
    // There is no `length` function and never was (see `sigs.rs`) — so the right warning
    // is UNBOUND, not a type mismatch. Asserted explicitly, because the old assertion
    // ("some warning mentioning length") passes either way and so proved nothing.
    assert!(warnings("(length :k)")
        .iter()
        .any(|w| w.contains("unbound") && w.contains("length")));
    // not/zero? accept any arg but pin a bool *result*, so feeding it to a
    // numeric sink is caught (the result-type payoff).
    assert!(warnings("(+ 1 (not x))")
        .iter()
        .any(|w| w.contains('+') && w.contains("bool")));
    assert!(warnings("(+ 1 (math/zero? x))")
        .iter()
        .any(|w| w.contains('+') && w.contains("bool")));
    // Correct uses stay silent (no false positives).
    for ok in [
        "(math/even? 4)",
        "(math/abs -3)",
        "(count [1 2 3])",
        "(count \"hi\")",
        // `bytes` is seqable/countable: these iterate its octets at runtime.
        "(count (bytes 1 2 3))",
        "(first (bytes 1 2 3))",
        "(rest (bytes 1 2 3))",
        "(every? math/odd? (bytes 1 3 5))",
        "(not x)",
        "(math/zero? n)",
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

/// A curated sig must never be the reason a name looks *bound*.
///
/// An entry in `CURATED_SIGS` marks its name as one the checker knows, which suppresses the
/// unbound lint — so a stale entry for a name that has moved out of the prelude makes
/// `nest check` silent on code that dies at runtime. That is what happened after ADR-227
/// moved `even?`/`odd?`/`abs` into `std/math.blsp`: a bare `(even? 4)` with no `(:use math)`
/// is an unbound error when run, and `nest check` — the gate that exits nonzero on any
/// warning — reported nothing at all, while its uncurated siblings `sum`/`frequencies`
/// correctly said "unbound symbol". Nothing else in the tree observes this: the checker is
/// advisory, so the only symptom is a program that passes CI and then fails.
#[test]
fn a_curated_sig_does_not_mask_the_unbound_lint_for_a_moved_name() {
    for moved in ["even?", "odd?", "abs", "index-where"] {
        let src = format!("(defn f () ({moved} 4))");
        let ws = file_warnings(&src);
        assert!(
            ws.iter()
                .any(|w| w.contains("unbound") && w.contains(moved)),
            "bare `{moved}` is `math/`/`seq/` since ADR-227 and unbound without an import, \
             so the checker must say so — a curated sig keyed on the bare name silences this \
             and lets a runtime-unbound program pass `nest check`. Got: {ws:?}"
        );
    }
    // The qualified spelling is the one that exists, and it still gets the vetted signature.
    assert!(warnings("(math/abs :k)")
        .iter()
        .any(|w| w.contains("abs") && w.contains("number")));
}

#[test]
fn curated_output_and_numeric_sigs() {
    // io/puts and io/write return nil — feeding to a numeric sink is caught.
    for f in ["io/puts", "io/write"] {
        let w = warnings(&format!("(+ 1 ({f} \"hi\"))"));
        assert!(
            w.iter().any(|s| s.contains('+') && s.contains("nil")),
            "{f}: expected '+' nil-result warning, got {w:?}"
        );
    }
    // min/max require at least one number.
    assert!(warnings("(math/min \"a\" 2)")
        .iter()
        .any(|w| w.contains("min") && w.contains("number")));
    assert!(warnings("(math/max 1 :k)")
        .iter()
        .any(|w| w.contains("max") && w.contains("number")));
    // min/max return a number — feeding to a string sink is caught.
    assert!(warnings("(string/length (math/min 1 2))")
        .iter()
        .any(|w| w.contains("string/length")));
    // Correct uses stay silent.
    for ok in [
        "(io/puts \"hi\")",
        "(math/min 1 2 3)",
        "(math/max 0.5 1.5)",
        "(+ 1 (math/min 2 3))",
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

#[test]
fn unused_let_binding_lint() {
    // Basic unused binding — warned.
    let w = file_warnings("(let (x 1) 2)");
    assert!(
        w.iter()
            .any(|s| s.contains("unused let binding") && s.contains('x')),
        "expected unused-binding warning for x, got {w:?}"
    );
    // Binding used in body — silent.
    assert!(
        file_warnings("(let (x 1) x)").is_empty(),
        "used binding should be silent"
    );
    // Binding used in subsequent binding RHS — silent.
    assert!(
        file_warnings("(let (x 1 y (+ x 1)) y)").is_empty(),
        "x used by y's RHS should be silent"
    );
    // Only one of two is unused.
    let w = file_warnings("(let (x 1 y 2) x)");
    assert!(
        w.iter()
            .any(|s| s.contains("unused let binding") && s.contains('y')),
        "y should be flagged unused, got {w:?}"
    );
    assert!(
        w.iter().all(|s| !s.contains('x') || !s.contains("unused")),
        "x should not be flagged, got {w:?}"
    );
    // `_`-prefixed names are exempt.
    assert!(
        file_warnings("(let (_x 1) 2)").is_empty(),
        "_x should be exempt from unused-binding lint"
    );
    // Gensym temporaries (`<prefix>__<n>`) are exempt: a macro expansion can
    // attach its call-site position to the generated `let`, so the name — not
    // the position — is the reliable "compiler-generated" signal.
    assert!(
        file_warnings("(let (m__1380 1) 2)").is_empty(),
        "gensym-named binding should be exempt from unused-binding lint"
    );
    // …but a hand-written name that merely contains `__` (no trailing digits)
    // is still linted.
    assert!(
        file_warnings("(let (my__thing 1) 2)")
            .iter()
            .any(|s| s.contains("unused let binding")),
        "a non-gensym `__` name should still be flagged"
    );
    // match pattern variables (compiler-generated let, no source position)
    // must be exempt — a common pattern: match on shape, ignore values.
    assert!(
        file_warnings("(match (list 1 2) ([a b] :vec) (_ :other))").is_empty(),
        "match pattern variables should be exempt (no FP)"
    );
    // Nested let: inner binding used only in inner body.
    assert!(
        file_warnings("(let (x 1) (let (y x) y))").is_empty(),
        "nested let: both x and y are used"
    );
    // letrec: mutual recursion keeps both used.
    assert!(
        file_warnings("(letrec (f (fn (n) (if (= n 0) 1 (g (- n 1)))) g (fn (n) (f n))) (f 5))")
            .is_empty(),
        "letrec mutual recursion: both f and g are used"
    );
    // Binding used only inside a map literal — silent. Map literals are
    // heap maps, not pairs, so the occurrence scan must descend into their
    // keys and values too (regression: the editor's `{:start s :end e}`
    // edit forms were all falsely flagged unused).
    assert!(
        file_warnings("(let (s 1) {:start s})").is_empty(),
        "binding used as a map value should be silent"
    );
    assert!(
        file_warnings("(let (k :a) {k 1})").is_empty(),
        "binding used as a map key should be silent"
    );
    // …and a binding used only inside a closure that is itself a map value
    // (the minibuffer `:on-complete (fn …)` pattern).
    assert!(
        file_warnings("(let (p 1) {:on-complete (fn (x) (+ x p))})").is_empty(),
        "binding captured by a closure inside a map should be silent"
    );
    // The map descent must not mask genuine dead bindings.
    let w = file_warnings("(let (s 1) {:start 2})");
    assert!(
        w.iter()
            .any(|s| s.contains("unused let binding") && s.contains('s')),
        "s unused even though a map literal is present, got {w:?}"
    );
}

#[test]
fn curated_equality_and_string_sigs() {
    // = / not= are multi-arm closures; pin bool result so numeric sinks catch it.
    assert!(warnings("(+ 1 (= x y))")
        .iter()
        .any(|w| w.contains('+') && w.contains("bool")));
    assert!(warnings("(+ 1 (not= x y))")
        .iter()
        .any(|w| w.contains('+') && w.contains("bool")));
    // string/->symbol requires a string.
    assert!(warnings("(string/->symbol 99)")
        .iter()
        .any(|w| w.contains("string/->symbol") && w.contains("string")));
    // String predicates require string args.
    for f in ["string/starts-with?", "string/ends-with?"] {
        assert!(
            warnings(&format!("({f} 5 \"x\")"))
                .iter()
                .any(|w| w.contains(f) && w.contains("string")),
            "{f}: expected string-domain warning"
        );
    }
    assert!(warnings("(string/blank? 0)")
        .iter()
        .any(|w| w.contains("string/blank?") && w.contains("string")));
    // String transforms require string args and return strings.
    for f in ["string/trim", "string/triml", "string/trimr"] {
        assert!(
            warnings(&format!("({f} 5)"))
                .iter()
                .any(|w| w.contains(f) && w.contains("string")),
            "{f}: expected string-domain warning"
        );
        // Result is string — safe to pass to string-length.
        assert!(
            warnings(&format!("(string/length ({f} s))")).is_empty(),
            "{f}: result should type as string"
        );
    }
    assert!(warnings("(string/replace 5 \"a\" \"b\")")
        .iter()
        .any(|w| w.contains("string/replace") && w.contains("string")));
    assert!(warnings("(string/repeat 3 5)")
        .iter()
        .any(|w| w.contains("string/repeat") && w.contains("string")));
    assert!(warnings("(string/format 5 \"extra\")")
        .iter()
        .any(|w| w.contains("format") && w.contains("string")));
    // format returns a string.
    assert!(warnings("(string/length (string/format \"hi %s\" x))").is_empty());
    // index-of/index-where/string/last-index-of return int — safe to add.
    // (`last-index-of` moved into the `string` module on 2026-08-27; the curated
    // entry is keyed qualified, so the bare name is now correctly unbound.)
    assert!(warnings("(+ 1 (index-of coll x))").is_empty());
    assert!(warnings("(+ 1 (string/last-index-of s needle))").is_empty());
    // Correct uses stay silent.
    for ok in [
        "(= 1 2)",
        "(not= x y)",
        "(string/starts-with? s \"pre\")",
        "(string/ends-with? s \".blsp\")",
        "(string/trim s)",
        "(string/replace s \"a\" \"b\")",
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

#[test]
fn skips_error_testing_forms() {
    // `try` and the error-asserting helpers deliberately exercise failures,
    // so misuse inside them is not flagged.
    assert!(warnings("(try (first 5) (catch e e))").is_empty());
    assert!(warnings("(error-of (first 5))").is_empty());
    assert!(warnings("(assert-error (first 5))").is_empty());
    // ...but a sibling form outside the skipped one is still checked.
    assert!(!warnings("(do (first 5) (try (first 6) (catch e e)))").is_empty());
}

/// KI-67. An error-testing form suppresses *misuse* — that is what it is for —
/// but an **unbound symbol** inside one is a dead call site, not the failure
/// under test. Skipping the body outright let a rename wave ship a broken `try`
/// with every gate green: hive's spool write was
/// `(try (bytes/append path piece) (catch e …))`, the callee was renamed to
/// `file/spit-bytes-append`, `nest check` said nothing, and every upload broke.
#[test]
fn unbound_inside_an_error_testing_form_is_still_flagged() {
    for src in [
        "(try (definitely-not-bound 1) (catch e e))",
        "(error-of (definitely-not-bound 1))",
        "(assert-error (definitely-not-bound 1))",
        // nested one level down, not just in head position
        "(try (first (definitely-not-bound 1)) (catch e e))",
    ] {
        let w = warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("unbound symbol: definitely-not-bound")),
            "{src} should flag the unbound name, got {w:?}"
        );
    }
}

/// KI-71 — a **reversed-args rename** is the one rename mistake with no natural gate: the
/// arity is unchanged and no name is unbound, so `nest check` is silent and the wrong answer
/// surfaces somewhere else entirely (`seq/remove-nth` moving to index-first read as seven
/// unrelated buffer-lifecycle failures downstream). A declared `sig` is what makes it
/// visible, and the index/collection functions in `std/seq.blsp` now carry one.
///
/// Argument types are precise on purpose and the return is `any`: the reversal is an
/// ARGUMENT mistake, and a too-narrow return would false-positive at every call site.
#[test]
fn a_reversed_index_and_collection_call_is_flagged() {
    for (src, fname) in [
        ("(seq/remove-nth [1 2 3] 1)", "remove-nth"),
        ("(seq/take-last (list 1 2 3) 2)", "take-last"),
        ("(seq/chunk-every [1 2 3 4] 2)", "chunk-every"),
        ("(seq/split-at [1 2 3] 1)", "split-at"),
    ] {
        let w = warnings_with(&["seq"], src);
        assert!(
            w.iter()
                .any(|m| m.contains(fname) && m.contains("argument 1 expects int")),
            "{src} reverses index and collection and should be flagged, got {w:?}"
        );
    }
}

/// The false-positive half: the CORRECT order must stay silent, and so must a call whose
/// arguments are untyped locals — the checker only knows a param's type when something
/// says so, and guessing would make these sigs unusable.
#[test]
fn the_correct_index_first_order_stays_silent() {
    for src in [
        "(seq/remove-nth 1 [1 2 3])",
        "(seq/take-last 2 (list 1 2 3))",
        "(seq/chunk-every 2 [1 2 3 4])",
        "(seq/split-at 1 [1 2 3])",
        "(fn (i coll) (seq/remove-nth i coll))",
    ] {
        assert!(
            warnings_with(&["seq"], src).is_empty(),
            "{src} is correct and should be silent, got {:?}",
            warnings_with(&["seq"], src)
        );
    }
}

/// KI-70 — the walk used to `return` for any form that was not a `Pair`, so every
/// expression nested inside a vector or map LITERAL was invisible to every lint.
/// Hiccup-shaped code is written entirely that way, which is how `(str (max 2 …))`
/// survived in hive's `/docs` renderer long after `max` moved to `math`, with
/// `nest check` green and only a page render raising it.
#[test]
fn unbound_inside_a_vector_or_map_literal_is_flagged() {
    for src in [
        "[:tag (definitely-not-bound 1)]",            // vector literal
        "{:k (definitely-not-bound 1)}",              // map literal
        "{(definitely-not-bound 1) :v}",              // map literal, KEY position
        "[:tag {:k (definitely-not-bound 1)}]",       // map inside a vector
        "[:tag {:k (str (definitely-not-bound 1))}]", // the shape found in the wild
        "[[[(definitely-not-bound 1)]]]",             // nested vectors
    ] {
        let w = warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("unbound symbol: definitely-not-bound")),
            "{src} should flag the unbound name, got {w:?}"
        );
    }
}

/// The false-positive half of KI-70. Descending into literals must not start
/// reading DATA as code: `quote`/`quasiquote` stop the walk before their contents
/// are ever handed down, and the checker runs on macroexpanded forms, so a `match`
/// pattern vector has already become `let`/`if` binders by the time we get here.
#[test]
fn descending_into_a_literal_does_not_read_data_as_code() {
    for src in [
        "'[a b c]",                                   // quoted vector of bare symbols
        "'{:k v}",                                    // quoted map
        "(quote [definitely-not-bound])",             // explicit quote
        "(match [1 2] ([a b] (+ a b)) (_ 0))",        // pattern binders, post-expansion
        "(let (xs [1 2 3]) (map (fn (n) [n n]) xs))", // ordinary literal use
    ] {
        assert!(
            warnings(src).is_empty(),
            "{src} should stay silent, got {:?}",
            warnings(src)
        );
    }
}

/// The other half of KI-67: everything that is *not* an unbound symbol stays
/// suppressed inside an error-testing form. Filtering happens at the collection
/// point, so a lint added later is suppressed here by default — which is the
/// right default for a form whose purpose is to exercise a failure.
#[test]
fn only_unbound_survives_an_error_testing_form() {
    for src in [
        "(error-of (cons 1))",              // arity
        "(try (first 5) (catch e e))",      // type misuse
        "(assert-error (string/length 5))", // sig mismatch
    ] {
        assert!(
            warnings(src).is_empty(),
            "{src} should stay silent, got {:?}",
            warnings(src)
        );
    }
}

/// A test that really does assert on an unbound name opts out explicitly.
#[test]
fn check_allow_unbound_still_silences_an_error_testing_body() {
    assert!(
        warnings("(check-allow :unbound (try (definitely-not-bound 1) (catch e e)))").is_empty()
    );
}

#[test]
fn map_kv_refinement_flows_through_checker() {
    // (sig f ((map keyword int) -> int)): the get result is int | nil.
    // Feeding that to string-length should warn. Without the sig the result
    // type is unknown → no warning, so the sig must be declared — use
    // file_warnings so the `sig` form is parsed.
    let src = "
(defn f (m) (get m :k))
(sig f ((map keyword int) -> int))
(string/length (f {:a 1}))
";
    let w = file_warnings(src);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "expected string-length warning for int|nil arg, got {w:?}"
    );

    // `(keys m)` where m : map<keyword, int> → nil | list<keyword>.
    // Feeding to string-length warns (list is not a string).
    let src2 = "
(defn g (m) (keys m))
(sig g ((map keyword int) -> (list keyword)))
(string/length (g {:a 1}))
";
    let w2 = file_warnings(src2);
    assert!(
        w2.iter().any(|s| s.contains("string/length")),
        "expected string-length warning for list<keyword> arg, got {w2:?}"
    );

    // Correct uses stay silent.
    for ok in [
        "(get {:a 1} :a)", // any map get — flat result, no warning
        "(keys {:a 1})",
        "(vals {:a 1})",
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

#[test]
fn record_type_annotation_parses_and_accepts_valid_calls() {
    // `(record …)` is accepted as a `(sig …)` annotation and carries a
    // full field refinement (see docs/type-records.md), so a valid call
    // produces no spurious warning.
    let src = "
(defn f (m) m)
(sig f ((record :a int :b (optional string)) -> any))
(f {:a 1 :b \"x\"})
";
    let w = file_warnings(src);
    assert!(w.is_empty(), "expected no warnings, got {w:?}");

    // A malformed record annotation (odd field-list length, or a
    // non-keyword key) is dropped rather than guessed — the sig source
    // still parses (it's just not read as an authoritative signature),
    // so the checker doesn't crash and falls back to no declared sig.
    for bad in [
        "(defn f (m) m)\n(sig f ((record :a int :b) -> any))\n(f {:a 1})",
        "(defn f (m) m)\n(sig f ((record a int) -> any))\n(f {:a 1})",
    ] {
        let _ = file_warnings(bad); // must not panic
    }
}

#[test]
fn record_field_refinement_flows_through_checker() {
    // (sig f ((record :a int) -> int)): `(get m :a)` on a declared record
    // resolves to the *exact field type* (int | nil), not a flat
    // fallback — feeding that to string-length should warn.
    let src = "
(defn f (m) (get m :a))
(sig f ((record :a int) -> int))
(string/length (f {:a 1}))
";
    let w = file_warnings(src);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "expected string-length warning for int|nil arg, got {w:?}"
    );

    // A key a *fully-read literal* doesn't carry is provably absent (ADR-264), so it
    // reads as `nil` — and `(string/length nil)` is a real error, not a false positive.
    assert!(
        warnings("(let (m {:a 1}) (string/length (get m :other)))")
            .iter()
            .any(|w| w.contains("expects string")),
        "an absent key on a closed literal is nil, and must be caught"
    );
    // But a literal the checker could not read completely stays OPEN — the dropped
    // entry might be the key being asked for, so nothing may be concluded about it.
    assert!(
        warnings("(let (m {:a 1 :b (unknown-thing)}) (string/length (get m :other)))")
            .iter()
            .all(|w| !w.contains("expects string")),
        "an incompletely-read literal must not claim a key is absent"
    );

    // Record-literal type inference: `{:a 1}` infers a record shape
    // (`:a` required, type int) directly from the literal, no `sig`
    // needed — feeding the field straight to a sink warns.
    assert!(
        warnings("(string/length (get {:a 1} :a))")
            .iter()
            .any(|w| w.contains("string/length")),
        "expected a warning from the inferred record-literal shape"
    );

    // Correct uses stay silent.
    for ok in [
        "(get {:a 1} :a)",
        "(string/length (get {:a \"x\"} :a))",
        "(get {:a 1} :b)", // undeclared key — unresolved, not a warning
    ] {
        assert!(
            warnings(ok).iter().all(|w| !w.contains("expects")),
            "{ok} should be silent: {:?}",
            warnings(ok)
        );
    }
}

#[test]
fn overload_refinement_flows_through_checker() {
    // (sig f (and (int -> int) (string -> string))): `f`'s return type
    // depends on which arm matched the call's argument (ADR-116).
    let src = "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
(string/length (f 1))
";
    let w = file_warnings(src);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "an int arg should resolve to the int arm's return type, got {w:?}"
    );

    // The string arm's return type feeds `string-length` cleanly.
    let src2 = "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
(string/length (f \"hi\"))
";
    assert!(
        file_warnings(src2).is_empty(),
        "a string arg should resolve to the string arm's return type, got {:?}",
        file_warnings(src2)
    );

    // The string arm's return type is NOT a number — feeding it to `+` warns.
    let src3 = "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
(+ 1 (f \"hi\"))
";
    assert!(
        file_warnings(src3).iter().any(|s| s.contains('+')),
        "a string return type fed to + should warn, got {:?}",
        file_warnings(src3)
    );

    // An argument of unknown type widens to the union of every matching
    // arm's return — `int | string` — which is NOT disjoint from
    // `string`, so no false positive (sound, just less precise).
    let src4 = "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
(defn g (y) (string/length (f y)))
";
    assert!(
        file_warnings(src4).is_empty(),
        "an unknown-typed arg should widen, not warn, got {:?}",
        file_warnings(src4)
    );
}

#[test]
fn overload_resolves_cross_module_via_the_heap_store() {
    // `file_warnings`/`warnings` never *evaluate* — `%register-sig` only
    // runs at load time, so those helpers only ever exercise the
    // per-file `Ctx` path (`ctx.declared_overload`), never the
    // heap-level `runtime.declared_sigs` store that makes a plain
    // single-arrow sig visible cross-module. Simulate "module A defines
    // and declares f; module B (a fresh Ctx — no file-local knowledge of
    // f at all) calls it" by actually *evaluating* the declaration first
    // (`eval_str`, so `%register-sig` really populates the heap), then
    // typing a call form against an empty `Ctx` — exactly module B's
    // starting point.
    use super::infer::expr_ty;

    let mut interp = crate::Interp::new();
    interp
        .eval_str(
            "
(defn f (x) x)
(sig f (and (int -> int) (string -> string)))
",
        )
        .expect("module A loads cleanly");

    // An int-typed argument resolves to the int arm's return type.
    let call_int = reader::read_one(&mut interp.heap, "(f 1)").expect("parse");
    let t = expr_ty(&interp.heap, call_int, &Ctx::default())
        .expect("cross-module overload should resolve, not come back unknown");
    assert!(
        t.is_subtype(&Ty::of(Tag::Int)),
        "expected int for an int arg, got {t}"
    );

    // A string-typed argument resolves to the string arm's return type.
    let call_str = reader::read_one(&mut interp.heap, "(f \"hi\")").expect("parse");
    let t2 = expr_ty(&interp.heap, call_str, &Ctx::default())
        .expect("cross-module overload should resolve, not come back unknown");
    assert!(
        t2.is_subtype(&Ty::of(Tag::Str)),
        "expected string for a string arg, got {t2}"
    );
}

#[test]
fn int_literal_return_type_flows_through_checker() {
    // (sig f ((or 200 404 500) -> …)): f's declared return type is an
    // int-literal set (ADR-117), not flat `int` — feeding a call to
    // `string-length` should warn (disjoint tags: string vs. int), the
    // same as it would for a flat `int` return, proving the literal-set
    // Ty flows through `sig_of`/`declared_sig` like any other arrow.
    let src = "
(defn f (x) x)
(sig f ((or 200 404 500) -> (or 200 404 500)))
(string/length (f 200))
";
    let w = file_warnings(src);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "expected string-length warning for an int-literal return, got {w:?}"
    );

    // A correct use (an int sink) stays silent.
    let src2 = "
(defn f (x) x)
(sig f ((or 200 404 500) -> (or 200 404 500)))
(+ 1 (f 200))
";
    assert!(
        file_warnings(src2).is_empty(),
        "an int-literal return fed to + should be silent, got {:?}",
        file_warnings(src2)
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_keyword_arm() {
    let src = "
(defn f (status)
  (match status
    (:ok \"good\")
    (:error \"bad\")))
(sig f ((or :ok :error :pending) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains(":pending")),
        "expected a missing-:pending warning, got {w:?}"
    );
}

#[test]
fn guard_purity_flags_an_effect_in_a_match_when_guard() {
    let src = "
(defn f (n counter)
  (match n
    (x :when (do (%table-put counter :seen 1) (> x 0)) :pos)
    (_ :neg)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("%table-put") && s.contains(":when` guard")),
        "expected an effectful-guard warning naming %table-put, got {w:?}"
    );
}

#[test]
fn guard_purity_flags_an_effect_in_a_receive_when_guard() {
    let src = "
(defn worker (counter)
  (receive
    (n :when (< (%table-incr counter :seen) 100) n)
    (_ :skip)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("%table-incr") && s.contains("guard")),
        "expected an effectful-guard warning naming %table-incr, got {w:?}"
    );
}

#[test]
fn guard_purity_is_silent_for_a_pure_guard() {
    let src = "
(defn f (n)
  (match n
    (x :when (> x 0) :pos)
    (x :when (int? x) :int)
    (_ :other)))
(defn g (a b) :when (>= a b) a)
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains(":when` guard")),
        "a pure guard must be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn guard_purity_does_not_flag_an_effect_in_the_clause_body() {
    // The effect is in the body, where it belongs — only the guard is linted.
    let src = "
(defn f (n)
  (match n
    (x :when (> x 0) (io/puts x))
    (_ :neg)))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains(":when` guard")),
        "an effect in the clause body must not be flagged, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_is_silent_when_every_arm_is_covered() {
    let src = "
(defn f (status)
  (match status
    (:ok \"good\")
    (:error \"bad\")
    (:pending \"waiting\")))
(sig f ((or :ok :error :pending) -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a fully-covered match should be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_is_silent_with_a_catch_all_clause() {
    // A catch-all makes the throw disappear from the compiled tree
    // entirely — trivially exhaustive regardless of how few literal arms
    // are listed.
    let src = "
(defn f (status)
  (match status
    (:ok \"good\")
    (_ \"anything else\")))
(sig f ((or :ok :error :pending) -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a catch-all match should be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_int_arm() {
    let src = "
(defn f (code)
  (match code
    (200 \"ok\")
    (404 \"missing\")))
(sig f ((or 200 404 500) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains("500")),
        "expected a missing-500 warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_arm_in_a_mixed_kind_enum() {
    // (or :ok 5) — a keyword literal and an int literal on the same
    // declared type (ADR-121 generalizes the old pure-one-kind check).
    let src = "
(defn f (x)
  (match x
    (:ok \"good\")))
(sig f ((or :ok 5) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains('5')),
        "expected a missing-5 warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_arm_with_a_trailing_nil() {
    let src = "
(defn f (x)
  (match x
    (:ok \"good\")
    (:error \"bad\")))
(sig f ((or :ok :error nil) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains("nil")),
        "expected a missing-nil warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_bool_arm() {
    // Note: bare `bool` in a sig is the *unrefined* flat tag (no
    // `lit_bool` set) — `(or true false)` is what actually declares the
    // enumerable 2-value literal type this check needs.
    let src = "
(defn f (x) (match x (true \"yes\")))
(sig f ((or true false) -> string))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains("false")),
        "expected a missing-false warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_flags_a_missing_string_arm() {
    let src = "
(defn f (m)
  (match m
    (\"GET\" 1)))
(sig f ((or \"GET\" \"POST\") -> int))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("not exhaustive") && s.contains("POST")),
        "expected a missing-POST warning, got {w:?}"
    );
}

#[test]
fn match_exhaustiveness_is_silent_when_a_mixed_kind_enum_is_fully_covered() {
    let src = "
(defn f (x)
  (match x
    (:ok \"good\")
    (5 \"five\")
    (nil \"nothing\")))
(sig f ((or :ok 5 nil) -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a fully-covered mixed-kind match should be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_declines_a_destructuring_clause_mixed_in() {
    // A non-literal pattern among those tried (here, a vector destructure)
    // means the check can't reason about coverage — bail rather than
    // guess.
    let src = "
(defn f (x)
  (match x
    (:ok \"good\")
    ([a b] \"pair\")))
(sig f ((or :ok :error) -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a match mixing a literal with a destructuring pattern should stay silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_exhaustiveness_is_silent_for_a_non_literal_scrutinee_type() {
    // status's declared type is bare `keyword` — not a bounded literal
    // enum — so there's nothing to enumerate against.
    let src = "
(defn f (status)
  (match status
    (:ok \"good\")
    (:error \"bad\")))
(sig f (keyword -> string))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("not exhaustive")),
        "a non-literal-enum scrutinee should stay silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_redundancy_flags_an_adjacent_duplicate_clause() {
    let src = "
(defn f (x)
  (match x
    (:ok 1)
    (:ok 2)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("unreachable clause") && s.contains(":ok")),
        "expected an unreachable-clause warning, got {w:?}"
    );
}

#[test]
fn match_redundancy_flags_a_non_adjacent_duplicate_clause() {
    let src = "
(defn f (x)
  (match x
    (:ok 1)
    (:error 2)
    (:ok 3)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("unreachable clause") && s.contains(":ok")),
        "expected an unreachable-clause warning for the non-adjacent duplicate, got {w:?}"
    );
}

#[test]
fn match_redundancy_is_silent_with_no_duplicates() {
    let src = "
(defn f (x)
  (match x
    (:ok 1)
    (:error 2)
    (_ 3)))
";
    assert!(
        file_warnings(src)
            .iter()
            .all(|w| !w.contains("unreachable clause")),
        "no duplicate clauses should be silent, got {:?}",
        file_warnings(src)
    );
}

#[test]
fn match_redundancy_fires_on_a_hand_written_eq_chain_too() {
    // Purely structural — not `match`-specific. A hand-written same-symbol
    // `%eq`-if chain with a duplicate literal is unreachable the same way.
    let src = "
(defn f (x)
  (if (%eq x 5)
    :a
    (if (%eq x 5)
      :b
      :c)))
";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|s| s.contains("unreachable clause") && s.contains('5')),
        "expected an unreachable-clause warning for the hand-written chain, got {w:?}"
    );
}

#[test]
fn covers_the_other_signed_primitives() {
    assert!(warnings("(math/mod 7 3)").is_empty());
    assert!(warnings("(math/mod 7 \"x\")")
        .iter()
        .any(|w| w.contains("mod")));
    assert!(warnings("(math/rem :a 3)")
        .iter()
        .any(|w| w.contains("rem")));
    assert!(warnings("(%vector-length 5)")
        .iter()
        .any(|w| w.contains("vector-length")));
    assert!(warnings("(string/substring \"hi\" \"a\" 1)")
        .iter()
        .any(|w| w.contains("string/substring") && w.contains("argument 2")));
    assert!(warnings("(%lt 1 :k)").iter().any(|w| w.contains("%lt")));
}

#[test]
fn reports_each_bad_argument() {
    // Both args provably wrong → two distinct warnings (one per position).
    let w = warnings("(math/mod \"a\" :b)");
    assert_eq!(w.len(), 2, "{:?}", w);
    assert!(w.iter().any(|s| s.contains("argument 1")));
    assert!(w.iter().any(|s| s.contains("argument 2")));
}

#[test]
fn nested_misuse_is_found() {
    // A wrong call buried inside an argument is still reported.
    let w = warnings("(%vector-length (cons (first 5) 2))");
    assert!(w.iter().any(|s| s.contains("first")));
}

#[test]
fn atoms_and_malformed_forms_do_not_panic() {
    for src in ["5", "foo", "\"s\"", ":k", "()", "(5 6 7)", "(first)"] {
        // No panic, and no spurious warning on a bare atom / non-symbol head /
        // missing argument.
        let _ = warnings(src);
    }
    assert!(warnings("(5 6 7)").is_empty()); // head isn't a symbol — no diagnostics
                                             // `(first)` is now an arity diagnostic (0 args; first needs 1).
    assert!(warnings("(first)")
        .iter()
        .any(|w| w.contains("first") && w.contains("expected 1")));
}

// ------------- Step 3: sigs sourced from NativeFn, closure inference --------------

/// The eight test cases below need real user-defined closures, which means
/// running a `defn` against the global table. The `Interp` builds the full
/// prelude (curated stdlib closures and all) on top of the primitive kernel
/// — exactly the surface a checker is supposed to see.
fn check_with_defs(defs: &[&str], src: &str) -> Vec<String> {
    let mut interp = crate::Interp::new();
    for d in defs {
        interp.eval_str(d).expect("def");
    }
    let form = crate::syntax::reader::read_one(&mut interp.heap, src).expect("parse expression");
    // Macro-expand so any prelude wrappers (defn → fn, etc.) are gone, like
    // `brood --check`/the `check` builtin do before calling check_form.
    let form = crate::eval::macros::macroexpand_all(&mut interp.heap, form, interp.root).unwrap();
    check_form(&interp.heap, form)
}

#[test]
fn primitive_sigs_are_read_from_native_fn() {
    // The point of Step 3: there is no parallel `primitive_sig` table.
    // The sig the checker uses for `string-length` *is* the one declared
    // next to its `Arity` in `builtins.rs`. If we ever drop the sig field
    // (or set it wrong), this catches it.
    let interp = crate::Interp::new();
    let sig = primitive_sig(&interp.heap, crate::core::value::intern("string/length"))
        .expect("string-length is a primitive");
    assert_eq!(sig.params, vec![Ty::of(Tag::Str)]);
    assert_eq!(sig.ret, Ty::of(Tag::Int));
    // The "no useful info" lane: a variadic any-arg primitive (str) returns
    // a Sig that param-overlaps every input, so it never warns.
    let any_sig =
        primitive_sig(&interp.heap, crate::core::value::intern("str")).expect("str is a primitive");
    assert_eq!(any_sig.rest, Some(Ty::ANY));
}

#[test]
fn file_defn_shadowing_a_builtin_wins_over_its_signature() {
    // A file's own `defn %bytes->list` supersedes the `%bytes->list` builtin (ADR-123: a def
    // always wins) — the checker must not type its calls with the builtin's
    // list-returning signature. This exact shape (the bintree bench, which
    // spelled it `check` before that builtin moved to `reflect/check`)
    // produced "+: argument 2 expects number, got list" plus a phantom arity
    // from the builtin's 1-arg Arity.
    let w = file_warnings(
        "(defn %bytes->list (node) (if (nil? node) 1 (+ 1 (%bytes->list (nth node 0)))))\n\
             (io/puts (%bytes->list nil))",
    );
    assert!(
        !w.iter().any(|s| s.contains("expects")),
        "stale builtin signature leaked into a shadowed call: {:?}",
        w
    );
    // Arity from the stale builtin must not leak either: the builtin `%bytes->list`
    // is 1-ary, the file's redefinition is 2-ary.
    let w = file_warnings("(defn %bytes->list (a b) (+ a b))\n(io/puts (%bytes->list 1 2))");
    assert!(
        !w.iter().any(|s| s.contains("argument")),
        "stale builtin arity leaked into a shadowed call: {:?}",
        w
    );
    // No over-suppression: the real builtin (not redefined) still warns.
    let w = file_warnings("(io/puts (+ 1 (%bytes->list (bytes 1))))");
    assert!(
        w.iter().any(|s| s.contains("expects number")),
        "the un-shadowed builtin's signature should still warn: {:?}",
        w
    );
}

#[test]
fn infers_a_straight_line_wrapper() {
    // (defn bump (x) (+ x 1)) → x : number (from +'s rest type). Not named `inc`:
    // this fixture is *evaluated*, and a shipped name is reserved (ADR-166).
    // So `(bump :k)` is a provable misuse.
    let w = check_with_defs(&["(defn bump (x) (+ x 1))"], "(bump :k)");
    assert!(
        w.iter().any(|s| s.contains("bump") && s.contains("number")),
        "expected a `bump :k` warning, got {:?}",
        w
    );
}

#[test]
fn inferred_return_type_propagates() {
    // (defn bump (x) (+ x 1)) returns the number `+` returns; feeding it into
    // `string-length` (wants string) is a provable misuse. (Not `inc` — the fixture
    // is evaluated, and shipped names are reserved, ADR-166.)
    let w = check_with_defs(&["(defn bump (x) (+ x 1))"], "(string/length (bump 1))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "expected a `string-length` warning, got {:?}",
        w
    );
}

#[test]
fn inferred_params_intersect_across_positions() {
    // (defn add (x y) (+ x y)) — both x and y at + positions → number.
    let w = check_with_defs(&["(defn add (x y) (+ x y))"], "(add \"a\" 2)");
    assert!(w.iter().any(|s| s.contains("add")), "got {:?}", w);
}

#[test]
fn same_file_caller_checked_against_inferred_return() {
    // The file being checked isn't loaded, so this exercises Pass 2.8's form-based inference:
    // `dbl` is inferred (same-file) to return a number, so `(string/length (dbl 5))` is caught
    // — a same-file caller now gets the checking a loaded-function caller already did.
    let w = file_warnings(
        "(defmodule t)\n(defn dbl (x) (+ x 1))\n(defn bad () (string/length (dbl 5)))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "same-file inferred return should flow to a caller: {w:?}"
    );
}

#[test]
fn same_file_forward_reference_resolves_via_fixpoint() {
    // Caller defined BEFORE callee — the bounded fixpoint still resolves `later`'s return.
    let w = file_warnings("(defmodule t)\n(defn bad () (+ 1 (later 1)))\n(defn later (x) (str x))");
    assert!(
        w.iter().any(|s| s.contains('+') && s.contains("number")),
        "a forward reference should resolve in the fixpoint: {w:?}"
    );
}

#[test]
fn same_file_reassigned_global_return_stays_dynamic() {
    // SOUNDNESS: a lazily-initialized global (nil default, reassigned to a table) must make
    // the returning function's return *dynamic*, not the stale `nil` — else a table use of
    // the result would false-flag. Guards Pass 2.8 against the earmuffed / reassigned-global
    // imprecision.
    let w = file_warnings(
        "(defmodule t)\n(def *g* nil)\n(defn getg () (when (nil? *g*) (def *g* (%table))) *g*)\n(defn u () (%table-get (getg) :k))",
    );
    assert!(
        !w.iter()
            .any(|s| s.contains("%table-get") && s.contains("argument")),
        "a reassigned global's return must stay dynamic (no false positive): {w:?}"
    );
}

#[test]
fn infers_a_tail_recursive_function_return_from_its_base_case() {
    // A self-recursive call in a branch position contributes ⊥ to the return union, so
    // `count-down`'s return infers from its base case `:done` (keyword) — feeding it to
    // `string-length` (wants string) is then a provable misuse. Before this, the self-call
    // made the return uninferrable and the misuse went uncaught.
    let w = check_with_defs(
        &["(defn count-down (n) (if (<= n 0) :done (count-down (- n 1))))"],
        "(string/length (count-down 5))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "a recursive fn's base-case return should flow to its caller: {w:?}"
    );
}

#[test]
fn recursive_inference_defers_when_the_base_case_is_unknown() {
    // SOUNDNESS: an accumulator-returning recursion (`acc` is an unconstrained param → the
    // base case is unknown) must infer an unknown return, never a spuriously-narrow one — so
    // a caller using its result in any way is NOT false-flagged.
    let w = check_with_defs(
        &["(defn sum-acc (xs acc) (if (empty? xs) acc (sum-acc (rest xs) (+ acc (first xs)))))"],
        "(string/length (sum-acc (list 1 2) 0))",
    );
    assert!(
        !w.iter().any(|s| s.contains("string/length")),
        "an unknown (param) base case must defer, not false-flag: {w:?}"
    );
}

#[test]
fn infers_a_multi_arity_return_as_the_union_of_its_arms() {
    // A multi-arity closure has no single param signature, but its return is the union of
    // each arm's tail — here `:one | :two`. Feeding that to `string-length` (wants string)
    // is a provable misuse. (Before, a multi-arity closure was skipped entirely.)
    let w = check_with_defs(
        &["(defn describe ((x) :one) ((x y) :two))"],
        "(string/length (describe 5))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "a multi-arity fn's union return should flow: {w:?}"
    );
}

#[test]
fn infers_a_variadic_return() {
    // A rest-param closure was skipped before; now its return (`(str a)` → string) flows, so
    // feeding it to `+` (wants a number) is caught.
    let w = check_with_defs(&["(defn joiner (a & xs) (str a))"], "(+ 1 (joiner \"x\"))");
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("number")),
        "a variadic fn's return should flow: {w:?}"
    );
}

#[test]
fn complex_closure_return_only_keeps_arity_checking() {
    // The return-only sig is params-less, but arity is checked independently (`arity_of`),
    // so a wrong-arity call to the multi-arity fn is still flagged — no regression.
    let w = check_with_defs(
        &["(defn describe ((x) :one) ((x y) :two))"],
        "(describe 1 2 3)",
    );
    assert!(
        w.iter()
            .any(|s| s.contains("describe") && s.contains("arg")),
        "arity checking must survive return-only inference: {w:?}"
    );
}

#[test]
fn does_not_infer_through_branches_or_lets() {
    // A body with `if`/complex `let` is *not* a single straight-line expression
    // — inference must skip it, leaving the closure untyped (no warning).
    // (A plain let-alias `(let (y x) call)` IS inferred — see below.)
    let w = check_with_defs(&["(defn maybe (x) (if (int? x) (+ x 1) x))"], "(maybe :k)");
    assert!(
        w.is_empty(),
        "if-branching bodies must not infer (so no warning): {:?}",
        w
    );
}

#[test]
fn infers_through_let_alias() {
    // `(let (y x) call)` where y is just a rename of closure param x:
    // the body is still one straight-line call — inference should work.
    let w = check_with_defs(
        &["(defn double (x) (let (y x) (* y 2)))"],
        "(string/length (double 3))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "let-alias wrapper should not block infer_sig: {:?}",
        w
    );
    // The param type is also inferred: `y` at number position → x : number.
    let w = check_with_defs(&["(defn double (x) (let (y x) (* y 2)))"], "(double :k)");
    assert!(
        w.iter()
            .any(|s| s.contains("double") && s.contains("number")),
        "let-alias: param type should propagate from callee: {:?}",
        w
    );
    // A non-param let (binding a computed value) isn't peeled by the precise
    // *parameter*-inferring tier — but the sound **return-only** tier still
    // infers `wrap`'s result as `number` (`wrap 3` = 8), so a real misuse of
    // that result is caught (`(string/length 8)` genuinely errors at runtime).
    let w = check_with_defs(
        &["(defn wrap (x) (let (y (+ x 1)) (* y 2)))"],
        "(string/length (wrap 3))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "return-only inference should type wrap's result as number: {:?}",
        w
    );
    // …and the *parameter* IS inferred here too: `(+ x 1)` is a `let`-binding RHS,
    // which always executes when `wrap` is called (it dominates the body), so `x`
    // genuinely must be a number — `(wrap :k)` errors at runtime. The unconditional-
    // demand tier (`collect_param_demands`) catches it. (A *guarded* use — `(+ x 1)`
    // inside an `if`/`cond`/`and`-tail — would stay unconstrained; see
    // `param_inference_skips_guarded_uses`.)
    let w = check_with_defs(&["(defn wrap (x) (let (y (+ x 1)) (* y 2)))"], "(wrap :k)");
    assert!(
        w.iter().any(|s| s.contains("wrap") && s.contains("number")),
        "let-RHS is an unconditional demand: param should be inferred number: {:?}",
        w
    );
}

#[test]
fn param_inference_from_unconditional_positions() {
    // A parameter passed *directly* to a known-sig callee in a position that always
    // runs is inferred, even when the top-level body isn't a single call.

    // (a) Nested call argument: `(+ x 1)` is an argument to `f` (both always run),
    // so `x : number` — a keyword arg genuinely errors.
    let w = check_with_defs(&["(defn g (x) (list (+ x 1)))"], "(g :k)");
    assert!(
        w.iter().any(|s| s.contains("g") && s.contains("number")),
        "nested-call arg is an unconditional demand: {:?}",
        w
    );

    // (b) `do` form: every form runs; the last demands `x : number`.
    let w = check_with_defs(&["(defn h (x) (do 1 (+ x 1)))"], "(h :k)");
    assert!(
        w.iter().any(|s| s.contains("h") && s.contains("number")),
        "a do-form body is unconditional: {:?}",
        w
    );

    // (c) A demand through an unknown/user callee's argument still fires (the demand
    // comes from the inner *known* callee, not the outer unknown one).
    let w = check_with_defs(
        &["(defn user-sink (v) v)", "(defn k (x) (user-sink (* x 2)))"],
        "(k :k)",
    );
    assert!(
        w.iter().any(|s| s.contains("k") && s.contains("number")),
        "demand flows from the inner known callee: {:?}",
        w
    );
}

#[test]
fn param_inference_skips_guarded_uses() {
    // The soundness guard: a param used only inside a branch / guard / short-circuit
    // tail is NOT constrained — those positions don't always execute, so a
    // differently-typed argument must never warn.

    // (a) `if` branch (classic type-test guard): `x` is number only in the then-arm.
    let w = check_with_defs(&["(defn f (x) (if (number? x) (+ x 1) x))"], "(f :k)");
    assert!(
        w.is_empty(),
        "guarded (if-branch) use must not constrain: {:?}",
        w
    );

    // (b) `and`/`or` tail: only the first operand is unconditional.
    let w = check_with_defs(&["(defn f (x) (or (cached? x) (+ x 1)))"], "(f :k)");
    assert!(w.is_empty(), "or-tail use must not constrain: {:?}", w);

    // (c) `when` body is conditional on its test.
    let w = check_with_defs(&["(defn f (x) (when (ready?) (+ x 1)))"], "(f :k)");
    assert!(w.is_empty(), "when-body use must not constrain: {:?}", w);

    // (d) `try` body deliberately exercises failures — never constrain from it.
    let w = check_with_defs(&["(defn f (x) (try (+ x 1) (catch _ 0)))"], "(f :k)");
    assert!(w.is_empty(), "try-body use must not constrain: {:?}", w);
}

#[test]
fn param_inference_respects_shadowing() {
    // An inner `let` that rebinds the parameter's name hides it: the `(+ x 1)` here
    // refers to the let's `x` (a fresh unknown value), NOT the parameter, so the
    // parameter stays unconstrained and `(f :k)` must not warn.
    let w = check_with_defs(&["(defn f (x) (let (x (something)) (+ x 1)))"], "(f :k)");
    assert!(
        w.is_empty(),
        "a shadowing let binder must exclude the param from demand collection: {:?}",
        w
    );
}

#[test]
fn earmuffed_global_types_as_unknown_not_its_default() {
    // A `*earmuffed*` global is dynamic by convention — declared with a `nil` default
    // but reassigned at runtime (e.g. `*project-root*`) — so the checker must NOT pin
    // it to `nil` and flag a string-demanding use. (Regression guard for the false
    // positive the sound param-inference tier would otherwise surface at, e.g.,
    // `(path-join *project-root* rel)` after its `nil?` guard.)
    let w = check_with_defs(&["(def *root* nil)"], "(string/length *root*)");
    assert!(
        w.is_empty(),
        "earmuffed global must type as unknown, not its default nil: {:?}",
        w
    );
    // A non-earmuffed global is still pinned to its value (unchanged behaviour): a
    // real disjoint use is still caught.
    let w = check_with_defs(&["(def plain-root nil)"], "(string/length plain-root)");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "a plain (non-earmuffed) global is still typed by its value: {:?}",
        w
    );
}

#[test]
fn return_only_inference_is_sound() {
    // The return type of a branchy/multi-step body is inferred (sound: it's a
    // union of the possible results), so misusing the *result* is caught…
    let w = check_with_defs(
        &["(defn pick (c) (if c 1 2))"],
        "(string/length (pick true))",
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "a numeric-returning branchy body's result misuse must warn: {w:?}"
    );
    // …but a parameter used as a number only *inside a guard* must NOT be
    // inferred as number — that's the guarded-use false positive full param
    // inference would create. `(g "x")` is valid (returns 0), so no warning.
    let w = check_with_defs(&["(defn g (x) (if (number? x) (+ x 1) 0))"], "(g \"x\")");
    assert!(
        w.is_empty(),
        "a guarded numeric use must not infer the parameter as number: {w:?}"
    );
    // A union result that *overlaps* the sink must not warn (int | string fed
    // to `+` — the int arm overlaps `number`).
    let w = check_with_defs(&["(defn u (c) (if c 1 \"s\"))"], "(+ 1 (u true))");
    assert!(
        w.is_empty(),
        "a result overlapping the expected type must not warn: {w:?}"
    );
    // Recursion terminates (the re-entry guard) and stays sound — no hang,
    // no spurious warning on a valid use.
    let w = check_with_defs(
        &["(defn rfac (n) (if (< n 1) 1 (* n (rfac (- n 1)))))"],
        "(+ 1 (rfac 5))",
    );
    assert!(
        w.iter().all(|s| !s.contains("expects")),
        "recursive-body return inference must stay sound: {w:?}"
    );
}

#[test]
fn does_not_infer_through_recursion() {
    // A self-recursive call has no fixed sig to read from — must skip,
    // even though the body is structurally a single call.
    let w = check_with_defs(&["(defn go (x) (go x))"], "(go :k)");
    assert!(w.is_empty(), "recursive defns must not infer: {:?}", w);
}

#[test]
fn skips_inference_for_variadic_or_optional_closures() {
    // A variadic-tail closure isn't a "fixed-arity straight-line" — skip.
    let w = check_with_defs(&["(defn vlist (& xs) (first xs))"], "(vlist 1 2 3)");
    assert!(w.is_empty(), "variadic defns must not infer: {:?}", w);
}

// ------------- Step 4: scope tracking + guard narrowing --------------

#[test]
fn let_binding_propagates_its_rhs_type() {
    // The RHS is a literal int — `(first x)` should flag, because x : int
    // shadows "unknown" in the body. (This is the basic let-tracking.)
    let w = warnings("(let (x 1) (first x))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 1")),
        "expected a `first x` warning where x : 1 (int singleton), got {:?}",
        w
    );
}

#[test]
fn let_binding_from_nested_call_propagates() {
    // RHS is a known primitive whose return type is int. So `x : int`,
    // and `(first x)` flags.
    let w = warnings("(let (x (string/length \"hi\")) (first x))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "expected a `first x` warning where x : int, got {:?}",
        w
    );
}

#[test]
fn let_binding_of_unknown_rhs_stays_silent() {
    // RHS is a variable (unknown), so x stays unknown — `(first x)` must
    // not warn. (No false positives from let-tracking.)
    let w = warnings("(let (x foo) (first x))");
    assert!(w.is_empty(), "got {:?}", w);
}

#[test]
fn inner_let_shadows_outer_binding() {
    // The outer x : int; the inner x : string. `(first x)` in the body
    // refers to the inner, which is a string — and `first` accepts list /
    // vector, disjoint from string. So a warning is still expected, but
    // the *narrowing message* must be "string", not "int". This is the
    // shadowing-correctness check (outer narrowing must not leak in).
    let w = warnings("(let (x 1) (let (x \"hi\") (first x)))");
    assert!(
        w.iter()
            .any(|s| s.contains("first") && s.contains("got \"hi\"")),
        "expected the inner string to be the source, got {:?}",
        w
    );
    assert!(
        // Outer `x` is the literal `1` → singleton `{1}`; if it leaked the
        // message would say "got 1" (B0 — was "got int").
        !w.iter().any(|s| s.contains("got 1")),
        "outer int must not leak through shadowing: {:?}",
        w
    );
}

#[test]
fn shadowing_with_unknown_rhs_clears_prior_narrowing() {
    // Outer x : int; inner x : <unknown var>. Inside the inner let, x is
    // unknown — `(first x)` must NOT warn (the outer narrowing must not
    // leak through the shadow).
    let w = warnings("(let (x 1) (let (x foo) (first x)))");
    assert!(w.is_empty(), "shadow must clear the prior type: {:?}", w);
}

#[test]
fn vector_let_bindings_are_recognised() {
    // The bindings container is a LIST (ADR-010) — the vector shape is a compile
    // error now, so the checker only ever sees this spelling.
    let w = warnings("(let (x 1) (first x))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 1")),
        "vector-form let bindings must populate the ctx: {:?}",
        w
    );
}

#[test]
fn guard_narrowing_lets_a_then_branch_flag_a_misuse() {
    // In the then-branch of `(if (int? x) …)`, x : int — `(first x)` flags.
    let w = warnings("(if (int? x) (first x) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "expected guard narrowing to flag (first x) when x : int, got {:?}",
        w
    );
}

#[test]
fn guard_narrowing_does_not_leak_into_the_else_branch() {
    // The else-branch narrows x to `not int`, which overlaps list / vector;
    // so `(first x)` must NOT warn there.
    let w = warnings("(if (int? x) nil (first x))");
    assert!(
        !w.iter().any(|s| s.contains("first")),
        "else branch must not have x narrowed to int: {:?}",
        w
    );
}

#[test]
fn negated_guard_flips_the_narrowing() {
    // (if (not (int? x)) …) — the then-branch narrows x to `not int`, the
    // else-branch to int.
    let w = warnings("(if (not (int? x)) nil (first x))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "the else of a negated guard must narrow to the inner type: {:?}",
        w
    );
}

#[test]
fn guards_for_number_and_list_unions_narrow_to_the_union() {
    // (if (number? x) (first x) …) — x : number = int|float in the then,
    // which is disjoint from list/vector, so `(first x)` flags.
    let w = warnings("(if (number? x) (first x) nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("first") && s.contains("number")),
        "number? must narrow to int|float: {:?}",
        w
    );
    // The list? guard should *not* warn in the then (list overlaps first's
    // expected type).
    let w = warnings("(if (list? x) (first x) nil)");
    assert!(
        !w.iter().any(|s| s.contains("first")),
        "list? must not produce a false positive on (first x): {:?}",
        w
    );
}

#[test]
fn and_narrows_the_then_branch_on_every_conjunct() {
    // A truthy `and` proves ALL conjuncts, so the second (and third) conjunct's
    // narrowing must reach the then-branch, not just the first (ADR-011 gap close).
    let w = warnings_expanded("(if (and (int? a) (string? b)) (+ 1 b) 0)");
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("string")),
        "the 2nd `and` conjunct should narrow b to string in the then-branch: {w:?}"
    );
    let w3 = warnings_expanded("(if (and (int? a) (int? b) (string? c)) (+ c 1) 0)");
    assert!(
        w3.iter().any(|s| s.contains("+") && s.contains("string")),
        "the 3rd `and` conjunct should narrow c to string: {w3:?}"
    );
}

#[test]
fn and_falsy_does_not_narrow_the_else_branch() {
    // A falsy `and` may have failed on any conjunct, so it proves NOTHING — the
    // else-branch must not be narrowed (would be a false positive).
    let w = warnings_expanded("(if (and (int? a) (string? b)) 0 (+ 1 b))");
    assert!(
        !w.iter().any(|s| s.contains("+")),
        "a falsy `and` must not narrow the else-branch: {w:?}"
    );
}

#[test]
fn or_same_var_narrows_both_branches() {
    // Every disjunct a biconditional guard over the same var: the then-branch is the
    // union (a truthy `or` ⇒ some disjunct holds), the else-branch its complement (a
    // falsy `or` ⇒ none hold). So `(string/length c)` in the else flags — c is not string.
    let w = warnings_expanded("(if (or (nil? c) (string? c)) 0 (string/length c))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "the else of an all-same-var `or` should narrow c to ¬(nil|string): {w:?}"
    );
    // But a valid use in either branch stays silent — `str` accepts anything, and the
    // then-branch is the union (nil|string), which overlaps everything `str` wants.
    let ok = warnings_expanded("(if (or (nil? c) (string? c)) (str c) (str c))");
    assert!(
        ok.is_empty(),
        "a valid `or`-guarded use must stay silent: {ok:?}"
    );
}

#[test]
fn or_over_different_vars_does_not_narrow() {
    // Disjuncts over *different* variables give no single-variable narrowing — the
    // else-branch must not flag a use of either (would be a false positive).
    let w = warnings_expanded("(if (or (nil? a) (string? b)) 0 (string/length a))");
    assert!(
        !w.iter().any(|s| s.contains("string/length")),
        "an `or` over different vars must not narrow: {w:?}"
    );
}

#[test]
fn non_guard_tests_dont_narrow() {
    // The test isn't a recognised type predicate, so x stays unknown in
    // both branches — `(first x)` must not warn.
    let w = warnings("(if (math/zero? x) (first x) (first x))");
    assert!(w.is_empty(), "non-tag-guard test must not narrow: {:?}", w);
}

#[test]
fn nested_guards_compose_their_narrowings() {
    // (if (number? x) (if (int? x) … (first x)) …) — in the inner else,
    // x is narrowed to `number ∩ ¬int` = float, which is still disjoint
    // from list/vector, so `(first x)` flags.
    let w = warnings("(if (number? x) (if (int? x) nil (first x)) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("float")),
        "nested guards must compose to float (= number ∩ ¬int): {:?}",
        w
    );
}

#[test]
fn let_bound_guard_narrows_when_used_as_an_if_test() {
    // The user-written shape `(let (cond (int? x)) (if cond …))` — Brood is
    // immutable, so `cond` faithfully reflects `(int? x)` until the let
    // ends. The guard-alias table maps `cond → (x, int)`, and the inner
    // `if cond` narrows x to int in the then-branch.
    let w = warnings("(let (cond (int? x)) (if cond (first x) nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "expected let-bound guard to flag (first x) in the then: {:?}",
        w
    );
}

#[test]
fn let_bound_guard_narrows_in_the_else_branch_too() {
    // Else-branch sees x as `not int`, which overlaps list / vector, so
    // no warning — same as the direct-test case.
    let w = warnings("(let (cond (int? x)) (if cond nil (first x)))");
    assert!(
        !w.iter().any(|s| s.contains("first")),
        "the else of a let-bound guard must narrow to ¬int, not int: {:?}",
        w
    );
}

#[test]
fn let_bound_guard_can_be_negated_in_the_if() {
    // `(if (not cond) …)` flips the narrowing — same as `(not (int? x))`.
    let w = warnings("(let (cond (int? x)) (if (not cond) nil (first x)))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "expected negation to flip the let-bound guard: {:?}",
        w
    );
}

#[test]
fn rebinding_the_guard_name_clears_the_alias() {
    // After `(let (cond <unknown>) …)` shadowing, `cond` no longer aliases
    // the int-guard, so `(if cond …)` must not narrow x.
    let w = warnings("(let (cond (int? x)) (let (cond foo) (if cond (first x) nil)))");
    assert!(w.is_empty(), "shadowing must drop the guard alias: {:?}", w);
}

#[test]
fn rebinding_to_a_non_guard_value_clears_the_alias() {
    // Same as above but with an int literal rather than an unknown var.
    let w = warnings("(let (cond (int? x)) (let (cond 1) (if cond (first x) nil)))");
    assert!(
        w.is_empty(),
        "shadowing with a non-guard value must drop the alias: {:?}",
        w
    );
}

#[test]
fn self_aliased_guard_is_not_recorded() {
    // `(let (x (int? x)) …)` shadows the outer x with a bool; the inner
    // body's `x` is the bool, not the original — narrowing the original
    // would be unsound (it's no longer reachable), so we must not record
    // the guard. (No assertion about a warning either way — the point is
    // we don't crash and don't introduce a stale alias.)
    let w = warnings("(let (x (int? x)) (if x x nil))");
    assert!(
        !w.iter().any(|s| s.contains("first")),
        "self-aliased guards must not propagate to inner uses: {:?}",
        w
    );
}

#[test]
fn let_inside_a_then_branch_can_shadow_a_narrowing() {
    // Outer narrowing: x : int. Inner shadow: x : string. The body now
    // sees x as string, so the narrowing message names string.
    let w = warnings("(if (int? x) (let (x \"hi\") (first x)) nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("first") && s.contains("got \"hi\"")),
        "shadow must override the guard narrowing: {:?}",
        w
    );
    assert!(
        !w.iter().any(|s| s.contains("got int")),
        "the int narrowing must not leak through the shadow: {:?}",
        w
    );
}

// ---------------- Step 4: arity + unbound-symbol diagnostics ----------------

#[test]
fn flags_too_few_arguments() {
    // `first` expects exactly 1; 0 is wrong.
    assert!(warnings("(first)")
        .iter()
        .any(|w| w.contains("first") && w.contains("expected 1") && w.contains("got 0")));
    // `string-length` expects exactly 1.
    assert!(warnings("(string/length)")
        .iter()
        .any(|w| w.contains("string/length") && w.contains("expected 1")));
}

#[test]
fn flags_too_many_arguments() {
    // `inc` is `exact(1)`; calling with 2 is wrong. This used to use `rem`, which moved to
    // `math` on 2026-08-27 — and `warnings()` builds a bare `Interp::new()` with no module
    // loaded, so a `math/…` name has no bound function for the ARITY check to read (the
    // curated sig supplies types only). A still-bare function keeps the test about what it
    // is about.
    assert!(warnings("(inc 1 2)")
        .iter()
        .any(|w| w.contains("inc") && w.contains("expected 1") && w.contains("got 2")));
}

#[test]
fn arity_message_handles_range_and_variadic() {
    // `%map-get` is `range(2, 3)` → "expected 2 to 3".
    assert!(warnings("(%map-get {})")
        .iter()
        .any(|w| w.contains("%map-get") && w.contains("2 to 3")));
    // `apply` is `at_least(2)` → "expected 2 or more"; 1 is too few.
    assert!(warnings("(apply f)")
        .iter()
        .any(|w| w.contains("apply") && w.contains("2 or more")));
}

#[test]
fn arity_pass_is_silent_for_correct_calls() {
    assert!(warnings("(first [1 2])")
        .iter()
        .all(|w| !w.contains("number of arguments")));
    assert!(warnings("(math/rem 7 3)")
        .iter()
        .all(|w| !w.contains("number of arguments")));
    // Variadic: any count is fine.
    for n in 0..=5 {
        let args = (0..n).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
        let w = warnings(&format!("(+ {})", args));
        assert!(
            w.iter().all(|s| !s.contains("number of arguments")),
            "(+ {}…) should not warn arity: {:?}",
            n,
            w
        );
    }
}

#[test]
fn flags_unbound_call_heads() {
    assert!(warnings("(frobnicate 1)")
        .iter()
        .any(|w| w.contains("unbound symbol: frobnicate")));
    assert!(warnings("(typo-name :hi)")
        .iter()
        .any(|w| w.contains("unbound symbol: typo-name")));
}

// ---- Operand / value-slot unbound symbols (whole-file mode only) --------

#[test]
fn flags_unbound_operand_of_a_known_call() {
    // `+` evaluates its args, so a bare unresolvable operand is unbound.
    let w = file_warnings("(defn f (x) (+ x typo))");
    assert!(
        w.iter().any(|m| m.contains("unbound symbol: typo")),
        "operand typo should be flagged: {:?}",
        w
    );
    // Through a primitive too (cons), nested under a body.
    let w = file_warnings("(defn g () (cons 1 nope))");
    assert!(
        w.iter().any(|m| m.contains("unbound symbol: nope")),
        "{:?}",
        w
    );
}

#[test]
fn flags_unbound_value_in_def_let_if_slots() {
    assert!(file_warnings("(def y zilch)")
        .iter()
        .any(|m| m.contains("unbound symbol: zilch")));
    assert!(file_warnings("(defn f () (let (a absent) a))")
        .iter()
        .any(|m| m.contains("unbound symbol: absent")));
    assert!(file_warnings("(defn f () (if missing 1 2))")
        .iter()
        .any(|m| m.contains("unbound symbol: missing")));
}

#[test]
fn operand_check_respects_scope_and_forward_refs() {
    // A forward reference to a later top-level def — file-global, not unbound.
    assert!(file_warnings("(defn a () (cons 1 (b)))\n(defn b () 2)")
        .iter()
        .all(|m| !m.contains("unbound")));
    // A param / let-bound name used as an operand — in scope, not unbound.
    assert!(file_warnings("(defn f (x) (+ x 1))")
        .iter()
        .all(|m| !m.contains("unbound")));
    assert!(file_warnings("(defn f () (let (y 1) (+ y 2)))")
        .iter()
        .all(|m| !m.contains("unbound")));
    // A prelude name as an operand resolves through the heap globals.
    assert!(file_warnings("(defn f () (map inc (list 1 2)))")
        .iter()
        .all(|m| !m.contains("unbound")));
}

#[test]
fn operand_check_is_off_for_bare_fragments() {
    // The single-form path (REPL / `(check 'form)`) stays lenient: a free
    // operand variable is ambiguous, not provably unbound — only call *heads*
    // are flagged there. (Guards the no-false-positives rule for fragments.)
    assert!(warnings("(first xs)")
        .iter()
        .all(|m| !m.contains("unbound")));
    assert!(warnings("(+ 1 foo)").iter().all(|m| !m.contains("unbound")));
    assert!(warnings("(let (x bar) (first x))")
        .iter()
        .all(|m| !m.contains("unbound")));
}

#[test]
fn flags_zero_arg_fn_passed_bare_to_an_output_sink() {
    // The `(print ansi-clear)`-for-`(print (ansi-clear))` slip: a bare
    // zero-arity global handed to print/println/str/format stringifies the
    // function (#<fn …>), never its result — silent today.
    for sink in &["print", "println", "str", "format"] {
        let w = check_with_defs(&["(defn home () \"\\e[H\")"], &format!("({} home)", sink));
        assert!(
            w.iter()
                .any(|m| m.contains("home: function used as a value")
                    && m.contains("did you mean (home)")),
            "{} should flag a bare zero-arg fn: {:?}",
            sink,
            w
        );
    }
}

#[test]
fn function_as_value_lint_is_quiet_on_the_correct_and_legitimate_shapes() {
    // Called correctly — no warning.
    assert!(
        check_with_defs(&["(defn home () \"\\e[H\")"], "(print (home))")
            .iter()
            .all(|m| !m.contains("function used as a value"))
    );
    // A fn that *takes* arguments is a plausible intentional callback value.
    assert!(check_with_defs(&["(defn f (x) x)"], "(print f)")
        .iter()
        .all(|m| !m.contains("function used as a value")));
    // A same-named *local* (not the global zero-arg fn) is left alone.
    assert!(
        check_with_defs(&["(defn home () 1)"], "(let (home 42) (print home))")
            .iter()
            .all(|m| !m.contains("function used as a value"))
    );
    // A plain value is fine.
    assert!(warnings("(print 42)")
        .iter()
        .all(|m| !m.contains("function used as a value")));
    // The lint is sink-scoped: passing a bare zero-arg fn elsewhere (a real
    // higher-order use) is not flagged.
    assert!(check_with_defs(&["(defn home () 1)"], "(map home [1 2])")
        .iter()
        .all(|m| !m.contains("function used as a value")));
}

#[test]
fn unbound_is_silent_for_in_scope_names() {
    // fn params don't look unbound when used as call heads or
    // referenced in the body.
    assert!(warnings("(fn (f) (f 1 2))")
        .iter()
        .all(|w| !w.contains("unbound")));
    // let bindings: same.
    assert!(warnings("(let (g (fn (x) x)) (g 1))")
        .iter()
        .all(|w| !w.contains("unbound")));
    // Syntactic keywords aren't bound but are never "unbound".
    for src in &["(do 1 2 3)", "(when true 1)", "(cond)", "(and)", "(or)"] {
        assert!(
            warnings(src).iter().all(|w| !w.contains("unbound")),
            "syntactic keyword must not be flagged unbound: {} → {:?}",
            src,
            warnings(src)
        );
    }
}

#[test]
fn unbound_is_silent_for_prelude_names() {
    // The prelude is loaded in our test heap (via Interp::new()), so
    // stdlib names resolve. `inc`, `list`, `int?`, `even?`, … are all fine.
    for src in &[
        "(inc 1)",
        "(list 1 2 3)",
        "(int? 5)",
        "(math/zero? 0)",
        "(map (fn (x) x) [1 2 3])",
    ] {
        assert!(
            warnings(src).iter().all(|w| !w.contains("unbound")),
            "prelude name must not be flagged unbound: {} → {:?}",
            src,
            warnings(src)
        );
    }
}

#[test]
fn unbound_roots_a_bare_name_against_the_current_compile_ns() {
    // The checker mirrors eval's `compile_ns` rooting (ADR-070): after `%in-ns`, a bare
    // name that resolves to `<ns>/name` is not flagged unbound. This is the REPL case —
    // `nest repl` enters the project's `:main` namespace so a bare project fn resolves at
    // the prompt without the advisory checker crying "unbound".
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(%in-ns 'ns1) (defn foo () 1)")
        .expect("defines ns1/foo");
    // `eval_str` resets `compile_ns` on return; re-establish it the way the REPL loop
    // process holds it across interactive `eval-string`s (which don't reset).
    interp
        .heap
        .set_compile_ns(Some(crate::core::value::intern("ns1")));

    // A bare `foo` roots to the bound `ns1/foo` → no unbound warning.
    let bound = reader::read_one(&mut interp.heap, "(foo)").expect("parse");
    let w_bound = check_form(&interp.heap, bound);
    assert!(
        w_bound.iter().all(|m| !m.contains("unbound")),
        "a bare `foo` should root to ns1/foo (bound) and not warn, got {w_bound:?}"
    );

    // A genuinely unbound bare name is still flagged (rooting only ever *finds*, never masks).
    let unbound = reader::read_one(&mut interp.heap, "(nope-xyz)").expect("parse");
    let w_unbound = check_form(&interp.heap, unbound);
    assert!(
        w_unbound
            .iter()
            .any(|m| m.contains("unbound") && m.contains("nope-xyz")),
        "a genuinely unbound name must still be flagged, got {w_unbound:?}"
    );
}

#[test]
fn file_globals_make_later_forms_see_earlier_defs() {
    // `check_file` accumulates top-level def names. Without that,
    // `(my-fn 1)` in form 2 would be flagged unbound — `my-fn` isn't in
    // the heap (no eval), only in the file.
    let interp = crate::Interp::new();
    let src = "(defn my-fn (x) (+ x 1))\n(my-fn 1)";
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
    let out = check_file(&mut heap, &forms);
    let msgs: Vec<_> = out.into_iter().map(|(_, m)| m).collect();
    assert!(
        msgs.iter().all(|m| !m.contains("unbound symbol: my-fn")),
        "file-local defns must shield later calls: {:?}",
        msgs
    );
}

#[test]
fn fn_params_with_rest_and_optional_dont_leak() {
    // The marker symbols `&`/`&optional` themselves are *not* binders;
    // the names that follow them are.
    assert!(warnings("(fn (x & ys) (cons x ys))")
        .iter()
        .all(|w| !w.contains("unbound")));
    assert!(warnings("(fn (x &optional (y 0)) (+ x y))")
        .iter()
        .all(|w| !w.contains("unbound")));
}

#[test]
fn defn_body_sees_its_params_in_scope() {
    // A user defn whose body references its params must not flag them as
    // unbound. (The `defn` macro hasn't been expanded — the CLI checks
    // un-expanded forms — so this tests the un-expanded surface path.)
    assert!(warnings("(defn my-fn (x y) (+ x y))")
        .iter()
        .all(|w| !w.contains("unbound")));
}

#[test]
fn arity_check_works_for_user_defns_in_a_real_interp() {
    // Once a defn is evaluated, its arity is derivable from its Closure.
    // `inc` (prelude) is `(defn inc (n) …)` → exact(1).
    let w = check_with_defs(&[], "(inc 1 2)");
    assert!(
        w.iter()
            .any(|s| s.contains("inc") && s.contains("expected 1")),
        "user defn arity should be enforced: {:?}",
        w
    );
}

// ---- Step 4 final pieces: %eq-as-guard + let-alias propagation --------
//
// `match` lowers `(match x (5 body) …)` to
// `(let (m__N x) (if (%eq m__N 5) (do body) …))`. To flag a misuse on
// `x` in `body` (where the literal pattern asserts x's type), the checker
// needs two pieces: (1) recognise `(%eq sym lit)` as a guard asserting
// `sym : type-of(lit)`; (2) when a `let` binds a name to another symbol,
// propagate narrowings between the two via the alias chain.

#[test]
fn match_literal_pattern_narrows_the_scrutinee() {
    // `(match x (5 (first x)))` — the literal-int pattern asserts x : int;
    // `(first x)` in the body must then flag. Goes through macroexpansion
    // because `match` is a `defmacro` whose pattern compiler lowers to
    // `let`+`if`+`%eq`; the checker's narrowing rides the lowered shape.
    let w = warnings_expanded("(match x (5 (first x)) (_ nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "match int-literal pattern should narrow x: {:?}",
        w
    );
}

#[test]
fn match_keyword_pattern_narrows_the_scrutinee() {
    // Mirror of the int case for a keyword literal. The scrutinee narrows to
    // the literal singleton `:foo`, so the diagnostic names that exact value.
    let w = warnings_expanded("(match x (:foo (first x)) (_ nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains(":foo")),
        "match keyword-literal pattern should narrow x: {:?}",
        w
    );
}

#[test]
fn eq_against_a_literal_is_a_guard() {
    // The mechanism that powers match: `(%eq m 5)` in a test position
    // narrows `m` to `:int` in the then-branch. (Symmetric — both
    // `(%eq m 5)` and `(%eq 5 m)` should narrow.)
    let w = warnings("(if (%eq m 5) (first m) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "%eq with sym + literal should narrow: {:?}",
        w
    );
    let w = warnings("(if (%eq 5 m) (first m) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "%eq with literal + sym (reversed) should narrow: {:?}",
        w
    );
}

#[test]
fn eq_between_two_variables_is_not_a_guard() {
    // Equality between two unknowns asserts nothing about either's type.
    // No false positive must fire on the body.
    let w = warnings("(if (%eq a b) (first a) nil)");
    assert!(
        w.iter().all(|s| !s.contains("first")),
        "%eq between two vars should not narrow: {:?}",
        w
    );
}

#[test]
fn eq_guard_does_not_narrow_the_else_branch() {
    // `(= m "x")` being *false* does NOT prove `m` isn't a string — it could
    // be another string. So the else-branch must not narrow `m` to `¬string`
    // and flag a valid `(string/length m)`. (Same then-only soundness as the
    // `and` guard.)
    let w = warnings(r#"(if (%eq m "x") :yes (string/length m))"#);
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "the else-branch of an `=`/`%eq` guard must not be narrowed: {w:?}"
    );
    // The then-branch must still narrow (sanity): `(= m 5)` true ⇒ m : int.
    let w = warnings("(if (%eq m 5) (first m) nil)");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "the then-branch must still narrow m to int: {w:?}"
    );
}

#[test]
fn let_alias_propagates_narrowing_in_both_directions() {
    // The match pattern compiler's exact shape: alias `m` to `x`, then
    // narrow `m` via a guard. The narrowing must flow back onto `x` so a
    // body that uses `x` (not `m`) still sees the asserted type.
    let w = warnings("(let (m x) (if (int? m) (first x) nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "let-alias should propagate narrowing from m to x: {:?}",
        w
    );
    // And the symmetric direction: narrow x, alias-narrows m.
    let w = warnings("(let (m x) (if (int? x) (first m) nil))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("int")),
        "let-alias should propagate narrowing from x to m: {:?}",
        w
    );
}

#[test]
fn shadowing_clears_an_alias() {
    // An inner let that rebinds an aliased name to something else breaks
    // the chain — the new binding is the new name's type, no alias.
    // `(let (m x) (let (m 5) (first m)))` flags the inner `(first m)`
    // because `m` is now int, but that's via the literal-type binding,
    // not the broken alias.
    let w = warnings("(let (m x) (let (m 5) (first m)))");
    assert!(
        w.iter().any(|s| s.contains("first") && s.contains("got 5")),
        "shadowed let should still warn on the inner int: {:?}",
        w
    );
    // The outer `x` must not be narrowed by the inner shadowing.
    let w = warnings("(let (m x) (let (m 5) (io/puts x)))");
    assert!(
        w.iter().all(|s| !s.contains("first")),
        "shadowing must not leak narrowing back to the original: {:?}",
        w
    );
}

// ---- callback-arity check over higher-order combinators (ADR-078) ----

#[test]
fn flags_a_named_callback_of_the_wrong_arity() {
    // `cons` is arity 2; `map` calls its callback with 1 arg → real bug.
    let w = warnings("(map cons nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("map") && s.contains("callback") && s.contains("cons")),
        "map should flag a 2-arg callback called with 1: {w:?}"
    );
}

#[test]
fn accepts_a_named_callback_of_the_right_arity() {
    // `inc` is arity 1 — exactly what `map` supplies. No warning.
    let w = warnings("(map inc nil)");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a correct-arity callback must not warn: {w:?}"
    );
    // A variadic callback (`+` accepts 1) is fine too.
    let w = warnings("(map + nil)");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a variadic callback must not warn: {w:?}"
    );
}

#[test]
fn flags_an_inline_fn_callback_of_the_wrong_arity() {
    // A 2-param inline fn passed where `map` calls it with 1 arg.
    let w = warnings("(map (fn (a b) a) nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("map") && s.contains("callback") && s.contains("the fn")),
        "map should flag a 2-arg fn: {w:?}"
    );
    // Correct arity — no warning.
    let w = warnings("(map (fn (a) a) nil)");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a 1-arg fn must not warn under map: {w:?}"
    );
}

#[test]
fn lambda_is_retired_and_hints_at_fn() {
    // ADR-162 retired the alias: `fn` is the only spelling. `lambda` is now an
    // ordinary unbound name — with a hint naming `fn`, so the mistake is one line to
    // fix. (It was a synonym for years, claimed removed by the docs for months.)
    let w = warnings("(map (lambda (a b) a) nil)");
    assert!(
        w.iter().any(|s| s.contains("lambda")),
        "a `lambda` head must be reported now: {w:?}"
    );
    assert_eq!(
        crate::eval::foreign_construct_hint("lambda"),
        Some("Brood spells `lambda` as `fn`: `(fn (x) …)`.")
    );
    // The `fn` spelling still gets the callback-arity check it always did.
    let w = warnings("(map (fn (a b) a) nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("map") && s.contains("callback")),
        "map should flag a 2-arg `fn` callback: {w:?}"
    );
}

#[test]
fn fn_form_is_not_unbound() {
    // Regression (originally found via the `lambda` alias, retired in ADR-162): a fn
    // head missing from SPECIAL_HEAD / is_syntactic_keyword made whole-file mode flag
    // the head AND its params as unbound — a false positive on valid code.
    let w = file_warnings("(def f (map (fn (x) (+ x 1)) (list 1 2 3)))");
    assert!(
        w.iter().all(|m| !m.contains("unbound symbol")),
        "an `fn` literal must not draw unbound-symbol warnings: {w:?}"
    );
}

// ---- gradual-assignment check: `(def x …)` vs a non-arrow `(sig x T)` ----
// (GradualTy's first consumer — ADR-024.)

#[test]
fn def_against_value_sig_flags_a_literal_mismatch() {
    // `(sig n int)` then `(def n "hello")` — a precise literal disjoint from
    // the declared type. stat(string) ⊄ int → flagged.
    let w = file_warnings(r#"(sig n int) (def n "hello")"#);
    assert!(
        w.iter().any(|m| m.contains("n: value of type \"hello\"")
            && m.contains("not assignable")
            && m.contains("int")),
        "a string literal assigned to an int-declared name must warn: {w:?}"
    );
}

#[test]
fn def_against_value_sig_catches_a_bounded_dynamic_global() {
    // The genuine GradualTy value-add: `label` is a redefinable global with a
    // declared type, so it's dynamic_within(string) — a bounded dynamic that
    // Option<Ty> can't represent. Assigning it to an int-declared name is
    // disjoint (string ∩ int = ⊥) → flagged.
    let w =
        file_warnings(r#"(sig count int) (sig label string) (def label "x") (def count label)"#);
    assert!(
        w.iter()
            .any(|m| m.contains("count: value of type string") && m.contains("int")),
        "a string-typed global assigned to an int-declared name must warn: {w:?}"
    );
}

#[test]
fn def_against_value_sig_defers_when_consistent_or_unknown() {
    // Every one of these is consistent (or dynamic) → no assignment warning.
    for src in [
        "(sig n int) (def n 5)",                          // exact
        "(sig m number) (def m 5)",                       // int <: number
        "(sig n int) (def n (+ 1 2))",                    // call result widened → defer
        "(sig n int) (def n some-unknown-global)",        // unknown → pure dynamic
        "(sig a int) (sig b number) (def b 5) (def a b)", // int <- number: ∩≠⊥ → defer
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("not assignable")),
            "a consistent/dynamic assignment must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn value_sig_resolves_cross_module_via_the_heap_store() {
    // Same technique as `overload_resolves_cross_module_via_the_heap_store`,
    // for the *value-type* `(sig name T)` declaration instead of an arrow:
    // `file_warnings`/`warnings` never evaluate, so they only ever exercise
    // the per-file `Ctx` path (`ctx.declared_value_ty`), never the heap-wide
    // `declared_sigs` store that makes a plain value sig visible cross-module
    // (`sigs::declared_heap_value_ty`). Simulate "module A declares `label`
    // and `count`; module B (fresh `Ctx`, no file-local knowledge of either)
    // assigns `count`'s value from `label`" by actually *evaluating* the
    // declarations first, then checking the `(def …)` form against an empty
    // `Ctx` — module B's starting point.
    let mut interp = crate::Interp::new();
    interp
        .eval_str(r#"(sig label string) (def label "x") (sig count int)"#)
        .expect("module A loads cleanly");

    let form = reader::read_one(&mut interp.heap, "(def count label)").expect("parse");
    let w = check_form(&interp.heap, form);
    assert!(
        w.iter()
            .any(|m| m.contains("count: value of type string") && m.contains("int")),
        "a string-typed global (declared cross-module) assigned to an \
             int-declared name (declared cross-module) must warn: {w:?}"
    );

    // And the consistent case: assigning a `string`-declared name from
    // `label` must stay silent, proving this isn't just an always-warn bug.
    interp
        .eval_str("(sig other string)")
        .expect("module A extension loads cleanly");
    let form2 = reader::read_one(&mut interp.heap, "(def other label)").expect("parse");
    let w2 = check_form(&interp.heap, form2);
    assert!(
        w2.iter().all(|m| !m.contains("not assignable")),
        "a string-typed global assigned to a string-declared name must not \
             warn: {w2:?}"
    );
}

#[test]
fn defmodule_declared_arrow_sig_seeds_return_type_check() {
    // Regression: `(sig f (-> B))` declared inside a `defmodule` block
    // didn't seed `check_def`'s body-vs-declared-return-type check.
    // Pass 2.5 (`annot::parse_sig_decl`) records a declared sig under the
    // symbol exactly as written in the un-expanded `(sig …)` form — bare
    // `f`. But `defn f` inside a `defmodule` expands to
    // `(def mod/f (fn …))`, so `check_def`'s seeding lookup
    // (`ctx.declared_sig(name)`) looks up the *qualified* `mod/f`, which
    // never matches the bare-keyed entry — the sig silently never seeds.
    // Needs `%register-sig` to have actually run (real `eval`, not just
    // parse+check) for the heap-wide fallback to have anything to read,
    // so this uses the same real-`Interp` + `eval_str` technique as the
    // cross-module tests, then re-checks the same source as a whole file
    // (mirrors what `nest check` does on an already-loaded project).
    let src = r#"
(defmodule gap-check-test-mod "doc")
(sig gap-check-test-f (-> string))
(defn gap-check-test-f ()
  "doc"
  42)
"#;
    let mut interp = crate::Interp::new();
    interp.eval_str(src).expect("module loads cleanly");

    let forms = reader::read_all(&mut interp.heap, src).expect("parse");
    let w = check_file(&mut interp.heap, &forms);
    assert!(
        w.iter()
            .any(|(_, m)| m.contains("gap-check-test-mod/gap-check-test-f")
                && m.contains("declared return type string")
                && m.contains("yields 42")),
        "a defmodule-qualified defn's body vs its declared return type \
             must warn, same as at the root namespace: {w:?}"
    );
}

#[test]
fn cross_module_value_sig_dependency_is_captured_for_incremental_cache() {
    // Regression for a gap the ADR-119 Phase 2 merge surfaced: `sigs::
    // declared_heap_value_ty` (ADR-124) originally read `heap.
    // declared_sig_value` directly instead of through `deps::
    // obs_declared_sig_value` — the *only* sanctioned read of global state
    // Phase 2's incremental-cache dependency capture relies on.
    //
    // Specifically isolates `check_def`'s own gate (the *name being
    // defined*, not the value referenced): `other`'s sig lives only on
    // the heap (module A), never in this file's own text, so
    // `ctx.declared_value_ty("other")` is `None` and `check_def` must
    // fall through to `declared_heap_value_ty` to know `other`'s type at
    // all. `other` never appears as a *value reference* anywhere in this
    // file (it's purely a def target), so — unlike a referenced global —
    // nothing else (the unbound-symbol check, arity lookups, …) would
    // incidentally record it via `deps::obs_global` either. If
    // `declared_heap_value_ty` bypasses the recorder, `other` never
    // enters this file's dep-keys at all, and a later edit to its sig is
    // invisible to the fingerprint — exactly the bug this guards.
    let mut interp = crate::Interp::new();
    interp
        .eval_str(r#"(sig label string) (def label "x") (sig other int)"#)
        .expect("module A loads cleanly");

    let forms = reader::read_all(&mut interp.heap, "(def other label)").expect("parse");
    let (warnings, dep_keys) = check_file_with_deps(&mut interp.heap, &forms);
    assert!(
        warnings
            .iter()
            .any(|(_, m)| m.contains("other: value of type string") && m.contains("int")),
        "int-declared `other` assigned a string must warn even with no \
             local (sig other …): {warnings:?}"
    );
    let fp1 = deps_fingerprint(&interp.heap, dep_keys);

    // Module A is "edited": other's declared type widens to accept a
    // string. This file's fingerprint must change — `other` is never
    // referenced here, only defined, so this changed fact can only reach
    // the fingerprint through check_def's own heap-wide lookup.
    interp
        .eval_str("(sig other string)")
        .expect("module A edit loads cleanly");
    let fp2 = deps_fingerprint(&interp.heap, dep_keys);
    assert_ne!(
        fp1, fp2,
        "a cross-module value-sig change on a pure def-target global must \
             flip the dependent file's fingerprint, or the incremental cache \
             would go stale"
    );
}

#[test]
fn declared_return_type_mismatch_is_flagged() {
    // Body yields an int (the integer-closed `+` rule: `int + int = int`),
    // declared return is string → disjoint → flagged.
    let w = file_warnings("(sig f (int -> string)) (defn f (x) (+ x 1))");
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type string") && m.contains("yields int")),
        "an int body vs a string return must warn: {w:?}"
    );
    // A literal body mismatch too.
    let w = file_warnings(r#"(sig g (int -> int)) (defn g (x) "hello")"#);
    assert!(
        w.iter()
            .any(|m| m.contains("g: declared return type int") && m.contains("\"hello\"")),
        "a string-literal body vs an int return must warn: {w:?}"
    );
}

#[test]
fn sig_call_site_wrong_literal_arg_is_flagged() {
    // A literal argument whose type is disjoint from the parameter's
    // declared `(sig …)` type is flagged at the call site (the precise `⊆`
    // path — a string literal where an int is wanted).
    let w = file_warnings(r#"(sig f (int -> int)) (defn f (x) x) (f "hello")"#);
    assert!(
        w.iter()
            .any(|m| m.contains("f: argument 1 expects int") && m.contains("\"hello\"")),
        "a string literal passed where int is declared must warn: {w:?}"
    );
    // A correct literal, and a dynamic (non-literal) argument, must not warn.
    for src in [
        "(sig g (int -> int)) (defn g (x) x) (g 1)",
        "(sig h (int -> int)) (defn h (x) x) (defn use-h (y) (h y))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("argument 1 expects")),
            "a consistent/dynamic argument must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn record_arg_missing_optional_field_does_not_warn() {
    // A record value that omits an *optional* field is a valid argument — the
    // arg-check relaxes the param to its required fields only, so the missing
    // `:age` (declared `(optional int)`) never misfires.
    let decl = "(sig f ((record :name string :age (optional int)) -> int)) (defn f (r) 0)";
    for good in ["(f {:name \"Ada\"})", "(f {:name \"Ada\" :age 30})"] {
        let w = file_warnings(&format!("{decl} {good}"));
        assert!(
            w.iter().all(|m| !m.contains("argument 1 expects")),
            "a record arg omitting an optional field must not warn ({good}): {w:?}"
        );
    }
    // But a wrong-typed *required* field is still caught (the sound part the
    // optional-drop preserves).
    let w = file_warnings(&format!("{decl} (f {{:name 42}})"));
    assert!(
        w.iter().any(|m| m.contains("f: argument 1 expects")),
        "a record arg with a wrong-typed required field must warn: {w:?}"
    );
}

#[test]
fn check_allow_type_mismatch_suppresses_call_and_return_lints() {
    // `(check-allow :type-mismatch …)` opts a deliberately-wrong subtree out
    // of BOTH the call-site argument lint and the declared-return lint —
    // the negative-test escape hatch (a `sig!` runtime contract is what the
    // wrapped code actually exercises).
    let w = file_warnings(
        r#"(sig f (int -> int)) (defn f (x) x) (check-allow :type-mismatch (f "hello"))"#,
    );
    assert!(
        w.iter().all(|m| !m.contains("argument 1 expects")),
        "check-allow :type-mismatch must suppress the call-site arg lint: {w:?}"
    );
    // The sig stays at top level (pass 2.5 reads sigs from top-level forms);
    // only the deliberately-wrong defn is wrapped — the contract_test shape.
    let w =
        file_warnings(r#"(sig g (int -> int)) (check-allow :type-mismatch (defn g (x) "nope"))"#);
    assert!(
        w.iter().all(|m| !m.contains("return type")),
        "check-allow :type-mismatch must suppress the return-type lint: {w:?}"
    );
}

#[test]
fn wider_sig_param_returned_as_narrower_is_flagged() {
    // A sig-typed param carries its exact contract type, so returning a
    // `number` param where the declared return is `int` is caught via the
    // precise `⊆` path — the first non-disjoint ("merely wider") mismatch the
    // disjointness checker structurally can't produce.
    let w = file_warnings("(sig f (number -> int)) (defn f (x) x)");
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type int") && m.contains("number")),
        "a number param returned as int must warn: {w:?}"
    );
    // Same or narrower param, and a param narrowed by a guard, must not warn.
    for src in [
        "(sig g (int -> int)) (defn g (x) x)",
        "(sig h (int -> number)) (defn h (x) x)",
        "(sig k (number -> int)) (defn k (x) (if (int? x) x 0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("return type")),
            "a consistent/narrowed param return must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn declared_return_type_defers_when_consistent() {
    // (+ x 1) : number — int <: number and number ∩ int ≠ ⊥, so neither of
    // these declared returns warns (a widened body never over-warns).
    for src in [
        "(sig inc (int -> int)) (defn inc (x) (+ x 1))",
        "(sig h (int -> number)) (defn h (x) (+ x 1))",
        "(sig id (int -> int)) (defn id (x) x)",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("return type")),
            "a consistent return must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn precise_body_inference_int_closed_ops() {
    // The "int int thing": `(* x x)` with `x : int` is precisely `int`, so a
    // body declared `(int -> int)` must NOT warn (the false-positive flood the
    // curated `number` result would otherwise produce).
    let w = file_warnings("(sig f (int -> int)) (defn f (x) (* x x))");
    assert!(
        w.iter().all(|m| !m.contains("return type")),
        "`(* int int)` declared int must not warn: {w:?}"
    );
    // A real lie still warns: an int body declared `string`.
    let w = file_warnings("(sig f (int -> string)) (defn f (x) (* x 2))");
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type string") && m.contains("yields int")),
        "an int body declared string must warn: {w:?}"
    );
}

#[test]
fn precise_body_inference_float_contagion() {
    // Float-contagion: `+ - * /` with a provably-float operand is precisely
    // `float` (int⊕float → float in the tower), and the always-float unary math
    // `sqrt`/`sin`/`cos`/`tan` is `float` even for a whole-number argument. Since
    // `float` is disjoint from `int`, a body declared `(int -> int)` doing float
    // arithmetic warns — the merely-wider mismatch the flat `number` sig missed.
    for src in [
        "(sig f (int -> int)) (defn f (x) (+ x 1.5))",
        "(sig f (int -> int)) (defn f (x) (* x 2.0))",
        "(sig f (int -> int)) (defn f (x) (math/sqrt x))",
        "(sig f (int -> int)) (defn f (x) (/ x 2.0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("f: declared return type int") && m.contains("yields float")),
            "a float body declared int must warn ({src}): {w:?}"
        );
    }
    // Sound-defer cases that must NOT warn: a float body declared `float` or
    // `number`, an all-int body (int-closed rule), and `/` on two ints — which
    // is genuinely `number` (`(/ 6 2)` → 3, `(/ 5 2)` → 2.5), so it can't be
    // pinned to `float` and defers rather than false-positive.
    for src in [
        "(sig f (int -> float)) (defn f (x) (+ x 1.5))",
        "(sig f (int -> number)) (defn f (x) (* x 2.0))",
        "(sig f (int -> int)) (defn f (x) (* x x))",
        "(sig f (int -> int)) (defn f (x) (/ x 2))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("return type")),
            "a consistent/deferred float-arithmetic body must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn path_narrowing_through_a_record_field_guard() {
    // `(if (int? (get r :age)) …)` narrows the *path* `(get r :age)` to `int`
    // in the then-branch — so feeding it to `string-length` (wants string) is
    // caught, the miss occurrence typing on bare symbols couldn't reach.
    let w = file_warnings("(defn f (r) (if (int? (get r :age)) (string/length (get r :age)) 0))");
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("got int")),
        "an int-narrowed path fed to string-length must warn: {w:?}"
    );
    // A **nested** path narrows too: `(get (get cfg :db) :port)`.
    let nested = file_warnings(
        "(defn n (cfg) (if (int? (get (get cfg :db) :port)) \
             (string/length (get (get cfg :db) :port)) 0))",
    );
    assert!(
        nested
            .iter()
            .any(|m| m.contains("string/length") && m.contains("got int")),
        "an int-narrowed nested path must warn: {nested:?}"
    );
    // Uses consistent with the narrowed type — and an unguarded access (wide
    // type) — must NOT warn.
    for src in [
        "(defn g (r) (if (int? (get r :age)) (+ 1 (get r :age)) 0))",
        "(defn h (r) (if (string? (get r :n)) (string/length (get r :n)) 0))",
        "(defn m (r) (string/length (get r :age)))",
        // else-branch use of a `¬string`-narrowed path must not misfire.
        "(defn k (r) (if (string? (get r :x)) :s (get r :x)))",
        // a *different* nested path than the one narrowed must not warn.
        "(defn p (c) (if (int? (get (get c :db) :port)) (string/length (get (get c :web) :h)) 0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "a consistent/unguarded path use must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn path_narrowing_through_index_paths() {
    // `(nth t 0)` / `(first t)` / `(second …)` / `(third …)` narrow like a
    // field path: an int-narrowed index fed to `string-length` is caught.
    for src in [
        "(defn f (t) (if (int? (nth t 0)) (string/length (nth t 0)) 0))",
        "(defn f (t) (if (int? (first t)) (string/length (first t)) 0))",
        // mixed field + index path.
        "(defn f (r) (if (int? (nth (get r :xs) 0)) (string/length (nth (get r :xs) 0)) 0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("string/length") && m.contains("got int")),
            "an int-narrowed index path must warn ({src}): {w:?}"
        );
    }
    // A *different* index than the one narrowed must not warn (index-specific),
    // and a consistent use must not warn.
    for src in [
        "(defn f (t) (if (int? (nth t 0)) (string/length (nth t 1)) 0))",
        "(defn f (t) (if (int? (nth t 0)) (+ 1 (nth t 0)) 0))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "a different/consistent index use must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn path_narrowing_refines_base_record_type_into_calls() {
    // A path guard refines `base`'s *record type* in the then-branch, so it
    // flows into a call: `r` proven `{age: int}` passed where `{age: string}`
    // is wanted is caught (record disjointness on a conflicting required field).
    let decl = "(sig f ((record :age string) -> int)) (defn f (r) 0)";
    let bad = file_warnings(&format!(
        "{decl} (defn g (r) (if (int? (get r :age)) (f r) 0))"
    ));
    assert!(
        bad.iter()
            .any(|m| m.contains("f: argument 1 expects") && m.contains("got")),
        "a base refined to a conflicting record must warn at the call: {bad:?}"
    );
    // Matching field type, and an unguarded pass, must NOT warn.
    let okdecl = "(sig h ((record :age int) -> int)) (defn h (r) 0)";
    for src in [
        format!("{okdecl} (defn g (r) (if (int? (get r :age)) (h r) 0))"),
        format!("{okdecl} (defn g (r) (h r))"),
    ] {
        let w = file_warnings(&src);
        assert!(
            w.iter().all(|m| !m.contains("argument 1 expects")),
            "a matching/unguarded record arg must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn overload_call_matching_no_arm_is_flagged() {
    // (sig f (and (int -> int) (bool -> bool))): a call whose argument is
    // disjoint from *every* arm's domain is flagged (ADR-116 completion).
    let decl = "(sig f (and (int -> int) (bool -> bool))) \
                    (defn f (x) (if (int? x) (+ x 1) (not x)))";
    let bad = file_warnings(&format!(r#"{decl} (def c (f "hello"))"#));
    assert!(
        bad.iter()
            .any(|m| m.contains("f: no clause accepts these arguments")),
        "an arg matching no arm must warn: {bad:?}"
    );
    // An arg that matches *some* arm, and an unknown arg, must NOT warn.
    for src in [
        "(def a (f 5))",      // int → arm 1
        "(def b (f true))",   // bool → arm 2
        "(defn g (y) (f y))", // unknown arg → defer
    ] {
        let w = file_warnings(&format!("{decl} {src}"));
        assert!(
            w.iter().all(|m| !m.contains("no overload clause")),
            "a matching/unknown arg must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn check_allow_suppresses_targeted_lints() {
    // A `(check-allow :non-tail-recursion …)` wrapper silences the non-tail
    // lint for the wrapped defn — but only that category, and only inside it.
    let non_tail = "(defn f (n) (if (< n 1) 0 (+ 1 (f (- n 1)))))";
    let w = file_warnings(non_tail);
    assert!(
        w.iter().any(|m| m.contains("non-tail position")),
        "unwrapped non-tail recursion must warn: {w:?}"
    );
    let w = file_warnings(&format!("(check-allow :non-tail-recursion {non_tail})"));
    assert!(
        w.iter().all(|m| !m.contains("non-tail position")),
        "check-allow :non-tail-recursion must suppress: {w:?}"
    );
    // A mismatched category does NOT suppress (no silent blanket opt-out).
    let w = file_warnings(&format!("(check-allow :unreachable-clause {non_tail})"));
    assert!(
        w.iter().any(|m| m.contains("non-tail position")),
        "a mismatched category must not suppress the non-tail lint: {w:?}"
    );
    // Same for the redundant-`match`-clause lint.
    let dup = "(defn g (x) (match x (1 :a) (1 :b) (_ :z)))";
    assert!(
        file_warnings(dup)
            .iter()
            .any(|m| m.contains("unreachable clause")),
        "unwrapped duplicate clause must warn"
    );
    let wrapped = "(defn g (x) (check-allow :unreachable-clause (match x (1 :a) (1 :b) (_ :z))))";
    assert!(
        file_warnings(wrapped)
            .iter()
            .all(|m| !m.contains("unreachable clause")),
        "check-allow :unreachable-clause must suppress the redundancy lint"
    );
}

#[test]
fn precise_body_inference_control_flow() {
    // `(if (> x 0) x "neg")` yields `int | string`, which ⊄ int → must warn
    // (precise control-flow inference: both branches pin a type).
    let w = file_warnings(r#"(sig f (int -> int)) (defn f (x) (if (> x 0) x "neg"))"#);
    assert!(
        w.iter()
            .any(|m| m.contains("f: declared return type int") && m.contains("\"neg\"")),
        "an `int | string` body declared int must warn: {w:?}"
    );
    // A branchy body that stays within the declared type must NOT warn.
    let w = file_warnings("(sig f (int -> int)) (defn f (x) (if (> x 0) x 0))");
    assert!(
        w.iter().all(|m| !m.contains("return type")),
        "an all-int branchy body declared int must not warn: {w:?}"
    );
}

#[test]
fn precise_body_inference_defers_on_uncertainty() {
    // A body ending in a call to an un-sig'd local/global is unknown → defer,
    // never warn (graceful degradation keeps the check false-positive-clean).
    for src in [
        // an un-sig'd file-global call
        "(defn helper (x) x) (sig f (int -> int)) (defn f (x) (helper x))",
        // an un-sig'd let-bound local call
        "(sig f (int -> int)) (defn f (x) (let (g (fn (y) y)) (g x)))",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("return type")),
            "an unknown-result body must defer ({src}): {w:?}"
        );
    }
}

#[test]
fn argument_check_uses_the_full_gradual_relation() {
    // Gating B1 (docs/type-gating.md): the arg check now runs the gradual
    // relation, so a *merely-wider precise* argument is caught (a `number`
    // sig-param passed where `int` is wanted) — closing the return/arg
    // asymmetry.
    let w = file_warnings(
        "(sig wants-int (int -> int)) (defn wants-int (n) n) \
             (sig f (number -> int)) (defn f (x) (wants-int x))",
    );
    assert!(
        w.iter()
            .any(|m| m.contains("wants-int: argument 1 expects int") && m.contains("got number")),
        "a merely-wider precise argument must warn: {w:?}"
    );
    // But B0 keeps it sound: a literal argument is a faithful singleton, so
    // `200` passed where `(or 200 404 500)` is wanted does NOT false-positive.
    let w = file_warnings("(sig g ((or 200 404 500) -> int)) (defn g (c) c) (defn u () (g 200))");
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a literal in the accepted set must not warn: {w:?}"
    );
    // And a *dynamic* argument (a call result) still defers on `∩` — only a
    // provably-disjoint one warns, never a merely-wider one.
    let w = file_warnings(
        "(sig produce (int -> number)) (defn produce (n) n) \
             (sig h (int -> int)) (defn h (n) n) (defn top () (h (produce 3)))",
    );
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a dynamic (call-result) argument must defer, not over-warn: {w:?}"
    );
}

#[test]
fn undeclared_global_current_type_gates_its_use() {
    // Gap A (docs/type-gating.md): an *undeclared* global defined exactly once
    // by `(def g 5)` gets its inferred current-image type (`int`), so misusing
    // it is caught — via `dynamic_within` (the `∩` relation), reload-safe.
    let w = file_warnings("(def g 5) (defn f () (string/length g))");
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("got 5")),
        "an undeclared int global misused must warn: {w:?}"
    );
    // Consistent use, a redefined (ambiguous) global, and a function global
    // must NOT warn.
    for src in [
        "(def g 5) (defn f () (+ 1 g))", // int used as int
        "(def g 5) (def g \"s\") (defn f () (string/length g))", // redefined → dynamic
        "(defn g (x) x) (defn f () (+ 1 (g 2)))", // function global, not a value
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "a consistent/ambiguous/function global must not warn ({src}): {w:?}"
        );
    }
}

#[test]
fn cross_file_undeclared_global_gates_via_loaded_image() {
    // Cross-file Gap A: an undeclared global defined in one place (loaded into
    // the image) is typed from its heap value where it's used elsewhere — the
    // same mechanism `infer_sig` uses for functions. `check_with_defs` evals
    // the def, then checks a separate form (the cross-context path).
    let w = check_with_defs(&["(def gg 5)"], "(string/length gg)");
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("got 5")),
        "a cross-file undeclared int global misused must warn: {w:?}"
    );
    // A **dynamic variable** must be excluded — its heap value is only the
    // default; `binding` rebinds it to any type, so typing a use against the
    // default would false-positive. `(binding (*dv* "s") (string/length *dv*))`
    // is valid and must NOT warn.
    let w = check_with_defs(
        &["(defdyn *dv* 0)"],
        "(binding (*dv* \"s\") (string/length *dv*))",
    );
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a dynamic variable must stay unknown, not be typed from its default: {w:?}"
    );
    // A function global isn't gated as a value (its arrow is handled by sig_of).
    let w = check_with_defs(&["(defn ff (x) x)"], "(+ 1 ff)");
    assert!(
        w.iter().all(|m| !m.contains("expects")),
        "a function global must not be gated as a plain value: {w:?}"
    );
}

#[test]
fn declared_global_type_flows_into_value_position() {
    // `(sig g int)` makes `g`'s declared type visible where it's used, so a
    // disjoint use is caught — even though `g` is a redefinable global.
    let w = file_warnings("(sig g int) (def g 5) (def r (string/length g))");
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("int")),
        "a declared int global used where a string is wanted must warn: {w:?}"
    );
    // A compatible use defers (int ⊆ number).
    let w = file_warnings("(sig g int) (def g 5) (def r (+ 1 g))");
    assert!(
        w.iter().all(|m| !m.contains("expects number")),
        "a declared int global is fine for +: {w:?}"
    );
}

#[test]
fn unknown_module_qualified_name_is_not_unbound() {
    // A qualified reference whose module isn't loaded — defined dynamically
    // (`%load-string`, a required temp module) or in a file a single-file check
    // didn't load — can't be proven unbound, so it's left alone.
    for src in [
        "(some-unloaded-mod/thing 1)",
        "(a/b/c/deep-thing 1)",
        "(+ 1 other-mod/value)",
    ] {
        let w = file_warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("unbound symbol")),
            "an unknown-module qualified name must not be flagged ({src}): {w:?}"
        );
    }
    // But a typo in a *known* module (some `mod/*` is loaded) is still flagged:
    // requiring `io` makes `io/` a known prefix. `io` and not `test`, deliberately — a
    // lean (`--no-default-features`) runtime embeds no dev modules, so `(require-one 'test)`
    // resolves to nothing there and the assertion vanished with it. A CORE module keeps
    // the test about the checker instead of about the build's feature set.
    let w = file_warnings("(io/no-such-fn 1)");
    assert!(
        w.iter()
            .any(|m| m.contains("unbound symbol: io/no-such-fn")),
        "a typo in a known module must still be flagged: {w:?}"
    );
}

#[test]
fn ki17_qualified_reference_auto_requires_so_no_unrequired_warning() {
    // KI-17 is OBSOLETE since the ADR-227 follow-up: a qualified reference `mod/name`
    // now *infers* `(require-one 'mod)`, so "a reference to an unrequired module" can no
    // longer occur — there is no unrequired module to reference. The lint is a permanent
    // no-op, so a qualified reference draws NO "unrequired module" warning regardless of
    // the reachability set (empty or populated), and NO "unbound" (the reference resolves).
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(defmodule ki17mod \"m\")\n(defn foo (x) x)")
        .expect("module loads");
    let forms = crate::syntax::reader::read_all(&mut interp.heap, "(defn go (x) (ki17mod/foo x))")
        .expect("parse");

    // Empty reachability set — once the flag for KI-17, now silent.
    let warned = crate::types::check::check_file_ext(&mut interp.heap, &forms, &[]);
    assert!(
        warned.iter().all(|(_, m)| !m.contains("unrequired module")),
        "KI-17 is obsolete — no unrequired-module warning is expected, got {warned:?}"
    );
    assert!(
        warned.iter().all(|(_, m)| !m.contains("unbound symbol")),
        "the qualified reference resolves — it is not 'unbound': {warned:?}"
    );

    // The module in the reachability set — also silent (unchanged).
    let ok =
        crate::types::check::check_file_ext(&mut interp.heap, &forms, &["ki17mod".to_string()]);
    assert!(
        ok.iter().all(|(_, m)| !m.contains("unrequired module")),
        "expected silence when the module is reachable, got {ok:?}"
    );
}

#[test]
fn ki17_alias_clause_feeds_the_require_closure() {
    // Regression (generative fuzzer find): `(:alias mod :as x)` *loads* `mod` (it
    // `require`s it, then adds the `x/` prefix), so `module_direct_requires` must report
    // `mod` as a direct dependency — else a file that `:alias`es a module and also names
    // it qualified (or via the alias, which macro-expands to `mod/…`) false-positives.
    let mut interp = crate::Interp::new();
    let forms = crate::syntax::reader::read_all(
        &mut interp.heap,
        "(defmodule c \"c\" (:use ua) (:use-internals ub) (:alias uc :as x))\n(defn use () 1)",
    )
    .expect("parse");
    let (own, deps) = crate::types::check::module_direct_requires(&interp.heap, &forms);
    assert_eq!(own.as_deref(), Some("c"));
    for m in ["ua", "ub", "uc"] {
        assert!(
            deps.iter().any(|d| d == m),
            "{m} should be a direct require (deps = {deps:?})"
        );
    }
}

#[test]
fn unexpandable_macro_calls_dont_false_flag() {
    // A file-local macro the checker can't expand: its arguments are opaque
    // syntax. (a) A macro that `def`s its symbol arg — the name must not look
    // unbound later. (b) A macro that splices an arg into a binder — the
    // spliced names must not look unbound.
    let a = file_warnings("(defmacro mk (n) `(def ~n (fn (x) x))) (mk qf) (qf 5)");
    assert!(
        a.iter().all(|m| !m.contains("unbound symbol")),
        "a macro-defined name must not look unbound: {a:?}"
    );
    let b = file_warnings("(defmacro wp (v & body) `(let ((a b) ~v) ~@body)) (wp [1 2] (+ a b))");
    assert!(
        b.iter().all(|m| !m.contains("unbound symbol")),
        "names a macro splices into a binder must not look unbound: {b:?}"
    );
    // A genuine typo under a *known* (arg-evaluating) callee is still flagged.
    let c = file_warnings("(io/puts (genuine-typo 5))");
    assert!(
        c.iter().any(|m| m.contains("unbound symbol: genuine-typo")),
        "a real unbound call head must still be flagged: {c:?}"
    );
}

#[test]
fn transient_is_a_valid_count_and_contains_arg() {
    // count/contains? dispatch to transient-* kernel hooks at runtime, so a live
    // transient is a valid argument — the sigs must admit Tag::Transient. (`length` was
    // listed here too and does not exist; see `sigs.rs`.)
    for src in ["(count (transient {}))", "(contains? (transient {}) :k)"] {
        let w = warnings(src);
        assert!(
            w.iter().all(|m| !m.contains("expects")),
            "transient must be accepted by {src}: {w:?}"
        );
    }
    // A genuinely wrong arg (a number) is still flagged — the domain stays tight.
    assert!(warnings("(count 5)").iter().any(|m| m.contains("count")));
}

#[test]
fn multi_arity_fn_clause_params_are_bound() {
    // Regression: `check_fn` read a multi-arity fn's first clause as a param
    // list, so a param used only in a *later* clause looked unbound — a false
    // positive.
    let w = file_warnings("(def g (fn ((a) (* a 2)) ((a b) (+ a b))))");
    assert!(
        w.iter().all(|m| !m.contains("unbound symbol")),
        "multi-arity fn clause params must not look unbound: {w:?}"
    );
    // `defn` (which expands to `(def name (fn …))`) too.
    let w = file_warnings("(defn h ((a) a) ((a b) (+ a b)))");
    assert!(
        w.iter().all(|m| !m.contains("unbound symbol")),
        "defn: {w:?}"
    );
}

#[test]
fn self_recursive_let_bound_closure_is_bound() {
    // Regression: a `let`-bound `fn`/`lambda` that calls its own binding name
    // resolves at runtime (the closure captures the frame, late-binds on call),
    // but the checker flagged the self-reference unbound. Pre-binding fn-valued
    // let names fixes it — for `let` and `let*`, `fn` and `lambda`.
    let w = file_warnings("(defn t () (let (fac (fn (n) (if (= n 0) 1 (fac n)))) (fac 5)))");
    assert!(
        w.iter().all(|m| !m.contains("unbound symbol: fac")),
        "self-recursive let closure must not look unbound: {w:?}"
    );
    // But an *eager* forward reference in a non-closure RHS still surfaces.
    let w = file_warnings("(defn t () (let (a undefined-thing b 1) a))");
    assert!(
        w.iter()
            .any(|m| m.contains("unbound symbol: undefined-thing")),
        "an eager forward/undefined reference must still be flagged: {w:?}"
    );
}

#[test]
fn reduce_and_fold_expect_a_two_arg_callback() {
    // reduce/fold call `(f acc x)` — 2 args. A 1-arg callback is wrong.
    let w = warnings("(reduce (fn (a) a) 0 nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("reduce") && s.contains("callback")),
        "reduce should flag a 1-arg callback: {w:?}"
    );
    let w = warnings("(fold inc 0 nil)");
    assert!(
        w.iter()
            .any(|s| s.contains("fold") && s.contains("callback")),
        "fold should flag a 1-arg callback (inc): {w:?}"
    );
    // A correct 2-arg callback is silent.
    let w = warnings("(reduce (fn (a b) a) 0 nil)");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a 2-arg callback must not warn under reduce: {w:?}"
    );
}

#[test]
fn callback_arity_is_skipped_when_unknown() {
    // A multi-arity lambda accepts 1 *and* 2 — must not warn (we bail rather
    // than risk a false positive).
    let w = warnings("(map (fn ((a) a) ((a b) a)) nil)");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "multi-arity lambda must be skipped: {w:?}"
    );
    // A locally-bound callback has unknown arity here — skip.
    let w = warnings("(fn (f) (map f nil))");
    assert!(
        w.iter().all(|s| !s.contains("callback")),
        "a local callback must be skipped: {w:?}"
    );
}

// ---- element types flow through first/last/nth (ADR-078 slice 2) ----

#[test]
fn first_of_a_string_vector_is_not_a_number() {
    // `(first ["a" "b"])` : string | nil — disjoint from number → flagged.
    let w = warnings(r#"(+ 1 (first ["a" "b"]))"#);
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("\"a\"")),
        "expected a number/string mismatch from the element type: {w:?}"
    );
}

#[test]
fn first_of_an_int_vector_is_a_number() {
    // `(first [10 20])` : int | nil — overlaps number → no warning.
    let w = warnings("(+ 1 (first [10 20]))");
    assert!(
        w.iter().all(|s| !s.contains("expects number")),
        "an int element must not warn against +: {w:?}"
    );
}

#[test]
fn list_constructor_carries_its_element_type() {
    // `(list "a" "b")` : list<string>, so `(first …)` is string|nil.
    let w = warnings(r#"(+ 1 (first (list "a" "b")))"#);
    assert!(
        w.iter().any(|s| s.contains("+") && s.contains("\"a\"")),
        "(list …) element type should flow to first: {w:?}"
    );
}

#[test]
fn heterogeneous_or_unknown_elements_do_not_warn() {
    // Mixed elements → int|string element; first → int|string|nil, which
    // overlaps number → no false positive.
    let w = warnings(r#"(+ 1 (first [1 "a"]))"#);
    assert!(
        w.iter().all(|s| !s.contains("expects number")),
        "a heterogeneous element type must not warn: {w:?}"
    );
    // first of an unknown (variable) sequence → unknown → no warning.
    let w = warnings("(fn (xs) (+ 1 (first xs)))");
    assert!(
        w.iter().all(|s| !s.contains("expects number")),
        "an unknown sequence must not warn: {w:?}"
    );
}

// ---- `and`-guard narrowing in an `if` test (the match-lowering fix) ----

#[test]
fn and_guard_narrows_in_the_then_branch() {
    // `(and (int? x) …)` as an `if` test must narrow `x` to int in the then
    // branch — so a use that would mismatch the *original* type is suppressed
    // (here `x` is a string, narrowed to never → the `+` use is unreachable).
    let w = warnings_expanded(r#"(let (x "s") (if (and (int? x) true) (+ x 1) 0))"#);
    assert!(
        w.iter().all(|s| !s.contains("expects number")),
        "an `and` guard should narrow x in the then branch: {w:?}"
    );
}

#[test]
fn matching_a_list_against_a_vector_pattern_is_not_flagged() {
    // The match compiler lowers a vector pattern to
    // `(if (and (vector? m) (= (%vector-length m) 2)) (… (%vector-ref m i) …) …)`.
    // With `(list 1 2)` now typed `list<int>`, the guarded `vector-ref` must
    // not be flagged — the `and` guard narrows `m` to a vector (→ never here).
    let w = warnings_expanded("(match (list 1 2) ([a b] :vec) (_ :not-vec))");
    assert!(
        w.iter()
            .all(|s| !s.contains("vector-ref") && !s.contains("vector-length")),
        "a list matched against a vector pattern must not warn: {w:?}"
    );
}

#[test]
fn and_guard_does_not_narrow_the_else_branch() {
    // A falsy `(and (vector? m) …)` does NOT imply `m` isn't a vector — a
    // *later* conjunct may have failed. So the else-branch must keep `m`'s
    // full type; flagging a vector op there would be a false positive.
    let w = warnings_expanded(
        "(fn (m) (if (and (vector? m) (%eq (%vector-length m) 2)) \
                         (%vector-ref m 0) (%vector-ref m 0)))",
    );
    assert!(
        w.iter().all(|s| !s.contains("vector-ref")),
        "the else-branch of an `and` guard must not be narrowed: {w:?}"
    );
    // The then-branch still narrows (sanity: the guard didn't go silent).
    let w = warnings_expanded(r#"(fn (m) (if (and (int? m) true) (string/length m) 0))"#);
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "the then-branch should still narrow m to int: {w:?}"
    );
}

#[test]
fn or_guard_does_not_falsely_narrow() {
    // `or` must NOT narrow from its first operand (a truthy `or` implies
    // nothing about it). `(or (int? x) true)` is always true, so the then
    // branch keeps `x`'s full (string) type — and a genuine misuse there is
    // still seen. (Guards against the `and`-fix over-reaching into `or`.)
    let w = warnings_expanded(r#"(let (x "s") (if (or (int? x) true) (string/length x) 0))"#);
    assert!(
        w.iter().all(|s| !s.contains("expects")),
        "a correct use under an `or` guard must not warn: {w:?}"
    );
}

// ---- parametric HOF result types — map / filter (ADR-078, Option B) ----

#[test]
fn map_result_flows_the_callback_return() {
    // `(map inc (list 1 2 3))` : list<number>, so `(first …)` is number|nil —
    // disjoint from string → string-length flags it.
    let w = warnings("(string/length (first (map inc (list 1 2 3))))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "map's element type (number) should flow to first: {w:?}"
    );
    // ...and a numeric sink is fine (number overlaps).
    let w = warnings("(+ 1 (first (map inc (list 1 2 3))))");
    assert!(
        w.iter().all(|s| !s.contains("expects")),
        "a number element must not warn against +: {w:?}"
    );
}

#[test]
fn filter_preserves_the_element_type() {
    // `(filter even? (list 1 2 3))` : list<int> — element type unchanged.
    let w = warnings("(string/length (first (filter even? (list 1 2 3))))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "filter should preserve the int element type: {w:?}"
    );
}

#[test]
fn element_type_flows_through_more_combinators() {
    // Structured-types extension: second/third/rest/but-last/distinct/dedupe/
    // take-last/drop-last/remove/keep/interpose/range all flow the element type,
    // so a downstream string-vs-number mismatch is caught. Each must warn here.
    for src in [
        r#"(+ 1 (second ["a" "b"]))"#,
        r#"(+ 1 (first (rest ["a" "b"])))"#,
        r#"(+ 1 (first (but-last ["a" "b"])))"#,
        r#"(+ 1 (first (distinct ["a" "b"])))"#,
        r#"(+ 1 (first (seq/dedupe ["a" "b"])))"#,
        r#"(+ 1 (first (remove (fn (x) false) ["a" "b"])))"#,
        r#"(+ 1 (first (take-last 1 ["a" "b"])))"#,
        r#"(+ 1 (first (keep (fn (x) x) ["a" "b"])))"#,
        "(string/length (first (range 5)))",
    ] {
        let w = warnings(src);
        assert!(
            w.iter()
                .any(|s| s.contains("number") || s.contains("string")),
            "expected an element-type mismatch for {src}: {w:?}"
        );
    }
    // Negative controls — a valid element type must NOT warn.
    for src in [
        "(+ 1 (second [10 20]))",
        "(+ 1 (first (rest [10 20])))",
        // interpose unions the separator: int|string includes int → valid for +.
        r#"(+ 1 (first (seq/interpose "z" [1 2])))"#,
    ] {
        let w = warnings(src);
        assert!(
            w.iter().all(|s| !s.contains("expects number")),
            "a valid element type must not warn for {src}: {w:?}"
        );
    }
}

#[test]
fn identity_lambda_preserves_element_type() {
    // `(map (fn (x) x) (list 1 2 3))` : list<int> — the lambda returns its
    // argument, so B = the element type A.
    let w = warnings("(string/length (first (map (fn (x) x) (list 1 2 3))))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "an identity callback should preserve the element type: {w:?}"
    );
}

#[test]
fn map_filter_do_not_refine_when_uncertain() {
    // Unknown callback (a local) → no refinement → no warning.
    let w = warnings("(fn (g) (string/length (first (map g (list 1 2 3)))))");
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an unknown callback must not refine the result: {w:?}"
    );
    // Identity callback + unknown collection → B depends on the (unknown)
    // element type → no refinement.
    let w = warnings("(fn (xs) (string/length (first (map (fn (x) x) xs))))");
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an identity callback over an unknown collection must not refine: {w:?}"
    );
    // Branchy lambda body → can't type it → bail to flat (no false positive).
    let w = warnings(r#"(string/length (first (map (fn (x) (if x 1 "a")) (list 1 2 3))))"#);
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "a branchy lambda body must bail to a flat result: {w:?}"
    );
}

// ---- reduce / fold result types (slice 2) ----

#[test]
fn reduce_result_is_the_accumulator_type() {
    // `(reduce + 0 (list 1 2 3))` : number (init int ∪ +'s number return) —
    // disjoint from string → flagged.
    let w = warnings("(string/length (reduce + 0 (list 1 2 3)))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "reduce's accumulator type should flow out: {w:?}"
    );
    // ...and a numeric sink is fine.
    let w = warnings("(+ 1 (reduce + 0 (list 1 2 3)))");
    assert!(
        w.iter().all(|s| !s.contains("expects")),
        "a numeric reduce result must not warn against +: {w:?}"
    );
}

#[test]
fn fold_with_a_lambda_callback_types_the_result() {
    // `(fold (fn (acc x) (+ acc x)) 0 …)` : number — the 2-arg callback's
    // return (number) joined with the init (int).
    let w = warnings("(string/length (fold (fn (acc x) (+ acc x)) 0 (list 1 2 3)))");
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "fold should type the accumulator from a lambda callback: {w:?}"
    );
}

#[test]
fn reduce_fold_bail_when_init_or_callback_unknown() {
    // Unknown callback (local) → flat, no warning.
    let w = warnings("(fn (g) (string/length (reduce g 0 (list 1 2 3))))");
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an unknown reduce callback must not refine: {w:?}"
    );
    // Unknown init type (a fn param) → flat, no warning.
    let w = warnings("(fn (init) (string/length (reduce + init (list 1 2 3))))");
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an unknown init must not refine the reduce result: {w:?}"
    );
}

// ---- unused :use import lint (Pass 4.5) ----

#[test]
fn unused_use_import_is_flagged() {
    // `io` is an embedded module; not using any of its names should warn.
    let ws = file_warnings("(defmodule test/mod (:use io))\n(defn foo (x) (+ x 1))");
    assert!(
        ws.iter()
            .any(|w| w.contains("unused :use import") && w.contains("io")),
        "expected unused :use import warning for io, got {ws:?}"
    );
}

#[test]
fn used_use_import_is_silent() {
    // `write` is one of io's public exports; using it makes the :use needed.
    let ws = file_warnings("(defmodule test/mod (:use io))\n(defn foo (port s) (write port s))");
    assert!(
        !ws.iter().any(|w| w.contains("unused :use import")),
        "used :use import should be silent, got {ws:?}"
    );
}

#[test]
fn module_with_no_use_clauses_is_silent() {
    // A defmodule with no :use clauses should never trigger the import lint.
    let ws = file_warnings("(defmodule test/mod)\n(defn foo (x) x)");
    assert!(
        !ws.iter().any(|w| w.contains("unused :use import")),
        "no :use clause → no import warning, got {ws:?}"
    );
}

// (The unused-module-private-`defn` lint moved to a whole-project Brood pass —
// `std/tool/project.blsp` `project-unused-private-warnings` — because a `--`
// name is referenced cross-module/by tests, which a single-file check can't see.
// Its coverage lives with the project tooling tests.)

/// **Keyword accessors are typed** (ADR-165 + ADR-167). A keyword head is not a
/// `Sym`, so it bypassed every sig/arity path in the checker: `(:name 5)` drew no
/// warning at all, and `(:x p)` on a typed record had no result type while the
/// identical `(get p :x)` was flagged. Both halves are pinned here.
#[test]
fn keyword_accessor_receiver_is_checked() {
    // provably-unkeyable receiver → warns, naming the keyword
    let w = warnings("(:name 5)");
    assert!(
        w.iter()
            .any(|s| s.contains(":name") && s.contains("map, set or nil")),
        "{w:?}"
    );
    assert!(!warnings("(:name \"str\")").is_empty());
    // a keyable receiver is silent, and so is an unknown one (no false positives)
    assert!(warnings("(:name {:name 1})").is_empty());
    assert!(warnings("(:name #{:name})").is_empty());
    assert!(warnings("(:name nil)").is_empty());
    assert!(warnings("(defn f (m) (:name m))").is_empty());
}

#[test]
fn keyword_accessor_arity_is_checked() {
    assert!(warnings("(:name)")
        .iter()
        .any(|s| s.contains("1 or 2 arguments")));
    assert!(warnings("(:name {} 1 2)")
        .iter()
        .any(|s| s.contains("1 or 2 arguments")));
    // the two valid arities stay silent
    assert!(warnings("(:name {})").is_empty());
    assert!(warnings("(:name {} :dflt)").is_empty());
}

#[test]
fn keyword_accessor_result_type_matches_get() {
    // A record field's declared type flows through the keyword spelling exactly as
    // it does through `get`, so a misuse of the RESULT is caught either way.
    let src = "(defrecord pt ((x int) (y int)))\n(defn a () (string/length (:x (pt 1 2))))";
    let w = file_warnings(src);
    assert!(
        w.iter()
            .any(|m| m.contains("string/length") && m.contains("int")),
        "the keyword spelling must flow the field type: {w:?}"
    );
    // and the two spellings agree
    let via_get = file_warnings(
        "(defrecord pt ((x int) (y int)))\n(defn a () (string/length (get (pt 1 2) :x)))",
    );
    assert_eq!(w.len(), via_get.len(), "get: {via_get:?} vs kw: {w:?}");
}

/// `get` had **no curated signature at all** — it is multi-arity, and `infer_sig`
/// bails on multi-arm closures, so its domain was unconstrained while `count`/`first`
/// (which have domains) caught the same mistake. Plus the relationship a flat
/// signature can't express: a *literal keyword* key can only address a keyed
/// receiver, which is the write-time half of ADR-164's runtime error.
#[test]
fn get_receiver_is_checked() {
    for src in ["(get 5 :k)", "(get :kw :k)", "(get 5 0)", "(get true :k)"] {
        let w = warnings(src);
        assert!(
            w.iter()
                .any(|m| m.contains("get") && m.contains("argument 1")),
            "{src}: {w:?}"
        );
    }
}

#[test]
fn get_with_a_keyword_key_needs_a_keyed_receiver() {
    // the mistake: a collection OF maps where one map was meant
    for src in [
        "(get [1 2] :name)",
        "(get (list 1) :name)",
        "(get \"str\" :name)",
    ] {
        let w = warnings(src);
        assert!(
            w.iter().any(|m| m.contains("keyword key needs a map")),
            "{src}: {w:?}"
        );
    }
    // every legitimate shape stays silent — including a computed key and an
    // unknown receiver, so the rule can't misfire
    for src in [
        "(get {} :name)",
        "(get #{:a} :a)",
        "(get nil :name)",
        "(get [1 2] 0)",
        "(get \"str\" 0)",
        "(defn f (c) (get c :name))",
        "(defn f (c k) (get c k))",
    ] {
        assert!(warnings(src).is_empty(), "{src} must be silent");
    }
}

/// `arg_ty_at` — the position-keyed type query behind the LSP record-field
/// completion. Shared harness: parse `src` positioned, find the line/col of
/// `needle`'s opening paren, and ask for the type of the call's item 1.
fn arg_ty_of(src: &str, needle: &str, arg_index: usize) -> Option<Ty> {
    let mut interp = crate::Interp::new();
    let positioned = reader::read_all_positioned(&mut interp.heap, src).expect("parse");
    let forms: Vec<Value> = positioned.into_iter().map(|(f, _)| f).collect();
    let at = src.find(needle).expect("needle present");
    let line = src[..at].bytes().filter(|&b| b == b'\n').count() as u32 + 1;
    let line_start = src[..at].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = src[line_start..at].chars().count() as u32 + 1;
    arg_ty_at(&mut interp.heap, &forms, line, col, arg_index)
}

fn field_names(ty: &Ty) -> Vec<String> {
    ty.record_fields()
        .map(|f| f.keys().map(|&s| value::symbol_name(s)).collect())
        .unwrap_or_default()
}

#[test]
fn arg_ty_at_types_a_direct_ctor_argument() {
    let src = "(defrecord point (x y))\n(get (point 1 2) :x)";
    let ty = arg_ty_of(src, "(get", 1).expect("captured");
    let names = field_names(&ty);
    assert!(names.contains(&"x".to_string()), "{names:?}");
    assert!(names.contains(&"y".to_string()), "{names:?}");
}

#[test]
fn arg_ty_at_types_a_let_bound_record_inside_a_defn() {
    // The whole point of routing through the checker: `p` is a bare symbol
    // whose type only the scope walk knows (the let RHS's ctor sig).
    let src = "(defrecord point (x y))\n(defn f () (let (p (point 1 2)) (assoc p :x 3)))";
    let ty = arg_ty_of(src, "(assoc", 1).expect("captured");
    assert!(field_names(&ty).contains(&"x".to_string()));
}

#[test]
fn arg_ty_at_types_a_gap_a_global() {
    // A `(def g (ctor …))` global reaches the query via Gap A value inference.
    let src = "(defrecord point (x y))\n(def origin (point 0 0))\n(get origin :y)";
    let ty = arg_ty_of(src, "(get origin", 1).expect("captured");
    assert!(field_names(&ty).contains(&"y".to_string()));
}

#[test]
fn arg_ty_at_misses_degrade_to_none() {
    // Unknown-typed argument → None (never a wrong type); missing item → None;
    // position matching nothing → None.
    let src = "(defn f (p) (get p :x))";
    assert!(arg_ty_of(src, "(get", 1).is_none(), "untyped param");
    assert!(arg_ty_of(src, "(get", 9,).is_none(), "no such item");
    let mut interp = crate::Interp::new();
    let positioned = reader::read_all_positioned(&mut interp.heap, src).expect("parse");
    let forms: Vec<Value> = positioned.into_iter().map(|(f, _)| f).collect();
    assert!(arg_ty_at(&mut interp.heap, &forms, 99, 1, 1).is_none());
}

// ---- `(:use M)` module loading: the checker's "already loaded?" test ----

/// [`feature_loaded`] must answer from the `*features*` registry — the same record
/// the runtime's `require-one` consults — in BOTH directions. It replaced a test
/// that asked "does any `M/…` global exist", which reported *loaded* for a module
/// sharing its namespace with kernel primitives (`file/slurp` & co. exist with
/// `std/file.blsp` unread), so `(:use file)` imported the primitives and left every
/// Brood-level name in it unbound on the single-file `brood --check` path.
#[test]
fn feature_loaded_reads_the_feature_registry_not_the_namespace() {
    let mut interp = crate::Interp::new();
    // `file` has 18 `file/…` kernel primitives but its .blsp is not loaded yet — the
    // exact shape that fooled the old namespace-presence test. `string` is the same
    // shape twice over since ADR-246: its `.blsp` no longer loads at boot, while
    // `string/split`, `string/length`, … are kernel primitives AND the prelude binds
    // `string/join` and friends to autoload stubs. Neither may read as the module.
    for module in ["file", "string"] {
        assert!(
            !interp
                .heap
                .module_public_exports(&format!("{module}/"))
                .is_empty(),
            "precondition: {module}/ primitives exist without the module loaded"
        );
        assert!(
            !feature_loaded(&mut interp.heap, module),
            "primitives in a namespace must not read as the module being loaded"
        );
    }
    // A name no module has is not loaded either (and doesn't panic).
    assert!(!feature_loaded(&mut interp.heap, "no-such-module-zzz"));
    // After a real require each flips.
    for module in ["file", "string"] {
        interp
            .eval_str(&format!("(require-one '{module})"))
            .expect("require");
        assert!(
            feature_loaded(&mut interp.heap, module),
            "a required module must read as loaded"
        );
    }
}

// ---- same-file arity (the def site is the authority) ----
// The file being checked is never loaded, so `sigs::arity_of` (which reads the
// global table) sees nothing for its own functions. Before `Ctx::file_arity`, that
// meant a call to a function defined in the same file had NO arity check at all —
// the cheapest check in the system, absent exactly where a fresh edit is.

#[test]
fn same_file_call_with_too_few_arguments_is_flagged() {
    let ws = file_warnings("(defn f (x y) x)\n(defn g () (f 1))");
    assert!(
        ws.iter()
            .any(|w| w.contains("f: wrong number of arguments — expected 2, got 1")),
        "{ws:?}"
    );
}

#[test]
fn same_file_call_with_the_right_arity_is_silent() {
    let ws = file_warnings("(defn f (x y) x)\n(defn g () (f 1 2))");
    assert!(!ws.iter().any(|w| w.contains("wrong number")), "{ws:?}");
}

#[test]
fn same_file_variadic_and_optional_arities_admit_their_range() {
    // `&` collects a tail: 1-or-more. `&optional`: a range. Neither may false-flag.
    let ws = file_warnings(
        "(defn v (a & rest) a)\n\
         (defn o (a &optional b) a)\n\
         (defn use () (list (v 1) (v 1 2 3) (o 1) (o 1 2)))",
    );
    assert!(!ws.iter().any(|w| w.contains("wrong number")), "{ws:?}");
    // …but the arity floor still holds.
    let ws = file_warnings("(defn v (a & rest) a)\n(defn use () (v))");
    assert!(
        ws.iter()
            .any(|w| w.contains("v: wrong number of arguments — expected 1 or more, got 0")),
        "{ws:?}"
    );
}

#[test]
fn same_file_multi_arity_admits_every_arm() {
    let ws = file_warnings("(defn f ((x) x) ((x y) x))\n(defn g () (list (f 1) (f 1 2)))");
    assert!(!ws.iter().any(|w| w.contains("wrong number")), "{ws:?}");
    let ws = file_warnings("(defn f ((x) x) ((x y) x))\n(defn g () (f 1 2 3))");
    assert!(
        ws.iter()
            .any(|w| w.contains("f: wrong number of arguments — expected 1 to 2, got 3")),
        "{ws:?}"
    );
}

#[test]
fn the_definition_beats_a_disagreeing_sig_for_arity() {
    // A `(sig …)` used to be the ONLY arity source for a same-file name, so a sig that
    // disagreed with its `defn` silently made a wrong call look right — `nest check`
    // passed a program that died on its first call.
    let ws = file_warnings("(sig f (int -> int))\n(defn f (a b) a)\n(defn g () (f 5))");
    assert!(
        ws.iter()
            .any(|w| w.contains("f: wrong number of arguments — expected 2, got 1")),
        "{ws:?}"
    );
}

// ---- the declaration itself must be readable (Pass 2.85) ----
// A `(sig …)` is read ahead of every other signature source, so a declaration the
// parser silently drops is worse than none: the position widens to `any` and the
// author is told nothing. All four shapes below used to exit 0 with no diagnostic.

#[test]
fn a_misspelled_type_name_in_a_sig_is_reported() {
    let ws = file_warnings("(sig f (strng -> int))\n(defn f (s) 0)");
    assert!(
        ws.iter().any(|w| w.contains("sig f: unknown type `strng`")),
        "{ws:?}"
    );
}

#[test]
fn a_misspelled_type_constructor_in_a_sig_is_reported() {
    let ws = file_warnings("(sig f ((tupel int) -> int))\n(defn f (t) 0)");
    assert!(
        ws.iter()
            .any(|w| w.contains("sig f: unknown type constructor `tupel`")),
        "{ws:?}"
    );
    // …and the innermost offender wins, not the enclosing constructor.
    let ws = file_warnings("(sig f ((vector strng) -> int))\n(defn f (v) 0)");
    assert!(
        ws.iter().any(|w| w.contains("unknown type `strng`")),
        "{ws:?}"
    );
}

#[test]
fn a_sig_whose_arity_contradicts_the_definition_is_reported() {
    let ws = file_warnings("(sig f (int -> int))\n(defn f (a b) a)");
    assert!(
        ws.iter()
            .any(|w| w.contains("sig f: declares 1 argument(s) but the definition takes 2")),
        "{ws:?}"
    );
}

#[test]
fn a_sig_arity_that_merely_narrows_the_definition_is_silent() {
    // A multi-arm `defn` annotated with one arm's arrow overlaps the definition's
    // hull — not provably wrong, so it must stay silent (the no-false-positive rule).
    let ws = file_warnings("(sig f (int -> int))\n(defn f ((a) a) ((a b) a))");
    assert!(!ws.iter().any(|w| w.contains("sig f:")), "{ws:?}");
    // A `&optional` definition against a fixed-arity sig, likewise.
    let ws = file_warnings("(sig g (int -> int))\n(defn g (a &optional b) a)");
    assert!(!ws.iter().any(|w| w.contains("sig g:")), "{ws:?}");
}

#[test]
fn a_sig_for_a_name_that_is_never_defined_is_reported() {
    let ws = file_warnings("(sig ghost (int -> int))");
    assert!(
        ws.iter()
            .any(|w| w.contains("sig ghost: nothing named `ghost` is defined here")),
        "{ws:?}"
    );
    // Order doesn't matter — the def may follow the sig.
    let ws = file_warnings("(sig f (int -> int))\n(defn f (a) a)");
    assert!(!ws.iter().any(|w| w.contains("nothing named")), "{ws:?}");
}

#[test]
fn a_capitalised_unknown_type_name_stays_silent_inside_an_arrow_too() {
    // The first cut reported `(Shape -> int)` as a *malformed arrow*: every part read
    // as fine (the capitalised name being deliberately silent), so the walk fell
    // through to the structural message and named the wrong thing. A part that does
    // not parse now decides the whole expression — including when its verdict is
    // silence.
    let ws = file_warnings("(sig w (Shape -> int))\n(defn w (x) 1)\n(defn p () (w 42))");
    assert!(!ws.iter().any(|m| m.contains("sig w:")), "{ws:?}");
    let ws = file_warnings("(sig w ((vector Shape) -> int))\n(defn w (x) 1)");
    assert!(!ws.iter().any(|m| m.contains("sig w:")), "{ws:?}");
    let ws = file_warnings("(sig w ((record :s Shape) -> int))\n(defn w (x) 1)");
    assert!(!ws.iter().any(|m| m.contains("sig w:")), "{ws:?}");
}

#[test]
fn a_capitalised_unknown_type_name_stays_silent() {
    // An ability used as a type resolves by bare name (ADR-181/186), and a
    // single-file check only knows the abilities the file itself declares — so an
    // unknown *capitalised* name is assumed to be one, and never reported.
    let ws = file_warnings("(sig f (Shape -> int))\n(defn f (s) 0)");
    assert!(!ws.iter().any(|w| w.contains("unknown type")), "{ws:?}");
}

#[test]
fn every_type_constructor_the_grammar_parses_is_known_to_the_validator() {
    // `type_expr_problem` reports an unrecognised head, so a constructor added to
    // `parse_type` and not to `TYPE_HEADS` would be reported as unknown — a lint that
    // fires on correct code. Pin the two lists together: each head must parse.
    let mut interp = crate::Interp::new();
    for head in super::annot::TYPE_HEADS {
        let src = match head {
            "map" => "(map keyword int)".to_string(),
            "record" => "(record :a int)".to_string(),
            "tuple" => "(tuple int string)".to_string(),
            "or" | "and" => format!("({head} int string)"),
            "not" => "(not nil)".to_string(),
            _ => format!("({head} int)"),
        };
        let form = reader::read_one(&mut interp.heap, &src).expect("parse");
        assert!(
            super::annot::type_expr_problem(&interp.heap, form).is_none(),
            "`{head}` is in TYPE_HEADS but `{src}` does not read as a type"
        );
    }
}

// ---- the walk is TOTAL (the reach gate) ----
// KI-67 (`try` bodies) and KI-70 (vector/map literals) were the same bug at two
// depths: a `return`-early line in the walk behind which no lint ran at all, and
// which left no trace to grep for — a lint that is never *reached* is invisible in
// a way a suppressed one is not. Both were found by accident. This is the gate that
// makes a third one fail a test instead: every recognised special form, and every
// container literal, gets a planted unresolvable name in each of its *code*
// positions, and must report it. The two data-holding forms must stay silent.

/// `(head, source with a planted `zzz-…` name, must the walk report it?)`.
/// Every entry in `SPECIAL_HEAD` must appear here — see
/// `every_special_form_is_covered_by_the_reach_gate`.
const REACH_CASES: &[(&str, &str, bool)] = &[
    // Data, not code — reporting here would flag a sketched or quoted name.
    ("quote", "(defn f () (quote (zzz-q)))", false),
    ("comment", "(defn f () (comment (zzz-c)))", false),
    // A template is data, but its `~` escapes are code evaluated at expansion time.
    ("quasiquote", "(defmacro m (x) `(a ~(zzz-qq x)))", true),
    ("quasiquote", "(defmacro m (x) `(a zzz-quoted ~x))", false),
    // Deliberate-failure forms: every other lint is suppressed inside them, but an
    // unbound name is never the failure under test (KI-67).
    ("try", "(defn f () (try (zzz-try) (catch e e)))", true),
    (
        "%try",
        "(defn f () (%try (fn () (zzz-tp)) (fn (e) e)))",
        true,
    ),
    ("error-of", "(defn f () (error-of (zzz-eo)))", true),
    ("assert-error", "(defn f () (assert-error (zzz-ae)))", true),
    // Ordinary code positions.
    ("if", "(defn f (b) (if (zzz-test b) 1 2))", true),
    ("if", "(defn f (b) (if b (zzz-then) 2))", true),
    ("if", "(defn f (b) (if b 1 (zzz-else)))", true),
    ("let", "(defn f () (let (a (zzz-rhs)) a))", true),
    ("let", "(defn f () (let (a 1) (zzz-body a)))", true),
    (
        "letrec",
        "(defn f () (letrec (g (fn () (zzz-lr))) (g)))",
        true,
    ),
    ("fn", "(defn f () (fn () (zzz-fn)))", true),
    ("def", "(def x (zzz-def))", true),
    ("defn", "(defn f () (zzz-defn))", true),
    ("defmacro", "(defmacro m (x) (zzz-dm x))", true),
];

/// Container literals — KI-70's class, kept beside the special forms because it is
/// the same question ("does the walk go in?") for the other half of the syntax.
const REACH_CONTAINER_CASES: &[(&str, bool)] = &[
    ("(defn f (x) [:tag (zzz-vec x)])", true),
    ("(defn f (x) {:k (zzz-mapval x)})", true),
    ("(defn f (x) {(zzz-mapkey x) :v})", true),
    ("(defn f (x) [:tag {:k (str (zzz-deep x))}])", true), // the hive shape (KI-70)
    ("(defn f (x) (list [1 2] {:a 1}))", false),           // ordinary literals: silent
];

fn planted_name(src: &str) -> String {
    let start = src
        .find("zzz-")
        .expect("every reach case plants a `zzz-…` name");
    let rest = &src[start..];
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

#[test]
fn the_walk_reaches_every_code_position() {
    for (head, src, must_report) in REACH_CASES {
        let planted = planted_name(src);
        let want = format!("unbound symbol: {planted}");
        // Both walks, because they are different code paths reaching the same arm:
        // `check_file` (what `nest check` runs, with whole-file facts) and
        // `check_form` on the expanded fragment (what the REPL, the LSP and the MCP
        // `check` tool run). A form skipped by one and covered by the other is how
        // this gate's own first sabotage attempt passed — file mode caught the
        // quasiquote escape through an unrelated whole-file pass while the arm under
        // test was doing nothing.
        for (mode, ws) in [
            ("file", file_warnings(src)),
            ("fragment", warnings_expanded(src)),
        ] {
            let reported = ws.contains(&want);
            assert_eq!(
                reported, *must_report,
                "`{head}` ({mode}): expected report={must_report} for `{planted}` in `{src}` — got {ws:?}"
            );
        }
    }
    for (src, must_report) in REACH_CONTAINER_CASES {
        let ws = file_warnings(src);
        let reported = ws.iter().any(|w| w.starts_with("unbound symbol: zzz-"));
        assert_eq!(
            reported, *must_report,
            "container reach: `{src}` — got {ws:?}"
        );
    }
}

#[test]
fn every_special_form_is_covered_by_the_reach_gate() {
    // The completeness half: a head added to `SPECIAL_HEAD` with no case here would
    // otherwise inherit whatever reach it happened to get, unwatched — which is
    // exactly how KI-67 and KI-70 survived. Adding a head now fails this test until
    // someone says, in a case above, what the walk is supposed to do with its body.
    for &sym in super::walk::SPECIAL_HEAD.keys() {
        let name = crate::core::value::symbol_name(sym);
        assert!(
            REACH_CASES.iter().any(|(head, _, _)| *head == name),
            "special form `{name}` has no reach-gate case in REACH_CASES"
        );
    }
}

// ---- `(not T)` — the complement, sayable at last ----

#[test]
fn not_type_in_a_sig_rejects_a_member_of_the_negated_set() {
    let ws = file_warnings("(sig f ((not nil) -> int))\n(defn f (x) 0)\n(defn g () (f nil))");
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
    // …and admits everything else.
    let ws = file_warnings("(sig f ((not nil) -> int))\n(defn f (x) 0)\n(defn g () (f 5))");
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
}

#[test]
fn not_type_composes_with_and_and_or() {
    // The idiom the lattice could always compute and the grammar could not say.
    let ws = file_warnings(
        "(sig f ((and number (not float)) -> int))\n(defn f (x) 0)\n(defn g () (f 1.5))",
    );
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
    let ws = file_warnings(
        "(sig f ((and number (not float)) -> int))\n(defn f (x) 0)\n(defn g () (f 1))",
    );
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
}

#[test]
fn a_small_complement_renders_as_not_rather_than_a_tag_dump() {
    // `expects string, got nil | bool | number | symbol | keyword | pair | vector | fn
    // | macro | native | map | ref | pid | rope | socket | subprocess | table | bytes |
    // set` was a real diagnostic — the else-branch of a `(string? x)` guard.
    assert_eq!(Ty::of(Tag::Str).negate().to_string(), "(not string)");
    assert_eq!(
        Ty::of(Tag::Str)
            .union(Ty::of(Tag::Nil))
            .negate()
            .to_string(),
        "(not (nil | string))"
    );
    // An ordinary wide union is still a union — the rendering only fires for a
    // genuinely small complement.
    assert_eq!(
        Ty::of(Tag::Int).union(Ty::of(Tag::Str)).to_string(),
        "int | string"
    );
}

// ---- arrow-typed parameters are enforced at the call site ----
// A `sig`-declared higher-order parameter was annotated and then not checked:
// `(sig g ((int -> int) -> int))` accepted `string/length` in silence.

#[test]
fn a_callback_that_cannot_accept_what_it_is_handed_is_flagged() {
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n(defn c () (g string/length))",
    );
    assert!(
        ws.iter().any(|w| w.contains(
            "g: argument 1 is a callback handed int at position 1, but string/length takes string there"
        )),
        "{ws:?}"
    );
}

#[test]
fn a_callback_whose_parameter_merely_widens_is_silent() {
    // `math/abs` takes `number`, which overlaps `int` — not provably wrong, so
    // silent. Disjointness, never subtyping: the no-false-positive rule.
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n(defn c () (g math/abs))",
    );
    assert!(!ws.iter().any(|w| w.contains("callback handed")), "{ws:?}");
}

#[test]
fn a_callback_whose_result_cannot_be_used_is_flagged() {
    // Comparing results by *subtyping* would false-positive at every call site (an
    // over-approximated return is not a subtype of a specific one). Disjointness is
    // sound in the same way the parameter direction is: the inferred return is a
    // superset of the truth, so if the superset shares nothing with what the caller
    // does with it, neither does the truth.
    let ws = file_warnings(
        "(sig g ((int -> string) -> int))\n(defn g (f) 0)\n(defn h (n) (+ n 1))\n(defn c () (g h))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("callback whose result is used as string")
                && w.contains("h returns")),
        "{ws:?}"
    );
}

#[test]
fn a_callback_whose_result_merely_widens_is_silent() {
    // The over-approximation must not warn: `number` overlaps `int`, and an unknown
    // return (`any`) overlaps everything.
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) 0)\n(defn h (n) (+ n 1))\n(defn c () (g h))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("callback whose result")),
        "{ws:?}"
    );
    let ws = file_warnings(
        "(sig g ((int -> string) -> int))\n(defn g (f) 0)\n(defn h (n) n)\n(defn c () (g h))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("callback whose result")),
        "{ws:?}"
    );
}

#[test]
fn a_same_file_callback_is_checked_from_its_inferred_signature() {
    // An inferred parameter demand is a *superset* of what the function really
    // accepts, so disjoint-from-the-superset is disjoint from the truth.
    let ws = file_warnings(
        "(sig g ((int -> int) -> int))\n(defn g (f) (f 1))\n\
         (defn cb (s) (string/length s))\n(defn c () (g cb))",
    );
    assert!(
        ws.iter().any(|w| w.contains("but cb takes string there")),
        "{ws:?}"
    );
}

#[test]
fn a_permissive_higher_order_stdlib_callback_stays_silent() {
    // `map`'s curated arrow is `(any) -> any`, which is disjoint from nothing.
    let ws = file_warnings("(defn c () (map string/length [1 2 3]))");
    assert!(!ws.iter().any(|w| w.contains("callback handed")), "{ws:?}");
}

// ---- parameter DOMAINS: a branch's demand, credited within its guard ----
// The old rule credited only unconditional demands, so the ordinary shape of Brood
// code — a body that branches on what its argument is — constrained nothing at all.

#[test]
fn the_domain_walk_survives_a_pathologically_deep_body() {
    // The sibling of `checker_survives_pathologically_deep_forms`, for the passes that
    // walk a function's BODY: a body this deep can only arrive by construction (the
    // reader caps nesting at 256), which is exactly what a macro expansion can produce.
    // The property under test is "returns instead of crashing the host".
    //
    // It found one on its first run, and not in the pass it was written for: the
    // non-tail-recursion lint's `walk` recursed unguarded, so a deep body inside a
    // `(def n (fn …))` aborted the process — the 2026-07-23 host-panic pass hardened
    // that lint's entry point and not the recursion that descends the body.
    let interp = crate::Interp::new();
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let identity = crate::core::value::intern("identity");
    let x = crate::core::value::intern("x");
    // (identity (identity … x))
    let mut body = Value::Sym(x);
    for _ in 0..20_000 {
        let tail = heap.alloc_pair(body, Value::Nil);
        body = heap.alloc_pair(Value::Sym(identity), tail);
    }
    // (def deep (fn (x) <body>)) — the shape Pass 2.8 infers a parameter domain from.
    let params = heap.alloc_pair(Value::Sym(x), Value::Nil);
    let fn_tail = heap.alloc_pair(body, Value::Nil);
    let fn_parts = heap.alloc_pair(params, fn_tail);
    let fn_form = heap.alloc_pair(Value::Sym(crate::core::value::intern("fn")), fn_parts);
    let def_tail = heap.alloc_pair(fn_form, Value::Nil);
    let def_parts = heap.alloc_pair(Value::Sym(crate::core::value::intern("deep")), def_tail);
    let def_form = heap.alloc_pair(Value::Sym(crate::core::value::intern("def")), def_parts);
    let _ = check_file(&mut heap, &[def_form]);
}

#[test]
fn a_branch_union_domain_rejects_what_no_branch_admits() {
    let ws = file_warnings(
        "(defn f (x) (if (string? x) (string/length x) (+ x 1)))\n(defn c () (f :kw))",
    );
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_branch_union_domain_admits_what_either_branch_admits() {
    // Both members of the union must stay silent — this is the false-positive class
    // the unconditional-demand rule was protecting against.
    for arg in ["\"s\"", "5"] {
        let ws = file_warnings(&format!(
            "(defn f (x) (if (string? x) (string/length x) (+ x 1)))\n(defn c () (f {arg}))"
        ));
        assert!(
            !ws.iter().any(|w| w.contains("f: argument 1")),
            "arg {arg}: {ws:?}"
        );
    }
}

#[test]
fn a_guard_that_proves_nothing_leaves_the_branch_unconstrained() {
    // `(if b …)` on an unrelated variable: whichever branch runs, one of the two
    // demands must hold — but neither alone may constrain.
    let ws = file_warnings(
        "(defn f (b x) (if b (string/length x) (+ x 1)))\n(defn c () (f true \"s\"))",
    );
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
    // …and a value no branch admits is still caught.
    let ws =
        file_warnings("(defn f (b x) (if b (string/length x) (+ x 1)))\n(defn c () (f true :kw))");
    assert!(
        ws.iter().any(|w| w.contains("f: argument 2 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_when_body_does_not_constrain_what_the_test_does_not_reach() {
    // `(when test body)` runs `body` only sometimes, so an argument the body would
    // reject is not provably wrong — unless the test itself pins the argument.
    let ws = file_warnings("(defn f (b x) (when b (string/length x)))\n(defn c () (f true 5))");
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
}

#[test]
fn a_match_domain_comes_from_its_clause_patterns() {
    // Every clause's pattern is a guard; the no-clause-matched branch raises, so its
    // domain is `never` and the clauses' patterns add up to the function's domain.
    let ws = file_warnings("(defn f (x) (match x ((:ok v) v) ((:error e) e)))\n(defn c () (f 5))");
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_destructuring_head_constrains_the_argument() {
    let ws = file_warnings("(defn f ([a b]) (+ a b))\n(defn c () (f 5))");
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_clause_guard_constrains_the_argument() {
    let ws = file_warnings("(defn f (x) :when (string? x) (string/length x))\n(defn c () (f 5))");
    assert!(
        ws.iter()
            .any(|w| w.contains("f: argument 1 expects string")),
        "{ws:?}"
    );
}

#[test]
fn an_unexpanded_macro_body_does_not_leak_a_demand() {
    // A prelude closure keeps its body *as written*, so the walk meets `(cond …)`,
    // `(when …)` and user macros verbatim. Reading those as ordinary calls — every
    // operand evaluated — made `type-matches?` demand a `seqable` first argument,
    // because `(first t)` sat in a clause body. The whole-file gate caught it; these
    // pin the two rules that fixed it.
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(defn tm (t v) (cond (nil? t) (nil? v) (pair? t) (first t) else true))")
        .expect("def");
    let sig = super::sigs::sig_of(&interp.heap, crate::core::value::intern("tm"))
        .expect("a sig is inferred");
    assert_eq!(
        sig.params.first().map(Ty::to_string),
        Some("any".to_string()),
        "a `cond` clause body must not constrain unconditionally"
    );
}

// ---- multi-arm functions: every clause is a signature ----
// A multi-arm closure has no single `Sig`, so its callers' arguments went entirely
// unchecked. Each arm has one, and a call no arity-relevant arm accepts is a
// provable error — the same rule ADR-116's declared overloads already used.

#[test]
fn a_call_no_clause_of_a_multi_arity_function_accepts_is_flagged() {
    let ws = file_warnings("(defn f ((x) (string/length x)) ((x y) (+ x y)))\n(defn c () (f 5))");
    assert!(
        ws.iter()
            .any(|w| w.contains("f: no clause accepts these arguments")),
        "{ws:?}"
    );
    // The arity that fits a different clause stays silent.
    let ws = file_warnings("(defn f ((x) (string/length x)) ((x y) (+ x y)))\n(defn c () (f 1 2))");
    assert!(
        !ws.iter().any(|w| w.contains("no clause accepts")),
        "{ws:?}"
    );
}

#[test]
fn guarded_clauses_give_a_function_its_domain() {
    // `:when` guards (ADR-226) lower to a single variadic `fn` over `match*`, so the
    // clauses only exist in the un-expanded form — which is where this reads them.
    let ws = file_warnings(
        "(defn g ((x) :when (string? x) (string/length x)) ((x) :when (int? x) (+ x 1)))\n\
         (defn c () (g :kw))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("g: no clause accepts these arguments")
                && w.contains("(string), (int)")),
        "{ws:?}"
    );
    // Both admitted types stay silent.
    for arg in ["\"s\"", "5"] {
        let ws = file_warnings(&format!(
            "(defn g ((x) :when (string? x) (string/length x)) ((x) :when (int? x) (+ x 1)))\n\
             (defn c () (g {arg}))"
        ));
        assert!(
            !ws.iter().any(|w| w.contains("no clause accepts")),
            "arg {arg}: {ws:?}"
        );
    }
}

#[test]
fn an_unguarded_final_clause_keeps_a_multi_clause_call_silent() {
    // A clause that admits anything is the catch-all every dispatch-style function
    // ends with; its domain is `any`, so no call can be ruled out.
    let ws = file_warnings(
        "(defn g ((x) :when (string? x) (string/length x)) ((x) x))\n(defn c () (g :kw))",
    );
    assert!(
        !ws.iter().any(|w| w.contains("no clause accepts")),
        "{ws:?}"
    );
}

// ---- a union of shapes is checkable at a call site (ADR-262) ----

#[test]
fn a_union_of_tuple_shapes_rejects_a_member_of_neither() {
    let ws = file_warnings(
        "(sig f ((or (tuple int) (tuple string)) -> any))\n(defn f (t) t)\n(defn c () (f [true]))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("expects (tuple int) | (tuple string)")),
        "{ws:?}"
    );
    // …and admits either alternative.
    for arg in ["[1]", "[\"s\"]"] {
        let ws = file_warnings(&format!(
            "(sig f ((or (tuple int) (tuple string)) -> any))\n(defn f (t) t)\n(defn c () (f {arg}))"
        ));
        assert!(
            !ws.iter().any(|w| w.contains("f: argument")),
            "{arg}: {ws:?}"
        );
    }
}

#[test]
fn a_union_of_record_shapes_rejects_a_map_matching_neither() {
    let ws = file_warnings(
        "(sig f ((or (record :a int) (record :b int)) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:zzz 1}))",
    );
    assert!(
        ws.iter().any(|w| w.contains("expects {a: int} | {b: int}")),
        "{ws:?}"
    );
    let ws = file_warnings(
        "(sig f ((or (record :a int) (record :b int)) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:a 1}))",
    );
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
}

// ---- closed records (ADR-264) ----

#[test]
fn a_closed_record_rejects_an_undeclared_key() {
    let ws = file_warnings(
        "(sig f ((record :name string) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:name \"Ada\" :extra :k}))",
    );
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn an_open_record_admits_undeclared_keys() {
    let ws = file_warnings(
        "(sig f ((record &open :name string) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:name \"Ada\" :extra :k}))",
    );
    assert!(!ws.iter().any(|w| w.contains("f: argument")), "{ws:?}");
    // …and still enforces what it does declare.
    let ws = file_warnings(
        "(sig f ((record &open :name string) -> any))\n(defn f (m) m)\n\
         (defn c () (f {:name 42}))",
    );
    assert!(
        ws.iter().any(|w| w.contains("f: argument 1 expects")),
        "{ws:?}"
    );
}

#[test]
fn a_field_read_through_a_tagged_union_resolves() {
    // The payoff. Each term answers for `:ok` — `int` in the first, `nil` in the
    // second (closed: the key is absent) — so the union answers `int | nil`.
    let ws = file_warnings(
        "(sig f ((or (record :ok int) (record :error string)) -> any))\n\
         (defn f (r) (string/length (get r :ok)))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("string/length") && w.contains("int")),
        "{ws:?}"
    );
    // An open alternative says nothing about the key, so the union says nothing.
    let ws = file_warnings(
        "(sig f ((or (record :ok int) (record &open :error string)) -> any))\n\
         (defn f (r) (string/length (get r :ok)))",
    );
    assert!(!ws.iter().any(|w| w.contains("string/length")), "{ws:?}");
}

#[test]
fn a_defrecord_accessor_takes_any_record_carrying_its_field() {
    // The accessor sig is `&open` by construction: a real value carries `:__id__` and
    // every sibling field, so a closed one-field shape would describe nothing.
    let ws =
        file_warnings("(defrecord point ((x int) (y int)))\n(defn c () (point-x (point 1 2)))");
    assert!(!ws.iter().any(|w| w.contains("point-x")), "{ws:?}");
}

#[test]
fn a_bare_local_test_narrows_by_truthiness() {
    // `(if v …)` is itself a guard: only `nil` and `false` are falsy, so the
    // then-branch has `v` as neither. This is what `if-let`/`when-let` expand to, and
    // without it a closed literal's `nil` read as a false positive there.
    let ws = warnings("(let (v (get {:x 10} :y)) (if v (inc v) :none))");
    assert!(!ws.iter().any(|w| w.contains("inc")), "{ws:?}");
    // **Biconditional**, now that `¬{false}` is exactly `{true}`: the else-branch has
    // `v` falsy, so a use that needs a number there is a real error and is caught.
    // (While the truthy type was only approximable as `not nil`, this had to stay
    // one-sided — its complement, `nil`, is not implied by a false test.)
    let ws = warnings("(fn (x) (let (v (if (int? x) 1 nil)) (if v :ok (inc v))))");
    assert!(
        ws.iter().any(|w| w.contains("inc") && w.contains("nil")),
        "{ws:?}"
    );
    // …and `(not v)` must NOT read as "v is nil": that inversion reported live code
    // as dead when the guard was two-sided.
    let ws = warnings_expanded("(let (s (if true true false)) (let (v (not s)) (if v 1 2)))");
    assert!(!ws.iter().any(|w| w.contains("unreachable")), "{ws:?}");
}

// ---- effective signatures for a buffer (the LSP inlay-hint source) ----

/// `file_signatures` over a source string, as `name → rendered sig (declared?)`.
fn signatures(src: &str) -> Vec<(String, String, bool)> {
    let interp = crate::Interp::new();
    let mut heap =
        crate::core::heap::Heap::with_regions(interp.heap.prelude_arc(), interp.heap.runtime_arc());
    heap.set_global(crate::core::value::EnvId::GLOBAL);
    let forms = crate::syntax::reader::read_all(&mut heap, src).expect("parse");
    super::file_signatures(&mut heap, &forms)
        .into_iter()
        .map(|s| (s.name, s.sig.to_string(), s.declared))
        .collect()
}

#[test]
fn file_signatures_reports_what_the_checker_inferred() {
    // The buffer is never loaded, which is exactly why hover cannot answer this and
    // the form-based inference can.
    let sigs = signatures("(defn f (s) (string/length s))");
    assert_eq!(sigs.len(), 1, "{sigs:?}");
    assert_eq!(sigs[0].0, "f");
    assert!(sigs[0].1.contains("string"), "{sigs:?}");
    assert!(!sigs[0].2, "an inferred sig must not read as declared");
}

#[test]
fn file_signatures_prefer_and_mark_a_declaration() {
    let sigs = signatures("(sig f (int -> string))\n(defn f (n) \"x\")");
    assert_eq!(sigs.len(), 1, "{sigs:?}");
    assert!(sigs[0].2, "a declared sig must be marked: {sigs:?}");
    assert!(sigs[0].1.contains("int"), "{sigs:?}");
}

#[test]
fn file_signatures_covers_guarded_clauses_and_skips_non_functions() {
    // A multi-clause definition has one signature per clause (ADR-261); the first is
    // what a reader is looking at.
    let sigs = signatures(
        "(defn g ((x) :when (string? x) 1) ((x) :when (int? x) 2))\n\
         (def not-a-fn 5)\n\
         (defmacro m (x) x)",
    );
    let names: Vec<&str> = sigs.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(names, vec!["g"], "{sigs:?}");
    assert!(sigs[0].1.contains("string"), "{sigs:?}");
    // …and it carries the union of the clauses' returns rather than a bare `any`.
    assert!(sigs[0].1.contains("1 | 2"), "{sigs:?}");
}

#[test]
fn file_signatures_costs_nothing_when_unarmed() {
    // The capture is a no-op on the ordinary checking path — pinned because it runs
    // inside `check_file`, which every diagnostic request goes through.
    assert!(file_warnings("(defn f (s) (string/length s))")
        .iter()
        .all(|w| !w.contains("panic")));
    let ws = file_warnings("(defn f (s) (string/length s))\n(defn c () (f 5))");
    assert!(ws.iter().any(|w| w.contains("expects string")), "{ws:?}");
}

#[test]
fn a_module_private_function_is_inferred_and_its_call_sites_are_checked() {
    // `defn-` expands to `(do (def name (fn …)) (%mark-private 'name))`, so every pass
    // keyed on a top-level `(def …)` used to see NO definition at all for a private
    // function — and most definitions in a real module are private (40 of
    // `std/json.blsp`'s 42). The consequence was silent: their call sites went
    // unchecked, in exactly the internals where an argument-order slip lives.
    let warnings = file_warnings(
        r#"
        (defmodule privacy-demo)
        (defn- widen (s) (string/length s))
        (defn use-it () (widen 42))
        "#,
    );
    assert!(
        warnings.iter().any(|w| w.contains("expects string")),
        "a wrong-typed call to a private function must be flagged; got {warnings:?}"
    );
}

#[test]
fn a_macros_generated_temporary_is_never_typed() {
    // Two guards, one property. The descent into a top-level `do` is limited to the
    // `defn-`/`def-` expansion, and a gensym'd name is never typed regardless.
    // Opening a top-level `do` reaches more than `defn-`: other macros emit one over
    // GENERATED names. The linear-map rewrite wraps a fold's result in
    // `(do (def linmap-out__N …) …)`, and typing that temporary made the checker flag a
    // branch of the rewrite's own wrapper that cannot run with that value — a warning
    // naming a symbol the author never wrote and cannot fix.
    // `tests/linmap_soundness_test.blsp` caught it live; this pins the rule.
    let generated = file_warnings(
        r#"
        (do (def helper__42 (fn (s) (string/length s))) (io/puts "generated"))
        (defn consume () (helper__42 3))
        "#,
    );
    assert!(
        !generated.iter().any(|w| w.contains("expects string")),
        "a gensym'd definition must not be typed — nobody can act on the warning; got \
         {generated:?}"
    );

    // The descent that reaches it is narrow for a second reason: `defability`/`defimpl`
    // define their ops inside a top-level `do` too, and an INFERRED signature for an op
    // displaces the `:-> T` return the ability declares. Opening every `do` stopped that
    // return flowing to call sites — which is what
    // `ability_op_return_type_flows_to_call_site` pins.
}

#[test]
fn a_failed_equality_test_narrows_the_else_branch_for_a_literal() {
    // The tagged-union dispatch every Brood program writes: after `(= tag :ok)` fails,
    // a `(or :ok :err)` tag is `:err`. This needed the literal complement to be exact —
    // `¬:ok` used to widen to `any`, so the else branch learned nothing and the guard
    // was one-sided (`then_only`).
    let w = file_warnings(
        r#"
        (defn describe (tag)
          (if (%eq tag :ok) "fine" (string/length tag)))
        (sig describe ((or :ok :err) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains(":err")),
        "the else branch should know `tag` is `:err`, got {w:?}"
    );

    // …and the narrowing must not over-claim: a value the guard says nothing about
    // stays unnarrowed. `of_value` leaves a string literal flat (no heap for the
    // bytes), so `(= m "x")` proves only `m : string` and its negation proves nothing.
    let w = warnings(r#"(if (%eq m "x") :yes (string/length m))"#);
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "a non-literal guard type must stay one-sided: {w:?}"
    );
}

#[test]
fn a_record_shape_survives_keys_vals_assoc_and_dissoc() {
    // Closed records (ADR-264) made these sinks load-bearing rather than nice-to-have:
    // without them a closed record degrades to a flat `map` on its first update, and the
    // idiom that builds one field at a time loses its shape immediately.
    //
    // `assoc` adds the field it definitely puts there…
    let w = file_warnings(
        r#"
        (defn widen (r) (string/length (get (assoc r :count 1) :count)))
        (sig widen ((record :name string) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains("1")),
        "assoc should carry the shape forward with :count added, got {w:?}"
    );

    // …`dissoc` removes it, so reading it back is `nil`…
    let w = file_warnings(
        r#"
        (defn drop-it (r) (string/length (get (dissoc r :count) :count)))
        (sig drop-it ((record :name string :count int) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains("nil")),
        "dissoc should remove :count, so reading it yields nil, got {w:?}"
    );

    // …`keys` on a CLOSED record yields exactly the declared names…
    let w = file_warnings(
        r#"
        (defn ks (r) (string/length (first (keys r))))
        (sig ks ((record :name string :count int) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains(":count")),
        "keys should be the declared keyword literals, got {w:?}"
    );

    // …and `vals` the union of the declared field types. (A record with a `string`
    // field would NOT be flagged: the argument check fires on provable disjointness,
    // and a union containing `string` is not disjoint from it.)
    let w = file_warnings(
        r#"
        (defn vs (r) (string/length (first (vals r))))
        (sig vs ((record :count int :flag bool) -> any))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("string/length") && s.contains("int")),
        "vals should union the declared field types, got {w:?}"
    );
}

#[test]
fn an_open_record_declines_the_exhaustive_sinks() {
    // `keys`/`vals` read "these are ALL the keys", which is only true of a closed
    // record. An open one may carry keys nothing declares, so it must fall through to
    // the flat rule rather than claim a set it cannot know.
    let w = file_warnings(
        r#"
        (defn ks (r) (string/length (first (keys r))))
        (sig ks ((record &open :name string) -> any))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "an open record's keys are not exhaustively known: {w:?}"
    );
}

#[test]
fn assoc_widens_the_map_refinement_to_what_it_adds() {
    // `(assoc m :extra "text")` on a `(map keyword int)` genuinely holds a string at
    // `:extra`. Carrying `K`/`V` forward unchanged claimed otherwise, so reading the key
    // back gave `nil | int` and flagged correct code — a false positive on the one
    // operation everyone uses to build a map.
    let w = file_warnings(
        r#"
        (defn widen (m) (string/length (get (assoc m :extra "text") :extra)))
        (sig widen ((map keyword int) -> any))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("string/length")),
        "assoc must widen V to include the value it adds: {w:?}"
    );

    // The refinement still narrows what it can: adding an int keeps the value type
    // `int`, so a string read out of it is still flagged.
    let w = file_warnings(
        r#"
        (defn keep (m) (string/length (get (assoc m :extra 1) :extra)))
        (sig keep ((map keyword int) -> any))
        "#,
    );
    assert!(
        w.iter().any(|s| s.contains("string/length")),
        "adding an int must not widen V away: {w:?}"
    );
}

#[test]
fn a_parameter_in_call_head_position_is_callable() {
    // The callback shape. Without this a higher-order function's function parameter
    // types as `any`, so passing the sequence first — the classic argument-order slip —
    // is accepted in silence.
    let w = file_warnings(
        r#"
        (defn each-of (f xs) (f (first xs)))
        (defn misuse () (each-of 5 [1 2 3]))
        "#,
    );
    assert!(
        w.iter()
            .any(|s| s.contains("each-of") && s.contains("argument 1")),
        "a non-callable passed where the body calls it should be flagged: {w:?}"
    );

    // Callable is not just `fn`: a keyword is a function of a map, so passing one must
    // stay silent. (Maps, vectors and strings are NOT callable — each raises — so they
    // are correctly excluded.)
    let w = file_warnings(
        r#"
        (defn lookup (f m) (f m))
        (defn use-it () (lookup :a {:a 1}))
        "#,
    );
    assert!(
        w.iter().all(|s| !s.contains("lookup")),
        "a keyword IS callable on a map: {w:?}"
    );
}
