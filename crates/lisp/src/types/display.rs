//! Display rendering for [`Ty`] (extracted from mod.rs).
use super::*;

/// The excluded values of a negatively-stated literal set, rendered; empty for a
/// positive one or an unrefined slot.
fn excluded_of<T: Ord, F: Fn(&T) -> String>(
    slot: &Option<Arc<LitSet<T>>>,
    render: F,
) -> Vec<String> {
    let mut out: Vec<String> = slot
        .as_deref()
        .and_then(LitSet::excluded)
        .map(|set| set.iter().map(&render).collect())
        .unwrap_or_default();
    out.sort();
    out
}

/// The record identities a shape names — `:ns/circle` → `ns/circle`, a union of them joined
/// by the caller — or `None` when the shape is not nominal: no `__id__` at all (an ordinary
/// map type), or an `__id__` that is not a positively-enumerated set of keyword literals (a
/// bare `keyword`, or a negatively-stated set, which cannot be listed).
///
/// The leading colon is dropped because the result is meant to be the spelling you would
/// write in a `sig` — `(sig area (shapes/circle -> float))` — not the keyword the runtime
/// stores. The FULL path is kept rather than the last segment: two modules may each define
/// `pt`, and "expects pt, got pt" would be worse than no message at all.
pub(super) fn nominal_ids(
    fields: &std::collections::BTreeMap<Symbol, (Ty, bool)>,
) -> Option<Vec<String>> {
    let (id_ty, _) = fields.get(&value::intern("__id__"))?;
    let members = id_ty.lit.as_ref()?.members()?;
    if members.is_empty() {
        return None;
    }
    Some(
        members
            .iter()
            .map(|s| value::symbol_name_ref(*s).to_string())
            .collect(),
    )
}

/// Render a term's **subtractions**, if any: `(not X)` when the positive part is the whole
/// universe, `(and P (not X))` otherwise.
///
/// Not optional. A subtraction that renders as its positive part is a message that says
/// `vector` for a type that excludes every `vector<int>` — the reader is told the opposite
/// of the truth, which is worse than the widening this replaced.
fn render_subtraction(positive: &str, negs: &[Ty], universe: bool) -> String {
    let inner = if negs.len() == 1 {
        negs[0].to_string()
    } else {
        format!(
            "({})",
            negs.iter()
                .map(Ty::to_string)
                .collect::<Vec<_>>()
                .join(" | ")
        )
    };
    if universe {
        format!("(not {inner})")
    } else {
        format!("({positive} and (not {inner}))")
    }
}

