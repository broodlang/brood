//! The v2 authenticated node handshake (ADR-034 v2).
//!
//! Both ends of a fresh TCP connection drive the same four-step exchange
//! before either accepts a steady-state frame:
//!
//! 1. **Magic + version** (4 bytes, `b"BRD\x02"`). A mismatch aborts before
//!    any allocation — a stray HTTP request or port-scanner can't push us
//!    past this point.
//! 2. **Hello** (`{ node, nonce, addr }`) — each side announces its name, a
//!    fresh 32-byte nonce, and the address peers should dial to reach it (for
//!    the cluster mesh, ADR-088). The initiator writes first; the responder
//!    reads, then writes its own. The cookie is **never** on the wire.
//! 3. **Auth** (`{ mac }`) — each side computes
//!    `HMAC-SHA256(cookie, peer_nonce || peer_name || 0x00 || my_name || 0x00
//!    || my_addr)` and sends it. Same write-then-read shape as Hello. Binding
//!    `my_addr` into the MAC means an on-path attacker can't rewrite the
//!    advertised mesh address in a `Hello` without the cookie — the `Auth`
//!    check would fail.
//! 4. The peer's `Auth` is constant-time-compared against the expected MAC.
//!    A mismatch is `PermissionDenied`; the link never enters `NODES`.
//!
//! Because the MAC is over a *fresh per-handshake* peer nonce, a passive
//! observer can't replay a captured `Auth` against a different handshake.
//! The HMAC also doesn't disclose the cookie (it only proves possession).

use std::io::{self, Read, Write};

use x25519_dalek::{PublicKey, StaticSecret};

use crate::core::value::{self, Symbol};

use super::session::{OpenKey, SealKey, Session, KEY_LEN};
use super::wire::{
    read_frame_capped, write_frame, Frame, EPH_PUB_LEN, MAC_LEN, NONCE_LEN, PROTOCOL_MAGIC,
};
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

/// Drive the four-step exchange. Returns the peer's authoritative node name, its
/// advertised dial address, *and* the agreed encrypted [`Session`] on success —
/// `dist::establish` registers the link under the name, stores the address for
/// mesh gossip, and hands the session's two directional keys to the reader/writer.
pub(super) fn handshake<S: Read + Write>(
    stream: &mut S,
    role: Role,
) -> io::Result<(Symbol, String, Session)> {
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

    // Step 2: Hellos with nonces, a fresh ephemeral DH pubkey, and the advertised
    // mesh address. The ephemeral keypair is generated per handshake and never
    // persisted, giving the session forward secrecy (ADR-089).
    let (my_name, cookie) = {
        let n = crate::core::sync::read(&super::NODE);
        (n.name, n.cookie.clone())
    };
    let my_addr = super::advertised_addr();
    let my_nonce = fresh_nonce()?;
    let my_secret = ephemeral_secret()?;
    let my_eph_pub: [u8; EPH_PUB_LEN] = PublicKey::from(&my_secret).to_bytes();
    let my_hello = Frame::Hello {
        node: my_name,
        nonce: my_nonce,
        eph_pub: my_eph_pub,
        addr: my_addr.clone(),
    };
    let their_hello = match role {
        Role::Initiator => {
            write_frame(stream, &my_hello)?;
            read_hello(stream)?
        }
        Role::Responder => {
            let h = read_hello(stream)?;
            write_frame(stream, &my_hello)?;
            h
        }
    };
    let (peer_name, peer_nonce, peer_eph_pub, peer_addr) = their_hello;

    // Step 3 + 4: MAC the *peer's* nonce + ephemeral pubkey + the names + my own
    // advertised addr + my own ephemeral pubkey; exchange and verify. The input is
    // symmetric — both sides put their own name/addr/pubkey last — so the two MACs
    // cover identical-shaped bytes from opposite vantage points, and each covers
    // *both* ephemeral pubkeys. Folding the pubkeys in authenticates the DH: a
    // man-in-the-middle can't substitute its own key without the cookie (ADR-089),
    // just as folding the addr authenticates the gossiped address (ADR-088).
    let my_mac = compute_mac(
        &cookie,
        &peer_nonce,
        &peer_eph_pub,
        peer_name,
        my_name,
        &my_addr,
        &my_eph_pub,
    );
    let expected_peer_mac = compute_mac(
        &cookie,
        &my_nonce,
        &my_eph_pub,
        my_name,
        peer_name,
        &peer_addr,
        &peer_eph_pub,
    );
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

    // Authenticated: derive the session. The DH secret is keyed by the *initiator's*
    // nonce first (both ends order it the same regardless of role), so both compute
    // identical directional keys.
    let shared = my_secret.diffie_hellman(&PublicKey::from(peer_eph_pub));
    let (init_nonce, resp_nonce) = match role {
        Role::Initiator => (my_nonce, peer_nonce),
        Role::Responder => (peer_nonce, my_nonce),
    };
    let keys = derive_keys(shared.as_bytes(), &init_nonce, &resp_nonce);
    let session = match role {
        // Each side seals with its own outbound direction and opens the other.
        Role::Initiator => Session {
            send: SealKey::new(keys.i2r),
            recv: OpenKey::new(keys.r2i),
        },
        Role::Responder => Session {
            send: SealKey::new(keys.r2i),
            recv: OpenKey::new(keys.i2r),
        },
    };
    Ok((peer_name, peer_addr, session))
}

