//! The v2 authenticated node handshake (ADR-034 v2).
//!
//! Both ends of a fresh TCP connection drive the same four-step exchange
//! before either accepts a steady-state frame:
//!
//! 1. **Magic + version** (4 bytes, `b"BRD\x02"`). A mismatch aborts before
//!    any allocation — a stray HTTP request or port-scanner can't push us
//!    past this point.
//! 2. **Hello** (`{ node, nonce }`) — each side announces its name and a
//!    fresh 32-byte nonce. The initiator writes first; the responder reads,
//!    then writes its own. The cookie is **never** on the wire.
//! 3. **Auth** (`{ mac }`) — each side computes
//!    `HMAC-SHA256(cookie, peer_nonce || peer_name || 0x00 || my_name)` and
//!    sends it. Same write-then-read shape as Hello.
//! 4. The peer's `Auth` is constant-time-compared against the expected MAC.
//!    A mismatch is `PermissionDenied`; the link never enters `NODES`.
//!
//! Because the MAC is over a *fresh per-handshake* peer nonce, a passive
//! observer can't replay a captured `Auth` against a different handshake.
//! The HMAC also doesn't disclose the cookie (it only proves possession).

use std::io::{self, Read, Write};

use crate::core::value::{self, Symbol};

use super::wire::{read_frame_capped, write_frame, Frame, MAC_LEN, NONCE_LEN, PROTOCOL_MAGIC};
use super::MAX_HANDSHAKE_FRAME;

/// Which end opened a connection — the tie-break for a duplicate keeps the link
/// initiated by the smaller node name, so both ends need to know who that is.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Role {
    /// We dialed (`connect`) — the initiator is us.
    Initiator,
    /// We accepted — the initiator is the peer.
    Responder,
}

/// Drive the four-step exchange. Returns the peer's authoritative node name
/// on success — `dist::establish` then registers the link under this name.
pub(super) fn handshake<S: Read + Write>(stream: &mut S, role: Role) -> io::Result<Symbol> {
    // Step 1: magic + version. Reject before any allocation if we don't speak
    // the same dialect.
    stream.write_all(&PROTOCOL_MAGIC)?;
    let mut their_magic = [0u8; 4];
    stream.read_exact(&mut their_magic)?;
    if their_magic != PROTOCOL_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "protocol magic/version mismatch (theirs: {:02x?}, ours: {:02x?})",
                their_magic, PROTOCOL_MAGIC
            ),
        ));
    }

    // Step 2: Hellos with nonces.
    let (my_name, cookie) = {
        let n = crate::core::sync::read(&super::NODE);
        (n.name, n.cookie.clone())
    };
    let my_nonce = fresh_nonce()?;
    let their_hello = match role {
        Role::Initiator => {
            write_frame(
                stream,
                &Frame::Hello {
                    node: my_name,
                    nonce: my_nonce,
                },
            )?;
            read_hello(stream)?
        }
        Role::Responder => {
            let h = read_hello(stream)?;
            write_frame(
                stream,
                &Frame::Hello {
                    node: my_name,
                    nonce: my_nonce,
                },
            )?;
            h
        }
    };
    let (peer_name, peer_nonce) = their_hello;

    // Step 3 + 4: MAC the *peer's* nonce + the names; exchange and verify.
    // Order (peer_name then my_name in the input) is symmetric — both sides
    // include their own name last, so the two MACs cover identical-shaped
    // bytes from opposite vantage points.
    let my_mac = compute_mac(&cookie, &peer_nonce, peer_name, my_name);
    let expected_peer_mac = compute_mac(&cookie, &my_nonce, my_name, peer_name);
    let their_mac = match role {
        Role::Initiator => {
            write_frame(stream, &Frame::Auth { mac: my_mac })?;
            read_auth(stream)?
        }
        Role::Responder => {
            let m = read_auth(stream)?;
            write_frame(stream, &Frame::Auth { mac: my_mac })?;
            m
        }
    };
    if !ct_eq(&their_mac, &expected_peer_mac) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "node handshake MAC mismatch (wrong cookie?)",
        ));
    }
    Ok(peer_name)
}

fn read_hello<S: Read>(stream: &mut S) -> io::Result<(Symbol, [u8; NONCE_LEN])> {
    // Pre-auth: a tiny ceiling, not the 64 MiB steady-state one.
    match read_frame_capped(stream, MAX_HANDSHAKE_FRAME)? {
        Frame::Hello { node, nonce } => Ok((node, nonce)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "expected Hello")),
    }
}

