//! Sampling CPU profiler over the reified call stacks of the VM and the native tier: the
//! mechanism under `std/tool/profile.blsp` (ADR-789; first shipped in the observability timing
//! tier, ROADMAP survey gap #4).
//!
//! **Where samples come from.** A ticker thread bumps [`EPOCH`] at the requested rate. Code
//! that is running compares the epoch with the one its process last sampled at (the
//! process's [`Mark`], on its heap) at points it passes anyway, and records a sample when the
//! epoch moved: the bytecode driver each time its interpreter or a native activation hands
//! control back (a frame boundary), and a native activation at its batched reduction poll
//! (`brood_rt_tick_n`, every 128 back edges or calls). No signals, no unwinder.
//!
//! **What a sample is worth.** The ticks since the process last sampled: a process that ran
//! five ticks inside one long builtin is charged five ticks, at the frame that called it. A
//! process's mark restarts when its top-level driver is (re)entered, so time spent waiting to
//! be scheduled is never charged.
//!
//! **Whose stack.** The sample is the frame that was running and every frame around it,
//! innermost first: a driver's own frames, the native activations' names through
//! `crate::jit::JitCallCtx::parent`, and the frames of the drivers outside a nested one.
//! A nested driver (a builtin calling back into Brood, a native arm calling interpreted code)
//! cannot see the frames outside it, so a sample taken inside one is held on this thread as a
//! [`Partial`] and each enclosing driver appends its frames when control comes back to it; the
//! outermost driver on the thread commits it to the histogram. A process runs on one thread
//! for as long as a nested driver is open (a nested run cannot capture), so the thread is the
//! right place to keep them.
//!
//! **Costs.** Off (the default): one relaxed load per driver frame boundary and per native
//! reduction poll, a thread-local count per driver entry, one store per native→native call.
//! On: a stack walk and a mutex insert per sample, bounded by the ticker rate, not by the
//! workload. Policy (reports, folded stacks) is Brood, over the raw `(%profile-stop)` data.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::core::value::Symbol;

static ARMED: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU64 = AtomicU64::new(0);
/// Histogram: call stack (innermost first, named frames only) → ticks.
static SAMPLES: Mutex<Option<HashMap<Box<[Symbol]>, u64>>> = Mutex::new(None);
/// Generation counter for the ticker thread: bumping it retires the previous
/// ticker (it checks on each tick), so start/stop cycles never leak threads.
static TICKER_GEN: AtomicU64 = AtomicU64::new(0);
/// Which profile is running: bumped by every start, so a process's [`Mark`] and a held
/// [`Partial`] from an earlier profile are recognised as stale.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The one process to sample, or 0 for every process. Every process that runs takes a
/// sample per tick, so a profile of a busy program is the sum of all of them — and an
/// editor's UI loop, the process whose time a user feels, was a few percent of its own
/// profile, under language servers, checkers and buffer processes running beside it.
static TARGET_PID: AtomicU64 = AtomicU64::new(0);

/// Is the profiler armed? The driver's per-boundary gate — a relaxed load.
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// The current sampling epoch.
#[inline]
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::Relaxed)
}

/// Where a process stands with the profiler: the profile and the epoch it last sampled at.
/// Kept on the process's heap, so it travels with the process between worker threads.
#[derive(Clone, Copy, Default)]
pub struct Mark {
    generation: u64,
    epoch: u64,
}

/// Start charging this process from now: called when its top-level driver is entered, so
/// the time it spent parked or queued is not charged to the first frame it runs.
#[inline]
pub fn restart(mark: &mut Mark) {
    *mark = Mark {
        generation: GENERATION.load(Ordering::Relaxed),
        epoch: epoch(),
    };
}

