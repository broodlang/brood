//! Unit tests of the node layer that need no second node: monitor and link setup racing a
//! dropped link, link registration (eviction, restarts, duplicates, guests), the dial
//! bookkeeping, the writer queue's `send-latest` slots, name registration, the pre-auth
//! handshake gates, and listener addresses.

use super::*;

thread_local! {
    static SETUP_GAP_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run, once, whatever a test armed for the gap between `monitor_remote` /
/// `link_remote` recording their entry and sending the frame — the window a link
/// can drop in.
pub(super) fn run_setup_gap_hook() {
    if let Some(hook) = SETUP_GAP_HOOK.with(|slot| slot.borrow_mut().take()) {
        hook();
    }
}

fn arm_setup_gap_hook(hook: impl FnOnce() + 'static) {
    SETUP_GAP_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

/// A link that drops between `monitor_remote` recording its pending entry and
/// sending the `Monitor` frame fires the watcher's `:noconnection` through the
/// node-down path; the send then fails, and the fallback must not fire it again.
#[test]
fn a_link_dropping_during_monitor_setup_fires_one_noconnection() {
    let me = process::self_pid();
    let node = value::intern("gone-during-monitor@test");
    let mref = process::next_ref();
    arm_setup_gap_hook(move || fire_nodedown(node));
    monitor_remote(node, 7, me, mref);
    assert_eq!(
        process::mailbox_len(me),
        Some(1),
        "one [:down … :noconnection] per monitor, not one from each path"
    );
    // Control: with no drop in the gap, the fallback alone fires it.
    monitor_remote(node, 8, me, process::next_ref());
    assert_eq!(process::mailbox_len(me), Some(2));
}

/// The link twin: a link dropping between `link_remote`'s record and its send
/// delivers one `[:EXIT … :noconnection]`, not two.
#[test]
fn a_link_dropping_during_link_setup_fires_one_noconnection() {
    let me = process::self_pid();
    process::set_trap_exit(me, true);
    let node = value::intern("gone-during-link@test");
    arm_setup_gap_hook(move || fire_nodedown(node));
    link_remote(node, 9, me);
    assert_eq!(
        process::mailbox_len(me),
        Some(1),
        "one [:EXIT … :noconnection] per link, not one from each path"
    );
    link_remote(node, 10, me);
    assert_eq!(process::mailbox_len(me), Some(2));
}

/// A remote DOWN already in flight when the watcher demonitors must not land after
/// the demonitor — the guarantee the local path makes (KI-213).
#[test]
fn a_remote_down_arriving_after_demonitor_is_dropped() {
    let me = process::self_pid();
    let node = value::intern("peer-for-demonitor@test");
    let reason = Message::Keyword(value::intern("normal"));
    let mref = process::next_ref();
    process::record_pending_remote(node, 11, me, mref);
    demonitor_remote(node, me, mref);
    process::deliver_remote_down(node, me, mref, 11, reason.clone());
    assert_eq!(
        process::mailbox_len(me),
        Some(0),
        "a DOWN for a demonitored ref was delivered"
    );
    // Control: a monitor still pending gets its DOWN.
    let pending = process::next_ref();
    process::record_pending_remote(node, 12, me, pending);
    process::deliver_remote_down(node, me, pending, 12, reason);
    assert_eq!(process::mailbox_len(me), Some(1));
}

/// A remote link exit already in flight when the local process unlinks must not
/// arrive after the unlink.
#[test]
fn a_remote_link_exit_arriving_after_unlink_is_dropped() {
    let me = process::self_pid();
    process::set_trap_exit(me, true);
    let node = value::intern("peer-for-unlink@test");
    let reason = Message::Keyword(value::intern("boom"));
    assert!(process::record_remote_link(me, node, 13));
    unlink_remote(node, 13, me);
    process::deliver_remote_link_exit(me, node, 13, reason.clone());
    assert_eq!(
        process::mailbox_len(me),
        Some(0),
        "a link exit for an unlinked peer was delivered"
    );
    // Control: a live link's exit arrives.
    assert!(process::record_remote_link(me, node, 14));
    process::deliver_remote_link_exit(me, node, 14, reason);
    assert_eq!(process::mailbox_len(me), Some(1));
}

/// A duplicate link that loses the tie-break to a newer one is evicted with its
/// writer queue — and with it any `Monitor` request still queued there. Its reader's
/// `drop_link` no-ops on the new id, so the eviction itself must resolve the
/// couplings made over it: a pending remote monitor gets its `:noconnection`.
#[test]
fn evicting_a_link_fires_noconnection_for_its_pending_monitors() {
    let me = process::self_pid();
    // This runtime never ran `node-start`, so it is `nonode`: a link WE dialed has
    // connector "nonode", one the peer dialed has the peer's name. "nonode" sorts
    // before "zz-…", so the second (Initiator) link evicts the first.
    let peer = value::intern("zz-evicted-peer@test");
    let (first, _first_far_end) = stream_pair();
    establish(authenticated_as(peer, 5), first, Role::Responder);
    // A monitor that went out over the first link.
    process::record_pending_remote(peer, 21, me, process::next_ref());
    let (second, _second_far_end) = stream_pair();
    establish(authenticated_as(peer, 5), second, Role::Initiator);
    assert_eq!(
        crate::core::sync::read(&NODES)
            .get(&peer)
            .map(|link| link.connector),
        Some(local_node()),
        "the newer, smaller-connector link must have won"
    );
    assert_eq!(
        process::mailbox_len(me),
        Some(1),
        "the evicted link's pending monitor must get its :noconnection"
    );
}

/// A connected link carrier and its far end, over loopback TCP: the transport every
/// platform has, so these tests run on Windows as on Unix.
fn stream_pair() -> (Stream, TcpStream) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (far, _) = listener.accept().unwrap();
    (Stream::Tcp(near), far)
}

/// What a completed handshake with `peer` (in its life `incarnation`) hands
/// `establish`, with a throwaway session.
fn authenticated_as(peer: Symbol, incarnation: u64) -> handshake::Authenticated {
    handshake::Authenticated {
        id: next_link_id(),
        peer,
        addr: String::new(),
        incarnation,
        grant: None,
        lanes: true,
        session: Session {
            send: session::SealKey::new([1; session::KEY_LEN]),
            recv: session::OpenKey::new([2; session::KEY_LEN]),
        },
    }
}

fn registered_link(peer: Symbol) -> Option<(u64, u64)> {
    crate::core::sync::read(&NODES)
        .get(&peer)
        .map(|link| (link.id, link.incarnation))
}

/// The same, for a GUEST link (ADR-415) authenticated with a grant that names nothing.
fn authenticated_as_guest(peer: Symbol, incarnation: u64) -> handshake::Authenticated {
    handshake::Authenticated {
        grant: Some(Arc::new(guest::Grant {
            cookie: "unit-test-guest-cookie".to_string(),
            names: HashSet::new(),
            as_name: None,
        })),
        ..authenticated_as(peer, incarnation)
    }
}

/// KI-372: gossip naming a peer while a program's dial is under way must not start a
/// second dial, or the two links can register in opposite orders on the two ends and
/// each end closes the one the other kept. It waits for the dial, and is then
/// considered, so a peer it names is still reached.
#[test]
fn gossip_waits_for_a_programs_dial_and_is_considered_after_it() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = format!("tcp:{}", listener.local_addr().unwrap());
    let dialed = || {
        (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            listener.accept().is_ok()
        })
    };
    let gossip = || mesh_consider(vec![(value::intern("gossiped@test"), addr.clone())]);

    let dial = ProgramDial::begin();
    gossip();
    assert!(!dialed(), "gossip dialed during a program's dial");
    drop(dial);
    assert!(dialed(), "the held gossip was never considered");
}

