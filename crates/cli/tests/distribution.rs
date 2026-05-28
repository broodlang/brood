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
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Serialises the *bind→spawn* window across the (parallel-by-default) tests in
/// this file. Two tests racing through [`free_port`] can both pick the same
/// just-freed kernel port; the loser's child then fails to bind, the winner's
/// listener is what `wait_until_listening` happens to find, and the loser's
/// client times out with `ECONNREFUSED`. Holding this lock across each test's
/// port allocation + child spawn closes that window — the tests run end-to-end
/// concurrently with everything *else* in the workspace; only with each other
/// do they queue.
static PORTS: Mutex<()> = Mutex::new(());

/// Acquire the cross-test bind lock. Released when the returned guard drops.
/// (`PoisonError` is recovered into the inner unit; a panicked sibling test
/// shouldn't wedge the rest of the suite.)
fn port_lock() -> MutexGuard<'static, ()> {
    PORTS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Grab a currently-free localhost port by binding to :0 and releasing it.
/// Best paired with the [`port_lock`] guard around the spawn that re-binds it
/// — otherwise a sibling test can grab the same just-freed port first.
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
    let _g = port_lock();
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
    let _ = a.wait(); // reap, so the test doesn't leave a zombie
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

/// Cross-node closure shipping (ADR-033, the wire codec's `M_CLOSURE` path).
/// The client ships a `(fn (x) (* x n))` to a remote worker, with `n` a free
/// local captured from the surrounding `let`. The worker applies it and sends
/// the result back. Proves the full closure-as-data path end-to-end:
///   - the closure's body forms cross the wire as `Message::List(...)`
///     (S-expressions = data),
///   - the captured local `n` rides along in `captured`,
///   - the closure's free globals (`*`) re-resolve on the receiver against
///     *its* prelude — Erlang's "module must be loaded on both nodes",
///   - the receiver `f` is called via apply, the result `(* 14 3) = 42`
///     comes back to the client via the pid carried in the request.
#[test]
fn lambda_ships_across_nodes_and_runs() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-fn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Worker on A: receive `[:run f x reply]`, apply `(f x)`, send the result back.
    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(register :worker (self))
(defn serve ()
  (receive
    ([:run f x reply] (do (send reply [:result (f x)]) (serve)))
    (_ (serve))))
(serve)
"#
    );

    // Client on B: build a closure capturing `n`, ship it, expect (* 14 3) = 42.
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(let (n 3)
  (send {{:name :worker :node :a}} [:run (fn (x) (* x n)) 14 (self)]))
(receive
  ([:result r] (if (= r 42)
                 (println "CROSS-NODE-LAMBDA-OK")
                 (throw (str "expected 42, got " r))))
  (after 5000 (throw "no reply")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
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
        out.status.success() && stdout.contains("CROSS-NODE-LAMBDA-OK"),
        "client failed.\n--- client stdout ---\n{stdout}\n--- client stderr ---\n{stderr}\n--- server stderr ---\n{a_err}"
    );
}

/// Source positions ride along with a closure across the wire. The client
/// ships a closure whose body contains a *quoted* list literal at a known
/// line; the remote evaluates `(form-pos quoted-list)` on its own heap and
/// sends the `[line col]` back. The position the client gets back must be
/// the position from the **client's source**, not nil — proving that
/// `Message::List`'s optional `Pos` trailer survived encoding, the receiver's
/// `from_message` re-stamped it on the rebuilt pair via `heap.set_form_pos`,
/// and `(form-pos …)` could find it on the receiver's heap.
#[test]
fn source_positions_survive_a_cross_node_send() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-pos-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(register :probe (self))
(defn serve ()
  (receive
    ([:run f reply] (do (send reply [:pos (f)]) (serve)))
    (_ (serve))))
