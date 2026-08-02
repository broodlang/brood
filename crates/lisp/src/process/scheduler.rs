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
    clear_parked, parked_count, set_status, wake_parked, Mailbox, REGISTRY, ST_RUNNABLE,
    ST_RUNNING, ST_WAITING,
};
use super::message::Message;
use super::monitor;

// Execution-safety guards (GC-block / macro-block depth + the stack-overflow byte
// guard) live in a child module; re-exported so `scheduler::…` (hence
// `process::…`, ADR consumers) resolves unchanged. `gc_block_set`/`macro_block_set`/
// `stack_base_set` are scheduler-internal (only `install_ctx` resets them).
mod guards;
pub use guards::{
    gc_block_depth, macro_block_active, native_stack_headroom_ok, stack_budget,
    stack_overflow_check, GcBlockGuard, MacroBlockGuard, NATIVE_STACK_MARGIN_BYTES,
    WORKER_STACK_BYTES,
};
use guards::{gc_block_set, macro_block_set, stack_base_set};

// Process lifecycle (spawn/exit/deregister) lives in a child module; the shared
// scheduling state stays in the root, reached from there via `use super::*`.
// Re-export the public surface so `scheduler::…`/`process::…` paths are unchanged.
mod lifecycle;
pub(crate) use lifecycle::exit_propagate;
use lifecycle::{deregister, proc_descr};
pub use lifecycle::{exit, spawn, spawn_linked, spawn_root_program};

// The worker pool + run-queue execution loop lives in a child module; shared
// scheduling state stays in the root (reached via `use super::*`). Re-export the
// public/`process`-facing surface so `scheduler::…`/`process::…` paths are unchanged.
mod pool;
use pool::spawn_overflow_drainer;
pub(crate) use pool::{enqueue, ensure_workers, wake_enqueue};
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
    heap: Heap,
    /// The 0-arg body thunk (a shared-runtime `Fn` handle, valid in `heap`). Unused
    /// (`nil`) when `program` is set — a whole-program root process (ADR-135).
    body: Value,
    /// Set for the **root program process** (ADR-135): drives the top-level forms one at
    /// a time instead of a single body thunk, so `(self)` is one stable pid across the
    /// whole program and a top-level `receive` park-captures. `None` for an ordinary
    /// `spawn`ed process.
    program: Option<Box<crate::eval::compile::ProgramState>>,
    /// The parked/preempted VM continuation, or `None` if not yet started.
    resume: Option<Box<crate::eval::compile::Suspended>>,
    /// The output-capture stack snapshot (the process carries it — no coroutine holds a
    /// `Ctx`). `run_one` installs it into `CURRENT` per quantum and reads it back after,
    /// so `begin_capture`/`take_capture` persist across `receive` suspends.
    capture: Vec<Arc<Mutex<String>>>,
    /// Monotonic nanos at which this process was last enqueued, or 0 if never. Read only
    /// by `try_steal`, to give the owning worker a brief **first refusal** on freshly
    /// queued work (see `STEAL_GRACE`).
    queued_at: u64,
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
    /// the **top** buffer instead (see builtins' `capture_write`). It's a *stack*
    /// so captures **nest**: `begin_capture` pushes a fresh buffer, `take_capture`
    /// pops the top and returns its text — so a `with-out-str` running inside a
    /// `nest mcp` `tools/call` (which itself installs a capture) drains only its
    /// own buffer and the MCP envelope's capture survives underneath. A SPAWNED
    /// child **inherits** a snapshot of the parent's stack (the same `Arc`s), so a
    /// process tree the dispatcher ran under a watchdog still diverts off the
    /// JSON-RPC channel even on a worker thread. Each `Arc` is minted fresh per
    /// `begin_capture`, so concurrent captures never share a buffer. Rides
    /// `CURRENT`, so it's saved/restored across suspend for free.
    pub(super) capture: Vec<Arc<Mutex<String>>>,
}

thread_local! {
    pub(super) static CURRENT: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

// ----- reduction-counted preemption ------------------------------------------

thread_local! {
    /// Reductions left in the current process's scheduling quantum. The worker
    /// resets it to `REDUCTION_BUDGET` before each `resume` (see `run_one`); `eval`
    /// decrements it via `tick`, and the process yields when it hits zero.
    static REDUCTIONS: Cell<u32> = const { Cell::new(0) };

    /// An eval **deadline** (wall clock) for this thread, or `None`. The `nest mcp`
    /// dispatcher sets it around an `eval`/`load` so a runaway (an infinite Brood
    /// loop) is aborted — see [`deadline_exceeded`], checked in eval's `'tail:`
    /// loop — instead of wedging the server. Inline (no spawn), so the dispatcher's
    /// error / panic / output-capture handling is untouched; a *native* blocking
    /// call still can't be interrupted (it never reaches the check — the same limit
    /// `(exit … :kill)` has).
    static DEADLINE: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    /// Call counter so [`deadline_exceeded`] reads the clock only every ~1024 ticks;
    /// the no-deadline fast path is a single `Cell` get, so eval's loop pays ~nothing.
    static DEADLINE_TICK: Cell<u32> = const { Cell::new(0) };

}

/// How many `eval` loop iterations a process runs before it must yield its worker
/// (cooperative fairness — the BEAM's mechanism). ~2000 ≈ the BEAM default; tunable.
/// DEBUG (bug #2): `BROOD_REDUCTIONS=<n>` overrides it — a huge value effectively disables
/// capture-mode preemption (processes run to completion, yielding only at `receive`), to
/// test whether the capture/restore mechanism is the corruption source.
fn reduction_budget() -> u32 {
    static N: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("BROOD_REDUCTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000)
    })
}

