//! The listening side of the connection lifecycle: `node-start` (publishing this
//! runtime's identity and binding its first listener), `node-also-listen`, the shared
//! accept loop, and the server side of the handshake for each accepted connection.

use super::*;

// ----- connection lifecycle --------------------------------------------------

/// `(%node-listen name addr cookie)` — set this runtime's identity (name +
/// cookie) and listen for peers. `addr` carries the transport: `"unix:PATH"`
/// (local, addressed by name) or `"tcp:HOST:PORT"` (remote). Each accepted
/// connection is authenticated (cookie) and, on success, gets reader + writer
/// threads. Errors if this runtime is already a node — a second listener would
/// leak the first. The *policy* (socket path, cookie source, transport choice)
/// lives in `std/prelude.blsp`; this primitive is the mechanism (ADR-068).
pub(crate) fn node_listen(name: Symbol, addr: &str, cookie: String) -> io::Result<()> {
    // Guardrail: the cookie is the *entire* trust
    // boundary — a holder has remote code execution by design — so refuse one
    // short enough to guess or brute-force online. The default policy
    // (`node/cookie`) generates `(rand/token 32)`; this only
    // rejects a deliberately weak override (e.g. a short `$BROOD_COOKIE`).
    if cookie.len() < MIN_COOKIE_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "node cookie too short: {} bytes (minimum {MIN_COOKIE_LEN}) — \
                 a cookie-holder has full remote eval on this node; use the \
                 default (node/cookie) or e.g. (rand/token 32)",
                cookie.len()
            ),
        ));
    }
    // The guest namespace is the host's to mint in (ADR-425): every peer refuses a full
    // link under such a name, so a node started with one could link to nothing.
    if guest::is_guest_name(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "node name {} is reserved: names starting {:?} are what a host calls its \
                 guest links",
                value::symbol_name(name),
                guest::NAME_PREFIX
            ),
        ));
    }
    // Guard against a second node-start and publish identity atomically under
    // the same write lock — closing the TOCTOU window a separate read-check
    // + set_identity would leave. The acceptor reads identity lazily (at
    // accept time), so the write happens before any peer is served; if the
    // bind fails below, clear_identity rolls it back so node-start can retry.
    {
        let mut n = crate::core::sync::write(&NODE);
        if n.started {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "this runtime is already a node (node-start called twice)",
            ));
        }
        n.name = name;
        n.cookie = cookie;
        n.started = true;
    }
    LOCAL_NODE.store(name, Ordering::Release);
    if let Err(e) = start_listener(addr) {
        clear_identity();
        return Err(e);
    }
    Ok(())
}

/// `(%node-also-listen addr)` — add another listener (`unix:PATH` / `tcp:HOST:PORT`)
/// to an already-started node, so one node serves several transports at once
/// (ADR-074): a local Unix socket *and* a remote TCP endpoint — the editor-daemon
/// "reachable locally by name and remotely over the network" shape. Shares the
/// node's existing identity + cookie; errors if this runtime isn't a node yet.
pub(crate) fn node_also_listen(addr: &str) -> io::Result<()> {
    {
        let n = crate::core::sync::read(&NODE);
        if !n.started {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "node-also-listen: this runtime is not a node yet (call node-start first)",
            ));
        }
    }
    start_listener(addr).map(|_| ())
}

/// Bind one listener for `addr` and spawn its accept loop, returning the address it
/// records for peers to dial. Identity-agnostic — the per-connection handshake reads
/// `NODE` at accept time — so it serves both the first listener (`node_listen`) and any
/// added later (`node_also_listen`).
pub(super) fn start_listener(addr: &str) -> io::Result<String> {
    // What peers are told to dial: the BOUND address, never the requested one — a
    // requested port 0 would otherwise be advertised as port 0.
    let advertised = if let Some(path) = addr.strip_prefix("unix:") {
        bind_unix_listener(path)?;
        addr.to_string()
    } else if let Some(host_and_port) = addr.strip_prefix("tcp:") {
        let listener = TcpListener::bind(host_and_port)?;
        let bound = listener.local_addr()?;
        spawn_acceptor(move || listener.accept().map(|(s, _)| Stream::Tcp(s)));
        // A wildcard host (`0.0.0.0`, `::`) listens everywhere but is not an address
        // anyone can dial from another machine: advertise the node's own host instead.
        if bound.ip().is_unspecified() {
            format!("tcp:{}:{}", node_host(), bound.port())
        } else {
            format!("tcp:{bound}")
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("node address must start with 'unix:' or 'tcp:' (got {addr})"),
        ));
    };
    // Record only after a successful bind, so a failed listener doesn't leave a
    // dead address we'd advertise to peers.
    crate::core::sync::write(&LISTEN_ADDRS).push(advertised.clone());
    Ok(advertised)
}

