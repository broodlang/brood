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
//! - a clean top-level green `receive` on an empty mailbox **captures** its
//!   continuation (returning a suspend signal for the scheduler to park);
//!   [`wait_for_message`] **blocks** the caller on the condvar for the cases
//!   that can't capture — the root thread, a tree-walked body, and a
//!   native-nested receive (the §7.4 dirty carve-out).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::Duration;
use web_time::Instant;

use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, MapId, Symbol, Value};
use crate::error::{LispError, LispResult};
use crate::eval;
use crate::process::keywords as pk;

use super::message::{from_message, to_message_to_runtime, Message};
// Only the `dev-tools` trace path converts a whole trace to a message; importing it
// unconditionally makes a `--no-default-features` build warn.
#[cfg(feature = "dev-tools")]
use super::message::to_message;
use super::scheduler::{ensure_ctx, wake_enqueue, Ctx, Process};
use super::timer::arm_timer;

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
/// report_parked_liveness`]) skip its O(all-processes) `REGISTRY` scan entirely when
/// nothing is parked — the common case during a `spawn` fan-out, where workers
/// compute-and-exit without ever parking. Without this gate that whole-registry scan
/// (under the global `REGISTRY` lock, on every throttled drain-advance of every one of
/// thousands of live workers) serialized into an O(processes²) lock storm that made a
/// fan-out under a lingering drain regress ~300×. Relaxed: it only *gates* an
/// optimization — an over-count merely runs a scan that finds nothing (correct, slower),
/// and every genuine park increments it before the parked process can matter, so it never
/// under-counts a process that needs inspecting.
static PARKED: AtomicUsize = AtomicUsize::new(0);

/// Currently-parked process count — see [`PARKED`]. O(1) relaxed load.
pub(super) fn parked_count() -> usize {
    PARKED.load(Ordering::Relaxed)
}

/// Set a mailbox's run-status, keeping the global [`PARKED`] count in step. Every
/// status transition funnels through here so a `ST_WAITING`-boundary crossing (in
/// either direction) adjusts the counter exactly once. The `swap` reads the true prior
/// status even under a race, so concurrent transitions can't double-count.
pub(super) fn set_status(mb: &Mailbox, new: u8) {
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
    /// The owning process's cumulative GC-collection count (`Heap::gc_counters().0`),
    /// republished alongside `mem` when it parks in `receive`. Lets the observer flag a
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
    /// firing a spurious wakeup when its long-past deadline finally comes due.
    ///
    /// Reaping a stale entry only when its deadline comes due is not, on its own, a
    /// useful bound: it bounds each entry's *lifetime* by the deadline horizon, so the
    /// heap's size is bounded by **arm-rate × horizon**. That is the message rate — a
    /// server looping `(receive … (after 3600000 …))` at 10k msg/s reaches ~36M dead
    /// entries before the first comes due. So `super::timer::arm_timer` also *compacts*
    /// the heap against this counter (see there); this gen is what makes both the
    /// firing-time drop and the compaction able to tell a superseded entry from a live
    /// one. The "spurious wakeups are harmless" re-validation in `receive_match` stays
    /// as the backstop.
    pub(super) timer_gen: AtomicU64,
    /// The pid that spawned this process (`0` = none, i.e. the root). Written once before
    /// the mailbox is published to the registry and never mutated after, so `Relaxed` is
    /// enough — the registry insert is the release that publishes it.
    pub(super) parent: AtomicU64,
}

