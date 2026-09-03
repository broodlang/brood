//! The worker pool + run queue — process placement/stealing/migration and the
//! per-quantum execution loop. `enqueue`/`wake_enqueue` insert a ready process;
//! `try_steal*`/`assign_worker` (the latter in the root) balance across workers;
//! `ensure_workers`/`worker_loop`/`run_one`/`finish_quantum`/`handle_capture_outcome`/
//! `park_on_receive` drive a quantum and route its outcome. Split out of
//! `scheduler.rs`; the shared scheduling state (WORKERS/STEALABLE/counters/pid
//! tables, and TEST_NO_WORKERS) stays in the root, reached via `use super::*`, so
//! this is a pure relocation.
use super::*;

/// Push a ready process onto its owning worker's queue and wake that worker.
/// Preempt re-enqueue routes here so a hot process stays on its worker (cache
/// locality); a *woken-from-park* process may migrate instead — see [`wake_enqueue`].

/// `BROOD_SCHED_DBG=1`: trace every enqueue, quantum start (with the body's source
/// prefix) and quantum outcome, per pid. The tool that turned KI-88's "one reader of
/// fifty is lost" into "created, enqueued, RAN, and its quantum ended/hung <thus>" in
/// three runs — the per-pid lifecycle is otherwise invisible (counters aggregate).
/// Read once and cached; costs nothing when off.
/// KI-88's quantum ledger (armed by `BROOD_SCHED_DBG`): each entry is `(pid, started)`
/// for the quantum a thread is currently INSIDE — set after the `run` trace line,
/// cleared after `handle_capture_outcome`. The watchdog names any quantum older than
/// 3 s: cross-checked against a core dump, it distinguishes "drive() never returned on
/// thread T" (ledger holds the pid, T's stack shows where) from "the tail never ran"
/// (ledger holds the pid, NO thread shows it — the impossible case session 3 saw via
/// prints alone, now with a data structure that cannot race a line buffer).
static QUANTUM_LEDGER: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<std::thread::ThreadId, (u64, std::time::Instant)>>,
> = std::sync::LazyLock::new(Default::default);

fn ledger_enter(pid: u64) {
    if sched_dbg() {
        QUANTUM_LEDGER
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                std::thread::current().id(),
                (pid, std::time::Instant::now()),
            );
    }
}
fn ledger_exit() {
    if sched_dbg() {
        QUANTUM_LEDGER
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&std::thread::current().id());
    }
}
fn ledger_watchdog() {
    if !sched_dbg() {
        return;
    }
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::Builder::new()
            .name("quantum-watchdog".into())
            .spawn(|| loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let now = std::time::Instant::now();
                for (tid, (pid, t0)) in QUANTUM_LEDGER
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                {
                    let age = now.duration_since(*t0).as_secs();
                    if age >= 3 {
                        eprintln!("[sched] LEDGER: {tid:?} inside pid={pid} for {age}s");
                    }
                }
            })
            .ok();
    });
}

/// KI-88's OTHER half — the case the quantum ledger above cannot see. The ledger tracks a
/// process a thread is *inside*; a process that is enqueued and **never scheduled at all**
/// ("created, promoted, registered — never executes its first instruction") has no ledger
/// entry, no death line, and today surfaces only as a collector timeout thirty seconds
/// later, with the evidence gone.
///
/// The invariant this watches: work on a queue is found within one `STEAL_BACKOFF`, because
/// every parked worker re-probes on that timeout and `try_steal` scans every queue. So a
/// full find-nothing cycle — own queue empty, steal found nothing, nothing grace-deferred —
/// while `STEALABLE` says work exists is *individually* normal (a process can be mid-pull,
/// its decrement pending) but **cannot persist**: any progress anywhere resets the window in
/// `run_one`. Seconds of it mean pool-wide starvation with queued work, which is KI-88
/// witnessed live.
///
/// Default-ON, for KI-36's reason (a diagnostic you must arm before the bug is absent when
/// it matters — KI-88 has been sighted only in runs nobody had instrumented). The healthy-
/// path cost is two relaxed stores in `run_one` and one relaxed load on the park path,
/// which is already the cold path. The report names the stranded pids — the one fact every
/// previous sighting lacked — then latches until progress resumes.
///
/// The window is measured in wall time, not find-nothing cycles: a cycle count's meaning
/// scales with how many workers are parked (12 parked workers burn 512 cycles in ~0.4 s at
/// the 10 ms backoff, one worker takes 5 s), and a diagnostic whose trip point depends on
/// the core count is one you cannot reason about from a log.
static STRANDED_SINCE: AtomicU64 = AtomicU64::new(0);
static STRANDED_LATCH: AtomicBool = AtomicBool::new(false);
/// Far past any transient (a quantum is bounded in milliseconds), well before the suite's
/// 30 s collector timeout kills the evidence.
const STRANDED_REPORT_AFTER_NS: u64 = 3_000_000_000;

