//! Registering an authenticated link as THE link to its peer and tearing it down again:
//! `establish` with the duplicate resolution in `register_link` — the incarnation check
//! for a restarted peer and the connector tie-break for a second link to the same life —
//! and the node-down path that removes a link and severs the couplings made over it.

use super::*;

/// Register the authenticated link and spawn its reader + writer threads —
/// resolving a duplicate against any existing link to the same peer first.
/// `authenticated.addr` is how a third node should dial this peer (for mesh gossip).
///
/// A duplicate is one of two different things, and the peer's incarnation tells
/// them apart:
///
/// - **The peer restarted** (incarnations differ). The registered link reaches a
///   process that no longer exists — its host vanished without a FIN, so nothing has
///   torn it down yet, and the heartbeat would only notice after `DOWN_AFTER`. The
///   new link always wins: the old one is retired as a node-down of that old life
///   (`retire_previous_incarnation` — `[:nodedown]` to node monitors, `:noconnection`
///   to every monitor and link made over it), then this link registers as a
///   brand-new peer.
/// - **A second link to the same live peer** (same incarnation: two dials crossing,
///   or two dials from one side racing). Both reach the live peer, so tearing either
///   down must not reach a monitor: the connector tie-break picks one of two crossing
///   dials, and an equal connector keeps the registered link, which is not symmetric, so
///   this node makes one dial to a peer at a time ([`DialClaim`]).
///
/// A GUEST link (ADR-415) stands outside both: its name is only its claim, so it is never
/// registered under it. It gets a name this node mints in the guest namespace
/// ([`guest::GuestLink::name`], ADR-425), unique to the link — so it can be neither the
/// duplicate nor the restart of anything, and cannot share a name with any full peer,
/// here or anywhere in the cluster (KI-287).
pub(super) fn establish(authenticated: handshake::Authenticated, stream: Stream, role: Role) {
    let handshake::Authenticated {
        peer,
        addr: peer_addr,
        incarnation,
        session,
        grant,
        lanes,
    } = authenticated;
    // Who initiated *this* connection (the tie-break key).
    let connector = match role {
        Role::Initiator => local_node(),
        Role::Responder => peer,
    };
    let sock = Arc::new(stream);
    let (tx, rx) = mpsc::sync_channel::<Outbound>(WRITER_QUEUE_CAP);
    let latest = LatestSlots::default();
    let last_seen = Arc::new(AtomicU64::new(now_millis()));
    let id = NEXT_LINK.fetch_add(1, Ordering::Relaxed);
    // A guest link's every frame is filtered (ADR-415), through a standing that belongs
    // to THIS connection: the `Conn` and the reader share it, so it is in place before
    // the link is visible in `NODES`, it goes when the link goes, and no other link under
    // the same name — a full peer's that reclaims it, a later guest's — can change what
    // this link's reader admits, even for frames it is still draining after the name has
    // moved on (KI-286).
    //
    // And a guest link is known by the name minted for it, never the one it claimed
    // (ADR-425): that name is its `NODES` key, the node its reader stamps on what it
    // reads, and the node of its pids here.
    let guest = grant.map(|grant| Arc::new(guest::GuestLink::new(grant, peer)));
    let guest_link = guest.is_some();
    let peer = guest.as_ref().map_or(peer, |standing| standing.name());
    let link = Conn {
        id,
        connector,
        incarnation,
        addr: peer_addr,
        tx: tx.clone(),
        lanes,
        latest: Arc::clone(&latest),
        sock: Arc::clone(&sock),
        last_seen: Arc::clone(&last_seen),
        guest: guest.clone(),
    };
    let Some(was_new) = register_link(peer, link) else {
        // We lost the tie-break: close our socket, register and spawn nothing.
        let _ = sock.shutdown(Shutdown::Both);
        return;
    };

    ensure_heartbeat();

    // The link is authenticated and registered: split the session into its two
    // directional ciphers (ADR-089) — the writer owns the send cipher, the reader the
    // receive cipher, so neither shares crypto state.
    let Session { send, recv } = session;
    spawn_link_writer(peer, Arc::clone(&sock), rx, latest, send);
    spawn_link_reader(peer, id, sock, tx, recv, last_seen, guest);

    // A brand-new peer just joined: tell the cluster (ADR-088) — one broadcast sends the
    // newcomer our other peers and tells our peers about the newcomer. Skipped for a
    // reconnect/duplicate (the name was already known) and when meshing is disabled.
    if was_new && !guest_link && mesh_enabled() {
        broadcast_peer_table();
    }
}