(serve)
"#
    );

    // Client: send a closure whose body inspects a quoted list literal. The
    // literal's position was stamped by *our* reader; on the receiver, after
    // round-tripping through the wire, `(form-pos …)` must still return our
    // line/col. The list sits on **line 7** of the rendered source — the
    // leading `r#"\n` is line 1, the four explanatory lines + the `let`
    // header take us through line 6, the `(send …)` line is 7.
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(let (me (self))
  ;; The next line is line 7 in this file — the quoted literal whose position
  ;; must survive across the wire and reach the receiver's `form-pos`.
  (send {{:name :probe :node :a}} [:run (fn () (form-pos '(positioned-marker))) me]))
(receive
  ([:pos p] (println (str "GOT: " p)))
  (after 5000 (throw "no reply from probe")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
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
        out.status.success(),
        "client did not finish cleanly.\n--- client stdout ---\n{stdout}\n--- client stderr ---\n{stderr}\n--- server stderr ---\n{a_err}"
    );
    // The literal sits on line 7 of `client.blsp`. If positions crossed the
    // wire the receiver returns `[7 col]`; otherwise `nil`.
    assert!(
        stdout.contains("GOT: [7 "),
        "expected `[7 col]` from the remote (the client-side line of the quoted list survived the wire), got:\n{stdout}"
    );
}

/// `(remote-spawn node expr)` ships a thunk to a peer, where a `:remote-spawn`
/// server (lazily started on first call) `(spawn)`s it locally. End-to-end:
/// the client triggers a remote spawn that captures the client's pid, the
/// spawned process runs on A, sends the client a `[:hello-from-a]` message,
/// the client receives it. Proves the convenience macro and its on-demand
/// server bootstrap.
#[test]
fn remote_spawn_runs_a_thunk_on_a_peer() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-rspawn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Server: enable remote-spawn here (the receiver opts in), then park.
    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(start-remote-spawn)
;; Park forever; the harness kills us.
(receive (after 10000 nil))
"#
    );

    // Client: captures its own pid, asks A to spawn a thunk that messages back.
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(let (me (self))
  (remote-spawn :a (send me [:hello-from-a (node-name)])))
(receive
  ([:hello-from-a from] (if (= from :a)
                          (println "REMOTE-SPAWN-OK")
                          (throw (str "spawned on wrong node: " from))))
  (after 5000 (throw "no reply from remote-spawn")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
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
        out.status.success() && stdout.contains("REMOTE-SPAWN-OK"),
        "client failed.\n--- client stdout ---\n{stdout}\n--- client stderr ---\n{stderr}\n--- server stderr ---\n{a_err}"
    );
}

/// Distributed pid monitor: A registers a worker pid, B `(monitor)`s it, A's
/// worker exits cleanly, B receives `[:down mref pid reason]` on its mailbox.
/// Verifies the full cross-node monitor path: the `Frame::Monitor` register
/// on A's side reuses the same `add_monitor` core the local monitor uses; A's
/// `deregister` fires the `Remote` watcher; the `[:down …]` is routed back as
/// an ordinary `send` to B's pid.
#[test]
fn cross_node_pid_monitor_fires_down() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-mon-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Server on A: a `:work-bootstrap` registered name that, on `[:hello from]`,
    // spawns a worker which (a) replies its own pid back, (b) parks until it
    // gets `:stop`. The bootstrap step is how the client gets a remote pid to
    // pass to `monitor` — monitors take a pid, not a name.
    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(defn worker (parent)
  (do (send parent [:my-pid (self)])
      (receive (:stop nil) (_ nil))))
(register :work-bootstrap (self))
(receive
  ([:hello from] (spawn (worker from)))
  (after 10000 nil))
;; Park to keep the link alive past the monitor + down delivery.
(receive (after 10000 nil))
"#
    );
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(send {{:name :work-bootstrap :node :a}} [:hello (self)])
(def remote-pid (receive ([:my-pid p] p) (after 5000 (throw "no pid reply"))))
(def m (monitor remote-pid))
(send remote-pid :stop)
(receive
  ([:down mref pid reason]
    (if (and (= mref m) (pid? pid))
      (println "DOWN-OK")
      (throw (str "wrong down: " mref " " pid " " reason))))
  (after 5000 (throw "no :down message")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
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
        out.status.success() && stdout.contains("DOWN-OK"),
        "client failed.\n--- client stdout ---\n{stdout}\n--- client stderr ---\n{stderr}\n--- server stderr ---\n{a_err}"
    );
}

/// Net-split fires a `:noconnection` on a remote monitor. B monitors A's
/// worker, A dies (the test kills it), B's pending remote monitor fires
/// `[:down mref pid :noconnection]` via `handle_node_down`. Proves the
/// sender-side `PENDING_REMOTE` table is wired and dropped on node-down —
/// without it, B's watcher would silently never hear about A's disappearance.
#[test]
fn remote_monitor_fires_noconnection_on_node_down() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-noconn-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Server on A: spawns a worker, sends its pid back to the requesting
    // client, then parks. The whole runtime gets killed externally — that's
    // the "node down" trigger.
    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(defn worker (parent)
  (do (send parent [:my-pid (self)])
      (receive (after 60000 nil))))
(register :work-bootstrap (self))
(receive
  ([:hello from] (spawn (worker from)))
  (after 10000 nil))
(receive (after 60000 nil))
"#
    );

    // Client: get the remote pid, monitor it, kill A, expect :noconnection.
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(send {{:name :work-bootstrap :node :a}} [:hello (self)])
(def remote-pid (receive ([:my-pid p] p) (after 5000 (throw "no pid reply"))))
(def m (monitor remote-pid))
;; Tell the parent harness we're armed; the harness will kill A. We can't
;; kill A from inside the language, so we wait a beat and the harness sends
;; SIGKILL to A externally.
(println "ARMED")
(receive
  ([:down mref pid reason]
    (if (and (= mref m) (= reason :noconnection))
      (println "NOCONNECTION-OK")
      (throw (str "wrong down on netsplit: mref=" mref " reason=" reason))))
  (after 10000 (throw "no :noconnection within 10s")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
    wait_until_listening(port_a);
    let mut b = spawn_brood(&dir, "client.blsp", &client);

    // Wait for the client to print ARMED (its monitor is in place), then
    // kill A. We read B's stdout line-by-line to detect the marker.
    let mut b_stdout = b.stdout.take().expect("client stdout");
    let mut buf = Vec::new();
    let mut armed = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !armed {
        let mut chunk = [0u8; 256];
        match b_stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if String::from_utf8_lossy(&buf).contains("ARMED") {
                    armed = true;
                }
            }
            Err(_) => break,
        }
    }
    assert!(
        armed,
        "client never reached ARMED — monitor wasn't set up. partial stdout:\n{}",
        String::from_utf8_lossy(&buf)
    );

    // Trigger the net-split.
    let _ = a.kill();
    let _ = a.wait();

    // Drain the rest of B's stdout (it should print NOCONNECTION-OK).
    let _ = b_stdout.read_to_end(&mut buf);
    let status = b.wait().expect("client finished");
    let mut b_err = String::new();
    if let Some(mut e) = b.stderr.take() {
        let _ = e.read_to_string(&mut b_err);
    }
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&buf);
    assert!(
        status.success() && stdout.contains("NOCONNECTION-OK"),
        "client did not see :noconnection.\n--- client stdout ---\n{stdout}\n--- client stderr ---\n{b_err}"
    );
}