fn stranded_probe(reporter: usize) {
    if STEALABLE.load(Ordering::SeqCst) == 0 {
        STRANDED_SINCE.store(0, Ordering::Relaxed);
        return;
    }
    let now = now_nanos().max(1);
    let since = STRANDED_SINCE.load(Ordering::Relaxed);
    if since == 0 {
        // First find-nothing cycle with work queued: open the window. A lost race here
        // merely lets another prober open it a few nanoseconds later.
        let _ = STRANDED_SINCE.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
        return;
    }
    let age = now.saturating_sub(since);
    if age >= STRANDED_REPORT_AFTER_NS
        && STRANDED_LATCH
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    {
        let mut lines = String::new();
        for (wid, (lock, _)) in WORKERS.iter().enumerate() {
            // The reporter holds its own queue lock (the caller checked it empty), so
            // `try_lock` on it would report a phantom `<locked>`.
            let pids: Vec<u64> = if wid == reporter {
                Vec::new()
            } else {
                match lock.try_lock() {
                    Ok(q) => q.iter().map(|p| p.pid).collect(),
                    Err(_) => {
                        lines.push_str(&format!("  w{wid}: <locked>\n"));
                        continue;
                    }
                }
            };
            lines.push_str(&format!(
                "  w{wid}: parked={} dirty={} busy={} queue={pids:?}\n",
                WORKER_PARKED[wid].load(Ordering::Relaxed),
                WORKER_DIRTY[wid].load(Ordering::Relaxed),
                WORKER_BUSY[wid].load(Ordering::Relaxed),
            ));
        }
        eprintln!(
            "[sched] STRANDED WORK (KI-88 signature): stealable={} but no worker has found \
             anything to run for {} ms (live_executors={}, parked={}). Any pid listed below \
             that never runs is the stranded one — preserve this binary and the log.\n{lines}",
            STEALABLE.load(Ordering::SeqCst),
            age / 1_000_000,
            LIVE_EXECUTORS.load(Ordering::SeqCst),
            PARKED_COUNT.load(Ordering::Relaxed),
        );
    }
}

pub(crate) fn sched_dbg() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("BROOD_SCHED_DBG").is_some())
}

pub(crate) fn enqueue(mut proc: Box<Process>) {
    let wid = proc.worker_id;
    if sched_dbg() {
        eprintln!("[sched] enq pid={} wid={}", proc.pid, wid);
    }
    proc.queued_at = now_nanos();
    set_status(&proc.mailbox, ST_RUNNABLE); // queued, awaiting a worker turn
                                            // Count it as stealable runnable work (the `try_steal` fast-path hint). Balanced by
                                            // the single decrement in `run_one` when it's pulled to run (by its owner or a thief).
                                            // SeqCst (not Relaxed) so the dirty-block / drainer exhaustion checks reliably observe
                                            // this newly-queued work in the same total order as their `LIVE_EXECUTORS` updates —
                                            // the guarantee that work is never stranded with no live executor (see `dirty_block`).
    STEALABLE.fetch_add(1, Ordering::SeqCst);
    let (lock, cv) = &WORKERS[wid];
    crate::core::sync::lock(lock).push_back(proc);
    // Elide the wake syscall when we're enqueueing onto the very worker running THIS
    // thread (the direct-handoff case: a `send` readying a receiver, run next on our own
    // queue). `Condvar::notify_one` is an unconditional `futex_wake` syscall on Linux, but
    // the current worker can't be parked on its own cv while it's here executing the
    // enqueue — it will drain its own queue before it ever parks. On a `send`-then-`receive`
    // ping-pong this removes ~1 futex syscall per message (measured ~2/round-trip → ~0).
    // Any OTHER worker that might steal this process wakes via its own cv / steal-backoff,
    // not ours, so the elision is invisible to them.
    let on_current_worker = CURRENT_WORKER.with(|c| c.get()) == Some(wid);
    if !on_current_worker {
        cv.notify_one();
    }
    // If no executor is live to run this (every fixed worker is dirty-blocked), spawn an
    // on-demand drainer. Closes the window the per-block check can't see: work woken (a
    // timer fire, a cross-worker wake) *after* the last executor already blocked.
    if LIVE_EXECUTORS.load(Ordering::SeqCst) == 0 {
        spawn_overflow_drainer();
    }
}

/// Enqueue a process that is **waking from a park** (a `receive`/timer/exit wake, or a
/// message that raced its park). The **live-migration** point (ADR-100 §7): the process
/// was idle and has no native stack, so it may resume on any worker. Migrate **only when
/// the home worker is busy** — a single atomic load, vs an O(workers) `assign_worker`
/// scan on every wake. If home is idle it runs the woken process right away, so keep it
/// there (cache locality, and no scan on the hot receive/reply path — ~all of the per-wake
/// cost). Migrate (to the least-loaded worker) only when home is busy: re-queueing there
/// would sit behind the running process. This also covers a home worker stuck in a
/// **dirty** block — it reads busy (its `run_one` hasn't returned) and `assign_worker`
/// excludes it, so the woken process is moved off it. (Preempt re-enqueue uses plain
/// [`enqueue`] instead, to keep a hot process local.)
/// `BROOD_NO_HANDOFF=1` disables the direct-handoff wake policy in [`wake_enqueue`]
/// (reverts to waking the woken process's home worker). Cached — read once.
fn handoff_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("BROOD_NO_HANDOFF").is_some())
}

pub(crate) fn wake_enqueue(mut proc: Box<Process>) {
    // Direct handoff (BEAM-style, the message-passing latency win): when a *running
    // worker* wakes a process — the overwhelmingly common case is a `send` that readies
    // a parked receiver — enqueue it onto THIS worker's own run queue instead of waking
    // the receiver's (parked) home worker. The dominant message shape is
    // send-then-block (`(send q …)` immediately followed by `(receive …)`): running `q`
    // next on the same worker turns a cross-thread futex wake + OS context switch (~2 per
    // ping-pong round-trip, ~40% of wall in `sys`) into a userspace green-process switch.
    // `notify_one` on our own cv is a no-op (we're running, not parked), and the worker
    // loop drains its queue before parking, so `q` runs the moment our process yields.
    // Work-stealing still rebalances a genuinely parallel fan-out (an idle peer steals),
    // and reduction preemption bounds how long `q` waits if we DON'T block. The
    // root/timer threads (no `CURRENT_WORKER`) fall through to the load-based path.
    // `BROOD_NO_HANDOFF=1` opts out (the A/B / safety lever), reverting to the
    // wake-the-home-worker policy.
    if !handoff_disabled() {
        if let Some(cur) = CURRENT_WORKER.with(|c| c.get()) {
            proc.worker_id = cur;
            enqueue(proc);
            return;
        }
    }
    if WORKER_BUSY[proc.worker_id].load(Ordering::Relaxed) {
        let new_wid = assign_worker();
        if new_wid != proc.worker_id {
            MIGRATED.fetch_add(1, Ordering::Relaxed);
        }
        proc.worker_id = new_wid;
    }
    enqueue(proc);
}

