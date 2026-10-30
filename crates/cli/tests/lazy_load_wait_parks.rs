//! **A first-use load another process is running makes its waiters park, not block.**
//!
//! A qualified call into an unloaded module loads it at the global-lookup miss (ADR-335).
//! When another process already holds that load, the miss waits for it in
//! `%require-await`'s `receive`, and from the miss path that `receive` sat under a native
//! frame: it could not park, so it blocked its OS worker. 200 processes making their first
//! call into one module together dirty-blocked up to 27 workers a run (2026-10-20); with the
//! load held in flight, 596-804 waits a run. The VM now sends that miss through
//! `%lazy-global` as an ordinary frame, where the wait parks.
//!
//! The LOADER waits too: for a dependency its header names that another process is mid-load
//! of. That wait sat under two native frames — `%with-load-journal` calling the load, and the
//! native load of the module's source — and blocked 139 workers for 40 loaders whose header
//! `(:use http)` while `http` was held (2026-10-21). The journal is now a Brood `try` around a
//! native open/close pair, and the header's modules load before the native load starts.
//! Counted by the runtime's `dirty-receive-block` line under `BROOD_JIT_BAIL_TRACE`.

use std::path::Path;
use std::process::Command;

/// A holder takes the load claim for `http` and keeps it 300 ms, so every child's first use
/// finds the load in flight in ANOTHER process — the wait this is about, every run. (Left to
/// the scheduler, the race reproduced 0 to 27 times a run.)
const HOLD_HTTP: &str = r#"
(def me (self))
(def holder
  (spawn
    (do (%registry-update! '*features-loading* :assoc-new ["http"] (self))
      (send me :claimed)
      (proc/sleep 300)
      (%registry-update! '*features-loading* :dissoc ["http"] nil)
      (%require-release! "http" nil))))
(receive (:claimed nil))
(defn collect (left acc) (if (= left 0) acc (collect (dec left) (+ acc (receive ([:r v] v))))))
"#;

/// `http/parse-url` is the FIRST use of `http`: the checker is off (it would load the module
/// eagerly), so each child loads it at its own lookup miss.
const WAITERS: &str = r#"
(defn child () (send me [:r (if (get (http/parse-url "http://a/b") :host) 1 0)]))
(def kids (map (range 0 200) (fn (i) (spawn (child)))))
(io/puts (str "sum=" (collect 200 0)))
"#;

/// Each child LOADS a module of its own, `m<i>`, whose header uses `http`.
const LOADERS: &str = r#"
(defn child (i)
  (let (m (str "m" i))
    (require-one m)
    (send me [:r (if ((reflect/eval (symbol (str m "/host"))) "http://a/b") 1 0)])))
(def kids (map (range 0 40) (fn (i) (spawn (child i)))))
(io/puts (str "sum=" (collect 40 0)))
"#;

/// Run `program` in `dir` and return how many waits blocked an OS worker — or `None` when no
/// stdlib image was installed. The case is a module MATERIALISED from the image; loaded from
/// source instead (`BROOD_NO_STDIMAGE`, or no image built for this binary), the waiters' race
/// never showed — the unfixed runtime read 2 there and 596-804 from the image — so the caller
/// says so rather than pass on a path that cannot fail.
fn dirty_blocks(dir: &Path, program: &str, expect: &str) -> Option<usize> {
    let path = dir.join("main.blsp");
    std::fs::write(&path, program).expect("write program");
    let out = Command::new(env!("CARGO_BIN_EXE_brood"))
        .current_dir(dir)
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .env("BROOD_JIT_BAIL_TRACE", "1")
        .env("BROOD_IMAGE_TRACE", "1")
        // The tree-walker job runs everything on tier 0, where this path is not the VM's.
        .env_remove("BROOD_TIER")
        .env_remove("BROOD_NO_LAZY_LOAD")
        .env_remove("BROOD_NO_STDIMAGE")
        .arg(&path)
        .output()
        .expect("run brood");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stdout.contains(expect),
        "every child should have parsed its url:\n{stdout}\n{stderr}"
    );
    if !stderr
        .lines()
        .any(|l| l.starts_with("[image] install:") && !l.contains("nil sections"))
    {
        return None;
    }
    let dirty = stderr
        .lines()
        .filter(|l| l.contains("dirty-receive-block"))
        .count();
    if dirty > 0 {
        eprintln!("{stderr}");
    }
    Some(dirty)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("brood-lazy-wait-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn processes_racing_a_first_use_load_park_while_it_finishes() {
    let dir = scratch("waiters");
    let dirty = dirty_blocks(&dir, &format!("{HOLD_HTTP}{WAITERS}"), "sum=200");
    let _ = std::fs::remove_dir_all(&dir);
    let Some(dirty) = dirty else {
        eprintln!("lazy_load_wait_parks: no stdlib image installed — the race needs one; vacuous");
        return;
    };
    assert_eq!(
        dirty, 0,
        "{dirty} processes blocked their OS worker waiting for another process's load of \
         `http` (596-804 a run before the miss path handed the wait to an ordinary frame)"
    );
}

#[test]
fn a_loader_waiting_on_its_headers_dependency_parks() {
    let dir = scratch("loaders");
    for i in 0..40 {
        std::fs::write(
            dir.join(format!("m{i}.blsp")),
            format!(
                "(defmodule m{i} \"Uses http.\" (:use http))\n\
                 (defn host (u) (get (parse-url u) :host))\n"
            ),
        )
        .expect("write module");
    }
    let dirty = dirty_blocks(&dir, &format!("{HOLD_HTTP}{LOADERS}"), "sum=40");
    let _ = std::fs::remove_dir_all(&dir);
    let Some(dirty) = dirty else {
        eprintln!("lazy_load_wait_parks: no stdlib image installed — the race needs one; vacuous");
        return;
    };
    assert_eq!(
        dirty, 0,
        "{dirty} loaders blocked their OS worker waiting on `http`, which their header uses and \
         another process held (139 for 40 loaders before the load's frames were Brood's)"
    );
}