impl fmt::Display for Ty {
    /// A readable rendering for diagnostics: the named lattice points where they
    /// apply (`never`, `any`, `number`, `list`), a single tag by its `type-of`
    /// name, otherwise the members joined with ` | ` (e.g. `int | string`). A
    /// purely-function type with a known arrow renders as `(p1, p2) -> ret`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A union of terms renders as its terms, joined — `(tuple int) | (tuple
        // string)`, the shape that used to print as bare `vector` because the union
        // had nowhere to keep both (ADR-262). Each term renders by the rules below.
        if let Some(terms) = self.alt_terms() {
            let rendered: Vec<String> = terms.iter().map(Ty::to_string).collect();
            return f.write_str(&rendered.join(" | "));
        }
        // A term that SUBTRACTS (ADR-288) renders as what it is. Falling through would
        // print the positive part alone — `vector` for a type that excludes every
        // `vector<int>` — telling the reader the opposite of the truth.
        if let Some(negs) = self.subtracted() {
            if self.term_is_empty_for_display() {
                return f.write_str("never");
            }
            let positive = self.positive_for_display();
            return f.write_str(&render_subtraction(
                &positive.to_string(),
                negs,
                positive.is_any(),
            ));
        }
        // Named points (compared by value — `Arc` isn't structural, so these
        // can't be `match` patterns).
        if *self == Ty::NEVER {
            return f.write_str("never");
        }
        if *self == Ty::ANY {
            return f.write_str("any");
        }
        if *self == Ty::NUMBER {
            return f.write_str("number");
        }
        if *self == Ty::LIST {
            return f.write_str("list");
        }
        if *self == Ty::COUNTABLE {
            return f.write_str("countable");
        }
        // A named cover (ADR-299) with two or more records in it reads better as its name
        // than as the list; with one record, `number | t/usd` says more than `numeric`.
        if let Some((name, records)) = super::check::cover_name_of(self) {
            if records >= 2 {
                return f.write_str(name);
            }
        }
        if *self == Ty::SEQABLE {
            return f.write_str("seqable");
        }
        // A purely-function type with a known signature: show the arrow, or
        // every arm of an overload joined with ` and ` (matching the `(and
        // …)` annotation syntax that produces it).
        if self.tags & !FN_BITS == 0 {
            if let Some(sig) = self.as_arrow() {
                return write!(f, "{sig}");
            }
            if let Some(sigs) = self.overload_sigs() {
                let joined = sigs
                    .iter()
                    .map(Sig::to_string)
                    .collect::<Vec<_>>()
                    .join(" and ");
                return f.write_str(&joined);
            }
        }
        // A pure sequence type with a known element type: `vector<E>` / `list<E>`
        // — with a leading `nil | ` when the empty/empty-list case rides along
        // (e.g. a `(map …)` result is `nil | list<E>`), so the rendering names
        // every tag the value can actually have.
        // A pure map type with a known key/value type: `map<K, V>`.
        if let Some((k, v)) = self.map_kv() {
            if self.tags == MAP_BIT {
                return write!(f, "map<{k}, {v}>");
            }
        }
        // A record shape riding in a wider term — `number | usd`, the domain of `+` once a
        // record has a `num/add` method. The term's map member IS the record, and only the
        // `tags == MAP_BIT` branch below knows to render one by name; listing the tags
        // would print it as the bare `map` and lose the one word that matters. Split the
        // term into its map projection and everything else, each rendered by its own rule.
        if self.record_fields().is_some()
            && self.neg.is_none()
            && self.tags != MAP_BIT
            && self.tags & MAP_BIT != 0
        {
            let rest = (0..32)
                .map(|i| 1u32 << i)
                .filter(|b| self.tags & b != 0 && *b != MAP_BIT)
                .fold(Ty::NEVER, |acc, b| acc.union(self.project_tag(b)));
            return write!(f, "{rest} | {}", self.project_tag(MAP_BIT));
        }
        // A record shape: `{name: string, age?: int}` — `?` marks an
        // optional field. `fields` is keyed by interned `Symbol` (intern
        // order, not alphabetical — same trap `lit` avoids below), so sort
        // by spelling for a stable rendering.
        if let Some(fields) = self.record_fields() {
            if self.tags == MAP_BIT {
                // A record's NAME is its type. `{__id__: :shapes/circle, ...}` renders the
                // REPRESENTATION: `:__id__` is how nominal identity is *carried*, not
                // something anyone writes, so a mismatch read
                //   expects {__id__: :t/circle, ...}, got {__id__: :t/square, ...}
                // — which buries the one word that matters, twice, behind punctuation. Lead
                // with the identity in the spelling a `sig` takes (`expects t/circle, got
                // t/square`), and keep only the fields that say something the declaration
                // does not, so a refined shape still shows its refinement.
                if let Some(names) = nominal_ids(fields) {
                    let mut refined: Vec<String> = fields
                        .iter()
                        .filter(|(name, (ty, _))| {
                            value::symbol_name_ref(**name) != "__id__" && *ty != Ty::ANY
                        })
                        .map(|(name, (ty, required))| {
                            let mark = if *required { "" } else { "?" };
                            format!("{}{mark}: {ty}", value::symbol_name_ref(*name))
                        })
                        .collect();
                    refined.sort();
                    let joined = names.join(" | ");
                    if refined.is_empty() {
                        return write!(f, "{joined}");
                    }
                    return write!(f, "{joined}{{{}}}", refined.join(", "));
                }
                let mut parts: Vec<String> = fields
                    .iter()
                    .map(|(name, (ty, required))| {
                        let mark = if *required { "" } else { "?" };
                        format!("{}{mark}: {ty}", value::symbol_name_ref(*name))
                    })
                    .collect();
                parts.sort();
                // An **open** shape says so (ADR-264) — without the marker `{a: int}`
                // would read as "exactly this", which is what the *closed* shape means.
                if self.record_is_open() == Some(true) {
                    parts.push("...".to_string());
                }
                return write!(f, "{{{}}}", parts.join(", "));
            }
        }
        // A tuple shape: `(tuple int, string)` — matches the annotation
        // grammar directly, unlike a record's `{ }` shorthand.
        if let Some(elems) = self.tuple_elems() {
            if self.tags == VECTOR_BIT {
                let joined = elems
                    .iter()
                    .map(Ty::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                return write!(f, "(tuple {joined})");
            }
        }
        if let Some(elem) = self.elem_ty() {
            if self.tags & !(SEQ_BITS | (1u32 << bit(Tag::Nil))) == 0 {
                let kinds: Vec<&str> = [
                    (Tag::Pair, "list"),
                    (Tag::Vector, "vector"),
                    (Tag::Set, "set"),
                ]
                .iter()
                .filter(|(tag, _)| self.contains_tag(*tag))
                .map(|(_, name)| *name)
                .collect();
                let nil = if self.contains_tag(Tag::Nil) && !kinds.is_empty() {
                    "nil | "
                } else {
                    ""
                };
                match kinds.as_slice() {
                    [] => {}
                    [one] => return write!(f, "{nil}{one}<{elem}>"),
                    many => return write!(f, "{nil}({})<{elem}>", many.join(" | ")),
                }
            }
        }
        // A literal type: the enumerated keywords (`:a | :b`), ints (`5 | 6`),
        // bools (`true`), and/or strings (`"a" | "b"`) — any combination may
        // be present at once (`(or :ok 5)`, independent tags/fields) — plus
        // any other tag this type also admits (`:a | nil`). Keywords sorted
        // by name (stable regardless of intern order); ints numerically;
        // bools/strings lexicographically.
        if let Some(negation) = self.near_universe_negation() {
            return f.write_str(&negation);
        }
        if self.lit.is_some()
            || self.lit_int.is_some()
            || self.lit_bool.is_some()
            || self.lit_str.is_some()
        {
            let mut kw_parts: Vec<String> = self
                .lit
                .iter()
                .flat_map(|set| set.members())
                .flat_map(|set| set.iter())
                .map(|s| format!(":{}", value::symbol_name_ref(*s)))
                .collect();
            kw_parts.sort();
            let mut int_parts: Vec<String> = self
                .lit_int
                .iter()
                .flat_map(|set| set.members())
                .flat_map(|set| set.iter())
                .map(|n| n.to_string())
                .collect();
            int_parts.sort_by_key(|s| s.parse::<i64>().unwrap());
            let mut bool_parts: Vec<String> = self
                .lit_bool
                .iter()
                .flat_map(|set| set.members())
                .flat_map(|set| set.iter())
                .map(|b| b.to_string())
                .collect();
            bool_parts.sort();
            let mut str_parts: Vec<String> = self
                .lit_str
                .iter()
                .flat_map(|set| set.members())
                .flat_map(|set| set.iter())
                .map(|s| format!("{s:?}"))
                .collect();
            str_parts.sort();
            // A **negatively** stated set — `¬:ok`, the else-branch of an equality
            // test. It cannot be enumerated, so it renders as what it is. When the type
            // is otherwise everything, that is exactly `(not :ok)`; when the tag has
            // been narrowed alongside it, `(keyword and (not :ok))` says both halves.
            // Rendering nothing here would be the dangerous option: the reader would see
            // a union with the keyword member silently missing.
            let excluded: Vec<(Tag, Vec<String>)> = vec![
                (
                    Tag::Keyword,
                    excluded_of(&self.lit, |s| format!(":{}", value::symbol_name_ref(*s))),
                ),
                (Tag::Int, excluded_of(&self.lit_int, |n| n.to_string())),
                (Tag::Bool, excluded_of(&self.lit_bool, |b| b.to_string())),
                (Tag::Str, excluded_of(&self.lit_str, |s| format!("{s:?}"))),
            ];
            let negated_tags: u32 = excluded
                .iter()
                .filter(|(_, values)| !values.is_empty())
                .fold(0, |acc, (tag, _)| acc | 1u32 << bit(*tag));

            // The pure complement of one literal: every tag, nothing else refined.
            if negated_tags != 0 && self.tags == UNIVERSE {
                let all: Vec<String> = excluded
                    .iter()
                    .flat_map(|(_, values)| values.iter().cloned())
                    .collect();
                if all.len() == 1 {
                    return write!(f, "(not {})", all[0]);
                }
                return write!(f, "(not ({}))", all.join(" | "));
            }

            let mut parts = kw_parts;
            parts.extend(int_parts);
            parts.extend(bool_parts);
            parts.extend(str_parts);
            for (tag, values) in &excluded {
                if values.is_empty() {
                    continue;
                }
                let inner = if values.len() == 1 {
                    values[0].clone()
                } else {
                    format!("({})", values.join(" | "))
                };
                parts.push(format!("({} and (not {}))", tag.name(), inner));
            }
            for tag in ALL_TAGS {
                let is_literal_tag = (tag as u8 as u32 == bit(Tag::Keyword) && self.lit.is_some())
                    || (tag as u8 as u32 == bit(Tag::Int) && self.lit_int.is_some())
                    || (tag as u8 as u32 == bit(Tag::Bool) && self.lit_bool.is_some())
                    || (tag as u8 as u32 == bit(Tag::Str) && self.lit_str.is_some());
                if !is_literal_tag && self.contains_tag(tag) {
                    parts.push(tag.name().to_string());
                }
            }
            return f.write_str(&parts.join(" | "));
        }
        // A **complement**: a pure tag union that omits only a handful of tags is what
        // negation produces (`¬string`, the else-branch of a `(string? x)` guard), and
        // spelling out the twenty-two tags it *does* admit tells the reader nothing —
        if let Some(negation) = self.near_universe_negation() {
            return f.write_str(&negation);
        }
        // Factor the `number` alias out of a *larger* pure-tag union: a type that admits
        // every `number` member (int, float, decimal) plus something else — e.g. the
        // arithmetic operators' argument domain `number | map` (records participating via
        // the `Num` ability) — renders as `number | map`, not `int | float | map | decimal`.
        // (An exact `number` is already named above; this only fires for a strict superset.)
        let number_tags = Ty::NUMBER.tags;
        let factor_number = (self.tags & number_tags) == number_tags && self.tags != number_tags;
        // …and the same for `fn`. The `Fn`/`Native` split is an implementation detail the
        // LANGUAGE does not have: `(type-of inc)` is `:fn`, `(fn? inc)` is true for a
        // builtin and a closure alike, and the type grammar's `fn` already parses to both
        // members. Only the renderers still spelled them apart, which is how a warning
        // came to read `expects keyword | fn | native` — naming a kind no Brood program
        // can observe or write down.
        // Unlike `number`, this fires for the EXACT pair too — there is no earlier
        // exact-match arm naming it, and reaching here with exactly `FN_BITS` means the
        // type carries no arrow refinement (that case returned above), so it is plainly
        // `fn`.
        let fn_tags = FN_BITS;
        let factor_fn = (self.tags & fn_tags) == fn_tags;
        let mut first = true;
        let mut number_emitted = false;
        let mut fn_emitted = false;
        for tag in ALL_TAGS {
            if self.contains_tag(tag) {
                // Collapse the number members into a single `number` token at the first one.
                if factor_number && (1u32 << bit(tag)) & number_tags != 0 {
                    if number_emitted {
                        continue;
                    }
                    number_emitted = true;
                    if !first {
                        f.write_str(" | ")?;
                    }
                    first = false;
                    f.write_str("number")?;
                    continue;
                }
                if factor_fn && (1u32 << bit(tag)) & fn_tags != 0 {
                    if fn_emitted {
                        continue;
                    }
                    fn_emitted = true;
                    if !first {
                        f.write_str(" | ")?;
                    }
                    first = false;
                    f.write_str("fn")?;
                    continue;
                }
                if !first {
                    f.write_str(" | ")?;
                }
                first = false;
                f.write_str(tag.name())?;
            }
        }
        Ok(())
    }
}

