//! `(sig name (params… -> ret))` type annotations — the parser from a Brood
//! type-expression *form* to a [`Ty`]/[`Sig`], plus the recogniser that pulls a
//! declaration out of a top-level form. See `docs/type-annotations.md`.
//!
//! Slice 1 is **checker-facing only**: a declared `Sig` is read by the checker as
//! an authoritative signature source (ahead of primitive / curated / inferred).
//! The `sig` form is a runtime no-op (a prelude macro expanding to `nil`), so the
//! scan runs over the *un-expanded* forms — like the hygiene lint. Nothing here
//! enforces the declaration at run time yet; that is slice 2 (the strong arrow).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use crate::core::heap::Heap;
use crate::core::value::{self, Symbol, Tag, Value};
use crate::types::{Sig, Ty};

use super::ctx::{SigTerm, SigWithVars};
use super::walk::list_items;

thread_local! {
    /// Per-file table: an ability's name → its type resolution (ADR-181/186). `Some(members)`
    /// for a **sealed** ability — the closed, ns-qualified member id set that becomes the
    /// finite union of member record shapes; `None` for an **open** ability — no closed set,
    /// so it resolves to the permissive `any` (its safety is enforced at op call sites, not
    /// at the type). A name *absent* from the table isn't an ability at all. Populated at the
    /// start of each `check_file` (`protocol::ability_type_table`), cleared per file. Lets
    /// [`parse_type`] resolve any ability name in type position (`(sig f (Shape -> float))`,
    /// `:-> Shape`, `(sig g (Display -> string))`).
    static ABILITY_TYPES: RefCell<HashMap<String, Option<Vec<String>>>> =
        RefCell::new(HashMap::new());

    /// op-name (last `/` segment) → its sealed ability's member ids — the occurrence-typing
    /// domain (ADR-190). Only ops declared by exactly one ability, that ability SEALED and
    /// `:default`-free, so a `(area s)` use derives `s : Shape` soundly. Built per file from
    /// `AbilityInfo` (which sees this file's abilities, unlike the heap registries under
    /// `--check`) and installed by `set_sealed_op_domains`.
    static SEALED_OP_DOMAINS: RefCell<HashMap<String, Vec<String>>> =
        RefCell::new(HashMap::new());

    /// The record ids `defrecord` has registered (`protocol::record_id_names`). Consulted
    /// only to disambiguate an UNQUALIFIED sealed member that also spells a built-in kind —
    /// see [`sealed_members_ty`]. Populated and cleared with the tables above.
    static RECORD_IDS: RefCell<std::collections::HashSet<String>> =
        RefCell::new(std::collections::HashSet::new());
}

/// Install this file's registered-record-id set (call with the ability-type table).
pub(super) fn set_record_ids(ids: std::collections::HashSet<String>) {
    RECORD_IDS.with(|m| *m.borrow_mut() = ids);
}

thread_local! {
    /// THIS file's records' declared field types, by record id — read by [`record_ty`]
    /// ahead of the heap, because the file is checked before it is loaded, so its own
    /// constructors' sigs are not on the heap yet. Populated from the `%register-sig`
    /// forms `defrecord` expands to (`check_file`), cleared per file with the tables above.
    static RECORD_FIELD_TYPES: RefCell<HashMap<String, BTreeMap<Symbol, (Ty, bool)>>> =
        RefCell::new(HashMap::new());
}

/// Install this file's records' declared field types (see [`RECORD_FIELD_TYPES`]).
pub(super) fn set_record_field_types(map: HashMap<String, BTreeMap<Symbol, (Ty, bool)>>) {
    RECORD_FIELD_TYPES.with(|m| *m.borrow_mut() = map);
}

/// Install this file's ability-type table (call before parsing sigs). Overwrites any prior
/// file's table.
pub(super) fn set_ability_types(map: HashMap<String, Option<Vec<String>>>) {
    ABILITY_TYPES.with(|m| *m.borrow_mut() = map);
}

/// Drop the ability-type table — per-file hygiene, mirroring [`super::sigs::clear_sig_memo`].
pub(super) fn clear_ability_types() {
    ABILITY_TYPES.with(|m| m.borrow_mut().clear());
    RECORD_IDS.with(|m| m.borrow_mut().clear());
    RECORD_FIELD_TYPES.with(|m| m.borrow_mut().clear());
}

/// Install this file's sealed-op occurrence-typing domains (ADR-190). Overwrites the prior
/// file's (an empty map when nothing is eligible, so nothing leaks across files).
pub(super) fn set_sealed_op_domains(map: HashMap<String, Vec<String>>) {
    SEALED_OP_DOMAINS.with(|m| *m.borrow_mut() = map);
}

/// The member ids of the sealed ability an op-name anchors, if it is an eligible occurrence-
/// typing op (see `set_sealed_op_domains`), else `None`.
pub(super) fn sealed_op_members(op_name: &str) -> Option<Vec<String>> {
    SEALED_OP_DOMAINS.with(|m| m.borrow().get(op_name).cloned())
}