/// A program's dial to a peer gossip is already dialing waits for that dial, which is
/// what lets it find the link instead of making a second one. Gossip's claim stands in
/// for the dial; the address refuses, so the program's dial ends as soon as it starts.
#[test]
fn a_programs_dial_waits_for_a_dial_already_under_way() {
    let peer = value::intern("claimed@test");
    crate::core::sync::write(&PENDING_DIALS).insert(peer);
    let (finished, dial_ended) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = node_connect(peer, "tcp:127.0.0.1:1");
        finished.send(()).unwrap();
    });
    assert!(
        dial_ended.recv_timeout(Duration::from_millis(200)).is_err(),
        "the program dialed alongside the dial under way"
    );
    crate::core::sync::write(&PENDING_DIALS).remove(&peer);
    dial_ended.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(
        !crate::core::sync::read(&PENDING_DIALS).contains(&peer),
        "the claim outlived the dial"
    );
}

/// The guest standing of the link now registered for `peer`, if it is a guest link.
fn registered_guest(peer: Symbol) -> Option<Arc<guest::GuestLink>> {
    crate::core::sync::read(&NODES)
        .get(&peer)
        .and_then(|link| link.guest.clone())
}

/// The minted name and standing of every guest link registered as having claimed to
/// be `claimed` (ADR-425), in no particular order.
fn guests_claiming(claimed: Symbol) -> Vec<(Symbol, Arc<guest::GuestLink>)> {
    crate::core::sync::read(&NODES)
        .iter()
        .filter_map(|(&name, link)| {
            link.guest
                .as_ref()
                .filter(|standing| standing.claimed() == claimed)
                .map(|standing| (name, Arc::clone(standing)))
        })
        .collect()
}

/// The one guest link that claimed `claimed`.
fn the_guest_claiming(claimed: Symbol) -> (Symbol, Arc<guest::GuestLink>) {
    let mut found = guests_claiming(claimed);
    assert_eq!(found.len(), 1, "one guest link claimed the name");
    found.pop().expect("one")
}

