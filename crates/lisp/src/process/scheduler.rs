//! Green-process scheduler: the state-capture driver, the shared run queue,
//! the worker pool, and the public `spawn` / `self` / `pid-value` /
//! `spawn-count` / `peak-threads` / `set-max-parallel` surface.
//!
//! Each green process runs its 0-arg body's bytecode directly on a worker
//! thread (ADR-100 §8.4 — corosensei removed). `receive` on an empty mailbox
//! **captures** the process's continuation as relocatable heap data
//! (`Suspended`) and returns the worker to the pool, so a small pool of worker
//! OS threads (≈ `nproc`) multiplexes many processes — and a captured process,
//! carrying no native stack, may resume on *any* worker (live migration, §7).
//! The root thread (REPL / file runner) instead blocks on its mailbox condvar
//! (see [`super::mailbox::wait_for_message`]).
//!
//! ## Thread-locals
//! - [`CURRENT`] — the running process's [`Ctx`] (`pid`, `mailbox`, capture
//!   stack). Installed by `run_one` at the start of each quantum and read back
//!   after, so `(self)` / `receive` find their process even after the worker
//!   has run others, and survive migration to another worker.
//! - [`REDUCTIONS`] — countdown to the next preempt; [`tick`] decrements
//!   it from inside `eval`'s loop.
//! - [`GC_BLOCK`] — eval/macroexpand nesting depth; feeds the stack-overflow
//!   byte guard (no longer the GC safepoint — ADR-061). [`MACRO_BLOCK`] —
//!   compile-pass depth; the GC safepoint suppresses collection while it's
//!   nonzero. Both reset per quantum (each quantum runs on a fresh worker
//!   stack), so workers multiplexing several processes don't leak depths.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, Once};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::error::LispError;
use crate::process::keywords as pk;

use super::links;
use super::mailbox::{
    Mailbox, REGISTRY, ST_RUNNABLE, ST_RUNNING, ST_WAITING, book_cpu_ns, clear_parked,
    parked_count, pin_cpu_clock_to_this_thread, set_status,
};
use super::message::Message;
use super::monitor;

// Execution-safety guards (GC-block / macro-block depth + the stack-overflow byte
// guard) live in a child module; re-exported so `scheduler::…` (hence
// `process::…`, ADR consumers) resolves unchanged. `gc_block_set`/`macro_block_set`/
// `stack_base_set` are scheduler-internal (only `install_ctx` resets them).
mod guards;
pub use guards::{
    GcBlockGuard, MacroBlockGuard, NATIVE_STACK_MARGIN_BYTES, WORKER_STACK_BYTES, gc_block_depth,
    macro_block_active, native_stack_headroom_ok, stack_budget, stack_overflow_check,
};
use guards::{gc_block_set, macro_block_set, stack_base_set};

// Process lifecycle (spawn/exit/deregister) lives in a child module; the shared
// scheduling state stays in the root, reached from there via `use super::*`.
// Re-export the public surface so `scheduler::…`/`process::…` paths are unchanged.
mod lifecycle;
pub(super) use lifecycle::proc_descr;
use lifecycle::{deregister, retire_root_ctx};
pub use lifecycle::{
    exit, pending_kill_phrase, spawn, spawn_linked, spawn_monitored, spawn_root_program,
};
pub(crate) use lifecycle::{exit_propagate, exit_reason_phrase};

// The worker pool + run-queue execution loop lives in a child module; shared
// scheduling state stays in the root (reached via `use super::*`). Re-export the
// public/`process`-facing surface so `scheduler::…`/`process::…` paths are unchanged.
mod pool;
use pool::spawn_overflow_drainer;
pub(crate) use pool::{
    arm_cpu_accounting, enqueue, ensure_workers, quantum_cpu_so_far, wake_enqueue,
};
#[cfg(target_arch = "wasm32")]
pub(crate) use pool::{pump_due, pump_until_quiescent};
pub use pool::{set_test_no_workers, test_drive_quanta};

/// A green process (ADR-100 §8.4 — state capture, corosensei removed): its own `Heap`,
/// the 0-arg body thunk, and its parked/preempted continuation. The worker drives the
/// body's bytecode (`vm_run_bc`) directly — no coroutine — so a paused process is
/// **relocatable heap data** (`Suspended`), genuinely `Send`, and may resume on any
/// worker (live migration, §7). It is owned by exactly one worker at any instant: the
/// queue/waiter handoff serialises ownership (INV-2).
pub(super) struct Process {
    pub(super) pid: u64,
    pub(super) mailbox: Arc<Mailbox>,
    /// The worker currently owning this process. Re-assigned on a wake (`wake_enqueue`)
    /// or steal — safe because a process has no native stack to migrate (§7).
    pub(super) worker_id: usize,
    /// This process's LOCAL data heap — travels with it across workers.
    ///
    /// **Inline by value — don't box it.** `Heap` is most of `Process`'s size, but `Box`ing
    /// it adds a *second allocation per spawn*, which costs spawn throughput more than the
    /// few percent of idle memory it saves. Don't re-attempt it without a plan that avoids
    /// the extra allocation. Measure with `scripts/fuzz/stress/process_floor.blsp`.
    heap: Heap,
    /// The 0-arg body thunk (a shared-runtime `Fn` handle, valid in `heap`). Unused
    /// (`nil`) when `program` is set — a whole-program root process (ADR-135).
    body: Value,
    /// Set for the **root program process** (ADR-135): drives the top-level forms one at
    /// a time instead of a single body thunk, so `(self)` is one stable pid across the
    /// whole program and a top-level `receive` park-captures. `None` for an ordinary
    /// `spawn`ed process.
    program: Option<Box<crate::eval::compile::ProgramState>>,
    /// The parked/preempted continuation (the VM's or the tree-walker's), or `None` if not
    /// yet started.
    resume: Option<crate::eval::compile::Continuation>,
    /// The output-capture stack snapshot (the process carries it — no coroutine holds a
    /// `Ctx`). `run_one` installs it into `CURRENT` per quantum and reads it back after,
    /// so `begin_capture`/`take_capture` persist across `receive` suspends.
    capture: Vec<Arc<Mutex<String>>>,
    /// Monotonic nanos at which this process was queued, or 0 when it is unstamped. Read
    /// only by `try_steal`, to give the owning worker a brief **first refusal** on queued
    /// work (see `steal_grace_ns`). `enqueue` stamps a spawn or a preempted process; a
    /// woken one goes unstamped (`wake_enqueue`) and the first thief to see it stamps it.
    queued_at: u64,
    /// How many children this process has spawned since it last **parked** in `receive`.
    /// Preemption does not reset it — only actually blocking does — so it answers exactly
    /// one question at spawn time: *is this spawner going to yield soon?* See
    /// `SPAWNS_SINCE_PARK`.
    spawns_since_park: u32,
}

/// What a running process needs to find from deep inside `eval` (for
/// `receive`/`self`). Stored in a thread-local, installed by `run_one` at the start of
/// each quantum and read back after (so it survives the worker multiplexing other
/// processes, and migration to another worker).
#[derive(Clone)]
pub(super) struct Ctx {
    pub(super) pid: u64,
    pub(super) mailbox: Arc<Mailbox>,
    /// The **output-capture stack**. Empty means no capture; output goes to real
    /// stdout. When non-empty, this process's `print` / terminal output appends to
    /// the **top** buffer instead (see builtins' `capture_write`). A *stack* so
    /// captures **nest** (`with-out-str` inside an MCP `tools/call` drains only its
    /// own buffer). A SPAWNED child **inherits** a snapshot of the parent's stack (the
    /// same `Arc`s), so a whole process tree stays diverted. Each `Arc` is minted fresh
    /// per `begin_capture`. Rides `CURRENT`, so it survives suspend for free.
    pub(super) capture: Vec<Arc<Mutex<String>>>,
}

