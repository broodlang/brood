//! Erlang-style monitors: a one-shot, unidirectional `[:down …]` to a
//! **watcher** when the **watched** pid dies (or, for a remote target, when
//! the link to its node drops).
//!
//! - [`MONITORS`] is the watched-side table: `pid → [Watcher]`. Both `Local`
//!   and `Remote` watchers live in the same table so [`fire_down`] /
//!   [`add_monitor`] / [`drop_monitor`] are one code path that picks the
//!   delivery channel by variant.
//! - [`PENDING_REMOTE`] is the **sender side** of a cross-node monitor —
//!   what we'd need to fire `:noconnection` if the link to a peer drops.
//!   The complement of [`MONITORS`], with no down-delivery state, just
//!   enough to wake the local watcher on a net-split.
//!
//! Lock ordering: REGISTRY first, MONITORS second when a function needs
//! both. [`add_monitor`] takes them in that nested order (briefly takes
//! REGISTRY while holding MONITORS); [`super::deregister`] takes them
//! sequentially (REGISTRY, release, MONITORS). Don't introduce a function
//! that holds REGISTRY while reaching for MONITORS or this becomes a real
//! deadlock hazard.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::core::value::{self, Symbol, Value};

use super::mailbox::{deliver, REGISTRY};
use super::message::Message;
use super::scheduler::self_pid;

/// A monitor's *watcher* side — who gets the `[:down …]` when the watched pid
/// dies. The same enum carries a process on this runtime (`Local`) and a
/// process on a peer runtime (`Remote`), so one [`MONITORS`] table holds both
/// shapes and the same deregister / demonitor code drives them. The peer
/// learns we want a watch via the `dist::Frame::Monitor` frame, the down
/// notification rides back as an ordinary `Message::Vector([:down …])` send.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Watcher {
    /// A process on *this* runtime.
    Local { pid: u64, mref: u64 },
    /// A process on a peer runtime — `node` names that runtime, `pid` is the
    /// watcher's pid *over there* (so a peer's `[:down …]` lands in its own
    /// mailbox), `mref` is the watcher's monitor reference (opaque to us).
    Remote { node: Symbol, pid: u64, mref: u64 },
}

impl Watcher {
    /// Both variants carry a monitor ref; surface it for shared code paths.
    fn mref(&self) -> u64 {
        match *self {
            Watcher::Local { mref, .. } | Watcher::Remote { mref, .. } => mref,
        }
    }
}

/// Monitors: watched-pid → the watchers to notify when it dies. Each watcher
/// gets a `[:down <mref> <pid> <reason>]` message (Erlang `monitor`,
/// unidirectional and one-shot). No links yet — a monitor never affects the
/// watched process. A single table holds both `Local` and `Remote` watchers,
/// so the local-monitor path and the cross-node-monitor path share the same
/// "is the target alive? add or fire :noproc" logic and the same fan-out from
/// `deregister`.
pub(super) static MONITORS: LazyLock<Mutex<HashMap<u64, Vec<Watcher>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// **Pending remote monitors** — the *sender* side of `(monitor remote-pid)`.
/// Keyed by the peer's node-name, valued by the local triples we'd need to
/// fire `[:down mref pid :noconnection]` should the link to that peer die
/// (Erlang semantics: a monitor fires on net-split). Compact mirror of
/// [`MONITORS`] — no down-delivery state, just enough to wake the watcher
/// when the wire goes away.
static PENDING_REMOTE: LazyLock<Mutex<HashMap<Symbol, Vec<PendingRemote>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
struct PendingRemote {
    /// The local watcher to deliver `[:down …]` to.
    watcher_pid: u64,
    /// The monitor reference the watcher will pattern-match on.
    mref: u64,
    /// The remote pid being watched — the message's `pid` field.
    target_node: Symbol,
    target_pid: u64,
}

/// Source of unique reference ids, shared by `(ref)` and `(monitor …)` so every
/// ref — token or monitor handle — is distinct across the whole runtime.
static NEXT_REF: AtomicU64 = AtomicU64::new(0);

/// A fresh, globally-unique reference id. Backs `Value::Ref`.
pub fn next_ref() -> u64 {
    NEXT_REF.fetch_add(1, Ordering::Relaxed)
}

