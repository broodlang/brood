//! A definition published while the RUNTIME code region is aging keeps its source positions.
//!
//! Code a load promoted into the old generation, and published after the flip, is re-homed
//! into the current generation at `def` time (`rehome_to_current_locked`). The re-home made
//! new pairs and left their positions behind, so an error inside such a function reported the
//! CALLER's line — found 2026-10-23 when the parallel cold load (several processes loading
//! modules at once) lost them on about one module in 25. Reproduced here the same way: four
//! processes `require-one` twenty modules concurrently under `BROOD_GC_STRESS`, which turns
//! the region over constantly; every module's inner error must report its own line. Without
//! the fix this failed three runs in three; with aging disabled it never failed.
//!
//! The loaders are `spawn-link`ed: a loader that crashes takes the main process with it, so
//! the run exits at once with the crash on stderr. Unlinked, a crashed loader left main
//! waiting for a `:done` that never came, and the test hung to nextest's 120 s cap with
//! nothing printed (KI-409's freed generation, seen that way in the tree-walker job).

use std::process::Command;

fn module(i: usize) -> String {
    let mut src = format!("(defmodule pm{i} \"fixture module {i}\")\n\n");
    for f in 0..60 {
        // the `(+ x 3)` sits on the third line of each function: line 3 + 7·f + 2
        src.push_str(&format!(
            "(defn m{i}-f{f} (x)\n  \"variant {f}\"\n  (let (y (+ x 3)\n        z (* y 4))\n    (+ z {f})))\n\n\n"
        ));
    }
    src
}

#[test]
fn concurrent_loads_under_runtime_aging_keep_inner_positions() {
    let dir = std::env::temp_dir().join(format!("brood-rehome-pos-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..20 {
        std::fs::write(dir.join(format!("pm{i}.blsp")), module(i)).unwrap();
    }
    // f2 starts on line 3 + 7·2 = 17; its `(+ x 3)` is two lines down
    let main = format!(
        "(reflect/add-load-path {dir:?})\n\
         (let (me (self))\n\
           (doseq (w (range 4))\n\
             (spawn-link (do (doseq (i (range 20)) (when (= w (math/rem i 4)) (require-one (symbol (str \"pm\" i)))))\n\
                        (send me :done))))\n\
           (doseq (_ (range 4)) (receive (:done nil))))\n\
         (def bad (seq/filter (range 20)\n\
           (fn (i) (let (e (try ((reflect/eval (symbol (str \"pm\" i \"/m\" i \"-f2\"))) \"x\") (catch e e)))\n\
                     (not (= 19 (get e :line)))))))\n\
         (io/puts (str \"BAD \" (pr-str bad)))\n",
        dir = dir.display().to_string()
    );
    std::fs::write(dir.join("main.blsp"), main).unwrap();
    for run in 0..2 {
        let out = Command::new(env!("CARGO_BIN_EXE_brood"))
            .arg(dir.join("main.blsp"))
            .env("BROOD_GC_STRESS", "1")
            .output()
            .expect("run brood");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("BAD nil"),
            "run {run} ({}): modules whose inner error lost its line, or a loader crashed:\n{stdout}\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
