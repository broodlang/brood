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
/// snapshot *map* instead of a table, so every `table-put` against it would then fail
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
/// A **module-private** name (ADR-146). Privacy is recorded by *evaluating*
/// `(%mark-private 'name)`, and materialising a section evaluates nothing — so without
/// this every `defn-` in an imaged module came back PUBLIC. Silently: `(:use mod)` then
/// refers the module's internals, `nest doc` publishes them as API, and `nest check`'s
/// cross-module private-reference error stops firing — a gate that stops gating, the same
/// shape as the declared sigs that a named section once dropped entirely.
///
/// Carried as its own entry rather than a flag on the value entry so it round-trips for a
/// name whose value is not encodable (a `Table`, a closure `to_message` refuses): the
/// privacy fact survives even when the binding itself is skipped.
const KIND_PRIVATE: u8 = 4;

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

/// Encode one section: the global bindings named by `syms`, plus their declared sigs.
/// Returns the section bytes and how many entries went in.
///
/// `all_sigs` asks for EVERY declared sig rather than just this section's. That is the
/// root/project section's shape (one unnamed section carrying the whole image), and it is
/// what `with_sigs` used to mean — the only shape there was. A NAMED section now carries
/// the sigs of its own symbols, because the stdlib image is written as one named section
/// per module and never writes an unnamed one: every std signature was therefore dropped on
/// the floor, and a module restored from the image came back bound but **unsigned**. The
/// visible effect was a gate quietly ceasing to gate — with the image installed, `nest
/// check` lost every std signature, so a reversed-argument call it is supposed to catch
/// (`types::check::tests::a_reversed_index_and_collection_call_is_flagged`) passed silently.
/// Deterministic: 3/3 with the image, 0/3 without.
fn encode_section(
    heap: &mut Heap,
    syms: &[Value],
    all_sigs: bool,
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
    // Sigs: every one for the root/project section, else exactly this section's own — so a
    // signature materialises with the module that declared it, and a module this binary does
    // not bake never contributes one.
    let sig_pairs: Vec<(value::Symbol, Value)> = if all_sigs {
        heap.declared_sigs_snapshot()
    } else {
        let mine: std::collections::HashSet<value::Symbol> = syms
            .iter()
            .filter_map(|nv| match nv.unpack() {
                value::ValueRef::Sym(s) => Some(s),
                _ => None,
            })
            .collect();
        heap.declared_sigs_snapshot()
            .into_iter()
            .filter(|(sym, _)| mine.contains(sym))
            .collect()
    };
    for (sym, tv) in sig_pairs {
        let Ok(msg) = to_message(heap, tv) else {
            continue; // a type expression with no portable form: skip, never fail
        };
        entries.push(KIND_SIG);
        put_str(&mut entries, &value::symbol_name(sym));
        encode_msg(&mut entries, &msg)
            .map_err(|e| LispError::runtime(format!("%image-write: sig encode: {e}")))?;
        count += 1;
    }
    // Privacy, for every name in this section the image records as private. Written after
    // the values so a reader that stops early still has consistent bindings.
    for nv in syms {
        let value::ValueRef::Sym(sym) = nv.unpack() else {
            continue;
        };
        if heap.is_private(sym) {
            entries.push(KIND_PRIVATE);
            put_str(&mut entries, &value::symbol_name(sym));
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
        // The unnamed root/project section takes every sig; a named one takes its own.
        let all_sigs = name.is_empty();
        let (bytes, n) = encode_section(
            heap,
            &syms,
            all_sigs,
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
    // ATOMIC: write a sibling temp file, then rename over the target. A plain
    // `fs::write` truncates in place, so a reader that indexes the image while
    // another process is rebuilding it sees a torn file — and images are now built
    // by `nest`, which a test suite or a build script can easily run several of at
    // once. `rename` within a directory is atomic on POSIX and replaces on Windows,
    // so a reader observes either the old complete image or the new one, never half.
    //
    // The temp name carries the pid so two concurrent builders do not collide with
    // each other either; last rename wins, and both wrote identical bytes anyway
    // (the content is a pure function of `stdlib-id`, which is in the file name).
    let tmp = format!("{path}.{}.tmp", std::process::id());
    std::fs::write(&tmp, &body)
        .map_err(|e| LispError::runtime(format!("%image-write: {tmp}: {e}")))?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(LispError::runtime(format!("%image-write: {path}: {e}")));
    }
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
/// `(%boot-source)` — how this process's prelude arrived, as a keyword.
///
/// A keyword rather than a string because it is a closed set the caller matches on, and
/// because every other "which state is this artifact in" answer in this area
/// (`stdimage/status`'s `:live`/`:stale`/`:absent`) already reads that way.
pub(super) fn boot_source(_args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let _ = heap;
    Ok(Value::Keyword(crate::core::value::intern(
        crate::boot_source(),
    )))
}

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

/// `(%image-load-section path offset len &optional reserve?)` — materialise one section's
/// entries: define its globals (rebuilding macros as macros) and register its declared sigs.
/// Returns how many entries were defined, or **nil** if the bytes could not be read or decoded.
///
/// Seeks straight to the section, so loading one module never touches the rest of the image.
///
/// `reserve?` reproduces the source path's ADR-166 rule: while an embedded module loads, its
/// FUNCTION-valued definitions join the reserved set (so a program cannot redefine
/// `set/union`) while its data globals — its own registries — stay rebindable. Materialising
/// evaluates no `def`, so without this a module restored from an image came back unreserved
/// and `(def path/join …)` was accepted. The caller passes it, because only the caller knows
/// Bind one imaged entry to its global. **The single definition site**, called from both
/// passes of `image_load_section` — the in-order pass and the deferred-entry-point pass
/// (KI-72). It was inlined in both, and the deferred copy carried only the `KIND_MACRO` arm:
/// a deferred TABLE global therefore landed as the decoded map rather than a rebuilt table
/// (`(type-of …)` answered `:map`, and `table-get` on it raised), which
/// `tests/startup_image_test.blsp` catches. Two copies of a `match` over a kind tag is the
/// shape that silently loses an arm, so there is one.
fn define_image_entry(
    heap: &mut Heap,
    global: EnvId,
    kind: u8,
    sym: value::Symbol,
    v: Value,
    reserve: bool,
) -> Result<(), LispError> {
    // `env_define` clears the private mark — right for a real `def` (editing `def-` → `def`
    // and reloading must publish the name), wrong for materialising, which is not a
    // redefinition at all. It bit through the DEFERRED pass: a section's privacy entries are
    // written last, so for a name whose global already exists the value entry is deferred to
    // the second pass and lands AFTER the `KIND_PRIVATE` entry marked it — undoing the mark.
    // That is how installing a stdlib image left `*std-impls*`, `*std-regs*` and
    // `*std-require-edges*` public: private at image-build time, private for one pass of the
    // load, public by the end of it. Save and restore around the define, exactly as
    // `registry_update`/`registry_cas` do, so the order of the two entries stops mattering.
    let was_private = heap.is_private(sym);
    let out = define_image_entry_inner(heap, global, kind, sym, v, reserve);
    if was_private {
        heap.mark_private(sym);
    }
    out
}

fn define_image_entry_inner(
    heap: &mut Heap,
    global: EnvId,
    kind: u8,
    sym: value::Symbol,
    v: Value,
    reserve: bool,
) -> Result<(), LispError> {
    match kind {
        KIND_SIG => heap.set_declared_sig(sym, v),
        KIND_MACRO => {
            let m = match v {
                Value::Fn(id) => Value::Macro(id),
                other => other,
            };
            heap.env_define(global, sym, m);
        }
        // Rebuild the table the write side snapshotted: a fresh store in THIS runtime's
        // registry, refilled from the imaged pairs, so the global is bound to a live table
        // with the contents loading produced. `table::put` deep-clones in, exactly as a
        // `table-put` from Brood would.
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
    // Same predicate the `def` path uses under `in_module_load`: functions, macros and
    // natives become reserved; data globals stay rebindable.
    if reserve
        && matches!(
            v.unpack(),
            value::ValueRef::Fn(_) | value::ValueRef::Macro(_) | value::ValueRef::Native(_)
        )
    {
        heap.reserve_global(sym);
    }
    Ok(())
}

/// whether this section is an embedded module (reserved) or a project's own (not).
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
    let reserve = crate::eval::truthy(arg(args, 3));
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
    // **Names that already have a binding are defined LAST** (KI-72). For a module that is
    // not yet loaded, the only pre-existing binding one of its names can have is an
    // ADR-246 **autoload stub** — and a stub is the door every other process comes
    // through: it routes a caller into `require-one`, which sees the in-flight claim and
    // waits for `provide`. Overwriting it mid-section opens that door early, onto a module
    // whose remaining entries are not bound yet.
    //
    // That is the whole of KI-72's hang. `string/blank?` (public, stubbed) was installed
    // before `string/whitespace?` (private, called from `blank?`'s body), so a racing
    // process took the real `blank?` and died on `unbound symbol: string/whitespace?`. It
    // never sent its reply, and the test's root waited forever for a 24th message — a
    // *wrong answer* presenting as a hang. The source path cannot produce this: it
    // evaluates in file order, where a helper is written before its caller.
    //
    // Deferring is enough; atomicity is not needed. While any stub is still in place the
    // module remains unreachable except through `require-one`, and by the time the stubs
    // are replaced every other entry is bound.
    let mut deferred: Vec<(u8, value::Symbol, Value)> = Vec::new();
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
        // A privacy entry carries a name and no value — branch before decoding one.
        if kind == KIND_PRIVATE {
            heap.mark_private(value::intern(&name));
            done += 1;
            continue;
        }
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
        // A sig is not callable and cannot open the door, so it is never deferred.
        if kind != KIND_SIG && heap.env_get(global, sym).is_some() {
            deferred.push((kind, sym, v));
            ns_define += t2.elapsed().as_nanos() as u64;
            done += 1;
            continue;
        }
        define_image_entry(heap, global, kind, sym, v, reserve)?;
        ns_define += t2.elapsed().as_nanos() as u64;
        done += 1;
    }
    // …and now the deferred entry points, every other binding in the section being live.
    //
    // With `reserve` — an EMBEDDED module materialising from the stdlib image — an entry
    // whose name is already bound to DATA keeps its binding. The stdlib image is pristine:
    // it was written before any program ran, so a module's registries are in it as their
    // empty seeds (`editor/layers/*type-layers*` as `{}`). Loading the same module from
    // SOURCE runs `defonce`, which leaves an existing registry alone; a raw define here did
    // not, and clobbered the registry a project image had just restored — every buffer type
    // lost its layers on every imaged start, and only on the runs that read the image (the
    // run that wrote it had loaded the module from source first). An imaged start must be
    // indistinguishable from a source start, and `defonce` is part of the source start.
    //
    // Only DATA, and only for the pristine image. A pre-existing FUNCTION binding is an
    // autoload stub (ADR-246) that must be replaced with the real thing — the whole reason
    // this pass exists (KI-72). And a PROJECT image describes a later state than the heap
    // it restores into, so it may overwrite (the "basic" test in startup_image_test.blsp
    // writes 41, redefines to :clobbered, and expects 41 back).
    for (kind, sym, v) in deferred {
        if reserve {
            let existing_is_data = !matches!(
                heap.env_get(global, sym).map(|e| e.unpack()),
                Some(value::ValueRef::Fn(_))
                    | Some(value::ValueRef::Macro(_))
                    | Some(value::ValueRef::Native(_))
            );
            if existing_is_data {
                continue;
            }
        }
        define_image_entry(heap, global, kind, sym, v, reserve)?;
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

// ─── The PRELUDE image (ADR-314) ───────────────────────────────────────────────
//
// The sectioned image above serves *modules*, which are `require`d into a live runtime.
// The prelude is a different shape: it is built once per OS process, into a throwaway
// builder heap that is then frozen into the shared read-only region, and every `brood`,
// `nest` and `brood-lsp` invocation pays for it before running a single user form.
//
// ADR-138 chose to cache the prelude as expanded *text*, which removed the ~27 ms macro
// expansion and left parse + eval + freeze — "only ~4 ms" at the time. That residual is now
// **9.4 ms of a 12.4 ms empty run (76%)**: the prelude has grown, and everything else got
// faster. So the same value-materialising trick the module image uses is applied here: the
// cold boot writes the prelude's bindings, and a warm boot rebuilds them structurally
// instead of reading and evaluating 544 forms.
//
// Only the *warm* path is optimized, deliberately. The cold boot may cost whatever it costs
// — it happens once per binary build — so it keeps doing the full source boot and simply
// writes one more artifact at the end.

/// Serialize the prelude's `meta` facts (ADR-283). Four optional fields; a `use_instead`
/// rides as its spelling, since symbol ids are not stable across processes.
fn put_meta(out: &mut Vec<u8>, m: &crate::core::heap::NameMeta) {
    for s in [&m.since, &m.deprecated, &m.beta] {
        match s {
            Some(v) => {
                out.push(1);
                put_str(out, v);
            }
            None => out.push(0),
        }
    }
    match m.use_instead {
        Some(sym) => {
            out.push(1);
            put_str(out, &value::symbol_name(sym));
        }
        None => out.push(0),
    }
}

fn get_meta(r: &mut Cursor<Vec<u8>>) -> Option<crate::core::heap::NameMeta> {
    let opt = |r: &mut Cursor<Vec<u8>>| -> Option<Option<String>> {
        let p = r.position() as usize;
        let tag = *r.get_ref().get(p)?;
        r.set_position((p + 1) as u64);
        if tag == 0 {
            Some(None)
        } else {
            Some(Some(get_str(r)?))
        }
    };
    let since = opt(r)?;
    let deprecated = opt(r)?;
    let beta = opt(r)?;
    let use_instead = opt(r)?.map(|s| value::intern(&s));
    Some(crate::core::heap::NameMeta {
        since,
        deprecated,
        use_instead,
        beta,
    })
}

/// Tags for the side-fact journal (ADR-320). Distinct from the `KIND_*` section tags: a
/// section entry is a BINDING, a fact is something recorded *about* a name. The numbers are
/// format, so append rather than renumber — `PRELUDE_MAGIC` guards a layout change, not a
/// tag reshuffle within one.
const FACT_PRIVATE: u8 = 0;
const FACT_META: u8 = 1;
const FACT_DEF_SITE: u8 = 2;
const FACT_REGISTRY_NAME: u8 = 3;
const FACT_DYNAMIC: u8 = 4;

/// Encode one side fact. **Exhaustive on purpose**: this is the second half of ADR-320's
/// guarantee — `Heap::side_facts` makes a new kind impossible to leave out of the *carry*,
/// and this match makes it impossible to leave out of the *encoding*. Add a `Fact` variant
/// and this stops compiling.
fn put_fact(out: &mut Vec<u8>, fact: &crate::core::heap::Fact) {
    use crate::core::heap::Fact;
    match fact {
        Fact::Private(sym) => {
            out.push(FACT_PRIVATE);
            put_str(out, &value::symbol_name(*sym));
        }
        Fact::Meta(sym, meta) => {
            out.push(FACT_META);
            put_str(out, &value::symbol_name(*sym));
            put_meta(out, meta);
        }
        Fact::DefSite(sym, loc) => {
            out.push(FACT_DEF_SITE);
            put_str(out, &value::symbol_name(*sym));
            // The file is written per entry rather than hoisted: prelude def sites all name
            // the same materialised copy today, but nothing in the format should assume it.
            put_str(out, &loc.file);
            put_u32(out, loc.pos.line);
            put_u32(out, loc.pos.col);
        }
        Fact::RegistryName(sym) => {
            out.push(FACT_REGISTRY_NAME);
            put_str(out, &value::symbol_name(*sym));
        }
        Fact::Dynamic(sym) => {
            out.push(FACT_DYNAMIC);
            put_str(out, &value::symbol_name(*sym));
        }
    }
}

/// Decode one side fact, or `None` for a truncated or unknown-tag file — which fails the
/// whole load, so the text cache takes over rather than a partial fact set being installed.
fn get_fact(r: &mut Cursor<Vec<u8>>) -> Option<crate::core::heap::Fact> {
    use crate::core::heap::Fact;
    let p = r.position() as usize;
    let tag = *r.get_ref().get(p)?;
    r.set_position((p + 1) as u64);
    let sym = value::intern(&get_str(r)?);
    Some(match tag {
        FACT_PRIVATE => Fact::Private(sym),
        FACT_META => Fact::Meta(sym, get_meta(r)?),
        FACT_DEF_SITE => {
            let file = get_str(r)?;
            let line = get_u32(r)?;
            let col = get_u32(r)?;
            Fact::DefSite(
                sym,
                crate::core::heap::SourceLoc {
                    file,
                    pos: crate::error::Pos { line, col },
                },
            )
        }
        FACT_REGISTRY_NAME => Fact::RegistryName(sym),
        FACT_DYNAMIC => Fact::Dynamic(sym),
        _ => return None,
    })
}

/// Write the prelude image for `fingerprint` to `path`: every non-native binding in `root`,
/// plus declared sigs, privacy and `meta`. Best-effort — an error simply means the next boot
/// takes the text-cache path, exactly as a missing file does.
pub(crate) fn write_prelude_image(
    heap: &mut Heap,
    root: EnvId,
    skip: &std::collections::HashSet<value::Symbol>,
    path: &std::path::Path,
    fingerprint: &str,
) -> Result<(), LispError> {
    // `skip` is the set of names bound immediately after `builtins::register` — exactly
    // what the warm path re-creates for itself, and therefore the only thing safe to leave
    // out (395 of ~1109 bindings).
    //
    // The tempting filter is "skip any binding whose VALUE is a native", and it is wrong:
    // a prelude `(def *out* <native>)` binds a primitive under a name `register` never
    // rebinds, so dropping it left `*out*` unbound and every `io/puts` dead. The set of
    // names the registration creates is the precise question, and the caller is the only
    // place that can answer it.
    let syms: Vec<Value> = heap
        .env_chain_names(root)
        .into_iter()
        .filter(|s| !skip.contains(s))
        .map(Value::Sym)
        .collect();

    let (mut body, _count) = encode_section(
        heap,
        &syms,
        true,
        &mut 0u64,
        &mut 0u64,
        &mut std::collections::HashMap::new(),
    )?;

    // The SIDE FACTS — everything the prelude's evaluation RECORDED about a name rather
    // than bound to it: privacy, `meta`, def sites, registry names, `defdyn` marks. These
    // used to be five hand-written blocks here, one added after each of KI-72, KI-84,
    // KI-89, KI-105 and KI-106, because "materialising evaluates nothing" is a rule prose
    // cannot enforce over an open set of fact kinds. `Heap::side_facts` now enumerates
    // them through an exhaustive match, so a sixth kind is carried by construction and its
    // ENCODING is the only thing left to write — `put_fact` below, also exhaustive, which
    // makes forgetting it a compile error rather than a silent omission surfacing in
    // another subsystem three weeks later (ADR-320).
    let facts = heap.side_facts();
    put_u32(&mut body, facts.len() as u32);
    for fact in &facts {
        put_fact(&mut body, fact);
    }

    let mut out = Vec::with_capacity(body.len() + 64);
    out.extend_from_slice(PRELUDE_MAGIC);
    put_str(&mut out, fingerprint);
    out.extend_from_slice(&body);

    let dir = path
        .parent()
        .ok_or_else(|| LispError::runtime("prelude image: no parent dir"))?;
    std::fs::create_dir_all(dir).map_err(|e| LispError::runtime(format!("prelude image: {e}")))?;
    // Temp + rename, so a concurrently booting process never reads a torn file — the same
    // discipline the text cache uses, and it matters more here (many nextest processes).
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, &out).map_err(|e| LispError::runtime(format!("prelude image: {e}")))?;
    std::fs::rename(&tmp, path).map_err(|e| LispError::runtime(format!("prelude image: {e}")))?;
    Ok(())
}

/// Bumped to v2 when the five per-fact blocks became one side-fact journal (ADR-320). The
/// fingerprint below already invalidates on any binary or `std/` change, so this is belt and
/// braces — but a magic that tracks the LAYOUT is what makes a hand-copied or half-written
/// file fail as "not my format" instead of decoding into nonsense.
const PRELUDE_MAGIC: &[u8] = b"brood-prelude-image-v2\n";

/// Restore a prelude image into `root`. `None` for any miss — absent, stale, truncated,
/// or a value that will not decode — and the caller falls back to the text cache. Returns
/// the number of entries defined.
pub(crate) fn load_prelude_image(
    heap: &mut Heap,
    root: EnvId,
    path: &std::path::Path,
    fingerprint: &str,
) -> Option<usize> {
    let bytes = std::fs::read(path).ok()?;
    if !bytes.starts_with(PRELUDE_MAGIC) {
        return None;
    }
    let mut r = Cursor::new(bytes);
    r.set_position(PRELUDE_MAGIC.len() as u64);
    if get_str(&mut r)? != fingerprint {
        return None;
    }
    let count = get_u32(&mut r)?;
    let mut done = 0usize;
    for _ in 0..count {
        let p = r.position() as usize;
        if p >= r.get_ref().len() {
            return None;
        }
        let kind = r.get_ref()[p];
        r.set_position((p + 1) as u64);
        let name = get_str(&mut r)?;
        if kind == KIND_PRIVATE {
            heap.mark_private(value::intern(&name));
            done += 1;
            continue;
        }
        let msg = decode_msg(&mut r).ok()?;
        let v = from_message(heap, &msg);
        let sym = value::intern(&name);
        // No deferral dance here (contrast `image_load_section`): this heap is fresh and
        // holds only the natives `register` just bound, so there are no autoload stubs to
        // open early and nothing to overwrite out of order. Definition order is free.
        define_image_entry(heap, root, kind, sym, v, false).ok()?;
        done += 1;
    }
    // The side facts, replayed through the same entry points ordinary evaluation uses, so a
    // restored fact is indistinguishable from a recorded one (see the writer, and ADR-320).
    // A truncated file fails `get_u32`/`get_fact` here and the WHOLE load returns None — the
    // text cache takes over, and a half-restored fact set is never observable.
    let fact_count = get_u32(&mut r)?;
    for _ in 0..fact_count {
        let fact = get_fact(&mut r)?;
        heap.replay_fact(&fact);
    }
    Some(done)
}
