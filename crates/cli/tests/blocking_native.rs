//! **A worker blocked in a long native call runs nothing else, and the scheduler knows it**
//! (KI-280, ADR-100 §7.4).
//!
//! `os/cmd` waits for its child on the calling worker's thread. It used to do that without
//! marking the worker dirty-blocked, so the scheduler still counted it as live: a process
//! woken meanwhile was routed to it and sat in its queue until the command returned. On a
//! two-worker pool, two such calls (or one beside a tree-walked `receive`, which does mark
//! its worker) left nobody to run anything — the memory-pressure stuck-victim test starved
//! its leak for the whole 20 s of the hoarder's `sleep` under `BROOD_TIER=0` on two cores.

use std::process::Command;

const SCRIPT: &str = r#"
(def me (self))
(defn blocker () (os/cmd "sleep" ["4"]))
(spawn (blocker))
(spawn (blocker))
(proc/sleep 300)
(spawn (send me :ran))
(receive (:ran (io/puts "ran while both workers were in os/cmd"))
         (after 2000 (io/puts "starved behind the blocked workers")))
"#;

#[test]
fn a_process_runs_while_every_worker_waits_in_os_cmd() {
    let directory = std::env::temp_dir().join(format!("brood-ki280-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    std::fs::write(directory.join("main.blsp"), SCRIPT).expect("write the script");
    for tier in ["0", "1", "2"] {
        let output = Command::new(env!("CARGO_BIN_EXE_brood"))
            .arg("main.blsp")
            .current_dir(&directory)
            // Two workers: the pool's floor, and the size that both blockers fill.
            .env("BROOD_J", "2")
            .env("BROOD_TIER", tier)
            .output()
            .expect("run brood");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("ran while both workers were in os/cmd"),
            "tier {tier}: a process woken while both workers waited in os/cmd did not run\n\
             stdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// Both workers BUSY under a native frame: each process loops inside `reflect/eval-string`,
/// whose nested run cannot yield. The parent sleeps, then kills them — if it ever gets a
/// worker. It did not: the busy workers stayed counted as live, nothing replaced them, and
/// the program hung for good on two workers. The in-language suite sat 30 minutes on two
/// cores with both workers looping under `eval` (2026-10-21), the runner's own timeout
/// unable to run, and on a loaded machine the same starvation turned one test into a 600 s
/// timeout whose clean-up failed 343 more. A nested run that spends its quantum with work
/// waiting now takes its worker out of the pool (`cpu_dirty_enter`).
const SPINNERS: &str = r#"
(def spinners
  (map (range 2)
    (fn (_) (spawn (reflect/eval-string "(defn spin (n) (spin (inc n))) (spin 0)")))))
(proc/sleep 300)
(doseq (p spinners) (exit p :kill))
(io/puts "ran while both workers looped under eval")
"#;

#[test]
fn a_process_runs_while_every_worker_loops_under_a_native_frame() {
    let directory = std::env::temp_dir().join(format!("brood-busy-nested-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    std::fs::write(directory.join("main.blsp"), SPINNERS).expect("write the script");
    for tier in ["0", "1", "2"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_brood"))
            .arg("main.blsp")
            .current_dir(&directory)
            .env("BROOD_J", "2")
            .env("BROOD_TIER", tier)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("run brood");
        // Starved, the program never exits: kill it at a deadline instead of hanging the case.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while child.try_wait().expect("poll brood").is_none() {
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let output = child.wait_with_output().expect("collect brood's output");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("ran while both workers looped under eval"),
            "tier {tier}: the parent never got a worker while both looped under eval \
             (killed after 20 s)\nstdout: {stdout}\nstderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&directory);
}

/// **The child of `os/cmd` dies with its caller.** `os/cmd` waits in native code, where an
/// exit signal used to reach only the caller's flag: the caller died when the command
/// returned, and the command ran to the end on a worker nothing read from again — a
/// cancelled search task left its `rg` running. The wait now looks for its caller's exit
/// signal between polls and kills the child's group. Each stop here — the untrappable
/// `:kill`, a soft exit, `task/cancel` — is tried on a caller blocked on a 30 s `sleep`.
const CALLER_DIES: &str = r#"
(defn wait-for (ready? tries)
  (cond
    (ready?) true
    (= tries 0) false
    else (do (proc/sleep 20) (wait-for ready? (dec tries)))))
(defn alive? (pid) (= 0 (:exit (os/cmd "kill" ["-0" pid]))))
(defn started (pid-file)
  ;; the child's pid, once it has written it
  (wait-for (fn () (and (file/exists? pid-file) (not (= "" (string/trim (file/slurp pid-file))))))
    250)
  (string/trim (file/slurp pid-file)))
(defn blocked (pid-file)
  (os/cmd "sh" ["-c" (str "echo $$ > " pid-file "; exec sleep 30")]))
(defn report (label pid)
  (io/puts label
    (if (wait-for (fn () (not (alive? pid))) 100) "child died with its caller" "child outlived its caller")))
(defn by-exit (label reason)
  (let ([caller ref] (spawn-monitor (blocked (str label ".pid")))
        pid (started (str label ".pid")))
    (exit caller reason)
    (receive
      ([:down ^ref _ _] nil)
      (after 2000 (io/puts label "caller survived")))
    (report label pid)))
(by-exit "kill" :kill)
(by-exit "shutdown" :shutdown)
(let (handle (task/task (fn () (blocked "cancel.pid")))
      pid (started "cancel.pid"))
  (task/cancel handle)
  (report "cancel" pid))
"#;

#[test]
fn a_command_dies_with_the_process_waiting_on_it() {
    let directory = std::env::temp_dir().join(format!("brood-cmd-dies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    std::fs::write(directory.join("main.blsp"), CALLER_DIES).expect("write the script");
    for tier in ["0", "2"] {
        let output = Command::new(env!("CARGO_BIN_EXE_brood"))
            .arg("main.blsp")
            .current_dir(&directory)
            .env("BROOD_TIER", tier)
            .output()
            .expect("run brood");
        let stdout = String::from_utf8_lossy(&output.stdout);
        for label in ["kill", "shutdown", "cancel"] {
            assert!(
                stdout.contains(&format!("{label} child died with its caller")),
                "tier {tier}, stopped by {label}: the command outlived its caller\n\
                 stdout: {stdout}\nstderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        for label in ["kill", "shutdown", "cancel"] {
            let _ = std::fs::remove_file(directory.join(format!("{label}.pid")));
        }
    }
    let _ = std::fs::remove_dir_all(&directory);
}
