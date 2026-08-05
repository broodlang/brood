//! Shared harness for the tests that run **real `brood` OS processes over loopback**
//! (`distribution.rs`, `serve_attach.rs`, `observe_attach.rs`).
//!
//! These three had their own copies of the same four helpers, which is how the port
//! allocator's KI-27 bug came to be fixed in one file and left in two. One definition now.
#![allow(dead_code)] // each test binary uses a subset

use std::ops::{Deref, DerefMut};
use std::process::{Child, Command, Output, Stdio};
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

/// A `brood` child process that cannot outlive the test which spawned it (KI-29).
///
/// Two *independent* nets, because the two ways a child gets orphaned are different and
/// neither net covers the other's case:
///
/// 1. **The test ends without killing it.** An assertion panics between the spawn and the
///    `kill`, or an early `return`/`?` skips the cleanup. [`Drop`] covers this: it kills and
///    reaps whatever is still running.
/// 2. **The test binary is killed outright** — `nextest` fail-fast, a `timeout`, a `^C`. No
///    destructor runs at all, so net 1 is worth nothing here. The child therefore asks the
///    *kernel* to do it: `PR_SET_PDEATHSIG(SIGKILL)` before `exec`, so it dies when the
///    thread that spawned it does, however that thread ends. This is the net that matters —
///    the leak KI-29 recorded was a target node still listening **nine days** later.
///
/// **Deliberately not a process group.** KI-29's filed fix direction was "put children in
/// their own process group and kill the group from a drop guard", but a group is the wrong
/// lever for case 2: moving the child into its *own* group removes it from any group an outer
/// tool might kill, and nothing runs our group-kill when we are SIGKILLed. It buys only
/// grandchildren — which no test here creates (these programs are green processes, no
/// `run-process`) — and it costs the pid-recycling hazard, since `killpg` on a reaped
/// leader's recycled pgid can hit an unrelated group. `Child::kill` cannot: std caches the
/// exit status, so a kill after a `wait` is a no-op instead of a signal to a stranger's pid.
pub struct BroodChild(Option<Child>);

impl BroodChild {
    /// Wait for exit and collect the output. Mirrors [`Child::wait_with_output`], which takes
    /// the `Child` by value and so cannot be reached through [`Deref`].
    pub fn wait_with_output(mut self) -> std::io::Result<Output> {
        self.0
            .take()
            .expect("child already consumed")
            .wait_with_output()
    }
}

impl Deref for BroodChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        self.0.as_ref().expect("child already consumed")
    }
}

impl DerefMut for BroodChild {
    fn deref_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("child already consumed")
    }
}

impl Drop for BroodChild {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            // Both are no-ops if the test already killed and reaped it: std remembers the
            // exit status, so `kill` returns `InvalidInput` without signalling anything and
            // `wait` returns the cached status without blocking.
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Arm net 2 of [`BroodChild`] on any [`Command`]: the child is SIGKILLed by the kernel when
/// the thread that spawned it terminates.
///
/// Separate from [`BroodChild`] so the tests that merely `.output()` a one-shot program get it
/// too. Those cannot orphan on the happy path — `output()` blocks until the child exits — but a
/// program that *hangs* while the test binary is killed is the same leak, and KI-29's lesson is
/// that nothing reports it when it happens. Cheap enough to make the invariant uniform: every
/// child a `cli` test starts dies with the test.
pub fn dies_with_parent(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The parent pid as the child should see it, read *before* the fork so the check below
        // can notice that it changed.
        let parent = std::process::id();
        // SAFETY: between fork and exec only async-signal-safe calls are allowed. `prctl`,
        // `getppid` and `_exit` all are; nothing here allocates or takes a lock.
        unsafe {
            cmd.pre_exec(move || {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                // Close the one hole: if the parent died in the window between the fork and
                // that `prctl`, the signal was already missed and will never arrive. Then we
                // are the orphan KI-29 is about, so leave instead.
                if libc::getppid() != parent as libc::pid_t {
                    libc::_exit(0);
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = cmd;
}

/// Run a `.blsp` program in a fresh `brood` subprocess.
pub fn spawn_brood(dir: &std::path::Path, name: &str, src: &str) -> BroodChild {
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
) -> BroodChild {
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
    dies_with_parent(&mut cmd);
    BroodChild(Some(cmd.spawn().expect("spawn brood")))
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
