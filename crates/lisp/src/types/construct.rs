//! The named points, constructors and refinement accessors of [`Ty`]: the constants
//! (`NEVER`, `ANY`, `NUMBER`, …), the builders for arrows, sequences, maps, records, tuples,
//! literals and intervals, the readers a checker uses to see a refinement back, and the
//! bridges from a runtime value or a type predicate to a type.

use super::*;

impl Ty {
    /// `⊥` — the empty set; the type of no value. A subtype of every type.
    pub const NEVER: Ty = Ty::flat(0);
    /// `⊤` — every tag; the type of any value. A supertype of every type.
    pub const ANY: Ty = Ty::flat(UNIVERSE);
    /// `int ∪ float ∪ decimal` — the named union the prelude's `number?` predicate
    /// implies. A `decimal` is a number (but not an integer).
    pub const NUMBER: Ty = Ty::flat(
        (1u32 << bit(Tag::Int))
            | (1u32 << bit(Tag::Float))
            | (1u32 << bit(Tag::Decimal))
            | (1u32 << bit(Tag::Ratio)),
    );
    /// `nil ∪ pair` — the named union the prelude's `list?` predicate implies.
    pub const LIST: Ty = Ty::flat((1u32 << bit(Tag::Nil)) | (1u32 << bit(Tag::Pair)));
    /// The **seqable** union — every collection the sequence combinators walk (a list —
    /// `nil`/`pair`, a range/seqview reading as `pair` — a vector, set, map, or `bytes`;
    /// `string` is deliberately excluded). The named type a polymorphic-sequence `sig`
    /// parameter uses (`(sig f (seqable -> …))`) instead of falling back to `any`.
    pub const SEQABLE: Ty = Ty::flat(
        (1u32 << bit(Tag::Nil))
            | (1u32 << bit(Tag::Pair))
            | (1u32 << bit(Tag::Vector))
            | (1u32 << bit(Tag::Set))
            | (1u32 << bit(Tag::Map))
            | (1u32 << bit(Tag::Record))
            | (1u32 << bit(Tag::Bytes)),
    );

    /// The **countable** union — everything `count`/`get`/`empty?` accept: the seqable
    /// collections plus the three sized non-sequences, `string`, `rope` and `table`. The
    /// named type of `count`'s parameter, so a function that counts its argument is
    /// suggested `(countable -> …)` rather than the six-way union spelled out.
    pub const COUNTABLE: Ty = Ty::flat(
        (1u32 << bit(Tag::Nil))
            | (1u32 << bit(Tag::Pair))
            | (1u32 << bit(Tag::Vector))
            | (1u32 << bit(Tag::Set))
            | (1u32 << bit(Tag::Map))
            | (1u32 << bit(Tag::Record))
            | (1u32 << bit(Tag::Bytes))
            | (1u32 << bit(Tag::Str))
            | (1u32 << bit(Tag::Rope))
            | (1u32 << bit(Tag::Table)),
    );

    /// Every truthy value — `any ∖ nil ∖ false`: what an `(if x …)` / `(when x …)` /
    /// `(or x …)` guard leaves of an unknown.
    pub fn truthy() -> Ty {
        Ty::ANY
            .difference(Ty::of(Tag::Nil))
            .difference(Ty::bool_lit(false))
    }

    /// The **unknown keyed value** (ADR-542): `map | record` with nothing known of either —
    /// no key/value types, no shape, no id. A record is its own kind, so a parameter read
    /// only by keyword (`(:server m)`) or an update of an unknown receiver (`(assoc m :k v)`,
    /// which keeps whichever kind `m` is) is "some map or record": it says the value is
    /// keyed and nothing about which kind — as `any` says nothing at all. Strict checking
    /// reads this part by overlap, like the unknown, and everything else in the term by
    /// inclusion. `None` when the term does not carry both kinds unrefined; otherwise the
    /// rest of the term, without the two keyed tags (`never` when there is none).
    pub fn without_unknown_keyed(&self) -> Option<Ty> {
        if self.fields.is_some() {
            return None;
        }
        self.keyed_kind_unknown().map(|(_, _, rest)| rest)
    }

    /// A term whose keyed KIND is unknown — it carries both `map` and `record`, with no
    /// `map<K, V>` refinement (which only a map has) — split into its map projection, its
    /// record projection, and the rest. A shape riding on both (a field guard over an
    /// unannotated value, [`Ty::keyed_of_open`]) is one such term; the bare `map | record`
    /// is the other. Strict checking accepts the keyed part when EITHER projection fits:
    /// the value is one kind or the other, and which is not known.
    pub fn keyed_kind_unknown(&self) -> Option<(Ty, Ty, Ty)> {
        if self.tags & SHAPE_BITS != SHAPE_BITS || self.map_kv.is_some() || self.neg.is_some() {
            return None;
        }
        let rest = (0..32)
            .map(|i| 1u32 << i)
            .filter(|b| self.tags & b != 0 && b & SHAPE_BITS == 0)
            .fold(Ty::NEVER, |acc, b| acc.union(self.project_tag(b)));
        Some((
            self.project_tag(MAP_BIT),
            self.project_tag(RECORD_BIT),
            rest,
        ))
    }

    /// Is this type known only by what it is NOT — `any`, or `any` less a guard's
    /// `nil`/`false`? Such a bound says nothing positive about the value, so strict
    /// checking (`GradualTy::consistent_with_mode`) keeps the overlap reading for it.
    pub fn is_known_only_by_exclusion(&self) -> bool {
        // An INTERVAL (ADR-350) — a length a guard established, an int's bounds — does not
        // make a term positively known: `(not (nil | false))` of length at least one is
        // still "not nil", so the question is asked of the term without its intervals.
        let this = self.without_intervals();
        let this = &this;
        if Ty::truthy().is_subtype(this) {
            return true;
        }
        // `any ∖ (a finite literal set)` — what a failed `(= x 0)` / `(= tag :done)` leaves
        // of an unknown: it says which values `x` is NOT, and nothing it is. Reading
        // `(not 0)` by inclusion flagged `(- i 1)` in every loop that tested `(= i 0)` first.
        let excluded = Ty::ANY.difference(this.clone());
        let without_falsy = excluded
            .difference(Ty::of(Tag::Nil))
            .difference(Ty::of(Tag::Bool));
        // The excluded set must BE the literal set — `as_lit_int` reads the int member's
        // literals and ignores the other tags, so `any ∖ (int ∖ 0)`, which is `0 | (not
        // int)`, answered `Some({0})` and a positively-known `(int and (not 0))` read as
        // known only by exclusion.
        let only_literals = (without_falsy.as_lit().is_some()
            && without_falsy.is_subtype(&Ty::of(Tag::Keyword)))
            || (without_falsy.as_lit_int().is_some()
                && without_falsy.is_subtype(&Ty::of(Tag::Int)))
            || (without_falsy.as_lit_str().is_some()
                && without_falsy.is_subtype(&Ty::of(Tag::Str)));
        if without_falsy.is_never() || only_literals {
            return true;
        }
        // `any ∖ vector` — what a failed `(vector? x)` leaves: a flat tag set with no
        // positive refinement, describing MORE than half the universe. Its complement is
        // the shorter description, i.e. what is known is what the value is not.
        let refined = this.alts.is_some()
            || this.arrow.is_some()
            || this.elem.is_some()
            || this.map_kv.is_some()
            || this.fields.is_some()
            || this.tuple.is_some()
            || this.lit.as_deref().is_some_and(|l| l.members().is_some())
            || this
                .lit_int
                .as_deref()
                .is_some_and(|l| l.members().is_some())
            || this
                .lit_bool
                .as_deref()
                .is_some_and(|l| l.members().is_some())
            || this
                .lit_str
                .as_deref()
                .is_some_and(|l| l.members().is_some());
        !refined && this.tags.count_ones() * 2 > UNIVERSE.count_ones()
    }