/// Is a sample due for the running process? `Some(ticks)` — the ticks since it last sampled,
/// which the sample is worth — when the epoch moved and the process is one being profiled.
/// A process first seen in this profile starts its count here and samples on the next tick.
#[inline]
pub fn due(mark: &mut Mark) -> Option<u64> {
    let now = epoch();
    if mark.epoch == now {
        return None;
    }
    let generation = GENERATION.load(Ordering::Relaxed);
    let since = mark.epoch;
    let fresh = mark.generation != generation || now < since;
    *mark = Mark {
        generation,
        epoch: now,
    };
    if fresh {
        return None;
    }
    let target = TARGET_PID.load(Ordering::Relaxed);
    if target != 0 && crate::process::current_pid() != Some(target) {
        return None;
    }
    Some(now - since)
}

/// A sample still missing the frames outside the nested driver or native activation it was
/// taken in. Each enclosing driver appends its frames; the outermost commits it.
struct Partial {
    /// Innermost first.
    stack: Vec<Symbol>,
    ticks: u64,
    generation: u64,
    /// The stack ends at a native activation a bytecode driver entered (its chain reached
    /// `crate::jit::backend::PARENT_DRIVER`); `root` is that activation's name. The driver
    /// that entered it holds the same arm as its current frame and must not name it twice.
    native_root: bool,
    root: u32,
}

/// A native activation's `crate::jit::JitCallCtx`, type-erased so this module builds without
/// the JIT; only JIT code makes or reads one.
pub(crate) type NativeContext = *const ();

/// The bytecode drivers open on this thread and how many [`Partial`]s are held. A `const`
/// thread-local of plain cells: an access is one thread-pointer-relative load, which is what
/// a driver entry pays when the profiler is off.
struct Drivers {
    depth: Cell<u32>,
    held: Cell<u32>,
}

thread_local! {
    static DRIVERS: Drivers = const {
        Drivers {
            depth: Cell::new(0),
            held: Cell::new(0),
        }
    };
    static HELD: RefCell<Vec<Partial>> = const { RefCell::new(Vec::new()) };
    /// The native activation a runtime callback is running for, while it runs
    /// ([`NativeCallerScope`]): what a native activation or a driver it enters continues
    /// its stack into. Null outside every such callback, and inside a driver's extent (the
    /// driver took it, [`DriverMark`]).
    static NATIVE_CALLER: Cell<NativeContext> = const { Cell::new(std::ptr::null()) };
}

/// The native caller of the Rust code running now, for a native entry's
/// `crate::jit::JitCallCtx::parent`: null unless the profiler is armed and a runtime callback
/// named one.
#[cfg_attr(not(feature = "jit"), allow(dead_code))]
#[inline]
pub(crate) fn native_caller() -> NativeContext {
    if !armed() {
        return std::ptr::null();
    }
    NATIVE_CALLER.with(|caller| caller.get())
}

/// Names `ctx` as the native caller for the extent of a runtime callback that can run Brood
/// code (a slow call, a fast link, a staged tail call), so what that code samples continues
/// into the native activation that called it. Inert while the profiler is off.
#[cfg_attr(not(feature = "jit"), allow(dead_code))]
pub(crate) struct NativeCallerScope(Option<NativeContext>);

#[cfg_attr(not(feature = "jit"), allow(dead_code))]
impl NativeCallerScope {
    /// # Safety
    /// `ctx` is the live context the calling native activation handed the callback, and the
    /// scope is dropped before the callback returns.
    #[inline]
    pub(crate) unsafe fn enter(ctx: NativeContext) -> Self {
        if !armed() {
            return NativeCallerScope(None);
        }
        NativeCallerScope(Some(NATIVE_CALLER.with(|caller| caller.replace(ctx))))
    }
}

impl Drop for NativeCallerScope {
    #[inline]
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            NATIVE_CALLER.with(|caller| caller.set(previous));
        }
    }
}

/// One bytecode driver's place among the drivers open on its thread, held for the driver's
/// extent ([`driver_enter`]); dropping it closes the driver.
pub(crate) struct DriverMark {
    /// Drivers open on the thread, this one included: 1 is the outermost.
    depth: u32,
    /// Partials held when the driver was entered — not this driver's.
    entry_floor: u32,
    /// Partials below this are not for this driver to complete: those held at entry, and
    /// those it has already given its frames to.
    work_floor: u32,
    /// The native caller this driver took from [`NATIVE_CALLER`] at entry (null when there
    /// was none, or the profiler was off): its frames go on what the driver hands back.
    caller: NativeContext,
    took_caller: bool,
}

