//! End-to-end test for `nest run --main MODULE[/FN]` (the manifest `:main`
//! override). Regression for the bug where `--main` was silently ignored:
//! the override was set before `run-project`, but `run-project` calls
//! `project-setup`, which re-applies the manifest's `:main` into
//! `*project-main*` — clobbering the override. The fix routes `--main` through
//! a dedicated `*project-main-override*` slot that `run-project` prefers.
//!
//! Runs the real `nest` binary in a child process (so the global env it mutates
//! is isolated per case — in-language tests share one runtime's global table).

use std::path::Path;
use std::process::Command;

/// Scaffold a project whose manifest entry is `app/main`, plus a second
/// `scratch` module, then return the project root.
fn scaffold(dir: &Path) {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(dir.join("project.blsp"), "(project\n  :name demo\n  :main app)\n").unwrap();
    std::fs::write(
        src.join("app.blsp"),
        "(defmodule app)\n(defn main () (println \"RAN: app/main\"))\n",
    )
    .unwrap();
    std::fs::write(
        src.join("scratch.blsp"),
        "(defmodule scratch)\n\
         (defn main () (println \"RAN: scratch/main\"))\n\
         (defn other () (println \"RAN: scratch/other\"))\n",
    )
    .unwrap();
}

/// Run `nest run <extra args>` in `dir` and return combined stdout.
fn nest_run(dir: &Path, extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("run")
        .args(extra)
        .current_dir(dir)
        .output()
        .expect("spawn nest");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "nest run {extra:?} failed: status={:?}\nstdout:\n{stdout}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// Run `nest check` in `dir` and return combined stderr (where the GNU
/// `FILE:LINE:COL: warning: …` lines are printed). `nest check` exits 0 even
/// with warnings (advisory), so we don't assert on status.
fn nest_check(dir: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .arg("check")
        .current_dir(dir)
        .output()
        .expect("spawn nest check");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A **user-declared `(sig …)` is authoritative for callers everywhere** — both
/// inside the declaring module and across a `(:use …)` boundary — not just in a
/// bare file (regression for the heap-backed declared-sig store + `%register-sig`).
///
/// Module `b` declares `(sig sq (int -> int))` but its body `(* x 1)` infers
/// `number`; before the store, a caller (intra- or cross-module) resolved to the
/// qualified global `b/sq` and fell back to that inferred `number`, which accepts
/// a float — so `(sq 2.5)` was silently fine. With the store, the declared `int`
/// wins and both call sites are flagged "expects int, got float".
#[test]
fn declared_sig_is_authoritative_cross_module() {
    let dir =
        std::env::temp_dir().join(format!("brood-nest-check-sig-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(dir.join("project.blsp"), "(project\n  :name sigdemo)\n").unwrap();
    // Module b: declared int, body infers number; plus an intra-module float call.
    std::fs::write(
        src.join("b.blsp"),
        "(defmodule b)\n\
         (sig sq (int -> int))\n\
         (defn sq (x) (* x 1))\n\
         (defn use-here () (sq 2.5))\n",
    )
    .unwrap();
    // Module a: uses b, calls sq with a float across the module boundary.
    std::fs::write(
        src.join("a.blsp"),
        "(defmodule a (:use b))\n\
         (defn call-sq () (sq 2.5))\n",
    )
    .unwrap();

    let warnings = nest_check(&dir);
    // Cross-module: a's call is flagged, and crucially with the DECLARED `int`
    // (not the body-inferred `number`, which would accept the float silently).
    assert!(
        warnings.contains("a.blsp") && warnings.contains("expects int, got float"),
        "cross-module declared sig did not constrain a's caller:\n{warnings}"
    );
    // Intra-module: b's own call (resolving to the qualified `b/sq`) is flagged too.
    assert!(
        warnings.contains("b.blsp") && warnings.contains("expects int, got float"),
        "intra-module declared sig did not constrain b's own caller:\n{warnings}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nest_run_main_overrides_the_manifest_entry() {
    let dir = std::env::temp_dir().join(format!("brood-nest-run-main-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    scaffold(&dir);

    // Default: the manifest's `:main app` runs `app/main`.
    let default = nest_run(&dir, &[]);
    assert!(default.contains("RAN: app/main"), "default run:\n{default}");
    assert!(!default.contains("scratch"), "default leaked scratch:\n{default}");

    // `--main scratch`: module-only spec defaults the fn to `main`.
    let m = nest_run(&dir, &["--main", "scratch"]);
    assert!(m.contains("RAN: scratch/main"), "--main scratch:\n{m}");
    assert!(!m.contains("app/main"), "--main scratch still ran app:\n{m}");

    // `--main scratch/other`: explicit module/fn spec.
    let mf = nest_run(&dir, &["--main", "scratch/other"]);
    assert!(mf.contains("RAN: scratch/other"), "--main scratch/other:\n{mf}");
    assert!(!mf.contains("app/main"), "--main scratch/other still ran app:\n{mf}");

    let _ = std::fs::remove_dir_all(&dir);
}
