//! Cluster-mesh gossip (ADR-088): whether to mesh at all, the address this node
//! advertises for others to dial, broadcasting the peer table on a join, and dialing the
//! peers an inbound gossip list names that this node is not yet linked to.

use super::*;

/// Whether to form a transitive **cluster mesh** (ADR-088): when set (the
/// default), connecting to one node auto-connects you to
/// every node it knows. Set `BROOD_NO_MESH=1` for point-to-point links only
/// (you connect to exactly the nodes you dial, no transitive discovery). Read
/// once at first use.
pub(super) static MESH_ENABLED: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("BROOD_NO_MESH").is_none());

pub(super) fn mesh_enabled() -> bool {
    *MESH_ENABLED
}

/// The address a peer should advertise as "dial this to reach me" — the first
/// TCP listener if any (reachable from other machines *and* same-machine over
/// loopback), else the first listener (a local Unix socket), else empty (not
/// listening; peers then can't gossip us onward). Preferring TCP means a
/// dual-listen node (local Unix + remote TCP) advertises the address that works
/// from anywhere.
pub(super) fn advertised_addr() -> String {
    let addrs = crate::core::sync::read(&LISTEN_ADDRS);
    addrs
        .iter()
        .find(|a| a.starts_with("tcp:"))
        .or_else(|| addrs.first())
        .cloned()
        .unwrap_or_default()
}

/// Send every connected peer the current peer table (each *other* peer's name +
/// dial address), so newcomers and incumbents converge to a full mesh. Idempotent:
/// a recipient ignores any entry it's already connected to, so re-broadcasting on
/// each join can't loop. Entries with no advertised address are skipped — a peer
/// that isn't listening can't be dialed onward.
pub(super) fn broadcast_peer_table() {
    // Snapshot the peer table (cheap: Arc/channel clones) and release the
    // NODES lock before encoding or enqueueing. `enqueue` calls
    // `sock.shutdown()` when the writer queue is full — that syscall must not
    // run while holding NODES or it delays concurrent link registration and
    // teardown for the duration of every shutdown it triggers.
    struct PeerSnap {
        name: Symbol,
        addr: String,
        tx: SyncSender<Outbound>,
        sock: Arc<Stream>,
    }
    let snaps: Vec<PeerSnap> = {
        let nodes = crate::core::sync::read(&NODES);
        nodes
            .iter()
            // a guest is neither told about the cluster nor introduced to it (ADR-415)
            .filter(|(_, conn)| conn.guest.is_none())
            .map(|(&name, c)| PeerSnap {
                name,
                addr: c.addr.clone(),
                tx: c.tx.clone(),
                sock: Arc::clone(&c.sock),
            })
            .collect()
    };
    for s in &snaps {
        let peers: Vec<(Symbol, String)> = snaps
            .iter()
            .filter(|p| p.name != s.name && !p.addr.is_empty())
            .map(|p| (p.name, p.addr.clone()))
            .collect();
        if peers.is_empty() {
            continue;
        }
        if let Ok(bytes) = encode_payload(&Frame::Peers { peers })
            && s.tx.try_send(Arc::new(bytes).into()).is_err()
        {
            let _ = s.sock.shutdown(Shutdown::Both);
        }
    }
}

/// Handle an inbound gossip list: dial any named peer we're neither connected to
/// nor already dialing. Each dial runs on its own short-lived thread (the dial +
/// handshake blocks, and must not stall the reader). The `PENDING_DIALS` claim
/// makes concurrent gossip frames naming the same peer dial it once; the dialed
/// link's own `establish` re-gossips, so the mesh closes transitively.
pub(super) fn mesh_consider(peers: Vec<(Symbol, String)>) {
    if !mesh_enabled() {
        return;
    }
    {
        let mut dials = crate::core::sync::lock(&PROGRAM_DIALS);
        if dials.under_way > 0 {
            dials.held_gossip.extend(peers);
            return;
        }
    }
    let me = local_node();
    // Snapshot who we're already linked to (a different lock than PENDING_DIALS;
    // take it first and drop it before claiming, so the two are never held nested).
    // No gossiped name is a guest's here: a guest's name is minted by its host, in a
    // namespace a full peer may not use (ADR-425), and no host gossips its guests — so a
    // gossiped name in that namespace is no node to dial, and is skipped.
    let connected: HashSet<Symbol> = crate::core::sync::read(&NODES).keys().copied().collect();
    let mut to_dial: Vec<(Symbol, String)> = Vec::new();
    {
        let mut pending = crate::core::sync::write(&PENDING_DIALS);
        for (name, addr) in peers {
            if name == me
                || addr.is_empty()
                || guest::is_guest_name(name)
                || connected.contains(&name)
                || pending.contains(&name)
            {
                continue;
            }
            pending.insert(name);
            to_dial.push((name, addr));
        }
    }
    for (name, addr) in to_dial {
        // One thread per gossiped peer, and a `Peers` frame may name up to
        // MAX_GOSSIP_PEERS of them — the highest-rate, most attacker-influenced spawn in
        // the runtime. A refusal must drop this dial, not unwind the caller.
        let pending_name = name;
        if !spawn_bg("dist-dial", move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Best-effort: a peer may be unreachable or a simultaneous dial
                // from the other side may win the tie-break — either way we just
                // stop. `connect_to` re-checks `NODES` and handles the self/dup
                // guards; `establish` does the tie-break.
                let _ = connect_to(name, &addr);
            }));
            crate::core::sync::write(&PENDING_DIALS).remove(&name);
        }) {
            // The dial never started, so nothing will clear the in-flight marker we
            // inserted above; drop it here or this peer could never be dialed again.
            crate::core::sync::write(&PENDING_DIALS).remove(&pending_name);
        }
    }
}