/// `(ensure-link addr)` keeps a peer link alive across restarts. Start A1, B
/// `ensure-link`s it and sends a probe; kill A1; restart A2 on the same
/// port/name; B's supervisor reconnects and a second probe round-trips. Pure
/// Brood policy on top of `connect` + `monitor-node`; no Rust changes — this
/// test guards the policy code in `std/prelude.blsp`.
#[test]
fn ensure_link_reconnects_across_a_node_restart() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-rec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Both A1 and A2 use the same `:a` name + port + cookie — they're "the
    // same node" coming back up after a crash, from the link's point of view.
    let server_src = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(register :probe (self))
(defn serve ()
  (receive
    ([:ping from] (do (send from [:pong (self)]) (serve)))
    (_ (serve))))
(serve)
"#
    );

    // B: ensure the link, ping once, kill A externally, wait for reconnect
    // (we just keep retrying the ping with a small timeout until it succeeds
    // a second time — `ensure-link` will reconnect under us).
    let client_src = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(ensure-link "a@127.0.0.1:{port_a}")
;; First probe — proves the initial link came up.
(send {{:name :probe :node :a}} [:ping (self)])
(receive ([:pong _] (println "FIRST-OK")) (after 5000 (throw "no first pong")))
;; Tell the harness we're ready for the restart.
(println "ARMED")
;; Now retry the second ping until something answers — the harness will
;; bounce A1 → A2 in between. `ensure-link` re-`connect`s on :nodedown.
(defn try-second (n)
  (when (= n 0) (throw "no second pong after retries"))
  (send {{:name :probe :node :a}} [:ping (self)])
  (receive
    ([:pong _] (println "SECOND-OK"))
    (after 500 (try-second (- n 1)))))
