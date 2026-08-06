//! The **startup image** mechanism (ADR-218): write a set of global bindings to a
//! binary artifact, and restore them without re-reading or re-evaluating source.
//!
//! Loading a large project from source costs ~1.1 ms per file, essentially all of it
//! evaluating and materialising code into the heap (measured: read 9%, macroexpand 20%,
//! eval+promote 70%). Rebuilding the same bindings *structurally* is ~9× faster — 16 300
//! modules take 20.0 s from source and 2.2 s to rebuild — because it skips parsing,
//! macroexpansion, closure construction from source, and the promote walk.
//!
//! This is **mechanism only** (ADR-006): Rust encodes and decodes, Brood decides what to
//! snapshot, when an image is stale, and who rebuilds it (`std/tool/project.blsp`). The
//! staleness key is an opaque `fingerprint` string the caller computes and this module only
//! stores and compares — so the *policy* for what invalidates an image stays in Brood.
//!
//! The encoding reuses the distribution wire format (`dist::wire`) over `process::Message`,
//! which already serialises closures as data (ADR-033) — so code serialisation is existing,
//! tested machinery rather than a second implementation. It also solves the interner
//! problem for free: `put_sym` writes symbol *names*, not the dense ids, so an image never
//! depends on the order symbols happened to be interned in.
//!
//! **Macros.** `to_message` refuses `Value::Macro` — sending one to another process is
//! meaningless, since macros run at expansion time. An image is the opposite case: a
//! project's `defmacro`s must survive. `Value::Macro` and `Value::Fn` share a `ClosureId`,
//! so a macro is encoded as its closure plus a flag and rebuilt as a macro on load. That
//! is confined to a *top-level binding*, which is where macros live; a macro buried inside
//! a data structure still refuses, exactly as it does for `send`.

use std::io::Cursor;

use super::*;
use crate::core::value::{self, Value};
use crate::dist::wire::{decode_msg, encode_msg};
use crate::process::message::{from_message, to_message};

/// `brood-image` + a format version. Bumping the version invalidates every image
/// written by an older binary, on top of whatever fingerprint the caller supplies.
const MAGIC: &[u8] = b"brood-image-v1\n";

/// `BROOD_IMAGE_TRACE=1` — report where image time goes, split by phase. Both sides run
/// through an intermediate `process::Message` tree (`to_message` / `from_message`) before
/// or after the byte codec, and the whole question for making this faster is how much of
/// the cost is the tree versus the bytes. Cheap to keep: one cached bool.
fn trace() -> bool {
    use std::sync::OnceLock;
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| std::env::var_os("BROOD_IMAGE_TRACE").is_some())
}

/// A string argument, or a type error naming `who`.
fn need_str(heap: &Heap, v: Value, who: &str) -> Result<String, LispError> {
    match v {
        Value::Str(id) => Ok(heap.string(id).to_string()),
        _ => Err(LispError::wrong_type(heap, who, "string", v)),
    }
}

fn put_u32(w: &mut Vec<u8>, n: u32) {
    w.extend_from_slice(&n.to_le_bytes());
}

fn put_str(w: &mut Vec<u8>, s: &str) {
    put_u32(w, s.len() as u32);
    w.extend_from_slice(s.as_bytes());
}

fn get_u32(r: &mut Cursor<Vec<u8>>) -> Option<u32> {
    let p = r.position() as usize;
    let n = {
        let b = r.get_ref();
        if p + 4 > b.len() {
            return None;
        }
        u32::from_le_bytes(b[p..p + 4].try_into().ok()?)
    };
    r.set_position((p + 4) as u64);
    Some(n)
}

fn get_str(r: &mut Cursor<Vec<u8>>) -> Option<String> {
    let n = get_u32(r)? as usize;
    let p = r.position() as usize;
    let s = {
        let b = r.get_ref();
        if p + n > b.len() {
            return None;
        }
        String::from_utf8(b[p..p + n].to_vec()).ok()?
    };
    r.set_position((p + n) as u64);
    Some(s)
}

