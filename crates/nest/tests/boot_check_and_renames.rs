//! End-to-end tests for the two toolchain gates a downstream migration exposed
//! (KI-66/KI-67): `nest run --check-boot` and `nest check --fix-renames`.
//!
//! Both run the real `nest` binary in a child process against a throwaway project.
//!
//! **Every case here asserts the gate can FAIL, not only that it can pass.** That is the
//! standing lesson of the dead-gates session: `check-boot` returning 0 proves nothing on
//! its own — a boot check that never loads anything returns 0 too — so each positive case
//! is paired with a sabotage that must turn it red.

use std::path::Path;
use std::process::Command;

mod support;
use support::{TempDir, temp_dir};

// ---------- scaffolding ----------

fn tempdir(tag: &str) -> TempDir {
    let tmp = temp_dir(tag);
    std::fs::create_dir_all(tmp.path.join("src")).unwrap();
    tmp
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

/// A minimal project with an entry point — the shape `nest new` produces, minus
/// the parts these gates don't read.
fn project(tag: &str) -> TempDir {
    let tmp = tempdir(tag);
    let root = &tmp.path;
    write(&root.join("project.blsp"), "(project :name scratch)\n");
    write(
        &root.join("src/main.blsp"),
        "(defmodule main \"entry\")\n\n(defn main (& _args) 0)\n",
    );
    tmp
}

/// Run `nest` in `dir`, returning `(exit-code, stdout+stderr)`.
fn nest(dir: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run nest");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

// ---------- nest run --check-boot ----------

#[test]
fn check_boot_passes_then_fails_on_a_load_time_unbound_name() {
    let tmp = project("bootchk");
    let root = &tmp.path;
    write(
        &root.join("src/helper.blsp"),
        "(defmodule helper \"a sibling module\")\n\n(defn helped () 1)\n",
    );

    let (code, out) = nest(root, &["run", "--check-boot"]);
    assert_eq!(code, 0, "a healthy project must boot-check clean:\n{out}");
    // Package-rooted (ADR-070), so the entry reads `scratch/main/main`, not `main/main`.
    assert!(
        out.contains("boot ok: ") && out.contains("main/main"),
        "the check must name the entry it resolved:\n{out}"
    );
    assert!(
        out.contains("nothing run"),
        "it must say it ran nothing — that is the whole contract:\n{out}"
    );

    // SABOTAGE, the exact KI-66 shape: a stale name in a TOP-LEVEL form, so it raises
    // during `require` rather than when something calls it. `nest check` sees a warning
    // here and `nest test` never loads the module at all; only a boot fails.
    write(
        &root.join("src/helper.blsp"),
        "(defmodule helper \"a sibling module\")\n\n\
         (def stale (int->char 65))\n\n(defn helped () 1)\n",
    );
    let (code, out) = nest(root, &["run", "--check-boot"]);
    assert_ne!(
        code, 0,
        "a load-time unbound name must fail the boot check:\n{out}"
    );
    assert!(
        out.contains("int->char"),
        "the failure must name the symbol that killed the boot:\n{out}"
    );
}

#[test]
fn check_boot_fails_when_the_entry_point_is_missing() {
    let tmp = project("bootentry");
    let root = &tmp.path;

    // Every module loads cleanly; only `:main` is unresolvable. This is the half a
    // load-everything check would still miss if it stopped at loading.
    write(
        &root.join("src/main.blsp"),
        "(defmodule main \"entry\")\n\n(defn not-main (& _args) 0)\n",
    );
    let (code, out) = nest(root, &["run", "--check-boot"]);
    assert_ne!(code, 0, "a missing entry fn must fail:\n{out}");
    assert!(
        out.contains("is not defined"),
        "the failure must say the entry fn is not defined:\n{out}"
    );
}

#[test]
fn check_boot_runs_nothing() {
    let tmp = project("bootnorun");
    let root = &tmp.path;
    // `main` would raise if it were called. The check must resolve it and stop.
    write(
        &root.join("src/main.blsp"),
        "(defmodule main \"entry\")\n\n(defn main (& _args) (error \"main was CALLED\"))\n",
    );
    let (code, out) = nest(root, &["run", "--check-boot"]);
    assert_eq!(code, 0, "resolving an entry must not call it:\n{out}");
    assert!(
        !out.contains("main was CALLED"),
        "the boot check invoked :main — it must only resolve it:\n{out}"
    );
}

#[test]
fn check_boot_starts_and_stops_an_application_and_fails_when_it_cannot_start() {
    // ADR-436: an application's `:start` is where a missing dependency or a bad child spec
    // dies, so for an application `:main` the check starts it and stops it.
    let tmp = project("bootapp");
    let root = &tmp.path;
    let main_source = |store_start: &str| {
        format!(
            "(defmodule main \"entry\")\n\n\
             (application/defapp store\n  \
               {{:start (fn () {store_start}) :stop (fn (_root) (io/puts \"store stopped\"))}})\n\n\
             (application/defapp main\n  \
               {{:start (fn () (io/puts \"main started\") :main)\n   \
                 :stop (fn (_root) (io/puts \"main stopped\"))\n   \
                 :requires [store]}})\n"
        )
    };
    write(
        &root.join("src/main.blsp"),
        &main_source("(io/puts \"store started\") :store"),
    );
    let (code, out) = nest(root, &["run", "--check-boot"]);
    assert_eq!(
        code, 0,
        "a healthy application must boot-check clean:\n{out}"
    );
    let order: Vec<&str> = out
        .lines()
        .filter(|line| line.ends_with(" started") || line.ends_with(" stopped"))
        .collect();
    assert_eq!(
        order,
        [
            "store started",
            "main started",
            "main stopped",
            "store stopped"
        ],
        "requirements start first and everything stops in reverse:\n{out}"
    );
    assert!(
        out.contains("started and stopped store, main"),
        "the check must say what it started:\n{out}"
    );

    // SABOTAGE: a start that raises, the way a missing dependency does at boot.
    write(
        &root.join("src/main.blsp"),
        &main_source("(error \"store has no disk\")"),
    );
    let (code, out) = nest(root, &["run", "--check-boot"]);
    assert_ne!(
        code, 0,
        "an application that cannot start must fail:\n{out}"
    );
    assert!(
        out.contains("store has no disk"),
        "the failure must carry the start's error:\n{out}"
    );
    assert!(
        !out.contains("main started"),
        "nothing that requires a failed start may start:\n{out}"
    );
}

// ---------- nest check --fix-renames ----------

/// A project carrying one of each rename-recovery class, so a single run exercises
/// every branch of the decision.
fn rot_project(tag: &str) -> TempDir {
    let tmp = project(tag);
    let root = &tmp.path;
    write(
        &root.join("src/rot.blsp"),
        // `lonely` imports nothing, so a bare reference to a sibling's public name is
        // genuinely unbound here — that is what makes the project-owned case reachable.
        "(defmodule rot \"deliberate rename-wave rot\")\n\n\
         (defn moved (n) (int->char n))          ; public move -> string/int->char\n\
         (defn withdrawn (m) (map-pairs m))      ; moved behind % -> %map-pairs\n\
         (defn dead (x) (no-such-name-at-all x)) ; rot: defined nowhere\n\
         (defn owned (x) (sibling-fn x))         ; this project's own, in another module\n\
         (defn prose () \"mentions int->char\" nil) ; int->char in a comment\n",
    );
    write(
        &root.join("src/sibling.blsp"),
        "(defmodule sibling \"owns a name `rot` reaches for bare\")\n\n(defn sibling-fn (x) x)\n",
    );
    tmp
}

#[test]
fn fix_renames_dry_run_classifies_without_writing() {
    let tmp = rot_project("fixdry");
    let root = &tmp.path;
    let before = std::fs::read_to_string(root.join("src/rot.blsp")).unwrap();

    let (_code, out) = nest(root, &["check", "--fix-renames", "--dry-run"]);

    assert!(
        out.contains("would fix: int->char → string/int->char"),
        "the unambiguous public move is the one fix to propose:\n{out}"
    );
    // The three declines, each for its own stated reason — the reason is the product
    // here, since a bare "skipped" would leave the same guesswork the tool exists to end.
    assert!(
        out.contains("map-pairs") && out.contains("%map-pairs"),
        "a name withdrawn behind `%` must be NAMED, not silently skipped:\n{out}"
    );
    assert!(
        out.contains("sibling-fn") && out.contains("this project defines it"),
        "a name this project owns must be declined as an import, not renamed:\n{out}"
    );
    assert!(
        out.contains("no-such-name-at-all") && out.contains("rot, not a rename"),
        "a name defined nowhere must be called rot:\n{out}"
    );

    assert_eq!(
        std::fs::read_to_string(root.join("src/rot.blsp")).unwrap(),
        before,
        "--dry-run must not write"
    );
}

#[test]
fn fix_renames_rewrites_references_only() {
    let tmp = rot_project("fixapply");
    let root = &tmp.path;
    let sibling_before = std::fs::read_to_string(root.join("src/sibling.blsp")).unwrap();

    let (_code, out) = nest(root, &["check", "--fix-renames"]);
    assert!(out.contains("fix:"), "expected an applied fix:\n{out}");

    let after = std::fs::read_to_string(root.join("src/rot.blsp")).unwrap();
    assert!(
        after.contains("(string/int->char n)"),
        "the call site must be qualified:\n{after}"
    );
    // The CST rewrite is symbol-tokens-only: a `;` comment naming the same identifier is
    // data, not a call, and a rename wave that edits prose is how one corrupts a repo.
    assert!(
        after.contains("; public move -> string/int->char"),
        "the trailing comment must be byte-identical:\n{after}"
    );
    assert!(
        after.contains("\"mentions int->char\""),
        "a docstring naming the identifier must not be rewritten:\n{after}"
    );
    // Declined names stay exactly as they were.
    assert!(
        after.contains("(map-pairs m)") && after.contains("(sibling-fn x)"),
        "declined names must be left alone:\n{after}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/sibling.blsp")).unwrap(),
        sibling_before,
        "a definition head must never move — that is what produced the reserved \
         `(defn proc/register …)` and cost a revert"
    );
}

