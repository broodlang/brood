//! A process that finds another one mid-load of the module it needs PARKS until that load is
//! done — at every tier, and the JIT's tier-up does not wait for it either.
//!
//! The VM hands such a miss to the prelude's `%lazy-global`/`%lazy-call` as an ordinary frame
//! (`lazy_load_wait_parks.rs` is that guard, under the stdlib image). The tree-walker makes the
//! same hand-off from its machine, and the tier-up pre-load, which loads what a hot arm names
//! before it goes native, declines instead of waiting. A wait under a native frame cannot park:
//! it blocks the OS worker (ADR-100 §7.4). The runtime compensates for a blocked worker, so the
//! program's ANSWER is the same either way; what differs is whether the waiters park, which the
//! scheduler log says (`BROOD_SCHED_DBG`: one `park pid=N` line per park, none for a blocked
//! worker).
//!
//! The programs hold a module load open deterministically: every reference to `slowmod` is
//! compiled while the file does not exist (so none loads it at expansion), then the file is
//! written. Its body tells the program it is loading and waits for a go. The program starts the
//! waiters — half miss on a call head, half on a value — waits until each of them is waiting,
//! and only then lets the load finish.

use std::process::Command;

const PROGRAM: &str = r#"
(defn head-call (i) (slowmod/f i))
(defn value-call (i) (apply slowmod/f [i]))
(proc/register :main (self))
(file/spit "slowmod.blsp"
  "(defmodule slowmod)\n(send :main [:loading])\n(receive ([:go] nil))\n(defn f (x) (+ x 1))\n")
(%forget-absent-modules!)
(def loader (spawn (send :main [:got (head-call 0)])))
(receive ([:loading] nil) (after 20000 (io/puts "loader never started")))
(def waiters
  (append
    (map (range 0 3) (fn (i) (spawn (send :main [:got (head-call i)]))))
    (map (range 3 6) (fn (i) (spawn (send :main [:got (value-call i)]))))))
(doseq (w waiters) (io/puts (str "waiter " w)))
(defn all-waiting? ()
  (every? waiters (fn (w) (= (->string (get (proc/info w) :status)) "waiting"))))
(defn wait-all (k)
  (cond (all-waiting?) :ok (= k 0) :timeout else (do (proc/sleep 10) (wait-all (- k 1)))))
(io/puts (str "waiters " (wait-all 2000)))
(send loader [:go])
(defn collect (k acc)
  (if (= k 0) acc (collect (- k 1) (+ acc (receive ([:got v] v) (after 20000 -1000))))))
(io/puts (str "sum " (collect 7 0)))
"#;

/// Run `program.blsp` (the first file) beside the other `files` at `tier`; stdout, stderr.
fn run_at(name: &str, files: &[(&str, &str)], tier: &str) -> (String, String) {
    run_with(name, files, tier, &[])
}

