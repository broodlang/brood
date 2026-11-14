//! **The tree-walker's pending work lives on its heap frame stack** (ADR-416 stage 2).
//!
//! A non-tail call on the tree-walker used to be a nested Rust `eval`, so runaway recursion was
//! caught by the native-stack byte budget. Now every pending argument, test, binding and body
//! form is a continuation on the machine's own stack, and the limit is a frame count. Both
//! halves are observable from a program: a recursion deeper than the old byte budget allowed
//! completes, and a runaway one fails with the frame-limit message, catchably.
//!
//! Top-level forms run on the tree-walker under `BROOD_TIER=0` (a spawned process body still
//! runs on the VM until ADR-416 stage 5), so the recursion is driven from the top level.

use std::process::Command;

const PROGRAM: &str = r#"
(defn deep (n) (if (= n 0) 0 (+ 1 (deep (- n 1)))))
(io/puts (str "deep = " (deep 200000)))
(io/puts (str "runaway = " (try (deep 100000000) (catch e (get e :message)))))
"#;

/// A recursion that runs through an `&optional` default: each level's call is pending inside
/// the default of the level above, which entering a closure evaluates as a frame (stage 2b),
/// not as a nested native `eval`.
const DEFAULTS: &str = r#"
(defn depth (n &optional (acc (if (= n 0) 0 (+ 1 (depth (- n 1)))))) acc)
(io/puts (str "depth = " (depth 100000)))
"#;

/// Run `source` at tier 0; its stdout and stderr.
fn run_tier0(name: &str, source: &str) -> (String, String) {
    run_tier0_with(name, source, &[])
}

fn run_tier0_with(name: &str, source: &str, envs: &[(&str, &str)]) -> (String, String) {
    let dir = std::env::temp_dir().join(format!("brood-tw-frames-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let program = dir.join("program.blsp");
    std::fs::write(&program, source).expect("write program");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_TIER", "0")
        .envs(envs.iter().copied())
        .arg(&program)
        .output()
        .expect("run brood");
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy_owned(out.stdout),
        String::from_utf8_lossy_owned(out.stderr),
    )
}

#[test]
fn tier0_recursion_is_bounded_by_frames_not_native_stack() {
    let (stdout, stderr) = run_tier0("deep", PROGRAM);
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some("deep = 200000"),
        "a 200 000-deep non-tail recursion did not complete on the tree-walker.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let runaway = lines.next().unwrap_or_default();
    assert!(
        runaway.starts_with("runaway = recursion too deep: exceeded the tree-walker's"),
        "runaway recursion was not stopped by the tree-walker's frame limit: {runaway:?}\n\
         stderr:\n{stderr}"
    );
}

#[test]
fn tier0_optional_defaults_run_as_frames() {
    let (stdout, stderr) = run_tier0("defaults", DEFAULTS);
    assert_eq!(
        stdout.lines().next(),
        Some("depth = 100000"),
        "a 100 000-deep recursion through an `&optional` default did not complete on the \
         tree-walker.\nstderr:\n{}",
        stderr.lines().take(8).collect::<Vec<_>>().join("\n")
    );
}

/// A native calling back into Brood (`%try`, under `try`) enters the thunk across a boundary.
/// An error raised without a position by the thunk's tail form (`(let)` has no bindings to
/// complain at) takes the position of the body's last form, the `do` on line 2, as the
/// boundary's own loop gave it and as the VM does.
const TAIL_POSITION: &str = r#"(io/puts (try
  (do
    (io/puts "a")
    (let))
  (catch e (str "line = " (get e :line)))))
"#;

