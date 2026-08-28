//! End-to-end remote-attach test (ADR-053): a target `brood` runtime makes itself
//! observable (`node/start` + `observe-serve`), and a second runtime attaches over
//! the node link, requests a process snapshot, and — when the target dies — sees
//! the link drop. Exercises the data/protocol path the `nest observe --connect`
//! TUI rides on, without a terminal.
//!
//! Mirrors the harness in `distribution.rs` (two real OS processes over loopback).

use std::io::Read;
use std::time::Duration;

mod support;
use support::*;

/// Attach to a running target, read a snapshot (proving it sees the target's own
/// processes and that the snapshot's node panel is the *peer's*), then have the
/// harness kill the target and confirm the observer's request reports `:down`.
#[test]
fn remote_attach_reads_snapshot_then_sees_disconnect() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-observe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Target: become observable, spawn a couple of identifiable workers, park.
    let target = format!(
        r#"
(node/start :app "127.0.0.1:{port_a}" "secret-test-cookie-16+")
(observer/observe-serve)
(spawn (receive (_ :done)))
(spawn (receive ([:work _] :done)))
(defn park () (receive (_ (park))))
(park)
"#
    );

    // Observer: attach, request a snapshot (the agent ships process-info maps over
    // the link), then poll until the link drops (the harness kills the target).
    let observer = format!(
        r#"
(node/start :obs "127.0.0.1:{port_b}" "secret-test-cookie-16+")
(def peer (node/connect "app@127.0.0.1:{port_a}"))
(node/monitor peer)
(def snap (observer/observe-request peer))
(if (map? snap)
  (io/puts (str "ATTACH-OK node=" (->string (get (get snap :node) :name))
                " procs=" (count (get snap :procs))))
  (io/puts (str "ATTACH-FAIL " snap)))
(defn poll-down (n)
  (if (<= n 0)
    (io/puts "NO-DOWN")
    (let (r (observer/observe-request peer))
      (if (= r :down)
        (io/puts "DOWN-OK")
        (do (sleep 300) (poll-down (dec n)))))))
(poll-down 40)
"#
    );

    let mut a = spawn_brood(&dir, "target.blsp", &target);
    wait_until_listening(port_a);
    let mut b = spawn_brood(&dir, "observer.blsp", &observer);

    // Kill the target only once the observer has REPORTED its attach — never after a
    // fixed sleep. KI-43: this was `sleep(5000)`, and no constant can be right here. The
    // observer has to boot a debug `brood`, `node/start`, `require 'observer'` (a large
    // module) and connect before the kill lands; under peak suite load — a 600 s
    // in-language suite plus the scaffold tests on the same box — that overran 5 s, the
    // target died first, and the observer's `(node/connect …)` failed with `Connection refused`
    // and an EMPTY stdout. Two earlier bumps (1500 → 5000 ms) were the same fix applied to
    // the same wrong idea. Reading the marker makes the wait proportional to the machine
    // instead of to a guess, and it strengthens the test: "attached BEFORE the kill" is
    // what the case actually means, and it is now asserted rather than assumed.
    //
    // The deadline below is a backstop against a hang, not a timing assumption — the
    // common path returns as soon as the line arrives.
    let stdout_pipe = b.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    let pump = std::thread::spawn(move || {
        let mut acc = String::new();
        let mut buf = [0u8; 4096];
        let mut pipe = stdout_pipe;
        let mut announced = false;
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                    // Either outcome unblocks the test: ATTACH-FAIL is a real failure we
                    // want reported by the assert below, not by a 60 s timeout.
                    if !announced && (acc.contains("ATTACH-OK") || acc.contains("ATTACH-FAIL")) {
                        announced = true;
                        let _ = tx.send(());
                    }
                }
            }
        }
        acc
    });
    let attached = rx.recv_timeout(Duration::from_secs(60)).is_ok();

    let _ = a.kill();
    let _ = a.wait();

    let status = b.wait().expect("observer finished");
    let stdout = pump.join().expect("stdout pump");
    let mut stderr = String::new();
    if let Some(mut e) = b.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    let mut a_err = String::new();
    if let Some(mut e) = a.stderr.take() {
        let _ = e.read_to_string(&mut a_err);
    }
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        status.success()
            && attached
            && stdout.contains("ATTACH-OK node=app")
            && stdout.contains("DOWN-OK"),
        "observer failed (attach marker seen: {attached}).\n--- observer stdout ---\n{stdout}\n--- observer stderr ---\n{stderr}\n--- target stderr ---\n{a_err}"
    );
}
