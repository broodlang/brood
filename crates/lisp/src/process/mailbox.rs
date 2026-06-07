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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, MapId, Symbol, Value};
use crate::process::keywords as pk;
use crate::error::{LispError, LispResult};
use crate::eval;

use super::message::{from_message, to_message, Message};
use super::scheduler::{
    enqueue, ensure_ctx, gc_block_save, gc_block_set, macro_block_save, macro_block_set,
    stack_base_save, stack_base_set, state_capture_enabled, Ctx, Process, Suspend,
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
    /// the last arena reset / collection, not a tracing-GC live set.
    pub(super) mem: AtomicUsize,
    /// The owning process's cumulative GC-collection count (`Heap::gc_counters().0`),
    /// republished alongside `mem` on each `receive`. Lets the observer flag a
    /// process that's churning memory (many collections) vs. a quiet one. Backs
    /// `process-info`'s `:collections`.
    pub(super) gc_runs: AtomicU64,
    /// The owning process's **cumulative reduction count** — the Erlang scheduling
    /// unit (≈ one per eval combination). Accumulated by the scheduler at each
    /// quantum boundary (`run_one`: `REDUCTION_BUDGET − remaining`), so it grows
    /// continuously as the process runs, not only on `receive` like `mem`/`gc_runs`.
    /// Registry-reachable for `process-info`'s `:reductions`; the observer's
    /// "is this process doing work?" signal. Exact for spawned processes; the root
    /// accrues only in whole-budget increments (it's not scheduled via `run_one`),
    /// so its figure is coarse.
    pub(super) reductions: AtomicU64,
    /// Set by `(exit pid …)`: an exit signal is pending. The lock-free fast flag;
    /// the reason lives in `MailboxState.kill`. The target notices at its next
    /// reduction tick (hard `:kill`, via `preempt`) or `receive` (soft), and `exit`
    /// kills it directly if it's already parked. Stored on the mailbox (not the
    /// `Process`) because that's what's registry-reachable from another worker.
    pub(super) kill_pending: AtomicBool,
    /// `process_flag(trap_exit, …)` (ADR-067): when set, a *linked* peer's death
    /// arrives as a trappable `[:EXIT pid reason]` message instead of killing this
    /// process. Read from another worker during the dying peer's `deregister`
    /// (link teardown), so it lives on the registry-reachable mailbox too.
    pub(super) trap_exit: AtomicBool,
    /// **Park generation** — monotonic counter bumped each time this process parks
    /// in `receive` *with a deadline* (see `wait_for_message`). It implements lazy
    /// cancellation of superseded timer entries: each `arm_timer` stamps the entry
    /// with the gen current at park time, and the timer thread (via
    /// `wake_for_timeout`) drops an entry whose gen is stale — i.e. the process has
    /// since re-parked (a new deadline) or moved on. Without this, a server looping
    /// `(receive … (after ms …))` that is woken by `send` each iteration leaves a
    /// fresh entry on the TIMERS heap every iteration, none ever cancelled, each
    /// firing a spurious wakeup when its long-past deadline finally comes due. Heap
    /// growth is still bounded by the deadline horizon (stale entries are reaped at
    /// their deadline, not before) — acceptable. The "spurious wakeups are harmless"
    /// re-validation in `receive_match` stays as the backstop.
    pub(super) timer_gen: AtomicU64,
}

