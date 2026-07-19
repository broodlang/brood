//! System monitor — kernel runtime events delivered as ordinary mailbox
//! messages to **one** subscriber process (the observability event stream's
//! kernel sources: BEAM `erlang:system_monitor/2` in shape, the .NET
//! EventPipe analogue at Brood's grain).
//!
//! Mechanism only: the kernel *pushes* each selected event as a plain message
//! (`process::deliver`) the moment it happens — no ring buffer, no polling, no
//! new wait primitive; the subscriber is an ordinary process using `receive`.
//! Policy (re-emitting as ADR-106 telemetry events, aggregation, `nest
//! observe`) lives in Brood (`std/telemetry.blsp` `watch-runtime`).
//!
//! Every event has one uniform shape — `[:system <kind> <subject-pid>
//! <detail>]` — so a single `receive` arm can route all kinds:
//!
//! - `[:system :gc pid {:pause-us n :collections n :live n}]` — a collection
//!   of `pid`'s LOCAL heap finished (only pauses ≥ the configured
//!   `:gc-min-pause-us` are reported — BEAM's `long_gc` threshold).
//! - `[:system :spawn child parent]` — a green process was spawned.
//! - `[:system :exit pid reason]` — a green process exited; `reason` is the
//!   same structured value monitors see (`[:error {… :trace}]` and friends).
//! - `[:system :deopt pid fn-name]` — a JIT'd arm deopted to the VM
//!   (`fn-name` is a string, or nil for an anonymous arm).
//!
//! Guards, both load-bearing:
//! - **No feedback loops:** events whose *subject* is the monitor itself are
//!   never emitted — otherwise the monitor's own GC (triggered by the event
//!   messages it receives) would emit to itself forever.
//! - **Death disarms:** `deregister` clears the config when the monitor pid
//!   itself exits, so a dead subscriber doesn't keep paying event-construction
//!   cost on every spawn/exit/GC in the runtime.
//!
//! Cost when off (the default): one relaxed `AtomicBool` load at each emit
//! site. When armed: config read + `Message` build + a mailbox push per
//! selected event — the subscriber opts into that traffic knowingly (as with
//! BEAM trace/system_monitor).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use super::keywords as pk;
use super::message::Message;
use crate::core::value::{self, Symbol};

/// The armed configuration: which events the subscriber wants, and where.
#[derive(Clone, Copy)]
pub struct SysMon {
    /// The subscriber process — every selected event is `deliver`ed here.
    pub pid: u64,
    /// Report GC collections (of every process's LOCAL heap but the monitor's).
    pub gc: bool,
    /// Only report a collection whose pause was ≥ this many µs (0 = all).
    pub gc_min_pause_us: u64,
    /// Report green-process spawns.
    pub spawn: bool,
    /// Report green-process exits (with the structured reason).
    pub exit: bool,
    /// Report JIT deopts (native arm fell back to the VM).
    pub deopt: bool,
}

/// Fast off-check — kept in sync with `MONITOR` by [`install`]/[`clear_if`].
static ARMED: AtomicBool = AtomicBool::new(false);
static MONITOR: Mutex<Option<SysMon>> = Mutex::new(None);

/// Is any monitor armed? One relaxed load — the only cost every emit site
/// pays when the feature is unused.
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Install (or, with `None`, clear) the system monitor, returning the previous
/// configuration. Last-caller-wins — one subscriber at a time, like BEAM's
/// `system_monitor`; fan-out belongs in Brood policy.
pub fn install(m: Option<SysMon>) -> Option<SysMon> {
    let mut g = crate::core::sync::lock(&MONITOR);
    let prev = g.take();
    *g = m;
    // Store the flag while still holding the lock, so armed() can't observe a
    // torn install/clear pair from a racing second caller.
    ARMED.store(g.is_some(), Ordering::Relaxed);
    prev
}

/// The current configuration (for the `(system-monitor)` read form).
pub fn current() -> Option<SysMon> {
    *crate::core::sync::lock(&MONITOR)
}

/// Disarm if `dead_pid` is the monitor — called by `deregister` so a dead
/// subscriber doesn't keep charging every event site in the runtime.
pub(super) fn clear_if(dead_pid: u64) {
    if !armed() {
        return;
    }
    let mut g = crate::core::sync::lock(&MONITOR);
    if g.as_ref().is_some_and(|m| m.pid == dead_pid) {
        *g = None;
        ARMED.store(false, Ordering::Relaxed);
    }
}

/// The armed config, if `subject` should be reported (never the monitor
/// itself — the feedback-loop guard).
fn config_for(subject: u64) -> Option<SysMon> {
    let m = (*crate::core::sync::lock(&MONITOR))?;
    (m.pid != subject).then_some(m)
}

fn kw(s: &str) -> Message {
    Message::Keyword(value::intern(s))
}

fn pid_msg(pid: u64) -> Message {
    Message::Pid {
        node: crate::dist::local_node(),
        id: pid,
    }
}

fn event(kind: &str, subject: u64, detail: Message) -> Message {
    Message::Vector(vec![kw(pk::SYSTEM), kw(kind), pid_msg(subject), detail])
}

/// A collection of `subject`'s LOCAL heap finished. Call sites gate on
/// [`armed`] first; the threshold and self-exclusion are re-checked here.
pub fn emit_gc(subject: u64, pause_ns: u64, collections: u64, live: u64) {
    let Some(m) = config_for(subject) else { return };
    if !m.gc || pause_ns / 1_000 < m.gc_min_pause_us {
        return;
    }
    let detail = Message::Map(vec![
        (kw("pause-us"), Message::Int((pause_ns / 1_000) as i64)),
        (kw("collections"), Message::Int(collections as i64)),
        (kw("live"), Message::Int(live as i64)),
    ]);
    super::deliver(m.pid, event(pk::SYS_GC, subject, detail));
}

/// A green process was spawned.
pub fn emit_spawn(child: u64, parent: u64) {
    let Some(m) = config_for(child) else { return };
    if m.spawn {
        super::deliver(m.pid, event(pk::SYS_SPAWN, child, pid_msg(parent)));
    }
}

/// A green process exited; `reason` is the structured monitor-visible reason.
pub fn emit_exit(subject: u64, reason: &Message) {
    let Some(m) = config_for(subject) else { return };
    if m.exit {
        super::deliver(m.pid, event(pk::SYS_EXIT, subject, reason.clone()));
    }
}

/// A JIT'd arm deopted back to the VM in `subject`.
pub fn emit_deopt(subject: u64, fn_name: Option<Symbol>) {
    let Some(m) = config_for(subject) else { return };
    if m.deopt {
        let name = match fn_name {
            Some(s) => Message::Str(value::symbol_name_ref(s).to_string()),
            None => Message::Nil,
        };
        super::deliver(m.pid, event(pk::SYS_DEOPT, subject, name));
    }
}
