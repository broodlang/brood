//! `nest test --watch`: one process that reruns the tests on every save (2026-11-13).
//!
//! A one-file `nest test` spends most of its time before the first test runs: the boot, the
//! tool modules and the project's setup (hatch: ~75 of 100 ms). The watcher pays them once.
//! What has to hold for it to be worth anything, each asserted below against a live session:
//!
//! - an edit to a source file is SEEN: its definitions are re-evaluated and the run after it
//!   reads the new code (red after a breaking edit, green after the fix);
//! - only the test files the change reaches rerun (`other_test` does not);
//! - a changed MACRO reaches the modules that expanded it — late binding covers a changed
//!   function, not an expansion already compiled into a caller;
//! - a save that does not load is reported, and the next good save runs again: the watch
//!   survives the broken state every edit passes through.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod support;
use support::temp_dir;

/// Everything the session printed, stdout and stderr, in arrival order.
struct Session {
    child: Child,
    out: Arc<Mutex<String>>,
}

impl Session {
    fn start(dir: &Path) -> Session {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nest"))
            .args(["test", "--watch"])
            .current_dir(dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn nest test --watch");
        let out = Arc::new(Mutex::new(String::new()));
        fn pump(stream: impl Read + Send + 'static, out: Arc<Mutex<String>>) {
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let mut text = out.lock().unwrap();
                    text.push_str(&line);
                    text.push('\n');
                }
            });
        }
        pump(child.stdout.take().unwrap(), out.clone());
        pump(child.stderr.take().unwrap(), out.clone());
        Session { child, out }
    }

    fn len(&self) -> usize {
        self.out.lock().unwrap().len()
    }

    /// Wait until the output after byte `from` holds `needle` (and the banner that ends a
    /// cycle after it), returning that part; panic with the whole transcript on timeout.
    fn wait_for(&self, from: usize, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            {
                let text = self.out.lock().unwrap();
                let tail = &text[from..];
                if let Some(at) = tail.find(needle)
                    && tail[at..].contains("save to rerun")
                {
                    return tail.to_string();
                }
            }
            assert!(
                Instant::now() < deadline,
                "no `{needle}` within 60 s; the session printed:\n{}",
                self.out.lock().unwrap()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write(dir: &Path, rel: &str, src: &str) {
    std::fs::write(dir.join(rel), src).unwrap();
}

#[test]
fn a_watch_reruns_what_a_save_reaches_and_survives_a_broken_save() {
    let tmp = temp_dir("test-watch");
    let dir = &tmp.path;
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    write(dir, "project.blsp", "(project :name watchdemo)\n");
    write(
        dir,
        "src/calc.blsp",
        "(defmodule calc)\n(defn add (a b) (+ a b))\n",
    );
    write(
        dir,
        "src/other.blsp",
        "(defmodule other)\n(defn one () 1)\n",
    );
    write(
        dir,
        "src/mac.blsp",
        "(defmodule mac)\n(defmacro twice (x) `(* 2 ~x))\n",
    );
    write(
        dir,
        "src/usemac.blsp",
        "(defmodule usemac (:use mac))\n(defn eight () (twice 4))\n",
    );
    write(
        dir,
        "tests/calc_test.blsp",
        "(defmodule calc-test (:use test) (:use calc))\n\
         (describe \"calc\" (test \"adds\" (assert= 5 (add 2 3))))\n",
    );
    write(
        dir,
        "tests/other_test.blsp",
        "(defmodule other-test (:use test) (:use other))\n\
         (describe \"other\" (test \"one\" (assert= 1 (one))) (test \"again\" (assert= 1 (one))))\n",
    );
    write(
        dir,
        "tests/usemac_test.blsp",
        "(defmodule usemac-test (:use test) (:use usemac))\n\
         (describe \"usemac\" (test \"eight\" (assert= 8 (eight))))\n",
    );

    let session = Session::start(dir);
    let first = session.wait_for(0, "4 tests, 4 passed");
    assert!(first.contains("watching"), "{first}");

    // A breaking edit: the run after it reads the new `add`, and reruns calc_test ALONE —
    // other_test's two tests would make it "3 tests".
    let at = session.len();
    write(
        dir,
        "src/calc.blsp",
        "(defmodule calc)\n(defn add (a b) (- a b))\n",
    );
    let broke = session.wait_for(at, "1 tests, 0 passed, 1 failed");
    assert!(broke.contains("calc › adds"), "{broke}");

    let at = session.len();
    write(
        dir,
        "src/calc.blsp",
        "(defmodule calc)\n(defn add (a b) (+ a b))\n",
    );
    session.wait_for(at, "1 tests, 1 passed");

    // A changed macro reaches its caller: `eight` expands `twice`, and reads 12 only if
    // `usemac` was re-evaluated after `mac`.
    let at = session.len();
    write(
        dir,
        "src/mac.blsp",
        "(defmodule mac)\n(defmacro twice (x) `(* 3 ~x))\n",
    );
    let mac = session.wait_for(at, "1 failed");
    assert!(
        mac.contains("usemac › eight") && mac.contains("12"),
        "the macro's caller kept the old expansion:\n{mac}"
    );

    // A save that does not load is reported and runs nothing; the next good save runs again.
    let at = session.len();
    write(
        dir,
        "src/calc.blsp",
        "(defmodule calc)\n(defn add (a b) (+ a b)\n",
    );
    let bad = session.wait_for(at, "does not load");
    assert!(!bad.contains("tests, "), "a broken save ran tests:\n{bad}");
    let at = session.len();
    write(
        dir,
        "src/calc.blsp",
        "(defmodule calc)\n(defn add (a b) (+ a b))\n",
    );
    session.wait_for(at, "1 tests, 1 passed");
}
