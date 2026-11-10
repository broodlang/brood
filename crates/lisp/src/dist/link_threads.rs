//! A steady-state link's two OS threads: the writer, which seals and writes what the
//! link's queue holds, and the reader, which opens each inbound frame, answers pings, and
//! dispatches the rest — deliveries, monitor and link signals, gossip — on the link's
//! authenticated peer name.

use super::*;

/// The link's writer thread: pull each plaintext frame payload off the channel, **seal**
/// it (with whatever else is already queued, into one reused buffer), and write the
/// ciphertext. A per-write timeout (`WRITE_TIMEOUT`) keeps a slowloris
/// peer from pinning the writer and ballooning the queue; a timeout or a seal failure is
/// treated like any I/O error — shut the socket down.
pub(super) fn spawn_link_writer(
    peer: Symbol,
    sock: Arc<Stream>,
    queue: mpsc::Receiver<Outbound>,
    latest: LatestSlots,
    mut seal: session::SealKey,
) {
    if let Err(error) = sock.set_write_timeout(Some(WRITE_TIMEOUT)) {
        eprintln!(
            "dist: warning: could not set write timeout on link to {}: {error}",
            value::symbol_name(peer)
        );
    }
    spawn_bg("dist-writer", move || {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            // One buffer for the life of the link: each wake seals the frame it woke for
            // and every frame already queued behind it (up to `WRITE_BATCH_BYTES`) into
            // it, and writes them with one `write_all`. The byte stream is the one a
            // write per frame produced — the frames in queue order, each sealed with the
            // next nonce — in fewer syscalls; nothing waits for a batch to fill, since
            // only what is already queued joins it.
            let mut batch: Vec<u8> = Vec::new();
            // A replaced `send-latest` ticket claims nothing and is passed over (ADR-492).
            while let Ok(item) = queue.recv() {
                let Some(payload) = claim_outbound(&latest, item) else {
                    continue;
                };
                batch.clear();
                let mut sealed = seal.seal_into(&payload, &mut batch).is_ok();
                while sealed && batch.len() < WRITE_BATCH_BYTES {
                    match queue.try_recv() {
                        Ok(next) => {
                            if let Some(next) = claim_outbound(&latest, next) {
                                sealed = seal.seal_into(&next, &mut batch).is_ok();
                            }
                        }
                        Err(_) => break,
                    }
                }
                // A seal failure leaves the frames before it intact in the batch; they
                // are written before the link goes down, as they were one at a time.
                let written = (&*sock).write_all(&batch).is_ok();
                if !(sealed && written) {
                    let _ = sock.shutdown(Shutdown::Both);
                    break;
                }
                // One huge frame must not pin its buffer for the life of the link.
                if batch.capacity() > 4 * WRITE_BATCH_BYTES {
                    batch = Vec::new();
                }
            }
        }));
    });
}

/// The link's reader thread: every inbound frame refreshes liveness and is dispatched by
/// [`handle_inbound_frame`]. On EOF/error (incl. a `shutdown` from the writer or the
/// heartbeat) it runs `drop_link`, which removes the entry iff it's still this
/// generation. The receive cipher is owned solely by this thread; a tag failure (a
/// tampered, forged, replayed or reordered frame) ends the loop and tears the link down.
pub(super) fn spawn_link_reader(
    peer: Symbol,
    id: u64,
    sock: Arc<Stream>,
    writer_queue: SyncSender<Outbound>,
    mut open: session::OpenKey,
    last_seen: Arc<AtomicU64>,
    guest: Option<Arc<guest::GuestLink>>,
) {
    // One shared Pong buffer per reader; sending is an `Arc::clone`, not a copy.
    let pong: Arc<Vec<u8>> = Arc::new(encode_payload(&Frame::Pong).expect("encode Pong"));
    spawn_bg("dist-reader", move || {
        // Buffered: a frame is a 4-byte length read then a body read, so unbuffered it was
        // two syscalls a frame at least. The buffer only gathers bytes the socket already
        // has (a read returns what is there, it does not wait to fill), so a frame is
        // handled — and `last_seen` refreshed — exactly when its last byte arrives, as
        // before; the steady-state socket has no read timeout for it to interact with.
        // Nothing else reads this socket after the handshake, so nothing is lost in it.
        let mut reader = std::io::BufReader::new(&*sock);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Ok(frame) = open.open(&mut reader) {
                last_seen.store(now_millis(), Ordering::Release);
                if let Frame::Ping = frame {
                    // Bounded queue: if we can't even enqueue a Pong, the writer is
                    // stalled (peer not draining) or gone — sever and let `drop_link`
                    // below deregister, rather than buffer.
                    if writer_queue.try_send(Arc::clone(&pong).into()).is_err() {
                        let _ = sock.shutdown(Shutdown::Both);
                        break;
                    }
                } else {
                    handle_inbound_frame(peer, guest.as_deref(), frame);
                }
            }
        }));
        drop_link(peer, id);
    });
}