/// KI-286: a guest link's reader can still be delivering frames it read after its
/// socket was shut down — by `node/disconnect`, say — and after a full link came up
/// under the name the guest claimed. Those frames are the guest's. When the filter
/// was keyed by name, a full link taking the name emptied the guest's entry and they
/// were admitted as a full peer's — a `:remote-spawn` carrying a closure among them.
/// The reader hands each frame to `handle_inbound_frame` with ITS link's standing;
/// that must refuse it whoever holds which name, while the same frame from a full link
/// is delivered.
#[test]
fn a_guest_links_frames_stay_a_guests_whoever_holds_its_claimed_name() {
    let me = process::self_pid();
    let peer = value::intern("reclaimed-guest-peer@test");
    let probe = value::intern("ki-286-probe@test");
    assert!(matches!(register(probe, me), Registration::Bound));
    let to_probe = || Frame::Send {
        target: Target::Name(probe),
        msg: Message::Keyword(value::intern("ki-286")),
        lane: crate::process::Lane::Normal,
    };

    let (guest_stream, _guest_far_end) = stream_pair();
    establish(
        authenticated_as_guest(peer, 1),
        guest_stream,
        Role::Responder,
    );
    let (guest_name, guest_standing) = the_guest_claiming(peer);

    // A full link under the name the guest claimed: a different link, under a
    // different name, so the guest's is untouched (ADR-425).
    let (full_stream, _full_far_end) = stream_pair();
    establish(authenticated_as(peer, 2), full_stream, Role::Responder);
    assert!(
        registered_link(peer).is_some() && registered_guest(peer).is_none(),
        "the claimed name is the full link's"
    );
    handle_inbound_frame(guest_name, Some(&guest_standing), to_probe());
    assert_eq!(
        process::mailbox_len(me),
        Some(0),
        "a guest link's frame was admitted with a full link's privileges"
    );

    // And once the guest link is gone, with its reader still draining.
    assert!(disconnect(guest_name));
    handle_inbound_frame(guest_name, Some(&guest_standing), to_probe());
    assert_eq!(
        process::mailbox_len(me),
        Some(0),
        "a guest link's frame was admitted once it had no link"
    );

    // The control: the same frame from a full link is delivered.
    handle_inbound_frame(peer, None, to_probe());
    assert_eq!(process::mailbox_len(me), Some(1));
    assert!(unregister(probe));
    assert!(disconnect(peer));
}

/// KI-286's other half: a local pid sent to a peer becomes reachable by the LINK the
/// message went out on. A pid sent to the full peer whose name the guest claimed went
/// to that full link, so the guest link must not be able to reach it.
#[test]
fn a_pid_sent_to_a_peer_is_reachable_only_by_the_link_it_went_out_on() {
    let peer = value::intern("introduced-guest-peer@test");
    let local_pid = |id| Message::Pid { node: *NONODE, id };
    let to_pid = |id| Frame::Send {
        target: Target::Pid(id),
        msg: Message::Keyword(value::intern("ki-286")),
        lane: crate::process::Lane::Normal,
    };

    let (guest_stream, _guest_far_end) = stream_pair();
    establish(
        authenticated_as_guest(peer, 1),
        guest_stream,
        Role::Responder,
    );
    let (guest_name, guest_standing) = the_guest_claiming(peer);
    assert!(route(guest_name, Target::Pid(1), local_pid(77)));
    assert!(
        guest::admit(guest_name, Some(&guest_standing), &to_pid(77)),
        "a pid sent over the guest link is reachable by it"
    );

    let (full_stream, _full_far_end) = stream_pair();
    establish(authenticated_as(peer, 2), full_stream, Role::Responder);
    assert!(route(peer, Target::Pid(1), local_pid(78)));
    assert!(
        !guest::admit(guest_name, Some(&guest_standing), &to_pid(78)),
        "a pid sent over the full link under the guest's claimed name reached the guest"
    );
    assert!(disconnect(peer));
    assert!(disconnect(guest_name));
}

