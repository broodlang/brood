//! Large bytes values cross a link ONCE (ADR-695): a bytes value of [`SHARED_BYTES_MIN`] or
//! more that a link has already carried goes the next time as its digest alone.
//!
//! A thin client is sent a frame per change, and a frame holding a picture (`[:image …]`,
//! ADR-686) holds its encoded bytes — the same hundreds of kilobytes in every frame that
//! shows the page. Any other repeated large value (a sound, a texture, a file sent twice) is
//! the same waste, so the link carries it once, not the display protocol.
//!
//! **Two ledgers that cannot disagree.** Each end of a link keeps a ledger of the large values
//! the link carried: the writer thread of the sender's, the reader thread of the receiver's.
//! Both see the same values in the same order — the writer decides, at write time and in
//! queue order, whether a value goes whole (`M_BYTES_NEW`: digest and bytes) or by digest
//! (`M_BYTES_SEEN`), and the reader meets them in that order — and both apply the same bounded
//! least-recently-used rule ([`Ledger::note`]), so the sender always knows exactly what the
//! receiver still has, with no acknowledgement and no request for a value it lost. A new link
//! (a reconnect, a restarted peer) starts both ledgers empty. A digest the receiver does not
//! have, or bytes that do not hash to the digest they came with, is a broken link: the
//! decode fails and the link is torn down, never a wrong value delivered.
//!
//! The encoder leaves a large value OUT of a queued `Send` payload, noting its offset
//! ([`SharingPayload`]); the writer thread splices it back in when the payload's turn comes
//! ([`splice`]). Deciding then rather than at encode time is what keeps the ledgers in step:
//! senders encode concurrently, and a `send-latest` frame may be replaced in the queue and
//! never written.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use crate::core::blob::SharedBlob;

/// The smallest bytes value carried by reference once the link has carried it: below it the
/// 33-byte reference saves little and the ledger would fill with small values.
pub(super) const SHARED_BYTES_MIN: usize = 16 * 1024;

/// How many large values each end remembers, and how many of their bytes: the receiver holds
/// them all, so this is a client's memory bound per link.
const LEDGER_KEPT: usize = 64;
const LEDGER_BUDGET: usize = 64 * 1024 * 1024;

/// The large values a link has carried, by digest, least recently used dropped past the
/// bounds. Both ends of a link apply exactly the same notes in the same order, so their
/// ledgers stay identical.
#[derive(Default)]
pub(super) struct Ledger {
    entries: HashMap<[u8; 32], (usize, u64)>,
    bytes: usize,
    clock: u64,
}

impl Ledger {
    /// Note that the link carried the `len`-byte value `digest`: whether it was already in
    /// the ledger (it went by reference), and the digests dropped to stay within the bounds.
    pub(super) fn note(&mut self, digest: [u8; 32], len: usize) -> (bool, Vec<[u8; 32]>) {
        self.clock += 1;
        if let Some((_, used)) = self.entries.get_mut(&digest) {
            *used = self.clock;
            return (true, Vec::new());
        }
        self.entries.insert(digest, (len, self.clock));
        self.bytes += len;
        let mut dropped = Vec::new();
        while (self.entries.len() > LEDGER_KEPT || self.bytes > LEDGER_BUDGET)
            && self.entries.len() > 1
        {
            let Some(oldest) = self
                .entries
                .iter()
                .filter(|(entry, _)| **entry != digest)
                .min_by_key(|(_, (_, used))| *used)
                .map(|(entry, _)| *entry)
            else {
                break;
            };
            if let Some((gone, _)) = self.entries.remove(&oldest) {
                self.bytes -= gone;
            }
            dropped.push(oldest);
        }
        (false, dropped)
    }
}

/// What a link's reader keeps: its ledger and the values in it, which a reference names.
#[derive(Default)]
pub(super) struct ReceivedBytes {
    ledger: Ledger,
    values: HashMap<[u8; 32], Arc<SharedBlob>>,
}

/// A `Send` payload as a current link queues it: the encoded frame with each large bytes
/// value left out, and where each goes (its offset in `bytes`, in order).
pub(crate) struct SharingPayload {
    bytes: Vec<u8>,
    shared: Vec<(usize, Arc<SharedBlob>)>,
}

thread_local! {
    /// The large values an encode on this thread left out, while [`encode_sharing`] runs.
    static LEFT_OUT: RefCell<Option<Vec<(usize, Arc<SharedBlob>)>>> = const { RefCell::new(None) };
    /// The reader's ledger, while [`with_received`] decodes on this thread.
    static RECEIVING: RefCell<Option<ReceivedBytes>> = const { RefCell::new(None) };
}

