//! Phase-2 incremental-check dependency capture (ADR-119).
//!
//! While `check_file` runs under [`begin_record`], every observation of GLOBAL
//! state — a global binding, a declared `(sig …)`, a known-namespace prefix, a
//! module's exports, the protocol table — is recorded through the `obs_*`
//! wrappers here (the ONLY way the checker is allowed to read global state, so the
//! recorded set is complete by construction). [`take_dep_keys`] turns the record
//! into a serializable per-file "dep-keys" value; [`fingerprint`] re-observes those
//! keys against the *current* image and returns a compact stamp. A file's cached
//! warnings are reusable iff its text (mtime) AND its dep-fingerprint are unchanged
//! — so a change to any global it observed (including transitively, since inference
//! walks a callee's body and thereby records the callee's own references) flips the
//! stamp and forces a re-check.
//!
//! Soundness of a key's fingerprint (what could change its contribution to the
//! file's warnings, and how it's captured):
//!   - a user global → its DEFINING FILE's mtime (a body/arity/sig change ⟺ that
//!     file changed) PLUS its declared-sig value (which may live in another file),
//!   - a prelude/builtin global → its kind (stable within a build; the whole cache
//!     is stamped with `(system/build-id)`, so a rebuild drops it),
//!   - an unbound name → "U" (so it invalidates the day something defines it),
//!   - a known-ns prefix → whether it is currently known,
//!   - a `:use`d module → its current export SET,
//!   - the protocol table → a structural hash of `*protocols*`.
//! The def-site-mtime assumption (every mutable user global has a def-site) is
//! verified by the differential battery, not by trust.

use crate::core::heap::Heap;
use crate::core::registries as reg;
use crate::core::value::{self, Symbol, Value};

// The recorder itself lives on the **heap** (`Heap::check_dep_rec` +
// `rec_check_dep_*`), so it's per-process and safe under parallel dep-capture and
// green-process migration — see the field's doc. This module only wraps the
// checker's global reads to feed it, and turns the record into keys + a fingerprint.

/// Start recording global observations on `heap`. Pair with [`take_dep_keys`].
pub(super) fn begin_record(heap: &Heap) {
    heap.begin_check_dep_record();
}

// ── observation wrappers — the ONLY sanctioned reads of global state in check/ ──

/// Record + read a global binding (`heap.env_get(global, sym)`).
pub(super) fn obs_global(heap: &Heap, sym: Symbol) -> Option<Value> {
    heap.rec_check_dep_sym(sym);
    if let Some(v) = heap.env_get(heap.global(), sym) {
        return Some(v);
    }
    // Mirror eval's `compile_ns` rooting: a BARE name unbound at root retries under the
    // namespace currently being compiled into (`compile_ns/name`), so the checker
    // resolves an interactive / in-`%in-ns` reference exactly as eval does — e.g. a bare
    // project fn at the prompt after `nest repl` entered the project's `:main` namespace
    // (ADR-070). Sound: this only ever FINDS a binding eval would also reach (eval roots
    // the same way), so it suppresses a false "unbound" and can never mask a real one. A
    // name that already carries a `/` is qualified — don't double-root it.
    let ns = heap.compile_ns()?;
    let name = value::symbol_name(sym);
    if name.contains('/') {
        return None;
    }
    let rooted = value::intern(&format!("{}/{}", value::symbol_name(ns), name));
    heap.rec_check_dep_sym(rooted);
    heap.env_get(heap.global(), rooted)
}

/// Record + read the FUNCTION a global names, for a reading of its body or its arms: the
/// original closure when a contract shim stands in its place (ADR-381/383), else the
/// global itself. With contracts armed a `sig`'d global holds the shim, whose body only
/// checks and forwards — inferring from it read every such function's result as `any`,
/// and a narrowing of that (`(not vector)` after a `vector?` test) then failed a precise
/// demand that the unarmed check, inferring from the real body, met (KI-384). The original
/// is read from the shim itself (`shim_original`, the closure its frame captures), so a root
/// name, which has no uncontracted alias, and a `sig!`-forced one, whose alias IS the shim,
/// are read through too.
pub(super) fn obs_function(heap: &Heap, sym: Symbol) -> Option<Value> {
    let global = obs_global(heap, sym)?;
    Some(crate::builtins::contracts::shim_original(heap, global).unwrap_or(global))
}

