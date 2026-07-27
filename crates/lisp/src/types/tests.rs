use super::*;
use crate::core::value::Value;

#[test]
fn singletons_and_named_unions() {
    assert_eq!(
        Ty::NUMBER,
        Ty::of(Tag::Int)
            .union(Ty::of(Tag::Float))
            .union(Ty::of(Tag::Decimal))
    );
    assert_eq!(Ty::LIST, Ty::of(Tag::Nil).union(Ty::of(Tag::Pair)));
    assert!(Ty::of(Tag::Int).contains_tag(Tag::Int));
    assert!(!Ty::of(Tag::Int).contains_tag(Tag::Float));
}

#[test]
fn subtyping_is_set_inclusion() {
    assert!(Ty::of(Tag::Int).is_subtype(&Ty::NUMBER)); // int ⊆ number
    assert!(Ty::NUMBER.is_subtype(&Ty::ANY)); // number ⊆ any
    assert!(!Ty::NUMBER.is_subtype(&Ty::of(Tag::Int))); // number ⊄ int
                                                        // ⊥ is a subtype of everything; everything is a subtype of ⊤.
    assert!(Ty::NEVER.is_subtype(&Ty::of(Tag::Str)));
    assert!(Ty::of(Tag::Str).is_subtype(&Ty::ANY));
    assert!(Ty::of(Tag::Int).is_subtype(&Ty::of(Tag::Int))); // reflexive
}

#[test]
fn intersection_and_disjointness() {
    assert_eq!(Ty::NUMBER.intersect(Ty::of(Tag::Int)), Ty::of(Tag::Int));
    assert_eq!(Ty::NUMBER.intersect(Ty::of(Tag::Str)), Ty::NEVER);
    assert!(Ty::NUMBER.is_disjoint(&Ty::LIST));
    assert!(!Ty::NUMBER.is_disjoint(&Ty::of(Tag::Float)));
}

#[test]
fn negation_and_difference() {
    assert_eq!(Ty::NEVER.negate(), Ty::ANY);
    assert_eq!(Ty::ANY.negate(), Ty::NEVER);
    let not_nil = Ty::of(Tag::Nil).negate();
    assert!(!not_nil.contains_tag(Tag::Nil));
    assert!(not_nil.contains_tag(Tag::Int));
    // number \ int = float ∪ decimal
    assert_eq!(
        Ty::NUMBER.difference(Ty::of(Tag::Int)),
        Ty::of(Tag::Float).union(Ty::of(Tag::Decimal))
    );
}

#[test]
fn of_value_bridges_runtime_values() {
    // These Value variants are heap-free, so no Heap is needed. Int/bool carry
    // their *singleton* (B0 — literal-singleton precision), a subtype of the
    // flat tag; `nil` (no singleton refinement) stays flat.
    assert_eq!(Ty::of_value(Value::int(1)), Ty::int_lit(1));
    assert_eq!(Ty::of_value(Value::nil()), Ty::of(Tag::Nil));
    assert_eq!(Ty::of_value(Value::boolean(true)), Ty::bool_lit(true));
    // …and each singleton is still a subtype of its flat tag / of number.
    assert!(Ty::of_value(Value::int(1)).is_subtype(&Ty::of(Tag::Int)));
    assert!(Ty::of_value(Value::int(1)).is_subtype(&Ty::NUMBER));
    assert!(Ty::of_value(Value::boolean(true)).is_subtype(&Ty::of(Tag::Bool)));
}

#[test]
fn display_is_readable() {
    assert_eq!(Ty::NEVER.to_string(), "never");
    assert_eq!(Ty::ANY.to_string(), "any");
    assert_eq!(Ty::NUMBER.to_string(), "number");
    assert_eq!(Ty::LIST.to_string(), "list");
    assert_eq!(Ty::of(Tag::Int).to_string(), "int");
    assert_eq!(
        Ty::of(Tag::Int).union(Ty::of(Tag::Str)).to_string(),
        "int | string"
    );
}

#[test]
fn tested_by_maps_predicates_to_the_type_they_assert() {
    assert_eq!(Ty::tested_by("int?"), Some(Ty::of(Tag::Int)));
    assert_eq!(Ty::tested_by("number?"), Some(Ty::NUMBER));
    assert_eq!(Ty::tested_by("list?"), Some(Ty::LIST));
    assert_eq!(Ty::tested_by("nil?"), Some(Ty::of(Tag::Nil)));
    // fn? covers Brood closures and Rust builtins both.
    assert_eq!(
        Ty::tested_by("fn?"),
        Some(Ty::of(Tag::Fn).union(Ty::of(Tag::Native)))
    );
    // Non-tag predicates and unknown names don't narrow.
    assert_eq!(Ty::tested_by("empty?"), None);
    assert_eq!(Ty::tested_by("zero?"), None);
    assert_eq!(Ty::tested_by("frobnicate?"), None);
}

#[test]
fn single_tag_display_matches_tag_name() {
    // Contract point #9: a singleton Ty prints as its `type-of` / `Tag::name`
    // spelling, so a type named in a message reads the same as `type-of`
    // returns. (Locks errors / type-of / Ty against name drift.)
    for tag in ALL_TAGS {
        assert_eq!(Ty::of(tag).to_string(), tag.name());
    }
}