/// `(%image-write path names fingerprint)` — snapshot the global bindings named by
/// `names` (a sequence of symbols) into the binary image at `path`, stamped with the
/// opaque `fingerprint` string. Returns the number of bindings written.
///
/// A name that is unbound is skipped rather than erroring: the caller derives its list
/// from a live image, and a global can legitimately vanish between the two (a `def`
/// inside a conditional, a test fixture). A value that cannot be encoded — a builtin, a
/// pid, a macro nested in data — raises, because silently dropping a binding would make
/// the image a *different* program from the source, which is far worse than failing loudly.
pub(super) fn image_write(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = need_str(heap, arg(args, 0), "%image-write")?;
    // A list or a vector — callers naturally write `['a 'b]`.
    let names = match arg(args, 1).unpack() {
        value::ValueRef::Vector(id) => heap.vector(id).to_vec(),
        _ => heap.list_to_vec(arg(args, 1))?,
    };
    let fingerprint = need_str(heap, arg(args, 2), "%image-write")?;

    let mut body: Vec<u8> = Vec::with_capacity(1 << 20);
    let (mut ns_to_msg, mut ns_encode) = (0u64, 0u64);
    let mut count: u32 = 0;
    let mut entries: Vec<u8> = Vec::with_capacity(1 << 20);
    let global = heap.global();
    for nv in names {
        let sym = match nv.unpack() {
            value::ValueRef::Sym(s) => s,
            _ => return Err(LispError::type_err("%image-write: names must be symbols")),
        };
        let Some(v) = heap.env_get(global, sym) else {
            continue;
        };
        // A macro rides as its closure plus a flag — see the module docs.
        let (v, is_macro) = match v {
            Value::Macro(id) => (Value::Fn(id), 1u8),
            other => (other, 0u8),
        };
        let t0 = std::time::Instant::now();
        let msg = to_message(heap, v).map_err(|e| {
            LispError::type_err(format!(
                "%image-write: cannot image global '{}': {}",
                value::symbol_name(sym),
                e
            ))
        })?;
        ns_to_msg += t0.elapsed().as_nanos() as u64;
        let t1 = std::time::Instant::now();
        put_str(&mut entries, &value::symbol_name(sym));
        entries.push(is_macro);
        encode_msg(&mut entries, &msg)
            .map_err(|e| LispError::runtime(format!("%image-write: encode failed: {e}")))?;
        ns_encode += t1.elapsed().as_nanos() as u64;
        count += 1;
    }

    // Declared sigs (ADR-218). These live in `RuntimeCode::declared_sigs`, NOT in the
    // globals table, so imaging only globals loses every user `(sig …)` and the checker
    // silently falls back to inferring from the body — weaker advice with no error, which
    // is exactly the failure shape worth spending a file section on.
    let mut sigs: Vec<u8> = Vec::new();
    let mut sig_count: u32 = 0;
    for (sym, tv) in heap.declared_sigs_snapshot() {
        let Ok(msg) = to_message(heap, tv) else {
            continue; // a type expression with no portable form: skip, never fail the image
        };
        put_str(&mut sigs, &value::symbol_name(sym));
        encode_msg(&mut sigs, &msg)
            .map_err(|e| LispError::runtime(format!("%image-write: sig encode failed: {e}")))?;
        sig_count += 1;
    }

    body.extend_from_slice(MAGIC);
    put_str(&mut body, &fingerprint);
    put_u32(&mut body, count);
    body.extend_from_slice(&entries);
    put_u32(&mut body, sig_count);
    body.extend_from_slice(&sigs);

    let t_io = std::time::Instant::now();
    std::fs::write(&path, &body)
        .map_err(|e| LispError::runtime(format!("%image-write: {path}: {e}")))?;
    if trace() {
        eprintln!(
            "[image-write] {} bindings, {} MB — to_message {} ms, encode {} ms, write {} ms",
            count,
            body.len() / (1 << 20),
            ns_to_msg / 1_000_000,
            ns_encode / 1_000_000,
            t_io.elapsed().as_millis()
        );
    }
    Ok(Value::int(count as i64))
}

