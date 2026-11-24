//! Sampling CPU profiler over the reified call stacks of the VM and the native tier: the
//! mechanism under `std/tool/profile.blsp` (ADR-791; first shipped in the observability timing
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
//! cannot see the frames outside it, so every open driver is a [`Level`] in a thread-local
//! chain, and publishes where its pending frames are for exactly as long as it is running code
//! that could sample — its interpreter or a native activation. A sample walks the chain outward
//! and is committed at once. A process runs on one thread for as long as a nested driver is
//! open (a nested run cannot capture), so the chain is always the sampling process's.
//!
//! **Costs.** Off (the default): one relaxed load per driver frame boundary and per native
//! reduction poll, three cell stores around each interpreter or native run and a thread-local
//! link per driver entry, one store per native→native call.
//! On: a stack walk and a mutex insert per sample, bounded by the ticker rate, not by the
//! workload. Policy (reports, folded stacks) is Brood, over the raw `(%profile-stop)` data.

use std::cell::Cell;
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
/// Which profile is running: bumped by every start, so a process's [`Mark`] from an earlier
/// profile, and a sample still in flight from one, are recognised as stale.
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

/// A native activation's `crate::jit::JitCallCtx`, type-erased so this module builds without
/// the JIT; only JIT code makes or reads one.
pub(crate) type NativeContext = *const ();

/// Pushes the names of a driver's pending frames, innermost first, from the pointer it
/// published (`Level::publish`). Supplied by the driver, which knows its frame type.
pub(crate) type FrameWalk = unsafe fn(*const (), &mut Vec<Symbol>);

/// One open bytecode driver, as the samples taken inside what it calls see it: a link in a
/// thread-local chain from the innermost driver outward. It lives in the driver's own stack
/// frame and is linked for the driver's extent (`Level::link`). While the driver is running
/// its interpreter or a native activation — the only time anything it calls can sample — it
/// publishes where its pending frames are and which frame is current; outside that window the
/// frames are unpublished and a sample stops there. Only cells, so the driver and a sample
/// reading it through the chain share it without either holding a unique borrow.
pub(crate) struct Level {
    parent: Cell<*const Level>,
    /// The native activation that called the Rust code that entered this driver, taken from
    /// `NATIVE_CALLER` when it was entered: the frames between this driver and its parent.
    native_caller: Cell<NativeContext>,
    frames: Cell<*const ()>,
    current: Cell<u32>,
    walk: FrameWalk,
}

/// Keeps a [`Level`] linked; dropping it unlinks it and gives back the native caller it took.
pub(crate) struct LevelLink<'a>(&'a Level);

/// What a sample on this thread walks outward through. A `const` thread-local of plain cells:
/// an access is one thread-pointer-relative load.
struct Thread {
    /// The innermost open driver's [`Level`], or null.
    innermost: Cell<*const Level>,
    /// The native activation a runtime callback is running for, while it runs
    /// ([`NativeCallerScope`]): what a native activation or a driver it enters continues its
    /// stack into. Null outside every such callback, and inside a driver's extent (the driver
    /// took it).
    native_caller: Cell<NativeContext>,
}

thread_local! {
    static THREAD: Thread = const {
        Thread {
            innermost: Cell::new(std::ptr::null()),
            native_caller: Cell::new(std::ptr::null()),
        }
    };
}

impl Level {
    /// An unlinked level for a driver whose pending frames `walk` reads.
    #[inline]
    pub(crate) const fn new(walk: FrameWalk) -> Level {
        Level {
            parent: Cell::new(std::ptr::null()),
            native_caller: Cell::new(std::ptr::null()),
            frames: Cell::new(std::ptr::null()),
            current: Cell::new(u32::MAX),
            walk,
        }
    }

    /// Make this the innermost driver on the thread until the link is dropped. Always done,
    /// armed or not: a profile started while a driver runs must still find it.
    #[inline]
    pub(crate) fn link(&self) -> LevelLink<'_> {
        THREAD.with(|thread| {
            self.parent
                .set(thread.innermost.replace(self as *const Level));
            self.native_caller
                .set(thread.native_caller.replace(std::ptr::null()));
        });
        LevelLink(self)
    }

    /// The driver is about to run its interpreter or a native activation: `frames` points at
    /// its pending frames (what its [`FrameWalk`] reads), `current` names its current frame.
    /// The pointer must stay valid, and the frames unchanged, until [`Level::unpublish`].
    #[inline]
    pub(crate) fn publish(&self, frames: *const (), current: Option<Symbol>) {
        self.frames.set(frames);
        self.current.set(current.unwrap_or(u32::MAX));
    }

    /// The current frame changed inside the window (a deopt resuming in another chunk).
    #[inline]
    #[cfg_attr(not(feature = "jit"), allow(dead_code))]
    pub(crate) fn set_current(&self, current: Option<Symbol>) {
        self.current.set(current.unwrap_or(u32::MAX));
    }

    /// The window closed: the driver is about to change its frames.
    #[inline]
    pub(crate) fn unpublish(&self) {
        self.frames.set(std::ptr::null());
    }
}