#[test]
fn tag_universe_is_consistent() {
    // Guards contract point #1: the bits, ALL_TAGS, and the universe size all
    // agree. `bit` is the `#[repr(u8)]` discriminant, so this also catches a
    // tag missing from (or misordered in) ALL_TAGS — the gap a plain
    // exhaustive match can't, since Rust can't enumerate enum variants.
    for (i, tag) in ALL_TAGS.iter().enumerate() {
        // ALL_TAGS is in discriminant/bit order, densely from 0.
        assert_eq!(
            bit(*tag),
            i as u32,
            "{} is out of order in ALL_TAGS",
            tag.name()
        );
        // Every atom's bit is inside the universe...
        assert!(bit(*tag) < TAG_COUNT);
        // ...so every singleton is a subtype of ANY (none falls outside ⊤).
        assert!(Ty::of(*tag).is_subtype(&Ty::ANY));
    }
    assert_eq!(
        UNIVERSE.count_ones(),
        TAG_COUNT,
        "universe must cover every atom"
    );
}

#[test]
fn pure_dynamic_is_consistent_with_every_inhabited_type() {
    let d = GradualTy::dynamic();
    assert!(d.is_dynamic());
    for t in [
        Ty::of(Tag::Int),
        Ty::NUMBER,
        Ty::of(Tag::Str),
        Ty::LIST,
        Ty::ANY,
    ] {
        assert!(
            d.consistent_with(t.clone()),
            "dynamic() should be consistent with {t}"
        );
    }
}

#[test]
fn bounded_dynamic_still_discriminates() {
    // dynamic(number) defers within numbers but is still caught against string.
    let dnum = GradualTy::dynamic_within(Ty::NUMBER);
    assert!(dnum.consistent_with(Ty::of(Tag::Int)));
    assert!(dnum.consistent_with(Ty::of(Tag::Float)));
    assert!(!dnum.consistent_with(Ty::of(Tag::Str)));
}

#[test]
fn static_gradual_is_plain_subtyping() {
    // Flag clear → consistent_with is exactly set inclusion.
    assert!(GradualTy::stat(Ty::of(Tag::Int)).consistent_with(Ty::NUMBER));
    assert!(!GradualTy::stat(Ty::NUMBER).consistent_with(Ty::of(Tag::Int)));
}

#[test]
fn composes_with_set_operations() {
    let g = GradualTy::dynamic_within(Ty::of(Tag::Int)).union(GradualTy::stat(Ty::of(Tag::Str)));
    assert_eq!(g.bound, Ty::of(Tag::Int).union(Ty::of(Tag::Str)));
    assert!(g.is_dynamic()); // dynamic propagates through the union
}

#[test]
fn static_union_stays_static() {
    let g = GradualTy::stat(Ty::of(Tag::Int)).union(GradualTy::stat(Ty::of(Tag::Str)));
    assert!(!g.is_dynamic());
}

#[test]
fn dynamic_vs_never_is_the_degenerate_case() {
    // Nothing inhabits NEVER, so even dynamic() can't be used there...
    assert!(!GradualTy::dynamic().consistent_with(Ty::NEVER));
    // ...while a *static* NEVER (⊥) is a subtype of every type.
    assert!(GradualTy::stat(Ty::NEVER).consistent_with(Ty::of(Tag::Int)));
}

// ---- the set algebra obeys the lattice laws, over a representative sample ----

// Deliberately **flat** types only — no refined (element-typed / arrow)
// types. `negate` widens a refinement (see `Ty::negate`, the doc at
// ~line 291), so double-negation and De Morgan are exact *only* for flat
// types and would fail here for a refined one. That widening is intentional
// (advisory soundness), so it's excluded from the laws and pinned on its own
// in `negate_of_a_refined_type_is_a_sound_overapproximation` /
// `double_negation_widens_a_refined_type`.
fn sample_tys() -> Vec<Ty> {
    let mut v = vec![Ty::NEVER, Ty::ANY, Ty::NUMBER, Ty::LIST];
    for t in ALL_TAGS {
        v.push(Ty::of(t));
    }
    v.push(Ty::of(Tag::Int).union(Ty::of(Tag::Str)));
    v.push(Ty::NUMBER.union(Ty::of(Tag::Nil)));
    v
}

#[test]
fn lattice_laws_hold() {
    // `Ty` is no longer `Copy` (the arrow refinement), so the by-value set
    // ops `.clone()` their operands here; the sample is all flat, so this is
    // exactly the pre-Step-5 algebra.
    let s = sample_tys();
    for a in &s {
        assert_eq!(a.clone().union(Ty::NEVER), *a, "∪⊥ identity");
        assert_eq!(a.clone().intersect(Ty::ANY), *a, "∩⊤ identity");
        assert_eq!(a.clone().union(a.clone()), *a, "∪ idempotent");
        assert_eq!(a.clone().intersect(a.clone()), *a, "∩ idempotent");
        assert_eq!(a.clone().union(a.clone().negate()), Ty::ANY, "complement ∪");
        assert_eq!(
            a.clone().intersect(a.clone().negate()),
            Ty::NEVER,
            "complement ∩"
        );
        assert_eq!(a.clone().negate().negate(), *a, "double negation");
        for b in &s {
            assert_eq!(
                a.clone().union(b.clone()),
                b.clone().union(a.clone()),
                "∪ commutes"
            );
            assert_eq!(
                a.clone().intersect(b.clone()),
                b.clone().intersect(a.clone()),
                "∩ commutes"
            );
            // subtyping IS set inclusion: a ⊆ b ⟺ a ∩ b = a
            assert_eq!(
                a.is_subtype(b),
                a.clone().intersect(b.clone()) == *a,
                "subtype ⟺ inclusion"
            );
            // disjoint IS empty intersection
            assert_eq!(
                a.is_disjoint(b),
                a.clone().intersect(b.clone()).is_never(),
                "disjoint ⟺ ∅"
            );
            // De Morgan
            assert_eq!(
                a.clone().union(b.clone()).negate(),
                a.clone().negate().intersect(b.clone().negate()),
                "De Morgan"
            );
        }
    }
}

