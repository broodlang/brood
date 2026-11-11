//! Location-transparent message routing: the [`Target`] a `send` names within a node,
//! and delivering a message to it — in-process for a local node, as a `Send` frame over
//! the link for a remote one — with the once-per-name warning for a message to a
//! registered name no process holds.

use super::*;

// ----- routing ---------------------------------------------------------------

/// How a `send` names its target within a node.
pub(crate) enum Target {
    /// A concrete process id (a pid's local part).
    Pid(u64),
    /// A registered name resolved on the destination node.
    Name(Symbol),
}
/// Warn, once per name, that a message addressed to a **registered name no process holds**
/// was dropped.
///
/// The drop itself is correct and stays correct, and legitimate — the name
/// may simply be gone, or never have existed. What this fixes is the **silence**. `send` is
/// fire-and-forget, so the sender has already moved on and cannot be told; the receiving node
/// is the only party that knows the message died. A silent drop otherwise surfaces only as a
/// deadline expiring with no information anywhere (KI-36).
///
/// **Default-on, deliberately.** A flag-gated trace is worth nothing for this class, because
/// nobody arms a flag before the bug they have not diagnosed yet (KI-39). Silence it with
/// `BROOD_NO_DROP_WARN=1`.
///
/// **Deduplicated per name**, so a hot loop addressing a dead service warns once rather than
/// flooding: the set is bounded by the number of distinct names, not by traffic.
pub(super) fn warn_dropped_to_unregistered_name(name: Symbol, whence: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static QUIET: AtomicBool = AtomicBool::new(false);
    static QUIET_INIT: std::sync::Once = std::sync::Once::new();
    QUIET_INIT.call_once(|| {
        QUIET.store(
            std::env::var_os("BROOD_NO_DROP_WARN").is_some(),
            Ordering::Relaxed,
        );
    });
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    /// Ceiling on the dedup set. The set exists so a hot loop addressing a dead service
    /// warns once rather than flooding, and its size is "the number of distinct names" —
    /// which for an INBOUND drop is chosen by the peer, not by us. Past the cap we stop
    /// growing and simply warn again: a flood of distinct names is itself worth seeing,
    /// and the alternative is a remote-controlled set that never shrinks. KI-97 item 4.
    const MAX_WARNED_NAMES: usize = 4096;
    static SEEN: std::sync::Mutex<Option<std::collections::HashSet<Symbol>>> =
        std::sync::Mutex::new(None);
    let first = match SEEN.lock() {
        Ok(mut g) => {
            let seen = g.get_or_insert_with(Default::default);
            if seen.len() >= MAX_WARNED_NAMES && !seen.contains(&name) {
                true // at the cap: warn without recording, rather than grow
            } else {
                seen.insert(name)
            }
        }
        // A poisoned lock must not silence the diagnostic this exists to print.
        Err(_) => true,
    };
    if first {
        eprintln!(
            "dist: dropped {whence} message for unregistered name :{} \
             (no process holds this name; warns once per name, BROOD_NO_DROP_WARN=1 to silence)",
            value::symbol_name(name)
        );
    }
}

/// Deliver `msg` to `target` on `node`, location-transparently: a local node
/// delivers in-process; a remote one forwards over the link. Unknown name,
/// unknown/disconnected node, or a dead pid is a silent no-op.
///
/// Returns whether a route existed at all: `true` for any local target (even a
/// dead pid — process liveness stays silent) and for a remote node with
/// a live link; `false` when the node is **unknown/disconnected** (the message
/// was dropped on the floor). `send` surfaces that as a catchable
/// `:noconnection` error when the sending process opted in via
/// `(proc/flag :send-errors true)` — so a caller can queue-and-retry instead
/// of silently losing messages until the reconnect (the dist self-healing
/// seam); every other caller ignores it.
///
/// The message is queued in `lane` wherever it lands (ADR-492): a remote node gets the lane
/// in the `Send` frame, so `proc/send-priority` and `proc/send-latest` mean the same thing
/// to a pid on another node as to one here.
pub(crate) fn route_in_lane(
    node: Symbol,
    target: Target,
    msg: Message,
    lane: process::Lane,
) -> bool {
    if is_local(node) {
        let id = match target {
            Target::Pid(id) => id,
            Target::Name(name) => match crate::core::sync::read(&NAMES).get(name) {
                Some(id) => id,
                None => {
                    warn_dropped_to_unregistered_name(name, "local");
                    // Still `true`: a route existed, the *name* did not. Returning false here
                    // would surface this as `:noconnection` to a `send-errors` sender, which
                    // is a different and wrong diagnosis — the node is fine.
                    return true;
                }
            },
        };
        process::deliver_in_lane(id, msg, lane);
        return true;
    }
    // Remote: encode a Send frame and hand it to the peer's writer thread. Snapshot the
    // writer handles (and a guest link's standing), then release NODES before enqueueing —
    // see [`enqueue_to`] for why the shutdown syscall must not run under the lock.
    let link = crate::core::sync::read(&NODES).get(&node).map(|conn| {
        (
            conn.writer_handles(),
            conn.guest.clone(),
            Arc::clone(&conn.latest),
        )
    });
    let Some(((tx, sock), guest, latest)) = link else {
        return false;
    };
    // A `send-latest` replaces its slot's unwritten frame on the link (ADR-492).
    let slot = match &lane {
        process::Lane::Latest(key) => Some(latest_slot(&target, key)),
        _ => None,
    };
    // A guest link is told about every local pid in the message, which is what lets it
    // reply to one (ADR-415) — noted on the link the message goes out on, before it is
    // queued there.
    if let Some(guest) = &guest {
        guest.note_outbound(&msg);
    }
    // And its own pids go back in the guest's own spelling (ADR-425).
    let mut frame = Frame::Send { target, msg, lane };
    if let Some(guest) = &guest {
        guest.translate_outbound(&mut frame);
    }
    // A large bytes value is left out and spliced in by the writer, whole the first time
    // and by digest after (ADR-695).
    let encoded: io::Result<Payload> = shared_bytes::encode_sharing(|| encode_payload(&frame))
        .and_then(|payload| {
            if payload.whole_len() > MAX_FRAME {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "frame of {} bytes exceeds the {MAX_FRAME}-byte limit",
                        payload.whole_len()
                    ),
                ))
            } else {
                Ok(payload.into())
            }
        });
    let bytes = match encoded {
        Ok(payload) => payload,
        Err(e) => {
            eprintln!(
                "dist: cannot encode message for {}: {}",
                value::symbol_name(node),
                e
            );
            return true; // a link exists; the payload was the problem
        }
    };
    // Either severs the link if the writer is gone or stalled.
    let _ = match slot {
        Some(slot) => enqueue_latest(&tx, &sock, &latest, slot, bytes),
        None => enqueue_to(&tx, &sock, bytes),
    };
    true
}

/// [`route_in_lane`] on the normal lane: the tests' plain send.
#[cfg(test)]
pub(crate) fn route(node: Symbol, target: Target, msg: Message) -> bool {
    route_in_lane(node, target, msg, process::Lane::Normal)
}
