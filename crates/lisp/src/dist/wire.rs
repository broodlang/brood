//! Wire codec for the distributed-node protocol. Pure data in / bytes out:
//! no sockets, no scheduler, no globals beyond the [`value`] symbol interner
//! (symbols travel by *name* since separate runtimes have independent
//! interners — see [`put_sym`]).
//!
//! Two stacked formats:
//! - **Frame** (`[u32 len][payload]`). The unit of transport; `payload` starts
//!   with a `FRAME_*` tag byte, then variant fields. [`read_frame`] /
//!   [`write_frame`].
//! - **Message** — Erlang-style deep-copied value, encoded inline inside a
//!   `Frame::Send` (and embedded in `ClosureMsg` for closure shipping).
//!   Symbols travel by name; the receiver re-interns them.
//!
//! Both directions cap nesting at [`MAX_DECODE_DEPTH`] so a small malicious
//! frame can't recurse the receiver's Rust stack into a SIGSEGV.

use std::io::{self, Cursor, Read, Write};

use crate::core::blob::SharedBlob;
use crate::core::value::{self, Symbol};
use crate::process::Message;

use super::{Target, MAX_FRAME};

/// Frames travel over the wire as `[u32 len][payload]`. `pub(super)` so the
/// connection-lifecycle code in `dist::mod` can construct and pattern-match
/// them; the codec is otherwise private to this module.
pub(super) enum Frame {
    /// Handshake step 1 & 2: who I am, a fresh nonce I want you to MAC, and the
    /// **address peers should dial to reach me** (`"unix:PATH"` / `"tcp:HOST:PORT"`,
    /// or empty if I'm not listening). The cookie never travels — it's an HMAC
    /// key, not a credential. Both sides send a `Hello` (initiator first,
    /// responder second); each computes its `Auth` over the peer's nonce *and*
    /// its own advertised `addr`, so an on-path attacker can't redirect the
    /// gossiped address without breaking auth (ADR-088). The address feeds the
    /// cluster mesh: a peer stores it so it can later *gossip* us to nodes that
    /// don't know us yet.
    Hello {
        node: Symbol,
        nonce: [u8; NONCE_LEN],
        /// An **ephemeral X25519 public key**, fresh per handshake (ADR-089). Both
        /// sides exchange one in their `Hello`; the shared DH secret derives the
        /// session's AEAD keys (forward secrecy — recorded traffic stays secret
        /// even if the long-term cookie later leaks). It is *authenticated* by
        /// being folded into the `Auth` MAC alongside the names + addr, so an
        /// on-path attacker can't substitute their own DH key without the cookie.
        eph_pub: [u8; EPH_PUB_LEN],
        /// The **address peers should dial to reach me** (`"unix:PATH"` /
        /// `"tcp:HOST:PORT"`, or empty if I'm not listening). Authenticated by
        /// being folded into the `Auth` MAC (see `handshake::compute_mac`), so an
        /// on-path attacker can't redirect the gossiped address.
        addr: String,
    },
    /// Handshake step 3 & 4: an HMAC-SHA256 over the peer's nonce, both names,
    /// my advertised address, and both ephemeral DH keys — proves possession of
    /// the cookie without disclosing it. The exact, authoritative input layout
    /// lives in `handshake::compute_mac` (don't restate it here — it has drifted
    /// before). Mismatch on either side aborts before the link enters `NODES`.
    Auth { mac: [u8; MAC_LEN] },
    /// Route `msg` to `target` on the receiving node.
    Send { target: Target, msg: Message },
    /// Liveness probe; the peer answers with `Pong`.
    Ping,
    /// Reply to a `Ping`. (Receiving any frame refreshes liveness; these two carry
    /// no payload, just keep an idle link demonstrably alive.)
    Pong,
    // Security note for the five process-coupling frames below (Monitor /
    // Demonitor / Link / Unlink / Exit): none of them carries the sender's node
    // name on the wire. The receiver always takes the originating node from the
    // *authenticated* link (the `peer` established by the handshake), never from
    // wire data — so a malicious peer can't claim to be node X and inject
    // `[:down …]` / link-exit deliveries to processes coupled with X. A
    // `from_node` field used to be shipped here and deliberately ignored; it was
    // removed entirely (greenfield, no wire compat) to keep the wire honest.
    /// "Watch local pid `target` for me; deliver `[:down ref pid reason]` to my
    /// `watcher_pid` (on the authenticated peer node) when it dies." The receiver
    /// routes through `process::add_monitor` with a `Watcher::Remote`, reusing
    /// the local "alive? register; dead? fire :noproc" logic — same code path,
    /// just a different watcher variant.
    Monitor {
        watcher_pid: u64,
        target: u64,
        mref: u64,
    },
    /// Drop the matching remote watcher (best effort; identified by the
    /// authenticated peer node + pid + mref). Goes through `process::drop_monitor`,
    /// the same dropper local `demonitor` uses.
    Demonitor { watcher_pid: u64, mref: u64 },
    /// "A monitor you registered with me (`Frame::Monitor`) just fired" —
    /// `target_pid` (local to the *sending* node) died with `reason`; deliver
    /// `[:down mref pid reason]` to `watcher_pid`. A dedicated frame rather
    /// than an ordinary `Send` because the receiver needs a hook: the monitor
    /// is one-shot, so its `PENDING_REMOTE` entry must be retired the moment
    /// the DOWN delivers — as a plain message the entry outlived its own DOWN,
    /// leaking per completed monitor and firing a *second*
    /// `[:down mref … :noconnection]` on a later node-down (KI-96). The dying
    /// pid is node-qualified by the authenticated peer on the receiving side,
    /// never by wire data (see the security note above).
    Down {
        watcher_pid: u64,
        mref: u64,
        target_pid: u64,
        reason: Message,
    },
    /// "Link my `from_pid` (on the authenticated peer node) to your local
    /// `to_pid`" (ADR-067). The receiver records its half in `links::REMOTE_LINKS`
    /// so either side's death — or a net-split — reaches the other. Symmetric:
    /// each node keeps `local_pid → (peer_node, peer_pid)`.
    Link { from_pid: u64, to_pid: u64 },
    /// Drop the cross-node link `from_pid@peer ↔ to_pid` (best effort).
    Unlink { from_pid: u64, to_pid: u64 },
    /// An exit signal for local `to_pid`. `link = true` is a **link death**:
    /// `from_pid` (a linked peer on the authenticated peer node) exited with
    /// `reason`, delivered via the trap-or-propagate path (a trapping target gets
    /// `[:EXIT pid reason]`). `link = false` is an explicit remote
    /// `(exit pid reason)` — routed straight to `scheduler::exit` (kill-style,
    /// like the local builtin).
    Exit {
        from_pid: u64,
        to_pid: u64,
        reason: Message,
        link: bool,
    },
    /// Cluster-mesh gossip (ADR-088): "here are the other peers I know about, and
    /// how to reach them." Each entry is a `(node-name, dial-addr)` pair. The
    /// receiver dials any peer it isn't already connected to, so connecting to
    /// one cluster member transitively joins the whole mesh. Sent right after a
    /// *new* link is established (to the new peer and every existing peer), so a
    /// node that joins via any single member learns about all the rest.
    Peers { peers: Vec<(Symbol, String)> },
}