#[test]
fn subtyping_is_reflexive_and_transitive() {
    let s = sample_tys();
    for a in &s {
        assert!(a.is_subtype(a));
        for b in &s {
            for c in &s {
                if a.is_subtype(b) && b.is_subtype(c) {
                    assert!(a.is_subtype(c), "subtype transitivity");
                }
            }
        }
    }
}

// ---- structured (arrow) types — Step 5+, ADR-078 ----

fn arr(params: Vec<Ty>, ret: Ty) -> Ty {
    Ty::arrow(Sig::new(params, ret))
}

#[test]
fn arrow_renders_as_an_arrow() {
    assert_eq!(
        arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int)).to_string(),
        "(int) -> int"
    );
    assert_eq!(
        arr(vec![Ty::of(Tag::Int), Ty::of(Tag::Str)], Ty::NUMBER).to_string(),
        "(int, string) -> number"
    );
    // A bare "any function" (no refinement) still prints as its tags.
    assert_eq!(
        Ty::of_tags(&[Tag::Fn, Tag::Native]).to_string(),
        "fn | native"
    );
}

#[test]
fn arrow_subtyping_is_contravariant_then_covariant() {
    // (number) -> int  <:  (int) -> number
    //   params contravariant: int ⊆ number ✓     result covariant: int ⊆ number ✓
    let wide_in_narrow_out = arr(vec![Ty::NUMBER], Ty::of(Tag::Int));
    let narrow_in_wide_out = arr(vec![Ty::of(Tag::Int)], Ty::NUMBER);
    assert!(wide_in_narrow_out.is_subtype(&narrow_in_wide_out));
    assert!(!narrow_in_wide_out.is_subtype(&wide_in_narrow_out));
    // an unrefined "any function" is not a subtype of a specific arrow
    let any_fn = Ty::of_tags(&[Tag::Fn, Tag::Native]);
    assert!(!any_fn.is_subtype(&narrow_in_wide_out));
    // ...but a specific arrow *is* a subtype of "any function"
    assert!(narrow_in_wide_out.is_subtype(&any_fn));
}

#[test]
fn arrow_arity_matters_for_subtyping() {
    let unary = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let binary = arr(vec![Ty::of(Tag::Int), Ty::of(Tag::Int)], Ty::of(Tag::Int));
    assert!(!unary.is_subtype(&binary));
    assert!(!binary.is_subtype(&unary));
}

#[test]
fn union_keeps_a_lone_arrow_but_widens_two() {
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let g = arr(vec![Ty::of(Tag::Str)], Ty::of(Tag::Str));
    // int ∪ (int -> int): only one side contributes functions → arrow survives.
    let mixed = Ty::of(Tag::Int).union(f.clone());
    assert!(mixed.contains_tag(Tag::Int));
    assert_eq!(mixed.as_arrow(), f.as_arrow());
    // two distinct arrows can't be one arrow → widen to "any function".
    let widened = f.clone().union(g);
    assert!(widened.contains_tag(Tag::Fn));
    assert_eq!(widened.as_arrow(), None);
}

#[test]
fn intersect_narrows_to_the_known_arrow() {
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let any_fn = Ty::of_tags(&[Tag::Fn, Tag::Native]); // unrefined
                                                       // refined ∩ any-function → keep the refinement (narrowing via fn? guard).
    assert_eq!(f.clone().intersect(any_fn).as_arrow(), f.as_arrow());
}

#[test]
fn disjointness_ignores_arrow_mismatch() {
    // Two incompatible arrows are still both functions — NOT disjoint, so the
    // advisory checker never raises a false positive off an arrow mismatch.
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let g = arr(vec![Ty::of(Tag::Str)], Ty::of(Tag::Str));
    assert!(!f.is_disjoint(&g));
    // a function and a non-function are disjoint (tags don't overlap).
    assert!(f.is_disjoint(&Ty::of(Tag::Int)));
}

// ---- overloaded arrows (intersection of arrows) — ADR-116 ----

#[test]
fn intersect_of_two_distinct_arrows_builds_an_overload() {
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let g = arr(vec![Ty::of(Tag::Bool)], Ty::of(Tag::Bool));
    // (int -> int) and (bool -> bool): two distinct sigs → a real overload,
    // not the old "widen to any function" behavior.
    let overloaded = f.clone().intersect(g.clone());
    assert_eq!(overloaded.as_arrow(), None);
    let sigs = overloaded.overload_sigs().expect("expected an overload");
    assert_eq!(sigs.len(), 2);
    assert!(sigs.contains(f.as_arrow().unwrap()));
    assert!(sigs.contains(g.as_arrow().unwrap()));
}

#[test]
fn intersect_of_identical_arrows_collapses_to_a_single_arrow() {
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let f_again = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    // Same backward-compatible collapse `merge_intersect` already gave —
    // two equal sigs are just one, no overload needed.
    let same = f.clone().intersect(f_again);
    assert_eq!(same.as_arrow(), f.as_arrow());
    assert_eq!(same.overload_sigs(), None);
}

#[test]
fn intersect_with_any_function_keeps_the_others_candidates_unchanged() {
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let g = arr(vec![Ty::of(Tag::Bool)], Ty::of(Tag::Bool));
    let overloaded = f.clone().intersect(g.clone());
    let any_fn = Ty::of_tags(&[Tag::Fn, Tag::Native]); // unrefined
                                                       // any_fn ∩ overloaded and overloaded ∩ any_fn both keep the overload
                                                       // untouched (one side contributes zero candidates).
    assert_eq!(
        any_fn.clone().intersect(overloaded.clone()).overload_sigs(),
        overloaded.overload_sigs()
    );
    assert_eq!(
        overloaded.clone().intersect(any_fn).overload_sigs(),
        overloaded.overload_sigs()
    );
}

