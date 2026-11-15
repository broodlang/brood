//! **A refused OS thread is an error the program sees, never a panic or a hang** (KI-241).
//!
//! The OS refuses a new thread (`EAGAIN`) under a thread or task limit — `ulimit -u`, a
//! container's pids limit. Several runtime facilities start a thread on first use, and
//! three of them panicked on a refusal: a child process's pipe readers (after the child had
//! started, leaving it running with nobody to reap it), the offload pool, and the net
//! reactor. A runtime that could start no scheduler worker at all hung without a word.
//!
//! The limit is per user, so each program runs in a fresh user namespace (`unshare -U`),
//! where the count starts from zero and the budget is the same on every machine.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Every facility that starts a thread on first use, each caught and reported, then work
/// that must still finish.
const SCRIPT: &str = r#"
(io/puts "started")
(io/puts "offload" (try (count (proc/offload file/slurp-bytes "main.blsp")) (catch e (error-message e))))
(io/puts "listen" (try (do (tcp/listen "127.0.0.1" 0) :listening) (catch e (error-message e))))
(io/puts "spawn" (try (do (os/spawn "true" []) :spawned) (catch e (error-message e))))
(defn spin (n acc) (if (= n 0) acc (spin (- n 1) (+ acc 1))))
(io/puts "finished" (spin 300000 0))
"#;

struct Outcome {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run `SCRIPT` with at most `limit` threads, killing it after a minute.
fn run_with_thread_limit(directory: &Path, limit: u32) -> Outcome {
    let binary = env!("CARGO_BIN_EXE_brood");
    let mut child = Command::new("unshare")
        .args(["-U", "bash", "-c"])
        .arg(format!("ulimit -u {limit} && exec '{binary}' main.blsp"))
        .current_dir(directory)
        .env("BROOD_J", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start unshare");
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().expect("poll brood").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().expect("collect brood's output");
    Outcome {
        status: output.status.code(),
        stdout: String::from_utf8_lossy_owned(output.stdout),
        stderr: String::from_utf8_lossy_owned(output.stderr),
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("brood-ki241-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    std::fs::write(directory.join("main.blsp"), SCRIPT).expect("write the script");
    directory
}

/// A user namespace is what makes the budget exact. Where the kernel refuses one, this
/// guard cannot run, and that has to be loud: a skipped guard reads as coverage.
fn assert_namespaces_available() {
    let probe = Command::new("unshare")
        .args(["-U", "true"])
        .status()
        .expect("run unshare");
    assert!(
        probe.success(),
        "`unshare -U` is refused here, so this test cannot set a per-run thread budget"
    );
}

#[test]
fn each_refused_thread_is_a_caught_error_and_the_program_finishes() {
    assert_namespaces_available();
    let directory = scratch("refused");
    // Enough for the runtime to boot with one worker, too few for anything more.
    let outcome = run_with_thread_limit(&directory, 4);
    let _ = std::fs::remove_dir_all(&directory);
    let report = format!("{}\n{}", outcome.stdout, outcome.stderr);
    assert!(
        !outcome.stderr.contains("panicked"),
        "a refused thread panicked:\n{report}"
    );
    assert!(
        outcome.stdout.contains("finished 300000"),
        "the program did not finish:\n{report}"
    );
    for (facility, refusal) in [
        ("offload", "cannot start an offload thread"),
        ("listen", "cannot start the network thread"),
        ("spawn", "cannot start a thread for the child's pipes"),
    ] {
        let line = outcome
            .stdout
            .lines()
            .find(|line| line.starts_with(facility))
            .unwrap_or_else(|| panic!("no `{facility}` line:\n{report}"));
        assert!(
            line.contains(refusal),
            "`{facility}` was not refused as an error ({line}); the budget no longer starves it:\n{report}"
        );
    }
    assert_eq!(outcome.status, Some(0), "{report}");
}

#[test]
fn a_runtime_without_a_worker_says_so_and_exits() {
    assert_namespaces_available();
    let directory = scratch("no-worker");
    // Two: the main thread and the program's, and nothing else. Three left room for a worker
    // whenever the JIT compiler thread had not started first — and whether it had depended on
    // how much the boot compiled: loading the standard library from source tiered something up
    // before the pool started, booting from the stdlib image did not. So the test passed only
    // without an image, failing at tier 0 always and at tier 2 once the image was live.
    let outcome = run_with_thread_limit(&directory, 2);
    let _ = std::fs::remove_dir_all(&directory);
    let report = format!("{}\n{}", outcome.stdout, outcome.stderr);
    assert!(
        outcome.stderr.contains("cannot start any scheduler worker"),
        "no error named the missing workers:\n{report}"
    );
    assert_eq!(outcome.status, Some(1), "{report}");
}