/// [`run_at`] with extra environment variables.
fn run_with(
    name: &str,
    files: &[(&str, &str)],
    tier: &str,
    envs: &[(&str, &str)],
) -> (String, String) {
    let dir = std::env::temp_dir().join(format!(
        "brood-autoload-{name}-{}-{tier}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (file, source) in files {
        std::fs::write(dir.join(file), source).expect("write source");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(&dir)
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_SCHED_DBG", "1")
        .env("BROOD_TIER", tier)
        .envs(envs.iter().copied())
        .arg(files[0].0)
        .output()
        .expect("run brood");
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy_owned(out.stdout),
        String::from_utf8_lossy_owned(out.stderr),
    )
}

fn waiters_park_at(tier: &str) {
    let (stdout, stderr) = run_at("parks", &[("program.blsp", PROGRAM)], tier);
    let context = || {
        format!(
            "tier {tier}\nstdout:\n{stdout}\nstderr (tail):\n{}",
            tail(&stderr)
        )
    };
    assert!(
        stdout.contains("waiters :ok"),
        "the waiters never all waited — {}",
        context()
    );
    // The loader's f(0) = 1, plus f(0..6) = 1+2+3+4+5+6 from the waiters.
    assert!(
        stdout.contains("sum 22"),
        "a waiter did not get its answer — {}",
        context()
    );
    let pids: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("waiter #<pid nonode/"))
        .filter_map(|l| l.strip_suffix('>'))
        .collect();
    assert_eq!(pids.len(), 6, "expected six waiters — {}", context());
    for pid in pids {
        let parked = stderr
            .lines()
            .any(|l| l.contains(&format!("park pid={pid} ")));
        assert!(
            parked,
            "waiter {pid} never parked: it waited for the load with its worker blocked — {}",
            context()
        );
    }
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

#[test]
fn a_waiter_for_a_load_in_flight_parks_on_the_tree_walker() {
    waiters_park_at("0");
}

#[test]
fn a_waiter_for_a_load_in_flight_parks_on_the_vm() {
    waiters_park_at("1");
}

#[test]
fn a_waiter_for_a_load_in_flight_parks_with_the_jit() {
    waiters_park_at("2");
}

// The JIT's tier-up pre-load loads what a hot arm names before it goes native (ADR-335), and
// never waits to: while a module it names is mid-load in another process, the tier-up is
// declined, the arm stays on the VM, and the election retries later. `g` names `slowmod/f` only in a branch it never
// takes, and gets hot while `slowmod` is held mid-load; its answer must arrive before the load
// is let go. Waiting would hold the process until then.
const HOT: &str = r#"
(defn g (i) (if (< i 0) (slowmod/f i) i))
(defn spin (k acc) (if (= k 0) acc (spin (- k 1) (+ acc (g k)))))
(defn load-it () (slowmod/f 0))
(proc/register :main (self))
(file/spit "slowmod.blsp"
  "(defmodule slowmod)\n(send :main [:loading])\n(receive ([:go] nil))\n(defn f (x) (+ x 1))\n")
(%forget-absent-modules!)
(def loader (spawn (send :main [:got (load-it)])))
(receive ([:loading] nil) (after 20000 (io/puts "loader never started")))
(spawn (send :main [:hot (spin 200000 0)]))
(io/puts (str "hot " (receive ([:hot v] v) (after 20000 :blocked))))
(send loader [:go])
(io/puts (str "loaded " (receive ([:got v] v) (after 20000 :never))))
"#;

#[test]
fn a_tier_up_does_not_wait_for_a_load_in_flight() {
    let (stdout, stderr) = run_at("hot", &[("program.blsp", HOT)], "2");
    let context = || format!("stdout:\n{stdout}\nstderr (tail):\n{}", tail(&stderr));
    assert!(
        stdout.contains("hot 20000100000"),
        "the hot loop's answer waited for the load — {}",
        context()
    );
    assert!(
        stdout.contains("loaded 1"),
        "the load never finished — {}",
        context()
    );
}

// The claim race. A miss first asks whether another process holds its module's load and, if
// none does, loads it itself — and another process can take the claim BETWEEN the two. The
// loser then meets the claim inside `require-one`, under the native lookup, and its wait there
// could not park. `BROOD_FAULT_CLAIM_RACE=slowmod` holds the racer inside that window for
// 500 ms while the loader takes the claim; the racer must still park, on a call head and on a
// value, at every tier.
const RACE: &str = r#"
(defn head-call (i) (slowmod/f i))
(defn value-call (i) (apply slowmod/f [i]))
(proc/register :main (self))
(file/spit "slowmod.blsp"
  "(defmodule slowmod)\n(send :main [:loading])\n(receive ([:go] nil))\n(defn f (x) (+ x 1))\n")
(%forget-absent-modules!)
(def racer (spawn (send :main [:got (CALL 1)])))
(proc/sleep 100)
(def loader (spawn (send :main [:got (head-call 0)])))
(receive ([:loading] nil) (after 20000 (io/puts "loader never started")))
(io/puts (str "waiter " racer))
(defn wait-racer (k)
  (cond (= (->string (get (proc/info racer) :status)) "waiting") :ok
        (= k 0) :timeout
        else (do (proc/sleep 10) (wait-racer (- k 1)))))
(io/puts (str "racer " (wait-racer 2000)))
(send loader [:go])
(defn collect (k acc)
  (if (= k 0) acc (collect (- k 1) (+ acc (receive ([:got v] v) (after 20000 -1000))))))
(io/puts (str "sum " (collect 2 0)))
"#;

fn racer_parks_at(tier: &str, call: &str) {
    let program = RACE.replace("CALL", call);
    let (stdout, stderr) = run_with(
        "race",
        &[("program.blsp", &program)],
        tier,
        &[("BROOD_FAULT_CLAIM_RACE", "slowmod"), ("BROOD_J", "4")],
    );
    let context = || {
        format!(
            "tier {tier}, {call}\nstdout:\n{stdout}\nstderr (tail):\n{}",
            tail(&stderr)
        )
    };
    assert!(
        stdout.contains("racer :ok"),
        "the racer never waited — {}",
        context()
    );
    // The loader's f(0) = 1, plus the racer's f(1) = 2.
    assert!(
        stdout.contains("sum 3"),
        "an answer went missing — {}",
        context()
    );
    let pid = stdout
        .lines()
        .find_map(|l| l.strip_prefix("waiter #<pid nonode/"))
        .and_then(|l| l.strip_suffix('>'))
        .unwrap_or_else(|| panic!("no racer pid — {}", context()));
    assert!(
        stderr
            .lines()
            .any(|l| l.contains(&format!("park pid={pid} "))),
        "the racer lost the claim race and waited with its worker blocked — {}",
        context()
    );
}

#[test]
fn a_call_head_that_loses_the_claim_race_parks_on_the_tree_walker() {
    racer_parks_at("0", "head-call");
}

#[test]
fn a_value_read_that_loses_the_claim_race_parks_on_the_tree_walker() {
    racer_parks_at("0", "value-call");
}

#[test]
fn a_call_head_that_loses_the_claim_race_parks_on_the_vm() {
    racer_parks_at("1", "head-call");
}

#[test]
fn a_value_read_that_loses_the_claim_race_parks_on_the_vm() {
    racer_parks_at("1", "value-call");
}

#[test]
fn a_call_head_that_loses_the_claim_race_parks_with_the_jit() {
    racer_parks_at("2", "head-call");
}

#[test]
fn a_value_read_that_loses_the_claim_race_parks_with_the_jit() {
    racer_parks_at("2", "value-call");
}