/// Register `link` as THE link to `peer`, resolving a duplicate against any existing
/// one (see [`establish`]). Returns `None` when an existing link to the same life of
/// the peer wins the tie-break, else whether `peer` was brand-new (no link before).
///
/// Connectors are compared by *name* — interned ids differ per process, but both ends
/// share the names, so they pick the same physical link.
pub(super) fn register_link(peer: Symbol, link: Conn) -> Option<bool> {
    let (was_new, evicted) = loop {
        // The guest namespace is this node's to mint in (ADR-425): a full link claiming a
        // name in it — an old node started under one — would share a namespace with the
        // guests, so it is refused, and every name in it here is a guest link's.
        if link.guest.is_none() && guest::is_guest_name(peer) {
            eprintln!(
                "dist: refused a peer claiming the guest name {}: that namespace is a host's \
                 own, for its guest links",
                value::symbol_name(peer)
            );
            return None;
        }
        let mut nodes = crate::core::sync::write(&NODES);
        // A guest link's name was minted for it alone, so no link is ever registered under
        // it before: a guest is never the duplicate or the restart of anything (KI-287).
        debug_assert!(
            link.guest.is_none() || !nodes.contains_key(&peer),
            "a guest link's minted name was already registered"
        );
        match nodes.get(&peer) {
            Some(existing) if existing.incarnation != link.incarnation => {
                // The peer restarted. Retire the previous link OUTSIDE the
                // lock — its node-down delivers to monitors and sweeps the process
                // tables — and with `NODES` empty for this name meanwhile, so a
                // monitor or link made in that window fails over to `:noconnection`
                // rather than being recorded against a link the sweep then claims.
                // Then decide again: another link of the new life may have registered.
                let previous = nodes.remove(&peer);
                drop(nodes);
                if let Some(previous) = previous {
                    retire_previous_incarnation(peer, previous);
                }
                continue;
            }
            Some(existing)
                if value::symbol_name(link.connector) >= value::symbol_name(existing.connector) =>
            {
                // The existing link to this same life of the peer wins (its connector
                // sorts first, or it's the same initiator — a plain duplicate).
                return None;
            }
            other => {
                // We win (or there was no existing link). Take over the slot; any
                // evicted link is torn down below, outside the lock.
                let was_new = other.is_none();
                let old = nodes.remove(&peer);
                nodes.insert(peer, link);
                break (was_new, old);
            }
        }
    };
    if let Some(old) = evicted {
        // Its reader unblocks, and its `drop_link` no-ops on the new id.
        let _ = old.sock.shutdown(Shutdown::Both);
        // Whatever was queued on the evicted link dies with its writer, so nothing else
        // would ever resolve the couplings made over it: sever them now, before this
        // link's reader starts, so nothing it carries is swept. No `[:nodedown]`: the
        // node is still connected, over the winner.
        sever_couplings(peer);
    }
    Some(was_new)
}

/// Remove a link from `NODES` **iff** it's still this generation (so an evicted or
/// replaced link can't tear down its successor), and fire node-down watchers.
pub(super) fn drop_link(peer: Symbol, id: u64) {
    let removed = remove_current_link(peer, id);
    if removed {
        forget_dial_routes(peer);
        fire_nodedown(peer);
    }
}

/// Remove `peer`'s `NODES` entry iff it is still generation `id`; `true` when this call
/// removed it, so the caller is the one that fires its node-down.
pub(super) fn remove_current_link(peer: Symbol, id: u64) -> bool {
    let mut nodes = crate::core::sync::write(&NODES);
    match nodes.get(&peer) {
        Some(c) if c.id == id => {
            nodes.remove(&peer);
            true
        }
        _ => false,
    }
}

/// Tear down `previous`, a link to an earlier life of `peer` that `establish` has
/// already removed from `NODES` because the peer came back with a new incarnation.
///
/// This is a real node-down, not a duplicate eviction: the process at the far end of
/// `previous` is gone, so its watchers get `[:nodedown]`, every monitor and link made
/// over it gets its `:noconnection`, and the watchers it held on our pids are dropped
/// — exactly what the heartbeat would have done once it expired the link, only now
/// rather than `DOWN_AFTER` later. The link's own reader then hits the shutdown and
/// runs `drop_link`, which no-ops on the generation id.
pub(super) fn retire_previous_incarnation(peer: Symbol, previous: Conn) {
    let _ = previous.sock.shutdown(Shutdown::Both);
    forget_dial_routes(peer);
    fire_nodedown(peer);
}

/// Deliver `[:nodedown name]` to every process that called `(monitor-node name)`,
/// and fire any pid-monitors that crossed this link — pending remote monitors
/// fire `:noconnection` to their local watchers, and inbound remote watchers
/// the peer had registered are dropped (no point keeping entries that route
/// to a vanished peer). All three sit behind one node-down trigger so a
/// reconnect later starts from a clean slate.
pub(super) fn fire_nodedown(peer: Symbol) {
    let watchers = crate::core::sync::read(&NODE_MONITORS).get(&peer).cloned();
    if let Some(watchers) = watchers {
        let msg = nodedown_msg(peer);
        for w in watchers {
            process::deliver(w, msg.clone());
        }
    }
    sever_couplings(peer);
}

/// The net-split cleanup for every coupling made over a link to `peer` that is gone:
/// each pending remote monitor fires `:noconnection`, as does each cross-node link
/// (ADR-067), and the watchers `peer` held on our pids are dropped. Each entry is claimed,
/// so it fires once. A node-down runs it after `[:nodedown]`; an evicted duplicate link
/// runs it alone, since the node is still up over the winner.
pub(super) fn sever_couplings(peer: Symbol) {
    process::handle_node_down(peer);
    process::handle_link_node_down(peer);
}
