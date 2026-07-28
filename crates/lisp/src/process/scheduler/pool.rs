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
pub(crate) fn enqueue(proc: Box<Process>) {
    let wid = proc.worker_id;
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

/// Steal one queued process from a backed-up peer's queue, re-assigning it to
/// `thief_wid`. Returns `None` if nothing is stealable. Since a process has no native
/// stack (state capture, ADR-100 §8.4), **any** process is safe to resume on the thief —
/// the cross-thread-resume hazard (KI-1b) that once forced fresh-only stealing is gone.
/// The queue handoff (`try_lock`) serialises ownership, so exactly one worker owns it at
/// a time (INV-2).
fn try_steal(thief_wid: usize) -> Option<Box<Process>> {
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
    if TEST_NO_WORKERS.load(Ordering::SeqCst) {
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
        if TEST_NO_WORKERS.load(Ordering::SeqCst) {
            return;
        }
        // The pool's `n` fixed workers are all live executors from the moment they're
        // started (each will run work). Seed the gauge here — not per-worker on entry —
        // so it's correct before the first `enqueue` can observe it (which would otherwise
        // see 0 and spawn a spurious startup drainer).
        LIVE_EXECUTORS.store(n, Ordering::SeqCst);
        // A process body runs directly on its worker thread (ADR-100 §8.4 — no coroutine
        // stack), and nested native / tree-walked sub-calls recurse here, so the worker
        // stack must be at least `stack_budget`'s reference size (`WORKER_STACK_BYTES`),
        // else a deep native recursion would overflow the default ~2 MiB thread stack
        // *before* the guard trips a clean error. The reservation is virtual/lazy.
        for wid in 0..n {
            let started = std::thread::Builder::new()
                .stack_size(WORKER_STACK_BYTES)
                .spawn(move || worker_loop(wid))
                .is_ok();
            if !started {
                std::thread::spawn(move || worker_loop(wid));
            }
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
        if let Some(p) = try_steal(wid) {
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
        if q.is_empty() {
            let _ = cv.wait_timeout(q, STEAL_BACKOFF);
        }
    }
}

/// Resume a process once, then either retire it (it finished) or, if it suspended
/// at `receive`, park it on its mailbox (or re-queue it if a message raced in).
fn run_one(mut proc: Box<Process>) {
    let mailbox = Arc::clone(&proc.mailbox);
    let wid = proc.worker_id;
    // Pulled to run: the single `STEALABLE` decrement site, paired with the increment in
    // `enqueue`, whether its owner drained it or a thief stole it.
    STEALABLE.fetch_sub(1, Ordering::Relaxed);
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
    set_capture_run(true);
    // Stall trace (BROOD_STALL_MS): a green-process quantum is bounded by the reduction
    // budget, so it should be quick — if one runs ≥ n ms, the time went into a blocking
    // builtin (terminal render, file I/O, sleep) or a long native call, NOT minor GC /
    // compaction (those have their own guards). Pinpoints a gameplay lag the GC guards miss.
    let _sg = {
        let pid = proc.pid;
        crate::core::heap::stall_guard_pid("quantum", pid)
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc.drive()));
    drop(_sg);
    set_capture_run(false);
    proc.save_ctx();
    finish_quantum(&mailbox, wid);
    handle_capture_outcome(proc, &mailbox, outcome);
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
    match outcome {
        Ok(Ok(VmOutcome::Done(_))) => {
            deregister(
                proc.pid,
                Message::Keyword(value::intern(pk::NORMAL)),
                &proc.heap,
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
            deregister(proc.pid, reason, &proc.heap);
        }
        Ok(Err(e)) => {
            // An uncaught throw/error killed the process (Erlang let-it-crash).
            // The death reason carries the STRUCTURED error — `[:error {:kind
            // :message … :trace}]`, see `message::error_reason` — so a monitor /
            // trapping link / supervisor gets BEAM's `{Reason, Stacktrace}`
            // rather than a flattened string.
            eprintln!("process {} died: {}", proc_descr(proc.pid), e.located());
            deregister(
                proc.pid,
                crate::process::message::error_reason(&e),
                &proc.heap,
            );
        }
        Err(_) => {
            eprintln!("process {} panicked", proc_descr(proc.pid));
            deregister(
                proc.pid,
                Message::Keyword(value::intern(pk::KILLED)),
                &proc.heap,
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
        deregister(proc.pid, reason, &proc.heap);
        // `proc` dropped here → its captured continuation + LOCAL heap are freed.
    } else if st.queue.len() > st.scanned {
        // A message raced in during the park — resume instead of parking. This is a
        // wake, so the process may migrate (`wake_enqueue`).
        drop(st);
        wake_enqueue(proc);
    } else if st
        .recv_deadline
        .is_some_and(|d| std::time::Instant::now() >= d)
    {
        // The receive deadline elapsed inside the suspend→park window: the timer fired
        // before we got here, found no `waiter`, and consumed its (current-gen) entry, so
        // parking now would hang forever (nothing left to wake us). Re-queue instead — the
        // process re-scans, finds the deadline passed, and takes its `after` clause. The
        // timer-fire and this check both serialise on `mailbox.state`, so exactly one of
        // them re-queues: either the timer saw a `waiter`, or we see the passed deadline.
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
        st.waiter = Some(proc);
    }
}
