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

    let slice = port_slice(
        std::env::var("NEXTEST_TEST_GLOBAL_SLOT")
            .ok()
            .and_then(|s| s.parse().ok()),
        std::process::id(),
        SPAN / SLICE,
    );
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

/// Which of the `slices` port slices this test process draws from.
///
/// Under nextest every test runs in its own process, so two *concurrently running* tests
/// must never share a slice: both start at offset 0 of theirs (`NEXT` is per process), so a
/// shared slice hands both the SAME first port, and the loser's node fails to bind or — worse
/// — the winner's node answers the loser's dial with a cookie mismatch. Slicing by pid
/// almost always separates them, but not always: every `brood` child a test spawns burns
/// a pid per THREAD (~16 each), so under a full suite the pid counter advances by hundreds a
/// second and two live test processes can sit exactly `slices` apart (KI-99 is one sighting
/// of this class; KI-27 and KI-38 are its relatives). nextest publishes a slot number that is
/// unique among the tests running at any instant — `NEXTEST_TEST_GLOBAL_SLOT`, `0..threads`
/// — so that is the slice when present. The pid is the fallback for plain `cargo test`,
/// where one process runs every test and needs only one slice.
pub fn port_slice(nextest_slot: Option<u32>, pid: u32, slices: u16) -> u16 {
    let key = nextest_slot.unwrap_or(pid);
    (key % slices as u32) as u16
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
            panic!(
                "server never started listening on port {port}{}",
                stall_report(&format!("wait_until_listening({port}) gave up after 20 s"))
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A snapshot of what the box and every live `brood`/`nest` was doing at the instant a boot
/// wait gave up — see **KI-38**, and note this only ever runs on the failure path.
///
/// Why it exists. The three boot-wait tests (this helper, plus `wait_until_up` in
/// `child_cleanup.rs`) fail at 20–30 s, while a debug `brood` boot was measured at **151 ms
/// idle and 4066 ms worst case over ~14 600 samples taken during full suite runs, with
/// nothing above 5 s**. A 20 s failure is therefore not the slow tail of a loaded box; it is
/// a different mode. This prints the fields that separate the candidates, because a sighting
/// that arrives roughly once in eleven suite runs is too expensive to waste:
///
/// - **any thread `D`** — uninterruptible sleep, i.e. parked in a blocked syscall. That is
///   the stall shape, and the reported `wchan` then names where.
/// - **any thread `R`** — actually running, so it really was contention and the deadline is
///   the question after all. `cpu=` corroborates: sample twice and a contended child's
///   number moves.
/// - **no `brood` at all** — the child died rather than hung, which is a third thing again
///   and would be visible nowhere else, since these helpers only ever report a timeout.
///
/// **Read states PER THREAD, not per process (fixed 2026-08-10).** The first version read
/// `/proc/<pid>/stat`, which is the main thread only, and a `brood` runtime parks its root
/// thread on a futex while workers run — so in the KI-38 reproduction every process printed
/// `S futex_do_wait`, children burning CPU included, and all three cases above were
/// indistinguishable. The report was worse than useless: it looked like an answer.
///
/// Modelled on what KI-28 did: it armed "print B's stderr on failure", and that is exactly
/// what answered its open question when it recurred. Same move, one level down.
#[cfg(target_os = "linux")]
pub fn stall_report(what: &str) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "\n--- KI-38 stall report: {what} ---");
    let _ = writeln!(
        s,
        "loadavg: {}",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );
    if let Ok(mi) = std::fs::read_to_string("/proc/meminfo") {
        let grab = |k: &str| {
            mi.lines()
                .find(|l| l.starts_with(k))
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let _ = writeln!(s, "{} | {}", grab("MemAvailable"), grab("SwapFree"));
    }
    let _ = writeln!(
        s,
        "live brood/nest (pid states-by-thread cpu-ms wchan cmd):"
    );
    let mut n = 0;
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for e in entries.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|p| p.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) else {
                continue;
            };
            let cmd = cmdline.replace('\0', " ");
            if !is_brood_process(&cmd) {
                continue;
            }
            let (states, busiest) = thread_states(pid);
            let cpu_ms = process_cpu_ms(pid);
            let _ = writeln!(
                s,
                "  {pid} [{states}] cpu={cpu_ms}ms {busiest} {}",
                cmd.trim()
            );
            n += 1;
            if n >= 40 {
                let _ = writeln!(s, "  … truncated at 40");
                break;
            }
        }
    }
    if n == 0 {
        let _ = writeln!(s, "  (none — the child is gone, so this was not a stall)");
    }
    s
}