const FRAME_HELLO: u8 = 0;
const FRAME_SEND: u8 = 1;
const FRAME_PING: u8 = 2;
const FRAME_PONG: u8 = 3;
const FRAME_MONITOR: u8 = 4;
const FRAME_DEMONITOR: u8 = 5;
const FRAME_AUTH: u8 = 6;
const FRAME_LINK: u8 = 7;
const FRAME_UNLINK: u8 = 8;
const FRAME_EXIT: u8 = 9;
const FRAME_PEERS: u8 = 10;
const FRAME_DOWN: u8 = 11;
const TARGET_PID: u8 = 0;
const TARGET_NAME: u8 = 1;

/// Hard cap on entries in a single `Peers` gossip frame, so an (authenticated
/// but possibly buggy/hostile) peer can't make us spawn an unbounded number of
/// dial threads off one frame. Far above any realistic cluster size; the
/// `prealloc` bound already stops a tiny frame from claiming a huge count, this
/// caps the *honest-length* case too.
const MAX_GOSSIP_PEERS: usize = 4096;

/// Protocol magic + version byte sent before any frame. `b"BRD"` lets a
/// `tcpdump` reader recognise the protocol; the trailing version byte gates
/// future wire-format changes — a peer that sees anything else aborts before
/// allocating buffers. Version history (greenfield — no back-compat kept): v1
/// plaintext cookie (retired); v2 HMAC handshake; v3 adds an advertised `addr`
/// to `Hello` + the `Peers` gossip frame for cluster meshing (ADR-088); **v4**
/// adds an ephemeral X25519 pubkey to `Hello` and **encrypts every steady-state
/// frame** (Noise-style session, ADR-089) — so a v3 and v4 node can't interop;
/// **v5** drops the redundant `from_node` field from the link/monitor/exit
/// frames (the reader always used the authenticated peer instead), shifting
/// every later field — a v4 peer would mis-decode them, so the byte bumps;
/// **v6** adds the shipped-closure module list to the `M_CLOSURE` record (KI-55),
/// which a v5 peer would read as the start of `captured` — a silent mis-decode of
/// every closure sent, so again the byte bumps rather than being made optional;
/// **v7** carries a monitor's DOWN in a dedicated `Down` frame instead of an
/// ordinary `Send`, so the watcher's node can retire its `PENDING_REMOTE` entry
/// when the one-shot delivers (KI-96) — to a v6 peer the new tag is a decode
/// error that tears the whole link down on the first monitor to fire, so the
/// byte bumps.
pub(super) const PROTOCOL_MAGIC: [u8; 4] = *b"BRD\x07";
pub(super) const NONCE_LEN: usize = 32;
pub(super) const MAC_LEN: usize = 32;
/// Length of an X25519 public key (the ephemeral DH key in `Hello`, ADR-089).
pub(super) const EPH_PUB_LEN: usize = 32;

