//! The scheduling quantum: how much a green process may run before it yields its worker,
//! and when it must. Every preemption decision is made here — the driver's loop top, the
//! eval loop's tick, a native activation's poll — so the policy has one home and the places
//! that run code only ask.
//!
//! A quantum is a budget of reductions (~2000). Code that can capture a
//! continuation yields when the budget is spent. Code that cannot — a nested evaluation
//! behind a builtin, a native activation that cannot unwind — runs on into a fresh budget,
//! and the process OWES the scheduler a preemption that the next point that can capture
//! pays (ADR-403). And the costly preemptions — unwinding a native stack into interpreter
//! frames (ADR-401) — are paid only when some process is waiting for a worker: with none
//! queued, a preempted process would be re-queued and picked straight back up.

use super::*;

thread_local! {
    /// Reductions left in the current process's quantum (per worker thread: one quantum
    /// runs on it at a time).
    static REDUCTIONS: Cell<u32> = const { Cell::new(0) };
    /// A preemption the process owes: a quantum ran out where nothing could yield, while a
    /// process was waiting (ADR-403) — `OWED_FRESH`, or `OWED_STOPPED` once a native loop
    /// has stopped between elements to pay it ([`stop_between_elements`]).
    static OWED: Cell<u8> = const { Cell::new(OWED_NONE) };
}
const OWED_NONE: u8 = 0;
const OWED_FRESH: u8 = 1;
const OWED_STOPPED: u8 = 2;

/// How many reductions a quantum holds. `BROOD_REDUCTIONS=<n>` overrides it — a huge value
/// effectively disables preemption (processes yield only at `receive`).
fn reduction_budget() -> u32 {
    static N: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("BROOD_REDUCTIONS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000)
    })
}

/// A quantum starts on this worker (`run_one`): a full budget, nothing owed, and the
/// `:long-schedule` timer armed (ADR-404).
pub(crate) fn begin() {
    REDUCTIONS.with(|r| r.set(reduction_budget()));
    OWED.with(|o| o.set(OWED_NONE));
    crate::process::sysmon::quantum_begin();
}

/// The quantum `begin` started has ended: report it to `:long-schedule` if it ran long.
pub(crate) fn end(pid: u64) {
    crate::process::sysmon::quantum_end(pid);
}

/// The reductions the quantum used — the budget less what is left (a preempted process left
/// 0).
pub(super) fn used() -> u32 {
    reduction_budget().saturating_sub(REDUCTIONS.with(|r| r.get()))
}

/// Is some process waiting for a worker? One relaxed load of the queued count — or, on
/// wasm, is the host waiting for its thread back ([`set_host_deadline`]).
pub(crate) fn work_waiting() -> bool {
    #[cfg(test)]
    if let Some(waiting) = TEST_WORK_WAITING.with(|w| w.get()) {
        return waiting;
    }
    STEALABLE.load(Ordering::Relaxed) != 0 || host_waiting()
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// When the host that is pumping this runtime wants its thread back (a browser page,
    /// between paints): `None` when no pump is bounded.
    static HOST_DEADLINE: Cell<Option<web_time::Instant>> = const { Cell::new(None) };
}

/// Bound the pump the host is running (`pump_due`): past `deadline` a spent quantum yields
/// even with no other process queued, as it would to a waiting one, so a lone process with
/// more work than time — a game behind its frame rate — still hands the page back. On wasm
/// there is one thread and nothing else to preempt it.
#[cfg(target_arch = "wasm32")]
pub(crate) fn set_host_deadline(deadline: Option<web_time::Instant>) {
    HOST_DEADLINE.with(|h| h.set(deadline));
}

