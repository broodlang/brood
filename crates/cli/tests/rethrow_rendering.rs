//! A rethrown built-in error renders as the original error, not as a printed map.
//!
//! `(catch e (throw e))` hands `throw` the error MAP the catch bound, and until ADR-306
//! that printed `error: {:kind :arity, :file …, :message …}` — the diagnostic buried in
//! a map dump, with no `at` lines. `finally` rethrows exactly this way, so every error
//! escaping a `finally` would have rendered that way too. `LispError::from_error_map`
//! rebuilds the error from the map; this test pins the rendering.

use std::io::Write;
use std::process::Command;

fn script(name: &str, source: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("brood-rethrow-{name}.blsp"));
    let mut file = std::fs::File::create(&path).expect("create script");
    file.write_all(source.as_bytes()).expect("write script");
    path
}

fn run(path: &std::path::Path) -> (String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .arg(path)
        .output()
        .expect("run brood");
    (
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

#[test]
fn a_rethrown_arity_error_renders_as_an_arity_error() {
    let path = script(
        "catch",
        "(defn f (x) x)\n(try (f 1 2) (catch e (throw e)))\n",
    );
    let (stderr, ok) = run(&path);
    assert!(!ok);
    assert!(
        stderr.contains("arity error: f: expected 1 argument, got 2"),
        "stderr: {stderr}"
    );
    assert!(!stderr.contains("{:kind"), "map dump leaked: {stderr}");
}

#[test]
fn an_error_escaping_a_finally_renders_as_itself_with_its_trace() {
    let path = script(
        "finally",
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
