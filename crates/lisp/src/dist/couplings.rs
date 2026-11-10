//! Monitors and links across nodes: the control frames that set up and tear down a
//! process monitor or link whose other end lives on a peer (ADR-067), and node monitors
//! (`monitor-node`) with their `[:nodedown name]` message.

use super::*;

/// Encode `frame` and enqueue it on the link to `target_node`. Returns whether
/// the bytes actually made it onto that link's writer queue: `false` if the link
/// is down (no `NODES` entry) *or* its bounded send queue is full/severed. This
/// is the single send path for every control frame (monitor/link/exit family),
/// so their error handling is uniform — the two callers that care about delivery
/// (`monitor_remote`, `link_remote`) branch on the bool to fire `:noconnection`;
/// the rest are best-effort and ignore it.
///
/// Error-handling choice: a failed *encode* is logged via `eprintln!` (adapted
/// from the old `monitor_remote`, the better of the two prior tails — the others
/// silently swallowed it) rather than dropped, because a control frame failing to
/// encode is a real bug, not an expected condition — its only variable-width field
/// is `Exit`'s `reason`, and an oversized reason that can't be shipped is worth a
/// diagnostic. A failed encode counts as not-sent.
///
/// A frame to a GUEST link names the guest's pids the way the guest spells itself
/// (ADR-425): the link's [`guest::GuestLink::translate_outbound`] rewrites it first.
pub(super) fn send_frame(target_node: Symbol, mut frame: Frame) -> bool {
    // Snapshot the writer handles, then release NODES before encoding and enqueueing —
    // see [`enqueue_to`] for why the shutdown syscall must not run under the lock.
    let link = crate::core::sync::read(&NODES)
        .get(&target_node)
        .map(|conn| (conn.writer_handles(), conn.guest.clone()));
    let Some(((tx, sock), guest)) = link else {
        return false;
    };
    if let Some(guest) = &guest {
        guest.translate_outbound(&mut frame);
    }
    let bytes: Arc<Vec<u8>> = match encode_payload(&frame) {
        Ok(b) => Arc::new(b),
        Err(e) => {
            eprintln!(
                "dist: cannot encode control frame for {}: {}",
                value::symbol_name(target_node),
                e
            );
            return false;
        }
    };
    enqueue_to(&tx, &sock, bytes)
}

/// `(monitor (Pid remote_node remote_pid))` from the cross-node path: ship a
/// `Frame::Monitor` to the peer and record the pending remote watcher locally
/// (so net-split can fire `:noconnection` to the watcher even though the
/// monitor target lives elsewhere). If the peer link isn't up, deliver
/// `:noconnection` immediately — same shape an immediately-dead local target
/// gets (`:noproc` from `add_monitor`), just a different reason.
pub(crate) fn monitor_remote(target_node: Symbol, target_pid: u64, watcher_pid: u64, mref: u64) {
    // Record the pending entry **before** sending. This closes a race against
    // `drop_link`/`handle_node_down`:
    //   • If we record before they run, they'll find our entry in
    //     `PENDING_REMOTE` and fire `:noconnection` to us — even if our send
    //     never made it onto the wire.
    //   • If they run first (`NODES` already empty when `send_frame` looks),
    //     `send_frame` returns false and we fall through to the explicit cleanup
    //     below, dropping our pending entry and firing `:noconnection` ourselves.
    // The pending entry can't be orphaned in either branch. And it fires ONCE: when
    // the link drops after the record but before the send, `handle_node_down` has
    // already claimed the entry and fired, the send then fails, and the fallback finds
    // nothing left to claim, so no second `[:down mref … :noconnection]` fires.
    process::record_pending_remote(target_node, target_pid, watcher_pid, mref);
    #[cfg(test)]
    tests::run_setup_gap_hook();
    let sent = send_frame(
        target_node,
        Frame::Monitor {
            watcher_pid,
            target: target_pid,
            mref,
        },
    );
    if !sent {
        process::fire_noconnection_if_pending(target_node, target_pid, watcher_pid, mref);
    }
}

