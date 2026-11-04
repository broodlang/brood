//! The runtime contract's verdict memo (ADR-480) — child of heap.
//!
//! An armed `(sig …)` re-asks `type-matches?` about every argument and result of every call
//! across a module boundary, and a program's central value — an editor's model, a server's
//! state — is checked dozens of times per event, each time in full: every buffer, every
//! diagnostic. But a Brood value is immutable, so a sub-value that did not change is the
//! very same heap object it was on the previous call, and the verdict about it cannot have
//! changed either. This table remembers `(type expression, value) → verdict` by the
//! IDENTITY of both, so re-checking an unchanged value is one lookup and checking a new
//! value with one changed field re-walks only the path to that field.
//!
//! **Identity, not equality.** Hashing a value structurally would cost what the check costs.
//! A key is the raw handle bits — region, age, epoch stamp and slab index — which name one
//! object for as long as it lives where it is. What can make the same bits name a DIFFERENT
//! object is exactly what this module listens to:
//!
//! - a **minor collection** moves every surviving nursery object and drops the rest, and a
//!   `reset_local_to` truncates the nursery so its slots are minted again — every entry
//!   that names a nursery object goes ([`Heap::type_verdicts_forget_nursery`]);
//! - a **major collection** compacts the old generation — every LOCAL entry goes;
//! - the shared RUNTIME region relocating or freeing code (`relocations`, `free_epoch`) —
//!   the whole table goes (a RUNTIME literal is a type expression's usual home).
//!
//! PRELUDE objects never move. A symbol is interned for the life of the runtime.
//!
//! **What can change a verdict for the same pair**, and so clears the table when it moves
//! (the [`VerdictStamp`]): any global binding (`version` — `*record-ids*` growing, a sealed
//! ability gaining members, a redefined record), any declared signature (a new or changed
//! `deftype` alias), and the namespace a bare type name resolves in (`compile_ns`, the
//! package being loaded). A cleared table costs one full check per value, never a wrong
//! answer.
//!
//! **Bounded**: past [`VERDICT_CAPACITY`] entries the table starts over, which is cheaper
//! than tracking recency and costs the same one full check per value.
//!
//! Per heap, so per process: a verdict about a LOCAL object means nothing to another
//! heap, and no lock is ever taken.
//!
//! **What is decided here, not remembered.** Beside the table, [`Heap::type_verdict`]
//! answers what it can state exactly from a value's tag and a type's form — a base name, a
//! literal, `?A`, an arrow, an `or` of decided parts — and, on a miss, a record or uniform
//! vector or list one level deep over decided parts. Everything else (`len`, `rec`, `map`,
//! `set`, `seqable`, sealed abilities, record names, a record instance's absent field) is
//! left to `%type-match`, the Brood walk, which stays the one full reading of the grammar.

use super::*;
use crate::core::value::{AGE_OLD, REGION_SHIFT};

/// The most verdicts one heap keeps before starting over. An editor's model with a few
/// dozen buffers and a few thousand diagnostics holds well under this; the bound exists
/// for the program that checks a stream of distinct values and would otherwise grow the
/// table without end.
pub(crate) const VERDICT_CAPACITY: usize = 1 << 16;

/// How deep `or` alternatives and alias expansions are followed when deciding a form
/// without the walk. Real types nest a few levels; a cycle is cut here and left to the walk.
const FORM_DEPTH: u32 = 16;

/// What a verdict was reached under. A verdict is reused only while every part is equal.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct VerdictStamp {
    globals_version: u64,
    declared_sigs_version: u64,
    relocations: u64,
    free_epoch: u64,
    compile_ns: Option<Symbol>,
    package_prefix: Option<Symbol>,
}

/// One side of a key: a symbol id, or a heap handle's raw bits. Tagged so a symbol's id
/// can never equal a handle's bits.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Identity {
    Symbol(Symbol),
    Handle(u64),
}

impl Identity {
    /// Whether this names an object a minor collection moves (a LOCAL nursery object).
    fn in_nursery(self) -> bool {
        match self {
            Identity::Symbol(_) => false,
            Identity::Handle(raw) => handle_region(raw) == LOCAL && (raw & AGE_OLD) == 0,
        }
    }
    /// Whether this names any LOCAL object (nursery or old), which a major collection moves.
    fn in_local(self) -> bool {
        match self {
            Identity::Symbol(_) => false,
            Identity::Handle(raw) => handle_region(raw) == LOCAL,
        }
    }
}