/// Enter a bytecode driver. Always counted, armed or not: whether a driver is the outermost
/// on its thread must be right for a profile started while it runs.
#[inline]
pub(crate) fn driver_enter() -> DriverMark {
    let (depth, held) = DRIVERS.with(|drivers| {
        let depth = drivers.depth.get() + 1;
        drivers.depth.set(depth);
        (depth, drivers.held.get())
    });
    let (caller, took_caller) = if armed() {
        (
            NATIVE_CALLER.with(|caller| caller.replace(std::ptr::null())),
            true,
        )
    } else {
        (std::ptr::null(), false)
    };
    DriverMark {
        depth,
        entry_floor: held,
        work_floor: held,
        caller,
        took_caller,
    }
}

impl Drop for DriverMark {
    #[inline]
    fn drop(&mut self) {
        let held = DRIVERS.with(|drivers| {
            drivers.depth.set(drivers.depth.get() - 1);
            drivers.held.get()
        });
        if self.took_caller {
            NATIVE_CALLER.with(|caller| caller.set(self.caller));
        }
        if held > self.entry_floor {
            driver_exit(self);
        }
    }
}

/// A driver leaves with partials it was given: put the native caller's frames on them (the
/// frames between this driver and the one outside it), or commit them when no driver is
/// outside it.
#[cold]
#[inline(never)]
fn driver_exit(mark: &DriverMark) {
    let generation = GENERATION.load(Ordering::Relaxed);
    let live = armed();
    with_held(|held| {
        let floor = mark.entry_floor as usize;
        if !live {
            held.truncate(floor);
            return;
        }
        // Only native code names a caller, so without the JIT `caller` is always null.
        #[cfg(feature = "jit")]
        if !mark.caller.is_null() {
            for partial in &mut held[floor..] {
                // SAFETY: `caller` was named by a callback that is still running beneath this
                // driver (`NativeCallerScope`), so it and its parents are live.
                let (native_root, root) = unsafe {
                    crate::jit::backend::native_stack(mark.caller.cast(), &mut partial.stack)
                };
                partial.native_root = native_root;
                partial.root = root;
            }
        }
        if mark.depth == 1 {
            commit_all(held.drain(floor..), generation);
        }
    });
}

/// The driver's sample and its partials, at a frame boundary: `current` is the frame that was
/// running (the native arm itself after a native run, `ran_native`), `callers` the driver's
/// pending frames innermost first. Called only while armed.
#[cold]
#[inline(never)]
pub(crate) fn driver_boundary(
    mark: &mut DriverMark,
    process: &mut Mark,
    current: Option<Symbol>,
    callers: impl Iterator<Item = Option<Symbol>> + Clone,
    ran_native: bool,
) {
    let ticks = due(process);
    let held = DRIVERS.with(|drivers| drivers.held.get());
    if ticks.is_none() && held <= mark.work_floor {
        return;
    }
    let generation = GENERATION.load(Ordering::Relaxed);
    let push_frames = |stack: &mut Vec<Symbol>, skip_current: bool| {
        if !skip_current && let Some(name) = current {
            stack.push(name);
        }
        stack.extend(callers.clone().flatten());
    };
    with_held(|held| {
        let floor = mark.work_floor as usize;
        for partial in &mut held[floor..] {
            let skip = ran_native
                && partial.native_root
                && current.is_some_and(|name| name == partial.root);
            push_frames(&mut partial.stack, skip);
            partial.native_root = false;
        }
        if let Some(ticks) = ticks {
            let mut stack = Vec::with_capacity(8);
            push_frames(&mut stack, false);
            held.push(Partial {
                stack,
                ticks,
                generation,
                native_root: false,
                root: u32::MAX,
            });
        }
        if mark.depth == 1 {
            commit_all(held.drain(floor..), generation);
        }
        mark.work_floor = held.len() as u32;
    });
}