/// The host this node is reachable at, for advertising a wildcard listener: the host
/// half of its qualified `name@host` identity (ADR-073), else `localhost`.
pub(super) fn node_host() -> String {
    let name = crate::core::sync::read(&NODE).name;
    value::symbol_name(name)
        .split_once('@')
        .map(|(_, host)| host.to_string())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

/// Roll back a failed `node-start` — used when the first listener's bind fails
/// so the runtime stays a non-node (retryable) rather than a node with no listener.
pub(super) fn clear_identity() {
    {
        let mut n = crate::core::sync::write(&NODE);
        n.name = *NONODE;
        n.cookie = String::new();
        n.started = false;
    }
    LOCAL_NODE.store(u32::MAX, Ordering::Release);
}

/// The accept loop, shared by both transports: pull the next link off `accept`
/// and hand each to a panic-isolated per-connection thread. A transient accept
/// error (EMFILE etc.) logs and re-loops with a tiny backoff rather than
/// burn-looping or killing the acceptor.
pub(super) fn spawn_acceptor(accept: impl FnMut() -> io::Result<Stream> + Send + 'static) {
    let mut accept = accept;
    spawn_bg("dist-acceptor", move || {
        loop {
            match accept() {
                Ok(stream) => {
                    // Shed past the in-flight-handshake cap *before* spawning a thread
                    // or reading a byte, so a flood of unauthenticated connections
                    // can't exhaust threads/memory. Closing the socket is the whole
                    // response, but not a *silent* one: reaching the cap means inbound
                    // links are being refused outright, which the node reports
                    // (rate-limited — see `note_shed_handshake`).
                    let permit = match HandshakeSlot::try_acquire() {
                        Some(p) => p,
                        None => {
                            note_shed_handshake();
                            let _ = stream.shutdown(Shutdown::Both);
                            continue;
                        }
                    };
                    // `spawn_bg`, not `thread::spawn`: a refused thread must not panic
                    // *inside the accept loop* (that would close the listener permanently);
                    // the connection is refused and the node keeps accepting.
                    spawn_bg("dist-conn", move || {
                        // Hold the slot until the handshake finishes (this thread ends
                        // right after `establish` hands off to the steady-state reader
                        // and writer threads, which don't hold a slot).
                        let _permit = permit;
                        // Catch a panic in the per-connection thread so one bad peer
                        // doesn't take down the runtime via thread-panic unwind (the
                        // rest of the dist surface assumes its background threads
                        // stay alive).
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            if let Err(e) = accept_link(stream) {
                                eprintln!("dist: incoming connection failed: {}", e);
                            }
                        }));
                    });
                    // No explicit shed on failure is needed: `Builder::spawn` drops the
                    // closure when it cannot start, which drops the accepted `stream` (closing
                    // the socket) and the `HandshakeSlot` permit with it. The connection is
                    // refused and the acceptor loops on, which is the whole point.
                }
                Err(e) => {
                    eprintln!("dist: accept error: {}", e);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    });
}

/// Server side of the handshake: drive the v2 exchange, then start the link
/// threads. See [`handshake`] for the protocol.
pub(super) fn accept_link(mut stream: Stream) -> io::Result<()> {
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    // The per-read timeout above bounds a peer that goes *silent*; the deadline
    // bounds one that stays slow. Only the second bounds how long an
    // unauthenticated peer can hold its `HandshakeSlot` (KI-97 item 1).
    let authenticated = {
        let mut guarded = Deadline::new(&mut stream, HANDSHAKE_DEADLINE);
        handshake(&mut guarded, Role::Responder)?
    };
    let peer = authenticated.peer;
    // Refuse a peer claiming to BE us — the accept-side counterpart of the check
    // `node_connect` already makes on the dial side. Reachable by a relay that
    // cross-wires two connections back to our own listener, or by a misconfigured
    // second node reusing our name. `establish` would otherwise register a
    // `NODES[our-own-name]` entry: mostly inert (`is_local()` still short-circuits
    // sends to our own name) but it produces heartbeat traffic, corrupts `(nodes)`
    // output, and emits a spurious `[:nodedown our-name]` on teardown.
    if peer == local_node() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "peer claims our own node name ({})",
                value::symbol_name(peer)
            ),
        ));
    }
    stream.set_read_timeout(None)?; // steady-state reader blocks until the next message
    establish(authenticated, stream, Role::Responder);
    Ok(())
}
