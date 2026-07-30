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
}

/// Install this file's ability-type table (call before parsing sigs). Overwrites any prior
/// file's table.
pub(super) fn set_ability_types(map: HashMap<String, Option<Vec<String>>>) {
    ABILITY_TYPES.with(|m| *m.borrow_mut() = map);
}

/// Drop the ability-type table — per-file hygiene, mirroring [`super::sigs::clear_sig_memo`].
pub(super) fn clear_ability_types() {
    ABILITY_TYPES.with(|m| m.borrow_mut().clear());
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
    ABILITY_TYPES.with(|m| {
        match m.borrow().get(name)? {
            // Open ability: permissive — the type checks nothing, but the sig survives.
            None => Some(Ty::ANY),
            // Sealed ability: the finite union of its members' record shapes, built at its
            // true set-theoretic denotation as a SINGLE record shape whose `:__id__` is the
            // union of the member keyword-literals: `%{__id__: (:c | :r | …)}`.
            //
            // This is *equal as a set of values* to `⋃ₘ %{__id__: :m}` precisely because each
            // member shape is an OPEN record constraining only `:__id__` — so the union is
            // exactly "maps whose `:__id__` ∈ members". We build the single shape directly
            // rather than `Ty::union`-ing the member shapes, because `Ty::union` widens a
            // differing `fields` map away (a sound over-approximation → `map`, but it drops the
            // member set). Field-wise-merging arbitrary records in `Ty::union` would be UNSOUND
            // (it invents cross terms), so that stays as-is; only *this* union, where the
            // shapes differ solely in `:__id__`, collapses soundly to a lit-union field. The
            // preserved `:__id__` lit set is what drives both a precise non-member rejection
            // and sealed-`match` exhaustiveness (ADR-187 part 2).
            Some(members) => {
                let id_ty = members.iter().fold(None::<Ty>, |acc, member| {
                    let lit = Ty::keyword_lit(value::intern(member));
                    Some(match acc {
                        Some(a) => a.union(lit),
                        None => lit,
                    })
                });
                match id_ty {
                    Some(id) => {
                        let mut fields = BTreeMap::new();
                        fields.insert(value::intern("__id__"), (id, true));
                        Some(Ty::record_of(fields))
                    }
                    // A sealed ability with no members is degenerate → permissive, not NEVER.
                    None => Some(Ty::ANY),
                }
            }
        }
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
pub(super) fn parse_type(heap: &Heap, form: Value) -> Option<Ty> {
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
            // A base type name wins; otherwise a bare **sealed ability** name resolves to
            // the union of its members' record shapes (ADR-181) — `(sig f (Shape -> …))`,
            // `:-> Shape`. An unknown symbol still yields `None` (dropped, never guessed).
            base_ty(&name).or_else(|| ability_type(&name))
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
                let rest = &items[1..];
                if rest.len() % 2 != 0 {
                    return None; // malformed — odd field-list length
                }
                let mut fields = std::collections::BTreeMap::new();
                for pair in rest.chunks_exact(2) {
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
                return Some(Ty::record_of(fields));
            }
            None
        }
        _ => None,
    }
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
            // Compound forms without inner-var support — delegate to parse_type
            // (type vars inside `or`/`and`/`map` widen to Ty::ANY there).
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
    let ty_items = list_items(heap, items[2])?;
    let pos = ty_items.iter().position(|v| is_arrow_marker(*v))?;
    let mut vars: HashMap<String, u32> = HashMap::new();
    let sig = parse_arrow_with_vars(heap, &ty_items, pos, &mut vars)?;
    if vars.is_empty() {
        return None;
    }
    Some((name, sig))
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
