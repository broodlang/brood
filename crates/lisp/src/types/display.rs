//! Display rendering for [`Ty`] (extracted from mod.rs).
use super::*;

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
        // A record shape: `{name: string, age?: int}` — `?` marks an
        // optional field. `fields` is keyed by interned `Symbol` (intern
        // order, not alphabetical — same trap `lit` avoids below), so sort
        // by spelling for a stable rendering.
        if let Some(fields) = self.record_fields() {
            if self.tags == MAP_BIT {
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
                let has_vec = self.contains_tag(Tag::Vector);
                let has_pair = self.contains_tag(Tag::Pair);
                let nil = if self.contains_tag(Tag::Nil) && (has_vec || has_pair) {
                    "nil | "
                } else {
                    ""
                };
                if has_vec && !has_pair {
                    return write!(f, "{nil}vector<{elem}>");
                }
                if has_pair && !has_vec {
                    return write!(f, "{nil}list<{elem}>");
                }
                if has_vec && has_pair {
                    return write!(f, "{nil}(list | vector)<{elem}>");
                }
            }
        }
        // A literal type: the enumerated keywords (`:a | :b`), ints (`5 | 6`),
        // bools (`true`), and/or strings (`"a" | "b"`) — any combination may
        // be present at once (`(or :ok 5)`, independent tags/fields) — plus
        // any other tag this type also admits (`:a | nil`). Keywords sorted
        // by name (stable regardless of intern order); ints numerically;
        // bools/strings lexicographically.
        if self.lit.is_some()
            || self.lit_int.is_some()
            || self.lit_bool.is_some()
            || self.lit_str.is_some()
        {
            let mut kw_parts: Vec<String> = self
                .lit
                .iter()
                .flat_map(|set| set.iter())
                .map(|s| format!(":{}", value::symbol_name_ref(*s)))
                .collect();
            kw_parts.sort();
            let mut int_parts: Vec<String> = self
                .lit_int
                .iter()
                .flat_map(|set| set.iter())
                .map(|n| n.to_string())
                .collect();
            int_parts.sort_by_key(|s| s.parse::<i64>().unwrap());
            let mut bool_parts: Vec<String> = self
                .lit_bool
                .iter()
                .flat_map(|set| set.iter())
                .map(|b| b.to_string())
                .collect();
            bool_parts.sort();
            let mut str_parts: Vec<String> = self
                .lit_str
                .iter()
                .flat_map(|set| set.iter())
                .map(|s| format!("{s:?}"))
                .collect();
            str_parts.sort();
            let mut parts = kw_parts;
            parts.extend(int_parts);
            parts.extend(bool_parts);
            parts.extend(str_parts);
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
        // `expects string, got nil | bool | number | symbol | keyword | pair | vector |
        // fn | macro | native | map | ref | pid | rope | socket | subprocess | table |
        // bytes | set` was a real diagnostic. Say what it is instead: `not string`.
        // Only for a genuinely small complement (at most three omitted tags, and far
        // past half the universe), so an ordinary wide union still renders as a union.
        let missing = UNIVERSE & !self.tags;
        if self.is_flat()
            && (1..=3).contains(&missing.count_ones())
            && self.tags.count_ones() >= TAG_COUNT - 4
        {
            let omitted: Vec<&str> = ALL_TAGS
                .iter()
                .filter(|&&tag| missing & (1u32 << bit(tag)) != 0)
                .map(|tag| tag.name())
                .collect();
            // Parenthesised, matching the `(not T)` annotation grammar exactly — and
            // reading as a *name* inside a message ("got (not string)") rather than as
            // a negated sentence ("got not string").
            return if omitted.len() == 1 {
                write!(f, "(not {})", omitted[0])
            } else {
                write!(f, "(not ({}))", omitted.join(" | "))
            };
        }
        // Factor the `number` alias out of a *larger* pure-tag union: a type that admits
        // every `number` member (int, float, decimal) plus something else — e.g. the
        // arithmetic operators' argument domain `number | map` (records participating via
        // the `Num` ability) — renders as `number | map`, not `int | float | map | decimal`.
        // (An exact `number` is already named above; this only fires for a strict superset.)
        let number_tags = Ty::NUMBER.tags;
        let factor_number = (self.tags & number_tags) == number_tags && self.tags != number_tags;
        let mut first = true;
        let mut number_emitted = false;
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
        if self.tags == MAP_BIT {
            if let Some((k, v)) = self.map_kv() {
                return Some(format!("(map {} {})", k.to_source()?, v.to_source()?));
            }
            if let Some(fields) = self.record_fields() {
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
                return match parts.len() {
                    0 => None,
                    1 => Some(parts.remove(0)),
                    _ => Some(format!("(or {})", parts.join(" "))),
                };
            }
        }
        // Otherwise: the members, as literals where a literal set pins them and as tag
        // names elsewhere, joined with `or`.
        let mut parts: Vec<String> = Vec::new();
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
            if !self.contains_tag(tag) {
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
