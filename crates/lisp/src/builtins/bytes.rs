//! The raw-bytes surface (`Value::Bytes`). Bytes are immutable byte sequences
//! (`Arc<SharedBlob>`): construction, indexing, slicing, concatenation, and the
//! UTF-8 ↔ string conversions. The generic sequence ops (`count`/`nth`/`get`/
//! `first`/`rest`/`empty?`) also accept bytes — see `sequences.rs` + the prelude.

use crate::core::blob::SharedBlob;
use crate::core::heap::Heap;
use crate::core::value::{EnvId, Value};
use crate::error::{LispError, LispResult, error_codes};

use super::args::{arg, expect_int};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(super) fn register(primitives: &mut super::Primitives) {
    use super::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // ---- raw bytes (Value::Bytes) ----
    primitives.def(
        "bytes",
        Arity::any(),
        Sig::variadic(any, bytes_ty),
        &["&", "byte-ints"],
        "Build a bytes value from byte integers 0–255: (bytes 1 2 3), or (bytes [1 2 3]) / (bytes (list …)) taking a single vector/list as the sequence. An existing bytes value passes through unchanged.\n\n    (bytes 1 2 3)            ;=> #b\"\\x01\\x02\\x03\"\n    (count (bytes [255 0]))  ;=> 2",
        bytes_make);
    primitives.def(
        "%byte-length",
        Arity::exact(1),
        Sig::new(vec![bytes_ty], int),
        &["b"],
        "The number of bytes in b. O(1).",
        byte_length,
    );
    primitives.def(
        "%byte-at",
        Arity::exact(2),
        Sig::new(vec![bytes_ty, int], int),
        &["b", "i"],
        "The byte at index i of b as an int 0–255; errors if i is out of range.",
        byte_at,
    );
    primitives.def(
        "%subbytes",
        Arity::range(2, 3),
        Sig::variadic(any, bytes_ty),
        &["b", "start", "&optional", "end"],
        "The byte slice [start, end) of b as a fresh bytes value (end defaults to the length). Errors if the range is out of bounds.",
        subbytes);
    primitives.def(
        "%bytes-concat",
        Arity::any(),
        Sig::variadic(iolist, bytes_ty),
        &["&", "iolists"],
        "One bytes value joining all arguments, each an iolist (ADR-139): a string (UTF-8), a bytes value, a byte int 0–255, or an arbitrarily nested list/vector of those. The in-memory materialiser of the iolist model.",
        bytes_concat);
    // String<->bytes conversion is UTF-8 (a Brood string is UTF-8, like Rust's),
    // exposed under the explicit `string->utf8-bytes` / `utf8-bytes->string` names
    // (registered above) — the former duplicate `string->bytes` / `bytes->string`
    // prims were removed (they did the identical UTF-8 encode/decode).
    primitives.def(
        "%bytes->list",
        Arity::exact(1),
        Sig::new(vec![bytes_ty], pair),
        &["b"],
        "The bytes b as a list of integers 0–255.",
        bytes_to_list,
    );
    primitives.def(
        "%bytes-index-of",
        Arity::range(2, 3),
        Sig::new(vec![bytes_ty, bytes_ty], int),
        &["haystack", "needle", "&optional", "from"],
        "The first index of the needle bytes within haystack at or after from (default 0), or -1 if absent. The byte-protocol workhorse (locate a \\r\\n\\r\\n, a frame delimiter, …).",
        bytes_index_of);
    primitives.def(
        "%base64-encode",
        Arity::exact(2),
        Sig::new(vec![any, any], string),
        &["byte-seq", "url?"],
        "Base64 of byte-seq (a bytes value, or a vector or list of byte ints 0–255), with `=` padding: the standard `+/` alphabet, or the URL-safe `-_` one when url? is truthy. The engine of `encoding/base64-encode-bytes` and its siblings.",
        base64_encode,
    );
    primitives.def(
        "%base64-decode",
        Arity::exact(2),
        Sig::new(vec![string, any], any),
        &["s", "url?"],
        "The bytes that Base64 string s encodes (the URL-safe alphabet when url? is truthy), or nil when s is not Base64: a character outside the alphabet, a `=` anywhere but a trailing run of at most two, or one character left over from a group of four. The engine of `encoding/base64-decode-bytes` and its siblings.",
        base64_decode,
    );
}