/// Encode with large bytes values left out: `encode` writes the frame, and every bytes value
/// of [`SHARED_BYTES_MIN`] or more it meets is noted at its offset instead of written
/// ([`leave_out`]). The payload is complete once [`splice`]d.
pub(super) fn encode_sharing(
    encode: impl FnOnce() -> io::Result<Vec<u8>>,
) -> io::Result<SharingPayload> {
    LEFT_OUT.with(|left_out| *left_out.borrow_mut() = Some(Vec::new()));
    let encoded = encode();
    let shared = LEFT_OUT
        .with(|left_out| left_out.borrow_mut().take())
        .unwrap_or_default();
    Ok(SharingPayload {
        bytes: encoded?,
        shared,
    })
}

/// The encoder's question for a bytes value at offset `at`: true when it is left out (an
/// [`encode_sharing`] is running and the value is large), so nothing is written for it now.
pub(super) fn leave_out(at: usize, blob: &Arc<SharedBlob>) -> bool {
    if blob.len() < SHARED_BYTES_MIN {
        return false;
    }
    LEFT_OUT.with(|left_out| match left_out.borrow_mut().as_mut() {
        Some(noted) => {
            noted.push((at, blob.clone()));
            true
        }
        None => false,
    })
}

impl SharingPayload {
    /// How large the payload is when every value goes whole: what the frame limit applies to.
    pub(super) fn whole_len(&self) -> usize {
        self.bytes.len()
            + self
                .shared
                .iter()
                .map(|(_, blob)| 1 + 32 + 4 + blob.len())
                .sum::<usize>()
    }
}

/// The payload as written on the link: each left-out value spliced back in at its offset —
/// by digest when the link carried it before (`sent` says, and notes it), else whole.
/// Called by the link's writer thread, in queue order.
pub(super) fn splice(payload: &SharingPayload, sent: &mut Ledger) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.bytes.len() + 64 * payload.shared.len());
    let mut copied = 0;
    for (at, blob) in &payload.shared {
        out.extend_from_slice(&payload.bytes[copied..*at]);
        copied = *at;
        let digest = blob.digest();
        let (seen, _) = sent.note(digest, blob.len());
        if seen {
            out.push(super::wire::M_BYTES_SEEN);
            out.extend_from_slice(&digest);
        } else {
            out.push(super::wire::M_BYTES_NEW);
            out.extend_from_slice(&digest);
            out.extend_from_slice(&(blob.len() as u32).to_be_bytes());
            out.extend_from_slice(blob.as_bytes());
        }
    }
    out.extend_from_slice(&payload.bytes[copied..]);
    out
}

/// Run `decode` (a link reader's frame decode) with `received` as the ledger a large
/// value's reference is read against.
pub(super) fn with_received<T>(received: &mut ReceivedBytes, decode: impl FnOnce() -> T) -> T {
    RECEIVING.with(|slot| *slot.borrow_mut() = Some(std::mem::take(received)));
    let result = decode();
    if let Some(back) = RECEIVING.with(|slot| slot.borrow_mut().take()) {
        *received = back;
    }
    result
}

fn broken(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason.to_string())
}

/// A large value sent whole, `bytes` under `digest`: kept for the references that follow.
/// Bytes that do not hash to their digest break the link — a peer must not be able to make
/// a later reference resolve to other bytes than the digest names.
pub(super) fn received_new(digest: [u8; 32], bytes: &[u8]) -> io::Result<Arc<SharedBlob>> {
    let blob = SharedBlob::new(bytes);
    if blob.digest() != digest {
        return Err(broken("shared bytes do not match their digest"));
    }
    RECEIVING.with(|slot| {
        if let Some(received) = slot.borrow_mut().as_mut() {
            let (_, dropped) = received.ledger.note(digest, blob.len());
            for gone in dropped {
                received.values.remove(&gone);
            }
            received.values.insert(digest, blob.clone());
        }
    });
    Ok(blob)
}