fn read_hello<S: Read>(
    stream: &mut S,
) -> io::Result<(Symbol, [u8; NONCE_LEN], [u8; EPH_PUB_LEN], String)> {
    // Pre-auth: a tiny ceiling, not the 64 MiB steady-state one.
    match read_frame_capped(stream, MAX_HANDSHAKE_FRAME)? {
        Frame::Hello {
            node,
            nonce,
            eph_pub,
            addr,
        } => Ok((node, nonce, eph_pub, addr)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "expected Hello")),
    }
}

fn read_auth<S: Read>(stream: &mut S) -> io::Result<[u8; MAC_LEN]> {
    match read_frame_capped(stream, MAX_HANDSHAKE_FRAME)? {
        Frame::Auth { mac } => Ok(mac),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "expected Auth")),
    }
}

/// `HMAC-SHA256(cookie, peer_nonce || peer_eph_pub || peer_name || 0x00 ||
/// my_name || 0x00 || my_addr || 0x00 || my_eph_pub)`.
///
/// **Encoding is collision-free** under these assumptions, all of which hold:
///   1. `peer_nonce` and `peer_eph_pub` are fixed-length (`NONCE_LEN` / `EPH_PUB_LEN`)
///      and lead, so the bytes after them are unambiguously the start of `peer_name`.
///   2. The `0x00` delimiters separate the variable-length name/addr fields —
///      without them, `("ab", "c")` and `("a", "bc")` would HMAC to the same
///      value. NUL is not a legal character in a Brood symbol name (the reader
///      rejects it), and the address is a `unix:`/`tcp:` form with no NUL, so
///      the delimiters genuinely separate the fields.
///   3. `my_eph_pub` is fixed-length and *last*, after a `0x00` delimiter closing
///      the variable-length `my_addr`, so it can't merge with the address.
///
/// `my_addr` is each side's *own* advertised dial address; folding it in
/// authenticates the `Hello.addr` field the cluster mesh relies on (ADR-088), so
/// a MitM can't redirect where peers later dial us. Both ephemeral DH pubkeys are
/// folded in — each side's MAC covers the peer's pubkey (2nd) and its own (last),
/// so each MAC authenticates *both* keys, defeating a MitM DH-key substitution
/// (ADR-089): a swapped `Hello.eph_pub` makes the `Auth` check fail.
///
/// Names travel as canonical (interned) UTF-8 spellings, identical on both
/// sides regardless of interner state.
#[allow(clippy::too_many_arguments)]
fn compute_mac(
    cookie: &str,
    peer_nonce: &[u8; NONCE_LEN],
    peer_eph_pub: &[u8; EPH_PUB_LEN],
    peer_name: Symbol,
    my_name: Symbol,
    my_addr: &str,
    my_eph_pub: &[u8; EPH_PUB_LEN],
) -> [u8; MAC_LEN] {
    use hmac::{KeyInit, Mac};
    type HmacSha256 = hmac::Hmac<sha2::Sha256>;
    let mut mac = HmacSha256::new_from_slice(cookie.as_bytes()).expect("HMAC key length is fine");
    mac.update(peer_nonce);
    mac.update(peer_eph_pub);
    mac.update(value::symbol_name(peer_name).as_bytes());
    mac.update(&[0]);
    mac.update(value::symbol_name(my_name).as_bytes());
    mac.update(&[0]);
    mac.update(my_addr.as_bytes());
    mac.update(&[0]);
    mac.update(my_eph_pub);
    mac.finalize().into_bytes().into()
}

