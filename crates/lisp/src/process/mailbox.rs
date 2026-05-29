//! The mailbox: where messages queue between `send` and `receive`.
//!
//! - [`Mailbox`] is one mutex around the queue + a parked-waiter slot + a
//!   condvar (for the root thread's blocking `receive`); [`REGISTRY`] maps
//!   `pid → Arc<Mailbox>` so `send` can find a target from any thread.
//! - [`deliver`] pushes a message and wakes the receiver — either by
//!   re-queueing a parked green process (`super::scheduler::enqueue`) or
//!   by signalling the condvar for a blocked root thread.
//! - [`send`] is the public surface: takes a `Value`, deep-copies it into
//!   a `Message`, dispatches by `Value::Pid` or `{:name :node}` map.
//! - [`receive_match`] is the **selective** receive — scans messages in
//!   order, runs the user's matcher in eval-tail position, removes the
//!   first match. Non-matches stay queued (Erlang semantics).
//! - [`wait_for_message`] parks the caller: a green process suspends
//!   via its coroutine yielder; the root thread blocks on the condvar.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, MapId, Symbol, Value};
use crate::error::{LispError, LispResult};
use crate::eval;

use super::message::{from_message, to_message, Message};
use super::scheduler::{
    enqueue, ensure_ctx, gc_block_save, gc_block_set, stack_base_save, stack_base_set, Ctx,
    Process, Suspend,
};
use super::timer::arm_timer;

/// Coarse run-status for `process-info` (ADR-051), stored as a lock-free cell on
/// the mailbox (which is registry-reachable, unlike the `Process` itself). Set at
/// the scheduler transitions: `enqueue` → RUNNABLE, `run_one` → RUNNING,
/// `wait_for_message` → WAITING (covers green *and* root). A dead process is gone
/// from the registry, so `process_status` returns `None` for it.
pub(super) const ST_RUNNABLE: u8 = 0;
pub(super) const ST_RUNNING: u8 = 1;
pub(super) const ST_WAITING: u8 = 2;

/// A process's mailbox. Guarded by one mutex so the "check empty → park" and
/// "deliver → wake" handshakes stay race-free (see `receive_match`/`send`/`run_one`).
pub(super) struct Mailbox {
    pub(super) state: Mutex<MailboxState>,
    /// Wakes a *root* process blocked in `receive` (greens are woken by being
    /// re-queued instead).
    pub(super) cv: Condvar,
    /// Run-status (`ST_*`) for `process-info`, written at scheduler transitions.
    pub(super) status: AtomicU8,
    /// The owning process's LOCAL heap footprint in bytes, republished each time
    /// it enters `receive` (`Heap::local_bytes`). Registry-reachable for
    /// `process-info`'s `:memory`; bump-allocated, so it reflects allocation since
    /// the last arena reset / `hibernate`, not a tracing-GC live set.
    pub(super) mem: AtomicUsize,
}

pub(super) struct MailboxState {
    pub(super) queue: VecDeque<Message>,
    /// The parked green process waiting on this mailbox, if any. `send` takes it
    /// and re-queues it. (A short-lived `Process → Arc<Mailbox> → Process` cycle
    /// while parked; broken the moment it's re-queued or the process ends.)
    pub(super) waiter: Option<Box<Process>>,
    /// How many leading messages the parked waiter already scanned and rejected
    /// (selective receive). The worker re-runs it only when a message arrives
    /// *beyond* this — not for ones it already skipped. 0 for a plain FIFO receive.
    pub(super) scanned: usize,
}

impl Mailbox {
    pub(super) fn new() -> Arc<Mailbox> {
        Arc::new(Mailbox {
            state: Mutex::new(MailboxState {
                queue: VecDeque::new(),
                waiter: None,
                scanned: 0,
            }),
            cv: Condvar::new(),
            // The root (which never goes through enqueue/run_one) keeps this; a
            // spawned green is set RUNNABLE by `enqueue` immediately after.
            status: AtomicU8::new(ST_RUNNING),
            mem: AtomicUsize::new(0),
        })
    }
}