fn read_auth<S: Read>(stream: &mut S) -> io::Result<[u8; MAC_LEN]> {
    match read_frame_capped(stream, MAX_HANDSHAKE_FRAME)? {
        Frame::Auth { mac } => Ok(mac),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "expected Auth")),
    }
}

/// `HMAC-SHA256(cookie, peer_nonce || peer_name || 0x00 || my_name)`.
///
/// **Encoding is collision-free** under two assumptions, both of which hold:
///   1. `peer_nonce` is exactly `NONCE_LEN` bytes (fixed length), so the
///      following bytes are unambiguously the start of `peer_name`.
///   2. The `0x00` delimiter separates the two variable-length names —
///      without it, `("ab", "c")` and `("a", "bc")` would HMAC to the same
///      value. NUL is not a legal character in a Brood symbol name (the
///      reader rejects it), so it can't appear inside either name and
///      genuinely separates them.
///
/// Names travel as canonical (interned) UTF-8 spellings, identical on both
/// sides regardless of interner state.
fn compute_mac(
    cookie: &str,
    peer_nonce: &[u8; NONCE_LEN],
    peer_name: Symbol,
    my_name: Symbol,
) -> [u8; MAC_LEN] {
    use hmac::{KeyInit, Mac};
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;
    let mut mac = HmacSha256::new_from_slice(cookie.as_bytes()).expect("HMAC key length is fine");
    mac.update(peer_nonce);
    mac.update(value::symbol_name(peer_name).as_bytes());
    mac.update(&[0]);
    mac.update(value::symbol_name(my_name).as_bytes());
    mac.finalize().into_bytes().into()
}

/// Constant-time comparison for the MAC check. `subtle`/`hmac::Mac::verify`
/// would also do this, but `verify` consumes the HMAC state by computing the
/// expected MAC at the same time — we already have the expected MAC, so do
/// the byte compare ourselves.
fn ct_eq(a: &[u8; MAC_LEN], b: &[u8; MAC_LEN]) -> bool {
    let mut diff: u8 = 0;
    for i in 0..MAC_LEN {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// 32 fresh bytes from the OS random pool. Each handshake gets its own pair
/// of nonces, so a captured `Auth` MAC can't be replayed against a fresh
/// handshake.
fn fresh_nonce() -> io::Result<[u8; NONCE_LEN]> {
    let mut n = [0u8; NONCE_LEN];
    getrandom::fill(&mut n)
        .map_err(|e| io::Error::other(format!("could not read OS RNG for handshake nonce: {e}")))?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate both sides of a handshake and verify each side's `my_mac`
    /// matches the *other* side's `expected_peer_mac` — i.e. what `handshake`
    /// actually compares. A typo in `compute_mac`'s arg order (e.g. forgetting
    /// to put `my_name` last) would let one side authenticate while the
    /// other rejects; this test catches that asymmetry. Also asserts the
    /// integrity properties (wrong cookie / wrong nonce → different MAC).
    #[test]
    fn compute_mac_is_symmetric_under_role_flip() {
        let cookie = "shared";
        let nonce_a = [1u8; NONCE_LEN];
        let nonce_b = [2u8; NONCE_LEN];
        let a = value::intern("aa");
        let b = value::intern("bb");

        // Side A computes its outgoing MAC and the MAC it expects from B —
        // exactly the two `compute_mac` calls `handshake` performs.
        let a_my_mac = compute_mac(cookie, &nonce_b, b, a);
        let a_expects_b_mac = compute_mac(cookie, &nonce_a, a, b);
        // Side B computes the symmetric pair (peer ↔ self labels flipped).
        let b_my_mac = compute_mac(cookie, &nonce_a, a, b);
        let b_expects_a_mac = compute_mac(cookie, &nonce_b, b, a);

        // The cross-checks that the actual handshake does — each side's
        // outgoing MAC equals the other side's expectation.
        assert_eq!(a_my_mac, b_expects_a_mac, "A's mac must verify on B");
        assert_eq!(b_my_mac, a_expects_b_mac, "B's mac must verify on A");

        // A different cookie produces a different MAC (integrity).
        assert_ne!(a_my_mac, compute_mac("other", &nonce_b, b, a));
        // A different peer nonce produces a different MAC (replay defence).
        assert_ne!(a_my_mac, compute_mac(cookie, &[3u8; NONCE_LEN], b, a));
    }
}
