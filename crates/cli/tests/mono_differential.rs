//! **`BROOD_MONO=1` must compute exactly what the dynamic path computes.**
//!
//! Ability-dispatch monomorphization (ADR-182) is opt-in and rewrites call sites, which the
//! design doc names as *the* miscompile surface: a wrong devirtualization silently calls the
//! wrong impl — no crash, no error, just a different answer. Its validation plan asked for
//! "flag on: full suites green". That was never wired up, and **nothing in the repo ever set
//! the flag** — not CI, not the Makefile, not a test. The rewrite had no coverage at all.
//!
//! Turning it on for the first time failed immediately (ADR-294): the rewrite baked the
//! resolved impl *fn value* into the chunk, and a body is compiled before it runs, so
//! `(do (impl Display rec …) (->string (rec 7)))` captured the impl from before its own
//! `impl` line and called the wrong one. The fix proves only the *identity* and leaves
//! resolution behind the epoch-guarded dispatch cache.
//!
//! This gate is a differential: the same suite, both ways, byte-identical. It also asserts
//! the rewrite actually **fired**, because a comparison in which nothing was rewritten is
//! two identical dynamic runs and proves nothing (the ADR-280 lesson).

use std::path::{Path, PathBuf};
use std::process::Command;

mod support;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

/// Run `brood --test <file>` from the repo root, optionally with the flag (and its tracer).
fn run(file: &str, mono: bool) -> (String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg("--test").arg(file).current_dir(repo_root());
    if mono {
        cmd.env("BROOD_MONO", "1").env("BROOD_MONO_DBG", "1");
    } else {
        cmd.env_remove("BROOD_MONO").env_remove("BROOD_MONO_DBG");
    }
    support::dies_with_parent(&mut cmd);
    let out = cmd.output().expect("run brood --test");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (text, out.status.success())
}

/// The tracer's lines, and the run with them stripped — so the two arms can be compared on
/// what they *computed* rather than on what the optimizer narrated.
fn split_trace(text: &str) -> (Vec<String>, String) {
    let mut trace = Vec::new();
    let mut rest = String::new();
    for line in text.lines() {
        if line.starts_with("[mono]") {
            trace.push(line.to_string());
        } else {
            rest.push_str(line);
            rest.push('\n');
        }
    }
    (trace, rest)
}

/// Timings differ run to run and say nothing about correctness.
///
/// This must also drop the framework's **per-test slow annotation** — a test slower than
/// `*test-slow-ms*` (1 s) prints `  group › name   13.9s`, and whether a given test crosses
/// that threshold depends on machine load, not on the code under test. Under full-suite
/// parallelism one arm's nested run crossed it and the other's did not, and this
/// differential reported "monomorphization changed an ANSWER" over a timing line while both
/// arms said `92 passed` (2026-08-29). So: any line whose last token is a duration
/// (`13.9s`, `2ms`) is timing chatter. A real divergence that only manifests in such a line
/// is theoretically maskable, but failures already fail the `*_ok` asserts before this
/// comparison runs.
fn without_timings(text: &str) -> String {
    fn ends_with_duration(l: &str) -> bool {
        let Some(tok) = l.trim_end().rsplit(char::is_whitespace).next() else {
            return false;
        };
        let num = tok.strip_suffix("ms").or_else(|| tok.strip_suffix('s'));
        matches!(num, Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.'))
    }
    text.lines()
        .filter(|l| !l.contains("ms wall") && !l.contains("Slow tests") && !ends_with_duration(l))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn monomorphization_computes_what_the_dynamic_path_computes() {
    // The ability suite is the concentrated case: ~92 tests over dispatch, `:default`
    // fallback, sealed members, retraction, and cross-module override precedence.
    let (plain_text, plain_ok) = run("tests/ability_test.blsp", false);
    let (mono_text, mono_ok) = run("tests/ability_test.blsp", true);
    let (trace, mono_clean) = split_trace(&mono_text);

    assert!(plain_ok, "the dynamic run must pass:\n{plain_text}");
    assert!(
        mono_ok,
        "the monomorphized run must pass — a failure here is a miscompile, not a flake:\n\
         {mono_clean}"
    );
    // Not vacuous: the rewrite has to have happened for the comparison to mean anything.
    assert!(
        trace.len() >= 5,
        "only {} call sites were rewritten — the flag is not reaching the pass, so this \
         differential is comparing two identical dynamic runs:\n{trace:?}",
        trace.len()
    );
    assert_eq!(
        without_timings(&plain_text),
        without_timings(&mono_clean),
        "the two arms disagree — monomorphization changed an ANSWER"
    );
}

#[test]
fn an_impl_registered_after_compile_time_is_still_the_one_called() {
    // ADR-294's regression, reduced. Every line here is inside ONE `do`, so the whole body
    // is compiled before any of it runs: at compile time the only impl is `:default`, and a
    // rewrite that captured that would answer "DEFAULT" for the rest of the program. The
    // retraction afterwards is the same question from the other side.
    let dir = std::env::temp_dir().join(format!("brood-mono-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("late.blsp");
    std::fs::write(
        &file,
        "(defrecord rec (n))\n\
         (defability Show (render [self] :-> string))\n\
         (impl Show :default (render [x] \"DEFAULT\"))\n\
         (do\n\
         \x20\x20(impl Show rec (render [r] (str \"R\" (get r :n))))\n\
         \x20\x20(io/puts (str \"same-body: \" (render (rec 7)))))\n\
         (%unimpl Show rec)\n\
         (io/puts (str \"after-retract: \" (render (rec 7))))\n",
    )
    .expect("write fixture");

    for mono in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
        cmd.arg(&file);
        if mono {
            cmd.env("BROOD_MONO", "1");
        } else {
            cmd.env_remove("BROOD_MONO");
        }
        support::dies_with_parent(&mut cmd);
        let out = cmd.output().expect("run brood");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let how = if mono {
            "BROOD_MONO=1"
        } else {
            "the dynamic path"
        };
        assert!(
            text.contains("same-body: R7"),
            "under {how}, an impl registered earlier in the SAME compiled body must be the \
             one called — got:\n{text}"
        );
        assert!(
            text.contains("after-retract: DEFAULT"),
            "under {how}, a retracted impl must stop being called — got:\n{text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
