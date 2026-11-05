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
//!   first match. Non-matches stay queued.
//! - a clean top-level green `receive` on an empty mailbox **captures** its
//!   continuation (returning a suspend signal for the scheduler to park);
//!   [`wait_for_message`] **blocks** the caller on the condvar for the cases
//!   that can't capture — the root thread, a tree-walked body, and a
//!   native-nested receive (the §7.4 dirty carve-out).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, MapId, Symbol, Value};
use crate::error::{LispError, LispResult};
use crate::eval;
use crate::process::keywords as pk;

use super::message::{Message, from_message, to_message_to_runtime};
// Only the `dev-tools` trace path converts a whole trace to a message; importing it
// unconditionally makes a `--no-default-features` build warn.
#[cfg(feature = "dev-tools")]
use super::message::to_message;
use super::scheduler::{CurrentProcess, Process, current_process, wake_enqueue};
use super::timer::{SchedDeadline, arm_timer};

/// Coarse run-status for `process-info` (ADR-051), stored as a lock-free cell on
/// the mailbox (which is registry-reachable, unlike the `Process` itself). Set at
/// the scheduler transitions: `enqueue` → RUNNABLE, `run_one` → RUNNING,
/// `wait_for_message` → WAITING (covers green *and* root). A dead process is gone
/// from the registry, so `process_status` returns `None` for it.
pub(super) const ST_RUNNABLE: u8 = 0;
pub(super) const ST_RUNNING: u8 = 1;
pub(super) const ST_WAITING: u8 = 2;

/// How many processes are currently **parked** (`ST_WAITING`) across the whole
/// runtime. Maintained by [`set_status`] on every `ST_WAITING`-boundary crossing
/// (and decremented by `deregister` for a process killed while parked). It lets the
/// RUNTIME-drain coordinator's parked-process inspector ([`super::scheduler::
/// report_parked_liveness`]) skip its O(all-processes) `REGISTRY` scan when nothing is
/// parked — without the gate that scan, on every drain-advance of every worker, was an
/// O(processes²) lock storm. Relaxed: it only gates an optimization — an over-count runs
/// a scan that finds nothing, and every park increments it before it can matter.
static PARKED: super::scheduler::Padded<AtomicUsize> =
    super::scheduler::Padded(AtomicUsize::new(0));

/// Currently-parked process count — see [`PARKED`]. O(1) relaxed load.
pub(super) fn parked_count() -> usize {
    PARKED.load(Ordering::Relaxed)
}

