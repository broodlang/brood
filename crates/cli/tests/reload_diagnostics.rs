//! **A redefinition that strands old code says so** (ADR-042 §5, and the arity half beside
//! it in `eval.rs`'s `def`).
//!
//! Redefining a macro does not re-expand the callers already compiled with it: they keep the
//! old expansion until they are re-evaluated. Rebinding a function to a different arity
//! leaves every caller written for the old shape one call away from an arity error. Both are
//! silent surprises in a live session, so `def` prints a `[reload]` line for each — and must
//! stay silent on a first definition, a same-arity rebinding, and a value `def` in a loop,
//! or the line becomes noise that nobody reads.
//!
//! The diagnostics fire from `def` itself, so each case runs on every tier: a top-level form
//! that some engine lowered without passing through that `def` would lose them silently.

use std::process::Command;

const TIERS: [&str; 3] = ["0", "1", "2"];

const MACRO_LINE: &str =
    "[reload] macro rd-twice redefined; callers expanded before now keep the old expansion";

struct Run {
    stdout: String,
    stderr: String,
}

fn run(files: &[(&str, &str)], tier: &str, env: &[(&str, &str)]) -> Run {
    let directory = std::env::temp_dir().join(format!(
        "brood-reload-diag-{}-{tier}-{}",
        std::process::id(),
        files[0].1.len() + env.len()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    for (name, source) in files {
        std::fs::write(directory.join(name), source).expect("write a source file");
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_brood"));
    command
        .arg(files[0].0)
        .current_dir(&directory)
        .env("BROOD_TIER", tier)
        .env_remove("BROOD_NO_RELOAD_DIAG");
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("run brood");
    let _ = std::fs::remove_dir_all(&directory);
    let run = Run {
        stdout: String::from_utf8_lossy_owned(output.stdout),
        stderr: String::from_utf8_lossy_owned(output.stderr),
    };
    assert!(
        output.status.success(),
        "brood failed on tier {tier}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        run.stdout,
        run.stderr
    );
    run
}

fn reload_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.starts_with("[reload]"))
        .collect()
}

const IN_ONE_FILE: &str = r#"
(defmacro rd-twice (x) `(* 2 ~x))
(defn rd-early () (rd-twice 5))
(defmacro rd-twice (x) `(* 3 ~x))
(defn rd-late () (rd-twice 5))
(io/puts (str "early=" (rd-early) " late=" (rd-late)))
(defn rd-f (a) a)
(defn rd-f (a b) b)
(defn rd-same (a) a)
(defn rd-same (a) (+ a 1))
(defn rd-loop (n) (if (= n 0) :done (do (def rd-v n) (rd-loop (- n 1)))))
(io/puts (rd-loop 3))
"#;

#[test]
fn a_macro_or_arity_redefinition_warns_and_nothing_else_does() {
    for tier in TIERS {
        let run = run(&[("main.blsp", IN_ONE_FILE)], tier, &[]);
        // The documented semantics the warning describes: the caller compiled before the
        // redefinition keeps the old expansion; one compiled after sees the new one.
        assert!(
            run.stdout.contains("early=10 late=15"),
            "tier {tier}: expected the early caller to keep the old expansion\n{}",
            run.stdout
        );
        let lines = reload_lines(&run.stderr);
        assert_eq!(
            lines.len(),
            2,
            "tier {tier}: expected exactly the macro and the arity line — a first definition, \
             a same-arity rebinding and a value `def` in a loop must stay silent\n{}",
            run.stderr
        );
        assert!(
            lines[0].starts_with(MACRO_LINE),
            "tier {tier}: {}",
            lines[0]
        );
        assert_eq!(
            lines[1], "[reload] arity changed for rd-f: 1 -> 2",
            "tier {tier}"
        );
    }
}

const RELOADED_MAIN: &str = r#"
(defmacro rd-twice (x) `(* 2 ~x))
(defn rd-early () (rd-twice 5))
(defn rd-f (a) a)
(io/puts (str "before=" (rd-early)))
(system/reload-defs "edited.blsp")
(io/puts (str "after=" (rd-early) " fresh=" (rd-fresh) " f=" (rd-f 1 2)))
"#;

const RELOADED_EDIT: &str = r#"
(defmacro rd-twice (x) `(* 3 ~x))
(defn rd-fresh () (rd-twice 5))
(defn rd-f (a b) b)
(io/puts "a top-level call reload-defs must skip")
"#;

#[test]
fn reload_defs_reports_the_macro_it_strands() {
    for tier in TIERS {
        let run = run(
            &[("main.blsp", RELOADED_MAIN), ("edited.blsp", RELOADED_EDIT)],
            tier,
            &[],
        );
        assert!(
            run.stdout.contains("before=10") && run.stdout.contains("after=10 fresh=15 f=2"),
            "tier {tier}: the reload must land, and the stranded caller keep its expansion\n{}",
            run.stdout
        );
        assert!(
            !run.stdout.contains("must skip"),
            "tier {tier}: reload-defs ran a top-level call"
        );
        let lines = reload_lines(&run.stderr);
        assert_eq!(lines.len(), 2, "tier {tier}:\n{}", run.stderr);
        assert!(
            lines[0].starts_with(MACRO_LINE),
            "tier {tier}: {}",
            lines[0]
        );
        assert_eq!(
            lines[1], "[reload] arity changed for rd-f: 1 -> 2",
            "tier {tier}"
        );
    }
}

#[test]
fn the_opt_out_silences_both() {
    let run = run(
        &[("main.blsp", IN_ONE_FILE)],
        "1",
        &[("BROOD_NO_RELOAD_DIAG", "1")],
    );
    assert!(run.stdout.contains("early=10 late=15"), "{}", run.stdout);
    assert!(reload_lines(&run.stderr).is_empty(), "{}", run.stderr);
}
