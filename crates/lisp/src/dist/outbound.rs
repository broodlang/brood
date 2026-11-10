//! A link's bounded writer queue: handing a frame to the writer thread (severing the link
//! when the queue is full), and the `proc/send-latest` slots (ADR-492) that let a newer
//! frame to one target and key replace an unwritten older one.

use super::*;

/// A frame payload as a link's queue holds it: the bytes to seal, or — a `Send` to a current
/// peer — the bytes with its large bytes values left out, which the writer splices back in,
/// whole or by reference, when the frame's turn comes (ADR-695).
#[derive(Clone)]
pub(crate) enum Payload {
    Plain(Arc<Vec<u8>>),
    Sharing(Arc<shared_bytes::SharingPayload>),
}

impl From<Arc<Vec<u8>>> for Payload {
    fn from(bytes: Arc<Vec<u8>>) -> Self {
        Payload::Plain(bytes)
    }
}

impl From<shared_bytes::SharingPayload> for Payload {
    fn from(payload: shared_bytes::SharingPayload) -> Self {
        Payload::Sharing(Arc::new(payload))
    }
}

/// Hand `payload` to a link's writer thread, severing the link if its queue is full.
///
/// Free-standing so a caller can run it **without holding `NODES`**: the `Err` arm
/// issues a `shutdown` syscall, and that must not run under the `NODES` lock or it
/// delays every concurrent link registration and teardown for the syscall's
/// duration — the rule [`broadcast_peer_table`] documents. So never call
/// `Conn::enqueue` straight out of a `read(&NODES).get(..)`.
pub(super) fn enqueue_to(
    tx: &SyncSender<Outbound>,
    sock: &Arc<Stream>,
    payload: impl Into<Payload>,
) -> bool {
    match tx.try_send(Outbound::Frame(payload.into())) {
        Ok(()) => true,
        Err(_) => {
            let _ = sock.shutdown(Shutdown::Both);
            false
        }
    }
}

/// What a link's writer queue holds: a frame payload, or a ticket for the newest
/// `proc/send-latest` frame to one target and key (ADR-492), whose payload waits in the
/// link's [`LatestSlots`] so a newer one can replace it before it is written.
pub(crate) enum Outbound {
    Frame(Payload),
    Latest { slot: String, generation: u64 },
}

impl From<Arc<Vec<u8>>> for Outbound {
    fn from(payload: Arc<Vec<u8>>) -> Self {
        Outbound::Frame(Payload::Plain(payload))
    }
}

/// A link's queued `proc/send-latest` frames: slot (target and key) → the generation
/// and payload of the newest one sent and not yet written.
pub(super) type LatestSlots = Arc<Mutex<HashMap<String, (u64, Payload)>>>;

/// Source of [`Outbound::Latest`] generations.
pub(super) static NEXT_LATEST: AtomicU64 = AtomicU64::new(0);

/// The slot a `proc/send-latest` to `target` with `key` (its printed form) is queued in:
/// one per target, since two processes' keys are not each other's.
pub(super) fn latest_slot(target: &Target, key: &str) -> String {
    match target {
        Target::Pid(id) => format!("pid {id} {key}"),
        Target::Name(name) => format!("name {} {key}", value::symbol_name(*name)),
    }
}

/// Queue a `proc/send-latest` frame on a link: its payload replaces any for the same
/// slot still unwritten, and its ticket goes to the back of the queue, so the frame is
/// written after everything sent before it — what the receiving mailbox does with a
/// LATEST message (ADR-490), done here before a slow link ships a stale copy. Severs the
/// link when the queue is full, as [`enqueue_to`] does.
pub(super) fn enqueue_latest(
    tx: &SyncSender<Outbound>,
    sock: &Arc<Stream>,
    latest: &LatestSlots,
    slot: String,
    payload: impl Into<Payload>,
) -> bool {
    let generation = NEXT_LATEST.fetch_add(1, Ordering::Relaxed);
    crate::core::sync::lock(latest).insert(slot.clone(), (generation, payload.into()));
    match tx.try_send(Outbound::Latest { slot, generation }) {
        Ok(()) => true,
        Err(_) => {
            let _ = sock.shutdown(Shutdown::Both);
            false
        }
    }
}

/// The payload the writer writes for `item`: a frame's own, or a ticket's when it is
/// still the newest for its slot (which it then leaves). A ticket a later
/// `send-latest` replaced writes nothing; the later one's ticket is behind it.
pub(super) fn claim_outbound(latest: &LatestSlots, item: Outbound) -> Option<Payload> {
    match item {
        Outbound::Frame(payload) => Some(payload),
        Outbound::Latest { slot, generation } => {
            let mut slots = crate::core::sync::lock(latest);
            match slots.get(&slot) {
                Some((newest, _)) if *newest == generation => {
                    slots.remove(&slot).map(|(_, payload)| payload)
                }
                _ => None,
            }
        }
    }
}