/// An ability `name` as a **type**. A **sealed** ability → the union of its members' record
/// shapes, each a `(record :__id__ :<member>)` (the nominal identity a `defrecord` value
/// carries); the `:__id__`-only shape is intentionally minimal — records are open, so a real
/// `(circle 2)` with extra fields is still a subtype, and the singleton `:__id__` is exactly
/// what nominal dispatch keys on. An **open** ability → `any`: it has no closed member set
/// and impls (incl. `:default`) may cover any value, so no argument can be soundly rejected
/// on the type — but naming it keeps the rest of a `sig` alive (its return, its other
/// params) instead of dropping the whole declaration. `None` when `name` isn't an ability.
fn ability_type(name: &str) -> Option<Ty> {
    // A **qualified** spelling names the same ability: the registry (`*abilities*`) is
    // keyed by the bare CamelCase name (ADR-255), one flat namespace — `(:use shapes)`
    // does not create a second `Shape`. So `shapes/Shape` in a `sig` resolves through
    // its last segment, which is the only part that identifies anything. The module
    // half cannot be *verified* for the same reason it is not needed: nothing records
    // which module declared an ability. Before this, a qualified spelling read as an
    // unknown type — silently widening the annotated position to `any` until ADR-259
    // started reporting it.
    let bare = name.rsplit('/').next().unwrap_or(name);
    ABILITY_TYPES.with(|m| {
        match m.borrow().get(bare)? {
            // Open ability: permissive — the type checks nothing, but the sig survives.
            None => Some(Ty::ANY),
            // Sealed ability: the finite union its member set denotes.
            // A sealed ability with no members is degenerate → permissive, not NEVER.
            Some(members) => Some(sealed_members_ty(members).unwrap_or(Ty::ANY)),
        }
    })
}

/// The type a **sealed** ability's member set denotes (ADR-181) — shared by the
/// ability-name-as-a-type resolution above and the occurrence-typing domain
/// (`protocol::sealed_op_domain`), which must agree or a value accepted by a `sig` is
/// rejected at the op call inside it.
///
/// Members come in two kinds, because `impl` dispatches on both:
///
/// - A **record** member is ns-qualified (`shapes/circle` — `%ability-id-kw` qualifies a bare
///   record symbol with the current ns) and denotes the open record shape whose `:__id__` is
///   that keyword. All of them collapse into ONE shape whose `:__id__` is the union of their
///   literals: `%{__id__: (:c | :r | …)}`. That is *equal as a set of values* to
///   `⋃ₘ %{__id__: :m}` precisely because each member shape is an OPEN record constraining
///   only `:__id__` — so the union is exactly "maps whose `:__id__` ∈ members". We build the
///   single shape directly rather than `Ty::union`-ing the member shapes, because `Ty::union`
///   widens a differing `fields` map away (a sound over-approximation → `map`, but it drops
///   the member set). Field-wise-merging arbitrary records in `Ty::union` would be UNSOUND (it
///   invents cross terms), so that stays as-is; only *this* union, where the shapes differ
///   solely in `:__id__`, collapses soundly to a lit-union field. The preserved `:__id__` lit
///   set is what drives both a precise non-member rejection and sealed-`match` exhaustiveness
///   (ADR-187 part 2).
/// - A **built-in kind** member (`(impl Numeric :int …)` — the numeric tower, `:string`,
///   `:vector`, …) is not a record at all, so it denotes its own lattice point via
///   [`base_ty`]. An int is an int; it is not a map carrying `:__id__ :int`.
///
/// A seal may **mix** the two. The record half then degrades to `map` in the union, because
/// `Ty::union` widens a differing `fields` map away — sound (it only ever accepts more), and
/// the alternative, field-wise-merging arbitrary records, would be unsound. So a mixed seal
/// trades `:__id__` precision for coverage; a purely-record seal (the common case) keeps it.
///
/// Treating every member as a record shape was the defect: sealing over kinds produced a
/// domain that **rejected its own members**. `(defability Sizey :sealed [:int :float] …)` with
/// both impls present warned that `(use-it 42)` "expects `{__id__: :float | :int, ...}`, got
/// 42" — a `nest check` false positive on a program that runs correctly. It read as working
/// because the two paths that *do* only need the id set were unaffected: the exhaustiveness
/// gate, and rejecting a non-member. Only passing a real member exposed it. Nothing in-tree is
/// kind-sealed, which is why ADR-181's own false-positive audit came back clean.
pub(super) fn sealed_members_ty(members: &[String]) -> Option<Ty> {
    let mut record_ids: Option<Ty> = None;
    let mut kind_ty: Option<Ty> = None;
    for member in members {
        // A member is a built-in kind when it is unqualified, `base_ty` knows the spelling,
        // and **no registered record claims that id**. The last clause is not paranoia: a
        // record declared at ROOT namespace registers under its bare name, so
        // `(defrecord ratio …)` outside any `defmodule` owns the id `:ratio` — the identical
        // dispatch key the built-in ratio kind uses. The language conflates them (a real
        // `1/2` reaches that record's impl), so the registry is the only thing that can say
        // which one a seal meant, and a record that exists wins. With the registry
        // unreadable we fall through to the kind, which is right for every member that is
        // one and wrong only for the root-namespace collision that made the id ambiguous.
        let is_kind = !member.contains('/')
            && !RECORD_IDS.with(|m| m.borrow().contains(member))
            && base_ty(member).is_some();
        match if is_kind { base_ty(member) } else { None } {
            Some(t) => {
                kind_ty = Some(match kind_ty {
                    Some(acc) => acc.union(t),
                    None => t,
                })
            }
            None => {
                let lit = Ty::keyword_lit(value::intern(member));
                record_ids = Some(match record_ids {
                    Some(acc) => acc.union(lit),
                    None => lit,
                });
            }
        }
    }
    let records = record_ids.map(|id| {
        let mut fields = BTreeMap::new();
        fields.insert(value::intern("__id__"), (id, true));
        // **Open** (ADR-264): a real `(circle 2)` carries `:radius` as well as `:__id__`, so a
        // shape that pinned the id and nothing else would, closed, describe no record at all.
        Ty::record_of_open(fields)
    });
    match (records, kind_ty) {
        (Some(r), Some(k)) => Some(r.union(k)),
        (Some(r), None) => Some(r),
        (None, Some(k)) => Some(k),
        (None, None) => None,
    }
}

