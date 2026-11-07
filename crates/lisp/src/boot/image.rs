//! The **startup image** mechanism (ADR-218): write a set of global bindings to a binary
//! artifact, and restore them without re-reading or re-evaluating source.
//!
//! Loading a large project from source is dominated by evaluating and materialising code
//! into the heap, not by reading it. Rebuilding the same bindings *structurally* is
//! several times faster, because it skips parsing, macroexpansion, closure construction
//! from source, and the promote walk.
//!
//! **The image is sectioned by module, and a section is loaded on demand.** A whole-image
//! restore of a 16 300-file project costs seconds and hundreds of MB of resident code that
//! an entry point touching twenty modules will never call. Sections make `require` the unit
//! of materialisation — one compiled module at a time — so a program
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

use crate::builtins::args::{arg, expect_string};
use crate::core::heap::Heap;
use crate::core::value::{self, EnvId, Value};
use crate::dist::wire::{decode_msg, encode_msg};
use crate::error::{LispError, LispResult};
use crate::process::message::{from_message, to_message, to_message_image};

/// Every primitive this file contributes: name, arity, signature, arglist, docstring.
pub(crate) fn register(primitives: &mut crate::builtins::Primitives) {
    use crate::builtins::signature_types::*;
    use crate::core::value::Arity;
    use crate::types::Sig;
    // Startup-image prims (ADR-218) — encode/decode a set of global bindings to a
    // binary artifact so a large project starts by rebuilding values instead of
    // re-evaluating source. Mechanism only: the fingerprint is an opaque string this
    // layer stores and compares, and every *policy* question — what to snapshot, what
    // invalidates an image, who rebuilds it — is Brood in std/tool/project.blsp.
    primitives.def(
        "%image-write",
        Arity::exact(3),
        Sig::new(vec![any, any, any], any),
        &["path", "sections", "fingerprint"],
        "Write a sectioned startup image to `path`, stamped with the opaque `fingerprint` string; returns the number of entries written. `sections` is a sequence of [name syms] pairs — `name` is the section key (a module's feature name, or `\"\"` for the always-materialised root), `syms` the global names it holds; declared sigs are added to the root section automatically. In place of `syms`, `[:copy offset length]` copies that section's bytes verbatim from the image now at `path` (a partial rebuild keeps an unchanged module's section without materialising it, ADR-419). An unbound name is skipped; a value with no portable form (a pid, a socket) raises rather than silently dropping a binding. Two kinds are carried specially at a *top-level binding* (never nested inside a structure): a **builtin** rides by name, and a **table** rides as its contents — the snapshot is imaged and a fresh table rebuilt from it on restore, since a handle is per-runtime. Two globals aliasing ONE table raise, because restoring them would split it. Mechanism only (ADR-218) — the grouping and the staleness policy are Brood in std/tool/project.blsp.",
        image_write);
    primitives.def(
        "%boot-source",
        Arity::exact(0),
        Sig::new(vec![], any),
        &[],
        "How THIS process's prelude arrived, as a keyword: `:prelude-image` (ADR-314) or `:source` (a cold boot, which also WRITES the image — or `BROOD_NO_PRELUDE_IMAGE=1`, which reads and writes nothing; ADR-329). The question every image measurement depends on and none could ask: the cache id embeds the binary's build id, so the first run after any rebuild is a source boot and every later one is not — a developer checking an image fix meets the un-imaged path exactly when they least expect it. Three \"it is fixed\" readings during KI-106 were cold boots, and ADR-314 records the same trap corrupting a diagnosis in a session already caught by it twice.",
        boot_source_prim);
    primitives.def(
        "%image-index",
        Arity::exact(2),
        Sig::new(vec![any, any], any),
        &["path", "fingerprint"],
        "Open the sectioned startup image at `path` if its stamp equals `fingerprint`, returning its section directory as a map {name -> [offset len]} — `\"\"` is the always-materialised root section, every other key a module's feature name. nil for any miss (absent, unreadable, older format, stale), which is the rebuild-me signal. Reads the header and directory only, never the payload (ADR-218).",
        image_index);
    primitives.def(
        "%image-exists?",
        Arity::exact(1),
        Sig::new(vec![any], bool_ty),
        &["path"],
        "Whether an image exists at `path`: a file, or — for a path under `mem:` — bytes the host handed the runtime (a tab has no filesystem, ADR-414) or `%image-write` wrote there. Says nothing about whether it is current; `%image-index` checks the stamp.",
        image_exists_prim);
    primitives.def(
        "%image-load-section",
        Arity::range(3, 5),
        Sig::new(vec![any, any, any, any, any], any),
        &["path", "offset", "len", "reserve?", "module"],
        "Materialise one section of a startup image: define its globals (rebuilding macros as macros) and register its declared sigs. Returns how many entries were defined, or nil if the bytes could not be read or decoded. Seeks straight to the section, so loading one module never reads the rest of the image (ADR-218). With `reserve?` truthy, function-valued globals join the reserved set as an embedded module's own defs do when it loads from source (ADR-166). `module` names the std module this section holds, so the positions its forms carry are stamped with THAT module's source file rather than whatever the process is loading (KI-184).",
        image_load_section);
}

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
/// v6 records, after the dynamic marks, the names of every MACRO the image holds (ADR-335):
/// a qualified call head into an imaged module may defer its load to first use only once the
/// runtime knows the head is not a macro, and the kind is a fact only the image has without
/// materialising the section. A v5 reader would read the list as trailing garbage and a v6
/// reader would mis-read a v5 footer, so the bump is required.
/// v7 (ADR-370) appends, after the macro names, the SIGNATURES of every imaged module's
/// functions — `(qualified name, type-form text)` pairs, declared or inferred at build time —
/// so a checker can read a callee's type from the footer without materialising its module.
/// v8 (ADR-383): the closure record — the wire codec's, shared with `dist` — carries the
/// closure's authoring module, its contract boundary. A v7 reader would take the extra
/// optional symbol for the next entry's kind byte.
const MAGIC: &[u8] = b"brood-image-v8\n";

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
/// Where a name was DEFINED — file, line, column — the fact `(source-location 'name)`, the
/// LSP's go-to-definition and the incremental checker's dependency fingerprint all read
/// (`types::check::deps::fact_of_sym`: a user global's fact is `D<file>@<mtime>`). Recorded
/// by *evaluating* the `def`, so without this entry a materialised module had none and a
/// global's fact depended on HOW its module arrived — defeating the ADR-119 check cache
/// across every source ↔ image flip. Carried like privacy — a name and no value — so it
/// round-trips for a name whose binding is not encodable. The prelude image carries the
/// same fact through `Heap::side_facts` (ADR-320); its root section holds it twice,
/// idempotently.
const KIND_DEF_SITE: u8 = 5;

