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

// ---------- scaffolding ----------

struct TempDir {
    path: std::path::PathBuf,
}
impl TempDir {
    fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir(tag: &str) -> TempDir {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(path.join("src")).unwrap();
    TempDir { path }
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

/// A minimal project with an entry point — the shape `nest new` produces, minus
/// the parts these gates don't read.
fn project(tag: &str) -> TempDir {
    let tmp = tempdir(tag);
    let root = tmp.path();
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
    let root = tmp.path();
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
    let root = tmp.path();

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
    let root = tmp.path();
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

// ---------- nest check --fix-renames ----------

/// A project carrying one of each rename-recovery class, so a single run exercises
/// every branch of the decision.
fn rot_project(tag: &str) -> TempDir {
    let tmp = project(tag);
    let root = tmp.path();
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
    let root = tmp.path();
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
    let root = tmp.path();
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
fn fix_renames_is_quiet_and_clean_on_a_healthy_project() {
    // The negative control. Without it the assertions above could all be satisfied by a
    // tool that reports rot unconditionally.
    let tmp = project("fixclean");
    let (code, out) = nest(tmp.path(), &["check", "--fix-renames", "--dry-run"]);
    assert_eq!(code, 0, "a clean project must exit 0:\n{out}");
    assert!(
        out.contains("no unbound names"),
        "a clean project must say there is nothing to do:\n{out}"
    );
}