/// Wake one parked worker so it can come and steal, skipping `wid` (the worker that just
/// enqueued). Gated on `PARKED_COUNT` so a busy pool pays a single relaxed load and issues
/// no syscall: if nobody is parked there is no one to tell, and the work will be picked up
/// by whichever worker frees up first.
///
/// **Called only from the spawn path**, deliberately. It was first wired into `enqueue`
/// itself, which also carries the *direct-handoff* wake (`send` readying a receiver, run
/// next on this very worker). That is the hot message path, and the handoff exists
/// precisely to avoid a cross-thread wake: adding one back cost `pingpong` 1.6×, `ring`
/// 2.2× and `supervisor` 4.9×. A send-woken receiver must stay where the handoff put it;
/// only a freshly *spawned* child is a candidate for an idle peer.
pub(crate) fn wake_a_parked_peer(wid: usize) {
    if steal_wake_disabled() || PARKED_COUNT.load(Ordering::Relaxed) == 0 {
        return;
    }
    let n = WORKERS.len();
    // Rotate the probe start so repeated spawns spread their wakes over the idle peers
    // instead of hammering the same one (which would serialise a fan-out onto one thief).
    let start = NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n;
    for off in 0..n {
        let peer = (start + off) % n;
        if peer == wid {
            continue;
        }
        if WORKER_PARKED[peer].load(Ordering::Relaxed) {
            // One `futex_wake`. The peer re-checks its own queue, then tries a steal; if it
            // loses the race (the owner drained it first) it simply parks again.
            WORKERS[peer].1.notify_one();
            return;
        }
    }
}

/// `BROOD_NO_STEAL_WAKE=1` disables the spawn-time peer wake, leaving idle workers to
/// discover stealable work on their own `STEAL_BACKOFF` re-probe (the pre-2026-08-02
/// behaviour). The A/B lever for attributing a latency or throughput change to the wake
/// itself rather than to the first-refusal grace. Read once.
fn steal_wake_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("BROOD_NO_STEAL_WAKE").is_some())
}

/// Steal one queued process from a backed-up peer's queue, re-assigning it to
/// `thief_wid`. Returns `None` if nothing is stealable. Since a process has no native
/// stack (state capture, ADR-100 §8.4), **any** process is safe to resume on the thief —
/// the cross-thread-resume hazard (KI-1b) that once forced fresh-only stealing is gone.
/// The queue handoff (`try_lock`) serialises ownership, so exactly one worker owns it at
/// a time (INV-2).
/// `saw_young` is set when a victim was skipped *only* because its back entry was still
/// inside its owner's first-refusal window. The caller uses it to re-probe after the grace
/// instead of parking for the full `STEAL_BACKOFF` — without it a thief woken to collect a
/// fresh child finds it too young, sleeps 10 ms, and the child waits for its owner anyway,
/// which is exactly the behaviour the grace was added to avoid.
fn try_steal(thief_wid: usize, saw_young: &mut bool) -> Option<Box<Process>> {
    // Fast path: nothing queued anywhere — re-park on one relaxed load.
    if STEALABLE.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let n = WORKERS.len();
    // Rotating start so thieves spread their probes across victims rather than all
    // hammering worker 0 (shares `NEXT_WORKER` with `assign_worker`; only
    // approximate rotation is needed).
    let start = NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n;
    for off in 0..n {
        let victim = (start + off) % n;
        if victim == thief_wid {
            continue; // don't steal from ourselves
        }
        // `try_lock`: never block a would-be thief on a contended victim — skip it
        // and try the next. A momentarily-locked queue just isn't probed this pass;
        // the `STEAL_BACKOFF` timeout brings us back.
        let mut q = match WORKERS[victim].0.try_lock() {
            Ok(q) => q,
            Err(_) => continue,
        };
        // Take from the back (the owner pops the front): the process the owner is
        // least likely to run next. `STEALABLE` is only a hint, so an empty queue
        // is normal here.
        // First refusal: leave a just-queued process to its owner for `STEAL_GRACE_NS`
        // (see there). Only the BACK entry is inspected — the one we would take — so this
        // is a single compare, and a queue whose back is too young is simply skipped this
        // pass rather than scanned.
        if q.back()
            .is_some_and(|p| now_nanos().saturating_sub(p.queued_at) < steal_grace_ns())
        {
            *saw_young = true;
            continue;
        }
        if let Some(mut proc) = q.pop_back() {
            drop(q);
            proc.worker_id = thief_wid; // re-assign: the thief owns it from now on
            STOLEN.fetch_add(1, Ordering::Relaxed);
            // `STEALABLE` is decremented by `run_one` (the single pulled-to-run
            // site the caller invokes next), not here — so the count stays balanced
            // whether a process is drained by its owner or stolen.
            return Some(proc);
        }
    }
    None
}

/// Steal one queued process from **any** worker's queue — the [`overflow_drain`] variant
/// of [`try_steal`] (no home worker to exclude). The stolen process keeps its existing
/// `worker_id` (a valid index, used only by `run_one` for the `WORKER_BUSY` load gauge),
/// since the drainer owns no queue of its own. Returns `None` if nothing is stealable.
fn try_steal_any() -> Option<Box<Process>> {
    if STEALABLE.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let n = WORKERS.len();
    let start = NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n;
    for off in 0..n {
        let victim = (start + off) % n;
        let mut q = match WORKERS[victim].0.try_lock() {
            Ok(q) => q,
            Err(_) => continue,
        };
        if let Some(proc) = q.pop_back() {
            drop(q);
            STOLEN.fetch_add(1, Ordering::Relaxed);
            return Some(proc); // keep its worker_id; STEALABLE decremented in run_one
        }
    }
    None
}

