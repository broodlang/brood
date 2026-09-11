//! The TCP/TLS socket primitives (ADR-062): connect, listen, send, close, and the
//! iolist flattener a send accepts. Mechanism is `crate::host::net`; the `std/net/*`
//! library (http, sse, tcp, reconnect) is the policy over these `%tcp-*` names.

use crate::core::heap::Heap;

use crate::core::value::{EnvId, Value};

use crate::error::{LispError, LispResult};

use super::numeric::{arg, expect_int, expect_string};

use super::bytes::{flatten_iolist, send_payload};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // TCP sockets (ADR-062), built on the blocking-IO → mailbox seam (ADR-059):
    // inbound data is delivered to the owning process's mailbox as `[:tcp sock
    // data]` / `[:tcp-closed sock]` / `[:tcp-accept lsock client]` messages, which
    // Brood `receive`s — no polling, no worker ever blocked. `connect`/`listen`
    // register the *calling* process as the owner. A socket is an opaque handle,
    // valid across this runtime's processes, never sent across nodes.
    primitives.def(
        "%tcp-connect",
        Arity::exact(2),
        Sig::new(vec![string, int], socket_ty),
        &["host", "port"],
        "Connect to host:port; inbound data is delivered to the calling process as [:tcp sock data] / [:tcp-closed sock] messages. Returns a socket. Throws on failure.",
        tcp_connect);
    primitives.def(
        "%tcp-listen",
        Arity::exact(2),
        Sig::new(vec![string, int], socket_ty),
        &["host", "port"],
        "Bind a listening socket on host:port (port 0 = OS-assigned); connections arrive as [:tcp-accept lsock client] messages to the calling process. Returns a socket.",
        tcp_listen);
    primitives.def(
        "%tls-request",
        Arity::range(3, 4),
        Sig::variadic(any, socket_ty),
        &["host", "port", "request", "ca-pem"],
        "Make one HTTPS request to host:port (TLS): the response arrives at the calling process as [:tcp sock data] … [:tcp-closed sock] messages (or [:tcp-error sock msg]). request is any iolist (a string, bytes, or nested tree — ADR-141); the socket honors tcp-set-binary for the response. Optional ca-pem (a PEM certificate) replaces the Mozilla roots as the trust anchor — for private CAs and tls-self-signed dev servers. Returns a socket id; pair with tcp-drain. Low-level — prefer http-get.",
        tls_request);
    primitives.def(
        "%tls-listen",
        Arity::exact(4),
        Sig::new(vec![string, int, string, string], socket_ty),
        &["host", "port", "cert-pem", "key-pem"],
        "Bind a TLS listening socket on host:port using the PEM certificate chain cert-pem and private key key-pem (port 0 = OS-assigned). Like tcp-listen, connections arrive as [:tcp-accept lsock client]; each accepted socket transparently decrypts inbound to [:tcp …] and encrypts tcp-send, so code above the transport is unchanged. Returns a socket.",
        tls_listen);
    primitives.def(
        "%tls-self-signed",
        Arity::exact(1),
        // A VECTOR (`alloc_vector`): declared `list`, the Brood `tls/self-signed` contract failed
        // every call under BROOD_CONTRACTS=1 and the checker warned on the corrected sig (KI-113).
        Sig::new(vec![string], vec_ty),
        &["host"],
        "Generate a self-signed TLS certificate + private key for host (a DNS name like \"localhost\"), for zero-config dev TLS. Returns [cert-pem key-pem] — pass them to tls-listen. Not for production (clients reject a self-signed cert unless told to trust it).",
        tls_self_signed);
    primitives.def(
        "%tcp-send",
        Arity::exact(2),
        Sig::new(vec![socket_ty, iolist], nil_ty),
        &["sock", "data"],
        "Write data to sock (blocking). data is any iolist — a string, a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those, flattened once at the write (ADR-139). A string leaf is always sent as its UTF-8 bytes, whatever the socket's mode (ADR-141); raw bytes go out as bytes values. Returns nil; throws on error.",
        tcp_send);
    primitives.def(
        "%tcp-set-binary",
        Arity::exact(2),
        Sig::new(vec![socket_ty, bool_ty], nil_ty),
        &["sock", "on"],
        "Switch sock's INBOUND decode between text mode (default) and binary mode; outbound tcp-send is unaffected (ADR-141). In binary mode inbound [:tcp sock data] delivers data as a byte-faithful `bytes` value (not a string) — for length-prefixed / control-byte protocols like WebSocket framing or a database wire protocol. Text mode delivers a UTF-8 string. Returns nil; throws if sock is gone or a listener.",
        tcp_set_binary);
    primitives.def(
        "%tcp-set-idle-timeout",
        Arity::exact(2),
        Sig::new(vec![socket_ty, int], nil_ty),
        &["sock", "ms"],
        "Arm (or, with ms 0, disarm) an idle timeout on an established stream: the reactor drops the connection if no bytes move in EITHER direction for ms milliseconds, delivering [:tcp-closed] (or [:tcp-error] for a one-shot TLS client). Off by default — arm it on a connection accepting untrusted input as slow-loris protection the reactor applies even if the app forgets to close; leave it off for a legitimately long-idle stream (SSE, long-poll). Returns nil; throws if sock is gone or a listener.",
        tcp_set_idle_timeout);
    primitives.def(
        "%tcp-controlling-process",
        Arity::exact(2),
        Sig::new(vec![socket_ty, pid_ty], nil_ty),
        &["sock", "pid"],
        "Make pid the owner of sock's inbound data: starts reading a just-accepted (passive) socket, or retargets an active one. Returns nil.",
        tcp_controlling_process);
    primitives.def(
        "%tcp-close",
        Arity::exact(1),
        Sig::new(vec![socket_ty], nil_ty),
        &["sock"],
        "Close sock (a stream or listener), releasing its fd / stopping its accept loop. Idempotent; returns nil.",
        tcp_close);
    primitives.def(
        "%tcp-local-port",
        Arity::exact(1),
        Sig::new(vec![socket_ty], int.union(nil_ty)),
        &["sock"],
        "The local port sock is bound to, or nil.",
        tcp_local_port,
    );
}

