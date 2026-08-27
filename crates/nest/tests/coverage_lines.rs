//! `nest test --cover-lines` end to end (ADR-148 tier 2).
//!
//! Line coverage cannot be tested from inside the suite: the recording is emitted at
//! COMPILE time, so `BROOD_COVERAGE` has to be set before the prelude is built — which
//! means a fresh process, which means an integration test.
//!
//! What is actually being pinned here is that the number is *honest*. Two earlier
//! attempts produced a plausible-looking report that was wrong, and both would have
//! passed a weaker test:
//!
//!   1. Denominator from the source text ("lines holding a form") — a fully exercised
//!      fixture reported 14%, because a `defmodule` header and a `defn`'s own line hold
//!      forms but are not instrumented nodes.
//!   2. Denominator from what had been instrumented, without forcing compilation — arms
//!      compile on first CALL, so a dead function was absent from both halves and the
//!      fixture reported 100% with a function nothing called.
//!
//! Hence the fixture: one function the tests call, one they never call. A correct
//! report is strictly between 0% and 100% and names the dead function's lines.

use std::path::Path;
use std::process::Command;

struct TempDir {
    path: std::path::PathBuf,
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir(tag: &str) -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("brood-cov-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(path.join("src")).unwrap();
    std::fs::create_dir_all(path.join("tests")).unwrap();
    TempDir { path }
}

/// A project with `live` (called by the tests) and `dead` (never called). `dead`'s body
/// is two call-bearing lines, so it contributes to the denominator and to nothing else.
fn fixture(tag: &str) -> TempDir {
    let dir = tempdir(tag);
    let root = &dir.path;
    std::fs::write(
        root.join("project.blsp"),
        "(project :name \"cov\" :version \"0.1.0\" \
         :source-paths [\"src\"] :test-paths [\"tests\"])\n",
    )
    .unwrap();
    // Line 3 is `live`'s body; lines 6 and 7 are `dead`'s.
    std::fs::write(
        root.join("src/cov.blsp"),
        "(defmodule cov \"Fixture.\")\n\n\
         (defn live (x) (+ x 1))\n\n\
         (defn dead (x)\n  (let (doubled (* x 2))\n    (+ doubled 1)))\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/cov_test.blsp"),
        "(defmodule cov-test (:use test) (:use cov))\n\n\
         (describe \"cov\"\n  (test \"live\" (assert= (live 1) 2)))\n",
    )
    .unwrap();
    dir
}

struct Out {
    text: String,
    ok: bool,
}

fn nest(dir: &Path, args: &[&str]) -> Out {
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run nest");
    Out {
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        ok: out.status.success(),
    }
}

/// `coverage: N% of T executable lines` → `(N, T)`.
fn parse_total(text: &str) -> Option<(u32, u32)> {
    // The SUMMARY line, not the section header — which also says "executable lines".
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("coverage: ") && l.contains("executable lines"))?;
    let percent = line
        .split_whitespace()
        .find_map(|word| word.strip_suffix('%'))?
        .parse()
        .ok()?;
    let total = line
        .split(" of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some((percent, total))
}

#[test]
fn a_dead_function_is_reported_as_dead() {
    let dir = fixture("dead");
    let run = nest(&dir.path, &["test", "--cover-lines"]);
    assert!(run.ok, "the run itself should pass:\n{}", run.text);

    let (percent, total) =
        parse_total(&run.text).unwrap_or_else(|| panic!("no line-coverage summary:\n{}", run.text));

    // The whole point: neither extreme. 100% would mean the dead function was never
    // counted; 0% would mean the live one wasn't recorded.
    assert!(
        percent > 0 && percent < 100,
        "a fixture with one live and one dead function must report between 0 and 100, \
         got {percent}%:\n{}",
        run.text
    );
    // 3 instrumented lines: `live`'s body, and `dead`'s two.
    assert_eq!(
        total, 3,
        "expected 3 instrumented lines (live's 1 + dead's 2):\n{}",
        run.text
    );
    assert!(
        run.text.contains("Never ran in src/cov.blsp"),
        "the report should name the file with unexecuted lines:\n{}",
        run.text
    );
    // `dead` occupies source lines 6-7.
    let never_ran = run
        .text
        .lines()
        .skip_while(|l| !l.contains("Never ran in"))
        .nth(1)
        .unwrap_or("");
    assert!(
        never_ran.contains('6') && never_ran.contains('7'),
        "the dead function's lines (6, 7) should be listed, got `{never_ran}`:\n{}",
        run.text
    );
}