/// A **record** name as a type: `(sig area (circle -> float))`, `:-> circle`.
///
/// A record is the language's nominal type, and `defrecord` already emits one in its own
/// constructor sig (`(any any -> (record :__id__ :ns/pt …))`) — but the *name* could not be
/// written in a type position, so the natural spelling warned "unknown type `pt`" about a
/// type the checker held in `*record-ids*` all along. Sealed abilities have resolved this way
/// since ADR-181; a record is the more obvious case.
///
/// The shape is the nominal `:__id__` singleton and nothing else, **open** — the same
/// denotation `ability_type` gives a sealed member, and for the same reason: a real
/// `(pt 1 2)` carries `:x`/`:y` beside `:__id__`, and `(assoc (pt 1 2) :z 3)` is still a
/// `pt`, so pinning the fields would describe no value anyone passes.
///
/// A **qualified** spelling (`shapes/circle`) hits the registry directly. A **bare** one is
/// how you name a record inside its own module, and is accepted only when exactly one
/// registered record ends in that segment: two modules may each define `pt`, and choosing
/// between them would produce a *wrong* type where declining produces a missing one. Sound,
/// not complete — the ADR-181 discipline.
fn record_ty(heap: &Heap, name: &str) -> Option<Ty> {
    let id = record_id_for(name)?;
    // The record's DECLARED field types travel with the name (2026-08-30). `defrecord` puts
    // them on the constructor's sig (`(sig pt (int int -> (record :__id__ :pt :x int …)))`),
    // and a `(sig f (pt -> …))` that resolved to the bare `:__id__` shape lost every one of
    // them at the parameter: `(get dt :hour 0)` on a `datetime` read as `any`, and a body
    // over its fields could not close over `int`. The shape stays OPEN for the reason given
    // above (`(assoc (pt 1 2) :z 3)` is still a `pt`); only the declared keys are typed. An
    // undeclared field is a `?var` on the constructor, which parses as `any` — the same
    // reading the open rest gives it, so nothing is narrowed that was not declared.
    let declared = RECORD_FIELD_TYPES
        .with(|m| m.borrow().get(&id).cloned())
        .or_else(|| {
            super::sigs::declared_heap_sig(heap, value::intern(&id))
                .and_then(|sig| sig.ret.record_fields().cloned())
        });
    let mut fields = declared.unwrap_or_default();
    fields.insert(
        value::intern("__id__"),
        (Ty::keyword_lit(value::intern(&id)), true),
    );
    Some(Ty::record_of_open(fields))
}

/// The registered record id `name` denotes — `name` itself when registered, else the one
/// registered id ending in `/name` (two candidates decline: see [`record_ty`]).
fn record_id_for(name: &str) -> Option<String> {
    RECORD_IDS.with(|m| {
        let ids = m.borrow();
        let id = if ids.contains(name) {
            name.to_string()
        } else {
            let suffix = format!("/{name}");
            let mut hits = ids.iter().filter(|id| id.ends_with(&suffix));
            let first = hits.next()?.clone();
            if hits.next().is_some() {
                return None; // ambiguous across modules — decline rather than guess
            }
            first
        };
        Some(id)
    })
}

/// The lattice point a base type *name* denotes — the spellings `type-of`
/// returns, plus the named unions (`number` = int∪float, `list` = nil∪pair,
/// `fn` = fn∪native). `None` for an unknown name, so an unrecognised annotation
/// is dropped rather than guessed (never a false signal).
fn base_ty(name: &str) -> Option<Ty> {
    Some(match name {
        "any" => Ty::ANY,
        "never" => Ty::NEVER,
        "int" => Ty::of(Tag::Int),
        "float" => Ty::of(Tag::Float),
        "number" => Ty::NUMBER,
        "string" => Ty::of(Tag::Str),
        "symbol" => Ty::of(Tag::Sym),
        "keyword" => Ty::of(Tag::Keyword),
        "bool" => Ty::of(Tag::Bool),
        "nil" => Ty::of(Tag::Nil),
        "pair" => Ty::of(Tag::Pair),
        "vector" => Ty::of(Tag::Vector),
        "list" => Ty::LIST,
        "map" => Ty::of(Tag::Map),
        "set" => Ty::of(Tag::Set),
        // The **seqable** union — every collection the sequence combinators (`fold`/`map`/
        // `filter`/`count`/`first`/…) walk: a list (`nil`/`pair` — a range/seqview reads as
        // `pair`), a vector, a set, a map (as `[k v]` pairs), or `bytes`. Deliberately
        // excludes `string` (not seqable — bridge with `string->list`). Lets a `sig` name a
        // polymorphic-sequence parameter precisely instead of falling back to `any`, so a
        // vector caller is no longer false-flagged by a `(list T)` annotation. Mirrors the
        // internal `seq` domain the curated combinator sigs already use.
        "seqable" => Ty::SEQABLE,
        "countable" => Ty::COUNTABLE,
        "numeric" | "ordered" => super::sigs::named_cover(None, name)?,
        // `bytes` and `decimal` are runtime tags (`type-of` returns them, and
        // `bytes?`/`decimal?` narrow to them) that had no *spelling* here — so no
        // signature could mention a bytes value, which is most of what
        // `std/encoding`, `std/hash`, and `std/net/tcp` take and return. Found by
        // annotating std for real (the `sig` adoption pilot, 2026-07-26): the
        // compatibility contract in `docs/types.md` says a Value kind needs a Tag
        // *and* a way to name it, and these had only the Tag.
        "bytes" => Ty::of(Tag::Bytes),
        "decimal" => Ty::of(Tag::Decimal),
        "ratio" => Ty::of(Tag::Ratio),
        "fn" => Ty::of(Tag::Fn).union(Ty::of(Tag::Native)),
        "rope" => Ty::of(Tag::Rope),
        "pid" => Ty::of(Tag::Pid),
        "ref" => Ty::of(Tag::Ref),
        "socket" => Ty::of(Tag::Socket),
        "subprocess" => Ty::of(Tag::Subprocess),
        "table" => Ty::of(Tag::Table),
        _ => return None,
    })
}

