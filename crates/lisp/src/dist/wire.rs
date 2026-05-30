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

use crate::core::value::{self, Symbol};
use crate::process::Message;

use super::{Target, MAX_FRAME};

/// Frames travel over the wire as `[u32 len][payload]`. `pub(super)` so the
/// connection-lifecycle code in `dist::mod` can construct and pattern-match
/// them; the codec is otherwise private to this module.
pub(super) enum Frame {
    /// Handshake step 1 & 2: who I am + a fresh nonce I want you to MAC. The
    /// cookie never travels — it's an HMAC key, not a credential. Both sides
    /// send a `Hello` (initiator first, responder second); each computes its
    /// `Auth` over the peer's nonce.
    Hello {
        node: Symbol,
        nonce: [u8; NONCE_LEN],
    },
    /// Handshake step 3 & 4: `HMAC-SHA256(cookie, peer_nonce || peer_name ||
    /// my_name)` — proves possession of the cookie without disclosing it.
    /// Mismatch on either side aborts before the link enters `NODES`.
    Auth { mac: [u8; MAC_LEN] },
    /// Route `msg` to `target` on the receiving node.
    Send { target: Target, msg: Message },
    /// Liveness probe; the peer answers with `Pong`.
    Ping,
    /// Reply to a `Ping`. (Receiving any frame refreshes liveness; these two carry
    /// no payload, just keep an idle link demonstrably alive.)
    Pong,
    /// "Watch local pid `target` for me; deliver `[:down ref pid reason]` to
    /// my `watcher_pid` (on this sender's `from_node`) when it dies." The
    /// receiver routes through `process::add_monitor` with a
    /// `Watcher::Remote`, reusing the local "alive? register; dead? fire
    /// :noproc" logic — same code path, just a different watcher variant.
    Monitor {
        from_node: Symbol,
        watcher_pid: u64,
        target: u64,
        mref: u64,
    },
    /// Drop the matching remote watcher (best effort; identified by sender's
    /// node + pid + mref). Goes through `process::drop_monitor`, the same
    /// dropper local `demonitor` uses.
    Demonitor {
        from_node: Symbol,
        watcher_pid: u64,
        mref: u64,
    },
    /// "Link my `from_pid` (on `from_node`) to your local `to_pid`" (ADR-067).
    /// The receiver records its half in `links::REMOTE_LINKS` so either side's
    /// death — or a net-split — reaches the other. Symmetric: each node keeps
    /// `local_pid → (peer_node, peer_pid)`.
    Link {
        from_node: Symbol,
        from_pid: u64,
        to_pid: u64,
    },
    /// Drop the cross-node link `from_pid@from_node ↔ to_pid` (best effort).
    Unlink {
        from_node: Symbol,
        from_pid: u64,
        to_pid: u64,
    },
    /// An exit signal for local `to_pid`. `link = true` is a **link death**:
    /// `from_pid@from_node` (a linked peer) exited with `reason`, delivered via
    /// the trap-or-propagate path (a trapping target gets `[:EXIT pid reason]`).
    /// `link = false` is an explicit remote `(exit pid reason)` — routed straight
    /// to `scheduler::exit` (kill-style, like the local builtin).
    Exit {
        from_node: Symbol,
        from_pid: u64,
        to_pid: u64,
        reason: Message,
        link: bool,
    },
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
const TARGET_PID: u8 = 0;
const TARGET_NAME: u8 = 1;

/// Protocol magic + version byte sent before any frame. `b"BRD"` lets a
/// `tcpdump` reader recognise the protocol; the trailing version byte gates
/// future wire-format changes — a v2 peer that sees anything else aborts
/// before allocating buffers. The v1 protocol (plaintext cookie in Hello)
/// has been retired: this is greenfield, so we don't preserve compatibility.
pub(super) const PROTOCOL_MAGIC: [u8; 4] = *b"BRD\x02";
pub(super) const NONCE_LEN: usize = 32;
pub(super) const MAC_LEN: usize = 32;

/// Encode a frame with its `[u32 len][payload]` length prefix, ready to write.
/// A payload over [`MAX_FRAME`] is rejected here too — symmetric with
/// `read_frame` — so an oversized local `(send pid huge-thing)` returns a clean
/// error rather than silently truncating the `u32` length and producing a frame
/// the peer can't parse.
pub(super) fn frame_bytes(frame: &Frame) -> io::Result<Vec<u8>> {
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
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

pub(super) fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    w.write_all(&frame_bytes(frame)?)
}

/// Read one length-prefixed frame, rejecting an over-large prefix before
/// allocating for it.
pub(super) fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {MAX_FRAME}-byte limit"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    decode_frame(&mut Cursor::new(buf))
}