// ---------- TCP sockets (ADR-062) ----------
//
// Thin non-blocking mechanism over `crate::host::net`; the active-socket / framing /
// HTTP policy is Brood (std/net/tcp.blsp). A socket is `Value::Socket(id)`.

pub(super) fn expect_socket(heap: &Heap, who: &str, v: Value) -> Result<u64, LispError> {
    expect!(heap, who, v, "socket",
        Value::Socket(id) => id,
    )
}

pub(super) fn socket_port(who: &str, p: i64) -> Result<u16, LispError> {
    u16::try_from(p)
        .map_err(|_| LispError::runtime(format!("{}: port {} out of range 0..=65535", who, p)))
}

pub(super) fn tcp_connect(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "%tcp-connect", arg(args, 0))?;
    let port = socket_port(
        "%tcp-connect",
        expect_int(heap, "%tcp-connect", arg(args, 1))?,
    )?;
    let owner = crate::process::self_pid();
    match crate::host::net::connect(&host, port, owner) {
        Ok(id) => Ok(Value::socket(id)),
        Err(e) => Err(
            LispError::runtime(format!("tcp-connect {}:{}: {}", host, port, e))
                .with_code(crate::error::error_codes::FILE_IO),
        ),
    }
}

pub(super) fn tcp_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "%tcp-listen", arg(args, 0))?;
    let port = socket_port(
        "%tcp-listen",
        expect_int(heap, "%tcp-listen", arg(args, 1))?,
    )?;
    let owner = crate::process::self_pid();
    match crate::host::net::listen(&host, port, owner) {
        Ok(id) => Ok(Value::socket(id)),
        Err(e) => Err(
            LispError::runtime(format!("tcp-listen {}:{}: {}", host, port, e))
                .with_code(crate::error::error_codes::FILE_IO),
        ),
    }
}