/// Parse a type-expression form to a [`Ty`]. Handles base names, type
/// variables (`?A` → `Ty::ANY`), arrows `(p… -> r)`, `(list E)` /
/// `(vector E)`, `(or A B …)`, `(and A B …)`, `(map K V)` (flat
/// `Ty::Map` in slice 1), `(record …)`, and `(tuple T1 T2 …)` (ADR-128,
/// a fixed-arity positional vector shape). `None` for anything unrecognised
/// — the annotation is then dropped, never guessed.
pub fn parse_type(heap: &Heap, form: Value) -> Option<Ty> {
    match form {
        Value::Sym(s) => {
            let name = value::symbol_name(s);
            // Type variables (`?A`, `?el`, etc.) — static-only, no runtime meaning.
            // Unknown to `type-matches?` → accepts everything (correct: it's a
            // static constraint, not a runtime one). Resolve to ANY here so the
            // checker uses the widest safe type at positions it can't unify.
            if name.starts_with('?') {
                return Some(Ty::ANY);
            }
            // A base type name wins; then a bare **sealed ability** name (ADR-181) — the
            // union of its members' record shapes — then a **record** name. An unknown
            // symbol still yields `None` (dropped, never guessed).
            //
            // Base types win the tie deliberately: in a *type expression* `int` means the
            // int kind, even where a root-namespace record has claimed the id `:int`. That
            // is the opposite precedence to `sealed_members_ty`, and for the opposite
            // reason — there the members are `impl` dispatch keys, here they are type
            // syntax.
            base_ty(&name)
                .or_else(|| ability_type(&name))
                .or_else(|| record_ty(heap, &name))
        }
        // `nil` reads as the literal `Value::Nil`, not a symbol — so a type-expr
        // like `(or int nil)` lands here, not in `base_ty`.
        Value::Nil => Some(Ty::of(Tag::Nil)),
        // A bare keyword in type position is a literal (singleton) type — exactly
        // that value. Unambiguous (base types are bare *symbols*), and the form
        // `(or :maximized :fullboth nil)` composes via the `(or …)` union above.
        Value::Keyword(s) => Some(Ty::keyword_lit(s)),
        // A bare int literal in type position, likewise (ADR-117) — no
        // ambiguity with a symbol-spelled base type either. `(or 200 404 500)`
        // composes the same way via the `(or …)` union. A `BigInt` (outside
        // `i64` range) isn't handled — falls through to `_ => None` below,
        // dropped rather than guessed.
        Value::Int(n) => Some(Ty::int_lit(n)),
        // Bare bool/string literals, same story (ADR-120).
        Value::Bool(b) => Some(Ty::bool_lit(b)),
        Value::Str(id) => Some(Ty::str_lit(&heap.string(id))),
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            // An arrow: a list containing the `->` marker. Detect it first, so
            // `(int -> int)` isn't mistaken for an `(int …)` application.
            if let Some(pos) = items.iter().position(|v| is_arrow_marker(*v)) {
                return parse_arrow(heap, &items, pos).map(Ty::arrow);
            }
            let Value::Sym(head) = *items.first()? else {
                return None;
            };
            // (list E) / (vector E) — element-typed sequences.
            if value::symbol_is(head, "list") && items.len() == 2 {
                return Some(Ty::list_of(parse_type(heap, items[1])?));
            }
            if value::symbol_is(head, "vector") && items.len() == 2 {
                return Some(Ty::vector_of(parse_type(heap, items[1])?));
            }
            if value::symbol_is(head, "set") && items.len() == 2 {
                return Some(Ty::set_of(parse_type(heap, items[1])?));
            }
            // (or A B …) — a union.
            if value::symbol_is(head, "or") && items.len() >= 2 {
                let mut acc: Option<Ty> = None;
                for &it in &items[1..] {
                    let t = parse_type(heap, it)?;
                    acc = Some(match acc {
                        Some(a) => a.union(t),
                        None => t,
                    });
                }
                return acc;
            }
            // (and A B …) — an intersection.  Ty::intersect is already
            // well-tested set intersection; no new Ty variant needed.
            // A bare (and) with no args is Ty::ANY (vacuously true).
            if value::symbol_is(head, "and") {
                if items.len() == 1 {
                    return Some(Ty::ANY);
                }
                let mut acc: Option<Ty> = None;
                for &it in &items[1..] {
                    let t = parse_type(heap, it)?;
                    acc = Some(match acc {
                        Some(a) => a.intersect(t),
                        None => t,
                    });
                }
                return acc;
            }
            // (not T) — the complement: every value that is NOT a `T`. The lattice
            // has had `negate`/`difference` since ADR-023 (the else-branch of a guard
            // is a complement), but there was no way to *write* one — so the most
            // wanted annotation in a nil-carrying language, "anything but nil", could
            // not be said. `(and any (not nil))` says it now.
            //
            // Exact on the flat tag lattice; a complement of a *refined* type widens
            // to that tag (see `Ty::negate`), which over-approximates — sound, and it
            // can only ever suppress a warning.
            if value::symbol_is(head, "not") && items.len() == 2 {
                return Some(parse_type(heap, items[1])?.negate());
            }
            // (map K V) — key/value typed map.  Full refinement: produce Ty::map_of
            // so the checker can derive `get`/`keys`/`vals`/`assoc` result types.
            if value::symbol_is(head, "map") && items.len() == 3 {
                let k = parse_type(heap, items[1])?;
                let v = parse_type(heap, items[2])?;
                return Some(Ty::map_of(k, v));
            }
            // (tuple T1 T2 …) — a fixed-arity positional vector shape
            // (ADR-128). `(tuple)` (zero elements) is a legitimate empty
            // tuple, so no minimum-length check — unlike `(list E)`/
            // `(vector E)`, which take exactly one element type, every
            // remaining item here is its own position's type.
            if value::symbol_is(head, "tuple") {
                let mut elems = Vec::with_capacity(items.len() - 1);
                for &it in &items[1..] {
                    elems.push(parse_type(heap, it)?);
                }
                return Some(Ty::tuple_of(elems));
            }
            // (record :k1 T1 :k2 T2 …) — a keyword-keyed heterogeneous map
            // shape. A field's type may be wrapped `(optional T)` to allow
            // the field to be absent/`nil`; every other field is required.
            // `Ty::record_of` carries the full field map so the checker can
            // derive `get`'s exact per-field result type (see
            // docs/type-records.md).
            if value::symbol_is(head, "record") {
                let mut rest = &items[1..];
                // `(record &open :a int)` — the leading marker makes the shape OPEN: a
                // value may carry keys it does not declare. Without it a record is
                // **closed** (ADR-264), which is what makes it say what a value is *not*.
                let open = matches!(rest.first(), Some(&Value::Sym(m))
                    if value::symbol_is(m, "&open"));
                if open {
                    rest = &rest[1..];
                }
                if rest.len() % 2 != 0 {
                    return None; // malformed — odd field-list length
                }
                let mut fields = std::collections::BTreeMap::new();
                for pair in rest.as_chunks::<2>().0 {
                    let Value::Keyword(name) = pair[0] else {
                        return None;
                    };
                    let (field_form, required) = match unwrap_optional(heap, pair[1]) {
                        Some(inner) => (inner, false),
                        None => (pair[1], true),
                    };
                    let field_ty = parse_type(heap, field_form)?;
                    fields.insert(name, (field_ty, required));
                }
                return Some(if open {
                    Ty::record_of_open(fields)
                } else {
                    Ty::record_of(fields)
                });
            }
            None
        }
        _ => None,
    }
}

