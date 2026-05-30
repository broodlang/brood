//! Erlang-style process **links**: symmetric, bidirectional failure coupling —
//! the complement of monitors ([`super::monitor`]). Where a monitor is a one-way
//! notification (`[:down …]`) that never affects the watched process, a link is
//! a two-way coupling: when a linked process dies, each peer either
//!
//!   - receives a **trappable** `[:EXIT <pid> <reason>]` message (if it set
//!     `trap_exit`), or
//!   - is itself **killed** for an abnormal reason (propagation), which cascades
//!     through *its* links. A `:normal` exit never kills a non-trapping peer.
//!
//! This is what lets a supervisor tear its whole subtree down when the supervisor
//! itself dies (the "orphan on supervisor crash" gap monitors can't close), and
//! it's the general Erlang `link`/`unlink`/`process_flag(trap_exit, …)`.
//!
//! **Cross-node links (ADR-067 dist).** A link may span nodes, mirroring the
//! remote-monitor machinery ([`super::monitor`] + [`crate::dist`]). Each side
//! records its half in [`REMOTE_LINKS`] (`local_pid → [(node, remote_pid)]`); the
//! link request rides a `Frame::Link`, a death rides a `Frame::Exit`, and a
//! net-split fires `:noconnection` to every local peer of a process on the dropped
//! node — exactly the `:noconnection`-on-net-split semantics monitors have.
//!
//! **Propagation hardness (D-simple, ADR-067).** Brood's `(exit pid reason)`
//! couples "untrappable/immediate" to `reason == :kill`. A non-trapping peer must
//! die *immediately*, so link propagation routes through the hard `(exit peer
//! :kill)` — the peer dies promptly but reports `:kill` to its own monitors rather
//! than the originating reason (immaterial for supervision).
//!
//! Lock ordering mirrors [`super::monitor`]: [`link`] takes LINKS then (nested)
//! REGISTRY for its race-free liveness check; [`super::deregister`] takes its
//! tables **sequentially** and never holds REGISTRY while reaching for LINKS, so
//! the two pairings can't deadlock. REMOTE_LINKS is taken on its own (never nested
//! under LINKS/REGISTRY), and any wire send / `deliver` happens after it's dropped.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex};

use crate::core::sync::lock;
use crate::core::value::{self, Symbol};

use super::mailbox::{deliver, REGISTRY};
use super::message::Message;
use super::scheduler::{self, self_pid};

/// Links between two **local** processes: pid → its set of linked local peers.
/// **Symmetric** — [`link`] inserts both directions. One table for the runtime,
/// like [`super::monitor::MONITORS`].
static LINKS: LazyLock<Mutex<HashMap<u64, HashSet<u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A remote link peer — a process `(node, pid)` on another runtime.
type RemotePeer = (Symbol, u64);

/// **Cross-node** links: a *local* pid → the remote peers it is linked to. Both
/// nodes keep their own half (each maps its local pid → the peer's `(node, pid)`),
/// so either process dying — or the link net-splitting — is actionable from either
/// side. The dual of [`super::monitor`]'s `PENDING_REMOTE`, but symmetric (a link
/// couples both ways, a monitor one).
static REMOTE_LINKS: LazyLock<Mutex<HashMap<u64, Vec<RemotePeer>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `(link pid)` for a **local** `pid` — symmetrically link the current process and
/// `pid`. If `pid` is already dead, the caller is notified immediately (a trappable
/// `[:EXIT pid :noproc]` if it traps, otherwise the caller dies — `:noproc` is
/// abnormal), matching Erlang. Self-links are a no-op.
pub fn link(a: u64, b: u64) {
    if a == b {
        return;
    }
    // Race-free against `deregister`: check `b`'s liveness inside the LINKS
    // critical section (same shape as `monitor::add_monitor`).
    {
        let mut links = lock(&LINKS);
        if lock(&REGISTRY).contains_key(&b) {
            links.entry(a).or_default().insert(b);
            links.entry(b).or_default().insert(a);
            return;
        }
    }
    // `b` already dead — notify the linker via the same trap-or-propagate path.
    deliver_exit_to(a, local_pid_msg(b), Message::Keyword(value::intern("noproc")));
}

/// `(unlink pid)` for a **local** `pid` — drop the symmetric link. Best-effort.
pub fn unlink(a: u64, b: u64) {
    let mut links = lock(&LINKS);
    if let Some(s) = links.get_mut(&a) {
        s.remove(&b);
    }
    if let Some(s) = links.get_mut(&b) {
        s.remove(&a);
    }
}

/// Set the current process's `trap_exit` flag; returns the previous value. When
/// set, a linked peer's death arrives as a `[:EXIT pid reason]` *message* instead
/// of killing this process. No-op (returns false) for a dead/unknown pid.
pub fn set_trap_exit(pid: u64, on: bool) -> bool {
    match lock(&REGISTRY).get(&pid) {
        Some(mb) => mb.trap_exit.swap(on, Ordering::Relaxed),
        None => false,
    }
}

/// A linked process `dead` (local) exited with `reason`: notify every peer —
/// **local and remote** — and clear the links. Called from [`super::deregister`]
/// **after** monitors, with no other lock held.
pub(super) fn notify_peers(dead: u64, reason: &Message) {
    // Local peers: extract under LINKS, drop reverse edges, release, then deliver.
    let local_peers = {
        let mut links = lock(&LINKS);
        let peers = links.remove(&dead).unwrap_or_default();
        for &q in &peers {
            if let Some(s) = links.get_mut(&q) {
                s.remove(&dead);
            }
        }
        peers
    };
    let dead_msg = local_pid_msg(dead);
    for q in local_peers {
        deliver_exit_to(q, dead_msg.clone(), reason.clone());
    }
    // Remote peers: ship a link `Frame::Exit` to each peer's node. The remote
    // side delivers it to its local process and drops its reverse entry.
    let remote_peers = lock(&REMOTE_LINKS).remove(&dead).unwrap_or_default();
    for (node, q) in remote_peers {
        crate::dist::send_link_exit(node, q, dead, reason.clone());
    }
}

