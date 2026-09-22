//! **The pool must not change a verdict.** `nest check` fans its per-file walk across the
//! worker pool once a project is big enough (ADR-386); each worker has its own heap, and a
//! checker verdict can depend on what that heap has LOADED (KI-137 — `std/math.blsp` was
//! clean in a fresh process and red in strict once `datetime` was). `project-preload!`
//! brings the heap to one state before any file is checked, and the pool is only sound
//! because of it, so the property worth gating is the end-to-end one: the same files
//! checked both ways report the same warnings, in content if not in order.
//!
//! `BROOD_CHECK_SEQUENTIAL=1` is the control arm — it pins the check in this process — so
//! the two runs differ in nothing but the pool. The fixture is deliberately WARNING-HEAVY:
//! a differential over a clean project passes vacuously, which is exactly how a parallel
//! path that silently reported nothing would look.

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
    let path = std::env::temp_dir().join(format!("brood-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create temp dir");
    TempDir { path }
}

/// A project whose every module carries the same three warnings — an unbound name, a call
/// that cannot type, and a `match` clause no value reaches — padded past
/// `*check-min-parallel-bytes*` (7 MB) so the byte test engages the pool. Each module is
/// its own file so the fan-out has something to fan.
fn write_project(root: &Path, modules: usize, pad_lines: usize) {
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"par\" :version \"0.1.0\")\n",
    )
    .unwrap();
    for i in 0..modules {
        let mut body = format!("(defmodule m{i})\n\n");
        // The padding is ordinary checkable code, so the walk has real work per file.
        for k in 0..pad_lines {
            body.push_str(&format!(
                "(defn m{i}-f{k} (x)\n  (let (y (+ x {k}))\n    (cond (< y 0) 0 else (+ y 1))))\n"
            ));
        }
        // A qualified reference into a DIFFERENT std module per file, reached only from
        // the body — the shape whose lazy load mid-check made a verdict depend on the
        // order files were checked in (KI-137). Each worker therefore starts from a
        // different load set unless `project-preload!` has levelled the heap first.
        let std_mods = [
            "math/quot",
            "json/encode",
            "url/encode",
            "csv/csv-parse",
            "uuid/uuid-v4",
            "encoding/base64-encode",
            "stats/mean",
            "set/union",
        ];
        body.push_str(&format!(
            "(defn m{i}-reaches (a b) ({} a b))\n",
            std_mods[i % std_mods.len()]
        ));
        // Three warnings per file, each a different part of the checker.
        body.push_str(&format!(
            "(defn m{i}-unbound (x) (no-such-function-{i} x))\n\
             (defn m{i}-mistyped (s) (string/pad-left 5 s))\n\
             (defn m{i}-redundant (v)\n  (match v (1 :one) (1 :again) (_ :other)))\n"
        ));
        std::fs::write(src.join(format!("m{i}.blsp")), body).unwrap();
    }
}

/// `nest check`'s stderr warning lines, sorted (the pool does not preserve file order — by
/// design, these are advisory GNU `FILE:LINE:COL` diagnostics) and with the project root
/// stripped, so two runs in different temp dirs compare.
fn warnings(root: &Path, sequential: bool) -> Vec<String> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nest"));
    cmd.arg("check")
        .current_dir(root)
        .env("BROOD_NO_CHECK_CACHE", "1");
    if sequential {
        cmd.env("BROOD_CHECK_SEQUENTIAL", "1");
    } else {
        cmd.env_remove("BROOD_CHECK_SEQUENTIAL");
    }
    let out = cmd.output().expect("run nest check");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let root = root.to_string_lossy().to_string();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| l.contains("warning:"))
        .map(|l| l.replace(&root, "<root>").trim().to_string())
        .collect();
    lines.sort();
    lines
}

#[test]
fn the_pool_and_this_process_report_the_same_warnings() {
    // 250 modules × 400 padded functions ≈ 7.6 MB — just past the byte threshold, which is
    // the point of the test: this project has far fewer than `*check-min-parallel*` files
    // and must still take the pool.
    let tmp = tempdir("checkpar");
    let root = tmp.path.join("par");
    write_project(&root, 250, 400);
    let total: u64 = std::fs::read_dir(root.join("src"))
        .unwrap()
        .filter_map(|e| e.ok()?.metadata().ok())
        .map(|m| m.len())
        .sum();
    assert!(
        total >= 7_000_000,
        "the fixture must clear *check-min-parallel-bytes* or the pool never engages \
         (got {total} bytes)"
    );

    let parallel = warnings(&root, false);
    let sequential = warnings(&root, true);

    // Not vacuous: at least the three planted warnings per module (the std references add
    // a few of their own, which is fine — what matters is that the corpus is large and
    // non-empty, so a path that silently reported nothing cannot pass).
    assert!(
        sequential.len() >= 750,
        "the fixture must warn at least three times per module; got {}:\n{}",
        sequential.len(),
        sequential.join("\n")
    );
    assert_eq!(
        parallel, sequential,
        "the pool changed the verdict — a worker's heap saw something this process did not"
    );
}