fn encode_frame(w: &mut Vec<u8>, frame: &Frame) -> io::Result<()> {
    match frame {
        Frame::Hello { node, nonce } => {
            w.push(FRAME_HELLO);
            put_sym(w, *node);
            w.extend_from_slice(nonce);
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
            from_node,
            watcher_pid,
            target,
            mref,
        } => {
            w.push(FRAME_MONITOR);
            put_sym(w, *from_node);
            w.extend_from_slice(&watcher_pid.to_be_bytes());
            w.extend_from_slice(&target.to_be_bytes());
            w.extend_from_slice(&mref.to_be_bytes());
        }
        Frame::Demonitor {
            from_node,
            watcher_pid,
            mref,
        } => {
            w.push(FRAME_DEMONITOR);
            put_sym(w, *from_node);
            w.extend_from_slice(&watcher_pid.to_be_bytes());
            w.extend_from_slice(&mref.to_be_bytes());
        }
        Frame::Link {
            from_node,
            from_pid,
            to_pid,
        } => {
            w.push(FRAME_LINK);
            put_sym(w, *from_node);
            w.extend_from_slice(&from_pid.to_be_bytes());
            w.extend_from_slice(&to_pid.to_be_bytes());
        }
        Frame::Unlink {
            from_node,
            from_pid,
            to_pid,
        } => {
            w.push(FRAME_UNLINK);
            put_sym(w, *from_node);
            w.extend_from_slice(&from_pid.to_be_bytes());
            w.extend_from_slice(&to_pid.to_be_bytes());
        }
        Frame::Exit {
            from_node,
            from_pid,
            to_pid,
            reason,
            link,
        } => {
            w.push(FRAME_EXIT);
            put_sym(w, *from_node);
            w.extend_from_slice(&from_pid.to_be_bytes());
            w.extend_from_slice(&to_pid.to_be_bytes());
            w.push(*link as u8);
            encode_msg(w, reason)?;
        }
    }
    Ok(())
}