const BASE64_STANDARD: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// `(%base64-encode byte-seq url?)` — padded Base64 of the bytes.
pub(super) fn base64_encode(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let alphabet = if !matches!(arg(args, 1), Value::Nil | Value::Bool(false)) {
        BASE64_URL
    } else {
        BASE64_STANDARD
    };
    let encoded = match arg(args, 0) {
        Value::Bytes(id) => base64_encode_with(heap.bytes(id).as_bytes(), alphabet),
        other => base64_encode_with(&collect_bytes("%base64-encode", other, heap)?, alphabet),
    };
    Ok(heap.alloc_string(&encoded))
}

/// Padded standard-alphabet Base64 of `input`: what the terminal's kitty graphics protocol
/// carries a picture in (ADR-695).
pub(in crate::builtins) fn base64_standard(input: &[u8]) -> String {
    base64_encode_with(input, BASE64_STANDARD)
}

fn base64_encode_with(input: &[u8], alphabet: &[u8; 64]) -> String {
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    for group in input.chunks(3) {
        let b1 = group[0];
        let b2 = group.get(1).copied().unwrap_or(0);
        let b3 = group.get(2).copied().unwrap_or(0);
        out.push(alphabet[(b1 >> 2) as usize]);
        out.push(alphabet[(((b1 & 0b11) << 4) | (b2 >> 4)) as usize]);
        out.push(if group.len() > 1 {
            alphabet[(((b2 & 0b1111) << 2) | (b3 >> 6)) as usize]
        } else {
            b'='
        });
        out.push(if group.len() > 2 {
            alphabet[(b3 & 0b11_1111) as usize]
        } else {
            b'='
        });
    }
    // Every byte pushed is from the ASCII alphabet or `=`.
    String::from_utf8(out).unwrap_or_default()
}

/// `(%base64-decode s url?)` — the bytes, or nil for text that is not Base64.
pub(super) fn base64_decode(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let alphabet = if !matches!(arg(args, 1), Value::Nil | Value::Bool(false)) {
        BASE64_URL
    } else {
        BASE64_STANDARD
    };
    let text = super::args::expect_string_ref(heap, "%base64-decode", arg(args, 0))?;
    Ok(match base64_decode_with(text.as_bytes(), alphabet) {
        Some(decoded) => heap.alloc_bytes(SharedBlob::new(&decoded)),
        None => Value::Nil,
    })
}

fn base64_decode_with(input: &[u8], alphabet: &[u8; 64]) -> Option<Vec<u8>> {
    let mut values = [u8::MAX; 256];
    for (value, &symbol) in alphabet.iter().enumerate() {
        values[symbol as usize] = value as u8;
    }
    let body_length = input.len() - input.iter().rev().take_while(|&&c| c == b'=').count();
    if input.len() - body_length > 2 || body_length % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(body_length / 4 * 3 + 2);
    for group in input[..body_length].chunks(4) {
        let mut sextets = [0u8; 4];
        for (slot, &c) in sextets.iter_mut().zip(group) {
            *slot = values[c as usize];
            if *slot == u8::MAX {
                return None;
            }
        }
        out.push((sextets[0] << 2) | (sextets[1] >> 4));
        if group.len() > 2 {
            out.push((sextets[1] << 4) | (sextets[2] >> 2));
        }
        if group.len() > 3 {
            out.push((sextets[2] << 6) | sextets[3]);
        }
    }
    Some(out)
}

/// Borrow a `Value::Bytes`'s raw bytes, or a type error. The borrow lives as long as
/// `&heap`, which is what keeps a RUNTIME bytes value's generation alive while it's read
/// (ADR-091 Stage 4, via `Heap::code_gen_ref`'s cache).
fn as_bytes<'h>(heap: &'h Heap, who: &str, v: Value) -> Result<&'h [u8], LispError> {
    expect!(heap, who, v, "bytes",
        Value::Bytes(id) => heap.bytes(id).as_bytes(),
    )
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
    Ok(Value::int(
        as_bytes(heap, "bytes/length", arg(args, 0))?.len() as i64,
    ))
}

/// `(byte-at b i)` — the byte at index `i` as an int 0–255 (out of range errors).
pub(super) fn byte_at(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let i = expect_int(heap, "bytes/at", arg(args, 1))?;
    let b = as_bytes(heap, "bytes/at", v)?;
    if i >= 0 && (i as usize) < b.len() {
        Ok(Value::int(b[i as usize] as i64))
    } else {
        Err(LispError::runtime(format!(
            "bytes/at: index {} out of range [0, {})",
            i,
            b.len()
        ))
        .with_code(error_codes::INDEX_OUT_OF_RANGE))
    }
}