pub(super) fn tls_listen(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "%tls-listen", arg(args, 0))?;
    let port = socket_port(
        "%tls-listen",
        expect_int(heap, "%tls-listen", arg(args, 1))?,
    )?;
    let cert = expect_string(heap, "%tls-listen", arg(args, 2))?;
    let key = expect_string(heap, "%tls-listen", arg(args, 3))?;
    let owner = crate::process::self_pid();
    match crate::host::net::tls_listen(&host, port, &cert, &key, owner) {
        Ok(id) => Ok(Value::socket(id)),
        Err(e) => Err(
            LispError::runtime(format!("tls-listen {}:{}: {}", host, port, e))
                .with_code(crate::error::error_codes::FILE_IO),
        ),
    }
}

pub(super) fn tls_self_signed(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "%tls-self-signed", arg(args, 0))?.to_string();
    match crate::host::net::tls_self_signed(vec![host]) {
        Ok((cert, key)) => {
            let c = heap.alloc_string(&cert);
            let k = heap.alloc_string(&key);
            Ok(heap.alloc_vector(vec![c, k]))
        }
        Err(e) => Err(LispError::runtime(format!("tls-self-signed: {}", e))),
    }
}

pub(super) fn tls_request(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let host = expect_string(heap, "%tls-request", arg(args, 0))?.to_string();
    let port = socket_port(
        "%tls-request",
        expect_int(heap, "%tls-request", arg(args, 1))?,
    )?;
    // The request is any iolist (ADR-141/143) — a string, bytes, or a nested
    // tree — flattened once here, so binary https request bodies work.
    let mut request = Vec::new();
    flatten_iolist(heap, "%tls-request", arg(args, 2), &mut request)?;
    // Optional 4th arg: a PEM trust anchor replacing the Mozilla roots for
    // this request (private CAs, tls-self-signed dev servers).
    let ca = match args.get(3) {
        Some(v) if !matches!(v, Value::Nil) => {
            Some(expect_string(heap, "%tls-request", *v)?.to_string())
        }
        _ => None,
    };
    let owner = crate::process::self_pid();
    let id = crate::host::net::tls_request(&host, port, request, ca, owner)
        .map_err(|e| LispError::runtime(format!("tls-request: {}", e)))?;
    Ok(Value::socket(id))
}

pub(super) fn tcp_send(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-send", arg(args, 0))?;
    let out = send_payload(heap, "%tcp-send", arg(args, 1))?;
    crate::host::net::send(id, &out).map_err(|e| LispError::runtime(format!("tcp-send: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_set_binary(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-set-binary", arg(args, 0))?;
    let on = !matches!(arg(args, 1), Value::Nil | Value::Bool(false));
    crate::host::net::set_binary(id, on)
        .map_err(|e| LispError::runtime(format!("tcp-set-binary: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_set_idle_timeout(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-set-idle-timeout", arg(args, 0))?;
    let ms = expect_int(heap, "%tcp-set-idle-timeout", arg(args, 1))?;
    if ms < 0 {
        return Err(LispError::runtime(
            "tcp-set-idle-timeout: ms must be >= 0 (0 disarms)",
        ));
    }
    crate::host::net::set_idle_timeout(id, ms as u64)
        .map_err(|e| LispError::runtime(format!("tcp-set-idle-timeout: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_controlling_process(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-controlling-process", arg(args, 0))?;
    let pid = match arg(args, 1) {
        Value::Pid { id, .. } => id,
        other => {
            return Err(LispError::wrong_type(
                heap,
                "%tcp-controlling-process",
                "pid",
                other,
            ))
        }
    };
    crate::host::net::controlling_process(id, pid)
        .map_err(|e| LispError::runtime(format!("tcp-controlling-process: {}", e)))?;
    Ok(Value::nil())
}

pub(super) fn tcp_close(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-close", arg(args, 0))?;
    crate::host::net::close(id);
    Ok(Value::nil())
}

pub(super) fn tcp_local_port(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let id = expect_socket(heap, "%tcp-local-port", arg(args, 0))?;
    Ok(crate::host::net::local_port(id)
        .map(|p| Value::int(p as i64))
        .unwrap_or(Value::nil()))
}
