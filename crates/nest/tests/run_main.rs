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
