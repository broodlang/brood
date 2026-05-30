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
//! **Propagation hardness (D-simple, ADR-067).** Brood's `(exit pid reason)`
//! couples "untrappable/immediate" to `reason == :kill`. A non-trapping peer must
//! die *immediately* (even mid-CPU-loop), so link propagation routes through the
//! hard `(exit peer :kill)` — the peer dies promptly but reports `:kill` to its
//! own monitors rather than the originating reason. That's immaterial for the
//! supervision use (a torn-down worker isn't monitored by anyone but its dead
//! supervisor); a future "hard kill carrying an arbitrary reason" would make it
//! exact.
//!
//! Lock ordering mirrors [`super::monitor`]: [`link`] takes LINKS then (nested)
//! REGISTRY for its race-free liveness check; [`super::deregister`] takes its
//! tables **sequentially** and never holds REGISTRY while reaching for LINKS, so
//! the two pairings can't deadlock.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex};

use crate::core::sync::lock;
use crate::core::value;

use super::mailbox::{deliver, REGISTRY};
use super::message::Message;
use super::scheduler::{self, self_pid};

/// Links: pid → its set of linked peer pids. **Symmetric** — [`link`] inserts
/// both directions, so either process dying finds the other here. One table for
/// the whole runtime, like [`super::monitor::MONITORS`].
static LINKS: LazyLock<Mutex<HashMap<u64, HashSet<u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `(link pid)` — symmetrically link the current process and `pid`. If `pid` is
/// already dead, the caller is notified immediately (a trappable `[:EXIT pid
/// :noproc]` if it traps, otherwise the caller dies — `:noproc` is abnormal),
/// matching Erlang's "link to a dead process" behaviour. Self-links are a no-op.
pub fn link(a: u64, b: u64) {
    if a == b {
        return;
    }
    // Race-free against `deregister`: check `b`'s liveness inside the LINKS
    // critical section (same shape as `monitor::add_monitor`). Either we see `b`
    // alive and insert before `deregister` can drain its links (so its death will
    // notify us), or we see it gone (deregister already ran) and notify the
    // linker ourselves below.
    {
        let mut links = lock(&LINKS);
        if lock(&REGISTRY).contains_key(&b) {
            links.entry(a).or_default().insert(b);
            links.entry(b).or_default().insert(a);
            return;
        }
    }
    // `b` already dead — tell the linker, via the same trap-or-propagate path a
    // real death takes.
    deliver_exit_to(a, b, Message::Keyword(value::intern("noproc")));
}

/// `(unlink pid)` — drop the symmetric link between the current process and
/// `pid`. Best-effort (an `[:EXIT]` already queued is not recalled).
pub fn unlink(a: u64, b: u64) {
    let mut links = lock(&LINKS);
    if let Some(s) = links.get_mut(&a) {
        s.remove(&b);
    }
    if let Some(s) = links.get_mut(&b) {
        s.remove(&a);
    }
}

/// Set the current process's `trap_exit` flag (Erlang `process_flag(trap_exit,
/// …)`); returns the previous value. When set, a linked peer's death arrives as
/// a `[:EXIT pid reason]` *message* instead of killing this process. No-op
/// (returns false) if `pid` isn't a registered live process.
pub fn set_trap_exit(pid: u64, on: bool) -> bool {
    match lock(&REGISTRY).get(&pid) {
        Some(mb) => mb.trap_exit.swap(on, Ordering::Relaxed),
        None => false,
    }
}

/// A linked process `dead` exited with `reason`: notify every peer and clear the
/// links (both directions). Called from [`super::deregister`] **after** monitors,
/// with no other lock held. Extracts the peer set under the LINKS lock, releases
/// it, then notifies — `deliver_exit_to` may take REGISTRY / a mailbox lock / re-
/// enter `scheduler::exit`, none of which may run under LINKS.
pub(super) fn notify_peers(dead: u64, reason: &Message) {
    let peers = {
        let mut links = lock(&LINKS);
        let peers = links.remove(&dead).unwrap_or_default();
        // Drop the reverse edges so a peer's later death doesn't re-notify `dead`.
        for &q in &peers {
            if let Some(s) = links.get_mut(&q) {
                s.remove(&dead);
            }
        }
        peers
    };
    for q in peers {
        deliver_exit_to(q, dead, reason.clone());
    }
}

/// Notify one `peer` that linked process `dead` exited with `reason`: a trappable
/// `[:EXIT dead reason]` message if `peer` traps exits, otherwise — for an
/// abnormal reason — propagate by hard-killing `peer` (which cascades through its
/// own links when it dies). A `:normal` reason to a non-trapping peer does
/// nothing (Erlang semantics).
fn deliver_exit_to(peer: u64, dead: u64, reason: Message) {
    if traps_exit(peer) {
        deliver(peer, exit_message(dead, reason));
    } else if !is_normal(&reason) {
        // D-simple: immediate, untrappable. The peer dies on its own worker and
        // its `deregister` cascades the exit through its links.
        scheduler::exit(peer, Message::Keyword(value::intern("kill")));
    }
}

/// Does `pid` trap exits? Reads the flag off its registry-reachable mailbox;
/// false for a dead/unknown pid.
fn traps_exit(pid: u64) -> bool {
    lock(&REGISTRY).get(&pid).is_some_and(|mb| mb.trap_exit.load(Ordering::Relaxed))
}

fn is_normal(reason: &Message) -> bool {
    matches!(reason, Message::Keyword(k) if *k == value::intern("normal"))
}

/// The `[:EXIT <pid> <reason>]` message a trapping process receives when a linked
/// peer dies. The pid is the (local) dead process, as a `Message::Pid`.
fn exit_message(dead: u64, reason: Message) -> Message {
    Message::Vector(vec![
        Message::Keyword(value::intern("EXIT")),
        Message::Pid {
            node: crate::dist::local_node(),
            id: dead,
        },
        reason,
    ])
}

/// `(linked-to pid)` introspection support — how many processes `pid` is linked
/// to (0 for a dead/unlinked pid). Cheap; takes only the LINKS lock.
pub fn link_count(pid: u64) -> usize {
    lock(&LINKS).get(&pid).map_or(0, |s| s.len())
}

/// Convenience for the builtins: link the *current* process to `target`.
pub fn link_self(target: u64) {
    link(self_pid(), target);
}

/// Convenience for the builtins: unlink the *current* process from `target`.
pub fn unlink_self(target: u64) {
    unlink(self_pid(), target);
}
