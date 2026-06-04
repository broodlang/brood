//! End-to-end remote-attach test (ADR-053): a target `brood` runtime makes itself
//! observable (`node-start` + `observe-serve`), and a second runtime attaches over
//! the node link, requests a process snapshot, and — when the target dies — sees
//! the link drop. Exercises the data/protocol path the `nest observe --connect`
//! TUI rides on, without a terminal.
//!
//! Mirrors the harness in `distribution.rs` (two real OS processes over loopback).

use std::io::Read;
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Serialise the bind→spawn window so parallel tests don't grab the same port.
static PORTS: Mutex<()> = Mutex::new(());
fn port_lock() -> MutexGuard<'static, ()> {
    PORTS.lock().unwrap_or_else(|p| p.into_inner())
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn spawn_brood(dir: &std::path::Path, name: &str, src: &str) -> Child {
    let path = dir.join(name);
    std::fs::write(&path, src).unwrap();
    Command::new(env!("CARGO_BIN_EXE_brood"))
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn brood")
}

fn wait_until_listening(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("target never started listening on port {port}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

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
(node-start :app "127.0.0.1:{port_a}" "secret-test-cookie-16+")
(require 'observer)
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
(node-start :obs "127.0.0.1:{port_b}" "secret-test-cookie-16+")
(require 'observer)
(def peer (connect "app@127.0.0.1:{port_a}"))
(monitor-node peer)
(def snap (observer/observe--request peer))
(if (map? snap)
  (println (str "ATTACH-OK node=" (name (get (get snap :node) :name))
                " procs=" (count (get snap :procs))))
  (println (str "ATTACH-FAIL " snap)))
(defn poll-down (n)
  (if (<= n 0)
    (println "NO-DOWN")
    (let (r (observer/observe--request peer))
      (if (= r :down)
        (println "DOWN-OK")
        (do (sleep 300) (poll-down (dec n)))))))
(poll-down 40)
"#
    );

    let mut a = spawn_brood(&dir, "target.blsp", &target);
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "observer.blsp", &observer);

    // Let the observer attach + take its first snapshot, then drop the target so it
    // observes the disconnect. Give it extra room on a loaded system (observer loads
    // the full observer module before connecting; under a busy test suite that can
    // take noticeably longer than the default 1500 ms).
    std::thread::sleep(Duration::from_millis(5000));
    let _ = a.kill();
    let _ = a.wait();

    let out = b.wait_with_output().expect("observer finished");
    let mut a_err = String::new();
    if let Some(mut e) = a.stderr.take() {
        let _ = e.read_to_string(&mut a_err);
    }
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success()
            && stdout.contains("ATTACH-OK node=app")
            && stdout.contains("DOWN-OK"),
        "observer failed.\n--- observer stdout ---\n{stdout}\n--- observer stderr ---\n{stderr}\n--- target stderr ---\n{a_err}"
    );
}
