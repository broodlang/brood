//! Display rendering for [`Ty`] (extracted from mod.rs).
use super::*;

impl fmt::Display for Ty {
    /// A readable rendering for diagnostics: the named lattice points where they
    /// apply (`never`, `any`, `number`, `list`), a single tag by its `type-of`
    /// name, otherwise the members joined with ` | ` (e.g. `int | string`). A
    /// purely-function type with a known arrow renders as `(p1, p2) -> ret`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
