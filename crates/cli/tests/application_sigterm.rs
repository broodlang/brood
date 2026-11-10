//! ADR-435/436: a running application stops cleanly on SIGTERM, and a trap whose subscriber
//! has died leaves SIGTERM's default action in place.
//!
//! These run a real `brood` child and signal it from outside — the only way to reach the
//! entry point a supervisor (systemd, a container runtime, `kill`) reaches. The child
//! prints `ready` only once the trap is armed, so the signal can never beat it.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// Generous for a debug binary on a loaded machine.
const DEADLINE: Duration = Duration::from_secs(30);

/// Start `brood` on `source`, returning the child and a channel of its stdout lines.
fn start(name: &str, source: &str) -> (Child, Receiver<String>, std::path::PathBuf) {
    start_with(name, source, |_| {})
}

/// [`start`], letting `configure` adjust the command first.
fn start_with(
    name: &str,
    source: &str,
    configure: impl FnOnce(&mut Command),
) -> (Child, Receiver<String>, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "brood-application-{}-{name}.blsp",
        std::process::id()
    ));
    let mut file = std::fs::File::create(&path).expect("create script");
    file.write_all(source.as_bytes()).expect("write script");
    drop(file);
    let mut command = Command::new(env!("CARGO_BIN_EXE_brood"));
    command
        .env("BROOD_NO_CHECK", "1")
        .env("BROOD_NO_CRASH_REPORT", "1")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Inherited, not piped: nothing drains a pipe, and a full one would block the child.
        .stderr(Stdio::inherit());
    configure(&mut command);
    let mut child = command.spawn().expect("run brood");
    let stdout = child.stdout.take().expect("piped stdout");
    let (lines_in, lines_out) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if lines_in.send(line).is_err() {
                return;
            }
        }
    });
    (child, lines_out, path)
}

/// Wait for the line `expected`, returning every line seen before it.
fn wait_for(child: &mut Child, lines: &Receiver<String>, expected: &str) -> Vec<String> {
    let started = Instant::now();
    let mut seen = Vec::new();
    loop {
        match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) if line == expected => return seen,
            Ok(line) => seen.push(line),
            Err(_) if started.elapsed() > DEADLINE => {
                let _ = child.kill();
                panic!("never printed {expected:?}; saw {seen:?}");
            }
            Err(_) => {}
        }
    }
}

fn wait_exit(child: &mut Child) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll brood") {
            return status;
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("brood did not exit after SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn sigterm(child: &Child) {
    // SAFETY: `kill` has no memory preconditions. Every caller signals before `wait_exit`
    // reaps the child, so the pid is still the child's and cannot have been reused.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
}

#[test]
fn sigterm_stops_an_application_in_reverse_and_exits_cleanly() {
    let source = r#"
(defn- idle () (receive (:never nil)))
(defn- announce-when-running ()
  (if (proc/whereis :application) (io/puts "ready") (do (proc/sleep 10) (announce-when-running))))
(application/defapp store
  {:start (fn () (spawn (idle)))
   :stop (fn (root) (io/puts "stopped store") (exit root :kill))})
(application/defapp web
  {:start (fn () (spawn (announce-when-running)) (spawn (idle)))
   :stop (fn (root) (io/puts "stopped web") (exit root :kill))
   :requires [store]})
(io/puts "returned" (application/run web))
"#;
    let (mut child, lines, path) = start("reverse", source);
    wait_for(&mut child, &lines, "ready");
    sigterm(&child);
    let status = wait_exit(&mut child);
    // Until the reader sees EOF: the child has exited, so its stdout is closed.
    let rest: Vec<String> = lines.iter().collect();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        rest,
        ["stopped web", "stopped store", "returned :ok"],
        "SIGTERM must stop the applications in reverse and let run return; status {status:?}"
    );
    assert_eq!(status.code(), Some(0), "a clean shutdown exits 0");
}

#[test]
fn a_dead_subscriber_leaves_sigterm_fatal() {
    let source = r#"
(let ([pid ref] (spawn-monitor nil))
  (os/trap :term pid)
  (receive ([:down ^ref _ _] nil)))
(io/puts "ready")
(proc/sleep 60000)
(io/puts "survived")
"#;
    let (mut child, lines, path) = start("dead-subscriber", source);
    wait_for(&mut child, &lines, "ready");
    sigterm(&child);
    let status = wait_exit(&mut child);
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        status.signal(),
        Some(libc::SIGTERM),
        "a trap whose subscriber exited must not swallow SIGTERM; status {status:?}"
    );
}

#[test]
fn lifting_a_trap_gives_back_an_ignored_sighup() {
    // Started the way `nohup` starts a program: SIGHUP ignored. Lifting a trap must put
    // that back, not the default action, which would end the runtime on a hangup.
    let source = r#"
(os/trap :hup)
(os/trap :hup nil)
(io/puts "ready")
(io/read-line)
(io/puts "survived")
"#;
    let (mut child, lines, path) = start_with("ignored-hangup", source, |command| {
        use std::os::unix::process::CommandExt;
        // SAFETY: the closure runs in the forked child before exec, and calls only
        // `signal`, which is async-signal-safe; it allocates nothing.
        unsafe {
            command.pre_exec(|| {
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
                Ok(())
            });
        }
    });
    wait_for(&mut child, &lines, "ready");
    // SAFETY: `kill` has no memory preconditions; the child is not reaped until `wait_exit`
    // below, so its pid cannot have been reused.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGHUP);
    }
    // A fatal signal ends the process at `kill`, so the line below reaches a live child
    // only if the hangup was ignored.
    let mut stdin = child.stdin.take().expect("piped stdin");
    let _ = stdin.write_all(b"go\n");
    drop(stdin);
    let status = wait_exit(&mut child);
    let rest: Vec<String> = lines.iter().collect();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        rest,
        ["survived"],
        "SIGHUP must stay ignored after the trap is lifted; status {status:?}"
    );
    assert_eq!(status.code(), Some(0));
}
