//! System monitor — kernel runtime events delivered as ordinary mailbox
//! messages to subscriber processes (the observability event stream's
//! kernel sources: BEAM `erlang:system_monitor/2` in shape, the .NET
//! EventPipe analogue at Brood's grain).
//!
//! Mechanism only: the kernel *pushes* each selected event as a plain message
//! (`process::deliver`) the moment it happens — no ring buffer, no polling, no
//! new wait primitive; a subscriber is an ordinary process using `receive`.
//! Policy (re-emitting as ADR-106 telemetry events, aggregation, `nest
//! observe`, the default crash reporter) lives in Brood (`std/telemetry.blsp`
//! `watch-runtime`, `std/proc/crash-report.blsp`).
//!
//! **One subscription per subscriber pid** (ADR-305). The stream was a single
//! last-wins slot until 2026-08-30, which made a *default* subscriber (the crash
//! reporter) impossible: any later `watch-runtime` or MCP snapshot displaced it
//! silently, and the MCP tool's clear-on-exit then left nothing armed. Now each
//! subscriber owns its own selection; re-arming the same pid replaces that pid's
//! entry only, and clearing names the pid it clears. Fan-out stays the kernel's
//! (one `deliver` per interested subscriber) because there is no other place a
//! second subscriber could get the event from.
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
//!   A subscriber may select `:exit` (every exit) or `:exit-abnormal` (only a
//!   reason other than `:normal`) — the latter is what a default crash reporter
//!   wants, and it is filtered *before* any lock or message build, so 100k
//!   clean exits cost 100k relaxed loads and nothing else.
//! - `[:system :deopt pid fn-name]` — a JIT'd arm deopted to the VM
//!   (`fn-name` is a string, or nil for an anonymous arm).
//!
//! Guards, both load-bearing:
//! - **No feedback loops:** events whose *subject* is a subscriber are never
//!   sent to that subscriber — otherwise its own GC (triggered by the event
//!   messages it receives) would emit to itself forever.
//! - **Death disarms:** `deregister` removes a subscriber's entry when it
//!   exits, so a dead subscriber doesn't keep paying event-construction cost on
//!   every spawn/exit/GC in the runtime.
//!
//! Cost when off (the default): one relaxed `AtomicU8` load at each emit
//! site. When armed: a per-kind relaxed load, then a lock + `Message` build +
//! a mailbox push per selected event per interested subscriber — the
//! subscriber opts into that traffic knowingly (as with BEAM
//! trace/system_monitor).

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;

use super::keywords as pk;
use super::message::Message;
use crate::core::value::{self, Symbol};

/// One subscriber's selection: which events it wants, and where to deliver.
#[derive(Clone, Copy)]
pub struct SysMon {
    /// The subscriber process — every selected event is `deliver`ed here.
    pub pid: u64,
    /// Report GC collections (of every process's LOCAL heap but the subscriber's).
    pub gc: bool,
    /// Only report a collection whose pause was ≥ this many µs (0 = all).
    pub gc_min_pause_us: u64,
    /// Report green-process spawns.
    pub spawn: bool,
    /// Report every green-process exit (with the structured reason).
    pub exit: bool,
    /// Report only exits whose reason is not `:normal` (the crash reporter's
    /// selection). `exit` implies this.
    pub exit_abnormal: bool,
    /// Report JIT deopts (native arm fell back to the VM).
    pub deopt: bool,
}

/// Bits of [`ARMED`]: which kinds *some* subscriber wants. Recomputed under the
/// lock on every install/clear, so an emit site can skip the lock (and the
/// message build) for a kind nobody asked for.
const WANT_GC: u8 = 1;
const WANT_SPAWN: u8 = 2;
const WANT_EXIT_ALL: u8 = 4;
const WANT_EXIT_ABNORMAL: u8 = 8;
const WANT_DEOPT: u8 = 16;

/// Fast off-check — kept in sync with `MONITORS` by every mutation.
static ARMED: AtomicU8 = AtomicU8::new(0);
static MONITORS: Mutex<Vec<SysMon>> = Mutex::new(Vec::new());

/// Is any subscriber armed? One relaxed load — the only cost every emit site
/// pays when the feature is unused.
#[inline]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed) != 0
}

/// Is some subscriber listening for abnormal exits? When one is, an uncaught
/// error's report is that subscriber's to make — the crash reporter's, normally
/// (ADR-305) — and the kernel's own `process N died: …` one-liner stands down so
/// a crash is printed once, with the trace, rather than twice. The durable
/// `.brood_crash_dump` note is written regardless.
#[inline]
pub fn crash_reported_elsewhere() -> bool {
    wants(WANT_EXIT_ABNORMAL)
}

#[inline]
fn wants(bit: u8) -> bool {
    ARMED.load(Ordering::Relaxed) & bit != 0
}

/// Recompute the per-kind bits from the subscriber list. Caller holds the lock,
/// so `armed()` can't observe a torn install/clear pair from a racing caller.
fn refresh_bits(list: &[SysMon]) {
    let mut bits = 0;
    for m in list {
        if m.gc {
            bits |= WANT_GC;
        }
        if m.spawn {
            bits |= WANT_SPAWN;
        }
        if m.exit {
            bits |= WANT_EXIT_ALL | WANT_EXIT_ABNORMAL;
        }
        if m.exit_abnormal {
            bits |= WANT_EXIT_ABNORMAL;
        }
        if m.deopt {
            bits |= WANT_DEOPT;
        }
    }
    ARMED.store(bits, Ordering::Relaxed);
}