/// Spawn an on-demand overflow drainer (ADR-100 §7.4, dirty-scheduler growth): a transient
/// thread that runs any queued process until the pool is drained, then exits. Called when
/// a dirty-block or an `enqueue` finds no live executor left to run stranded work. A no-op
/// under the deterministic `TEST_NO_WORKERS` driver (it starts no threads and can't
/// dirty-block). The drainer is counted live (`LIVE_EXECUTORS`) **before** the thread
/// starts, so a racing second exhaustion check sees it and doesn't spawn a redundant one.
pub(crate) fn spawn_overflow_drainer() {
    // No OS threads under the deterministic test driver or on wasm (the cooperative pump
    // drives stranded work instead of an overflow thread — which would trap under wasm).
    if TEST_NO_WORKERS.load(Ordering::SeqCst) || cfg!(target_arch = "wasm32") {
        return;
    }
    LIVE_EXECUTORS.fetch_add(1, Ordering::SeqCst);
    OVERFLOW_DRAINERS.fetch_add(1, Ordering::SeqCst);
    let ok = std::thread::Builder::new()
        .stack_size(WORKER_STACK_BYTES)
        .spawn(overflow_drain)
        .is_ok()
        || std::thread::Builder::new().spawn(overflow_drain).is_ok();
    if !ok {
        // Couldn't start a thread — undo the accounting rather than leave it inflated
        // (which would suppress every future spawn and re-strand the work).
        OVERFLOW_DRAINERS.fetch_sub(1, Ordering::SeqCst);
        LIVE_EXECUTORS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An on-demand "dirty scheduler" thread: drain any worker's run queue (the same steal
/// path a fixed worker uses) until nothing is runnable, then exit. Spawned by
/// [`spawn_overflow_drainer`] when every executor is dirty-blocked and work is stranded.
/// Counts as a live executor while it runs (so a *further* exhaustion spawns another),
/// and can itself dirty-block inside `run_one` (a process it runs does a native-nested
/// receive) — that goes through `dirty_block` like any executor.
fn overflow_drain() {
    IS_EXECUTOR.with(|e| e.set(true));
    // (`LIVE_EXECUTORS` was already incremented by `spawn_overflow_drainer`.)
    loop {
        match try_steal_any() {
            Some(p) => run_one(p),
            None => {
                // Nothing runnable. Tentatively retire — but decrement-then-recheck, so a
                // process enqueued in the same instant (whose `enqueue` saw us still live
                // and skipped spawning) isn't stranded. Mirrors `dirty_block`'s ordering:
                // in the SeqCst total order, either we see the new work and stay, or that
                // `enqueue`'s `LIVE_EXECUTORS` load sees our decrement and spawns afresh.
                if STEALABLE.load(Ordering::SeqCst) != 0 {
                    std::thread::yield_now();
                    continue;
                }
                LIVE_EXECUTORS.fetch_sub(1, Ordering::SeqCst);
                if STEALABLE.load(Ordering::SeqCst) == 0 {
                    OVERFLOW_DRAINERS.fetch_sub(1, Ordering::SeqCst);
                    return;
                }
                LIVE_EXECUTORS.fetch_add(1, Ordering::SeqCst); // work raced in — keep draining
            }
        }
    }
}

/// Test-only: enable/disable [`TEST_NO_WORKERS`]. See [`test_drive_quanta`].
#[doc(hidden)]
pub fn set_test_no_workers(on: bool) {
    TEST_NO_WORKERS.store(on, Ordering::SeqCst);
}

/// Test-only: synchronously run up to `max` scheduling quanta on the **calling** thread
/// by popping worker 0's run queue and driving [`run_one`] (the real quantum logic,
/// including the preempt → re-enqueue-at-the-back path). Returns the number of quanta
/// actually run; stops early once the queue drains. This bounds a liveness test by
/// **work units, not wall-clock**, so it is fully deterministic. Pair with
/// [`set_test_no_workers`]`(true)` so no OS worker races the driving, and call it from a
/// **fresh** thread (not the spawner's), so `run_one`'s per-quantum ctx install doesn't
/// clobber the caller's process ctx / scheduling TLS.
#[doc(hidden)]
pub fn test_drive_quanta(max: usize) -> usize {
    let mut ran = 0;
    for _ in 0..max {
        let next = crate::core::sync::lock(&WORKERS[0].0).pop_front();
        match next {
            Some(p) => {
                run_one(p);
                ran += 1;
            }
            None => break,
        }
    }
    ran
}

/// The cooperative single-thread scheduler (wasm): with no OS workers, drive every queued
/// process to quiescence on the **calling** thread. Sweep all worker queues, running each
/// process a quantum via [`run_one`] (the real logic, so a `receive` park-captures and a
/// `send` re-enqueues its receiver onto some queue), and repeat until a whole sweep runs
/// nothing. A process that blocks forever in `receive` simply leaves the queues (parked on
/// its mailbox) and the sweep ends. This is [`test_drive_quanta`] generalised to all
/// workers and run to a fixpoint — the same "drive `run_one` off a non-worker thread"
/// pattern the deterministic test driver proves out.
#[cfg(target_arch = "wasm32")]
pub(crate) fn pump_until_quiescent() {
    loop {
        let mut ran_any = false;
        for wid in 0..WORKERS.len() {
            loop {
                // Bind the pop to a `let` so the queue guard drops before `run_one` — the
                // running process's receive/preempt re-enqueue re-locks this same queue.
                let next = crate::core::sync::lock(&WORKERS[wid].0).pop_front();
                match next {
                    Some(p) => {
                        run_one(p);
                        ran_any = true;
                    }
                    None => break,
                }
            }
        }
        if !ran_any {
            // A full sweep ran nothing. If a receive-timeout is pending, fire the earliest
            // (advance logical time) and keep going — a process waiting only on its `after`
            // clause makes progress. Otherwise every process is done or blocked forever.
            if crate::process::timer::fire_next_timer() {
                continue;
            }
            break;
        }
    }
}

/// Start the worker pool exactly once (on the first `spawn`).
pub(crate) fn ensure_workers() {
    WORKERS_STARTED.call_once(|| {
        // Force the WORKERS LazyLock to initialise *now*, with the pool size
        // committed by the current `set_max_parallel` (or the default ≈ nproc).
        // A later `set_max_parallel` won't resize the pool — sized once.
        let n = WORKERS.len();
        ACTIVE_WORKERS.store(n, Ordering::SeqCst);
        // Test hook: skip starting OS workers so a test can drive quanta itself
        // (`test_drive_quanta`). Inert in normal builds — the flag is never set.
        // On wasm there are no OS threads at all: `spawn`/`send`/`receive` are driven
        // cooperatively on the single thread by `pump_until_quiescent` (see there), so we
        // start no workers and never reach `thread::spawn` (which traps under wasm).
        if TEST_NO_WORKERS.load(Ordering::SeqCst) || cfg!(target_arch = "wasm32") {
            return;
        }
        // The pool's fixed workers are all live executors from the moment they're
        // started (each will run work). Seed the gauge here — not per-worker on entry —
        // so it's correct before the first `enqueue` can observe it (which would otherwise
        // see 0 and spawn a spurious startup drainer). Seeded with `n` and **corrected
        // below to the number that actually started**: an over-count is not cosmetic here,
        // because `enqueue`'s safety net only fires at `LIVE_EXECUTORS == 0`, so a gauge
        // stranded above reality means work can be queued with nothing alive to run it and
        // no drainer will ever be spawned (KI-97 item 3).
        LIVE_EXECUTORS.store(n, Ordering::SeqCst);
        // A process body runs directly on its worker thread (ADR-100 §8.4 — no coroutine
        // stack), and nested native / tree-walked sub-calls recurse here, so the worker
        // stack must be at least `stack_budget`'s reference size (`WORKER_STACK_BYTES`),
        // else a deep native recursion would overflow the default ~2 MiB thread stack
        // *before* the guard trips a clean error. The reservation is virtual/lazy.
        let mut live = 0usize;
        for wid in 0..n {
            let started = std::thread::Builder::new()
                .stack_size(WORKER_STACK_BYTES)
                .spawn(move || worker_loop(wid))
                .is_ok()
                // Retry once at the default stack size: the big reservation is the most
                // likely thing to be refused, and a worker with a small stack is far
                // better than no worker (a deep native recursion on it trips the stack
                // guard, which is a clean Brood error). `Builder::spawn` again, NOT
                // `thread::spawn` — the panicking variant here used to unwind inside
                // `call_once`, which both poisoned `WORKERS_STARTED` (every later
                // `ensure_workers` then panicked) and blew up whichever caller happened
                // to start the pool.
                || std::thread::Builder::new()
                    .name(format!("brood-worker-{wid}"))
                    .spawn(move || worker_loop(wid))
                    .is_ok();
            if started {
                live += 1;
            }
        }
        fault_stranded_work();
        if live != n {
            // Correct both gauges to reality, then say so: a pool smaller than requested
            // is a real degradation, and a silent one would present later as unexplained
            // latency under load.
            ACTIVE_WORKERS.store(live, Ordering::SeqCst);
            LIVE_EXECUTORS.store(live, Ordering::SeqCst);
            eprintln!(
                "brood: only {live} of {n} scheduler workers could be started; \
                 the runtime is running with a reduced pool"
            );
        }
    });
}

fn worker_loop(wid: usize) {
    CURRENT_WORKER.with(|c| c.set(Some(wid)));
    IS_EXECUTOR.with(|e| e.set(true));
    loop {
        // 1. Our own queue first (FIFO).
        //
        //    Bind the pop to a `let` so the queue `MutexGuard` is dropped at the
        //    end of *this statement*, BEFORE `run_one`. In edition 2021 a guard
        //    held in an `if let` scrutinee lives to the end of the whole block, so
        //    `if let Some(p) = lock(..).pop_front() { run_one(p) }` would hold the
        //    queue lock across the run — and the running process's preempt/receive
        //    re-enqueue (which re-locks this same queue) would deadlock the worker.
        let own = crate::core::sync::lock(&WORKERS[wid].0).pop_front();
        if let Some(p) = own {
            run_one(p);
            continue;
        }
        // 2. Nothing of our own: steal any queued process from a backed-up peer
        //    (every process is migratable — no native stack). See `try_steal`.
        let mut saw_young = false;
        if let Some(p) = try_steal(wid, &mut saw_young) {
            run_one(p);
            continue;
        }
        // 3. Nothing runnable anywhere we can reach: park on our condvar. We're
        //    woken immediately when a process is enqueued onto *our* queue, but
        //    NOT when a peer's queue grows — so park with a `STEAL_BACKOFF`
        //    backstop and re-attempt the steal on timeout. Re-check our own queue
        //    under the lock first to close the enqueue/park lost-wakeup window.
        let (lock, cv) = &WORKERS[wid];
        let q = crate::core::sync::lock(lock);
        if saw_young {
            // Work exists but is inside its owner's first-refusal window. Come back when
            // that expires rather than sleeping the full backoff: if the owner drains it
            // (the spawn-then-block shape) we find nothing and park normally; if the owner
            // is CPU-bound and still running, we take it a few microseconds after spawn.
            let _ = cv.wait_timeout(q, std::time::Duration::from_nanos(steal_grace_ns().max(1)));
            continue;
        }
        if q.is_empty() {
            // A full find-nothing cycle: own queue empty, steal empty, nothing deferred.
            // Feeds the stranded-work detector above; healthy runs reset it in `run_one`.
            stranded_probe(wid);
            // Publish that we are parked BEFORE releasing the queue lock in `wait_timeout`,
            // so an enqueuer that takes the lock after us is guaranteed to see the flag and
            // send the wake. Clearing it after we wake keeps the count honest.
            WORKER_PARKED[wid].store(true, Ordering::Relaxed);
            PARKED_COUNT.fetch_add(1, Ordering::Relaxed);
            let _ = cv.wait_timeout(q, STEAL_BACKOFF);
            WORKER_PARKED[wid].store(false, Ordering::Relaxed);
            PARKED_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Resume a process once, then either retire it (it finished) or, if it suspended
/// at `receive`, park it on its mailbox (or re-queue it if a message raced in).
fn run_one(proc: Box<Process>) {
    crate::perf_time!(ns_quantum, { run_one_timed(proc) })
}

fn run_one_timed(mut proc: Box<Process>) {
    if sched_dbg() {
        let body = match proc.body {
            crate::core::value::Value::Fn(id) => {
                let c = proc.heap.closure(id);
                let src = c
                    .arms
                    .first()
                    .and_then(|a| a.body.first())
                    .map(|&f| crate::syntax::printer::print(&proc.heap, f))
                    .unwrap_or_default();
                let src: String = src.chars().take(120).collect();
                format!(
                    "fn name={:?} env={:?} src={}",
                    c.name.map(crate::core::value::symbol_name_ref),
                    c.env,
                    src
                )
            }
            other => format!("{other:?}"),
        };
        eprintln!(
            "[sched] run pid={} wid={} resume={} body={}",
            proc.pid,
            proc.worker_id,
            proc.resume.is_some(),
            body
        );
    }
    let mailbox = Arc::clone(&proc.mailbox);
    let wid = proc.worker_id;
    // Pulled to run: the single `STEALABLE` decrement site, paired with the increment in
    // `enqueue`, whether its owner drained it or a thief stole it.
    STEALABLE.fetch_sub(1, Ordering::Relaxed);
    // Progress: something is running, so the pool is not stranded. Re-arm the detector.
    STRANDED_SINCE.store(0, Ordering::Relaxed);
    STRANDED_LATCH.store(false, Ordering::Relaxed);
    set_status(&mailbox, ST_RUNNING); // about to resume on this worker

    // Pure diagnostics (`peak_threads()`): no invariant needs a total order with other
    // atomics, so `Relaxed` is enough on this per-quantum path.
    let live = RUNNING.fetch_add(1, Ordering::Relaxed) + 1;
    PEAK_RUNNING.fetch_max(live, Ordering::Relaxed);
    // Mark this worker busy for `assign_worker`'s load metric while we're inside the run
    // (cleared in `finish_quantum`).
    WORKER_BUSY[wid].store(true, Ordering::Relaxed);
    // Fresh reduction budget for this scheduling quantum (decremented in the VM driver's
    // loop top via `tick_capture`; at zero the process captures + re-enqueues — preempt).
    REDUCTIONS.with(|r| r.set(reduction_budget()));

    // The worker drives the body's bytecode (`vm_run_bc`) directly — no coroutine — so a
    // paused process's continuation is relocatable heap data (`Suspended`) and can resume
    // on whichever worker `wake_enqueue` routes it to (live migration). No coroutine holds
    // the `Ctx`, so the worker installs it for the quantum (rebuilt each resume — the
    // worker multiplexes processes) and reads any capture-stack changes back afterwards.
    proc.install_ctx();
    SPAWNS_SINCE_PARK.with(|c| c.set(proc.spawns_since_park));
    set_capture_run(true);
    // Stall trace (BROOD_STALL_MS): a green-process quantum is bounded by the reduction
    // budget, so it should be quick — if one runs ≥ n ms, the time went into a blocking
    // builtin (terminal render, file I/O, sleep) or a long native call, NOT minor GC /
    // compaction (those have their own guards). Pinpoints a gameplay lag the GC guards miss.
    let _sg = {
        let pid = proc.pid;
        crate::core::heap::stall_guard_pid("quantum", pid)
    };
    ledger_enter(proc.pid);
    ledger_watchdog();
    let pid = proc.pid;
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc.drive()));
    drop(_sg);
    // The tail is caught too, not just `drive()`. A panic in HERE — a bad handle in
    // `save_ctx`/`store_resume`, an OOB slab index, a broken invariant in the outcome
    // routing — used to unwind straight through `worker_loop`, which has no catch: the
    // worker thread died for good (the pool silently shrank, and nothing restarts it)
    // and the unwind dropped the `Box<Process>` on the way out, so the process vanished
    // with no `deregister` — no death line, no monitors fired, no `[:down …]`. Anything
    // waiting on it then waited forever, so ONE tail panic hung the whole runtime.
    // Verified by fault injection: pre-fix, `BROOD_FAULT_QUANTUM_TAIL` on the chaos2
    // gen-server wedges the program at P47 until it is killed.
    //
    // That shape is also, exactly, KI-88's recorded signature (`run` with no `end`, the
    // ledger holding a pid no thread is inside, no death line, the collector timing out)
    // — which is why the tail is hardened here even though KI-88 itself is dormant and
    // was never caught with a panic message on stderr. This closes the mechanism rather
    // than diagnosing that bug: if the wedge is ever seen again, it is NOT this.
    let tail = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        fault_quantum_tail();
        set_capture_run(false);
        proc.save_ctx();
        // Read back before `handle_capture_outcome`, which routes a parking process into
        // `park_on_receive` — the one place that resets this.
        proc.spawns_since_park = SPAWNS_SINCE_PARK.with(|c| c.get());
        finish_quantum(&mailbox, wid);
        handle_capture_outcome(proc, &mailbox, outcome);
    }));
    if tail.is_err() {
        // Loud, durable, and — above all — survivable: this worker returns to its loop.
        let who = proc_descr(pid);
        eprintln!("process {who} lost: the scheduler's post-quantum tail panicked");
        crate::cli_support::dump_process_death(&who, "scheduler post-quantum tail panicked");
        // Retire it so monitors/links fire and nothing waits on it forever. Guarded by a
        // liveness check because the panic may have struck *after* `handle_capture_outcome`
        // already deregistered — `deregister` is not idempotent (it would re-fire every
        // watcher and double-count the live-process gauge). No heap: the unwind dropped it.
        if crate::process::mailbox::is_alive(pid) {
            deregister(pid, Message::Keyword(value::intern(pk::KILLED)), None);
        }
        // `finish_quantum`'s gauges (RUNNING / WORKER_BUSY) may be skewed if the panic
        // preceded it; both are documented as pure diagnostics, and squaring them up
        // blind would double-count the ordinary path.
    }
    ledger_exit();
}