thread_local! {
    pub(super) static CURRENT: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

thread_local! {
    /// The `CURRENT` a quantum displaced, so `save_ctx` can put it back rather than
    /// clearing it. On a worker there is nothing to restore, but on wasm the run queue
    /// is pumped on the CALLING thread, whose own context (holding the playground's
    /// stdout capture) a quantum must not erase (KI-115). A stack, so a re-entered pump
    /// cannot lose a frame.
    static DISPLACED: RefCell<Vec<Option<Ctx>>> = const { RefCell::new(Vec::new()) };
}

// ----- reduction-counted preemption: `quantum` ------------------------------------

mod quantum;
pub(crate) use quantum::*;
pub use quantum::{tick, yield_now};

thread_local! {
    /// True while this worker is driving a green process body (`run_one`). The
    /// discriminator the `receive` path and the VM driver use to decide "capture the
    /// continuation" vs. "block the root": a green process running here can capture,
    /// while the root thread (which never enters `run_one`) must block on its mailbox.
    /// Set true around the `run_process_body` call, restored after (the worker
    /// multiplexes other processes between quanta).
    static CAPTURE_RUN: Cell<bool> = const { Cell::new(false) };
}

/// Are we inside a capture-mode green-process body run (ADR-100 §8)? The `receive`
/// suspend gate and the VM driver's loop-top preempt/kill capture both key off this.
pub(crate) fn in_capture_run() -> bool {
    CAPTURE_RUN.with(|c| c.get())
}

/// Set/clear the capture-run flag around a `run_process_body` call (`run_one`).
/// `pub(crate)` so the JIT's tests can simulate a green-process (preemptible) context.
pub(crate) fn set_capture_run(on: bool) {
    CAPTURE_RUN.with(|c| c.set(on));
}

thread_local! {
    /// This thread's worker id, set once at `worker_loop` entry; `None` off a worker
    /// (the root thread). Lets a worker mark *itself* dirty-blocked when it parks in a
    /// native-nested receive.
    static CURRENT_WORKER: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Per-worker "dirty-blocked" flag (ADR-100 §7.4): set while a worker is parked inside
/// a **native-nested** capture `receive` (the dirty-scheduler carve-out — it blocks the
/// thread, never returning to its run loop). A dirty worker is excluded from
/// `assign_worker` and its movable backlog is re-routed, so no process is stranded on a
/// worker that won't run it. Sized to match `WORKERS`.
static WORKER_DIRTY: LazyLock<Vec<AtomicBool>> =
    LazyLock::new(|| (0..WORKERS.len()).map(|_| AtomicBool::new(false)).collect());

/// Mark the current worker dirty-blocked and re-route its backlog, returning a guard that
/// clears the flag on drop. A no-op off a worker thread (the root, which owns no queue).
/// Called by the native-nested `receive` block (`wait_for_message`).
pub(crate) fn dirty_block() -> DirtyBlockGuard {
    if !IS_EXECUTOR.with(|e| e.get()) || CPU_DIRTY.with(|d| d.get()) {
        // Root thread (or any non-executor): it blocks on its own mailbox but owns no
        // queue and runs no other process, so it strands nothing and isn't a vanishing
        // executor — no accounting, no re-route. A worker already CPU-dirty
        // ([`cpu_dirty_enter`]) was accounted when it went dirty.
        return DirtyBlockGuard {
            executor: false,
            wid: None,
        };
    }
    let wid = CURRENT_WORKER.with(|c| c.get());
    leave_executors(wid);
    DirtyBlockGuard {
        executor: true,
        wid,
    }
}

/// Take this executor out of the pool that can run queued work: a native frame holds it
/// and it will not return to its run loop until that frame does. Shared by both ways a
/// worker goes dirty — blocked in a native-nested `receive` ([`dirty_block`]) and busy in a
/// native-nested run ([`cpu_dirty_enter`]).
fn leave_executors(wid: Option<usize>) {
    if let Some(wid) = wid {
        // A fixed worker: exclude it from `assign_worker` and re-route its backlog so a
        // *live* idle peer can steal it within `STEAL_BACKOFF` without spawning a drainer.
        WORKER_DIRTY[wid].store(true, Ordering::Relaxed);
        drain_worker_queue(wid);
    }
    // One fewer executor can run anything. If that was the *last* live one and work is
    // queued behind the blocks, no fixed worker can reach it (a dirty worker won't steal,
    // and an idle one would — but there is none) — so spawn an on-demand drainer.
    let remaining = LIVE_EXECUTORS.fetch_sub(1, Ordering::SeqCst) - 1;
    if remaining == 0 && STEALABLE.load(Ordering::SeqCst) > 0 {
        spawn_overflow_drainer();
    }
}

/// [`leave_executors`] undone: the native frame returned and this thread goes back to the
/// process it was running.
fn rejoin_executors(wid: Option<usize>) {
    if let Some(wid) = wid {
        WORKER_DIRTY[wid].store(false, Ordering::Relaxed);
    }
    // This thread goes back to the process it was running, not to the queues, so if
    // a refused thread stranded queued work, now is the chance to start one for it.
    pool::retry_stranded_work();
    LIVE_EXECUTORS.fetch_add(1, Ordering::SeqCst);
}

thread_local! {
    /// True while this executor is CPU-dirty ([`cpu_dirty_enter`]): a native-nested run on
    /// it spent a quantum with other work waiting, and it is out of the pool until the run
    /// returns to a frame that can capture.
    static CPU_DIRTY: Cell<bool> = const { Cell::new(false) };
}

/// A native-nested run — Brood under a builtin's frame (`reflect/eval-string`, `%isolate`,
/// a native callback) — spent its quantum while other work waits. It cannot yield: the
/// native frame cannot be captured. Without this the worker stays counted as live, so
/// nothing replaces it, and as many such runs as there are workers starve every other
/// process for as long as they run — including whichever one was going to kill them. A
/// two-worker suite sat 30 minutes with both workers looping under `eval`. So it goes
/// dirty, as a worker blocked in a native-nested `receive` does: its queue moves to live
/// workers, and the last live executor going dirty starts an overflow drainer (a dirty
/// CPU scheduler, at the moment the run proves long). Until [`cpu_dirty_leave`].
pub(crate) fn cpu_dirty_enter() {
    if !IS_EXECUTOR.with(|e| e.get()) || CPU_DIRTY.with(|d| d.replace(true)) {
        return;
    }
    leave_executors(CURRENT_WORKER.with(|c| c.get()));
}

/// The CPU-dirty run returned to a frame that can capture (or its process left the
/// worker): rejoin the pool. A no-op on a worker that is not CPU-dirty.
pub(crate) fn cpu_dirty_leave() {
    if CPU_DIRTY.with(|d| d.replace(false)) {
        rejoin_executors(CURRENT_WORKER.with(|c| c.get()));
    }
}

/// Restores the executor accounting when a native-nested receive's blocking wait returns:
/// clears the worker's dirty flag and re-counts it as a live executor.
pub(crate) struct DirtyBlockGuard {
    executor: bool,
    wid: Option<usize>,
}
impl Drop for DirtyBlockGuard {
    fn drop(&mut self) {
        if self.executor {
            rejoin_executors(self.wid);
        }
    }
}

/// Re-route **every** queued process off a dirty worker. A worker stuck in a native-nested
/// block won't return to its run loop, so anything queued behind it is stranded — and
/// since every process is now migratable (no native stack), the simplest correct move is
/// to push them all elsewhere (the mass-kill/monitor deadlock fix). `assign_worker`
/// already excludes `wid`, so they land on live workers.
fn drain_worker_queue(wid: usize) {
    // Every queued process is stranded on this dirty worker (it won't return to its run
    // loop) and is migratable (no native stack), so re-route them all off it.
    let stranded: Vec<Box<Process>> = crate::core::sync::lock(&WORKERS[wid].0.0)
        .drain(..)
        .collect();
    // These left a queue without going through `run_one` (the usual decrement site), so
    // account for the removal here; the `enqueue` below re-adds one each. Net zero — the
    // `STEALABLE` count stays equal to the processes actually sitting in queues.
    STEALABLE.fetch_sub(stranded.len(), Ordering::Relaxed);
    for mut proc in stranded {
        // Force off this (dirty) worker directly — `assign_worker` already excludes a
        // dirty worker, so this lands elsewhere. (Not `wake_enqueue`: its "migrate only
        // when home is busy" heuristic would depend on `WORKER_BUSY[wid]` being set,
        // which it is during a block, but relying on that coupling here is fragile —
        // the whole point is that nothing must stay on a worker that won't run it.)
        proc.worker_id = assign_worker();
        enqueue(proc);
    }
}

thread_local! {
    /// True while the **innermost** `vm_run_bc` is the *top-level* body driver — i.e.
    /// the running `receive` is reached purely through bytecode, with no native frame
    /// (a `%isolate`/`%try`/`map` callback) between it and the driver. A clean
    /// top-level receive can capture its continuation (and migrate); a **native-nested**
    /// receive cannot (the native frame can't be captured, and re-running it repeats
    /// side effects — the §8.1 footgun), so it falls back to **blocking** its worker
    /// like a dirty scheduler (§7.4). Set per `vm_run_bc` entry to that call's
    /// `top_level` and restored on exit, so it reflects the innermost driver.
    static CAPTURE_TOP_LEVEL: Cell<bool> = const { Cell::new(false) };
}

/// Is the innermost capture-mode VM driver the top-level body driver (so a `receive`
/// here is bytecode-reachable and may capture), as opposed to nested under a native
/// (must block instead)? See [`CAPTURE_TOP_LEVEL`].
pub(crate) fn capture_top_level() -> bool {
    CAPTURE_TOP_LEVEL.with(|c| c.get())
}

/// Set the top-level-driver flag, returning the previous value (so `vm_run_bc` can
/// restore it on exit — nested runs set it `false`, the outer run restores `true`).
pub(crate) fn set_capture_top_level(on: bool) -> bool {
    if on {
        // The innermost frames can capture again: whatever native frame held this worker
        // has returned, so a CPU-dirty worker rejoins the pool.
        cpu_dirty_leave();
    }
    CAPTURE_TOP_LEVEL.with(|c| c.replace(on))
}

/// RAII guard marking "the innermost frames are TREE-WALKED": clears
/// [`CAPTURE_TOP_LEVEL`] for its lifetime, restoring the previous value on drop.
///
/// A tree-walker frame is a *native* frame for capture purposes — the capture
/// driver can't reify it — so a `receive` reached through one must take the
/// blocking (§7.4 dirty-scheduler) path, never the capture path — capturing across
/// un-reifiable TW frames makes the resume **re-run the whole native thunk**, repeating
/// its side effects (the §8.1 footgun). Entered at the tree-walker's two entry points
/// (`eval::eval`, `eval::apply`); nested entries are cheap no-op re-clears.
pub(crate) struct TreeWalkGuard(bool);

impl TreeWalkGuard {
    pub(crate) fn enter() -> Self {
        TreeWalkGuard(set_capture_top_level(false))
    }
}

impl Drop for TreeWalkGuard {
    fn drop(&mut self) {
        set_capture_top_level(self.0);
    }
}

/// Is the current process's mailbox overflow waiting to be raised? A peek for the native
/// loop's poll; `take_current_mailbox_overflow` is what raises it.
#[cfg(feature = "jit")]
pub(crate) fn current_mailbox_overflow_pending() -> bool {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|ctx| ctx.mailbox.overflow_pending())
    })
}

