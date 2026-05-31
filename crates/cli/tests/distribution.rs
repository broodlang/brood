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
/// panic after ~20s.
fn wait_until_listening(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
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
(send {{:name :echo :node :a@127.0.0.1}} [:hi (self)])
(def remote (receive ([:pong p] p) (after 30000 (throw "no reply by name"))))
(unless (pid? remote) (throw "reply was not a pid"))
(send remote [:ping (self)])
(receive ([:pong _] (println "ROUNDTRIP-OK")) (after 30000 (throw "no reply by pid")))
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

/// A **clean** peer exit (process returns → OS closes the socket) fires
/// `[:nodedown]` on the survivor **promptly** — via the reader's socket-EOF path,
/// not the ~6s heartbeat timeout — and drops the peer from `(nodes)`.
///
/// Regression guard for a misdiagnosed report ("clean disconnects are not detected
/// until heartbeat timeout"): in fact the kernel has detected clean close since
/// the reader's `drop_link`-on-EOF landed (2026-05-28). The survivor's
/// `(after 5000 …)` cap is *below* the 6s heartbeat window, so reaching the
/// `[:nodedown]` clause proves close-detection rather than heartbeat liveness.
/// (The dialer-watches-acceptor direction; the symmetric case works the same.)
#[test]
fn clean_peer_exit_fires_nodedown_promptly() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-nodedown-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port_a = free_port();
    let port_b = free_port();

    // Node B: come up, accept A's link, stay briefly, then *return* — a clean exit
    // that closes the socket (the `/quit` path, not a `kill`).
    let quitter = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(sleep 1500)
"#
    );

    // Node A: connect, monitor B by the authoritative name `connect` returns, then
    // wait for `[:nodedown]`. B exits ~1.5s in; nodedown must arrive well under the
    // 5s cap (which is itself under the 6s heartbeat) — and `(nodes)` must prune.
    let watcher = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(def peer (connect "b@127.0.0.1:{port_b}"))
(monitor-node peer)
(receive
  ([:nodedown p]
    (if (empty? (nodes))
      (println "NODEDOWN-OK " p)
      (println "NODEDOWN-BUT-NODES-NOT-PRUNED " (nodes))))
  (after 5000 (println "TIMEOUT-no-nodedown " (nodes))))
"#
    );

    let mut b = spawn_brood(&dir, "quitter.blsp", &quitter);
    wait_until_listening(port_b);
    let a = spawn_brood(&dir, "watcher.blsp", &watcher);

    let out = a.wait_with_output().expect("watcher finished");
    let _ = b.kill(); // already exited cleanly; reap defensively
    let _ = b.wait();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("NODEDOWN-OK"),
        "expected prompt nodedown + pruned (nodes) on a clean peer exit.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}

/// Run a `.blsp` program in a fresh `brood` subprocess with extra env vars —
/// used by the Unix-socket tests to sandbox `$HOME`/`$XDG_*` (so the cookie file
/// lands in the test's temp dir, never the runner's real `~/.config`) and to set
/// `$BROOD_COOKIE` for the wrong-cookie case.
fn spawn_brood_env(dir: &std::path::Path, name: &str, src: &str, env: &[(&str, &str)]) -> Child {
    let path = dir.join(name);
    std::fs::write(&path, src).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brood"));
    cmd.arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.spawn().expect("spawn brood")
}

