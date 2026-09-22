//! Runtime contracts are ON by default in dev mode (ADR-381): `nest run` and `nest test`
//! enforce every `(sig …)` — the project's own and std's — unless `BROOD_CONTRACTS` says
//! otherwise, and the stdlib image, which every unarmed run reads too, never carries a shim.
//!
//! Each case is a minimal project written by hand (no scaffold: the point is the mode, and
//! a scaffold costs ~18 s). The program declares a lying signature and calls it, and calls a
//! std function with the wrong argument; a run with contracts armed must raise a BLAMED
//! contract error for each, and `BROOD_CONTRACTS=0` must let the same program through.
//!
//! The std case is the load-bearing one: a std module reaches a `nest run` from the stdlib
//! IMAGE, which evaluates no `(sig …)` form, so it is only contracted if enforcement is a
//! property of the binding and not of the declaring form — exactly what ADR-381 changed. So
//! the image is BUILT first, into a private cache, and the run is checked to have read it.

use std::path::{Path, PathBuf};
use std::process::Command;

struct TempDir {
    path: PathBuf,
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
    let path =
        std::env::temp_dir().join(format!("brood-contracts-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    TempDir { path }
}

struct Run {
    out: String,
    ok: bool,
}

/// `nest args…` in `dir`, with `XDG_CACHE_HOME` at `cache` so the stdlib image this test
/// builds is the one the run reads, and `BROOD_CONTRACTS` as `contracts` says (None
/// removes it, i.e. the default).
fn nest(dir: &Path, cache: &Path, contracts: Option<&str>, args: &[&str]) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nest"));
    cmd.current_dir(dir)
        .args(args)
        .env("XDG_CACHE_HOME", cache)
        .env_remove("BROOD_NO_STDIMAGE");
    match contracts {
        Some(v) => cmd.env("BROOD_CONTRACTS", v),
        None => cmd.env_remove("BROOD_CONTRACTS"),
    };
    let out = cmd.output().expect("run nest");
    Run {
        out: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        ok: out.status.success(),
    }
}

/// A project whose `main` reports, for three calls, whether a contract raised: `liar/lies`
/// is declared `int -> int` and returns a string (the callee's fault) — called from `main`,
/// across the module boundary, and from `liar/inside`, within it, where a contract does not
/// apply (ADR-383); `string/pad-left` is handed an int where its declared `string` belongs
/// (the caller's fault).
fn write_project(root: &Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"cdef\" :version \"0.1.0\")\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/liar.blsp"),
        "(defmodule liar)\n\
         (sig lies (int -> int))\n\
         (check-allow :type-mismatch (defn lies (n) \"not an int\"))\n\
         (defn inside (n) (lies n))\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.blsp"),
        "(defmodule main (:use liar))\n\
         (defn- report (label thunk)\n\
         \x20\x20(io/puts label (try (thunk) (catch e (str \"RAISED blame=\" (get e :blame) \" :: \" (error-message e))))))\n\
         (defn main ()\n\
         \x20\x20(report \"own:\" (fn () (lies 1)))\n\
         \x20\x20(report \"inside:\" (fn () (inside 1)))\n\
         \x20\x20(report \"std:\" (fn () (check-allow :type-mismatch (string/pad-left 5 \"x\"))))\n\
         \x20\x20(io/puts \"image:\" (get (stdimage/status) :installed)))\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/main_test.blsp"),
        "(defmodule main-test (:use test) (:use liar))\n\
         (describe \"contracts in nest test\"\n\
         \x20\x20(test \"a lying sig raises under the default\"\n\
         \x20\x20\x20\x20(io/puts \"test-own:\" (try (lies 1) (catch e (str \"RAISED blame=\" (get e :blame)))))))\n",
    )
    .unwrap();
}