pub(super) struct MailboxState {
    pub(super) queue: VecDeque<Message>,
    /// The exit reason set by `(exit pid reason)`, paired with `kill_pending`. Read
    /// (and cleared) when the target dies; written under this lock before the flag
    /// is published, so a reader that sees the flag set always sees the reason.
    pub(super) kill: Option<Message>,
    /// The parked green process waiting on this mailbox, if any. `send` takes it
    /// and re-queues it. (A short-lived `Process → Arc<Mailbox> → Process` cycle
    /// while parked; broken the moment it's re-queued or the process ends.)
    ///
    /// **Known limitation — permanently-parked waiters leak in an embedded host.**
    /// The cycle above is only "short-lived" for a process that *will* be woken. A
    /// process parked on a `(receive)` that nothing ever sends to (and no deadline)
    /// holds its `Box<Process>` here for the life of the `REGISTRY` entry, and the
    /// `Process → Mailbox → Process` cycle keeps the heap alive. The standalone
    /// binaries exit the OS process, so this is harmless there. But an embedded,
    /// long-lived `Interp` that spawns such processes and is then dropped has **no
    /// teardown path that drains permanently-parked waiters** — they (and their
    /// heaps) leak until the host process exits. Implementing a registry-wide drain
    /// on `Interp` drop is out of scope here; flagged so it isn't mistaken for a
    /// transient cycle.
    pub(super) waiter: Option<Box<Process>>,
    /// How many leading messages the parked waiter already scanned and rejected
    /// (selective receive). The worker re-runs it only when a message arrives
    /// *beyond* this — not for ones it already skipped. 0 for a plain FIFO receive.
    ///
    /// **Invariant — never reset between suspend cycles, and that is correct.**
    /// `scanned` carries no meaning while the process is *running*; it's only read
    /// in `run_one`'s `Suspend::Receive` arm (the park-or-requeue decision). And
    /// every such read is preceded, *in the same suspend cycle*, by a write in
    /// `wait_for_message` (the green branch sets `st.scanned = i` immediately before
    /// suspending). So the value `run_one` observes is always the one this cycle's
    /// `wait_for_message` just wrote — a stale value from a prior cycle can never be
    /// read, because a `Suspend::Receive` is unreachable without going through that
    /// write first. Don't add a `Suspend::Receive` path that skips the
    /// `wait_for_message` write, or this read goes stale.
    pub(super) scanned: usize,
}

impl Mailbox {
    pub(super) fn new() -> Arc<Mailbox> {
        Arc::new(Mailbox {
            state: Mutex::new(MailboxState {
                queue: VecDeque::new(),
                waiter: None,
                scanned: 0,
                kill: None,
            }),
            cv: Condvar::new(),
            // The root (which never goes through enqueue/run_one) keeps this; a
            // spawned green is set RUNNABLE by `enqueue` immediately after.
            status: AtomicU8::new(ST_RUNNING),
            mem: AtomicUsize::new(0),
            gc_runs: AtomicU64::new(0),
            reductions: AtomicU64::new(0),
            kill_pending: AtomicBool::new(false),
            trap_exit: AtomicBool::new(false),
            timer_gen: AtomicU64::new(0),
        })
    }

    /// Record a pending exit signal (`(exit pid reason)`). Stores the reason *then*
    /// publishes the flag, so any reader (`pending_kill`) that observes the flag set
    /// is guaranteed to see the reason. A later signal overwrites the reason.
    pub(super) fn request_kill(&self, reason: Message) {
        {
            let mut st = crate::core::sync::lock(&self.state);
            // A latched untrappable `:kill` is **sticky**: a later *soft* `(exit pid
            // reason)` must not overwrite it (Erlang's guarantee that `exit(pid, kill)`
            // can't be undone — otherwise a racing soft exit could downgrade the kill
            // and spare a CPU-bound target, which only honours `:kill` at `preempt`).
            // A fresh `:kill` may still upgrade a pending soft reason.
            let latched_kill =
                matches!(&st.kill, Some(existing) if super::scheduler::is_kill_reason(existing));
            if !latched_kill {
                st.kill = Some(reason);
            }
        }
        self.kill_pending.store(true, Ordering::Relaxed);
    }