/// The two directional AEAD keys derived from the DH shared secret (ADR-089):
/// `i2r` seals initiator→responder traffic, `r2i` the reverse. Both ends compute
/// the same pair (same shared secret, same nonce order), then pick send/recv by role.
struct DerivedKeys {
    i2r: [u8; KEY_LEN],
    r2i: [u8; KEY_LEN],
}

/// HKDF-SHA256 over the X25519 shared secret → the two directional keys. The salt
/// binds the keys to *this* handshake's nonces (initiator's first — role-independent
/// ordering), so a replayed DH can't resurrect an old session's keys. Built on the
/// `hmac`/`sha2` crates already in the tree (HKDF = HMAC-extract + HMAC-expand),
/// avoiding a separate `hkdf` crate version pin.
fn derive_keys(
    shared: &[u8],
    init_nonce: &[u8; NONCE_LEN],
    resp_nonce: &[u8; NONCE_LEN],
) -> DerivedKeys {
    use hmac::{KeyInit, Mac};
    type H = hmac::Hmac<sha2::Sha256>;
    const INFO: &[u8] = b"brood node-link v4 session keys";

    // Extract: PRK = HMAC(salt = init_nonce || resp_nonce, ikm = shared).
    let mut salt = [0u8; NONCE_LEN * 2];
    salt[..NONCE_LEN].copy_from_slice(init_nonce);
    salt[NONCE_LEN..].copy_from_slice(resp_nonce);
    let prk = {
        let mut e = H::new_from_slice(&salt).expect("HMAC salt length is fine");
        e.update(shared);
        e.finalize().into_bytes()
    };

    // Expand: T(1) = HMAC(PRK, INFO || 0x01), T(2) = HMAC(PRK, T(1) || INFO || 0x02).
    // 64 bytes of output material = two SHA-256 blocks → the two 32-byte keys.
    let block = |prev: &[u8], counter: u8| -> [u8; 32] {
        let mut h = H::new_from_slice(&prk).expect("HMAC PRK length is fine");
        h.update(prev);
        h.update(INFO);
        h.update(&[counter]);
        h.finalize().into_bytes().into()
    };
    let t1 = block(&[], 1);
    let t2 = block(&t1, 2);
    DerivedKeys { i2r: t1, r2i: t2 }
}

/// A fresh ephemeral X25519 secret from the OS RNG. `StaticSecret` (vs the typed
/// `EphemeralSecret`) lets us seed it from the `getrandom` bytes we already use —
/// no extra `rand_core` feature — and we simply never persist it, so it's ephemeral
/// in practice: a new keypair per handshake gives the session forward secrecy.
fn ephemeral_secret() -> io::Result<StaticSecret> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed)
        .map_err(|e| io::Error::other(format!("could not read OS RNG for DH secret: {e}")))?;
    Ok(StaticSecret::from(seed))
}