/// Is this cmdline an actual `brood`/`nest` BINARY, rather than anything that merely has
/// those letters in its path?
///
/// The first version tested `cmd.contains("/brood")`, which matches every test binary,
/// every `cargo` invocation and the invoking shell — because they all live under
/// `…/broodlang/brood/`. In the KI-38 reproduction the report was ~40 lines of test
/// harness with the two children that mattered buried inside it. Match on the executable
/// (argv[0]'s file name) instead.
#[cfg(target_os = "linux")]
fn is_brood_process(cmd: &str) -> bool {
    let exe = cmd.split_whitespace().next().unwrap_or("");
    let base = exe.rsplit('/').next().unwrap_or("");
    matches!(base, "brood" | "nest")
}

/// Every thread's state char, plus the `wchan` of the most interesting one.
///
/// **This is the fix that makes the report mean anything.** It used to read
/// `/proc/<pid>/stat`, which is the MAIN thread only — and a `brood` runtime parks its
/// root thread on a futex while worker threads do the work. So in the KI-38 reproduction
/// every process printed `S futex_do_wait`, including children burning CPU, and the three
/// candidates the report exists to separate (`D` blocked in a syscall / `R` running /
/// gone) were indistinguishable. Per-thread state distinguishes them: a runtime with any
/// `R` thread is running, one with a `D` thread is blocked in a syscall, and all-`S` with
/// flat CPU is genuinely idle.
///
/// Returns e.g. `("R,S,S,S", "run:-")` — the state chars in thread order, and
/// `state:wchan` for the first non-`S` thread (the one worth looking at), or the main
/// thread's when every thread is sleeping.
#[cfg(target_os = "linux")]
fn thread_states(pid: u32) -> (String, String) {
    let state_of = |stat: &str| -> String {
        // Split after comm's ')': comm can contain spaces, so field indexing before it is
        // unreliable — the same reason `alive()` in child_cleanup.rs does this.
        stat.rsplit_once(')')
            .and_then(|(_, r)| r.split_whitespace().next())
            .unwrap_or("?")
            .to_string()
    };
    let mut states = Vec::new();
    let mut interesting: Option<String> = None;
    if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
        let mut tids: Vec<u32> = tasks
            .flatten()
            .filter_map(|t| t.file_name().to_str().and_then(|s| s.parse().ok()))
            .collect();
        tids.sort_unstable(); // main thread (tid == pid) first, then stable order
        for tid in tids {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/stat")) else {
                continue;
            };
            let st = state_of(&stat);
            if st != "S" && interesting.is_none() {
                let wchan = std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/wchan"))
                    .unwrap_or_else(|_| "-".into());
                interesting = Some(format!("{st}:{}", wchan.trim()));
            }
            states.push(st);
        }
    }
    if states.is_empty() {
        return ("?".into(), "-".into());
    }
    let fallback = || {
        let wchan =
            std::fs::read_to_string(format!("/proc/{pid}/wchan")).unwrap_or_else(|_| "-".into());
        format!("S:{}", wchan.trim())
    };
    (states.join(","), interesting.unwrap_or_else(fallback))
}

/// Total CPU (utime + stime) this process has burned, in ms. A stalled child and a
/// contended one both look asleep in a single sample; CPU time separates them — sample the
/// report twice and a contended child's number moves while a stalled one's does not.
#[cfg(target_os = "linux")]
fn process_cpu_ms(pid: u32) -> u64 {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return 0;
    };
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return 0;
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After comm, fields are 1-indexed from `state`: utime is 12, stime 13 (procfs(5)
    // numbers them 14/15 counting pid and comm).
    let tick = |i: usize| f.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let hz = 100; // USER_HZ is 100 on every Linux target we run on
    (tick(11) + tick(12)) * 1000 / hz
}