fn decode_frame(r: &mut Cursor<Vec<u8>>) -> io::Result<Frame> {
    match get_u8(r)? {
        FRAME_HELLO => Ok(Frame::Hello {
            node: get_sym(r)?,
            nonce: get_fixed::<NONCE_LEN>(r)?,
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
            from_node: get_sym(r)?,
            watcher_pid: get_u64(r)?,
            target: get_u64(r)?,
            mref: get_u64(r)?,
        }),
        FRAME_DEMONITOR => Ok(Frame::Demonitor {
            from_node: get_sym(r)?,
            watcher_pid: get_u64(r)?,
            mref: get_u64(r)?,
        }),
        FRAME_LINK => Ok(Frame::Link {
            from_node: get_sym(r)?,
            from_pid: get_u64(r)?,
            to_pid: get_u64(r)?,
        }),
        FRAME_UNLINK => Ok(Frame::Unlink {
            from_node: get_sym(r)?,
            from_pid: get_u64(r)?,
            to_pid: get_u64(r)?,
        }),
        FRAME_EXIT => Ok(Frame::Exit {
            from_node: get_sym(r)?,
            from_pid: get_u64(r)?,
            to_pid: get_u64(r)?,
            link: get_u8(r)? != 0,
            reason: decode_msg(r)?,
        }),
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

fn encode_msg(w: &mut Vec<u8>, m: &Message) -> io::Result<()> {
    match m {
        Message::Nil => w.push(M_NIL),
        Message::Bool(false) => w.push(M_FALSE),
        Message::Bool(true) => w.push(M_TRUE),
        Message::Int(n) => {
            w.push(M_INT);
            w.extend_from_slice(&n.to_be_bytes());
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
/// small frame and overflow the receiver thread's native Rust stack.
const MAX_DECODE_DEPTH: u32 = 256;

fn decode_msg(r: &mut Cursor<Vec<u8>>) -> io::Result<Message> {
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
        M_FLOAT => Message::Float(f64::from_bits(get_u64(r)?)),
        M_STR => Message::Str(get_str(r)?),
        M_SYM => Message::Sym(get_sym(r)?),
        M_KEYWORD => Message::Keyword(get_sym(r)?),
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

/// A safe pre-allocation size for a claimed count of `n` items: never reserve
/// more than the frame's remaining bytes can hold, so a tiny frame claiming a
/// huge count can't trigger a giant up-front allocation (the decode loop then
/// fails cleanly on EOF).
fn prealloc(r: &Cursor<Vec<u8>>, n: usize) -> usize {
    n.min(remaining(r))
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
mod tests {
    use super::*;

    /// Encode a frame (with its length prefix) and decode it back.
    fn read_full(frame: &Frame) -> Frame {
        let bytes = frame_bytes(frame).unwrap();
        read_frame(&mut Cursor::new(bytes)).unwrap()
    }

    #[test]
    fn hello_roundtrips() {
        let nonce = [7u8; NONCE_LEN];
        let f = Frame::Hello {
            node: value::intern("alpha"),
            nonce,
        };
        match read_full(&f) {
            Frame::Hello { node, nonce: n2 } => {
                assert_eq!(value::symbol_name(node), "alpha");
                assert_eq!(n2, nonce);
            }
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn auth_roundtrips() {
        let mac = [0xabu8; MAC_LEN];
        let f = Frame::Auth { mac };
        match read_full(&f) {
            Frame::Auth { mac: m2 } => assert_eq!(m2, mac),
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn send_with_rich_message_roundtrips() {
        // A message exercising symbols/keywords/pids/maps/nesting — the symbol
        // fields must survive as *names* (re-interned on decode).
        let msg = Message::Vector(vec![
            Message::Keyword(value::intern("pong")),
            Message::Pid {
                node: value::intern("beta"),
                id: 7,
            },
            Message::Map(vec![(
                Message::Keyword(value::intern("status")),
                Message::Sym(value::intern("ok")),
            )]),
            Message::Int(-42),
            Message::Str("hi".to_string()),
        ]);
        let f = Frame::Send {
            target: Target::Name(value::intern("echo")),
            msg,
        };
        match read_full(&f) {
            Frame::Send { target, msg } => {
                match target {
                    Target::Name(s) => assert_eq!(value::symbol_name(s), "echo"),
                    _ => panic!("wrong target"),
                }
                match msg {
                    Message::Vector(items) => {
                        assert!(
                            matches!(&items[0], Message::Keyword(k) if value::symbol_name(*k) == "pong")
                        );
                        assert!(
                            matches!(&items[1], Message::Pid { node, id } if value::symbol_name(*node) == "beta" && *id == 7)
                        );
                    }
                    _ => panic!("wrong message"),
                }
            }
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn pid_id_survives_above_u32() {
        // The local id is u64 end-to-end (the scheduler counter is u64); a value
        // past u32::MAX must round-trip, not truncate.
        let big = (u32::MAX as u64) + 12345;
        let f = Frame::Send {
            target: Target::Pid(big),
            msg: Message::Pid {
                node: value::intern("n"),
                id: big,
            },
        };
        match read_full(&f) {
            Frame::Send {
                target: Target::Pid(t),
                msg: Message::Pid { id, .. },
            } => {
                assert_eq!(t, big);
                assert_eq!(id, big);
            }
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn oversized_length_prefix_is_rejected_not_allocated() {
        // A 4-byte prefix claiming ~4 GiB must error, never `vec![0; 4e9]`.
        let mut bytes = (u32::MAX).to_be_bytes().to_vec();
        bytes.push(M_NIL); // a token byte of "payload"
        match read_frame(&mut Cursor::new(bytes)) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
            Ok(_) => panic!("oversized frame should be rejected"),
        }
    }

    #[test]
    fn closure_roundtrips_through_the_wire() {
        // A `ClosureMsg` exercising every optional + every list — the kind a
        // real `(fn (a &optional (b 10) &) … )` would serialise to. Captures
        // are stand-ins for free locals copied from the sender's frame; on
        // the receiver they chain onto its global scope.
        use crate::process::{ClosureArmMsg, ClosureMsg};
        // TWO arms, so the round-trip exercises multi-arity dispatch over the
        // wire: a fixed `(a)` arm and a variadic `(a &optional (c 10) & xs)` arm.
        let c = ClosureMsg {
            name: Some(value::intern("worker")),
            arms: vec![
                ClosureArmMsg {
                    params: vec![value::intern("a")],
                    optionals: vec![],
                    rest: None,
                    body: vec![Message::Sym(value::intern("a"))],
                },
                ClosureArmMsg {
                    params: vec![value::intern("a"), value::intern("b")],
                    optionals: vec![(value::intern("c"), Message::Int(10))],
                    rest: Some(value::intern("xs")),
                    // (a body of `(+ a b c)` — just the *message* form of it, with
                    // a source position so the round-trip exercises the optional
                    // `pos` trailer on `Message::List` too)
                    body: vec![Message::List(
                        vec![
                            Message::Sym(value::intern("+")),
                            Message::Sym(value::intern("a")),
                            Message::Sym(value::intern("b")),
                            Message::Sym(value::intern("c")),
                        ],
                        Some(crate::error::Pos { line: 7, col: 3 }),
                    )],
                },
            ],
            doc: Some("add three".to_string()),
            captured: vec![(value::intern("seed"), Message::Int(42))],
        };
        let f = Frame::Send {
            target: Target::Pid(1),
            msg: Message::Closure(Box::new(c)),
        };
        match read_full(&f) {
            Frame::Send {
                msg: Message::Closure(c),
                ..
            } => {
                assert_eq!(value::symbol_name(c.name.unwrap()), "worker");
                assert_eq!(c.arms.len(), 2);
                // arm 0: fixed (a)
                assert_eq!(c.arms[0].params.len(), 1);
                assert_eq!(value::symbol_name(c.arms[0].params[0]), "a");
                assert!(c.arms[0].rest.is_none());
                // arm 1: (a b &optional (c 10) & xs)
                let arm = &c.arms[1];
                assert_eq!(arm.params.len(), 2);
                assert_eq!(value::symbol_name(arm.params[0]), "a");
                assert_eq!(arm.optionals.len(), 1);
                assert!(matches!(&arm.optionals[0].1, Message::Int(10)));
                assert_eq!(value::symbol_name(arm.rest.unwrap()), "xs");
                assert_eq!(arm.body.len(), 1);
                // The body form's source position survived the round-trip,
                // so a remote diagnostic can point at the sender's line.
                match &arm.body[0] {
                    Message::List(items, pos) => {
                        assert_eq!(items.len(), 4);
                        assert_eq!(*pos, Some(crate::error::Pos { line: 7, col: 3 }));
                    }
                    _ => panic!("body[0] should be Message::List"),
                }
                assert_eq!(c.doc.as_deref(), Some("add three"));
                assert_eq!(c.captured.len(), 1);
                assert!(matches!(&c.captured[0].1, Message::Int(42)));
            }
            other => panic!(
                "wrong frame after round-trip: {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn closure_with_all_options_absent_roundtrips() {
        // The minimal case: no name, no rest, no doc, no optionals, no captures —
        // a global-capturing `(fn (x) x)`. Each Option's 0/1 tag has to survive
        // cleanly, otherwise decoding would mis-align.
        use crate::process::{ClosureArmMsg, ClosureMsg};
        let c = ClosureMsg {
            name: None,
            arms: vec![ClosureArmMsg {
                params: vec![value::intern("x")],
                optionals: vec![],
                rest: None,
                body: vec![Message::Sym(value::intern("x"))],
            }],
            doc: None,
            captured: vec![],
        };
        let f = Frame::Send {
            target: Target::Pid(1),
            msg: Message::Closure(Box::new(c)),
        };
        match read_full(&f) {
            Frame::Send {
                msg: Message::Closure(c),
                ..
            } => {
                assert!(c.name.is_none());
                assert!(c.doc.is_none());
                assert!(c.captured.is_empty());
                assert_eq!(c.arms.len(), 1);
                assert!(c.arms[0].rest.is_none());
                assert!(c.arms[0].optionals.is_empty());
                assert_eq!(c.arms[0].params.len(), 1);
                assert_eq!(c.arms[0].body.len(), 1);
            }
            _ => panic!("wrong frame"),
        }
    }

    #[test]
    fn bogus_collection_count_errors_without_huge_alloc() {
        // A tiny frame whose list claims billions of elements: prealloc is bounded
        // by the remaining bytes, and decoding fails cleanly on EOF (no OOM).
        let mut payload = vec![FRAME_SEND];
        encode_target(&mut payload, &Target::Pid(1));
        payload.push(M_LIST);
        payload.extend_from_slice(&u32::MAX.to_be_bytes()); // claims 4 billion items
                                                            // …but no item bytes follow.
        let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(&payload);
        match read_frame(&mut Cursor::new(framed)) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
            Ok(_) => panic!("a list claiming more items than bytes should fail"),
        }
    }
}