/// The type constructors the grammar knows, by head symbol — the vocabulary
/// [`type_expr_problem`] validates an unrecognised head against. Kept beside
/// [`parse_type`]'s dispatch (which is the authority); a head added there and not
/// here would be reported as unknown, so `sig_grammar_heads_are_all_validated`
/// pins the two lists together.
pub(super) const TYPE_HEADS: [&str; 8] = [
    "list", "vector", "or", "and", "not", "map", "tuple", "record",
];

/// Why this type-expression can't be read as a type, or `None` if it can.
///
/// [`parse_type`] answers "did it parse", and its `None` is silently discarded —
/// so a misspelled type name (`strng`, `(tupel int)`) used to widen the annotated
/// position to `any` with no diagnostic at all: an annotation that is ignored when
/// wrong is a gate that cannot fail. This finds the *innermost* offending
/// sub-expression and says what is wrong with it.
///
/// **The one deliberate silence:** an unknown symbol whose name starts with an
/// uppercase letter is taken to be an ability used as a type (ADR-181/186) whose
/// defining module this particular check didn't load — abilities resolve by bare
/// name through `ABILITY_TYPES`, which under a single-file `brood --check` only
/// carries what the file itself declares. Ability names are capitalised (they are
/// named like records) and no base type is, so the split is exact for
/// well-named code and false-positive-free for the rest.
///
/// The test reads the symbol's **last `/` segment**, not the whole spelling, because
/// `shapes/Shape` and `Shape` name the same ability — the registry is keyed by the bare
/// CamelCase name (ADR-255) and [`ability_type`] resolves through `rsplit('/')` for exactly
/// that reason. Testing the whole spelling gave one ability two verdicts: in a project check
/// both forms resolve and neither warns, but in a loose single-file check the bare form fell
/// into this silence while the qualified form reported `unknown type` — a diagnostic that
/// appeared only when the module the ability actually comes from was named.
pub(super) fn type_expr_problem(heap: &Heap, form: Value) -> Option<String> {
    if parse_type(heap, form).is_some() {
        return None;
    }
    match form {
        Value::Sym(s) => {
            let name = value::symbol_name(s);
            let bare = name.rsplit('/').next().unwrap_or(&name);
            if bare.starts_with(|c: char| c.is_uppercase()) {
                return None; // an ability from a module this check didn't load
            }
            Some(format!("unknown type `{name}`"))
        }
        Value::Pair(_) => {
            let Some(items) = list_items(heap, form) else {
                return Some("malformed type expression".to_string());
            };
            // An arrow: every part must be a type, and the shape must be
            // `params… [&optional …] [& rest] -> ret` with exactly one result.
            if let Some(pos) = items.iter().position(|v| is_arrow_marker(*v)) {
                for &part in items.iter() {
                    if is_arrow_marker(part) || is_param_marker(part) {
                        continue;
                    }
                    // A part that doesn't parse decides the whole expression —
                    // *including* when its verdict is a deliberate silence (an unknown
                    // capitalised name). Reporting a structural problem instead would
                    // name the wrong thing: `(Shape -> int)` is not a malformed arrow,
                    // it is an arrow over an ability this check could not resolve.
                    if parse_type(heap, part).is_none() {
                        return type_expr_problem(heap, part);
                    }
                }
                return Some(if pos + 2 != items.len() {
                    "malformed function type: exactly one result type must follow `->`".to_string()
                } else {
                    "malformed function type: `&optional` and `&` must trail the parameters"
                        .to_string()
                });
            }
            let Some(&Value::Sym(head)) = items.first() else {
                return Some("malformed type expression".to_string());
            };
            let head_name = value::symbol_name(head);
            if !TYPE_HEADS.contains(&head_name.as_str()) {
                return Some(format!("unknown type constructor `{head_name}`"));
            }
            // A known constructor: report a bad argument before the arity, so the
            // innermost real problem wins.
            let args = if head_name == "record" {
                // `(record [&open] :k T …)` — the keys are keywords, only the values
                // are types.
                let body = match items.get(1) {
                    Some(&Value::Sym(m)) if value::symbol_is(m, "&open") => &items[2..],
                    _ => &items[1..],
                };
                if body.len() % 2 != 0 {
                    return Some(
                        "malformed `record` type: each field needs a keyword and a type"
                            .to_string(),
                    );
                }
                for pair in body.as_chunks::<2>().0 {
                    if !matches!(pair[0], Value::Keyword(_)) {
                        return Some(
                            "malformed `record` type: field names must be keywords".to_string(),
                        );
                    }
                    let value_form = unwrap_optional(heap, pair[1]).unwrap_or(pair[1]);
                    if parse_type(heap, value_form).is_none() {
                        return type_expr_problem(heap, value_form);
                    }
                }
                Vec::new()
            } else {
                items[1..].to_vec()
            };
            for &arg in &args {
                if parse_type(heap, arg).is_none() {
                    return type_expr_problem(heap, arg);
                }
            }
            // Every part reads as a type, so the arity of the constructor is what's wrong.
            Some(match head_name.as_str() {
                "list" | "vector" => {
                    format!("`{head_name}` takes exactly one element type")
                }
                "map" => "`map` takes exactly two types — a key and a value".to_string(),
                "or" => "`or` needs at least one member type".to_string(),
                _ => format!("malformed `{head_name}` type"),
            })
        }
        _ => Some("unrecognised type expression".to_string()),
    }
}