/// Install `m` as its pid's subscription (replacing that pid's previous one, if
/// any), returning the previous configuration for that pid.
pub fn install(m: SysMon) -> Option<SysMon> {
    let mut list = crate::core::sync::lock(&MONITORS);
    let prev = remove_from(&mut list, m.pid);
    list.push(m);
    refresh_bits(&list);
    prev
}

/// Clear `pid`'s subscription, returning what it was (None if it had none).
pub fn clear(pid: u64) -> Option<SysMon> {
    let mut list = crate::core::sync::lock(&MONITORS);
    let prev = remove_from(&mut list, pid);
    refresh_bits(&list);
    prev
}

fn remove_from(list: &mut Vec<SysMon>, pid: u64) -> Option<SysMon> {
    let at = list.iter().position(|m| m.pid == pid)?;
    Some(list.remove(at))
}

/// `pid`'s subscription (for the `(proc/system-monitor)` read form).
pub fn current_for(pid: u64) -> Option<SysMon> {
    crate::core::sync::lock(&MONITORS)
        .iter()
        .find(|m| m.pid == pid)
        .copied()
}

/// Every subscription, in arming order (for `(proc/system-monitor :all)`).
pub fn all() -> Vec<SysMon> {
    crate::core::sync::lock(&MONITORS).clone()
}

/// Disarm `dead_pid`'s subscription if it had one — called by `deregister` so a
/// dead subscriber doesn't keep charging every event site in the runtime.
pub(super) fn clear_if(dead_pid: u64) {
    if !armed() {
        return;
    }
    let mut list = crate::core::sync::lock(&MONITORS);
    if remove_from(&mut list, dead_pid).is_some() {
        refresh_bits(&list);
    }
}

/// The subscribers interested in an event about `subject` (never the subject
/// itself — the feedback-loop guard), filtered by `select`.
fn interested(subject: u64, select: impl Fn(&SysMon) -> bool) -> Vec<u64> {
    crate::core::sync::lock(&MONITORS)
        .iter()
        .filter(|m| m.pid != subject && select(m))
        .map(|m| m.pid)
        .collect()
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

/// Build the event once and deliver it to each of `targets` (a clone per
/// extra subscriber — the common case is exactly one).
fn fan_out(targets: &[u64], kind: &str, subject: u64, detail: Message) {
    let msg = event(kind, subject, detail);
    for (i, pid) in targets.iter().enumerate() {
        if i + 1 == targets.len() {
            super::deliver(*pid, msg);
            break;
        }
        super::deliver(*pid, msg.clone());
    }
}

/// A collection of `subject`'s LOCAL heap finished. Call sites gate on
/// [`armed`] first; the threshold and self-exclusion are re-checked here.
pub fn emit_gc(subject: u64, pause_ns: u64, collections: u64, live: u64) {
    if !wants(WANT_GC) {
        return;
    }
    let pause_us = pause_ns / 1_000;
    let targets = interested(subject, |m| m.gc && pause_us >= m.gc_min_pause_us);
    if targets.is_empty() {
        return;
    }
    let detail = Message::Map(vec![
        (kw("pause-us"), Message::Int(pause_us as i64)),
        (kw("collections"), Message::Int(collections as i64)),
        (kw("live"), Message::Int(live as i64)),
    ]);
    fan_out(&targets, pk::SYS_GC, subject, detail);
}

/// A green process was spawned.
pub fn emit_spawn(child: u64, parent: u64) {
    if !wants(WANT_SPAWN) {
        return;
    }
    let targets = interested(child, |m| m.spawn);
    if !targets.is_empty() {
        fan_out(&targets, pk::SYS_SPAWN, child, pid_msg(parent));
    }
}

fn is_normal(reason: &Message) -> bool {
    matches!(reason, Message::Keyword(k) if *k == value::intern(pk::NORMAL))
}

/// A green process exited; `reason` is the structured monitor-visible reason.
pub fn emit_exit(subject: u64, reason: &Message) {
    // The default crash reporter selects only abnormal exits, so the common
    // clean exit must cost nothing past this load — no lock, no message.
    let normal = is_normal(reason);
    if !wants(if normal {
        WANT_EXIT_ALL
    } else {
        WANT_EXIT_ABNORMAL
    }) {
        return;
    }
    let targets = interested(subject, |m| m.exit || (m.exit_abnormal && !normal));
    if !targets.is_empty() {
        fan_out(&targets, pk::SYS_EXIT, subject, reason.clone());
    }
}

/// A JIT'd arm deopted back to the VM in `subject`.
pub fn emit_deopt(subject: u64, fn_name: Option<Symbol>) {
    if !wants(WANT_DEOPT) {
        return;
    }
    let targets = interested(subject, |m| m.deopt);
    if targets.is_empty() {
        return;
    }
    let name = match fn_name {
        Some(s) => Message::Str(value::symbol_name_ref(s).to_string()),
        None => Message::Nil,
    };
    fan_out(&targets, pk::SYS_DEOPT, subject, name);
}