#[test]
fn fix_renames_leaves_a_local_spelled_like_the_renamed_name_alone() {
    // KI-385: the rewrite renamed every symbol spelled like the stale name, so a parameter,
    // a `let` binder and a pattern variable spelled `int->char` became `string/int->char`
    // along with their uses. Only the references the checker reports are rewritten now —
    // including one a macro moved (`->` reports its own form, not the symbol).
    let tmp = project("fixscope");
    let root = &tmp.path;
    write(
        &root.join("src/scope.blsp"),
        "(defmodule scope \"a stale global beside locals spelled the same\")\n\n\
         (defn moved (n) (int->char n))\n\
         (defn threaded (n) (-> n int->char))\n\
         (defn param (int->char) (str int->char))\n\
         (defn binder (n) (let (int->char (+ n 1)) (* int->char 2)))\n\
         (defn pattern (v) (match v ([int->char] int->char) (_ (int->char 66))))\n",
    );

    let (_code, out) = nest(root, &["check", "--fix-renames"]);
    assert!(
        out.contains("fix:  int->char → string/int->char"),
        "expected the fix to apply:\n{out}"
    );

    let after = std::fs::read_to_string(root.join("src/scope.blsp")).unwrap();
    for expected in [
        "(defn moved (n) (string/int->char n))",
        "(defn threaded (n) (-> n string/int->char))",
        "(defn param (int->char) (str int->char))",
        "(defn binder (n) (let (int->char (+ n 1)) (* int->char 2)))",
        "(defn pattern (v) (match v ([int->char] int->char) (_ (string/int->char 66))))",
    ] {
        assert!(
            after.contains(expected),
            "expected `{expected}` in:\n{after}"
        );
    }

    let (code, out) = nest(root, &["check"]);
    assert_eq!(code, 0, "the rewritten project checks clean:\n{out}");
}

