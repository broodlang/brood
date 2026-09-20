//! **A module is never "loaded" with nothing bound.**
//!
//! `*features*` listing a module is the promise every requirer relies on: `require-one`
//! returns immediately for a key it finds there, and `(:use mod)` refers that module's
//! publics bare. If the key is present while the module's globals are not, the state is
//! **permanent for the life of the runtime** — the requirer that could repair it is exactly
//! the one that short-circuits — and every bare use dies `unbound symbol` in whichever
//! process reaches it, far from the cause. The runtime prints
//! `[refer] (:use m) imported NOTHING — no public m/ global is bound; *features* lists it:
//! true` when it sees this, which is the only reason it is ever attributable.
//!
//! The window this pins: a file loaded by a DIRECT `reflect/load` (no `require` driving it)
//! has `defmodule` `provide` its key at the TOP of the file — before a single definition
//! exists — and that load is wrapped in neither the staging frame (ADR-344) nor the load
//! journal (ADR-339/KI-134). So a globals snapshot taken between the provide and the
//! definitions records "loaded, nothing bound", and the `%isolate` restore that follows
//! rolls the definitions back (nothing journalled them) while keeping the provide (it was
//! already in the saved table). Seen in the wild as `set`, `sexp` and `sse` in one
//! `brood_suite_passes` run under load (2026-09-20), and as KI-119/KI-120 before that.
//!
//! The control in the same file is what makes this a test of the WINDOW and not of the
//! timing: the identical race driven through `require-one` — same file, same sleeps, same
//! isolate — must come out consistent, because that path stages and journals the load.
use std::process::Command;

const SLOWMOD: &str = r#"(defmodule slowmod "defs arrive after the provide")
(sleep 700)
(defn hello () "hi")
"#;

/// `(reflect/load …)` — the direct load, and the one under test.
const DRIVER_DIRECT: &str = r#"(defn main ()
  (let (parent (self)
        _a (spawn (fn () (do (reflect/load "MODDIR/slowmod.blsp") (send parent [:loaded])))))
    (sleep 250)
    (%isolate (fn () (sleep 900)))
    (receive ([:loaded] nil) (after 5000 nil))
    (sleep 200)
    (io/puts (str "features=" (contains? *features* "slowmod")
               " bound=" (bound? 'slowmod/hello)))))
(main)
"#;

/// `require-one` — the control: the same race through the staged, journalled path.
const DRIVER_REQUIRE: &str = r#"(reflect/set-load-path (list "MODDIR" "."))
(defn main ()
  (let (parent (self)
        _a (spawn (fn () (do (require-one 'slowmod) (send parent [:loaded])))))
    (sleep 250)
    (%isolate (fn () (sleep 900)))
    (receive ([:loaded] nil) (after 5000 nil))
    (sleep 200)
    (io/puts (str "features=" (contains? *features* "slowmod")
               " bound=" (bound? 'slowmod/hello)))))
(main)
"#;

fn run(tag: &str, driver_src: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "brood-load-provide-window-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("sandbox");
    std::fs::write(dir.join("slowmod.blsp"), SLOWMOD).expect("write module");
    let driver = driver_src.replace("MODDIR", dir.to_str().expect("utf-8 temp path"));
    let driver_path = dir.join("driver.blsp");
    std::fs::write(&driver_path, driver).expect("write driver");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(&dir)
        .env("BROOD_NO_CRASH_REPORT", "1")
        // The race is between a load and a concurrent snapshot, not between engines: pin the
        // ceiling so the tree-walker job measures the same thing this was written against.
        .env("BROOD_TIER", "2")
        .env_remove("BROOD_VM")
        .arg(&driver_path)
        .output()
        .expect("run brood");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
    text
}

/// The invariant, stated the way a caller depends on it: if `*features*` says loaded, the
/// module's names are bound. Asserted for BOTH load paths — the control would pass on a
/// tree where the window is wide open, so it is not evidence on its own; it is here to show
/// the timing really does close over the definition, i.e. that a failure of the first case
/// is the window and not a sleep that fired in the wrong order.
#[test]
fn a_module_that_features_calls_loaded_has_its_names_bound() {
    for (tag, src, what) in [
        ("direct", DRIVER_DIRECT, "a direct reflect/load"),
        ("require", DRIVER_REQUIRE, "require-one (the control)"),
    ] {
        let out = run(tag, src);
        assert!(
            out.contains("features="),
            "{what}: the driver did not report:\n{out}"
        );
        assert!(
            !out.contains("features=true bound=false"),
            "{what}: the module is permanently LOADED WITH NOTHING BOUND — `*features*` \
             lists it, so `require-one` will never repair it, and every `(:use …)` of it \
             imports nothing:\n{out}"
        );
    }
}
