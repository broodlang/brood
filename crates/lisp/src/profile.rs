//! Sampling CPU profiler over the VM's reified frame stacks — the "no
//! Brood-level CPU profile" half of the observability timing tier (ROADMAP
//! survey gap #4; BEAM has `:fprof`/`eprof`, .NET has dotnet-trace).
//!
//! Mechanism: a ticker thread bumps [`EPOCH`] at the requested rate; every
//! `vm_run_bc` driver compares a loop-local last-seen value against it at the
//! frame-boundary safepoint and, on change, records its current call stack
//! (the arm `fn_name`s of `cur` + the pending `BcFrame`s — data the
//! state-capture rewrite already reifies) into a global histogram. So sampling
//! needs no signals, no unwinder, and no cooperation from Brood code: any
//! process that executes at all gets sampled at its next safepoint.
//!
//! Costs: **off** (the default) — one relaxed `AtomicBool` load per driver
//! loop iteration (a frame boundary, not an instruction). **On** — one
//! `AtomicU64` load per iteration, plus a stack walk + mutex insert per
//! *sample* (rate-bounded by the ticker, not the workload).
//!
//! Coverage notes: samples are taken at VM frame boundaries, so a JIT-resident
//! loop is attributed when it yields to the driver (its reduction-budget
//! preempt — about once a quantum), and the legacy tree-walker isn't sampled.
//! Policy (aggregation, rendering, flame-graph export) belongs in Brood on top
//! of the raw `(%)` data, per the mechanism/policy split.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use crate::core::value::Symbol;

static ARMED: AtomicBool = AtomicBool::new(false);
static EPOCH: AtomicU64 = AtomicU64::new(0);
/// Histogram: call stack (innermost first, named frames only) → sample count.
static SAMPLES: Mutex<Option<HashMap<Box<[Symbol]>, u64>>> = Mutex::new(None);
/// Generation counter for the ticker thread: bumping it retires the previous
/// ticker (it checks on each tick), so start/stop cycles never leak threads.
static TICKER_GEN: AtomicU64 = AtomicU64::new(0);

/// Is the profiler armed? The driver's per-iteration gate — a relaxed load.
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// The current sampling epoch. The driver samples when this differs from its
/// loop-local last-seen value.
#[inline]
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::Relaxed)
}

/// Record one sample: the calling driver's stack of *named* frames, innermost
/// first (anonymous frames are skipped; a stack with no named frame at all is
/// recorded under the empty stack, which the Brood side labels `<anonymous>`).
pub fn record(stack: &[Symbol]) {
    let mut g = SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(map) = g.as_mut() {
        *map.entry(Box::from(stack)).or_insert(0) += 1;
    }
}

/// Arm the profiler at `hz` samples/sec (clamped to 1..=10_000), resetting the
/// histogram. Idempotent-ish: a second start re-arms at the new rate (the old
/// ticker retires on its next tick via the generation counter).
pub fn start(hz: u32) {
    let hz = hz.clamp(1, 10_000);
    {
        let mut g = SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
        *g = Some(HashMap::new());
    }
    let generation = TICKER_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    ARMED.store(true, Ordering::SeqCst);
    let interval = std::time::Duration::from_nanos(1_000_000_000u64 / hz as u64);
    std::thread::Builder::new()
        .name("brood-profiler".into())
        .spawn(move || {
            while TICKER_GEN.load(Ordering::SeqCst) == generation && ARMED.load(Ordering::SeqCst) {
                std::thread::sleep(interval);
                EPOCH.fetch_add(1, Ordering::Relaxed);
            }
        })
        .ok();
}

/// Disarm and take the histogram (stack → count), largest first. Empty if the
/// profiler was never armed (or already stopped).
pub fn stop() -> Vec<(Box<[Symbol]>, u64)> {
    ARMED.store(false, Ordering::SeqCst);
    TICKER_GEN.fetch_add(1, Ordering::SeqCst); // retire the ticker promptly
    let taken = {
        let mut g = SAMPLES.lock().unwrap_or_else(|p| p.into_inner());
        g.take()
    };
    let mut out: Vec<(Box<[Symbol]>, u64)> =
        taken.map(|m| m.into_iter().collect()).unwrap_or_default();
    out.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
    out
}