impl Drop for LevelLink<'_> {
    #[inline]
    fn drop(&mut self) {
        let level = self.0;
        THREAD.with(|thread| {
            thread.innermost.set(level.parent.get());
            thread.native_caller.set(level.native_caller.get());
        });
    }
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
    THREAD.with(|thread| thread.native_caller.get())
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
        NativeCallerScope(Some(
            THREAD.with(|thread| thread.native_caller.replace(ctx)),
        ))
    }
}

impl Drop for NativeCallerScope {
    #[inline]
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            THREAD.with(|thread| thread.native_caller.set(previous));
        }
    }
}

/// Extend `stack` outward from where a sample was taken: through the native caller `caller`
/// (null for none), then each enclosing driver from `level` out — its current frame, its
/// pending frames, the native caller that entered it — stopping at a driver whose frames are
/// not published. `skip` names a current frame not to push again: the root of a native
/// chain that a driver entered is that driver's current frame.
fn extend_outward(
    stack: &mut Vec<Symbol>,
    mut skip: Option<u32>,
    mut caller: NativeContext,
    mut level: *const Level,
) {
    loop {
        #[cfg(feature = "jit")]
        if !caller.is_null() {
            // SAFETY: a native caller is named only by a callback still running beneath the
            // code sampling now (`NativeCallerScope`), so it and its parents are live.
            let (driver_root, root) =
                unsafe { crate::jit::backend::native_stack(caller.cast(), stack) };
            skip = driver_root.then_some(root);
        }
        #[cfg(not(feature = "jit"))]
        let _ = caller;
        if level.is_null() {
            return;
        }
        // SAFETY: a linked level lives in the stack frame of a driver that is still open
        // beneath the code sampling now (links are made and dropped in stack order).
        let open = unsafe { &*level };
        let frames = open.frames.get();
        if frames.is_null() {
            return;
        }
        let current = open.current.get();
        if current != u32::MAX && skip != Some(current) {
            stack.push(current);
        }
        // SAFETY: the driver published `frames` before running the code that is sampling now
        // and changes them only after it returns ([`Level::publish`]).
        unsafe { (open.walk)(frames, stack) };
        skip = None;
        caller = open.native_caller.get();
        level = open.parent.get();
    }
}

/// The driver's sample at a frame boundary, where the frame that just ran is still current:
/// `current` and `callers` (its pending frames, innermost first) are the driver's own; the
/// drivers around it come from its [`Level`]. Called only while armed.
#[cold]
#[inline(never)]
pub(crate) fn driver_sample(
    level: &Level,
    process: &mut Mark,
    current: Option<Symbol>,
    callers: impl Iterator<Item = Option<Symbol>>,
) {
    let Some(ticks) = due(process) else {
        return;
    };
    let generation = GENERATION.load(Ordering::Relaxed);
    let mut stack = Vec::with_capacity(16);
    stack.extend(current);
    stack.extend(callers.flatten());
    extend_outward(
        &mut stack,
        None,
        level.native_caller.get(),
        level.parent.get(),
    );
    commit(&stack, ticks, generation);
}

/// A sample taken in native code: the native activations' names from `ctx` outward, then the
/// drivers around them.
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
    let generation = GENERATION.load(Ordering::Relaxed);
    let mut stack = Vec::with_capacity(16);
    // SAFETY: the caller hands the running activation's live context (this fn's contract).
    let (driver_root, root) = unsafe { crate::jit::backend::native_stack(ctx, &mut stack) };
    let innermost = THREAD.with(|thread| thread.innermost.get());
    extend_outward(
        &mut stack,
        driver_root.then_some(root),
        std::ptr::null(),
        innermost,
    );
    commit(&stack, ticks, generation);
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
/// profiler was never armed (or already stopped).
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
