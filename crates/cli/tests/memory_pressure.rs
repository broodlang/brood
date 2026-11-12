//! **The soft memory limit stops the process holding the memory, not a bystander**
//! (KI-229).
//!
//! The soft limit counts every byte the allocator holds, so crossing it does not say which
//! process is to blame. It used to raise in whichever process next reached a safepoint: a
//! busy worker that makes only garbage reaches one constantly, so it died, run after run,
//! while the process actually accumulating kept going. Now the runtime measures every
//! process and stops the largest holder (`process/pressure.rs`).
//!
//! Each case runs a real program under `BROOD_MEM_LIMIT` with one culprit and one bystander,
//! and asserts on what a supervisor would see: which pid went down, and that the bystander
//! is still alive afterwards.

use std::process::{Command, Stdio};

/// Run `script` under a 512 MiB limit; return `(stdout, stderr)`.
fn run_limited(name: &str, script: &str) -> (String, String) {
    run_with_soft_limit(name, script, None)
}

/// Run `script` under a 512 MiB hard limit and the given soft one (default: three
/// quarters of it); return `(stdout, stderr)`.
fn run_with_soft_limit(name: &str, script: &str, soft: Option<&str>) -> (String, String) {
    let directory = std::env::temp_dir().join(format!("brood-ki229-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("create the scratch directory");
    std::fs::write(directory.join("main.blsp"), script).expect("write the script");
    let mut command = Command::new(env!("CARGO_BIN_EXE_brood"));
    if let Some(soft) = soft {
        command.env("BROOD_MEM_SOFT_LIMIT", soft);
    }
    let output = command
        .arg("main.blsp")
        .current_dir(&directory)
        .env("BROOD_MEM_LIMIT", "512M")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run brood");
    let _ = std::fs::remove_dir_all(&directory);
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Wait for the culprit's `[:down …]`, then report who went down and whether the bystander
/// lives. Shared by both programs, which define `culprit` and `bystander`.
const VERDICT: &str = r#"
(monitor culprit)
(monitor bystander)
(io/puts "culprit" culprit)
(receive
  ([:down _ pid [:error {:code code :message message}]]
    (io/puts "down" pid code)
    (io/puts "reason" message))
  ([:down _ pid reason] (io/puts "down" pid reason))
  (after 60000 (io/puts "nothing went down")))
(io/puts "bystander alive" (proc/alive? bystander))
"#;

fn assert_culprit_stopped(stdout: &str, stderr: &str) {
    let culprit = stdout
        .lines()
        .find_map(|line| line.strip_prefix("culprit "))
        .unwrap_or_else(|| panic!("the program never started:\n{stdout}\n{stderr}"));
    assert!(
        stdout.contains(&format!("down {culprit} E0043")),
        "the culprit was not the process stopped:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("bystander alive true"),
        "the bystander did not survive:\n{stdout}\n{stderr}"
    );
    // Who noticed the crossing decides the path. Usually a bystander's safepoint does, and
    // the selector stops the culprit and says so on stderr before the stop lands. Under
    // load the culprit can reach a safepoint first: it is then the largest holder and
    // raises the error in itself — catchable, so the runtime prints nothing, and its crash
    // report is asynchronous (the main process may exit before it is written). Both stop
    // the right process; only the selector's path promises the stderr line.
    let reason = stdout
        .lines()
        .find_map(|line| line.strip_prefix("reason "))
        .unwrap_or_else(|| panic!("no exit reason printed:\n{stdout}\n{stderr}"));
    if reason.contains("held the most memory") {
        assert!(
            stderr.contains("stopping process"),
            "the selector stopped the culprit but nothing on stderr named it:\n{stderr}"
        );
    } else {
        assert!(
            reason.contains("allocated process-wide exceeds"),
            "the culprit went down with neither memory-limit error:\n{stdout}\n{stderr}"
        );
    }
}

/// The leak grows in pieces under the allocator's large-allocation size, so it never
/// notices the crossing itself: the busy worker does, and must not be the one to pay.
#[test]
fn a_leak_is_stopped_and_a_busy_worker_beside_it_is_not() {
    let script = format!(
        r#"
(defn leaky (kept)
  (receive (after 2 (leaky (conj kept (map (range 20) (fn (_) (string/repeat "x" 50000))))))))
(defn busy (n)
  (let (garbage (map (range 2000) (fn (i) [i (str i)])))
    (busy (+ n (count garbage)))))
(def culprit (spawn (leaky [])))
(def bystander (spawn (busy 0)))
{VERDICT}"#
    );
    let (stdout, stderr) = run_limited("leak", &script);
    assert_culprit_stopped(&stdout, &stderr);
}

#[test]
fn a_flooded_mailbox_is_stopped_and_a_ticker_beside_it_is_not() {
    let script = format!(
        r#"
(defn sink () (receive ([:never] nil)))
(defn flood (target n)
  (send target [:blob (string/repeat "x" 100000) n])
  (flood target (inc n)))
(defn ticker () (receive (after 10 (ticker))))
(def culprit (spawn (sink)))
(def bystander (spawn (ticker)))
(spawn (flood culprit 0))
{VERDICT}"#
    );
    let (stdout, stderr) = run_limited("flood", &script);
    assert_culprit_stopped(&stdout, &stderr);
}

/// The largest holder is stopped while it is inside a long native call, so it reaches no
/// safepoint to die at until the call returns. A leak beside it kept growing, and while the
/// stopped process lived nobody else was stopped: the leak reached the hard limit and the
/// runtime aborted. Now a victim that has not died within the grace is looked past.
#[test]
fn a_victim_stuck_in_a_native_call_does_not_shield_the_next_holder() {
    let (stdout, stderr) = run_limited(
        "stuck",
        // The hoarder stops growing inside the last eighth below the soft limit (384 MiB
        // here; the eighth starts at 352 MB), where it has measured itself, then blocks.
        //
        // The leak is primed to 64 MB first and held there while the hoarder fills up, then
        // released. Without the head start it had grown only ~40-50 MB when the hoarder's
        // grace ran out — under the eighth of the count (~55 MB) a process must hold to be
        // stopped — so nobody was attributable and the error was raised in whichever
        // process reached a safepoint first. On a slow runner that was the waiting root
        // (CI's tree-walker job, 2026-10-18, KI-279), which died instead of the leak. Primed, the
        // leak holds an attributable share whoever reaches the safepoint, and the total
        // (~370 + 64 + what it grows in the grace) stays under the 512 MiB hard limit.
        r#"
(defn hoard (kept parent)
  (if (> (%mem-bytes) 370000000)
    (do (send parent :hoarded) (os/cmd "sleep" ["20"]) (count kept))
    (hoard (conj kept (string/repeat "x" 1000000)) parent)))
(defn leaky (kept)
  (receive (after 5 (leaky (conj kept (string/repeat "x" 1000000))))))
(defn primed-leak (kept parent)
  (if (< (count kept) 64)
    (primed-leak (conj kept (string/repeat "x" 1000000)) parent)
    (do (send parent :primed) (receive (:go (leaky kept))))))
(def me (self))
(def leak (spawn (primed-leak [] me)))
(monitor leak)
(receive (:primed nil) (after 60000 (io/puts "never primed")))
(def hoarder (spawn (hoard [] me)))
(receive (:hoarded nil) (after 60000 (io/puts "never hoarded")))
(io/puts "leak" leak)
(send leak :go)
(receive
  ([:down _ pid [:error {:code code}]] (io/puts "down" pid code))
  ([:down _ pid reason] (io/puts "down" pid reason))
  (after 60000 (io/puts "nothing went down")))
"#,
    );
    let leak = stdout
        .lines()
        .find_map(|line| line.strip_prefix("leak "))
        .unwrap_or_else(|| panic!("the leak never started:\n{stdout}\n{stderr}"));
    assert!(
        stderr.contains("stopping process"),
        "the hoarder was never chosen, so nothing was stuck:\n{stderr}"
    );
    assert!(
        stdout.contains(&format!("down {leak} E0043")),
        "the leak beside the stuck victim was not stopped:\n{stdout}\n{stderr}"
    );
    assert!(
        !stderr.contains("hard memory limit reached"),
        "the runtime reached the hard limit:\n{stderr}"
    );
}

/// Memory held in a table belongs to no process, so stopping one frees none of it, and the
/// `E0043` used to say nothing about where the memory was. It names the table now.
#[test]
fn a_table_holding_the_memory_is_named() {
    let (_, stderr) = run_limited(
        "table",
        r#"
(def t (table/new))
(defn fill (i) (table/put t i (string/repeat "x" 1000000)) (fill (inc i)))
(fill 0)"#,
    );
    assert!(
        stderr.contains("memory limit exceeded") && stderr.contains("hint: table #"),
        "the error did not name the table:\n{stderr}"
    );
}

/// One allocation past the hard limit cannot be stopped gracefully — no safepoint runs
/// inside it — so the runtime aborts. It used to say only `memory allocation of N bytes
/// failed`, naming neither the limit nor that one was set. (The soft limit sits just under
/// the hard one so the 100 MB piece can be built first; the 500 MB `str` is the one step.)
#[test]
fn one_allocation_past_the_hard_limit_names_the_limit() {
    let (stdout, stderr) = run_with_soft_limit(
        "hard",
        r#"
(def quarter (string/repeat "x" 100000000))
(%gc-collect)
(io/puts "started")
(io/puts (%string-length (str quarter quarter quarter quarter quarter)))"#,
        Some("500M"),
    );
    assert!(
        stdout.contains("started"),
        "the program never started:\n{stderr}"
    );
    assert!(
        stderr.contains("hard memory limit reached (512 MiB, BROOD_MEM_LIMIT)")
            && stderr.contains("one value this large"),
        "the abort did not name the limit and the cause:\n{stderr}"
    );
}

/// A loop that allocates a megabyte an iteration, with no `receive`, covered the soft
/// limit's whole headroom inside one scheduling quantum and reached the hard limit — an
/// abort — before the once-per-quantum check ran. A large allocation over the soft limit
/// now ends the quantum, so the loop stops with the catchable error.
#[test]
fn a_fast_allocating_loop_stops_at_the_soft_limit_not_the_hard_one() {
    let (_, stderr) = run_limited(
        "fast",
        r#"
(defn hoard (kept) (hoard (conj kept (string/repeat "x" 1000000))))
(hoard [])"#,
    );
    assert!(
        !stderr.contains("hard memory limit reached"),
        "the loop reached the hard limit:\n{stderr}"
    );
    // The soft limit's error, wherever it landed: the tree-walker raises it inside the
    // Brood function that was allocating and names that function first
    // (`string/%repeat-doubling: memory limit exceeded …`); the compiled tiers raise it
    // unnamed.
    assert!(
        stderr.contains("runtime error: ")
            && stderr.contains("memory limit exceeded")
            && stderr.contains("soft limit"),
        "the loop was not stopped by the soft limit:\n{stderr}"
    );
}
