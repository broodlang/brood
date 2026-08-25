//! Regression guards for the **single-file** advisory check (`brood --check FILE`,
//! i.e. `types::check::check_file` against an image holding only the prelude) and
//! `(:use …)` name resolution.
//!
//! The bug these pin down: `setup_check_imports` decided a `(:use M)` target was
//! "already loaded" by asking whether *any* `M/…` global existed. For a std module
//! sharing its namespace with kernel primitives — `file` has 18 `file/…` primitives
//! (`file/slurp`, `file/ls`, …) with `std/file.blsp` unread — that test said yes
//! before the module's source had ever been loaded. The `(:use file)` then imported
//! the primitives only, so every Brood-level name in the module (`walk-files`,
//! `read-lines`, `regular?`) was reported `unbound symbol:` …and, because none of
//! the primitives' bare names were referenced either, the same run *also* said
//! `unused :use import: file` — advice that would have the reader delete the import
//! their program needs. The whole-project path (`nest check`) never saw it: it loads
//! every module before checking.
//!
//! The checker is advisory and must never warn on a use that is valid for the
//! image's current state (ADR-123/124/125/126) — and these two warnings are
//! mutually contradictory, so the pair must be impossible to emit together.

use brood::types::check::check_file;
use brood::Interp;

/// The warnings a self-contained source draws on the single-file path: a fresh
/// `Interp` (prelude only — no project, no pre-loaded modules) and the same
/// `check_file` entry point `brood --check` calls.
fn warnings(src: &str) -> Vec<String> {
    let mut interp = Interp::new();
    let forms = brood::syntax::reader::read_all(&mut interp.heap, src).expect("parse");
    check_file(&mut interp.heap, &forms)
        .into_iter()
        .map(|(_, msg)| msg)
        .collect()
}

/// The reported bug, exactly: a bare-file check of `(:use file)` + `walk-files`.
/// `walk-files` is `(defn walk-files (dir) …)` in `std/file.blsp` and resolves at
/// run time, so neither warning may fire.
#[test]
fn use_import_resolves_a_module_level_name() {
    let ws = warnings("(defmodule wftest (:use file))\n(println (fn? walk-files))");
    assert!(ws.is_empty(), "expected a clean check, got {ws:?}");
}

/// The same for the module's other Brood-level names — the ones the sibling reports
/// hit (`regular?` in `std/net/http.blsp`, `walk-files` in `std/tool/codemod.blsp`).
#[test]
fn use_import_resolves_every_module_level_name() {
    let ws = warnings(
        "(defmodule wftest (:use file))\n\
         (defn probe (p) (list (regular? p) (read-lines p) (list-files p) (list-dirs p)))",
    );
    assert!(ws.is_empty(), "expected a clean check, got {ws:?}");
}

/// The already-loaded path is unchanged: `string` is `require`d during boot, so its
/// `:use` never reaches the loader at all. (`file` is the only namespace a fresh
/// image shares between kernel primitives and an *unloaded* module — `map`, `seq`
/// and `string` are the other three prefixes present at boot, and all three are
/// loaded features — so this is the non-regression half of the same test.)
#[test]
fn use_import_of_an_already_loaded_module_still_resolves() {
    let ws = warnings("(defmodule st (:use string))\n(defn probe (s) (blank? s))");
    assert!(ws.is_empty(), "expected a clean check, got {ws:?}");
}

/// **The lint is not silenced.** A name no module provides is still reported, so
/// the false positive wasn't "fixed" by turning the diagnostic off.
#[test]
fn a_genuinely_unbound_name_still_warns() {
    let ws = warnings("(defmodule wftest (:use file))\n(println (fn? walk-filez))");
    assert!(
        ws.iter().any(|w| w.contains("unbound symbol: walk-filez")),
        "a real typo must still be flagged, got {ws:?}"
    );
}

/// …including a typo in the *qualified* spelling of a module that is now genuinely
/// loaded — the module being resolvable is what makes this provable.
#[test]
fn a_qualified_typo_in_a_used_module_still_warns() {
    let ws = warnings("(defmodule wftest (:use file))\n(println (fn? file/walk-filez))");
    assert!(
        ws.iter()
            .any(|w| w.contains("unbound symbol: file/walk-filez")),
        "a real qualified typo must still be flagged, got {ws:?}"
    );
}

/// The unused-import lint keeps working on the single-file path for a `:use` the
/// file genuinely never touches (the fix must not have blanket-disabled it).
#[test]
fn a_genuinely_unused_use_import_still_warns() {
    let ws = warnings("(defmodule wftest (:use file))\n(defn foo (x) (+ x 1))");
    assert!(
        ws.iter()
            .any(|w| w.contains("unused :use import") && w.contains("file")),
        "an untouched :use must still be flagged, got {ws:?}"
    );
}

/// The two diagnostics are mutually contradictory — "this name is unbound" and
/// "this import contributes nothing you use" cannot both be true advice — so they
/// may never appear in one file's warnings. Checked over sources that provoke each.
#[test]
fn unbound_and_unused_import_are_never_reported_together() {
    for src in [
        "(defmodule wftest (:use file))\n(println (fn? walk-files))",
        "(defmodule wftest (:use file))\n(println (fn? walk-filez))",
        "(defmodule wftest (:use file))\n(defn foo (x) (+ x 1))",
        "(defmodule wftest (:use file))\n(defn foo (x) (nope-zzz x))",
    ] {
        let ws = warnings(src);
        let unbound_bare = ws.iter().any(|w| {
            w.strip_prefix("unbound symbol: ")
                .is_some_and(|rest| !rest.split(' ').next().unwrap_or("").contains('/'))
        });
        let unused = ws.iter().any(|w| w.contains("unused :use import"));
        assert!(
            !(unbound_bare && unused),
            "contradictory pair emitted for {src:?}: {ws:?}"
        );
    }
}

/// A `(bound? 'name)`-guarded reference to an ambient another module defines
/// (`*project-name*` / `*ns-package*`, `defdyn`'d by `std/tool/project.blsp` and
/// absent under a bare `brood script.blsp`) is correct code *because* of the guard
/// — `std/prelude/tools.blsp`'s `impl-app?` is the in-tree instance.
#[test]
fn a_bound_guarded_ambient_is_not_unbound() {
    let ws = warnings(
        "(defn app? (from)\n  (and from (bound? '*absent-ambient-zzz*) *absent-ambient-zzz*))",
    );
    assert!(ws.is_empty(), "expected a clean check, got {ws:?}");
}

/// …and the exemption is scoped to the form that does the guarding: an unguarded
/// reference in another function is still flagged.
#[test]
fn an_unguarded_reference_elsewhere_still_warns() {
    let ws = warnings(
        "(defn app? (from) (and from (bound? '*absent-ambient-zzz*) *absent-ambient-zzz*))\n\
         (defn other () (println *absent-ambient-zzz*))",
    );
    assert!(
        ws.iter()
            .any(|w| w.contains("unbound symbol: *absent-ambient-zzz*")),
        "an unguarded reference must still be flagged, got {ws:?}"
    );
}
