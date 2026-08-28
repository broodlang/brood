use super::*;
use crate::core::value::Value;

#[test]
fn singletons_and_named_unions() {
    assert_eq!(
        Ty::NUMBER,
        Ty::of(Tag::Int)
            .union(Ty::of(Tag::Float))
            .union(Ty::of(Tag::Decimal))
            .union(Ty::of(Tag::Ratio))
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
    // number \ int = float ∪ decimal ∪ ratio  (ratio joined NUMBER with ADR-196)
    assert_eq!(
        Ty::NUMBER.difference(Ty::of(Tag::Int)),
        Ty::of(Tag::Float)
            .union(Ty::of(Tag::Decimal))
            .union(Ty::of(Tag::Ratio))
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
    // A bare "any function" (no refinement) prints as the one word the language has.
    // It used to print `fn | native`, naming a kind no Brood program can observe:
    // `(type-of inc)` is `:fn` and `(fn? inc)` is true for a builtin and a closure
    // alike. See `the_two_function_members_render_as_the_one_word_the_language_has`.
    assert_eq!(Ty::of_tags(&[Tag::Fn, Tag::Native]).to_string(), "fn");
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
    // two distinct arrows can't be ONE arrow — the union keeps both terms, and the
    // single-arrow accessor reports none for it (an *intersection* of arrows is what
    // `overload_of` is for; this is a union).
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
    Ty::record_of(field_map(fields))
}

/// The `(record &open …)` counterpart — a shape that admits keys it doesn't declare.
fn rec_open(fields: &[(&str, Ty, bool)]) -> Ty {
    Ty::record_of_open(field_map(fields))
}

fn field_map(fields: &[(&str, Ty, bool)]) -> BTreeMap<value::Symbol, (Ty, bool)> {
    let mut m = BTreeMap::new();
    for (name, ty, required) in fields {
        m.insert(value::intern(name), (ty.clone(), *required));
    }
    m
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

    // Width: extra fields are fine when the supertype is **open** — that is what open
    // means. Into a CLOSED supertype they are not (ADR-264): `{a: int}` closed says
    // `:b` is absent, and a value carrying `:b` is not one of its values.
    let two_fields = rec(&[("a", Ty::of(Tag::Int), true), ("b", Ty::of(Tag::Str), true)]);
    let one_field = rec(&[("a", Ty::of(Tag::Int), true)]);
    let one_field_open = rec_open(&[("a", Ty::of(Tag::Int), true)]);
    assert!(two_fields.is_subtype(&one_field_open));
    assert!(!two_fields.is_subtype(&one_field));
    // Closed IS a subtype of open with the same fields — it promises more.
    assert!(one_field.is_subtype(&one_field_open));
    assert!(!one_field_open.is_subtype(&one_field));
    // Not the reverse on width either — `one_field` doesn't declare `b` at all.
    assert!(!one_field.is_subtype(&two_fields));
    assert!(!one_field_open.is_subtype(&two_fields));

    // A required field in `other` must also be required in `self` — an
    // optional field isn't guaranteed present, so it can't satisfy a
    // required one.
    let a_optional = rec(&[("a", Ty::of(Tag::Int), false)]);
    let a_required = rec(&[("a", Ty::of(Tag::Int), true)]);
    assert!(!a_optional.is_subtype(&a_required));
    // The reverse holds: a required field trivially satisfies "optional".
    assert!(a_required.is_subtype(&a_optional));

    // Absence is now *reasoned about*, not declined (ADR-264): a CLOSED `{}` carries no
    // `:a`, and `{a?: int}` reads `:a` as `int | nil`, so the subtype relation holds —
    // where the old open-only rule had to refuse it as unprovable.
    let bare = rec(&[]);
    assert!(bare.is_subtype(&a_optional));
    // An OPEN `{}` still isn't: it may carry an `:a` of any type at all.
    assert!(!rec_open(&[]).is_subtype(&a_optional));
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
    // Distinct field maps are kept as two terms (ADR-262); the accessor reports no
    // single declared shape for the union, as it did when the union widened.
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
    // a value omitting `a` satisfies both.
    let ao = rec_open(&[("a", Ty::of(Tag::Int), false)]);
    let bo = rec_open(&[("a", Ty::of(Tag::Str), false)]);
    assert!(!ao.is_disjoint(&bo));
    // When only one side mentions the field, it depends on the OTHER side's kind
    // (ADR-264). Open: the extra field is permitted, so they overlap.
    let just_b_open = rec_open(&[("b", Ty::of(Tag::Str), true)]);
    assert!(!rec_open(&[("a", Ty::of(Tag::Int), true)]).is_disjoint(&just_b_open));
    // Closed: `{a: int}` says `:b` is absent and `{b: string}` requires it — no value
    // is both. This is the discrimination a tagged union is made of.
    let just_b = rec(&[("b", Ty::of(Tag::Str), true)]);
    assert!(a.is_disjoint(&just_b));
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
    // vector<int> ∪ vector<string> keeps both terms (ADR-262), and the *accessor*
    // reports no single element type for the union — which is what every consumer
    // saw when the union widened, so none of them changed. See
    // `a_union_of_two_tuple_shapes_keeps_both` for what the terms now buy.
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
    // Both values IS `bool` — the domain is finite, so the two spellings are one type
    // (canonicalised; see `two_spellings_of_the_same_set_are_one_type`).
    assert_eq!(bool_union(&[true, false]).to_string(), "bool");
}

#[test]
fn bool_literal_union_is_exact_but_open_bool_widens() {
    // A union of literals keeps both, exactly — unlike every structural refinement,
    // which widens on mismatch.
    let mut want = BTreeSet::new();
    want.insert(true);
    let u = bool_union(&[true, true]);
    assert_eq!(u.as_lit_bool(), Some(&want));

    // …and once the set covers bool's whole (finite) domain it IS `bool`: the refinement
    // is dropped, so the two spellings do not survive as different types.
    assert_eq!(bool_union(&[true, false]).as_lit_bool(), None);
    assert_eq!(bool_union(&[true, false]), Ty::of(Tag::Bool));

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

// ---- a union keeps its terms (ADR-262) ----
// One term carries one refinement per slot, so `(or (tuple int) (tuple string))` had
// no representation at all: both refinements were widened away and the type became
// bare `vector`. Sound, and it made the tagged-union idiom — the shape most Brood
// code returns — invisible to every check.

#[test]
fn a_union_of_two_tuple_shapes_keeps_both() {
    let a = Ty::tuple_of(vec![Ty::of(Tag::Int)]);
    let b = Ty::tuple_of(vec![Ty::of(Tag::Str)]);
    let u = a.clone().union(b.clone());
    assert_eq!(u.to_string(), "(tuple int) | (tuple string)");
    // Each alternative is still a subtype of the union…
    assert!(a.is_subtype(&u) && b.is_subtype(&u));
    // …and a shape neither admits is provably outside it.
    let c = Ty::tuple_of(vec![Ty::of(Tag::Bool)]);
    assert!(!c.is_subtype(&u));
    assert!(c.is_disjoint(&u));
}

#[test]
fn a_union_of_two_record_shapes_keeps_both() {
    let a = rec(&[("a", Ty::of(Tag::Int), true)]);
    let b = rec(&[("b", Ty::of(Tag::Str), true)]);
    let u = a.clone().union(b.clone());
    assert!(a.is_subtype(&u) && b.is_subtype(&u));
    // A record conflicting with *both* alternatives is disjoint from the union.
    let c = rec(&[("a", Ty::of(Tag::Str), true), ("b", Ty::of(Tag::Int), true)]);
    assert!(c.is_disjoint(&u));
    // …while one that satisfies an alternative is not.
    assert!(!a.is_disjoint(&u));
}

#[test]
fn a_union_still_merges_when_one_term_can_hold_it() {
    // Nothing that already had an exact single-term union pays for the new machinery:
    // agreeing refinements merge, and a side that contributes no refined member
    // carries the other's refinement through.
    let vi = Ty::vector_of(Ty::of(Tag::Int));
    assert_eq!(vi.clone().union(vi.clone()).elem_ty(), vi.elem_ty());
    let mixed = Ty::of(Tag::Int).union(vi.clone());
    assert_eq!(mixed.elem_ty(), vi.elem_ty());
    // Literal sets were always exact, and stay one term.
    let kw = kw_union(&["a", "b"]);
    assert_eq!(kw.to_string(), ":a | :b");
    assert!(kw.as_lit().is_some());
}

#[test]
fn a_union_absorbs_a_term_another_already_covers() {
    let vi = Ty::vector_of(Ty::of(Tag::Int));
    let v = Ty::of(Tag::Vector); // any vector
                                 // vector<int> ⊆ vector, so the union is just `vector` — one term, not two.
    assert_eq!(vi.clone().union(v.clone()).to_string(), "vector");
    assert_eq!(v.union(vi).to_string(), "vector");
}

#[test]
fn a_union_of_many_shapes_collapses_at_the_cap() {
    // Beyond `MAX_TY_TERMS` the terms merge by the old widening union, so a runaway
    // fixpoint can't grow a type without limit (the KI-13 property).
    let mut u = Ty::NEVER;
    for i in 0..12 {
        u = u.union(Ty::tuple_of(vec![Ty::int_lit(i)]));
    }
    assert!(
        u.alt_terms().is_none_or(|t| t.len() <= 4),
        "terms must stay capped: {u}"
    );
    // Still a supertype of what went in — collapsing only ever widens.
    assert!(Ty::tuple_of(vec![Ty::int_lit(0)]).is_subtype(&u));
}

#[test]
fn a_refinement_accessor_reports_nothing_for_a_union() {
    // A refinement that holds for one term does not hold for the union, so every
    // accessor reports `None` there — exactly what a widened type reported before,
    // which is why no consumer had to change.
    let u = Ty::tuple_of(vec![Ty::of(Tag::Int)]).union(Ty::tuple_of(vec![Ty::of(Tag::Str)]));
    assert_eq!(u.tuple_elems(), None);
    assert_eq!(u.elem_ty(), None);
    let r = rec(&[("a", Ty::of(Tag::Int), true)]).union(rec(&[("b", Ty::of(Tag::Str), true)]));
    assert_eq!(r.record_fields(), None);
}

#[test]
fn union_equality_ignores_term_order() {
    // `A ∪ B` and `B ∪ A` are the same set — and every memo keyed on a `Ty` depends
    // on them comparing (and hashing) equal.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let a = Ty::tuple_of(vec![Ty::of(Tag::Int)]);
    let b = Ty::tuple_of(vec![Ty::of(Tag::Str)]);
    let ab = a.clone().union(b.clone());
    let ba = b.union(a);
    assert_eq!(ab, ba);
    let hash = |t: &Ty| {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    };
    assert_eq!(hash(&ab), hash(&ba));
}

#[test]
fn negating_a_union_is_the_intersection_of_the_negations() {
    // De Morgan, and the flat case stays exact.
    let u = Ty::of(Tag::Int).union(Ty::of(Tag::Str));
    let n = u.clone().negate();
    assert!(n.is_disjoint(&u));
    assert!(!n.contains_tag(Tag::Int) && !n.contains_tag(Tag::Str));
    assert!(n.contains_tag(Tag::Nil));
}

#[test]
fn term_equality_distinguishes_every_refinement_slot() {
    // The compile-time half is the destructuring in `term_eq`/`hash_term` (a new slot
    // fails to compile until it is listed); this is the behavioural half — each slot
    // must actually *participate*, so a listed-but-unused field can't slip through
    // either. Two types differing only in slot N must compare unequal.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let hash = |t: &Ty| {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    };
    let variants: Vec<(&str, Ty)> = vec![
        ("tags", Ty::of(Tag::Int)),
        (
            "arrow",
            Ty::arrow(Sig::new(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int))),
        ),
        (
            "overload",
            Ty::overload_of(vec![
                Sig::new(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int)),
                Sig::new(vec![Ty::of(Tag::Bool)], Ty::of(Tag::Bool)),
            ]),
        ),
        ("elem", Ty::vector_of(Ty::of(Tag::Int))),
        ("map_kv", Ty::map_of(Ty::of(Tag::Keyword), Ty::of(Tag::Int))),
        ("fields", rec(&[("a", Ty::of(Tag::Int), true)])),
        ("tuple", Ty::tuple_of(vec![Ty::of(Tag::Int)])),
        ("lit", Ty::keyword_lit(value::intern("a"))),
        ("lit_int", Ty::int_lit(5)),
        ("lit_bool", Ty::bool_lit(true)),
        ("lit_str", Ty::str_lit("s")),
    ];
    for (i, (name_a, a)) in variants.iter().enumerate() {
        for (name_b, b) in variants.iter().skip(i + 1) {
            assert_ne!(a, b, "`{name_a}` and `{name_b}` must not compare equal");
            assert_ne!(
                hash(a),
                hash(b),
                "`{name_a}` and `{name_b}` must not hash the same"
            );
        }
    }
}

// ---- algebraic properties, over a cross-product of representative types ----
//
// The tests above check one relation at a time against a hand-picked expectation.
// These check the relations *against each other*, over every pair (and triple) of a
// deliberately awkward corpus — flat unions, refined terms of every kind, multi-term
// unions, closed and open shapes. That is the shape of test that catches the bugs a
// per-case test cannot: `is_disjoint` and `intersect` answer the same question through
// completely separate code, and a review of this change found a defect (an open shape
// hiding a `map<K,V>` refinement) that exactly this cross-check would have flagged.
//
// A fixed corpus rather than a generator: deterministic, no dependency, and every
// failure names two concrete types you can paste into a REPL.
fn property_corpus() -> Vec<Ty> {
    vec![
        Ty::NEVER,
        Ty::ANY,
        Ty::of(Tag::Int),
        Ty::of(Tag::Str),
        Ty::of(Tag::Nil),
        Ty::NUMBER,
        Ty::LIST,
        Ty::of(Tag::Int).union(Ty::of(Tag::Str)),
        Ty::of(Tag::Nil).negate(),
        Ty::int_lit(5),
        Ty::int_lit(5).union(Ty::int_lit(6)),
        Ty::keyword_lit(value::intern("ok")),
        Ty::bool_lit(true),
        Ty::str_lit("x"),
        Ty::vector_of(Ty::of(Tag::Int)),
        Ty::vector_of(Ty::of(Tag::Str)),
        Ty::list_of(Ty::NUMBER),
        Ty::tuple_of(vec![Ty::of(Tag::Int)]),
        Ty::tuple_of(vec![Ty::of(Tag::Str)]),
        Ty::tuple_of(vec![Ty::of(Tag::Int), Ty::of(Tag::Str)]),
        Ty::map_of(Ty::of(Tag::Keyword), Ty::of(Tag::Int)),
        rec(&[("a", Ty::of(Tag::Int), true)]),
        rec(&[("a", Ty::of(Tag::Str), true)]),
        rec(&[("b", Ty::of(Tag::Int), true)]),
        rec(&[("a", Ty::of(Tag::Int), false)]),
        rec(&[]),
        rec_open(&[("a", Ty::of(Tag::Int), true)]),
        rec_open(&[]),
        // multi-term unions — the representation ADR-262 added
        Ty::tuple_of(vec![Ty::of(Tag::Int)]).union(Ty::tuple_of(vec![Ty::of(Tag::Str)])),
        rec(&[("a", Ty::of(Tag::Int), true)]).union(rec(&[("b", Ty::of(Tag::Str), true)])),
        Ty::vector_of(Ty::of(Tag::Int)).union(Ty::of(Tag::Nil)),
        Ty::vector_of(Ty::of(Tag::Int))
            .union(Ty::vector_of(Ty::of(Tag::Str)))
            .union(Ty::of(Tag::Int)),
        // negative literal atoms — the complement half of the lattice
        Ty::keyword_lit(value::intern("ok")).negate(),
        Ty::int_lit(5).negate(),
        Ty::keyword_lit(value::intern("ok"))
            .negate()
            .intersect(Ty::of(Tag::Keyword)),
        arr(vec![Ty::of(Tag::Int)], Ty::of(Tag::Int)),
        // the callable type every callback parameter infers (ADR-272)
        Ty::of(Tag::Fn)
            .union(Ty::of(Tag::Native))
            .union(Ty::of(Tag::Keyword)),
    ]
}

#[test]
fn a_term_may_be_covered_by_several_alternatives_at_once() {
    // The completeness direction cross-term subtyping used to miss. `int | vector<int>`
    // merges into ONE term (the union is exact), and it fits inside the three-way union
    // only because its `int` half and its `vector` half land in DIFFERENT alternatives.
    // Requiring a single alternative to cover the whole term rejected it — a false
    // positive on any call passing such a value.
    let ints = Ty::of(Tag::Int);
    let vec_int = Ty::vector_of(ints.clone());
    let vec_str = Ty::vector_of(Ty::of(Tag::Str));

    let left = ints.clone().union(vec_int.clone());
    let right = vec_str.clone().union(vec_int.clone()).union(ints.clone());
    assert!(
        left.is_subtype(&right),
        "`{left}` is contained in `{right}` — its int half and its vector half are in          different alternatives"
    );

    // …and the sound direction still holds: a half that nothing covers is rejected.
    let unrelated = vec_str.union(Ty::of(Tag::Str));
    assert!(
        !left.is_subtype(&unrelated),
        "`{left}` is NOT contained in `{unrelated}` — nothing there admits an int"
    );

    // The projection is per tag, not per term, so a refinement that only part of the
    // union satisfies cannot leak: vector<int> is not covered by vector<string> alone.
    assert!(!vec_int.is_subtype(&Ty::vector_of(Ty::of(Tag::Str))));
}

#[test]
fn disjointness_agrees_with_intersection() {
    // The strongest cross-check available: two independent implementations of "do
    // these share a value". `is_disjoint` walks tags, literal sets and field readings;
    // `intersect` builds the actual intersection. They must not disagree.
    //
    // Only the *sound* direction is asserted as a law — claiming disjoint when the
    // intersection is inhabited would be a false positive, the one unacceptable class.
    // The converse (an empty intersection that `is_disjoint` won't confirm) is mere
    // incompleteness, so it is reported rather than failed, and the corpus is kept at
    // zero of them so a regression is visible.
    let corpus = property_corpus();
    let mut incomplete = Vec::new();
    for a in &corpus {
        for b in &corpus {
            let inter = a.clone().intersect(b.clone());
            if a.is_disjoint(b) {
                assert!(
                    inter.is_never(),
                    "UNSOUND: `{a}` and `{b}` reported disjoint, but their intersection \
                     is `{inter}`"
                );
            } else if inter.is_never() {
                incomplete.push(format!("{a} ∩ {b}"));
            }
        }
    }
    assert!(
        incomplete.is_empty(),
        "intersection is empty but `is_disjoint` says otherwise (incomplete, not \
         unsound) — new entries here mean the two answers drifted: {incomplete:#?}"
    );
}

#[test]
fn subtyping_agrees_with_the_other_relations() {
    let corpus = property_corpus();
    for a in &corpus {
        // Reflexive.
        assert!(a.is_subtype(a), "`{a}` is not a subtype of itself");
        for b in &corpus {
            let union = a.clone().union(b.clone());
            let inter = a.clone().intersect(b.clone());
            // A union is an upper bound; an intersection is a lower bound.
            assert!(a.is_subtype(&union), "`{a}` ⊄ `{a}` ∪ `{b}` = `{union}`");
            assert!(inter.is_subtype(a), "`{a}` ∩ `{b}` = `{inter}` ⊄ `{a}`");
            // Sharing a subtype means not disjoint — a non-empty `a` inside `b`
            // is a value they both contain.
            if a.is_subtype(b) && !a.is_never() {
                assert!(
                    !a.is_disjoint(b),
                    "`{a}` ⊆ `{b}` yet they are reported disjoint"
                );
            }
        }
    }
}

#[test]
fn union_and_intersection_are_commutative_as_sets() {
    // `A ∪ B` and `B ∪ A` are one set, and every memo keyed on a `Ty` depends on them
    // comparing *and hashing* equal — which the term representation makes a real
    // question rather than a syntactic given.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let hash = |t: &Ty| {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    };
    let corpus = property_corpus();
    for a in &corpus {
        for b in &corpus {
            let (ab, ba) = (a.clone().union(b.clone()), b.clone().union(a.clone()));
            assert_eq!(ab, ba, "`{a}` ∪ `{b}` ≠ `{b}` ∪ `{a}`");
            assert_eq!(hash(&ab), hash(&ba), "`{ab}` and `{ba}` hash differently");
            let (ab, ba) = (
                a.clone().intersect(b.clone()),
                b.clone().intersect(a.clone()),
            );
            assert_eq!(ab, ba, "`{a}` ∩ `{b}` ≠ `{b}` ∩ `{a}`");
        }
    }
}

#[test]
fn a_complement_shares_nothing_with_what_it_negates() {
    // Exact for flat types; for a refined one the complement widens (it keeps the
    // refined tag), so the law is asserted where it must hold and the widening is
    // pinned separately by `negate_of_a_refined_type_is_a_sound_overapproximation`.
    for a in property_corpus().iter().filter(|t| t.is_flat()) {
        let n = a.clone().negate();
        assert!(
            a.clone().intersect(n.clone()).is_never(),
            "`{a}` ∩ ¬`{a}` = `{}` is not empty",
            a.clone().intersect(n)
        );
    }
}

#[test]
fn a_record_shape_answers_for_every_key() {
    // The reading that makes closedness composable: declared-required is the type,
    // declared-optional adds `nil` (it may be absent), and undeclared is the shape's
    // remainder — `nil` closed, `any` open.
    let closed = rec(&[
        ("a", Ty::of(Tag::Int), true),
        ("b", Ty::of(Tag::Str), false),
    ]);
    let (a, b, z) = (value::intern("a"), value::intern("b"), value::intern("zzz"));
    assert_eq!(closed.record_field_ty(a), Some(Ty::of(Tag::Int)));
    assert_eq!(
        closed.record_field_ty(b),
        Some(Ty::of(Tag::Str).union(Ty::of(Tag::Nil)))
    );
    assert_eq!(closed.record_field_ty(z), Some(Ty::of(Tag::Nil)));
    assert_eq!(
        rec_open(&[("a", Ty::of(Tag::Int), true)]).record_field_ty(z),
        Some(Ty::ANY)
    );
}

#[test]
fn a_field_read_over_a_union_of_shapes_is_the_union_of_the_readings() {
    // `{ok: int} | {error: string}` — `:ok` is `int` in one term and `nil` in the
    // other, so the union answers `int | nil`. This is what a closed shape buys.
    let ok = rec(&[("ok", Ty::of(Tag::Int), true)]);
    let err = rec(&[("error", Ty::of(Tag::Str), true)]);
    assert_eq!(
        ok.clone()
            .union(err.clone())
            .record_field_ty(value::intern("ok")),
        Some(Ty::of(Tag::Int).union(Ty::of(Tag::Nil)))
    );
    // The two alternatives are provably disjoint — each says the other's key is absent.
    assert!(ok.is_disjoint(&err));
}

#[test]
fn intersecting_record_shapes_is_exact() {
    // A guard narrows by intersection, so shapes must intersect precisely rather than
    // widening away the fact the guard established.
    let both = rec_open(&[("x", Ty::NUMBER, true)]).intersect(rec_open(&[
        ("x", Ty::of(Tag::Int), true),
        ("y", Ty::of(Tag::Str), true),
    ]));
    assert_eq!(
        both.record_field_ty(value::intern("x")),
        Some(Ty::of(Tag::Int))
    );
    // Contradictory shapes intersect to nothing at all — not to "some map".
    assert!(rec(&[("x", Ty::of(Tag::Int), true)])
        .intersect(rec(&[("y", Ty::of(Tag::Str), true)]))
        .is_never());
}

#[test]
fn an_open_shape_renders_its_openness() {
    assert_eq!(
        rec(&[("a", Ty::of(Tag::Int), true)]).to_string(),
        "{a: int}"
    );
    assert_eq!(
        rec_open(&[("a", Ty::of(Tag::Int), true)]).to_string(),
        "{a: int, ...}"
    );
}

#[test]
fn a_union_is_never_flat() {
    // `is_flat` asks whether a type is exactly its tag set. A union's *head* term can
    // be refinement-free while the alternatives do the describing, so answering from
    // the head alone would answer about one term while reading as if it answered about
    // the type.
    assert!(!Ty::of(Tag::Int)
        .union(Ty::tuple_of(vec![Ty::of(Tag::Str)]))
        .is_flat());
    assert!(Ty::of(Tag::Int).union(Ty::of(Tag::Str)).is_flat());
}

#[test]
fn an_open_shape_does_not_hide_a_map_kv_refinement() {
    // An intersection can carry both refinements. For a key the shape doesn't declare,
    // an OPEN shape says only `any`, while `map<K,V>` says `V | nil` — the sharper of
    // the two must win, or intersecting with a shape would lose precision.
    let both = Ty::map_of(Ty::of(Tag::Keyword), Ty::of(Tag::Int)).intersect(rec_open(&[(
        "a",
        Ty::of(Tag::Str),
        true,
    )]));
    assert_eq!(
        both.record_field_ty(value::intern("zzz")),
        Some(Ty::of(Tag::Int).union(Ty::of(Tag::Nil)))
    );
    assert_eq!(
        both.record_field_ty(value::intern("a")),
        Some(Ty::of(Tag::Str))
    );
    // A CLOSED shape's undeclared key is `nil` — it is absent, whatever a uniform
    // value type would say about the keys that ARE present.
    assert_eq!(
        Ty::map_of(Ty::of(Tag::Keyword), Ty::of(Tag::Int))
            .intersect(rec(&[("a", Ty::of(Tag::Str), true)]))
            .record_field_ty(value::intern("zzz")),
        Some(Ty::of(Tag::Nil))
    );
}

#[test]
fn an_intersection_of_element_and_tuple_types_is_a_lower_bound() {
    // Both were widening merges before the property tests went in: `vector<int> ∩
    // vector<string>` came out as plain `vector` — not even a subtype of either side —
    // and `(tuple int) ∩ (tuple string)` as `vector`, while `is_disjoint` (correctly)
    // called the same pair disjoint. Two answers to one question, contradicting.
    let vi = Ty::vector_of(Ty::of(Tag::Int));
    let vs = Ty::vector_of(Ty::of(Tag::Str));
    let both = vi.clone().intersect(vs);
    assert!(both.is_subtype(&vi), "`{both}` is not a lower bound");
    assert_eq!(both.elem_ty(), Some(Ty::NEVER)); // only the empty vector
                                                 // A tuple's arity IS its shape, so no vector satisfies both.
    assert!(Ty::tuple_of(vec![Ty::of(Tag::Int)])
        .intersect(Ty::tuple_of(vec![Ty::of(Tag::Str)]))
        .is_never());
    assert!(Ty::tuple_of(vec![Ty::of(Tag::Int)])
        .intersect(Ty::tuple_of(vec![Ty::of(Tag::Int), Ty::of(Tag::Int)]))
        .is_never());
}

#[test]
fn to_source_round_trips_through_the_annotation_parser() {
    // The quick-fix that writes an inferred signature into a `(sig …)` must write the
    // type it showed. Anything that renders must parse back to an equal type — and
    // anything that cannot be written faithfully must decline (`None`) rather than
    // approximate, which is the whole contract.
    let mut interp = crate::Interp::new();
    for ty in property_corpus() {
        let Some(src) = ty.to_source() else { continue };
        let form = crate::syntax::reader::read_one(&mut interp.heap, &src)
            .unwrap_or_else(|e| panic!("`{src}` (from `{ty}`) does not parse: {e:?}"));
        let back = super::check::annot::parse_type(&interp.heap, form)
            .unwrap_or_else(|| panic!("`{src}` (from `{ty}`) is not a type expression"));
        assert_eq!(
            back, ty,
            "`{ty}` rendered as `{src}`, which reads back as `{back}`"
        );
    }
}

#[test]
fn to_source_declines_what_it_cannot_write() {
    // `macro` has no spelling in the grammar, and neither does a LONE `native` — the
    // `fn` alias covers the two function members TOGETHER, which is the only form the
    // grammar can express and the only form the language can produce (`type-of` reports
    // `:fn` for both). Half of it would be a widening if written as `fn`, so it declines.
    assert_eq!(Ty::of(Tag::Macro).to_source(), None);
    assert_eq!(Ty::of(Tag::Native).to_source(), None);
    // …and a type carrying one declines as a whole rather than dropping it.
    assert_eq!(Ty::of(Tag::Int).union(Ty::of(Tag::Macro)).to_source(), None);
}

#[test]
fn a_bool_literal_complement_is_exact() {
    // Bool is the one literal kind with a finite domain, so its complement is a set
    // this lattice can hold: `¬{false}` is `{true}`, not "any bool". That is what makes
    // *truthy* — `¬(nil ∪ false)` — sayable, and with it the `(if x …)` guard
    // biconditional rather than one-sided.
    assert_eq!(
        Ty::bool_lit(false).negate().as_lit_bool().map(|s| s.len()),
        Some(1)
    );
    assert!(Ty::bool_lit(false)
        .negate()
        .is_disjoint(&Ty::bool_lit(false)));
    assert!(!Ty::bool_lit(false)
        .negate()
        .is_disjoint(&Ty::bool_lit(true)));
    // The truthy type itself: everything but `nil` and `false`.
    let truthy = Ty::of(Tag::Nil).union(Ty::bool_lit(false)).negate();
    assert!(truthy.is_disjoint(&Ty::of(Tag::Nil)));
    assert!(truthy.is_disjoint(&Ty::bool_lit(false)));
    assert!(!truthy.is_disjoint(&Ty::bool_lit(true)));
    assert!(!truthy.is_disjoint(&Ty::of(Tag::Int)));
    // …and negating it back gives falsy, which is why the guard can narrow both ways.
    let falsy = truthy.clone().negate();
    assert!(falsy.is_disjoint(&Ty::of(Tag::Int)));
    assert!(!falsy.is_disjoint(&Ty::of(Tag::Nil)));
    assert!(!falsy.is_disjoint(&Ty::bool_lit(false)));
    // Negating BOTH bool literals removes the tag: no bool is neither true nor false.
    let both = Ty::bool_lit(true).union(Ty::bool_lit(false));
    assert!(!both.negate().contains_tag(Tag::Bool));
}

#[test]
fn a_literal_complement_is_exact_for_every_literal_kind() {
    // The negative half of the lattice. `¬:ok` is "anything but the keyword :ok", held
    // as an exclusion because the keyword domain is infinite — not the `any` it widened
    // to before. That exactness is what lets an equality test narrow its else branch.
    let ok = Ty::keyword_lit(value::intern("ok"));
    let err = Ty::keyword_lit(value::intern("err"));
    assert_eq!(
        ok.clone().union(err.clone()).intersect(ok.clone().negate()),
        err,
        "(:ok | :err) minus :ok is :err"
    );
    assert_eq!(
        Ty::int_lit(5)
            .union(Ty::int_lit(6))
            .intersect(Ty::int_lit(5).negate()),
        Ty::int_lit(6)
    );
    assert_eq!(
        Ty::str_lit("a")
            .union(Ty::str_lit("b"))
            .intersect(Ty::str_lit("a").negate()),
        Ty::str_lit("b")
    );

    // A complement keeps its own tag — `¬:ok` still admits every OTHER keyword, which
    // is the property that makes narrowing sound rather than merely narrow.
    let other = Ty::keyword_lit(value::intern("other"));
    assert!(other.is_subtype(&ok.clone().negate()));
    assert!(!ok.is_subtype(&ok.clone().negate()));
    assert!(Ty::of(Tag::Str).is_subtype(&ok.clone().negate()));

    // Double negation returns the literal exactly.
    assert_eq!(
        ok.clone().negate().negate().intersect(Ty::of(Tag::Keyword)),
        ok
    );

    // Bool stays positive: its domain is finite, so `¬false` is `{true}` rather than an
    // exclusion (`LitSet::Out` may assume an infinite complement).
    assert_eq!(
        Ty::bool_lit(false).negate().intersect(Ty::of(Tag::Bool)),
        Ty::bool_lit(true)
    );
}

#[test]
fn two_spellings_of_the_same_set_are_one_type() {
    // `Ty` derives equality and hashing from its slots, so two representations of the
    // same set are a real defect, not a cosmetic one: `bool <: (or false true)` came out
    // FALSE for two identical sets — a spurious warning waiting to happen — and a
    // fixpoint that iterates until a type stops changing would never settle between them.
    //
    // Bool is the only tag this can happen to: its domain is finite, so a positive set
    // can cover it. The general case is an exclusion that excludes nothing.
    let both = Ty::bool_lit(true).union(Ty::bool_lit(false));
    assert_eq!(both, Ty::of(Tag::Bool), "`{both}` should BE `bool`");
    assert!(Ty::of(Tag::Bool).is_subtype(&both));
    assert!(both.is_subtype(&Ty::of(Tag::Bool)));
    assert_eq!(both.to_source().as_deref(), Some("bool"));

    // The same set reached by complement rather than union.
    let neither = Ty::bool_lit(true)
        .negate()
        .union(Ty::bool_lit(false).negate())
        .intersect(Ty::of(Tag::Bool));
    assert_eq!(neither, Ty::of(Tag::Bool));

    // …and an exclusion that excludes nothing is "every value of the tag".
    let all_keywords = Ty::keyword_lit(value::intern("a"))
        .negate()
        .union(Ty::keyword_lit(value::intern("a")))
        .intersect(Ty::of(Tag::Keyword));
    assert_eq!(all_keywords, Ty::of(Tag::Keyword));
}

#[test]
fn the_two_function_members_render_as_the_one_word_the_language_has() {
    // `Fn`/`Native` is an implementation detail the LANGUAGE does not have: `(type-of
    // inc)` is `:fn`, `(fn? inc)` is true for a builtin and a closure alike, and the
    // grammar's `fn` already parses to both members. Only the renderers spelled them
    // apart, so a warning read `expects keyword | fn | native` — naming a kind no Brood
    // program can observe or write down — and `to_source` DECLINED on `Tag::Native`, so
    // the callable type inferred for every callback parameter (ADR-272) had no faithful
    // annotation and the declare-sig surfaces could not offer it.
    let callable = Ty::of(Tag::Fn).union(Ty::of(Tag::Native));
    assert_eq!(callable.to_string(), "fn");
    assert_eq!(callable.to_source().as_deref(), Some("fn"));

    // …including inside a wider union, which is the shape a callback parameter has
    // (a keyword is a function of a map, ADR-272).
    let with_keyword = callable.clone().union(Ty::of(Tag::Keyword));
    assert_eq!(with_keyword.to_string(), "keyword | fn");
    let source = with_keyword
        .to_source()
        .expect("a callable param must be writable");
    assert_eq!(source, "(or fn keyword)");

    // …and it round-trips: what the tool offers is what the checker meant.
    let mut interp = crate::Interp::new();
    let form = crate::syntax::reader::read_one(&mut interp.heap, &source).expect("parses");
    let back = super::check::annot::parse_type(&interp.heap, form).expect("is a type");
    assert_eq!(
        back, with_keyword,
        "`{source}` must read back as what it rendered"
    );
}