/// Act on one steady-state frame from `peer`. `peer` is the *authenticated* node name
/// from the handshake — every process-coupling frame (Monitor/Demonitor/Link/Unlink/Exit)
/// is keyed to it, never to wire-supplied data, so a malicious peer can't claim to be
/// node X and inject `[:down …]` / link-exit deliveries to processes coupled with X.
/// (These frames carry no `from_node` field at all — see the security note on `Frame`.)
///
/// `guest` is the standing of the link the frame was READ from, as `establish` gave it to
/// that link's reader — never looked up by `peer`, which another link may hold by now.
pub(super) fn handle_inbound_frame(
    peer: Symbol,
    guest: Option<&guest::GuestLink>,
    mut frame: Frame,
) {
    // A guest link acts only on frames that reach what it was given and carry no code
    // (ADR-415); a full link's pass untouched.
    if !guest::admit(peer, guest, &frame) {
        return;
    }
    // A guest names its own pids by the name it claimed, which is no link here: they
    // become pids of the name this node minted for the link (ADR-425), so a reply routes
    // back over it — and a pid it spells with any other node's name stays that node's.
    if let Some(guest) = guest {
        guest.translate_inbound(&mut frame);
    }
    match frame {
        Frame::Send { target, msg, lane } => process::interrupt::delivering_from(
            process::interrupt::Origin::Peer {
                node: peer,
                guest: guest.is_some(),
            },
            || deliver_inbound(target, msg, lane),
        ),
        // A peer asked to watch one of our local pids — the shared `add_monitor` core
        // with a `Watcher::Remote`, so the alive/dead-target paths are the local ones.
        Frame::Monitor {
            watcher_pid,
            target,
            mref,
        } => process::add_monitor(
            target,
            process::Watcher::Remote {
                node: peer,
                pid: watcher_pid,
                mref,
            },
        ),
        // The same for a process registered under `name` here: resolve it now, and an
        // unregistered name watches the placeholder pid, which fires `:noproc` at once —
        // the local `monitor`'s behaviour for an unregistered name.
        Frame::MonitorName {
            watcher_pid,
            name,
            mref,
        } => process::add_monitor(
            whereis(name).unwrap_or(u64::MAX),
            process::Watcher::Remote {
                node: peer,
                pid: watcher_pid,
                mref,
            },
        ),
        // Peer dropped a remote monitor — the local `demonitor`'s `drop_monitor`, matching
        // the Remote variant's identity (node + pid + mref).
        Frame::Demonitor { watcher_pid, mref } => process::drop_monitor(|watcher| {
            matches!(*watcher, process::Watcher::Remote { node, pid, mref: r }
                         if node == peer && pid == watcher_pid && r == mref)
        }),
        // A monitor we asked the peer to keep just fired. Retire our `PENDING_REMOTE`
        // entry (a later node-down must not fire it again, KI-96), then deliver the
        // `[:down …]`, the dying pid node-qualified by the authenticated `peer`.
        Frame::Down {
            watcher_pid,
            mref,
            target_pid,
            reason,
        } => process::deliver_remote_down(peer, watcher_pid, mref, target_pid, reason),
        // A peer linked its `from_pid` to our local `to_pid` — record our half.
        Frame::Link { from_pid, to_pid } => {
            // `to_pid` is wire data and may name a dead pid; recording that would leak
            // an entry forever (see `record_remote_link`), so tell the peer instead — its
            // linked process gets `:noproc`, exactly what a LOCAL `link` to a dead pid gets.
            if !process::record_remote_link(to_pid, peer, from_pid) {
                send_link_exit(peer, from_pid, to_pid, Message::Keyword(*pk::NOPROC_SYMBOL));
            }
        }
        Frame::Unlink { from_pid, to_pid } => {
            process::drop_remote_link(to_pid, peer, from_pid);
        }
        // An exit signal for our local `to_pid`: a link death goes through the
        // trap-or-propagate path; an explicit remote exit is kill-style.
        Frame::Exit {
            from_pid,
            to_pid,
            reason,
            link,
        } => {
            if link {
                process::deliver_remote_link_exit(to_pid, peer, from_pid, reason);
            } else {
                process::exit(to_pid, reason);
            }
        }
        // Cluster-mesh gossip: dial any node the peer knows that we don't (ADR-088).
        Frame::Peers { peers } => mesh_consider(peers),
        // `Ping` is answered by the reader itself. Handshake-only frames in steady state
        // are malformed but harmless — keep reading.
        Frame::Ping | Frame::Pong | Frame::Hello { .. } | Frame::Auth { .. } => {}
    }
}

/// An inbound `Send` from a peer: resolve the target locally and deliver.
pub(super) fn deliver_inbound(target: Target, msg: Message, lane: process::Lane) {
    let id = match target {
        Target::Pid(id) => id,
        Target::Name(name) => match crate::core::sync::read(&NAMES).get(name) {
            Some(id) => id,
            None => {
                warn_dropped_to_unregistered_name(name, "inbound");
                return;
            }
        },
    };
    process::deliver_in_lane(id, msg, lane);
}