/// Called once per `eval` `'tail:` iteration. Cheap: a thread-local decrement; only
/// when the budget is exhausted does it touch `CURRENT`. The top-level VM driver does
/// its own capture-mode preemption (`tick_capture`); this `tick`/`preempt` path is the
/// fallback for non-driver runs (nested native callbacks, the tree-walker, the root
/// thread), which can't suspend, so it just refreshes the budget.
#[inline]
pub fn tick() {
    REDUCTIONS.with(|r| {
        let n = r.get();
        if n == 0 {
            preempt();
        } else {
            r.set(n - 1);
        }
    });
}

/// Dirty-CPU accounting for native builtins (BEAM's NIF-reduction model; ROADMAP
/// "Robustness gaps" survey). A long-running native can't be preempted mid-call
/// and holds its worker; what we CAN do is charge the time it held the worker
/// against the process's reduction budget afterwards, so the **next** safepoint
/// preempts promptly instead of the process keeping its quantum as if the call
/// were one reduction. ~2 reductions per µs (a 2000-reduction quantum ≈ ~1 ms of
/// Brood work), saturating — ≥ ~1 ms of native work drains the budget outright.
/// Called only from a green process (`in_capture_run`, the caller's gate): a
/// root-thread native starves no peers. Returns `Some(elapsed_ms)` when the
/// `BROOD_STALL_MS` tracer is armed and tripped, so the caller — which knows the
/// builtin's *name* — can log it (the missing half of the stall tracer: "which
/// native stalled the worker").
pub(crate) fn charge_native(t0: std::time::Instant) -> Option<u128> {
    let elapsed = t0.elapsed();
    let us = elapsed.as_micros().min(u32::MAX as u128) as u32;
    // Below ~50 µs the call is quantum-noise — skip the TLS write.
    if us >= 50 {
        let charge = us.saturating_mul(2);
        REDUCTIONS.with(|r| r.set(r.get().saturating_sub(charge)));
    }
    match crate::core::heap::stall_threshold_ms() {
        Some(ms) if elapsed.as_millis() >= ms => Some(elapsed.as_millis()),
        _ => None,
    }
}

/// Set (or clear with `None`) this thread's eval deadline. Paired set/clear by the
/// `nest mcp` dispatcher around a guarded `eval`/`load`. Thread-local: only the
/// thread running the guarded eval is affected.
pub fn set_deadline(at: Option<std::time::Instant>) {
    DEADLINE.with(|d| d.set(at));
    DEADLINE_TICK.with(|c| c.set(0));
}

/// True iff a deadline is set and has passed. The clock is read only every ~1024
/// calls, so the common (no-deadline) path is one `Cell` get — eval's loop checks
/// this every combination but pays almost nothing when no deadline is armed.
pub fn deadline_exceeded() -> bool {
    DEADLINE.with(|d| match d.get() {
        None => false,
        Some(at) => DEADLINE_TICK.with(|c| {
            let n = c.get().wrapping_add(1);
            c.set(n);
            n % 1024 == 0 && std::time::Instant::now() >= at
        }),
    })
}

