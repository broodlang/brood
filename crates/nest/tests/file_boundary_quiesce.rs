//! The scoped `nest test` runner quiesces the runtime at every FILE boundary: a process
//! that outlived the file whose tests spawned it is killed before that file's `%isolate`
//! restores the globals (KI-89's residual class). `%isolate` itself reaps only the thunk's
//! *descendants by spawn ancestry*, and a process whose parent — the test's worker — has
//! already exited falls through that filter, so without the runner's step it runs on into
//! the next file against globals that were rolled back under it. Measured 2026-09-03: one
//! to eighteen such processes per file across brood's own suite, on every run.
//!
//! Two files, run in one scoped `nest test`: the first leaves a registered process parked
//! forever; the second asserts the name is gone. Runs the real `nest` binary because the
//! scoped path (per-file `%isolate`) is only taken for a whole-project run, never for
//! explicit files. Sabotage-verified: with the `test-quiesce-file` call removed from
//! `drain-files-scoped` the second file fails (`:ki89-leaker` still registered) and the
//! trace line below never prints.
//!
//! The count in the assertion is EXACT on purpose. On 2026-09-04 this test read
//! `2 process(es) … (nil :ki89-leaker)` one run in three: the runner's own worker (or driver)
//! was still retiring at the file boundary — `collect-loop` counted a unit done at its result
//! and dropped the `:down`, and `drain-runner` flushed the driver's `:down` with `(after 0)`,
//! which only consumes one that has already arrived. Both now wait for the exit. A `contains`
//! assertion would have passed while the runner counted itself as a leak.

use std::path::Path;
use std::process::Command;

fn scaffold(dir: &Path) {
    let tests = dir.join("tests");
    std::fs::create_dir_all(&tests).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project :name \"quiesce-demo\")\n",
    )
    .unwrap();
    // File A: the leak. The spawned process's parent is the test's WORKER, which exits as
    // soon as the unit reports — so at the file boundary the leaker has no live ancestor
    // and `%isolate`'s own reaper cannot attribute it to the thunk.
    std::fs::write(
        tests.join("aaa_leak_test.blsp"),
        "(defmodule aaa-leak-test (:use test))\n\
         (describe \"a test that leaks a process\"\n\
           (test \"leaves a registered process parked behind\"\n\
             (proc/register :ki89-leaker (spawn (fn () (receive (:never nil)))))\n\
             (is (not (nil? (proc/whereis :ki89-leaker))))))\n",
    )
    .unwrap();
    // File B: nothing from a finished file may still be running.
    std::fs::write(
        tests.join("zzz_check_test.blsp"),
        "(defmodule zzz-check-test (:use test))\n\
         (describe \"the next file starts quiescent\"\n\
           (test \"the previous file's leaked process is gone\"\n\
             (is (nil? (proc/whereis :ki89-leaker)))))\n",
    )
    .unwrap();
}

#[test]
fn a_process_leaked_by_one_file_is_dead_before_the_next_file_runs() {
    let dir = std::env::temp_dir().join(format!("brood-quiesce-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    scaffold(&dir);
    // `--trace` makes the runner print `[test] quiesce: N process(es) …` per file, which is
    // what proves the leak EXISTED and was killed — without it a run where file B happened
    // to go first would pass vacuously.
    let out = Command::new(env!("CARGO_BIN_EXE_nest"))
        .args(["test", "--trace"])
        .current_dir(&dir)
        .output()
        .expect("spawn nest test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "the leaked process outlived its file (KI-89 class)\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        stderr.contains("[test] quiesce: 1 process(es) outlived their file and were killed"),
        "the runner must report the one straggler it killed at the file boundary — \
         no report means the leak never happened and this test proved nothing\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("2 tests, 2 passed"),
        "both files must have run\n--- stdout ---\n{stdout}"
    );
}