/// `(monitor {:name n :node target_node})`: [`monitor_remote`] for a process registered
/// under `name` on the peer, which resolves it on arrival (`Frame::MonitorName`). The pid
/// is not known here, so the pending entry and an unreachable `:noconnection` name the
/// placeholder pid `u64::MAX` — the one a local `monitor` of an unregistered name fires
/// `:noproc` for. A resolved monitor's DOWN carries the real pid.
pub(crate) fn monitor_remote_name(target_node: Symbol, name: Symbol, watcher_pid: u64, mref: u64) {
    process::record_pending_remote(target_node, u64::MAX, watcher_pid, mref);
    let sent = send_frame(
        target_node,
        Frame::MonitorName {
            watcher_pid,
            name,
            mref,
        },
    );
    if !sent {
        process::fire_noconnection_if_pending(target_node, u64::MAX, watcher_pid, mref);
    }
}

/// `(demonitor mref)` for a monitor that was set up against a remote pid:
/// ship a `Frame::Demonitor` and forget the pending entry locally. Best
/// effort, like the local demonitor — the peer drops the matching watcher
/// from its `MONITORS` table.
pub(crate) fn demonitor_remote(target_node: Symbol, watcher_pid: u64, mref: u64) {
    process::drop_pending_remote(target_node, watcher_pid, mref);
    let _ = send_frame(target_node, Frame::Demonitor { watcher_pid, mref });
}

/// A monitor a peer registered on one of our local pids (`Frame::Monitor`) just
/// fired: ship the DOWN as a dedicated `Frame::Down`, not an ordinary `Send`, so
/// the watcher's node can retire its `PENDING_REMOTE` entry the moment the
/// one-shot delivers — as a plain message that entry outlived its own DOWN and a
/// later node-down fired a second `[:down mref … :noconnection]` (KI-96).
/// Best-effort: a DOWN to a disconnected watcher has nowhere to go (its own
/// `[:nodedown]`/`:noconnection` path already covers it).
pub(crate) fn send_down(
    target_node: Symbol,
    watcher_pid: u64,
    mref: u64,
    dying_pid: u64,
    reason: Message,
) {
    let _ = send_frame(
        target_node,
        Frame::Down {
            watcher_pid,
            mref,
            target_pid: dying_pid,
            reason,
        },
    );
}

// ---- cross-node links (ADR-067) — the symmetric cousin of monitor_remote ----

/// `(link remote-pid)`: record our half of the link, ship a `Frame::Link` so the
/// peer records its half, and — if the link to that node isn't up — fire an
/// immediate `:noconnection` to the local linker (same shape a monitor's
/// unreachable target gets). `local_pid` is the linker (self). Race-free against
/// net-split exactly as `monitor_remote`: record before sending.
pub(crate) fn link_remote(target_node: Symbol, target_pid: u64, local_pid: u64) {
    // `local_pid` is the calling process, so this is true by construction; binding it
    // keeps the `#[must_use]`-ish intent visible rather than discarding a meaningful bool.
    let _linked = process::record_remote_link(local_pid, target_node, target_pid);
    #[cfg(test)]
    tests::run_setup_gap_hook();
    let sent = send_frame(
        target_node,
        Frame::Link {
            from_pid: local_pid,
            to_pid: target_pid,
        },
    );
    if !sent {
        // No link to that node: the target is unreachable. Fire `:noconnection`
        // to the linker (this also drops the half-entry we just recorded) — unless a
        // link that dropped between the record and the send already did, in which
        // case the entry is gone and this fires nothing (no duplicate exit).
        process::deliver_remote_link_exit(
            local_pid,
            target_node,
            target_pid,
            Message::Keyword(value::intern(pk::NOCONNECTION)),
        );
    }
}

/// `(unlink remote-pid)`: drop our half and ship a best-effort `Frame::Unlink`.
pub(crate) fn unlink_remote(target_node: Symbol, target_pid: u64, local_pid: u64) {
    process::drop_remote_link(local_pid, target_node, target_pid);
    let _ = send_frame(
        target_node,
        Frame::Unlink {
            from_pid: local_pid,
            to_pid: target_pid,
        },
    );
}

/// A local linked process `from_pid` died with `reason`: ship a link
/// `Frame::Exit` to its remote peer `target_pid` on `target_node`. Best-effort —
/// if the link is down the peer already learns via its own net-split handling.
/// Called from `links::notify_peers`.
pub(crate) fn send_link_exit(target_node: Symbol, target_pid: u64, from_pid: u64, reason: Message) {
    let _ = send_frame(
        target_node,
        Frame::Exit {
            from_pid,
            to_pid: target_pid,
            reason,
            link: true,
        },
    );
}