/// How many watchers are currently monitoring `pid` (the `:monitored-by` count
/// in `process-info`). Takes only the MONITORS lock; 0 for an unwatched/dead pid.
pub fn monitored_by(pid: u64) -> usize {
    crate::core::sync::lock(&MONITORS)
        .get(&pid)
        .map_or(0, |watchers| watchers.len())
}

/// Deliver a `[:down …]` to one watcher — the single fan-out point both
/// [`super::deregister`] (target died) and [`add_monitor`] (target was already dead)
/// use. Local watchers get an in-process mailbox push; remote watchers get
/// a routed `send`, so the wire-format `[:down …]` is exactly the message a
/// peer's process would receive locally.
pub(super) fn fire_down(w: Watcher, dying_pid: u64, reason: Message) {
    let msg = down_message(local_node_pid_msg(dying_pid), w.mref(), reason);
    match w {
        Watcher::Local { pid, .. } => deliver(pid, msg),
        Watcher::Remote { node, pid, .. } => {
            crate::dist::route(node, crate::dist::Target::Pid(pid), msg);
        }
    }
}

/// The `Message::Pid` for a process that lives on **this** runtime — the
/// `pid` field of a `[:down …]` we're firing. Wraps the (always-local-here)
/// node-name lookup so the call site reads as "make the pid value".
fn local_node_pid_msg(pid: u64) -> Message {
    Message::Pid {
        node: crate::dist::local_node(),
        id: pid,
    }
}

/// The `[:down <pid> <mref> <reason>]` message a monitor fires. `pid_msg`
/// is the dying process's pid as a `Message::Pid` — `Local` watchers see
/// this runtime's name there, `Remote` watchers see the same thing (still
/// correct: the dying pid lives on us).
fn down_message(pid_msg: Message, mref: u64, reason: Message) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("down")),
        Message::Ref(mref),
        pid_msg,
        reason,
    ])
}

/// `(monitor pid)` — start watching the **local** `pid`; returns a fresh
/// monitor `ref`. When `pid` dies, the caller receives `[:down <this-ref>
/// <pid> <reason>]`. If `pid` is already dead, the DOWN (`reason` `:noproc`)
/// is delivered immediately. The monitor is unidirectional and one-shot.
///
/// Routes through [`add_monitor`] with a `Watcher::Local`; the cross-node
/// path (`dist::Frame::Monitor`) calls the same function with a
/// `Watcher::Remote`, so the alive/dead branch and the `:noproc` fast path
/// are shared.
pub fn monitor(target: u64) -> Value {
    let mref = next_ref();
    add_monitor(
        target,
        Watcher::Local {
            pid: self_pid(),
            mref,
        },
    );
    Value::Ref(mref)
}

/// The shared "register a watcher" core — used by the local `monitor` builtin
/// and by `dist`'s `Frame::Monitor` handler. If `target` is alive, append
/// `watcher` to its monitor list; otherwise fire a synthetic `:noproc` down
/// to that same watcher immediately. The "delivery channel" of the down
/// (in-process mailbox vs. routed `send`) is decided inside [`fire_down`].
///
/// **Race-free against `deregister`**: the REGISTRY liveness check happens
/// inside the MONITORS critical section. `deregister` takes the same locks
/// in the same order (REGISTRY first, then MONITORS), so the two pairings
/// resolve cleanly: either we see the target alive and insert before
/// deregister can drain the bin (in which case deregister fires the down to
/// us), or we see the target gone (because deregister already drained
/// REGISTRY) and fire `:noproc` ourselves. Without the critical section we'd
/// have a TOCTOU window where the watcher gets stuck.
pub(crate) fn add_monitor(target: u64, watcher: Watcher) {
    let mut mons = crate::core::sync::lock(&MONITORS);
    if crate::core::sync::lock(&REGISTRY).contains_key(&target) {
        mons.entry(target).or_default().push(watcher);
        return;
    }
    drop(mons); // release before delivering — `fire_down` may need other locks
    fire_down(watcher, target, Message::Keyword(value::intern("noproc")));
}

/// `(demonitor mref)` — drop the calling process's monitor with that ref. Best
/// effort: a `[:down …]` already queued is not recalled.
pub fn demonitor(mref: u64) {
    let me = self_pid();
    drop_monitor(|w| matches!(*w, Watcher::Local { pid, mref: r } if pid == me && r == mref));
}

