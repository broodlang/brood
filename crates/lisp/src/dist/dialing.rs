//! The dialing side of the connection lifecycle: `node/connect`, the client handshake,
//! and the dial bookkeeping that keeps one node from making two dials to a peer — the
//! address-to-node routes a pre-dial check answers from, the pending-dial claims, and the
//! gossip held while a program's dial is under way (KI-372).

use super::*;

/// Dial address (`"unix:PATH"` / `"tcp:HOST:PORT"`, as `node_connect` was given it)
/// → the authenticated name of the node a handshake over it reached.
///
/// The name a caller hands `node_connect` is only a *claim* — `(node/connect "ed")`
/// claims `ed@<this host>`, while the node listening on that socket may call itself
/// `ed@127.0.0.1` — so checking `NODES` by the claimed name alone misses a live link
/// spelled differently and dials it again. The address is what actually identifies
/// the listener, so this lets the pre-dial check answer a second connect to a live
/// peer from the existing link whatever name it was spelled with. Entries are
/// dropped with their peer's link (`forget_dial_routes`), and a lookup is only
/// trusted while `NODES` still holds the name.
pub(super) static DIAL_ROUTES: LazyLock<RwLock<HashMap<String, Symbol>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Peer node-names we're *currently dialing*, by gossip (ADR-088) or by a program
/// ([`DialClaim`]). Entered before a dial starts and cleared when it finishes, so one
/// node never has two dials to a peer under way. A peer already in `NODES` is never
/// re-dialed.
pub(super) static PENDING_DIALS: LazyLock<RwLock<HashSet<Symbol>>> =
    LazyLock::new(|| RwLock::new(HashSet::new()));

/// The longest a dial can take: its connect, and a handshake and the older-version retry.
pub(super) const DIAL_CEILING: Duration =
    Duration::from_secs(CONNECT_TIMEOUT.as_secs() + 2 * HANDSHAKE_TIMEOUT.as_secs());

/// A program's claim on dialing one peer. [`ProgramDial`] holds back gossip that arrives
/// during a dial, but not a dial gossip already started, nor another program's dial: this
/// waits those out, so the second dial finds the first one's link and makes none (KI-372).
/// Nothing is left to wait for past [`DIAL_CEILING`], so a claim that outlives it, which
/// nothing here should, is dialed through rather than hung on.
pub(super) struct DialClaim(Option<Symbol>);