/// Mailbox-bound probe for the CURRENT process (ADR-307) — the safepoint twin of the
/// `receive`-entry check, for a flooded process that computes instead of receiving.
/// Probed beside `take_proc_limit_hit` at the same loop-top safepoints; one borrow +
/// one relaxed load when clean, `None` always on the root thread (CURRENT unset),
/// matching the kill probe's root rule.
pub(crate) fn take_current_mailbox_overflow() -> Option<(usize, usize)> {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .and_then(|ctx| ctx.mailbox.take_overflow_hit())
    })
}

/// Is an untrappable hard `:kill` pending for the current process? The driver checks this
/// at a loop-top safepoint and stops. A *soft* exit isn't honoured here — it waits for
/// the next `receive` (checked when `run_one` would park).
pub(crate) fn capture_hard_kill_pending() -> bool {
    CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|ctx| ctx.mailbox.pending_hard_kill())
    })
}

/// Has the current green process been sent an exit signal it will die of, hard or soft?
/// What a native call that waits on the outside world (`%os-cmd`'s child) checks between
/// waits, as a blocked native-nested `receive` does when it wakes: the call is the
/// process's wait, so a soft exit is due there too. Never true on the root thread, which
/// is not killable.
pub(crate) fn capture_kill_pending() -> bool {
    in_capture_run()
        && CURRENT.with(|c| {
            c.borrow()
                .as_ref()
                .is_some_and(|ctx| ctx.mailbox.kill_pending.load(Ordering::Relaxed))
        })
}

// ----- the run queue + worker pool -------------------------------------------

pub(super) static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static SPAWNED: AtomicU64 = AtomicU64::new(0);
/// How many processes have been work-stolen across worker threads since program start
/// (read by `(%)`). A diagnostic of how much rebalancing the scheduler actually
/// did — 0 means placement-at-spawn kept the pool even and no thief ever needed to pull
/// work.
static STOLEN: AtomicU64 = AtomicU64::new(0);
/// How many times a process was re-assigned to a *different* worker when woken from a
/// park (`receive`/timer/exit) — i.e. a live migration of a mid-computation continuation
/// across worker threads (ADR-100 §7). Read by the live-migration regression test as
/// direct evidence that captured continuations actually crossed threads.
static MIGRATED: AtomicU64 = AtomicU64::new(0);

/// The pid that spawned `pid`, or `None` for the root process (or a dead pid). Backs
/// `process-info`'s `:parent`.
pub fn parent_of(pid: u64) -> Option<u64> {
    // The parent rides the process's own mailbox (`Mailbox::parent`) — readable while
    // the `Process` runs, no lock, nothing to clean up on exit. `0` is the root's
    // "no parent" sentinel.
    REGISTRY
        .get(pid)
        .map(|mb| mb.parent.load(Ordering::Relaxed))
        .filter(|&p| p != 0)
}

/// Hands out `%isolate` scope tokens. Starts at 1 so `0` stays "no isolate", and only ever
/// increments, so a token is never reused and a stale stamp can never match a live isolate.
static NEXT_ISOLATE_TOKEN: AtomicU64 = AtomicU64::new(1);

/// A fresh `%isolate` scope token.
pub fn next_isolate_token() -> u64 {
    NEXT_ISOLATE_TOKEN.fetch_add(1, Ordering::Relaxed)
}

/// The scope this process stamps on children it spawns (`0` = not inside any isolate).
pub fn self_isolate_scope() -> u64 {
    with_ctx(|ctx| ctx.mailbox.isolate_scope.load(Ordering::Relaxed))
}

/// Set the scope stamped on this process's future children, returning the previous one.
/// `%isolate` brackets its thunk with this; nothing else should call it.
pub fn set_self_isolate_scope(scope: u64) -> u64 {
    with_ctx(|ctx| ctx.mailbox.isolate_scope.swap(scope, Ordering::Relaxed))
}

/// The `%isolate` scope `pid` was spawned under, or `0` (also for a pid that has since
/// exited — an unknown process is not owned by anyone).
pub fn isolate_owner_of(pid: u64) -> u64 {
    REGISTRY
        .get(pid)
        .map(|mb| mb.isolate_owner.load(Ordering::Relaxed))
        .unwrap_or(0)
}

static RUNNING: Padded<AtomicUsize> = Padded(AtomicUsize::new(0)); // processes inside `resume` right now
static PEAK_RUNNING: Padded<AtomicUsize> = Padded(AtomicUsize::new(0));
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0); // 0 = default (≈ nproc)
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0); // worker threads actually started
static WORKERS_STARTED: Once = Once::new();

/// Executor threads (fixed workers + on-demand overflow drainers) that are **not**
/// currently dirty-blocked — i.e. the count able to run a runnable process *right now*.
/// A native-nested `receive` blocks its executor thread (§7.4); when every executor is
/// blocked this hits 0 and any work queued behind the blocks is stranded (no live thread
/// to drain or steal it). The scheduler then spawns an [`overflow_drain`] thread — an
/// on-demand "dirty scheduler" — so progress is
/// always possible. Incremented when an executor starts running and on a dirty-block
/// wake; decremented when an executor dirty-blocks and when an overflow drainer exits.
static LIVE_EXECUTORS: AtomicUsize = AtomicUsize::new(0);
/// Overflow drainer threads alive right now (diagnostics + a spawn-churn guard).
static OVERFLOW_DRAINERS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// True on a thread that runs green processes (a fixed worker or an overflow
    /// drainer) — so [`dirty_block`] does the [`LIVE_EXECUTORS`] accounting for it. The
    /// **root** thread (which blocks on its own mailbox but owns no queue and runs no
    /// other process) leaves this `false`: its block strands nothing and must not be
    /// counted as a vanishing executor.
    static IS_EXECUTOR: Cell<bool> = const { Cell::new(false) };
}

/// One worker's run queue + the condvar that parks it when the queue is empty.
type WorkerQueue = (Mutex<VecDeque<Box<Process>>>, Condvar);

/// Per-worker run queues. Index = `worker_id`. A worker drains its own queue
/// first; when empty, it may **steal any queued process** from a backed-up peer
/// (`try_steal`) — every process is migratable now that continuations are captured
/// to the heap (ADR-100 §8.4), so there's no pinning. Preempt re-enqueues to the
/// same worker (keep a hot process local); a wake may migrate (`wake_enqueue`). The
/// Vec is sized once at the first `ensure_workers` from `worker_count()`, then never
/// resized.
///
/// Each queue sits on its own cache line ([`Padded`]): a queue is ~50 bytes, so unpadded two
/// workers shared a line and every push/pop/steal on one invalidated the other's. Hence the
/// `WORKERS[i].0.0.0` (the lock) / `.0.1` (the condvar) spelling at the use sites.
static WORKERS: LazyLock<Vec<Padded<WorkerQueue>>> = LazyLock::new(|| {
    (0..worker_count())
        .map(|_| Padded((Mutex::new(VecDeque::new()), Condvar::new())))
        .collect()
});

/// A value on its own cache line (128 bytes: two lines, for the adjacent-line prefetcher).
/// For the scheduler's shared counters and per-worker flags, which every worker writes per
/// quantum or per park: packed together, one worker's store invalidates the line its peers
/// are reading an unrelated counter from.
#[repr(align(128))]
pub(crate) struct Padded<T>(pub(crate) T);

impl<T> std::ops::Deref for Padded<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

/// Count of processes currently sitting in some worker's queue — i.e. the pool of
/// stealable work. Incremented in `enqueue` (every queueing: spawn, wake, preempt)
/// and decremented in `run_one` (the single pulled-to-run site, whether the owner
/// drained it or a thief stole it).
/// A cheap, relaxed atomic gate: an idle worker checks it before scanning peer
/// queues, so a truly-idle pool (`STEALABLE == 0`) re-parks on one atomic load
/// instead of an O(workers) scan. May briefly over-count a process popped but
/// not yet in `run_one` (a wasted scan, self-correcting) — it is a hint, never a
/// correctness gate.
static STEALABLE: Padded<AtomicUsize> = Padded(AtomicUsize::new(0));

