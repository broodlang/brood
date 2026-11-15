//! Dialling a TCP peer by name with every wait bounded — the one policy both the node
//! links (`dist`) and the socket primitives (`host::net`) dial through.
//!
//! `TcpStream::connect` bounds neither half: `to_socket_addrs` is a blocking
//! `getaddrinfo` with no timeout, and the connect waits on the kernel's SYN retries.
//! [`dial_tcp`] bounds the resolution off the calling thread ([`RESOLVE_TIMEOUT`]) and
//! each address's handshake by the caller's `connect_timeout`.

use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::Duration;

/// Bound on resolving a host name. `getaddrinfo` has no timeout of its own and on a
/// scheduler worker it cannot be preempted mid-syscall (ADR-059); an unreachable DNS
/// server held the calling thread for the resolver's own timeout (often 5 s per attempt,
/// retries across several `nameserver` lines). KI-97 item 2.
pub(crate) const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Resolve `host:port` and connect to each address in turn, as `TcpStream::connect`
/// does, with the resolution bounded by [`RESOLVE_TIMEOUT`] and each handshake by
/// `connect_timeout`. `host` is a name or a bare IP literal (an IPv6 one unbracketed).
pub(crate) fn dial_tcp(host: &str, port: u16, connect_timeout: Duration) -> io::Result<TcpStream> {
    let mut last_error: Option<io::Error> = None;
    for address in resolve_bounded(host, port)? {
        match TcpStream::connect_timeout(&address, connect_timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("could not resolve any address for {host}:{port}"),
        )
    }))
}

/// Split a `host:port` address (`[v6]:port` for an IPv6 literal) for [`dial_tcp`].
pub(crate) fn split_host_port(address: &str) -> io::Result<(&str, u16)> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("expected host:port, got {address:?}"),
        )
    };
    let (host, port) = address.rsplit_once(':').ok_or_else(invalid)?;
    let host = host.strip_circumfix('[', ']').unwrap_or(host);
    Ok((host, port.parse().map_err(|_| invalid())?))
}

/// Resolve `host:port` with a wall-clock bound, off the calling thread.
///
/// The resolve happens on a throwaway thread and the caller waits on a channel, so a
/// hung resolver costs *that* thread rather than the caller. The thread is deliberately
/// left to finish on its own if we time out: there is no way to cancel a blocking
/// `getaddrinfo`, and detaching it is what keeps the caller bounded. That is safe because
/// it touches nothing after the send — a closed channel is simply a dropped result — and
/// the rate is bounded by the callers (a user-initiated connect, or `reconnect/watch`'s
/// exponential backoff), not by inbound traffic.
fn resolve_bounded(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    // An IP literal needs no resolver at all. Answer it inline: it is the overwhelmingly
    // common address (every `127.0.0.1` in the suite, and most real deployments), and the
    // fast path keeps a thread spawn off the dial entirely.
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let (sender, receiver) = mpsc::channel();
    let owned = host.to_string();
    if std::thread::Builder::new()
        .name("brood-resolve".into())
        .spawn(move || {
            let _ = sender.send(
                (owned.as_str(), port)
                    .to_socket_addrs()
                    .map(Iterator::collect),
            );
        })
        .is_err()
    {
        // Out of threads (EAGAIN under load). An inline resolve is strictly better than
        // failing the dial: connectivity is preserved, and the unbounded wait it risks is
        // the lesser failure next to refusing to connect on a briefly thread-starved box.
        return (host, port).to_socket_addrs().map(Iterator::collect);
    }
    match receiver.recv_timeout(RESOLVE_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("resolving {host} took longer than {RESOLVE_TIMEOUT:?}"),
        )),
        // The resolver thread vanished without answering (a panic in getaddrinfo's
        // wrapper, say). Report it rather than hanging on a channel nobody will send to.
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(format!(
            "resolver thread for {host} ended without a result"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// KI-97 item 2: a name resolution is bounded, and the bound is enforced off the
    /// calling thread. There is no way to make a real resolver hang on demand here, so
    /// this asserts the two properties that can be checked: a literal resolves at once,
    /// and a bad name fails rather than hanging.
    #[test]
    fn resolving_is_bounded_and_still_works() {
        let started = Instant::now();
        let resolved = resolve_bounded("127.0.0.1", 9).expect("a literal address must resolve");
        assert_eq!(resolved, vec!["127.0.0.1:9".parse().unwrap()]);
        assert!(
            started.elapsed() < RESOLVE_TIMEOUT,
            "a literal address must not go anywhere near the bound"
        );

        let started = Instant::now();
        let unresolvable = resolve_bounded("invalid.invalid.", 9);
        assert!(
            unresolvable.is_err(),
            "an unresolvable name must be an error, not a hang"
        );
        assert!(
            started.elapsed() < RESOLVE_TIMEOUT * 4,
            "resolution must be bounded, took {:?}",
            started.elapsed()
        );
    }

    /// The address forms `dist` hands over: a name, an IPv4 literal, and a bracketed IPv6
    /// literal, whose brackets come off so the IP fast path sees a bare address.
    #[test]
    fn host_port_splits_every_form_dist_accepts() {
        assert_eq!(
            split_host_port("example.org:4369").unwrap(),
            ("example.org", 4369)
        );
        assert_eq!(split_host_port("127.0.0.1:9").unwrap(), ("127.0.0.1", 9));
        assert_eq!(split_host_port("[::1]:9").unwrap(), ("::1", 9));
        assert!(split_host_port("no-port").is_err());
        assert!(split_host_port("host:notaport").is_err());
    }
}