/// A parameter-list marker inside an arrow type-expr (`&`, `&optional`) — not a
/// type, and not an error either.
fn is_param_marker(v: Value) -> bool {
    matches!(v, Value::Sym(s)
        if value::symbol_is(s, "&") || value::symbol_is(s, "&optional"))
}

/// If a field type is wrapped `(optional T)`, peel it to `Some(T)`; anything
/// else (including a malformed `(optional …)` with the wrong arity) is
/// `None` — a plain, required field type.
fn unwrap_optional(heap: &Heap, form: Value) -> Option<Value> {
    let items = list_items(heap, form)?;
    if let [Value::Sym(h), inner] = items[..] {
        if value::symbol_is(h, "optional") {
            return Some(inner);
        }
    }
    None
}

fn is_arrow_marker(v: Value) -> bool {
    matches!(v, Value::Sym(s) if value::symbol_is(s, "->"))
}

/// Parse the items of an arrow type-expr (the `->` at index `pos`) to a [`Sig`]:
/// the items before `->` are parameter types, the single item after is the
/// result. `params... [&optional opt...] [& rest] -> ret`, mirroring a
/// closure's `(req &optional opt & rest)` param shape:
///   - `(int & number -> int)` → `Sig::with_rest([int], number, int)`
///   - `(int &optional string -> int)` → `Sig::with_optional([int], [string], int)`
///   - `(int &optional string & number -> int)` →
///     `Sig::with_optional_and_rest([int], [string], number, int)`
/// `&optional` must come before `&` when both are present. `None` if
/// malformed (no single result, markers out of order, or any part
/// unparseable).
fn parse_arrow(heap: &Heap, items: &[Value], pos: usize) -> Option<Sig> {
    if pos + 2 != items.len() {
        return None; // exactly one result type must follow `->`
    }
    let ret = parse_type(heap, items[pos + 1])?;

    let params_region = &items[..pos];
    let amp = params_region
        .iter()
        .position(|v| matches!(v, Value::Sym(s) if value::symbol_is(*s, "&")));
    let amp_opt = params_region
        .iter()
        .position(|v| matches!(v, Value::Sym(s) if value::symbol_is(*s, "&optional")));

    // `&` before `&optional` is a malformed order (rest must trail everything).
    if let (Some(a), Some(o)) = (amp, amp_opt) {
        if a < o {
            return None;
        }
    }

    let parse_all = |types: &[Value]| -> Option<Vec<Ty>> {
        types.iter().map(|&p| parse_type(heap, p)).collect()
    };

    match (amp_opt, amp) {
        (Some(opos), Some(apos)) => {
            // Must be exactly one type after `&` before `->`.
            if apos + 2 != pos {
                return None;
            }
            let params = parse_all(&params_region[..opos])?;
            let optional = parse_all(&params_region[opos + 1..apos])?;
            let rest = parse_type(heap, items[apos + 1])?;
            Some(Sig::with_optional_and_rest(params, optional, rest, ret))
        }
        (Some(opos), None) => {
            let params = parse_all(&params_region[..opos])?;
            let optional = parse_all(&params_region[opos + 1..])?;
            Some(Sig::with_optional(params, optional, ret))
        }
        (None, Some(apos)) => {
            // Must be exactly one type after `&` before `->`.
            if apos + 2 != pos {
                return None;
            }
            let params = parse_all(&params_region[..apos])?;
            let rest = parse_type(heap, items[apos + 1])?;
            Some(Sig::with_rest(params, rest, ret))
        }
        (None, None) => {
            let params = parse_all(params_region)?;
            Some(Sig::new(params, ret))
        }
    }
}

