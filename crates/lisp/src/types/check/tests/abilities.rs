//! Abilities and behaviours: impl conformance, missing impls at call sites, sealed members, typed op parameters and returns, `(:implements …)`.

use super::*;

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
