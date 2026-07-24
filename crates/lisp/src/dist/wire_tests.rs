
use super::*;

/// Encode a frame (with its length prefix) and decode it back.
fn read_full(frame: &Frame) -> Frame {
    let bytes = frame_bytes(frame).unwrap();
    read_frame(&mut Cursor::new(bytes)).unwrap()
}

#[test]
fn hello_roundtrips() {
    let nonce = [7u8; NONCE_LEN];
    let eph_pub = [9u8; EPH_PUB_LEN];
    let f = Frame::Hello {
        node: value::intern("alpha"),
        nonce,
        eph_pub,
        addr: "tcp:127.0.0.1:9000".to_string(),
    };
    match read_full(&f) {
        Frame::Hello {
            node,
            nonce: n2,
            eph_pub: e2,
            addr,
        } => {
            assert_eq!(value::symbol_name(node), "alpha");
            assert_eq!(n2, nonce);
            assert_eq!(e2, eph_pub);
            assert_eq!(addr, "tcp:127.0.0.1:9000");
        }
        _ => panic!("wrong frame"),
    }
}

#[test]
fn peers_gossip_roundtrips() {
    // The cluster-mesh frame: a list of (node-name, dial-addr) pairs. Names
    // travel by spelling (re-interned on decode); addresses are plain strings.
    let f = Frame::Peers {
        peers: vec![
            (
                value::intern("b@127.0.0.1"),
                "tcp:127.0.0.1:9002".to_string(),
            ),
            (
                value::intern("c@127.0.0.1"),
                "unix:/run/brood/c.sock".to_string(),
            ),
        ],
    };
    match read_full(&f) {
        Frame::Peers { peers } => {
            assert_eq!(peers.len(), 2);
            assert_eq!(value::symbol_name(peers[0].0), "b@127.0.0.1");
            assert_eq!(peers[0].1, "tcp:127.0.0.1:9002");
            assert_eq!(value::symbol_name(peers[1].0), "c@127.0.0.1");
            assert_eq!(peers[1].1, "unix:/run/brood/c.sock");
        }
        _ => panic!("wrong frame"),
    }
}

#[test]
fn oversized_gossip_count_is_rejected() {
    // A Peers frame claiming more entries than the cap must error at the
    // count check — before the decode loop — so one frame can't fan out into
    // an unbounded number of dial threads.
    let mut payload = vec![FRAME_PEERS];
    payload.extend_from_slice(&(MAX_GOSSIP_PEERS as u32 + 1).to_be_bytes());
    let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(&payload);
    match read_frame(&mut Cursor::new(framed)) {
        Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
        Ok(_) => panic!("a gossip frame over the cap should be rejected"),
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
fn handshake_cap_rejects_a_frame_over_the_small_ceiling_pre_auth() {
    // A frame whose length prefix is within MAX_FRAME but over the tiny
    // handshake ceiling must be rejected at the length check — never
    // allocated — so an unauthenticated peer can't force a big buffer with
    // a few probe bytes. (8 KiB > the 4 KiB handshake cap, < 64 MiB MAX_FRAME.)
    let mut bytes = (8 * 1024u32).to_be_bytes().to_vec();
    bytes.push(M_NIL); // a token byte; we must fail before reading a body
    match read_frame_capped(&mut Cursor::new(bytes), 4 * 1024) {
        Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
        Ok(_) => panic!("a frame over the handshake cap should be rejected"),
    }
    // …and the same bytes are fine under the steady-state ceiling — proving
    // the cap is the gate, not a malformed frame. (It'll EOF on the missing
    // body, not reject on length.)
    let mut bytes = (8 * 1024u32).to_be_bytes().to_vec();
    bytes.push(M_NIL);
    match read_frame_capped(&mut Cursor::new(bytes), MAX_FRAME) {
        Err(e) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
        Ok(_) => panic!("the truncated body should EOF, not parse"),
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

#[test]
fn prealloc_caps_the_reservation_against_element_size_amplification() {
    // The tiny-frame case above is bounded by `remaining`. The remaining gap
    // (kernel audit #5): a LARGE frame (lots of remaining bytes) claiming a
    // huge count must not reserve `remaining` *elements* up front — that's
    // `remaining × size_of::<Element>()` bytes (≈6 GiB for a 64 MiB frame of
    // 96-byte Map entries). `prealloc` caps the reservation at `PREALLOC_CAP`;
    // a genuinely larger collection grows its `Vec` as it decodes.
    let r = Cursor::new(vec![0u8; 16 * 1024 * 1024]); // 16 MiB of "remaining"
    assert_eq!(remaining(&r), 16 * 1024 * 1024);
    assert_eq!(prealloc(&r, usize::MAX), PREALLOC_CAP);
    assert_eq!(prealloc(&r, 10 * 1024 * 1024), PREALLOC_CAP);
    // A small claim is still honoured exactly — prealloc stays useful.
    assert_eq!(prealloc(&r, 7), 7);
    // And a tiny frame is still bounded by its own remaining bytes.
    assert_eq!(prealloc(&Cursor::new(vec![0u8; 5]), usize::MAX), 5);
}