/// `(exit remote-pid reason)`: ship a non-link `Frame::Exit` routed straight to
/// the peer's `scheduler::exit` (kill-style, like the local builtin). Used for an
/// explicit remote exit and for a supervisor terminating a remote child.
pub(crate) fn exit_remote(target_node: Symbol, target_pid: u64, reason: Message) {
    let _ = send_frame(
        target_node,
        Frame::Exit {
            from_pid: 0, // unused for an explicit (non-link) exit
            to_pid: target_pid,
            reason,
            link: false,
        },
    );
}

/// Resolve a bare node name (no `@`) to the qualified form by looking in NODES.
///
/// `(monitor-node :a)` passes symbol `a`; NODES is keyed by `a@127.0.0.1`.
/// Without this step the liveness check `!NODES.contains_key(&name)` always
/// returns true for bare names, firing an immediate `[:nodedown]` even while the
/// peer is alive — and `fire_nodedown` never finds the watcher on a real down.
///
/// Returns the name unchanged if it already contains `@`, or if no connected
/// peer with the given base name exists (peer is already down; the bare name
/// is used for the immediate-delivery path in `monitor_node`).
pub(super) fn qualify_node_name(name: Symbol) -> Symbol {
    let s = value::symbol_name(name);
    if s.contains('@') {
        return name;
    }
    let prefix = format!("{s}@");
    crate::core::sync::read(&NODES)
        .keys()
        .find(|&&k| value::symbol_name(k).starts_with(&prefix))
        .copied()
        .unwrap_or(name)
}

/// `(monitor-node name pid)` — deliver `[:nodedown name]` to `pid` when a link to
/// `name` goes down. Persistent (fires on each down) until the process exits.
/// If `name` isn't us and there's no current link, the node is effectively
/// already down and `[:nodedown]` is delivered immediately.
pub(crate) fn monitor_node(name: Symbol, pid: u64) {
    let name = qualify_node_name(name);
    // Registration and the liveness check must be atomic w.r.t. `fire_nodedown`
    // (which reads NODE_MONITORS then delivers). Holding the write lock across
    // both prevents the race where fire_nodedown sees a new watcher AND our
    // own fallback also fires.
    //
    // Always register — monitor_node is persistent: fires on
    // every future down event until the process exits. Dedup so a pid calling
    // (monitor-node name) again after a reconnect doesn't double-fire per down.
    //
    // Lock order: NODE_MONITORS write → NODES read. Safe: drop_link releases
    // NODES write *before* calling fire_nodedown, so no thread holds NODES write
    // while waiting for NODE_MONITORS.
    let immediate = {
        let mut monitors = crate::core::sync::write(&NODE_MONITORS);
        let watchers = monitors.entry(name).or_default();
        if !watchers.contains(&pid) {
            watchers.push(pid);
        }
        // If the peer is already down, deliver immediately as well as register.
        // A tiny residual race: if fire_nodedown was blocked on our write lock
        // (peer died in this same instant), it will also deliver to our new
        // watcher once we release → two [:nodedown] messages possible in that
        // sub-microsecond window. Receivers must tolerate duplicate nodedowns.
        !is_local(name) && !crate::core::sync::read(&NODES).contains_key(&name)
    };
    if immediate {
        process::deliver(pid, nodedown_msg(name));
    }
}

/// Cancel `pid`'s node monitor for `name`. A no-op if no monitor is registered.
/// Needed when a live process wants to stop watching a node before it exits;
/// [`unregister_dead_pid_while`] handles the death case automatically.
///
/// **Residual race**: `fire_nodedown` snapshots the watcher list under a *read*
/// lock and then delivers outside any lock. If a `fire_nodedown` for `name` is
/// already past the snapshot step when `demonitor_node` removes `pid`, one
/// spurious `[:nodedown name]` will still arrive. Callers must tolerate it
/// (the same tolerance required for the `monitor_node` registration race).
pub(crate) fn demonitor_node(name: Symbol, pid: u64) {
    let name = qualify_node_name(name);
    let mut monitors = crate::core::sync::write(&NODE_MONITORS);
    if let Some(watchers) = monitors.get_mut(&name) {
        watchers.retain(|&w| w != pid);
        if watchers.is_empty() {
            monitors.remove(&name);
        }
    }
}

/// The `[:nodedown <name>]` message a downed link delivers to its watchers.
pub(super) fn nodedown_msg(name: Symbol) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern(pk::NODEDOWN)),
        Message::Keyword(name),
    ])
}
