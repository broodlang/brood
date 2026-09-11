//! Record and ability names as types in a `sig`: resolution, ambiguity, sealed-over-kinds membership.

use super::*;

#[test]
fn a_qualified_ability_name_is_read_like_its_bare_spelling() {
    // `shapes/Shape` and `Shape` name the same ability — the registry is keyed by the bare
    // CamelCase name (ADR-255). In a PROJECT check both resolve and neither warns; this is
    // the loose single-file case, where neither can resolve because the defining module was
    // never loaded, and the checker falls back to "capitalised means an ability I can't see".
    // That fallback read the whole spelling, so `shapes/Shape` — starting with a lowercase
    // `s` — reported `unknown type` while the bare form was accepted silently. Naming the
    // module the ability comes from is not a mistake, and must not be the thing that
    // manufactures a diagnostic.
    let ws = file_warnings(
        "\
         (defmodule loose)\n\
         (sig a (Shape -> float))\n\
         (defn a (s) 1.0)\n\
         (sig b (shapes/Shape -> float))\n\
         (defn b (s) 1.0)",
    );
    assert!(
        !ws.iter().any(|w| w.contains("unknown type")),
        "neither spelling of an unloaded ability may warn — {ws:?}"
    );
    // The silence is still keyed on capitalisation, not on merely containing a slash: a
    // qualified LOWERCASE name is an ordinary unknown type and must still be reported.
    let ws = file_warnings(
        "\
         (defmodule loose)\n\
         (sig c (shapes/strng -> float))\n\
         (defn c (s) 1.0)",
    );
    assert!(
        ws.iter().any(|w| w.contains("unknown type `shapes/strng`")),
        "a qualified lowercase name is still an unknown type — {ws:?}"
    );
}

#[test]
fn a_record_name_is_a_type_in_a_sig() {
    // A record is the language's nominal type and `defrecord` already emits one in its own
    // constructor sig — but the NAME could not be written in type position, so the natural
    // spelling warned "unknown type `circle`" about a type the checker held in
    // `*record-ids*` all along. Sealed ability names have resolved since ADR-181; a record
    // is the more obvious case. Bare and qualified spellings both resolve.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (sig area (circle -> float))\n\
         (defn area (c) (get c :r))\n\
         (sig qual (t/circle -> float))\n\
         (defn qual (c) (get c :r))\n\
         (defn ok () (area (circle 2)))\n\
         (defn okq () (qual (circle 2)))\n\
         (defn wider () (area (assoc (circle 2) :z 3)))",
    );
    assert!(!ws.iter().any(|w| w.contains("unknown type")), "{ws:?}");
    // The shape is `:__id__`-only and OPEN, so a record carrying extra fields is still one.
    assert!(!ws.iter().any(|w| w.contains("argument 1")), "{ws:?}");
}

#[test]
fn a_record_type_rejects_a_different_record_and_a_non_record() {
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defrecord square (s))\n\
         (sig area (circle -> float))\n\
         (defn area (c) (get c :r))\n\
         (defn bad1 () (area (square 2)))\n\
         (defn bad2 () (area 42))",
    );
    assert_eq!(
        ws.iter().filter(|w| w.contains("argument 1")).count(),
        2,
        "{ws:?}"
    );
}

#[test]
fn a_base_type_name_outranks_a_record_that_claimed_it() {
    // In a TYPE EXPRESSION `int` means the int kind, even where a root-namespace record has
    // taken the id `:int`. That is the opposite precedence to `sealed_members_ty` — there
    // the members are `impl` dispatch keys, here they are type syntax.
    let ws = file_warnings(
        "\
         (defrecord int (n))\n\
         (sig f (int -> int))\n\
         (defn f (n) (+ n 1))\n\
         (defn ok () (f 42))",
    );
    assert!(!ws.iter().any(|w| w.contains("argument 1")), "{ws:?}");
}

#[test]
fn an_ambiguous_bare_record_name_declines_rather_than_guessing() {
    // Two modules may each define `pt`. Choosing between them would produce a WRONG type
    // where declining produces a missing one — the ADR-181 "sound, not complete" rule.
    let ws = file_warnings(
        "\
         (defmodule a)\n\
         (defrecord pt (x y))\n\
         (defmodule b)\n\
         (defrecord pt (m n))\n\
         (sig amb (pt -> int))\n\
         (defn amb (p) 1)",
    );
    assert!(ws.iter().any(|w| w.contains("unknown type `pt`")), "{ws:?}");
}