/// Fault injection for the stranded-work watchdog (`BROOD_FAULT_STRANDED=1`): over-count
/// `STEALABLE` by one at pool start, so the pool believes a process is queued that no
/// worker can ever find — which is precisely what a stranded process looks like from the
/// probe's side. Exists so `stranded_probe` is *testable*: the failure it watches for has
/// never been provoked on demand (KI-88 is dormant), and a detector nobody has seen fire is
/// indistinguishable from one that cannot. See `crates/cli/tests/stranded_watchdog.rs`.
fn fault_stranded_work() {
    if std::env::var_os("BROOD_FAULT_STRANDED").is_some() {
        STEALABLE.fetch_add(1, Ordering::SeqCst);
        eprintln!("[sched] BROOD_FAULT_STRANDED: STEALABLE over-counted by one at pool start");
    }
}

/// Fault injection for the quantum tail (`BROOD_FAULT_QUANTUM_TAIL=<n>`): panic on the
/// `n`th quantum, in the window between `drive()` returning and the outcome being routed.
/// Exists so the hardening above is *testable* — the failure it guards against is a
/// permanently dead worker plus a silently destroyed process, which no ordinary input
/// provokes on demand. See `quantum_tail_panic_does_not_wedge_the_runtime`.
fn fault_quantum_tail() {
    static AT: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    let at = *AT.get_or_init(|| {
        std::env::var_os("BROOD_FAULT_QUANTUM_TAIL")
            .and_then(|v| v.to_string_lossy().parse::<u64>().ok())
    });
    if let Some(n) = at {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        if COUNT.fetch_add(1, Ordering::Relaxed) == n {
            panic!("BROOD_FAULT_QUANTUM_TAIL: injected panic on quantum {n}");
        }
    }
}