#[test]
fn intersect_accumulates_three_distinct_arrows() {
    // (and (int->int) (bool->bool) (string->string)) — folding the
    // pairwise `intersect` the `(and A B C)` grammar already does.
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let g = arr(vec![Ty::of(Tag::Bool)], Ty::of(Tag::Bool));
    let h = arr(vec![Ty::of(Tag::Str)], Ty::of(Tag::Str));
    let acc = f.clone().intersect(g.clone()).intersect(h.clone());
    let sigs = acc.overload_sigs().expect("expected an overload");
    assert_eq!(sigs.len(), 3);
    for expected in [f, g, h] {
        assert!(sigs.contains(expected.as_arrow().unwrap()));
    }
}

#[test]
fn overload_renders_each_arm_joined_by_and() {
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let g = arr(vec![Ty::of(Tag::Bool)], Ty::of(Tag::Bool));
    assert_eq!(
        f.intersect(g).to_string(),
        "(int) -> int and (bool) -> bool"
    );
}

#[test]
fn overload_subtyping_is_conservative_but_sound() {
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let g = arr(vec![Ty::of(Tag::Bool)], Ty::of(Tag::Bool));
    let overloaded = f.clone().intersect(g.clone());
    // A value satisfying the overload also satisfies each arm on its own.
    assert!(overloaded.is_subtype(&f));
    assert!(overloaded.is_subtype(&g));
    // A single arrow is NOT a subtype of an overload requiring a second,
    // unrelated arm it doesn't carry.
    assert!(!f.is_subtype(&overloaded));
    // The overload is (trivially) a subtype of itself and of "any function".
    assert!(overloaded.is_subtype(&overloaded));
    let any_fn = Ty::of_tags(&[Tag::Fn, Tag::Native]);
    assert!(overloaded.is_subtype(&any_fn));
}

#[test]
fn overload_is_disjoint_only_on_tags_like_every_other_refinement() {
    let f = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    let g = arr(vec![Ty::of(Tag::Bool)], Ty::of(Tag::Bool));
    let h = arr(vec![Ty::of(Tag::Str)], Ty::of(Tag::Str));
    let overloaded = f.intersect(g);
    // Still both functions — never disjoint off a refinement mismatch.
    assert!(!overloaded.is_disjoint(&h));
}

// ---- structured (element) types — Step 5+, ADR-078 slice 2 ----

#[test]
fn sequence_types_render_with_element() {
    assert_eq!(Ty::vector_of(Ty::of(Tag::Int)).to_string(), "vector<int>");
    assert_eq!(Ty::list_of(Ty::NUMBER).to_string(), "list<number>");
    assert_eq!(
        Ty::vector_of(Ty::of(Tag::Int).union(Ty::of(Tag::Str))).to_string(),
        "vector<int | string>"
    );
    // a bare vector (no element refinement) still prints as its tag
    assert_eq!(Ty::of(Tag::Vector).to_string(), "vector");
    // `nil | list<E>` (the shape a `(map …)`/`(filter …)` result carries)
    // names the nil rather than hiding it.
    assert_eq!(
        Ty::list_of(Ty::of(Tag::Int))
            .union(Ty::of(Tag::Nil))
            .to_string(),
        "nil | list<int>"
    );
}

// ---- record/shape types — Step 5+, ADR-115 ----

fn rec(fields: &[(&str, Ty, bool)]) -> Ty {
    let mut m = BTreeMap::new();
    for (name, ty, required) in fields {
        m.insert(value::intern(name), (ty.clone(), *required));
    }
    Ty::record_of(m)
}

#[test]
fn record_renders_as_a_field_shape() {
    let r = rec(&[
        ("name", Ty::of(Tag::Str), true),
        ("age", Ty::of(Tag::Int), false),
    ]);
    // Sorted by field name, `?` marks the optional field.
    assert_eq!(r.to_string(), "{age?: int, name: string}");
    // A bare record with no fields renders as an empty shape.
    assert_eq!(rec(&[]).to_string(), "{}");
}

#[test]
fn record_subtyping_is_width_and_depth_but_conservative() {
    // Depth: a narrower field type is a subtype when both sides agree the
    // field is required.
    let narrow = rec(&[("a", Ty::of(Tag::Int), true)]);
    let wide = rec(&[("a", Ty::NUMBER, true)]);
    assert!(narrow.is_subtype(&wide));
    assert!(!wide.is_subtype(&narrow));

    // Width: extra fields self declares beyond what `other` requires are
    // fine (open records) — self may have MORE fields than other.
    let two_fields = rec(&[("a", Ty::of(Tag::Int), true), ("b", Ty::of(Tag::Str), true)]);
    let one_field = rec(&[("a", Ty::of(Tag::Int), true)]);
    assert!(two_fields.is_subtype(&one_field));
    // But not the reverse — `one_field` doesn't declare `b` at all, so it
    // can't prove it satisfies a shape requiring `b`.
    assert!(!one_field.is_subtype(&two_fields));

    // A required field in `other` must also be required in `self` — an
    // optional field isn't guaranteed present, so it can't satisfy a
    // required one.
    let a_optional = rec(&[("a", Ty::of(Tag::Int), false)]);
    let a_required = rec(&[("a", Ty::of(Tag::Int), true)]);
    assert!(!a_optional.is_subtype(&a_required));
    // The reverse holds: a required field trivially satisfies "optional".
    assert!(a_required.is_subtype(&a_optional));

    // Conservative-on-purpose: `self` not declaring a field `other` marks
    // merely *optional* still isn't provably a subtype (no attempt to
    // reason about absence) — sound (never claims a false subtype), just
    // incomplete.
    let bare = rec(&[]);
    assert!(!bare.is_subtype(&a_optional));
}

