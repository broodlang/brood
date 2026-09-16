//! **Every fuzz generator must emit programs that RUN on today's language.**
//!
//! `scripts/fuzz/run.sh` is a differential: a program's output under the tree-walker, the
//! VM, the JIT and GC-stress must agree. That is evidence only when the program executes.
//! After the ADR-302/ADR-330 rename waves every generator still emitted `println`, `rem`,
//! `concat`, `rope-insert`, … — each program died on its first form, all four engines
//! agreed on an empty stdout, and the runner reported **"0 divergences" on 1850 programs**
//! (found 2026-09-15, in a session that had just cited that number as a clean result).
//! Brood's rename sweeps cover `std/`, `tests/`, `examples/` and `breakage/`; a generator
//! writes Brood from Python and no static gate can see it — the same blind spot
//! `stress/fuzz_programs.py` and `bench/smoke.py` each had to close on their own.
//!
//! So: one program per generator, run once on the reference engine, must report no
//! `unbound symbol` and — for the generators whose programs are meant to run — print
//! something. Two generators' programs are MEANT to be rejected: `checker` (a malformed
//! `sig`; its preamble is real code, so the unbound-symbol rule still applies) and
//! `syntax` (random tokens — an unbound name there is the point, so it is held only to
//! `run.sh`'s no-crash rule and skipped here). Skips (does not fail) when `python3` is
//! absent, the way the stress tooling does.
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

#[test]
fn every_fuzz_generator_emits_programs_that_run() {
    let root = repo_root();
    let gens = root.join("scripts/fuzz/generators");
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not found — skipping the generator liveness check");
        return;
    }
    let work = std::env::temp_dir().join(format!("brood-fuzz-live-{}", std::process::id()));
    std::fs::create_dir_all(&work).unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&gens)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".py"))
        .map(|n| n.trim_end_matches(".py").to_string())
        .collect();
    names.sort();
    assert!(
        names.len() >= 8,
        "expected the generator directory to be populated, saw {names:?}"
    );
    let mut problems = Vec::new();
    for g in &names {
        let out = work.join(g);
        std::fs::create_dir_all(&out).unwrap();
        // Seed 20260915: the day this gate was written. Two programs, so a generator whose
        // stale name sits on a random branch has two draws to show it.
        let gen = Command::new("python3")
            .arg(gens.join(format!("{g}.py")))
            .args(["2", "20260915"])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            gen.status.success(),
            "{g}.py failed: {}",
            String::from_utf8_lossy(&gen.stderr)
        );
        let mut files: Vec<PathBuf> = std::fs::read_dir(&out)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "blsp"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "{g}.py wrote no .blsp");
        if g == "syntax" {
            continue;
        }
        let must_print = g != "checker";
        for f in files {
            let run = Command::new(env!("CARGO_BIN_EXE_brood"))
                .env("BROOD_VM", "0")
                .arg(&f)
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&run.stderr);
            let stdout = String::from_utf8_lossy(&run.stdout);
            if let Some(line) = stderr.lines().find(|l| l.contains("unbound symbol")) {
                problems.push(format!("{g}: {} — {}", f.display(), line.trim()));
            } else if must_print && stdout.trim().is_empty() {
                let first_err = stderr
                    .lines()
                    .find(|l| l.contains("error"))
                    .unwrap_or("")
                    .trim();
                problems.push(format!(
                    "{g}: {} printed nothing — {first_err}",
                    f.display()
                ));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    assert!(
        problems.is_empty(),
        "STALE GENERATOR(S) — the differential fuzz runs are vacuous until these run:\n  {}",
        problems.join("\n  ")
    );
}