/// KI-287 / ADR-425: a guest's node name is only its claim, so it is never the name
/// it is registered under — not a connected full peer's (whose link it used to
/// retire, taking its traffic and the pids in it), not a full peer's that is down,
/// and not one that is a full peer's somewhere in the cluster and has NEVER linked
/// here, which no check on this node could have known to refuse. Each gets a name
/// minted in the guest namespace, and a pid of the real peer routes to no guest.
#[test]
fn a_guest_is_never_registered_under_the_name_it_claims() {
    let me = process::self_pid();
    let peer = value::intern("full-peer-name-taken@test");
    let (full_stream, _full_far_end) = stream_pair();
    establish(authenticated_as(peer, 1), full_stream, Role::Responder);
    let full_link = registered_link(peer).expect("the full link registers");
    monitor_node(peer, me);

    let (guest_stream, _guest_far_end) = stream_pair();
    establish(
        authenticated_as_guest(peer, 2),
        guest_stream,
        Role::Responder,
    );
    assert_eq!(
        registered_link(peer),
        Some(full_link),
        "a guest claiming a full peer's name displaced its link"
    );
    assert!(registered_guest(peer).is_none());
    assert_eq!(
        process::mailbox_len(me),
        Some(0),
        "the full peer never went down"
    );
    let (guest_name, _) = the_guest_claiming(peer);
    assert!(guest::is_guest_name(guest_name) && guest_name != peer);

    // The full peer goes away; a guest claiming its name still does not get it.
    assert!(disconnect(peer));
    assert_eq!(process::mailbox_len(me), Some(1), "its [:nodedown]");
    demonitor_node(peer, me);
    let (late_guest_stream, _late_guest_far_end) = stream_pair();
    establish(
        authenticated_as_guest(peer, 3),
        late_guest_stream,
        Role::Responder,
    );
    assert_eq!(
        registered_link(peer),
        None,
        "a guest took the name of a full peer that was down"
    );

    // The case KI-287 left open: a full peer elsewhere in the cluster that has never
    // linked here. A third node forwarding one of its pids here must not reach the
    // guest that claimed its name: nothing is registered under it, so the send has no
    // route (`:noconnection` to a sender that asked) rather than going to the guest.
    let never_linked = value::intern("never-linked-full-peer@elsewhere");
    let (stranger_stream, _stranger_far_end) = stream_pair();
    establish(
        authenticated_as_guest(never_linked, 4),
        stranger_stream,
        Role::Responder,
    );
    assert_eq!(
        registered_link(never_linked),
        None,
        "a guest took the name of a full peer that has never linked here"
    );
    assert!(
        !route(
            never_linked,
            Target::Pid(5),
            Message::Keyword(value::intern("for-the-real-peer"))
        ),
        "a pid of a never-linked full peer routed to the guest that claimed its name"
    );
    let (stranger_name, _) = the_guest_claiming(never_linked);
    for (name, _) in guests_claiming(peer) {
        assert!(disconnect(name));
    }
    assert!(disconnect(stranger_name));
}

/// Two guests claiming one name are two links under two names — neither the
/// duplicate nor the restart of the other — and a full link under the name they
/// claimed is a third, displacing neither; a dial never takes a guest's link for the
/// peer it was looking for, whether by name, by an earlier route or by the address a
/// guest advertised.
#[test]
fn guests_and_a_full_link_claiming_one_name_are_separate_links() {
    let peer = value::intern("guest-held-name@test");
    let address = "unix:/nonexistent/brood-test/guest-held-name.sock";
    let advertising = |incarnation| handshake::Authenticated {
        addr: address.to_string(),
        ..authenticated_as_guest(peer, incarnation)
    };
    let (first_stream, _first_far_end) = stream_pair();
    establish(advertising(5), first_stream, Role::Responder);
    let (second_stream, _second_far_end) = stream_pair();
    establish(advertising(6), second_stream, Role::Responder);
    let guests = guests_claiming(peer);
    assert_eq!(
        guests.len(),
        2,
        "a second guest claiming a name retired the first, or was refused"
    );
    assert_ne!(guests[0].0, guests[1].0);

    crate::core::sync::write(&DIAL_ROUTES).insert(address.to_string(), guests[0].0);
    assert_eq!(
        linked_peer_for(peer, address),
        None,
        "a dial reused a guest's link as the peer it was looking for"
    );

    let (full_stream, _full_far_end) = stream_pair();
    establish(authenticated_as(peer, 5), full_stream, Role::Responder);
    assert!(
        registered_link(peer).is_some() && registered_guest(peer).is_none(),
        "the full link registers under its own name"
    );
    assert_eq!(
        guests_claiming(peer).len(),
        2,
        "a full link displaced a guest that had claimed its name"
    );
    assert!(disconnect(peer));
    for (name, _) in guests {
        assert!(disconnect(name));
    }
    crate::core::sync::write(&DIAL_ROUTES).remove(address);
}

/// The guest namespace is the host's alone: a full link claiming a name in it is
/// refused, and `node/start` will not take one, so every name in it is a guest link
/// some host minted (ADR-425).
#[test]
fn no_full_peer_may_use_the_guest_namespace() {
    let squatter = value::intern("guest~0.0000000000000000~squatter@test");
    let (stream, _far_end) = stream_pair();
    establish(authenticated_as(squatter, 1), stream, Role::Responder);
    assert_eq!(
        registered_link(squatter),
        None,
        "a full link registered under a name in the guest namespace"
    );
    let refused = node_listen(
        value::intern("guest~started-by-hand@test"),
        "tcp:127.0.0.1:0",
        "a-long-enough-cookie-for-the-test".to_string(),
    );
    assert_eq!(
        refused.map_err(|error| error.kind()),
        Err(io::ErrorKind::InvalidInput),
        "node/start took a name in the guest namespace"
    );
}