/// Constant-time comparison for the MAC check — prevents a timing oracle from
/// leaking bits about the shared cookie via the comparison path. Uses
/// `subtle::ConstantTimeEq` rather than a hand-rolled XOR loop so a future
/// "simplification" to `a == b` can't silently reintroduce the side-channel.
fn ct_eq(a: &[u8; MAC_LEN], b: &[u8; MAC_LEN]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
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
        let eph_a = [10u8; EPH_PUB_LEN];
        let eph_b = [20u8; EPH_PUB_LEN];
        let a = value::intern("aa");
        let b = value::intern("bb");
        let addr_a = "tcp:127.0.0.1:9001";
        let addr_b = "tcp:127.0.0.1:9002";

        // Side A computes its outgoing MAC (over B's nonce+pubkey, then its own
        // addr+pubkey) and the MAC it expects from B — exactly the two
        // `compute_mac` calls `handshake` performs.
        let a_my_mac = compute_mac(cookie, &nonce_b, &eph_b, b, a, addr_a, &eph_a);
        let a_expects_b_mac = compute_mac(cookie, &nonce_a, &eph_a, a, b, addr_b, &eph_b);
        // Side B computes the symmetric pair (peer ↔ self labels flipped).
        let b_my_mac = compute_mac(cookie, &nonce_a, &eph_a, a, b, addr_b, &eph_b);
        let b_expects_a_mac = compute_mac(cookie, &nonce_b, &eph_b, b, a, addr_a, &eph_a);

        // The cross-checks that the actual handshake does — each side's
        // outgoing MAC equals the other side's expectation.
        assert_eq!(a_my_mac, b_expects_a_mac, "A's mac must verify on B");
        assert_eq!(b_my_mac, a_expects_b_mac, "B's mac must verify on A");

        // A different cookie produces a different MAC (integrity).
        assert_ne!(a_my_mac, compute_mac("other", &nonce_b, &eph_b, b, a, addr_a, &eph_a));
        // A different peer nonce produces a different MAC (replay defence).
        assert_ne!(a_my_mac, compute_mac(cookie, &[3u8; NONCE_LEN], &eph_b, b, a, addr_a, &eph_a));
        // A tampered advertised address produces a different MAC, so a MitM
        // can't rewrite where peers will later dial us (ADR-088).
        assert_ne!(a_my_mac, compute_mac(cookie, &nonce_b, &eph_b, b, a, "tcp:evil:6666", &eph_a));
        // A swapped *peer* ephemeral pubkey produces a different MAC, so a MitM
        // can't substitute its own DH key (ADR-089) — the Auth check would fail.
        assert_ne!(a_my_mac, compute_mac(cookie, &nonce_b, &[99u8; EPH_PUB_LEN], b, a, addr_a, &eph_a));
        // A swapped *own* ephemeral pubkey also changes the MAC (both keys bound).
        assert_ne!(a_my_mac, compute_mac(cookie, &nonce_b, &eph_b, b, a, addr_a, &[99u8; EPH_PUB_LEN]));
    }

    /// Both ends derive the *same* directional keys from the X25519 exchange, and
    /// the two directions differ — so the role-based send/recv assignment in
    /// `handshake` lets each side seal what the other opens (ADR-089).
    #[test]
    fn session_keys_agree_under_role_flip() {
        // Two ephemeral keypairs (deterministic seeds for the test).
        let a_secret = StaticSecret::from([7u8; 32]);
        let b_secret = StaticSecret::from([9u8; 32]);
        let a_pub = PublicKey::from(&a_secret);
        let b_pub = PublicKey::from(&b_secret);

        // X25519 is symmetric: both sides compute the same shared secret.
        let shared_a = a_secret.diffie_hellman(&b_pub);
        let shared_b = b_secret.diffie_hellman(&a_pub);
        assert_eq!(shared_a.as_bytes(), shared_b.as_bytes());

        // With the same shared secret and nonce order, both ends derive identical
        // directional keys, and the two directions are distinct.
        let ni = [1u8; NONCE_LEN];
        let nr = [2u8; NONCE_LEN];
        let ka = derive_keys(shared_a.as_bytes(), &ni, &nr);
        let kb = derive_keys(shared_b.as_bytes(), &ni, &nr);
        assert_eq!(ka.i2r, kb.i2r, "both ends must agree on the i→r key");
        assert_eq!(ka.r2i, kb.r2i, "both ends must agree on the r→i key");
        assert_ne!(ka.i2r, ka.r2i, "the two directions must use different keys");

        // A different shared secret yields different keys (the DH actually matters).
        let other = StaticSecret::from([3u8; 32]);
        let shared_other = a_secret.diffie_hellman(&PublicKey::from(&other));
        let ko = derive_keys(shared_other.as_bytes(), &ni, &nr);
        assert_ne!(ka.i2r, ko.i2r);
    }
}