/// How long an idle worker parks before re-attempting a steal, when it has no
/// work of its own. A backstop, not the primary wakeup: a worker is woken
/// immediately when a process is enqueued onto *its* queue (a preempt re-enqueue
/// or a spawn placed here), but it is *not* notified when a **peer's** queue
/// grows — so it re-checks for stealable work every `STEAL_BACKOFF`. This is the
/// cadence while work exists *somewhere* (`STEALABLE > 0`); see `IDLE_BACKOFF_MAX` for
/// the empty pool.
const STEAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(10);

/// The park backstop once `STEALABLE == 0` — a re-probe has nothing to find, so the
/// timeout buys only the wake, and a 10 ms wake on every worker cost an idle runtime
/// 0.25% of a core per worker. A worker doubles its backstop from `STEAL_BACKOFF` up to
/// this bound while it keeps finding an empty pool, and drops straight back the moment it
/// runs anything or the pool is not empty. Real work never waits on this timeout: an
/// enqueue notifies its worker directly, and a spawn wakes an idle peer
/// (`wake_a_parked_peer`).
const IDLE_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_millis(500);

/// How long a newly enqueued process is left for its **owning** worker before a thief may
/// take it. The owner is exempt, so the window costs nothing when it is about to drain.
/// A spawner that blocks right after spawning (spawn, then `receive` the reply) yields
/// within microseconds and its child should run locally on a warm cache; one that keeps
/// running won't yield for a quantum and its child should go to an idle peer. A few
/// microseconds of first refusal separates the two without predicting which we have.
/// `BROOD_STEAL_GRACE_NS=<n>` overrides it (0 disables first refusal). Read once.
fn steal_grace_ns() -> u64 {
    static G: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *G.get_or_init(|| {
        std::env::var("BROOD_STEAL_GRACE_NS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5_000)
    })
}

/// The running process's `spawns_since_park`, held in a thread-local for the duration of a
/// quantum — the same install-and-read-back shape `Ctx`/the capture stack use, because the
/// worker multiplexes processes and the spawn site (deep inside `eval`) cannot reach the
/// `Process` box it is running inside. It tells the two spawner shapes apart by
/// *history*: a process that has spawned again without blocking in between is a
/// dispatcher, not a supervisor about to block, so its children should reach an idle peer.
pub(super) fn spawns_since_park_bump() -> u32 {
    SPAWNS_SINCE_PARK.with(|c| {
        let n = c.get().saturating_add(1);
        c.set(n);
        n
    })
}

thread_local! {
    static SPAWNS_SINCE_PARK: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Monotonic nanoseconds since the first call, never 0 — a cheap `u64` clock for
/// `queued_at` (where 0 means unstamped), so a `Process` carries a plain integer rather
/// than an `Instant`.
fn now_nanos() -> u64 {
    // `web_time::Instant` (std on native, `performance.now()` on wasm) — plain
    // `std::time::Instant::now()` panics under wasm, which the cooperative scheduler hits
    // on the scheduler's first clock read.
    static EPOCH: LazyLock<web_time::Instant> = LazyLock::new(web_time::Instant::now);
    (EPOCH.elapsed().as_nanos() as u64).max(1)
}

/// Per-worker "is currently running a process" flag. Index = `worker_id`, sized
/// to match `WORKERS`. A worker runs at most one process at a time, so this is a
/// 0/1 gauge of in-flight work. `assign_worker` folds it into a worker's load:
/// a worker draining one CPU-bound process has an *empty queue* yet is saturated,
/// and queue length alone would wrongly read it as idle. Set/cleared around the
/// `resume` in `run_one`; read (lock-free) at spawn placement.
static WORKER_BUSY: LazyLock<Vec<Padded<AtomicBool>>> = LazyLock::new(|| {
    (0..WORKERS.len())
        .map(|_| Padded(AtomicBool::new(false)))
        .collect()
});

/// Per-worker "is parked on its condvar" flag, and a count of how many are. Together they
/// let the spawn path ([`wake_a_parked_peer`](crate::process::scheduler::pool::wake_a_parked_peer))
/// hand a *steal opportunity* to an idle peer instead of leaving it to the `STEAL_BACKOFF`
/// re-probe — a worker is woken only when work lands on its own queue, so without this a
/// child behind a CPU-bound spawner waited a full quantum. The count is the cheap gate: a
/// saturated pool (no one parked) pays one load and never issues a wake.
static WORKER_PARKED: LazyLock<Vec<Padded<AtomicBool>>> = LazyLock::new(|| {
    (0..WORKERS.len())
        .map(|_| Padded(AtomicBool::new(false)))
        .collect()
});
static PARKED_COUNT: Padded<AtomicUsize> = Padded(AtomicUsize::new(0));

/// Rotating start point for `assign_worker`'s least-loaded scan. Read +
/// incremented under relaxed ordering — the only requirement is approximate
/// rotation; an occasional duplicate or skipped index is fine.
static NEXT_WORKER: AtomicUsize = AtomicUsize::new(0);

/// Pick the worker a `Process` should be placed on — at spawn, on a wake migration, or
/// when a thief re-homes a stolen process. **Least-loaded with a rotating start:** scan the
/// queues beginning at a round-robin offset and choose the shortest, breaking
/// ties toward the rotation. When load is even (the common case — most queues
/// empty) this degrades to plain round-robin; when one worker is backed up (a
/// spawn burst, or uneven drain) processes steer to idle workers instead.
/// Queue lengths are sampled via `try_lock`, so a momentarily-contended queue is
/// skipped rather than blocking the spawner. Validated clean (incl. under
/// `BROOD_GC_STRESS`) in the Track-A experiment; replaces pure round-robin.
fn assign_worker() -> usize {
    // `WORKERS.len()`, not `worker_count()`: touching the LazyLock commits the
    // pool size, so the modulus always matches the queues we index — a
    // `set_max_parallel` after the pool starts can no longer skew the count
    // (latent OOB), and the old per-spawn `BROOD_J` env read (+ the global env
    // lock) is gone — `worker_count()` now runs once, at pool init.
    let n = WORKERS.len().max(1);
    // A worker's load is its runnable backlog (queue length) **plus** the process
    // it's currently running, if any: a worker draining one long CPU-bound process
    // has an empty queue but no spare capacity, so queue length alone would wrongly
    // steer a newcomer onto it. (Parked/blocked processes aren't in the queue — they
    // sit in mailbox waiter slots — so the queue already excludes them; the running
    // process is the one thing it misses.) Sampled via `try_lock`, so a momentarily
    // contended queue reads as `MAX` and is skipped rather than blocking the spawner.
    let load = |i: usize| -> usize {
        // A dirty-blocked worker (parked in a native-nested receive — §7.4) won't return
        // to its run loop, so never route work to it (it would be stranded there).
        if WORKER_DIRTY[i].load(Ordering::Relaxed) {
            return usize::MAX;
        }
        match WORKERS[i].0.0.try_lock() {
            Ok(q) => q
                .len()
                .saturating_add(WORKER_BUSY[i].load(Ordering::Relaxed) as usize),
            Err(_) => usize::MAX,
        }
    };
    let start = NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n;
    let mut best = start;
    let mut best_len = load(start);
    for off in 1..n {
        if best_len == 0 {
            break; // can't do better than an empty, idle worker
        }
        let i = (start + off) % n;
        let len = load(i);
        if len < best_len {
            best_len = len;
            best = i;
        }
    }
    best
}

/// Pick the worker a freshly **spawned** `Process` is placed on — cheaply, without the
/// O(workers) scan [`assign_worker`] does. A child spawned from inside a worker stays on
/// **that worker's own queue** while the queue is short (warm cache; the spawner keeps
/// running); once the backlog reaches [`spawn_spill_threshold`] it spills round-robin,
/// because a dispatcher spawning a handler per request onto its own queue lets one slow
/// handler block all the rest, and stealing rebalances too little of that. Off-worker
/// (the root program, an overflow drainer) or on a dirty-blocked worker (§7.4 — it won't
/// drain its queue) there is no usable own queue, so round-robin. Either way the cost is
/// one atomic add or one `try_lock` on our own queue; a failed `try_lock` reads as "no
/// backlog" (the only contender is a thief, which is draining it anyway).
fn pick_spawn_worker() -> usize {
    let worker_count = WORKERS.len().max(1);
    match CURRENT_WORKER.with(|c| c.get()) {
        Some(worker) if worker < worker_count && !WORKER_DIRTY[worker].load(Ordering::Relaxed) => {
            let backlog = match WORKERS[worker].0.0.try_lock() {
                Ok(queue) => queue.len(),
                Err(_) => 0,
            };
            if backlog >= spawn_spill_threshold() {
                NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % worker_count
            } else {
                worker
            }
        }
        _ => NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % worker_count,
    }
}

/// Backlog at which a spawn stops going to the spawner's own worker and spills
/// round-robin. `BROOD_SPAWN_SPILL=<n>` overrides (`0` always spills; a huge value keeps
/// every child local).
///
/// Default **1**: keep the child local only when our queue is *empty* — the case locality
/// is about (a supervisor spawning one child). One waiting process means we are
/// dispatching. On the `latency` row spill 1 cut p50/p99 ~5×/3× against always-local, and
/// unlike always-round-robin it leaves `supervisor` unharmed (round-robin scatters every
/// child of a request/reply spawn: 2.6× slower).
fn spawn_spill_threshold() -> usize {
    static THRESHOLD: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        std::env::var("BROOD_SPAWN_SPILL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
    })
}

/// Total processes spawned since program start (read by `(%)`).
pub fn spawn_count() -> u64 {
    SPAWNED.load(Ordering::SeqCst)
}

/// Total processes work-stolen across worker threads since program start
/// (read by `(%)`). See [`STOLEN`].
pub fn steal_count() -> u64 {
    STOLEN.load(Ordering::SeqCst)
}

/// Total live migrations of running processes across worker threads since
/// program start (ADR-100 §7). See [`MIGRATED`].
pub fn migrate_count() -> u64 {
    MIGRATED.load(Ordering::SeqCst)
}

/// Cumulative green-process `receive`s that DIRTY-BLOCKED their worker thread (the §7.4
/// carve-out: a native frame between the body driver and the receive means the process
/// cannot be state-captured, so the whole OS worker waits on the mailbox condvar), so a
/// program quietly losing capture-mode (and migration) is observable. The JIT's
/// suspend-latch heals the JIT-frame cause of it (see `jit_suspend_feedback`), so this
/// figure staying flat after warmup is the observable that latch is working; the
/// regression test asserts exactly that.
pub(crate) static DIRTY_RECEIVE_BLOCKS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn note_dirty_receive_block() {
    DIRTY_RECEIVE_BLOCKS.fetch_add(1, Ordering::SeqCst);
}

/// Total dirty-blocked green `receive`s since program start. See [`DIRTY_RECEIVE_BLOCKS`].
pub fn dirty_receive_block_count() -> u64 {
    DIRTY_RECEIVE_BLOCKS.load(Ordering::SeqCst)
}

/// Cumulative quantum preemptions (a process exhausted its reduction budget
/// and was re-enqueued) — the scheduler half of the observability timing tier.
/// Read by `(%)`.
static PREEMPTED: AtomicU64 = AtomicU64::new(0);
/// Cumulative green-process exits (any reason). Read by `(%)`;
/// `spawn_count() - exit_count()` is the live-process figure.
static EXITED: AtomicU64 = AtomicU64::new(0);

pub fn preempt_count() -> u64 {
    PREEMPTED.load(Ordering::Relaxed)
}
pub fn exit_count() -> u64 {
    EXITED.load(Ordering::Relaxed)
}

/// Set the worker-pool size (0 = default ≈ `nproc`). Call once at startup, before
/// any spawning — once the `WORKERS` pool has initialised the size is committed
/// and this has no further effect (everything indexes by `WORKERS.len()`).
/// (Replaces the old per-spawn thread cap.)
pub fn set_max_parallel(n: usize) {
    WORKER_COUNT.store(n, Ordering::SeqCst);
}

/// High-water mark of processes running simultaneously (≤ pool size).
pub fn peak_threads() -> u64 {
    PEAK_RUNNING.load(Ordering::SeqCst) as u64
}

/// Worker OS threads in the scheduler pool (0 until the first `spawn` starts them).
pub fn worker_threads() -> u64 {
    ACTIVE_WORKERS.load(Ordering::SeqCst) as u64
}

/// Resolve the pool size: `BROOD_J` env override, else `set_max_parallel`'s
/// value, else ≈ `nproc`. Called exactly once — at the `WORKERS` LazyLock
/// init — so the env read never lands on the spawn hot path.
///
/// **Floored at 2.** A native-nested `receive` *blocks* its worker (§7.4) and
/// `drain_worker_queue` re-routes its backlog to another worker; with a single worker
/// there is nowhere to re-route to, so a child queued behind the block never runs. The
/// pool always keeps a spare to drain a dirty-blocked worker.
fn worker_count() -> usize {
    let requested = std::env::var_os("BROOD_J")
        .and_then(|s| s.to_str().and_then(|t| t.parse::<usize>().ok()))
        .filter(|&n| n > 0)
        .unwrap_or_else(|| match WORKER_COUNT.load(Ordering::SeqCst) {
            0 => std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            n => n,
        });
    // The floor applies only to the real OS-thread pool. The deterministic test driver
    // (`set_test_no_workers`) starts no threads and drives quanta by hand, so it can't
    // dirty-block a worker and *wants* the exact requested count (e.g. the one-worker
    // preemption test pins both processes to worker 0).
    let floor = if TEST_NO_WORKERS.load(Ordering::SeqCst) {
        1
    } else {
        2
    };
    requested.max(floor)
}

/// Test-only: when set, [`ensure_workers`] starts **no** OS worker threads, so a test
/// can drive scheduling quanta synchronously and deterministically via
/// [`test_drive_quanta`] (bounded by work units, not wall-clock). Inert (`false`) in
/// every normal build — only the isolated preemption test sets it. A plain runtime
/// `AtomicBool` (not `#[cfg(test)]`) because it is reached from an *integration* test,
/// a separate crate that doesn't see the lib's `test` cfg; the one-time branch in
/// `ensure_workers` is free in production (the flag is never set).
static TEST_NO_WORKERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl Process {
    /// Everything this process's heap holds ([`Heap::footprint`]). Called on a parked
    /// process by the memory-pressure selector, under its mailbox lock.
    pub(super) fn footprint(&self) -> usize {
        self.heap.footprint()
    }

    /// Mutable access to this process's heap. Used by the L1 local-send fast path, which
    /// holds the process exclusively (taken out of `MailboxState::waiter` under the lock)
    /// while it copies a message straight in.
    pub(super) fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
    }

    /// Retire this process with `reason` — the one death path for a process the
    /// scheduler holds. Fires its links and monitors (`deregister`), and for the root
    /// program process also tells the thread blocked on the program's outcome, which
    /// otherwise waits forever when an exit signal kills it mid-run (KI-225).
    pub(super) fn retire(self: Box<Self>, reason: Message) {
        let outcome = self
            .program
            .as_ref()
            .map(|_| crate::eval::compile::ProgramState::retire_outcome(&reason));
        deregister(self.pid, reason, Some(&self.heap));
        if let (Some(program), Some(outcome)) = (&self.program, outcome) {
            program.retired(outcome);
        }
    }

    /// Drive the body one quantum: run fresh, or resume the parked continuation
    /// (`resume` is taken). The `&mut self` borrow ends when this returns, so `run_one`
    /// is then free to move/park/re-queue `self` on the outcome.
    fn drive(&mut self) -> Result<crate::eval::compile::VmOutcome, LispError> {
        // Second of the two stamp points for the KI-14 native-stack guard (the other is the
        // outermost native entry, see `stamp_stack_limit_if_outermost`). A process resumes on
        // whichever worker the scheduler routed it to and worker stack bases differ, so the
        // limit — an absolute address — has to be re-derived for this worker's stack before
        // any of this quantum's native code compares against it.
        #[cfg(feature = "jit")]
        crate::eval::compile::stamp_stack_limit(&mut self.heap);
        let resume = self.resume.take();
        if super::scheduler::pool::sched_dbg()
            && let Some(r) = &resume
        {
            eprintln!("[sched] resume pid={} {}", self.pid, r.dbg_line());
        }
        match self.program.as_mut() {
            // The root program process (ADR-135): drive the top-level forms.
            Some(prog) => crate::eval::compile::run_program_body(&mut self.heap, prog, resume),
            // The fresh run TAKES the body: it may be a handle into this process's own
            // heap (a spawned closure copied in, ADR-405), which nothing roots once the
            // driver has it — a collection would leave a copy here stale.
            None => {
                let body = if resume.is_none() {
                    std::mem::replace(&mut self.body, Value::nil())
                } else {
                    self.body
                };
                crate::eval::compile::run_process_body(&mut self.heap, body, resume)
            }
        }
    }

    /// Trim this process's heap as it parks — see [`Heap::trim_parked`]. Kept here rather
    /// than in `pool` so the `heap` field stays private to this module.
    fn trim_on_park(&mut self) {
        self.heap.trim_parked();
    }

    /// Would [`trim_on_park`](Self::trim_on_park) do anything? See [`Heap::park_trim_due`].
    fn park_trim_due(&self) -> bool {
        self.heap.park_trim_due()
    }

    /// Stash a captured continuation back into the process before it parks or re-queues
    /// (so the next `run_one` resumes from it).
    fn store_resume(&mut self, s: crate::eval::compile::Continuation) {
        if super::scheduler::pool::sched_dbg() {
            eprintln!("[sched] park pid={} {}", self.pid, s.dbg_line());
        }
        self.resume = Some(s);
    }

    /// Establish `CURRENT` for this quantum. Resets the per-quantum thread-locals
    /// (GC-block depth, stack base, macro block) to 0: each quantum runs on a fresh
    /// worker stack, so they are re-established here.
    ///
    /// The capture stack MOVES into the ctx (and `save_ctx` moves it back): for the
    /// quantum's duration `CURRENT` is its only home — nothing reads `Process::capture`
    /// while the process runs — so a clone in and a clone out was two `Vec` copies per
    /// quantum for nothing.
    fn install_ctx(&mut self) {
        let ctx = Ctx {
            pid: self.pid,
            mailbox: Arc::clone(&self.mailbox),
            capture: std::mem::take(&mut self.capture),
        };
        let displaced = CURRENT.with(|c| c.borrow_mut().take());
        DISPLACED.with(|d| d.borrow_mut().push(displaced));
        CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
        gc_block_set(0);
        stack_base_set(0);
        macro_block_set(0);
    }

    /// Read the (possibly mutated) capture stack back out of `CURRENT` into the process,
    /// then put back whatever `install_ctx` displaced — so `begin_capture`/`take_capture`
    /// done this quantum persist across the next `receive` suspend, and the thread's TLS
    /// doesn't leak this process's ctx into whatever runs next.
    ///
    /// RESTORES rather than clears. On a worker thread the displaced value is `None` and the
    /// two are the same thing; on wasm the quantum runs on the caller's thread, and clearing
    /// threw away the caller's own context — see `DISPLACED` (KI-115).
    ///
    /// The quantum's ctx is TAKEN, not cloned: it is overwritten by the restore below
    /// either way, so its capture stack moves straight back into the process. (A quantum
    /// whose `CURRENT` was cleared from under it — only `deregister_root_ctx`, a host
    /// API — comes back with no capture stack; it had already lost its pid by then.)
    ///
    /// Returns this process's mailbox `Arc` for the post-quantum tail, which needs one it
    /// owns (the process moves into that mailbox's waiter slot while the tail holds its
    /// lock): the reference `install_ctx` took, handed on instead of dropped here and
    /// re-taken there.
    fn save_ctx(&mut self) -> Arc<Mailbox> {
        let mailbox = match CURRENT.with(|c| c.borrow_mut().take()) {
            Some(ctx) => {
                self.capture = ctx.capture;
                ctx.mailbox
            }
            None => Arc::clone(&self.mailbox),
        };
        let displaced = DISPLACED.with(|d| d.borrow_mut().pop()).flatten();
        CURRENT.with(|c| *c.borrow_mut() = displaced);
        // Always the same mailbox: the stack of displaced contexts makes every nested
        // install/save pair restore before this one runs. Checked rather than trusted,
        // since a wrong mailbox here would park the process where no sender looks.
        if Arc::ptr_eq(&mailbox, &self.mailbox) {
            mailbox
        } else {
            Arc::clone(&self.mailbox)
        }
    }
}