/// What goes out over a guest link names the guest's pids the way the guest spells
/// itself (ADR-425): read off the far end of the socket, a pid of the minted name is a
/// pid of the claimed one, so the guest's own processes are still its own.
#[test]
fn a_guest_receives_its_own_pids_in_its_own_spelling() {
    let peer = value::intern("spelled-by-itself@test");
    let (guest_stream, guest_far_end) = stream_pair();
    establish(
        authenticated_as_guest(peer, 1),
        guest_stream,
        Role::Responder,
    );
    let (guest_name, _) = the_guest_claiming(peer);
    assert!(route(
        guest_name,
        Target::Pid(3),
        Message::Vector(vec![Message::Pid {
            node: guest_name,
            id: 4
        }]),
    ));
    guest_far_end
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    // The far end opens what the link's writer sealed (`authenticated_as`'s send key).
    let mut open = session::OpenKey::new([1; session::KEY_LEN]);
    let mut reader = std::io::BufReader::new(&guest_far_end);
    let msg = loop {
        match open.open(&mut reader).expect("the routed frame arrives") {
            Frame::Send { msg, .. } => break msg,
            _ => continue, // a heartbeat ping
        }
    };
    let Message::Vector(ref items) = msg else {
        panic!("the message arrives as sent")
    };
    assert!(
        matches!(items[0], Message::Pid { node, id: 4 } if node == peer),
        "a guest was sent its own pid under the name the host minted for it"
    );
    assert!(disconnect(guest_name));
}

/// A lane reaches the wire only on a link to a v11 peer (ADR-492): to a v10 peer, a
/// priority and a latest send each go as the plain `Send` it knows, and to a v11 peer
/// both keep their lane.
#[test]
fn a_lane_goes_on_the_wire_only_to_a_v11_peer() {
    let lanes_seen = |peer: &str, lanes: bool| {
        let peer = value::intern(peer);
        let (stream, far_end) = stream_pair();
        establish(
            handshake::Authenticated {
                lanes,
                ..authenticated_as(peer, 1)
            },
            stream,
            Role::Responder,
        );
        let latest = process::Lane::Latest(Box::new(":frame".to_string()));
        for lane in [process::Lane::Priority, latest] {
            assert!(route_in_lane(peer, Target::Pid(3), Message::Int(1), lane));
        }
        far_end
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut open = session::OpenKey::new([1; session::KEY_LEN]);
        let mut reader = std::io::BufReader::new(&far_end);
        let mut seen = Vec::new();
        while seen.len() < 2 {
            if let Frame::Send { lane, .. } = open.open(&mut reader).expect("a frame") {
                seen.push(lane);
            }
        }
        assert!(disconnect(peer));
        seen
    };
    assert_eq!(
        lanes_seen("v10-lane-peer@test", false),
        vec![process::Lane::Normal, process::Lane::Normal]
    );
    assert_eq!(
        lanes_seen("v11-lane-peer@test", true),
        vec![
            process::Lane::Priority,
            process::Lane::Latest(Box::new(":frame".to_string()))
        ]
    );
}

/// A `send-latest` replaces its slot's frame in a link's writer queue while it is
/// unwritten, its replacement written after everything queued before it; another
/// target's slot, a plain frame, and a slot already written are left alone (ADR-492).
#[test]
fn a_send_latest_replaces_its_unwritten_frame_in_the_link_queue() {
    let (tx, rx) = mpsc::sync_channel::<Outbound>(16);
    let (stream, _far_end) = stream_pair();
    let sock = Arc::new(stream);
    let latest = LatestSlots::default();
    let payload = |byte: u8| Arc::new(vec![byte]);
    let frame_slot = latest_slot(&Target::Pid(1), ":frame");
    let other_slot = latest_slot(&Target::Pid(2), ":frame");
    assert!(enqueue_latest(
        &tx,
        &sock,
        &latest,
        frame_slot.clone(),
        payload(1)
    ));
    assert!(enqueue_to(&tx, &sock, payload(2)));
    assert!(enqueue_latest(&tx, &sock, &latest, other_slot, payload(3)));
    assert!(enqueue_latest(
        &tx,
        &sock,
        &latest,
        frame_slot.clone(),
        payload(4)
    ));
    let written = |count: usize| -> Vec<u8> {
        rx.try_iter()
            .take(count)
            .filter_map(|item| claim_outbound(&latest, item))
            .map(|bytes| bytes[0])
            .collect()
    };
    assert_eq!(written(4), vec![2, 3, 4]);
    assert!(crate::core::sync::lock(&latest).is_empty());
    // Written, the slot is free: the next one is queued and written in its turn.
    assert!(enqueue_latest(&tx, &sock, &latest, frame_slot, payload(5)));
    assert_eq!(written(1), vec![5]);
}