impl Ty {
    /// This type as **source syntax** — the inverse of `annot::parse_type`, so the
    /// result can be pasted into a `(sig …)` and read back as the same type.
    ///
    /// `None` when the type cannot be written faithfully: a `macro`/`native` member has
    /// no spelling in the grammar, and an inferred arrow inside a parameter position is
    /// not something the round-trip preserves. Returning `None` rather than an
    /// approximation is the point — a quick-fix that writes a *different* type than the
    /// one it showed would be worse than no quick-fix.
    ///
    /// [`Display`] is the diagnostic rendering and deliberately reads differently
    /// (`vector<int>`, `{a: int}`); this is the one that has to parse.
    pub fn to_source(&self) -> Option<String> {
        // A term that SUBTRACTS (ADR-288) is written with the `(not …)` the grammar
        // already has. Falling through would emit the positive part alone, which is
        // strictly WIDER than the type — and ADR-271's rule is that a suggestion must
        // never claim a different type than the checker meant.
        if let Some(negs) = self.subtracted() {
            if self.term_is_empty_for_display() {
                return Some("never".to_string());
            }
            let inner = if negs.len() == 1 {
                negs[0].to_source()?
            } else {
                let parts: Vec<String> =
                    negs.iter().map(Ty::to_source).collect::<Option<Vec<_>>>()?;
                format!("(or {})", parts.join(" "))
            };
            let positive = self.positive_for_display();
            if positive.is_any() {
                return Some(format!("(not {inner})"));
            }
            return Some(format!("(and {} (not {inner}))", positive.to_source()?));
        }
        // A union of terms: `(or …)` over each.
        if let Some(terms) = self.alt_terms() {
            let parts: Vec<String> = terms
                .iter()
                .map(Ty::to_source)
                .collect::<Option<Vec<_>>>()?;
            return Some(format!("({} {})", "or", parts.join(" ")));
        }
        if *self == Ty::NEVER {
            return Some("never".to_string());
        }
        if *self == Ty::ANY {
            return Some("any".to_string());
        }
        if *self == Ty::NUMBER {
            return Some("number".to_string());
        }
        if *self == Ty::LIST {
            return Some("list".to_string());
        }
        if *self == Ty::COUNTABLE {
            return Some("countable".to_string());
        }
        // A named cover is always spelled by its name in a `sig`: `(or number t/usd)` goes
        // stale the day another record gains a `num/add` method; `numeric` does not.
        if let Some((name, _)) = super::check::cover_name_of(self) {
            return Some(name.to_string());
        }
        if *self == Ty::SEQABLE {
            return Some("seqable".to_string());
        }
        // Structured refinements, each of which owns its whole tag set.
        if self.tags & !FN_BITS == 0 {
            if let Some(sig) = self.as_arrow() {
                return sig.to_source();
            }
            if let Some(sigs) = self.overload_sigs() {
                let parts: Vec<String> = sigs
                    .iter()
                    .map(Sig::to_source)
                    .collect::<Option<Vec<_>>>()?;
                return Some(format!("(and {})", parts.join(" ")));
            }
        }
        // A record riding in a wider term (`number | t/usd`, an operator's domain once a
        // record has a `num/add` method — ADR-299): spell the map member by the record
        // rule below and the rest by theirs, as one flat `(or …)`.
        if self.record_fields().is_some()
            && self.neg.is_none()
            && self.tags != MAP_BIT
            && self.tags & MAP_BIT != 0
        {
            let rest = (0..32)
                .map(|i| 1u32 << i)
                .filter(|b| self.tags & b != 0 && *b != MAP_BIT)
                .fold(Ty::NEVER, |acc, b| acc.union(self.project_tag(b)));
            // Both halves may be unions of their own; one flat `(or …)` reads best.
            fn flat(source: String) -> String {
                source
                    .strip_prefix("(or ")
                    .and_then(|r| r.strip_suffix(')'))
                    .map(str::to_string)
                    .unwrap_or(source)
            }
            let rest = flat(rest.to_source()?);
            let record = flat(self.project_tag(MAP_BIT).to_source()?);
            return Some(format!("(or {rest} {record})"));
        }
        if self.tags == MAP_BIT {
            if let Some((k, v)) = self.map_kv() {
                return Some(format!("(map {} {})", k.to_source()?, v.to_source()?));
            }
            if let Some(fields) = self.record_fields() {
                // A record's NAME is its type in a `sig` (`(sig area (t/circle -> float))`),
                // so a nominal shape is spelled by its ids — the `:__id__` representation is
                // not something anyone writes. Field refinements the checker inferred
                // (`utc-now` returning `datetime{year: int, …}`) are dropped here on
                // purpose: the name denotes the open `:__id__` shape, a supertype, so the
                // suggested declaration stays sound, and it is the declaration a reader
                // would write. `Display` keeps the refinements, where a diagnostic wants
                // them.
                if let Some(names) = nominal_ids(fields) {
                    return Some(if names.len() == 1 {
                        names[0].clone()
                    } else {
                        format!("(or {})", names.join(" "))
                    });
                }
                let open = if self.record_is_open() == Some(true) {
                    " &open"
                } else {
                    ""
                };
                let mut parts: Vec<String> = Vec::with_capacity(fields.len());
                for (name, (ty, required)) in fields {
                    let inner = ty.to_source()?;
                    let rendered = if *required {
                        inner
                    } else {
                        format!("(optional {inner})")
                    };
                    parts.push(format!(":{} {rendered}", value::symbol_name_ref(*name)));
                }
                parts.sort();
                return Some(format!("(record{open} {})", parts.join(" ")));
            }
        }
        if self.tags == VECTOR_BIT {
            if let Some(elems) = self.tuple_elems() {
                let parts: Vec<String> = elems
                    .iter()
                    .map(Ty::to_source)
                    .collect::<Option<Vec<_>>>()?;
                return Some(format!("(tuple {})", parts.join(" ")));
            }
        }
        if let Some(elem) = self.elem_ty() {
            // An element refinement describes the `pair`/`vector` members, and the type
            // may carry `nil` beside them — `nil | list<int>` is what every sequence
            // combinator returns. Render each member with its element type rather than
            // falling through to the bare tag join, which silently dropped the
            // refinement (caught by the round-trip test, which is why it exists).
            let nil_bit = 1u32 << bit(Tag::Nil);
            if self.tags & !(SEQ_BITS | nil_bit) == 0 {
                let inner = elem.to_source()?;
                let mut parts: Vec<String> = Vec::new();
                if self.contains_tag(Tag::Nil) {
                    parts.push("nil".to_string());
                }
                if self.contains_tag(Tag::Pair) {
                    parts.push(format!("(list {inner})"));
                }
                if self.contains_tag(Tag::Vector) {
                    parts.push(format!("(vector {inner})"));
                }
                if self.contains_tag(Tag::Set) {
                    parts.push(format!("(set {inner})"));
                }
                return match parts.len() {
                    0 => None,
                    1 => Some(parts.remove(0)),
                    _ => Some(format!("(or {})", parts.join(" "))),
                };
            }
        }
        // A structural refinement (element type, tuple shape, record shape, map
        // key/value, arrow) constrains only its own tag family. When the term carries
        // tags *outside* that family, none of the branches above fire and the
        // fall-through renders bare tag names — silently dropping the refinement.
        // `int | vector<int>` is exactly such a term: the union merged exactly, so it
        // is a single term rather than an `alts` case, and it used to render as
        // `(or int vector)`. Split it into per-tag projections and let each render
        // itself; a projection has one tag, so this recursion terminates.
        let mut structural_tags = 0u32;
        if self.elem.is_some() {
            structural_tags |= SEQ_BITS;
        }
        if self.tuple.is_some() {
            structural_tags |= VECTOR_BIT;
        }
        if self.map_kv.is_some() || self.fields.is_some() {
            structural_tags |= MAP_BIT;
        }
        if self.arrow.is_some() || self.overload.is_some() {
            structural_tags |= FN_BITS;
        }
        if structural_tags != 0 && self.tags & !structural_tags != 0 {
            // Tag-table iteration, not bit isolation — see the note in
            // `term_is_subtype_of_union`: the lint's suggested `isolate_lowest_one` is
            // unstable below Rust 1.98 and this crate builds on 1.95.
            let mut parts: Vec<String> = Vec::new();
            for tag in ALL_TAGS {
                let tag_bit = 1u32 << bit(tag);
                if self.tags & tag_bit == 0 {
                    continue;
                }
                parts.push(self.project_tag(tag_bit).to_source()?);
            }
            parts.sort();
            return match parts.len() {
                0 => None,
                1 => Some(parts.remove(0)),
                _ => Some(format!("(or {})", parts.join(" "))),
            };
        }

        // Otherwise: the members, as literals where a literal set pins them and as tag
        // names elsewhere, joined with `or`. The named unions are factored out first —
        // `Display` does this and source has to as well, or the arithmetic domain
        // (`number | map`, which records join through the `Num` ability) renders as a
        // five-way `or` of its members. A generated `(sig …)` is read by people.
        // A negatively stated literal set — `¬:ok`, what an equality guard's else
        // branch produces. `to_source` must render it or decline; falling through would
        // emit the bare tag name and silently widen the type it claims to write.
        let excluded: Vec<String> = [
            excluded_of(&self.lit, |s| format!(":{}", value::symbol_name_ref(*s))),
            excluded_of(&self.lit_int, |n| n.to_string()),
            excluded_of(&self.lit_bool, |b| b.to_string()),
            excluded_of(&self.lit_str, |s| format!("{s:?}")),
        ]
        .concat();
        if !excluded.is_empty() {
            let inner = if excluded.len() == 1 {
                excluded[0].clone()
            } else {
                format!("(or {})", excluded.join(" "))
            };
            // The whole universe minus those values is exactly `(not …)`. A narrowed
            // one — a single tag, minus values — is that intersected with the tag.
            if self.tags == UNIVERSE {
                return Some(format!("(not {inner})"));
            }
            let mut tags = ALL_TAGS
                .iter()
                .filter(|&&tag| self.contains_tag(tag))
                .map(|tag| tag.name());
            let (Some(only), None) = (tags.next(), tags.next()) else {
                return None; // a partial universe minus literals: no faithful spelling
            };
            return Some(format!("(and {only} (not {inner}))"));
        }

        let mut parts: Vec<String> = Vec::new();
        let mut named_tags = 0u32;
        for (named, source) in [
            // the wider name first: `countable` contains `seqable`
            (Ty::COUNTABLE, "countable"),
            (Ty::SEQABLE, "seqable"),
            (Ty::NUMBER, "number"),
            (Ty::LIST, "list"),
            // `fn` covers both function members, which is what the grammar parses it to
            // and what the language means by a function. Without it `to_source` hit
            // `Tag::Native` and DECLINED, so the callable type ADR-272 infers for every
            // callback parameter had no faithful spelling and the declare-sig surfaces
            // could not offer it.
            (Ty::of(Tag::Fn).union(Ty::of(Tag::Native)), "fn"),
        ] {
            // Only when the whole named union is present and nothing has pinned one of
            // its members to a literal set (which would make the alias a lie).
            let members = named.tags;
            if self.tags & members == members
                && named_tags & members == 0
                && !(members & INT_BIT != 0 && self.lit_int.is_some())
            {
                named_tags |= members;
                parts.push(source.to_string());
            }
        }
        for set in self.as_lit().iter() {
            for k in set.iter() {
                parts.push(format!(":{}", value::symbol_name_ref(*k)));
            }
        }
        for set in self.as_lit_int().iter() {
            for n in set.iter() {
                parts.push(n.to_string());
            }
        }
        for set in self.as_lit_bool().iter() {
            for b in set.iter() {
                parts.push(b.to_string());
            }
        }
        for set in self.as_lit_str().iter() {
            for t in set.iter() {
                parts.push(format!("{t:?}"));
            }
        }
        for tag in ALL_TAGS {
            if !self.contains_tag(tag) || named_tags & (1u32 << bit(tag)) != 0 {
                continue;
            }
            let pinned = (tag == Tag::Keyword && self.as_lit().is_some())
                || (tag == Tag::Int && self.as_lit_int().is_some())
                || (tag == Tag::Bool && self.as_lit_bool().is_some())
                || (tag == Tag::Str && self.as_lit_str().is_some());
            if pinned {
                continue;
            }
            // `macro` and `native` are runtime kinds the type grammar cannot name.
            if matches!(tag, Tag::Macro | Tag::Native) {
                return None;
            }
            parts.push(tag.name().to_string());
        }
        parts.sort();
        match parts.len() {
            0 => None,
            1 => Some(parts.remove(0)),
            _ => Some(format!("(or {})", parts.join(" "))),
        }
    }
}