#[cfg(target_arch = "wasm32")]
fn host_waiting() -> bool {
    HOST_DEADLINE.with(|h| {
        h.get()
            .is_some_and(|deadline| web_time::Instant::now() >= deadline)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn host_waiting() -> bool {
    false
}

/// Charge one reduction where nothing can capture (the eval loop): on rollover, run on into
/// a fresh quantum ([`refill_nested_quantum`]).
#[inline]
pub fn tick() {
    REDUCTIONS.with(|r| {
        let n = r.get();
        if n == 0 {
            refill_nested_quantum();
        } else {
            r.set(n - 1);
        }
    });
}

/// Charge one reduction for the capture-mode driver: `true` when the budget is spent and it
/// should yield. The budget is refilled by the next quantum's [`begin`].
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

/// End this thread's quantum, so the next safepoint runs the once-per-quantum checks. The
/// allocator calls it for a large allocation over the soft memory limit: a loop that
/// allocates a megabyte an iteration covers hundreds of megabytes inside one quantum, past
/// the soft limit to the hard one, before the check would otherwise run (KI-242). Safe
/// inside the allocator: `REDUCTIONS` is a `const` thread-local with no destructor.
pub(crate) fn expire_quantum() {
    REDUCTIONS.with(|r| r.set(0));
}

/// What a driver's safepoint should do ([`driver_safepoint`]).
pub(crate) enum Safepoint {
    /// Keep running.
    Run,
    /// A quantum boundary: run the once-per-quantum checks, then yield the worker if
    /// `yield_now` (only ever for a driver that can capture).
    Quantum { yield_now: bool },
}

/// A driver's safepoint (the loop top, an inline loop's back edge) charges one reduction. When
/// the budget is spent a driver that can capture yields — if a process is waiting for a
/// worker; with none, a yield would re-queue the process and pick it straight back up, so it
/// runs on into a fresh quantum, as native code does (ADR-401). A driver that cannot capture
/// runs on and owes the preemption ([`refill_nested_quantum`]). And a driver that can capture
/// pays a preemption owed by code that could not (ADR-403).
#[inline]
pub(crate) fn driver_safepoint(capture: bool) -> Safepoint {
    if tick_capture() {
        let yield_now = capture && work_waiting();
        if !yield_now {
            if capture {
                fresh_quantum();
            } else {
                refill_nested_quantum();
            }
        }
        return Safepoint::Quantum { yield_now };
    }
    if capture && take_owed() {
        return Safepoint::Quantum { yield_now: true };
    }
    Safepoint::Run
}

/// A spent quantum that need not yield: book it, refill the budget, and restart the
/// `:long-schedule` timer — the worker was this process's to keep.
#[cold]
fn fresh_quantum() {
    book_quantum();
    REDUCTIONS.with(|r| r.set(reduction_budget()));
    crate::process::sysmon::quantum_restart();
}

/// Book a full quantum into the current process's `process-info` `:reductions`.
fn book_quantum() {
    if let Some(c) = CURRENT.with(|c| c.borrow().clone()) {
        c.mailbox
            .reductions
            .fetch_add(reduction_budget() as u64, Ordering::Relaxed);
    }
}

/// Take an owed preemption, if any, to pay it now.
fn take_owed() -> bool {
    OWED.with(|o| o.replace(OWED_NONE)) != OWED_NONE
}

/// Should a native loop that calls Brood per element — a fold's span (ADR-403) — stop before
/// its next element? Once per owed preemption: the loop returns, and the Brood walk around it
/// yields at its back edge if it can capture. Where nothing can, the debt stays owed for the
/// next point that can, and only a FRESH one stops the loop again — so a fold running nested
/// stops once per quantum, not once per element.
pub(crate) fn stop_between_elements() -> bool {
    OWED.with(|o| {
        let fresh = o.get() == OWED_FRESH;
        if fresh {
            o.set(OWED_STOPPED);
        }
        fresh
    })
}

/// A quantum ran out where the code cannot yield: book it into `process-info`'s
/// `:reductions`, refill the budget, and — in a green process, with a process waiting — owe
/// the preemption to the next point that can capture. The root thread never preempts and
/// owes nothing. A green process that cannot pay soon — it is still running nested when its
/// quantum rolls over — also takes its worker out of the pool until it can
/// ([`cpu_dirty_enter`]), so the waiting process gets another.
pub(crate) fn refill_nested_quantum() {
    if in_capture_run() && work_waiting() {
        OWED.with(|o| o.set(OWED_FRESH));
        cpu_dirty_enter();
    }
    book_quantum();
    REDUCTIONS.with(|r| r.set(reduction_budget()));
}

/// [`tick`], additionally raising an untrappable hard `:kill` that is pending, or a due
/// interrupt (ADR-491) — the tick for every eval path that cannot capture, so a process
/// looping under `reflect/eval-string` stays killable and interruptible. Checked only on
/// rollover (once per quantum); a kill never reaches the root thread, so the REPL can never
/// kill itself.
pub(crate) fn tick_nested() -> Result<(), crate::error::LispError> {
    if !tick_capture() {
        return Ok(());
    }
    refill_nested_quantum();
    if capture_hard_kill_pending() {
        return Err(crate::error::LispError::kill_signal());
    }
    crate::eval::check_interrupt()
}

/// What a native activation's reduction poll decided ([`native_poll`]).
#[cfg(feature = "jit")]
pub(crate) enum NativePoll {
    /// Keep running.
    Run,
    /// Preempt by unwinding to the driver (ADR-401).
    Preempt,
    /// Leave for the interpreter: a hard kill is pending, which only its safepoint raises.
    Leave,
    /// Keep running on a fresh quantum the activation could not yield at — the point where
    /// the sampling profiler, which records at interpreter frame boundaries, must sample it.
    RunOnFreshQuantum,
}

/// A native activation charges `n` reductions (a loop's batched back-edge poll, a call's entry
/// poll). `unwind`: it may preempt by unwinding (`JitCallCtx::unwind`). An activation that
/// cannot unwind has a frame between it and the driver that nothing can capture, so it runs
/// on — once it used to deopt into an interpreter that could not capture either, and simply
/// ran the rest slower.
#[cfg(feature = "jit")]
pub(crate) fn native_poll(n: u32, unwind: bool) -> NativePoll {
    if !in_capture_run() {
        // The root thread never preempts (matches the VM), but its quantum still rolls
        // over, and the rollover is where an interrupt (ADR-491) is noticed: leave for the
        // interpreter, whose safepoint raises it.
        return root_native_poll(n);
    }
    let spent = REDUCTIONS.with(|r| {
        let left = r.get();
        r.set(left.saturating_sub(n));
        left < n
    });
    if !spent {
        // Budget left — but an unwindable activation also pays a preemption owed by code
        // that could not yield itself.
        return if unwind && take_owed() {
            NativePoll::Preempt
        } else {
            NativePoll::Run
        };
    }
    if unwind {
        if work_waiting() {
            return NativePoll::Preempt;
        }
        // Nothing would run instead: go on as a fresh quantum, and do not let
        // `:long-schedule` report a worker this process had to itself.
        crate::process::sysmon::quantum_restart();
    }
    if capture_hard_kill_pending() || crate::process::current_interrupt_due() {
        return NativePoll::Leave;
    }
    refill_nested_quantum();
    NativePoll::RunOnFreshQuantum
}

/// [`native_poll`] on the root thread: charge `n`, and on the rollover leave for the
/// interpreter if an interrupt is due — the budget left spent, so the interpreter's next
/// safepoint is a quantum boundary and raises it. Otherwise run on a fresh budget.
#[cfg(feature = "jit")]
#[inline]
fn root_native_poll(n: u32) -> NativePoll {
    let spent = REDUCTIONS.with(|r| {
        let left = r.get();
        r.set(left.saturating_sub(n));
        left < n
    });
    if !spent {
        return NativePoll::Run;
    }
    if crate::process::current_interrupt_due() {
        return NativePoll::Leave;
    }
    REDUCTIONS.with(|r| r.set(reduction_budget()));
    NativePoll::Run
}

/// Charge `n` reductions for work a native loop did without ticking — a fold's span of
/// primitive steps (ADR-403). Nothing yields here: the next safepoint does, if this spent
/// the budget.
pub(crate) fn charge(n: u32) {
    REDUCTIONS.update(|reductions| reductions.saturating_sub(n));
}

/// Dirty-CPU accounting for native builtins (charged in reductions). A long-running
/// native cannot be preempted mid-call and holds its worker; what we CAN do is charge the
/// time it held the worker against the budget afterwards, so the **next** safepoint
/// preempts promptly. ~2 reductions per µs (a 2000-reduction quantum ≈ ~1 ms of Brood
/// work), saturating. Called only from a green process. Returns `Some(elapsed_ms)` when
/// the `BROOD_STALL_MS` tracer is armed and tripped, so the caller — which knows the
/// builtin's *name* — can log it.
pub(crate) fn charge_native(t0: web_time::Instant) -> Option<u128> {
    let elapsed = t0.elapsed();
    let us = elapsed.as_micros().min(u32::MAX as u128) as u32;
    // Below ~50 µs the call is quantum-noise — skip the TLS write.
    if us >= 50 {
        let charge = us.saturating_mul(2);
        REDUCTIONS.update(|reductions| reductions.saturating_sub(charge));
    }
    match crate::core::heap::stall_threshold_ms() {
        Some(ms) if elapsed.as_millis() >= ms => Some(elapsed.as_millis()),
        _ => None,
    }
}

/// Cooperatively yield so other ready work can make progress (`(yield)`, `%isolate`'s reap).
/// A process cannot free its worker mid-eval, so this hints the OS scheduler — the other
/// worker threads run their processes — and refreshes the budget so the caller is not
/// preempted on return. Not `sleep`: a busy spinner should not add latency per iteration.
pub fn yield_now() {
    REDUCTIONS.with(|r| r.set(reduction_budget()));
    std::thread::yield_now();
}

/// Test hook: set this thread's remaining budget, so a unit test can drive the capture-mode
/// VM through preemptions without a scheduler.
#[cfg(all(test, feature = "jit"))]
pub(crate) fn set_reduction_budget_for_test(n: u32) {
    REDUCTIONS.with(|r| r.set(n));
}

#[cfg(test)]
thread_local! {
    static TEST_WORK_WAITING: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Unit tests that stand in for a scheduler: answer `work_waiting` on THIS thread as
/// `waiting` says (`None` restores the real answer). Thread-local on purpose: the queued
/// count is process-wide, and tests running beside this one spawn processes, so a test
/// that needed "nothing is waiting" read a stranger's queue and preempted anyway.
#[cfg(all(test, feature = "jit"))]
pub(crate) fn test_work_waiting(waiting: Option<bool>) {
    TEST_WORK_WAITING.with(|w| w.set(waiting));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dirty-CPU accounting: a long native call drains the reduction budget
    /// (proportionally, saturating), so the next tick preempts promptly; a
    /// sub-threshold call leaves the budget untouched.
    #[test]
    fn charge_native_drains_reductions_proportionally() {
        // ~1 ms of native work at 2 red/µs ≥ the whole 2000-reduction budget.
        REDUCTIONS.with(|r| r.set(reduction_budget()));
        let long_ago = web_time::Instant::now() - std::time::Duration::from_millis(2);
        charge_native(long_ago);
        assert_eq!(
            REDUCTIONS.with(|r| r.get()),
            0,
            "a ≥1ms native must drain the whole budget"
        );

        // ~100 µs charges ~200 reductions — proportional, not all-or-nothing.
        REDUCTIONS.with(|r| r.set(2000));
        let recent = web_time::Instant::now() - std::time::Duration::from_micros(100);
        charge_native(recent);
        let left = REDUCTIONS.with(|r| r.get());
        assert!(
            (1400..=1900).contains(&left),
            "a ~100µs native charges ~200 reductions, got {left} left"
        );

        // Below the 50 µs floor: budget untouched.
        REDUCTIONS.with(|r| r.set(2000));
        charge_native(web_time::Instant::now());
        assert_eq!(REDUCTIONS.with(|r| r.get()), 2000);
    }
}