/// Shared post-quantum bookkeeping: drop the live-process gauge + worker-busy flag and
/// tally the reductions this quantum consumed (budget minus the remainder — a preempted
/// process left 0) into `process-info`'s `:reductions`. The quantum's eval shares this
/// worker's `REDUCTIONS` TLS, so its post-yield value is the remainder (Erlang counts
/// reductions the same way).
fn finish_quantum(mailbox: &Arc<Mailbox>, wid: usize) {
    RUNNING.fetch_sub(1, Ordering::Relaxed); // pure diagnostic — see `run_one`
    WORKER_BUSY[wid].store(false, Ordering::Relaxed);
    let used = reduction_budget().saturating_sub(REDUCTIONS.with(|r| r.get()));
    mailbox.reductions.fetch_add(used as u64, Ordering::Relaxed);
}

/// Handle a quantum's outcome (ADR-100 §8.3): `Done` retires `:normal`, an `Err` retires `[:error …]`,
/// `Killed` retires with the pending kill reason, `Preempted` stores the continuation
/// and re-queues (migrating), `Suspended` stores it and parks on the mailbox.
fn handle_capture_outcome(
    mut proc: Box<Process>,
    mailbox: &Arc<Mailbox>,
    outcome: std::thread::Result<Result<crate::eval::compile::VmOutcome, LispError>>,
) {
    use crate::eval::compile::VmOutcome;
    if sched_dbg() {
        let o = match &outcome {
            Ok(Ok(VmOutcome::Done(_))) => "done".to_string(),
            Ok(Ok(VmOutcome::Suspended(_))) => "suspended".to_string(),
            Ok(Ok(VmOutcome::Preempted(_))) => "preempted".to_string(),
            Ok(Ok(VmOutcome::Killed)) => "killed".to_string(),
            Ok(Err(e)) => format!("err:{}", e.message),
            Err(_) => "panic".to_string(),
        };
        eprintln!("[sched] end pid={} outcome={o}", proc.pid);
    }
    match outcome {
        Ok(Ok(VmOutcome::Done(_))) => {
            // A **soft** exit signal waits for the target's next `receive` — but a body
            // that ends without reaching one never runs that check, and retiring
            // `:normal` here DISCARDED the reason outright: `(exit (self) :badness)` in
            // a short body reported a clean exit, so links did not cascade and monitors
            // read `:normal`. The signal outlives the body, so take it as the reason;
            // absent one, the body really did finish normally.
            let pending = crate::core::sync::lock(&mailbox.state).kill.take();
            deregister(
                proc.pid,
                pending.unwrap_or_else(|| Message::Keyword(value::intern(pk::NORMAL))),
                Some(&proc.heap),
            );
        }
        Ok(Ok(VmOutcome::Suspended(s))) => {
            // Store the parked continuation in the process, then park (the
            // `receive`-boundary kill check + raced-message recheck).
            // `receive_match` already set `scanned` and armed any timer.
            proc.store_resume(s);
            park_on_receive(proc, mailbox);
        }
        Ok(Ok(VmOutcome::Preempted(s))) => {
            // Budget hit: stash the continuation and re-queue on the **same** worker
            // (`enqueue`, not `wake_enqueue`) — a hot, actively-running process stays
            // put for cache locality; migration is for *idle* (parked) processes.
            PREEMPTED.fetch_add(1, Ordering::Relaxed);
            proc.store_resume(s);
            enqueue(proc);
        }
        Ok(Ok(VmOutcome::Killed)) => {
            // A hard `:kill` was observed at a loop-top safepoint. Take its reason.
            let reason = crate::core::sync::lock(&mailbox.state)
                .kill
                .take()
                .unwrap_or_else(|| Message::Keyword(value::intern(pk::KILLED)));
            deregister(proc.pid, reason, Some(&proc.heap));
        }
        Ok(Err(e)) => {
            // An unwinding untrappable kill (`Control::Kill`) that no VM driver
            // converted — a tree-walked top-level body has no `Err`-arm conversion —
            // is a kill, not a crash: retire with the mailbox's pending reason, no
            // "process died" noise (a killed process is expected to die).
            if e.is_kill_signal() {
                let reason = crate::core::sync::lock(&mailbox.state)
                    .kill
                    .take()
                    .unwrap_or_else(|| Message::Keyword(value::intern(pk::KILLED)));
                deregister(proc.pid, reason, Some(&proc.heap));
                return;
            }
            // An uncaught throw/error killed the process (Erlang let-it-crash).
            // The death reason carries the STRUCTURED error — `[:error {:kind
            // :message … :trace}]`, see `message::error_reason` — so a monitor /
            // trapping link / supervisor gets BEAM's `{Reason, Stacktrace}`
            // rather than a flattened string.
            // …and durably, not only to stderr. KI-72 spent two sessions believing this
            // line was never printed: libtest discards a never-completing test's stderr,
            // so the one message naming the cause was written and thrown away. A TUI or
            // `nest run --watch` loses it the same way.
            let who = proc_descr(proc.pid);
            let why = e.located();
            // The one-liner yields to a subscriber listening for abnormal exits (the
            // crash reporter, ADR-305), whose report carries the trace; the durable
            // dump is written either way.
            if !crate::process::sysmon::crash_reported_elsewhere() {
                eprintln!("process {who} died: {why}");
            }
            crate::cli_support::dump_process_death(&who, &why.to_string());
            deregister(
                proc.pid,
                crate::process::message::error_reason(&e),
                Some(&proc.heap),
            );
        }
        Err(_) => {
            let who = proc_descr(proc.pid);
            eprintln!("process {who} panicked");
            crate::cli_support::dump_process_death(&who, "panicked");
            deregister(
                proc.pid,
                Message::Keyword(value::intern(pk::KILLED)),
                Some(&proc.heap),
            );
        }
    }
}

