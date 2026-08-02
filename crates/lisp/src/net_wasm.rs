//! WASM stub for the TCP/TLS socket mechanism (`crate::net`).
//!
//! The browser/headless wasm runtime has no sockets, so every entry point mirrors
//! the native `net.rs` public API but fails with an "unsupported on wasm" error at
//! runtime (the builtins in `builtins/io.rs` call these unchanged). Networking is
//! not part of the in-browser playground; keeping the same signatures avoids
//! littering the socket builtins with `#[cfg]`s. See `docs/wasm.md`.

fn unsupported<T>() -> std::io::Result<T> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "networking is not available in the wasm runtime",
    ))
}

pub fn connect(_host: &str, _port: u16, _subscriber: u64) -> std::io::Result<u64> {
    unsupported()
}

pub fn listen(_host: &str, _port: u16, _subscriber: u64) -> std::io::Result<u64> {
    unsupported()
}

pub fn controlling_process(_id: u64, _pid: u64) -> std::io::Result<()> {
    unsupported()
}

pub fn set_binary(_id: u64, _on: bool) -> std::io::Result<()> {
    unsupported()
}

pub fn set_idle_timeout(_id: u64, _ms: u64) -> std::io::Result<()> {
    unsupported()
}

pub fn send(_id: u64, _data: &[u8]) -> std::io::Result<()> {
    unsupported()
}

pub fn close(_id: u64) {}

pub fn close_process_sockets(_pid: u64) {}

pub fn local_port(_id: u64) -> Option<u16> {
    None
}

pub fn tls_request(
    _host: &str,
    _port: u16,
    _request: Vec<u8>,
    _ca_pem: Option<String>,
    _subscriber: u64,
) -> std::io::Result<u64> {
    unsupported()
}

pub fn tls_self_signed(_names: Vec<String>) -> std::io::Result<(String, String)> {
    unsupported()
}

pub fn tls_listen(
    _host: &str,
    _port: u16,
    _cert_pem: &str,
    _key_pem: &str,
    _subscriber: u64,
) -> std::io::Result<u64> {
    unsupported()
}
