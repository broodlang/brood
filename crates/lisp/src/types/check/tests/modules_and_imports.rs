//! Modules as the checker sees them: the unused `:use` lint, keyword accessors, `arg_ty_at`, the feature registry, same-file arity.

use super::*;

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

/// A module imported only for a MACRO is used. The macro head does not survive
/// expansion, so scanning only the expanded tree reported `gen` — whose `defserver`
/// is the single most likely reason to import it — as unused, and the fix it advised
/// (delete the `:use`) leaves the file unable to expand at all.
#[test]
fn a_use_import_reached_only_through_a_macro_is_not_unused() {
    let ws =
        file_warnings("(defmodule test/mod (:use gen))\n(defserver s (n)\n  (cast :inc (+ n 1)))");
    assert!(
        !ws.iter().any(|w| w.contains("unused :use import")),
        "a macro-only :use import is load-bearing, got {ws:?}"
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
            .any(|w| w.contains("f: expected 2 arguments, got 1")),
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
            .any(|w| w.contains("v: expected at least 1 argument, got 0")),
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
            .any(|w| w.contains("f: expected 1 to 2 arguments, got 3")),
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
            .any(|w| w.contains("f: expected 2 arguments, got 1")),
        "{ws:?}"
    );
}

/// A call into a module the checker cannot resolve is **opaque**: it might be a
/// macro, so its arguments might be syntax rather than code, and reporting them
/// as unbound is a false positive.
///
/// The shape that found it (wos, an OS written in Brood): an aarch64 assembler
/// exposed as `(asm/asm (movz x0 …) (adr x1 …) …)`, reached through a load path
/// the program adds at runtime — so at check time no `asm/*` is loaded. Every
/// mnemonic was reported as an unbound symbol on a file that assembles, boots
/// and prints. `is_unbound` already declines to flag the HEAD for exactly this
/// reason; the walk then descended into the arguments anyway, leaving the
/// checker silent about the thing it did not know and loud about everything
/// that followed from it.
#[test]
fn an_unresolvable_qualified_head_makes_its_arguments_opaque() {
    let ws = file_warnings("(defmodule test/mod)\n(defn build () (nosuchmod/emit (movz x0 255) (wfe)))");
    assert!(
        !ws.iter().any(|w| w.contains("unbound symbol: movz")
            || w.contains("unbound symbol: wfe")
            || w.contains("unbound symbol: x0")),
        "arguments of an unresolvable qualified head may be macro syntax, got {ws:?}"
    );
}

/// The other half of the same rule, and the reason it is stated on the *module*
/// and not on "anything unresolvable": a **bare** head that resolves to nothing
/// is a typo, not a possible macro, so it and its arguments are still reported.
/// A fix that silenced those would trade one false positive for a blind spot in
/// the lint people rely on most.
#[test]
fn a_bare_unresolvable_head_still_reports_its_arguments() {
    let ws = file_warnings("(defmodule test/mod)\n(defn build () (nosuchfn (alsomissing 1)))");
    assert!(
        ws.iter().any(|w| w.contains("unbound symbol: nosuchfn")),
        "a bare unresolvable head is a typo and must be reported, got {ws:?}"
    );
    assert!(
        ws.iter().any(|w| w.contains("unbound symbol: alsomissing")),
        "its arguments are evaluated code and must be reported, got {ws:?}"
    );
}

/// And a typo in a module that IS loaded stays reported, arguments included —
/// the carve-out keys on the module being unknown, never on the name.
#[test]
fn a_known_module_with_an_unknown_name_still_reports() {
    let ws = file_warnings("(defmodule test/mod)\n(defn build () (string/nosuchname (alsomissing 1)))");
    assert!(
        ws.iter().any(|w| w.contains("unbound symbol: string/nosuchname")),
        "a typo in a known module is still a typo, got {ws:?}"
    );
}