/// Record + read a global's declared `(sig …)` type-expression value — the type-form
/// alone, unwrapped from the properties a `(sig name T :pure …)` stores beside it
/// (ADR-351; `None` for a name that declared properties and no type).
pub(super) fn obs_declared_sig_value(heap: &Heap, sym: Symbol) -> Option<Value> {
    heap.rec_check_dep_sym(sym);
    crate::builtins::modules::sig_type_of(heap, heap.declared_sig_value(sym))
}

/// Record + read a global's declared signature PROPERTIES (`:pure`, `:total`, … —
/// ADR-351), empty when none were declared.
pub(super) fn obs_declared_sig_props(heap: &Heap, sym: Symbol) -> Vec<Symbol> {
    heap.rec_check_dep_sym(sym);
    crate::builtins::modules::sig_props_of(heap, heap.declared_sig_value(sym))
}

/// Record + read a module's public exports (for `(:use m)` resolution).
pub(super) fn obs_module_exports(heap: &Heap, prefix: &str) -> Vec<(Symbol, Symbol)> {
    heap.rec_check_dep_exports(prefix);
    heap.module_public_exports(prefix)
}

/// Record a known-namespace query (the caller computes the answer from its cached set).
pub(super) fn obs_known_ns(heap: &Heap, prefix: &str) {
    heap.rec_check_dep_ns(prefix);
}

/// Record + read one of the dispatch registries the protocol passes consult (`*impls*`,
/// `*abilities*`, `*sealed*`, `*methods*`, …). Recorded as a dependency on the registry's
/// global, whose fact (see [`registry_fact`]) is the registry's PROJECT content: an
/// `impl` added in another file changes the verdict of a file that never mentions it,
/// and no def site or mtime of the checked file can see that.
pub(super) fn obs_registry(heap: &Heap, name: &str) -> Option<Value> {
    let sym = value::intern(name);
    heap.rec_check_dep_sym(sym);
    heap.env_get(heap.global(), sym)
}

/// Record that the `*protocols*` table was consulted.
pub(super) fn obs_protocols(heap: &Heap) {
    heap.rec_check_dep_protocols();
}

// ── dep-keys value + fingerprint ──────────────────────────────────────────────

const K_SYMS: &str = "syms";
const K_KNS: &str = "kns";
const K_EXP: &str = "exp";
const K_PROTO: &str = "proto";

/// Take the recorded observations and build the serializable dep-keys value
/// `{:syms [names] :kns [prefixes] :exp [prefixes] :proto bool}` (sorted for a
/// stable on-disk form). Empty record → empty lists.
pub(super) fn take_dep_keys(heap: &mut Heap) -> Value {
    let dep = heap.take_check_dep_record().unwrap_or_default();
    // Exclude a file's OWN def-name from the dep-keys ONLY when it's fully
    // self-contained — defined here with no external `(sig …)`. Then its every fact
    // is this file's (covered by the file's own mtime), and dropping it avoids the
    // manifest bloat of a def-heavy file recording ~all its own names. But a
    // def-target whose sig lives in ANOTHER file (ADR-110 value-position check) is a
    // real cross-file dependency — keep it, or an edit to that sig goes unseen
    // (regression test: cross_module_value_sig_dependency_is_captured…).
    let mut syms: Vec<String> = dep
        .syms
        .iter()
        .filter(|&&s| !(dep.own.contains(&s) && heap.declared_sig_value(s).is_none()))
        .map(|&s| value::symbol_name(s).to_string())
        .collect();
    syms.sort_unstable();
    let mut known_namespaces: Vec<String> = dep.known_ns.into_iter().collect();
    known_namespaces.sort_unstable();
    let mut exports: Vec<String> = dep.exports.into_iter().collect();
    exports.sort_unstable();

    let syms_list = string_list(heap, &syms);
    let known_namespaces_list = string_list(heap, &known_namespaces);
    let exports_list = string_list(heap, &exports);
    heap.map_from_pairs(vec![
        (Value::keyword(value::intern(K_SYMS)), syms_list),
        (Value::keyword(value::intern(K_KNS)), known_namespaces_list),
        (Value::keyword(value::intern(K_EXP)), exports_list),
        (
            Value::keyword(value::intern(K_PROTO)),
            Value::Bool(dep.protocols),
        ),
    ])
}

