//! **A test that mutates the process environment must be alone in its test binary.**
//!
//! This is KI-86's class, made structural. `std::env::set_var` is process-global, and under
//! plain `cargo test` every test in a binary shares ONE process on parallel threads — so a
//! flag one test sets reaches its siblings mid-flight. nextest hides the hazard completely
//! (each test gets its own process), which is exactly why it keeps coming back on the
//! harnesses that still use libtest: `make asan`, `make tsan`, and anyone's bare
//! `cargo test`.
//!
//! It has now happened twice:
//!   * KI-86 (2026-08-29): two `runtime_collector` tests `set_var`'d `BROOD_RT_GC_FLOOR`,
//!     and the leaked floor armed the scheduler WORKER heaps of a neighbouring test's
//!     `Interp` — three tests red under `cargo test`, green under nextest.
//!   * 2026-09-02: `crash_report_optout` set `BROOD_NO_CRASH_REPORT`, which reached
//!     `crash_report_lazy`'s arming test under `make asan` and made it report that the
//!     reporter "did not arm". Same shape, different variable, found only because ASAN
//!     runs libtest.
//!
//! Both fixes were the same: give the env-mutating test its own binary, so the isolation is
//! real rather than harness-dependent. This test makes that the rule. It walks every
//! integration-test file in the workspace, strips comments and strings, and requires that a
//! file touching `set_var`/`remove_var` contains at most ONE `#[test]`.
//!
//! Unit tests inside `src/` are deliberately also covered: they all share the single lib
//! test binary, so an env mutation there can never be isolated — the count for `src/` is
//! therefore required to be ZERO (use a constructor parameter or a per-heap setter instead,
//! which is exactly what KI-86's fix introduced).

use std::path::{Path, PathBuf};

/// The line, minus `//` comments and the contents of string literals — good enough to keep
/// a *mention* of set_var in a doc comment or an error message from counting as a call.
fn code_of(line: &str) -> String {
    let line = match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    };
    let mut out = String::with_capacity(line.len());
    let mut in_str = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_str = !in_str;
                out.push(c);
            }
            _ if in_str => {}
            _ => out.push(c),
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn an_env_mutating_test_lives_alone_in_its_binary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();

    let mut offenders = Vec::new();
    for crate_dir in ["crates/lisp", "crates/cli", "crates/nest", "crates/lsp"] {
        // Integration tests: one FILE is one binary, so "alone" means one #[test] in it.
        let mut files = Vec::new();
        rust_files(&root.join(crate_dir).join("tests"), &mut files);
        for f in &files {
            let src = std::fs::read_to_string(f).unwrap_or_default();
            let mutates = src
                .lines()
                .map(code_of)
                .any(|l| l.contains("set_var") || l.contains("remove_var"));
            if !mutates {
                continue;
            }
            // Count attributes, not mentions: a doc comment quoting `#[test]` is not a test.
            let tests = src
                .lines()
                .map(code_of)
                .filter(|l| l.trim_start().starts_with("#[test]"))
                .count();
            if tests > 1 {
                offenders.push(format!(
                    "{} mutates the environment and holds {tests} tests — under plain \
                     `cargo test` they share one process, so the mutation reaches its \
                     siblings (KI-86). Move the env-mutating test into its own file.",
                    f.strip_prefix(&root).unwrap_or(f).display()
                ));
            }
        }

        // Unit tests: every module shares the ONE lib test binary — no isolation exists.
        let mut srcs = Vec::new();
        rust_files(&root.join(crate_dir).join("src"), &mut srcs);
        for f in &srcs {
            let src = std::fs::read_to_string(f).unwrap_or_default();
            let mut in_tests = false;
            for line in src.lines() {
                if line.contains("#[cfg(test)]") {
                    in_tests = true;
                }
                if in_tests {
                    let code = code_of(line);
                    if code.contains("set_var") || code.contains("remove_var") {
                        offenders.push(format!(
                            "{} calls set_var/remove_var inside #[cfg(test)] — the lib \
                             test binary cannot be isolated; use a per-heap setter or a \
                             constructor parameter instead (KI-86's own fix)",
                            f.strip_prefix(&root).unwrap_or(f).display()
                        ));
                        break;
                    }
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "env mutation without process isolation:\n  {}",
        offenders.join("\n  ")
    );
}