/// Build the stdlib image into `cache` — unarmed on purpose, as `nest` itself does — and
/// prove it is there.
fn build_image(root: &Path, cache: &Path) {
    let built = nest(root, cache, Some("0"), &["stdimage"]);
    assert!(built.ok, "nest stdimage failed:\n{}", built.out);
}

#[test]
fn nest_run_enforces_own_and_std_contracts_by_default() {
    let tmp = tempdir("run");
    let root = tmp.path.join("cdef");
    let cache = tmp.path.join("cache");
    write_project(&root);
    build_image(&root, &cache);

    let ran = nest(&root, &cache, None, &["run"]);
    assert!(
        ran.ok,
        "nest run should complete (the program catches):\n{}",
        ran.out
    );
    assert!(
        ran.out
            .contains("own: RAISED blame=:callee :: cdef/liar/lies: result expected int"),
        "the project's own lying sig must raise, blaming the callee:\n{}",
        ran.out
    );
    // …but not for the module's own call to it: a contract guards the boundary (ADR-383).
    assert!(
        ran.out.contains("inside: not an int"),
        "a module's call to its own contracted function must not be checked:\n{}",
        ran.out
    );
    assert!(
        ran.out
            .contains("std: RAISED blame=:caller :: string/pad-left: argument 1 expected string"),
        "a std declaration must be enforced too, blaming the caller:\n{}",
        ran.out
    );
    // …and it was enforced on a module the IMAGE materialised, not one loaded from source.
    assert!(
        !ran.out.contains("image: nil"),
        "the run did not read the stdlib image, so the std case proves nothing:\n{}",
        ran.out
    );
}

#[test]
fn brood_contracts_zero_opts_out_of_the_default() {
    let tmp = tempdir("optout");
    let root = tmp.path.join("cdef");
    let cache = tmp.path.join("cache");
    write_project(&root);
    build_image(&root, &cache);

    let ran = nest(&root, &cache, Some("0"), &["run"]);
    assert!(ran.ok, "nest run should complete:\n{}", ran.out);
    assert!(
        ran.out.contains("own: not an int"),
        "with BROOD_CONTRACTS=0 a declaration is advisory and the lie flows through:\n{}",
        ran.out
    );
    // The std call still fails — on `string/length`'s own type check, which is not a
    // contract and carries no blame.
    assert!(
        !ran.out.contains("blame=:") && ran.out.contains("std: RAISED blame=nil"),
        "with BROOD_CONTRACTS=0 nothing may raise a contract error:\n{}",
        ran.out
    );
}

#[test]
fn nest_test_enforces_contracts_by_default() {
    let tmp = tempdir("test");
    let root = tmp.path.join("cdef");
    let cache = tmp.path.join("cache");
    write_project(&root);
    build_image(&root, &cache);

    let ran = nest(&root, &cache, None, &["test"]);
    assert!(
        ran.out.contains("test-own: RAISED blame=:callee"),
        "under `nest test` the project's lying sig must raise:\n{}",
        ran.out
    );
    assert!(
        ran.ok,
        "the suite catches the error, so it passes:\n{}",
        ran.out
    );
}

/// The image writer refuses to run armed: an image is keyed on the stdlib's content and
/// read by every process, and one built with contracts on would carry the shims into every
/// unarmed run. `nest` builds it in a child with the variable removed; an explicit
/// `BROOD_CONTRACTS=1 nest stdimage` must say no.
#[test]
fn stdimage_refuses_to_build_with_contracts_armed() {
    let tmp = tempdir("stdimage");
    let root = tmp.path.join("cdef");
    let cache = tmp.path.join("cache");
    write_project(&root);
    let refused = nest(&root, &cache, Some("1"), &["stdimage"]);
    assert!(
        !refused.ok && refused.out.contains("refusing to write a stdlib image"),
        "an armed `nest stdimage` must refuse, naming the reason:\n{}",
        refused.out
    );
    // And the unarmed build in the same cache then succeeds — the refusal wrote nothing
    // that a later build trips over.
    build_image(&root, &cache);
}
