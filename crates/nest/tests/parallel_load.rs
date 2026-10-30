//! The parallel cold load (ADR-426, the default; `BROOD_LOAD_SEQUENTIAL=1` opts out): a
//! cold `nest run` of a project with many modules loads them in a pool of processes, each
//! `require-one`ing modules off a queue. It must build the SAME project as the sequential walk
//! — the opt-out, and the control arm here — so every case runs both and compares:
//!
//! - the program's answer, over modules that depend on each other through `(:use …)`, a
//!   macro whose expansion names another module, a registry another module reads, and a
//!   headerless file's root-level global;
//! - the record beside the image (owners, pins, registries — a parallel build once carried
//!   the require-waiters registry, pids and all, and the `dev` module's registries);
//! - what a WARM start materialises: the walk's own requires once became edges of the root
//!   section, which a warm start replays, loading the whole project (0.2 s → 1.3 s);
//! - a module that throws at load fails the build, naming its file, instead of hanging;
//! - a RING of modules aliasing each other builds — every worker that starts one meets the others
//!   as a require cycle split across processes — and a broken module under the ring fails
//!   the build instead of hanging it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MODULES: usize = 24;

fn fixture(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brood-parallel-load-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project\n  :name pl\n  :main main)\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/mac.blsp"),
        "(defmodule mac)\n(def here true)\n(defmacro twice (x) `(* 2 ~x m0/base))\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/regs.blsp"),
        "(defmodule regs)\n(def here true)\n(registry/put! :pl/hooks :a 1)\n(registry/put! :pl/hooks :b 2)\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/loose.blsp"), "(def loose-k 7)\n").unwrap();
    for i in 0..MODULES {
        // a chain: every module after the first `:use`s its predecessor, and every third one
        // also expands `mac`'s macro (whose expansion names `m0`)
        let header = if i == 0 {
            String::new()
        } else {
            format!(" (:use m{})", i - 1)
        };
        let body = if i == 0 {
            "(def base 3)\n(defn v () base)".to_string()
        } else if i % 3 == 0 {
            format!("(defn v () (+ (m{}/v) (mac/twice {i})))", i - 1)
        } else {
            format!("(defn v () (+ (m{}/v) {i}))", i - 1)
        };
        std::fs::write(
            dir.join(format!("src/m{i}.blsp")),
            format!("(defmodule m{i}{header})\n(def here true)\n{body}\n"),
        )
        .unwrap();
    }
    std::fs::write(
        dir.join("src/main.blsp"),
        format!(
            "(defmodule main)\n\
             (defn main ()\n\
               (io/puts (str \"ANSWER \" (m{}/v) \" \" loose-k \" \"\n\
                          (sort (keys (or (registry/entries :pl/hooks) {{}}))))))\n",
            MODULES - 1
        ),
    )
    .unwrap();
    dir
}

