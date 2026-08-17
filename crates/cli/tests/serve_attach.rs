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

mod support;
use support::*;

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
(node-start :ed "127.0.0.1:{port_a}" "secret-test-cookie-16+")
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
(node-start :cli "127.0.0.1:{port_b}" "secret-test-cookie-16+")
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