/// Encode a frame to its bare payload (the `FRAME_*` tag byte + variant fields),
/// **without** a length prefix, rejecting anything over [`MAX_FRAME`]. This is the
/// plaintext a steady-state link feeds to the session layer to **seal** (ADR-089):
/// the AEAD ciphertext gets its own `[u32 len]` prefix once it's encrypted, so
/// adding one here would double-frame. The cap is enforced here — symmetric with
/// the read side — so an oversized local `(send pid huge-thing)` errors cleanly.
pub(super) fn encode_payload(frame: &Frame) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    encode_frame(&mut payload, frame)?;
    if payload.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "frame of {} bytes exceeds the {MAX_FRAME}-byte limit",
                payload.len()
            ),
        ));
    }
    Ok(payload)
}

/// Encode a frame with its `[u32 len][payload]` length prefix, ready to write as
/// **plaintext** — used only by the handshake (`write_frame`), which runs before
/// the session keys exist. Steady-state frames go through `encode_payload` + the
/// session's `seal` instead.
pub(super) fn frame_bytes(frame: &Frame) -> io::Result<Vec<u8>> {
    Ok(len_prefixed(&encode_payload(frame)?))
}

/// Prepend the protocol's 4-byte big-endian length prefix to `body`, yielding a
/// `[u32 len][body]` frame ready to write. Shared by the two producers of that
/// framing: this module's plaintext `frame_bytes` (handshake) and the session
/// layer's `seal` (ciphertext, ADR-089) — the *body* differs (plaintext payload
/// vs. AEAD ciphertext+tag) but the on-wire framing is identical, so it lives in
/// one place. `body.len()` is always within `u32` (the payload is capped at
/// `MAX_FRAME` upstream; ciphertext adds only the 16-byte AEAD tag).
pub(super) fn len_prefixed(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

pub(super) fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    w.write_all(&frame_bytes(frame)?)
}

/// Read one length-prefixed **plaintext** frame, capped at [`MAX_FRAME`]. Steady-
/// state frames are now sealed (read via `session::OpenKey::open`, ADR-089) and the
/// handshake reads through `read_frame_capped` with its own tiny ceiling — so this
/// convenience wrapper is used only by the wire-codec round-trip tests below.
#[cfg(test)]
pub(super) fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    read_frame_capped(r, MAX_FRAME)
}

/// Read one frame, rejecting a length prefix over `max` **before** allocating
/// the buffer. The cap is a parameter so the *handshake* can pass a far smaller
/// ceiling than the 64 MiB steady-state one: a `Hello`/`Auth` is only tens of
/// bytes, and an unauthenticated peer must not be able to make us `vec![0u8;
/// 64MiB]` off an 8-byte (magic + length-prefix) probe. See
/// `super::MAX_HANDSHAKE_FRAME`.
pub(super) fn read_frame_capped(r: &mut impl Read, max: usize) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {max}-byte limit"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    decode_frame(&mut Cursor::new(buf))
}

fn encode_frame(w: &mut Vec<u8>, frame: &Frame) -> io::Result<()> {
    match frame {
        Frame::Hello {
            node,
            nonce,
            eph_pub,
            addr,
        } => {
            w.push(FRAME_HELLO);
            put_sym(w, *node);
            w.extend_from_slice(nonce);
            w.extend_from_slice(eph_pub);
            put_str(w, addr);
        }
        Frame::Auth { mac } => {
            w.push(FRAME_AUTH);
            w.extend_from_slice(mac);
        }
        Frame::Send { target, msg } => {
            w.push(FRAME_SEND);
            encode_target(w, target);
            encode_msg(w, msg)?;
        }
        Frame::Ping => w.push(FRAME_PING),
        Frame::Pong => w.push(FRAME_PONG),
        Frame::Monitor {
            watcher_pid,
            target,
            mref,
        } => {
            w.push(FRAME_MONITOR);
            w.extend_from_slice(&watcher_pid.to_be_bytes());
            w.extend_from_slice(&target.to_be_bytes());
            w.extend_from_slice(&mref.to_be_bytes());
        }
        Frame::Demonitor { watcher_pid, mref } => {
            w.push(FRAME_DEMONITOR);
            w.extend_from_slice(&watcher_pid.to_be_bytes());
            w.extend_from_slice(&mref.to_be_bytes());
        }
        Frame::Down {
            watcher_pid,
            mref,
            target_pid,
            reason,
        } => {
            w.push(FRAME_DOWN);
            w.extend_from_slice(&watcher_pid.to_be_bytes());
            w.extend_from_slice(&mref.to_be_bytes());
            w.extend_from_slice(&target_pid.to_be_bytes());
            encode_msg(w, reason)?;
        }
        Frame::Link { from_pid, to_pid } => {
            w.push(FRAME_LINK);
            w.extend_from_slice(&from_pid.to_be_bytes());
            w.extend_from_slice(&to_pid.to_be_bytes());
        }
        Frame::Unlink { from_pid, to_pid } => {
            w.push(FRAME_UNLINK);
            w.extend_from_slice(&from_pid.to_be_bytes());
            w.extend_from_slice(&to_pid.to_be_bytes());
        }
        Frame::Exit {
            from_pid,
            to_pid,
            reason,
            link,
        } => {
            w.push(FRAME_EXIT);
            w.extend_from_slice(&from_pid.to_be_bytes());
            w.extend_from_slice(&to_pid.to_be_bytes());
            w.push(*link as u8);
            encode_msg(w, reason)?;
        }
        Frame::Peers { peers } => {
            w.push(FRAME_PEERS);
            put_u32(w, peers.len() as u32);
            for (node, addr) in peers {
                put_sym(w, *node);
                put_str(w, addr);
            }
        }
    }
    Ok(())
}

