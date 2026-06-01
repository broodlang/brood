//! End-to-end serve/attach test (ADR-090): a daemon `brood` runtime serves a tiny
//! `ui-run` app (`node-start` + `editor/serve/serve`), and a second runtime attaches
//! over the (encrypted) node link, drives the app with keys, and watches the pushed
//! frames change. Exercises the protocol path `nest attach` rides on, without a TTY
//! — the client speaks the serve protocol (`[:attach …]` / `[:frame …]` / `[:key …]`
//! / `[:bye]`) directly.
//!
//! Mirrors the harness in `observe_attach.rs` / `distribution.rs` (two real OS
//! processes over loopback).

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
            panic!("daemon never started listening on port {port}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A daemon serves a counter app; a client attaches, reads the initial frame
/// (n=0), presses "+", reads the key-driven frame (n=1), then quits ("q") and
/// receives the daemon's `[:bye]`. Proves serve → remote-display → session works
/// end to end over a real node link with the app running on the daemon.
#[test]
fn attach_drives_a_served_app_over_the_link() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-serve-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Daemon: serve a tiny counter app ("+" increments, "q" quits), then park.
    let daemon = format!(
        r#"
(node-start :ed "127.0.0.1:{port_a}" "secret")
(require 'editor/serve)
(require 'editor/display)
(defn mk () {{:n 0}})
(defn vw (m c r) [(editor/display/text 0 0 (str "n=" (get m :n)))])
(defn up (m input c r)
  (cond
    (= input "+") (assoc m :n (+ (get m :n) 1))
    (= input "q") (assoc m :done true)
    :else m))
(editor/serve/serve mk vw up)
(defn park () (receive (_ (park))))
(park)
"#
    );

    // Client: connect, then speak the serve protocol directly (no terminal).
    // Retry the attach so a not-yet-registered manager (serve runs just after the
    // port binds) doesn't lose the first request.
    let client = format!(
        r#"
(node-start :cli "127.0.0.1:{port_b}" "secret")
(require 'editor/serve)
(def peer (connect "ed@127.0.0.1:{port_a}"))
(monitor-node peer)
(defn frame-text (f) (nth (first f) 3))
(defn try-attach (n)
  (if (<= n 0)
    :no-attach
    (do
      (send {{:name editor/serve/serve-name :node peer}} [:attach (self) 80 24])
      (receive ([:attached s] s) (after 500 (do (sleep 200) (try-attach (- n 1))))))))
(def session (try-attach 20))
(def f0 (receive ([:frame f] (frame-text f)) (after 5000 :no-frame)))
(send session [:key "+"])
(def f1 (receive ([:frame f] (frame-text f)) (after 5000 :no-frame)))
(send session [:key "q"])
(def bye (receive ([:bye] :bye) (after 5000 :no-bye)))
(println (str "SERVE f0=" f0 " f1=" f1 " bye=" bye))
"#
    );

    let mut a = spawn_brood(&dir, "daemon.blsp", &daemon);
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "client.blsp", &client);

    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = a.wait();
    let mut a_err = String::new();
    if let Some(mut e) = a.stderr.take() {
        let _ = e.read_to_string(&mut a_err);
    }
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("SERVE f0=n=0 f1=n=1 bye=:bye"),
        "serve/attach failed.\n--- client stdout ---\n{stdout}\n--- client stderr ---\n{stderr}\n--- daemon stderr ---\n{a_err}"
    );
}