    /// The record identities named by this type's map member (`:t/usd`, …), as spelled
    /// in a `sig` — `None` when the map member is not a nominal shape.
    pub fn project_record_ids(&self) -> Option<Vec<String>> {
        let fields = self.record_fields()?;
        display::nominal_ids(fields)
    }

    /// A flat (unrefined) type from a raw tag bitset — the internal constructor
    /// every flat `Ty` funnels through. `const` so the named points above can be
    /// `const`; the set operations that combine refinements can't be.
    pub(super) const fn flat(tags: u32) -> Ty {
        Ty {
            tags,
            neg: None,
            arrow: None,
            elem: None,
            map_kv: None,
            overload: None,
            fields: None,
            tuple: None,
            list_shape: None,
            lit: None,
            lit_int: None,
            lit_bool: None,
            lit_str: None,
            alts: None,
            rec_ref: false,
            mu: false,
            int_range: None,
            len: None,
        }
    }

    /// Does this type carry **no** refinements — is it exactly its tag set? The
    /// question the complement rendering and the DNF-free fast paths ask: a flat type
    /// is fully described by `tags`, so set reasoning on it is exact.
    pub fn is_flat(&self) -> bool {
        // A union of terms is not flat even when its *head* carries no refinement —
        // the alternatives do the describing. Without this the predicate answers about
        // one term while reading as if it answered about the type.
        self.alts.is_none()
            && self.arrow.is_none()
            && self.overload.is_none()
            && self.elem.is_none()
            && self.map_kv.is_none()
            && self.fields.is_none()
            && self.tuple.is_none()
            && self.lit.is_none()
            && self.lit_int.is_none()
            && self.lit_bool.is_none()
            && self.lit_str.is_none()
    }

    /// The singleton type containing exactly the values with this tag.
    pub const fn of(tag: Tag) -> Ty {
        Ty::flat(1u32 << bit(tag))
    }

    /// The tag type named by a `type-of` keyword — `:table` is `table`, `:pair` is
    /// `pair` — or `None` for a keyword that names no tag. What lets `(= :table (type-of
    /// x))` guard `x` exactly as `(table? x)` does (`guards::type_of_eq_guard`).
    pub fn of_type_keyword(keyword: Symbol) -> Option<Ty> {
        ALL_TAGS
            .iter()
            .find(|tag| tag.keyword() == keyword)
            .map(|&tag| Ty::of(tag))
    }

    /// The flat union of several tags — `const`, so callers can build named
    /// shorthands (e.g. `seq = nil | pair | vector`) as `const` items without the
    /// non-`const` [`union`](Ty::union). Unrefined (every flat type is).
    pub const fn of_tags(tags: &[Tag]) -> Ty {
        let mut bits = 0u32;
        let mut i = 0;
        while i < tags.len() {
            bits |= 1u32 << bit(tags[i]);
            i += 1;
        }
        Ty::flat(bits)
    }

    /// A function type `(params...) -> ret` — the function members refined to
    /// exactly those matching `sig`. Tagged `Fn|Native` (an arrow describes both
    /// closures and builtins).
    pub fn arrow(sig: Sig) -> Ty {
        Ty {
            arrow: Some(Arc::new(sig)),
            ..Ty::flat(FN_BITS)
        }
    }

    /// The function-arrow refinement, if this type carries one. The bridge the
    /// advisory checker reads to compare a callback against what a higher-order
    /// function expects.
    pub fn as_arrow(&self) -> Option<&Sig> {
        self.single()?.arrow.as_deref()
    }

    /// An overloaded function type — `sigs[0] and sigs[1] and …` (an
    /// intersection of 2+ distinct arrows), e.g. `(int -> int) and (bool ->
    /// bool)`. Tagged `Fn|Native` like a plain arrow. `sigs` must have at
    /// least 2 elements; a single signature belongs in [`Ty::arrow`] instead
    /// — [`Ty::intersect`] enforces this collapse automatically, so this
    /// constructor is for tests/direct construction only.
    pub fn overload_of(sigs: Vec<Sig>) -> Ty {
        // The arms in the canonical order and absorption `Ty::intersect` builds, so a
        // directly constructed overload equals the parsed `(and …)` of the same arms.
        let mut arms = canonical_arms(sigs);
        if arms.len() == 1 {
            return Ty::arrow(arms.remove(0));
        }
        Ty {
            overload: Some(Arc::new(arms)),
            ..Ty::flat(FN_BITS)
        }
    }

    /// The overload refinement, if this type carries one — the bridge the
    /// checker reads to resolve a call's return type per matching arm. `None`
    /// when this type carries at most a single [`Ty::arrow`].
    pub fn overload_sigs(&self) -> Option<&Vec<Sig>> {
        self.single()?.overload.as_deref()
    }

    /// A sequence type over `tags` (some subset of `pair`/`vector`) whose elements
    /// have type `elem` — the general element-refinement constructor.
    pub fn seq_of(tags: u32, elem: Ty) -> Ty {
        Ty {
            elem: Some(Arc::new(elem)),
            ..Ty::flat(tags & SEQ_BITS)
        }
        .canonical_shape()
    }

    /// A freshly built shape in canonical form: its slots normalised, and an uninhabited
    /// one (`list<never>`, `(tuple int never)`, `{a: never}`) as [`Ty::NEVER`] itself.
    /// Built raw, such a shape compared unequal to the empty type it denotes, and the
    /// Pass 2.9 fixpoint, which compares rounds by `==`, flipped between the two
    /// spellings forever (a `cons` of a tuple with a not-yet-derived `never` position).
    pub(super) fn canonical_shape(self) -> Ty {
        let out = self.normalise_len().bounded();
        if out.alts.is_none() && out.term_is_never() {
            return Ty::NEVER;
        }
        out
    }

    /// `map<K, V>` — a map whose keys have type `K` and values have type `V`.
    pub fn map_of(key: Ty, val: Ty) -> Ty {
        Ty {
            map_kv: Some(Arc::new((key, val))),
            ..Ty::flat(MAP_BIT)
        }
        .bounded()
    }

    /// A heterogeneous record shape — `field name → (declared type,
    /// required?)`. Tagged `map` (a record is still a runtime `map` value;
    /// this only refines it, the same trick [`Ty::keyword_lit`] uses layering
    /// onto the `Keyword` tag). See `docs/type-records.md`.
    /// A **closed** record shape — `(record :a int)`. Exactly the declared fields;
    /// every other key is absent, which reads as `nil` (ADR-264).
    pub fn record_of(fields: BTreeMap<Symbol, (Ty, bool)>) -> Ty {
        Ty::record_shape(fields, Ty::of(Tag::Nil))
    }