/// `(%image-read path fingerprint)` — restore the bindings in the image at `path`,
/// but only if its stamp equals `fingerprint`. Returns the number of bindings defined,
/// or **nil** when the image is absent, unreadable, of a different format version, or
/// stale. Nil is the "rebuild me" signal: every failure is a cache miss, never an error,
/// so a corrupt or half-written image degrades to loading from source.
pub(super) fn image_read(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = need_str(heap, arg(args, 0), "%image-read")?;
    let want = need_str(heap, arg(args, 1), "%image-read")?;
    let t_all = std::time::Instant::now();
    let t_io = std::time::Instant::now();
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(Value::Nil);
    };
    let ns_io = t_io.elapsed().as_nanos() as u64;
    if !bytes.starts_with(MAGIC) {
        return Ok(Value::Nil);
    }
    let mut r = Cursor::new(bytes);
    r.set_position(MAGIC.len() as u64);
    let Some(got) = get_str(&mut r) else {
        return Ok(Value::Nil);
    };
    if got != want {
        return Ok(Value::Nil);
    }
    let Some(count) = get_u32(&mut r) else {
        return Ok(Value::Nil);
    };

    let global = heap.global();
    let mut done: i64 = 0;
    let (mut ns_decode, mut ns_from_msg) = (0u64, 0u64);
    let (mut ns_intern, mut ns_define) = (0u64, 0u64);
    for _ in 0..count {
        let Some(name) = get_str(&mut r) else {
            return Ok(Value::Nil);
        };
        let p = r.position() as usize;
        if p >= r.get_ref().len() {
            return Ok(Value::Nil);
        }
        let is_macro = r.get_ref()[p];
        r.set_position((p + 1) as u64);
        let t0 = std::time::Instant::now();
        let Ok(msg) = decode_msg(&mut r) else {
            return Ok(Value::Nil);
        };
        ns_decode += t0.elapsed().as_nanos() as u64;
        let t1 = std::time::Instant::now();
        let v = from_message(heap, &msg);
        ns_from_msg += t1.elapsed().as_nanos() as u64;
        let v = if is_macro == 1 {
            match v {
                Value::Fn(id) => Value::Macro(id),
                other => other,
            }
        } else {
            v
        };
        let t2 = std::time::Instant::now();
        let sym = value::intern(&name);
        ns_intern += t2.elapsed().as_nanos() as u64;
        let t3 = std::time::Instant::now();
        heap.env_define(global, sym, v);
        ns_define += t3.elapsed().as_nanos() as u64;
        done += 1;
    }
    // The sig section. Absent in an older image: treat as "no sigs" rather than a miss,
    // so a format that only gained a section stays readable.
    if let Some(nsigs) = get_u32(&mut r) {
        for _ in 0..nsigs {
            let Some(name) = get_str(&mut r) else { break };
            let Ok(msg) = decode_msg(&mut r) else { break };
            let tv = from_message(heap, &msg);
            heap.set_declared_sig(value::intern(&name), tv);
        }
    }
    // Everything just promoted is bound to a global, so it is all live. Tell the RUNTIME
    // collector that rather than let it find out by compacting the whole region at the next
    // safepoint and reclaiming nothing — measured at 4.0 s of an 8.3 s restore.
    heap.rt_gc_rebaseline_all_live();
    if trace() {
        eprintln!(
            "[image-read] {} bindings — TOTAL {} ms = read {} ms + decode {} ms + from_message {} ms + intern {} ms + define {} ms + other {} ms",
            done,
            t_all.elapsed().as_millis(),
            ns_io / 1_000_000,
            ns_decode / 1_000_000,
            ns_from_msg / 1_000_000,
            ns_intern / 1_000_000,
            ns_define / 1_000_000,
            (t_all.elapsed().as_nanos() as u64)
                .saturating_sub(ns_io + ns_decode + ns_from_msg + ns_intern + ns_define)
                / 1_000_000
        );
    }
    Ok(Value::int(done))
}
