//! The **startup image** mechanism (ADR-218): write a set of global bindings to a binary
//! artifact, and restore them without re-reading or re-evaluating source.
//!
//! Loading a large project from source costs ~1.1 ms per file, essentially all of it
//! evaluating and materialising code into the heap (measured: read 9%, macroexpand 20%,
//! eval+promote 70%). Rebuilding the same bindings *structurally* is several times faster,
//! because it skips parsing, macroexpansion, closure construction from source, and the
//! promote walk.
//!
//! **The image is sectioned by module, and a section is loaded on demand.** A whole-image
//! restore of a 16 300-file project costs seconds and hundreds of MB of resident code that
//! an entry point touching twenty modules will never call. Sections make `require` the unit
//! of materialisation — the same granularity BEAM loads `.beam` files at — so a program
//! pays only for the modules it actually reaches. The section directory lives at the END of
//! the file with absolute offsets, so opening an image reads the header and directory only,
//! and loading one module seeks straight to its bytes instead of reading the whole file.
//!
//! This is **mechanism only** (ADR-006): Rust encodes, decodes and seeks; Brood decides what
//! to snapshot, how to group it, when an image is stale, and who rebuilds it
//! (`std/tool/project.blsp`). The staleness key is an opaque `fingerprint` string this layer
//! only stores and compares.
//!
//! The encoding reuses the distribution wire format (`dist::wire`) over `process::Message`,
//! which already serialises closures as data (ADR-033) — so code serialisation is existing,
//! tested machinery rather than a second implementation. It also solves the interner problem
//! for free: `put_sym` writes symbol *names*, not the dense ids, so an image never depends on
//! the order symbols happened to be interned in.
//!
//! **Macros.** `to_message` refuses `Value::Macro`, correctly: sending one to another process
//! is meaningless, since macros run at expansion time. An image is the opposite case, and a
//! project's `defmacro`s must survive. `Value::Macro` and `Value::Fn` share a `ClosureId`, so
//! a macro is encoded as its closure plus a kind tag and rebuilt as a macro. That is confined
//! to a *top-level binding*; a macro buried inside a data structure still refuses, as for
//! `send`.
//!
//! **Declared sigs** (`(sig f (int -> int))`) live in `RuntimeCode::declared_sigs`, not in the
//! globals table, so an image that snapshotted only globals silently lost them and the checker
//! fell back to inferring from the body — `expects int` became `expects number | map`. Weaker
//! advice with no error. They ride in the root section, which is always materialised.

use std::io::{Cursor, Read, Seek, SeekFrom};

use super::*;
use crate::core::value::{self, Value};
use crate::dist::wire::{decode_msg, encode_msg};
use crate::process::message::{from_message, to_message, to_message_image};

/// `brood-image` + a format version. Bumping the version invalidates every image written by
/// an older binary, on top of whatever fingerprint the caller supplies.
///
/// v4 adds `KIND_TABLE`. The bump is required, not cosmetic: a v3 reader meeting a
/// `KIND_TABLE` entry would fall through its `_ =>` arm and bind the global to the
/// snapshot *map* instead of a table, so every `table/put` against it would then fail
/// on a type error far from the cause.
///
/// v5 records the `defdyn` dynamic-var marks (after the directory, in the region
/// `%image-index` reads whole) and re-establishes them on open — an imaged start restores a
/// dynamic global's value but skips the module load that ran the `defdyn`, so without this the
/// mark is missing and `binding` rejects the var. A v4 image has no such list, so a v5 reader
/// must reject it (else it would read the footer as the dynamic-name count); the bump does that.
const MAGIC: &[u8] = b"brood-image-v5\n";

/// Entry kinds inside a section.
const KIND_GLOBAL: u8 = 0;
const KIND_MACRO: u8 = 1;
const KIND_SIG: u8 = 2;
/// A global bound to a `Value::Table` — stored as the table's *contents* (its snapshot
/// map) and rebuilt into a fresh table on restore. See `encode_section`.
const KIND_TABLE: u8 = 3;