fn handle_region(raw: u64) -> u8 {
    (raw >> REGION_SHIFT) as u8
}

/// The identity of a type expression worth remembering a verdict under: a name (`model`,
/// `int`, an alias) or a compound form (`(vector buffer)`). A literal type (`:ok`, `3`) is
/// answered by one `=`, so it is not worth a lookup.
///
/// A compound form in the NURSERY is not kept either. A declared type lives where its
/// declaration does — a shim's captured spec, the declared-sig store, compiled code — all
/// promoted out of the nursery; a nursery type was built for this one check (a
/// `(rec X body)` unrolled per level), will never be asked about again, and would fill the
/// table with one entry per node of the value it walked.
fn type_identity(type_expression: Value) -> Option<Identity> {
    match type_expression {
        Value::Sym(symbol) => Some(Identity::Symbol(symbol)),
        Value::Pair(id) => Some(Identity::Handle(id.0)).filter(|identity| !identity.in_nursery()),
        _ => None,
    }
}

/// The identity of a value worth remembering a verdict about: the compounds a check walks
/// into. A scalar is answered by one predicate, and a lazy range or seq-view is realised
/// afresh by every walk, so neither is kept.
fn value_identity(value: Value) -> Option<Identity> {
    match value {
        Value::Pair(id) => Some(Identity::Handle(id.0)),
        Value::Vector(id) => Some(Identity::Handle(id.0)),
        Value::Map(id) => Some(Identity::Handle(id.0)),
        Value::Set(id) => Some(Identity::Handle(id.0)),
        _ => None,
    }
}

type VerdictKey = (Identity, Identity);

/// The table and its counters.
#[derive(Default)]
pub(crate) struct TypeVerdictCache {
    stamp: VerdictStamp,
    verdicts: HashMap<VerdictKey, bool, std::hash::BuildHasherDefault<IdentityHasher>>,
    /// Per type NAME: whether some record id in `*record-ids*` is called that — the scan
    /// `%nominal-type-matches?` made per checked value, through `->string` on every id. It
    /// depends on the name and the registry alone, and the registry moving moves the stamp.
    names_record: HashMap<Symbol, bool, std::hash::BuildHasherDefault<IdentityHasher>>,
    hits: u64,
    asks: u64,
    vouched: u64,
    misses: u64,
    clears: u64,
}

/// What `(%type-verdict-counts)` reports: memo hits and misses, verdicts held, times the
/// table started over, and contracted calls whose arguments were put to the kernel
/// (`asks`) and vouched for without the Brood check (`vouched`).
#[derive(Default)]
pub(crate) struct VerdictCounts {
    pub hits: u64,
    pub misses: u64,
    pub entries: u64,
    pub clears: u64,
    pub asks: u64,
    pub vouched: u64,
}

impl TypeVerdictCache {
    fn start_over(&mut self) {
        if !self.verdicts.is_empty() {
            self.verdicts.clear();
            self.clears += 1;
        }
        self.names_record.clear();
    }
}

/// A hasher for keys that are already well-distributed integers — the odd-multiply mix
/// [`SymbolHasher`] uses, fed every word of the key.
#[derive(Default)]
pub(crate) struct IdentityHasher(u64);

impl std::hash::Hasher for IdentityHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write_u32(&mut self, word: u32) {
        self.write_u64(word as u64);
    }
    #[inline]
    fn write_u64(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    #[inline]
    fn write_isize(&mut self, word: isize) {
        self.write_u64(word as u64);
    }
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u64(byte as u64);
        }
    }
}