impl DialClaim {
    fn wait_for(peer: Symbol) -> Self {
        let deadline = Instant::now() + DIAL_CEILING;
        loop {
            if crate::core::sync::write(&PENDING_DIALS).insert(peer) {
                return DialClaim(Some(peer));
            }
            if Instant::now() >= deadline {
                return DialClaim(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for DialClaim {
    fn drop(&mut self) {
        if let Some(peer) = self.0 {
            crate::core::sync::write(&PENDING_DIALS).remove(&peer);
        }
    }
}

/// The dials a program has under way, and the gossip held until they finish
/// ([`ProgramDial`]).
#[derive(Default)]
pub(super) struct ProgramDials {
    pub(super) under_way: usize,
    pub(super) held_gossip: HashMap<Symbol, String>,
}

pub(super) static PROGRAM_DIALS: LazyLock<Mutex<ProgramDials>> = LazyLock::new(Mutex::default);

/// One `node/connect` under way. While any is, gossip is held rather than considered: the
/// dial's link is not registered until its handshake ends, so gossip naming that same peer
/// would start a second dial from this node at the same time. Two links one node opens at
/// once can be opened in opposite orders on the two ends, so the opening order that decides
/// between them (`existing_link_wins`) has each end close the one the other kept, both die,
/// and the mesh does not re-heal (KI-372). Dropping the last one considers what
/// was held, by when the peer is in `NODES`.
pub(super) struct ProgramDial;

impl ProgramDial {
    pub(super) fn begin() -> Self {
        crate::core::sync::lock(&PROGRAM_DIALS).under_way += 1;
        ProgramDial
    }
}

impl Drop for ProgramDial {
    fn drop(&mut self) {
        let held = {
            let mut dials = crate::core::sync::lock(&PROGRAM_DIALS);
            dials.under_way -= 1;
            if dials.under_way == 0 {
                dials.held_gossip.drain().collect()
            } else {
                Vec::new()
            }
        };
        if !held.is_empty() {
            mesh_consider(held);
        }
    }
}

/// The protocol version this node offers in a handshake (ADR-492): v11, or exactly v10
/// under `BROOD_DIST_V10` — a runtime from before lanes, which a mixed-cluster test
/// needs and cannot otherwise build.
pub(super) fn local_offer() -> Offer {
    static OFFER: LazyLock<Offer> = LazyLock::new(|| {
        if std::env::var_os("BROOD_DIST_V10").is_some() {
            Offer::V10
        } else {
            Offer::Current
        }
    });
    *OFFER
}

/// Dial `addr` and drive the client handshake, offering `offer`.
pub(super) fn dial_and_greet(
    addr: &str,
    offer: Offer,
) -> io::Result<(Stream, handshake::Authenticated)> {
    let mut stream = dial(addr)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    // Deadlined like the accept side: a malicious or wedged *listener* can
    // trickle at a dialer just as easily, and this call is made from a scheduler
    // worker, so an unbounded hold here wedges a worker rather than a slot.
    let authenticated = {
        let mut guarded = Deadline::new(&mut stream, HANDSHAKE_DEADLINE);
        handshake(&mut guarded, Role::Initiator, offer)?
    };
    Ok((stream, authenticated))
}

/// `(%node-connect peer addr)` — dial a peer and complete the client handshake.
/// `addr` carries the transport (`"unix:PATH"` / `"tcp:HOST:PORT"`); `peer` is
/// the name we expect (used for the self-dial guard + de-dup, before the
/// handshake reveals the peer's authoritative name). Uses this runtime's
/// already-published identity (the prelude `connect` requires a prior
/// `node-start`). Returns the peer's authoritative node name on success.
pub(crate) fn node_connect(peer: Symbol, addr: &str) -> io::Result<Symbol> {
    let _dial = ProgramDial::begin();
    let _claim = DialClaim::wait_for(peer);
    connect_to(peer, addr)
}

/// [`node_connect`] for a dial gossip started, which holds no gossip back.
pub(super) fn connect_to(peer: Symbol, addr: &str) -> io::Result<Symbol> {
    // Refuse to dial ourselves — it would race through the handshake and form a
    // tie-break loser in the same process; cleaner to reject up front.
    if peer == local_node() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("cannot connect to self ({})", value::symbol_name(peer)),
        ));
    }
    // Pre-dial de-dup: if we already have a link to this node — under the claimed
    // name, or reached before through this very address under whatever name the
    // handshake gave it — reuse it without dialing. A second dial to a live peer is
    // not harmless: it costs a handshake and leaves `establish` a same-incarnation
    // duplicate to resolve. The claim may still be stale/wrong (e.g. gossip lag, a
    // never-dialed spelling), so `establish` re-checks under the authenticated name.
    if let Some(linked) = linked_peer_for(peer, addr) {
        return Ok(linked);
    }
    // A v10 node answers a v11 offer with its own magic and closes: dial it again in v10,
    // so a node from before lanes still links (ADR-492).
    let (stream, authenticated) = match dial_and_greet(addr, local_offer()) {
        Err(error) if handshake::is_v10_peer(&error) => dial_and_greet(addr, Offer::V10)?,
        greeted => greeted?,
    };
    let peer = authenticated.peer;
    // The claim can name another node while the address reaches us (a second
    // spelling of our own name, a relay looping back): the accept side refuses a
    // peer claiming to be us, and so must the dial side, or `NODES` gains an entry
    // for ourselves.
    if peer == local_node() {
        let _ = stream.shutdown(Shutdown::Both);
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "cannot connect to self ({addr} is this node, {})",
                value::symbol_name(peer)
            ),
        ));
    }
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    // Always pass to `establish` — even when we already have a link under the
    // authenticated name. `establish` has its own tie-break: two nodes dialing each
    // other compare connectors by name and reach the same decision, the losing side
    // closing its own socket; links from ONE connector tie, which [`DialClaim`] keeps
    // from arising. A short-circuit `stream.shutdown` here would skip the tie-break on our end
    // while the peer still runs `establish` on theirs — they might win, register
    // our doomed socket, and later fire a spurious `[:nodedown]` when the reader
    // hits the EOF our shutdown sent.
    establish(authenticated, stream, Role::Initiator);
    // Whether our link won or an existing one did, `addr` reaches `peer`.
    crate::core::sync::write(&DIAL_ROUTES).insert(addr.to_string(), peer);
    Ok(peer)
}

/// The connected node a dial of `addr` claiming `claimed` would reach, if we can
/// tell without dialing: `claimed` itself when linked; else the node an earlier dial
/// of the same address authenticated as (`DIAL_ROUTES`); else the node that
/// advertises `addr` as its dial address. Each answer is trusted only while that
/// node is still in `NODES`.
///
/// Only a FULL link answers (KI-287). A guest's advertised address is its own claim, so
/// a guest link is never the link a dial was looking for, even when it advertises the
/// dialed address: the dial goes ahead. (Its name cannot match a dial's claim — it is
/// one this node minted, ADR-425.)
pub(super) fn linked_peer_for(claimed: Symbol, addr: &str) -> Option<Symbol> {
    // Lock order: NODES, then DIAL_ROUTES — nothing takes them the other way round.
    let nodes = crate::core::sync::read(&NODES);
    let full = |name: &Symbol| nodes.get(name).is_some_and(|link| link.guest.is_none());
    if full(&claimed) {
        return Some(claimed);
    }
    if let Some(&routed) = crate::core::sync::read(&DIAL_ROUTES).get(addr)
        && full(&routed)
    {
        return Some(routed);
    }
    nodes
        .iter()
        .find(|(_, link)| link.guest.is_none() && !link.addr.is_empty() && link.addr == addr)
        .map(|(&name, _)| name)
}

/// Drop every `DIAL_ROUTES` entry naming `peer` — its link is gone, so an address
/// that reached it no longer proves anything about what answers there now.
pub(super) fn forget_dial_routes(peer: Symbol) {
    crate::core::sync::write(&DIAL_ROUTES).retain(|_, routed| *routed != peer);
}