/// Wait until a Unix socket file appears (the peer's listener is bound), or panic
/// after ~20s — the name-addressed analogue of [`wait_until_listening`].
fn wait_until_socket(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if path.exists() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("node socket never appeared at {}", path.display());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Two local nodes connect **by name** over a Unix-domain socket — no port, no
/// explicit cookie. Proves the name-addressed transport + the auto-generated
/// shared cookie file end-to-end (ADR-068): both children share one sandboxed
/// `$HOME`, so `(node-cookie)` mints the secret once and the second reads it back.
#[test]
fn two_unix_nodes_connect_by_name_and_message() {
    let home = std::env::temp_dir().join(format!("brood-unix-{}", std::process::id()));
    let run = home.join("run");
    let cfg = home.join(".config");
    std::fs::create_dir_all(&run).unwrap();
    let env: Vec<(&str, &str)> = vec![
        ("HOME", home.to_str().unwrap()),
        ("XDG_CONFIG_HOME", cfg.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", run.to_str().unwrap()),
    ];

    let server = r#"
(node-start :ua)
(register :echo (self))
(defn serve ()
  (receive
    ([:hi from] (do (send from [:pong (self)]) (serve)))
    (_ (serve))))
(serve)
"#;
    let client = r#"
(node-start :ub)
;; connect returns the peer's authoritative name@host (ADR-073) — address by it.
(def peer (connect "ua"))
(send {:name :echo :node peer} [:hi (self)])
(def remote (receive ([:pong p] p) (after 30000 (throw "no reply by name"))))
(unless (pid? remote) (throw "reply was not a pid"))
(send remote [:hi (self)])
(receive ([:pong _] (println "UNIX-ROUNDTRIP-OK")) (after 30000 (throw "no reply by pid")))
"#;

    let mut a = spawn_brood_env(&home, "userver.blsp", server, &env);
    wait_until_socket(&run.join("brood").join("ua.sock"));
    let b = spawn_brood_env(&home, "uclient.blsp", client, &env);

    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = a.wait();
    let mut a_err = String::new();
    if let Some(mut e) = a.stderr.take() {
        let _ = e.read_to_string(&mut a_err);
    }
    let _ = std::fs::remove_dir_all(&home);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("UNIX-ROUNDTRIP-OK"),
        "unix client failed.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}\n--- server stderr ---\n{a_err}"
    );
}

/// A mismatched cookie is rejected over the Unix transport: the HMAC handshake
/// fails (the same `PermissionDenied` path as TCP) and `connect` raises. Each
/// side pins a distinct `$BROOD_COOKIE`, so the file fallback is bypassed.
#[test]
fn wrong_cookie_rejected_over_unix() {
    let home = std::env::temp_dir().join(format!("brood-unix-bad-{}", std::process::id()));
    let run = home.join("run");
    std::fs::create_dir_all(&run).unwrap();
    let run_s = run.to_str().unwrap();
    let home_s = home.to_str().unwrap();
    let env_a: Vec<(&str, &str)> = vec![
        ("XDG_RUNTIME_DIR", run_s),
        ("HOME", home_s),
        ("BROOD_COOKIE", "alpha"),
    ];
    let env_b: Vec<(&str, &str)> = vec![
        ("XDG_RUNTIME_DIR", run_s),
        ("HOME", home_s),
        ("BROOD_COOKIE", "beta"),
    ];

    let server = r#"
(node-start :uc)
(register :echo (self))
(defn serve () (receive (_ (serve))))
(serve)
"#;
    // A wrong cookie → handshake MAC mismatch → connect raises → we print REJECTED.
    let client = r#"
(node-start :ud)
(try (do (connect "uc") (println "UNEXPECTED-CONNECT"))
     (catch _ (println "REJECTED")))
"#;

    let mut a = spawn_brood_env(&home, "bserver.blsp", server, &env_a);
    wait_until_socket(&run.join("brood").join("uc.sock"));
    let b = spawn_brood_env(&home, "bclient.blsp", client, &env_b);
    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = a.wait();
    let _ = std::fs::remove_dir_all(&home);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("REJECTED") && !stdout.contains("UNEXPECTED-CONNECT"),
        "wrong cookie should have been rejected.\n--- stdout ---\n{stdout}"
    );
}