/// Decode one frame's payload (no length prefix — the caller has already read the
/// bytes, whether from the plaintext `read_frame*` path or after the session layer
/// decrypts a sealed frame, ADR-089). `pub(super)` so `dist::session` can decode an
/// opened ciphertext.
pub(super) fn decode_frame(r: &mut Cursor<Vec<u8>>) -> io::Result<Frame> {
    match get_u8(r)? {
        FRAME_HELLO => Ok(Frame::Hello {
            node: get_sym(r)?,
            nonce: get_fixed::<NONCE_LEN>(r)?,
            eph_pub: get_fixed::<EPH_PUB_LEN>(r)?,
            addr: get_str(r)?,
        }),
        FRAME_AUTH => Ok(Frame::Auth {
            mac: get_fixed::<MAC_LEN>(r)?,
        }),
        FRAME_SEND => Ok(Frame::Send {
            target: decode_target(r)?,
            msg: decode_msg(r)?,
        }),
        FRAME_PING => Ok(Frame::Ping),
        FRAME_PONG => Ok(Frame::Pong),
        FRAME_MONITOR => Ok(Frame::Monitor {
            watcher_pid: get_u64(r)?,
            target: get_u64(r)?,
            mref: get_u64(r)?,
        }),
        FRAME_DEMONITOR => Ok(Frame::Demonitor {
            watcher_pid: get_u64(r)?,
            mref: get_u64(r)?,
        }),
        FRAME_DOWN => Ok(Frame::Down {
            watcher_pid: get_u64(r)?,
            mref: get_u64(r)?,
            target_pid: get_u64(r)?,
            reason: decode_msg(r)?,
        }),
        FRAME_LINK => Ok(Frame::Link {
            from_pid: get_u64(r)?,
            to_pid: get_u64(r)?,
        }),
        FRAME_UNLINK => Ok(Frame::Unlink {
            from_pid: get_u64(r)?,
            to_pid: get_u64(r)?,
        }),
        FRAME_EXIT => Ok(Frame::Exit {
            from_pid: get_u64(r)?,
            to_pid: get_u64(r)?,
            link: get_u8(r)? != 0,
            reason: decode_msg(r)?,
        }),
        FRAME_PEERS => {
            let n = get_u32(r)? as usize;
            if n > MAX_GOSSIP_PEERS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("gossip frame of {n} peers exceeds the {MAX_GOSSIP_PEERS} limit"),
                ));
            }
            let mut peers = Vec::with_capacity(prealloc(r, n));
            for _ in 0..n {
                let node = get_sym(r)?;
                let addr = get_str(r)?;
                peers.push((node, addr));
            }
            Ok(Frame::Peers { peers })
        }
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown frame tag {t}"),
        )),
    }
}

fn encode_target(w: &mut Vec<u8>, target: &Target) {
    match target {
        Target::Pid(id) => {
            w.push(TARGET_PID);
            w.extend_from_slice(&id.to_be_bytes()); // u64
        }
        Target::Name(s) => {
            w.push(TARGET_NAME);
            put_sym(w, *s);
        }
    }
}

fn decode_target(r: &mut Cursor<Vec<u8>>) -> io::Result<Target> {
    match get_u8(r)? {
        TARGET_PID => Ok(Target::Pid(get_u64(r)?)),
        TARGET_NAME => Ok(Target::Name(get_sym(r)?)),
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown target tag {t}"),
        )),
    }
}

// ----- Message codec (symbols travel by name) --------------------------------

