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
use super::numeric::{arg, expect_int};

/// The compression level for an encoder prim's optional 2nd arg: absent → the
/// library default (6), else an integer clamped to the valid `0..=9` (0 = store,
/// 9 = best). An out-of-range level is a clean runtime error, not a silent clamp.
fn level_arg(name: &str, args: &[Value], heap: &Heap) -> Result<Compression, LispError> {
    match arg(args, 1) {
        Value::Nil => Ok(Compression::default()),
        v => {
            let n = expect_int(heap, name, v)?;
            if !(0..=9).contains(&n) {
                return Err(LispError::runtime(format!(
                    "{name}: compression level must be 0-9 (got {n})"
                )));
            }
            Ok(Compression::new(n as u32))
        }
    }
}

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

/// `(%gzip bytes [level])` — gzip-compress a byte sequence at optional `level`
/// (0-9, default 6), returned as `bytes`.
pub(super) fn gzip(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let level = level_arg("%gzip", args, heap)?;
    let data = collect_bytes("%gzip", arg(args, 0), heap)?;
    let out = encode("%gzip", GzEncoder::new(Vec::new(), level), &data)?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%gunzip bytes)` — decompress gzip data, returned as `bytes`.
pub(super) fn gunzip(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%gunzip", arg(args, 0), heap)?;
    let out = decode("%gunzip", GzDecoder::new(&data[..]))?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%zlib-compress bytes [level])` — zlib-compress (RFC 1950) at optional
/// `level` (0-9, default 6), returned as `bytes`.
pub(super) fn zlib_compress(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let level = level_arg("%zlib-compress", args, heap)?;
    let data = collect_bytes("%zlib-compress", arg(args, 0), heap)?;
    let out = encode("%zlib-compress", ZlibEncoder::new(Vec::new(), level), &data)?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%zlib-uncompress bytes)` — decompress zlib data, returned as `bytes`.
pub(super) fn zlib_uncompress(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%zlib-uncompress", arg(args, 0), heap)?;
    let out = decode("%zlib-uncompress", ZlibDecoder::new(&data[..]))?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%deflate bytes [level])` — raw DEFLATE (RFC 1951, no header/checksum) at
/// optional `level` (0-9, default 6), as `bytes`.
pub(super) fn deflate(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let level = level_arg("%deflate", args, heap)?;
    let data = collect_bytes("%deflate", arg(args, 0), heap)?;
    let out = encode("%deflate", DeflateEncoder::new(Vec::new(), level), &data)?;
    Ok(bytes_to_value(&out, heap))
}

/// `(%inflate bytes)` — decompress raw DEFLATE data, returned as `bytes`.
pub(super) fn inflate(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%inflate", arg(args, 0), heap)?;
    let out = decode("%inflate", DeflateDecoder::new(&data[..]))?;
    Ok(bytes_to_value(&out, heap))
}

/// The brotli quality for the encoder's optional 2nd arg: absent → 5 (a balanced
/// default suited to per-request compression), else an integer clamped to the valid
/// `0..=11` (0 = fastest, 11 = best). Out of range is a clean runtime error.
fn brotli_quality(name: &str, args: &[Value], heap: &Heap) -> Result<u32, LispError> {
    match arg(args, 1) {
        Value::Nil => Ok(5),
        v => {
            let n = expect_int(heap, name, v)?;
            if !(0..=11).contains(&n) {
                return Err(LispError::runtime(format!(
                    "{name}: brotli quality must be 0-11 (got {n})"
                )));
            }
            Ok(n as u32)
        }
    }
}

/// `(%brotli bytes [quality])` — brotli-compress a byte sequence (the
/// `Content-Encoding: br` wire format) at optional `quality` (0-11, default 5),
/// returned as `bytes`.
pub(super) fn brotli(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let quality = brotli_quality("%brotli", args, heap)?;
    let data = collect_bytes("%brotli", arg(args, 0), heap)?;
    // lgwin 22 is brotli's common default window (RFC 7932 allows 10..=24).
    let mut writer = brotli::CompressorWriter::new(Vec::new(), 4096, quality, 22);
    writer
        .write_all(&data)
        .map_err(|e| LispError::runtime(format!("%brotli: {e}")))?;
    writer
        .flush()
        .map_err(|e| LispError::runtime(format!("%brotli: {e}")))?;
    let out = writer.into_inner();
    Ok(bytes_to_value(&out, heap))
}

/// `(%unbrotli bytes)` — decompress brotli data, returned as `bytes`.
pub(super) fn unbrotli(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let data = collect_bytes("%unbrotli", arg(args, 0), heap)?;
    let out = decode("%unbrotli", brotli::Decompressor::new(&data[..], 4096))?;
    Ok(bytes_to_value(&out, heap))
}