#[test]
fn record_is_a_subtype_of_map_with_keyword_keys() {
    // A closed record IS a map with keyword keys, so it satisfies a
    // `map<keyword, any>` annotation (config maps / option bags). Regression:
    // the checker used to flag `(config/window-mode {:fullscreen :maximized})`.
    let r = rec(&[("fullscreen", Ty::of_value(value::kw("maximized")), true)]);
    let map_kw_any = Ty::map_of(Ty::of(Tag::Keyword), Ty::ANY);
    assert!(r.is_subtype(&map_kw_any));
    assert!(rec(&[]).is_subtype(&map_kw_any)); // an empty record too

    // Depth still bites: each field value must fit V. A string-valued field
    // fits `map<keyword, string>` but not `map<keyword, int>`.
    let str_rec = rec(&[("k", Ty::of(Tag::Str), true)]);
    assert!(str_rec.is_subtype(&Ty::map_of(Ty::of(Tag::Keyword), Ty::of(Tag::Str))));
    assert!(!str_rec.is_subtype(&Ty::map_of(Ty::of(Tag::Keyword), Ty::of(Tag::Int))));
}

#[test]
fn record_union_widens_on_field_mismatch_but_keeps_a_match() {
    let a = rec(&[("a", Ty::of(Tag::Int), true)]);
    let a_again = rec(&[("a", Ty::of(Tag::Int), true)]);
    let b = rec(&[("b", Ty::of(Tag::Str), true)]);

    // Identical field maps survive a union unchanged.
    assert_eq!(a.clone().union(a_again).record_fields(), a.record_fields());
    // Distinct field maps widen to "no declared shape" — still sound (a
    // union is always a supertype, and dropping the refinement only
    // widens further), just less precise.
    assert!(a.union(b).record_fields().is_none());
}

#[test]
fn record_disjointness_needs_a_required_conflicting_field() {
    // Two records are disjoint when they both constrain a field, it's
    // *required* on at least one side (so any value must carry it), and the
    // field types are disjoint — no value can be `a: int` and `a: string`
    // at once. Sound, mirroring the tuple case (only ever adds a genuine
    // disjoint verdict).
    let a = rec(&[("a", Ty::of(Tag::Int), true)]);
    let b = rec(&[("a", Ty::of(Tag::Str), true)]);
    assert!(a.is_disjoint(&b));
    // NOT disjoint when the conflicting field is optional on *both* sides —
    // a value omitting `a` satisfies both open records.
    let ao = rec(&[("a", Ty::of(Tag::Int), false)]);
    let bo = rec(&[("a", Ty::of(Tag::Str), false)]);
    assert!(!ao.is_disjoint(&bo));
    // NOT disjoint when only one side mentions the field (open records let
    // the other carry the extra field freely).
    let just_b = rec(&[("b", Ty::of(Tag::Str), true)]);
    assert!(!a.is_disjoint(&just_b));
    // NOT disjoint when the shared field's types overlap (`int ⊆ number`).
    let anum = rec(&[("a", Ty::NUMBER, true)]);
    assert!(!a.is_disjoint(&anum));
}

#[test]
fn negate_of_a_refined_type_is_a_sound_overapproximation() {
    // ¬(vector<int>) must be a *superset* of the true complement, so it has
    // to KEEP the `vector` tag — vectors holding a non-int element are in the
    // complement. The earlier impl dropped the tag (a subset), which could
    // manufacture a false `is_disjoint`.
    let nvi = Ty::vector_of(Ty::of(Tag::Int)).negate();
    assert!(nvi.contains_tag(Tag::Vector), "must keep the refined tag");
    // ...so it is NOT disjoint from another vector type — no false positive.
    assert!(!nvi.is_disjoint(&Ty::vector_of(Ty::of(Tag::Str))));
    assert!(!nvi.is_disjoint(&Ty::of(Tag::Vector)));
    // and it still admits the obviously-complement tags.
    assert!(nvi.contains_tag(Tag::Int));
    // Same widening for an arrow refinement: keep both function tags.
    let narr = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int)).negate();
    assert!(narr.contains_tag(Tag::Fn) && narr.contains_tag(Tag::Native));
    // Flat negate is unchanged (exact): ¬int still excludes int.
    assert!(!Ty::of(Tag::Int).negate().contains_tag(Tag::Int));
}

#[test]
fn double_negation_widens_a_refined_type() {
    // Pins the documented exception the lattice-laws test deliberately can't
    // exercise (its `sample_tys` is flat-only): for a *refined* type the
    // widening in `negate` means double-negation does NOT round-trip.
    //
    // ¬(vector<int>) keeps the `vector` tag (a vector of non-ints is in the
    // complement) and adds every non-vector tag → that's `any`. ¬any = never.
    // So ¬¬(vector<int>) == never, neither the original nor a bare `vector`.
    let vi = Ty::vector_of(Ty::of(Tag::Int));
    let once = vi.clone().negate();
    assert_eq!(once, Ty::ANY, "¬(vector<int>) widens all the way to any");
    assert_eq!(once.negate(), Ty::NEVER, "…so ¬¬ collapses to never");
    assert_ne!(
        vi.clone().negate().negate(),
        vi,
        "double negation does NOT hold"
    );
    // The same collapse for an arrow refinement: ¬¬((int)->int) == never.
    let ai = arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int));
    assert_eq!(ai.clone().negate(), Ty::ANY);
    assert_eq!(ai.negate().negate(), Ty::NEVER);
}