impl Sig {
    /// This signature as source — `(P… -> R)`, the shape a `(sig …)` takes. `None`
    /// when any part cannot be written faithfully (see [`Ty::to_source`]).
    pub fn to_source(&self) -> Option<String> {
        let mut parts: Vec<String> = self
            .params
            .iter()
            .map(Ty::to_source)
            .collect::<Option<Vec<_>>>()?;
        if !self.optional.is_empty() {
            parts.push("&optional".to_string());
            for o in &self.optional {
                parts.push(o.to_source()?);
            }
        }
        if let Some(rest) = &self.rest {
            parts.push("&".to_string());
            parts.push(rest.to_source()?);
        }
        parts.push("->".to_string());
        parts.push(self.ret.to_source()?);
        Some(format!("({})", parts.join(" ")))
    }
}

impl Ty {
    /// A small complement rendered as what it is — `(not string)`, `(not (nil | false))` —
    /// see the comment inside; `None` for an ordinary union.
    fn near_universe_negation(&self) -> Option<String> {
        // `expects string, got nil | bool | number | symbol | keyword | pair | vector |
        // fn | macro | native | map | ref | pid | rope | socket | subprocess | table |
        // bytes | set` was a real diagnostic. Say what it is instead: `not string`.
        // Only for a genuinely small complement (at most three omitted tags, and far
        // past half the universe), so an ordinary wide union still renders as a union.
        let missing = UNIVERSE & !self.tags;
        // `bool` narrowed to `{true}` is `false` omitted — the truthy half of an
        // `(or x default)` — and reads as `(not (nil | false))`, not as a 21-tag list.
        let false_omitted = self
            .lit_bool
            .as_ref()
            .and_then(|set| set.members())
            .is_some_and(|m| m.len() == 1 && m.contains(&true))
            && Ty {
                lit_bool: None,
                ..self.clone()
            }
            .is_flat();
        let omitted_count = missing.count_ones() + u32::from(false_omitted);
        if (self.is_flat() || false_omitted)
            && (1..=3).contains(&omitted_count)
            && self.tags.count_ones() >= TAG_COUNT - 4
        {
            let mut omitted: Vec<&str> = ALL_TAGS
                .iter()
                .filter(|&&tag| missing & (1u32 << bit(tag)) != 0)
                .map(|tag| tag.name())
                .collect();
            if false_omitted {
                omitted.push("false");
            }
            // Parenthesised, matching the `(not T)` annotation grammar exactly — and
            // reading as a *name* inside a message ("got (not string)") rather than as
            // a negated sentence ("got not string").
            return Some(if omitted.len() == 1 {
                format!("(not {})", omitted[0])
            } else {
                format!("(not ({}))", omitted.join(" | "))
            });
        }
        None
    }
}