/// Coverage that moves when the tests move is the only kind worth reporting.
#[test]
fn exercising_the_dead_function_raises_the_number() {
    let dir = fixture("moves");
    let before = parse_total(&nest(&dir.path, &["test", "--cover-lines"]).text).unwrap();

    std::fs::write(
        dir.path.join("tests/cov_test.blsp"),
        "(defmodule cov-test (:use test) (:use cov))\n\n\
         (describe \"cov\"\n  (test \"live\" (assert= (live 1) 2))\n  \
         (test \"dead\" (assert= (dead 2) 5)))\n",
    )
    .unwrap();
    let after_run = nest(&dir.path, &["test", "--cover-lines"]);
    let after = parse_total(&after_run.text).unwrap();

    assert_eq!(
        after.0, 100,
        "with every function called it should be 100%:\n{}",
        after_run.text
    );
    assert!(
        after.0 > before.0,
        "covering more lines must raise the percentage ({}% → {}%)",
        before.0,
        after.0
    );
    assert!(
        !after_run.text.contains("Never ran in"),
        "nothing should be listed as never run:\n{}",
        after_run.text
    );
}

#[test]
fn cover_min_gates_on_the_line_percentage_and_reports_it_cleanly() {
    let dir = fixture("gate");
    let run = nest(&dir.path, &["test", "--cover-lines", "--cover-min", "90"]);
    assert!(!run.ok, "falling short of the floor must exit non-zero");
    assert!(
        run.text.contains("FAILED: coverage") && run.text.contains("minimum 90%"),
        "the shortfall should be stated plainly:\n{}",
        run.text
    );
    // A verdict, not a crash: no error banner, no trace, no version line.
    assert!(
        !run.text.contains("error:") && !run.text.contains("    at "),
        "a coverage shortfall must not be reported as an error with a trace:\n{}",
        run.text
    );

    let passing = nest(&dir.path, &["test", "--cover-lines", "--cover-min", "10"]);
    assert!(
        passing.ok,
        "a floor the run clears must pass:\n{}",
        passing.text
    );
}

/// The instrumentation must be genuinely opt-in — an ordinary run pays nothing and
/// prints nothing, since `RecordLine` is only emitted when the flag was set at startup.
#[test]
fn an_ordinary_run_reports_no_coverage() {
    let dir = fixture("plain");
    let run = nest(&dir.path, &["test"]);
    assert!(run.ok, "the run should pass:\n{}", run.text);
    assert!(
        !run.text.contains("coverage"),
        "a run without --cover-lines should say nothing about coverage:\n{}",
        run.text
    );
}

/// Both tiers at once measure different things; the line percentage is the stricter
/// one, so it is what a floor gates on.
#[test]
fn the_function_and_line_tiers_coexist() {
    let dir = fixture("both");
    let run = nest(&dir.path, &["test", "--cover", "--cover-lines"]);
    assert!(run.ok, "the run should pass:\n{}", run.text);
    assert!(
        run.text.contains("functions") && run.text.contains("executable lines"),
        "both reports should appear:\n{}",
        run.text
    );
    assert!(
        run.text.contains("dead"),
        "the function tier should name the uncovered function:\n{}",
        run.text
    );
}

/// A project whose functions are all literal-bodied has no instrumented lines at all.
/// It must report that, not 0% — a 0% would fail a `--cover-min` gate for having
/// nothing measurable.
#[test]
fn a_project_with_nothing_instrumented_says_so() {
    let dir = tempdir("empty");
    std::fs::write(
        dir.path.join("project.blsp"),
        "(project :name \"empty\" :version \"0.1.0\" \
         :source-paths [\"src\"] :test-paths [\"tests\"])\n",
    )
    .unwrap();
    std::fs::write(
        dir.path.join("src/empty.blsp"),
        "(defmodule empty \"No calls anywhere.\")\n\n(defn constant () 42)\n",
    )
    .unwrap();
    std::fs::write(
        dir.path.join("tests/empty_test.blsp"),
        "(defmodule empty-test (:use test) (:use empty))\n\n\
         (describe \"empty\"\n  (test \"constant\" (assert= (constant) 42)))\n",
    )
    .unwrap();

    let run = nest(&dir.path, &["test", "--cover-lines", "--cover-min", "80"]);
    assert!(
        run.ok,
        "nothing measurable must not fail a floor:\n{}",
        run.text
    );
    assert!(
        run.text.contains("no instrumented lines"),
        "it should say there was nothing to measure:\n{}",
        run.text
    );
}