/// The verdict a BASE type gives `value` with no walk and no memo: a base name is a set of
/// runtime tags — the tags `type-of` reports, which every Brood type predicate reads — so
/// it is answered from the value's tag alone. `%type-match` answers these names the same
/// way and in the same place (before any alias lookup), so a `deftype` cannot rebind one;
/// this is that table moved to where a record's scalar fields reach it in one call. A
/// literal keyword or bool type is the value itself. Anything else is `None`: the walk's.
fn base_type_verdict(type_expression: Value, value: Value) -> Option<bool> {
    let name = match type_expression {
        Value::Sym(symbol) => crate::core::value::symbol_name_ref(symbol),
        Value::Keyword(keyword) => return Some(matches!(value, Value::Keyword(k) if k == keyword)),
        Value::Bool(literal) => return Some(matches!(value, Value::Bool(b) if b == literal)),
        _ => return None,
    };
    use crate::core::value::Tag;
    let tag = crate::core::value::tag(value);
    Some(match name {
        "any" | "_" => true,
        "never" => false,
        "nil" => tag == Tag::Nil,
        "int" => tag == Tag::Int,
        "float" => tag == Tag::Float,
        "decimal" => tag == Tag::Decimal,
        "ratio" => tag == Tag::Ratio,
        "number" => matches!(tag, Tag::Int | Tag::Float | Tag::Decimal | Tag::Ratio),
        "string" => tag == Tag::Str,
        "bool" => tag == Tag::Bool,
        "keyword" => tag == Tag::Keyword,
        "symbol" => tag == Tag::Sym,
        "pair" => tag == Tag::Pair,
        "list" => matches!(tag, Tag::Nil | Tag::Pair),
        "vector" => tag == Tag::Vector,
        "map" => tag == Tag::Map,
        "set" => tag == Tag::Set,
        "bytes" => tag == Tag::Bytes,
        "fn" => matches!(tag, Tag::Fn | Tag::Native),
        "seqable" => matches!(
            tag,
            Tag::Nil | Tag::Pair | Tag::Vector | Tag::Set | Tag::Map | Tag::Bytes
        ),
        "countable" => matches!(
            tag,
            Tag::Nil
                | Tag::Pair
                | Tag::Vector
                | Tag::Set
                | Tag::Map
                | Tag::Bytes
                | Tag::Str
                | Tag::Rope
                | Tag::Table
        ),
        "failure" => tag == Tag::Failure,
        "rope" => tag == Tag::Rope,
        "pid" => tag == Tag::Pid,
        "ref" => tag == Tag::Ref,
        "table" => tag == Tag::Table,
        "socket" => tag == Tag::Socket,
        "subprocess" => tag == Tag::Subprocess,
        _ => return None,
    })
}
/// What `(%type-verdict t v)` found.
pub(crate) enum KnownVerdict {
    /// Remembered: the verdict.
    Known(bool),
    /// Worth remembering, not yet known: check, then record.
    Unknown,
    /// Not worth remembering (a scalar, a literal type): check and move on.
    Unkept,
}

impl Heap {
    fn verdict_stamp(&self) -> VerdictStamp {
        VerdictStamp {
            globals_version: self.runtime.version.load(Ordering::Relaxed),
            declared_sigs_version: self.runtime.declared_sigs_version.load(Ordering::Acquire),
            relocations: self.code_relocations(),
            free_epoch: self.free_epoch_now(),
            compile_ns: self.compile_ns(),
            package_prefix: self.package_prefix(),
        }
    }

    fn verdict_cache(&self) -> &RefCell<TypeVerdictCache> {
        self.type_verdicts.get_or_init(Box::default)
    }

    /// The remembered verdict on whether `value` inhabits `type_expression`, if one is
    /// still valid — see the module docs for what invalidates one. A miss on a record or a
    /// uniform sequence is then tried ONE level deep, here: when every field or element is
    /// decided by its type's tag or by a remembered verdict — a new model whose buffers are
    /// the same objects as last time — the whole is decided without the Brood walk, and
    /// remembered. Anything undecided leaves it to the walk.
    pub(crate) fn type_verdict(&self, type_expression: Value, value: Value) -> KnownVerdict {
        if let Some(verdict) = self.form_verdict(type_expression, value, 0) {
            return KnownVerdict::Known(verdict);
        }
        let keys = (type_identity(type_expression), value_identity(value));
        if let (Some(type_key), Some(value_key)) = keys {
            let mut cache = self.synced_verdict_cache();
            if let Some(verdict) = cache.verdicts.get(&(type_key, value_key)).copied() {
                cache.hits += 1;
                return KnownVerdict::Known(verdict);
            }
            cache.misses += 1;
        }
        let keep = keys.0.is_some() && keys.1.is_some();
        match self.one_level_verdict(type_expression, value) {
            Some(verdict) => {
                if keep {
                    self.record_type_verdict(type_expression, value, verdict);
                }
                KnownVerdict::Known(verdict)
            }
            None if keep => KnownVerdict::Unknown,
            None => KnownVerdict::Unkept,
        }
    }