#[test]
fn sealed_over_builtin_kinds_accepts_its_own_members() {
    // `impl` dispatches on built-in kinds as well as records, so `:sealed [:int :float]` is
    // legal — but every member used to be turned into a record shape `%{__id__: :int}`, which
    // no int satisfies. The domain rejected its own members: `(use-it 42)` warned on a program
    // that runs correctly, and `nest check` exits nonzero on a warning, so this was a
    // CI-breaking false positive. Both the exhaustiveness gate and the reject-a-non-member
    // path only need the id set, so they were unaffected — passing a real member is the only
    // thing that exposed it.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defability Sizey :sealed [:int :float] (sizey [self] :-> int))\n\
         (impl Sizey :int (sizey [n] n))\n\
         (impl Sizey :float (sizey [n] 0))\n\
         (sig use-it (Sizey -> int))\n\
         (defn use-it (x) (sizey x))\n\
         (defn good-int () (use-it 42))\n\
         (defn good-float () (use-it 1.5))",
    );
    assert!(!ws.iter().any(|w| w.contains("argument 1")), "{ws:?}");
}

#[test]
fn sealed_over_builtin_kinds_still_rejects_a_non_member() {
    // The other half of the contract: widening to the kinds' own lattice points must not cost
    // the rejection. A string is provably neither an int nor a float.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defability Sizey :sealed [:int :float] (sizey [self] :-> int))\n\
         (impl Sizey :int (sizey [n] n))\n\
         (impl Sizey :float (sizey [n] 0))\n\
         (sig use-it (Sizey -> int))\n\
         (defn use-it (x) (sizey x))\n\
         (defn bad () (use-it \"not a number\"))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("argument 1") && w.contains("int") && w.contains("float")),
        "{ws:?}"
    );
}

#[test]
fn sealed_over_records_keeps_its_id_precision() {
    // The control for the two above: a purely-record seal must still denote the precise
    // `%{__id__: …}` shape, not widen to `map`. This is what drives a precise non-member
    // rejection and sealed-`match` exhaustiveness (ADR-187 part 2).
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord circle (r))\n\
         (defrecord rect (w h))\n\
         (defability Shape :sealed [circle rect] (area [self] :-> float))\n\
         (impl Shape t/circle (area [c] (get c :r)))\n\
         (impl Shape t/rect (area [r] (get r :w)))\n\
         (sig total (Shape -> float))\n\
         (defn total (s) (area s))\n\
         (defn good () (total (circle 2)))\n\
         (defn bad () (total 42))",
    );
    // Pin the RENDERING, not the internal `:__id__` marker: a record type now prints as the
    // name you would write in a `sig`, never as the shape that carries its identity.
    assert!(
        ws.iter()
            .any(|w| w.contains("argument 1") && w.contains("t/circle | t/rect")),
        "{ws:?}"
    );
    assert_eq!(
        ws.iter().filter(|w| w.contains("argument 1")).count(),
        1,
        "only the int is a non-member; the circle must pass — {ws:?}"
    );
}

#[test]
fn sealed_member_id_that_collides_with_a_kind_name_is_the_record() {
    // A record declared at ROOT namespace registers under its bare name, so
    // `(defrecord ratio …)` owns the id `:ratio` — the identical dispatch key the built-in
    // ratio kind uses (the language conflates them: a real `1/2` reaches that record's impl).
    // Classifying members by spelling alone therefore read this member as the KIND and
    // rejected the record — reintroducing, in a narrower case, the bug being fixed. The
    // registry breaks the tie, and it must see THIS file's records: `nest check` expands but
    // never evaluates, so `*record-ids*` alone does not know them.
    let ws = file_warnings(
        "\
         (defrecord ratio (n d))\n\
         (defability Sz :sealed [ratio] (sz [self] :-> int))\n\
         (impl Sz ratio (sz [v] 1))\n\
         (sig g (Sz -> int))\n\
         (defn g (v) (sz v))\n\
         (defn ok () (g (ratio 1 2)))",
    );
    assert!(!ws.iter().any(|w| w.contains("argument 1")), "{ws:?}");
}