/// `(subbytes b start)` / `(subbytes b start end)` — the byte slice `[start, end)`
/// (end defaults to the length) as a fresh bytes value.
pub(super) fn subbytes(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let v = arg(args, 0);
    let start = expect_int(heap, "bytes/slice", arg(args, 1))?;
    let b = as_bytes(heap, "bytes/slice", v)?;
    let len = b.len() as i64;
    let end = if args.len() >= 3 {
        expect_int(heap, "bytes/slice", arg(args, 2))?
    } else {
        len
    };
    if start < 0 || end > len || start > end {
        return Err(LispError::runtime(format!(
            "bytes/slice: range [{}, {}) out of bounds [0, {})",
            start, end, len
        ))
        .with_code(error_codes::INDEX_OUT_OF_RANGE));
    }
    let slice = b[start as usize..end as usize].to_vec();
    Ok(heap.alloc_bytes(SharedBlob::new(&slice)))
}

/// `(bytes-concat io1 io2 …)` — one bytes value joining all the arguments, each
/// an **iolist** (ADR-139): a string (UTF-8), a `bytes`, a byte int 0–255, or an
/// arbitrarily nested list/vector of those. The in-memory materialiser of the
/// iolist model — build the tree, flatten once here.
pub(super) fn bytes_concat(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let mut out = Vec::new();
    for &v in args {
        flatten_iolist(heap, "bytes/concat", v, &mut out)?;
    }
    Ok(heap.alloc_bytes(SharedBlob::new(&out)))
}

/// `(bytes-index-of haystack needle)` / `(bytes-index-of haystack needle from)` —
/// the first index of the `needle` bytes in `haystack` at or after `from` (default
/// 0), or -1 if not present. The byte-protocol workhorse (find a `\r\n\r\n`, a frame
/// delimiter, …).
pub(super) fn bytes_index_of(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let from = if args.len() >= 3 {
        expect_int(heap, "bytes/index-of", arg(args, 2))?.max(0) as usize
    } else {
        0
    };
    let hay = as_bytes(heap, "bytes/index-of", arg(args, 0))?;
    let needle = as_bytes(heap, "bytes/index-of", arg(args, 1))?;
    let idx = if needle.is_empty() {
        from.min(hay.len()) as i64
    } else if from >= hay.len() {
        -1
    } else {
        hay[from..]
            .windows(needle.len())
            .position(|w| w == &*needle)
            .map(|p| (p + from) as i64)
            .unwrap_or(-1)
    };
    Ok(Value::int(idx))
}

/// `(bytes->list b)` — the bytes as a list of integers 0–255.
pub(super) fn bytes_to_list(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let items: Vec<Value> = as_bytes(heap, "%bytes->list", arg(args, 0))?
        .iter()
        .map(|&x| Value::int(x as i64))
        .collect();
    Ok(heap.list(items))
}

/// Extract raw bytes from a `Value`: a `bytes` value, or (leniently) a vector
/// or list of byte ints (0–255).
pub(in crate::builtins) fn collect_bytes(
    name: &'static str,
    bv: Value,
    heap: &mut Heap,
) -> Result<Vec<u8>, LispError> {
    match bv {
        Value::Bytes(id) => Ok(heap.bytes(id).as_bytes().to_vec()),
        Value::Vector(id) => {
            let vec = heap.vector(id).to_vec();
            vec.iter()
                .map(|v| match v {
                    Value::Int(n) if *n >= 0 && *n <= 255 => Ok(*n as u8),
                    other => Err(LispError::wrong_type(
                        heap,
                        name,
                        "byte int (0-255)",
                        *other,
                    )),
                })
                .collect::<Result<Vec<u8>, LispError>>()
        }
        Value::Pair(_) | Value::Nil => {
            let mut out = Vec::new();
            let mut cur = bv;
            loop {
                match cur {
                    Value::Nil => break,
                    Value::Pair(id) => {
                        let (h, t) = heap.pair(id);
                        match h {
                            Value::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
                            other => {
                                return Err(LispError::wrong_type(
                                    heap,
                                    name,
                                    "byte int (0-255)",
                                    other,
                                ));
                            }
                        }
                        cur = t;
                    }
                    other => return Err(LispError::wrong_type(heap, name, "proper list", other)),
                }
            }
            Ok(out)
        }
        other => Err(LispError::wrong_type(heap, name, "vector or list", other)),
    }
}

/// Allocate a raw-byte result (digest, HMAC, derived key) as a Brood `bytes`
/// value — the raw-byte counterpart of the Brood `bytes->hex` shaping. The byte-oriented
/// crypto layer (store-driver findings 2/3) returns these so digests can be
/// chained over bytes without a hex round-trip at each step.
pub(super) fn bytes_to_value(bytes: impl AsRef<[u8]>, heap: &mut Heap) -> Value {
    heap.alloc_bytes(crate::core::blob::SharedBlob::new(bytes.as_ref()))
}