/// Parse a type-expression form into a [`SigTerm`], tracking type-variable
/// assignments in `vars` (variable name → sequential index). Every `?`-prefixed
/// symbol that hasn't been seen before gets the next index.
fn parse_type_term(heap: &Heap, form: Value, vars: &mut HashMap<String, u32>) -> Option<SigTerm> {
    match form {
        Value::Sym(s) => {
            let name = value::symbol_name(s);
            if name.starts_with('?') {
                let next = vars.len() as u32;
                let idx = *vars.entry(name.to_owned()).or_insert(next);
                return Some(SigTerm::Var(idx));
            }
            base_ty(&name)
                .or_else(|| ability_type(&name))
                .map(SigTerm::Ty)
        }
        Value::Nil => Some(SigTerm::Ty(Ty::of(Tag::Nil))),
        Value::Pair(_) => {
            let items = list_items(heap, form)?;
            // Arrow markers are only valid at top level — skip nested arrows.
            if items.iter().any(|v| is_arrow_marker(*v)) {
                return None;
            }
            let Value::Sym(head) = *items.first()? else {
                return None;
            };
            if value::symbol_is(head, "list") && items.len() == 2 {
                let inner = parse_type_term(heap, items[1], vars)?;
                return Some(SigTerm::ListOf(Box::new(inner)));
            }
            if value::symbol_is(head, "vector") && items.len() == 2 {
                let inner = parse_type_term(heap, items[1], vars)?;
                return Some(SigTerm::VectorOf(Box::new(inner)));
            }
            if value::symbol_is(head, "set") && items.len() == 2 {
                let inner = parse_type_term(heap, items[1], vars)?;
                return Some(SigTerm::SetOf(Box::new(inner)));
            }
            // `(record [&open] :k T …)` with a `?var` in some field: the same grammar
            // `parse_type` accepts, each field parsed as a term. With no variable anywhere
            // it is the concrete shape, exactly as before.
            if value::symbol_is(head, "record") {
                let mut rest = &items[1..];
                let open = matches!(rest.first(), Some(&Value::Sym(m))
                    if value::symbol_is(m, "&open"));
                if open {
                    rest = &rest[1..];
                }
                if rest.len() % 2 != 0 {
                    return None;
                }
                let mut fields: Vec<(value::Symbol, SigTerm, bool)> = Vec::new();
                let mut any_var = false;
                for pair in rest.as_chunks::<2>().0 {
                    let Value::Keyword(name) = pair[0] else {
                        return None;
                    };
                    let (field_form, required) = match unwrap_optional(heap, pair[1]) {
                        Some(inner) => (inner, false),
                        None => (pair[1], true),
                    };
                    let term = parse_type_term(heap, field_form, vars)?;
                    any_var |= !matches!(term, SigTerm::Ty(_));
                    fields.push((name, term, required));
                }
                if any_var {
                    return Some(SigTerm::RecordOf { fields, open });
                }
                return parse_type(heap, form).map(SigTerm::Ty);
            }
            // `(or …)` / `(and …)` with a variable inside: each alternative a term. Without
            // one, the concrete type as before.
            if value::symbol_is(head, "or") || value::symbol_is(head, "and") {
                let mut parts = Vec::with_capacity(items.len() - 1);
                let mut any_var = false;
                for &it in &items[1..] {
                    let t = parse_type_term(heap, it, vars)?;
                    any_var |= !matches!(t, SigTerm::Ty(_));
                    parts.push(t);
                }
                if any_var {
                    return Some(if value::symbol_is(head, "or") {
                        SigTerm::Or(parts)
                    } else {
                        SigTerm::And(parts)
                    });
                }
                return parse_type(heap, form).map(SigTerm::Ty);
            }
            // Compound forms without inner-var support — delegate to parse_type
            // (a type var inside `map` widens to Ty::ANY there).
            parse_type(heap, form).map(SigTerm::Ty)
        }
        _ => parse_type(heap, form).map(SigTerm::Ty),
    }
}