#[test]
fn sealed_member_colliding_with_a_kind_name_still_rejects_a_non_member() {
    let ws = file_warnings(
        "\
         (defrecord ratio (n d))\n\
         (defability Sz :sealed [ratio] (sz [self] :-> int))\n\
         (impl Sz ratio (sz [v] 1))\n\
         (sig g (Sz -> int))\n\
         (defn g (v) (sz v))\n\
         (defn bad () (g \"x\"))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("argument 1") && w.contains("ratio")),
        "{ws:?}"
    );
}

#[test]
fn sealed_mixing_records_and_kinds_accepts_every_member() {
    // A seal may name both. The record half degrades to `map` in the union (`Ty::union`
    // widens a differing `fields` map away — sound, since it only ever accepts MORE), so a
    // mixed seal trades id precision for coverage. What must not happen is a member being
    // rejected.
    let ws = file_warnings(
        "\
         (defmodule t)\n\
         (defrecord money (cents))\n\
         (defability Amt :sealed [:int :float money] (amt [self] :-> float))\n\
         (impl Amt :int (amt [n] (* 1.0 n)))\n\
         (impl Amt :float (amt [n] n))\n\
         (impl Amt t/money (amt [m] (* 0.01 (get m :cents))))\n\
         (sig twice (Amt -> float))\n\
         (defn twice (x) (* 2.0 (amt x)))\n\
         (defn a () (twice 42))\n\
         (defn b () (twice 1.5))\n\
         (defn c () (twice (money 500)))",
    );
    assert!(!ws.iter().any(|w| w.contains("argument 1")), "{ws:?}");
}

// `(deftype name T)` names a type for the checker (ADR-327): structural, so `name` in a
// `sig` IS `T` — a record shape, a tuple, another alias. Resolved for THIS file from its
// expanded `%register-type` forms (a checked file is never evaluated), qualified to the
// file's namespace; a recursive alias reads as `any` rather than looping; an unknown name
// is still an unknown type.
#[test]
fn deftype_names_a_structural_type_for_sigs() {
    let src = "\
         (defmodule t)\n\
         (deftype pane (record &open :rect (tuple int int int int) :selected bool))\n\
         (deftype cell (tuple int int))\n\
         (deftype maybe-cell (or nil cell))\n\
         (deftype loopy (or nil (vector loopy)))\n\
         (sig rows (pane -> int))\n\
         (defn rows (p) (let ([x y w h] (:rect p)) (math/quot (dec h) 2)))\n\
         (sig bad-field (pane -> int))\n\
         (defn bad-field (p) (string/length (:rect p)))\n\
         (sig first-of (maybe-cell -> int))\n\
         (defn first-of (c) (if (nil? c) 0 (first c)))\n\
         (sig f-loop (loopy -> int))\n\
         (defn f-loop (l) 0)\n\
         (sig f-unknown (nosuch -> int))\n\
         (defn f-unknown (x) 0)\n\
         (defn bad-call () (rows \"not a pane\"))\n\
         (defn own-ns () (rows {:rect [0 0 80 24] :selected true}))";
    let strict = file_warnings_mode(src, true);
    // the alias resolves: a field read through it is typed, and a wrong argument is named
    assert!(
        strict.iter().any(|w| w
            .contains("string/length: argument 1 expects string, got (tuple int, int, int, int)")),
        "{strict:?}"
    );
    assert!(
        strict.iter().any(|w| w.contains("t/rows: argument 1 expects {rect: (tuple int, int, int, int), selected: bool, ...}, got \"not a pane\"")),
        "{strict:?}"
    );
    // an unknown name is still reported (ADR-259); the alias names are not
    assert!(
        strict
            .iter()
            .any(|w| w.contains("sig f-unknown: unknown type `nosuch`")),
        "{strict:?}"
    );
    assert!(
        !strict.iter().any(|w| w.contains("unknown type `pane`")
            || w.contains("unknown type `cell`")
            || w.contains("unknown type `maybe-cell`")
            || w.contains("unknown type `loopy`")),
        "{strict:?}"
    );
    // `(dec h)` off the tuple is an int, `(first c)` off the narrowed alias is an int, and a
    // literal that fits the shape passes — strictly.
    assert!(
        !strict
            .iter()
            .any(|w| w.contains("t/rows: argument 1") && w.contains("own-ns")
                || w.contains("math/quot")
                || w.contains("first-of")),
        "{strict:?}"
    );
    assert_eq!(strict.len(), 3, "{strict:?}");
}