#[test]
fn tier0_boundary_tail_error_takes_the_tail_forms_position() {
    let (stdout, stderr) = run_tier0("tail-pos", TAIL_POSITION);
    assert_eq!(
        stdout.lines().nth(1),
        Some("line = 2"),
        "an untagged error leaving a native-boundary closure's tail lost the tail form's \
         position.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// A recursion whose every level is inside a `try` and a `binding`: under a tier-0 ceiling
/// `%try` and `%binding` are frames (ADR-416 stage 3), not natives calling their thunk back,
/// so the depth is the frame stack's. The handler and the dynamic stack's restore are checked
/// on the way back out. (20 000, not more: a global read on the tree-walker scans every active
/// binding first, so this program is quadratic in its depth — devlog 2026-10-19 "night".)
const TRY_BINDING: &str = r#"
(defdyn *level* 0)
(defn deep (n)
  (if (= n 0)
    (throw *level*)
    (+ 1 (try (binding (*level* n) (deep (- n 1))) (catch e (+ e *level*))))))
(io/puts (str "deep = " (deep 20000) " level = " *level*))
"#;

#[test]
fn tier0_try_and_binding_run_as_frames() {
    let (stdout, stderr) = run_tier0("try-binding", TRY_BINDING);
    // Every level adds 1. The innermost throws `*level*` = 1, and the handler that catches
    // it, outside that level's binding, adds the enclosing `*level*` = 2: 20 000 + 1 + 2.
    // Back at the top the dynamic stack is restored.
    assert_eq!(
        stdout.lines().next(),
        Some("deep = 20003 level = 0"),
        "a 20 000-deep recursion through `try` and `binding` did not complete on the \
         tree-walker.\nstderr:\n{}",
        stderr.lines().take(8).collect::<Vec<_>>().join("\n")
    );
}

/// Three processes wait in `receive`, each for the one before it. A tier-0 `receive` that
/// finds nothing parks (ADR-416 stage 4): the machine's frames go back to the scheduler and
/// are resumed when a message comes. The answer alone cannot tell that from a `receive` that
/// blocks its worker — the runtime makes up for a blocked worker, so the chain completes
/// either way — so the scheduler's own log (`BROOD_SCHED_DBG`) is what is asserted: a park
/// of a tree-walker continuation at a `receive`, and its resume.
const PARKING: &str = r#"
(def me (self))
(defn relay (next) (receive (n (send next (+ n 1))) (after 3000 (send next :blocked))))
(def c (spawn (fn () (receive (n (send me [:end n])) (after 3000 (send me [:end :blocked]))))))
(def b (spawn (fn () (relay c))))
(def a (spawn (fn () (relay b))))
(proc/sleep 50)
(send a 1)
(io/puts (str "chain = " (receive ([:end n] n) (after 10000 :lost))))
"#;

#[test]
fn tier0_receive_parks_and_resumes() {
    let (stdout, stderr) = run_tier0_with(
        "parking",
        PARKING,
        &[("BROOD_J", "1"), ("BROOD_SCHED_DBG", "1")],
    );
    assert_eq!(
        stdout.trim(),
        "chain = 3",
        "the chain did not complete at tier 0.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let parks = stderr
        .lines()
        .filter(|l| l.starts_with("[sched] park pid=") && l.contains(" tw receive "))
        .count();
    assert!(
        parks >= 3,
        "expected each relay to park its tree-walker frames at `receive`, saw {parks} \
         such parks.\nstderr:\n{stderr}"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("[sched] resume pid=") && l.contains(" tw receive ")),
        "no parked tree-walker continuation was resumed.\nstderr:\n{stderr}"
    );
}

/// The same 100 000-deep recursion with and without a `binding` per level. A global read
/// used to scan the whole dynamic stack (`Heap::env_get`), so N nested `binding`s cost N
/// per read and this took quadratic time — 5.5 s at 50 000 levels in a release build,
/// against 0.2 s now. Each run is timed against its own control rather than a fixed
/// budget, so a slow machine slows both: linear lookup keeps the ratio small, the scan
/// made it about a hundredfold.
const BINDING_DEEP: &str = r#"
(defdyn *level* 0)
(defn deep (n) (if (= n 0) *level* (binding (*level* (+ *level* 1)) (+ 0 (deep (- n 1))))))
(io/puts (str "deep = " (deep 100000)))
"#;
const PLAIN_DEEP: &str = r#"
(def level 0)
(defn deep (n) (if (= n 0) 100000 (let (l (+ level 1)) (+ 0 (deep (- n 1))))))
(io/puts (str "deep = " (deep 100000)))
"#;

#[test]
fn tier0_global_reads_under_deep_binding_stay_linear() {
    let timed = |name: &str, source: &str| {
        let started = std::time::Instant::now();
        let (stdout, stderr) = run_tier0(name, source);
        assert_eq!(
            stdout.trim(),
            "deep = 100000",
            "{name} did not complete.\nstderr:\n{stderr}"
        );
        started.elapsed()
    };
    let plain = timed("deep-plain", PLAIN_DEEP);
    let bound = timed("deep-binding", BINDING_DEEP);
    assert!(
        bound < plain * 10 + std::time::Duration::from_secs(2),
        "100 000 nested `binding`s took {bound:?} against {plain:?} without: a global read \
         is scanning the dynamic stack again"
    );
}