/// A peer that restarted (its old host vanished without a FIN, so the old link is
/// still registered) and dials us again must get its new link — the old one reaches
/// a process that no longer exists. The incarnation is what tells the two lives
/// apart (same connector), so the new link is not refused as a duplicate; the old
/// life's watchers get their node-down.
#[test]
fn a_restarted_peer_replaces_its_previous_life_and_fires_its_node_down() {
    let me = process::self_pid();
    let peer = value::intern("restarted-peer@test");
    let (old_life, _old_far_end) = stream_pair();
    establish(authenticated_as(peer, 1), old_life, Role::Responder);
    let (old_id, _) = registered_link(peer).expect("the first life registers");
    monitor_node(peer, me);
    // A pid monitor made over the old life.
    process::record_pending_remote(peer, 31, me, process::next_ref());
    assert_eq!(
        process::mailbox_len(me),
        Some(0),
        "nothing fires while it is up"
    );

    // The peer comes back and dials us: the same connector (the peer) as before.
    let (new_life, _new_far_end) = stream_pair();
    establish(authenticated_as(peer, 2), new_life, Role::Responder);
    let (new_id, new_incarnation) = registered_link(peer).expect("the new life registers");
    assert_ne!(
        new_id, old_id,
        "the restarted peer's link must replace the old one"
    );
    assert_eq!(new_incarnation, 2);
    assert_eq!(
        process::mailbox_len(me),
        Some(2),
        "the old life's node monitor gets [:nodedown] and its pid monitor :noconnection"
    );
    demonitor_node(peer, me);
}

/// The other half of the same decision: a second link to the SAME life of a live
/// peer — the shape two dials racing can produce — must not disturb the link that is
/// up. Same connector, same incarnation: the registered link stays, and no monitor
/// hears anything.
#[test]
fn a_second_link_to_the_same_life_leaves_the_live_link_alone() {
    let me = process::self_pid();
    let peer = value::intern("same-life-peer@test");
    let (first, _first_far_end) = stream_pair();
    establish(authenticated_as(peer, 7), first, Role::Responder);
    let registered = registered_link(peer).expect("the first link registers");
    monitor_node(peer, me);
    let mref = process::next_ref();
    process::record_pending_remote(peer, 41, me, mref);

    let (second, _second_far_end) = stream_pair();
    establish(authenticated_as(peer, 7), second, Role::Responder);
    assert_eq!(
        registered_link(peer),
        Some(registered),
        "a duplicate of a live link must lose to it"
    );
    assert_eq!(
        process::mailbox_len(me),
        Some(0),
        "no [:nodedown] and no :noconnection for a peer that never went away"
    );
    demonitor_node(peer, me);
    process::drop_pending_remote(peer, me, mref);
}

/// KI-381: one connector's two links to the same life, REGISTERED in the opposite order
/// to the one they were opened in — a responder whose thread for the first link was
/// descheduled between its handshake and its registration. The dialer keeps the link it
/// opened first, so this end must too, or each end closes the link the other kept.
#[test]
fn one_connectors_two_links_keep_the_one_opened_first_whatever_registers_first() {
    let peer = value::intern("opened-first-peer@test");
    let opened_first = authenticated_as(peer, 9);
    let opened_second = authenticated_as(peer, 9);
    let first_id = opened_first.id;
    assert!(first_id < opened_second.id);

    let (second, _second_far_end) = stream_pair();
    establish(opened_second, second, Role::Responder);
    let (first, _first_far_end) = stream_pair();
    establish(opened_first, first, Role::Responder);
    assert_eq!(
        registered_link(peer),
        Some((first_id, 9)),
        "the link opened first must win, though it registered second"
    );
}

/// A connect that names a live peer by another spelling — `(node/connect "ed")`
/// claims `ed@<this host>` for a node that calls itself `ed@127.0.0.1` — is
/// answered from the existing link reached through the same address, without
/// dialing. The address here is unreachable, so a dial would fail the call.
#[test]
fn a_connect_spelled_differently_reuses_the_link_its_address_reached() {
    let peer = value::intern("spelled-peer@127.0.0.1");
    let address = "unix:/nonexistent/brood-test/spelled-peer.sock";
    let (link, _far_end) = stream_pair();
    establish(authenticated_as(peer, 9), link, Role::Responder);
    crate::core::sync::write(&DIAL_ROUTES).insert(address.to_string(), peer);
    let other_spelling = value::intern("spelled-peer@some-host");
    assert_eq!(
        node_connect(other_spelling, address).ok(),
        Some(peer),
        "a second connect to a live peer must reuse its link, whatever the spelling"
    );
    // Once the link is gone, the route proves nothing: the dial is attempted.
    let id = registered_link(peer).expect("registered").0;
    drop_link(peer, id);
    assert!(
        node_connect(other_spelling, address).is_err(),
        "a route to a node that is no longer linked must not answer a connect"
    );
}