/// Notify one **local** `peer` that linked process `dead` (given as its
/// `Message::Pid`, local or remote) exited with `reason`: a trappable
/// `[:EXIT dead reason]` if `peer` traps, otherwise — for an abnormal reason —
/// propagate by hard-killing `peer`. A `:normal` reason to a non-trapping peer
/// does nothing (Erlang semantics).
fn deliver_exit_to(peer: u64, dead_msg: Message, reason: Message) {
    if traps_exit(peer) {
        deliver(peer, exit_message(dead_msg, reason));
    } else if !is_normal(&reason) {
        scheduler::exit(peer, Message::Keyword(value::intern("kill")));
    }
}

/// Does `pid` trap exits? false for a dead/unknown pid.
fn traps_exit(pid: u64) -> bool {
    lock(&REGISTRY)
        .get(&pid)
        .is_some_and(|mb| mb.trap_exit.load(Ordering::Relaxed))
}

fn is_normal(reason: &Message) -> bool {
    matches!(reason, Message::Keyword(k) if *k == value::intern("normal"))
}

/// `[:EXIT <pid> <reason>]` — the message a trapping process receives. `dead_msg`
/// is the dead peer's `Message::Pid` (its node may be local or remote).
fn exit_message(dead_msg: Message, reason: Message) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("EXIT")),
        dead_msg,
        reason,
    ])
}

/// The `Message::Pid` of a local process `id`.
fn local_pid_msg(id: u64) -> Message {
    Message::Pid {
        node: crate::dist::local_node(),
        id,
    }
}

/// How many *local* peers `pid` is linked to (0 for a dead/unlinked pid).
pub fn link_count(pid: u64) -> usize {
    lock(&LINKS).get(&pid).map_or(0, |s| s.len())
}

/// Convenience: link the current process to a **local** `target`.
pub fn link_self(target: u64) {
    link(self_pid(), target);
}

/// Convenience: unlink the current process from a **local** `target`.
pub fn unlink_self(target: u64) {
    unlink(self_pid(), target);
}

// ---- cross-node links (ADR-067 dist) ---------------------------------------

/// Record that **local** `local_pid` is linked to remote `(node, remote_pid)`.
/// Idempotent-ish (dedups the exact pair). Called by [`crate::dist::link_remote`]
/// on the linker's side and by the inbound `Frame::Link` handler on the peer's
/// side, so both nodes hold their half. Returns whether `local_pid` is currently
/// alive — the caller (linker side) fires an immediate `:noproc` if not.
pub(crate) fn record_remote_link(local_pid: u64, node: Symbol, remote_pid: u64) {
    let mut t = lock(&REMOTE_LINKS);
    let v = t.entry(local_pid).or_default();
    if !v.iter().any(|&(n, p)| n == node && p == remote_pid) {
        v.push((node, remote_pid));
    }
}

/// Drop the cross-node link `local_pid ↔ (node, remote_pid)` (best-effort).
pub(crate) fn drop_remote_link(local_pid: u64, node: Symbol, remote_pid: u64) {
    if let Some(v) = lock(&REMOTE_LINKS).get_mut(&local_pid) {
        v.retain(|&(n, p)| !(n == node && p == remote_pid));
    }
}

/// Deliver a **remote link death** to local `to_pid`: the linked process
/// `from_pid` on `from_node` exited with `reason`. Drops the reverse remote-link
/// entry, then delivers via the trap-or-propagate path (the `[:EXIT]` message
/// carries the *remote* pid). Inbound `Frame::Exit { link: true }`.
pub(crate) fn deliver_remote_link_exit(
    to_pid: u64,
    from_node: Symbol,
    from_pid: u64,
    reason: Message,
) {
    drop_remote_link(to_pid, from_node, from_pid);
    deliver_exit_to(
        to_pid,
        Message::Pid {
            node: from_node,
            id: from_pid,
        },
        reason,
    );
}

/// A node link dropped (net-split). For every local process linked across it,
/// fire a `:noconnection` exit (trap → `[:EXIT remote :noconnection]`, else the
/// peer dies — `:noconnection` is abnormal) and drop the entries. Mirrors
/// `monitor::handle_node_down`'s `:noconnection` fan-out; called from
/// `dist::fire_nodedown`.
pub(crate) fn handle_node_down(node: Symbol) {
    // Collect (local_pid, remote_pid) for the dropped node under the lock, prune
    // those entries, release, then deliver (deliver may re-enter scheduler::exit).
    let affected: Vec<(u64, u64)> = {
        let mut t = lock(&REMOTE_LINKS);
        let mut hits = Vec::new();
        for (&local, peers) in t.iter_mut() {
            for &(n, p) in peers.iter() {
                if n == node {
                    hits.push((local, p));
                }
            }
            peers.retain(|&(n, _)| n != node);
        }
        hits
    };
    let reason = Message::Keyword(value::intern("noconnection"));
    for (local, remote) in affected {
        deliver_exit_to(
            local,
            Message::Pid { node, id: remote },
            reason.clone(),
        );
    }
}