#[test]
fn element_type_is_covariant_under_subtyping() {
    // vector<int> <: vector<number>  (int ⊆ number; immutable seqs are covariant)
    assert!(Ty::vector_of(Ty::of(Tag::Int)).is_subtype(&Ty::vector_of(Ty::NUMBER)));
    assert!(!Ty::vector_of(Ty::NUMBER).is_subtype(&Ty::vector_of(Ty::of(Tag::Int))));
    // a specific element type <: an unrefined vector ("any elements")
    assert!(Ty::vector_of(Ty::of(Tag::Int)).is_subtype(&Ty::of(Tag::Vector)));
    // ...but "any elements" is NOT a subtype of a specific element type
    assert!(!Ty::of(Tag::Vector).is_subtype(&Ty::vector_of(Ty::of(Tag::Int))));
    // different containers don't subtype (tags differ)
    assert!(!Ty::vector_of(Ty::of(Tag::Int)).is_subtype(&Ty::list_of(Ty::of(Tag::Int))));
}

#[test]
fn element_refinement_widens_on_a_union_mismatch_but_keeps_a_match() {
    let vi = Ty::vector_of(Ty::of(Tag::Int));
    let vs = Ty::vector_of(Ty::of(Tag::Str));
    // vector<int> ∪ vector<string> → vector (element widened; sound supertype)
    let u = vi.clone().union(vs);
    assert!(u.contains_tag(Tag::Vector));
    assert_eq!(u.elem_ty(), None);
    // vector<int> ∪ vector<int> → vector<int> (agree → kept)
    assert_eq!(vi.clone().union(vi.clone()).elem_ty(), vi.elem_ty());
    // int ∪ vector<int> → only the vector side contributes elements → kept
    let mixed = Ty::of(Tag::Int).union(vi.clone());
    assert!(mixed.contains_tag(Tag::Int) && mixed.contains_tag(Tag::Vector));
    assert_eq!(mixed.elem_ty(), vi.elem_ty());
}

#[test]
fn element_disjointness_is_tags_only() {
    // vector<int> and vector<string> overlap (both vectors) — not disjoint, so
    // no false positive off an element mismatch.
    assert!(!Ty::vector_of(Ty::of(Tag::Int)).is_disjoint(&Ty::vector_of(Ty::of(Tag::Str))));
    // a vector and an int are disjoint (tags don't overlap).
    assert!(Ty::vector_of(Ty::of(Tag::Int)).is_disjoint(&Ty::of(Tag::Int)));
}

// ---- keyword-literal (singleton) types — ADR, keyword-only slice ----

/// `(or :a :b)` as a `Ty` — the union of two keyword singletons.
fn kw_union(names: &[&str]) -> Ty {
    names
        .iter()
        .map(|n| Ty::keyword_lit(value::intern(n)))
        .reduce(|a, b| a.union(b))
        .unwrap()
}

#[test]
fn keyword_literal_renders_as_its_value() {
    assert_eq!(
        Ty::keyword_lit(value::intern("maximized")).to_string(),
        ":maximized"
    );
    // a union keeps both (set-union is exact, not a widening); rendered sorted.
    assert_eq!(kw_union(&["a", "b"]).to_string(), ":a | :b");
    // mixed with another tag: the literals plus the open tag.
    assert_eq!(
        kw_union(&["maximized", "fullscreen"])
            .union(Ty::of(Tag::Nil))
            .to_string(),
        ":fullscreen | :maximized | nil"
    );
}

#[test]
fn keyword_literal_union_is_exact_but_open_keyword_widens() {
    // {:a} ∪ {:b} = {:a, :b} — exact, both kept.
    let u = kw_union(&["a", "b"]);
    let mut want = BTreeSet::new();
    want.insert(value::intern("a"));
    want.insert(value::intern("b"));
    assert_eq!(u.as_lit(), Some(&want));
    // {:a} ∪ keyword(any) → any keyword (open side wins).
    let widened = Ty::keyword_lit(value::intern("a")).union(Ty::of(Tag::Keyword));
    assert!(widened.contains_tag(Tag::Keyword));
    assert_eq!(widened.as_lit(), None);
}

#[test]
fn keyword_literal_subtyping() {
    let ab = kw_union(&["a", "b"]);
    // :a <: (:a | :b)
    assert!(Ty::keyword_lit(value::intern("a")).is_subtype(&ab));
    // (:a | :b) <: keyword(any)
    assert!(ab.is_subtype(&Ty::of(Tag::Keyword)));
    // :c ⊄ (:a | :b)
    assert!(!Ty::keyword_lit(value::intern("c")).is_subtype(&ab));
    // any keyword ⊄ a specific literal set
    assert!(!Ty::of(Tag::Keyword).is_subtype(&ab));
}

#[test]
fn keyword_literal_disjointness_is_precise() {
    let ab = kw_union(&["a", "b"]);
    // :c is provably not one of (:a | :b) → disjoint → the checker can warn.
    assert!(Ty::keyword_lit(value::intern("c")).is_disjoint(&ab));
    // :a overlaps → not disjoint.
    assert!(!Ty::keyword_lit(value::intern("a")).is_disjoint(&ab));
    // any keyword could be :a → NOT provably disjoint (no false positive).
    assert!(!Ty::of(Tag::Keyword).is_disjoint(&ab));
    // a non-keyword is disjoint by tags as before.
    assert!(ab.is_disjoint(&Ty::of(Tag::Int)));
    // sharing another tag (nil) means not disjoint even if keywords differ.
    let c_or_nil = Ty::keyword_lit(value::intern("c")).union(Ty::of(Tag::Nil));
    let ab_or_nil = ab.clone().union(Ty::of(Tag::Nil));
    assert!(!c_or_nil.is_disjoint(&ab_or_nil));
}