/// pid → mailbox, for `send` to find a target from any thread.
/// `pub(super)` so the `monitor` submodule can take the REGISTRY ↔ MONITORS
/// liveness check inside its critical section (see `monitor::add_monitor`).
pub(super) static REGISTRY: LazyLock<Mutex<HashMap<u64, Arc<Mailbox>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Is `pid` currently registered (i.e. still alive)? Used by the
/// named-spawn idempotence check in `dist::spawn_or_get` to decide
/// whether to reuse an existing pid registered under a name or treat
/// the name as stale and spawn fresh. Cheap — one mutex acquisition.
pub(crate) fn is_alive(pid: u64) -> bool {
    crate::core::sync::lock(&REGISTRY).contains_key(&pid)
}

/// Push a (already-`Send`) message into local process `pid`'s mailbox and wake it;
/// a no-op if `pid` is gone. The shared tail of `send`, monitor `[:down …]`
/// delivery, and inbound node-link messages (`crate::dist`).
pub(crate) fn deliver(pid: u64, msg: Message) {
    let mailbox = crate::core::sync::lock(&REGISTRY).get(&pid).cloned();
    if let Some(mb) = mailbox {
        let mut st = crate::core::sync::lock(&mb.state);
        st.queue.push_back(msg);
        if let Some(proc) = st.waiter.take() {
            drop(st);
            enqueue(proc); // wake a parked green process
        } else {
            mb.cv.notify_one(); // wake the root thread, if it's blocked in receive
        }
    }
}

/// `(send target msg)` — copy `msg` into `target`'s mailbox and wake it. `target`
/// is a **pid** (local or remote — it carries node identity) or a `{:name :node}`
/// **registered-name address** for bootstrapping a peer before you hold its pid.
/// Routing is location-transparent: a local target delivers in-process; a remote
/// one is forwarded over the node link (`crate::dist`). Sending to a dead/unknown
/// target is a silent no-op (Erlang semantics).
pub fn send(heap: &Heap, target_val: Value, msg_val: Value) -> Result<(), LispError> {
    let msg = to_message(heap, msg_val)?;
    match target_val {
        Value::Pid { node, id } => crate::dist::route(node, crate::dist::Target::Pid(id), msg),
        Value::Map(mid) => {
            let (name, node) = read_name_address(heap, mid)?;
            crate::dist::route(node, crate::dist::Target::Name(name), msg);
        }
        _ => {
            return Err(LispError::type_err(
                "send: target must be a pid or a {:name :node} address",
            ))
        }
    }
    Ok(())
}

/// Read a `{:name <kw> :node <kw>}` registered-name address out of a map, returning
/// the `(name, node)` symbols. Accepts keyword or symbol values for each field.
pub(crate) fn read_name_address(heap: &Heap, mid: MapId) -> Result<(Symbol, Symbol), LispError> {
    let field = |key: &str| -> Result<Symbol, LispError> {
        let v = heap
            .map_get(mid, Value::Keyword(value::intern(key)))
            .ok_or_else(|| LispError::type_err("send: name address needs :name and :node keys"))?;
        match v {
            Value::Keyword(s) | Value::Sym(s) => Ok(s),
            _ => Err(LispError::type_err(
                "send: :name and :node must be keywords or symbols",
            )),
        }
    };
    Ok((field("name")?, field("node")?))
}