#[test]
fn fix_renames_is_quiet_and_clean_on_a_healthy_project() {
    // The negative control. Without it the assertions above could all be satisfied by a
    // tool that reports rot unconditionally.
    let tmp = project("fixclean");
    let (code, out) = nest(&tmp.path, &["check", "--fix-renames", "--dry-run"]);
    assert_eq!(code, 0, "a clean project must exit 0:\n{out}");
    assert!(
        out.contains("no unbound names"),
        "a clean project must say there is nothing to do:\n{out}"
    );
}

// ---------- nest run's launch gate + the rename ledger (ADR-304) ----------

/// A project whose entry reaches a ledgered rename inside a blanket guard — the bedit
/// shape: `nest run` used to print the warning and launch anyway.
fn ledger_project(tag: &str) -> TempDir {
    let tmp = project(tag);
    let root = &tmp.path;
    write(
        &root.join("src/main.blsp"),
        "(defmodule main \"entry\")\n\n\
         (defn main (& _args)\n  (try (gui/font! 1) (catch e nil))\n  (io/puts \"launched\")\n  0)\n",
    );
    tmp
}

#[test]
fn run_refuses_to_launch_over_an_unbound_symbol() {
    let tmp = ledger_project("rungate");
    let root = &tmp.path;

    let (code, out) = nest(root, &["run"]);

    assert_ne!(code, 0, "an unbound symbol must refuse the launch:\n{out}");
    assert!(
        out.contains("unbound symbol: gui/font! — renamed to gui/font (ADR-302)"),
        "the warning names where the name went:\n{out}"
    );
    assert!(
        out.contains("nest run: 1 unbound symbol(s)"),
        "the one-line verdict:\n{out}"
    );
    assert!(
        !out.contains("launched"),
        "the program must not have run:\n{out}"
    );
}

#[test]
fn run_no_check_launches_regardless() {
    let tmp = ledger_project("runnocheck");
    let root = &tmp.path;

    let (code, out) = nest(root, &["run", "--no-check"]);

    assert_eq!(code, 0, "--no-check launches:\n{out}");
    assert!(out.contains("launched"), "the program ran:\n{out}");
}

#[test]
fn fix_renames_consults_the_ledger_first() {
    let tmp = ledger_project("fixledger");
    let root = &tmp.path;

    let (_code, out) = nest(root, &["check", "--fix-renames"]);

    assert!(
        out.contains("fix:  gui/font! → gui/font (ADR-302)"),
        "a ledgered QUALIFIED rename is fixed exactly, with its ADR:\n{out}"
    );
    let after = std::fs::read_to_string(root.join("src/main.blsp")).unwrap();
    assert!(
        after.contains("(gui/font 1)") && !after.contains("gui/font!"),
        "{after}"
    );

    // And the launch gate is satisfied by the rewrite.
    let (code, out) = nest(root, &["run"]);
    assert_eq!(code, 0, "after the fix, nest run launches:\n{out}");
}
