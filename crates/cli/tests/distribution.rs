//! End-to-end distributed-node test: two real `brood` runtimes (separate OS
//! processes) connect over loopback TCP and message each other.
//!
//! Proves the full slice-1 path: node naming + cookie handshake (`node-start` /
//! `connect`), bootstrapping a peer by registered name (`{:name :node}`), and —
//! the payoff — addressing the **remote pid** the peer replies with directly,
//! location-transparently. Symbols/pids cross the wire by name and re-intern on
//! the far side (separate interners).

use std::io::Read;
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Grab a currently-free localhost port by binding to :0 and releasing it. A
/// tiny race window before the child re-binds, acceptable for a test.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Run a `.blsp` program in a fresh `brood` subprocess.
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

/// Wait until `port` accepts a TCP connection (the peer's listener is up), or
/// panic after ~5s.
fn wait_until_listening(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("server never started listening on port {port}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn two_nodes_connect_and_message() {
    let dir = std::env::temp_dir().join(format!("brood-dist-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Node A: register an `:echo` server that replies `[:pong (self)]` to whoever
    // sends it `[:hi from]` or `[:ping from]`. Loops forever (the harness kills it).
    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(register :echo (self))
(defn serve ()
  (receive
    ([:hi from]   (do (send from [:pong (self)]) (serve)))
    ([:ping from] (do (send from [:pong (self)]) (serve)))
    (_ (serve))))
(serve)
"#
    );

    // Node B: connect to A, reach `:echo` by registered name, then address the
    // remote pid it replies with *directly* (location transparency).
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(send {{:name :echo :node :a}} [:hi (self)])
(def remote (receive ([:pong p] p) (after 5000 (throw "no reply by name"))))
(unless (pid? remote) (throw "reply was not a pid"))
(send remote [:ping (self)])
(receive ([:pong _] (println "ROUNDTRIP-OK")) (after 5000 (throw "no reply by pid")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "client.blsp", &client);

    let out = b.wait_with_output().expect("client finished");
    // Tear the server down regardless of the assertion outcome.
    let _ = a.kill();
    let mut a_err = String::new();
    if let Some(mut e) = a.stderr.take() {
        let _ = e.read_to_string(&mut a_err);
    }
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("ROUNDTRIP-OK"),
        "client failed.\n--- client stdout ---\n{stdout}\n--- client stderr ---\n{stderr}\n--- server stderr ---\n{a_err}"
    );
}

/// A bad cookie must be rejected: B cannot reach A's `:echo`, so the by-name
/// send is silently dropped and B times out (Erlang semantics — no delivery, no
/// error at the sender).
#[test]
fn mismatched_cookie_is_rejected() {
    let dir = std::env::temp_dir().join(format!("brood-dist-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "right-cookie")
(register :echo (self))
(defn serve () (receive ([:hi from] (do (send from [:pong (self)]) (serve))) (_ (serve))))
(serve)
"#
    );
    // Wrong cookie → the handshake fails, so `connect` errors and no link forms.
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "wrong-cookie")
(println (try (do (connect "a@127.0.0.1:{port_a}") "UNEXPECTED-CONNECTED")
              (catch e "REJECTED-AS-EXPECTED")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
    wait_until_listening(port_a);
    // Best-effort connect from B; it may error out on the bad cookie — that's fine,
    // we still then attempt the (dropped) by-name send below in the same program.
    let b = spawn_brood(&dir, "client.blsp", &client);

    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("REJECTED-AS-EXPECTED") && !stdout.contains("UNEXPECTED-CONNECTED"),
        "expected the bad-cookie handshake to be rejected.\n--- stdout ---\n{stdout}"
    );
}

/// An `:echo` server that replies `[:welcome]` to `[:hi from]`, and exits cleanly
/// on `[:bye from]` (its main process returns → the OS process exits → the link's
/// socket closes). Shared by the de-dup and node-down tests.
fn echo_server_src(port: u16) -> String {
    format!(
        r#"
(node-start :a "127.0.0.1:{port}" "secret")
(register :echo (self))
(defn serve ()
  (receive
    ([:hi from]  (do (send from [:welcome]) (serve)))
    ([:bye _]    :exiting)               ; return → the runtime exits
    (_ (serve))))
(serve)
"#
    )
}

/// Connecting to the same peer twice yields **one** link, not two — the second
/// `connect` reuses the existing one. Messaging still works.
#[test]
fn duplicate_connect_is_deduplicated() {
    let dir = std::env::temp_dir().join(format!("brood-dist-dup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_a = free_port();
    let port_b = free_port();

    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(connect "a@127.0.0.1:{port_a}")          ; second connect — should reuse, not add
(send {{:name :echo :node :a}} [:hi (self)])
(receive ([:welcome] :ok) (after 5000 (throw "no welcome")))
(println (str "NODES=" (nodes)))           ; expect exactly (:a)
(send {{:name :echo :node :a}} [:bye (self)])
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &echo_server_src(port_a));
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "client.blsp", &client);
    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("NODES=(:a)"),
        "expected a single deduplicated link.\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `connect` to our own node name is refused up-front (no self-dial loop).
#[test]
fn connect_to_self_refused() {
    let dir = std::env::temp_dir().join(format!("brood-dist-self-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_a = free_port();

    let src = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(println (try (do (connect "a@127.0.0.1:{port_a}") "UNEXPECTED-CONNECTED")
              (catch e "REFUSED-AS-EXPECTED")))
"#
    );
    let p = spawn_brood(&dir, "self.blsp", &src);
    let out = p.wait_with_output().expect("finished");
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("REFUSED-AS-EXPECTED") && !stdout.contains("UNEXPECTED-CONNECTED"),
        "expected self-connect to be refused.\n--- stdout ---\n{stdout}"
    );
}

/// `(monitor-node :ghost)` for a node we've never linked to fires `[:nodedown]`
/// immediately (Erlang `monitor_node` semantics).
#[test]
fn monitor_unconnected_node_fires_immediately() {
    let dir = std::env::temp_dir().join(format!("brood-dist-ghost-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_b = free_port();

    let src = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(monitor-node :ghost)
(receive ([:nodedown :ghost] (println "IMMEDIATE-NODEDOWN"))
         (after 1000 (throw "monitor-node did not fire immediately")))
"#
    );
    let p = spawn_brood(&dir, "ghost.blsp", &src);
    let out = p.wait_with_output().expect("finished");
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("IMMEDIATE-NODEDOWN"),
        "expected an immediate [:nodedown] for an unconnected node.\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `(monitor-node :a)` delivers `[:nodedown :a]` when the link to `:a` drops. The
/// client establishes the link (proven by a `:welcome` round-trip, after which the
/// monitor is registered), asks `:a` to exit, and must then receive the nodedown.
#[test]
fn node_down_is_detected() {
    let dir = std::env::temp_dir().join(format!("brood-dist-down-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_a = free_port();
    let port_b = free_port();

    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(monitor-node :a)
(send {{:name :echo :node :a}} [:hi (self)])
(receive ([:welcome] :ok) (after 5000 (throw "no welcome")))   ; link + monitor are up
(send {{:name :echo :node :a}} [:bye (self)])                  ; make :a exit
(receive ([:nodedown :a] (println "NODEDOWN-OK"))
         (after 10000 (throw "no nodedown")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &echo_server_src(port_a));
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "client.blsp", &client);
    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("NODEDOWN-OK"),
        "expected a [:nodedown :a] after the peer exited.\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
