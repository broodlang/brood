//! **A loop handed to its recompiled body (ADR-366) keeps its `receive`s clean suspends.**
//!
//! A self-tail loop whose first iteration misses on a lazily-loaded module (ADR-335) is
//! marked stale and recompiled; the running activation adopts the recompile at its next
//! back-edge through the `SelfCall` hot-reload guard's tail transition. That transition used
//! to be a nested `apply_value` — "one native frame for the transition only" — and the frame
//! was not the cost: the recompiled body ran the REST OF THE LOOP'S LIFE under it, where a
//! `receive` cannot be state-captured and parks its whole OS worker instead. 20 000 of
//! 20 000 receives parked dirty on this program; a `(defn serve (state) … (receive …) (serve
//! next))` whose first iteration touches a lazily-loaded module is the shape of every server
//! loop. The nested run also had no driver to yield a native preempt to, so the loop fell to
//! the interpreter for up to 256 iterations per preempt (`collatz` +6.5%, `sort` +5.7%
//! instructions under lazy loading against the eager path).
//!
//! The transition is now a `ChunkExit::Tail` — the driver reuses the frame, exactly as a tail
//! `Inst::Call` does. Pinned by the runtime's own counter of dirty parks
//! (`process::dirty_receive_block_count`, printed by `BROOD_JIT_BAIL_TRACE` as
//! `dirty-receive-block`), and refusing to be vacuous: the loop must have been marked stale
//! (`BROOD_TRACE_COMPILE`'s `[compile] stale-bindings arm=drive` line), or the transition
//! under test never ran — which it does not for a STD module under a source boot (the first
//! shape of this file used `math/max`, and CI's no-image job would have failed its vacuity
//! guard); the lazily-loaded module is a load-path one, which loads at its first lookup miss
//! under every boot. Sabotage-verified: restoring the nested `apply_value` in
//! `exec_chunk`'s `SelfCall` guard turns the count into thousands.

use std::process::Command;

const PROGRAM: &str = r#"
(reflect/add-load-path "LOAD_PATH")
(def root (self))
(defn echo () (receive ([:ping from] (do (send from :pong) (echo)))))
(def e (spawn (echo)))
(defn rpc (v) (do (send e [:ping (self)]) (receive (:pong v))))
;; `drive` stays on the VM (the `try` keeps its chunk out of the JIT subset), so the only
;; way a receive down its body can park dirty is the transition under test. Its FIRST
;; iteration misses on `lazymod/mx`, which lazily loads `lazymod` and marks `drive` stale.
;; A LOAD-PATH module REFERENCED as a value, not a std one called by name: a call head into
;; a module the stdlib image does not describe loads at expansion (the kind index that says
;; it is not a macro is the image's — ADR-335), so under a source boot a `(math/max …)`
;; here never lazily loaded and the shape under test never arose; a function reference to
;; a load-path module loads at its first lookup miss under every boot (checked: image, no
;; image, no prelude image).
(defn drive (i acc)
  (if (= i 0) acc (drive (- i 1) (apply lazymod/mx (list acc (+ (rpc i) (try 0 (catch _ 0))))))))
;; Run in a spawned process: only a green process in a capture run can be parked cleanly
;; (or dirtily) at all — the root thread's receive always blocks.
(defn run (n)
  (let (w (spawn (send root [:r (drive n 0)])))
    (receive ([:r v] v) (after 60000 :timeout))))
(io/puts (str "run = " (pr-str (run 20000))))
"#;

#[test]
fn a_loop_recompiled_after_a_lazy_load_never_parks_its_receives_dirty() {
    let dir = std::env::temp_dir().join(format!("brood-stale-loop-handoff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("sandbox dir");
    std::fs::write(
        dir.join("lazymod.blsp"),
        "(defmodule lazymod)\n(defn mx (a b) (if (> a b) a b))\n",
    )
    .expect("write module");
    let path = dir.join("drive.blsp");
    std::fs::write(
        &path,
        PROGRAM.replace("LOAD_PATH", &dir.display().to_string()),
    )
    .expect("write program");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        // The runner's pre-flight check loads the file's references eagerly, which is
        // exactly the load that must instead happen INSIDE the loop here.
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_JIT_BAIL_TRACE", "1")
        .env("BROOD_TRACE_COMPILE", "1")
        // Pin the engine to the shipped ceiling. The property under test is the VM's —
        // `SelfCall`'s hot-reload guard taking a `ChunkExit::Tail` instead of a nested
        // `apply_value` — and under `BROOD_VM=0` there is no chunk, no guard and no such
        // transition, so CI's `differential (tree-walker)` job ran this against an engine it
        // was never about and read 1 dirty park as a failure (2026-09-20, red on `2c1596c0`).
        // Ceiling 2 rather than 1 so the DEFAULT configuration is what is exercised; `drive`
        // stays on the VM there regardless, because its `try` keeps the chunk out of the JIT
        // subset — which is the same reason the assertion below can attribute a dirty park to
        // the transition at all. Same fix, and the same reason, as `mapprim3_differential`
        // and `prelude_image_matches_source`.
        .env("BROOD_TIER", "2")
        .env_remove("BROOD_VM")
        .env_remove("BROOD_NO_JIT")
        .env_remove("BROOD_NO_LAZY_LOAD")
        .arg(&path)
        .output()
        .expect("run brood");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains("run = 20000"),
        "the parked receives did not all round-trip:\n{stdout}\n{stderr}"
    );
    // Vacuity guard: the loop must have been marked stale by the lazy load, or the
    // transition under test never happened.
    assert!(
        stderr
            .lines()
            .any(|l| l.starts_with("[compile] stale-bindings arm=drive")),
        "`drive` was never marked stale, so the ADR-366 handoff was never taken — the \
         shape under test no longer lazily loads; find out why before trusting this file:\n{stderr}"
    );
    let dirty = stderr
        .lines()
        .filter(|l| l.contains("dirty-receive-block"))
        .count();
    assert_eq!(
        dirty, 0,
        "{dirty} receives parked their OS worker (dirty blocks): the loop's recompiled body \
         ran nested under the stale activation instead of being handed to the driver"
    );
}