/// `BROOD_IMAGE_TRACE=1` — report where image time goes, split by phase. Both sides run
/// through an intermediate `process::Message` tree before or after the byte codec, and the
/// question for making this faster is how much of the cost is the tree versus the bytes.
/// Cheap to keep: one cached bool.
fn trace() -> bool {
    crate::diagnostics::debug_flags::image_trace()
}

/// A list or a vector — callers naturally write `['a 'b]`.
fn seq_items(heap: &Heap, v: Value) -> Result<Vec<Value>, LispError> {
    heap.list_or_vector_items(v)
        .ok_or_else(|| LispError::type_err("improper list"))
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

/// The next `n` bytes of `r`, advancing past them; `None` (and `r` unmoved) when fewer
/// remain. The one bounds check every image read goes through.
fn take(r: &mut Cursor<Vec<u8>>, n: usize) -> Option<&[u8]> {
    let start = r.position() as usize;
    let end = start.checked_add(n)?;
    if end > r.get_ref().len() {
        return None;
    }
    r.set_position(end as u64);
    Some(&r.get_ref()[start..end])
}

fn get_u8(r: &mut Cursor<Vec<u8>>) -> Option<u8> {
    Some(take(r, 1)?[0])
}

fn get_u64(r: &mut Cursor<Vec<u8>>) -> Option<u64> {
    Some(u64::from_le_bytes(take(r, 8)?.try_into().ok()?))
}

fn get_u32(r: &mut Cursor<Vec<u8>>) -> Option<u32> {
    Some(u32::from_le_bytes(take(r, 4)?.try_into().ok()?))
}

/// Write a def site as [`get_def_site`] reads it: file, line, column.
fn put_def_site(out: &mut Vec<u8>, loc: &crate::core::heap::SourceLoc) {
    put_str(out, &loc.file);
    put_u32(out, loc.pos.line);
    put_u32(out, loc.pos.col);
}

/// The payload of a `KIND_DEF_SITE` entry (after its name): file, line, column.
fn get_def_site(r: &mut Cursor<Vec<u8>>) -> Option<crate::core::heap::SourceLoc> {
    let file = get_str(r)?;
    let line = get_u32(r)?;
    let col = get_u32(r)?;
    Some(crate::core::heap::SourceLoc {
        file,
        pos: crate::error::Pos { line, col },
    })
}

fn get_str(r: &mut Cursor<Vec<u8>>) -> Option<String> {
    let n = get_u32(r)? as usize;
    String::from_utf8(take(r, n)?.to_vec()).ok()
}

/// Encode one section: the global bindings named by `syms`, plus their declared sigs.
/// Returns the section bytes and how many entries went in.
///
/// `all_sigs` asks for EVERY declared sig rather than just this section's. That is the
/// root/project section's shape (one unnamed section carrying the whole image). A NAMED
/// section carries the sigs of its own symbols: the stdlib image is one named section per
/// module, and a module restored without its sigs would come back bound but **unsigned**,
/// silently weakening `nest check`.
fn encode_section(
    heap: &mut Heap,
    section: &str,
    syms: &[Value],
    all_sigs: bool,
    ns_to_msg: &mut u64,
    ns_encode: &mut u64,
    tables_seen: &mut std::collections::HashMap<u64, String>,
    positions: &mut usize,
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
        // form and `to_message` refuses it. But `table` is the language's only sanctioned
        // mutable structure (ADR-026/107), so `(def *cache* (table))` must not forfeit the
        // project's image.
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
        let t0 = web_time::Instant::now();
        // A value with no portable form (a pid, a table, an open socket) raises: silently
        // dropping a binding would make the image a *different program* from the source, which
        // is far worse than failing loudly. The caller reports it and loads from source
        // instead. A *builtin* is the exception — `to_message_image` carries it by name (the
        // image restores bindings in the same, binary-keyed runtime), so a global holding a
        // primitive (e.g. `editor/frontend/*term-display*`, a map of `term-*` prims) no
        // longer forfeits the whole image.
        let msg = to_message_image(heap, v).map_err(|e| {
            LispError::type_err(format!(
                "%image-write: cannot image global '{}': {e}",
                value::symbol_name(sym)
            ))
        })?;
        *ns_to_msg += t0.elapsed().as_nanos() as u64;
        *positions += crate::process::message::positioned_lists(&msg);
        let t1 = web_time::Instant::now();
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
    // not bake never contributes one. "Its own" is a sig on one of the section's globals OR
    // one keyed in the section's namespace: a `(deftype …)` alias (ADR-326) is a declaration
    // with no global behind it, and matching on globals alone dropped every alias std
    // declared — a module restored from the image came back with its sigs but with every
    // `pane` in them an unknown type.
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
        let prefix = format!("{section}/");
        heap.declared_sigs_snapshot()
            .into_iter()
            .filter(|(sym, _)| mine.contains(sym) || value::symbol_name(*sym).starts_with(&prefix))
            .collect()
    };
    for (sym, tv) in sig_pairs {
        let Ok(msg) = to_message(heap, tv) else {
            continue; // a type expression with no portable form: skip, never fail
        };
        *positions += crate::process::message::positioned_lists(&msg);
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
    // Def sites, for every name in this section the runtime recorded one for — so a global
    // materialised from the image answers `def_site` exactly as one loaded from source.
    for nv in syms {
        let value::ValueRef::Sym(sym) = nv.unpack() else {
            continue;
        };
        if let Some(loc) = heap.def_site(sym) {
            entries.push(KIND_DEF_SITE);
            put_str(&mut entries, &value::symbol_name(sym));
            put_def_site(&mut entries, &loc);
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
pub(crate) fn image_write(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%image-write", arg(args, 0))?;
    let sections = seq_items(heap, arg(args, 1))?;
    let fingerprint = expect_string(heap, "%image-write", arg(args, 2))?;

    let (mut ns_to_msg, mut ns_encode) = (0u64, 0u64);

    // Header first, so section offsets can be absolute from the start of the file.
    let mut body: Vec<u8> = Vec::with_capacity(1 << 20);
    body.extend_from_slice(MAGIC);
    put_str(&mut body, &fingerprint);

    let mut dir: Vec<(String, u64, u32)> = Vec::new();
    let mut total: u32 = 0;
    // Every MACRO among the imaged names, for the v6 kind index (see `MAGIC`). Collected
    // from the live bindings as the sections are walked — the same fact `encode_section`
    // tags each entry with, gathered once so the footer can be read without the payload.
    let mut macro_names: Vec<String> = Vec::new();
    // Table handles seen anywhere in this write, so an aliased pair is caught across
    // sections and not just within one (see `encode_section`).
    let mut tables_seen: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    // The macro names the image now at `path` indexes, read once if a section is copied.
    let mut old_footer_macros: Option<Vec<String>> = None;
    for sv in sections {
        let pair = seq_items(heap, sv)?;
        if pair.len() != 2 {
            return Err(LispError::type_err(
                "%image-write: each section must be [name syms]",
            ));
        }
        let name = expect_string(heap, "%image-write", pair[0])?;
        // `[name [:copy offset length]]` — the section's bytes as the image now at `path` holds
        // them, copied verbatim (ADR-419's partial rebuild): a module whose section is still
        // right is not materialised and re-encoded to be written again. A section is
        // self-contained (its entry count, then its entries), so its bytes move unchanged; its
        // macro names, which the footer indexes, come from the old footer.
        if let Some((offset, length)) = copy_spec(heap, pair[1]) {
            let file = image_file(heap, &path).ok_or_else(|| {
                LispError::runtime(format!(
                    "%image-write: cannot open {path} to copy section {name}"
                ))
            })?;
            let bytes = read_at(&file, offset, length as usize).ok_or_else(|| {
                LispError::runtime(format!(
                    "%image-write: cannot read section {name} from {path}"
                ))
            })?;
            let old_macros = old_footer_macros.get_or_insert_with(|| footer_macro_names(&file));
            macro_names.extend(
                old_macros
                    .iter()
                    .filter(|m| m.rsplit_once('/').is_some_and(|(module, _)| module == name))
                    .cloned(),
            );
            total += bytes
                .get(..4)
                .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            dir.push((name, body.len() as u64, bytes.len() as u32));
            body.extend_from_slice(&bytes);
            continue;
        }
        let syms = seq_items(heap, pair[1])?;
        for nv in &syms {
            if let value::ValueRef::Sym(s) = nv.unpack()
                && let Some(Value::Macro(_)) = heap.env_get(heap.global(), s)
            {
                macro_names.push(value::symbol_name(s));
            }
        }
        // The unnamed root/project section takes every sig; a named one takes its own.
        let all_sigs = name.is_empty();
        let (bytes, n) = encode_section(
            heap,
            &name,
            &syms,
            all_sigs,
            &mut ns_to_msg,
            &mut ns_encode,
            &mut tables_seen,
            &mut 0,
        )?;
        dir.push((name, body.len() as u64, bytes.len() as u32));
        body.extend_from_slice(&bytes);
        total += n;
    }

    // Directory at the END, with a footer pointing at it: an image can then be *opened* by
    // reading its header and directory alone, and one section loaded by seeking to its
    // offset — neither reads the payload.
    let directory_offset = body.len() as u64;
    put_u32(&mut body, dir.len() as u32);
    for (name, offset, length) in &dir {
        put_str(&mut body, name);
        put_u64(&mut body, *offset);
        put_u32(&mut body, *length);
    }
    // The `defdyn` marks (which globals are dynamic vars). They live *after* the directory but
    // still before the footer, so they sit inside the `[directory_offset, footer)` span `%image-index`
    // reads whole — the image restores them when opened, no payload read. Written last (at
    // image-build time the whole runtime is loaded, so its dynamic set is complete).
    let dynamic_names = heap.dynamic_names();
    put_u32(&mut body, dynamic_names.len() as u32);
    for &name in &dynamic_names {
        put_str(&mut body, value::symbol_name_ref(name));
    }
    // v6: the macro names (ADR-335's kind index), same region, same reason — read at open,
    // never from the payload.
    put_u32(&mut body, macro_names.len() as u32);
    for name in &macro_names {
        put_str(&mut body, name);
    }
    // v7 (ADR-370): the signature index of the baked-in std — the output of
    // `check::std_signature_index`, the ONE scanner a process booted without an image also
    // runs on the same embedded sources, so what an imaged process reads from this footer is
    // byte for byte what an un-imaged one computes (the artifact matrix requires it). Kept
    // as text: no heap work at open, a form only when a check first asks.
    let sig_entries: Vec<(String, String, u32, u32)> =
        crate::types::check::std_signature_index(heap)
            .into_iter()
            .map(|e| (e.name, e.text, e.min, e.max))
            .collect();
    put_u32(&mut body, sig_entries.len() as u32);
    for (name, text, min, max) in &sig_entries {
        put_str(&mut body, name);
        put_str(&mut body, text);
        put_u32(&mut body, *min);
        put_u32(&mut body, *max);
    }
    put_u64(&mut body, directory_offset);

    let t_io = web_time::Instant::now();
    if path.starts_with(MEMORY_IMAGE_PREFIX) {
        provide_memory_image(&path, body);
        return Ok(Value::int(total as i64));
    }
    // ATOMIC: write a sibling temp file, then rename over the target. A plain
    // `fs::write` truncates in place, so a reader that indexes the image while
    // another process is rebuilding it sees a torn file — and images are now built
    // by `nest`, which a test suite or a build script can easily run several of at
    // once. `rename` within a directory is atomic on POSIX and replaces on Windows,
    // so a reader observes either the old complete image or the new one, never half.
    //
    // The temp name is unique per write (`temp_suffix`) so two concurrent builders do not
    // collide with each other either; last rename wins. Two builds of one tree are NOT
    // byte-identical, so a rename moves every section offset, and a process that indexed the
    // previous file must keep reading THAT file: the runtime holds the handle open
    // (KI-119/224).
    let tmp = format!("{path}.{}.tmp", temp_suffix());
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

// The image files a RUNTIME has indexed are **held open** (`Heap::image_files`) — one handle
// per path, replaced whenever that runtime indexes the same path again.
//
// This is what makes a section read agree with the directory it came from (KI-119). A
// directory is read once, at `%image-index`; the sections it names are read LATER, on each
// module's first `require`, seconds or minutes after — and in between the file at that path
// can be replaced (`nest` rebuilds a stale image, and two builds of the SAME tree are not
// byte-identical, so every rebuild moves every offset). Reading by path would apply the old
// offsets to the new file and decode some *other* module's bytes.
//
// An open descriptor pins the inode: the atomic rename the writer does leaves an old reader
// on the old, complete file for as long as it holds the handle. The handles are per runtime,
// not per process (KI-224): two runtimes in one process may index the same path across a
// rebuild, and a process-wide handle let the second's index re-point the first's directory
// at the new file.
//
// A path under `mem:` names no file: it is an image the HOST handed over as bytes
// (`provide_memory_image`) or one `%image-write` wrote there — how a runtime with no
// filesystem, the tab's wasm build (ADR-414), reads a stdlib image. The bytes are immutable
// once provided, so holding the `Arc` pins them as a descriptor pins an inode.

/// The prefix of an image path that names host-provided bytes rather than a file.
pub const MEMORY_IMAGE_PREFIX: &str = "mem:";

/// The images held in memory under `mem:` paths, process-wide: the host provides them
/// before any runtime boots, and every runtime that indexes one reads the same bytes.
static MEMORY_IMAGES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<[u8]>>>,
> = std::sync::LazyLock::new(Default::default);

/// Hold `bytes` as the image at `path` (`mem:…`), for `%image-index` and
/// `%image-load-section` to read as they read a file.
pub fn provide_memory_image(path: &str, bytes: Vec<u8>) {
    crate::core::sync::lock(&MEMORY_IMAGES).insert(path.to_string(), bytes.into());
}

/// The bytes held at `path` (`mem:…`), if any: what `%image-write` wrote there, for a host
/// to save.
pub fn memory_image(path: &str) -> Option<Vec<u8>> {
    crate::core::sync::lock(&MEMORY_IMAGES)
        .get(path)
        .map(|bytes| bytes.to_vec())
}

/// Whether an image exists at `path`: a file, or bytes held under a `mem:` path.
pub(crate) fn image_exists(path: &str) -> bool {
    if path.starts_with(MEMORY_IMAGE_PREFIX) {
        crate::core::sync::lock(&MEMORY_IMAGES).contains_key(path)
    } else {
        std::path::Path::new(path).is_file()
    }
}

/// An image a runtime reads sections from: a file held open, or bytes held in memory.
pub(crate) enum ImageFile {
    Disk(std::sync::Mutex<std::fs::File>),
    Memory(std::sync::Arc<[u8]>),
}

impl ImageFile {
    fn size(&self) -> Option<u64> {
        match self {
            ImageFile::Disk(file) => file
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .metadata()
                .ok()
                .map(|m| m.len()),
            ImageFile::Memory(bytes) => Some(bytes.len() as u64),
        }
    }
}

/// Open `path` and make it THE file this runtime reads `path`'s sections from, replacing any
/// earlier handle for the same path — an index deliberately adopts whatever is on disk now.
fn open_image(heap: &Heap, path: &str) -> Option<std::sync::Arc<ImageFile>> {
    let file = if path.starts_with(MEMORY_IMAGE_PREFIX) {
        ImageFile::Memory(crate::core::sync::lock(&MEMORY_IMAGES).get(path)?.clone())
    } else {
        ImageFile::Disk(std::sync::Mutex::new(std::fs::File::open(path).ok()?))
    };
    let file = std::sync::Arc::new(file);
    heap.image_files().insert(path.to_string(), file.clone());
    Some(file)
}

/// The held handle for `path`, opening (and holding) it if this runtime has not indexed it.
/// The miss path exists for a caller that got its directory from somewhere other than this
/// runtime's own `%image-index` — it is exactly as safe as a read by path, no more.
fn image_file(heap: &Heap, path: &str) -> Option<std::sync::Arc<ImageFile>> {
    // Take the hit out from under the lock BEFORE the miss path re-locks it in `open_image` —
    // `std::sync::Mutex` is not reentrant, and an `if let` on the guard would keep it alive
    // across that call.
    let held = heap.image_files().get(path).cloned();
    match held {
        Some(file) => Some(file),
        None => open_image(heap, path),
    }
}

/// `[:copy offset length]` → `(offset, length)`, or `None` for anything else (a section's
/// name list).
fn copy_spec(heap: &Heap, v: Value) -> Option<(u64, u32)> {
    let items = heap.list_or_vector_items(v)?;
    match items.as_slice() {
        [Value::Keyword(k), off, len] if value::symbol_is(*k, "copy") => {
            Some((off.as_int()? as u64, len.as_int()? as u32))
        }
        _ => None,
    }
}

/// The macro names an image's footer indexes (v6) — the list after the directory and the
/// `defdyn` marks. Empty when the footer cannot be read: a copied section then simply goes
/// unindexed as a macro holder, which costs its callers an eager load, never a wrong answer.
fn footer_macro_names(file: &ImageFile) -> Vec<String> {
    let read = || -> Option<Vec<String>> {
        let size = file.size()?;
        let foot = read_at(file, size.checked_sub(8)?, 8)?;
        let directory_offset = u64::from_le_bytes(foot.try_into().ok()?);
        let bytes = read_at(
            file,
            directory_offset,
            (size - 8 - directory_offset) as usize,
        )?;
        let mut r = Cursor::new(bytes);
        let sections = get_u32(&mut r)?;
        for _ in 0..sections {
            get_str(&mut r)?;
            get_u64(&mut r)?;
            get_u32(&mut r)?;
        }
        let dynamics = get_u32(&mut r)?;
        for _ in 0..dynamics {
            get_str(&mut r)?;
        }
        let macros = get_u32(&mut r)?;
        (0..macros).map(|_| get_str(&mut r)).collect()
    };
    read().unwrap_or_default()
}

/// Read `len` bytes at `off` from an open image handle without reading the rest of the file.
/// Seek + read under the handle's lock, so two concurrent section loads cannot interleave
/// their seeks.
fn read_at(file: &ImageFile, off: u64, len: usize) -> Option<Vec<u8>> {
    match file {
        ImageFile::Disk(file) => {
            let mut f = file.lock().unwrap_or_else(|e| e.into_inner());
            f.seek(SeekFrom::Start(off)).ok()?;
            let mut buf = vec![0u8; len];
            f.read_exact(&mut buf).ok()?;
            Some(buf)
        }
        ImageFile::Memory(bytes) => {
            let start = usize::try_from(off).ok()?;
            bytes
                .get(start..start.checked_add(len)?)
                .map(<[u8]>::to_vec)
        }
    }
}

/// `(%image-exists? path)` — see `image_exists`.
pub(crate) fn image_exists_prim(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%image-exists?", arg(args, 0))?;
    Ok(Value::Bool(image_exists(&path)))
}

/// `(%boot-source)` — how this process's prelude arrived, as a keyword.
///
/// A keyword rather than a string because it is a closed set the caller matches on, and
/// because every other "which state is this artifact in" answer in this area
/// (`stdimage/status`'s `:live`/`:stale`/`:absent`) already reads that way.
pub(crate) fn boot_source_prim(_args: &[Value], _: EnvId, _heap: &mut Heap) -> LispResult {
    Ok(Value::Keyword(crate::core::value::intern(
        crate::boot::boot_source(),
    )))
}

/// Decode the v7 signature index from the footer bytes `image_index` set aside — `(name,
/// type text, min arity, max arity)` per function; a truncated tail ends the list.
pub(crate) fn decode_sig_index(bytes: Vec<u8>) -> Vec<(String, String, u32, u32)> {
    // An entry is at least two string lengths and two arities (16 bytes): the bytes bound
    // the reservation, never the untrusted count alone.
    let entry_bound = bytes.len() / 16;
    let mut reader = Cursor::new(bytes);
    let mut out: Vec<(String, String, u32, u32)> = Vec::new();
    let Some(count) = get_u32(&mut reader) else {
        return out;
    };
    out.reserve((count as usize).min(entry_bound));
    for _ in 0..count {
        match (
            get_str(&mut reader),
            get_str(&mut reader),
            get_u32(&mut reader),
            get_u32(&mut reader),
        ) {
            (Some(name), Some(text), Some(min), Some(max)) => out.push((name, text, min, max)),
            _ => break,
        }
    }
    out
}

/// `(%image-index path fingerprint)` — open the image at `path` if it exists and its stamp
/// equals `fingerprint`, returning its section directory as a map `{name → [offset len]}`.
/// **nil** for any miss: absent, unreadable, wrong format version, or stale.
///
/// Reads the header and the directory only — never the payload — so opening a large image
/// costs a few kilobytes of I/O.
pub(crate) fn image_index(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%image-index", arg(args, 0))?;
    let want = expect_string(heap, "%image-index", arg(args, 1))?;

    // Open ONCE and hold the handle (see `open_image`): the header, the footer, the
    // directory and — later, per module — every section are read from this same file, so a
    // rebuild that replaces the path between now and a section load cannot move the bytes
    // the directory points at out from under us.
    let Some(file) = open_image(heap, &path) else {
        return Ok(Value::Nil);
    };
    let Some(size) = file.size() else {
        return Ok(Value::Nil);
    };
    if size < (MAGIC.len() + 8) as u64 {
        return Ok(Value::Nil);
    }
    // Header: magic + fingerprint. Read a bounded prefix — the fingerprint is the only
    // variable-length part and the caller built it, so its length is known to be sane.
    let head_len = (MAGIC.len() + 4 + want.len()).min(size as usize);
    let Some(head) = read_at(&file, 0, head_len) else {
        return Ok(Value::Nil);
    };
    if !head.starts_with(MAGIC) {
        return Ok(Value::Nil);
    }
    let mut header = Cursor::new(head);
    header.set_position(MAGIC.len() as u64);
    let Some(got) = get_str(&mut header) else {
        return Ok(Value::Nil);
    };
    if got != want {
        return Ok(Value::Nil);
    }
    // Footer: the directory's offset.
    let Some(foot) = read_at(&file, size - 8, 8) else {
        return Ok(Value::Nil);
    };
    let directory_offset = u64::from_le_bytes(foot.try_into().ok().unwrap_or([0; 8]));
    if directory_offset >= size - 8 {
        return Ok(Value::Nil);
    }
    let Some(dir_bytes) = read_at(
        &file,
        directory_offset,
        (size - 8 - directory_offset) as usize,
    ) else {
        return Ok(Value::Nil);
    };
    // A directory entry is at least a string length + offset + length (16 bytes), so the
    // file's own size bounds the count — never reserve from the untrusted count alone.
    let entry_bound = dir_bytes.len() / 16;
    let mut directory = Cursor::new(dir_bytes);
    let Some(section_count) = get_u32(&mut directory) else {
        return Ok(Value::Nil);
    };
    let capacity = (section_count as usize).min(entry_bound);
    let mut pairs: Vec<(Value, Value)> = Vec::with_capacity(capacity);
    let mut section_names: Vec<String> = Vec::with_capacity(capacity);
    for _ in 0..section_count {
        let Some(name) = get_str(&mut directory) else {
            return Ok(Value::Nil);
        };
        section_names.push(name.clone());
        let (Some(offset), Some(length)) = (get_u64(&mut directory), get_u32(&mut directory))
        else {
            return Ok(Value::Nil);
        };
        let key = heap.alloc_string(&name);
        let span = heap.alloc_vector2(Value::int(offset as i64), Value::int(length as i64));
        pairs.push((key, span));
    }
    // Re-establish the `defdyn` dynamic-var marks recorded after the directory (v5): an imaged
    // start restores a dynamic global's value but skips the module load that ran the `defdyn`,
    // so `binding` on it would fail. Marking is idempotent + monotonic, so a spurious entry is
    // harmless (and a stale image is fingerprint-rejected before we get here). Done in the same
    // `%image-index` pass every install runs, so the marks are back before any section loads.
    if let Some(dynamic_count) = get_u32(&mut directory) {
        for _ in 0..dynamic_count {
            match get_str(&mut directory) {
                Some(name) => heap.mark_dynamic(value::intern(&name)),
                None => break,
            }
        }
    }
    // The kind index (v6, ADR-335): which of the imaged modules' names are macros. With it
    // registered, a qualified call head into one of these modules can wait for its first
    // call unless it names a macro — see `derive::require_qualified_head`. Registered from
    // the same open pass as the dynamic marks, so it is in place before any form that
    // references an imaged module expands.
    let mut macro_names: Vec<String> = Vec::new();
    if let Some(macro_count) = get_u32(&mut directory) {
        for _ in 0..macro_count {
            match get_str(&mut directory) {
                Some(name) => macro_names.push(name),
                None => break,
            }
        }
    }
    crate::eval::derive::register_image_kinds(heap, &section_names, &macro_names);
    // v7 (ADR-370): the signature index — handed over as the footer's remaining BYTES and
    // decoded by the first check that asks, so a run that never checks never pays for it.
    let position = directory.position() as usize;
    let raw = directory.into_inner().split_off(position);
    crate::eval::derive::register_image_sigs_raw(raw);
    Ok(heap.map_from_pairs(pairs))
}

/// Bind one imaged entry to its global. **The single definition site**, called from both
/// passes of `image_load_section` — the in-order pass and the deferred-entry-point pass
/// (KI-72) — so the `match` over the kind tag cannot lose an arm in one copy.
fn define_image_entry(
    heap: &mut Heap,
    global: EnvId,
    kind: u8,
    sym: value::Symbol,
    v: Value,
    reserve: bool,
) -> Result<(), LispError> {
    // `env_define` clears the private mark — right for a real `def`, wrong for
    // materialising, which is not a redefinition. A section's privacy entries are written
    // last, so a value entry deferred to the second pass lands AFTER the `KIND_PRIVATE`
    // entry marked it. Save and restore around the define, as `registry_update` does, so
    // the order of the two entries does not matter.
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

/// `(%image-load-section path offset len &optional reserve?)` — materialise one section's
/// entries: define its globals (rebuilding macros as macros) and register its declared sigs.
/// Returns how many entries were defined, or **nil** if the bytes could not be read or decoded.
///
/// Seeks straight to the section, so loading one module never touches the rest of the image.
///
/// `reserve?` reproduces the source path's ADR-166 rule: while an embedded module loads, its
/// FUNCTION-valued definitions join the reserved set (so a program cannot redefine
/// `set/union`) while its data globals — its own registries — stay rebindable. Materialising
/// evaluates no `def`, so the caller passes it, because only the caller knows whether this
/// section is an embedded module (reserved) or a project's own (not).
pub(crate) fn image_load_section(args: &[Value], _: EnvId, heap: &mut Heap) -> LispResult {
    let path = expect_string(heap, "%image-load-section", arg(args, 0))?;
    let off = match arg(args, 1) {
        Value::Int(n) if n >= 0 => n as u64,
        v => return Err(LispError::wrong_type(heap, "%image-load-section", "int", v)),
    };
    let len = match arg(args, 2) {
        Value::Int(n) if n >= 0 => n as usize,
        v => return Err(LispError::wrong_type(heap, "%image-load-section", "int", v)),
    };
    let reserve = crate::eval::truthy(arg(args, 3));
    // From the handle `%image-index` opened for this path — never a fresh open by path, which
    // would read whatever a concurrent rebuild has since put there (KI-119/224).
    let Some(file) = image_file(heap, &path) else {
        return Ok(Value::Nil);
    };
    let Some(bytes) = read_at(&file, off, len) else {
        return Ok(Value::Nil);
    };
    let mut r = Cursor::new(bytes);
    let Some(count) = get_u32(&mut r) else {
        return Ok(Value::Nil);
    };
    // The module whose section this is, when the caller named it (the std loader does).
    // Its forms' positions were read from that module's source, so the file must be stamped
    // WITH them: `from_message` re-stamps each rebuilt list through `set_form_pos`, which
    // takes the file from `current_file` — during a materialise that is whatever the process
    // happens to be loading, so an imaged module's code came back carrying a line from one
    // file under another file's name. That is not a cosmetic mismatch: it is how a contract
    // shim's line 135 was reported against the user's own script (KI-184), and the same
    // trap `set_form_pos_in_file` documents for the expander. Restored below.
    let section_file = match arg(args, 4) {
        Value::Nil => None,
        v => expect_string(heap, "%image-load-section", v)
            .ok()
            .and_then(|key| crate::builtins::modules::embedded_module_path(&key)),
    };
    // Restored on EVERY exit — the decode-failure nils and an error from a define alike —
    // because every exit of the body passes back through here.
    let prior_file = heap.set_current_file(section_file.map(str::to_string));
    let loaded = load_section_entries(heap, &mut r, count, reserve);
    heap.set_current_file(prior_file);
    Ok(loaded?.map_or(Value::Nil, Value::int))
}

/// The body of [`image_load_section`]: define a section's `count` entries from `r`.
/// `Ok(None)` when the bytes do not decode.
fn load_section_entries(
    heap: &mut Heap,
    r: &mut Cursor<Vec<u8>>,
    count: u32,
    reserve: bool,
) -> Result<Option<i64>, LispError> {
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
        let (Some(kind), Some(name)) = (get_u8(r), get_str(r)) else {
            return Ok(None);
        };
        // A privacy entry carries a name and no value — branch before decoding one.
        if kind == KIND_PRIVATE {
            heap.mark_private(value::intern(&name));
            done += 1;
            continue;
        }
        // So does a def-site entry: file, line, column.
        if kind == KIND_DEF_SITE {
            let Some(loc) = get_def_site(r) else {
                return Ok(None);
            };
            heap.set_def_site(value::intern(&name), loc);
            done += 1;
            continue;
        }
        let t0 = web_time::Instant::now();
        let Ok(msg) = decode_msg(r) else {
            return Ok(None);
        };
        ns_decode += t0.elapsed().as_nanos() as u64;
        let t1 = web_time::Instant::now();
        let v = from_message(heap, &msg);
        ns_from_msg += t1.elapsed().as_nanos() as u64;
        let sym = value::intern(&name);
        let t2 = web_time::Instant::now();
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
    // safepoint and reclaiming nothing — otherwise about half of a whole-image restore.
    heap.rt_gc_rebaseline_all_live();
    if trace() {
        eprintln!(
            "[image-section] {done} entries — decode {} ms, from_message {} ms, define {} ms",
            ns_decode / 1_000_000,
            ns_from_msg / 1_000_000,
            ns_define / 1_000_000
        );
    }
    Ok(Some(done))
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
        let tag = get_u8(r)?;
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
            put_def_site(out, loc);
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
/// whole load, so the source boot takes over rather than a partial fact set being installed.
fn get_fact(r: &mut Cursor<Vec<u8>>) -> Option<crate::core::heap::Fact> {
    use crate::core::heap::Fact;
    let tag = get_u8(r)?;
    let sym = value::intern(&get_str(r)?);
    Some(match tag {
        FACT_PRIVATE => Fact::Private(sym),
        FACT_META => Fact::Meta(sym, get_meta(r)?),
        FACT_DEF_SITE => Fact::DefSite(sym, get_def_site(r)?),
        FACT_REGISTRY_NAME => Fact::RegistryName(sym),
        FACT_DYNAMIC => Fact::Dynamic(sym),
        _ => return None,
    })
}

/// Write the prelude image for `fingerprint` to `path`: every non-native binding in `root`,
/// plus declared sigs, privacy and `meta`. Best-effort — an error simply means the next boot
/// takes the source boot, exactly as a missing file does.
pub(crate) fn write_prelude_image(
    heap: &mut Heap,
    root: EnvId,
    skip: &std::collections::HashSet<value::Symbol>,
    path: &std::path::Path,
    fingerprint: &str,
    gensym_floor: u64,
) -> Result<(), LispError> {
    let out = prelude_image_bytes(heap, root, skip, fingerprint, gensym_floor)?;
    let dir = path
        .parent()
        .ok_or_else(|| LispError::runtime("prelude image: no parent dir"))?;
    std::fs::create_dir_all(dir).map_err(|e| LispError::runtime(format!("prelude image: {e}")))?;
    // Temp + rename, so a concurrently booting process never reads a torn file — the same
    // write-then-rename discipline, and it matters here (many nextest processes boot at once).
    let tmp = path.with_extension(format!("tmp.{}", temp_suffix()));
    std::fs::write(&tmp, &out).map_err(|e| LispError::runtime(format!("prelude image: {e}")))?;
    std::fs::rename(&tmp, path).map_err(|e| LispError::runtime(format!("prelude image: {e}")))?;
    Ok(())
}

/// The prelude image as bytes: what `write_prelude_image` writes, for a host with no file
/// to write it to (a browser tab ships it beside the program, `boot::provide_prelude_image`).
pub(crate) fn prelude_image_bytes(
    heap: &mut Heap,
    root: EnvId,
    skip: &std::collections::HashSet<value::Symbol>,
    fingerprint: &str,
    gensym_floor: u64,
) -> Result<Vec<u8>, LispError> {
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

    let mut positions = 0usize;
    let (mut body, _count) = encode_section(
        heap,
        "",
        &syms,
        true,
        &mut 0u64,
        &mut 0u64,
        &mut std::collections::HashMap::new(),
        &mut positions,
    )?;

    // The SIDE FACTS — everything the prelude's evaluation RECORDED about a name rather
    // than bound to it: privacy, `meta`, def sites, registry names, `defdyn` marks.
    // `Heap::side_facts` enumerates them through an exhaustive match and `put_fact` encodes
    // them through another, so a new fact kind is carried by construction and forgetting
    // its encoding is a compile error (ADR-320).
    let facts = heap.side_facts();
    put_u32(&mut body, facts.len() as u32);
    for fact in &facts {
        put_fact(&mut body, fact);
    }

    let mut out = Vec::with_capacity(body.len() + 64);
    write_prelude_header(&mut out, fingerprint, gensym_floor, positions as u64);
    out.extend_from_slice(&body);
    Ok(out)
}

/// A temp-file suffix unique to one write: the pid separates processes, the counter separates
/// runtimes (and threads) inside one process — a pid alone let two runtimes building at once
/// write the same temp file and rename each other's half-written bytes into place (KI-224).
fn temp_suffix() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let write = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{write}", std::process::id())
}

/// v3: the header carries the gensym floor.
/// v4 (ADR-383): the closure record carries its authoring module (the wire's v8 field).
/// v5: the header carries the number of source positions the load re-stamps (KI-302).
const PRELUDE_MAGIC: &[u8] = b"brood-prelude-image-v5\n";

/// The image header: magic, the build fingerprint, the gensym counter the writing boot
/// reached, and how many source positions the load re-stamps. A reader must start its own
/// `gensym` above that counter, or a fresh name could collide with one baked into the
/// image's closures. The position count sizes the position table before the load, which
/// otherwise grows it by doubling: one more doubling, when the prelude crossed 7 168
/// positioned forms, was +2.5M instructions on every start (KI-302).
fn write_prelude_header(out: &mut Vec<u8>, fingerprint: &str, gensym_floor: u64, positions: u64) {
    out.extend_from_slice(PRELUDE_MAGIC);
    put_str(out, fingerprint);
    put_u64(out, gensym_floor);
    put_u64(out, positions);
}

/// Parse [`write_prelude_header`]'s output into the gensym floor and the position count,
/// positioning `r` at the first entry. `None` on a wrong magic (another version's file) or a
/// fingerprint that is not this binary's — either means "not our image", and the caller
/// takes the source boot.
fn read_prelude_header(r: &mut Cursor<Vec<u8>>, fingerprint: &str) -> Option<(u64, u64)> {
    if !r.get_ref().starts_with(PRELUDE_MAGIC) {
        return None;
    }
    r.set_position(PRELUDE_MAGIC.len() as u64);
    if get_str(r)? != fingerprint {
        return None;
    }
    Some((get_u64(r)?, get_u64(r)?))
}

/// Restore a prelude image's bytes into `root` — read from the cache file, or handed over by
/// a host with no cache directory (`boot::provide_prelude_image`). `None` for any miss —
/// stale, truncated, or a value that will not decode — and the caller falls back to the
/// source boot. Returns the number of entries defined and the gensym floor.
pub(crate) fn load_prelude_image_bytes(
    heap: &mut Heap,
    root: EnvId,
    bytes: Vec<u8>,
    fingerprint: &str,
) -> Option<(usize, u64)> {
    let mut r = Cursor::new(bytes);
    let (gensym_floor, positions) = read_prelude_header(&mut r, fingerprint)?;
    heap.reserve_form_pos(positions as usize);
    let count = get_u32(&mut r)?;
    let mut done = 0usize;
    for _ in 0..count {
        let kind = get_u8(&mut r)?;
        let name = get_str(&mut r)?;
        if kind == KIND_PRIVATE {
            heap.mark_private(value::intern(&name));
            done += 1;
            continue;
        }
        if kind == KIND_DEF_SITE {
            heap.set_def_site(value::intern(&name), get_def_site(&mut r)?);
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
    // source boot takes over, and a half-restored fact set is never observable.
    let fact_count = get_u32(&mut r)?;
    for _ in 0..fact_count {
        let fact = get_fact(&mut r)?;
        heap.replay_fact(&fact);
    }
    Some((done, gensym_floor))
}

#[cfg(test)]
mod prelude_header_tests {
    use super::*;

    #[test]
    fn the_gensym_floor_survives_the_header_round_trip() {
        let mut out = Vec::new();
        write_prelude_header(&mut out, "build-abc", 0xDEAD_BEEF_0000_0042, 7324);
        let mut r = Cursor::new(out);
        assert_eq!(
            read_prelude_header(&mut r, "build-abc"),
            Some((0xDEAD_BEEF_0000_0042, 7324)),
            "the floor written must be the floor read — a boot that loses it can mint a \
             gensym that collides with a name baked into the image"
        );
    }

    #[test]
    fn another_builds_image_is_not_ours() {
        let mut out = Vec::new();
        write_prelude_header(&mut out, "build-abc", 7, 0);
        let mut r = Cursor::new(out);
        assert_eq!(read_prelude_header(&mut r, "build-xyz"), None);
    }
}

#[cfg(test)]
mod per_runtime_handle_tests {
    /// KI-224: two runtimes in ONE process index the same image path, with a rebuild
    /// between them. The first runtime's directory must keep reading the file it indexed —
    /// with a process-wide handle the second index re-pointed it at the rebuilt file, and
    /// the section read decoded the wrong bytes (`cargo test` threads and the ASan job hit
    /// it as a module that loaded without its declared signatures).
    #[test]
    fn a_rebuild_indexed_by_another_runtime_does_not_move_this_runtimes_sections() {
        let path = std::env::temp_dir().join(format!(
            "brood-per-runtime-image-{}.bin",
            std::process::id()
        ));
        let path = path.to_string_lossy().into_owned();
        let mut first = crate::Interp::new();
        let mut second = crate::Interp::new();
        let section = first
            .eval_str(&format!(
                "(do (def pinned 7)
                     (%image-write {path:?} [[\"\" ['pinned]]] \"fp\")
                     (get (%image-index {path:?} \"fp\") \"\"))"
            ))
            .expect("write and index the first image");
        let section = first.print(section);
        // The rebuild lands, and ANOTHER runtime indexes it.
        second
            .eval_str(&format!(
                "(do (%image-write {path:?} [] \"fp\") (%image-index {path:?} \"fp\"))"
            ))
            .expect("rebuild and index in the second runtime");
        let restored = first
            .eval_str(&format!(
                "(let (section '{section})
                   (def pinned :clobbered)
                   (%image-load-section {path:?} (first section) (second section))
                   pinned)"
            ))
            .expect("load the first runtime's section");
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            first.print(restored),
            "7",
            "the first runtime's section was read from the file the second runtime indexed"
        );
    }
}

#[cfg(test)]
mod memory_image_tests {
    /// A `mem:` image is the whole round trip of a file one — written, found, indexed by its
    /// stamp, a section restored — with no file anywhere: what a runtime without a
    /// filesystem (the tab, ADR-414) boots its std modules from.
    #[test]
    fn an_image_written_to_memory_restores_like_a_file() {
        let path = format!("mem:test-image-{}", std::process::id());
        let mut interp = crate::Interp::new();
        let restored = interp
            .eval_str(&format!(
                "(do (def held-in-memory 42)
                     (%image-write {path:?} [[\"\" ['held-in-memory]]] \"fp\")
                     (def held-in-memory :clobbered)
                     (let (section (get (%image-index {path:?} \"fp\") \"\"))
                       (%image-load-section {path:?} (first section) (second section))
                       [(%image-exists? {path:?}) (%image-index {path:?} \"stale\") held-in-memory]))"
            ))
            .expect("write, index and restore an image held in memory");
        assert_eq!(interp.print(restored), "[true nil 42]");
        assert!(
            super::memory_image(&path).is_some(),
            "the host can take the image back"
        );
        assert!(!std::path::Path::new(&path).exists(), "no file was written");
    }

    /// Bytes a host provides are the image: the stamp is checked as for a file, and a `mem:`
    /// path nobody provided is simply absent.
    #[test]
    fn a_provided_image_is_read_and_an_unprovided_one_is_absent() {
        let source = format!("mem:test-source-{}", std::process::id());
        let mut interp = crate::Interp::new();
        interp
            .eval_str(&format!(
                "(do (def provided 5) (%image-write {source:?} [[\"\" ['provided]]] \"fp\"))"
            ))
            .expect("write the image to hand over");
        let provided = format!("mem:test-provided-{}", std::process::id());
        super::provide_memory_image(&provided, super::memory_image(&source).expect("written"));
        let answer = interp
            .eval_str(&format!(
                "[(nil? (%image-index {provided:?} \"fp\"))
                  (%image-exists? \"mem:test-never-provided\")
                  (%image-index \"mem:test-never-provided\" \"fp\")]"
            ))
            .expect("index the provided image");
        assert_eq!(interp.print(answer), "[false false nil]");
    }
}