/// Reduction budget exhausted at a `tick()` site. With no coroutine, there's nothing to
/// yield to here: the **top-level** VM driver does capture-mode preemption itself
/// (`tick_capture` → capture the continuation + re-enqueue). This path is reached only by
/// runs that are *not* the body driver — a NESTED native callback's `vm_apply`, the
/// tree-walker, the root thread, the prelude build — so it just accumulates the quantum's
/// reductions into `process-info`'s `:reductions` (if a process ctx exists) and refreshes
/// the budget so the caller keeps running. (A long native callback thus runs as a "dirty"
/// section — not preempted mid-call — the §7.4 carve-out.)
fn preempt() {
    if let Some(c) = CURRENT.with(|c| c.borrow().clone()) {
        c.mailbox
            .reductions
            .fetch_add(reduction_budget() as u64, Ordering::Relaxed);
    }
    REDUCTIONS.with(|r| r.set(reduction_budget()));
}

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
    if !IS_EXECUTOR.with(|e| e.get()) {
        // Root thread (or any non-executor): it blocks on its own mailbox but owns no
        // queue and runs no other process, so it strands nothing and isn't a vanishing
        // executor — no accounting, no re-route.
        return DirtyBlockGuard {
            executor: false,
            wid: None,
        };
    }
    let wid = CURRENT_WORKER.with(|c| c.get());
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
    DirtyBlockGuard {
        executor: true,
        wid,
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
        if let Some(wid) = self.wid {
            WORKER_DIRTY[wid].store(false, Ordering::Relaxed);
        }
        if self.executor {
            LIVE_EXECUTORS.fetch_add(1, Ordering::SeqCst);
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
    let stranded: Vec<Box<Process>> = crate::core::sync::lock(&WORKERS[wid].0).drain(..).collect();
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
    /// like a BEAM dirty scheduler (§7.4). Set per `vm_run_bc` entry to that call's
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
    CAPTURE_TOP_LEVEL.with(|c| c.replace(on))
}

/// RAII guard marking "the innermost frames are TREE-WALKED": clears
/// [`CAPTURE_TOP_LEVEL`] for its lifetime, restoring the previous value on drop.
///
/// A tree-walker frame is a *native* frame for capture purposes — the capture
/// driver can't reify it — so a `receive` reached through one must take the
/// blocking (§7.4 dirty-scheduler) path, never the capture path. Without this,
/// TW code reached from inside a capture-mode VM driver (`BROOD_VM=0` runs of
/// `%isolate`/`%try`/HOF callbacks, or a VM tw-defer) saw the driver's stale
/// `true`, "captured" across the un-reifiable TW frames, and the resume
/// **re-ran the whole native thunk from the top** — repeating its side effects
/// (the §8.1 footgun; surfaced 2026-07-19 as the test runner re-running
/// `:isolated` bodies under `BROOD_VM=0`). Entered at the tree-walker's two
/// entry points (`eval::eval`, `eval::apply`), so every seam into the TW is
/// covered; nested entries are cheap no-op re-clears.
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

/// Reduction tick for the capture-mode VM driver: like [`tick`] but **returns**
/// whether the budget is exhausted (so the driver captures + yields a `Preempted`).
/// Decrements otherwise. The budget is refreshed by `run_one` at the next resume.
pub(crate) fn tick_capture() -> bool {
    REDUCTIONS.with(|r| {
        let n = r.get();
        if n == 0 {
            true
        } else {
            r.set(n - 1);
            false
        }
    })
}

/// Batched [`tick_capture`]: burn `n` reductions at once — the JIT's loop back-edge
/// polls every N iterations with an in-register countdown (BEAM-style reduction
/// batching) instead of one FFI per iteration, and settles the account here so
/// scheduler fairness is unchanged (the budget depletes at the same reduction rate).
/// Only the JIT runtime callback calls it, so it is compiled out with the feature.
#[cfg(feature = "jit")]
pub(crate) fn tick_capture_n(n: u32) -> bool {
    REDUCTIONS.with(|r| {
        let cur = r.get();
        if cur < n {
            r.set(0);
            true
        } else {
            r.set(cur - n);
            false
        }
    })
}

/// [`tick`], additionally reporting whether an untrappable hard `:kill` is pending —
/// the reduction tick for every eval path that **cannot capture** a continuation.
///
/// The top-level VM body driver honours a hard kill by returning `VmOutcome::Killed`
/// from its safepoint (`tick_capture` + `capture_hard_kill_pending`). Every other
/// execution path — the tree-walker's `'tail:` loop, its `'dispatch` passthrough
/// redirect, and a *nested* VM run behind `eval`/`try`/an HOF native — used plain
/// [`tick`], which is pure accounting: on rollover, `preempt()` refreshes the budget
/// and the loop keeps going. Nothing on those paths ever looked at the kill flag, so
/// a process evaluating code via `eval`/`eval-string` was **unkillable**: `(exit pid
/// :kill)` latched the flag and the target spun forever. (Measured: a spinning child
/// died from a direct call but survived the identical loop under `eval-string`. The
/// decisive routing detail: the loop's `>`/`-` are thin-wrapper passthroughs, so the
/// hot path ticks in `passthrough_redirect_ok` — the same spot the eval *deadline*
/// once escaped through, and for the same reason.)
///
/// A non-capturing path can't *return* an outcome across its native frames, but a
/// kill doesn't need one — only unwinding. On `true` the caller raises
/// [`LispError::kill_signal`](crate::error::LispError::kill_signal): untrappable
/// (`%try` and the cleanup natives re-raise control signals), converted to
/// `VmOutcome::Killed` by the body driver exactly as the native-nested-`receive`
/// kill already is. The check runs only on rollover — once per ~2000-reduction
/// quantum — so the hot path stays a plain thread-local decrement, and on the root
/// thread (`CURRENT` unset) it is always `false`, so the REPL's own top-level eval
/// can never kill itself.
pub(crate) fn tick_reporting_hard_kill() -> bool {
    let rolled_over = REDUCTIONS.with(|r| {
        let n = r.get();
        if n == 0 {
            true
        } else {
            r.set(n - 1);
            false
        }
    });
    if !rolled_over {
        return false;
    }
    preempt();
    capture_hard_kill_pending()
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

/// Cooperatively yield so other ready work can make progress (`(yield)` / used by
/// `%isolate`'s reap to wait for killed orphans). A process can't free its worker
/// mid-eval (the continuation is only captured at a `receive`), so this hints the OS
/// scheduler (`std::thread::yield_now`) — the other worker threads run their processes,
/// so work the caller is spinning on (e.g. orphans being reaped on other workers) makes
/// progress — and refreshes the reduction budget so the caller isn't immediately
/// preempted on return. Not `std::thread::sleep`: a busy spinner shouldn't add fixed
/// latency per iteration.
pub fn yield_now() {
    REDUCTIONS.with(|r| r.set(reduction_budget()));
    std::thread::yield_now();
}

// ----- the run queue + worker pool -------------------------------------------

pub(super) static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static SPAWNED: AtomicU64 = AtomicU64::new(0);
/// How many processes have been work-stolen across worker threads since program start
/// (read by `(steal-count)`). A diagnostic of how much rebalancing the scheduler actually
/// did — 0 means placement-at-spawn kept the pool even and no thief ever needed to pull
/// work.
static STOLEN: AtomicU64 = AtomicU64::new(0);
/// How many times a process was re-assigned to a *different* worker when woken from a
/// park (`receive`/timer/exit) — i.e. a live migration of a mid-computation continuation
/// across worker threads (ADR-100 §7). Read by the live-migration regression test as
/// direct evidence that captured continuations actually crossed threads.
static MIGRATED: AtomicU64 = AtomicU64::new(0);
/// child pid → the pid that `spawn`ed it. Populated at `spawn`, removed at
/// `deregister` (a parent record lives only as long as the child). Backs
/// `process-info`'s `:parent` (and a future process-tree view). A side table
/// rather than a `Process` field because the `Process` isn't reachable from the
/// registry while it runs; this is.

/// The pid that spawned `pid`, or `None` for the root process (or a dead pid).
pub fn parent_of(pid: u64) -> Option<u64> {
    // The parent rides the process's own mailbox (see `Mailbox::parent`) rather than a
    // global `pid -> pid` map: same answer, no lock, and nothing to clean up on exit.
    // `0` is the root's "no parent" sentinel, reported as `None` exactly as the absent
    // map entry used to be.
    REGISTRY
        .get(pid)
        .map(|mb| mb.parent.load(Ordering::Relaxed))
        .filter(|&p| p != 0)
}
static RUNNING: AtomicUsize = AtomicUsize::new(0); // processes inside `resume` right now
static PEAK_RUNNING: AtomicUsize = AtomicUsize::new(0);
static WORKER_COUNT: AtomicUsize = AtomicUsize::new(0); // 0 = default (≈ nproc)
static ACTIVE_WORKERS: AtomicUsize = AtomicUsize::new(0); // worker threads actually started
static WORKERS_STARTED: Once = Once::new();

/// Executor threads (fixed workers + on-demand overflow drainers) that are **not**
/// currently dirty-blocked — i.e. the count able to run a runnable process *right now*.
/// A native-nested `receive` blocks its executor thread (§7.4); when every executor is
/// blocked this hits 0 and any work queued behind the blocks is stranded (no live thread
/// to drain or steal it). The scheduler then spawns an [`overflow_drain`] thread — an
/// on-demand "dirty scheduler", exactly as BEAM grows dirty schedulers — so progress is
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
static WORKERS: LazyLock<Vec<WorkerQueue>> = LazyLock::new(|| {
    (0..worker_count())
        .map(|_| (Mutex::new(VecDeque::new()), Condvar::new()))
        .collect()
});

/// Count of processes currently sitting in some worker's queue — i.e. the pool of
/// stealable work. Incremented in `enqueue` (every queueing: spawn, wake, preempt)
/// and decremented in `run_one` (the single pulled-to-run site, whether the owner
/// drained it or a thief stole it).
/// A cheap, relaxed atomic gate: an idle worker checks it before scanning peer
/// queues, so a truly-idle pool (`STEALABLE == 0`) re-parks on one atomic load
/// instead of an O(workers) scan. May briefly over-count a process popped but
/// not yet in `run_one` (a wasted scan, self-correcting) — it is a hint, never a
/// correctness gate.
static STEALABLE: AtomicUsize = AtomicUsize::new(0);

/// How long an idle worker parks before re-attempting a steal, when it has no
/// work of its own. A backstop, not the primary wakeup: a worker is woken
/// immediately when a process is enqueued onto *its* queue (a preempt re-enqueue
/// or a spawn placed here), but it is *not* notified when a **peer's** queue
/// grows — so it re-checks for stealable work every `STEAL_BACKOFF`. Short
/// enough that a steal opportunity isn't missed for long; long enough that a
/// genuinely idle pool wakes rarely (each wake is a single `STEALABLE` load when
/// nothing is stealable). Tunable.
const STEAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(10);

/// How long a newly enqueued process is left for its **owning** worker before a thief may
/// take it. The owner is exempt — it pops its own queue without consulting this — so the
/// window costs nothing when the owner is about to drain anyway.
///
/// It exists because the two workloads want opposite things from a spawn, and the
/// difference between them is *time*. A spawner that blocks right after spawning (the
/// `supervisor` shape: spawn, then `receive` the reply) yields within a microsecond or two,
/// and its child should run right there on a warm cache — stealing it costs that row 2.6×.
/// A spawner that keeps running (the `latency` dispatcher, busy-waiting to its next
/// scheduled instant) will not yield for a whole quantum, and its child should go to an idle
/// peer — leaving it local costs p50 27 µs against 9 µs. A few microseconds of first refusal
/// separates the two without having to predict which kind of spawner we have.
/// `BROOD_STEAL_GRACE_NS=<n>` overrides it (0 disables first refusal entirely) — the A/B
/// lever, since this constant is precisely the knob that trades the two rows against each
/// other. Read once.
fn steal_grace_ns() -> u64 {
    static G: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *G.get_or_init(|| {
        std::env::var("BROOD_STEAL_GRACE_NS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5_000)
    })
}

/// Monotonic nanoseconds since the first call — a cheap `u64` clock for `queued_at`, so a
/// `Process` carries a plain integer rather than an `Instant`.
fn now_nanos() -> u64 {
    static EPOCH: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
    EPOCH.elapsed().as_nanos() as u64
}

/// Per-worker "is currently running a process" flag. Index = `worker_id`, sized
/// to match `WORKERS`. A worker runs at most one process at a time, so this is a
/// 0/1 gauge of in-flight work. `assign_worker` folds it into a worker's load:
/// a worker draining one CPU-bound process has an *empty queue* yet is saturated,
/// and queue length alone would wrongly read it as idle. Set/cleared around the
/// `resume` in `run_one`; read (lock-free) at spawn placement.
static WORKER_BUSY: LazyLock<Vec<AtomicBool>> =
    LazyLock::new(|| (0..WORKERS.len()).map(|_| AtomicBool::new(false)).collect());

/// Per-worker "is parked on its condvar" flag, and a count of how many are. Together they
/// let [`enqueue`](crate::process::scheduler::pool::enqueue) hand a *steal opportunity* to
/// an idle peer instead of leaving it to the `STEAL_BACKOFF` timer.
///
/// Why this exists: a worker is woken when work lands on **its own** queue, never when a
/// peer's queue grows, so an idle peer could only discover stealable work on its 10 ms
/// re-probe. That is far too slow to matter, which made stealing a background rebalancer
/// rather than a latency mechanism — and left a child enqueued behind a **CPU-bound**
/// spawner waiting a full quantum. Measured on the `latency` row: p50 27 µs, of which
/// essentially all was spawn→first-instruction (with the handler doing no work at all it
/// was still 26 µs), against 11 µs when placement was forced round-robin.
///
/// The count is the cheap gate — one relaxed load on the enqueue path — so a saturated
/// pool (no one parked) pays nothing and never issues a wake.
static WORKER_PARKED: LazyLock<Vec<AtomicBool>> =
    LazyLock::new(|| (0..WORKERS.len()).map(|_| AtomicBool::new(false)).collect());
static PARKED_COUNT: AtomicUsize = AtomicUsize::new(0);

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
        match WORKERS[i].0.try_lock() {
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

/// Pick the worker a freshly **spawned** `Process` is placed on — cheaply, without
/// the O(workers) least-loaded scan [`assign_worker`] does. The BEAM model: a process
/// spawned from inside a worker goes on **that worker's own queue** (cache locality, and
/// the spawner is about to keep running anyway), and work-stealing ([`try_steal`])
/// rebalances lazily — so a spawn burst doesn't pay an O(n) `try_lock` scan per child.
/// Off-worker (the root program on the main thread, or an overflow drainer) there is no
/// "own" queue, so round-robin — one relaxed atomic add, still no scan. A dirty-blocked
/// current worker (parked in a native-nested receive, §7.4) won't drain its queue, so
/// fall back to round-robin rather than strand the child there.
fn pick_spawn_worker() -> usize {
    let n = WORKERS.len().max(1);
    if spawn_round_robin() {
        // `BROOD_SPAWN_RR=1`: place every child round-robin instead of on the spawner's
        // worker. The A/B lever for the tail-latency question — a dispatcher that spawns a
        // handler per request puts them all on its own queue, where one slow handler blocks
        // the rest, and stealing only rebalanced 12% of them on the `latency` row.
        return NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n;
    }
    match CURRENT_WORKER.with(|c| c.get()) {
        Some(w) if w < n && !WORKER_DIRTY[w].load(Ordering::Relaxed) => {
            // Local placement is right while our own queue is short — the child is about to
            // run on a warm cache and the spawner keeps going. It is wrong once we have a
            // backlog: a dispatcher spawning a handler per request piles every one onto its
            // own queue, where a single slow handler blocks all of them, and stealing only
            // rebalanced 12% of them on the `latency` row (p50 142µs local vs 11µs
            // round-robin). So spill to another worker once the backlog crosses a threshold.
            //
            // Cost is one `try_lock` on our OWN queue — uncontended in the common case, and
            // nothing like the O(workers) scan `assign_worker` runs. A failed `try_lock`
            // reads as "no backlog" and keeps the child local: the only contender for that
            // lock is a thief, whose presence means the queue is being drained anyway.
            let backlog = match WORKERS[w].0.try_lock() {
                Ok(q) => q.len(),
                Err(_) => 0,
            };
            if backlog >= spawn_spill_threshold() {
                NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n
            } else {
                w
            }
        }
        _ => NEXT_WORKER.fetch_add(1, Ordering::Relaxed) % n,
    }
}

/// Backlog at which a spawn stops going to the spawner's own worker and spills round-robin.
/// `BROOD_SPAWN_SPILL=<n>` overrides; `0` spills always (equivalent to `BROOD_SPAWN_RR=1`),
/// and a huge value restores the pre-2026-07-30 always-local behaviour for an A/B.
///
/// Default **1**: keep the child local only when our queue is *empty*, which is the case the
/// locality argument is actually about (a supervisor spawning one child, the spawner about to
/// continue). One already-waiting process means we are dispatching, and dispatching to our own
/// queue is what produced the tail. Swept on the `latency` row (median of 5, p50/p99/p99.9):
/// always-local 141/674/2902 µs · spill 8 78/457/3354 · spill 4 62/397/3852 · spill 2 48/289
/// · **spill 1 27/232/562** · always-RR 12/168/3864. Always-RR looks tempting on p50 and is
/// not the answer: it costs `supervisor` 2.6× (862 → 2223 ms) by scattering every child of a
/// request/reply spawn across workers. Spill 1 leaves that row at 843 ms.
fn spawn_spill_threshold() -> usize {
    static T: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *T.get_or_init(|| {
        std::env::var("BROOD_SPAWN_SPILL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
    })
}

/// Whether spawn placement is forced round-robin (`BROOD_SPAWN_RR=1`). Read once, cached.
fn spawn_round_robin() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("BROOD_SPAWN_RR").is_some())
}

/// Total processes spawned since program start (read by `(spawn-count)`).
pub fn spawn_count() -> u64 {
    SPAWNED.load(Ordering::SeqCst)
}

/// Total processes work-stolen across worker threads since program start
/// (read by `(steal-count)`). See [`STOLEN`].
pub fn steal_count() -> u64 {
    STOLEN.load(Ordering::SeqCst)
}

/// Total live migrations of running processes across worker threads since
/// program start (ADR-100 §7). See [`MIGRATED`].
pub fn migrate_count() -> u64 {
    MIGRATED.load(Ordering::SeqCst)
}

/// Cumulative quantum preemptions (a process exhausted its reduction budget
/// and was re-enqueued) — the scheduler half of the observability timing tier.
/// Read by `(sched-stats)`.
static PREEMPTED: AtomicU64 = AtomicU64::new(0);
/// Cumulative green-process exits (any reason). Read by `(sched-stats)`;
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
/// **Floored at 2.** A native-nested `receive` *blocks* its worker thread like a BEAM
/// dirty scheduler (§7.4) instead of capturing a continuation, and `drain_worker_queue`
/// re-routes anything queued behind it off the (now stranded) worker. With a single
/// worker there is nowhere to re-route to — `assign_worker` hands the work back to the
/// same dirty worker — so a process queued behind the block never runs. That deadlocks
/// e.g. a test that spawns a child and then receives from it under `--max-parallel 1`
/// (the spawned child is stranded; the receive times out). The pool therefore always
/// keeps at least one spare thread to drain a dirty-blocked worker, exactly as BEAM
/// runs dirty schedulers in addition to its normal scheduler count. `--max-parallel`
/// still caps how many tests the framework runs *concurrently*; it just can't starve
/// the runtime of the spare a dirty-block needs.
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
    /// Mutable access to this process's heap. Used by the L1 local-send fast path, which
    /// holds the process exclusively (taken out of `MailboxState::waiter` under the lock)
    /// while it copies a message straight in.
    pub(super) fn heap_mut(&mut self) -> &mut Heap {
        &mut self.heap
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
        let resume = self.resume.take().map(|b| *b);
        match self.program.as_mut() {
            // The root program process (ADR-135): drive the top-level forms.
            Some(prog) => crate::eval::compile::run_program_body(&mut self.heap, prog, resume),
            None => crate::eval::compile::run_process_body(&mut self.heap, self.body, resume),
        }
    }

    /// Trim this process's heap as it parks — see [`Heap::trim_parked`]. Kept here rather
    /// than in `pool` so the `heap` field stays private to this module.
    fn trim_on_park(&mut self) {
        self.heap.trim_parked();
    }

    /// Stash a captured continuation back into the process before it parks or re-queues
    /// (so the next `run_one` resumes from it).
    fn store_resume(&mut self, s: crate::eval::compile::Suspended) {
        self.resume = Some(Box::new(s));
    }

    /// Establish `CURRENT` for this quantum. Resets the per-quantum thread-locals
    /// (GC-block depth, stack base, macro block) to 0: each quantum runs on a fresh
    /// worker stack, so they are re-established here.
    fn install_ctx(&self) {
        let ctx = Ctx {
            pid: self.pid,
            mailbox: Arc::clone(&self.mailbox),
            capture: self.capture.clone(),
        };
        CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
        gc_block_set(0);
        stack_base_set(0);
        macro_block_set(0);
    }

    /// Read the (possibly mutated) capture stack back out of `CURRENT` into the process
    /// and clear `CURRENT`, so `begin_capture`/`take_capture` done this quantum persist
    /// across the next `receive` suspend, and the worker's TLS doesn't leak this
    /// process's ctx into the next one it runs.
    fn save_ctx(&mut self) {
        if let Some(cap) = CURRENT.with(|c| c.borrow().as_ref().map(|ctx| ctx.capture.clone())) {
            self.capture = cap;
        }
        CURRENT.with(|c| *c.borrow_mut() = None);
    }
}

/// `(self)` — this process's pid.
pub fn self_pid() -> u64 {
    ensure_ctx().pid
}

/// This process's mailbox arrival sequence right now, for `(ref)` to stamp a receive-mark
/// (ADR-195). Takes the mailbox lock — deliberately, rather than keeping a lock-free hint
/// republished on every push: a hint costs an atomic store per *message*, which rows that
/// never call `(ref)` still pay (measured: `pingpong` +5.7%, `ring` +4.0%). Reading it here
/// moves that cost onto ref creation, which is rarer than sending by construction, and the
/// caller is about to take the same lock to `send` anyway.
pub fn self_mailbox_seq() -> u64 {
    let mb = ensure_ctx().mailbox;
    let seq = crate::core::sync::lock(&mb.state).next_seq;
    seq
}

/// This process's pid **without** minting a context — `None` if it has none yet.
/// A process with no ctx has never `spawn`ed or messaged, so it isn't in
/// [`REGISTRY`] and can't be sharing a runtime under a drain — so skipping its
/// report is sound. The cheap read the eval / VM safepoint uses to report drain
/// liveness (ADR-091 Stage 3c); a single thread-local borrow, no allocation.
pub fn current_pid() -> Option<u64> {
    CURRENT.with(|c| c.borrow().as_ref().map(|ctx| ctx.pid))
}

/// A snapshot of every live local process's pid — the [`REGISTRY`] keys. This is
/// the live set the RUNTIME-drain union queries (ADR-091 Stage 3c). It is
/// **complete** for the drain's purposes: `spawn` registers a child (before it can
/// run) and its parent, and `receive`/`self` register the root, so every process
/// that could hold a reference to the draining generation is present.
pub fn live_pids() -> Vec<u64> {
    REGISTRY.pids()
}

/// Tear down every **permanently-parked** green process belonging to
/// `runtime` — the embedded-host teardown the mailbox waiter-slot comment
/// long flagged as missing. A process parked on a `(receive)` nothing will
/// ever send to (no deadline) holds its `Box<Process>` (and its whole heap)
/// in its mailbox's waiter slot for the life of the `REGISTRY` entry; the
/// standalone binaries exit the OS process so it never mattered there, but a
/// long-lived embedded host dropping an `Interp` leaked them until host
/// exit. Called from `Interp::drop`.
///
/// Each reaped process goes through the **normal death path** (`deregister`
/// with reason `:killed`): monitors fire `[:down …]`, links propagate, names
/// unregister, sockets close. Only *parked* waiters of *this* runtime are
/// touched — a runnable/running process is left to finish or park (a host
/// should quiesce before dropping; a later drop of another `Interp` sharing
/// the scheduler reaps nothing of ours since our entries are gone). Racing
/// `send`s are safe: the waiter is taken under the state lock, so a
/// concurrent deliver either woke it first (we skip) or queues into a
/// mailbox that is removed right after (a send to a dead pid — a no-op).
/// Returns how many processes were reaped.
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
            deregister(
                pid,
                Message::Keyword(crate::core::value::intern(pk::KILLED)),
                &p.heap,
            );
            reaped += 1;
        }
    }
    reaped
}

