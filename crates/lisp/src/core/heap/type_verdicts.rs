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
//! **What is decided here, not remembered.** Beside the table, [`Heap::decide`] reads the
//! type grammar natively (ADR-773): base names and literals by the value's tag, `?A`, an
//! alias by its expansion, a `defrecord` name or a sealed ability nominally, and every
//! compound form but `rec` — an arrow, `or`/`and`/`not`, `is`, `(int lo hi)`, `len`,
//! `list`, `vector`, `set`, `seqable`, `map`, `record` and `tuple` — descending into the value as
//! the walk does and remembering each compound part it decides. A form is read by its head
//! symbol's interned id, with no list built, and what a NAME stands for is remembered per
//! name under the stamp. Left to `%type-match`, the Brood walk and the one full reading of
//! the grammar: `rec`, an int or string literal type, a string's `len`, a record instance's
//! absent field when its record implements `Lookup`, and anything nested past `DECIDE_DEPTH`.

use super::*;
use crate::core::value::{AGE_OLD, REGION_SHIFT};

/// The most verdicts one heap keeps before starting over. An editor's model with a few
/// dozen buffers and a few thousand diagnostics holds well under this; the bound exists
/// for the program that checks a stream of distinct values and would otherwise grow the
/// table without end.
pub(crate) const VERDICT_CAPACITY: usize = 1 << 16;

/// How deep [`Heap::decide`] follows a type into a value — alias expansions, `or`/`and`/`not`
/// alternatives and descents into elements and fields together. Real types and values nest a
/// few levels; a cyclic alias, or a value deeper than this, is cut here and left to the walk.
const DECIDE_DEPTH: u32 = 24;

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
    /// Per type NAME that is not a base name: what it names ([`NameKind`]) — an alias, a
    /// type variable, a sealed ability, or a nominal name (ADR-773).
    name_kinds: HashMap<Symbol, NameKind, std::hash::BuildHasherDefault<IdentityHasher>>,
    /// Per `(record id, type name)`: `%record-id-named?`'s answer.
    ids_named: HashMap<(Symbol, Symbol), bool, std::hash::BuildHasherDefault<IdentityHasher>>,
    /// Per record id: whether it implements `Lookup` (`get` may answer a missing field).
    lookups: HashMap<Symbol, bool, std::hash::BuildHasherDefault<IdentityHasher>>,
    hits: u64,
    asks: u64,
    vouched: u64,
    /// Contracted calls the dispatcher entered without the shim's own frame (ADR-748).
    entered: u64,
    misses: u64,
    clears: u64,
}

/// What `(%type-verdict-counts)` reports: memo hits and misses, verdicts held, times the
/// table started over, and contracted calls whose arguments were put to the kernel
/// (`asks`) and vouched for without the Brood check (`vouched`), and of those, the calls
/// the dispatcher entered without running the shim at all (`entered`, ADR-748).
#[derive(Default)]
pub(crate) struct VerdictCounts {
    pub hits: u64,
    pub misses: u64,
    pub entries: u64,
    pub clears: u64,
    pub asks: u64,
    pub vouched: u64,
    pub entered: u64,
}

impl TypeVerdictCache {
    fn start_over(&mut self) {
        if !self.verdicts.is_empty() {
            self.verdicts.clear();
            self.clears += 1;
        }
        self.names_record.clear();
        self.name_kinds.clear();
        self.ids_named.clear();
        self.lookups.clear();
    }
}

/// What a type name that is not a base name stands for, as `%type-match` reads it — decided
/// once per name under the verdict stamp, since every part of the answer (the alias store,
/// `*sealed*`, `*record-ids*`, the namespace a bare name resolves in) moves the stamp.
#[derive(Clone)]
enum NameKind {
    /// A `deftype` alias: the verdict is its expansion's.
    Alias,
    /// A type variable (`?A`): a static-only constraint, accepted.
    Variable,
    /// A sealed ability: the closed set of its members' record ids.
    Sealed(Vec<Symbol>),
    /// Anything else — a `defrecord` name or a name nothing defines. `names_record`: some
    /// registered record is called this, so a value that is not one such record fails.
    Nominal { names_record: bool },
}