const M_NIL: u8 = 0;
const M_FALSE: u8 = 1;
const M_TRUE: u8 = 2;
const M_INT: u8 = 3;
const M_FLOAT: u8 = 4;
const M_STR: u8 = 5;
const M_SYM: u8 = 6;
const M_KEYWORD: u8 = 7;
const M_LIST: u8 = 8;
const M_VECTOR: u8 = 9;
const M_MAP: u8 = 10;
const M_REF: u8 = 11;
const M_PID: u8 = 12;
/// A serialised closure (ADR-033 closure-as-data path). Body and optionals'
/// defaults are S-expression forms — already messages — so the wire encoding
/// is a flat record: name?, params, optionals, rest?, body, doc?, captured.
/// The receiver's `closure_from_message` chains captured frees onto its own
/// global scope; free globals re-resolve there (Erlang's "the module must be
/// loaded on both nodes").
const M_CLOSURE: u8 = 13;
/// An arbitrary-precision integer, sent as its decimal string (see
/// [`Message::BigInt`]) — portable across nodes with independent heaps.
const M_BIGINT: u8 = 14;
/// An arbitrary-precision base-10 decimal, sent as its canonical decimal string
/// (mirrors [`M_BIGINT`] / [`Message::Decimal`]) — portable across nodes.
const M_DECIMAL: u8 = 15;
/// A set — its element count then each element (values are all `true`, dropped).
/// Portable across nodes; the receiver rebuilds a `Value::Set`.
const M_SET: u8 = 16;
/// An exact rational, sent as its `num/den` string (mirrors [`M_DECIMAL`] /
/// [`Message::Ratio`]) — portable across nodes.
const M_RATIO: u8 = 17;
/// A builtin referenced by name ([`Message::Native`]) — the name travels like any other
/// symbol, and the reader re-resolves it to that runtime's primitive. Only the startup image
/// produces this; a plain message refuses a builtin before it reaches the wire.
const M_NATIVE: u8 = 18;
/// A raw-bytes value ([`Message::Bytes`]), length-prefixed and copied inline. Immutable data,
/// so it crosses by value (the receiver re-`SharedBlob::new`s its own copy — no shared Arc
/// identity, which is why blob lifetimes don't matter here). Carries the startup image's byte
/// literals (`#b"…"`), and a cross-node byte send.
const M_BYTES: u8 = 19;

pub(crate) fn encode_msg(w: &mut Vec<u8>, m: &Message) -> io::Result<()> {
    match m {
        Message::Nil => w.push(M_NIL),
        Message::Bool(false) => w.push(M_FALSE),
        Message::Bool(true) => w.push(M_TRUE),
        Message::Int(n) => {
            w.push(M_INT);
            w.extend_from_slice(&n.to_be_bytes());
        }
        Message::BigInt(s) => {
            w.push(M_BIGINT);
            put_str(w, s);
        }
        Message::Decimal(s) => {
            w.push(M_DECIMAL);
            put_str(w, s);
        }
        Message::Ratio(s) => {
            w.push(M_RATIO);
            put_str(w, s);
        }
        Message::Float(f) => {
            w.push(M_FLOAT);
            w.extend_from_slice(&f.to_bits().to_be_bytes());
        }
        Message::Str(s) => {
            w.push(M_STR);
            put_str(w, s);
        }
        // Shared blobs cannot cross a runtime boundary — separate runtimes
        // have independent `Arc<BlobHeap>` lifetimes. Encode the bytes inline
        // as a plain string; the receiver's `from_message` re-routes through
        // `alloc_string`, so anything still at-or-above
        // `SHARED_BLOB_THRESHOLD` rebecomes Shared on the destination side
        // (with a fresh `Arc`, no shared identity with the sender). The wire
        // format intentionally has no separate tag for shared blobs.
        Message::StrShared(blob) => {
            w.push(M_STR);
            put_str(
                w,
                std::str::from_utf8(blob.as_bytes())
                    .expect("shared blob bytes are valid UTF-8 by construction"),
            );
        }
        Message::Sym(s) => {
            w.push(M_SYM);
            put_sym(w, *s);
        }
        Message::Keyword(s) => {
            w.push(M_KEYWORD);
            put_sym(w, *s);
        }
        Message::Native(s) => {
            w.push(M_NATIVE);
            put_sym(w, *s);
        }
        Message::List(items, pos) => {
            w.push(M_LIST);
            put_u32(w, items.len() as u32);
            for it in items {
                encode_msg(w, it)?;
            }
            // Optional source position trailer — one byte for presence, then
            // line/col as u32 each when set. Trailing so a reader that didn't
            // expect it can stop early on the count, but every encoder/decoder
            // pair after this revision writes it. See `Message::List`'s docs.
            put_opt_pos(w, *pos);
        }
        Message::Vector(items) => {
            w.push(M_VECTOR);
            put_u32(w, items.len() as u32);
            for it in items {
                encode_msg(w, it)?;
            }
        }
        Message::Map(entries) => {
            w.push(M_MAP);
            put_u32(w, entries.len() as u32);
            for (k, v) in entries {
                encode_msg(w, k)?;
                encode_msg(w, v)?;
            }
        }
        Message::Set(items) => {
            w.push(M_SET);
            put_u32(w, items.len() as u32);
            for it in items {
                encode_msg(w, it)?;
            }
        }
        Message::Ref(n) => {
            w.push(M_REF);
            w.extend_from_slice(&n.to_be_bytes());
        }
        Message::Pid { node, id } => {
            w.push(M_PID);
            put_sym(w, *node);
            w.extend_from_slice(&id.to_be_bytes());
        }
        Message::Closure(c) => {
            w.push(M_CLOSURE);
            encode_closure(w, c)?;
        }
        Message::Socket(_) => {
            // A socket id is local to one runtime's global registry; it has no
            // meaning on a peer node. Refuse rather than ship a dangling handle.
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot send a socket across nodes; it is local to its runtime",
            ));
        }
        Message::Subprocess(_) => {
            // A subprocess id is local to one runtime's global registry (and names
            // an OS process on this host); it has no meaning on a peer node.
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot send a subprocess across nodes; it is local to its runtime",
            ));
        }
        Message::Table(_) => {
            // A table id is local to one runtime's global registry; it has no meaning
            // on a peer node. Refuse rather than ship a dangling handle. (Send the
            // table's snapshot — an ordinary map — across nodes instead.)
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot send a table across nodes; it is local to its runtime",
            ));
        }
        Message::FnShared { .. } => {
            // A RUNTIME handle names a slot in *this* runtime's shared code region. Another
            // node has its own region, so the handle is meaningless there — and would silently
            // resolve to whatever unrelated code occupies that index. Refuse. `send` only ever
            // produces this variant for a same-runtime local target, so reaching here means a
            // routing bug rather than user error, but it is cheap to make unreachable-by-type.
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot send a shared closure handle across nodes; it is local to its runtime",
            ));
        }
        Message::Bytes(blob) => {
            // Immutable bytes, copied inline (a length-prefixed raw blob — it isn't UTF-8, so
            // it can't ride the `M_STR` path). The receiver allocates its own `SharedBlob`, so
            // there is no shared-Arc lifetime to worry about across the boundary. Carries the
            // startup image's `#b"…"` literals and a genuine cross-node byte send alike.
            w.push(M_BYTES);
            put_bytes(w, blob.as_bytes());
        }
    }
    Ok(())
}