/// A Brood list of `strings`.
fn string_list(heap: &mut Heap, strings: &[String]) -> Value {
    let items: Vec<Value> = strings.iter().map(|s| heap.alloc_string(s)).collect();
    heap.list(items)
}

/// Read a `:key`'d list-of-strings field from the dep-keys map.
fn field_strings(heap: &Heap, map_id: crate::core::value::MapId, key: &str) -> Vec<String> {
    match heap.map_get(map_id, Value::keyword(value::intern(key))) {
        Some(v) => list_of_strings(heap, v),
        None => Vec::new(),
    }
}

/// Collect a Brood list/vector of strings into a Rust `Vec<String>`.
fn list_of_strings(heap: &Heap, v: Value) -> Vec<String> {
    let mut out = Vec::new();
    for item in heap.seq_items(v).unwrap_or_default() {
        if let Value::Str(id) = item {
            out.push(heap.string(id).to_string());
        }
    }
    out
}

/// Last-modified stamp for `path` (epoch-millis), or a sentinel if it's missing —
/// so a deleted/created file always flips the fingerprint.
fn mtime_stamp(path: &str) -> String {
    match std::fs::metadata(path).and_then(|m| m.modified()) {
        Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_millis().to_string(),
            Err(_) => "pre-epoch".to_string(),
        },
        Err(_) => "MISSING".to_string(),
    }
}

/// The fingerprint contribution of one referenced global: its defining file's
/// mtime (user global) or kind (prelude/builtin) or "U" (unbound), plus its
/// declared-sig value hash (which can change independently, in another file).
fn fact_of_sym(heap: &Heap, sym: Symbol) -> String {
    let base = match heap.def_site(sym) {
        // The DEFINITION's content where it can be read (ADR-485), the defining file's
        // mtime where it cannot.
        Some(loc) => match definition_digest(heap, sym) {
            Some(digest) => format!("D{}#{digest:016x}", loc.file),
            None => format!("D{}@{}", loc.file, mtime_stamp(&loc.file)),
        },
        None => match heap.env_get(heap.global(), sym) {
            Some(Value::Native(_)) => "N".to_string(),
            Some(Value::Fn(_)) => "F".to_string(),
            Some(Value::Macro(_)) => "M".to_string(),
            Some(_) => "V".to_string(),
            None => "U".to_string(),
        },
    };
    let base = match registry_fact(heap, sym) {
        Some(fact) => format!("{base}|R{:016x}", fnv1a(&fact)),
        None => base,
    };
    // The sig's PRINTED form, not `hash_value`: that hashes symbols by interned id, which
    // depends on interning order and so differs from process to process — `nest check`
    // and `nest run` must agree on a declared function's fact.
    match heap.declared_sig_value(sym) {
        Some(v) => format!(
            "{base}|S{:016x}",
            fnv1a(&crate::syntax::printer::print(heap, v))
        ),
        None => base,
    }
}

// ── a definition's content digest (ADR-485) ──────────────────────────────────
//
// A user global's fact used to be its defining FILE's mtime, which is the right answer
// at the wrong grain: a check that observed one function of a module was stale the moment
// ANY line of that module moved. On bedit (204 files) touching one module re-checked 158
// files, 42 s, because inference walks callee bodies and so nearly every file observes
// something in nearly every module. The fact is now what the checker can actually SEE of
// the definition:
//
//   - a closure or macro: its name and every arm's parameters, optionals and body forms —
//     the very values `infer_sig`, `infer_overload_of`, `arity_of` and `param_names_of`
//     read. A body edit changes the digest; an edit elsewhere in the file does not;
//   - a primitive bound under a user name: the primitive's name (its signature is the
//     build's, and the cache is stamped with the build id);
//   - any other value: its KIND — all `global_value_ty` takes from it, the literal being
//     deliberately ignored there (`(def- dev? (env/dev?))`), so a value computed at load
//     (`(def started (os/now))`) keeps one fact across runs — and for a dynamic or
//     earmuffed global, which is never typed, not even that.
//
// The declared sig, the registries, the export sets and known namespaces are separate
// facts, as before. Anything the walk cannot render process-independently (a function
// value spliced into a body, a pid, a rope) gives `None`, and the fact falls back to the
// file's mtime: never less sound than before.
//
// Process-independent by construction: symbols by NAME (interned ids differ per process),
// a gensym by its first-appearance index within the definition (the counter differs per
// process and per load order — an imaged start and a source load mint different numbers
// for the same expansion), maps and sets order-independently.