/// `(%receive matcher timeout on-timeout)` — selective receive. `matcher` is a unary
/// function: given a message value it returns a 0-arg thunk (the clause body, closing
/// over its bindings) on a match, or `nil` on no match. Scan the mailbox in order;
/// the first message a clause matches is removed and its body thunk **returned** —
/// not run here. The `receive` macro applies it in tail position (`((%receive …))`),
/// so a loop that tail-calls back into `receive` trampolines through eval's TCO and
/// stays O(1) native stack (running it here would nest a `receive_match` per message
/// and overflow the green-process coroutine stack). Non-matching messages stay queued
/// (Erlang selective receive). `timeout` is `nil` (wait forever) or an integer of
/// milliseconds; on expiry the `on-timeout` thunk is returned the same way (a `throw`
/// inside it still propagates through `try`/`catch`). A green process suspends while
/// waiting; the root thread blocks.
pub fn receive_match(
    heap: &mut Heap,
    matcher: Value,
    timeout: Value,
    on_timeout: Value,
) -> LispResult {
    let deadline = match timeout {
        Value::Nil => None,
        Value::Int(ms) if ms >= 0 => Some(Instant::now() + Duration::from_millis(ms as u64)),
        _ => {
            return Err(LispError::type_err(
                "receive: timeout must be an integer (milliseconds) or nil",
            ))
        }
    };
    let ctx = ensure_ctx();
    // Republish this process's LOCAL footprint and mark it running (the root never
    // goes through `run_one`, so this is where its status flips back from waiting).
    ctx.mailbox.mem.store(heap.local_bytes(), Ordering::Relaxed);
    set_self_status(&ctx, ST_RUNNING);
    let mut i = 0usize;
    loop {
        // Rebuild candidate `i` into the heap, then run the matcher *without* holding
        // the mailbox lock (the matcher calls eval). Only this process removes from
        // its own mailbox, so the scanned prefix is stable; `send` only appends.
        let candidate = {
            let st = crate::core::sync::lock(&ctx.mailbox.state);
            if i < st.queue.len() {
                Some(from_message(heap, &st.queue[i]))
            } else {
                None
            }
        };
        match candidate {
            Some(v) => {
                let thunk = eval::apply(heap, matcher, &[v], EnvId::GLOBAL)?;
                if matches!(thunk, Value::Fn(_)) {
                    // Matched — remove exactly this message, then hand the body thunk
                    // *back* (don't run it here). The `receive` macro applies it in
                    // TAIL position — `((%receive …))` — so a loop that tail-calls
                    // back into `receive` trampolines through eval's TCO and stays
                    // O(1) native stack (running it here instead nests a `receive_match`
                    // per message → green-process coroutine-stack overflow).
                    crate::core::sync::lock(&ctx.mailbox.state).queue.remove(i);
                    return Ok(thunk);
                }
                i += 1; // no clause matched — leave it queued, try the next message
            }
            None => {
                // Scanned every queued message with no match.
                if let Some(d) = deadline {
                    if Instant::now() >= d {
                        // Same trampoline: return the timeout thunk to be applied in
                        // tail position (the `receive` macro always supplies a fn).
                        return Ok(on_timeout);
                    }
                }
                wait_for_message(&ctx, i, deadline);
            }
        }
    }
}

/// Wait until a message beyond index `i` might be available, honouring `deadline`.
/// Green: record the scan position and suspend (arming a timer if there's a deadline,
/// so it wakes to check). Root: block on the mailbox condvar (with timeout). Returns
/// when the caller should re-scan from `i`.
fn wait_for_message(ctx: &Ctx, i: usize, deadline: Option<Instant>) {
    match ctx.yielder {
        // Root thread: block on the condvar (with timeout) until a send or deadline.
        None => {
            let st = crate::core::sync::lock(&ctx.mailbox.state);
            if st.queue.len() > i {
                return; // a message arrived between the scan and here — re-scan
            }
            set_self_status(ctx, ST_WAITING);
            match deadline {
                Some(d) => {
                    let now = Instant::now();
                    if now < d {
                        // Re-acquired guard dropped at end of scope (before we return).
                        let _g = ctx.mailbox.cv.wait_timeout(st, d - now);
                    }
                }
                None => {
                    let _g = ctx.mailbox.cv.wait(st);
                }
            }
        }
        // Green process: record how far we scanned (so the worker re-runs us only on
        // a *new* message — see `run_one`), then suspend. A timer wakes us at the
        // deadline; `send` wakes us on a new message.
        Some(yptr) => {
            {
                let mut st = crate::core::sync::lock(&ctx.mailbox.state);
                if st.queue.len() > i {
                    return; // raced — a message arrived; re-scan without suspending
                }
                st.scanned = i;
            }
            set_self_status(ctx, ST_WAITING);
            if let Some(d) = deadline {
                arm_timer(ctx.pid, d);
            }
            // Save this process's GC-block depth before yielding (same
            // rationale as `preempt`): the worker may run other processes
            // whose eval/macroexpand changes the thread-local before we resume.
            let saved_block = gc_block_save();
            let saved_base = stack_base_save();
            // SAFETY: the yielder is valid while this coroutine runs — which is now
            // (called from within eval, within the coroutine body). Suspending
            // returns control to the worker (`run_one`), which parks us.
            unsafe { (*yptr).suspend(Suspend::Receive) };
            // Resumed (by send or timer): the worker may have run others or migrated
            // us to another worker — re-establish the context, depth and stack base.
            super::scheduler::CURRENT.with(|c| *c.borrow_mut() = Some(ctx.clone()));
            gc_block_set(saved_block);
            stack_base_set(saved_base);
        }
    }
}