/// Wire form of a `ClosureMsg`. Same field order as the struct; symbols travel
/// by name (separate runtimes have independent interners — see [`put_sym`]).
/// Two callouts:
///   - Symbol/string optionals carry a 1-byte `0`/`1` tag, then the value
///     when present. Cheap and unambiguous in a stream codec.
///   - Body/optional-default *forms* are already `Message`s (S-expression
///     data), so they recurse through [`encode_msg`] — code travels exactly
///     like any other data.
fn encode_closure(w: &mut Vec<u8>, c: &crate::process::ClosureMsg) -> io::Result<()> {
    put_opt_sym(w, c.name);
    // One block per arity arm: params, optionals (sym + default), rest, body.
    put_u32(w, c.arms.len() as u32);
    for arm in &c.arms {
        put_u32(w, arm.params.len() as u32);
        for &s in &arm.params {
            put_sym(w, s);
        }
        put_u32(w, arm.optionals.len() as u32);
        for (s, m) in &arm.optionals {
            put_sym(w, *s);
            encode_msg(w, m)?;
        }
        put_opt_sym(w, arm.rest);
        put_u32(w, arm.body.len() as u32);
        for m in &arm.body {
            encode_msg(w, m)?;
        }
    }
    put_opt_str(w, c.doc.as_deref());
    // Modules the receiver must load before this body can run (KI-55): `(module, probe)`
    // symbol pairs, by name like every other symbol here. Almost always zero-length, which
    // costs the four bytes of the count.
    put_u32(w, c.modules.len() as u32);
    for need in &c.modules {
        put_sym(w, need.module);
        put_sym(w, need.probe);
    }
    put_u32(w, c.captured.len() as u32);
    for (s, m) in &c.captured {
        put_sym(w, *s);
        encode_msg(w, m)?;
    }
    Ok(())
}

/// Maximum nesting depth the wire decoder will descend into. Past this we
/// reject the frame as `InvalidData` — a peer (already authenticated, but
/// possibly malicious) can otherwise send a deeply nested `M_LIST` chain in a
/// small frame and overflow the receiver thread's native Rust stack. Defined in
/// terms of the serialiser's cap (`process::MAX_MESSAGE_DEPTH`) so the encode
/// and decode sides can't drift apart — round-trip stays symmetric.
const MAX_DECODE_DEPTH: u32 = crate::process::MAX_MESSAGE_DEPTH;

pub(crate) fn decode_msg(r: &mut Cursor<Vec<u8>>) -> io::Result<Message> {
    decode_msg_at(r, 0)
}