/// A queued message plus, under `dev-tools`, the debugger causal context (ADR-174
/// send-level) the sender carried. Access to the payload is always `.msg` — uniform
/// across builds, so the receive matcher is untouched — and in a lean release this is
/// a zero-cost newtype over `Message` (the `trace` field does not exist).
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
}
impl MailboxState {
    /// Push `env` onto the queue, stamping its arrival sequence and republishing the
    /// lock-free hint. Every enqueue goes through here so `seq` is never left at 0.
    #[inline]
    pub(super) fn push(&mut self, mut env: Envelope) {
        env.seq = self.next_seq;
        self.next_seq += 1;
        self.queue.push_back(env);
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
    pub(super) kill: Option<Message>,
    /// Is the pending `kill` **hard** (untrappable: die at the next reduction tick,
    /// not the next `receive`)? Hardness is a property of the *request*, separate
    /// from the reason value: `(exit pid :kill)` is hard with reason `:kill`, and
    /// **link propagation is hard with the originating reason** — so a cascading
    /// death reports *why* the tree fell (BEAM propagates the reason), not a
    /// blanket `:kill`. Meaningful only while `kill` is `Some`.
    pub(super) kill_hard: bool,
    /// The parked green process waiting on this mailbox, if any. `send` takes it
    /// and re-queues it. (A short-lived `Process → Arc<Mailbox> → Process` cycle
    /// while parked; broken the moment it's re-queued or the process ends.)
    ///
    /// A process parked on a `(receive)` nothing will ever send to (no deadline)
    /// holds its `Box<Process>` here for the life of the `REGISTRY` entry — which
    /// is fine in the standalone binaries (the OS process exits) and no longer
    /// leaks in an embedded host: `Interp::drop` runs
    /// [`super::scheduler::shutdown_runtime_parked`], which takes each such
    /// waiter belonging to the dropped runtime and routes it through the normal
    /// death path (fixed 2026-07-23; it was a flagged leak).
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
    /// `wait_for_message` write, or this read goes stale. (The capture-mode suspend in
    /// `receive_match` also writes it before returning the suspend signal, so the
    /// invariant holds there too.)
    pub(super) scanned: usize,
    /// The absolute deadline of the `receive` currently in progress, or `None`. A
    /// captured `receive` is re-entered from scratch on each wake (the continuation
    /// replays the `%receive` call), so recomputing `now + ms` every resume would
    /// reset — and never fire — the `after` timeout. The first entry stores the
    /// absolute deadline here; resumes reuse it; the matching/timeout exit clears it.
    /// Single slot: one `receive` runs at a time per process.
    pub(super) recv_deadline: Option<Instant>,
    /// **Deactivated one-shot reply aliases** — see [`DeadAliases`]. A message
    /// addressed to one of these refs is dropped *at delivery*, before it is ever
    /// queued. Empty for the overwhelming majority of processes, which is what makes
    /// the delivery check free (`is_empty` on a line the deliverer already holds).
    pub(super) dead_aliases: DeadAliases,
}

/// How many deactivated aliases one mailbox remembers. Fixed and small — the whole
/// set is 8 × `u64` stored inline in [`MailboxState`], so this costs no allocation
/// and cannot grow.
///
/// **Why a bound at all, and why this one.** An entry only has to outlive the *one*
/// in-flight reply the alias was minted for, so the common case reclaims itself: the
/// late reply arrives, is dropped, and [`DeadAliases::take`] forgets the entry in the
/// same step. The capacity only matters for a caller that times out repeatedly
/// against a server that never answers at all — nothing ever arrives to clear those
/// entries. Eight covers any realistic nesting of in-flight calls (a process has one
/// `gen/call` outstanding per stack frame, and a *reply* is what a `call` blocks for),
/// and the failure mode past it is a graceful one: evicting the oldest entry restores
/// exactly the pre-alias behaviour for that one long-abandoned ref (its late reply, if
/// one ever comes, queues as it always did). Never a wrong drop, never unbounded.
const DEAD_ALIAS_CAP: usize = 8;

/// The per-mailbox set of deactivated one-shot reply aliases (OTP 24's process
/// aliases, `{alias, demonitor}`) — ADR-244.
///
/// Erlang through OTP 23 answered the stale-reply problem by having the caller *exit*
/// on a `gen_server:call` timeout, so a late reply was moot. OTP 24 added aliases: the
/// reply is addressed to a one-shot alias rather than the pid, and once the alias is
/// deactivated at the deadline the VM drops later replies addressed to it — they never
/// enter the mailbox. This is that, minus the separate alias identity: a Brood `ref` is
/// already the unforgeable request token a reply carries, so deactivating *the ref*
/// (`%ref-deactivate`) is the same guarantee with no new `Value` kind.
///
/// A tiny insertion-ordered array rather than a set: it is capped at
/// [`DEAD_ALIAS_CAP`], so a linear scan is a handful of `u64` compares on one cache
/// line — cheaper than hashing, and it never allocates on the delivery path.
#[derive(Default)]
pub(super) struct DeadAliases {
    ids: [u64; DEAD_ALIAS_CAP],
    len: usize,
}

impl DeadAliases {
    /// Nothing deactivated — the case every delivery to every ordinary process takes.
    #[inline(always)]
    fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Deactivate `id`. Idempotent; evicts the oldest entry when full (see
    /// [`DEAD_ALIAS_CAP`] for why that is the right way to fail).
    fn insert(&mut self, id: u64) {
        if self.ids[..self.len].contains(&id) {
            return;
        }
        if self.len == DEAD_ALIAS_CAP {
            self.ids.copy_within(1..DEAD_ALIAS_CAP, 0);
            self.len -= 1;
        }
        self.ids[self.len] = id;
        self.len += 1;
    }

    /// Is `id` deactivated? If so, **forget it** — an alias is one-shot, so the entry
    /// has done its job the moment the reply it was waiting to swallow arrives. This
    /// is what keeps the set at zero in steady state rather than at its cap.
    fn take(&mut self, id: u64) -> bool {
        let Some(i) = self.ids[..self.len].iter().position(|&x| x == id) else {
            return false;
        };
        self.ids.copy_within(i + 1..self.len, i);
        self.len -= 1;
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
    let mb = ensure_ctx().mailbox;
    crate::core::sync::lock(&mb.state)
        .dead_aliases
        .insert(ref_id);
}

impl Mailbox {
    /// A mailbox for a process whose spawner is `parent` (`0` for the root, which has
    /// none). The parent pid lives here rather than in a global `pid -> pid` map: it is
    /// per-process data, the per-process record already exists and is already
    /// registry-reachable, and the map cost two locked operations on every spawn and every
    /// exit — pure contention under fan-out, for a value read in exactly one place
    /// (`process-info`'s `:parent`).
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

    pub(super) fn new_with_parent(parent: u64) -> Arc<Mailbox> {
        let mb = Mailbox::new();
        // Safe: the mailbox is not published to the registry until after this returns, so
        // no other thread can observe the write.
        mb.parent.store(parent, Ordering::Relaxed);
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
            parent: AtomicU64::new(0),
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
            // reason)` must not overwrite it (Erlang's guarantee that `exit(pid,
            // kill)` can't be undone — otherwise a racing soft exit could downgrade
            // the kill and spare a CPU-bound target, which only honours a hard kill
            // at `preempt`). A fresh hard kill may still upgrade a pending soft
            // reason; the first hard reason wins thereafter.
            let latched_hard = st.kill.is_some() && st.kill_hard;
            if !latched_hard {
                st.kill = Some(reason);
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
        crate::core::sync::lock(&self.state).kill.clone()
    }

    /// Is an untrappable **hard** kill pending? The loop-top safepoint probe: a
    /// soft exit isn't honoured there (it waits for the next `receive`). Same
    /// fast path as [`pending_kill`](Self::pending_kill).
    pub(super) fn pending_hard_kill(&self) -> bool {
        if !self.kill_pending.load(Ordering::Relaxed) {
            return false;
        }
        let st = crate::core::sync::lock(&self.state);
        st.kill.is_some() && st.kill_hard
    }
}

/// pid → mailbox, for `send` to find a target from any thread.
/// `pub(super)` so the `monitor` submodule can take the REGISTRY ↔ MONITORS
/// liveness check inside its critical section (see `monitor::add_monitor`).
/// Number of registry shards. A power of two so the shard index is a mask, and comfortably
/// above the worker count so a fan-out rarely collides.
const REGISTRY_SHARDS: usize = 64;

/// `pid -> Arc<Mailbox>`, sharded by pid.
///
/// This was a single global `Mutex<HashMap<..>>`, which every `spawn`, every `deregister`
/// and every `deliver` (i.e. every message) had to take. Uncontended that is ~20 ns and
/// invisible — `ring`/`pingpong` measured flat across worker counts, so message delivery
/// was never the problem. **Fan-out is**: one process spawning while N workers deregister
/// exited children serialises every one of them on the same mutex, and pure spawn measured
/// 19 ms at 12 workers against 13 ms at 4 — the extra workers made it *slower*.
///
/// Sharding keeps the semantics exactly: a pid still resolves to a mailbox from any thread
/// (so location-transparent `send` and the whole `dist` path are unchanged), and it is
/// invisible to hot reload, which concerns the shared code region and globals table, never
/// mailboxes.
///
/// **Lock ordering is preserved.** `add_monitor` takes MONITORS then a registry shard
/// (nested); `deregister` takes a shard, releases, then NAMES, then MONITORS (sequential).
/// A shard is therefore always the *inner* lock, exactly as the single registry was — and
/// the two race against each other on the *same pid*, hence the same shard, so their
/// check-and-modify pairing resolves as before. Whole-registry walks (`pids`, `len`) take
/// one shard at a time and never hold two at once.
pub(super) struct Registry {
    shards: Vec<Mutex<HashMap<u64, Arc<Mailbox>>>>,
}

impl Registry {
    fn shard(&self, pid: u64) -> &Mutex<HashMap<u64, Arc<Mailbox>>> {
        &self.shards[(pid as usize) & (REGISTRY_SHARDS - 1)]
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

pub(super) static REGISTRY: LazyLock<Registry> = LazyLock::new(|| Registry {
    shards: (0..REGISTRY_SHARDS)
        .map(|_| Mutex::new(HashMap::new()))
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
    deliver_envelope(pid, Envelope::plain(msg));
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
        },
    );
}

/// **The L1 local-send fast path.** Try to deliver `v` to local process `pid` by copying
/// it straight from the sender's heap into the receiver's, skipping the `Value → Message
/// → Value` round trip. Returns `true` when it did.
///
/// Only fires when the receiver is **parked**: taking its `waiter` out of the mailbox
/// state under the lock gives us its `Box<Process>`, and therefore exclusive `&mut` on
/// its heap for the duration — the same quiescence `trim_parked` relies on. A running
/// receiver, a remote pid, or a value the copier declines (a closure, a rope, an
/// unrealised seq-view) all answer `false` and take the normal `Message` path with its
/// existing semantics and error messages.
///
/// The copied value is parked in the receiver's `msg_roots` — a *traced* slot table —
/// because the mailbox is not a GC root and `roots` is the operand stack.
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
        *F.get_or_init(|| std::env::var_os("BROOD_L1_STATS").is_some())
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

/// Outcome of the L1 attempt. When it declines, it hands back the target's runtime tag so the
/// caller does not have to look the registry up a *second* time to decide whether a shared
/// closure handle may cross (ADR-208). That duplicate lookup sat on the serialised send path —
/// precisely the path a busy receiver takes, and the one ADR-208 made hot.
enum LocalDelivery {
    /// Delivered into the parked receiver's heap; nothing further to do.
    Delivered,
    /// Not delivered. `runtime_tag` is the target's, or `None` if there is no such process.
    Declined { runtime_tag: Option<u64> },
}

fn try_deliver_local(src: &Heap, pid: u64, v: Value) -> LocalDelivery {
    let Some(mb) = REGISTRY.get(pid) else {
        l1_stats::bump(&l1_stats::NO_PROC);
        return LocalDelivery::Declined { runtime_tag: None };
    };
    let tag = mb.runtime_tag();
    let mut st = crate::core::sync::lock(&mb.state);
    // One-shot reply alias (OTP 24): the receiver deactivated this ref, so the reply is
    // dropped here — before the cross-heap copy, before the queue, before any wake.
    // `is_empty` is the whole cost when nothing is deactivated, which is every ordinary
    // process; we already hold the lock, so it is a field read on a hot line.
    if !st.dead_aliases.is_empty() {
        if let Some(id) = value_alias(src, v) {
            if st.dead_aliases.take(id) {
                return LocalDelivery::Delivered; // silently swallowed, like a send to a dead pid
            }
        }
    }
    // Not parked → we have no safe access to its heap.
    let Some(mut proc) = st.waiter.take() else {
        l1_stats::bump(&l1_stats::NO_WAITER);
        return LocalDelivery::Declined {
            runtime_tag: Some(tag),
        };
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
        return LocalDelivery::Declined {
            runtime_tag: Some(tag),
        };
    };
    let tag = if no_msg_tag() {
        None
    } else {
        leading_keyword(proc.heap_mut(), copied)
    };
    let slot = proc.heap_mut().msg_root_add(copied);
    st.push(Envelope {
        msg: Payload::Local { slot, tag },
        seq: 0,
        #[cfg(feature = "dev-tools")]
        trace: None,
    });
    drop(st);
    // Both paths, for the reason `wake_both` documents: taking a `waiter` here does not prove
    // the receiver is *only* reachable by re-queue. This path never notified the condvar at
    // all, so a receiver that was both parked and cv-blocked was unreachable through it.
    wake_both(&mb, Some(proc));
    l1_stats::bump(&l1_stats::HIT);
    LocalDelivery::Delivered
}

fn deliver_envelope(pid: u64, env: Envelope) {
    crate::perf_time!(ns_deliver, { deliver_envelope_timed(pid, env) })
}

fn deliver_envelope_timed(pid: u64, env: Envelope) {
    let mailbox = REGISTRY.get(pid);
    if let Some(mb) = mailbox {
        let mut st = crate::core::sync::lock(&mb.state);
        // One-shot reply alias (OTP 24) — the wire-path twin of the check in
        // `try_deliver_local`. Dropped here, so it never occupies a queue slot and no
        // later selective receive ever re-scans it. See `DeadAliases`.
        if !st.dead_aliases.is_empty() {
            if let Payload::Wire(msg) = &env.msg {
                if let Some(id) = message_alias(msg) {
                    if st.dead_aliases.take(id) {
                        return;
                    }
                }
            }
        }
        st.push(env);
        let parked = wake_parked(&mut st);
        drop(st);
        wake_both(&mb, parked);
    }
}

/// Signal **both** wake paths for a mailbox we have just pushed to (or set a kill on):
/// re-queue the parked green process if there was one, *and* notify the condvar.
///
/// The `if parked { requeue } else { notify }` this replaces looks exhaustive and is not.
/// It assumes a receiver is reachable by exactly one path, but the two are not mutually
/// exclusive: a green process that entered a **native-nested** `receive` (a `receive` inside
/// a HOF running in a Rust builtin — the §7.4 dirty-scheduler carve-out) blocks its worker on
/// this condvar *without* clearing any `waiter` a previous park left behind. Whenever a
/// `waiter` is present the `else` never runs, so that receiver's condvar is never notified
/// and — with no `after` to self-wake it — it blocks forever. A condvar notify does **not
/// latch**: delivered to no one it is discarded, and nothing later recovers it.
///
/// BEAM has no such either/or, because it has no second wake path: `erts_queue_message`
/// enqueues and then `erts_proc_notify_new_message` schedules the process if it is not
/// already `ERTS_PSFLG_ACTIVE` — a **persistent state bit**, so however the interleaving
/// falls, a process made active gets run. We keep two mechanisms (a root/file-runner thread
/// really does block an OS thread), so the invariant has to be: signal both, every time.
///
/// Notifying with no waiter is a no-op, and a spurious wake costs one re-scan — `receive_match`
/// re-scans after every `wait_for_message` return anyway. Called with the state lock already
/// released: we pushed under it, so a receiver that locks after us sees the message on its
/// own `queue.len() > i` check and never reaches the wait.
pub(super) fn wake_both(mb: &Mailbox, parked: Option<Box<Process>>) {
    mb.cv.notify_all();
    if let Some(proc) = parked {
        wake_enqueue(proc); // wake a parked green process (capture-mode → may migrate)
    }
}

/// `(send target msg)` — copy `msg` into `target`'s mailbox and wake it. `target`
/// is a **pid** (local or remote — it carries node identity) or a `{:name :node}`
/// **registered-name address** for bootstrapping a peer before you hold its pid.
/// Routing is location-transparent: a local target delivers in-process; a remote
/// one is forwarded over the node link (`crate::dist`). Sending to a dead/unknown
/// target is a silent no-op (Erlang semantics) — with one opt-in exception: a
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
    // Reused below as the shared-handle destination (ADR-208) when L1 declines, so the
    // registry is consulted once for both purposes.
    let mut dest_runtime: Option<u64> = None;
    if !has_trace {
        if let Value::Pid { node, id } = target_val {
            if crate::dist::is_local(node) {
                match try_deliver_local(heap, id, msg_val) {
                    LocalDelivery::Delivered => return Ok(()),
                    LocalDelivery::Declined { runtime_tag } => dest_runtime = runtime_tag,
                }
            }
        }
    }
    // `dest_runtime` was filled in by the declining L1 attempt above (one registry lookup for
    // both jobs). It stays `None` for every other destination — a remote node, a name address,
    // an unknown pid, or a traced send — which serialises exactly as it did before ADR-208.
    let msg = to_message_to_runtime(heap, msg_val, dest_runtime)?;
    // (dev-tools) Send-level causality (ADR-174): if the sender carries a debugger
    // trace context and the target is a LOCAL pid, ship the context alongside so the
    // receiver adopts it on pop. Context is per-runtime, so it never crosses nodes;
    // remote / name-addressed / no-context sends fall through to the normal route.
    #[cfg(feature = "dev-tools")]
    if let (Value::Pid { node, id }, Some(ctx)) = (target_val, heap.trace_context()) {
        if crate::dist::is_local(node) {
            let trace = to_message(heap, ctx)?;
            deliver_traced(id, msg, Some(trace));
            return Ok(());
        }
    }
    let (routed, node) = match target_val {
        Value::Pid { node, id } => (
            crate::dist::route(node, crate::dist::Target::Pid(id), msg),
            node,
        ),
        Value::Map(mid) => {
            let (name, node) = read_name_address(heap, mid)?;
            (
                crate::dist::route(node, crate::dist::Target::Name(name), msg),
                node,
            )
        }
        _ => {
            return Err(LispError::type_err(
                "send: target must be a pid or a {:name :node} address",
            ))
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

/// `(%receive matcher timeout)` — selective receive. `matcher` is a unary function:
/// given a message value it answers **which clause matched and what that clause's
/// pattern bound**, as a `[idx var…]` vector, or `nil` on no match. Scan the mailbox in
/// order; the first message a clause matches is removed and the matcher's answer
/// returned. The clause **body does not run here** — the `receive` macro emits every
/// body at the call site and dispatches on `idx` there (`std/prelude.blsp`), so bodies
/// compile into the owning arm and a loop that tail-calls back into `receive` stays
/// O(1) native stack. Non-matching messages stay queued (Erlang selective receive).
///
/// `timeout` is `nil` (wait forever) or an integer of milliseconds; on expiry this
/// returns **`nil`**, which the macro's `after` branch tests for. That is unambiguous:
/// a match always answers with a vector, and with a `nil` timeout expiry is unreachable.
///
/// A green process suspends while waiting; the root thread blocks.
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

/// Can `msg` be rejected without rebuilding it into the heap? True only when the filter
/// is active AND the message is a vector whose head is a keyword that no clause names.
/// Every other shape answers `false` (scan it properly) — the filter must never skip a
/// message a clause could match.
/// Whether the receive-mark is armed (ADR-195). `BROOD_NO_RECV_MARK=1` disables it, so every
/// receive scans from the front as it did before — the A/B lever, the bisect lever, and the
/// stopgap if a skipped message is ever suspected.
///
/// This one earns an off-switch more than most changes do: a wrong skip does not crash, it
/// silently fails to deliver a message, which is the hardest class of fault to attribute
/// after the fact.
fn recv_mark_enabled() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("BROOD_NO_RECV_MARK").is_none())
}

#[inline]
/// `BROOD_NO_MSGTAG=1` — deliver L1 fast-path messages without their leading-keyword tag,
/// so the selective-receive prefilter can't use them. The A/B lever for what the tag carry
/// is worth; off-switch only.
fn no_msg_tag() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("BROOD_NO_MSGTAG").is_some())
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
    let ctx = ensure_ctx();
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
                Some(*st.recv_deadline.get_or_insert_with(|| {
                    super::timer::sched_now() + Duration::from_millis(ms as u64)
                }))
            } else {
                // Coroutine/root: the `receive_match` loop holds the deadline across its
                // waits (it never exits between scans), so compute it once here.
                Some(super::timer::sched_now() + Duration::from_millis(ms as u64))
            }
        }
        _ => {
            return Err(LispError::type_err(
                "receive: timeout must be an integer (milliseconds) or nil",
            ))
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
    let mut i = 0usize;
    loop {
        // Scan the queued messages for a ready clause (advancing `i` past
        // non-matches). The wait, below, is the only blocking step — this split is
        // the seam the coming state-capture path uses: there, a `None` becomes a
        // *suspend signal* returned to the scheduler instead of a `wait_for_message`.
        match scan_mailbox(heap, &ctx, rbase, &mut i, mark) {
            Ok(Some(matched)) => {
                // The receive completed (a clause matched) — clear any persisted
                // capture-mode deadline so the next receive starts fresh. A nil-timeout
                // receive (`deadline` None) never persisted one, so it skips the lock —
                // the previous receive's exit already cleared the slot.
                if capture && deadline.is_some() {
                    crate::core::sync::lock(&ctx.mailbox.state).recv_deadline = None;
                }
                heap.truncate_roots(rbase);
                return Ok(matched);
            }
            Ok(None) => {
                // Scanned every queued message with no match.
                if let Some(d) = deadline {
                    if super::timer::sched_now() >= d {
                        // The receive completed via timeout — answer `nil` (the macro's
                        // `after` branch tests for it) and clear the persisted
                        // capture-mode deadline.
                        if capture {
                            crate::core::sync::lock(&ctx.mailbox.state).recv_deadline = None;
                        }
                        heap.truncate_roots(rbase);
                        return Ok(Value::Nil);
                    }
                }
                // About to park (no clause matched, and no timeout has fired): republish
                // this process's LOCAL footprint + GC count so an observer's `process-info`
                // reads a current `:memory`/GC figure for the *waiting* process — the state
                // it's most likely inspected in. Kept off the fast (message-ready) path above.
                ctx.mailbox.mem.store(heap.local_bytes(), Ordering::Relaxed);
                ctx.mailbox
                    .gc_runs
                    .store(heap.gc_counters().0, Ordering::Relaxed);
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
                    {
                        // Clamp: a matcher that ran a *consuming* nested `receive` can
                        // leave the queue shorter than the scan index, and `scanned` past
                        // the length means `park_on_receive`'s `queue.len() > scanned`
                        // gate needs several more messages before it will re-run us — a
                        // process that sleeps through the very message it is waiting for.
                        let mut st = crate::core::sync::lock(&ctx.mailbox.state);
                        st.scanned = i.min(st.queue.len());
                    }
                    set_self_status(&ctx, ST_WAITING);
                    if let Some(d) = deadline {
                        let gen = ctx.mailbox.timer_gen.fetch_add(1, Ordering::Relaxed) + 1;
                        arm_timer(ctx.pid, d, gen);
                    }
                    heap.truncate_roots(rbase);
                    return Err(LispError::suspend(deadline));
                }
                wait_for_message(&ctx, i, deadline);
                // Back from the wait → running again. This *block* path (the root thread,
                // and a native-nested capture receive — §7.4) returns inline without a
                // `run_one`, so its `ST_WAITING` would otherwise leak into the next scan
                // (and a `process-info` after the receive would read `:waiting` while
                // running). The capture path that suspends above is flipped back by
                // `run_one` on resume instead.
                set_self_status(&ctx, ST_RUNNING);
                // An `(exit …)` may have woken us (its `cv.notify_one`) rather than a
                // message. There's no coroutine to suspend and no `run_one` boundary to
                // check `kill_pending` for a blocked receive, so do it here — but only in
                // a real scheduler-run green process (`in_capture_run`), where a top-level
                // body driver exists to convert the unwinding `Control::Kill` into process
                // death. On the root / file-runner thread there's no such driver, so a
                // `Control::Kill` would just leak as an error; that thread isn't a killable
                // process anyway, so leave it to re-scan/re-block (the pre-existing
                // behaviour). Unwind with `Control::Kill` — untrappable through the
                // enclosing `try`/`%isolate`/HOF — which `vm_run_bc` turns into
                // `VmOutcome::Killed`, retiring the process with the reason still in
                // `state.kill`. (Both a hard `:kill` and a soft `(exit pid reason)` die
                // here; the reason distinguishes them at death.)
                if crate::process::in_capture_run()
                    && ctx.mailbox.kill_pending.load(Ordering::Relaxed)
                {
                    heap.truncate_roots(rbase);
                    return Err(LispError::kill_signal());
                }
            }
            Err(e) => {
                // A control signal (suspend) keeps the in-progress receive's persisted
                // deadline; a real error (e.g. the matcher threw) ends this receive, so
                // clear it lest a later receive in this process reuse a stale deadline.
                // (Nothing to clear when this receive had no deadline — see the match arm.)
                if capture && deadline.is_some() && !e.is_control() {
                    crate::core::sync::lock(&ctx.mailbox.state).recv_deadline = None;
                }
                heap.truncate_roots(rbase);
                return Err(e);
            }
        }
    }
}

/// Scan the mailbox from index `*i` for the first message a clause matches, advancing
/// `*i` past non-matching messages (Erlang selective receive — non-matches stay
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
    ctx: &Ctx,
    rbase: usize,
    i: &mut usize,
    // The receive-mark, if this receive pinned a ref we minted (ADR-195): no message with
    // `seq` below it can match, so the scan may start past them.
    mark: Option<u64>,
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
    // copy into the receiver's heap. Sound because only the owner removes from its
    // own mailbox (`send` only appends), so position `*i` is stable across the
    // unlocked window. A non-match pays one extra lock to re-insert the message at
    // `*i` (arrival order preserved) and reverts the rest of the scan to
    // peek-in-place, so a long selective-receive backlog isn't popped/re-inserted
    // per candidate — the scan's lock count stays ≤ the peek-only scheme's for
    // every backlog length.
    //
    // Since ADR-245 that is true for SMALL candidates only. Peeking in place means
    // rebuilding under the lock, so a large candidate takes this same pop/re-insert
    // route regardless (see the `message_fits` check below): one extra lock acquire,
    // in exchange for a bounded hold. The leading-keyword filter is what keeps that
    // from applying to backlog length — a message no clause could match is rejected
    // on its tag and never rebuilt at all.
    let mut optimistic = true;
    // Lazily-decoded clause tags (see the filter below). `None` = not decoded yet.
    let mut tagbuf = [0u32; MAX_RECEIVE_TAGS];
    let mut ntags: Option<usize> = None;
    loop {
        // (A hard `:kill` is caught at the VM driver's loop top; a soft exit at the
        // `park_on_receive` boundary — not here, since there's no coroutine to suspend.)
        // Rebuild candidate `*i` into the heap (no eval here → no collection).
        let (popped, v) = {
            let mut st = crate::core::sync::lock(&ctx.mailbox.state);
            if *i >= st.queue.len() {
                return Ok(None); // scanned to the end with no match
            }
            // Receive-mark: skip straight past every message that predates the pinned ref.
            // The queue is ordered by `seq`, so this is a binary search, and it runs only
            // while `*i` is still pointing at an older message (i.e. once per receive).
            if let Some(m) = mark {
                if st.queue[*i].seq < m {
                    let lo = st.queue.partition_point(|e| e.seq < m);
                    if lo > *i {
                        *i = lo;
                        optimistic = false; // the head is no longer the candidate
                    }
                    if *i >= st.queue.len() {
                        return Ok(None);
                    }
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
                    return Ok(None); // scanned to the end with no match
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
            if optimistic {
                let m = st.queue.remove(*i).expect("bounds checked above");
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
                (Some(m), v)
            } else {
                // Peek-in-place: a Local payload must NOT be taken here (the candidate
                // may not match and has to stay queued), so read the slot without
                // clearing it.
                //
                // But `from_message` on a Wire payload is a full rebuild into this heap,
                // and peeking means doing it WITH THE LOCK HELD, once per candidate —
                // the receive-side twin of the send-side stall ADR-245 bounds. A Local
                // payload is free here (`msg_root_peek` is a slot read, no rebuild), so
                // only Wire is at issue, and only when it is big: ask first, and for
                // anything past the budget take the optimistic branch's route instead —
                // pop, rebuild unlocked, and let the non-match path re-insert it in seq
                // order. That costs one extra lock acquire for that candidate and bounds
                // the hold; the leading-keyword filter above means it applies only to
                // candidates that could match, never to backlog length.
                let over = match &st.queue[*i].msg {
                    Payload::Wire(w) => !crate::process::message::message_fits(
                        w,
                        crate::process::message::l1_copy_budget(),
                    ),
                    Payload::Local { .. } => false,
                };
                if over {
                    let m = st.queue.remove(*i).expect("bounds checked above");
                    drop(st);
                    let v = match &m.msg {
                        Payload::Wire(w) => from_message(heap, w),
                        Payload::Local { slot, .. } => heap.msg_root_peek(*slot),
                    };
                    // `Some(m)` routes it through the same re-insert path the optimistic
                    // branch uses on a non-match, so nothing new has to be maintained.
                    (Some(m), v)
                } else {
                    let v = match &st.queue[*i].msg {
                        Payload::Wire(w) => from_message(heap, w),
                        Payload::Local { slot, .. } => heap.msg_root_peek(*slot),
                    };
                    (None, v)
                }
            }
        };
        let matcher = heap.root_at(rbase);
        if hof.is_none() {
            hof = Some(crate::perf_time!(ns_match_resolve, {
                crate::eval::compile::hof_resolve(heap, matcher, 1)
            }));
        }
        // Apply the matcher via the VM / JIT fast-frame when it resolved to a plain
        // arm, falling back to the tree-walking `eval::apply` otherwise (or when
        // `hof_apply_step` deopts on an identity miss — e.g. a mid-scan GC relocated
        // the matcher). Same semantics either way: returns the clause body thunk on
        // a match, a non-`Fn` value on no-match.
        let applied = crate::perf_time!(ns_match_run, {
            match hof.as_ref().unwrap() {
                Some(h) => match crate::eval::compile::hof_apply_step(heap, h, matcher, &[v]) {
                    Some(r) => r,
                    None => eval::apply(heap, matcher, &[v], EnvId::GLOBAL),
                },
                None => eval::apply(heap, matcher, &[v], EnvId::GLOBAL),
            }
        });
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
            let consumed = match popped {
                Some(env) => Some(env),
                None => crate::core::sync::lock(&ctx.mailbox.state).queue.remove(*i),
            };
            if let Some(env) = consumed.as_ref() {
                if let Payload::Local { slot, .. } = &env.msg {
                    heap.msg_root_take(*slot);
                }
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
        *i += 1; // no clause matched — leave it queued, try the next message
    }
}

/// Put an optimistically-popped candidate back **in seq order**, returning the index it
/// landed at (the caller resumes its scan from there).
///
/// The obvious thing — insert at the recorded scan position `i` — is wrong, and used to
/// be what this did (clamped to the queue length). The matcher runs with the mailbox lock
/// **released**, and a matcher can run arbitrary code: a `receive` clause's `:when` guard
/// is evaluated during the scan (`std/prelude/process.blsp`), and a guard that itself runs
/// a *consuming* nested `receive` removes messages from this very queue during that
/// window. If it consumes entries *ahead* of `i`, everything after them shifts down and
/// position `i` no longer names this envelope's slot — it now names a message with a
/// **higher** `seq`. Re-inserting there leaves the queue out of order, e.g. `[10, 12, 5,
/// 13]`, and a later `receive` pinned on a fresh `ref` then runs `partition_point` over a
/// predicate that is not partitioned (`[T, F, T, F]`), which is free to return an index
/// past a matchable post-mark message. Nothing crashes: the reply is silently never seen
/// and the caller times out — the exact failure mode [`recv_mark_enabled`]'s comment calls
/// the hardest to attribute.
///
/// So restore the [`Envelope::seq`] invariant instead. The recorded position is checked
/// first (O(1)) because it is right whenever nothing shifted, which is every scan that
/// doesn't have a mailbox-consuming guard; only the perturbed case pays the search.
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
    st.queue.insert(idx, m);
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
fn reinsert_candidate(ctx: &Ctx, i: usize, m: Envelope) -> usize {
    let mut st = crate::core::sync::lock(&ctx.mailbox.state);
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
/// **Clock domain.** The deadline was minted on the scheduler clock
/// (`timer::sched_now()`, see `receive_match_timed`), so the elapsed check below reads
/// that same clock — never `Instant::now()`. Off wasm the two are literally the same
/// function, so this costs nothing; what it buys is that the domain-mixing shape which
/// spun the wasm pump forever at `park_on_receive` is now absent from *every* gate,
/// rather than from all-but-one. Guarded by `crates/lisp/tests/sched_clock_domain.rs`.
///
/// **This function does not work on wasm32 at all, for a reason unrelated to the
/// clock** — and that is a live defect, not a theoretical one. `std`'s `no_threads`
/// `Condvar` *panics* in `wait`/`wait_timeout`, and a Rust panic on wasm is an
/// uncatchable `RuntimeError: unreachable`. It is reachable there: verified 2026-08-25
/// against a `wasm-bindgen --target nodejs` build of `crates/playground`, a bare
/// top-level `(receive (after 1 :fired))` captures and answers `:fired`, but
/// `(do (sleep 5) :slept)` traps *here* — `sleep`'s `receive` sits inside a called
/// function, so it takes the native-nested carve-out above instead of capturing. The
/// fix for that is a non-blocking park for the nested case on wasm; it is not the
/// clock, and reading `sched_now()` here neither helps nor hurts it.
fn wait_for_message(ctx: &Ctx, i: usize, deadline: Option<Instant>) {
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
    // *before* it takes this state lock (in `wake_parked`), so a kill that fully
    // completed before we got here is visible now under the lock — without this check
    // its `cv.notify_one()` would have been lost (no waiter yet) and we'd block forever.
    // The caller (`receive_match`) re-checks and unwinds; serialised with `exit` by
    // this same lock, so no kill slips in between the check and the wait.
    if crate::process::in_capture_run() && ctx.mailbox.kill_pending.load(Ordering::Relaxed) {
        return;
    }
    set_self_status(ctx, ST_WAITING);
    let _dirty = crate::process::dirty_block();
    match deadline {
        Some(d) => {
            // The SCHEDULER clock — the one `d` was minted on. See the doc comment.
            let now = super::timer::sched_now();
            if now < d {
                // Guard dropped at end of scope (before we return).
                let _g = ctx.mailbox.cv.wait_timeout(st, d - now);
            }
        }
        None => {
            let _g = ctx.mailbox.cv.wait(st);
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

pub(super) fn wake_for_timeout(pid: u64, gen: u64) {
    let mailbox = REGISTRY.get(pid);
    if let Some(mb) = mailbox {
        // Stale entry — the process has re-parked (or moved on) since this timer
        // was armed. Skip it: the live deadline has its own, current-gen entry.
        if mb.timer_gen.load(Ordering::Relaxed) != gen {
            return;
        }
        let mut st = crate::core::sync::lock(&mb.state);
        let parked = wake_parked(&mut st);
        drop(st);
        // Both paths (`wake_both`). This site had **no** condvar notify at all — alone among
        // the wake sites — which was survivable only because a cv-blocked receiver with a
        // deadline sits in `wait_timeout` and self-wakes at the same instant. That made the
        // timer thread's wake redundant *for that shape* and missing for any other; leaving
        // one wake site with different reachability is how the either/or bug above got in.
        wake_both(&mb, parked); // timer wake (capture-mode → may migrate)
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
    set_status(&ctx.mailbox, status);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seq ordering after a candidate is re-inserted — the invariant the receive-mark
    /// (ADR-195) binary-searches on, and the one the clamped re-insert used to break.
    ///
    /// The interleaving: a `receive` scan resumed at index 3 (the root thread and a
    /// native-nested receive both keep their scan index across a wait) and
    /// *optimistically popped* that candidate; the matcher then ran with the mailbox lock
    /// released, and a clause's `:when` guard did a **consuming nested `receive`**
    /// (`std/prelude/process.blsp` evaluates guards during the scan, and `scan_mailbox`'s
    /// own comment names this case). That consume shifted the queue, so the recorded scan
    /// position no longer named this envelope's slot — it named a message with a *higher*
    /// seq. Putting the candidate back there left `[4, 5, 3, 6]`, and a later `receive`
    /// pinned on a fresh `ref` then ran `partition_point` over a predicate that is not
    /// partitioned. Nothing crashes: it silently skips a matchable post-mark message and
    /// the caller times out.
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
        let ctx = Ctx {
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
}