/// `(self)` — this process's pid. A borrow of the installed ctx, not a clone of it
/// (which copied the capture stack and bumped the mailbox `Arc` to read one `u64`).
pub fn self_pid() -> u64 {
    with_ctx(|ctx| ctx.pid)
}

/// This process's mailbox arrival sequence right now, for `(ref)` to stamp a receive-mark
/// (ADR-195). Takes the mailbox lock — deliberately, rather than keeping a lock-free hint
/// republished on every push: a hint costs an atomic store per *message*, which rows that
/// never call `(ref)` would still pay. Reading it here
/// moves that cost onto ref creation, which is rarer than sending by construction, and the
/// caller is about to take the same lock to `send` anyway.
pub fn self_mailbox_seq() -> u64 {
    with_ctx(|ctx| crate::core::sync::lock(&ctx.mailbox.state).next_seq)
}

/// Run `f` on this thread's process context, minting a root one first exactly as
/// [`ensure_ctx`] does — but BORROWED, where `ensure_ctx` hands back a clone (the
/// mailbox `Arc` and the whole capture stack). For the accessors that read one field.
///
/// `f` runs under a shared borrow of `CURRENT`, so it must not reach anything that
/// installs, clears or pushes to the context (`begin_capture`, a quantum): a field read
/// or the mailbox lock, nothing that can run Brood.
pub(super) fn with_ctx<T>(f: impl FnOnce(&Ctx) -> T) -> T {
    CURRENT.with(|current| {
        if current.borrow().is_none() {
            ensure_ctx();
        }
        let installed = current.borrow();
        f(installed
            .as_ref()
            .expect("ensure_ctx installs this thread's context"))
    })
}