fn decode_msg_at(r: &mut Cursor<Vec<u8>>, depth: u32) -> io::Result<Message> {
    if depth >= MAX_DECODE_DEPTH {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message nested deeper than {MAX_DECODE_DEPTH} levels"),
        ));
    }
    Ok(match get_u8(r)? {
        M_NIL => Message::Nil,
        M_FALSE => Message::Bool(false),
        M_TRUE => Message::Bool(true),
        M_INT => Message::Int(get_i64(r)?),
        M_BIGINT => Message::BigInt(get_str(r)?),
        M_DECIMAL => Message::Decimal(get_str(r)?),
        M_RATIO => Message::Ratio(get_str(r)?),
        M_FLOAT => Message::Float(f64::from_bits(get_u64(r)?)),
        M_STR => Message::Str(get_str(r)?),
        M_SYM => Message::Sym(get_sym(r)?),
        M_KEYWORD => Message::Keyword(get_sym(r)?),
        M_NATIVE => Message::Native(get_sym(r)?),
        M_BYTES => Message::Bytes(SharedBlob::new(&get_bytes(r)?)),
        M_LIST => {
            let n = get_u32(r)? as usize;
            let mut items = Vec::with_capacity(prealloc(r, n));
            for _ in 0..n {
                items.push(decode_msg_at(r, depth + 1)?);
            }
            let pos = get_opt_pos(r)?;
            Message::List(items, pos)
        }
        M_VECTOR => {
            let n = get_u32(r)? as usize;
            let mut items = Vec::with_capacity(prealloc(r, n));
            for _ in 0..n {
                items.push(decode_msg_at(r, depth + 1)?);
            }
            Message::Vector(items)
        }
        M_MAP => {
            let n = get_u32(r)? as usize;
            let mut entries = Vec::with_capacity(prealloc(r, n));
            for _ in 0..n {
                let k = decode_msg_at(r, depth + 1)?;
                let v = decode_msg_at(r, depth + 1)?;
                entries.push((k, v));
            }
            Message::Map(entries)
        }
        M_SET => {
            let n = get_u32(r)? as usize;
            let mut items = Vec::with_capacity(prealloc(r, n));
            for _ in 0..n {
                items.push(decode_msg_at(r, depth + 1)?);
            }
            Message::Set(items)
        }
        M_REF => Message::Ref(get_u64(r)?),
        M_PID => Message::Pid {
            node: get_sym(r)?,
            id: get_u64(r)?,
        },
        M_CLOSURE => Message::Closure(Box::new(decode_closure_at(r, depth + 1)?)),
        t => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown message tag {t}"),
            ))
        }
    })
}

/// Inverse of [`encode_closure`]. Each `Vec`'s length is bounded by the
/// frame's remaining bytes (via [`prealloc`]) so a tiny frame claiming a huge
/// count can't trigger a large allocation up front — the decode loop fails
/// cleanly on EOF instead.
fn decode_closure_at(
    r: &mut Cursor<Vec<u8>>,
    depth: u32,
) -> io::Result<crate::process::ClosureMsg> {
    let name = get_opt_sym(r)?;
    let n_arms = get_u32(r)? as usize;
    let mut arms = Vec::with_capacity(prealloc(r, n_arms));
    for _ in 0..n_arms {
        let n = get_u32(r)? as usize;
        let mut params = Vec::with_capacity(prealloc(r, n));
        for _ in 0..n {
            params.push(get_sym(r)?);
        }
        let n = get_u32(r)? as usize;
        let mut optionals = Vec::with_capacity(prealloc(r, n));
        for _ in 0..n {
            let s = get_sym(r)?;
            let m = decode_msg_at(r, depth)?;
            optionals.push((s, m));
        }
        let rest = get_opt_sym(r)?;
        let n = get_u32(r)? as usize;
        let mut body = Vec::with_capacity(prealloc(r, n));
        for _ in 0..n {
            body.push(decode_msg_at(r, depth)?);
        }
        arms.push(crate::process::ClosureArmMsg {
            params,
            optionals,
            rest,
            body,
        });
    }
    let doc = get_opt_str(r)?;
    let n = get_u32(r)? as usize;
    let mut modules = Vec::with_capacity(prealloc(r, n));
    for _ in 0..n {
        let module = get_sym(r)?;
        let probe = get_sym(r)?;
        modules.push(crate::process::message::ModuleNeed { module, probe });
    }
    let n = get_u32(r)? as usize;
    let mut captured = Vec::with_capacity(prealloc(r, n));
    for _ in 0..n {
        let s = get_sym(r)?;
        let m = decode_msg_at(r, depth)?;
        captured.push((s, m));
    }
    Ok(crate::process::ClosureMsg {
        name,
        arms,
        doc,
        captured,
        modules,
    })
}

// ----- byte helpers ----------------------------------------------------------

fn put_u32(w: &mut Vec<u8>, n: u32) {
    w.extend_from_slice(&n.to_be_bytes());
}

fn put_str(w: &mut Vec<u8>, s: &str) {
    put_u32(w, s.len() as u32);
    w.extend_from_slice(s.as_bytes());
}

/// Length-prefixed raw bytes (mirror of [`put_str`] without the UTF-8 assumption).
fn put_bytes(w: &mut Vec<u8>, b: &[u8]) {
    put_u32(w, b.len() as u32);
    w.extend_from_slice(b);
}

/// A symbol is encoded **by name** — separate runtimes have independent
/// interners, so the id is meaningless across the wire.
fn put_sym(w: &mut Vec<u8>, s: Symbol) {
    put_str(w, &value::symbol_name(s));
}

/// `Option<Symbol>` as a `0`/`1` presence tag + the symbol's name when set.
/// One byte cheaper than encoding `nil` as a sentinel name, and unambiguous
/// in a stream codec.
fn put_opt_sym(w: &mut Vec<u8>, s: Option<Symbol>) {
    match s {
        Some(s) => {
            w.push(1);
            put_sym(w, s);
        }
        None => w.push(0),
    }
}

/// `Option<&str>` with the same `0`/`1` tag shape as [`put_opt_sym`].
fn put_opt_str(w: &mut Vec<u8>, s: Option<&str>) {
    match s {
        Some(s) => {
            w.push(1);
            put_str(w, s);
        }
        None => w.push(0),
    }
}