    /// What the type expression's FORM decides with no memo and no walk: a base name or a
    /// literal (by the value's tag), a type variable (`?A`, a static-only constraint the
    /// walk accepts), an arrow type (the walk's `fn?`), and an `or` whose alternatives are
    /// each decided. Mirrors the order `%type-match` asks in: an arrow before the head, a
    /// type variable only once no alias claims the name.
    fn form_verdict(&self, type_expression: Value, value: Value, depth: u32) -> Option<bool> {
        // An alias cycle through `or` (`(deftype a (or b))` `(deftype b (or a))`) the walk
        // stops with its `seen` path; here a bound leaves it undecided, for the walk.
        if depth > FORM_DEPTH {
            return None;
        }
        if matches!(type_expression, Value::Nil) {
            return Some(matches!(value, Value::Nil));
        }
        if let Some(verdict) = base_type_verdict(type_expression, value) {
            return Some(verdict);
        }
        match type_expression {
            Value::Sym(symbol) => {
                let type_variable = crate::core::value::symbol_name_ref(symbol).starts_with('?');
                (type_variable && crate::builtins::alias_expansion(self, symbol).is_none())
                    .then_some(true)
            }
            Value::Pair(id) => {
                let (head, _) = self.pair(id);
                let items = self.list_to_vec(type_expression).ok()?;
                let is_symbol = |item: &Value, name: &str| matches!(item, Value::Sym(symbol) if crate::core::value::symbol_name_ref(*symbol) == name);
                if items.iter().any(|item| is_symbol(item, "->")) {
                    return Some(matches!(
                        crate::core::value::tag(value),
                        crate::core::value::Tag::Fn | crate::core::value::Tag::Native
                    ));
                }
                if !is_symbol(&head, "or") {
                    return None;
                }
                let mut undecided = false;
                for &alternative in &items[1..] {
                    match self.decided_verdict(alternative, value, depth + 1) {
                        Some(true) => return Some(true),
                        Some(false) => {}
                        None => undecided = true,
                    }
                }
                if undecided { None } else { Some(false) }
            }
            _ => None,
        }
    }

