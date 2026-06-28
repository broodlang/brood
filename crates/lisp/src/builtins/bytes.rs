//! The raw-bytes surface (`Value::Bytes`). Bytes are immutable byte sequences
//! (`Arc<SharedBlob>`): construction, indexing, slicing, concatenation, and the
//! UTF-8 ↔ string conversions. The generic sequence ops (`count`/`nth`/`get`/
//! `first`/`rest`/`empty?`) also accept bytes — see `sequences.rs` + the prelude.

use crate::core::blob::SharedBlob;
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{error_codes, LispError, LispResult};

use super::numeric::{arg, expect_int};

/// Borrow a `Value::Bytes`'s raw bytes, or a type error.
fn as_bytes<'h>(heap: &'h Heap, who: &str, v: Value) -> Result<&'h [u8], LispError> {
    match v {
        Value::Bytes(id) => Ok(heap.bytes(id).as_bytes()),
        _ => Err(LispError::wrong_type(heap, who, "bytes", v)),
    }
}

/// `(bytes 1 2 3)` or `(bytes [1 2 3])` / `(bytes (list …))` — build a bytes value
/// from byte integers (0–255). A single vector/list arg is taken as the sequence;
/// an existing bytes value passes through unchanged.
pub(super) fn bytes_make(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let ints: Vec<Value> = if args.len() == 1 {
        match args[0] {
            Value::Bytes(_) => return Ok(args[0]),
            Value::Vector(id) => heap.vector(id).to_vec(),
            Value::Pair(_) => {
                let mut out = Vec::new();
                let mut cur = args[0];
                while let Value::Pair(p) = cur {
                    out.push(heap.car(p));
                    cur = heap.cdr(p);
                }
                out
            }
            Value::Nil => Vec::new(),
            other => vec![other],
        }
    } else {
        args.to_vec()
    };
    let mut out = Vec::with_capacity(ints.len());
    for v in ints {
        let n = expect_int(heap, "bytes", v)?;
        if !(0..=255).contains(&n) {
            return Err(LispError::runtime(format!(
                "bytes: {} is out of byte range 0–255",
                n
            )));
        }
        out.push(n as u8);
    }
    Ok(heap.alloc_bytes(SharedBlob::new(&out)))
}

/// `(byte-length b)` — the number of bytes. O(1).
pub(super) fn byte_length(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    Ok(Value::int(as_bytes(heap, "byte-length", arg(args, 0))?.len() as i64))
}

/// `(byte-at b i)` — the byte at index `i` as an int 0–255 (out of range errors).
pub(super) fn byte_at(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let i = expect_int(heap, "byte-at", arg(args, 1))?;
    let b = as_bytes(heap, "byte-at", v)?;
    if i >= 0 && (i as usize) < b.len() {
        Ok(Value::int(b[i as usize] as i64))
    } else {
        Err(
            LispError::runtime(format!("byte-at: index {} out of range [0, {})", i, b.len()))
                .with_code(error_codes::INDEX_OUT_OF_RANGE),
        )
    }
}

/// `(subbytes b start)` / `(subbytes b start end)` — the byte slice `[start, end)`
/// (end defaults to the length) as a fresh bytes value.
pub(super) fn subbytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let start = expect_int(heap, "subbytes", arg(args, 1))?;
    let b = as_bytes(heap, "subbytes", v)?;
    let len = b.len() as i64;
    let end = if args.len() >= 3 {
        expect_int(heap, "subbytes", arg(args, 2))?
    } else {
        len
    };
    if start < 0 || end > len || start > end {
        return Err(LispError::runtime(format!(
            "subbytes: range [{}, {}) out of bounds [0, {})",
            start, end, len
        ))
        .with_code(error_codes::INDEX_OUT_OF_RANGE));
    }
    let slice = b[start as usize..end as usize].to_vec();
    Ok(heap.alloc_bytes(SharedBlob::new(&slice)))
}

/// `(bytes-concat b1 b2 …)` — one bytes value joining all the arguments.
pub(super) fn bytes_concat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut out = Vec::new();
    for &v in args {
        out.extend_from_slice(as_bytes(heap, "bytes-concat", v)?);
    }
    Ok(heap.alloc_bytes(SharedBlob::new(&out)))
}

/// `(bytes-index-of haystack needle)` / `(bytes-index-of haystack needle from)` —
/// the first index of the `needle` bytes in `haystack` at or after `from` (default
/// 0), or -1 if not present. The byte-protocol workhorse (find a `\r\n\r\n`, a frame
/// delimiter, …).
pub(super) fn bytes_index_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = if args.len() >= 3 {
        expect_int(heap, "bytes-index-of", arg(args, 2))?.max(0) as usize
    } else {
        0
    };
    let hay = as_bytes(heap, "bytes-index-of", arg(args, 0))?;
    let needle = as_bytes(heap, "bytes-index-of", arg(args, 1))?;
    let idx = if needle.is_empty() {
        from.min(hay.len()) as i64
    } else if from >= hay.len() {
        -1
    } else {
        hay[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| (p + from) as i64)
            .unwrap_or(-1)
    };
    Ok(Value::int(idx))
}

/// `(bytes->list b)` — the bytes as a list of integers 0–255.
pub(super) fn bytes_to_list(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = as_bytes(heap, "bytes->list", arg(args, 0))?
        .iter()
        .map(|&x| Value::int(x as i64))
        .collect();
    Ok(heap.list(items))
}