    /// The pending exit reason, if any. Fast path: one atomic load returning `None`
    /// when no exit is pending (the common case, checked every `preempt`/`receive`).
    /// Used by `preempt` (hard `:kill`) and `receive_match` (soft) to decide whether
    /// to die; a clone (not a take) — the reason is finally consumed at death.
    pub(super) fn pending_kill(&self) -> Option<Message> {
        if !self.kill_pending.load(Ordering::Relaxed) {
            return None;
        }
        crate::core::sync::lock(&self.state).kill.clone()
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

/// Look up `pid`'s mailbox in the REGISTRY and, if it's a live local process, run
/// `f` against its `Arc<Mailbox>`. Returns `None` for a dead/unknown pid (so the
/// `process-info` accessors map that straight to their `None`). The registry lock
/// is dropped before `f` runs — `f` gets the cloned `Arc`, so it's free to take the
/// mailbox's own `state` lock without nesting it under REGISTRY (preserving the
/// "never hold REGISTRY while taking another lock" discipline `deregister` relies
/// on). The shared registry-lookup-then-act step behind the read-only `process_*`
/// accessors below.
fn with_mailbox<T>(pid: u64, f: impl FnOnce(&Arc<Mailbox>) -> T) -> Option<T> {
    let mailbox = crate::core::sync::lock(&REGISTRY).get(&pid).cloned()?;
    Some(f(&mailbox))
}

/// **The wakeup protocol, in one place.** Take the parked green waiter (if any)
/// out of `st` and re-queue it onto its owning worker, so it resumes, re-scans its
/// mailbox, and proceeds. Returns `true` iff a green process was woken this way.
///
/// This is the single step shared by every site that must wake a parked process:
/// `deliver` (a message arrived), `wake_for_timeout` (a `receive` deadline passed),
/// and `exit` (an exit signal must rouse a parked target so it self-kills). Route
/// all three through here so the take-and-enqueue stays identical.
///
/// **Caller holds the mailbox state lock** (`st` is the live guard). The take
/// happens under that lock, which serialises with `run_one`'s park: either we take
/// an already-parked process, or `run_one` hasn't parked it yet and will observe
/// the new state (message / `kill_pending`) when it does — exactly one path wins,
/// so a process can't end up parked-with-work-pending and stuck. The caller drops
/// the lock *before* the returned `proc` is enqueued (enqueue grabs the worker's
/// queue lock); callers that follow the lock-ordering do this by dropping `st`
/// after this returns. A `None` return means no green waiter — the caller decides
/// whether to wake a root thread blocked on the condvar instead.
pub(super) fn wake_parked(st: &mut MailboxState) -> Option<Box<Process>> {
    st.waiter.take()
}

/// Push a (already-`Send`) message into local process `pid`'s mailbox and wake it;
/// a no-op if `pid` is gone. The shared tail of `send`, monitor `[:down …]`
/// delivery, and inbound node-link messages (`crate::dist`).
pub(crate) fn deliver(pid: u64, msg: Message) {
    let mailbox = crate::core::sync::lock(&REGISTRY).get(&pid).cloned();
    if let Some(mb) = mailbox {
        let mut st = crate::core::sync::lock(&mb.state);
        st.queue.push_back(msg);
        if let Some(proc) = wake_parked(&mut st) {
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
    ctx.mailbox
        .gc_runs
        .store(heap.gc_counters().0, Ordering::Relaxed);
    set_self_status(&ctx, ST_RUNNING);
    // `matcher` and `on_timeout` are needed across every loop iteration, but
    // `eval::apply` (the matcher) can now collect at ANY eval depth (ADR-061),
    // which relocates these LOCAL closure handles. Root them on the operand stack
    // and re-read the relocated handles each use; tear the region down on every
    // return path. (The mailbox lock is dropped before `apply` — the matcher
    // calls eval; only this process removes from its own mailbox, so the scanned
    // prefix is stable and `send` only appends.)
    let rbase = heap.roots_len();
    heap.push_root(matcher); // rbase + 0
    heap.push_root(on_timeout); // rbase + 1
    let mut i = 0usize;
    loop {
        // Scan the queued messages for a ready clause (advancing `i` past
        // non-matches). The wait, below, is the only blocking step — this split is
        // the seam the coming state-capture path uses: there, a `None` becomes a
        // *suspend signal* returned to the scheduler instead of a `wait_for_message`.
        match scan_mailbox(heap, &ctx, rbase, &mut i) {
            Ok(Some(thunk)) => {
                heap.truncate_roots(rbase);
                return Ok(thunk);
            }
            Ok(None) => {
                // Scanned every queued message with no match.
                if let Some(d) = deadline {
                    if Instant::now() >= d {
                        // Same trampoline: return the timeout thunk to be applied in
                        // tail position (the `receive` macro always supplies a fn).
                        let on_timeout = heap.root_at(rbase + 1);
                        heap.truncate_roots(rbase);
                        return Ok(on_timeout);
                    }
                }
                // State-capture path (ADR-100 §8): a clean no-match in a green process
                // becomes a *suspend control signal* returned to the scheduler instead
                // of freezing the coroutine in `wait_for_message`. It rides the error
                // channel up through the `%receive` native to the bytecode driver
                // (`vm_run_bc`), which captures the VM continuation as relocatable heap
                // data and parks it. Drop this scan's operand roots first (the match /
                // timeout returns do the same) so the driver's `%receive` `Inst::Call`
                // re-runs against a clean operand stack on resume. Root thread (no
                // yielder) and default-off builds fall through to the coroutine wait.
                if state_capture_enabled() && ctx.yielder.is_some() {
                    heap.truncate_roots(rbase);
                    return Err(LispError::suspend(deadline));
                }
                wait_for_message(&ctx, i, deadline);
            }
            Err(e) => {
                heap.truncate_roots(rbase);
                return Err(e);
            }
        }
    }
}

/// Scan the mailbox from index `*i` for the first message a clause matches, advancing
/// `*i` past non-matching messages (Erlang selective receive — non-matches stay
/// queued). Returns `Ok(Some(thunk))` — the matched clause body, with that message
/// removed, to be applied in tail position by the caller — or `Ok(None)` when every
/// currently-queued message was scanned with no match (the caller then waits and
/// re-scans; in the coming state-capture path it instead returns a suspend signal).
///
/// This does the matcher `apply` (which can collect at any eval depth — ADR-061 — so
/// the operand-rooted `matcher` at `rbase+0` is re-read each candidate) but **never
/// blocks/yields for a message** — that's the caller's `wait_for_message`. It is thus
/// the reusable scan for both the corosensei wait and the future scheduler-parked
/// resume. The `receive`-boundary kill check rides at the top (a soft `(exit …)` — and
/// any hard `:kill` not caught at `preempt` — dies here with its reason rather than
/// taking another message; the root has no yielder and can't be killed this way).
fn scan_mailbox(
    heap: &mut Heap,
    ctx: &Ctx,
    rbase: usize,
    i: &mut usize,
) -> Result<Option<Value>, LispError> {
    loop {
        if let Some(yptr) = ctx.yielder {
            if let Some(reason) = ctx.mailbox.pending_kill() {
                // SAFETY: the yielder is valid while this coroutine runs (we're
                // inside its body, called from eval). `run_one` retires us on `Kill`.
                unsafe { (*yptr).suspend(Suspend::Kill(reason)) };
            }
        }
        // Rebuild candidate `*i` into the heap (no eval here → no collection).
        let candidate = {
            let st = crate::core::sync::lock(&ctx.mailbox.state);
            if *i < st.queue.len() {
                Some(from_message(heap, &st.queue[*i]))
            } else {
                None
            }
        };
        match candidate {
            Some(v) => {
                let matcher = heap.root_at(rbase);
                let thunk = eval::apply(heap, matcher, &[v], EnvId::GLOBAL)?;
                if matches!(thunk, Value::Fn(_)) {
                    // Matched — remove exactly this message and hand the body thunk
                    // *back* (don't run it here). The `receive` macro applies it in
                    // TAIL position — `((%receive …))` — so a loop that tail-calls back
                    // into `receive` trampolines through eval's TCO and stays O(1)
                    // native stack (running it here instead nests a `receive_match` per
                    // message → green-process coroutine-stack overflow).
                    crate::core::sync::lock(&ctx.mailbox.state).queue.remove(*i);
                    return Ok(Some(thunk));
                }
                *i += 1; // no clause matched — leave it queued, try the next message
            }
            None => return Ok(None), // scanned to the end with no match
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
                // Stamp the timer entry with a fresh park generation. If this
                // process is woken (by `send`) and re-parks before the deadline,
                // the next `arm_timer` carries a higher gen, so the timer thread's
                // `wake_for_timeout` discards *this* now-superseded entry instead of
                // firing a spurious wakeup. `fetch_add` returns the value to stamp;
                // the bump makes every earlier outstanding entry stale.
                let gen = ctx.mailbox.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
                arm_timer(ctx.pid, d, gen);
            }
            // Save this process's GC-block depth before yielding (same
            // rationale as `preempt`): the worker may run other processes
            // whose eval/macroexpand changes the thread-local before we resume.
            let saved_block = gc_block_save();
            let saved_base = stack_base_save();
            let saved_macro = macro_block_save();
            // SAFETY: the yielder is valid while this coroutine runs — which is now
            // (called from within eval, within the coroutine body). Suspending
            // returns control to the worker (`run_one`), which parks us.
            unsafe { (*yptr).suspend(Suspend::Receive) };
            // Resumed (by send or timer): the worker may have run others or migrated
            // us to another worker — re-establish the context, depth and stack base.
            crate::process::scheduler::CURRENT.with(|c| *c.borrow_mut() = Some(ctx.clone()));
            gc_block_set(saved_block);
            stack_base_set(saved_base);
            macro_block_set(saved_macro);
        }
    }
}

/// Re-queue green process `pid` if it's still parked on the deadline identified by
/// `gen`, so it wakes, re-scans, and — finding its deadline passed — runs its
/// `after` clause. `gen` is the park generation the timer entry was stamped with
/// (`arm_timer`); if the mailbox's `timer_gen` has since advanced, this entry is a
/// **superseded** deadline (the process re-parked with a newer one, or moved on),
/// so we drop it without waking — lazy timer cancellation (see `Mailbox::timer_gen`).
/// A no-op too if `send` already woke it. The process always re-validates its own
/// deadline, so even a wakeup that slips through is harmless (at most one spurious).
pub(super) fn wake_for_timeout(pid: u64, gen: u64) {
    let mailbox = crate::core::sync::lock(&REGISTRY).get(&pid).cloned();
    if let Some(mb) = mailbox {
        // Stale entry — the process has re-parked (or moved on) since this timer
        // was armed. Skip it: the live deadline has its own, current-gen entry.
        if mb.timer_gen.load(Ordering::Relaxed) != gen {
            return;
        }
        let mut st = crate::core::sync::lock(&mb.state);
        if let Some(proc) = wake_parked(&mut st) {
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
    with_mailbox(pid, |mb| crate::core::sync::lock(&mb.state).queue.len())
}

/// The run-status of live local process `pid`: `"running"` (executing on a
/// worker), `"runnable"` (queued, waiting for a worker turn), or `"waiting"`
/// (parked in `receive`). `None` if the pid is dead/unknown. Read from the
/// mailbox's `status` cell, which the scheduler sets at each transition. Backs
/// `process-info`'s `:status`.
pub fn process_status(pid: u64) -> Option<&'static str> {
    with_mailbox(pid, |mb| match mb.status.load(Ordering::Relaxed) {
        ST_RUNNING => pk::STATUS_RUNNING,
        ST_WAITING => pk::STATUS_WAITING,
        _ => pk::STATUS_RUNNABLE,
    })
}

/// The LOCAL heap footprint (bytes) of live local process `pid`, or `None` if the
/// pid is dead/unknown. Republished by the process each time it enters `receive`
/// (so an idle actor's figure is its resting working set); a process that never
/// `receive`s reports `0`. Bump-allocated, so it reflects allocation since the
/// last arena reset / collection. Backs `process-info`'s `:memory`.
pub fn process_mem(pid: u64) -> Option<usize> {
    with_mailbox(pid, |mb| mb.mem.load(Ordering::Relaxed))
}

/// The cumulative GC-collection count of live local process `pid`, or `None` if
/// the pid is dead/unknown. Republished by the process each time it enters
/// `receive` (so an idle actor's figure is its count as of its last receive);
/// a process that never `receive`s reports `0`. Backs `process-info`'s
/// `:collections`.
pub fn process_gc_runs(pid: u64) -> Option<u64> {
    with_mailbox(pid, |mb| mb.gc_runs.load(Ordering::Relaxed))
}

/// The cumulative reduction count of live local process `pid`, or `None` if the
/// pid is dead/unknown. Updated by the scheduler at every quantum boundary (see
/// `run_one`), so unlike `:memory`/`:collections` it reflects work up to the
/// process's *latest* scheduling point, not just its last `receive`. Backs
/// `process-info`'s `:reductions`. Exact for spawned processes; the root accrues
/// only in whole-budget increments (it bypasses `run_one`), so its figure is coarse.
pub fn process_reductions(pid: u64) -> Option<u64> {
    with_mailbox(pid, |mb| mb.reductions.load(Ordering::Relaxed))
}

/// Set the run-status of the *current* process (used by `receive_match` for the
/// root, which never goes through `run_one`).
fn set_self_status(ctx: &Ctx, status: u8) {
    ctx.mailbox.status.store(status, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lazy-cancellation core (Fix 1): each park-with-deadline bumps the
    /// mailbox's `timer_gen` and stamps its timer entry with the new value; the
    /// timer thread fires only the entry whose gen is still current. This unit test
    /// exercises the gen bookkeeping without standing up the timer thread or a real
    /// coroutine — it models the sequence of arms and asserts which entries are
    /// considered live vs. superseded.
    #[test]
    fn timer_gen_supersedes_earlier_entries() {
        let mb = Mailbox::new();
        assert_eq!(mb.timer_gen.load(Ordering::Relaxed), 0, "fresh mailbox at gen 0");

        // Mirror `wait_for_message`'s green branch: `fetch_add(1) + 1` is the gen
        // stamped onto the entry just armed. A server looping
        // `(receive … (after ms …))` woken by `send` each iteration arms repeatedly.
        let gen1 = mb.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let gen2 = mb.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let gen3 = mb.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
        assert_eq!((gen1, gen2, gen3), (1, 2, 3), "each park stamps a fresh gen");

        // The staleness gate `wake_for_timeout` applies: an entry fires only while
        // its gen equals the mailbox's current `timer_gen`. After three arms only
        // the third (the live deadline) is current; the first two are superseded and
        // would be dropped without a spurious wakeup.
        let current = mb.timer_gen.load(Ordering::Relaxed);
        assert_eq!(current, 3);
        assert_ne!(gen1, current, "first park's entry is superseded");
        assert_ne!(gen2, current, "second park's entry is superseded");
        assert_eq!(gen3, current, "only the latest park's entry is live");
    }

    /// Sticky `:kill` (the `request_kill` hardening): once an untrappable `:kill` is
    /// latched, a racing *soft* `(exit …)` must not overwrite it — otherwise the soft
    /// reason would downgrade the kill and a CPU-bound target (which honours only
    /// `:kill`, at `preempt`) could survive. A `:kill` may still upgrade a pending
    /// soft reason, and two soft reasons never become a kill.
    #[test]
    fn kill_is_sticky_against_a_racing_soft_exit() {
        use crate::process::scheduler::is_kill_reason;
        let kill = || Message::Keyword(value::intern(pk::KILL));
        let soft = || Message::Keyword(value::intern("shutdown"));

        // :kill, then a soft exit → still :kill (no downgrade).
        let mb = Mailbox::new();
        mb.request_kill(kill());
        mb.request_kill(soft());
        assert!(
            is_kill_reason(&mb.pending_kill().unwrap()),
            "a soft exit must not downgrade a latched :kill"
        );

        // soft, then :kill → upgraded to :kill.
        let mb = Mailbox::new();
        mb.request_kill(soft());
        mb.request_kill(kill());
        assert!(
            is_kill_reason(&mb.pending_kill().unwrap()),
            "a :kill must upgrade a pending soft reason"
        );

        // soft, then another soft → last soft wins; never spuriously a kill.
        let mb = Mailbox::new();
        mb.request_kill(soft());
        mb.request_kill(Message::Keyword(value::intern("other")));
        assert!(
            !is_kill_reason(&mb.pending_kill().unwrap()),
            "two soft reasons never become a kill"
        );
    }

    /// State-capture seam (ADR-100 §8.4 step 1): under `BROOD_STATE_CAPTURE`, a green
    /// process whose `receive` scans an empty mailbox with no match produces a
    /// `Control::Suspend` *control signal* (to be captured by the VM driver and parked
    /// by the scheduler) instead of blocking in `wait_for_message`. We model a green
    /// process by installing a `CURRENT` ctx with a (dummy, never-dereferenced) yielder
    /// and an empty mailbox, then assert `receive_match` returns that control signal —
    /// not a real error, and never blocks (the test would hang if it took the wait path).
    #[test]
    fn empty_receive_in_a_green_process_suspends_under_the_flag() {
        // Arm the flag before the first `state_capture_enabled()` read (it caches). This
        // is the only state-capture caller in the lib unit-test binary; nextest also
        // isolates each test in its own process.
        std::env::set_var("BROOD_STATE_CAPTURE", "1");
        assert!(state_capture_enabled(), "the flag must be armed for this test");

        // A green ctx: a yielder makes `ctx.yielder.is_some()` true (the greenness
        // check); it is never dereferenced on the suspend path (no kill pending, and
        // the suspend returns before any `(*yptr).suspend(..)`), so a dangling pointer
        // is safe here.
        let mailbox = Mailbox::new();
        let ctx = Ctx {
            pid: 999_999,
            mailbox: Arc::clone(&mailbox),
            yielder: Some(core::ptr::NonNull::dangling().as_ptr() as *const _),
            capture: Vec::new(),
        };
        crate::process::scheduler::CURRENT.with(|c| *c.borrow_mut() = Some(ctx));

        let mut heap = Heap::new();
        // Empty mailbox, no timeout: the scan finds nothing and the green+flag branch
        // returns the suspend signal. `matcher`/`on_timeout` are never applied (the
        // queue is empty), so plain `nil`s suffice.
        let r = receive_match(&mut heap, Value::Nil, Value::Nil, Value::Nil);
        crate::process::scheduler::CURRENT.with(|c| *c.borrow_mut() = None); // don't leak the dummy ctx
        let err = r.expect_err("an empty receive under the flag must signal a suspend, not return");
        assert!(err.is_control(), "the suspend must be a control signal, not a real error");
        assert!(
            matches!(err.control, Some(crate::error::Control::Suspend { deadline: None })),
            "an indefinite receive carries no deadline"
        );
    }
}
