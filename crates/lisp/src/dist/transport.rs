//! The link carrier: the [`Stream`] a node link runs over (TCP, or a Unix-domain socket on
//! unix), and opening one — dialing an address, and binding and readying a Unix socket
//! path. Everything above it (handshake, framing, heartbeat, teardown) is
//! transport-agnostic.

use super::*;

// ----- transport (the link carrier) ------------------------------------------

/// A live link's byte stream. The whole protocol above it — handshake, framing,
/// heartbeat, teardown — is transport-agnostic, so this enum is the *only* place
/// TCP-vs-Unix matters. The reader/writer threads hold an `Arc<Stream>` and do
/// I/O through `&Stream`, mirroring the `&TcpStream: Read` shape std provides;
/// the handshake runs over `&mut Stream` before the link goes steady-state.
pub(super) enum Stream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl Stream {
    pub(super) fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.shutdown(how),
            #[cfg(unix)]
            Stream::Unix(s) => s.shutdown(how),
        }
    }
    pub(super) fn set_read_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_read_timeout(d),
            #[cfg(unix)]
            Stream::Unix(s) => s.set_read_timeout(d),
        }
    }
    pub(super) fn set_write_timeout(&self, d: Option<Duration>) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.set_write_timeout(d),
            #[cfg(unix)]
            Stream::Unix(s) => s.set_write_timeout(d),
        }
    }
}

// Owned-stream I/O: the handshake drives `&mut Stream` (`TcpStream`/`UnixStream`
// each impl `Read`/`Write`).
impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.read(buf),
            #[cfg(unix)]
            Stream::Unix(s) => s.read(buf),
        }
    }
}
impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Tcp(s) => s.write(buf),
            #[cfg(unix)]
            Stream::Unix(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Tcp(s) => s.flush(),
            #[cfg(unix)]
            Stream::Unix(s) => s.flush(),
        }
    }
}

// Shared-ref I/O: the reader (`&*sock`) and writer (`(&*sock).write_all`) hold an
// `Arc<Stream>` and never have `&mut`, exactly like `&TcpStream: Read` in std.
impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match **self {
            Stream::Tcp(ref s) => (&*s).read(buf),
            #[cfg(unix)]
            Stream::Unix(ref s) => (&*s).read(buf),
        }
    }
}
impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match **self {
            Stream::Tcp(ref s) => (&*s).write(buf),
            #[cfg(unix)]
            Stream::Unix(ref s) => (&*s).write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match **self {
            Stream::Tcp(ref s) => (&*s).flush(),
            #[cfg(unix)]
            Stream::Unix(ref s) => (&*s).flush(),
        }
    }
}

/// Bind a Unix-domain listener and spawn its accept loop (the `unix:` transport).
/// wasm and other non-unix targets have no Unix sockets, so it reports unsupported.
#[cfg(unix)]
pub(super) fn bind_unix_listener(path: &str) -> io::Result<()> {
    let path = path.to_string();
    prepare_unix_path(&path)?;
    let listener = UnixListener::bind(&path)?;
    spawn_acceptor(move || listener.accept().map(|(s, _)| Stream::Unix(s)));
    Ok(())
}
#[cfg(not(unix))]
pub(super) fn bind_unix_listener(_path: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unix-socket nodes are not available on this target",
    ))
}

/// Dial a Unix-domain peer (the `unix:` transport); unsupported off unix.
#[cfg(unix)]
pub(super) fn dial_unix(path: &str) -> io::Result<Stream> {
    Ok(Stream::Unix(UnixStream::connect(path)?))
}
#[cfg(not(unix))]
pub(super) fn dial_unix(_path: &str) -> io::Result<Stream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unix-socket nodes are not available on this target",
    ))
}

/// Ready a Unix-socket path for `bind`: create the parent directory (`0700`) and
/// clear a **stale** socket left by a crashed node. A path that still has a live
/// listener is refused (another node owns that name); a path that refuses a
/// connection is stale and gets unlinked so we can rebind. Best-effort against a
/// concurrent same-name start — a same-user dev footgun, not a security boundary
/// (the `0700` dir already gates other users).
#[cfg(unix)]
pub(super) fn prepare_unix_path(path: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let p = std::path::Path::new(path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
        let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
    }
    if p.exists() {
        match UnixStream::connect(p) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("node socket {path} is already in use by a live node"),
                ));
            }
            // ConnectionRefused → no listener; socket file is stale from a
            // crashed node. Unlink it so `bind` can recreate it.
            Err(ref e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                let _ = std::fs::remove_file(p);
            }
            // Any other connect error (EACCES, ENOENT after a race, …) means
            // we can't determine liveness. Leave the file alone and let `bind`
            // fail with a clear OS error rather than destroying a potentially
            // live peer's socket.
            Err(_) => {}
        }
    }
    Ok(())
}

/// Open the carrier for `addr`. Unix connects are local and effectively instant
/// (or refuse immediately); TCP goes through [`dial_tcp`](crate::host::dial::dial_tcp),
/// which bounds **both** the name resolution and each connect — for `node/connect` the
/// dialling thread is a scheduler worker, which cannot be preempted mid-syscall, so
/// neither a wedged resolver nor a silently-dropping peer may pin it.
pub(super) fn dial(addr: &str) -> io::Result<Stream> {
    if let Some(path) = addr.strip_prefix("unix:") {
        dial_unix(path)
    } else if let Some(hostport) = addr.strip_prefix("tcp:") {
        let (host, port) = crate::host::dial::split_host_port(hostport)?;
        crate::host::dial::dial_tcp(host, port, CONNECT_TIMEOUT).map(Stream::Tcp)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("node address must start with 'unix:' or 'tcp:' (got {addr})"),
        ))
    }
}