/// `BROOD_IMAGE_TRACE=1` — report where image time goes, split by phase. Both sides run
/// through an intermediate `process::Message` tree before or after the byte codec, and the
/// question for making this faster is how much of the cost is the tree versus the bytes.
/// Cheap to keep: one cached bool.
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

/// A list or a vector — callers naturally write `['a 'b]`.
fn seq_items(heap: &Heap, v: Value) -> Result<Vec<Value>, LispError> {
    match v.unpack() {
        value::ValueRef::Vector(id) => Ok(heap.vector(id).to_vec()),
        _ => heap.list_to_vec(v),
    }
}

fn put_u32(w: &mut Vec<u8>, n: u32) {
    w.extend_from_slice(&n.to_le_bytes());
}

fn put_u64(w: &mut Vec<u8>, n: u64) {
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

/// Encode one section: the global bindings named by `syms`, plus (for the root section)
/// every declared sig. Returns the section bytes and how many entries went in.
fn encode_section(
    heap: &mut Heap,
    syms: &[Value],
    with_sigs: bool,
    ns_to_msg: &mut u64,
    ns_encode: &mut u64,
    tables_seen: &mut std::collections::HashMap<u64, String>,
) -> Result<(Vec<u8>, u32), LispError> {
    let mut out: Vec<u8> = Vec::new();
    let mut entries: Vec<u8> = Vec::new();
    let mut count: u32 = 0;
    let global = heap.global();
    for nv in syms {
        let sym = match nv.unpack() {
            value::ValueRef::Sym(s) => s,
            _ => return Err(LispError::type_err("%image-write: names must be symbols")),
        };
        // An unbound name is skipped rather than an error: the caller derives its list from
        // the live image, and a global can legitimately vanish between the two.
        let Some(v) = heap.env_get(global, sym) else {
            continue;
        };
        // A `Value::Table` is a handle into a per-runtime registry, so it has no portable
        // form and `to_message` refuses it — which used to forfeit the WHOLE image for the
        // project. That is the wrong trade: `table` is the language's only sanctioned
        // mutable structure (ADR-026/107), so `(def *cache* (table))` is the blessed way to
        // hold shared state, and any project doing it lost image startup entirely.
        //
        // Image the table by VALUE — its snapshot map — and rebuild a fresh table from
        // those pairs on restore. That reproduces "the program as it stood after loading":
        // whatever load-time code put in the table is still there, and a table that was
        // empty at load comes back empty. Identity is necessarily fresh, which costs
        // nothing, because a restored runtime has no other handle to the old store — the
        // registry is per-runtime and the old one does not exist in this process.
        //
        // Confined to a TOP-LEVEL binding, exactly as `Value::Macro` is: a table buried
        // inside a data structure still refuses, because rebuilding it there would silently
        // split an identity the program can observe.
        let (v, kind) = match v {
            Value::Macro(id) => (Value::Fn(id), KIND_MACRO),
            Value::Table(tid) => {
                // Two globals bound to the SAME table alias one store, and restoring them
                // independently would hand the program two — a real semantic change, where
                // a write through one stopped being visible through the other. Rare enough
                // not to be worth a cross-section identity map (sections restore lazily and
                // independently), so refuse it loudly and let the caller load from source.
                if let Some(first) = tables_seen.get(&tid) {
                    return Err(LispError::type_err(format!(
                        "%image-write: cannot image global '{}': it is the same table as \
                         '{first}', and the image would restore them as two separate tables",
                        value::symbol_name(sym)
                    )));
                }
                tables_seen.insert(tid, value::symbol_name(sym).to_string());
                (crate::core::table::snapshot(heap, tid)?, KIND_TABLE)
            }
            other => (other, KIND_GLOBAL),
        };
        let t0 = std::time::Instant::now();
        // A value with no portable form (a pid, a table, an open socket) raises: silently
        // dropping a binding would make the image a *different program* from the source, which
        // is far worse than failing loudly. The caller reports it and loads from source
        // instead. A *builtin* is the exception — `to_message_image` carries it by name (the
        // image restores bindings in the same, binary-keyed runtime), so a global holding a
        // primitive (e.g. `std/editor/ui`'s `*term-display*`, a map of `term-*` prims) no
        // longer forfeits the whole image.
        let msg = to_message_image(heap, v).map_err(|e| {
            LispError::type_err(format!(
                "%image-write: cannot image global '{}': {e}",
                value::symbol_name(sym)
            ))
        })?;
        *ns_to_msg += t0.elapsed().as_nanos() as u64;
        let t1 = std::time::Instant::now();
        entries.push(kind);
        put_str(&mut entries, &value::symbol_name(sym));
        encode_msg(&mut entries, &msg).map_err(|e| {
            LispError::runtime(format!(
                "%image-write: cannot image global '{}': {e}",
                value::symbol_name(sym)
            ))
        })?;
        *ns_encode += t1.elapsed().as_nanos() as u64;
        count += 1;
    }
    if with_sigs {
        for (sym, tv) in heap.declared_sigs_snapshot() {
            let Ok(msg) = to_message(heap, tv) else {
                continue; // a type expression with no portable form: skip, never fail
            };
            entries.push(KIND_SIG);
            put_str(&mut entries, &value::symbol_name(sym));
            encode_msg(&mut entries, &msg)
                .map_err(|e| LispError::runtime(format!("%image-write: sig encode: {e}")))?;
            count += 1;
        }
    }
    put_u32(&mut out, count);
    out.extend_from_slice(&entries);
    Ok((out, count))
}

/// `(%image-write path sections fingerprint)` — write a sectioned startup image.
///
/// `sections` is a sequence of `[name syms]` pairs: `name` is the section key (a module's
/// feature name, or `""` for the always-materialised root), `syms` the global names it
/// holds. Declared sigs are added to the root section automatically. Returns the total
/// number of entries written.
pub(super) fn image_write(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = need_str(heap, arg(args, 0), "%image-write")?;
    let sections = seq_items(heap, arg(args, 1))?;
    let fingerprint = need_str(heap, arg(args, 2), "%image-write")?;

    let (mut ns_to_msg, mut ns_encode) = (0u64, 0u64);

    // Header first, so section offsets can be absolute from the start of the file.
    let mut body: Vec<u8> = Vec::with_capacity(1 << 20);
    body.extend_from_slice(MAGIC);
    put_str(&mut body, &fingerprint);

    let mut dir: Vec<(String, u64, u32)> = Vec::new();
    let mut total: u32 = 0;
    // Table handles seen anywhere in this write, so an aliased pair is caught across
    // sections and not just within one (see `encode_section`).
    let mut tables_seen: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    for sv in sections {
        let pair = seq_items(heap, sv)?;
        if pair.len() != 2 {
            return Err(LispError::type_err(
                "%image-write: each section must be [name syms]",
            ));
        }
        let name = need_str(heap, pair[0], "%image-write")?;
        let syms = seq_items(heap, pair[1])?;
        let with_sigs = name.is_empty();
        let (bytes, n) = encode_section(
            heap,
            &syms,
            with_sigs,
            &mut ns_to_msg,
            &mut ns_encode,
            &mut tables_seen,
        )?;
        dir.push((name, body.len() as u64, bytes.len() as u32));
        body.extend_from_slice(&bytes);
        total += n;
    }

    // Directory at the END, with a footer pointing at it: an image can then be *opened* by
    // reading its header and directory alone, and one section loaded by seeking to its
    // offset — neither reads the payload.
    let dir_off = body.len() as u64;
    put_u32(&mut body, dir.len() as u32);
    for (name, off, len) in &dir {
        put_str(&mut body, name);
        put_u64(&mut body, *off);
        put_u32(&mut body, *len);
    }
    // The `defdyn` marks (which globals are dynamic vars). They live *after* the directory but
    // still before the footer, so they sit inside the `[dir_off, footer)` span `%image-index`
    // reads whole — the image restores them when opened, no payload read. Written last (at
    // image-build time the whole runtime is loaded, so `DYNAMICS` is complete).
    let dyn_names = value::dynamic_names();
    put_u32(&mut body, dyn_names.len() as u32);
    for name in &dyn_names {
        put_str(&mut body, name);
    }
    put_u64(&mut body, dir_off);

    let t_io = std::time::Instant::now();
    std::fs::write(&path, &body)
        .map_err(|e| LispError::runtime(format!("%image-write: {path}: {e}")))?;
    if trace() {
        eprintln!(
            "[image-write] {} entries in {} sections, {} MB — to_message {} ms, encode {} ms, write {} ms",
            total,
            dir.len(),
            body.len() / (1 << 20),
            ns_to_msg / 1_000_000,
            ns_encode / 1_000_000,
            t_io.elapsed().as_millis()
        );
    }
    Ok(Value::int(total as i64))
}

/// Read `len` bytes at `off` from `path` without reading the rest of the file.
fn read_at(path: &str, off: u64, len: usize) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// `(%image-index path fingerprint)` — open the image at `path` if it exists and its stamp
/// equals `fingerprint`, returning its section directory as a map `{name → [offset len]}`.
/// **nil** for any miss: absent, unreadable, wrong format version, or stale.
///
/// Reads the header and the directory only — never the payload — so opening a 143 MB image
/// costs a few kilobytes of I/O.
pub(super) fn image_index(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = need_str(heap, arg(args, 0), "%image-index")?;
    let want = need_str(heap, arg(args, 1), "%image-index")?;

    let Ok(meta) = std::fs::metadata(&path) else {
        return Ok(Value::Nil);
    };
    let size = meta.len();
    if size < (MAGIC.len() + 8) as u64 {
        return Ok(Value::Nil);
    }
    // Header: magic + fingerprint. Read a bounded prefix — the fingerprint is the only
    // variable-length part and the caller built it, so its length is known to be sane.
    let head_len = (MAGIC.len() + 4 + want.len()).min(size as usize);
    let Some(head) = read_at(&path, 0, head_len) else {
        return Ok(Value::Nil);
    };
    if !head.starts_with(MAGIC) {
        return Ok(Value::Nil);
    }
    let mut hr = Cursor::new(head);
    hr.set_position(MAGIC.len() as u64);
    let Some(got) = get_str(&mut hr) else {
        return Ok(Value::Nil);
    };
    if got != want {
        return Ok(Value::Nil);
    }
    // Footer: the directory's offset.
    let Some(foot) = read_at(&path, size - 8, 8) else {
        return Ok(Value::Nil);
    };
    let dir_off = u64::from_le_bytes(foot.try_into().ok().unwrap_or([0; 8]));
    if dir_off >= size - 8 {
        return Ok(Value::Nil);
    }
    let Some(dir_bytes) = read_at(&path, dir_off, (size - 8 - dir_off) as usize) else {
        return Ok(Value::Nil);
    };
    let mut dr = Cursor::new(dir_bytes);
    let Some(n) = get_u32(&mut dr) else {
        return Ok(Value::Nil);
    };
    let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let Some(name) = get_str(&mut dr) else {
            return Ok(Value::Nil);
        };
        let p = dr.position() as usize;
        let b = dr.get_ref();
        if p + 12 > b.len() {
            return Ok(Value::Nil);
        }
        let off = u64::from_le_bytes(b[p..p + 8].try_into().unwrap());
        let len = u32::from_le_bytes(b[p + 8..p + 12].try_into().unwrap());
        dr.set_position((p + 12) as u64);
        let k = heap.alloc_string(&name);
        let v = heap.alloc_vector(vec![Value::int(off as i64), Value::int(len as i64)]);
        pairs.push((k, v));
    }
    // Re-establish the `defdyn` dynamic-var marks recorded after the directory (v5): an imaged
    // start restores a dynamic global's value but skips the module load that ran the `defdyn`,
    // so `binding` on it would fail. Marking is idempotent + monotonic, so a spurious entry is
    // harmless (and a stale image is fingerprint-rejected before we get here). Done in the same
    // `%image-index` pass every install runs, so the marks are back before any section loads.
    if let Some(dyn_count) = get_u32(&mut dr) {
        for _ in 0..dyn_count {
            match get_str(&mut dr) {
                Some(name) => value::mark_dynamic(value::intern(&name)),
                None => break,
            }
        }
    }
    Ok(heap.map_from_pairs(pairs))
}