    /// The table, cleared first if its stamp has moved.
    fn synced_verdict_cache(&self) -> std::cell::RefMut<'_, TypeVerdictCache> {
        let stamp = self.verdict_stamp();
        let mut cache = self.verdict_cache().borrow_mut();
        if cache.stamp != stamp {
            cache.start_over();
            cache.stamp = stamp;
        }
        cache
    }

    /// A part's verdict when it is already decided: by its form ([`form_verdict`]), by a
    /// remembered verdict, or — for an alias over a base name (`(deftype line int)`) — by
    /// the tag under the alias. `None` is undecided, never "fails".
    ///
    /// [`form_verdict`]: Self::form_verdict
    fn decided_verdict(&self, type_expression: Value, value: Value, depth: u32) -> Option<bool> {
        if let Some(verdict) = self.form_verdict(type_expression, value, depth) {
            return Some(verdict);
        }
        if let (Some(type_key), Some(value_key)) =
            (type_identity(type_expression), value_identity(value))
        {
            let mut cache = self.synced_verdict_cache();
            if let Some(verdict) = cache.verdicts.get(&(type_key, value_key)).copied() {
                cache.hits += 1;
                return Some(verdict);
            }
        }
        match type_expression {
            // An alias stands for its expansion: decided when the expansion's form is.
            Value::Sym(symbol) => self.form_verdict(
                crate::builtins::alias_expansion(self, symbol)?,
                value,
                depth + 1,
            ),
            _ => None,
        }
    }

    /// `type_expression` against `value` one level deep — the cases `%type-match` handles
    /// that a contracted editor meets per call: a record (closed or `&open`, `(optional T)`
    /// fields), or a uniform vector or list, each behind an alias or not. Every part
    /// must be [`decided`](Self::decided_verdict); one that is not makes the whole `None`,
    /// unless another part already fails it. Mirrors `%type-match` and must stay in step
    /// with it: the record's `get` is the map's own entry (a record INSTANCE, whose `get` may
    /// go through `Lookup`, is left to the walk), and a closed record admits only its keys.
    fn one_level_verdict(&self, type_expression: Value, value: Value) -> Option<bool> {
        let form = match type_expression {
            Value::Sym(symbol) => crate::builtins::alias_expansion(self, symbol)?,
            form => form,
        };
        // An expansion is a new form: an arrow, an `or` or a name may decide it outright —
        // and an arrow is decided THERE, before any head below could claim it.
        if let Some(verdict) = self.form_verdict(form, value, 0) {
            return Some(verdict);
        }
        if let Value::Sym(_) = form {
            return self.decided_verdict(form, value, 0);
        }
        let items = self.list_to_vec(form).ok()?;
        let symbol_named = |item: &Value, name: &str| matches!(item, Value::Sym(symbol) if crate::core::value::symbol_name_ref(*symbol) == name);
        let head = items.first()?;
        // Fold part verdicts the way `and` does: one failure decides, else any undecided
        // part leaves the whole undecided.
        let all_of = |parts: &mut dyn Iterator<Item = Option<bool>>| {
            let mut undecided = false;
            for part in parts {
                match part {
                    Some(false) => return Some(false),
                    Some(true) => {}
                    None => undecided = true,
                }
            }
            if undecided { None } else { Some(true) }
        };
        if symbol_named(head, "record") {
            // A map, or a record's fields (ADR-542). A record INSTANCE answers `get` on a
            // missing (or nil) key through its `Lookup` impl (`%lookup-miss`), which only
            // the walk can call; a field it holds is its field map's own entry either way.
            let (map_id, instance_id) = match value {
                Value::Map(map_id) => (map_id, None),
                Value::Record { id, fields } => (fields, Some(id)),
                _ => return Some(false),
            };
            let instance = instance_id.is_some();
            let open = items.get(1).is_some_and(|item| symbol_named(item, "&open"));
            // `key type` pairs; a dangling odd key is ignored, as the walk ignores it.
            let (all_fields, _) = items[if open { 2 } else { 1 }..].as_chunks::<2>();
            // The `:__id__` pseudo-field a `defrecord` constructor's result type carries
            // names the record type: such a shape admits only a record of that id, and the
            // pseudo-field is no field of it. A shape without one admits a map or a record,
            // as `type-matches?` does (an accessor's `&open` shape takes any carrier).
            let id_key = Value::Keyword(*crate::core::value::RECORD_ID_SYMBOL);
            let is_id = |field: &[Value; 2]| matches!((field[0], id_key), (Value::Keyword(a), Value::Keyword(b)) if a == b);
            if let Some(field) = all_fields.iter().find(|field| is_id(field)) {
                let wanted = match field[1] {
                    Value::Keyword(wanted) => wanted,
                    _ => return None,
                };
                if instance_id != Some(wanted) {
                    return Some(false);
                }
            }
            let fields: Vec<[Value; 2]> = all_fields
                .iter()
                .filter(|field| !is_id(field))
                .copied()
                .collect();
            let fields = fields.as_slice();
            // Keyword field names only: the closed check below compares names by identity,
            // as `%record-declares?` does with `%eq`, which is exact only for interned keys.
            if fields
                .iter()
                .any(|field| !matches!(field[0], Value::Keyword(_)))
            {
                return None;
            }
            let field_verdicts = fields.iter().map(|field| {
                let field_value = self.map_get(map_id, field[0]).unwrap_or(Value::Nil);
                if instance && matches!(field_value, Value::Nil) {
                    return None;
                }
                let field_type = field[1];
                if let Value::Pair(id) = field_type {
                    let (wrapper, tail) = self.pair(id);
                    if symbol_named(&wrapper, "optional") {
                        if matches!(field_value, Value::Nil) {
                            return Some(true);
                        }
                        let Value::Pair(tail_id) = tail else {
                            return None;
                        };
                        return self.decided_verdict(self.pair(tail_id).0, field_value, 0);
                    }
                }
                self.decided_verdict(field_type, field_value, 0)
            });
            let fields_pass = all_of(&mut field_verdicts.collect::<Vec<_>>().into_iter());
            if fields_pass != Some(true) || open {
                return fields_pass;
            }
            let declared = |key: Value| {
                let Value::Keyword(key) = key else {
                    return false;
                };
                fields
                    .iter()
                    .any(|field| matches!(field[0], Value::Keyword(name) if name == key))
            };
            return Some(
                self.map_entries(map_id)
                    .into_iter()
                    .all(|(key, _)| declared(key)),
            );
        }
        if symbol_named(head, "vector") {
            let Value::Vector(vector_id) = value else {
                return Some(false);
            };
            let Some(&element_type) = items.get(1) else {
                return Some(true);
            };
            let elements: Vec<Value> = self.vector(vector_id).to_vec();
            return all_of(
                &mut elements
                    .into_iter()
                    .map(|element| self.decided_verdict(element_type, element, 0)),
            );
        }
        if symbol_named(head, "list") && items.len() <= 2 {
            match value {
                Value::Nil | Value::Pair(_) => {}
                // A lazy range or seq-view is a list to `list?`; realising one is the walk's.
                Value::Range(_) | Value::SeqView(_) => return None,
                _ => return Some(false),
            }
            let Some(&element_type) = items.get(1) else {
                return Some(true);
            };
            let elements = self.list_to_vec(value).ok()?;
            return all_of(
                &mut elements
                    .into_iter()
                    .map(|element| self.decided_verdict(element_type, element, 0)),
            );
        }
        None
    }

    /// Remember `verdict` for `(type_expression, value)` under the current stamp. A pair not
    /// worth keeping is ignored. A check performs no definition, so the stamp a walk ends
    /// under is the one it ran under.
    pub(crate) fn record_type_verdict(&self, type_expression: Value, value: Value, verdict: bool) {
        let (Some(type_key), Some(value_key)) =
            (type_identity(type_expression), value_identity(value))
        else {
            return;
        };
        let mut cache = self.synced_verdict_cache();
        if cache.verdicts.len() >= VERDICT_CAPACITY {
            cache.start_over();
        }
        cache.verdicts.insert((type_key, value_key), verdict);
    }

    /// Whether some record id `*record-ids*` holds is named `type_name` as a `sig` spells
    /// it — bare (`point`, compared with the record's registered NAME) or qualified
    /// (`geo/point`, the id's trailing path, or the whole of it) — exactly
    /// `%record-id-named?` over every id. Remembered per name under the verdict stamp, so
    /// a type name that is neither base nor alias costs one lookup per check instead of a
    /// walk of the registry.
    pub(crate) fn type_names_record(&self, type_name: Symbol) -> bool {
        if let Some(&known) = self.synced_verdict_cache().names_record.get(&type_name) {
            return known;
        }
        let name = crate::core::value::symbol_name_ref(type_name);
        let records = match self.env_get(
            self.global(),
            crate::core::value::intern(crate::core::registries::RECORD_IDS),
        ) {
            Some(Value::Map(id)) => self.map_entries(id),
            _ => Vec::new(),
        };
        let qualified_suffix = format!("/{name}");
        let names = records.iter().any(|&(id, registered)| {
            if !name.contains('/') {
                return matches!(registered, Value::Sym(symbol) if symbol == type_name);
            }
            let Value::Keyword(id) = id else {
                return false;
            };
            let id_name = crate::core::value::symbol_name_ref(id);
            id_name == name || id_name.ends_with(&qualified_suffix)
        });
        self.synced_verdict_cache()
            .names_record
            .insert(type_name, names);
        names
    }

    /// The memo's counters since this heap was made.
    pub(crate) fn type_verdict_counts(&self) -> VerdictCounts {
        match self.type_verdicts.get() {
            Some(cache) => {
                let cache = cache.borrow();
                VerdictCounts {
                    hits: cache.hits,
                    misses: cache.misses,
                    entries: cache.verdicts.len() as u64,
                    clears: cache.clears,
                    asks: cache.asks,
                    vouched: cache.vouched,
                }
            }
            None => VerdictCounts::default(),
        }
    }

    /// Count one contracted call's arguments put to the kernel, and whether it vouched for
    /// them all — what tells a test (and a profile) how often the shim's own check ran.
    pub(crate) fn note_contract_ask(&self, vouched: bool) {
        let mut cache = self.verdict_cache().borrow_mut();
        cache.asks += 1;
        cache.vouched += u64::from(vouched);
    }

    /// Drop every verdict that names a nursery object: after a minor collection (which
    /// moves the survivors and re-mints their handles) and a `reset_local_to` (which
    /// truncates the nursery, so its slots are minted again).
    pub(super) fn type_verdicts_forget_nursery(&mut self) {
        if let Some(cache) = self.type_verdicts.get_mut() {
            cache.get_mut().verdicts.retain(|(type_key, value_key), _| {
                !type_key.in_nursery() && !value_key.in_nursery()
            });
        }
    }

    /// Drop every verdict that names a LOCAL object: after a major collection, which
    /// compacts the old generation.
    pub(super) fn type_verdicts_forget_local(&mut self) {
        if let Some(cache) = self.type_verdicts.get_mut() {
            cache
                .get_mut()
                .verdicts
                .retain(|(type_key, value_key), _| !type_key.in_local() && !value_key.in_local());
        }
    }
}