/// `proc/register`'s one refusal, decided under the NAMES lock: a dead pid binds
/// nothing — and does not disturb a live holder's binding either.
#[test]
fn register_binds_only_live_pids() {
    let me = process::self_pid();
    let name = value::intern("register-refusals@test");
    assert!(matches!(
        register(name, u64::MAX - 77),
        Registration::DeadPid
    ));
    assert_eq!(whereis(name), None, "a dead pid's name was bound");
    assert!(matches!(register(name, me), Registration::Bound));
    assert!(matches!(register(name, me), Registration::Bound));
    assert!(matches!(
        register(name, u64::MAX - 77),
        Registration::DeadPid
    ));
    assert_eq!(
        whereis(name),
        Some(me),
        "a dead pid displaced a live holder"
    );
}

/// The pre-auth connection gate: slots are bounded at the cap, the
/// over-count from a losing `try_acquire` is rolled back (so the live count
/// never drifts above the cap), and a dropped slot frees capacity again.
/// On its own counter: the process's is shared with every sibling test thread
/// under `cargo test`, and a listener test's connect holds a real slot.
#[test]
fn handshake_slot_caps_in_flight_and_releases_on_drop() {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    // Fill every slot.
    let held: Vec<HandshakeSlot> = (0..MAX_IN_FLIGHT_HANDSHAKES)
        .map(|_| HandshakeSlot::try_acquire_on(&COUNT).expect("under the cap"))
        .collect();
    assert_eq!(COUNT.load(Ordering::Acquire), MAX_IN_FLIGHT_HANDSHAKES);

    // One past the cap is shed, and the failed attempt rolled its count back.
    assert!(
        HandshakeSlot::try_acquire_on(&COUNT).is_none(),
        "cap must shed"
    );
    assert_eq!(
        COUNT.load(Ordering::Acquire),
        MAX_IN_FLIGHT_HANDSHAKES,
        "a shed attempt must not leak a slot"
    );

    // Dropping a held slot frees exactly one, which a fresh acquire can take.
    drop(held);
    assert_eq!(COUNT.load(Ordering::Acquire), 0);
    let slot = HandshakeSlot::try_acquire_on(&COUNT).expect("capacity freed");
    assert_eq!(COUNT.load(Ordering::Acquire), 1);
    drop(slot);
    assert_eq!(COUNT.load(Ordering::Acquire), 0);
}

/// KI-97 item 1: a peer that **trickles** must not outlive the handshake
/// deadline. This is the case the socket's `SO_RCVTIMEO` cannot catch — every
/// individual read succeeds well inside it, so the per-read bound restarts
/// forever and a 4 KiB pre-auth frame could be dragged out for ~10 hours while
/// holding a `HandshakeSlot`.
///
/// The reader below is that attacker in miniature: one byte per call, always
/// promptly, never an error. Against a bare `read_exact` it always wins;
/// against `Deadline` it must be cut off with `TimedOut` having delivered only
/// a fraction of what was asked for.
#[test]
fn a_trickling_peer_is_cut_off_at_the_handshake_deadline() {
    /// Always returns exactly one byte — the shape a per-read timeout can
    /// never reject.
    struct Trickle {
        served: usize,
    }
    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(Duration::from_millis(1));
            self.served += 1;
            buf[0] = 0;
            Ok(1)
        }
    }

    let mut peer = Trickle { served: 0 };
    let budget = Duration::from_millis(50);
    let started = Instant::now();
    let mut guarded = Deadline::new(&mut peer, budget);

    // Ask for far more than the trickler will ever deliver in the budget.
    let mut buf = vec![0u8; MAX_HANDSHAKE_FRAME];
    let err = guarded
        .read_exact(&mut buf)
        .expect_err("a trickling peer must not satisfy read_exact past the deadline");
    assert_eq!(
        err.kind(),
        io::ErrorKind::TimedOut,
        "expected the deadline to fire, got: {err}"
    );

    let elapsed = started.elapsed();
    assert!(
        elapsed < budget * 10,
        "the deadline must cut the read off promptly, took {elapsed:?}"
    );
    // The point of the assertion: it was cut off mid-frame, not served.
    assert!(
        peer.served < MAX_HANDSHAKE_FRAME,
        "the trickler should never have completed the frame"
    );
}