/// Parse the items of an arrow type-expr to a [`SigWithVars`], tracking type
/// variables in `vars`. Mirrors [`parse_arrow`] but produces `SigTerm`s.
fn parse_arrow_with_vars(
    heap: &Heap,
    items: &[Value],
    pos: usize,
    vars: &mut HashMap<String, u32>,
) -> Option<SigWithVars> {
    if pos + 2 != items.len() {
        return None;
    }
    let ret = parse_type_term(heap, items[pos + 1], vars)?;
    let amp = items[..pos]
        .iter()
        .position(|v| matches!(v, Value::Sym(s) if value::symbol_is(*s, "&")));
    let (params, rest) = if let Some(apos) = amp {
        if apos + 2 != pos {
            return None;
        }
        let mut params = Vec::with_capacity(apos);
        for &p in &items[..apos] {
            params.push(parse_type_term(heap, p, vars)?);
        }
        let rest_term = parse_type_term(heap, items[apos + 1], vars)?;
        (params, Some(rest_term))
    } else {
        let mut params = Vec::with_capacity(pos);
        for &p in &items[..pos] {
            params.push(parse_type_term(heap, p, vars)?);
        }
        (params, None)
    };
    Some(SigWithVars { params, rest, ret })
}

/// If `form` is a `(sig name (… -> …))` declaration whose arrow contains at
/// least one type variable (`?A`, `?B` …), return `(name, sig_with_vars)`.
/// Returns `None` for non-`sig` forms, non-arrow type-exprs, or arrows with
/// no variables — the plain [`parse_sig_decl`] path handles those.
/// An arrow TYPE expression (the value part of a `(sig name type)`) as a [`SigWithVars`],
/// or `None` when it declares no variable at all — a plain [`Sig`] then serves. The
/// type-level half of [`parse_sig_decl_with_vars`], so the heap-recorded raw type value a
/// loaded module registered (`declared_heap_sig_with_vars`) resolves per call exactly as a
/// same-file declaration does — otherwise a constructor's `?x` fields read as `any` from
/// hover and `reflect/expr-type` while `nest check` on the file got them right.
pub(super) fn parse_arrow_type_with_vars(heap: &Heap, type_value: Value) -> Option<SigWithVars> {
    let ty_items = list_items(heap, type_value)?;
    let pos = ty_items.iter().position(|v| is_arrow_marker(*v))?;
    let mut vars: HashMap<String, u32> = HashMap::new();
    let sig = parse_arrow_with_vars(heap, &ty_items, pos, &mut vars)?;
    if vars.is_empty() {
        return None;
    }
    Some(sig)
}

pub(super) fn parse_sig_decl_with_vars(heap: &Heap, form: Value) -> Option<(Symbol, SigWithVars)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, "sig") && !value::symbol_is(head, "sig!") {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    Some((name, parse_arrow_type_with_vars(heap, items[2])?))
}

/// The `(name, type-form)` of any `(sig name T)` / `(sig! name T)` declaration,
/// whatever `T` turns out to be — the shape-only recogniser the *validation* pass
/// needs, since every other recogniser here returns `None` for a declaration it
/// cannot parse, which is exactly the case being reported on.
pub(super) fn sig_decl_parts(heap: &Heap, form: Value) -> Option<(Symbol, Value)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, "sig") && !value::symbol_is(head, "sig!") {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    Some((name, items[2]))
}

/// If `form` is a `(sig name (… -> …))` declaration whose type-expr is an arrow,
/// return `(name, sig)`. `None` for a non-`sig` form, a malformed one, or a
/// non-arrow type-expr (`(sig x int)` — accepted by the grammar but not a call
/// signature, so nothing to record in slice 1).
pub(super) fn parse_sig_decl(heap: &Heap, form: Value) -> Option<(Symbol, Sig)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    // `sig` (static only) and `sig!` (also runtime-enforced) declare the same
    // signature as far as the checker is concerned — it reads both.
    if !value::symbol_is(head, "sig") && !value::symbol_is(head, "sig!") {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    // Only an arrow type-expr is a callable signature worth recording.
    let sig = parse_type(heap, items[2])?.as_arrow()?.clone();
    Some((name, sig))
}

/// If `form` is a `(sig name (… -> …))` declaration whose type-expr is an
/// **overload** — an `(and …)` of 2+ distinct arrows, e.g. `(sig f (and (int
/// -> int) (bool -> bool)))` — return `(name, sigs)`. `None` for anything
/// [`parse_sig_decl`] already handles (a single arrow, or no arrow at all);
/// mirrors [`parse_sig_decl_with_vars`]'s parallel-path pattern. See
/// `docs/type-arrow-intersection.md`.
pub(super) fn parse_sig_decl_overload(heap: &Heap, form: Value) -> Option<(Symbol, Vec<Sig>)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, "sig") && !value::symbol_is(head, "sig!") {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    let sigs = parse_type(heap, items[2])?.overload_sigs()?.clone();
    Some((name, sigs))
}

/// If `form` is a `(sig name T)` declaration whose type-expr `T` is a **value
/// type** (not an arrow), return `(name, T)`. The non-function counterpart of
/// [`parse_sig_decl`]: `(sig x int)` declares the *value* `x` has type `int`,
/// which the gradual-assignment check consults to verify `(def x …)`. Returns
/// `None` for a non-`sig` form or an arrow type-expr (that's `parse_sig_decl`'s).
pub(super) fn parse_value_sig_decl(heap: &Heap, form: Value) -> Option<(Symbol, Ty)> {
    let items = list_items(heap, form)?;
    if items.len() != 3 {
        return None;
    }
    let Value::Sym(head) = items[0] else {
        return None;
    };
    if !value::symbol_is(head, "sig") && !value::symbol_is(head, "sig!") {
        return None;
    }
    let Value::Sym(name) = items[1] else {
        return None;
    };
    let ty = parse_type(heap, items[2])?;
    // A function arrow is a *callable* signature — that's `parse_sig_decl`'s job;
    // here we only take a plain value type.
    if ty.as_arrow().is_some() {
        return None;
    }
    Some((name, ty))
}