/// `(%image-load-section path offset len)` — materialise one section's entries: define its
/// globals (rebuilding macros as macros) and register its declared sigs. Returns how many
/// entries were defined, or **nil** if the bytes could not be read or decoded.
///
/// Seeks straight to the section, so loading one module never touches the rest of the image.
pub(super) fn image_load_section(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = need_str(heap, arg(args, 0), "%image-load-section")?;
    let off = match arg(args, 1) {
        Value::Int(n) if n >= 0 => n as u64,
        v => return Err(LispError::wrong_type(heap, "%image-load-section", "int", v)),
    };
    let len = match arg(args, 2) {
        Value::Int(n) if n >= 0 => n as usize,
        v => return Err(LispError::wrong_type(heap, "%image-load-section", "int", v)),
    };
    let Some(bytes) = read_at(&path, off, len) else {
        return Ok(Value::Nil);
    };
    let mut r = Cursor::new(bytes);
    let Some(count) = get_u32(&mut r) else {
        return Ok(Value::Nil);
    };
    let global = heap.global();
    let mut done: i64 = 0;
    let (mut ns_decode, mut ns_from_msg, mut ns_define) = (0u64, 0u64, 0u64);
    for _ in 0..count {
        let p = r.position() as usize;
        if p >= r.get_ref().len() {
            return Ok(Value::Nil);
        }
        let kind = r.get_ref()[p];
        r.set_position((p + 1) as u64);
        let Some(name) = get_str(&mut r) else {
            return Ok(Value::Nil);
        };
        let t0 = std::time::Instant::now();
        let Ok(msg) = decode_msg(&mut r) else {
            return Ok(Value::Nil);
        };
        ns_decode += t0.elapsed().as_nanos() as u64;
        let t1 = std::time::Instant::now();
        let v = from_message(heap, &msg);
        ns_from_msg += t1.elapsed().as_nanos() as u64;
        let sym = value::intern(&name);
        let t2 = std::time::Instant::now();
        match kind {
            KIND_SIG => heap.set_declared_sig(sym, v),
            KIND_MACRO => {
                let m = match v {
                    Value::Fn(id) => Value::Macro(id),
                    other => other,
                };
                heap.env_define(global, sym, m);
            }
            // Rebuild the table the write side snapshotted: a fresh store in THIS
            // runtime's registry, refilled from the imaged pairs, so the global is bound
            // to a live table with the contents loading produced. `table::put` deep-clones
            // in, exactly as a `table/put` from Brood would.
            KIND_TABLE => {
                let tid = crate::core::table::create();
                if let value::ValueRef::Map(mid) = v.unpack() {
                    for (k, val) in heap.map_entries(mid) {
                        crate::core::table::put(heap, tid, k, val)?;
                    }
                }
                heap.env_define(global, sym, Value::Table(tid));
            }
            _ => heap.env_define(global, sym, v),
        }
        ns_define += t2.elapsed().as_nanos() as u64;
        done += 1;
    }
    // Everything just promoted is bound to a global, so it is all live. Tell the RUNTIME
    // collector that rather than let it find out by compacting the whole region at the next
    // safepoint and reclaiming nothing — measured at 4.0 s of an 8.3 s whole-image restore.
    heap.rt_gc_rebaseline_all_live();
    if trace() {
        eprintln!(
            "[image-section] {done} entries — decode {} ms, from_message {} ms, define {} ms",
            ns_decode / 1_000_000,
            ns_from_msg / 1_000_000,
            ns_define / 1_000_000
        );
    }
    Ok(Value::int(done))
}