    /// An **open** record shape — `(record &open :a int)`. The declared fields, and a
    /// value may carry any others. This is what a shape used as a *parameter domain*
    /// wants (a `defrecord` accessor takes any record carrying its field), and what an
    /// ability-as-a-type resolves to.
    pub fn record_of_open(fields: BTreeMap<Symbol, (Ty, bool)>) -> Ty {
        Ty::record_shape(fields, Ty::ANY)
    }

    /// An open shape over BOTH keyed kinds — "a map or a record carrying these fields"
    /// (ADR-542): what a field guard proves of a value whose kind it does not know
    /// (`(when (:sudo b) …)` over an unannotated `b`). A `map` shape alone would rule out
    /// the record `b` may be.
    pub fn keyed_of_open(fields: BTreeMap<Symbol, (Ty, bool)>) -> Ty {
        Ty {
            fields: Some(Arc::new(RecordShape {
                fields,
                rest: Ty::ANY,
            })),
            ..Ty::flat(SHAPE_BITS)
        }
        .canonical_shape()
    }

    /// The general constructor: declared fields plus the type of every undeclared key.
    ///
    /// A shape whose `__id__` field holds an identity literal is a `defrecord` value's
    /// type and is tagged `record` (ADR-542) — the id stays in the shape as the nominal
    /// part the checker dispatches and seals on; every other shape is a plain map's.
    pub fn record_shape(fields: BTreeMap<Symbol, (Ty, bool)>, rest: Ty) -> Ty {
        let tag = if fields
            .get(&value::intern("__id__"))
            .is_some_and(|(ty, _)| is_record_identity(ty))
        {
            RECORD_BIT
        } else {
            MAP_BIT
        };
        Ty {
            fields: Some(Arc::new(RecordShape { fields, rest })),
            ..Ty::flat(tag)
        }
        .canonical_shape()
    }

    /// The one shape tag (`map` or `record`, ADR-542) this term carries a `fields` shape on,
    /// or 0 — for none, or for a term carrying both (its shape cannot say which it refines).
    pub(crate) fn shape_bit(&self) -> u32 {
        match self.tags & SHAPE_BITS {
            MAP_BIT if self.fields.is_some() => MAP_BIT,
            RECORD_BIT if self.fields.is_some() => RECORD_BIT,
            _ => 0,
        }
    }

    /// The record-shape refinement's declared fields, if this map type carries one.
    pub fn record_fields(&self) -> Option<&BTreeMap<Symbol, (Ty, bool)>> {
        Some(&self.single()?.fields.as_deref()?.fields)
    }

    /// The type a `(get r k)` yields on this record type, or `None` when there is no
    /// answer to give. Unlike [`Ty::record_fields`] this answers for **every** key: an
    /// undeclared key on a closed record is `nil` (it is absent), which is what lets a
    /// field read through a tagged union resolve at all.
    ///
    /// Over a *union* of shapes it is the union of what each term yields — the whole
    /// point of the closed shape: `{ok: int} | {error: string}` answers `int | nil` for
    /// `:ok`, because the second term says the key is absent rather than shrugging.
    pub fn record_field_ty(&self, key: Symbol) -> Option<Ty> {
        if self.mu {
            return self.unroll().record_field_ty(key);
        }
        // The single-term case is the common one and must not pay for the union: it
        // reads the shape in place rather than materialising a term vector (this is a
        // per-field-read hot path in the checker).
        if self.alts.is_none() {
            return self.term_field_ty_or_nil(key);
        }
        let mut acc: Option<Ty> = None;
        for term in self.terms_vec() {
            let t = term.term_field_ty_or_nil(key)?;
            acc = Some(match acc {
                Some(a) => a.union(t),
                None => t,
            });
        }
        acc
    }

    /// One term's field reading, with the `nil` member folded in: `(:k nil)` and `(get nil
    /// :k)` answer `nil` at runtime, so `(:k maybe-record)` over `nil | {k: int}` is
    /// `nil | int` — the shape every optional field read has. A `nil | {…}` union is ONE
    /// term (`nil` carries no refinement to keep it apart), tagged `nil|map` with the
    /// record's shape, and reading it used to demand `tags == map` and give up — so the
    /// whole read was UNKNOWN and every consumer downstream of an optional record lost
    /// the field types the record had declared. Any other non-map tag (a vector, a string
    /// — a keyword read raises on those) still declines.
    pub(super) fn term_field_ty_or_nil(&self, key: Symbol) -> Option<Ty> {
        if self.tags & !(SHAPE_BITS | NIL_BIT) != 0 {
            return None;
        }
        let from_map = if self.tags & SHAPE_BITS != 0 {
            Some(self.term_field_ty(key)?)
        } else {
            None
        };
        let from_nil = (self.tags & NIL_BIT != 0).then(|| Ty::of(Tag::Nil));
        match (from_map, from_nil) {
            (Some(m), Some(n)) => Some(m.union(n)),
            (Some(m), None) => Some(m),
            (None, Some(n)) => Some(n),
            (None, None) => None,
        }
    }

    /// [`record_field_ty`](Self::record_field_ty) for `(get r k default)`: the absence
    /// case reads as `default`'s type rather than `nil`. See [`RecordShape::field_ty_with_default`].
    pub fn record_field_ty_with_default(&self, key: Symbol, default: &Ty) -> Option<Ty> {
        let mut acc: Option<Ty> = None;
        for term in self.terms_vec() {
            // The `nil` member reads as the default — `(get nil :k 7)` is `7` — see
            // `term_field_ty_or_nil` for why a `nil | {…}` union is one term.
            if term.tags & !(SHAPE_BITS | NIL_BIT) != 0 {
                return None;
            }
            let from_nil = (term.tags & NIL_BIT != 0).then(|| default.clone());
            let from_map = if term.tags & SHAPE_BITS != 0 {
                let shape = term.fields.as_deref()?;
                Some(if !shape.fields.contains_key(&key) && shape.is_open() {
                    match term.map_kv.as_deref() {
                        Some((_, v)) => v.clone().union(default.clone()),
                        None => shape.field_ty_with_default(key, default.clone()),
                    }
                } else {
                    shape.field_ty_with_default(key, default.clone())
                })
            } else {
                None
            };
            let t = match (from_map, from_nil) {
                (Some(m), Some(n)) => m.union(n),
                (Some(m), None) => m,
                (None, Some(n)) => n,
                (None, None) => return None,
            };
            acc = Some(match acc {
                Some(a) => a.union(t),
                None => t,
            });
        }
        acc
    }

    /// One term's answer for `key`. Almost always the shape's own reading — except for
    /// an **undeclared** key on an **open** shape, where the shape says only `any` and a
    /// `map<K,V>` refinement on the same term (an intersection can carry both) says
    /// something sharper. Taking the shape's `any` there would be a precision
    /// regression against the pre-shape behaviour, which consulted `map_kv`.
    pub(super) fn term_field_ty(&self, key: Symbol) -> Option<Ty> {
        let shape = self.fields.as_deref()?;
        if !shape.fields.contains_key(&key)
            && shape.is_open()
            && let Some((_, v)) = self.map_kv.as_deref()
        {
            return Some(v.clone().union(Ty::of(Tag::Nil)));
        }
        Some(shape.field_ty(key))
    }