/// The deadline must not interfere with a handshake that simply completes:
/// reads and writes inside the budget pass through untouched.
#[test]
fn the_deadline_is_transparent_to_a_prompt_peer() {
    let mut peer = io::Cursor::new(b"brood".to_vec());
    let mut guarded = Deadline::new(&mut peer, Duration::from_secs(30));
    let mut buf = [0u8; 5];
    guarded.read_exact(&mut buf).expect("prompt read");
    assert_eq!(&buf, b"brood");

    let mut sink: Vec<u8> = Vec::new();
    let mut guarded = Deadline::new(&mut sink, Duration::from_secs(30));
    guarded.write_all(b"hi").expect("prompt write");
    guarded.flush().expect("prompt flush");
    assert_eq!(sink, b"hi");
}

/// A write is deadlined too. A peer that finishes its own side and then stops
/// reading would otherwise park us in `write_all` against a full socket
/// buffer, holding the same slot from the other direction.
#[test]
fn a_write_past_the_deadline_is_refused() {
    let mut sink: Vec<u8> = Vec::new();
    let mut guarded = Deadline::new(&mut sink, Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(5));
    let err = guarded
        .write_all(b"x")
        .expect_err("a write past the deadline must fail");
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
}

/// Shedding at the cap is neither silent nor a flood vector: the first shed warns, an
/// immediately following burst does not (rate-limited), and every shed is counted
/// regardless.
#[test]
fn shedding_counts_every_connection_and_warns_at_most_once_per_interval() {
    assert_eq!(SHED_HANDSHAKES.load(Ordering::Relaxed), 0);
    note_shed_handshake();
    let after_first = LAST_SHED_WARN_MS.load(Ordering::Relaxed);
    // Not the sentinel any more ⇒ the first shed warned. Deliberately not
    // `!= 0`: `now_millis()` legitimately IS 0 in the first millisecond of
    // the process, which is exactly the case this sentinel exists for.
    assert_ne!(
        after_first, SHED_WARN_NEVER,
        "the first shed must warn, even at timestamp 0"
    );

    for _ in 0..50 {
        note_shed_handshake();
    }
    assert_eq!(
        SHED_HANDSHAKES.load(Ordering::Relaxed),
        51,
        "every shed must be counted, warned or not"
    );
    assert_eq!(
        LAST_SHED_WARN_MS.load(Ordering::Relaxed),
        after_first,
        "a burst must not re-warn inside the interval"
    );
}

/// A weak cookie is rejected *before* any identity/listener side effect
/// (kernel audit guardrail): possession of the cookie is remote eval, and
/// the HMAC itself accepts any key length — so `node_listen` is the gate.
/// The runtime must remain a non-node afterwards so a corrected
/// `node-start` can be retried.
#[test]
fn node_listen_rejects_a_short_cookie() {
    let name = crate::core::value::intern("weak@test");
    for weak in ["", "x", "hunter2", "123456789012345"] {
        let err = node_listen(name, "tcp:127.0.0.1:0", weak.to_string())
            .expect_err("a sub-16-byte cookie must be refused");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("cookie too short"), "got: {err}");
    }
    // No identity was published by the failed attempts.
    assert!(
        !crate::core::sync::read(&NODE).started,
        "must stay a non-node"
    );
}

/// A port-0 listener advertises the port the OS actually bound, and that address
/// dials — peers learn it through the handshake and gossip.
#[test]
fn a_port_zero_listener_advertises_its_bound_port() {
    // The address list is process-wide and `cargo test` runs these tests as threads of
    // one process, so read what THIS listener recorded, not the list's first entry.
    let advertised = start_listener("tcp:127.0.0.1:0").expect("bind a port-0 listener");
    assert!(crate::core::sync::read(&LISTEN_ADDRS).contains(&advertised));
    let host_and_port = advertised
        .strip_prefix("tcp:127.0.0.1:")
        .unwrap_or_else(|| panic!("unexpected advertised address {advertised}"));
    let port: u16 = host_and_port.parse().expect("a numeric port");
    assert_ne!(
        port, 0,
        "advertised the requested port 0, not the bound one"
    );
    TcpStream::connect(("127.0.0.1", port)).expect("the advertised address dials");
}

/// A wildcard listener advertises the node's host, never `0.0.0.0` (which no peer
/// on another machine can dial), with the bound port.
#[test]
fn a_wildcard_listener_advertises_the_node_host() {
    let advertised = start_listener("tcp:0.0.0.0:0").expect("bind a wildcard listener");
    assert!(crate::core::sync::read(&LISTEN_ADDRS).contains(&advertised));
    // The host is the node's (`localhost` unless a concurrent test started a node).
    let (host, port) = advertised
        .strip_prefix("tcp:")
        .and_then(|host_and_port| host_and_port.rsplit_once(':'))
        .unwrap_or_else(|| panic!("unexpected advertised address {advertised}"));
    assert_ne!(
        host, "0.0.0.0",
        "advertised the wildcard, which no peer can dial"
    );
    let port: u16 = port.parse().expect("a numeric port");
    assert_ne!(
        port, 0,
        "advertised the requested port 0, not the bound one"
    );
    TcpStream::connect((host, port)).expect("the advertised address dials");
}