/// The cookie file is auto-generated once (mode `0600`) and reused: two runs in
/// the same sandboxed `$HOME` print the *same* secret (ADR-068).
#[test]
fn cookie_file_autogen_and_reuse() {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::temp_dir().join(format!("brood-cookie-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let cfg = home.join(".config");
    let env: Vec<(&str, &str)> = vec![
        ("HOME", home.to_str().unwrap()),
        ("XDG_CONFIG_HOME", cfg.to_str().unwrap()),
    ];
    let prog = "(println (node-cookie))";

    let first = spawn_brood_env(&home, "c1.blsp", prog, &env)
        .wait_with_output()
        .unwrap();
    let second = spawn_brood_env(&home, "c2.blsp", prog, &env)
        .wait_with_output()
        .unwrap();
    let s1 = String::from_utf8_lossy(&first.stdout).trim().to_string();
    let s2 = String::from_utf8_lossy(&second.stdout).trim().to_string();

    let cookie_path = home.join(".config").join("brood").join("cookie");
    let mode = std::fs::metadata(&cookie_path)
        .expect("cookie file exists")
        .permissions()
        .mode()
        & 0o777;
    let _ = std::fs::remove_dir_all(&home);

    assert_eq!(s1.len(), 64, "cookie should be 32 bytes hex");
    assert_eq!(s1, s2, "second run must reuse the persisted cookie");
    assert_eq!(mode, 0o600, "cookie file must be owner-only");
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
  (send {{:name :worker :node :a@127.0.0.1}} [:run (fn (x) (* x n)) 14 (self)]))
(receive
  ([:result r] (if (= r 42)
                 (println "CROSS-NODE-LAMBDA-OK")
                 (throw (str "expected 42, got " r))))
  (after 30000 (throw "no reply")))
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
  (send {{:name :probe :node :a@127.0.0.1}} [:run (fn () (form-pos '(positioned-marker))) me]))
(receive
  ([:pos p] (println (str "GOT: " p)))
  (after 30000 (throw "no reply from probe")))
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
  (remote-spawn :a@127.0.0.1 (send me [:hello-from-a (node-name)])))
(receive
  ([:hello-from-a from] (if (= from :a@127.0.0.1)
                          (println "REMOTE-SPAWN-OK")
                          (throw (str "spawned on wrong node: " from))))
  (after 30000 (throw "no reply from remote-spawn")))
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
(send {{:name :work-bootstrap :node :a@127.0.0.1}} [:hello (self)])
(def remote-pid (receive ([:my-pid p] p) (after 30000 (throw "no pid reply"))))
(def m (monitor remote-pid))
(send remote-pid :stop)
(receive
  ([:down mref pid reason]
    (if (and (= mref m) (pid? pid))
      (println "DOWN-OK")
      (throw (str "wrong down: " mref " " pid " " reason))))
  (after 30000 (throw "no :down message")))
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
(send {{:name :work-bootstrap :node :a@127.0.0.1}} [:hello (self)])
(def remote-pid (receive ([:my-pid p] p) (after 30000 (throw "no pid reply"))))
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
    let deadline = Instant::now() + Duration::from_secs(20);
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
(send {{:name :probe :node :a@127.0.0.1}} [:ping (self)])
(receive ([:pong _] (println "FIRST-OK")) (after 30000 (throw "no first pong")))
;; Tell the harness we're ready for the restart.
(println "ARMED")
;; Now retry the second ping until something answers — the harness will
;; bounce A1 → A2 in between. `ensure-link` re-`connect`s on :nodedown.
(defn try-second (n)
  (when (= n 0) (throw "no second pong after retries"))
  (send {{:name :probe :node :a@127.0.0.1}} [:ping (self)])
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
    let deadline = Instant::now() + Duration::from_secs(20);
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
(send {{:name :echo :node :a@127.0.0.1}} [:hi (self)])
(receive ([:welcome] :ok) (after 30000 (throw "no welcome")))
(println (str "NODES=" (nodes)))           ; expect exactly (:a@127.0.0.1)
(send {{:name :echo :node :a@127.0.0.1}} [:bye (self)])
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
        out.status.success() && stdout.contains("NODES=(:a@127.0.0.1)"),
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
         (after 5000 (throw "monitor-node did not fire immediately")))
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
(send {{:name :echo :node :a@127.0.0.1}} [:hi (self)])
(receive ([:welcome] :ok) (after 30000 (throw "no welcome")))   ; link + monitor are up
(send {{:name :echo :node :a@127.0.0.1}} [:bye (self)])                  ; make :a exit
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

/// Cross-node **links** (ADR-067): node B links a worker on node A and traps
/// exits; when the worker crashes, A propagates a `Frame::Exit` over the link and
/// B receives `[:EXIT <remote-pid> [:error …]]`. Exercises `Frame::Link` (B→A,
/// recording A's reverse half) + `Frame::Exit` (A→B, link death) + trap delivery
/// carrying a *remote* pid.
#[test]
fn remote_link_death_delivers_exit_to_a_trapping_peer() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-link-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_a = free_port();
    let port_b = free_port();

    // A: a worker (a *spawned child*, so the node survives its death) that
    // reports its pid on :whoami and crashes on :die-now; main parks to keep the
    // node up so the link `Frame::Exit` is delivered over a live connection.
    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(defn worker-loop ()
  (receive
    ([:whoami from] (do (send from [:iam (self)]) (worker-loop)))
    ([:die-now] (error "boom"))
    (_ (worker-loop))))
(register :worker (spawn (worker-loop)))
(receive (:never :x))
"#
    );
    // B: obtain the worker's remote pid, link it (trapping), make it crash, expect [:EXIT].
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(trap-exit true)
(send {{:name :worker :node :a@127.0.0.1}} [:whoami (self)])
(def w (receive ([:iam p] p) (after 30000 (throw "no whoami"))))
(unless (pid? w) (throw "whoami reply was not a pid"))
(link w)                                  ; cross-node link (Frame::Link → A records its half)
(send w [:die-now])                       ; ordered after the link on this connection
(receive ([:EXIT ~w [:error _]] (println "REMOTE-LINK-EXIT-OK"))
         ([:EXIT ~w r] (throw (str "unexpected EXIT reason " r)))
         (after 30000 (throw "no remote [:EXIT]")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "client.blsp", &client);
    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = a.wait();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("REMOTE-LINK-EXIT-OK"),
        "expected [:EXIT remote [:error …]] from a crashed linked remote worker.\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Cross-node **`(exit remote-pid :kill)`** (ADR-067): node B terminates a worker
/// on node A directly. B also `monitor`s it, so the resulting death comes back as
/// `[:down … :kill]`. Exercises the non-link `Frame::Exit` (B→A → `scheduler::exit`).
#[test]
fn remote_exit_kills_a_worker() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-rexit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_a = free_port();
    let port_b = free_port();

    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(defn worker-loop ()
  (receive
    ([:whoami from] (do (send from [:iam (self)]) (worker-loop)))
    (_ (worker-loop))))    ; parks; only an external exit can stop it
(register :worker (spawn (worker-loop)))
(receive (:never :x))      ; main parks so the node outlives the worker's kill
"#
    );
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(send {{:name :worker :node :a@127.0.0.1}} [:whoami (self)])
(def w (receive ([:iam p] p) (after 30000 (throw "no whoami"))))
(def m (monitor w))
(exit w :kill)                            ; remote kill (non-link Frame::Exit)
(receive ([:down ~m ~w _] (println "REMOTE-EXIT-KILL-OK"))
         (after 30000 (throw "remote worker did not die")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "client.blsp", &client);
    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = a.wait();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("REMOTE-EXIT-KILL-OK"),
        "expected a remote (exit w :kill) to kill the worker and fire [:down].\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// End-to-end **cross-node supervision** (ADR-067, the #1 payoff): a supervisor on
/// node B supervises a worker on node A, and restarts it when it crashes — all
/// over the distributed link. The child `:start` does a roundtrip to A's
/// `:factory` to obtain the remote worker's pid (since `remote-spawn` is
/// fire-and-forget); the supervisor links that remote pid, so the remote crash
/// arrives as a link `[:EXIT]` and triggers a restart that spins up a fresh worker.
#[test]
fn supervisor_restarts_a_remote_child() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-sup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_a = free_port();
    let port_b = free_port();

    // A: a factory that makes workers on demand and replies with their pids. Each
    // worker announces `[:up (self)]` to the observer it's given, then crashes on :die.
    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(defn worker-loop (obs)
  (do (send obs [:up (self)])
      (receive (:die (error "boom")) (_ (worker-loop obs)))))
(defn factory ()
  (receive
    ([:make reply obs] (do (send reply [:made (spawn (worker-loop obs))]) (factory)))
    (_ (factory))))
(register :factory (spawn (factory)))
(receive (:never :x))
"#
    );
    // B: supervise a remote child. `:start` asks A's factory for a worker pid and
    // returns it; the supervisor links it. Crash it; expect a fresh incarnation.
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(require 'supervisor)
(def me (self))
(def spec {{:id :w :restart :permanent
            :start (fn () (do (send {{:name :factory :node :a@127.0.0.1}} [:make (self) me])
                              (receive ([:made p] p) (after 30000 (throw "no :made")))))}})
(def sup (supervisor/start-supervisor (list spec)))
(def w1 (receive ([:up p] p) (after 6000 (throw "no first :up"))))
(send w1 :die)                              ; crash the remote worker
(def w2 (receive ([:up p] p) (after 6000 (throw "no restart :up"))))
(if (not (= w1 w2)) (println "CROSS-NODE-SUP-OK") (throw "restart reused the pid"))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "client.blsp", &client);
    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = a.wait();
    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("CROSS-NODE-SUP-OK"),
        "expected a supervisor on B to restart a crashed remote child on A.\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Node names are qualified to `name@host` (ADR-073): a TCP node takes the host
/// from its listen address (so peers can derive the same name from the dial
/// address), and pids print with that qualified node. Single process — no peer.
#[test]
fn node_name_is_qualified_with_host() {
    let dir = std::env::temp_dir().join(format!("brood-dist-qual-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let prog = r#"
(node-start :a "127.0.0.1:0")
(println (str "NODE=" (node-name)))
(println (str "SELF=" (self)))
"#;
    let out = spawn_brood(&dir, "qual.blsp", prog)
        .wait_with_output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("NODE=:a@127.0.0.1"),
        "node name should be qualified name@host; got:\n{stdout}"
    );
    assert!(
        stdout.contains("a@127.0.0.1/"),
        "a pid should print with the qualified node name; got:\n{stdout}"
    );
}

/// `(remote-spawn-sync node expr)` (ADR-067 residual) runs `expr` on a peer and
/// **returns the child's pid** — a remote pid carrying the peer's `name@host`.
/// Proves the request/reply roundtrip and that the returned pid is usable
/// (the child messages back through a captured local).
#[test]
fn remote_spawn_sync_returns_a_usable_remote_pid() {
    let _g = port_lock();
    let dir = std::env::temp_dir().join(format!("brood-dist-rss-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port_a = free_port();
    let port_b = free_port();

    let server = format!(
        r#"
(node-start :a "127.0.0.1:{port_a}" "secret")
(start-remote-spawn)
(receive (:never :x))
"#
    );
    let client = format!(
        r#"
(node-start :b "127.0.0.1:{port_b}" "secret")
(connect "a@127.0.0.1:{port_a}")
(let (me (self))
  (def child (remote-spawn-sync :a@127.0.0.1 (send me [:ran (self) (* 6 7)]))))
(unless (pid? child) (throw "remote-spawn-sync did not return a pid"))
(receive
  ([:ran on val] (if (= val 42)
                   (println (str "REMOTE-SPAWN-SYNC-OK child=" child " ran-on=" on))
                   (throw (str "wrong value " val))))
  (after 30000 (throw "remote child never ran")))
"#
    );

    let mut a = spawn_brood(&dir, "server.blsp", &server);
    wait_until_listening(port_a);
    let b = spawn_brood(&dir, "client.blsp", &client);
    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = a.wait();
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("REMOTE-SPAWN-SYNC-OK"),
        "remote-spawn-sync should return the child pid and the child should run.\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **Dual-listen** (ADR-074): one node serves *both* a TCP endpoint and a local
/// Unix socket via `node-also-listen`. A client reaches the same node — same
/// authoritative `name@host`, same registered `:echo` — over each transport.
/// Needs a shared `$HOME`/`$XDG_*` (for the cookie + the Unix socket dir) *and* a
/// TCP port, so it uses `spawn_brood_env` + `free_port` together.
#[test]
fn dual_listen_serves_tcp_and_unix_at_once() {
    let _g = port_lock();
    let home = std::env::temp_dir().join(format!("brood-dual-{}", std::process::id()));
    let run = home.join("run");
    let cfg = home.join(".config");
    std::fs::create_dir_all(&run).unwrap();
    let port = free_port();
    let env: Vec<(&str, &str)> = vec![
        ("HOME", home.to_str().unwrap()),
        ("XDG_CONFIG_HOME", cfg.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", run.to_str().unwrap()),
    ];

    // Explicit `:ed@127.0.0.1` so the TCP dial host matches the node's identity.
    let server = format!(
        r#"
(node-start :ed@127.0.0.1 "127.0.0.1:{port}")
(node-also-listen)                       ; + the local Unix socket "ed"
(register :echo (self))
(defn serve () (receive ([:hi from] (do (send from [:pong (self)]) (serve))) (_ (serve))))
(serve)
"#
    );
    let client = format!(
        r#"
(node-start :cli)
(defn tc (f n) (try (f) (catch e (if (> n 0) (do (sleep 100) (tc f (- n 1))) (throw e)))))
(def via-tcp  (tc (fn () (connect "ed@127.0.0.1:{port}")) 50))
(def via-unix (tc (fn () (connect "ed")) 50))
(unless (= via-tcp via-unix) (throw (str "transports gave different nodes: " via-tcp " vs " via-unix)))
(send {{:name :echo :node via-tcp}}  [:hi (self)])
(receive ([:pong _] :ok) (after 30000 (throw "no pong over tcp")))
(send {{:name :echo :node via-unix}} [:hi (self)])
(receive ([:pong _] :ok) (after 30000 (throw "no pong over unix")))
(println "DUAL-LISTEN-OK")
"#
    );

    let mut a = spawn_brood_env(&home, "dserver.blsp", &server, &env);
    wait_until_listening(port);
    let b = spawn_brood_env(&home, "dclient.blsp", &client, &env);
    let out = b.wait_with_output().expect("client finished");
    let _ = a.kill();
    let _ = a.wait();
    let _ = std::fs::remove_dir_all(&home);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("DUAL-LISTEN-OK"),
        "one node should be reachable over both TCP and the local Unix socket.\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