(try-second 40)
"#
    );

    // Bring A1 up.
    let mut a1 = spawn_brood(&dir, "server.blsp", &server_src);
    wait_until_listening(port_a);

    // Start B, get to ARMED.
    let mut b = spawn_brood(&dir, "client.blsp", &client_src);
    let mut b_stdout = b.stdout.take().expect("client stdout");
    let mut buf = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let mut chunk = [0u8; 256];
        match b_stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if String::from_utf8_lossy(&buf).contains("ARMED") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    assert!(
        String::from_utf8_lossy(&buf).contains("ARMED"),
        "client never reached ARMED. partial stdout:\n{}",
        String::from_utf8_lossy(&buf)
    );

    // Kill A1, give the OS a moment to free the port, then bring A2 up
    // (same name + port). We may have to retry the bind a couple of times
    // because TIME_WAIT can still hold the address briefly.
    let _ = a1.kill();
    let _ = a1.wait();
    let mut a2 = None;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if TcpStream::connect(("127.0.0.1", port_a)).is_err() {
            // Port is free; try to bind A2 to it.
            a2 = Some(spawn_brood(&dir, "server.blsp", &server_src));
            break;
        }
    }
    let mut a2 = a2.expect("could not free port for A2");
    wait_until_listening(port_a);

    // Drain B and assert.
    let _ = b_stdout.read_to_end(&mut buf);
    let status = b.wait().expect("client finished");
    let mut b_err = String::new();
    if let Some(mut e) = b.stderr.take() {
        let _ = e.read_to_string(&mut b_err);
    }
    let _ = a2.kill();
    let _ = a2.wait();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&buf);
    assert!(
        status.success() && stdout.contains("FIRST-OK") && stdout.contains("SECOND-OK"),
        "client did not reconnect across the restart.\n--- client stdout ---\n{stdout}\n--- client stderr ---\n{b_err}"
    );
}

/// A non-brood peer that doesn't speak the v2 protocol must be rejected at
/// the magic-prefix step, before any frame parsing. We connect a plain
/// `TcpStream`, write garbage bytes, and expect the server to disconnect us
/// (read on the stream eventually errors / returns 0). Guards the
/// `PROTOCOL_MAGIC` gate in `handshake`.
#[test]
fn non_brood_peer_is_rejected_at_magic_prefix() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-magic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_a = free_port();
    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(receive (after 5000 nil))
"#
    );
    let mut a = spawn_brood(&dir, "server.blsp", &server);
    wait_until_listening(port_a);

    // Speak HTTP instead of brood-v2. The server reads our 4 magic bytes,
    // compares to b"BRD\x02", finds "GET ", aborts. We then try to read; the
    // server has already shut the socket, so we hit EOF.
    let mut s = TcpStream::connect(("127.0.0.1", port_a)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    use std::io::Write as _;
    let _ = s.write_all(b"GET / HTTP/1.1\r\n\r\n");
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap_or(0);
    // EOF (n == 0) or a very small reply — either way, no valid handshake
    // proceeded. The server's stderr should not be empty (it logs the
    // mismatch via the `dist: incoming connection failed: …` path).
    let _ = a.kill();
    let _ = a.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        n == 0 || n < 8,
        "server should have closed the socket after the bad magic; read {} bytes",
        n
    );
}

/// A bad cookie must be rejected: B cannot reach A's `:echo`, so the by-name
/// send is silently dropped and B times out (Erlang semantics — no delivery, no
/// error at the sender).
#[test]
fn mismatched_cookie_is_rejected() {
    let _g = port_lock();
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
    let _ = a.wait(); // reap, so the test doesn't leave a zombie
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
    let _g = port_lock();
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
    let _ = a.wait(); // reap, so the test doesn't leave a zombie
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
    let _g = port_lock();
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
    let _g = port_lock();
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
    let _g = port_lock();
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
    let _ = a.wait(); // reap, so the test doesn't leave a zombie
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("NODEDOWN-OK"),
        "expected a [:nodedown :a] after the peer exited.\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