#[test]
fn keyword_literal_intersection() {
    // (:a | :b) ∩ (:b | :c) = {:b}
    let inter = kw_union(&["a", "b"]).intersect(kw_union(&["b", "c"]));
    let mut want = BTreeSet::new();
    want.insert(value::intern("b"));
    assert_eq!(inter.as_lit(), Some(&want));
    // (:a) ∩ (:b) = never (empty literal set clears the keyword tag).
    let empty = Ty::keyword_lit(value::intern("a")).intersect(Ty::keyword_lit(value::intern("b")));
    assert!(empty.is_never());
    // (:a | :b) ∩ keyword(any) = (:a | :b) (narrower wins).
    let narrowed = kw_union(&["a", "b"]).intersect(Ty::of(Tag::Keyword));
    assert_eq!(narrowed.as_lit(), kw_union(&["a", "b"]).as_lit());
}

// ---- int-literal (singleton) types — ADR-117 ----

/// `(or 1 2)` as a `Ty` — the union of two int singletons.
fn int_union(ns: &[i64]) -> Ty {
    ns.iter()
        .map(|&n| Ty::int_lit(n))
        .reduce(|a, b| a.union(b))
        .unwrap()
}

#[test]
fn int_literal_renders_as_its_value() {
    assert_eq!(Ty::int_lit(5).to_string(), "5");
    // a union keeps both (set-union is exact, not a widening); rendered sorted.
    assert_eq!(int_union(&[404, 200]).to_string(), "200 | 404");
    // mixed with another tag: the literals plus the open tag.
    assert_eq!(
        int_union(&[404, 200]).union(Ty::of(Tag::Nil)).to_string(),
        "200 | 404 | nil"
    );
}

#[test]
fn int_literal_union_is_exact_but_open_int_widens() {
    // {5} ∪ {6} = {5, 6} — exact, both kept.
    let u = int_union(&[5, 6]);
    let mut want = BTreeSet::new();
    want.insert(5);
    want.insert(6);
    assert_eq!(u.as_lit_int(), Some(&want));
    // {5} ∪ int(any) → any int (open side wins).
    let widened = Ty::int_lit(5).union(Ty::of(Tag::Int));
    assert!(widened.contains_tag(Tag::Int));
    assert_eq!(widened.as_lit_int(), None);
}

#[test]
fn int_literal_subtyping() {
    let ab = int_union(&[5, 6]);
    // 5 <: (5 | 6)
    assert!(Ty::int_lit(5).is_subtype(&ab));
    // (5 | 6) <: int(any)
    assert!(ab.is_subtype(&Ty::of(Tag::Int)));
    // 7 ⊄ (5 | 6)
    assert!(!Ty::int_lit(7).is_subtype(&ab));
    // any int ⊄ a specific literal set
    assert!(!Ty::of(Tag::Int).is_subtype(&ab));
}

#[test]
fn int_literal_disjointness_is_precise() {
    let ab = int_union(&[5, 6]);
    // 7 is provably not one of (5 | 6) → disjoint → the checker can warn.
    assert!(Ty::int_lit(7).is_disjoint(&ab));
    // 5 overlaps → not disjoint.
    assert!(!Ty::int_lit(5).is_disjoint(&ab));
    // any int could be 5 → NOT provably disjoint (no false positive).
    assert!(!Ty::of(Tag::Int).is_disjoint(&ab));
    // a non-int is disjoint by tags as before.
    assert!(ab.is_disjoint(&Ty::of(Tag::Keyword)));
    // sharing another tag (nil) means not disjoint even if ints differ.
    let seven_or_nil = Ty::int_lit(7).union(Ty::of(Tag::Nil));
    let ab_or_nil = ab.clone().union(Ty::of(Tag::Nil));
    assert!(!seven_or_nil.is_disjoint(&ab_or_nil));
}

#[test]
fn int_literal_intersection() {
    // (5 | 6) ∩ (6 | 7) = {6}
    let inter = int_union(&[5, 6]).intersect(int_union(&[6, 7]));
    let mut want = BTreeSet::new();
    want.insert(6);
    assert_eq!(inter.as_lit_int(), Some(&want));
    // (5) ∩ (6) = never (empty literal set clears the int tag).
    let empty = Ty::int_lit(5).intersect(Ty::int_lit(6));
    assert!(empty.is_never());
    // (5 | 6) ∩ int(any) = (5 | 6) (narrower wins).
    let narrowed = int_union(&[5, 6]).intersect(Ty::of(Tag::Int));
    assert_eq!(narrowed.as_lit_int(), int_union(&[5, 6]).as_lit_int());
}

#[test]
fn keyword_and_int_literals_coexist_on_one_ty() {
    // (or :ok 5) — two independent literal-bearing tags on the same Ty,
    // with zero special-casing needed (different tag bits / fields).
    let mixed = Ty::keyword_lit(value::intern("ok")).union(Ty::int_lit(5));
    assert!(mixed.contains_tag(Tag::Keyword));
    assert!(mixed.contains_tag(Tag::Int));
    let mut want_kw = BTreeSet::new();
    want_kw.insert(value::intern("ok"));
    assert_eq!(mixed.as_lit(), Some(&want_kw));
    let mut want_int = BTreeSet::new();
    want_int.insert(5);
    assert_eq!(mixed.as_lit_int(), Some(&want_int));
    assert_eq!(mixed.to_string(), ":ok | 5");
    // Subtyping: :ok <: (or :ok 5), and 5 <: (or :ok 5).
    assert!(Ty::keyword_lit(value::intern("ok")).is_subtype(&mixed));
    assert!(Ty::int_lit(5).is_subtype(&mixed));
    // A different keyword or int is not a subtype.
    assert!(!Ty::keyword_lit(value::intern("no")).is_subtype(&mixed));
    assert!(!Ty::int_lit(6).is_subtype(&mixed));
}