/// A large value sent by reference: the one this link carried under `digest`.
pub(super) fn received_seen(digest: [u8; 32]) -> io::Result<Arc<SharedBlob>> {
    RECEIVING.with(|slot| {
        let mut slot = slot.borrow_mut();
        let received = slot
            .as_mut()
            .ok_or_else(|| broken("a shared-bytes reference outside a link"))?;
        let blob =
            received.values.get(&digest).cloned().ok_or_else(|| {
                broken("a shared-bytes reference to a value this link does not hold")
            })?;
        let (seen, dropped) = received.ledger.note(digest, blob.len());
        debug_assert!(seen && dropped.is_empty());
        Ok(blob)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dist::Target;
    use crate::dist::wire::{Frame, decode_frame, encode_payload};
    use crate::process::{Lane, Message};

    /// A `Send` of `items` (each a bytes value) as the routing path queues it for a current
    /// peer.
    fn queued(items: &[Arc<SharedBlob>]) -> SharingPayload {
        let frame = Frame::Send {
            target: Target::Pid(7),
            msg: Message::Vector(items.iter().cloned().map(Message::Bytes).collect()),
            lane: Lane::Normal,
        };
        encode_sharing(|| encode_payload(&frame)).expect("encodes")
    }

    /// What the receiving reader decodes `written` to: the bytes values of the vector sent.
    fn received(written: Vec<u8>, ledger: &mut ReceivedBytes) -> io::Result<Vec<Arc<SharedBlob>>> {
        let frame = with_received(ledger, || decode_frame(&mut std::io::Cursor::new(written)))?;
        let Frame::Send { msg, .. } = &frame else {
            panic!("not the send");
        };
        let Message::Vector(items) = msg else {
            panic!("not the vector");
        };
        Ok(items
            .iter()
            .map(|item| match item {
                Message::Bytes(blob) => blob.clone(),
                _ => panic!("not bytes"),
            })
            .collect())
    }

    fn large(seed: u8) -> Arc<SharedBlob> {
        SharedBlob::new(&vec![seed; SHARED_BYTES_MIN + seed as usize])
    }

    // The point of it: a picture in every frame crosses the link once (ADR-695). The second
    // frame carries its digest alone and the receiver hands back the value it kept.
    #[test]
    fn a_large_value_crosses_a_link_once_then_by_digest() {
        let (mut sent, mut kept) = (Ledger::default(), ReceivedBytes::default());
        let picture = large(1);
        let first = splice(&queued(std::slice::from_ref(&picture)), &mut sent);
        assert!(first.len() > SHARED_BYTES_MIN, "whole the first time");
        let got = received(first, &mut kept).expect("decodes");
        assert_eq!(got[0].as_bytes(), picture.as_bytes());

        let second = splice(&queued(std::slice::from_ref(&picture)), &mut sent);
        assert!(
            second.len() < 128,
            "by digest after: {} bytes",
            second.len()
        );
        let again = received(second, &mut kept).expect("decodes");
        assert!(Arc::ptr_eq(&got[0], &again[0]), "the value the reader kept");
        // and the receiver's copy knows its digest without hashing it again
        assert_eq!(again[0].digest(), picture.digest());

        // the same value twice in ONE frame: whole, then by digest, both resolved
        let (mut sent, mut kept) = (Ledger::default(), ReceivedBytes::default());
        let both = splice(&queued(&[picture.clone(), picture.clone()]), &mut sent);
        assert!(both.len() < 2 * SHARED_BYTES_MIN);
        let got = received(both, &mut kept).expect("decodes");
        assert!(Arc::ptr_eq(&got[0], &got[1]));
    }

    #[test]
    fn a_small_value_is_never_left_out() {
        let small = SharedBlob::new(b"tiny");
        let payload = queued(std::slice::from_ref(&small));
        assert!(payload.shared.is_empty());
        let got = received(
            splice(&payload, &mut Ledger::default()),
            &mut ReceivedBytes::default(),
        )
        .expect("decodes");
        assert_eq!(got[0].as_bytes(), b"tiny");
        // outside a sharing encode a large value is written inline, as before
        let frame = Frame::Send {
            target: Target::Pid(7),
            msg: Message::Bytes(large(2)),
            lane: Lane::Normal,
        };
        assert!(encode_payload(&frame).expect("encodes").len() > SHARED_BYTES_MIN);
    }

    // The two ledgers drop the same values, so a value the receiver let go is sent whole
    // again — never a reference it cannot resolve.
    #[test]
    fn both_ends_forget_alike_past_the_bound() {
        let (mut sent, mut kept) = (Ledger::default(), ReceivedBytes::default());
        for seed in 0..=LEDGER_KEPT as u8 {
            let written = splice(&queued(&[large(seed)]), &mut sent);
            received(written, &mut kept).expect("decodes");
        }
        assert_eq!(kept.values.len(), LEDGER_KEPT);
        // seed 0 was the least recently used: gone at both ends, so whole again
        let written = splice(&queued(&[large(0)]), &mut sent);
        assert!(written.len() > SHARED_BYTES_MIN);
        received(written, &mut kept).expect("decodes");
        // seed 2 is still held: by digest
        let written = splice(&queued(&[large(2)]), &mut sent);
        assert!(written.len() < 128);
        received(written, &mut kept).expect("decodes");
    }

    // A reference the reader cannot resolve, or bytes that are not what their digest says,
    // break the link rather than deliver a wrong value.
    #[test]
    fn an_unknown_reference_or_a_wrong_digest_is_refused() {
        let picture = large(3);
        let mut sent = Ledger::default();
        let _ = splice(&queued(std::slice::from_ref(&picture)), &mut sent);
        let reference = splice(&queued(std::slice::from_ref(&picture)), &mut sent);
        assert!(received(reference, &mut ReceivedBytes::default()).is_err());

        let mut whole = splice(
            &queued(std::slice::from_ref(&picture)),
            &mut Ledger::default(),
        );
        let last = whole.len() - 1;
        whole[last - 10] ^= 1;
        assert!(received(whole, &mut ReceivedBytes::default()).is_err());
    }
}