fn get_opt_sym(r: &mut Cursor<Vec<u8>>) -> io::Result<Option<Symbol>> {
    match get_u8(r)? {
        0 => Ok(None),
        1 => Ok(Some(get_sym(r)?)),
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad Option<Symbol> tag {t}"),
        )),
    }
}

fn get_opt_str(r: &mut Cursor<Vec<u8>>) -> io::Result<Option<String>> {
    match get_u8(r)? {
        0 => Ok(None),
        1 => Ok(Some(get_str(r)?)),
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad Option<String> tag {t}"),
        )),
    }
}

/// `Option<Pos>` for the trailing source-position on `Message::List`. Same
/// `0`/`1` presence tag as the other `put_opt_*` helpers; on `1` the body is
/// two `u32`s (1-based line and column, as the reader records them).
fn put_opt_pos(w: &mut Vec<u8>, p: Option<crate::error::Pos>) {
    match p {
        Some(p) => {
            w.push(1);
            put_u32(w, p.line);
            put_u32(w, p.col);
        }
        None => w.push(0),
    }
}

fn get_opt_pos(r: &mut Cursor<Vec<u8>>) -> io::Result<Option<crate::error::Pos>> {
    match get_u8(r)? {
        0 => Ok(None),
        1 => Ok(Some(crate::error::Pos {
            line: get_u32(r)?,
            col: get_u32(r)?,
        })),
        t => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad Option<Pos> tag {t}"),
        )),
    }
}

/// Bytes left in the frame buffer. Used to bound allocations by what the frame
/// could actually contain — a count/length field is attacker-controlled, but the
/// buffer is already capped at [`MAX_FRAME`], so an element can't be smaller than
/// one byte and `n` items need at least `n` bytes.
fn remaining(r: &Cursor<Vec<u8>>) -> usize {
    (r.get_ref().len() as u64).saturating_sub(r.position()) as usize
}

/// Upper bound on a single up-front collection reservation. `remaining()` bounds
/// the claimed *count* (an item needs ≥1 wire byte), but [`prealloc`]'s result is
/// fed to `Vec::with_capacity`, which allocates `cap × size_of::<Element>()` bytes
/// — elements are 48–96 B (`Message`, `(Message, Message)`, …), so capping by the
/// byte count alone still lets a near-[`MAX_FRAME`] (64 MiB) frame reserve
/// gigabytes (64M × 96 B ≈ 6 GiB) up front. Capping the *reservation* to a small
/// constant removes that amplification: a genuinely large collection just grows
/// the `Vec` (amortized doubling) as its items are actually decoded.
const PREALLOC_CAP: usize = 4096;

/// A safe pre-allocation size for a claimed count of `n` items: never reserve more
/// than the frame's remaining bytes can hold (a tiny frame claiming a huge count
/// can't pre-reserve past its own length) *and* never more than [`PREALLOC_CAP`]
/// elements up front (so a large frame can't be amplified by the element size into
/// a multi-GiB reservation). A larger real collection grows the `Vec` as it
/// decodes; an over-claimed count still fails cleanly on EOF.
fn prealloc(r: &Cursor<Vec<u8>>, n: usize) -> usize {
    n.min(remaining(r)).min(PREALLOC_CAP)
}

fn get_u8(r: &mut Cursor<Vec<u8>>) -> io::Result<u8> {
    let mut b = [0u8; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn get_u32(r: &mut Cursor<Vec<u8>>) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn get_u64(r: &mut Cursor<Vec<u8>>) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}

fn get_i64(r: &mut Cursor<Vec<u8>>) -> io::Result<i64> {
    Ok(get_u64(r)? as i64)
}

fn get_str(r: &mut Cursor<Vec<u8>>) -> io::Result<String> {
    let n = get_u32(r)? as usize;
    // A string can't be longer than the bytes left in the frame; reject before
    // allocating, so a small frame claiming a huge length can't OOM us.
    if n > remaining(r) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "string length exceeds frame",
        ));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "bad utf8"))
}

/// Read length-prefixed raw bytes (mirror of [`get_str`] without the UTF-8 check). Bounds the
/// claimed length against the frame first, so a small frame can't force a huge allocation.
fn get_bytes(r: &mut Cursor<Vec<u8>>) -> io::Result<Vec<u8>> {
    let n = get_u32(r)? as usize;
    if n > remaining(r) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bytes length exceeds frame",
        ));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Read a fixed-size byte array from the frame. Used by the handshake for the
/// nonce + MAC fields (both 32 bytes). Errors cleanly on EOF — no allocation
/// past `N` even on a malformed frame.
pub(super) fn get_fixed<const N: usize>(r: &mut Cursor<Vec<u8>>) -> io::Result<[u8; N]> {
    let mut buf = [0u8; N];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Read a symbol name and re-intern it into *this* runtime's interner.
fn get_sym(r: &mut Cursor<Vec<u8>>) -> io::Result<Symbol> {
    Ok(value::intern(&get_str(r)?))
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod tests;
