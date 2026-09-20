//! A type-variable declaration the stdlib image carries resolves at a call site from the
//! arguments, with its module never loaded (ADR-370). Its own process because the footer
//! index is process-global and additive: this test PLANTS an entry, which would break the
//! in-crate `the_footer_is_the_source_scan_byte_for_byte` gate beside it.
//!
//! Planted rather than read from std because std's only variable-bearing declarations,
//! `math/max` and `math/min`, have a by-name rule (`is_extremum`) that answers from the
//! operands whatever the footer says — a verdict on either passed with the call-site
//! fallback removed (sabotage, 2026-09-20). `image-sig-probe/pick` has no rule, no binding
//! and no module: the only thing that can type the call is the footer's `(?A ?A -> ?A)`.

use brood::core::value::Tag;
use brood::eval::derive::register_image_sigs;
use brood::types::check::expr_ty_of;
use brood::types::Ty;
use brood::Interp;

#[test]
fn a_planted_type_variable_declaration_resolves_from_the_arguments() {
    if std::env::var_os("BROOD_NO_IMAGE_SIGS").is_some() {
        eprintln!("BROOD_NO_IMAGE_SIGS set — the footer is off by request, nothing to gate");
        return;
    }
    let mut interp = Interp::new();
    register_image_sigs(vec![(
        "image-sig-probe/pick".to_string(),
        "(?A ?A -> ?A)".to_string(),
        2,
        2,
    )]);
    let form = brood::syntax::reader::read_one(&mut interp.heap, "(image-sig-probe/pick 1 2)")
        .expect("parse");
    let ty = expr_ty_of(&interp.heap, form).expect("the call has a type");
    // `?A` bound to two int literals is a subtype of `int`; a reading that missed the footer
    // has nothing else to say about an unbound name and reads `any` (or nothing).
    assert!(
        ty.is_subtype(&Ty::of(Tag::Int)),
        "(image-sig-probe/pick 1 2) reads `{ty}` — the footer's declaration was not resolved"
    );
}