/// Remove every `Watcher` matching `pred` from `MONITORS`. The shared dropper
/// behind local `(demonitor mref)`, remote `Frame::Demonitor`, and the
/// node-down cleanup that flushes a peer's remote watchers from every target's
/// list.
pub(crate) fn drop_monitor(pred: impl Fn(&Watcher) -> bool) {
    let mut mons = crate::core::sync::lock(&MONITORS);
    for watchers in mons.values_mut() {
        watchers.retain(|w| !pred(w));
    }
}

// ---- pending remote monitors: the *sender* side ----------------------------
// When `(monitor remote-pid)` runs, the target lives on a peer; the entry that
// fires when the link dies (net-split = `:noconnection`) needs to be findable
// here. PENDING_REMOTE is the dual of MONITORS — same shape, watched-from
// instead of watched-by.

/// Remember "this local watcher is monitoring `target_node:target_pid`",
/// keyed by the peer node so net-split can find and fire it. Mirrors what
/// `add_monitor` does for local watchers, in a separate table because the
/// failure mode (link drop) is independent of any local target's death.
pub(crate) fn record_pending_remote(
    target_node: Symbol,
    target_pid: u64,
    watcher_pid: u64,
    mref: u64,
) {
    crate::core::sync::lock(&PENDING_REMOTE)
        .entry(target_node)
        .or_default()
        .push(PendingRemote {
            watcher_pid,
            mref,
            target_node,
            target_pid,
        });
}

/// Forget a pending remote monitor — the sender-side counterpart to
/// `drop_monitor`, called from `dist::demonitor_remote`. Identified by
/// (target_node, watcher_pid, mref) — the same triple `record_pending_remote`
/// stored.
pub(crate) fn drop_pending_remote(target_node: Symbol, watcher_pid: u64, mref: u64) {
    let mut t = crate::core::sync::lock(&PENDING_REMOTE);
    if let Some(v) = t.get_mut(&target_node) {
        v.retain(|p| !(p.watcher_pid == watcher_pid && p.mref == mref));
    }
}

/// `(demonitor mref)` on a ref the local table didn't claim: scan
/// `PENDING_REMOTE` for entries matching `(self_pid, mref)`, dispatch one
/// `Demonitor` frame per unique peer holding such an entry, and prune the
/// local pending side. The fan-out happens here (not in the builtin) so the
/// peer-set discovery and `drop_pending_remote` cleanup stay co-located.
pub(crate) fn demonitor_remote_fanout(mref: u64) {
    let me = self_pid();
    let peers: Vec<Symbol> = {
        let table = crate::core::sync::lock(&PENDING_REMOTE);
        table
            .iter()
            .filter(|(_, ps)| ps.iter().any(|p| p.watcher_pid == me && p.mref == mref))
            .map(|(node, _)| *node)
            .collect()
    };
    for node in peers {
        crate::dist::demonitor_remote(node, me, mref);
    }
}

/// The link to `node` just died. Drop **two** sets of monitors:
///   1. **Pending remote**: monitors *we* asked the peer to keep for us. Each
///      fires `[:down mref pid :noconnection]` to the local watcher (Erlang
///      semantics on net-split).
///   2. **Inbound remote**: watchers the peer registered on our local pids.
///      No notification — the peer is gone — but the entries would otherwise
///      leak and a future reconnect would still try to deliver to a fresh
///      generation of that peer.
pub(crate) fn handle_node_down(node: Symbol) {
    let pendings = crate::core::sync::lock(&PENDING_REMOTE)
        .remove(&node)
        .unwrap_or_default();
    for p in pendings {
        deliver(
            p.watcher_pid,
            down_message(
                Message::Pid {
                    node: p.target_node,
                    id: p.target_pid,
                },
                p.mref,
                Message::Keyword(value::intern("noconnection")),
            ),
        );
    }
    drop_monitor(|w| matches!(*w, Watcher::Remote { node: n, .. } if n == node));
}

/// Fire `:noconnection` to one watcher (the link isn't up, so we can't ask
/// the peer to monitor for us). Shared with `dist::monitor_remote`. Uses the
/// same `down_message` shape as a real DOWN so the watcher's `receive` clause
/// doesn't have to special-case anything.
pub(crate) fn fire_noconnection(target_node: Symbol, target_pid: u64, watcher_pid: u64, mref: u64) {
    deliver(
        watcher_pid,
        down_message(
            Message::Pid {
                node: target_node,
                id: target_pid,
            },
            mref,
            Message::Keyword(value::intern("noconnection")),
        ),
    );
}