/// A sample taken in native code: the native activations' names from `ctx` outward, held for
/// the drivers around it (or committed when there are none).
///
/// # Safety
/// `ctx` is the running native activation's live context.
#[cfg(feature = "jit")]
#[cold]
#[inline(never)]
pub(crate) unsafe fn native_sample(process: &mut Mark, ctx: *const crate::jit::JitCallCtx) {
    let Some(ticks) = due(process) else {
        return;
    };
    let mut stack = Vec::with_capacity(8);
    // SAFETY: the caller hands the running activation's live context (this fn's contract).
    let (native_root, root) = unsafe { crate::jit::backend::native_stack(ctx, &mut stack) };
    let generation = GENERATION.load(Ordering::Relaxed);
    if DRIVERS.with(|drivers| drivers.depth.get()) == 0 {
        commit(&stack, ticks, generation);
        return;
    }
    with_held(|held| {
        held.push(Partial {
            stack,
            ticks,
            generation,
            native_root,
            root,
        })
    });
}

/// Run `body` on this thread's held partials, keeping the count the drivers read in step.
fn with_held<T>(body: impl FnOnce(&mut Vec<Partial>) -> T) -> T {
    HELD.with(|cell| {
        let mut held = cell.borrow_mut();
        let answer = body(&mut held);
        let count = held.len() as u32;
        DRIVERS.with(|drivers| drivers.held.set(count));
        answer
    })
}

fn commit_all(partials: impl Iterator<Item = Partial>, generation: u64) {
    for partial in partials {
        if partial.generation == generation {
            commit(&partial.stack, partial.ticks, generation);
        }
    }
}

/// Add `ticks` to `stack`'s count — when the profile the sample was taken in is still the
/// one running.
fn commit(stack: &[Symbol], ticks: u64, generation: u64) {
    if generation != GENERATION.load(Ordering::Relaxed) {
        return;
    }
    let mut samples = SAMPLES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(map) = samples.as_mut() {
        *map.entry(Box::from(stack)).or_insert(0) += ticks;
    }
}

/// Arm the profiler at `hz` samples/sec (clamped to 1..=10_000), resetting the
/// histogram — sampling only process `pid` when it is `Some`, every process otherwise.
/// A second start re-arms at the new rate (the old ticker retires on its next tick via the
/// generation counter) and begins a new profile.
pub fn start(hz: u32, pid: Option<u64>) {
    TARGET_PID.store(pid.unwrap_or(0), Ordering::SeqCst);
    let hz = hz.clamp(1, 10_000);
    {
        let mut samples = SAMPLES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *samples = Some(HashMap::new());
    }
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let ticker = TICKER_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    ARMED.store(true, Ordering::SeqCst);
    let interval = std::time::Duration::from_nanos(1_000_000_000u64 / hz as u64);
    std::thread::Builder::new()
        .name("brood-profiler".into())
        .spawn(move || {
            while TICKER_GEN.load(Ordering::SeqCst) == ticker && ARMED.load(Ordering::SeqCst) {
                std::thread::sleep(interval);
                EPOCH.fetch_add(1, Ordering::Relaxed);
            }
        })
        .ok();
}

/// Disarm and take the histogram (stack → ticks), largest first. Empty if the
/// profiler was never armed (or already stopped). Samples still held for a driver that has
/// not handed control back are dropped.
pub fn stop() -> Vec<(Box<[Symbol]>, u64)> {
    ARMED.store(false, Ordering::SeqCst);
    TICKER_GEN.fetch_add(1, Ordering::SeqCst); // retire the ticker promptly
    let taken = {
        let mut samples = SAMPLES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        samples.take()
    };
    let mut out: Vec<(Box<[Symbol]>, u64)> =
        taken.map_or_default(|counts| counts.into_iter().collect());
    out.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
    out
}