/// Re-queue green process `pid` if it's still parked, so it wakes, re-scans, and —
/// finding its deadline passed — runs its `after` clause. A no-op if `send` already
/// woke it or it re-parked on another receive; the process always re-validates its
/// own deadline, so a stale timer is harmless (at most one spurious wakeup).
pub(super) fn wake_for_timeout(pid: u64) {
    let mailbox = crate::core::sync::lock(&REGISTRY).get(&pid).cloned();
    if let Some(mb) = mailbox {
        let mut st = crate::core::sync::lock(&mb.state);
        if let Some(proc) = st.waiter.take() {
            drop(st);
            enqueue(proc);
        }
    }
}

/// Every currently-registered local pid (one entry per live mailbox). Backs
/// the `(list-processes)` primitive — agents introspecting what they've
/// spawned, and the `nest mcp` `processes` tool (`std/mcp.blsp`, ADR-036).
/// Order is unspecified (hash-map iteration); callers that care can sort.
pub fn list_local_pids() -> Vec<u64> {
    crate::core::sync::lock(&REGISTRY).keys().copied().collect()
}

/// The number of messages queued in local process `pid`'s mailbox (its receive
/// backlog), or `None` if no live local process has that id. Backs the
/// `mailbox-size` primitive — the one bit of per-process state an observer needs
/// that lives behind the scheduler registry. Takes the registry lock, then the
/// mailbox's own lock, briefly.
pub fn mailbox_len(pid: u64) -> Option<usize> {
    let mailbox = crate::core::sync::lock(&REGISTRY).get(&pid).cloned()?;
    let len = crate::core::sync::lock(&mailbox.state).queue.len();
    Some(len)
}

/// The run-status of live local process `pid`: `"running"` (executing on a
/// worker), `"runnable"` (queued, waiting for a worker turn), or `"waiting"`
/// (parked in `receive`). `None` if the pid is dead/unknown. Read from the
/// mailbox's `status` cell, which the scheduler sets at each transition. Backs
/// `process-info`'s `:status`.
pub fn process_status(pid: u64) -> Option<&'static str> {
    let mailbox = crate::core::sync::lock(&REGISTRY).get(&pid).cloned()?;
    Some(match mailbox.status.load(Ordering::Relaxed) {
        ST_RUNNING => "running",
        ST_WAITING => "waiting",
        _ => "runnable",
    })
}

/// The LOCAL heap footprint (bytes) of live local process `pid`, or `None` if the
/// pid is dead/unknown. Republished by the process each time it enters `receive`
/// (so an idle actor's figure is its resting working set); a process that never
/// `receive`s reports `0`. Bump-allocated, so it reflects allocation since the
/// last arena reset / `hibernate`. Backs `process-info`'s `:memory`.
pub fn process_mem(pid: u64) -> Option<usize> {
    let mailbox = crate::core::sync::lock(&REGISTRY).get(&pid).cloned()?;
    Some(mailbox.mem.load(Ordering::Relaxed))
}

/// Set the run-status of the *current* process (used by `receive_match` for the
/// root, which never goes through `run_one`).
fn set_self_status(ctx: &Ctx, status: u8) {
    ctx.mailbox.status.store(status, Ordering::Relaxed);
}