/// The head symbols [`Heap::decide`] reads a form by, interned once.
struct FormHeads {
    arrow: Symbol,
    or: Symbol,
    and: Symbol,
    not: Symbol,
    is: Symbol,
    int: Symbol,
    len: Symbol,
    open_end: Symbol,
    list: Symbol,
    vector: Symbol,
    tuple: Symbol,
    set: Symbol,
    seqable: Symbol,
    map: Symbol,
    record: Symbol,
    open: Symbol,
    optional: Symbol,
    lookup: Symbol,
    lookup_get: Symbol,
}

fn form_heads() -> &'static FormHeads {
    use crate::core::value::intern;
    static HEADS: std::sync::OnceLock<FormHeads> = std::sync::OnceLock::new();
    HEADS.get_or_init(|| FormHeads {
        arrow: intern("->"),
        or: intern("or"),
        and: intern("and"),
        not: intern("not"),
        is: intern("is"),
        int: intern("int"),
        len: intern("len"),
        open_end: intern("_"),
        list: intern("list"),
        vector: intern("vector"),
        tuple: intern("tuple"),
        set: intern("set"),
        seqable: intern("seqable"),
        map: intern("map"),
        record: intern("record"),
        open: intern("&open"),
        optional: intern("optional"),
        lookup: intern("Lookup"),
        lookup_get: intern("lookup-get"),
    })
}

/// Part verdicts folded the way `and` folds them: one failure decides, else any undecided
/// part leaves the whole undecided.
fn all_of(parts: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    let mut undecided = false;
    for part in parts {
        match part {
            Some(false) => return Some(false),
            Some(true) => {}
            None => undecided = true,
        }
    }
    if undecided { None } else { Some(true) }
}

