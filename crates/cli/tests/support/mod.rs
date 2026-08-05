//! Shared harness for the tests that run **real `brood` OS processes over loopback**
//! (`distribution.rs`, `serve_attach.rs`, `observe_attach.rs`).
//!
//! These three had their own copies of the same four helpers, which is how the port
//! allocator's KI-27 bug came to be fixed in one file and left in two. One definition now.
#![allow(dead_code)] // each test binary uses a subset

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Serialises the *bind→spawn* window between tests **in this process**. Two tests racing
/// through [`free_port`] can both pick the same port; the loser's child then fails to bind,
/// the winner's listener is what [`wait_until_listening`] happens to find, and the loser's
/// client times out with `ECONNREFUSED`.
///
/// Note what this lock cannot do: `make test` runs the suite under `cargo-nextest`, which
/// gives **each test its own process**, so between the tests that actually run concurrently
/// there this mutex does not exist. Cross-process safety has to come from [`free_port`]
/// itself — see there.
static PORTS: Mutex<()> = Mutex::new(());

/// Acquire the cross-test bind lock. Released when the returned guard drops. (`PoisonError`
/// is recovered into the inner unit; a panicked sibling test shouldn't wedge the suite.)
pub fn port_lock() -> MutexGuard<'static, ()> {
    PORTS.lock().unwrap_or_else(|p| p.into_inner())
}

/// Reserve a localhost port for a test node.
///
/// Deliberately **not** `bind("127.0.0.1:0")`-and-drop, which is what this used to be. That
/// asks the kernel for a port from the *ephemeral* range (32768–60999 on this box) — the very
/// range every outbound connection in these tests is assigned from — and then releases it. So
/// the port a node is about to bind can be handed to some other process's client socket in
/// the gap, and the node's bind fails with `EADDRINUSE`. The failure is load-only, silent, and
/// looks nothing like a port problem from the test's side: it was KI-27, which reproduced only
/// under a full `make test` and never solo.
///
/// Instead: allocate from a fixed band **below** the ephemeral range, where the kernel will
/// never hand the port out on its own, sliced by pid so concurrently-running test *processes*
/// start in different places, and probe that the port really is bindable before returning it.
pub fn free_port() -> u16 {
    /// 12000..32768 — above the well-known services, below `ip_local_port_range`'s floor.
    const BASE: u16 = 12_000;
    const SPAN: u16 = 20_768;
    /// Ports per process slice. Must exceed what one *process* can need: under plain
    /// `cargo test` all 30-odd tests in a file share a process and take 2–3 ports each, so 64
    /// wrapped mid-run and started handing back ports it had already given out. Still small
    /// enough that concurrent test processes land in different slices — and since pids are
    /// allocated near-consecutively, processes started together almost always do.
    const SLICE: u16 = 128;
    static NEXT: AtomicU16 = AtomicU16::new(0);

    let slices = SPAN / SLICE;
    let slice = (std::process::id() % slices as u32) as u16;
    for probe in 0..SPAN {
        let off = NEXT.fetch_add(1, Ordering::Relaxed) % SLICE;
        // Stay in our own slice for the first pass; widen only if it is somehow exhausted.
        let port = BASE + (slice * SLICE + off + probe / SLICE * SLICE) % SPAN;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port in {BASE}..{}", BASE + SPAN);
}

/// Run a `.blsp` program in a fresh `brood` subprocess.
pub fn spawn_brood(dir: &std::path::Path, name: &str, src: &str) -> Child {
    spawn_brood_env(dir, name, src, &[])
}

/// [`spawn_brood`] with extra environment variables for the child — used by the Unix-socket
/// tests to sandbox `$HOME`/`$XDG_*` (so the cookie file lands in the test's temp dir, never
/// the runner's real `~/.config`) and to set `$BROOD_COOKIE` for the wrong-cookie case.
pub fn spawn_brood_env(
    dir: &std::path::Path,
    name: &str,
    src: &str,
    env: &[(&str, &str)],
) -> Child {
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

/// Wait until `port` accepts a TCP connection (the peer's listener is up), or panic.
pub fn wait_until_listening(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("server never started listening on port {port}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