/// What a `receive` needs of the running process: its pid and mailbox, without the
/// capture stack a full [`Ctx`] clone would copy on every receive.
pub(super) struct CurrentProcess {
    pub(super) pid: u64,
    pub(super) mailbox: Arc<Mailbox>,
}

/// The running process's [`CurrentProcess`] (minting a root context if this thread has
/// none). One `Arc` bump: the receive holds the mailbox across matcher runs that may
/// themselves touch `CURRENT`, so it cannot keep a borrow instead.
pub(super) fn current_process() -> CurrentProcess {
    with_ctx(|ctx| CurrentProcess {
        pid: ctx.pid,
        mailbox: Arc::clone(&ctx.mailbox),
    })
}

/// This process's pid **without** minting a context — `None` if it has none yet.
/// A process with no ctx has never `spawn`ed or messaged, so it isn't in
/// [`REGISTRY`] and can't be sharing a runtime under a drain — so skipping its
/// report is sound. The cheap read the eval / VM safepoint uses to report drain
/// liveness (ADR-091 Stage 3c); a single thread-local borrow, no allocation.
pub fn current_pid() -> Option<u64> {
    CURRENT.with(|c| c.borrow().as_ref().map(|ctx| ctx.pid))
}

/// Run `f` on the current process's mailbox, without the registry lock — `None` on a
/// thread with no process context. For a per-safepoint probe (`process/pressure.rs`).
pub(super) fn with_current_mailbox<T>(f: impl FnOnce(&Mailbox) -> T) -> Option<T> {
    CURRENT.with(|c| c.borrow().as_ref().map(|ctx| f(&ctx.mailbox)))
}

/// A snapshot of every live local process's pid — the [`REGISTRY`] keys. This is
/// the live set the RUNTIME-drain union queries (ADR-091 Stage 3c). It is
/// **complete** for the drain's purposes: `spawn` registers a child (before it can
/// run) and its parent, and `receive`/`self` register the root, so every process
/// that could hold a reference to the draining generation is present.
pub fn live_pids() -> Vec<u64> {
    REGISTRY.pids()
}

/// Tear down every **permanently-parked** green process belonging to `runtime` — one
/// parked on a `(receive)` nothing will ever send to holds its whole heap in its
/// mailbox's waiter slot, which an embedded host dropping an `Interp` would leak. Called
/// from `Interp::drop`.
///
/// Each reaped process goes through the **normal death path** (`deregister` with reason
/// `:killed`). Only *parked* waiters of *this* runtime are touched; a running process is
/// left to finish or park. Racing `send`s are safe: the waiter is taken under the state
/// lock, so a concurrent deliver either woke it first (we skip) or queues into a mailbox
/// removed right after. Returns how many processes were reaped.
pub fn shutdown_runtime_parked(runtime: &Arc<crate::core::heap::RuntimeCode>) -> usize {
    let pids: Vec<u64> = REGISTRY.pids();
    let mut reaped = 0;
    for pid in pids {
        let mailbox = match REGISTRY.get(pid) {
            Some(mb) => mb,
            None => continue,
        };
        let taken: Option<Box<Process>> = {
            let mut st = crate::core::sync::lock(&mailbox.state);
            match &st.waiter {
                Some(p) if Arc::ptr_eq(&p.heap.runtime_arc(), runtime) => st.waiter.take(),
                _ => None,
            }
        };
        if let Some(p) = taken {
            p.retire(Message::Keyword(*pk::KILLED_SYMBOL));
            reaped += 1;
        }
    }
    reaped
}

/// The eval / VM safepoint's cooperative RUNTIME-drain report (ADR-091 Stage 3c):
/// this process reports whether it still references the draining generation. Called
/// only when [`Heap::drain_active`](crate::core::heap::Heap::drain_active) already
/// returned true. **Throttled** to 1/`DRAIN_REPORT_STRIDE` safepoints via
/// [`Heap::drain_report_due`] (a lingering drain would otherwise re-report on nearly
/// every frame); the parked-process inspector calls `report_gen_liveness` directly. A
/// process with no ctx (not in the live set) is skipped.
pub fn report_drain_liveness(heap: &Heap) {
    if !heap.drain_report_due() {
        return;
    }
    if let Some(pid) = current_pid() {
        heap.report_gen_liveness(pid);
    }
}

/// **RUNTIME collector — Stage 5 (parked-process drain inspection, ADR-091).** A parked
/// process (suspended in `receive`) can't report its own liveness for a drain armed
/// *after* it parked — it isn't running, so it never reaches a safepoint. Left
/// unhandled, an idle server parked on current-generation code would block **every**
/// later drain forever. So the drain coordinator inspects each parked process's captured
/// continuation directly (relocatable heap data — ADR-100) via [`report_gen_liveness`],
/// acking it iff it's clean of the draining generation — no wakeup, no kill. A process
/// paused *in* old-generation code stays dirty, correctly.
///
/// [`report_gen_liveness`]: crate::core::heap::Heap::report_gen_liveness
fn report_parked_liveness() {
    // Nothing parked → skip the O(all-processes) walk below (the common case under a
    // fan-out drain, where it was an O(processes²) lock storm).
    if parked_count() == 0 {
        return;
    }
    // Snapshot the **parked** processes only, filtering on the lock-free `status` cell,
    // so only parked mailboxes are locked. A racy status read is harmless (re-inspected
    // next attempt). Never hold REGISTRY across a per-mailbox lock — hence the collect.
    // A parked process's heap is quiescent and shares the runtime `Arc`, so its
    // `report_gen_liveness` writes the shared drain ack map on its own behalf.
    let parked: Vec<(u64, Arc<Mailbox>)> = REGISTRY
        .entries()
        .into_iter()
        .filter(|(_, mb)| mb.status.load(Ordering::Relaxed) == ST_WAITING)
        .collect();
    for (pid, mailbox) in parked {
        let mut state = crate::core::sync::lock(&mailbox.state);
        if let Some(proc) = state.waiter.as_mut() {
            proc.heap.report_gen_liveness(pid);
            // Found pinning the drain through its data: copy that data out here and report
            // again (`Heap::rehome_drain_pins`). A parked process does not collect until it
            // wakes, and a server parked for good would otherwise pin the drain for good. Its
            // heap is quiescent and held under this lock, as the report relies on too.
            if proc.heap.rehome_drain_pins(&mut [], &mut []) {
                proc.heap.report_gen_liveness(pid);
            }
        }
    }
}