/// A positional shape — `(tuple A B)`, `(list A B …)` — against `elements`: exact arity,
/// then each position against its own type.
fn positional(heap: &Heap, types: &[Value], elements: &[Value], depth: u32) -> Option<bool> {
    if types.len() != elements.len() {
        return Some(false);
    }
    all_of(
        types
            .iter()
            .zip(elements)
            .map(|(&type_expression, &element)| heap.decide(type_expression, element, depth)),
    )
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

/// Whether `name` is a BASE type name — one `%type-match` answers by tag before any alias
/// or record lookup, so no module can rebind it (`builtins::type_scope` leaves it alone).
pub(crate) fn is_base_type_name(name: Symbol) -> bool {
    base_type_verdict(Value::symbol(name), Value::nil()).is_some()
}

/// The verdict a BASE type gives `value` with no walk and no memo: a base name is a set of
/// runtime tags — the tags `type-of` reports, which every Brood type predicate reads — so
/// it is answered from the value's tag alone. `%type-match` answers these names the same
/// way and in the same place (before any alias lookup), so a `deftype` cannot rebind one;
/// this is that table moved to where a record's scalar fields reach it in one call. A
/// literal keyword or bool type is the value itself. Anything else is `None`: the walk's.
fn base_type_verdict(type_expression: Value, value: Value) -> Option<bool> {
    let symbol = match type_expression {
        Value::Sym(symbol) => symbol,
        Value::Keyword(keyword) => return Some(matches!(value, Value::Keyword(k) if k == keyword)),
        Value::Bool(literal) => return Some(matches!(value, Value::Bool(b) if b == literal)),
        _ => return None,
    };
    let tags = *base_type_tags().get(&symbol)?;
    Some(tags & (1 << crate::core::value::tag(value) as u32) != 0)
}

/// Every base type name as the SET of runtime tags it admits, one bit per [`Tag`] — read
/// once per process (ADR-748). A contract asks this per argument and per result of every
/// call, and matching the name's spelling against thirty strings each time was a third of
/// what a vouched-for check cost; a base name's meaning never changes (no `deftype` can
/// rebind one), so it is decided once, here, and a check is one lookup and a bit test.
///
/// [`Tag`]: crate::core::value::Tag
fn base_type_tags() -> &'static HashMap<Symbol, u32, std::hash::BuildHasherDefault<IdentityHasher>>
{
    use crate::core::value::Tag;
    static TABLE: std::sync::OnceLock<
        HashMap<Symbol, u32, std::hash::BuildHasherDefault<IdentityHasher>>,
    > = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let set = |tags: &[Tag]| tags.iter().fold(0u32, |set, &tag| set | (1 << tag as u32));
        let names: [(&str, u32); 29] = [
            ("any", u32::MAX),
            ("_", u32::MAX),
            ("never", 0),
            ("nil", set(&[Tag::Nil])),
            ("int", set(&[Tag::Int])),
            ("float", set(&[Tag::Float])),
            ("decimal", set(&[Tag::Decimal])),
            ("ratio", set(&[Tag::Ratio])),
            (
                "number",
                set(&[Tag::Int, Tag::Float, Tag::Decimal, Tag::Ratio]),
            ),
            ("string", set(&[Tag::Str])),
            ("bool", set(&[Tag::Bool])),
            ("keyword", set(&[Tag::Keyword])),
            ("symbol", set(&[Tag::Sym])),
            ("pair", set(&[Tag::Pair])),
            ("list", set(&[Tag::Nil, Tag::Pair])),
            ("vector", set(&[Tag::Vector])),
            ("map", set(&[Tag::Map])),
            ("set", set(&[Tag::Set])),
            ("bytes", set(&[Tag::Bytes])),
            ("fn", set(&[Tag::Fn, Tag::Native])),
            (
                "seqable",
                set(&[
                    Tag::Nil,
                    Tag::Pair,
                    Tag::Vector,
                    Tag::Set,
                    Tag::Map,
                    Tag::Bytes,
                ]),
            ),
            (
                "countable",
                set(&[
                    Tag::Nil,
                    Tag::Pair,
                    Tag::Vector,
                    Tag::Set,
                    Tag::Map,
                    Tag::Bytes,
                    Tag::Str,
                    Tag::Rope,
                    Tag::Table,
                ]),
            ),
            ("failure", set(&[Tag::Failure])),
            ("rope", set(&[Tag::Rope])),
            ("pid", set(&[Tag::Pid])),
            ("ref", set(&[Tag::Ref])),
            ("table", set(&[Tag::Table])),
            ("socket", set(&[Tag::Socket])),
            ("subprocess", set(&[Tag::Subprocess])),
        ];
        names
            .iter()
            .map(|&(name, tags)| (crate::core::value::intern(name), tags))
            .collect()
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

    /// Whether `value` inhabits `type_expression`, when the kernel can say without the
    /// Brood walk — see [`Self::decide`] — else whether the pair is worth remembering once
    /// the walk has answered it.
    pub(crate) fn type_verdict(&self, type_expression: Value, value: Value) -> KnownVerdict {
        if let Some(verdict) = self.decide(type_expression, value, 0) {
            return KnownVerdict::Known(verdict);
        }
        match (type_identity(type_expression), value_identity(value)) {
            (Some(_), Some(_)) => KnownVerdict::Unknown,
            _ => KnownVerdict::Unkept,
        }
    }

    /// `type_expression` against `value`, decided natively (ADR-773) — or `None`, which
    /// leaves the question to `%type-match`, the Brood walk and the one full reading of the
    /// grammar. Mirrors `%type-match` case for case and must stay in step with it:
    ///
    /// - a base name or a literal keyword/bool by the value's tag, `nil` by identity;
    /// - a remembered verdict for a compound value (the memo, by identity);
    /// - a symbol by what it names ([`Self::symbol_verdict`]): an alias by its expansion, a
    ///   type variable accepts, a `defrecord` name or a sealed ability nominally;
    /// - a form by its head symbol, compared as an interned id — an arrow, `or`, `and`,
    ///   `not`, `is`, `(int lo hi)`, `len`, `list` (uniform or positional), `vector`, `set`,
    ///   `seqable`, `map`, `record` and `tuple` — descending into the value's parts the way
    ///   the walk does, so a whole model is decided natively, each newly decided compound
    ///   part remembered on the way.
    ///
    /// Left to the walk: `rec`, an int or string literal type, a string's `len`, an unknown head, a lazy
    /// range or seq-view where a list is asked for, a record instance's absent field when its
    /// record implements `Lookup`, and anything past [`DECIDE_DEPTH`] (a cyclic alias among
    /// them — the walk's `seen` answers that).
    pub(crate) fn decide(&self, type_expression: Value, value: Value, depth: u32) -> Option<bool> {
        if depth > DECIDE_DEPTH {
            return None;
        }
        if matches!(type_expression, Value::Nil) {
            return Some(matches!(value, Value::Nil));
        }
        if let Some(verdict) = base_type_verdict(type_expression, value) {
            return Some(verdict);
        }
        let keys = match (type_identity(type_expression), value_identity(value)) {
            (Some(type_key), Some(value_key)) => {
                let mut cache = self.synced_verdict_cache();
                if let Some(verdict) = cache.verdicts.get(&(type_key, value_key)).copied() {
                    cache.hits += 1;
                    return Some(verdict);
                }
                cache.misses += 1;
                true
            }
            _ => false,
        };
        let verdict = match type_expression {
            Value::Sym(symbol) => self.symbol_verdict(symbol, value, depth),
            Value::Pair(_) => self.form_verdict(type_expression, value, depth),
            _ => None,
        }?;
        if keys {
            self.record_type_verdict(type_expression, value, verdict);
        }
        Some(verdict)
    }

    /// A type NAME that is not a base name: what `%type-match` makes of a symbol once the
    /// base names are past — an alias stands for its expansion; otherwise
    /// `%nominal-type-matches?`: a type variable (`?A`) accepts, a sealed ability admits its
    /// member identities, a `defrecord` name admits a record of that id, a name some record
    /// carries refuses anything else, and a name nothing defines accepts. What a name IS is
    /// remembered per name under the verdict stamp ([`NameKind`]), so a record-typed
    /// argument costs a lookup and an id comparison, not the walk's string work.
    fn symbol_verdict(&self, symbol: Symbol, value: Value, depth: u32) -> Option<bool> {
        match self.name_kind(symbol) {
            NameKind::Alias => {
                let expansion = crate::builtins::alias_expansion(self, symbol)?;
                self.decide(expansion, value, depth + 1)
            }
            NameKind::Variable => Some(true),
            // A sealed ability over a value that is no record: its `type-of` identity is the
            // walk's to compare.
            NameKind::Sealed(members) => match value {
                Value::Record { id, .. } => Some(members.contains(&id)),
                _ => None,
            },
            NameKind::Nominal { names_record } => {
                if let Value::Record { id, .. } = value
                    && self.record_id_named(id, symbol)
                {
                    return Some(true);
                }
                Some(!names_record)
            }
        }
    }

    /// What the type name `symbol` names, remembered under the verdict stamp.
    fn name_kind(&self, symbol: Symbol) -> NameKind {
        if let Some(kind) = self.synced_verdict_cache().name_kinds.get(&symbol) {
            return kind.clone();
        }
        // `%type-alias` answers nil for no alias, so an alias OF nil reads as no alias there.
        let kind = if crate::builtins::alias_expansion(self, symbol)
            .is_some_and(|expansion| !matches!(expansion, Value::Nil))
        {
            NameKind::Alias
        } else if crate::core::value::symbol_name_ref(symbol).starts_with('?') {
            NameKind::Variable
        } else if let Some(members) = self.sealed_members(symbol) {
            NameKind::Sealed(members)
        } else {
            NameKind::Nominal {
                names_record: self.type_names_record(symbol),
            }
        };
        self.synced_verdict_cache()
            .name_kinds
            .insert(symbol, kind.clone());
        kind
    }

    /// The sealed member ids of the ability `symbol` names, looked up as written and by its
    /// last segment — `%nominal-type-matches?`'s `(or (%sealed-members t) (%sealed-members
    /// leaf))`. A member that is not a keyword id is kept out; one would never equal a
    /// record's id.
    fn sealed_members(&self, symbol: Symbol) -> Option<Vec<Symbol>> {
        let sealed = match self.env_get(
            EnvId::GLOBAL,
            crate::core::value::intern(crate::core::registries::SEALED),
        ) {
            Some(Value::Map(id)) => id,
            _ => return None,
        };
        let name = crate::core::value::symbol_name_ref(symbol);
        let leaf = name
            .rfind('/')
            .map(|slash| crate::core::value::intern(&name[slash + 1..]))
            .unwrap_or(symbol);
        let members = [symbol, leaf].into_iter().find_map(|key| {
            self.map_get(sealed, Value::Sym(key))
                .filter(|m| !matches!(m, Value::Nil))
        })?;
        let items = match members {
            Value::Vector(id) => self.vector(id).to_vec(),
            list => self.list_to_vec(list).ok()?,
        };
        Some(
            items
                .into_iter()
                .filter_map(|item| match item {
                    Value::Keyword(id) => Some(id),
                    _ => None,
                })
                .collect(),
        )
    }

    /// `%record-id-named?`: does the record id `id` name the type `type_name` as a `sig`
    /// spells it — bare (the record's registered NAME) or qualified (the id's trailing
    /// path, or the whole of it). Remembered per pair under the verdict stamp.
    fn record_id_named(&self, id: Symbol, type_name: Symbol) -> bool {
        if let Some(&named) = self.synced_verdict_cache().ids_named.get(&(id, type_name)) {
            return named;
        }
        let name = crate::core::value::symbol_name_ref(type_name);
        let named = if name.contains('/') {
            let id_name = crate::core::value::symbol_name_ref(id);
            id_name == name || id_name.ends_with(&format!("/{name}"))
        } else {
            match self.env_get(
                EnvId::GLOBAL,
                crate::core::value::intern(crate::core::registries::RECORD_IDS),
            ) {
                Some(Value::Map(records)) => matches!(
                    self.map_get(records, Value::Keyword(id)),
                    Some(Value::Sym(registered)) if registered == type_name
                ),
                _ => false,
            }
        };
        self.synced_verdict_cache()
            .ids_named
            .insert((id, type_name), named);
        named
    }

    /// Whether the record id `id` implements `Lookup` — so a field its map lacks may still
    /// answer through `lookup-get` (`get`'s `%lookup-miss`), which only the walk can call.
    /// Remembered per id under the verdict stamp (an `impl` rebinds `*impls*`).
    fn record_has_lookup(&self, id: Symbol) -> bool {
        if let Some(&has) = self.synced_verdict_cache().lookups.get(&id) {
            return has;
        }
        let has = match self.env_get(
            EnvId::GLOBAL,
            crate::core::value::intern(crate::core::registries::IMPLS),
        ) {
            // `*impls*` is keyed by `[ability op]`; a map lookup compares the vector by value.
            Some(Value::Map(impls)) => self.map_get_by_op_key(impls, id),
            // Unbound (`%lookup-miss`'s `bound?` guard): no record answers through it.
            Some(_) => false,
            None => false,
        };
        self.synced_verdict_cache().lookups.insert(id, has);
        has
    }

    /// `*impls*[[Lookup lookup-get]]` holds an entry for `id`. The key vector is found by
    /// scanning the (few) keys rather than allocating one to look it up, since this runs
    /// from a `&self` reader.
    fn map_get_by_op_key(&self, impls: MapId, id: Symbol) -> bool {
        let names = form_heads();
        let methods = self.map_entries(impls).into_iter().find_map(|(key, methods)| {
            let Value::Vector(key_id) = key else {
                return None;
            };
            matches!(
                self.vector(key_id),
                [Value::Sym(ability), Value::Sym(op)] if *ability == names.lookup && *op == names.lookup_get
            )
            .then_some(methods)
        });
        match methods {
            Some(Value::Map(methods)) => self
                .map_get(methods, Value::Keyword(id))
                .is_some_and(|method| !matches!(method, Value::Nil)),
            _ => false,
        }
    }

    /// A compound type form against `value`, read by its head symbol's interned id — no list
    /// is built and no spelling compared. An arrow anywhere in the form is checked first, as
    /// `%type-match` checks it before the head (KI-113).
    fn form_verdict(&self, form: Value, value: Value, depth: u32) -> Option<bool> {
        let names = form_heads();
        let Value::Pair(form_id) = form else {
            return None;
        };
        let (head, parts) = self.pair(form_id);
        if self.list_any(
            form,
            |item| matches!(item, Value::Sym(symbol) if symbol == names.arrow),
        ) {
            return Some(matches!(
                crate::core::value::tag(value),
                crate::core::value::Tag::Fn | crate::core::value::Tag::Native
            ));
        }
        let Value::Sym(head) = head else {
            return None;
        };
        let next = depth + 1;
        if head == names.or {
            let mut undecided = false;
            for alternative in self.type_items(parts) {
                match self.decide(alternative, value, next) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => undecided = true,
                }
            }
            return if undecided { None } else { Some(false) };
        }
        if head == names.and {
            return all_of(
                self.type_items(parts)
                    .map(|part| self.decide(part, value, next)),
            );
        }
        if head == names.not {
            let mut undecided = false;
            for part in self.type_items(parts) {
                match self.decide(part, value, next) {
                    Some(true) => return Some(false),
                    Some(false) => {}
                    None => undecided = true,
                }
            }
            return if undecided { None } else { Some(true) };
        }
        if head == names.is {
            return Some(matches!(value, Value::Bool(_)));
        }
        let first = self.nth_item(parts, 0);
        if head == names.int {
            let number = match value {
                Value::Int(number) => number,
                // A big integer is an `int?` outside every i64 bound; the walk compares it.
                Value::BigInt(_) => return None,
                _ => return Some(false),
            };
            let bound = |end: Value, admits: fn(i64, i64) -> bool| match end {
                Value::Sym(symbol) if symbol == names.open_end => Some(true),
                Value::Int(limit) => Some(admits(limit, number)),
                _ => None,
            };
            return Some(
                bound(first, |low, n| low <= n)?
                    && bound(self.nth_item(parts, 1), |high, n| n <= high)?,
            );
        }
        if head == names.len {
            // `(len T lo hi)`: a countable `T` whose count lies in the interval — `nil`
            // counts 0. A string's or bytes' count is the walk's (`count` of a string is its
            // characters); a non-countable value never fits.
            let count = match value {
                Value::Nil => 0,
                Value::Pair(_) => self.list_to_vec(value).ok()?.len(),
                Value::Vector(vector_id) => self.vector(vector_id).len(),
                Value::Map(map_id) | Value::Set(map_id) => self.map_size(map_id),
                Value::Str(_) | Value::Bytes(_) | Value::Range(_) | Value::SeqView(_) => {
                    return None;
                }
                _ => return Some(false),
            };
            let count = i64::try_from(count).ok()?;
            let bound = |end: Value, admits: fn(i64, i64) -> bool| match end {
                Value::Sym(symbol) if symbol == names.open_end => Some(true),
                Value::Int(limit) => Some(admits(limit, count)),
                _ => None,
            };
            if !(bound(self.nth_item(parts, 1), |low, n| low <= n)?
                && bound(self.nth_item(parts, 2), |high, n| n <= high)?)
            {
                return Some(false);
            }
            return self.decide(first, value, next);
        }
        if head == names.list {
            let elements = match value {
                Value::Nil | Value::Pair(_) => self.list_to_vec(value).ok()?,
                // A lazy range or seq-view is a list to `list?`; realising one is the walk's.
                Value::Range(_) | Value::SeqView(_) => return None,
                _ => return Some(false),
            };
            let types: Vec<Value> = self.type_items(parts).collect();
            // No element type, or a nil one, admits any list: the walk asks `(nil? elem)` first.
            return match types.len() {
                _ if matches!(types.first(), None | Some(Value::Nil)) => Some(true),
                1 => all_of(
                    elements
                        .into_iter()
                        .map(|element| self.decide(types[0], element, next)),
                ),
                _ => positional(self, &types, &elements, next),
            };
        }
        if head == names.vector {
            let Value::Vector(vector_id) = value else {
                return Some(false);
            };
            if matches!(first, Value::Nil) {
                return Some(true);
            }
            let elements: Vec<Value> = self.vector(vector_id).to_vec();
            return all_of(
                elements
                    .into_iter()
                    .map(|element| self.decide(first, element, next)),
            );
        }
        if head == names.tuple {
            let Value::Vector(vector_id) = value else {
                return Some(false);
            };
            let types: Vec<Value> = self.type_items(parts).collect();
            let elements: Vec<Value> = self.vector(vector_id).to_vec();
            return positional(self, &types, &elements, next);
        }
        if head == names.set || head == names.seqable {
            let elements = match value {
                Value::Set(set_id) => self.set_elems(set_id),
                Value::Vector(vector_id) if head == names.seqable => {
                    self.vector(vector_id).to_vec()
                }
                Value::Nil | Value::Pair(_) if head == names.seqable => {
                    self.list_to_vec(value).ok()?
                }
                Value::Range(_) | Value::SeqView(_) if head == names.seqable => return None,
                _ => return Some(false),
            };
            if matches!(first, Value::Nil) {
                return Some(true);
            }
            return all_of(
                elements
                    .into_iter()
                    .map(|element| self.decide(first, element, next)),
            );
        }
        if head == names.map {
            // A record is its own kind (ADR-542): `(map K V)` never admits one.
            let Value::Map(map_id) = value else {
                return Some(false);
            };
            let value_type = self.nth_item(parts, 1);
            if matches!((first, value_type), (Value::Nil, Value::Nil)) {
                return Some(true);
            }
            // A missing half is the `nil` type, as the walk reads `(first (rest …))` of it.
            return all_of(
                self.map_entries(map_id)
                    .into_iter()
                    .flat_map(|(key, entry)| {
                        [
                            self.decide(first, key, next),
                            self.decide(value_type, entry, next),
                        ]
                    }),
            );
        }
        if head == names.record {
            return self.record_verdict(parts, value, next);
        }
        None
    }

    /// `(record [&open] k1 T1 k2 T2 …)` against `value` — `%type-match`'s record arm. A field
    /// typed `(optional T)` may be absent or nil; a closed record admits only its keys; a
    /// shape carrying the `:__id__` pseudo-field (a `defrecord` constructor's result, ADR-542)
    /// admits only a record of that id. A record INSTANCE's field is read as `get` reads it:
    /// its field map's entry, and on a miss (or nil) its `Lookup` — decided here only when its
    /// record implements none, so the miss is nil.
    fn record_verdict(&self, parts: Value, value: Value, depth: u32) -> Option<bool> {
        let names = form_heads();
        let (map_id, instance) = match value {
            Value::Map(map_id) => (map_id, None),
            Value::Record { id, fields } => (fields, Some(id)),
            _ => return Some(false),
        };
        let open = matches!(self.nth_item(parts, 0), Value::Sym(symbol) if symbol == names.open);
        let items: Vec<Value> = self.type_items(parts).skip(usize::from(open)).collect();
        // `key type` pairs; a dangling odd key is ignored, as the walk ignores it.
        let (all_fields, _) = items.as_chunks::<2>();
        let id_key = *crate::core::value::RECORD_ID_SYMBOL;
        let is_id = |field: &[Value; 2]| matches!(field[0], Value::Keyword(key) if key == id_key);
        if let Some(field) = all_fields.iter().find(|field| is_id(field)) {
            let Value::Keyword(wanted) = field[1] else {
                return None;
            };
            if instance != Some(wanted) {
                return Some(false);
            }
        }
        let fields: Vec<[Value; 2]> = all_fields
            .iter()
            .filter(|field| !is_id(field))
            .copied()
            .collect();
        // Keyword field names only: the closed check compares names by identity, as
        // `%record-declares?` does with `%eq`, which is exact only for interned keys.
        if fields
            .iter()
            .any(|field| !matches!(field[0], Value::Keyword(_)))
        {
            return None;
        }
        let misses_are_nil = instance.is_none_or(|id| !self.record_has_lookup(id));
        let field_verdicts = fields.iter().map(|field| {
            let field_value = self.map_get(map_id, field[0]).unwrap_or(Value::Nil);
            if !misses_are_nil && matches!(field_value, Value::Nil) {
                return None;
            }
            let field_type = field[1];
            if let Value::Pair(id) = field_type {
                let (wrapper, tail) = self.pair(id);
                if matches!(wrapper, Value::Sym(symbol) if symbol == names.optional) {
                    if matches!(field_value, Value::Nil) {
                        return Some(true);
                    }
                    return self.decide(self.nth_item(tail, 0), field_value, depth);
                }
            }
            self.decide(field_type, field_value, depth)
        });
        let fields_pass = all_of(field_verdicts.collect::<Vec<_>>().into_iter());
        if fields_pass != Some(true) || open {
            return fields_pass;
        }
        let declared = |key: Value| matches!(key, Value::Keyword(key) if fields.iter().any(|field| matches!(field[0], Value::Keyword(name) if name == key)));
        Some(
            self.map_entries(map_id)
                .into_iter()
                .all(|(key, _)| declared(key)),
        )
    }

    /// The items of the proper list `list`, in order — stops at its end or at an improper tail.
    fn type_items(&self, list: Value) -> impl Iterator<Item = Value> + '_ {
        let mut rest = list;
        std::iter::from_fn(move || match rest {
            Value::Pair(id) => {
                let (item, tail) = self.pair(id);
                rest = tail;
                Some(item)
            }
            _ => None,
        })
    }

    /// The `index`th item of the list `list`, or nil past its end.
    fn nth_item(&self, list: Value, index: usize) -> Value {
        self.type_items(list).nth(index).unwrap_or(Value::Nil)
    }

    /// Whether any item of the list `list` satisfies `test`.
    fn list_any(&self, list: Value, test: impl Fn(Value) -> bool) -> bool {
        self.type_items(list).any(test)
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
            EnvId::GLOBAL,
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
                    entered: cache.entered,
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

    /// Count one contracted call the dispatcher entered itself (ADR-748): its arguments
    /// vouched for, its original called in the shim's place.
    pub(crate) fn note_contract_entered(&self) {
        let mut cache = self.verdict_cache().borrow_mut();
        cache.asks += 1;
        cache.vouched += 1;
        cache.entered += 1;
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