thread_local! {
    /// Digests of closures already walked, keyed by the closure's move-stable identity
    /// and its arms allocation — a closure is immutable, so one walk serves the process.
    /// Thread-local because a worker heap's closures are its own; a fingerprint pass asks
    /// for the same few thousand globals once per file, which is what this saves.
    static CLOSURE_DIGESTS: std::cell::RefCell<std::collections::HashMap<(u64, usize), Option<u64>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// See the section comment. `None` when the definition cannot be digested.
fn definition_digest(heap: &Heap, sym: Symbol) -> Option<u64> {
    let mut digest = Digest::new();
    let bound = heap.env_get(heap.global(), sym)?;
    match bound {
        Value::Fn(cid) | Value::Macro(cid) => {
            let closure = heap.closure(cid);
            let key = (
                closure.identity,
                std::sync::Arc::as_ptr(&closure.arms) as *const () as usize,
            );
            let known = CLOSURE_DIGESTS.with(|memo| memo.borrow().get(&key).copied());
            let computed = match known {
                Some(known) => known,
                None => {
                    let computed = closure_digest(heap, closure);
                    CLOSURE_DIGESTS.with(|memo| memo.borrow_mut().insert(key, computed));
                    computed
                }
            };
            // A function and a macro read differently (`resolves_to_macro`,
            // `evaluates_args`), so the kind is part of the fact.
            digest.text(if matches!(bound, Value::Macro(_)) {
                "M"
            } else {
                "F"
            });
            digest.number(computed?);
        }
        Value::Native(id) => {
            digest.text("N");
            digest.text(&heap.native(id).name);
        }
        // A dynamic or earmuffed global's value is never typed (`global_value_ty` stops
        // before reading it), and it is exactly the kind of global a process sets for
        // itself — `*image-path*` is nil in a source-loaded process and a string in an
        // imaged one — so its fact is only that it is bound.
        _ if heap.is_dynamic(sym) || super::infer::is_earmuffed(sym) => digest.text("V*"),
        other => {
            digest.text("V");
            digest.text(&format!("{:?}", value::tag(other)));
        }
    }
    Some(digest.finish())
}

/// A closure's digest: its name, then each arm's parameters, optionals, rest and body.
fn closure_digest(heap: &Heap, closure: &value::Closure) -> Option<u64> {
    let mut digest = Digest::new();
    let mut gensyms = std::collections::HashMap::new();
    digest.text(&closure.name.map(value::symbol_name).unwrap_or_default());
    for arm in closure.arms.iter() {
        digest.text("arm");
        for &param in &arm.params {
            digest.symbol(param, &mut gensyms);
        }
        digest.text("&optional");
        for &(name, default) in &arm.optionals {
            digest.symbol(name, &mut gensyms);
            digest.form(heap, default, &mut gensyms)?;
        }
        digest.text("&rest");
        if let Some(rest) = arm.rest {
            digest.symbol(rest, &mut gensyms);
        }
        digest.text("body");
        for &form in &arm.body {
            digest.form(heap, form, &mut gensyms)?;
        }
    }
    Some(digest.finish())
}

/// FNV-1a over a self-delimiting stream: every item is tagged and length-prefixed, so
/// `("ab" "c")` and `("a" "bc")` cannot meet.
struct Digest(u64);

impl Digest {
    fn new() -> Self {
        Digest(0xcbf2_9ce4_8422_2325)
    }
    fn byte(&mut self, byte: u8) {
        self.0 ^= byte as u64;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }
    fn number(&mut self, number: u64) {
        for byte in number.to_le_bytes() {
            self.byte(byte);
        }
    }
    fn text(&mut self, text: &str) {
        self.number(text.len() as u64);
        for &byte in text.as_bytes() {
            self.byte(byte);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
    /// A symbol by name, a gensym by its first-appearance index in this definition.
    fn symbol(&mut self, symbol: Symbol, gensyms: &mut std::collections::HashMap<Symbol, u64>) {
        if value::is_gensym(symbol) {
            let next = gensyms.len() as u64;
            let index = *gensyms.entry(symbol).or_insert(next);
            self.byte(b'g');
            self.number(index);
        } else {
            self.byte(b's');
            self.text(&value::symbol_name(symbol));
        }
    }
    /// A code or data form. `None` for anything without a process-independent rendering.
    fn form(
        &mut self,
        heap: &Heap,
        form: Value,
        gensyms: &mut std::collections::HashMap<Symbol, u64>,
    ) -> Option<()> {
        crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || {
            match form {
                Value::Nil => self.byte(b'n'),
                Value::Bool(flag) => {
                    self.byte(b'b');
                    self.byte(u8::from(flag));
                }
                Value::Int(number) => {
                    self.byte(b'i');
                    self.number(number as u64);
                }
                Value::Float(number) => {
                    self.byte(b'f');
                    self.number(number.to_bits());
                }
                Value::BigInt(_) | Value::Decimal(_) | Value::Ratio(_) => {
                    self.byte(b'#');
                    self.text(&crate::syntax::printer::print(heap, form));
                }
                Value::Str(id) => {
                    self.byte(b'"');
                    self.text(heap.string(id));
                }
                Value::Sym(symbol) => self.symbol(symbol, gensyms),
                Value::Keyword(symbol) => {
                    self.byte(b':');
                    self.text(&value::symbol_name(symbol));
                }
                Value::Native(id) => {
                    self.byte(b'N');
                    self.text(&heap.native(id).name);
                }
                Value::Pair(_) => {
                    self.byte(b'(');
                    let mut rest = form;
                    while let Value::Pair(cell) = rest {
                        self.form(heap, heap.car(cell), gensyms)?;
                        rest = heap.cdr(cell);
                    }
                    // the tail: nil for a proper list, the atom of a dotted one
                    self.byte(b'.');
                    self.form(heap, rest, gensyms)?;
                }
                Value::Vector(id) => {
                    self.byte(b'[');
                    let items = heap.vector(id);
                    self.number(items.len() as u64);
                    for &item in items.iter() {
                        self.form(heap, item, gensyms)?;
                    }
                }
                Value::Map(id) | Value::Set(id) => {
                    // Order-independent: a CHAMP map iterates in hash order, and a symbol
                    // hashes by its interned id. Each entry is digested on its own (its
                    // gensyms numbered within it) and the entry digests summed.
                    self.byte(if matches!(form, Value::Set(_)) {
                        b'#'
                    } else {
                        b'{'
                    });
                    let mut sum: u64 = 0;
                    let entries = heap.map_entries(id);
                    for (key, entry) in &entries {
                        let mut part = Digest::new();
                        let mut local = std::collections::HashMap::new();
                        part.form(heap, *key, &mut local)?;
                        part.form(heap, *entry, &mut local)?;
                        sum = sum.wrapping_add(part.finish());
                    }
                    self.number(entries.len() as u64);
                    self.number(sum);
                }
                _ => return None,
            }
            Some(())
        })
    }
}

/// Fingerprint of a `:use`d module's current export SET (which bare names it
/// exports). A change here (an export added/removed) changes how a bare name in the
/// importing file resolves. The exported globals' *definitions* are captured
/// separately via each resolved `mod/name` reference's `fact_of_sym`.
fn exports_fact(heap: &Heap, prefix: &str) -> String {
    let mut names: Vec<String> = heap
        .module_public_exports(prefix)
        .into_iter()
        .map(|(bare, qual)| format!("{}>{}", value::symbol_name(bare), value::symbol_name(qual)))
        .collect();
    names.sort_unstable();
    names.join(",")
}

/// Structural hash of the `*protocols*` table (accumulated across files, so its
/// def-site alone can't capture a later `extend`).
fn protocols_fact(heap: &Heap) -> String {
    match heap.env_get(heap.global(), value::intern(reg::PROTOCOLS)) {
        Some(v) => heap.hash_value(v).to_string(),
        None => "none".to_string(),
    }
}

/// A compact, deterministic stamp of the current-image facts for `dep_keys`.
/// Two calls return the same string iff every observed global fact is unchanged.
pub(super) fn fingerprint(heap: &Heap, dep_keys: Value) -> String {
    let Value::Map(id) = dep_keys else {
        return "∅".to_string();
    };
    let mut buf = String::new();
    for name in field_strings(heap, id, K_SYMS) {
        buf.push_str("s:");
        buf.push_str(&name);
        buf.push('=');
        buf.push_str(&fact_of_sym(heap, value::intern(&name)));
        buf.push(';');
    }
    let known = heap.known_ns_prefixes();
    for p in field_strings(heap, id, K_KNS) {
        buf.push_str("k:");
        buf.push_str(&p);
        buf.push('=');
        buf.push(if known.contains(&p) { '1' } else { '0' });
        buf.push(';');
    }
    for p in field_strings(heap, id, K_EXP) {
        buf.push_str("e:");
        buf.push_str(&p);
        buf.push('=');
        buf.push_str(&exports_fact(heap, &p));
        buf.push(';');
    }
    if matches!(
        heap.map_get(id, Value::keyword(value::intern(K_PROTO))),
        Some(Value::Bool(true))
    ) {
        buf.push_str("p:=");
        buf.push_str(&protocols_fact(heap));
        buf.push(';');
    }
    // Compact the (possibly large) fact string to a 64-bit FNV-1a hex stamp.
    // FNV-1a is fixed-constant → deterministic within a build; the cache carries a
    // (system/build-id) stamp so cross-build reuse can't happen anyway.
    format!("{:016x}", fnv1a(&buf))
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The registries whose content is a checker input ([`obs_registry`]).
const OBSERVED_REGISTRIES: &[&str] = &[
    reg::IMPLS,
    reg::OP_ABILITY,
    reg::ABILITIES,
    reg::SEALED,
    reg::ABILITY_REQUIRES,
    reg::METHODS,
    reg::METHOD_FROM,
    reg::MULTI_ALGEBRA,
    reg::MULTI_RET,
    reg::RECORD_IDS,
    "*ability-owner*",
];

/// The fact of a dispatch registry: a canonical rendering of its entries, minus those a
/// STANDARD-LIBRARY module registered. `None` for any other global.
///
/// Why std entries are left out: the reuse test runs once the project's sources are
/// loaded, while the entry was stamped after the re-check — which materialises every module
/// a loaded body names (`project-preload!`). A std module registers the same entries on
/// every run of one build (and the cache is stamped with the build id), so its entries say
/// nothing about the project; counted, they made a file whose check loaded `datetime` miss
/// the cache on every run. A root-namespace entry (the prelude's, a module-less file's) is
/// kept: the prelude is always loaded, so it is stable, and the file's is the project's.
///
/// Canonical: map entries sorted by their printed form, a function printed as `fn` — a
/// CHAMP map iterates in hash order and a symbol hashes by its interned id, which differs
/// from process to process, and a fingerprint written by one `nest check` is compared by
/// the next.
fn registry_fact(heap: &Heap, sym: Symbol) -> Option<String> {
    let name = value::symbol_name(sym);
    if !OBSERVED_REGISTRIES.contains(&name.as_str()) {
        return None;
    }
    let Some(Value::Map(registry)) = heap.env_get(heap.global(), sym) else {
        return Some("none".to_string());
    };
    let provenance =
        |registry_name: &str| match heap.env_get(heap.global(), value::intern(registry_name)) {
            Some(Value::Map(id)) => Some(id),
            _ => None,
        };
    let owners = provenance("*ability-owner*");
    let impl_from = provenance("*impl-from*");
    let method_from = provenance("*method-from*");
    let owner_of = |ability: Value| -> Option<String> {
        owners
            .and_then(|id| heap.map_get(id, ability))
            .and_then(namespace_name)
    };
    let mut entries: Vec<String> = Vec::new();
    let mut provenance_index: Option<std::collections::HashMap<String, Option<String>>> = None;
    for (key, entry) in heap.map_entries(registry) {
        match name.as_str() {
            // `[ability op] → {id → fn}`: each id's impl carries its own provenance.
            n if n == reg::IMPLS || n == reg::METHODS => {
                let Value::Map(inner) = entry else { continue };
                let from = if n == reg::IMPLS {
                    impl_from
                } else {
                    method_from
                };
                let index = provenance_index.get_or_insert_with(|| {
                    from.map(|from| {
                        heap.map_entries(from)
                            .into_iter()
                            .map(|(provenance, namespace)| {
                                (canonical(heap, provenance), namespace_name(namespace))
                            })
                            .collect::<std::collections::HashMap<_, _>>()
                    })
                    .unwrap_or_default()
                });
                for (inner_key, _) in heap.map_entries(inner) {
                    // `*impl-from*` keys `[ability op id]`, `*method-from*` `[mname key]`.
                    let wanted = match key {
                        Value::Vector(outer_id) if n == reg::IMPLS => {
                            let mut parts: Vec<String> = heap
                                .vector(outer_id)
                                .iter()
                                .map(|&part| canonical(heap, part))
                                .collect();
                            parts.push(canonical(heap, inner_key));
                            format!("[{}]", parts.join(" "))
                        }
                        _ => format!("[{} {}]", canonical(heap, key), canonical(heap, inner_key)),
                    };
                    let namespace = index
                        .get(&wanted)
                        .cloned()
                        .flatten()
                        .or_else(|| qualifier_of(heap, key));
                    if !is_std_namespace(namespace.as_deref()) {
                        entries.push(format!(
                            "{}/{}",
                            canonical(heap, key),
                            canonical(heap, inner_key)
                        ));
                    }
                }
            }
            n if n == reg::OP_ABILITY => {
                if !is_std_namespace(owner_of(entry).as_deref()) {
                    entries.push(format!(
                        "{}={}",
                        canonical(heap, key),
                        canonical(heap, entry)
                    ));
                }
            }
            n if n == reg::ABILITIES
                || n == reg::SEALED
                || n == reg::ABILITY_REQUIRES
                || n == "*ability-owner*" =>
            {
                if !is_std_namespace(owner_of(key).as_deref()) {
                    entries.push(format!(
                        "{}={}",
                        canonical(heap, key),
                        canonical(heap, entry)
                    ));
                }
            }
            // `*method-from*`, `*multi-algebra*`, `*multi-ret*`, `*record-ids*`: keyed by a
            // qualified name (or a `[name key]` whose name is), so the key says whose it is.
            _ => {
                let namespace = match name.as_str() {
                    n if n == reg::METHOD_FROM => namespace_name(entry),
                    _ => qualifier_of(heap, key),
                };
                if !is_std_namespace(namespace.as_deref()) {
                    entries.push(format!(
                        "{}={}",
                        canonical(heap, key),
                        canonical(heap, entry)
                    ));
                }
            }
        }
    }
    entries.sort_unstable();
    Some(entries.join(";"))
}

/// The namespace a provenance value names (a symbol, keyword or string), `None` for nil —
/// a registration made at the root.
fn namespace_name(value: Value) -> Option<String> {
    match value {
        Value::Sym(s) | Value::Keyword(s) => Some(value::symbol_name(s)),
        _ => None,
    }
}

/// The namespace qualifier of a registry key: `ns` of a qualified `ns/name` symbol or
/// keyword, or of the first element of a vector key. `None` for a bare (root) name.
fn qualifier_of(heap: &Heap, key: Value) -> Option<String> {
    let name = match key {
        Value::Sym(s) | Value::Keyword(s) => value::symbol_name(s),
        Value::Vector(id) => return heap.vector(id).first().and_then(|&k| qualifier_of(heap, k)),
        _ => return None,
    };
    name.rfind('/').map(|slash| name[..slash].to_string())
}

/// Is `namespace` a baked-in standard-library module? The root (`None`) is not.
fn is_std_namespace(namespace: Option<&str>) -> bool {
    namespace.is_some_and(|namespace| {
        crate::builtins::modules::embedded_module_source(namespace).is_some()
    })
}

/// A process-independent rendering of a registry value: maps sorted by printed entry,
/// functions reduced to `fn` (their identity is the defining file's business, and a
/// printed closure is not stable).
fn canonical(heap: &Heap, v: Value) -> String {
    crate::stack::maybe_grow(64 * 1024, 1024 * 1024, || match v {
        Value::Map(id) => {
            let mut entries: Vec<String> = heap
                .map_entries(id)
                .into_iter()
                .map(|(key, entry)| format!("{} {}", canonical(heap, key), canonical(heap, entry)))
                .collect();
            entries.sort_unstable();
            format!("{{{}}}", entries.join(", "))
        }
        Value::Vector(id) => {
            let items: Vec<String> = heap
                .vector(id)
                .iter()
                .map(|&e| canonical(heap, e))
                .collect();
            format!("[{}]", items.join(" "))
        }
        Value::Fn(_) | Value::Native(_) => "fn".to_string(),
        _ => crate::syntax::printer::print(heap, v),
    })
}