// ---- bool-literal (singleton) types — ADR-120 ----

fn bool_union(bs: &[bool]) -> Ty {
    bs.iter()
        .map(|&b| Ty::bool_lit(b))
        .reduce(|a, b| a.union(b))
        .unwrap()
}

#[test]
fn bool_literal_renders_as_its_value() {
    assert_eq!(Ty::bool_lit(true).to_string(), "true");
    assert_eq!(Ty::bool_lit(false).to_string(), "false");
    assert_eq!(bool_union(&[true, false]).to_string(), "false | true");
}

#[test]
fn bool_literal_union_is_exact_but_open_bool_widens() {
    let u = bool_union(&[true, false]);
    let mut want = BTreeSet::new();
    want.insert(true);
    want.insert(false);
    assert_eq!(u.as_lit_bool(), Some(&want));
    let widened = Ty::bool_lit(true).union(Ty::of(Tag::Bool));
    assert!(widened.contains_tag(Tag::Bool));
    assert_eq!(widened.as_lit_bool(), None);
}

#[test]
fn bool_literal_subtyping() {
    let t = Ty::bool_lit(true);
    let both = bool_union(&[true, false]);
    assert!(t.is_subtype(&both));
    assert!(both.is_subtype(&Ty::of(Tag::Bool)));
    assert!(!Ty::bool_lit(false).is_subtype(&t));
    assert!(!Ty::of(Tag::Bool).is_subtype(&t));
}

#[test]
fn bool_literal_disjointness_is_precise() {
    let t = Ty::bool_lit(true);
    let f = Ty::bool_lit(false);
    assert!(t.is_disjoint(&f));
    assert!(!Ty::of(Tag::Bool).is_disjoint(&t));
    assert!(t.is_disjoint(&Ty::of(Tag::Int)));
}

#[test]
fn bool_literal_intersection() {
    let both = bool_union(&[true, false]);
    let inter = both.clone().intersect(Ty::bool_lit(true));
    assert_eq!(inter.as_lit_bool(), Ty::bool_lit(true).as_lit_bool());
    let empty = Ty::bool_lit(true).intersect(Ty::bool_lit(false));
    assert!(empty.is_never());
}

// ---- string-literal (singleton) types — ADR-120 ----

fn str_union(ss: &[&str]) -> Ty {
    ss.iter()
        .map(|&s| Ty::str_lit(s))
        .reduce(|a, b| a.union(b))
        .unwrap()
}

#[test]
fn str_literal_renders_as_its_value() {
    assert_eq!(Ty::str_lit("hi").to_string(), "\"hi\"");
    assert_eq!(str_union(&["b", "a"]).to_string(), "\"a\" | \"b\"");
}

#[test]
fn str_literal_union_is_exact_but_open_str_widens() {
    let u = str_union(&["a", "b"]);
    let mut want = BTreeSet::new();
    want.insert("a".to_string());
    want.insert("b".to_string());
    assert_eq!(u.as_lit_str(), Some(&want));
    let widened = Ty::str_lit("a").union(Ty::of(Tag::Str));
    assert!(widened.contains_tag(Tag::Str));
    assert_eq!(widened.as_lit_str(), None);
}

#[test]
fn str_literal_subtyping() {
    let ab = str_union(&["a", "b"]);
    assert!(Ty::str_lit("a").is_subtype(&ab));
    assert!(ab.is_subtype(&Ty::of(Tag::Str)));
    assert!(!Ty::str_lit("c").is_subtype(&ab));
    assert!(!Ty::of(Tag::Str).is_subtype(&ab));
}

#[test]
fn str_literal_disjointness_is_precise() {
    let ab = str_union(&["a", "b"]);
    assert!(Ty::str_lit("c").is_disjoint(&ab));
    assert!(!Ty::str_lit("a").is_disjoint(&ab));
    assert!(!Ty::of(Tag::Str).is_disjoint(&ab));
    assert!(ab.is_disjoint(&Ty::of(Tag::Int)));
}

#[test]
fn str_literal_intersection() {
    let inter = str_union(&["a", "b"]).intersect(str_union(&["b", "c"]));
    let mut want = BTreeSet::new();
    want.insert("b".to_string());
    assert_eq!(inter.as_lit_str(), Some(&want));
    let empty = Ty::str_lit("a").intersect(Ty::str_lit("b"));
    assert!(empty.is_never());
}

#[test]
fn of_value_makes_a_keyword_singleton() {
    let t = Ty::of_value(value::kw("maximized"));
    assert_eq!(t.to_string(), ":maximized");
    assert!(t.is_subtype(&Ty::of(Tag::Keyword)));
}

#[test]
fn inferred_type_size_is_bounded_ki13() {
    // Building a list-of-list-of-… far past the cap must WIDEN (drop the deep refinement),
    // never retain the whole tree — otherwise `==`/`Hash`/`is_subtype` (recursive over the
    // `Arc` refinement DAG) go superlinear and inference hangs (KI-13). `bounded` runs in
    // every `seq_of`, so after many nestings the type stays within `MAX_TY_NODES`.
    let mut t = Ty::of(Tag::Int);
    for _ in 0..2000 {
        t = Ty::list_of(t);
    }
    assert!(
        t.node_count(MAX_TY_NODES * 8) <= MAX_TY_NODES,
        "a deeply nested inferred type must stay within MAX_TY_NODES, got {}",
        t.node_count(MAX_TY_NODES * 8)
    );
}