fn nest(dir: &Path, args: &[&str], sequential: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nest"));
    cmd.args(args)
        .current_dir(dir)
        .env("BROOD_NO_CRASH_REPORT", "1");
    if sequential {
        cmd.env("BROOD_LOAD_SEQUENTIAL", "1");
    } else {
        cmd.env_remove("BROOD_LOAD_SEQUENTIAL");
    }
    cmd.output().expect("spawn nest")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn answer(out: &Output) -> String {
    let all = text(out);
    assert!(out.status.success(), "nest failed:\n{all}");
    all.lines()
        .find(|l| l.starts_with("ANSWER "))
        .unwrap_or_else(|| panic!("no ANSWER:\n{all}"))
        .to_string()
}

/// Cold build, then a warm run that reports how many image sections it materialised.
fn build(dir: &Path, sequential: bool) -> (String, String, usize) {
    let _ = std::fs::remove_dir_all(dir.join(".brood"));
    let cold = answer(&nest(dir, &["run", "--no-check"], sequential));
    let record = std::fs::read_to_string(dir.join(".brood/image.modules")).expect("the record");
    let mut warm = Command::new(env!("CARGO_BIN_EXE_nest"));
    warm.args(["run", "--no-check"])
        .current_dir(dir)
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_IMAGE_TRACE", "1");
    let warm = warm.output().expect("spawn nest");
    assert_eq!(
        answer(&warm),
        cold,
        "the warm run answers like the cold one"
    );
    let sections = text(&warm)
        .lines()
        .filter(|l| l.starts_with("[image-section]"))
        .count();
    (cold, record, sections)
}

#[test]
fn a_parallel_cold_load_builds_what_the_sequential_one_does() {
    let dir = fixture("same");
    let (seq_answer, seq_record, seq_sections) = build(&dir, true);
    let (par_answer, par_record, par_sections) = build(&dir, false);
    assert_eq!(par_answer, seq_answer);
    assert!(par_answer.contains(" 7 (:a :b)"), "{par_answer}");
    assert_eq!(
        par_record, seq_record,
        "the record beside the image differs"
    );
    assert_eq!(
        par_sections, seq_sections,
        "a warm start materialised a different number of sections after a parallel build"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_module_that_throws_at_load_fails_the_parallel_build() {
    let dir = fixture("throws");
    std::fs::write(
        // the LAST module of the chain: nothing depends on it, so no waiter retries its load
        // (a failing load deep in a chain is slow on both paths — ROADMAP)
        dir.join(format!("src/m{}.blsp", MODULES - 1)),
        format!(
            "(defmodule m{} (:use m{}))\n(def here true)\n(error \"boom at load\")\n(defn v () 0)\n",
            MODULES - 1,
            MODULES - 2
        ),
    )
    .unwrap();
    let out = nest(&dir, &["run", "--no-check"], false);
    let all = text(&out);
    assert!(
        !out.status.success(),
        "a failing module must fail the build:\n{all}"
    );
    assert!(
        all.contains("boom at load"),
        "the error is reported:\n{all}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

const RING: usize = 24;

/// `RING` modules, each aliasing the next and the last the first, so a pool worker that starts
/// any of them meets the others' loads as one require cycle split across processes. `:alias`,
/// not `:use`: a refer-all `:use` of a module still loading is an error in its own right, met
/// whenever the walk reaches a module whose cycle partner is mid-load, so a `:use` ring is not
/// a program; an aliased ring is, and is how real projects close their cycles.
/// With `broken`, every module also `:use`s `bad`, which throws at load.
fn ring_fixture(tag: &str, broken: bool) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brood-parallel-ring-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("project.blsp"),
        "(project\n  :name pr\n  :main main)\n",
    )
    .unwrap();
    if broken {
        std::fs::write(
            dir.join("src/bad.blsp"),
            "(defmodule bad)\n(def here true)\n(error \"boom under the ring\")\n",
        )
        .unwrap();
    }
    for i in 0..RING {
        let uses_bad = if broken { " (:use bad)" } else { "" };
        std::fs::write(
            dir.join(format!("src/r{i}.blsp")),
            format!(
                "(defmodule r{i}{uses_bad} (:alias r{}))\n(def here true)\n(defn v{i} () {i})\n",
                (i + 1) % RING
            ),
        )
        .unwrap();
    }
    std::fs::write(
        dir.join("src/main.blsp"),
        format!(
            "(defmodule main)\n(defn main () (io/puts (str \"ANSWER \" (+ (r0/v0) (r{0}/v{0})))))\n",
            RING - 1
        ),
    )
    .unwrap();
    dir
}

/// `nest` under a deadline: a hang is the failure these cases guard, so it must fail the case
/// with what the child printed rather than stall the run until nextest's per-case kill.
fn nest_within(dir: &Path, args: &[&str], sequential: bool, seconds: u64) -> Output {
    let out_path = dir.join("nest.out");
    let err_path = dir.join("nest.err");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nest"));
    cmd.args(args)
        .current_dir(dir)
        .env("BROOD_NO_CRASH_REPORT", "1")
        .stdout(std::fs::File::create(&out_path).unwrap())
        .stderr(std::fs::File::create(&err_path).unwrap());
    if sequential {
        cmd.env("BROOD_LOAD_SEQUENTIAL", "1");
    } else {
        cmd.env_remove("BROOD_LOAD_SEQUENTIAL");
    }
    let mut child = cmd.spawn().expect("spawn nest");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait on nest") {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "nest {args:?} did not finish in {seconds} s:\n{}{}",
                std::fs::read_to_string(&out_path).unwrap_or_default(),
                std::fs::read_to_string(&err_path).unwrap_or_default()
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Output {
        status,
        stdout: std::fs::read(&out_path).unwrap_or_default(),
        stderr: std::fs::read(&err_path).unwrap_or_default(),
    }
}

/// Every pool worker on the ring used to wait on the next one's load. One process broke the
/// cycle a quarter-second round at a time, its claims passing to the process that had waited
/// on them, which closed the cycle again one member fewer; the waiters far from the break
/// reached the 30 s backstop and force-loaded mid-cycle, and the build never ended (bedit's
/// cold build hung the same way). The pool's workers now give a cycle up — every one on it —
/// and the sequential pass after the pool loads what they left.
#[test]
fn a_ring_of_modules_builds_in_parallel() {
    let dir = ring_fixture("builds", false);
    let sequential = answer(&nest_within(&dir, &["run", "--no-check"], true, 60));
    let _ = std::fs::remove_dir_all(dir.join(".brood"));
    let parallel = answer(&nest_within(&dir, &["run", "--no-check"], false, 60));
    assert_eq!(parallel, sequential);
    assert_eq!(parallel, format!("ANSWER {}", RING - 1));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A broken module every module of the ring uses: the failure and the cycle together. The
/// workers still loading when the first failure came in each loaded it again, failed,
/// discarded their claims and met the others' half-done loads as cycles to break and retry,
/// and the build hung. Either half of the fix ends it alone — the pool killed at the first
/// real failure, or its workers giving cycles up — so this case is red only with neither;
/// each half has its own case above and below.
#[test]
fn a_broken_module_under_a_ring_fails_the_parallel_build() {
    let dir = ring_fixture("broken", true);
    let out = nest_within(&dir, &["run", "--no-check"], false, 60);
    let all = text(&out);
    assert!(
        !out.status.success(),
        "a failing module must fail the build:\n{all}"
    );
    assert!(
        all.contains("boom under the ring"),
        "the error is reported:\n{all}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A failure does not wait out the loads still in flight. `slow` sleeps through its first load
/// (it leaves a marker, so a later load is quick) and `bad` throws two seconds in, by when
/// every job has been handed out and `slow` is mid-load in a worker — given a pool of two
/// workers or more. The driver used to stop handing out jobs and then wait for every worker to
/// finish its own, so the build reported the failure only once `slow` woke. It now kills the
/// pool at the first real failure, and the sequential pass raises it.
#[test]
fn a_failure_does_not_wait_for_the_loads_still_in_flight() {
    let dir = fixture("in-flight");
    let marker = dir.join("slow.marker");
    std::fs::write(
        dir.join("src/slow.blsp"),
        format!(
            "(defmodule slow)\n(def here true)\n\
             (when (not (file/exists? {marker:?})) (file/spit {marker:?} \"\") (proc/sleep 30000))\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("src/bad.blsp"),
        "(defmodule bad)\n(def here true)\n(proc/sleep 2000)\n(error \"boom after a while\")\n",
    )
    .unwrap();
    let out = nest_within(&dir, &["run", "--no-check"], false, 20);
    let all = text(&out);
    assert!(
        !out.status.success(),
        "a failing module must fail the build:\n{all}"
    );
    assert!(
        all.contains("boom after a while"),
        "the error is reported:\n{all}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