/// Park a process that suspended in `receive` (both execution modes). It scanned the
/// first `scanned` messages with no match: re-check under the lock — if a hard `:kill`
/// raced in, die; if a *new* (unscanned) message arrived during the suspend window,
/// re-queue to run again; otherwise park as the mailbox waiter for `send`/the timer to
/// wake. The state lock serialises this with `exit`'s waiter-take, so a process can't
/// end up parked-with-a-pending-kill and stuck forever.
fn park_on_receive(proc: Box<Process>, mailbox: &Arc<Mailbox>) {
    let mut st = crate::core::sync::lock(&mailbox.state);
    if mailbox.kill_pending.load(Ordering::Relaxed) {
        let reason = st
            .kill
            .take()
            .unwrap_or_else(|| Message::Keyword(value::intern(pk::KILLED)));
        drop(st);
        deregister(proc.pid, reason, Some(&proc.heap));
        // `proc` dropped here → its captured continuation + LOCAL heap are freed.
    } else if st.queue.len() > st.scanned {
        // A message raced in during the park — resume instead of parking. This is a
        // wake, so the process may migrate (`wake_enqueue`).
        drop(st);
        wake_enqueue(proc);
    } else if st
        .recv_deadline
        .is_some_and(|d| crate::process::timer::sched_now() >= d)
    {
        // The receive deadline elapsed inside the suspend→park window: the timer fired
        // before we got here, found no `waiter`, and consumed its (current-gen) entry, so
        // parking now would hang forever (nothing left to wake us). Re-queue instead — the
        // process re-scans, finds the deadline passed, and takes its `after` clause. The
        // timer-fire and this check both serialise on `mailbox.state`, so exactly one of
        // them re-queues: either the timer saw a `waiter`, or we see the passed deadline.
        //
        // Must read the **scheduler** clock ([`sched_now`]), not `Instant::now()`. On wasm
        // the two are different clocks: `recv_deadline` was computed as `sched_now() + ms`
        // on the FROZEN logical clock (there is no timer thread; `fire_next_timer` is the
        // only thing that advances it), so comparing it against real time declares the
        // deadline passed as soon as `ms` of wall-clock has elapsed. This branch would then
        // re-queue while `receive_match`'s own `sched_now() >= d` gate still says "not
        // yet" — suspend → park → re-queue forever, `pump_until_quiescent` seeing
        // `ran_any = true` every sweep so it never reaches `fire_next_timer` to advance
        // the clock. That is precisely the 100%-CPU spun tab the logical clock was added
        // to fix. On native `sched_now()` *is* `Instant::now()`, so nothing changes there.
        drop(st);
        wake_enqueue(proc);
    } else {
        // The process is genuinely parking: no message, no kill, no elapsed deadline. This
        // is the one moment we know it has nothing to do, so trim what it is holding —
        // collect its garbage and hand back retained slab capacity (see
        // `Heap::trim_parked`, which no-ops below its threshold so a small, frequently
        // parking responder is unaffected). Done under the mailbox lock, which is already
        // held and which a would-be sender needs anyway; the process owns its heap here
        // (no worker is running it), so the collection cannot race.
        let mut proc = proc;
        proc.trim_on_park();
        // It blocked, so whatever it spawned before now was a spawn-then-block: forget the
        // history that would have marked it CPU-bound (see `SPAWNS_SINCE_PARK`).
        proc.spawns_since_park = 0;
        st.waiter = Some(proc);
    }
}