/// Flatten an **iolist** into `out` at a write boundary (ADR-139): a leaf — a
/// string, a `bytes`, or a byte int 0–255 — or an arbitrarily nested proper
/// list/vector of iolists (`nil` = empty; an improper tail is a final leaf).
/// Callers describe output as a tree
/// (`[status-line headers "\r\n\r\n" body]`) and nothing is copied until this
/// single flatten at the device write — which deletes the O(n²)
/// `(str acc chunk)` accumulation class at its root. A string leaf is **always
/// its UTF-8 bytes**, whatever the device's mode — raw bytes are what `bytes`
/// values are for (the pre-`bytes` "Latin-1 byte-string" send rule is gone with
/// the carrier-string era, ADR-141). Iterative worklist, so nesting depth is
/// heap-bounded — and immutable data cannot be cyclic, so termination is
/// structural, no visited set needed.
pub(super) fn flatten_iolist(
    heap: &Heap,
    who: &str,
    root: Value,
    out: &mut Vec<u8>,
) -> Result<(), LispError> {
    let mut stack: Vec<Value> = vec![root];
    while let Some(v) = stack.pop() {
        match v {
            Value::Nil => {}
            Value::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
            Value::Bytes(b) => out.extend_from_slice(heap.bytes(b).as_bytes()),
            Value::Str(id) => {
                let s = heap.string(id);
                out.extend_from_slice(s.as_bytes());
            }
            Value::Pair(p) => {
                // Process car first; the cdr is the rest of the iolist (a leaf
                // there is an improper tail).
                let (car, cdr) = {
                    let cell = heap.pair(p);
                    (cell.0, cell.1)
                };
                stack.push(cdr);
                stack.push(car);
            }
            Value::Vector(id) => {
                let items = heap.vector(id).to_vec();
                for &item in items.iter().rev() {
                    stack.push(item);
                }
            }
            other => {
                return Err(LispError::wrong_type(
                    heap,
                    who,
                    "iolist (string, bytes, byte int 0-255, or a nested list/vector of those)",
                    other,
                ));
            }
        }
    }
    Ok(())
}

/// Lower a tcp-send/proc-send payload to raw bytes: any **iolist** (ADR-139).
/// String leaves are always UTF-8 — the device's binary flag affects only the
/// inbound decode (ADR-141); raw bytes go out as `bytes` values.
pub(super) fn send_payload(heap: &Heap, who: &str, v: Value) -> Result<Vec<u8>, LispError> {
    let mut out = Vec::new();
    flatten_iolist(heap, who, v, &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod base64_tests {
    use super::{BASE64_STANDARD, BASE64_URL, base64_decode_with, base64_encode_with};

    #[test]
    fn encodes_every_tail_length_with_padding() {
        assert_eq!(base64_encode_with(b"", BASE64_STANDARD), "");
        assert_eq!(base64_encode_with(b"hi", BASE64_STANDARD), "aGk=");
        assert_eq!(base64_encode_with(&[255, 0, 1], BASE64_STANDARD), "/wAB");
        assert_eq!(base64_encode_with(&[251, 255], BASE64_URL), "-_8=");
        assert_eq!(base64_encode_with(b"A", BASE64_STANDARD), "QQ==");
    }

    #[test]
    fn round_trips_every_byte_value() {
        let all: Vec<u8> = (0..=255).chain((0..=255).rev()).collect();
        for length in 0..all.len() {
            let encoded = base64_encode_with(&all[..length], BASE64_STANDARD);
            assert_eq!(
                base64_decode_with(encoded.as_bytes(), BASE64_STANDARD).as_deref(),
                Some(&all[..length])
            );
        }
    }

    #[test]
    fn refuses_what_is_not_base64_and_takes_unpadded_input() {
        assert_eq!(
            base64_decode_with(b"aGk", BASE64_STANDARD),
            Some(b"hi".to_vec())
        );
        assert_eq!(base64_decode_with(b"QQ==QQ==", BASE64_STANDARD), None);
        assert_eq!(base64_decode_with(b"QQ===", BASE64_STANDARD), None);
        assert_eq!(base64_decode_with(b"QUJDR", BASE64_STANDARD), None);
        assert_eq!(base64_decode_with(b"-_8=", BASE64_STANDARD), None);
        assert_eq!(base64_decode_with("aGké".as_bytes(), BASE64_STANDARD), None);
    }
}