/// Is the currently-draining RUNTIME generation dead — has *every* live process
/// reported clean for the current drain epoch (ADR-091 Stage 3c)? Reads the live
/// set from the scheduler [`REGISTRY`] and delegates the per-epoch ack check to the
/// heap. `false` when no drain is armed. Once this is `true`, Stage 4 may free the
/// generation.
///
/// Also drives [`report_parked_liveness`] first, so a parked-but-clean process (which
/// can't report for itself) doesn't block the drain.
pub fn old_gen_drained(heap: &Heap) -> bool {
    if !heap.drain_active() {
        return false;
    }
    // Two-stage O(1) gate, then the authoritative walk. The parked-process inspector must
    // not run while any *running* process is still un-acked (it would be pure waste, and
    // an O(processes²) storm under a fan-out): if `acked + parked < live`, some running
    // process still pins the generation and will self-ack at its own safepoint. An
    // over-count only opens the gate early — `gen_drained` below still guards the free.
    let live = REGISTRY.len() as u64;
    if heap.drain_acked_count() + (parked_count() as u64) < live {
        return false;
    }
    // The only holdouts are parked — inspect them so they can ack, then re-gate.
    report_parked_liveness();
    if heap.drain_acked_count() < live {
        return false;
    }
    heap.gen_drained(&live_pids())
}

/// **RUNTIME collector — Stage 4.** If the current drain has completed — every live
/// process reported clean ([`old_gen_drained`]) — free the drained generation
/// ([`Heap::free_runtime_gen`]) and end the drain. Returns whether it freed. The
/// union check reads the live set from the scheduler `REGISTRY`; the free itself is a
/// shared-safe `ArcSwap` store, so it needs no unique ownership. A no-op when no drain
/// is armed or the generation isn't drained yet.
pub fn free_drained_gen(heap: &Heap) -> bool {
    // Snapshot the drain identity (epoch + generation) ONCE and carry it into the free,
    // which re-validates it under the aging gate. Reading `drain_gen()` separately after
    // validating was the TOCTOU: another process can complete the free, end the drain and
    // arm a new one for the *other* generation in between, leaving this call to free a
    // generation that is neither drained nor dead. See `Heap::free_runtime_gen`.
    let (epoch, generation) = heap.drain_identity();
    if !old_gen_drained(heap) {
        return false;
    }
    heap.free_runtime_gen(generation, epoch)
}

/// Are we currently running inside a **green** (spawned) process — as opposed to the
/// *root* thread (the REPL / file runner / MCP dispatcher)? True when [`in_capture_run`]
/// is set (the worker is driving a process body). Used by the eval-time `unbound` raise
/// to attach a scheduler-race hint (the under-load failure mode
/// `docs/archive/claude-demo-findings.md` flagged — concurrent prelude lookups racing).
pub fn in_green_process() -> bool {
    in_capture_run()
}

/// Wrap a local process id in a [`Value::Pid`] tagged with this runtime's node
/// identity — what `self`/`spawn` hand back. The node part makes the pid routable
/// off-node once the holder is on another runtime.
pub fn pid_value(id: u64) -> Value {
    Value::pid(crate::dist::local_node(), id)
}

/// Deregisters a **root-thread** context when its OS thread exits.
///
/// `ensure_ctx` mints a pid + mailbox for any thread that touches `self`/`send`/`receive`
/// outside the scheduler, and nothing else removes it: green processes die through
/// `deregister` and `shutdown_runtime_parked` reaps only *parked* waiters, so without this
/// a host that churns OS threads would leak one registry entry, one mailbox (plus every
/// message sent to it), and one `live_pids()` member per thread.
///
/// Armed **only** on the minting branch of `ensure_ctx`, so it is never armed for a green
/// process's `Ctx` (which `run_one` installs into `CURRENT` and `save_ctx` takes back out
/// — a worker thread that later exits must not retire the process it last ran).
struct RootCtxGuard {
    pid: u64,
}

impl Drop for RootCtxGuard {
    fn drop(&mut self) {
        // This usually runs during **TLS destruction**. The teardown notifies monitors and
        // links, which can reach `wake_enqueue` → `CURRENT_WORKER`, and touching a
        // thread-local that has already been destroyed panics — a panic in a TLS
        // destructor aborts the process. The registry removal (a plain static) happens
        // first inside `retire_root_ctx` and always succeeds; catching here means a
        // late-destruction order at worst skips the *notifications*, instead of taking the
        // host down.
        let pid = self.pid;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            retire_root_ctx(pid);
        }));
    }
}

thread_local! {
    /// The live [`RootCtxGuard`] for this thread, if `ensure_ctx` minted a root context on
    /// it. Held in its own thread-local (not inside `Ctx`, which is `Clone` and gets copied
    /// into `Process`/`install_ctx`) so exactly one drop happens, at thread exit.
    static ROOT_CTX_GUARD: RefCell<Option<RootCtxGuard>> = const { RefCell::new(None) };
}

thread_local! {
    /// The runtime a freshly-minted root context should be stamped with — set for the
    /// duration of `Interp::eval_forms`, which is the only place a root context can be
    /// minted with a known owner. 0 means "no owning runtime known".
    static MINTING_RUNTIME_TAG: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Scope `tag` as the runtime that owns any root context minted on this thread while `f`
/// runs. Restores the previous value (rather than clearing) so nesting — a host evaluating
/// on one `Interp` from inside another's call — leaves the outer scope intact.
pub fn with_minting_runtime_tag<T>(tag: u64, f: impl FnOnce() -> T) -> T {
    let prev = MINTING_RUNTIME_TAG.with(|t| t.replace(tag));
    let out = f();
    MINTING_RUNTIME_TAG.with(|t| t.set(prev));
    out
}

/// Retire this thread's root context, but only if it belongs to runtime `tag`.
///
/// The unconditional [`deregister_root_ctx`] takes whatever context the thread holds, with
/// no check that the caller minted it. That is fine for a host tearing a thread down and
/// wrong for `Interp::drop`: a host with a long-lived `Interp` that builds a short-lived one
/// on the same thread would, on dropping the temporary, retire the long-lived
/// interpreter's context — changing its pid, discarding its queued mailbox, and firing its
/// monitors and links as a death. Returns false (and touches nothing) for a context minted
/// by another runtime, or minted with no known owner.
pub fn deregister_root_ctx_of(tag: u64) -> bool {
    let owned = CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|ctx| ctx.mailbox.runtime_tag() == tag)
    });
    if !owned {
        return false;
    }
    deregister_root_ctx()
}

/// Retire this thread's root context now, rather than at thread exit: remove its pid from
/// the process registry, fire its monitors/links, and unbind it, so a subsequent
/// `self`/`receive` mints a fresh one. Returns whether there was one to retire.
///
/// For an embedding host that reuses a long-lived thread (a request worker, a thread pool)
/// and wants the process-table entry gone at a known point instead of whenever the thread
/// happens to die. A thread that simply exits needs nothing — [`RootCtxGuard`] does this
/// for it. A no-op on a scheduler worker inside a quantum (that ctx belongs to a green
/// process, and no guard was ever armed for it).
pub fn deregister_root_ctx() -> bool {
    let guard = ROOT_CTX_GUARD.with(|g| g.borrow_mut().take());
    match guard {
        Some(g) => {
            // Unbind first: `retire_root_ctx` (via `Drop`) fires monitors and links, and
            // anything that runs then must not find a `CURRENT` naming a pid that is on
            // its way out of the registry.
            CURRENT.with(|c| *c.borrow_mut() = None);
            drop(g);
            true
        }
        None => false,
    }
}

