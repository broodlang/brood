//! The encrypted steady-state session (ADR-089) — confidentiality + per-frame
//! integrity for the node link, closing ADR-081's cleartext/injection gap.
//!
//! After the handshake authenticates the peer (cookie-HMAC) and agrees a shared
//! secret (ephemeral X25519, see `dist::handshake`), the link runs **encrypted**.
//! Every steady-state frame is sealed with **ChaCha20-Poly1305**: the 16-byte
//! Poly1305 tag *is* a per-frame MAC, so a forged frame injected after the
//! handshake — including a `Send` carrying a closure (→ RCE) — fails to open and
//! tears the link down.
//!
//! ## Why this fits the reader/writer thread split (and TLS wouldn't)
//! A live link runs two independent threads sharing an `Arc<Stream>` — a reader
//! (`&Stream: Read`) and a writer (`&Stream: Write`). A single TLS `Connection`
//! can't be driven from both (it holds shared mutable crypto state). Here each
//! **direction** has its own key + monotonic nonce counter: the writer owns a
//! [`SealKey`], the reader owns an [`OpenKey`], and they never share crypto state.
//!
//! ## Nonces
//! The nonce is a per-direction frame counter (`[0u8; 4] || counter_be_u64`).
//! Counters start at 0 and only increase; the two directions use *different*
//! keys, so every `(key, nonce)` pair is unique across the whole session — no
//! reuse, ever. A reordered or replayed frame decrypts under the wrong counter
//! and fails the tag check, so TCP's in-order delivery is enforced cryptographically
//! too. (Counter exhaustion at 2^64 frames is unreachable, but we error rather
//! than wrap — a wrap would reuse a nonce.)

use std::io::{self, Read};

use chacha20poly1305::aead::Aead;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};

use super::wire::{decode_frame, Frame};
use super::MAX_FRAME;

/// AEAD key length (ChaCha20-Poly1305 / X25519 shared-secret derived).
pub(super) const KEY_LEN: usize = 32;

/// An authenticated link's encrypted session: the two directional ciphers from
/// the handshake's key agreement (ADR-089). `dist::establish` moves `send` into
/// the writer thread and `recv` into the reader thread, so neither shares crypto
/// state — the property that lets a per-direction AEAD fit the reader/writer split.
pub(super) struct Session {
    pub(super) send: SealKey,
    pub(super) recv: OpenKey,
}
/// Poly1305 authentication tag length appended to each ciphertext.
const TAG_LEN: usize = 16;

/// The 12-byte nonce for frame number `counter` in one direction: four zero bytes
/// then the big-endian counter. Distinct per direction because the keys differ.
fn nonce_bytes(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_be_bytes());
    n
}

/// The send half of a session: seals outbound plaintext payloads. Owned by the
/// writer thread, so its counter needs no synchronisation.
pub(super) struct SealKey {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl SealKey {
    pub(super) fn new(key: [u8; KEY_LEN]) -> Self {
        SealKey {
            cipher: ChaCha20Poly1305::new_from_slice(&key).expect("32-byte AEAD key"),
            counter: 0,
        }
    }

    /// Seal one frame `payload` (the bare bytes from `wire::encode_payload`) into a
    /// ready-to-write `[u32 len][ciphertext+tag]` blob, advancing the nonce counter.
    pub(super) fn seal(&mut self, payload: &[u8]) -> io::Result<Vec<u8>> {
        let nonce = nonce_bytes(self.counter);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), payload)
            .map_err(|_| io::Error::other("frame encryption failed"))?;
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| io::Error::other("session nonce space exhausted"))?;
        // `payload` was already capped at MAX_FRAME by `encode_payload`, so
        // `ct.len()` (payload + 16) fits a u32 comfortably.
        let mut out = Vec::with_capacity(ct.len() + 4);
        out.extend_from_slice(&(ct.len() as u32).to_be_bytes());
        out.extend_from_slice(&ct);
        Ok(out)
    }
}

/// The receive half of a session: reads + opens inbound sealed frames. Owned by
/// the reader thread, so its counter needs no synchronisation.
pub(super) struct OpenKey {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl OpenKey {
    pub(super) fn new(key: [u8; KEY_LEN]) -> Self {
        OpenKey {
            cipher: ChaCha20Poly1305::new_from_slice(&key).expect("32-byte AEAD key"),
            counter: 0,
        }
    }