#[cfg(not(target_os = "linux"))]
pub fn stall_report(_what: &str) -> String {
    String::new()
}

/// **The canonical boot-state fingerprint**: one line per global — name, kind, privacy,
/// declared signature, def site and dynamic-ness — preceded by the registry-name set.
///
/// It lives here because two differentials compare it and a third would have copied it. That
/// is the failure this module was created for ("fixed in one file and left in two"), and it
/// bites harder here than for a port helper: the whole point of a boot differential is that
/// the fingerprint covers every fact materialisation has to reproduce, so a fingerprint that
/// gains a field in one copy and not the other silently narrows the gate it belongs to.
///
/// The fields are exactly the side facts ADR-320 enumerates — the ones an image must carry
/// because the evaluation RECORDS them rather than binding them, each of which has been
/// omitted at least once and found late.
pub const STATE_DUMP: &str = r#"
(defn- dyn? (n)
  "Is `n` a dynamic variable? Asked behaviourally, through the primitive `binding` uses,
so this needs no new introspection surface."
  (try (do (%binding (list (symbol n)) [nil] (fn () nil)) true)
    (catch _ (check-allow :discarded-catch false))))

(defn- loc-of (n)
  "A def site as `[basename line col]`. The FULL path is deliberately dropped: each arm runs
under its own XDG_CACHE_HOME, so the materialised `prelude.blsp` lives at a different
absolute path in each, and comparing those compares the harness rather than the boot. The
basename plus line:col still fails on a missing, wrong or shifted def site — which is the
thing worth catching (an imaged boot that records none takes stdlib `M-.` down)."
  (let (l (reflect/source-location n))
    (if (nil? l)
      "nil"
      (let (parts (string/split (->string (first l)) "/"))
        (->string [(nth parts (- (count parts) 1)) (nth l 1) (nth l 2)])))))

(io/puts "REGISTRIES " (->string (%registry-names)))
;; The SIDE-FACT JOURNAL (ADR-320) — every fact the evaluation recorded ABOUT a name rather
;; than bound to it, rendered one per line by the kernel. Printed wholesale rather than
;; field-by-field on purpose: the per-global lines below are a hand-listed set of attributes,
;; which is the same shape of hand-maintained list the journal removed from the image writer,
;; and it had already gone stale — `meta` (ADR-283) has been carried since ADR-314 and
;; compared by nothing. A new fact kind now enters this comparison with no change here.
;;
;; Restricted to facts about a name this boot actually BOUND, and the reason is a real
;; divergence rather than a convenience. A stdlib image re-marks **every** `defdyn` in the
;; whole image at index-install time, before any section loads — deliberately, because a
;; section may be materialised at any moment and the mark has to precede it
;; (`startup_image.rs`, the v5 directory footer). So an imaged boot carries dynamic marks for
;; ~49 names in modules it never loaded (`*project-name*`, `*repl-prompt*`, `*test-filter*`),
;; and a boot with no artifacts does not. That difference is intended, documented and
;; monotonic, and it is invisible to the per-global lines below because those iterate bound
;; names only. Comparing UNBOUND facts would therefore fail the matrix on a design decision
;; instead of on a defect. What must still match exactly — and does — is every fact about a
;; name the boot bound, which is the whole of the state a program can observe.
(let (facts (filter (%side-facts)
              (fn (f) (bound? (symbol (nth (string/split f " ") 1))))))
  (io/puts "FACTS " (count facts))
  (doseq (f facts) (io/puts f)))
(let (names (sort (reflect/global-names)))
  (io/puts "GLOBALS " (count names))
  (doseq (n names)
    (io/puts n
             " kind=" (->string (type-of (reflect/eval (symbol n))))
             " private=" (->string (reflect/private? (symbol n)))
             " sig=" (->string (reflect/type-signature n))
             " loc=" (loc-of n)
             " dyn=" (->string (dyn? n)))))
"#;