/// The current process's context. A green process has it installed by `run_one` each
/// quantum; the first time a *root* thread (the REPL / file runner) uses `self`/`receive`,
/// register it as a blocking-mailbox process so it can participate in message passing.
pub(super) fn ensure_ctx() -> Ctx {
    CURRENT.with(|c| {
        if let Some(ctx) = c.borrow().as_ref() {
            return ctx.clone();
        }
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        let mailbox = Mailbox::new();
        // A root context lives on THIS thread for good and is never run in quanta, so its
        // CPU time is the thread's own clock (`process-info` `:cpu-us`).
        pin_cpu_clock_to_this_thread(&mailbox);
        // Stamp the owning runtime, so this context can later be retired by the `Interp`
        // that caused it and by no other (see `deregister_root_ctx_of`). A green process
        // gets its tag in `spawn`; a root context had none at all — it read 0, which also
        // meant it silently declined every shared-closure handle (ADR-208).
        //
        // 0 when minted outside any `Interp::eval_forms` — an embedder touching
        // `self`/`send` directly off a bare thread. That is deliberately treated as
        // "unowned": `deregister_root_ctx_of` will not retire it, because retiring a
        // context we cannot prove is ours is the failure this guards against.
        mailbox.set_runtime_tag(MINTING_RUNTIME_TAG.with(|t| t.get()));
        REGISTRY.insert(pid, Arc::clone(&mailbox));
        let ctx = Ctx {
            pid,
            mailbox,
            capture: Vec::new(),
        };
        *c.borrow_mut() = Some(ctx.clone());
        // This is the root-thread branch by construction (a green process's ctx is
        // installed by `run_one`, never minted here), so give it the death path green
        // processes get from `deregister` — see `RootCtxGuard`.
        ROOT_CTX_GUARD.with(|g| *g.borrow_mut() = Some(RootCtxGuard { pid }));
        ctx
    })
}

/// Push a fresh output-capture buffer onto the current process's capture stack
/// (minting its ctx if needed). While it's the top of the stack, this process's —
/// and any it `spawn`s — `print` / terminal output appends to that buffer instead
/// of real stdout (see builtins' `capture_write`). Captures **nest**: an inner
/// `begin_capture` shadows an outer one until its matching `take_capture`. The
/// `nest mcp` dispatcher uses this so a tool handler's output (even a handler it
/// runs in a spawned, killable process) can't corrupt the JSON-RPC stdout stream;
/// a `with-out-str` *inside* such a handler nests cleanly on top. A fresh `Arc` per
/// call → concurrent captures never collide.
pub fn begin_capture() {
    ensure_ctx();
    let buf = Arc::new(Mutex::new(String::new()));
    CURRENT.with(|c| {
        if let Some(ctx) = c.borrow_mut().as_mut() {
            ctx.capture.push(buf);
        }
    });
}

/// Pop the top capture buffer and return what was written to it, or `None` if no
/// capture was active. Drains the buffer (a spawned child wrote to the same `Arc`);
/// an outer capture, if any, is uncovered and resumes catching subsequent output.
pub fn take_capture() -> Option<String> {
    let arc = CURRENT.with(|c| c.borrow_mut().as_mut().and_then(|ctx| ctx.capture.pop()));
    arc.map(|a| std::mem::take(&mut *crate::core::sync::lock(&a)))
}

/// If the current process has an active capture, append `s` to the **top** buffer
/// and return `true`; otherwise `false` (output goes to real stdout). The fast path
/// — no capture — is a thread-local borrow + a `Vec::last` check; the `print` hot
/// path pays no lock unless capturing.
pub fn capture_append(s: &str) -> bool {
    CURRENT.with(
        |c| match c.borrow().as_ref().and_then(|ctx| ctx.capture.last()) {
            Some(arc) => {
                crate::core::sync::lock(arc).push_str(s);
                true
            }
            None => false,
        },
    )
}

#[cfg(test)]
mod root_ctx_tests {
    use super::*;

    /// A **root-thread context** must not outlive its thread.
    ///
    /// Any OS thread that touches `self`/`send`/`receive` outside the scheduler gets a pid
    /// and a mailbox from `ensure_ctx`, and until `RootCtxGuard` existed nothing ever
    /// removed either: `deregister` runs only for green processes, and
    /// `shutdown_runtime_parked` reaps only *parked waiters*. A host that churns threads
    /// (a request-per-thread embedder, a thread pool that recycles) therefore accumulated a
    /// registry entry, a live mailbox that `deliver` still queues into, and one more
    /// `live_pids()` member — which also inflates the `live` count the RUNTIME
    /// drain-completion gate compares its ack count against — for the life of the OS
    /// process. Asserted per-pid rather than on `REGISTRY.len()` so a concurrently running
    /// test cannot make it flaky.
    #[test]
    fn a_root_thread_context_is_retired_when_its_thread_exits() {
        let mut pids = Vec::new();
        for _ in 0..8 {
            let h = std::thread::spawn(|| {
                let pid = ensure_ctx().pid;
                assert!(
                    REGISTRY.contains_key(pid),
                    "ensure_ctx must register the root ctx it mints"
                );
                pid
            });
            // `join` returns after the thread has fully terminated, so its thread-locals
            // (and therefore `RootCtxGuard`) have been dropped by the time we look.
            pids.push(h.join().expect("root-ctx thread"));
        }
        for pid in pids {
            assert!(
                !REGISTRY.contains_key(pid),
                "root ctx {pid} outlived its thread — the registry entry leaks"
            );
        }
    }

    /// A root context's retirement runs to the end when the thread's OTHER thread-locals
    /// are already gone. It runs in `RootCtxGuard`'s destructor and interns the exit
    /// reason; with the symbol cache destroyed first (this thread made its root context
    /// BEFORE first interning, so the cache was registered later and is torn down
    /// earlier) `intern` panicked with "cannot access a Thread Local Storage value during
    /// or after destruction", the guard swallowed it, and retirement stopped short of
    /// `retire_pid_tail` — so a monitor on the thread never got its DOWN.
    #[test]
    fn a_monitor_on_a_root_thread_gets_its_down_when_the_thread_exits() {
        let watcher = ensure_ctx().pid;
        let (pid_tx, pid_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let pid = ensure_ctx().pid;
            // First intern on this thread, after the root context exists.
            let _ = crate::core::value::intern("root-ctx-retire-test-name");
            pid_tx.send(pid).unwrap();
            go_rx.recv().unwrap();
        });
        let target = pid_rx.recv().unwrap();
        crate::process::monitor::monitor(target);
        go_tx.send(()).unwrap();
        thread.join().expect("root-ctx thread");
        assert_eq!(
            crate::process::mailbox::mailbox_len(watcher),
            Some(1),
            "the root thread's exit must deliver the watcher's DOWN"
        );
    }

    /// The explicit form, for a host that recycles a long-lived thread and wants the
    /// process-table entry gone at a known point: retiring unbinds the ctx, so the next
    /// `self` mints a fresh pid rather than handing back a deregistered one.
    #[test]
    fn deregister_root_ctx_retires_and_unbinds() {
        std::thread::spawn(|| {
            let first = ensure_ctx().pid;
            assert!(REGISTRY.contains_key(first));
            assert!(deregister_root_ctx(), "there was a root ctx to retire");
            assert!(!REGISTRY.contains_key(first));
            let second = ensure_ctx().pid;
            assert_ne!(first, second, "a retired ctx must not be handed back");
            assert!(REGISTRY.contains_key(second));
            assert!(deregister_root_ctx());
            assert!(!deregister_root_ctx(), "retiring twice is a no-op");
        })
        .join()
        .expect("root-ctx thread");
    }
}

#[cfg(test)]
mod charge_tests {
    use super::*;

    /// The per-process memory **floor**, structurally attributed.
    ///
    /// A live idle process's RSS cost is the *slope* of RSS against process count (RSS/N
    /// would fold in the runtime's base).
    ///
    /// This prints the breakdown so the attribution is reproducible rather than folklore, and
    /// asserts a ceiling so accidental growth is caught here instead of in a benchmark
    /// row much later. `Heap` is embedded *by value* in `Process`, so a new
    /// field on `Heap` is paid once per live process — which is exactly the kind of change
    /// that slips through review.
    #[test]
    fn per_process_floor_is_attributed() {
        use std::mem::size_of;
        let proc_sz = size_of::<Process>();
        let heap_sz = size_of::<crate::core::heap::Heap>();
        let mailbox_sz = size_of::<Mailbox>();
        let state_sz = size_of::<crate::process::mailbox::MailboxState>();
        eprintln!("[floor] size_of::<Process>()      = {proc_sz}");
        eprintln!("[floor]   of which Heap (by value) = {heap_sz}");
        eprintln!("[floor] size_of::<Mailbox>()      = {mailbox_sz}");
        eprintln!("[floor]   of which MailboxState    = {state_sz}");
        // The parked continuation: an idle process is parked in `receive`, so it holds one.
        let susp_sz =
            size_of::<crate::eval::compile::Suspended>().max(size_of::<crate::eval::TwSuspended>());
        eprintln!("[floor] size_of::<Suspended>()    = {susp_sz} (a parked process holds one)");
        eprintln!(
            "[floor] structural total            = {} bytes (measured floor ~4190)",
            proc_sz + mailbox_sz + susp_sz
        );
        eprintln!(
            "[floor] unattributed                = {} bytes (Vec/Box capacity + size-class rounding)",
            4190i64 - (proc_sz + mailbox_sz + susp_sz) as i64
        );
        // Generous ceilings: these are here to catch a doubling, not to pin a byte count.
        assert!(
            proc_sz < 8192,
            "Process grew to {proc_sz} bytes; it is paid per live process (Heap is inline)"
        );
        assert!(
            mailbox_sz < 4096,
            "Mailbox grew to {mailbox_sz} bytes; paid per live process"
        );
    }
}