/// Set a mailbox's run-status, keeping the global [`PARKED`] count in step. Every
/// status transition funnels through here so a `ST_WAITING`-boundary crossing (in
/// either direction) adjusts the counter exactly once. The `swap` reads the true prior
/// status even under a race, so concurrent transitions can't double-count.
///
/// A status that already reads `new` is left alone — every `receive` marks itself
/// running, and inside a green quantum `run_one` already did, so the swap (a locked RMW
/// on a line senders read) was a no-op paid per receive. Skipping it is the swap's own
/// outcome linearised at the load: a transition racing in after the load lands as if it
/// came after our write, which the swap admitted too, and no counter moves on a skip.
pub(super) fn set_status(mb: &Mailbox, new: u8) {
    if mb.status.load(Ordering::Relaxed) == new {
        return;
    }
    let old = mb.status.swap(new, Ordering::Relaxed);
    match (old == ST_WAITING, new == ST_WAITING) {
        (false, true) => {
            PARKED.fetch_add(1, Ordering::Relaxed);
        }
        (true, false) => {
            PARKED.fetch_sub(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Drop a mailbox's parked accounting when it leaves the registry (a process killed
/// while parked never runs a status transition back out of `ST_WAITING`). Idempotent:
/// swaps the status to `ST_RUNNABLE` via [`set_status`], so a second call is a no-op.
pub(super) fn clear_parked(mb: &Mailbox) {
    if mb.status.load(Ordering::Relaxed) == ST_WAITING {
        set_status(mb, ST_RUNNABLE);
    }
}

/// A process's mailbox. Guarded by one mutex so the "check empty → park" and
/// "deliver → wake" handshakes stay race-free (see `receive_match`/`send`/`run_one`).
pub(super) struct Mailbox {
    /// Which runtime this process belongs to — see [`Mailbox::set_runtime_tag`]. 0 = unset.
    pub(super) runtime_tag: AtomicU64,
    pub(super) state: Mutex<MailboxState>,
    /// Wakes a *root* process blocked in `receive` (greens are woken by being
    /// re-queued instead).
    pub(super) cv: Condvar,
    /// Run-status (`ST_*`) for `process-info`, written at scheduler transitions.
    pub(super) status: AtomicU8,
    /// The owning process's LOCAL heap footprint in bytes, republished each time
    /// it *parks* in `receive` (`Heap::local_bytes` — off the fast message-ready
    /// path, which recomputing an O(heap) walk there dominated). Registry-reachable
    /// for `process-info`'s `:memory`; bump-allocated, so it reflects allocation
    /// since the last arena reset / collection, not a tracing-GC live set.
    pub(super) mem: AtomicUsize,
    /// The owning process's whole heap ([`Heap::footprint`]) as it last measured it near
    /// the soft memory limit, in measuring period `footprint_period` — the figure the
    /// pressure selector compares a *running* process by, since only the process itself
    /// may walk its heap (`process/pressure.rs`). A parked process is measured directly.
    pub(super) footprint: AtomicUsize,
    pub(super) footprint_period: AtomicU64,
    /// The owning process's cumulative GC-collection count (`Heap::gc_counters().0`),
    /// republished alongside `mem` when it parks in `receive`. Lets the observer flag a
    /// process that's churning memory (many collections) vs. a quiet one. Backs
    /// `process-info`'s `:collections`.
    pub(super) gc_runs: AtomicU64,
    /// The owning process's **cumulative reduction count** — the scheduling
    /// unit (≈ one per eval combination). Accumulated by the scheduler at each
    /// quantum boundary (`run_one`: `REDUCTION_BUDGET − remaining`), so it grows
    /// continuously as the process runs, not only on `receive` like `mem`/`gc_runs`.
    /// Registry-reachable for `process-info`'s `:reductions`; the observer's
    /// "is this process doing work?" signal. Exact for spawned processes; the root
    /// accrues only in whole-budget increments (it's not scheduled via `run_one`),
    /// so its figure is coarse.
    pub(super) reductions: AtomicU64,
    /// CPU time the process's quanta consumed, in ns, booked at each quantum's end on the
    /// worker that ran it (`process-info`'s `:cpu-us`; see `cputime`).
    pub(super) cpu_ns: AtomicU64,
    /// For a ROOT context, pinned to its OS thread and never scheduled in quanta: that
    /// thread's CPU clock (`cputime::current_thread_clock`), read live. -1 for a green
    /// process, whose time is `cpu_ns`.
    pub(super) thread_clock: AtomicI64,
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
    /// `(proc/flag :max-mailbox n)` (ADR-307): this process's queue-length bound,
    /// `0` = unlimited (the default). Lives on the registry-reachable mailbox — like
    /// `trap_exit` — because it is the *senders* who observe it: every enqueue site
    /// compares the post-push length against it. The sender is NEVER blocked or
    /// errored and the message is still delivered (fire-and-forget stays
    /// fire-and-forget); what a breach does is arm `overflow_hit` for the receiver.
    pub(super) max_mailbox: AtomicUsize,
    /// Sticky breach flag, one word with three states: `0` clean, `1..` armed with the
    /// queue length observed when an enqueue crossed `max_mailbox`, or
    /// `OVERFLOW_LATCHED` once a probe has raised it. The receiver's own probes take it
    /// and raise a catchable `E0046` **in that process only** — the `:max-heap`
    /// protocol, with the same rescue rule. The bound is ONE-SHOT PER ARMING (KI-103):
    /// while latched no enqueue re-arms, so the catcher gets a window to drain or re-set
    /// it (re-setting re-arms) instead of dying inside the very `proc/flag` call the
    /// E0046 hint tells it to make.
    pub(super) overflow_hit: AtomicUsize,
    /// Whether the message the last `receive` took came on the PRIORITY lane (ADR-490):
    /// cleared by `proc/receive-lane` before it receives, set by the consume path only
    /// for a priority envelope — an ordinary receive pays a lane test, no store.
    pub(super) took_priority: AtomicBool,
    /// **Park generation** — monotonic counter bumped each time this process parks
    /// in `receive` *with a deadline* (see `wait_for_message`). It implements lazy
    /// cancellation of superseded timer entries: each `arm_timer` stamps the entry
    /// with the gen current at park time, and the timer thread (via
    /// `wake_for_timeout`) drops an entry whose gen is stale — i.e. the process has
    /// since re-parked (a new deadline) or moved on. Dropping only at the deadline would
    /// bound the heap by arm-rate × horizon (a server looping `(after 3600000 …)` at 10k
    /// msg/s holds ~36M dead entries), so `super::timer::arm_timer` also *compacts*
    /// against this counter. `receive_match`'s deadline re-validation is the backstop.
    pub(super) timer_gen: AtomicU64,
    /// The pid that spawned this process (`0` = none, i.e. the root). Written once before
    /// the mailbox is published to the registry and never mutated after, so `Relaxed` is
    /// enough — the registry insert is the release that publishes it.
    pub(super) parent: AtomicU64,
    /// The `%isolate` scope this process was **spawned under** (`0` = none). Written once
    /// before publication, like `parent`, and never mutated after.
    ///
    /// The ownership stamp `%isolate` reaps by. The `parent` chain is not durable — a
    /// grandchild's link dies with its intermediate parent — while a stamp copied at
    /// spawn survives every death above it (KI-89).
    pub(super) isolate_owner: AtomicU64,
    /// The scope this process stamps on the children it spawns. Equal to `isolate_owner`
    /// except while this process is *inside* `%isolate`, which swaps in a fresh token for
    /// the duration and puts the old one back. Kept apart from `isolate_owner` so running
    /// an isolate does not erase the stamp our own parent's isolate reaps us by.
    pub(super) isolate_scope: AtomicU64,
    /// The interrupt window (ADR-491): written by an interrupter or a quit-key delivery
    /// on another thread, probed by the process at its quantum-boundary safepoints.
    pub(super) interrupt: super::interrupt::Interrupt,
}

/// What a queued envelope carries. Two shapes, because a *local* send to a **parked**
/// receiver can copy straight into that receiver's heap (L1) and skip the wire format
/// entirely, while every other send — a running receiver, a remote node, a monitor
/// `[:down …]` — still needs the heap-independent `Message`.
pub(super) enum Payload {
    /// Wire-format: rebuilt into the receiver's heap by `from_message` when popped.
    Wire(Message),
    /// Already in the receiver's heap, parked in `Heap::msg_roots` at this slot. Popping
    /// it is `msg_root_take` — no rebuild, no allocation. `tag` carries the leading
    /// keyword (if the value is a keyword-led vector), computed once at delivery so the
    /// L3 selective-receive prefilter works on this payload too — without it, a
    /// backlogged selective receive would go back to rescanning every candidate.
    Local { slot: u32, tag: Option<u32> },
}

/// A queued message plus, under `dev-tools`, the debugger causal context (ADR-174
/// send-level) the sender carried. Access to the payload is always `.msg`, uniform
/// across builds, so the receive matcher is untouched.
pub(super) struct Envelope {
    pub(super) msg: Payload,
    /// Arrival sequence, assigned under the mailbox lock by [`MailboxState::push`].
    ///
    /// **Invariant: `queue` is ALWAYS strictly increasing in `seq`.** This is what lets a
    /// `receive` pinned on a fresh `ref` binary-search (`partition_point`) to the first
    /// message that could possibly carry that ref instead of walking the backlog (the
    /// receive-mark, ADR-195) — a `partition_point` over a predicate that is not actually
    /// partitioned returns an arbitrary index, so a break in this ordering makes a pinned
    /// receive **silently skip a matchable message**.
    ///
    /// Exactly three operations touch the queue, and all three preserve it:
    /// - [`MailboxState::push`] appends with a strictly increasing `next_seq`;
    /// - `queue.remove(i)` (the scan's optimistic pop / the match consume) drops one
    ///   element and shifts the rest, which cannot reorder anything;
    /// - [`reinsert_at_seq`] puts an optimistically-popped candidate back **at its
    ///   seq-ordered position**, not merely at its recorded scan index — see there for
    ///   why those two can differ.
    ///
    /// Nothing else may insert into `queue`. `debug_assert`s in `reinsert_at_seq` guard
    /// the one non-obvious case.
    pub(super) seq: u64,
    #[cfg(feature = "dev-tools")]
    pub(super) trace: Option<Message>,
    /// Inbound flow-control credit — `Some` only for a message a flow-controlled source
    /// (the net reactor's `[:tcp sock data]` chunks) delivered with
    /// [`deliver_credited`]; `None` for every other message. The credit is returned to
    /// its source when the envelope is **dropped**, which is exactly when the message
    /// leaves the mailbox for good: consumed by a `receive` (selective or not), dropped
    /// at delivery to a gone pid, cleared at process death. The candidate pop/re-insert
    /// of a selective scan MOVES the envelope, so a message merely skipped never returns
    /// its credit. Boxed so the field is one pointer on the queue every message shares.
    pub(super) credit: Option<Box<DeliveryCredit>>,
    /// Which lane the message travels in (ADR-490): an ordinary message, a PRIORITY one
    /// (`proc/send-priority`, and everything a window or tab delivers as user input) that
    /// `proc/take-priority` takes ahead of the backlog, or a LATEST one that a newer
    /// message with the same key replaces while it is still queued (`proc/send-latest`).
    /// A plain `receive` ignores the lane: it sees every message in arrival order.
    pub(super) lane: Lane,
}

/// The lane of a queued [`Envelope`] (ADR-490). A tag and a pointer: `Latest` boxes its key,
/// so an envelope that never uses a key pays 16 bytes for the field and allocates nothing.
/// Crate-visible because a `Send` frame carries it across a node link (ADR-492).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Lane {
    Normal,
    /// Taken ahead of the rest by `proc/take-priority`, oldest first.
    Priority,
    /// Replaced, while still queued, by a later `proc/send-latest` with the same key —
    /// the key's printed form, compared as a string at delivery.
    // Boxed though a `String` already is: the box keeps the lane at 16 bytes on every
    // envelope, where an inline `String` (or a fat `Box<str>`) would make it 24.
    #[allow(clippy::box_collection)]
    Latest(Box<String>),
}

/// A source that bounds how much it has delivered into mailboxes and not yet had
/// taken — the inbound half of socket backpressure (the net reactor pauses reading a
/// socket past a high-water mark and resumes when enough of its chunks are consumed).
/// `release` runs on whatever thread drops the envelope, possibly under that mailbox's
/// state lock, so it must not block or take a mailbox lock.
///
/// A source may also take back a message it delivered and that has not been received
/// (`withdrawn`): a receive scanning past it drops it unseen, which returns its credit.
/// A window's key-repeat gate does, so the repeat queued behind a slow frame is not
/// played after the key is released. Read under the mailbox lock, so it must not block.
pub(crate) trait CreditSource: Send + Sync {
    fn release(&self, cost: usize);
    fn withdrawn(&self) -> bool {
        false
    }
}

/// One message's share of a [`CreditSource`]'s budget. The source charges `cost` when
/// it builds the credit; dropping the credit returns it. Riding inside the [`Envelope`]
/// makes every way a message can leave the mailbox return it — there is no removal
/// path to forget.
pub(crate) struct DeliveryCredit {
    source: Arc<dyn CreditSource>,
    cost: usize,
}

impl DeliveryCredit {
    pub(crate) fn new(source: Arc<dyn CreditSource>, cost: usize) -> DeliveryCredit {
        DeliveryCredit { source, cost }
    }

    /// Whether the source has taken this message back (see [`CreditSource::withdrawn`]).
    fn withdrawn(&self) -> bool {
        self.source.withdrawn()
    }
}

impl Drop for DeliveryCredit {
    fn drop(&mut self) {
        self.source.release(self.cost);
    }
}

impl MailboxState {
    /// Push `env` onto the queue, stamping its arrival sequence and republishing the
    /// lock-free hint. Every enqueue goes through here so `seq` is never left at 0.
    #[inline]
    pub(super) fn push(&mut self, mut env: Envelope) {
        env.seq = self.next_seq;
        self.next_seq += 1;
        if let Lane::Latest(key) = &env.lane {
            self.drop_superseded(key);
        }
        self.note_queued(&env);
        self.queue.push_back(env);
    }

    /// Count `env`, entering the queue, in the lane bookkeeping: the PRIORITY count, the
    /// LATEST index.
    #[inline]
    fn note_queued(&mut self, env: &Envelope) {
        match &env.lane {
            Lane::Normal => {}
            Lane::Priority => self.priority_queued += 1,
            Lane::Latest(key) => {
                self.latest_queued
                    .get_or_insert_with(Default::default)
                    .insert(key.as_str().into(), env.seq);
            }
        }
    }

    /// Uncount `env`, leaving the queue — [`note_queued`](Self::note_queued) undone.
    #[inline]
    fn note_unqueued(&mut self, env: &Envelope) {
        match &env.lane {
            Lane::Normal => {}
            Lane::Priority => self.priority_queued -= 1,
            Lane::Latest(key) => {
                if let Some(index) = self.latest_queued.as_deref_mut()
                    && index.get(key.as_str()) == Some(&env.seq)
                {
                    index.remove(key.as_str());
                }
            }
        }
    }

    /// Remove the envelope at `index`, keeping the lane bookkeeping exact. Every removal
    /// from the queue goes through here.
    pub(super) fn remove_at(&mut self, index: usize) -> Option<Envelope> {
        let env = self.queue.remove(index)?;
        self.note_unqueued(&env);
        Some(env)
    }

    /// Insert `env` at `index` — a scan candidate put back in seq order — keeping the lane
    /// bookkeeping exact.
    fn insert_at(&mut self, index: usize, env: Envelope) {
        self.note_queued(&env);
        self.queue.insert(index, env);
    }

    /// Empty the queue, and its lane bookkeeping with it.
    pub(super) fn clear_queue(&mut self) {
        self.queue.clear();
        self.priority_queued = 0;
        self.latest_queued = None;
    }

    /// Drop the queued LATEST-lane envelope carrying `key`, which the one being pushed
    /// supersedes (ADR-490). At most one is queued — each push of a key drops the previous
    /// — and `latest_queued` says which, by seq, so it is found by a binary search on the
    /// seq-ordered queue rather than a walk of the backlog. Removing preserves the queue's
    /// seq order; `scanned` (the park gate's count of messages a parked receive already
    /// rejected) shrinks with a removal inside it, or the replacement would read as already
    /// scanned and the receiver would sleep through it. The owner's own scan in progress
    /// re-anchors on seq (`scan_mailbox`), so a removal under it is a shift it already
    /// handles. An envelope the owner has OUT of the queue as a scan candidate is not
    /// indexed: the scan asks [`holds_latest`](Self::holds_latest) before handing it over,
    /// and drops it then.
    fn drop_superseded(&mut self, key: &str) {
        let Some(seq) = self.latest_seq(key) else {
            return;
        };
        let index = self.queue.partition_point(|e| e.seq < seq);
        if self.queue.get(index).is_some_and(|e| e.seq == seq) {
            drop(self.remove_at(index));
            if index < self.scanned {
                self.scanned -= 1;
            }
        }
    }

    /// Whether a LATEST envelope with `key` is queued. Asked about a candidate the scan
    /// took OUT of the queue: at most one envelope per key is ever queued, so one queued
    /// now arrived while the candidate was out, and supersedes it. O(1).
    fn holds_latest(&self, key: &str) -> bool {
        self.latest_seq(key).is_some()
    }

    /// The seq of the queued LATEST envelope carrying `key`, if one is queued.
    fn latest_seq(&self, key: &str) -> Option<u64> {
        self.latest_queued.as_deref()?.get(key).copied()
    }
}

impl Envelope {
    #[inline]
    pub(super) fn plain(msg: Message) -> Self {
        Envelope {
            msg: Payload::Wire(msg),
            seq: 0, // replaced by `MailboxState::push`
            #[cfg(feature = "dev-tools")]
            trace: None,
            credit: None,
            lane: Lane::Normal,
        }
    }
}

pub(super) struct MailboxState {
    pub(super) queue: VecDeque<Envelope>,
    /// Next arrival sequence to hand out. Only ever increases; see [`Envelope::seq`].
    pub(super) next_seq: u64,
    /// The exit reason set by `(exit pid reason)` / link propagation, paired with
    /// `kill_pending`. Read (and cleared) when the target dies; written under this
    /// lock before the flag is published, so a reader that sees the flag set always
    /// sees the reason.
    ///
    /// Boxed: a pending exit reason is rare, and an inline `Message` is 32 bytes on every
    /// process's mailbox. Read through [`take_kill`](Self::take_kill) /
    /// [`kill_reason`](Self::kill_reason).
    pub(super) kill: Option<Box<Message>>,
    /// Is the pending `kill` **hard** (untrappable: die at the next reduction tick,
    /// not the next `receive`)? Hardness is a property of the *request*, separate
    /// from the reason value: `(exit pid :kill)` is hard with reason `:kill`, and
    /// **link propagation is hard with the originating reason** — so a cascading
    /// death reports *why* the tree fell, not a
    /// blanket `:kill`. Meaningful only while `kill` is `Some`.
    pub(super) kill_hard: bool,
    /// The parked green process waiting on this mailbox, if any. `send` takes it
    /// and re-queues it. (A short-lived `Process → Arc<Mailbox> → Process` cycle
    /// while parked; broken the moment it's re-queued or the process ends.) A waiter
    /// nothing will ever wake is reaped by `Interp::drop` via
    /// [`super::scheduler::shutdown_runtime_parked`].
    pub(super) waiter: Option<Box<Process>>,
    /// How many leading messages the parked waiter already scanned and rejected
    /// (selective receive). The worker re-runs it only when a message arrives
    /// *beyond* this — not for ones it already skipped. 0 for a plain FIFO receive.
    ///
    /// **Never reset between suspend cycles, and that is correct:** it is read only in
    /// `run_one`'s `Suspend::Receive` arm (`park_on_receive`), and every path to that arm
    /// writes it first in the same cycle: `receive_match`'s capture suspend follows a
    /// no-match scan, and `scan_mailbox` writes it at each of its no-match returns. Don't
    /// add a `Suspend::Receive` path that skips the write.
    pub(super) scanned: usize,
    /// The absolute deadline of the `receive` currently in progress, or `None`. A
    /// captured `receive` is re-entered from scratch on each wake (the continuation
    /// replays the `%receive` call), so recomputing `now + ms` every resume would
    /// reset — and never fire — the `after` timeout. The first entry stores the
    /// absolute deadline here; resumes reuse it; the matching/timeout exit clears it.
    /// Single slot: one `receive` runs at a time per process.
    pub(super) recv_deadline: Option<SchedDeadline>,
    /// **Deactivated one-shot reply aliases** — see [`DeadAliases`]. A message
    /// addressed to one of these refs is dropped *at delivery*, before it is ever
    /// queued. Empty for the overwhelming majority of processes, which is what makes
    /// the delivery check free (`is_empty` on a line the deliverer already holds).
    pub(super) dead_aliases: DeadAliases,
    /// How many OS threads are blocked (or committed to block) on this mailbox's
    /// condvar — the root/file-runner thread, and a native-nested `receive` (§7.4).
    /// Written only under this lock: [`wait_for_message`] increments before its
    /// `Condvar::wait` atomically releases the lock into the block, and decrements on
    /// the re-acquired guard when the wait returns. Read by every wake site inside the
    /// same critical section that makes its wake-relevant change (queue push, kill,
    /// timer check), so `wake_both` can skip the condvar notify (a `futex` syscall per
    /// message) when nobody is or can yet be committed to the blocked side. See
    /// `wake_both` for why the skip cannot lose a wake.
    pub(super) cv_waiters: u32,
    /// LATCHED wake: every wake site sets this under the state lock alongside its
    /// wake-relevant change; a waiter consumes it under the same lock before committing
    /// to `Condvar::wait`. Belt to `cv_waiters`' braces — a latch's failure mode is one
    /// extra rescan, a miscounted protocol's is a process asleep forever (KI-88). Both stay.
    pub(super) wake_pending: bool,
    /// How many PRIORITY-lane envelopes are queued (ADR-490), exactly: counted in and out
    /// by every push, removal and re-insert, so `proc/take-priority` answers "none" in O(1)
    /// — also for a loop whose plain `receive` consumed the priority messages it had.
    pub(super) priority_queued: usize,
    /// The LATEST-lane envelopes queued (ADR-490), by key: the seq of the one envelope per
    /// key, so a `proc/send-latest` finds the one it supersedes, and a receive asks whether
    /// its candidate was superseded, without walking the backlog. Boxed at the first
    /// LATEST envelope: few processes are ever sent one, and every process pays the field.
    // The box is the point: one pointer on every mailbox instead of the map's 48 B.
    #[allow(clippy::box_collection)]
    pub(super) latest_queued: Option<Box<std::collections::HashMap<Box<str>, u64>>>,
    /// The quit key (`proc/set-quit-key`, ADR-491): a PRIORITY-lane message equal to it
    /// interrupts this process when its interrupt window has been open long enough.
    /// Checked by the delivery, which holds this lock anyway; `None` (the default) costs
    /// a priority delivery one test and an ordinary one nothing.
    pub(super) quit_key: Option<Box<super::interrupt::QuitKey>>,
}

/// How many deactivated aliases one mailbox remembers. Fixed and small — the whole
/// set is 8 × `u64` stored inline in [`MailboxState`], so this costs no allocation
/// and cannot grow.
///
/// An entry only has to outlive the *one* late reply its alias was minted for, which
/// [`DeadAliases::take`] then forgets; the cap matters only for a caller timing out
/// repeatedly against a server that never answers. Past it the oldest entry is evicted,
/// which merely lets that one long-abandoned ref's late reply queue as an ordinary
/// message. Never a wrong drop, never unbounded.
const DEAD_ALIAS_CAP: usize = 8;

/// The per-mailbox set of deactivated one-shot reply aliases (ADR-244): a reply
/// addressed to a `ref` deactivated at its caller's deadline (`%ref-deactivate`) is
/// dropped at delivery and never enters the mailbox. A `ref` is already the
/// unforgeable request token a reply carries, so it needs no separate alias identity.
///
/// A tiny insertion-ordered array rather than a set: it is capped at
/// [`DEAD_ALIAS_CAP`], so a linear scan is a handful of `u64` compares on one cache
/// line — cheaper than hashing. Boxed, and only while non-empty: every process's mailbox
/// carries one, and almost none ever deactivates a ref (a call has to time out first), so
/// inline it was 72 bytes on every process for a set that is empty in steady state. The
/// delivery path still never allocates — it only reads and [`take`](Self::take)s.
#[derive(Default)]
pub(super) struct DeadAliases(Option<Box<DeadAliasIds>>);

struct DeadAliasIds {
    ids: [u64; DEAD_ALIAS_CAP],
    len: usize,
}

impl DeadAliases {
    /// Nothing deactivated — the case every delivery to every ordinary process takes.
    #[inline(always)]
    fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Deactivate `id`. Idempotent; evicts the oldest entry when full (see
    /// [`DEAD_ALIAS_CAP`] for why that is the right way to fail).
    fn insert(&mut self, id: u64) {
        let set = self.0.get_or_insert_with(|| {
            Box::new(DeadAliasIds {
                ids: [0; DEAD_ALIAS_CAP],
                len: 0,
            })
        });
        if set.ids[..set.len].contains(&id) {
            return;
        }
        if set.len == DEAD_ALIAS_CAP {
            set.ids.copy_within(1..DEAD_ALIAS_CAP, 0);
            set.len -= 1;
        }
        set.ids[set.len] = id;
        set.len += 1;
    }

    /// Is `id` deactivated? If so, **forget it** — an alias is one-shot, so the entry
    /// has done its job the moment the reply it was waiting to swallow arrives. This
    /// is what keeps the set at zero in steady state rather than at its cap, and the
    /// last one out frees the box.
    fn take(&mut self, id: u64) -> bool {
        let Some(set) = self.0.as_mut() else {
            return false;
        };
        let Some(i) = set.ids[..set.len].iter().position(|&x| x == id) else {
            return false;
        };
        set.ids.copy_within(i + 1..set.len, i);
        set.len -= 1;
        if set.len == 0 {
            self.0 = None;
        }
        true
    }
}

/// The ref a message is *addressed to*, if it is an alias-addressed envelope.
///
/// The shape is the request/reply idiom's own: a **keyword-led vector whose second
/// element is a `Ref`** — `[:reply <ref> v]`, `[:down <mref> pid reason]`. Nothing
/// else is considered, which is what keeps the check O(1) and keeps a deactivated
/// alias from ever swallowing an unrelated message: the ref was minted by the
/// deactivating process itself and handed out only as this call's reply token, so a
/// message carrying it in the reply-token position *is* that reply. (Refs are
/// node-qualified only by a random per-runtime prefix — KI-53 — so requiring both the
/// keyword tag and the exact position is deliberate belt-and-braces.)
#[inline]
fn message_alias(msg: &Message) -> Option<u64> {
    let Message::Vector(items) = msg else {
        return None;
    };
    match (items.first(), items.get(1)) {
        (Some(Message::Keyword(_)), Some(Message::Ref(id))) => Some(*id),
        _ => None,
    }
}

/// [`message_alias`] for a value still in the *sender's* heap — the L1 local-send
/// path checks before it copies, so a dropped reply costs no copy at all.
#[inline]
fn value_alias(heap: &Heap, v: Value) -> Option<u64> {
    let Value::Vector(h) = v else { return None };
    let items = heap.vector(h);
    match (items.first(), items.get(1)) {
        (Some(Value::Keyword(_)), Some(Value::Ref(id))) => Some(*id),
        _ => None,
    }
}

/// Deactivate `ref_id` as a one-shot reply alias on the **current** process's mailbox
/// (`%ref-deactivate`). From here on, a message addressed to that ref
/// (see [`message_alias`]) is dropped at delivery and never enters the queue.
///
/// Only the minting process can deactivate its own refs — the set lives on *its*
/// mailbox and only its own `%ref-deactivate` writes to it — so this cannot be used to
/// suppress someone else's traffic. Already-queued messages are untouched: this is
/// about what arrives next, exactly like `demonitor` without `flush`.
pub fn deactivate_alias(ref_id: u64) {
    super::scheduler::with_ctx(|ctx| {
        crate::core::sync::lock(&ctx.mailbox.state)
            .dead_aliases
            .insert(ref_id)
    });
}

impl Mailbox {
    /// Record which runtime this process belongs to, so `send` can tell whether a RUNTIME
    /// handle is meaningful in the target without touching its heap (which, for a *busy*
    /// receiver, the sender cannot reach). The process REGISTRY is global, so a second
    /// `Interp` in the same OS process has different regions and must not receive handles.
    /// Written before the mailbox is published to the registry, like `parent`.
    pub(super) fn set_runtime_tag(&self, tag: u64) {
        self.runtime_tag.store(tag, Ordering::Relaxed);
    }

    /// This process's runtime tag, or 0 if never set (which never matches a real tag, so an
    /// unset mailbox simply declines to receive shared handles).
    pub(super) fn runtime_tag(&self) -> u64 {
        self.runtime_tag.load(Ordering::Relaxed)
    }

    /// A mailbox for a process whose spawner is `parent` (`0` for the root, which has
    /// none). The parent pid lives here, on the registry-reachable per-process record,
    /// rather than in a global `pid -> pid` map that cost two locked operations on every
    /// spawn and exit.
    pub(super) fn new_with_parent(parent: u64, isolate_scope: u64) -> Arc<Mailbox> {
        let mb = Mailbox::new();
        // Safe: the mailbox is not published to the registry until after this returns, so
        // no other thread can observe the writes.
        mb.parent.store(parent, Ordering::Relaxed);
        // Both from the spawner's *scope*, not its owner: a process running an isolate
        // stamps its children with that isolate's token, and a child passes the same one
        // on to its own children, so a whole subtree is stamped at birth.
        mb.isolate_owner.store(isolate_scope, Ordering::Relaxed);
        mb.isolate_scope.store(isolate_scope, Ordering::Relaxed);
        mb
    }

    pub(super) fn new() -> Arc<Mailbox> {
        Arc::new(Mailbox {
            runtime_tag: AtomicU64::new(0),
            state: Mutex::new(MailboxState {
                queue: VecDeque::new(),
                next_seq: 0,
                waiter: None,
                scanned: 0,
                kill: None,
                kill_hard: false,
                recv_deadline: None,
                dead_aliases: DeadAliases::default(),
                cv_waiters: 0,
                wake_pending: false,
                priority_queued: 0,
                latest_queued: None,
                quit_key: None,
            }),
            cv: Condvar::new(),
            // The root (which never goes through enqueue/run_one) keeps this; a
            // spawned green is set RUNNABLE by `enqueue` immediately after.
            status: AtomicU8::new(ST_RUNNING),
            mem: AtomicUsize::new(0),
            footprint: AtomicUsize::new(0),
            footprint_period: AtomicU64::new(0),
            gc_runs: AtomicU64::new(0),
            reductions: AtomicU64::new(0),
            cpu_ns: AtomicU64::new(0),
            thread_clock: AtomicI64::new(-1),
            kill_pending: AtomicBool::new(false),
            trap_exit: AtomicBool::new(false),
            max_mailbox: AtomicUsize::new(0),
            overflow_hit: AtomicUsize::new(0),
            took_priority: AtomicBool::new(false),
            timer_gen: AtomicU64::new(0),
            parent: AtomicU64::new(0),
            isolate_owner: AtomicU64::new(0),
            isolate_scope: AtomicU64::new(0),
            interrupt: Default::default(),
        })
    }

    /// Record a pending exit signal (`(exit pid reason)` / link propagation).
    /// Stores the reason *then* publishes the flag, so any reader (`pending_kill`)
    /// that observes the flag set is guaranteed to see the reason. `hard` marks an
    /// untrappable kill honoured at the next reduction tick (vs the next `receive`)
    /// — a property of the request, independent of the reason value, so link
    /// propagation can be hard *and* carry the originating reason.
    pub(super) fn request_kill(&self, reason: Message, hard: bool) {
        {
            let mut st = crate::core::sync::lock(&self.state);
            // A latched **hard** kill is **sticky**: a later *soft* `(exit pid
            // reason)` must not overwrite it (an `(exit pid :kill)` can't be
            // undone — otherwise a racing soft exit could downgrade
            // the kill and spare a CPU-bound target, which only honours a hard kill
            // at `preempt`). A fresh hard kill may still upgrade a pending soft
            // reason; the first hard reason wins thereafter.
            let latched_hard = st.kill.is_some() && st.kill_hard;
            if !latched_hard {
                st.kill = Some(Box::new(reason));
                st.kill_hard = hard;
            }
        }
        self.kill_pending.store(true, Ordering::Relaxed);
    }

    /// The pending exit reason, if any (test-only now: production death sites take
    /// `state.kill` directly under the lock, and the loop-top probe is
    /// [`pending_hard_kill`](Self::pending_hard_kill)).
    #[cfg(test)]
    pub(super) fn pending_kill(&self) -> Option<Message> {
        if !self.kill_pending.load(Ordering::Relaxed) {
            return None;
        }
        crate::core::sync::lock(&self.state).kill_reason()
    }

    /// Post-push bound check (ADR-307): called by every site that enqueues a NEW
    /// message (`deliver_envelope_timed`, `try_deliver_local`) with the queue length
    /// read under the state lock. A selective-receive rebuild re-inserts already
    /// counted messages and deliberately does not re-check. One relaxed load when
    /// no bound is set.
    #[inline]
    pub(super) fn note_mailbox_bound(&self, queue_len: usize) {
        let limit = self.max_mailbox.load(Ordering::Relaxed);
        if limit != 0 && queue_len > limit {
            // Arm only from CLEAN, with a compare-exchange (KI-103): a check-then-store
            // could arm AFTER the receiver latched, killing its handler mid-recovery.
            // Losing the CAS means someone armed first or the latch is set — both fine.
            let _ = self.overflow_hit.compare_exchange(
                0,
                queue_len,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
    }

    /// A value no queue length can take, so one word carries three states: `0` clean,
    /// `1..` armed with the breaching length, `LATCHED` already raised for this arming.
    const OVERFLOW_LATCHED: usize = usize::MAX;

    /// Is an overflow waiting to be raised? A peek (`take_overflow_hit` takes and latches) —
    /// for the native loop's poll, which cannot raise.
    #[cfg(feature = "jit")]
    pub(super) fn overflow_pending(&self) -> bool {
        let v = self.overflow_hit.load(Ordering::Relaxed);
        v != 0 && v != Self::OVERFLOW_LATCHED
    }

    /// Take the sticky breach flag — the receiver-side probe (safepoints + `receive` entry).
    /// Taking LATCHES it, so the bound is one-shot per arming (KI-103): the handler that
    /// catches E0046 gets a guaranteed window to drain or re-set the bound. Setting the
    /// bound again (`set_max_mailbox`) clears the latch and re-arms the protection.
    pub(super) fn take_overflow_hit(&self) -> Option<(usize, usize)> {
        // Take AND latch in one compare-exchange: a plain `swap(LATCHED)` would latch a
        // mailbox that never breached, and a `swap(0)` would race a concurrent re-arm.
        loop {
            let v = self.overflow_hit.load(Ordering::Relaxed);
            if v == 0 || v == Self::OVERFLOW_LATCHED {
                return None;
            }
            if self
                .overflow_hit
                .compare_exchange_weak(
                    v,
                    Self::OVERFLOW_LATCHED,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return Some((v, self.max_mailbox.load(Ordering::Relaxed)));
            }
        }
    }

    /// Is an untrappable **hard** kill pending? The loop-top safepoint probe: a soft exit
    /// isn't honoured there (it waits for the next `receive`). One relaxed load when none.
    pub(super) fn pending_hard_kill(&self) -> bool {
        if !self.kill_pending.load(Ordering::Relaxed) {
            return false;
        }
        let st = crate::core::sync::lock(&self.state);
        st.kill.is_some() && st.kill_hard
    }
}

/// Number of registry shards. A power of two so the shard index is a mask, and comfortably
/// above the worker count so a fan-out rarely collides.
const REGISTRY_SHARDS: usize = 64;

/// `pid -> Arc<Mailbox>`, sharded by pid, so `send` finds a target from any thread. A
/// single mutex serialised a spawn fan-out against every worker's `deregister` (more
/// workers measured *slower*); sharding keeps the semantics exactly.
///
/// **Lock ordering is preserved.** `add_monitor` takes MONITORS then a registry shard
/// (nested); `deregister` takes a shard, releases, then NAMES, then MONITORS (sequential).
/// A shard is therefore always the *inner* lock, exactly as the single registry was — and
/// the two race against each other on the *same pid*, hence the same shard, so their
/// check-and-modify pairing resolves as before. Whole-registry walks (`pids`, `len`) take
/// one shard at a time and never hold two at once.
pub(super) struct Registry {
    /// Padded: sequential pids map to adjacent shards, and unpadded a spawn burst's
    /// registrations bounced one line between every worker.
    shards: Vec<super::scheduler::Padded<Mutex<PidMap>>>,
}

/// The registry's per-shard map. Keyed by pid through [`PidHasher`], not std's SipHash:
/// every `send` resolves its target here, and SipHash on one `u64` — a 128-bit state set
/// up and finalized per lookup — is a measurable share of a message-heavy workload.
type PidMap = HashMap<u64, Arc<Mailbox>, BuildPidHasher>;

/// A pid hash: the splitmix64 finalize the table/map path already uses for an int key
/// ([`crate::core::heap::Heap::hash_int`]). Not the identity: a shard holds only pids
/// that agree in their low six bits (the shard index), and hashbrown picks a bucket from
/// the LOW bits of the hash — an identity hash would put a whole shard in one probe run.
#[derive(Default, Clone, Copy)]
pub(super) struct BuildPidHasher;
impl std::hash::BuildHasher for BuildPidHasher {
    type Hasher = PidHasher;
    #[inline]
    fn build_hasher(&self) -> PidHasher {
        PidHasher(0)
    }
}
pub(super) struct PidHasher(u64);
impl std::hash::Hasher for PidHasher {
    #[inline]
    fn write_u64(&mut self, pid: u64) {
        self.0 = crate::core::heap::Heap::hash_int(pid as i64);
    }
    fn write(&mut self, _: &[u8]) {
        unreachable!("PidHasher hashes a u64 pid only")
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

impl Registry {
    fn shard(&self, pid: u64) -> &Mutex<PidMap> {
        &self.shards[(pid as usize) & (REGISTRY_SHARDS - 1)].0
    }

    pub(super) fn get(&self, pid: u64) -> Option<Arc<Mailbox>> {
        crate::core::sync::lock(self.shard(pid)).get(&pid).cloned()
    }

    pub(super) fn contains_key(&self, pid: u64) -> bool {
        crate::core::sync::lock(self.shard(pid)).contains_key(&pid)
    }

    pub(super) fn insert(&self, pid: u64, mb: Arc<Mailbox>) {
        crate::core::sync::lock(self.shard(pid)).insert(pid, mb);
    }

    pub(super) fn remove(&self, pid: u64) -> Option<Arc<Mailbox>> {
        crate::core::sync::lock(self.shard(pid)).remove(&pid)
    }

    /// Every live pid. One shard locked at a time — never two, so this cannot deadlock
    /// against a nested MONITORS/NAMES acquisition elsewhere.
    pub(super) fn pids(&self) -> Vec<u64> {
        let mut out = Vec::new();
        for sh in &self.shards {
            out.extend(crate::core::sync::lock(sh).keys().copied());
        }
        out
    }

    /// Every live `(pid, mailbox)`. Same one-shard-at-a-time discipline as [`pids`].
    pub(super) fn entries(&self) -> Vec<(u64, Arc<Mailbox>)> {
        let mut out = Vec::new();
        for sh in &self.shards {
            out.extend(
                crate::core::sync::lock(sh)
                    .iter()
                    .map(|(k, v)| (*k, Arc::clone(v))),
            );
        }
        out
    }

    pub(super) fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|sh| crate::core::sync::lock(sh).len())
            .sum()
    }
}

/// pid → mailbox. `pub(super)` so the `monitor` submodule can take the REGISTRY ↔
/// MONITORS liveness check inside its critical section (see `monitor::add_monitor`).
pub(super) static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry {
    shards: (0..REGISTRY_SHARDS)
        .map(|_| super::scheduler::Padded(Mutex::new(PidMap::default())))
        .collect(),
});

/// Is `pid` currently registered (i.e. still alive)? Used by the
/// named-spawn idempotence check in `dist::spawn_or_get` to decide
/// whether to reuse an existing pid registered under a name or treat
/// the name as stale and spawn fresh. Cheap — one mutex acquisition.
pub(crate) fn is_alive(pid: u64) -> bool {
    REGISTRY.contains_key(pid)
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
    let mailbox = REGISTRY.get(pid)?;
    Some(f(&mailbox))
}

/// **The wakeup protocol, in one place.** A wake decided under the mailbox state lock and
/// [`fire`](Wake::fire)d after it is released: the parked green waiter to re-queue (if
/// any) and whether a thread is blocked on the condvar.
///
/// Every site that must wake a process goes through this: `deliver` (a message arrived),
/// the L1 local delivery, `wake_for_timeout` (a `receive` deadline passed) and `exit` (an
/// exit signal must rouse its target). Each used to spell the steps out by hand, and two
/// of them once did half of it — one never notified the condvar, one woke only one path.
/// Built by [`MailboxState::take_wake`], so setting `wake_pending`, taking the waiter and
/// reading `cv_waiters` cannot come apart; `#[must_use]`, so it cannot be built and dropped.
///
/// The take happens under the lock, which serialises with `run_one`'s park: either it
/// takes an already-parked process, or `run_one` has not parked yet and will observe the
/// new state (message / `kill_pending`) when it does — exactly one path wins, so a process
/// cannot end up parked with work pending. `fire` runs with the lock released, because
/// re-queueing takes the worker's queue lock.
#[must_use = "a Wake does nothing until `fire`d, after the state lock is dropped"]
pub(super) struct Wake {
    parked: Option<Box<Process>>,
    cv_waiter: bool,
}

impl Wake {
    /// Signal both wake paths (see [`wake_both`]). Call with the state lock released.
    pub(super) fn fire(self, mb: &Mailbox) {
        wake_both(mb, self.parked, self.cv_waiter);
    }
}

impl MailboxState {
    /// Take the pending exit reason (the retire path owns it).
    pub(super) fn take_kill(&mut self) -> Option<Message> {
        self.kill.take().map(|reason| *reason)
    }

    /// Peek at the pending exit reason, leaving it for the retire path.
    pub(super) fn kill_reason(&self) -> Option<Message> {
        self.kill.as_deref().cloned()
    }

    /// Mark this mailbox as needing a re-scan and take what must be woken for it.
    pub(super) fn take_wake(&mut self) -> Wake {
        let parked = self.waiter.take();
        self.wake_with(parked)
    }

    /// [`take_wake`](Self::take_wake) for a caller that has already taken the waiter out
    /// (the L1 delivery takes it first, to copy into its heap).
    pub(super) fn wake_with(&mut self, parked: Option<Box<Process>>) -> Wake {
        self.wake_pending = true;
        Wake {
            parked,
            cv_waiter: self.cv_waiters > 0,
        }
    }
}

/// Push a (already-`Send`) message into local process `pid`'s mailbox and wake it;
/// a no-op if `pid` is gone. The shared tail of `send`, monitor `[:down …]`
/// delivery, and inbound node-link messages (`crate::dist`).
pub(crate) fn deliver(pid: u64, msg: Message) {
    deliver_envelope(pid, Envelope::plain(msg));
}

/// Like [`deliver`], but the message carries a flow-control `credit` that is returned
/// to its source when the message leaves the mailbox (see [`Envelope::credit`]). A gone
/// `pid` drops the envelope here, which returns the credit at once.
pub(crate) fn deliver_credited(pid: u64, msg: Message, credit: DeliveryCredit) {
    let mut envelope = Envelope::plain(msg);
    envelope.credit = Some(Box::new(credit));
    deliver_envelope(pid, envelope);
}

/// Like [`deliver`], but the message carries a debugger causal context (ADR-174
/// send-level): the receiver adopts `trace` when it pops this message. `dev-tools`
/// only; the `send` primitive uses it when the sender has a trace context set.
#[cfg(feature = "dev-tools")]
pub(crate) fn deliver_traced(pid: u64, msg: Message, trace: Option<Message>) {
    deliver_envelope(
        pid,
        Envelope {
            msg: Payload::Wire(msg),
            seq: 0, // replaced by `MailboxState::push`
            trace,
            credit: None,
            lane: Lane::Normal,
        },
    );
}

/// Like [`deliver`], on the PRIORITY lane (ADR-490): what a window or a browser tab
/// delivers as the user's input, so `proc/take-priority` finds it ahead of whatever
/// background messages are queued. An ordinary `receive` still sees it in arrival order.
pub(crate) fn deliver_priority(pid: u64, msg: Message) {
    let mut envelope = Envelope::plain(msg);
    envelope.lane = Lane::Priority;
    deliver_envelope(pid, envelope);
}

/// Like [`deliver`], in `lane`: a message a peer node sent with `proc/send-priority` or
/// `proc/send-latest` (ADR-492), queued here as the local send queues it — a LATEST one
/// superseding the queued message with its key.
pub(crate) fn deliver_in_lane(pid: u64, msg: Message, lane: Lane) {
    let mut envelope = Envelope::plain(msg);
    envelope.lane = lane;
    deliver_envelope(pid, envelope);
}

/// [`deliver_credited`] on the PRIORITY lane — a window's key repeat, whose source can
/// take it back (`CreditSource::withdrawn`).
#[cfg(feature = "gui")]
pub(crate) fn deliver_priority_credited(pid: u64, msg: Message, credit: DeliveryCredit) {
    let mut envelope = Envelope::plain(msg);
    envelope.credit = Some(Box::new(credit));
    envelope.lane = Lane::Priority;
    deliver_envelope(pid, envelope);
}

/// `(proc/send-priority target msg)` / `(proc/send-latest target key msg)`: `send` in
/// another lane (ADR-490). A remote target — a pid, or a `{:name :node}` address, on
/// another node — gets the lane in its `Send` frame and the receiving node queues the
/// message in it (ADR-492); a dead local pid or a name nobody holds drops the message, as
/// `send` does. Never the L1 copy
/// into a parked receiver's heap either: a lane must be on the envelope, and a LATEST
/// one must stay a wire payload, which the delivery that supersedes it can drop without
/// touching the receiver's heap.
pub fn send_in_lane(
    heap: &mut Heap,
    target: Value,
    msg: Value,
    latest_key: Option<Value>,
) -> Result<(), LispError> {
    let local_id = match target {
        Value::Pid { node, id } if crate::dist::is_local(node) => Some(id),
        // A registered name — bare, or a `{:name :node}` address naming this node — is a
        // local pid one lookup away, and keeps its lane: an app addresses its UI loop by
        // name (`:editor`), and that is exactly the receiver lanes exist for.
        Value::Keyword(name) => crate::dist::whereis(name),
        Value::Map(mid) => match read_name_address(heap, mid) {
            Ok((name, node)) if crate::dist::is_local(node) => crate::dist::whereis(name),
            _ => None,
        },
        _ => None,
    };
    let mailbox = local_id.and_then(|id| REGISTRY.get(id));
    let lane = match latest_key {
        None => Lane::Priority,
        Some(key) => Lane::Latest(Box::new(crate::syntax::printer::print(heap, key))),
    };
    let Some(mailbox) = mailbox else {
        let wire = to_message_to_runtime(heap, msg, None)?;
        return route_message(heap, target, wire, lane);
    };
    let wire = to_message_to_runtime(heap, msg, Some(mailbox.runtime_tag()))?;
    let mut envelope = Envelope::plain(wire);
    envelope.lane = lane;
    push_and_wake(&mailbox, envelope);
    Ok(())
}

/// `(proc/receive-lane ms)`: `(receive (m m) (after ms nil))` that also says which lane
/// the message came on — `[msg :priority]` or `[msg :normal]`, nil on timeout (ADR-490).
/// What lets a UI loop that parked on its mailbox tell the user's input that woke it from
/// a background message.
pub fn receive_lane(heap: &mut Heap, timeout: Value) -> LispResult {
    let ctx = current_process();
    ctx.mailbox.took_priority.store(false, Ordering::Relaxed);
    // `(receive (m m) …)`'s native spec: any message, bound — so this IS a receive, with
    // its parking, timeout and suspend/replay (a replay clears the flag again first).
    let any = Value::Keyword(value::intern("%rspec [[:bind]]"));
    let answer = receive_match(heap, any, timeout, Value::nil(), Value::nil())?;
    let Value::Vector(id) = answer else {
        return Ok(Value::nil()); // timed out
    };
    let msg = heap.vector(id)[1];
    let lane = if ctx.mailbox.took_priority.load(Ordering::Relaxed) {
        value::kw("priority")
    } else {
        value::kw("normal")
    };
    Ok(heap.alloc_vector2(msg, lane))
}

/// `(proc/take-priority)`: the calling process's oldest PRIORITY-lane message, removed
/// from its mailbox and answered as `[msg]`, or nil when none is queued — without
/// waiting and without looking at a single ordinary message (ADR-490). O(1) when the
/// mailbox holds none (`priority_queued`); otherwise a walk of the queue's lane
/// tags, which never rebuilds a message it passes. A priority message its source took
/// back (a key repeat released before it was read) is dropped on the way, as a scan does.
pub fn take_priority(heap: &mut Heap) -> LispResult {
    let ctx = current_process();
    let taken = {
        let mut st = crate::core::sync::lock(&ctx.mailbox.state);
        if st.priority_queued == 0 {
            return Ok(Value::nil());
        }
        let mut index = 0;
        let mut taken = None;
        while index < st.queue.len() {
            if !matches!(st.queue[index].lane, Lane::Priority) {
                index += 1;
                continue;
            }
            let envelope = st.remove_at(index).expect("bounds checked above");
            if index < st.scanned {
                st.scanned -= 1;
            }
            if envelope
                .credit
                .as_ref()
                .is_some_and(|credit| credit.withdrawn())
            {
                continue;
            }
            taken = Some(envelope);
            break;
        }
        taken
    };
    let Some(envelope) = taken else {
        return Ok(Value::nil());
    };
    let value = match &envelope.msg {
        Payload::Wire(wire) => from_message(heap, wire),
        Payload::Local { slot, .. } => heap.msg_root_take(*slot),
    };
    Ok(heap.alloc_vector(vec![value]))
}

/// `(proc/set-quit-key key busy-ms)` (ADR-491): `key` — a keyword, string or integer, what a
/// frontend delivers as a keystroke — arriving on the calling process's PRIORITY lane while
/// it has been running in an interrupt window for `busy-ms` interrupts it. nil clears it.
pub(crate) fn set_quit_key(
    key: Option<(super::interrupt::QuitPattern, u64)>,
    from: Option<crate::core::value::Symbol>,
) {
    let ctx = current_process();
    crate::core::sync::lock(&ctx.mailbox.state).quit_key = key.map(|(key, busy_ms)| {
        Box::new(super::interrupt::QuitKey {
            key,
            busy_us: busy_ms.saturating_mul(1000),
            from,
        })
    });
}

/// Open an interrupt window for the calling process, inside any already open, with an
/// optional ceiling — `proc/interruptible`'s start (ADR-491). Answers the window's depth,
/// which [`close_interrupt_window`] and [`interrupt_raised_for`] take (ADR-493).
pub fn open_interrupt_window(ceiling_ms: Option<u64>) -> u32 {
    current_process().mailbox.interrupt.open(ceiling_ms)
}

/// Close the calling process's interrupt window `depth`, and any inside it — however the
/// `proc/interruptible` that opened it ends (ADR-491, ADR-493).
pub fn close_interrupt_window(depth: u32) {
    current_process().mailbox.interrupt.close(depth)
}

/// Is the interrupt the calling process raised for its window `depth` — so that window's
/// `proc/interruptible` answers it, rather than letting it pass to the window it is for
/// (ADR-493)?
pub(crate) fn interrupt_raised_for(depth: u32) -> bool {
    current_process().mailbox.interrupt.raised_for(depth)
}

/// `(proc/interrupt pid)` (ADR-491): interrupt `pid` at its next safepoint, if it is a live
/// local process RUNNING in an open interrupt window. True when the request landed; a
/// parked, closed, dead or remote target is a no-op answering false.
pub fn interrupt_process(pid: u64) -> bool {
    let Some(mailbox) = REGISTRY.get(pid) else {
        return false;
    };
    mailbox.status.load(Ordering::Relaxed) != ST_WAITING && mailbox.interrupt.request(0)
}

/// The safepoint probe (ADR-491): take the calling process's due interrupt, if any —
/// `Some(by_ceiling)`. One thread-local borrow and a relaxed load when nothing is armed.
pub(crate) fn take_current_interrupt() -> Option<bool> {
    super::scheduler::with_current_mailbox(|mailbox| mailbox.interrupt.take()).flatten()
}

/// A `catch` inside the interrupted work bound an interrupt (ADR-491): it is sticky, so end
/// the quantum — the next safepoint, the first after the handler, raises it again. Called
/// for every caught `:interrupted`, so a catch-all cannot keep the process inside its window.
pub(crate) fn interrupt_caught() {
    if super::scheduler::with_current_mailbox(|mailbox| mailbox.interrupt.due()).unwrap_or(false) {
        super::scheduler::expire_quantum();
    }
}

/// Is `pid` busy — running a turn in an interrupt window that has been open as long as its
/// quit key's busy-ms (300 ms without one)? What a window's busy cursor asks of the loop it
/// delivers to (ADR-491): `None` when no window is open (the loop is idle, or gone), so the
/// frontend can stop asking; `Some(busy)` while one is.
pub(crate) fn interrupt_busy(pid: u64) -> Option<bool> {
    let mailbox = REGISTRY.get(pid)?;
    let open_us = mailbox.interrupt.open_for_us()?;
    let busy_us = crate::core::sync::lock(&mailbox.state)
        .quit_key
        .as_ref()
        .map_or(300_000, |quit| quit.busy_us);
    Some(open_us >= busy_us)
}

/// Is an interrupt due for the calling process? A peek for code that cannot raise where it
/// stands — a native loop's poll, a long builtin's own loop, a parser's progress callback —
/// and leaves it for the next safepoint (or raises [`take_current_interrupt`]'s itself).
pub(crate) fn current_interrupt_due() -> bool {
    super::scheduler::with_current_mailbox(|mailbox| mailbox.interrupt.due()).unwrap_or(false)
}

/// L1 hit/decline accounting, printed at exit under `BROOD_L1_STATS=1`. Plain relaxed
/// counters — diagnostic only, never read by the runtime.
pub mod l1_stats {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub static HIT: AtomicU64 = AtomicU64::new(0);
    pub static NO_WAITER: AtomicU64 = AtomicU64::new(0);
    pub static DECLINED: AtomicU64 = AtomicU64::new(0);
    /// Declined because the message exceeded the copy budget (KI-56), as opposed to
    /// `DECLINED`, which is a value kind the copier does not handle.
    pub static OVER_BUDGET: AtomicU64 = AtomicU64::new(0);
    pub static NO_PROC: AtomicU64 = AtomicU64::new(0);
    pub fn bump(c: &AtomicU64) {
        if enabled() {
            c.fetch_add(1, Relaxed);
        }
    }
    pub fn enabled() -> bool {
        static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        crate::diagnostics::debug_flags::is_set(&F, "BROOD_L1_STATS")
    }
    pub fn dump_if_requested() {
        if !enabled() {
            return;
        }
        let (h, w, d, n, b) = (
            HIT.load(Relaxed),
            NO_WAITER.load(Relaxed),
            DECLINED.load(Relaxed),
            NO_PROC.load(Relaxed),
            OVER_BUDGET.load(Relaxed),
        );
        let total = h + w + d + n + b;
        let pct = if total == 0 {
            0.0
        } else {
            100.0 * h as f64 / total as f64
        };
        eprintln!(
            "[l1] local-send fast path: {h} hit ({pct:.1}%), {w} not-parked, \
             {d} value-declined, {b} over-budget, {n} no-process  (of {total} local sends)"
        );
    }
}

/// Outcome of the L1 attempt. When it declines, it hands back the target's mailbox, so the
/// caller looks the registry up ONCE for everything the busy-receiver send path needs: the
/// runtime tag that decides whether a shared closure handle may cross (ADR-208), and the
/// mailbox the serialised message is then pushed to.
enum LocalDelivery {
    /// Delivered into the parked receiver's heap; nothing further to do.
    Delivered,
    /// Not delivered. `mailbox` is the target's, or `None` if there is no such process.
    Declined { mailbox: Option<Arc<Mailbox>> },
}

/// **The L1 local-send fast path.** Try to deliver `v` to local process `pid` by copying
/// it straight from the sender's heap into the receiver's, skipping the `Value → Message
/// → Value` round trip.
///
/// Only fires when the receiver is **parked**: taking its `waiter` out of the mailbox
/// state under the lock gives us its `Box<Process>`, and therefore exclusive `&mut` on
/// its heap for the duration — the same quiescence `trim_parked` relies on. A running
/// receiver, a remote pid, or a value the copier declines (a closure, a rope, an
/// unrealised seq-view) all decline and take the normal `Message` path.
///
/// The copied value is parked in the receiver's `msg_roots` — a *traced* slot table —
/// because the mailbox is not a GC root and `roots` is the operand stack.
fn try_deliver_local(src: &Heap, pid: u64, v: Value) -> LocalDelivery {
    let Some(mb) = REGISTRY.get(pid) else {
        l1_stats::bump(&l1_stats::NO_PROC);
        return LocalDelivery::Declined { mailbox: None };
    };
    // A receiver that is not parked has no waiter to take, so the lock below would only
    // find that out. `ST_WAITING` is set BEFORE a green process files itself as the
    // waiter (`receive_match`'s capture suspend, then `park_on_receive`) and left only
    // after the waiter is taken, so a parked receiver reads `ST_WAITING` here. The load is
    // relaxed and may be stale, which is safe in both directions: a stale "waiting" takes
    // the lock and finds no waiter (the decline below), and a stale "not waiting" sends
    // the message down the wire path, whose `take_wake` takes and wakes any waiter that
    // is there — the slow path is always correct, this only skips a lock that would fail.
    if mb.status.load(Ordering::Relaxed) != ST_WAITING {
        l1_stats::bump(&l1_stats::NO_WAITER);
        return LocalDelivery::Declined { mailbox: Some(mb) };
    }
    let mut st = crate::core::sync::lock(&mb.state);
    // One-shot reply alias: the receiver deactivated this ref, so the reply is
    // dropped here — before the cross-heap copy, before the queue, before any wake.
    // `is_empty` is the whole cost when nothing is deactivated, which is every ordinary
    // process; we already hold the lock, so it is a field read on a hot line.
    if !st.dead_aliases.is_empty()
        && let Some(id) = value_alias(src, v)
        && st.dead_aliases.take(id)
    {
        return LocalDelivery::Delivered; // silently swallowed, like a send to a dead pid
    }
    // Not parked → we have no safe access to its heap.
    let Some(mut proc) = st.waiter.take() else {
        l1_stats::bump(&l1_stats::NO_WAITER);
        drop(st);
        return LocalDelivery::Declined { mailbox: Some(mb) };
    };
    // Bounded (KI-56): this copy runs with the mailbox mutex held — that is what gives us
    // exclusive access to the parked receiver's heap — so its cost is a stall for every
    // unrelated operation on that mailbox. A message past the budget declines here and
    // takes the wire path, whose heavy work happens outside the lock. See `l1_copy_budget`.
    let mut budget = crate::process::message::l1_copy_budget();
    let copied = crate::process::message::copy_cross_heap(src, proc.heap_mut(), v, &mut budget);
    let Some(copied) = copied else {
        // Declined: put the process back exactly as we found it and let the caller
        // deliver through `Message`. Nothing observable happened — the partial copy left
        // unreachable cells in the receiver's heap, which is ordinary garbage (allocation
        // never collects; the receiver is parked and runs nothing until we wake it).
        st.waiter = Some(proc);
        // A negative budget is the size cap; anything else is a value kind we do not copy.
        // Kept apart because they call for opposite responses: one is the cap working as
        // intended, the other is a gap in the copier.
        l1_stats::bump(if budget < 0 {
            &l1_stats::OVER_BUDGET
        } else {
            &l1_stats::DECLINED
        });
        drop(st);
        return LocalDelivery::Declined { mailbox: Some(mb) };
    };
    let tag = leading_keyword(proc.heap_mut(), copied);
    let slot = proc.heap_mut().msg_root_add(copied);
    st.push(Envelope {
        msg: Payload::Local { slot, tag },
        seq: 0,
        #[cfg(feature = "dev-tools")]
        trace: None,
        credit: None,
        lane: Lane::Normal,
    });
    mb.note_mailbox_bound(st.queue.len());
    // Both paths, for the reason `wake_both` documents: taking a `waiter` here does not prove
    // the receiver is *only* reachable by re-queue. This path never notified the condvar at
    // all, so a receiver that was both parked and cv-blocked was unreachable through it.
    let wake = st.wake_with(Some(proc));
    drop(st);
    wake.fire(&mb);
    l1_stats::bump(&l1_stats::HIT);
    LocalDelivery::Delivered
}

fn deliver_envelope(pid: u64, env: Envelope) {
    crate::perf_time!(ns_deliver, { deliver_envelope_timed(pid, env) })
}

fn deliver_envelope_timed(pid: u64, env: Envelope) {
    if let Some(mb) = REGISTRY.get(pid) {
        push_and_wake(&mb, env);
    }
}

/// Push `env` onto a mailbox the caller already holds and wake its owner — the tail of
/// [`deliver_envelope`], for a sender whose declined L1 attempt already looked the
/// mailbox up (`send`).
///
/// Holding the `Arc` rather than re-reading the registry admits one more interleaving
/// than a lookup did, and it is the one a lookup already admitted: the owner dies after
/// the mailbox was found and before the push. The push then lands in a mailbox that has
/// left the registry; nothing scans it again (a death drains nothing from the queue —
/// `deregister` reads only the status, and the root-ctx retire's `clear` runs once), so
/// the message is dropped with the last `Arc`, which is what a send to a dead pid does.
/// A dead process holds no waiter (it was running when it died, or its retire took the
/// waiter), so the wake finds none to enqueue.
fn push_and_wake(mb: &Mailbox, env: Envelope) {
    let mut st = crate::core::sync::lock(&mb.state);
    // One-shot reply alias — the wire-path twin of the check in
    // `try_deliver_local`. Dropped here, so it never occupies a queue slot and no
    // later selective receive ever re-scans it. See `DeadAliases`.
    if !st.dead_aliases.is_empty()
        && let Payload::Wire(msg) = &env.msg
        && let Some(id) = message_alias(msg)
        && st.dead_aliases.take(id)
    {
        return;
    }
    // The quit key (ADR-491): the message is queued either way — the app still sees its
    // C-g — and, when the owner is running in an interrupt window open long enough, also
    // interrupts it. A parked owner is not running: nothing to interrupt.
    if let Some(quit) = st.quit_key.as_deref()
        && let Payload::Wire(msg) = &env.msg
        && quit.matches(msg, matches!(env.lane, Lane::Priority))
        && quit.admits(super::interrupt::delivery_origin())
        && mb.status.load(Ordering::Relaxed) != ST_WAITING
    {
        mb.interrupt.request(quit.busy_us);
    }
    st.push(env);
    mb.note_mailbox_bound(st.queue.len());
    let wake = st.take_wake();
    drop(st);
    wake.fire(mb);
}

/// Signal **both** wake paths for a mailbox we have just pushed to (or set a kill on):
/// re-queue the parked green process if there was one, *and* notify the condvar.
///
/// The two paths are not mutually exclusive: a green process in a **native-nested**
/// `receive` (§7.4) blocks its worker on this condvar *without* clearing any `waiter` a
/// previous park left behind, so an either/or wake left it blocked forever — a condvar
/// notify does **not latch**. With two wake mechanisms (a root thread really does block
/// an OS thread) the invariant is: signal both, every time. A spurious wake costs one
/// re-scan. Called with the state lock already released.
///
/// `cv_waiter` is [`MailboxState::cv_waiters`]` > 0`, read by the caller **inside the
/// same lock hold that made its wake-relevant change**. Skipping the notify on `false`
/// cannot lose a wake: a waiter registers its count and re-checks its wake condition
/// under that same mutex before `Condvar::wait` atomically releases it — so either the
/// wake site ran first (the waiter sees the change and never blocks) or the waiter did
/// (the wake site reads `cv_waiters > 0` and notifies). Only a notify delivered to
/// nobody is skipped — and it was a `futex` syscall per message.
pub(super) fn wake_both(mb: &Mailbox, parked: Option<Box<Process>>, cv_waiter: bool) {
    if cv_waiter {
        mb.cv.notify_all();
    }
    if let Some(proc) = parked {
        wake_enqueue(proc); // wake a parked green process (capture-mode → may migrate)
    }
}

/// `(send target msg)` — copy `msg` into `target`'s mailbox and wake it. `target`
/// is a **pid** (local or remote — it carries node identity) or a `{:name :node}`
/// **registered-name address** for bootstrapping a peer before you hold its pid.
/// Routing is location-transparent: a local target delivers in-process; a remote
/// one is forwarded over the node link (`crate::dist`). Sending to a dead/unknown
/// target is a silent no-op — with one opt-in exception: a
/// process that set `(process-flag :send-errors true)` gets a catchable
/// `:noconnection` error when the target *node* is unknown/disconnected (the
/// message would otherwise be dropped on the floor until a reconnect), so it can
/// queue-and-retry. Process liveness stays silent either way — `:send-errors`
/// is about the link, not the peer process.
pub fn send(heap: &Heap, target_val: Value, msg_val: Value) -> Result<(), LispError> {
    // L1 fast path: a LOCAL pid whose process is currently parked takes the value by
    // direct heap-to-heap copy, skipping `to_message`/`from_message` entirely. Tried
    // BEFORE serialising, since serialising is exactly the cost being avoided. Declines
    // (running receiver, remote node, a value the copier doesn't handle) fall through to
    // the wire path below with its semantics and error messages unchanged.
    //
    // Not taken when the sender carries a debugger trace context (dev-tools): that path
    // ships the context alongside the message and is handled below.
    #[cfg(feature = "dev-tools")]
    let has_trace = heap.trace_context().is_some();
    #[cfg(not(feature = "dev-tools"))]
    let has_trace = false;
    // Reused below when L1 declines, so the registry is consulted once for every purpose:
    // the shared-handle destination (ADR-208), and the mailbox the message is pushed to.
    let mut dest_mailbox: Option<Arc<Mailbox>> = None;
    if !has_trace
        && let Value::Pid { node, id } = target_val
        && crate::dist::is_local(node)
    {
        match try_deliver_local(heap, id, msg_val) {
            LocalDelivery::Delivered => return Ok(()),
            LocalDelivery::Declined { mailbox } => dest_mailbox = mailbox,
        }
    }
    // `dest_mailbox` was filled in by the declining L1 attempt above (one registry lookup for
    // all its jobs). It stays `None` for every other destination — a remote node, a name
    // address, an unknown pid, or a traced send — which serialises exactly as it did before
    // ADR-208 and is routed below.
    let msg = to_message_to_runtime(
        heap,
        msg_val,
        dest_mailbox.as_ref().map(|mailbox| mailbox.runtime_tag()),
    )?;
    // A live local pid whose receiver was busy: push to the mailbox L1 already found,
    // rather than letting `route` → `deliver` repeat the `is_local` test, the registry
    // shard lock and the `Arc` clone. For a local pid `route` does nothing else (no
    // counter, no trace, no guest bookkeeping — those are the remote branch), and it
    // reports `true`, so `:send-errors` has nothing to raise. See `push_and_wake` for the
    // receiver dying in between.
    if let Some(mailbox) = dest_mailbox {
        crate::perf_time!(ns_deliver, {
            push_and_wake(&mailbox, Envelope::plain(msg))
        });
        return Ok(());
    }
    // (dev-tools) Send-level causality (ADR-174): if the sender carries a debugger
    // trace context and the target is a LOCAL pid, ship the context alongside so the
    // receiver adopts it on pop. Context is per-runtime, so it never crosses nodes;
    // remote / name-addressed / no-context sends fall through to the normal route.
    #[cfg(feature = "dev-tools")]
    if let (Value::Pid { node, id }, Some(ctx)) = (target_val, heap.trace_context())
        && crate::dist::is_local(node)
    {
        let trace = to_message(heap, ctx)?;
        deliver_traced(id, msg, Some(trace));
        return Ok(());
    }
    route_message(heap, target_val, msg, Lane::Normal)
}

/// Route a serialised `msg` to `target_val` through `crate::dist::route`, in `lane`: the
/// part of [`send`] after its local fast paths, shared with [`send_in_lane`]'s remote
/// targets so a lane crosses a node link (ADR-492). Raises `:noconnection` for a process
/// that set `:send-errors` when the target's node has no link.
fn route_message(
    heap: &Heap,
    target_val: Value,
    msg: Message,
    lane: Lane,
) -> Result<(), LispError> {
    let (routed, node) = match target_val {
        Value::Pid { node, id } => (
            crate::dist::route_in_lane(node, crate::dist::Target::Pid(id), msg, lane),
            node,
        ),
        Value::Map(mid) => {
            let (name, node) = read_name_address(heap, mid)?;
            (
                crate::dist::route_in_lane(node, crate::dist::Target::Name(name), msg, lane),
                node,
            )
        }
        // A bare registered name. The local-node case of the
        // `{:name :node}` address, so a process that registered itself is reachable as
        // `(send :editor msg)` with no `whereis` round trip: that lookup answered
        // `nil | pid`, and every caller either guarded it or handed `nil` to `send` and
        // raised — where the name table already knows how to say "nobody holds this
        // name" (`route` drops and warns once). Same drop semantics as the map form.
        Value::Keyword(name) => {
            let node = crate::dist::local_node();
            (
                crate::dist::route_in_lane(node, crate::dist::Target::Name(name), msg, lane),
                node,
            )
        }
        _ => {
            return Err(LispError::type_err(
                "send: target must be a pid, a registered name (a keyword), or a {:name :node} address",
            ));
        }
    };
    if !routed && heap.proc_send_errors() {
        return Err(LispError::runtime(format!(
            "send: no connection to node {} (noconnection; raised because \
             this process set (proc/flag :send-errors true))",
            crate::core::value::symbol_name(node)
        ))
        .with_code(crate::error::error_codes::DISTRIBUTION)
        .with_hint(
            "reconnect with (connect addr) — or run a supervised reconnector \
             via reconnect/watch — then resend",
        ));
    }
    Ok(())
}

/// Read a `{:name <kw> :node <kw>}` registered-name address out of a map, returning
/// the `(name, node)` symbols. Accepts keyword or symbol values for each field.
pub(crate) fn read_name_address(heap: &Heap, mid: MapId) -> Result<(Symbol, Symbol), LispError> {
    let field = |key: &str| -> Result<Symbol, LispError> {
        let v = heap
            .map_get(mid, Value::keyword(value::intern(key)))
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

/// Upper bound on clause tags the leading-keyword filter tracks. A `receive` with more
/// clauses than this simply scans unfiltered — the array keeps the check branch-free and
/// off the heap, and real receives are far below it.
const MAX_RECEIVE_TAGS: usize = 8;

/// Read the `tags` argument the `receive` macro passes into a fixed array, returning how
/// many were collected. `0` means "no filtering": either the macro passed nil (some clause
/// is not tag-led) or the vector was longer than [`MAX_RECEIVE_TAGS`].
fn collect_receive_tags(heap: &Heap, tags: Value, out: &mut [u32; MAX_RECEIVE_TAGS]) -> usize {
    let Value::Vector(id) = tags else {
        return 0;
    };
    let items = heap.vector(id);
    if items.is_empty() || items.len() > MAX_RECEIVE_TAGS {
        return 0;
    }
    let mut n = 0;
    for v in items.iter() {
        match v {
            Value::Keyword(k) => {
                out[n] = *k;
                n += 1;
            }
            // Anything else means the macro built something unexpected; fail open.
            _ => return 0,
        }
    }
    n
}

/// Whether the receive-mark is armed (ADR-195). `BROOD_NO_RECV_MARK=1` disables it, so
/// every receive scans from the front — the A/B and bisect lever, and the stopgap if a
/// skipped message is ever suspected (a wrong skip does not crash; it silently fails to
/// deliver, the hardest fault to attribute).
fn recv_mark_enabled() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    !crate::diagnostics::debug_flags::is_set(&F, "BROOD_NO_RECV_MARK")
}

/// The leading keyword of a keyword-led vector, in the receiver's heap — the in-heap
/// counterpart of the `Message::Vector`/`Message::Keyword` peek `tag_rejects` does.
fn leading_keyword(heap: &Heap, v: Value) -> Option<u32> {
    let Value::Vector(h) = v else { return None };
    match heap.vector(h).first() {
        Some(Value::Keyword(k)) => Some(*k),
        _ => None,
    }
}

/// Can `msg` be rejected without rebuilding it into the heap? True only when the filter
/// is active AND the message is a vector whose head is a keyword that no clause names.
/// Every other shape answers `false` (scan it properly) — the filter must never skip a
/// message a clause could match.
fn tag_rejects(tagset: &[u32], msg: &Message) -> bool {
    if tagset.is_empty() {
        return false;
    }
    match msg {
        Message::Vector(items) => match items.first() {
            Some(Message::Keyword(k)) => !tagset.contains(k),
            _ => false,
        },
        _ => false,
    }
}

/// `(%receive matcher timeout)` — selective receive. `matcher` is a unary function:
/// given a message value it answers **which clause matched and what that clause's
/// pattern bound**, as a `[idx var…]` vector, or `nil` on no match. Scan the mailbox in
/// order; the first message a clause matches is removed and the matcher's answer
/// returned. The clause **body does not run here** — the `receive` macro emits every
/// body at the call site and dispatches on `idx` there, so bodies compile into the
/// owning arm and a loop that tail-calls back into `receive` stays O(1) native stack.
/// Non-matching messages stay queued.
///
/// `timeout` is `nil` (wait forever) or an integer of milliseconds; on expiry this
/// returns **`nil`**, which the macro's `after` branch tests for. That is unambiguous:
/// a match always answers with a vector, and with a `nil` timeout expiry is unreachable.
///
/// A green process suspends while waiting; the root thread blocks.
pub fn receive_match(
    heap: &mut Heap,
    matcher: Value,
    timeout: Value,
    tags: Value,
    pin: Value,
) -> LispResult {
    crate::perf_time!(ns_receive, {
        receive_match_timed(heap, matcher, timeout, tags, pin)
    })
}

fn receive_match_timed(
    heap: &mut Heap,
    matcher: Value,
    timeout: Value,
    tags: Value,
    pin: Value,
) -> LispResult {
    let ctx = current_process();
    // Mailbox-bound probe (ADR-307): a receiver parked in a never-matching `receive`
    // wakes here on every delivery and may never pass a VM safepoint, so the entry to
    // the scan is where a flooded process reliably notices its own breach. Raised
    // before any scan: catchable, and the handler can drain (`receive` still works —
    // the flag was taken) or clear the bound.
    if let Some((len, limit)) = ctx.mailbox.take_overflow_hit() {
        return Err(crate::eval::proc_mailbox_limit_error(len, limit));
    }
    // A soft `(exit pid reason)` is honoured at the NEXT `receive` — every one, not only
    // one that parks or blocks. Those two paths checked `kill_pending`; a receive whose
    // first scan matched, or whose `(after 0 …)` expired at once, ran straight past it,
    // so a process that sent itself an exit and polled its mailbox lived on until it next
    // blocked. Only in a scheduler-run green process, whose body driver turns the
    // untrappable `Control::Kill` into death (the root thread is not killable).
    if crate::process::in_capture_run() && ctx.mailbox.kill_pending.load(Ordering::Relaxed) {
        return Err(LispError::kill_signal());
    }
    // Whether this `receive` runs under state capture (a capture-mode green process):
    // the receive is re-entered from scratch on every wake, so its deadline must be
    // persisted across resumes rather than recomputed.
    let capture = crate::process::in_capture_run();
    let deadline = match timeout {
        Value::Nil => None,
        Value::Int(ms) if ms >= 0 => {
            if capture {
                // Persist the absolute deadline in the mailbox: the FIRST entry stores
                // it, later resumes reuse it — so `(after ms …)` is measured from when
                // the receive was first entered (and actually fires), instead of being
                // reset to `now + ms` on each wake. Cleared at every non-suspend exit.
                let mut st = crate::core::sync::lock(&ctx.mailbox.state);
                Some(
                    *st.recv_deadline
                        .get_or_insert_with(|| SchedDeadline::after_millis(ms as u64)),
                )
            } else {
                // Coroutine/root: the `receive_match` loop holds the deadline across its
                // waits (it never exits between scans), so compute it once here.
                Some(SchedDeadline::after_millis(ms as u64))
            }
        }
        _ => {
            return Err(LispError::type_err(
                "receive: timeout must be an integer (milliseconds) or nil",
            ));
        }
    };
    // Mark this process running (the root never goes through `run_one`, so this is
    // where its status flips back from waiting). The LOCAL-footprint figure for
    // `process-info`'s `:memory` is republished only at the *park* point below, not
    // here: `local_bytes` is an O(heap) slab walk, and recomputing it on every
    // fast-path receive (message already waiting) dominated message-passing
    // throughput (~25% of a ping-pong loop). A running process is inspected as
    // `:running`; the figure only needs to be current while it's parked.
    set_self_status(&ctx, ST_RUNNING);
    // `matcher` is needed across every loop iteration, but `eval::apply` (the matcher)
    // can now collect at ANY eval depth (ADR-061), which relocates its LOCAL closure
    // handle. Root it on the operand stack and re-read the relocated handle each use;
    // tear the region down on every return path. (The mailbox lock is dropped before
    // `apply` — the matcher calls eval; only this process removes from its own
    // mailbox, so the scanned prefix is stable and `send` only appends.)
    let rbase = heap.roots_len();
    heap.push_root(matcher); // rbase + 0
    // `tags` needs exactly the same treatment, for exactly the same reason, and used to
    // be passed down as a bare `Value` instead. It is a LOCAL vector the `receive` macro
    // built, and the scan decodes it LAZILY — inside the loop, so on any iteration after
    // the first the decode happens *after* a matcher `apply` has run and possibly
    // collected, dereferencing a handle that moved. `BROOD_GC_STRESS=1` makes that
    // deterministic: `use-after-GC: vector handle … is from epoch N` out of
    // `collect_receive_tags`. In release it is silent corruption of the tag filter, i.e.
    // a selective receive that skips a message it should have matched.
    heap.push_root(tags); // rbase + 1

    // Receive-mark (ADR-195): when EVERY clause pins a ref this process minted, no message
    // already queued when that ref was made can match — the ref did not exist yet — so the
    // scan may start at the first message with `seq >= mark` instead of the front. That is
    // what makes a request/reply receive O(1) in a backlogged mailbox rather than O(backlog).
    // Any doubt (pin absent, not a ref, not the ref we last minted) falls back to 0.
    let mark = match pin {
        Value::Ref(id) if recv_mark_enabled() => heap.recv_mark_for(id),
        _ => None,
    };
    // A native pattern spec (ADR-409) compares without an evaluator, so a pinned lazy
    // seq-view in it is realised here, once, as `%eq` would realise it per comparison. The
    // realise can collect, which is why it runs after the operands are rooted.
    let matcher_now = heap.root_at(rbase);
    let spec = super::receive_spec::resolve(heap, matcher_now);
    let realised = if spec.is_some() {
        super::receive_spec::realise_spec_views(heap, matcher_now)
            .map(|r| heap.set_root_at(rbase, r))
    } else {
        Ok(())
    };
    let result = realised
        .and_then(|()| scan_until_answered(heap, &ctx, rbase, mark, deadline, spec.as_ref()));
    // The one exit, for every way the receive ends: this scan's operand roots go, and so
    // does a persisted capture-mode deadline — unless a control signal (suspend, kill)
    // ended the scan, since a suspended receive resumes against the deadline it stored. A
    // nil-timeout receive never persisted one, so it skips the lock.
    heap.truncate_roots(rbase);
    let resumes = matches!(&result, Err(error) if error.is_control());
    if capture && deadline.is_some() && !resumes {
        crate::core::sync::lock(&ctx.mailbox.state).recv_deadline = None;
    }
    result
}

/// The scan-and-wait loop of [`receive_match_timed`]: scan the queue, then answer the
/// match, time out, suspend (a clean top-level capture receive), or block and re-scan.
/// Returns how the receive ended; the cleanup every ending shares is the caller's.
fn scan_until_answered(
    heap: &mut Heap,
    ctx: &CurrentProcess,
    rbase: usize,
    mark: Option<u64>,
    deadline: Option<SchedDeadline>,
    spec: Option<&super::receive_spec::Spec>,
) -> LispResult {
    let mut i = 0usize;
    loop {
        // Scan the queued messages for a ready clause (advancing `i` past
        // non-matches). The wait, below, is the only blocking step — this split is
        // the seam the coming state-capture path uses: there, a `None` becomes a
        // *suspend signal* returned to the scheduler instead of a `wait_for_message`.
        match scan_mailbox(heap, ctx, rbase, &mut i, mark, spec)? {
            Some(matched) => return Ok(matched),
            None => {
                // Scanned every queued message with no match. A deadline that has passed
                // completes the receive with `nil` (the macro's `after` branch tests for it).
                if deadline.is_some_and(SchedDeadline::reached) {
                    return Ok(Value::Nil);
                }
                // About to park (no clause matched, and no timeout has fired): republish
                // this process's LOCAL footprint + GC count so an observer's `process-info`
                // reads a current `:memory`/GC figure for the *waiting* process — the state
                // it's most likely inspected in. Kept off the fast (message-ready) path above.
                ctx.mailbox.mem.store(heap.local_bytes(), Ordering::Relaxed);
                ctx.mailbox
                    .gc_runs
                    .store(heap.gc_counters().0, Ordering::Relaxed);
                // Near the soft limit, measure the whole heap too: a process that runs in
                // short bursts between receives never reaches the once-per-quantum
                // safepoint that measures a running one, and while it waits in a run
                // queue the pressure selector cannot measure it (`process/pressure.rs`).
                if crate::core::alloc::near_soft_limit_approx() {
                    super::pressure::near_soft_limit(heap);
                }
                // A process that mostly waits reaches few quantum boundaries; a resized
                // container's cap is re-read here too (at most every few seconds).
                crate::core::alloc::refresh_container_limits();
                // State-capture path (ADR-100 §8): a clean no-match in a green process
                // (`in_capture_run`) becomes a *suspend control signal* returned to the
                // scheduler instead of blocking the worker. Record the scan position (so
                // the scheduler re-runs us only on a NEW message) and arm any deadline
                // timer, then drop this scan's operand roots (as the match / timeout
                // returns do, so the driver's `%receive` `Inst::Call` re-runs against a
                // clean operand stack on resume) and hand the suspend signal up: it rides
                // the error channel through `%receive` to `vm_run_bc`, which captures the
                // VM continuation and returns it for `run_one` to park. The root thread,
                // a tree-walked process body, and a **native-nested** capture receive
                // (`in_capture_run` but not `capture_top_level`: a `%isolate`/`%try`/HOF
                // callback sits between this receive and the body driver) all fall through
                // to the condvar wait below instead. A native-nested receive's continuation
                // can't be captured across the native frame, and re-running the native
                // repeats side effects (the §8.1 footgun), so it **blocks the worker** —
                // the dirty-scheduler carve-out (§7.4). Only a clean top-level receive
                // captures (and can migrate).
                if crate::process::in_capture_run() && crate::process::capture_top_level() {
                    // `scanned` was recorded by the scan's no-match return, under the lock
                    // it held to find the end of the queue — so suspending takes no lock
                    // of its own. That value is what re-locking here to store
                    // `i.min(queue.len())` computed: `i` was the queue's length then, and
                    // since then only senders have touched the queue, and they append.
                    set_self_status(ctx, ST_WAITING);
                    if let Some(d) = deadline {
                        let generation = ctx.mailbox.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
                        arm_timer(ctx.pid, d, generation);
                    }
                    return Err(LispError::suspend(deadline));
                }
                // A green process dirty-blocking (not the root thread): record which
                // native activation encloses this receive, so that gateway latches its
                // arm off native — a blocked worker and an unmigratable process are what
                // the latch heals (see `Heap::blocked_under_gateway`). Two stores on a
                // path that is about to park a whole OS thread.
                if crate::process::in_capture_run() {
                    heap.blocked_under_gateway = heap.cur_native_gateway;
                    crate::process::scheduler::note_dirty_receive_block();
                    // Under the JIT's bail-trace flag, name the token too: `token=0` is
                    // the UNLATCHABLE class — no JIT gateway alive, i.e. the nesting is a
                    // Rust-native shape (a builtin HOF's `vm_apply` driver, a `try`
                    // callback), which no latch can heal. Distinguishing that from a
                    // latchable native host by silence cost a debugging round.
                    if crate::diagnostics::debug_flags::jit_bail_trace() {
                        eprintln!(
                            "[jit-bail] dirty-receive-block gateway-token={} pid={}",
                            heap.cur_native_gateway, ctx.pid
                        );
                    }
                }
                wait_for_message(ctx, i, deadline);
                // Back from the wait → running again. This *block* path (the root thread,
                // and a native-nested capture receive — §7.4) returns inline without a
                // `run_one`, so its `ST_WAITING` would otherwise leak into the next scan
                // (and a `process-info` after the receive would read `:waiting` while
                // running). The capture path that suspends above is flipped back by
                // `run_one` on resume instead.
                set_self_status(ctx, ST_RUNNING);
                // An `(exit …)` may have woken us rather than a message, and a blocked
                // receive has no `run_one` boundary to check `kill_pending`, so do it here
                // — only in a scheduler-run green process, whose body driver turns the
                // untrappable `Control::Kill` into death (`VmOutcome::Killed`, reason still
                // in `state.kill`). The root thread is not killable; it re-scans.
                if crate::process::in_capture_run()
                    && ctx.mailbox.kill_pending.load(Ordering::Relaxed)
                {
                    return Err(LispError::kill_signal());
                }
                // The mailbox bound (ADR-307), re-probed on every wake of this BLOCKING
                // path: the entry probe ran once, and a flood arriving while blocked would
                // otherwise surface only at a quantum rollover. Raised like the entry probe.
                if let Some((len, limit)) = ctx.mailbox.take_overflow_hit() {
                    return Err(crate::eval::proc_mailbox_limit_error(len, limit));
                }
            }
        }
    }
}

/// Run a matcher CLOSURE on candidate `v`: through the VM / JIT fast-frame when it resolves
/// to a plain arm (resolved lazily, on the scan's first candidate, into `hof`), falling back
/// to the tree-walking `eval::apply` otherwise (or when `hof_apply_step` deopts on an identity
/// miss — e.g. a mid-scan GC relocated the matcher). Same semantics either way: the clause's
/// `[idx var…]` answer on a match, nil on no-match.
fn apply_matcher(
    heap: &mut Heap,
    hof: &mut Option<Option<crate::eval::compile::HofArm>>,
    matcher: Value,
    v: Value,
) -> LispResult {
    if hof.is_none() {
        // Below the bytecode tier no arm resolves, so the matcher tree-walks (ADR-416).
        let use_vm = crate::eval::compile::tier_ceiling() >= crate::eval::compile::Tier::Bytecode;
        *hof = Some(crate::perf_time!(ns_match_resolve, {
            if use_vm {
                crate::eval::compile::hof_resolve(heap, matcher, 1)
            } else {
                None
            }
        }));
    }
    crate::perf_time!(ns_match_run, {
        match hof.as_ref().unwrap() {
            Some(h) => match crate::eval::compile::hof_apply_step(heap, h, matcher, &[v]) {
                Some(r) => r,
                None => eval::apply(heap, matcher, &[v], EnvId::GLOBAL),
            },
            None => eval::apply(heap, matcher, &[v], EnvId::GLOBAL),
        }
    })
}

/// Scan the mailbox from index `*i` for the first message a clause matches, advancing
/// `*i` past non-matching messages (selective receive — non-matches stay
/// queued). Returns `Ok(Some(answer))` — the matcher's `[idx var…]` result for the
/// matched clause, with that message removed — or `Ok(None)` when every
/// currently-queued message was scanned with no match (the caller then either captures
/// a suspend signal or blocks and re-scans — see `receive_match`).
///
/// This does the matcher `apply` (which can collect at any eval depth — ADR-061 — so the
/// operand-rooted `matcher` at `rbase+0` and clause tags at `rbase+1` are re-read at each
/// use rather than captured) but **never
/// blocks/waits for a message** — that's the caller's `wait_for_message`. It is thus the
/// reusable scan for both the capture-suspend and the worker-block paths. The
/// `receive`-boundary kill check rides at the top (a soft `(exit …)` — and any hard
/// `:kill` not caught at the VM loop top — dies here with its reason rather than taking
/// another message).
fn scan_mailbox(
    heap: &mut Heap,
    ctx: &CurrentProcess,
    rbase: usize,
    i: &mut usize,
    // The receive-mark, if this receive pinned a ref we minted (ADR-195): no message with
    // `seq` below it can match, so the scan may start past them.
    mark: Option<u64>,
    // The receive's native pattern spec (ADR-409), or `None` to run the matcher closure.
    spec: Option<&super::receive_spec::Spec>,
) -> Result<Option<Value>, LispError> {
    // Resolve the matcher to its compiled VM arm lazily, on the FIRST queued candidate:
    // each candidate then runs the pattern dispatch through the bytecode VM / JIT
    // fast-frame — the same fast closure-call path `fold`/`map`/`reduce` use for their
    // step fn — instead of the tree-walking `eval::apply` (~10× slower per ADR-076). The
    // matcher is a fixed-arity-1 `(fn (msg) …)`; `None` (not a plain VM closure) leaves
    // the scan on the general path. Lazy so an empty-mailbox scan (the common suspend
    // entry — no candidate to match) pays nothing. `Some(None)` = resolved-to-unusable.
    let mut hof: Option<Option<crate::eval::compile::HofArm>> = None;
    // Single-lock fast path for the FIRST candidate of a scan (the common case: the
    // first queued / just-delivered message matches): pop it out under the one lock,
    // then build (`from_message`) and match it with the mutex RELEASED — a match is
    // done with no second acquisition, and the sender never contends with the deep
    // copy into the receiver's heap. `send` only appends, but the owner ITSELF can
    // remove from this queue during the unlocked window (a clause guard's consuming
    // nested `receive` — see `reinsert_at_seq`), so position `*i` is NOT stable
    // across it: every re-acquisition re-identifies the candidate by `seq`, never by
    // the recorded index. A non-match pays one extra lock to re-insert the message at
    // `*i` (arrival order preserved) and reverts the rest of the scan to
    // peek-in-place, so a long selective-receive backlog isn't popped/re-inserted
    // per candidate. A LARGE candidate always takes the pop/re-insert route (ADR-245,
    // `message_fits` below): peeking rebuilds under the lock, so one extra acquire buys
    // a bounded hold; the tag filter keeps that off unmatchable backlog entirely.
    let mut optimistic = true;
    // Arrival seq of the last examined-and-declined candidate. The matcher runs with the
    // mailbox lock RELEASED and can run arbitrary user code — a clause `:when` guard that
    // itself runs a *consuming* nested `receive` removes messages from this very queue
    // during that window (see `reinsert_at_seq`) — so the scan cursor `*i` can go stale
    // between iterations. Each loop top re-anchors `*i` against this seq.
    let mut prev_seq: Option<u64> = None;
    // Lazily-decoded clause tags (see the filter below). `None` = not decoded yet.
    let mut tagbuf = [0u32; MAX_RECEIVE_TAGS];
    let mut ntags: Option<usize> = None;
    loop {
        // (A hard `:kill` is caught at the VM driver's loop top; a soft exit at the
        // `park_on_receive` boundary — not here, since there's no coroutine to suspend.)
        // Rebuild candidate `*i` into the heap (no eval here → no collection).
        let (popped, v, cand_seq, cand_latest) = {
            let mut st = crate::core::sync::lock(&ctx.mailbox.state);
            // Re-anchor the cursor: `*i` is only meaningful relative to the queue the
            // previous iteration saw, and the matcher may have consumed entries out from
            // under it (the guard hazard above). The fast path — the entry just examined
            // still sits at `*i - 1` — is O(1) and holds whenever nothing shifted; only
            // the perturbed case pays the binary search (sound: `queue` is strictly
            // increasing in `seq`). Without this, a guard that consumed entries *ahead*
            // of the cursor makes `*i` skip an unexamined message — one that stays
            // queued, but that THIS receive never sees, so a matchable message reads as
            // absent until two more arrivals.
            if let Some(ps) = prev_seq {
                let anchored = *i > 0 && *i <= st.queue.len() && st.queue[*i - 1].seq == ps;
                if !anchored {
                    *i = st.queue.partition_point(|e| e.seq <= ps);
                }
            }
            // Every no-match return records `scanned` under the lock it already holds (here
            // and the two below), for `park_on_receive`'s "did a message arrive past what
            // we looked at?" gate. The whole queue, which is where `*i` stands at all
            // three: at least `*i` (the re-anchor and the mark search never leave it past
            // the end), and never MORE than the queue — a `scanned` past the length
            // needs several more messages before the gate re-runs the process, which then
            // sleeps through the one it is waiting for (a matcher's consuming nested
            // `receive` can shorten the queue under the scan).
            if *i >= st.queue.len() {
                st.scanned = st.queue.len();
                return Ok(None); // scanned to the end with no match
            }
            // Receive-mark: skip straight past every message that predates the pinned ref.
            // The queue is ordered by `seq`, so this is a binary search, and it runs only
            // while `*i` is still pointing at an older message (i.e. once per receive).
            if let Some(m) = mark
                && st.queue[*i].seq < m
            {
                let lo = st.queue.partition_point(|e| e.seq < m);
                if lo > *i {
                    *i = lo;
                    optimistic = false; // the head is no longer the candidate
                }
                if *i >= st.queue.len() {
                    st.scanned = st.queue.len();
                    return Ok(None);
                }
            }
            // Leading-keyword filter: reject a candidate no clause could match without
            // rebuilding it into the heap or running the matcher. This is what keeps a
            // selective receive from being O(rounds x backlog) — a non-matching message
            // costs a keyword compare instead of a `from_message` + a matcher
            // activation. Conservative: only fires for a tag-led vector whose head is
            // absent from the clause set (see `receive--tags`).
            //
            // Decoded lazily, and only when there is a backlog to filter: the dominant
            // shape is a one-message mailbox whose single candidate matches, and paying
            // a decode per receive for that cost `pingpong` ~3.5%.
            if st.queue.len() > 1 && ntags.is_none() {
                // Re-read the RELOCATED handle from the roots stack, exactly as `matcher`
                // is re-read below: this decode can run after a matcher `apply` collected.
                let tags = heap.root_at(rbase + 1);
                ntags = Some(collect_receive_tags(heap, tags, &mut tagbuf));
            }
            // Skip every tag-rejected candidate under THIS one lock hold. The filter needs
            // nothing but the envelope's tag, so releasing and re-acquiring the mailbox mutex
            // per rejected message — which is what this did — cost one lock round-trip per
            // queued message per receive, on top of the O(backlog) walk itself. A process
            // with a backlog pays that on every selective receive.
            let mut skipped_any = false;
            loop {
                if *i >= st.queue.len() {
                    st.scanned = st.queue.len();
                    return Ok(None); // scanned to the end with no match
                }
                // A message its source took back is dropped unseen, returning its credit
                // (`CreditSource::withdrawn`); the next one moves up to `*i`.
                if st.queue[*i]
                    .credit
                    .as_ref()
                    .is_some_and(|credit| credit.withdrawn())
                {
                    st.remove_at(*i);
                    continue;
                }
                let rejected = match &st.queue[*i].msg {
                    Payload::Wire(m) => tag_rejects(&tagbuf[..ntags.unwrap_or(0)], m),
                    // Already a heap value: the copy is spent, so there is nothing to save
                    // by rejecting it here. Let the matcher decide.
                    Payload::Local { tag, .. } => {
                        match (tagbuf[..ntags.unwrap_or(0)].is_empty(), tag) {
                            (false, Some(k)) => !tagbuf[..ntags.unwrap_or(0)].contains(k),
                            _ => false,
                        }
                    }
                };
                if !rejected {
                    break;
                }
                *i += 1;
                skipped_any = true;
            }
            if skipped_any {
                optimistic = false; // the head is no longer the candidate
            }
            // The candidate's identity for the rest of this iteration. `*i` alone is NOT
            // that identity: the matcher below runs with the lock released, so by consume
            // time the index may name a different envelope — see the consume path.
            let cand_seq = st.queue[*i].seq;
            // A LATEST candidate left in the queue while it is matched can be superseded
            // (removed) by a delivery in that window; the consume path checks for that.
            let cand_latest = matches!(st.queue[*i].lane, Lane::Latest(_));
            if optimistic {
                let m = st.remove_at(*i).expect("bounds checked above");
                drop(st);
                // A Local payload is already in this heap — read it from its traced slot.
                // A Wire one is rebuilt as before.
                let v = match &m.msg {
                    Payload::Wire(w) => from_message(heap, w),
                    // PEEK, not take: a candidate that fails to match is re-inserted into
                    // the queue with this same slot index, so clearing (tombstoning) the
                    // slot here would corrupt the re-queued message to `nil` — the slot is
                    // reused by the next `msg_root_add`. The slot is freed on the match
                    // path below, the one place the message is actually consumed.
                    Payload::Local { slot, .. } => heap.msg_root_peek(*slot),
                };
                (Some(m), v, cand_seq, false)
            } else {
                // Peek-in-place: a Local payload must NOT be taken here (the candidate
                // may not match and has to stay queued), so read the slot without
                // clearing it. A Wire payload's rebuild would run WITH THE LOCK HELD, so a
                // big one (past the ADR-245 budget) takes the optimistic route instead —
                // pop, rebuild unlocked, re-insert in seq order on a non-match.
                let over = match &st.queue[*i].msg {
                    Payload::Wire(w) => !crate::process::message::message_fits(
                        w,
                        crate::process::message::l1_copy_budget(),
                    ),
                    Payload::Local { .. } => false,
                };
                if over {
                    let m = st.remove_at(*i).expect("bounds checked above");
                    drop(st);
                    let v = match &m.msg {
                        Payload::Wire(w) => from_message(heap, w),
                        Payload::Local { slot, .. } => heap.msg_root_peek(*slot),
                    };
                    // `Some(m)` routes it through the same re-insert path the optimistic
                    // branch uses on a non-match, so nothing new has to be maintained.
                    (Some(m), v, cand_seq, false)
                } else {
                    let v = match &st.queue[*i].msg {
                        Payload::Wire(w) => from_message(heap, w),
                        Payload::Local { slot, .. } => heap.msg_root_peek(*slot),
                    };
                    (None, v, cand_seq, cand_latest)
                }
            }
        };
        let matcher = heap.root_at(rbase);
        // A native pattern spec (ADR-409): every clause is a shape the kernel matches
        // itself, so no closure is resolved or called for this candidate.
        let applied = if let Some(spec) = spec {
            crate::perf_time!(ns_match_run, {
                Ok(super::receive_spec::spec_answer(heap, spec, matcher, v))
            })
        } else {
            apply_matcher(heap, &mut hof, matcher, v)
        };
        let answer = match applied {
            Ok(t) => t,
            Err(e) => {
                // An erroring matcher must not lose the candidate: put an
                // optimistically-popped message back before propagating.
                if let Some(m) = popped {
                    let _ = reinsert_candidate(ctx, *i, m);
                }
                return Err(e);
            }
        };
        if !matches!(answer, Value::Nil) {
            // Matched — the message is consumed. An optimistically-popped candidate is
            // already out of the queue (`popped`); a peeked one is removed now. Either
            // way, free its msg_roots slot: a Local payload roots its value in that slot,
            // and the scan only PEEKS it (so a non-match can leave it queued intact), so
            // the consume path is the one place that tombstones it for reuse — without
            // this the slot table would grow unboundedly. The clause BODY is not run here:
            // the `receive` macro emits it at the call site and dispatches on `idx` there,
            // so a loop that tail-calls back into `receive` stays O(1) native stack. The
            // matcher's `[idx var…]` answer already roots any value the clause bound.
            //
            // A LATEST message is handed over only if it is still the newest of its key
            // (ADR-490): one superseded while it was out of the queue for this match is
            // dropped, and the scan goes on past it — so a receiver never sees an older
            // copy after a newer one was delivered. The check costs an ordinary message
            // nothing but the lane test.
            let consumed = match popped {
                Some(env) => {
                    let superseded = match &env.lane {
                        Lane::Latest(key) => {
                            crate::core::sync::lock(&ctx.mailbox.state).holds_latest(key)
                        }
                        _ => false,
                    };
                    if superseded {
                        drop(env);
                        prev_seq = Some(cand_seq); // re-anchors past the dropped candidate
                        continue;
                    }
                    Some(env)
                }
                None => {
                    // Remove by `seq`, NEVER by the recorded index: the matcher ran with
                    // the lock released, and a guard's consuming nested `receive` may have
                    // shifted the queue (the `reinsert_at_seq` hazard). Removing by a
                    // stale index silently deletes a *neighbour* — which is then LOST,
                    // and its msg_roots slot below is wrongly freed — while the matched
                    // message stays queued to match (and be delivered) again. `None`
                    // (the guard consumed the candidate itself) removes nothing.
                    let mut st = crate::core::sync::lock(&ctx.mailbox.state);
                    let j = if st.queue.get(*i).is_some_and(|e| e.seq == cand_seq) {
                        Some(*i) // unshifted — the common case, O(1)
                    } else {
                        let j = st.queue.partition_point(|e| e.seq < cand_seq);
                        st.queue
                            .get(j)
                            .is_some_and(|e| e.seq == cand_seq)
                            .then_some(j)
                    };
                    let removed = j.and_then(|j| st.remove_at(j));
                    // A LATEST candidate gone from the queue was superseded by a newer
                    // delivery while it was matched (or a guard's nested receive took it):
                    // either way it is not this receive's to hand over.
                    if removed.is_none() && cand_latest {
                        drop(st);
                        prev_seq = Some(cand_seq);
                        continue;
                    }
                    removed
                }
            };
            if let Some(env) = consumed.as_ref()
                && let Payload::Local { slot, .. } = &env.msg
            {
                heap.msg_root_take(*slot);
            }
            // `proc/receive-lane` reports the lane of what it took (ADR-490)
            if let Some(env) = consumed.as_ref()
                && matches!(env.lane, Lane::Priority)
            {
                ctx.mailbox.took_priority.store(true, Ordering::Relaxed);
            }
            // Adopt the message's causal context (ADR-174 send-level) so this receive —
            // and anything it then spawns or sends — runs in the sender's causal context.
            #[cfg(feature = "dev-tools")]
            if let Some(t) = consumed.and_then(|e| e.trace) {
                let v = from_message(heap, &t);
                // Adopted from a message (`own = false`): used to handle this message,
                // but not propagated onward by `spawn` — so it can't leak transitively.
                heap.set_trace_context(Some(v), false);
            }
            #[cfg(not(feature = "dev-tools"))]
            let _ = consumed;
            return Ok(Some(answer));
        }
        if let Some(m) = popped {
            // Resume from where the envelope actually landed, not from the stale recorded
            // position: if a matcher-run nested `receive` shifted the queue under us, `*i`
            // no longer names this message and advancing from it would either re-examine
            // it or run off the end (leaving `scanned` past the queue length, which parks
            // the process until *two* more messages arrive).
            *i = reinsert_candidate(ctx, *i, m);
            optimistic = false; // one non-match → peek-in-place for the rest of the scan
        }
        prev_seq = Some(cand_seq); // the loop top re-anchors `*i` against this
        *i += 1; // no clause matched — leave it queued, try the next message
    }
}

/// Put an optimistically-popped candidate back **in seq order**, returning the index it
/// landed at (the caller resumes its scan from there).
///
/// Not simply at the recorded scan position `i`: the matcher runs with the lock
/// **released**, and a clause `:when` guard that runs a *consuming* nested `receive` can
/// remove entries ahead of `i`, so `i` may now name a higher-`seq` slot. Inserting there
/// breaks the [`Envelope::seq`] ordering, and a later ref-pinned receive's
/// `partition_point` can then silently skip a matchable reply. The recorded position is
/// checked first (O(1), right whenever nothing shifted); only the perturbed case searches.
fn reinsert_at_seq(st: &mut MailboxState, i: usize, m: Envelope) -> usize {
    let seq = m.seq;
    let len = st.queue.len();
    // Is the recorded scan position still this envelope's seq-ordered slot?
    let unshifted =
        i <= len && (i == 0 || st.queue[i - 1].seq < seq) && (i == len || st.queue[i].seq > seq);
    let idx = if unshifted {
        i
    } else {
        // Sound because the queue is ordered by `seq` (the invariant this function is
        // what maintains), so the predicate really is partitioned here.
        st.queue.partition_point(|e| e.seq < seq)
    };
    st.insert_at(idx, m);
    debug_assert!(
        idx == 0 || st.queue[idx - 1].seq < seq,
        "mailbox queue left out of seq order by a reinsert"
    );
    debug_assert!(
        idx + 1 == st.queue.len() || st.queue[idx + 1].seq > seq,
        "mailbox queue left out of seq order by a reinsert"
    );
    idx
}

/// [`reinsert_at_seq`] against the current process's mailbox, taking the lock.
fn reinsert_candidate(ctx: &CurrentProcess, i: usize, m: Envelope) -> usize {
    let mut st = crate::core::sync::lock(&ctx.mailbox.state);
    // A LATEST candidate superseded while it was out (ADR-490) is not put back: the newer
    // one is queued. Answer where it would have landed; the scan's re-anchor on its seq
    // continues from the message after it.
    if let Lane::Latest(key) = &m.lane
        && st.holds_latest(key)
    {
        let seq = m.seq;
        drop(m);
        return st.queue.partition_point(|e| e.seq < seq);
    }
    reinsert_at_seq(&mut st, i, m)
}

/// Block until a message beyond index `i` might be available, honouring `deadline`,
/// then return for the caller to re-scan from `i`. With no coroutine, a `receive` that
/// must wait **blocks its worker thread** on the mailbox condvar — the dirty-scheduler
/// path (§7.4): `dirty_block` excludes the worker from `assign_worker` and re-routes its
/// backlog so nothing is stranded on a worker that won't run it (the mass-kill/monitor
/// deadlock); the real **root** thread (which owns no worker) just blocks. Only reached
/// by the root thread and a **native-nested** capture `receive`; a clean *top-level*
/// capture receive never gets here — it captures its continuation and returns instead
/// (the gate in `receive_match`).
///
/// **Clock domain.** A [`SchedDeadline`] can only be measured on the scheduler clock it
/// was minted on, so the wait below cannot mix in real time (which spins the wasm pump).
///
/// **Known defect: this does not work on wasm32.** `std`'s `no_threads` `Condvar`
/// panics in `wait`, an uncatchable trap there, and a native-nested receive (e.g.
/// `(do (sleep 5) :slept)`) reaches it. The fix is a non-blocking park for the nested
/// case on wasm.
fn wait_for_message(ctx: &CurrentProcess, i: usize, deadline: Option<SchedDeadline>) {
    let st = crate::core::sync::lock(&ctx.mailbox.state);
    if st.queue.len() > i {
        return; // a message arrived between the scan and here — re-scan
    }
    // Don't block if an `(exit …)` is already pending — but only inside a real
    // scheduler-run green process (`in_capture_run`), where a top-level body driver
    // exists to turn the unwinding `Control::Kill` into process death. On the root /
    // file-runner thread there's no such driver (a `Control::Kill` would just leak as
    // an error), and that thread isn't a killable process, so it keeps the old
    // behaviour: block normally, ignoring `kill_pending`. `exit` sets `kill_pending`
    // *before* it takes this state lock (in `take_wake`), so a kill that fully
    // completed before we got here is visible now under the lock — without this check
    // its `cv.notify_one()` would have been lost (no waiter yet) and we'd block forever.
    // The caller (`receive_match`) re-checks and unwinds; serialised with `exit` by
    // this same lock, so no kill slips in between the check and the wait.
    if crate::process::in_capture_run() && ctx.mailbox.kill_pending.load(Ordering::Relaxed) {
        return;
    }
    set_self_status(ctx, ST_WAITING);
    // Register on `cv_waiters` under this same lock hold, so every wake site's
    // critical section either runs before us (we see its effect on the re-checks
    // above and never block) or after (it reads a non-zero count and notifies).
    // The decrement runs on the guard `wait` hands back — exactly once per
    // increment, on timeout and spurious wakes included. See `wake_both`.
    let mut st = st;
    // Latched wake: a wake recorded since our scan means new state to look at — treat
    // it exactly like a spurious wakeup (return; `receive_match` rescans). Consumed
    // under the same lock `Condvar::wait` would atomically release, so no wake set
    // after this check can be missed: its setter either ran before (we see the flag)
    // or will run after our register below (and sees `cv_waiters > 0`, so it notifies).
    if st.wake_pending {
        st.wake_pending = false;
        return;
    }
    // Only now is this thread about to block, so only now does its worker leave the
    // executors (`dirty_block` re-routes its queue and may start a drainer thread). The
    // early returns above never block, and paid that whole-queue drain for nothing
    // (compute-frontier §7.8 item 6). Still under the state lock, still before the wait,
    // and the guard still lives until the wait returns.
    let _dirty = crate::process::dirty_block();
    match deadline {
        Some(d) => {
            let remaining = d.remaining();
            if !remaining.is_zero() {
                st.cv_waiters += 1;
                let (mut g, _timed_out) = ctx
                    .mailbox
                    .cv
                    .wait_timeout(st, remaining)
                    .unwrap_or_else(|e| e.into_inner());
                g.cv_waiters -= 1;
                g.wake_pending = false;
            }
        }
        None => {
            st.cv_waiters += 1;
            let mut g = ctx.mailbox.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            g.cv_waiters -= 1;
            g.wake_pending = false;
        }
    }
}

/// The park generation local process `pid`'s mailbox is currently on, or `None` if it is
/// dead/unknown. A pending timer entry is **live** iff its stamped gen equals this — the
/// same test [`wake_for_timeout`] applies at firing time, exposed so `super::timer`'s
/// compaction can apply it early instead of leaving superseded entries on the heap until
/// their deadlines come due. One relaxed load behind a sharded registry lookup; never
/// takes the mailbox `state` lock, so it cannot deadlock against a sender.
pub(super) fn current_timer_gen(pid: u64) -> Option<u64> {
    REGISTRY
        .get(pid)
        .map(|mb| mb.timer_gen.load(Ordering::Relaxed))
}

/// Re-queue green process `pid` if it's still parked on the deadline identified by
/// `gen`, so it wakes, re-scans, and — finding its deadline passed — runs its
/// `after` clause. `gen` is the park generation the timer entry was stamped with
/// (`arm_timer`); if the mailbox's `timer_gen` has since advanced, this entry is a
/// **superseded** deadline (the process re-parked with a newer one, or moved on),
/// so we drop it without waking — lazy timer cancellation (see `Mailbox::timer_gen`).
/// A no-op too if `send` already woke it. The process always re-validates its own
/// deadline, so even a wakeup that slips through is harmless (at most one spurious).
pub(super) fn wake_for_timeout(pid: u64, generation: u64) {
    let mailbox = REGISTRY.get(pid);
    if let Some(mb) = mailbox {
        // Stale entry — the process has re-parked (or moved on) since this timer
        // was armed. Skip it: the live deadline has its own, current-gen entry.
        if mb.timer_gen.load(Ordering::Relaxed) != generation {
            return;
        }
        // Both paths (`Wake`). This site once had **no** condvar notify at all — alone among
        // the wake sites — which was survivable only because a cv-blocked receiver with a
        // deadline sits in `wait_timeout` and self-wakes at the same instant. That made the
        // timer thread's wake redundant *for that shape* and missing for any other; leaving
        // one wake site with different reachability is how the either/or bug above got in.
        let wake = crate::core::sync::lock(&mb.state).take_wake();
        wake.fire(&mb); // timer wake (capture-mode → may migrate)
    }
}

/// Every currently-registered local pid (one entry per live mailbox). Backs
/// the `(list-processes)` primitive — agents introspecting what they've
/// spawned, and the `nest mcp` `processes` tool (`std/tool/mcp.blsp`, ADR-036).
/// Order is unspecified (hash-map iteration); callers that care can sort.
pub fn list_local_pids() -> Vec<u64> {
    REGISTRY.pids()
}

/// The number of messages queued in local process `pid`'s mailbox (its receive
/// backlog), or `None` if no live local process has that id. Backs the
/// `mailbox-size` primitive — the one bit of per-process state an observer needs
/// that lives behind the scheduler registry. Takes the registry lock, then the
/// mailbox's own lock, briefly.
pub fn mailbox_len(pid: u64) -> Option<usize> {
    with_mailbox(pid, |mb| crate::core::sync::lock(&mb.state).queue.len())
}

/// Set (or with `None` clear) `pid`'s mailbox bound, returning the previous one —
/// the `(proc/flag :max-mailbox n)` mechanism (ADR-307). Clearing also clears a
/// pending breach, so doing it inside a `catch` genuinely rescues the process.
///
/// EITHER direction re-arms the one-shot latch (KI-103): setting a bound is the act that
/// arms the protection, so a process that raises its bound after a breach is protected
/// again at the new value, and one that clears it starts clean if it sets a bound later.
pub fn set_max_mailbox(pid: u64, limit: Option<usize>) -> Option<usize> {
    with_mailbox(pid, |mb| {
        // Storing 0 both clears a pending breach and lifts the one-shot LATCH, in either
        // direction: setting a bound is the act that arms the protection (KI-103).
        mb.overflow_hit.store(0, Ordering::Relaxed);
        let prev = mb.max_mailbox.swap(limit.unwrap_or(0), Ordering::Relaxed);
        (prev != 0).then_some(prev)
    })
    .flatten()
}

/// `pid`'s mailbox bound, if set — backs the `(proc/flag :max-mailbox)` read.
pub fn max_mailbox(pid: u64) -> Option<usize> {
    with_mailbox(pid, |mb| {
        let limit = mb.max_mailbox.load(Ordering::Relaxed);
        (limit != 0).then_some(limit)
    })
    .flatten()
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

/// The CPU time process `pid` has consumed, in ns: its booked quanta, or — for a root
/// context pinned to its thread — that thread's CPU clock. `None` for an unknown pid.
pub fn process_cpu_ns(pid: u64) -> Option<u64> {
    with_mailbox(pid, |mb| {
        let clock = mb.thread_clock.load(Ordering::Relaxed);
        let live = if clock >= 0 {
            super::cputime::clock_ns(clock).unwrap_or(0)
        } else {
            0
        };
        mb.cpu_ns.load(Ordering::Relaxed) + live
    })
}

/// Book `ns` of CPU time to `mailbox` — a quantum's, at its end.
pub(super) fn book_cpu_ns(mailbox: &Mailbox, ns: u64) {
    mailbox.cpu_ns.fetch_add(ns, Ordering::Relaxed);
}

/// Pin `mailbox`'s CPU time to the CALLING thread's clock — for a root context, which runs
/// on that thread for its whole life and is never scheduled in quanta.
pub(super) fn pin_cpu_clock_to_this_thread(mailbox: &Mailbox) {
    if let Some(clock) = super::cputime::current_thread_clock() {
        mailbox.thread_clock.store(clock, Ordering::Relaxed);
    }
}

/// Set the run-status of the *current* process (used by `receive_match` for the
/// root, which never goes through `run_one`).
fn set_self_status(ctx: &CurrentProcess, status: u8) {
    set_status(&ctx.mailbox, status);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_lane(n: i64, lane: Lane) -> Envelope {
        let mut envelope = Envelope::plain(Message::Int(n));
        envelope.lane = lane;
        envelope
    }

    fn latest(n: i64, key: &str) -> Envelope {
        in_lane(n, Lane::Latest(Box::new(key.to_string())))
    }

    fn queued(st: &MailboxState) -> Vec<i64> {
        st.queue
            .iter()
            .map(|e| match &e.msg {
                Payload::Wire(Message::Int(n)) => *n,
                _ => panic!("test envelopes are ints"),
            })
            .collect()
    }

    /// A LATEST envelope supersedes the queued one with its key, and only that one: other
    /// keys and ordinary messages keep their place, the replacement queues at the back,
    /// and seq order — what the receive-mark binary-searches on — survives (ADR-490).
    #[test]
    fn a_latest_message_replaces_only_its_own_key() {
        let mb = Mailbox::new();
        let mut st = crate::core::sync::lock(&mb.state);
        st.push(latest(1, ":a"));
        st.push(Envelope::plain(Message::Int(2)));
        st.push(latest(3, ":b"));
        st.push(latest(4, ":a"));
        assert_eq!(queued(&st), vec![2, 3, 4]);
        st.push(latest(5, ":b"));
        st.push(latest(6, ":a"));
        assert_eq!(queued(&st), vec![2, 5, 6]);
        assert!(
            st.queue
                .iter()
                .zip(st.queue.iter().skip(1))
                .all(|(a, b)| a.seq < b.seq)
        );
        assert!(
            std::mem::size_of::<Lane>() <= 16,
            "a lane is a tag and a pointer"
        );
    }

    /// The park gate re-runs a parked receiver only for a message past the `scanned` it
    /// already rejected. A replacement that removes one of those must shrink the count,
    /// or the new message reads as already scanned and the receiver sleeps through it.
    #[test]
    fn superseding_a_scanned_message_keeps_the_park_gate_open() {
        let mb = Mailbox::new();
        let mut st = crate::core::sync::lock(&mb.state);
        st.push(latest(1, ":a"));
        st.push(Envelope::plain(Message::Int(2)));
        st.scanned = 2;
        st.push(latest(3, ":a"));
        assert_eq!(queued(&st), vec![2, 3]);
        assert!(st.queue.len() > st.scanned, "the replacement is unscanned");
    }

    /// The lane bookkeeping is exact through every way a message leaves and comes back:
    /// the PRIORITY count after a plain receive consumed one (so `take-priority` answers
    /// "none" without a walk), and the LATEST index after a candidate was taken out and put
    /// back, superseded, or consumed (ADR-490).
    #[test]
    fn the_lane_bookkeeping_is_exact() {
        let mb = Mailbox::new();
        let mut st = crate::core::sync::lock(&mb.state);
        st.push(Envelope::plain(Message::Int(1)));
        assert_eq!(st.priority_queued, 0);
        st.push(in_lane(2, Lane::Priority));
        st.push(in_lane(3, Lane::Priority));
        assert_eq!(st.priority_queued, 2);
        // a plain receive consumed one, by its index
        let index = st.queue.iter().position(|e| e.seq == 1).expect("queued");
        drop(st.remove_at(index));
        assert_eq!(st.priority_queued, 1);
        let count = |st: &MailboxState| {
            st.queue
                .iter()
                .filter(|e| matches!(e.lane, Lane::Priority))
                .count()
        };
        assert_eq!(st.priority_queued, count(&st));
        // a LATEST candidate out of the queue is not indexed; put back, it is again
        st.push(latest(4, ":a"));
        assert!(st.holds_latest(":a"));
        let index = st.queue.len() - 1;
        let candidate = st.remove_at(index).expect("queued");
        assert!(!st.holds_latest(":a"), "out for a match");
        reinsert_at_seq(&mut st, index, candidate);
        assert!(st.holds_latest(":a"), "back in the queue");
        // superseded: the index follows the newer one, and a consume of it clears it
        st.push(latest(5, ":a"));
        assert_eq!(
            st.latest_seq(":a"),
            Some(st.queue.back().expect("queued").seq)
        );
        let index = st.queue.len() - 1;
        drop(st.remove_at(index));
        assert!(!st.holds_latest(":a"));
        st.clear_queue();
        assert_eq!((st.priority_queued, st.latest_seq(":a")), (0, None));
        assert!(st.latest_queued.is_none(), "cleared, the index is freed");
    }

    /// The dead-alias set allocates only while it holds something, and keeps its
    /// one-shot, evict-the-oldest semantics through the box.
    #[test]
    fn dead_aliases_cost_nothing_until_used_and_free_when_emptied() {
        let mut d = DeadAliases::default();
        assert!(d.is_empty() && d.0.is_none());
        assert!(!d.take(1), "nothing to take from an empty set");
        d.insert(1);
        d.insert(1);
        assert!(
            d.0.as_ref().is_some_and(|s| s.len == 1),
            "insert is idempotent"
        );
        assert!(d.take(1), "a deactivated alias swallows its reply once");
        assert!(d.0.is_none(), "the last one out frees the box");
        assert!(!d.take(1), "and only once");
        for id in 0..=DEAD_ALIAS_CAP as u64 {
            d.insert(id);
        }
        assert!(!d.take(0), "the oldest was evicted at the cap");
        for id in 1..=DEAD_ALIAS_CAP as u64 {
            assert!(d.take(id), "alias {id} kept");
        }
        assert!(d.is_empty());
        assert!(
            std::mem::size_of::<DeadAliases>() <= 8,
            "one pointer per mailbox"
        );
    }

    /// Seq ordering after a candidate is re-inserted — the invariant the receive-mark
    /// (ADR-195) binary-searches on. A scan optimistically popped a candidate, then a
    /// clause guard's consuming nested `receive` shifted the queue; re-inserting at the
    /// recorded position would leave it out of seq order (see `reinsert_at_seq`).
    #[test]
    fn a_consuming_matcher_cannot_leave_the_queue_out_of_seq_order() {
        // Every mailbox seq the mark could take must skip only *pre-mark* messages —
        // that is exactly what makes the `partition_point` in `scan_mailbox` sound.
        fn assert_mark_skips_only_older(q: &VecDeque<Envelope>) {
            for mark in 0..=8u64 {
                let lo = q.partition_point(|e| e.seq < mark);
                assert!(
                    q.iter().take(lo).all(|e| e.seq < mark),
                    "receive-mark {mark} skipped a message that could carry the pinned \
                     ref (queue seqs {:?}, skipped to {lo})",
                    q.iter().map(|e| e.seq).collect::<Vec<_>>()
                );
            }
        }

        // The exact state `tests/mailbox_order_test.blsp` reaches at runtime.
        //
        // Five queued messages, seq 0..4; the scan resumed at index 4 (a native-nested
        // receive keeps its index across a wait) and optimistically popped that candidate.
        let mb = Mailbox::new();
        let mut st = crate::core::sync::lock(&mb.state);
        for n in 0..5 {
            st.push(Envelope::plain(Message::Int(n)));
        }
        let popped = st.queue.remove(4).expect("candidate at 4");
        assert_eq!(popped.seq, 4);
        // Unlocked window: the guard's nested `receive` consumed the three oldest
        // messages, and the request it fired brought its reply in as seq 5.
        for _ in 0..3 {
            st.queue.pop_front();
        }
        st.push(Envelope::plain(Message::Int(5)));
        assert_eq!(
            st.queue.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![3, 5]
        );

        // What the clamped re-insert did — kept here so this test states the bug, not
        // only the fix. The scan index (4) is now past the end, so it clamped to 2 and
        // produced `[3, 5, 4]`; one more arrival makes it `[3, 5, 4, 6]`, on which
        // `partition_point(seq < 5)` sees `[T, F, T, F]` and returns 3, stepping over
        // the seq-5 reply at index 1. `tests/mailbox_order_test.blsp` asserts that
        // user-visible consequence; asserted here is the invariant itself, which does
        // not depend on which index a binary search over an unpartitioned predicate
        // happens to pick.
        {
            let mut clamped: VecDeque<u64> = st.queue.iter().map(|e| e.seq).collect();
            clamped.insert(4usize.min(clamped.len()), popped.seq);
            assert_eq!(clamped, vec![3, 5, 4]);
            assert!(
                clamped
                    .iter()
                    .zip(clamped.iter().skip(1))
                    .any(|(a, b)| a > b),
                "the clamped re-insert is only a bug because it breaks seq order — if \
                 this stops holding, rebuild the case, don't delete it"
            );
        }

        // The fix: re-insert at the seq-ordered position instead, and report where it
        // landed so the scan resumes from there rather than from the stale index.
        let idx = reinsert_at_seq(&mut st, 4, popped);
        st.push(Envelope::plain(Message::Int(6)));
        assert_eq!(
            st.queue.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![3, 4, 5, 6],
            "re-insert must restore the Envelope::seq ordering"
        );
        assert_eq!(
            idx, 1,
            "the scan resumes from where the candidate actually landed"
        );
        assert_mark_skips_only_older(&st.queue);
    }

    /// The unperturbed path (every scan without a mailbox-consuming matcher): the
    /// recorded index is still the right slot, it is taken without a search, and the
    /// queue is unchanged from before the pop.
    #[test]
    fn reinsert_at_the_unshifted_position_is_a_no_op() {
        let mb = Mailbox::new();
        let mut st = crate::core::sync::lock(&mb.state);
        for n in 0..4 {
            st.push(Envelope::plain(Message::Int(n)));
        }
        for i in 0..4 {
            let popped = st.queue.remove(i).expect("candidate");
            assert_eq!(reinsert_at_seq(&mut st, i, popped), i);
            assert_eq!(
                st.queue.iter().map(|e| e.seq).collect::<Vec<_>>(),
                vec![0, 1, 2, 3]
            );
        }
    }

    /// The lazy-cancellation core (Fix 1): each park-with-deadline bumps the
    /// mailbox's `timer_gen` and stamps its timer entry with the new value; the
    /// timer thread fires only the entry whose gen is still current. This unit test
    /// exercises the gen bookkeeping without standing up the timer thread or the
    /// scheduler — it models the sequence of arms and asserts which entries are
    /// considered live vs. superseded.
    #[test]
    fn timer_gen_supersedes_earlier_entries() {
        let mb = Mailbox::new();
        assert_eq!(
            mb.timer_gen.load(Ordering::Relaxed),
            0,
            "fresh mailbox at gen 0"
        );

        // Mirror `wait_for_message`'s green branch: `fetch_add(1) + 1` is the gen
        // stamped onto the entry just armed. A server looping
        // `(receive … (after ms …))` woken by `send` each iteration arms repeatedly.
        let gen1 = mb.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let gen2 = mb.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let gen3 = mb.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
        assert_eq!(
            (gen1, gen2, gen3),
            (1, 2, 3),
            "each park stamps a fresh gen"
        );

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

    /// Sticky **hard** kill (the `request_kill` hardening): once an untrappable
    /// hard kill is latched, a racing *soft* `(exit …)` must not overwrite it —
    /// otherwise the soft reason would downgrade the kill and a CPU-bound target
    /// (which honours only a hard kill, at `preempt`) could survive. A hard kill
    /// may still upgrade a pending soft reason, two soft reasons never become
    /// hard, and — the link-propagation case — a hard kill carries whatever
    /// reason it was requested with (hardness ≠ the `:kill` reason value).
    #[test]
    fn kill_is_sticky_against_a_racing_soft_exit() {
        let kill = || Message::Keyword(value::intern(pk::KILL));
        let soft = || Message::Keyword(value::intern("shutdown"));

        // hard :kill, then a soft exit → still hard (no downgrade).
        let mb = Mailbox::new();
        mb.request_kill(kill(), true);
        mb.request_kill(soft(), false);
        assert!(
            mb.pending_hard_kill(),
            "a soft exit must not downgrade a latched hard kill"
        );

        // soft, then hard :kill → upgraded to hard.
        let mb = Mailbox::new();
        mb.request_kill(soft(), false);
        mb.request_kill(kill(), true);
        assert!(
            mb.pending_hard_kill(),
            "a hard kill must upgrade a pending soft reason"
        );

        // soft, then another soft → last soft wins; never spuriously hard.
        let mb = Mailbox::new();
        mb.request_kill(soft(), false);
        mb.request_kill(Message::Keyword(value::intern("other")), false);
        assert!(
            !mb.pending_hard_kill(),
            "two soft reasons never become a hard kill"
        );

        // link propagation: hard, but the reason is the ORIGINATING one — the
        // peer's monitors report why the tree fell, not a blanket :kill.
        let mb = Mailbox::new();
        mb.request_kill(soft(), true);
        assert!(mb.pending_hard_kill());
        assert!(
            matches!(mb.pending_kill().unwrap(),
                     Message::Keyword(k) if k == value::intern("shutdown")),
            "a hard kill carries its requested reason"
        );
    }

    /// State-capture seam (ADR-100 §8): in a green process (modelled by setting the
    /// `CAPTURE_RUN` + `CAPTURE_TOP_LEVEL` flags `run_one`/`vm_run_bc` set), a
    /// `receive` that scans an empty mailbox with no match produces a `Control::Suspend`
    /// *control signal* (for the VM driver to capture and the scheduler to park) instead
    /// of blocking in `wait_for_message`. Assert `receive_match` returns that signal —
    /// not a real error, and never blocks (the test would hang if it took the wait path).
    #[test]
    fn empty_receive_in_a_capture_run_suspends() {
        let mailbox = Mailbox::new();
        // Greenness comes from `in_capture_run` (set below), not a yielder.
        let ctx = crate::process::scheduler::Ctx {
            pid: 999_999,
            mailbox: Arc::clone(&mailbox),
            capture: Vec::new(),
        };
        crate::process::scheduler::CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
        crate::process::scheduler::set_capture_run(true);
        // A *top-level* capture receive (bytecode-reachable, no native frame between it
        // and the body driver) is the shape that suspends-and-captures; a native-nested
        // one instead blocks. Mark top-level so the gate `in_capture_run &&
        // capture_top_level` fires the suspend (else this would fall through to the
        // condvar block and hang the test). Restore both flags after.
        let prev_top = crate::process::scheduler::set_capture_top_level(true);

        let mut heap = Heap::new();
        // Empty mailbox, no timeout: the scan finds nothing and the capture branch
        // returns the suspend signal. `matcher` is never applied (the queue is empty),
        // so a plain `nil` suffices — and `nil` for `pin` means no receive-mark
        // (ADR-195), which is what an unpinned receive passes.
        let r = receive_match(
            &mut heap,
            Value::nil(),
            Value::nil(),
            Value::nil(),
            Value::nil(),
        );
        crate::process::scheduler::set_capture_top_level(prev_top);
        crate::process::scheduler::set_capture_run(false);
        crate::process::scheduler::CURRENT.with(|c| *c.borrow_mut() = None); // don't leak the dummy ctx
        let err =
            r.expect_err("an empty receive in a capture run must signal a suspend, not return");
        assert!(
            err.is_control(),
            "the suspend must be a control signal, not a real error"
        );
        assert!(
            matches!(
                err.control,
                Some(crate::error::Control::Suspend { deadline: None })
            ),
            "an indefinite receive carries no deadline"
        );
    }

    /// A credited message its source took back before it was received (a key repeat queued
    /// behind a slow frame, the key since released) is dropped by the receive that reaches
    /// it — not matched, not left queued — and dropping it returns the source's credit.
    #[test]
    fn a_withdrawn_message_is_dropped_unseen_and_returns_its_credit() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        #[derive(Default)]
        struct Source {
            withdrawn: AtomicBool,
            released: AtomicUsize,
        }
        impl CreditSource for Source {
            fn release(&self, cost: usize) {
                self.released.fetch_add(cost, Ordering::SeqCst);
            }
            fn withdrawn(&self) -> bool {
                self.withdrawn.load(Ordering::SeqCst)
            }
        }

        let mailbox = Mailbox::new();
        let source = Arc::new(Source::default());
        {
            let mut envelope = Envelope::plain(Message::Keyword(value::intern("down")));
            envelope.credit = Some(Box::new(DeliveryCredit::new(source.clone(), 1)));
            crate::core::sync::lock(&mailbox.state).push(envelope);
        }
        source.withdrawn.store(true, Ordering::SeqCst);

        let ctx = crate::process::scheduler::Ctx {
            pid: 999_998,
            mailbox: Arc::clone(&mailbox),
            capture: Vec::new(),
        };
        crate::process::scheduler::CURRENT.with(|c| *c.borrow_mut() = Some(ctx));
        let mut heap = Heap::new();
        // `(receive (after 0 …))` over it: the matcher (nil here) is never applied, since
        // the only message is dropped before a clause sees it, so the receive times out.
        let answer = receive_match(
            &mut heap,
            Value::nil(),
            Value::int(0),
            Value::nil(),
            Value::nil(),
        );
        crate::process::scheduler::CURRENT.with(|c| *c.borrow_mut() = None);

        assert!(
            matches!(answer, Ok(Value::Nil)),
            "nothing is left to receive, so the receive times out"
        );
        assert!(
            crate::core::sync::lock(&mailbox.state).queue.is_empty(),
            "the withdrawn message is gone from the queue"
        );
        assert_eq!(
            source.released.load(Ordering::SeqCst),
            1,
            "dropping it returned its credit"
        );
    }
}
