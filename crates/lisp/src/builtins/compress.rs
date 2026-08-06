//! Compression primitives — the single place the kernel reaches for the `flate2`
//! crate. Raw bytes in, raw `bytes` out (via [`bytes_to_value`]), so a payload
//! compresses/decompresses without a copy at each step. Three container formats,
//! each an encode/decode pair:
//!
//!   - **gzip** (`%gzip`/`%gunzip`) — RFC 1952, the `Content-Encoding: gzip` wire
//!     format (magic header + CRC32 + length).
//!   - **zlib** (`%zlib-compress`/`%zlib-uncompress`) — RFC 1950, a 2-byte header +
//!     Adler-32 checksum.
//!   - **raw deflate** (`%deflate`/`%inflate`) — RFC 1951, no header/checksum.
//!
//! The public names (`gzip`/`gunzip`, `compress`/`uncompress`,
//! `zip`/`unzip`) are Brood policy in `std/zlib.blsp` over these six prims.

use std::io::{Read, Write};

use flate2::read::{DeflateDecoder, GzDecoder, ZlibDecoder};
use flate2::write::{DeflateEncoder, GzEncoder, ZlibEncoder};
use flate2::Compression;

use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult};

use super::io::{bytes_to_value, collect_bytes};
use super::numeric::arg;

/// Feed `data` through a write-adapter encoder `enc` and return the compressed bytes.
fn encode<W: Write + FinishBytes>(
    name: &str,
    mut enc: W,
    data: &[u8],
) -> Result<Vec<u8>, LispError> {
    enc.write_all(data)
        .map_err(|e| LispError::runtime(format!("{name}: {e}")))?;
    enc.finish_bytes()
        .map_err(|e| LispError::runtime(format!("{name}: {e}")))
}

/// Read all bytes out of a read-adapter decoder `dec` (the decompressed output).
fn decode<R: Read>(name: &str, mut dec: R) -> Result<Vec<u8>, LispError> {
    let mut out = Vec::new();
    dec.read_to_end(&mut out)
        .map_err(|e| LispError::runtime(format!("{name}: not valid compressed data: {e}")))?;
    Ok(out)
}

/// `finish()` differs per flate2 write-encoder; unify them behind one trait.
trait FinishBytes {
    fn finish_bytes(self) -> std::io::Result<Vec<u8>>;
}
impl FinishBytes for GzEncoder<Vec<u8>> {
    fn finish_bytes(self) -> std::io::Result<Vec<u8>> {
        self.finish()
    }
}
impl FinishBytes for ZlibEncoder<Vec<u8>> {
    fn finish_bytes(self) -> std::io::Result<Vec<u8>> {
        self.finish()
    }
}
impl FinishBytes for DeflateEncoder<Vec<u8>> {
    fn finish_bytes(self) -> std::io::Result<Vec<u8>> {
        self.finish()
    }
}

/// `(%gzip bytes)` — gzip-compress a byte sequence, returned as `bytes`.
pub(super) fn gzip(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%gzip", arg(args, 0), heap)?;
    let out = encode(
        "%gzip",
        GzEncoder::new(Vec::new(), Compression::default()),
        &data,
    )?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%gunzip bytes)` — decompress gzip data, returned as `bytes`.
pub(super) fn gunzip(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%gunzip", arg(args, 0), heap)?;
    let out = decode("%gunzip", GzDecoder::new(&data[..]))?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%zlib-compress bytes)` — zlib-compress (RFC 1950), returned as `bytes`.
pub(super) fn zlib_compress(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%zlib-compress", arg(args, 0), heap)?;
    let out = encode(
        "%zlib-compress",
        ZlibEncoder::new(Vec::new(), Compression::default()),
        &data,
    )?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%zlib-uncompress bytes)` — decompress zlib data, returned as `bytes`.
pub(super) fn zlib_uncompress(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%zlib-uncompress", arg(args, 0), heap)?;
    let out = decode("%zlib-uncompress", ZlibDecoder::new(&data[..]))?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%deflate bytes)` — raw DEFLATE (RFC 1951, no header/checksum), as `bytes`.
pub(super) fn deflate(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%deflate", arg(args, 0), heap)?;
    let out = encode(
        "%deflate",
        DeflateEncoder::new(Vec::new(), Compression::default()),
        &data,
    )?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%inflate bytes)` — decompress raw DEFLATE data, returned as `bytes`.
pub(super) fn inflate(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%inflate", arg(args, 0), heap)?;
    let out = decode("%inflate", DeflateDecoder::new(&data[..]))?;
    Ok(bytes_to_value(&out, heap))
}
