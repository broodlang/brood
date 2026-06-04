//! Liveness detection: one shared OS thread probes every link on a fixed
//! cadence, declaring a link **down** (and tearing it down via `Shutdown::Both`)
//! when it's been silent past [`DOWN_AFTER`].
//!
//! Pulled out of the connection lifecycle so the timing constants and the
//! "single thread, started lazily" detail aren't tangled with the dial /
//! accept / register code. Inbound frames refresh `last_seen` directly from
//! the reader thread (which lives in `dist::mod`) — this module only reads
//! that timestamp.

use std::io;
use std::net::Shutdown;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Once};
use std::time::Duration;

use super::wire::{encode_payload, Frame};
use super::{now_millis, NODES};

/// How often the (single, shared) heartbeat thread probes each link with a `Ping`
/// and checks liveness. Idle-gated: a `Ping` is a 5-byte frame, only sent on the
/// tick, never per message.
pub(super) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);

/// A link with no inbound frame (data, `Ping`, or `Pong`) for this long is
/// declared **down**: we `shutdown` its socket, which tears it down and fires
/// `[:nodedown name]` to its watchers. Several heartbeat intervals, so a single
/// dropped probe doesn't flap a healthy link.
const DOWN_AFTER: Duration = Duration::from_secs(6);

static HEARTBEAT_STARTED: Once = Once::new();

/// Start the single shared heartbeat thread once, on the first established
/// link. `establish` calls this; subsequent calls are no-ops via [`Once`].
pub(super) fn ensure_heartbeat() {
    HEARTBEAT_STARTED.call_once(|| {
        // Re-spawn on panic so a single bad iteration doesn't silently stop
        // liveness detection for the rest of the process lifetime.
        std::thread::spawn(|| loop {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(heartbeat_loop));
            eprintln!("dist: heartbeat thread panicked; restarting");
            std::thread::sleep(HEARTBEAT_INTERVAL);
        });
    });
}

/// Probe every link each interval: if it's been silent past [`DOWN_AFTER`],
/// declare it down (shutdown → the reader runs `drop_link` → `[:nodedown]`);
/// otherwise send a `Ping` (the peer answers `Pong`, refreshing its
/// `last_seen`). One thread for all links; a `Ping` is sent only on the tick,
/// never per message.
fn heartbeat_loop() {
    // One shared Ping payload for every link, every tick: each send is an
    // `Arc::clone` (atomic incr), not a `Vec` copy. The payload is plaintext; each
    // link's writer seals it with that direction's next nonce (ADR-089), so the
    // same shared buffer yields distinct ciphertext per link — no nonce reuse.
    let ping: Arc<[u8]> = match encode_payload(&Frame::Ping) {
        Ok(b) => Arc::from(b),
        Err(e) => {
            // The Ping frame has no variable-width fields, so this can't fail
            // in practice; if it ever does, abort the loop cleanly rather than
            // panic — `ensure_heartbeat`'s restart machinery will re-enter.
            eprintln!("dist: cannot encode Ping: {}", io::Error::other(e));
            return;
        }
    };
    let down_after = DOWN_AFTER.as_millis() as u64;
    loop {
        std::thread::sleep(HEARTBEAT_INTERVAL);
        let now = now_millis();
        // Snapshot under the lock, then act without holding it (shutdown/send can block).
        // (sock, tx, last_seen_millis) per link.
        type LinkSnapshot = (Arc<super::Stream>, SyncSender<Arc<[u8]>>, u64);
        let links: Vec<LinkSnapshot> = {
            let nodes = crate::core::sync::read(&NODES);
            nodes
                .values()
                .map(|c| {
                    (
                        Arc::clone(&c.sock),
                        c.tx.clone(),
                        c.last_seen.load(Ordering::Acquire),
                    )
                })
                .collect()
        };
        for (sock, tx, last) in links {
            if now.saturating_sub(last) > down_after {
                let _ = sock.shutdown(Shutdown::Both); // dead peer → tear down via the reader
            } else if tx.try_send(Arc::clone(&ping)).is_err() {
                // Bounded queue Full/disconnected: the writer is stalled or gone —
                // sever via the socket (the reader's `drop_link` deregisters)
                // rather than buffer the ping.
                let _ = sock.shutdown(Shutdown::Both);
            }
        }
    }
}
