//! A rethrown built-in error renders as the original error, not as a printed map.
//!
//! `(catch e (throw e))` hands `throw` the error MAP the catch bound, and until ADR-306
//! that printed `error: {:kind :arity, :file …, :message …}` — the diagnostic buried in
//! a map dump, with no `at` lines. `finally` rethrows exactly this way, so every error
//! escaping a `finally` would have rendered that way too. `LispError::from_error_map`
//! rebuilds the error from the map; this test pins the rendering.

use std::process::Command;

mod support;
use support::script;

fn run(path: &std::path::Path) -> (String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .arg(path)
        .output()
        .expect("run brood");
    (
        String::from_utf8_lossy_owned(output.stderr),
        output.status.success(),
    )
}

#[test]
fn a_rethrown_arity_error_renders_as_an_arity_error() {
    let path = script(
        "rethrow-catch",
        "(defn f (x) x)\n(try (f 1 2) (catch e (throw e)))\n",
    );
    let (stderr, ok) = run(&path);
    assert!(!ok);
    assert!(
        stderr.contains("arity error: f: expected 1 argument (x), got 2"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("{:kind"), "map dump leaked: {stderr}");
}

#[test]
fn an_error_escaping_a_finally_renders_as_itself_with_its_trace() {
    let path = script(
        "rethrow-finally",
        "(defn boom () (* 2 (+ 1 \"x\")))\n(try (boom) (finally nil))\n",
    );
    let (stderr, ok) = run(&path);
    assert!(!ok);
    assert!(
        stderr.contains("type error: +: expected number"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("at boom"), "trace lost: {stderr}");
    assert!(!stderr.contains("{:kind"), "map dump leaked: {stderr}");
}

/// KI-291: a user `error` carries no position in what `catch` binds (the bare string), so
/// a rethrow used to be reported at the rethrow site — here the `try` on line 3 — instead
/// of the `error` on line 2.
#[test]
fn a_user_error_escaping_a_finally_is_reported_where_it_was_raised() {
    let path = script(
        "rethrow-user-finally",
        "(defn boom ()\n  (+ 1 (error \"user boom\")))\n(try\n  (boom)\n  (finally nil))\n",
    );
    let (stderr, ok) = run(&path);
    assert!(!ok);
    let headline = stderr
        .lines()
        .find(|line| line.contains("error: user boom"))
        .unwrap_or_else(|| panic!("no headline: {stderr}"));
    assert!(headline.contains(":2:"), "not at the `error`: {stderr}");
    assert!(stderr.contains("at boom"), "trace lost: {stderr}");
}

#[test]
fn a_thrown_map_of_a_library_kind_renders_its_message() {
    // A kind no built-in error has — a library's own — still names its sentence, and
    // the map is unchanged for a `catch`: only the headline differs.
    let path = script(
        "rethrow-library-kind",
        "(when (not= (try (throw {:kind :my-lib :message \"m\" :n 1}) (catch e (get e :n))) 1)\n\
           (throw \"catch lost the map\"))\n\
         (throw {:kind :my-lib :message \"the widget is closed\" :widget 7})\n",
    );
    let (stderr, ok) = run(&path);
    assert!(!ok);
    assert!(
        stderr.contains("error: my-lib: the widget is closed"),
        "stderr: {stderr}"
    );
    // The echoed source line quotes the map literal; the headline must not dump it.
    let headline = stderr.lines().find(|line| line.contains("error:")).unwrap();
    assert!(!headline.contains("{:"), "map dump leaked: {stderr}");
}

#[test]
fn a_contract_violation_renders_its_message() {
    let path = script(
        "rethrow-contract",
        "(sig f (int -> int))\n(defn f (x) x)\n(f \"two\")\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_CONTRACTS", "1")
        .arg(&path)
        .output()
        .expect("run brood");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("error: contract: "), "stderr: {stderr}");
    assert!(!stderr.contains("{:kind"), "map dump leaked: {stderr}");
}