    /// Read one sealed frame from `r`, authenticate + decrypt it, and decode the
    /// `Frame`. A tag failure — a tampered, forged, replayed, or reordered frame —
    /// surfaces as an `io::Error`, so the steady-state reader's
    /// `while let Ok(frame) = open.open(..)` loop tears the link down on any
    /// integrity violation. This is the gate that closes ADR-081's
    /// post-handshake-injection (RCE-by-forged-closure) hole.
    pub(super) fn open(&mut self, r: &mut impl Read) -> io::Result<Frame> {
        let mut len = [0u8; 4];
        r.read_exact(&mut len)?;
        let len = u32::from_be_bytes(len) as usize;
        // A sealed frame is `plaintext + TAG_LEN`; reject an over-large prefix
        // before allocating (mirrors the plaintext `read_frame_capped` ceiling).
        if len > MAX_FRAME + TAG_LEN || len < TAG_LEN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("sealed frame length {len} out of range"),
            ));
        }
        let mut ct = vec![0u8; len];
        r.read_exact(&mut ct)?;
        let nonce = nonce_bytes(self.counter);
        let pt = self
            .cipher
            .decrypt(Nonce::from_slice(&nonce), ct.as_slice())
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "frame authentication failed (tampered, forged, replayed, or reordered)",
                )
            })?;
        self.counter = self
            .counter
            .checked_add(1)
            .ok_or_else(|| io::Error::other("session nonce space exhausted"))?;
        decode_frame(&mut std::io::Cursor::new(pt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::value;
    use std::io::Cursor;

    fn key(b: u8) -> [u8; KEY_LEN] {
        [b; KEY_LEN]
    }

    /// A frame the tests can seal and recognise on the way back out.
    fn monitor(mref: u64) -> Frame {
        Frame::Monitor {
            from_node: value::intern("peer"),
            watcher_pid: 7,
            target: 42,
            mref,
        }
    }

    fn payload(f: &Frame) -> Vec<u8> {
        super::super::wire::encode_payload(f).unwrap()
    }

    /// A sealed pair built from the same key round-trips a stream of frames in
    /// order — the happy path the link runs on.
    #[test]
    fn seal_open_roundtrips_in_order() {
        let mut seal = SealKey::new(key(1));
        let mut open = OpenKey::new(key(1));
        let mut wire = Vec::new();
        for i in 0..5u64 {
            wire.extend_from_slice(&seal.seal(&payload(&monitor(i))).unwrap());
        }
        let mut r = Cursor::new(wire);
        for i in 0..5u64 {
            match open.open(&mut r).unwrap() {
                Frame::Monitor { mref, target, .. } => {
                    assert_eq!(mref, i);
                    assert_eq!(target, 42);
                }
                _ => panic!("wrong frame"),
            }
        }
    }

    /// Flipping a single ciphertext byte makes the frame fail to open — the
    /// per-frame integrity guarantee that closes the injection hole (ADR-081 #1).
    #[test]
    fn tampered_ciphertext_is_rejected() {
        let mut seal = SealKey::new(key(2));
        let mut framed = seal.seal(&payload(&monitor(1))).unwrap();
        let last = framed.len() - 1; // inside the ciphertext+tag, past the 4-byte len
        framed[last] ^= 0xff;
        let mut open = OpenKey::new(key(2));
        assert!(open.open(&mut Cursor::new(framed)).is_err());
    }

    /// A reordered (or replayed) frame decrypts under the wrong counter and is
    /// rejected, so an attacker can't reorder or replay captured frames.
    #[test]
    fn reordered_and_replayed_frames_are_rejected() {
        let mut seal = SealKey::new(key(3));
        let a = seal.seal(&payload(&monitor(10))).unwrap(); // counter 0
        let b = seal.seal(&payload(&monitor(11))).unwrap(); // counter 1

        // Feeding B first (sealed at counter 1) to a fresh OpenKey at counter 0 fails.
        let mut open = OpenKey::new(key(3));
        assert!(open.open(&mut Cursor::new(b)).is_err(), "reorder must fail");

        // In order, A opens; replaying A again (now counter 1) fails.
        let mut open = OpenKey::new(key(3));
        assert!(open.open(&mut Cursor::new(a.clone())).is_ok());
        assert!(open.open(&mut Cursor::new(a)).is_err(), "replay must fail");
    }

    /// The opposite direction's key can't open a frame — the two directions are
    /// cryptographically separated.
    #[test]
    fn wrong_direction_key_cannot_open() {
        let mut seal = SealKey::new(key(4));
        let framed = seal.seal(&payload(&monitor(1))).unwrap();
        let mut open = OpenKey::new(key(5)); // different key
        assert!(open.open(&mut Cursor::new(framed)).is_err());
    }

    /// Sealing the same plaintext twice yields different ciphertext (the counter
    /// advanced), proving nonces aren't reused.
    #[test]
    fn counter_advances_so_nonces_never_repeat() {
        let mut seal = SealKey::new(key(6));
        let p = payload(&monitor(1));
        let first = seal.seal(&p).unwrap();
        let second = seal.seal(&p).unwrap();
        assert_ne!(first, second);
    }
}