    /// Is this a record shape that admits keys it does not declare?
    pub fn record_is_open(&self) -> Option<bool> {
        Some(self.single()?.fields.as_deref()?.is_open())
    }

    /// A positional tuple shape — one type per index, fixed arity. Tagged
    /// `vector` (a tuple is still a runtime `[ ]` vector value; this only
    /// refines it, the same layering trick `record_of` uses onto `map`). See
    /// `docs/type-tuples.md` (ADR-128).
    pub fn tuple_of(elems: Vec<Ty>) -> Ty {
        Ty {
            tuple: Some(Arc::new(elems)),
            ..Ty::flat(VECTOR_BIT)
        }
        .canonical_shape()
    }

    /// A positional LIST shape — a non-empty list of exactly these elements, one type
    /// per index. Tagged `pair` alone (the empty list is `nil`, a separate tag); the
    /// list sibling of [`Ty::tuple_of`]. `elems` must be non-empty.
    pub fn list_shape_of(elems: Vec<Ty>) -> Ty {
        debug_assert!(!elems.is_empty(), "a list shape has at least one element");
        Ty {
            list_shape: Some(Arc::new(elems)),
            ..Ty::flat(PAIR_BIT)
        }
        .canonical_shape()
    }

    /// The list-shape refinement, if this type is nothing but a list of a known shape —
    /// the same refusal [`Ty::tuple_elems`] makes: a term that also admits `nil` or a
    /// vector has no positional answer.
    pub fn list_shape_elems(&self) -> Option<&Vec<Ty>> {
        let this = self.single()?;
        if this.tags != PAIR_BIT {
            return None;
        }
        this.list_shape.as_deref()
    }

    /// The positions of a type that is exactly a vector tuple or exactly a list shape —
    /// what `(first x)` / `(nth x <literal>)` / `(rest x)` read, since each answers the
    /// same way for either sequence kind.
    pub fn positional_elems(&self) -> Option<Vec<Ty>> {
        if self.mu {
            return self.unroll().positional_elems();
        }
        self.tuple_elems()
            .or_else(|| self.list_shape_elems())
            .cloned()
    }

    /// The tuple-shape refinement, if this vector type carries one. The
    /// bridge the checker reads to flow `(nth t i)`/`(first t)` to the exact
    /// per-position type.
    pub fn tuple_elems(&self) -> Option<&Vec<Ty>> {
        let this = self.single()?;
        // Positional access is exact ONLY on a term that is nothing but a vector of this
        // shape: a consumer reads "position 0 is `0`, and never nil". A term that also
        // admits a `pair` (unknown elements) or `nil` (`first` is nil) has no positional
        // answer at all — reporting the tuple's was how `(tuple 0) ∪ pair` typed `first`
        // as `0` on a value that was a string.
        if this.tags != VECTOR_BIT {
            return None;
        }
        this.tuple.as_deref()
    }

    /// The type at tuple position `n` — exact over a UNION of tuple terms too:
    /// `(tuple 0 nil) | (tuple int pair)` at position 0 is `0 | int`, since each term is
    /// nothing but a vector of a known shape and position `n` of the union is the union of
    /// the positions. `None` when any term is not such a tuple, or is too short — the same
    /// refusal [`Ty::tuple_elems`] makes for one term. What a destructuring `[j acc] st`
    /// over a fold accumulator reads: without this the union fell to `elem_ty` — every
    /// element of every term — and `j` came out `nil | int | pair`, which `inc` widened to
    /// `number` (a strict false positive in `std/editor/markdown`).
    pub fn tuple_elem_at(&self, n: usize) -> Option<Ty> {
        if self.mu {
            return self.unroll().tuple_elem_at(n);
        }
        let terms = self.terms_vec();
        let mut out = Ty::NEVER;
        for term in &terms {
            if term.tags != VECTOR_BIT {
                return None;
            }
            let elem = term.tuple.as_deref()?.get(n)?.clone();
            out = out.union(elem);
        }
        Some(out)
    }