/// The eval / VM safepoint's cooperative RUNTIME-drain report (ADR-091 Stage 3c):
/// this process reports whether it still references the draining generation. Called
/// only when [`Heap::drain_active`](crate::core::heap::Heap::drain_active) already
/// returned true. **Throttled** to 1/`DRAIN_REPORT_STRIDE` safepoints via
/// [`Heap::drain_report_due`]: while a drain lingers, a fan-out re-reports on nearly
/// every frame, almost all cheap no-op cell-hits whose cost is the redundant contended
/// shared-atomic loads — the residual `spawn` collector overhead. Only this safepoint path
/// throttles; the parked-process inspector and the drain-completion tests call
/// `report_gen_liveness` directly. A process with no ctx (not in the live set) is skipped.
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
/// later drain forever (the parked-can't-ack problem). So the drain coordinator inspects
/// each parked process's captured continuation directly and drives its self-report: a
/// paused process's live values sit on its own heap's `roots`/`env_roots`/`live_vm_arms`
/// (the continuation is relocatable heap data — ADR-100), which [`report_gen_liveness`]
/// walks, acking it iff it's clean of the draining generation. This is exactly Erlang's
/// `check_process_code` inspecting a process externally — no wakeup, no kill. A process
/// genuinely paused *in* old-generation code stays dirty (correct: it will resume that
/// code and so still pins the generation).
///
/// [`report_gen_liveness`]: crate::core::heap::Heap::report_gen_liveness
fn report_parked_liveness() {
    // Nothing parked → nothing to inspect. An O(1) global-counter load ([`parked_count`])
    // that lets us skip the O(all-processes) `REGISTRY` walk below entirely. This is the
    // common case under a lingering fan-out drain — the workers compute-and-exit without
    // ever parking — where re-walking every live process (under the global `REGISTRY`
    // lock) on each throttled drain-advance of every worker was the O(processes²) lock
    // storm behind the ~300× `spawn` regression.
    if parked_count() == 0 {
        return;
    }
    // Snapshot the **parked** processes only: filter on the lock-free `status` cell
    // (`ST_WAITING`) while holding REGISTRY — a cheap atomic load per entry — and clone
    // just those. This is what makes a lingering fan-out drain affordable: a running
    // process (the overwhelming majority during a `spawn` fan-out) never has its mailbox
    // *state* locked here, so the O(all-processes) mailbox-lock storm that made `spawn`
    // regress ~300× becomes O(parked) locks + O(all) cheap atomic reads. A racy status
    // read is harmless: a process that parks/unparks right at the check is re-inspected
    // on the next attempt (or self-reports once running). Never hold REGISTRY across a
    // per-mailbox lock (the crate lock discipline) — hence the collect. A parked
    // process's heap is quiescent (no worker owns it) and shares the runtime `Arc`, so
    // its `report_gen_liveness` writes the shared drain ack map on its own behalf.
    let parked: Vec<(u64, Arc<Mailbox>)> = REGISTRY
        .entries()
        .into_iter()
        .filter(|(_, mb)| mb.status.load(Ordering::Relaxed) == ST_WAITING)
        .collect();
    for (pid, mailbox) in parked {
        let state = crate::core::sync::lock(&mailbox.state);
        if let Some(proc) = state.waiter.as_ref() {
            proc.heap.report_gen_liveness(pid);
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
/// can't report for itself) doesn't block the drain — Erlang `check_process_code`-style.
pub fn old_gen_drained(heap: &Heap) -> bool {
    if !heap.drain_active() {
        return false;
    }
    // Two-stage O(1) gate, then the authoritative walk. The subtle ordering point is that
    // the parked-process inspector ([`report_parked_liveness`]) must **not** run until every
    // *running* process has already acked clean — otherwise, during a `spawn` fan-out where
    // thousands of running workers pin the draining generation (each never acks until it
    // exits), its O(all-processes) `REGISTRY` walk runs on every throttled drain-advance of
    // every worker: an O(processes²) lock storm (the ~300× regression). The running workers
    // alone keep the drain un-completable, so inspecting parked processes then is pure waste.
    //
    // `acked` counts processes that reported clean; `parked_count()` counts those suspended
    // (which can't self-ack — they need the inspector). If `acked + parked < live`, at least
    // one *running* process still hasn't acked (so still pins the generation) — bail before
    // any scan; it'll self-ack at its own safepoint. Only once the sole holdouts are parked
    // do we inspect them, then fold their acks into the authoritative `gen_drained` walk.
    // Both counts are O(1) relaxed loads; over-counting the sum (a parked process that
    // already acked is in both) only opens the gate an inspection early — `gen_drained`
    // below still guards the actual free, so a racy count never frees a referenced gen.
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
    let (epoch, gen) = heap.drain_identity();
    if !old_gen_drained(heap) {
        return false;
    }
    heap.free_runtime_gen(gen, epoch)
}

/// Are we currently running inside a **green** (spawned) process — as opposed to the
/// *root* thread (the REPL / file runner / MCP dispatcher)? True when [`in_capture_run`]
/// is set (the worker is driving a process body). Used by the eval-time `unbound` raise
/// to attach a scheduler-race hint (the under-load failure mode
/// `docs/claude-demo-findings.md` flagged — concurrent prelude lookups racing).
pub fn in_green_process() -> bool {
    in_capture_run()
}

/// Wrap a local process id in a [`Value::Pid`] tagged with this runtime's node
/// identity — what `self`/`spawn` hand back. The node part makes the pid routable
/// off-node once the holder is on another runtime.
pub fn pid_value(id: u64) -> Value {
    Value::pid(crate::dist::local_node(), id)
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
        REGISTRY.insert(pid, Arc::clone(&mailbox));
        let ctx = Ctx {
            pid,
            mailbox,
            capture: Vec::new(),
        };
        *c.borrow_mut() = Some(ctx.clone());
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
mod charge_tests {
    use super::*;

    /// Dirty-CPU accounting: a long native call drains the reduction budget
    /// (proportionally, saturating), so the next tick preempts promptly; a
    /// sub-threshold call leaves the budget untouched.
    #[test]
    fn charge_native_drains_reductions_proportionally() {
        // ~1 ms of native work at 2 red/µs ≥ the whole 2000-reduction budget.
        REDUCTIONS.with(|r| r.set(reduction_budget()));
        let long_ago = std::time::Instant::now() - std::time::Duration::from_millis(2);
        charge_native(long_ago);
        assert_eq!(
            REDUCTIONS.with(|r| r.get()),
            0,
            "a ≥1ms native must drain the whole budget"
        );

        // ~100 µs charges ~200 reductions — proportional, not all-or-nothing.
        REDUCTIONS.with(|r| r.set(2000));
        let recent = std::time::Instant::now() - std::time::Duration::from_micros(100);
        charge_native(recent);
        let left = REDUCTIONS.with(|r| r.get());
        assert!(
            (1400..=1900).contains(&left),
            "a ~100µs native charges ~200 reductions, got {left} left"
        );

        // Below the 50 µs floor: budget untouched.
        REDUCTIONS.with(|r| r.set(2000));
        charge_native(std::time::Instant::now());
        assert_eq!(REDUCTIONS.with(|r| r.get()), 2000);
    }
}
