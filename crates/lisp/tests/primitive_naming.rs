//! The invariant that stops the map/vector/table naming rabbit hole from recurring.
//!
//! A `/` in a name is *module-member* syntax across the whole toolchain: `(:use mod)`
//! refers a module's names by prefix, the project loader `require`s a module per image
//! section (sections are keyed by splitting names on `/`), and qualified-name rooting
//! (ADR-070) treats `mod/name` as a reference into module `mod`. A kernel PRIMITIVE is a
//! *flat* global, not a member of any module — so a `/`-named primitive whose prefix is not
//! a real module breaks all three (`map/get` was swept into `(:use map)` and shadowed the
//! prelude ops; `vector/ref` made `nest test` die on `require: cannot find module 'vector'`
//! for every project that touched a vector). `string/length` is fine ONLY because `string`
//! is a real module. So: kernel primitives use flat dash names; a slash namespace is
//! reserved for a real module-backed one. This test fails the moment a primitive violates
//! it — at CI, not three deploys later.

use brood::core::value::{self, EnvId, ValueRef};
use brood::Interp;

#[test]
fn primitives_do_not_borrow_a_non_module_slash_namespace() {
    // The module-backed primitive namespaces: each is a real `std/<name>.blsp` `defmodule`,
    // so `(:use <name>)` and `require`-ing `<name>` both succeed and the primitives ride its
    // namespace safely. Extend this ONLY when a new primitive family's prefix is likewise a
    // genuine module — otherwise give the primitive a flat dash name.
    // Every prefix here must be a REAL module — one with a `std/<name>.blsp` (or
    // `std/<name>/`) that owns the namespace — so a primitive sitting under it is a
    // primitive of that module, not a kernel name squatting on a slash. `bit`, `decimal`,
    // `proc` and `system` joined when the 510->298 bare-name refactors gave each family
    // its own module (ADR-251); `math` joined with `floor`/`numerator`/`denominator`, which
    // are kernel primitives sitting under the real `std/math.blsp` module; `reflect` joined
    // when the introspection family (`eval`, `load`, `global-names`, `private?`, …) left the
    // bare root for `std/reflect.blsp`, which owns that namespace the same way.
    let allowed: &[&str] = &[
        "string", "file", "bit", "decimal", "proc", "system", "math", "reflect",
    ];

    let interp = Interp::new();
    let mut violations: Vec<String> = Vec::new();
    for sym in interp.heap.global_symbols() {
        let name = value::symbol_name(sym);
        let slash = match name.rfind('/') {
            Some(i) => i,
            None => continue,
        };
        // Only kernel primitives (a `Native` value) — a Brood module function like
        // `stream/map` legitimately lives under its module's slash namespace.
        let is_native = interp
            .heap
            .env_get(EnvId::GLOBAL, sym)
            .map(|v| matches!(v.unpack(), ValueRef::Native(_)))
            .unwrap_or(false);
        if !is_native {
            continue;
        }
        if !allowed.contains(&&name[..slash]) {
            violations.push(name);
        }
    }
    violations.sort();
    assert!(
        violations.is_empty(),
        "kernel primitives borrow a non-module slash namespace — give them flat dash names, \
         or make the prefix a real module and add it to `allowed`: {violations:?}"
    );
}