    /// A keyword-literal (singleton) type — exactly the keyword `sym`. Unions of
    /// these build an enumerated keyword type, e.g. `(or :maximized :fullboth)`.
    pub fn keyword_lit(sym: Symbol) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(sym);
        Ty {
            lit: Some(Arc::new(LitSet::In(set))),
            ..Ty::flat(KEYWORD_BIT)
        }
    }

    /// The keyword-literal refinement, if this type carries one (the exact keyword
    /// symbols admitted). `None` means "any keyword" (or no keyword member).
    pub fn as_lit(&self) -> Option<&BTreeSet<Symbol>> {
        self.single()?.lit.as_deref()?.members()
    }

    /// An int-literal (singleton) type — exactly the integer `n` (ADR-117).
    /// Unions of these build an enumerated int type, e.g. `(or 200 404 500)`.
    /// Independent of [`Ty::keyword_lit`] (a different tag), so the two
    /// compose freely — `(or :ok 5)` carries both refinements at once.
    pub fn int_lit(n: i64) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(n);
        Ty {
            lit_int: Some(Arc::new(LitSet::In(set))),
            ..Ty::flat(INT_BIT)
        }
    }

    /// The int-literal refinement, if this type carries one (the exact
    /// integers admitted). `None` means "any int" (or no int member).
    pub fn as_lit_int(&self) -> Option<&BTreeSet<i64>> {
        self.single()?.lit_int.as_deref()?.members()
    }

    /// `int` within `range` (ADR-350): `Ty::int_in(Range::at_least(0))` is `int[0..]`.
    pub fn int_in(range: Range) -> Ty {
        if let Some(n) = range.as_point() {
            return Ty::int_lit(n);
        }
        Ty {
            int_range: (!range.is_all()).then_some(range),
            ..Ty::flat(INT_BIT)
        }
    }

    /// This type with its countable members' LENGTH refined to `range` — `(vector int)`
    /// of length 3, a non-empty list. A term with no countable member is unchanged; a
    /// length of exactly 0 drops the `pair` member (a list of length 0 is `nil`).
    pub fn with_len(self, range: Range) -> Ty {
        let terms: Vec<Ty> = self
            .terms_vec()
            .into_iter()
            .map(|mut term| {
                if term.mu || term.tags & COUNT_BITS == 0 {
                    return term;
                }
                // `nil` counts 0: a length that excludes 0 excludes it.
                if !Range::subset(Range::point(0), range) {
                    term.tags &= !NIL_BIT;
                }
                match Range::meet(term.len_eff(), range) {
                    None => term.tags &= !COUNT_BITS,
                    Some(r) => {
                        if r.hi == Some(0) {
                            term.tags &= !PAIR_BIT;
                        }
                        term.len = canon_len(term.tags, Some(r));
                        term = term.normalise_len();
                    }
                }
                term
            })
            .collect();
        let mu = self.mu;
        let mut out = Ty::from_terms(terms);
        if mu {
            out.mu = true;
        }
        out
    }

    /// This type with its int member OPENED — no literal set, no interval — for a result
    /// whose operand COUNT is unknown: `(apply + xs)` and a numeric fold stay in `+`'s
    /// closure (an int) but not in one step's interval (ADR-350).
    pub fn without_int_interval(&self) -> Ty {
        let terms: Vec<Ty> = self
            .terms_vec()
            .into_iter()
            .map(|mut term| {
                if term.tags & INT_BIT != 0 && !term.mu {
                    term.lit_int = None;
                    term.int_range = None;
                }
                term
            })
            .collect();
        let mut out = Ty::from_terms(terms);
        out.mu = self.mu;
        out
    }

    /// This type with every interval dropped — the int bounds and the lengths — on every
    /// term. What an interval-blind reading of the type sees.
    pub fn without_intervals(&self) -> Ty {
        let terms: Vec<Ty> = self
            .terms_vec()
            .into_iter()
            .map(|mut term| {
                if !term.mu {
                    term.int_range = None;
                    term.len = None;
                }
                term
            })
            .collect();
        let mut out = Ty::from_terms(terms);
        out.mu = self.mu;
        out
    }

    /// The interval every int this type admits lies in: the literal set's span when
    /// it is positive, else the interval refinement, else everything. Over a union, the
    /// hull. `None` when the type admits no int at all.
    pub fn int_range(&self) -> Option<Range> {
        let mut out: Option<Range> = None;
        for term in self.terms_vec() {
            if term.tags & INT_BIT == 0 {
                continue;
            }
            let r = term.int_range_eff();
            out = Some(match out {
                Some(acc) => Range::hull(acc, r),
                None => r,
            });
        }
        out
    }

    /// The int interval as the literal set it denotes, when it is bounded and holds at
    /// most [`MAX_ENUMERATED_RANGE`] values — `(int 1 2)` is `1 | 2`, and a relation
    /// against a literal set can only be decided on the enumeration. `None` for an open
    /// or wide interval (an infinite or large set is never inside a finite listing, and
    /// the caller's `None` reads exactly that way).
    pub(super) fn enumerated_int_range(&self) -> Option<Arc<LitSet<i64>>> {
        let Range {
            lo: Some(lo),
            hi: Some(hi),
        } = self.int_range?
        else {
            return None;
        };
        // `checked_sub`, not `hi - lo`: a full-width interval overflows the subtraction,
        // which panics under debug-assertions and wraps without them — and a `None` here
        // is the right answer for it anyway (far past the enumeration cap).
        let width = hi.checked_sub(lo)?;
        if width < 0 || width >= MAX_ENUMERATED_RANGE {
            return None;
        }
        Some(Arc::new(LitSet::In((lo..=hi).collect())))
    }

    /// One term's effective int interval (see [`Ty::int_range`]).
    pub(super) fn int_range_eff(&self) -> Range {
        if let Some(LitSet::In(set)) = self.lit_int.as_deref()
            && let (Some(lo), Some(hi)) = (set.iter().next(), set.iter().next_back())
        {
            return Range::new(Some(*lo), Some(*hi));
        }
        self.int_range.unwrap_or(Range::ALL)
    }

    /// The interval `(count x)` lies in for every value of this type: the length of the
    /// countable members (a `pair` is at least 1), `0` for `nil`, the hull over a union.
    /// `None` when the type has no countable member and no `nil` (nothing to count).
    /// The number of keys a CLOSED record shape has — `[required, declared]`, a point when
    /// every field is required (C16). Deliberately NOT part of [`Ty::count_range`] or the
    /// `len` slot: a length there is a refinement every RELATION reads, and giving a closed
    /// record one made a bare `map` — the unknown shape, which must fit any shape under the
    /// gradual relation — stop being a consistent subtype of it, because `[0, ∞)` does not
    /// fit `[2, 2]`. The field count is an answer about a `(count r)` CALL, not a fact about
    /// the type's length, so it is read where that call is typed.
    pub fn record_field_count(&self) -> Option<Range> {
        let shape = self.single()?.fields.as_deref()?;
        if shape.is_open() {
            return None;
        }
        let declared = shape.fields.len() as i64;
        let required = shape.fields.values().filter(|(_, req)| *req).count() as i64;
        Some(Range::new(Some(required), Some(declared)))
    }

    pub fn count_range(&self) -> Option<Range> {
        let mut out: Option<Range> = None;
        let mut fold = |r: Range| {
            out = Some(match out {
                Some(acc) => Range::hull(acc, r),
                None => r,
            })
        };
        for term in self.terms_vec() {
            if term.tags & COUNT_BITS != 0 {
                fold(term.len_eff());
            }
            if term.tags & NIL_BIT != 0 {
                fold(Range::point(0));
            }
        }
        out
    }

    /// One term's effective length interval: the slot, else `[1, ∞)` when the only
    /// countable member is a `pair`, else everything.
    pub(super) fn len_eff(&self) -> Range {
        if let Some(r) = self.len {
            return r;
        }
        // A vector (or set) of `never` holds no element: it is the empty one, length 0.
        // Read as unbounded, `vector<never> ∪ vector<int>[2]` hulled to `vector<int>`.
        if self.tags & COUNT_BITS != 0
            && self.tags & COUNT_BITS & !(VECTOR_BIT | (1u32 << bit(Tag::Set))) == 0
            && self.tuple.is_none()
            && self.elem.as_deref().is_some_and(Ty::is_never)
        {
            return Range::point(0);
        }
        // A positional shape IS its arity, when it is the only countable member.
        if self.tags & COUNT_BITS == VECTOR_BIT
            && let Some(ts) = &self.tuple
        {
            return Range::point(ts.len() as i64);
        }
        if self.tags & COUNT_BITS == PAIR_BIT {
            if let Some(ts) = &self.list_shape {
                return Range::point(ts.len() as i64);
            }
            return Range::at_least(1);
        }
        // A string literal set knows its lengths — chars, as `%string-length` counts — so
        // `"abc"` fits `(len string _ 3)` and `(into [] "abc")` has a length.
        if self.tags & COUNT_BITS == STR_BIT
            && let Some(LitSet::In(set)) = self.lit_str.as_deref()
        {
            let lengths = set.iter().map(|s| s.chars().count() as i64);
            if let (Some(lo), Some(hi)) = (lengths.clone().min(), lengths.max()) {
                return Range::new(Some(lo), Some(hi));
            }
        }
        Range::at_least(0)
    }

    /// This type's WIDENING against the value it had a round earlier (ADR-350): every
    /// interval — an int's, a length's — whose end moved outward goes to its infinity,
    /// so a counter's ascent `[0,0], [0,1], …` is `[0, ∞)` in one step. Walks the
    /// refinement trees in parallel where they have the same shape; leaves the rest.
    pub fn widen_intervals_against(&self, prev: &Ty) -> Ty {
        // Alternatives that share their tags are merged first — by the widening merge —
        // on both sides, WHEN the alternatives are multiplying: a tuple accumulator whose
        // steps differ in two positions (`(tuple 0 nil) | (tuple 1 (list 0)) | …`) grows
        // an alternative per round, and is widened as one shape instead. A union that is
        // not growing keeps its alternatives — `(tuple model int) | (tuple model' int)`,
        // two branches returning a pair, is not an ascent, and merging it lost the tuple
        // shape (a destructuring `[m idx]` then read `int | map`). A record alternative is
        // left alone either way: the tagged-union idiom (`{:ok v} | {:error e}`) must not
        // merge under a fixpoint.
        let (my_terms, their_terms) = (self.terms_vec(), prev.terms_vec());
        let growing = my_terms.len() > their_terms.len();
        let (mine, theirs) = if growing {
            (
                collapse_same_tags(my_terms),
                collapse_same_tags(their_terms),
            )
        } else {
            (my_terms, their_terms)
        };
        let widened: Vec<Ty> = mine
            .into_iter()
            .map(|term| {
                // The previous term with the same tags (`nil` aside — it rides along with
                // whichever alternative it merged into), if there is exactly one — or, among
                // several, the one this term GREW from: the previous term that is a subtype
                // of it. `(tuple "(" int[1..k] nil bool) | (tuple "(" 0 nil false)` — a
                // parser state beside its seed — matched two previous tuples by tag alone,
                // widened neither, and ran the derivation out of rounds.
                let same_tags: Vec<&Ty> = theirs
                    .iter()
                    .filter(|p| p.tags & !NIL_BIT == term.tags & !NIL_BIT)
                    .collect();
                let prev_term = match same_tags.as_slice() {
                    [one] => *one,
                    [] => return term,
                    several => {
                        let mut grew_from = several.iter().filter(|p| p.is_subtype(&term));
                        match (grew_from.next(), grew_from.next()) {
                            (Some(p), None) => *p,
                            _ => return term,
                        }
                    }
                };
                let mut out = term.clone();
                // A literal SET that grew is an interval on the move too — `0`, `0 | 1`,
                // `0 | 1 | 2` is a counter — so it widens the same way, to an interval.
                let lit_grew = match (term.lit_int.as_deref(), prev_term.lit_int.as_deref()) {
                    (Some(LitSet::In(now)), Some(LitSet::In(before))) => {
                        now.len() > before.len() && before.is_subset(now)
                    }
                    (Some(LitSet::In(_)), None) => prev_term.int_range.is_some(),
                    _ => false,
                };
                if term.tags & INT_BIT != 0
                    && (term.int_range.is_some() || prev_term.int_range.is_some() || lit_grew)
                {
                    let r = Range::widen(prev_term.int_range_eff(), term.int_range_eff());
                    if lit_grew {
                        out.lit_int = None;
                    }
                    out.int_range = canon_int_range(&out.lit_int, Some(r));
                }
                if term.tags & COUNT_BITS != 0 && (term.len.is_some() || prev_term.len.is_some()) {
                    let r = Range::widen(prev_term.len_eff(), term.len_eff());
                    out.len = canon_len(out.tags, Some(r));
                    out = out.normalise_len();
                }
                if !out.mu {
                    out = out.map_nested_with(prev_term, &mut |t, p| t.widen_intervals_against(p));
                }
                out
            })
            .collect();
        let mut out = Ty::from_terms(widened);
        out.mu = self.mu;
        out
    }

    /// [`Ty::map_nested`] over two terms of the same shape: `f` sees each nested Ty of
    /// `self` beside the corresponding one of `other` where `other` has it, and the
    /// nested Ty alone (unchanged) where it does not.
    pub(super) fn map_nested_with(&self, other: &Ty, f: &mut dyn FnMut(&Ty, &Ty) -> Ty) -> Ty {
        let mut out = self.clone();
        if let (Some(e), Some(p)) = (&self.elem, &other.elem) {
            out.elem = Some(Arc::new(f(e, p)));
        }
        if let (Some(kv), Some(pkv)) = (&self.map_kv, &other.map_kv) {
            out.map_kv = Some(Arc::new((f(&kv.0, &pkv.0), f(&kv.1, &pkv.1))));
        }
        if let (Some(shape), Some(pshape)) = (&self.fields, &other.fields) {
            out.fields = Some(Arc::new(RecordShape {
                fields: shape
                    .fields
                    .iter()
                    .map(|(k, (t, req))| {
                        let t = match pshape.fields.get(k) {
                            Some((p, _)) => f(t, p),
                            None => t.clone(),
                        };
                        (*k, (t, *req))
                    })
                    .collect(),
                rest: f(&shape.rest, &pshape.rest),
            }));
        }
        for (mine, theirs) in [
            (&mut out.tuple, &other.tuple),
            (&mut out.list_shape, &other.list_shape),
        ] {
            if let (Some(ts), Some(ps)) = (mine.as_deref(), theirs.as_deref())
                && ts.len() == ps.len()
            {
                *mine = Some(Arc::new(
                    ts.iter().zip(ps.iter()).map(|(t, p)| f(t, p)).collect(),
                ));
            }
        }
        out
    }

    /// A bool-literal (singleton) type — exactly `true` or `false` (ADR-120).
    /// Unlike the keyword-literal era's guidance ("`false` isn't a literal
    /// type"), that restriction was specific to avoiding `false`/`nil`
    /// confusion in an *enumerated keyword* set — now that bool-literal types
    /// are their own real kind, both values are legitimate singletons.
    pub fn bool_lit(b: bool) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(b);
        Ty {
            lit_bool: Some(Arc::new(LitSet::In(set))),
            ..Ty::flat(BOOL_BIT)
        }
    }

    /// The bool-literal refinement, if this type carries one. `None` means
    /// "any bool" (or no bool member).
    pub fn as_lit_bool(&self) -> Option<&BTreeSet<bool>> {
        self.single()?.lit_bool.as_deref()?.members()
    }

    /// A string-literal (singleton) type — exactly the string `s` (ADR-120).
    /// Takes `&str` rather than a `Value`/`Heap` pair — the caller reads the
    /// content out of its `Value::Str` heap handle first (`heap.string(id)`),
    /// so `Ty` itself stays heap-independent like every other constructor.
    pub fn str_lit(s: &str) -> Ty {
        let mut set = BTreeSet::new();
        set.insert(s.to_string());
        Ty {
            lit_str: Some(Arc::new(LitSet::In(set))),
            ..Ty::flat(STR_BIT)
        }
    }

    /// The string-literal refinement, if this type carries one. `None` means
    /// "any string" (or no string member).
    pub fn as_lit_str(&self) -> Option<&BTreeSet<String>> {
        self.single()?.lit_str.as_deref()?.members()
    }

    /// The key/value refinement, if this map type carries one. The bridge the
    /// checker reads to flow `(get m k)` → `V | nil`, `(keys m)` → `list<K>`, etc.
    pub fn map_kv(&self) -> Option<(Ty, Ty)> {
        if self.mu {
            return self.unroll().map_kv();
        }
        self.single()?.map_kv.as_deref().cloned()
    }

    /// `vector<elem>` — a vector whose elements have type `elem`.
    pub fn vector_of(elem: Ty) -> Ty {
        Ty::seq_of(1u32 << bit(Tag::Vector), elem)
    }

    /// `list<elem>` — a (non-empty) list whose elements have type `elem`. Tagged
    /// `pair`; the empty-list `nil` carries no element type, so a value that may
    /// be `nil` widens to plain `list` at the join.
    pub fn list_of(elem: Ty) -> Ty {
        Ty::seq_of(1u32 << bit(Tag::Pair), elem)
    }

    /// `set<elem>` — a set every element of which is an `elem`. Like a vector (and
    /// unlike a list, whose empty case is `nil`), the empty set inhabits every
    /// `set<T>`, so `set<never>` is the type of `#{}` and never uninhabited.
    pub fn set_of(elem: Ty) -> Ty {
        Ty::seq_of(1u32 << bit(Tag::Set), elem)
    }

    /// `(seqable elem)` — a **sequence** of `elem`, whichever of the three shapes it is
    /// built as: `nil | list<elem> | vector<elem> | set<elem>` (C11, ADR-365). The
    /// spelling a polymorphic-sequence parameter wants, where bare [`Ty::SEQABLE`] carries
    /// no element type at all and the longhand `(or (list T) (vector T) (set T))` is what
    /// authors were writing instead.
    ///
    /// **`map` and `bytes` are deliberately NOT in it**, though both are in bare
    /// `seqable`. Their element types are fixed by the KIND rather than carried — a map
    /// walks as `[key value]` entries, a `bytes` as octets — so neither is a collection of
    /// an arbitrary `elem`, and `(map K V)` / `bytes` already say what they hold. There is
    /// a mechanical reason too, and it points the same way: with them the union is six
    /// terms against a [`MAX_TY_TERMS`] of four, and the overflow collapse would widen
    /// away the very element types this spelling exists to state — silently, which is the
    /// worst way to lose them.
    ///
    /// So `(seqable T) ⊆ seqable` — refining drops members, exactly as `(map K V) ⊆ map`
    /// keeps only the maps that match. It is not "all of `seqable`, with elements".
    pub fn seqable_of(elem: Ty) -> Ty {
        Ty::list_of(elem.clone())
            .union(Ty::of(Tag::Nil))
            .union(Ty::vector_of(elem.clone()))
            .union(Ty::set_of(elem))
    }

    /// The `elem` of a [`Ty::seqable_of`], when this type is exactly one — every one of
    /// the four members present, each carrying the SAME element type. Used to spell it
    /// back as `(seqable T)`; anything less than the full shape renders as the union it
    /// is, since that is what it means.
    pub fn as_seqable_of(&self) -> Option<Ty> {
        // The four members MERGE into one term — they differ only in their tag, and the
        // `elem` refinement is the same on each, so `merge_is_exact` folds them: the type
        // is `{tags: nil|pair|vector|set, elem: E}`, not a four-alternative union. Union
        // the terms' tags anyway rather than read `self.tags` (which is the HEAD term's),
        // so this keeps answering if the merge ever stops applying.
        if self.mu {
            return None;
        }
        let tags = self
            .terms_vec()
            .iter()
            .fold(0u32, |acc, term| acc | term.tags);
        if tags != (SEQ_BITS | NIL_BIT) {
            return None;
        }
        // Recognise by RECONSTRUCTION rather than by re-reading the slots: take the
        // element type off any member that carries one and ask whether `seqable_of` of it
        // is this type. That cannot drift from the constructor the way a hand-written
        // shape test does — a slot added to `Ty` tomorrow makes the equality fail (and the
        // union print longhand), never makes this claim something it is not.
        let elem = self
            .terms_vec()
            .into_iter()
            .find_map(|term| term.elem.as_deref().cloned())?;
        (Ty::seqable_of(elem.clone()) == *self).then_some(elem)
    }

    /// [`Ty::elem_ty`] over a UNION too: the union of the terms' elements — sound, since a
    /// value of the union is a value of one term — when every term that is a collection
    /// says what it holds (a `nil` term holds nothing and is skipped; a non-collection
    /// term means the value may not be a collection at all, so there is no answer). For
    /// the combinators that walk a value's elements (`map`, `reverse`, `rest`, `cons`
    /// onto it); a POSITIONAL read (`nth`) wants [`Ty::element_at_over_union`] instead.
    pub fn elem_ty_union(&self) -> Option<Ty> {
        if self.alts.is_none() {
            return self.elem_ty();
        }
        let mut out = Ty::NEVER;
        for term in self.terms_vec() {
            if term.tags == NIL_BIT {
                continue;
            }
            out = out.union(term.elem_ty()?);
        }
        Some(out)
    }

    /// This type with only the terms `keep` admits — `never` when none survives.
    pub fn retain_terms(&self, keep: impl Fn(&Ty) -> bool) -> Ty {
        let terms: Vec<Ty> = self.terms_vec().into_iter().filter(|t| keep(t)).collect();
        let mut out = Ty::from_terms(terms);
        out.mu = self.mu;
        out.normalise_mu()
    }

    /// The type a positional read yields over a UNION, term by term: a positional
    /// shape answers its position (or `nil` past its end), a uniform sequence its
    /// element with `nil` beside it (the read may run off the end), `nil` reads `nil`.
    /// `index` is the literal position, or `None` for `last`. `None` when some term is
    /// not a collection at all.
    pub fn element_at_over_union(&self, index: Option<usize>) -> Option<Ty> {
        let mut out = Ty::NEVER;
        for term in self.terms_vec() {
            if term.tags == NIL_BIT {
                out = out.union(Ty::of(Tag::Nil));
                continue;
            }
            let read = match term.positional_elems() {
                Some(elems) => match index {
                    Some(i) => elems.get(i).cloned().unwrap_or(Ty::of(Tag::Nil)),
                    None => elems.last().cloned().unwrap_or(Ty::of(Tag::Nil)),
                },
                None => term.elem_ty()?.union(Ty::of(Tag::Nil)),
            };
            out = out.union(read);
        }
        Some(out)
    }

    /// The element-type refinement, if this sequence type carries one (or can
    /// be derived from one) — the bridge the checker reads to flow `(first
    /// xs)` / `(nth xs i)` to the element type. A tuple has no plain `elem`,
    /// but the union of its per-position types is exactly as sound a bound
    /// (every element of a `tuple<int, string>` is an `int | string`), so
    /// derive that when `elem` itself is absent (ADR-128) — this is the
    /// single choke point every `elem_ty` consumer already goes through, so
    /// `first`/`nth`/`rest`/etc. all pick up a tuple-typed vector for free.
    /// Owned (not `&Ty`) because the tuple case synthesizes a fresh value;
    /// every existing caller already immediately `.cloned()`s the borrowed
    /// case anyway.
    pub fn elem_ty(&self) -> Option<Ty> {
        if self.mu {
            return self.unroll().elem_ty();
        }
        let this = self.single()?;
        // A refinement the type CARRIES describes every sequence member it was put on,
        // and is the answer wherever it is present.
        if let Some(elem) = this.elem.as_deref() {
            return Some(elem.clone());
        }
        // Otherwise the element type can still be DERIVED from the kind — for the three
        // collections below. Each derivation speaks for the whole term, so each requires
        // the term to admit exactly one collection: when it admits a second, that
        // member's elements are not covered and the term's are unknown. Answering for one
        // member is the accepting direction of unsound — it produced a checker false
        // positive, `(tuple 0) ∪ pair` (a fold whose init is `[0]` and whose step conses
        // strings) answering `0` for `first`, so `(takes-str (first …))` warned "expects
        // string, got 0" on a value that was the string "t" at runtime.
        match this.tags & (SEQ_BITS | MAP_BIT | BYTES_BIT) {
            // A tuple has no plain `elem`, but its per-position types taken together are
            // exactly as sound a bound (every element of a `tuple<int, string>` is an
            // `int | string`). Positions only ever describe a VECTOR.
            VECTOR_BIT => this
                .tuple
                .as_ref()
                .map(|elems| elems.iter().cloned().fold(Ty::NEVER, |acc, t| acc.union(t))),
            // …and a list shape's, for the pair member.
            PAIR_BIT => this
                .list_shape
                .as_ref()
                .map(|elems| elems.iter().cloned().fold(Ty::NEVER, |acc, t| acc.union(t))),
            // `bytes` is a sequence of octets: its element type is fixed by the kind, so
            // it needs no refinement to carry one — and an octet is `int[0..255]`, which
            // is what makes `(nth codes (math/rem b 16))` an index the checker can bound.
            BYTES_BIT => Some(Ty::int_in(Range::new(Some(0), Some(255)))),
            // A map walks as its `[key value]` entries — a two-element VECTOR, verified
            // against the runtime rather than assumed. What is known of the key and the
            // value comes from whichever refinement the map carries: `map_kv` states them
            // directly, and a CLOSED record shape states them exactly (its keys are the
            // declared ones and nothing else — an OPEN shape may carry keys nothing
            // declares, so it declines). A map the checker knows nothing about declines
            // too — see the Seqable note below.
            MAP_BIT => {
                let (key, value) = if let Some((k, v)) = this.map_kv.as_deref() {
                    (k.clone(), v.clone())
                } else if let (Some(fields), Some(false)) =
                    (self.record_fields(), self.record_is_open())
                {
                    fields.iter().fold(
                        (Ty::NEVER, Ty::NEVER),
                        |(keys, values), (name, (ty, _optional))| {
                            (keys.union(Ty::keyword_lit(*name)), values.union(ty.clone()))
                        },
                    )
                } else {
                    // Neither: decline. An OPEN shape says nothing about the keys a
                    // value carries, and a NOMINAL record is open — which is the case
                    // that must not be answered, because a record may implement Seqable
                    // and then it walks as whatever that impl yields rather than as
                    // entries. `tests/queue_test.blsp` maps over a queue, and the
                    // "an entry is at least a pair of unknowns" fallback this replaces
                    // typed its elements as `(tuple any, any)` and flagged the callback.
                    return None;
                };
                Some(Ty::tuple_of(vec![key, value]))
            }
            _ => None,
        }
    }

    /// The type of a concrete value — the bridge from a runtime value to its type.
    /// A keyword/int/bool becomes its **literal singleton** (`:foo`/`5`/`true`, not
    /// the whole `keyword`/`int`/`bool` tag), so a literal in code is checked
    /// precisely against an enumerated sig (`5` vs `(or 5 6 7)`). Ints and bools
    /// were once left flat here to avoid the message-wording churn a singleton
    /// causes ("got int" → "got 5"); that churn was accepted and the singletons
    /// shipped in B0 (gating — see `docs/type-int-literals.md`). Strings need the
    /// heap to read their bytes, so `of_value` (heap-free) leaves a string flat and
    /// `expr_ty` builds the `str_lit` where it has the heap. Bignums stay flat `int`.
    pub fn of_value(v: Value) -> Ty {
        match v {
            // Literal singletons (B0 — literal-singleton precision): a literal's
            // *exact* type is the singleton, e.g. `5 : {5}` (a subtype of `int`),
            // so a literal argument checks precisely against a literal-set param
            // (`5` vs `(or 5 6 7)`). String singletons need the heap to read the
            // bytes, so `of_value` (heap-free) leaves a string flat; `expr_ty`
            // builds the `str_lit` where it has the heap.
            Value::Keyword(s) => Ty::keyword_lit(s),
            Value::Int(n) => Ty::int_lit(n),
            Value::Bool(b) => Ty::bool_lit(b),
            _ => Ty::of(value::tag(v)),
        }
    }

    /// The type asserted when the named type-predicate holds — the bridge from a
    /// guard `(pred x)` to a refinement of `x`'s type (occurrence typing, step 4):
    /// in the *then* branch `x` narrows to `T ∩ tested_by(pred)`, in the *else*
    /// branch to `T ∩ ¬tested_by(pred)`. `None` for predicates that don't pin a
    /// tag (`empty?`, `zero?`, …) and for unknown names. Spellings match the
    /// `int?`/`string?`/… builtins and the prelude's `number?`/`list?`.
    ///
    /// Keyed by `&str` for now; the Step 4 pass holds interned `Symbol`s, so this
    /// may move to a `Symbol`-keyed lookup if it proves hot.
    pub fn tested_by(predicate: &str) -> Option<Ty> {
        Some(match predicate {
            "nil?" => Ty::of(Tag::Nil),
            "bool?" => Ty::of(Tag::Bool),
            "int?" => Ty::of(Tag::Int),
            "float?" => Ty::of(Tag::Float),
            "decimal?" => Ty::of(Tag::Decimal),
            "ratio?" => Ty::of(Tag::Ratio),
            "symbol?" => Ty::of(Tag::Sym),
            "keyword?" => Ty::of(Tag::Keyword),
            "string?" => Ty::of(Tag::Str),
            "bytes?" => Ty::of(Tag::Bytes),
            "pair?" => Ty::of(Tag::Pair),
            "vector?" => Ty::of(Tag::Vector),
            "map?" => Ty::of(Tag::Map),
            // A record is its own kind (ADR-542), so `record?` is biconditional: it holds
            // for exactly the records.
            "record?" | "%record?" => Ty::of(Tag::Record),
            // A map or a record — the test a map pattern and the field operations make.
            "%map-like?" => Ty::of(Tag::Map).union(Ty::of(Tag::Record)),
            "set?" => Ty::of(Tag::Set),
            "ref?" => Ty::of(Tag::Ref),
            "pid?" => Ty::of(Tag::Pid),
            "rope?" => Ty::of(Tag::Rope),
            "socket?" => Ty::of(Tag::Socket),
            "subprocess?" => Ty::of(Tag::Subprocess),
            "table?" => Ty::of(Tag::Table),
            // ADR-310 appended `Failure` as a tag; a predicate over it narrows
            // like any other, so `(if (failure? n) 0 n)` types the else branch
            // without the failure in it.
            "failure?" => Ty::of(Tag::Failure),
            // `fn?` holds for both Brood closures and Rust builtins.
            "fn?" => Ty::of(Tag::Fn).union(Ty::of(Tag::Native)),
            "number?" => Ty::NUMBER,
            "list?" => Ty::LIST,
            _ => return None,
        })
    }

    /// The type a truthy `(pred x)` PROVES of `x` when the predicate is not biconditional:
    /// every value it accepts has this type, but so do values it rejects, so only the
    /// then-branch may narrow. None today: `record?`, the one such predicate while a record
    /// was a map, is biconditional since a record became its own kind (ADR-542) and is a
    /// [`tested_by`](Ty::tested_by) predicate. Kept as the seam a later one would use.
    pub fn implied_by(_predicate: &str) -> Option<Ty> {
        None
    }
}
