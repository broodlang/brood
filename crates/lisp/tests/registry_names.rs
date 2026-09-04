//! `%registry-names` — "which globals does LOADING mutate rather than create" — must include
//! the registries the PRELUDE wrote. `defmulti num/add` in the prelude writes `*multi-algebra*`
//! and `*multi-ret*` in the BUILD heap (`*methods*`/`*method-from*` are `defmethod`'s, written
//! later by std modules), whose name set the freeze used to drop, so a fresh runtime listed
//! neither. A startup-image install that
//! snapshots "the registries" by this list to merge them back after loading its root section
//! could therefore not protect `num/add`'s declaration, and an imaged `nest test` failed to
//! load every file with a `defmethod num/add` (KI-89, ADR-317).
//!
//! On a FRESH `Interp`, deliberately: under `nest test` the boot itself writes these
//! registries, so an in-language version of this test passed against the very bug it was
//! written for. Sabotage: drop the `self.prelude.registry_names` union in
//! `Heap::registry_names` and this fails.

use brood::Interp;

#[test]
fn a_fresh_runtime_lists_the_registries_the_prelude_wrote() {
    let interp = Interp::new();
    let names: Vec<String> = interp
        .heap
        .registry_names()
        .into_iter()
        .map(brood::core::value::symbol_name)
        .collect();
    for want in ["*multi-algebra*", "*multi-ret*"] {
        assert!(
            names.iter().any(|n| n == want),
            "{want} is written by the prelude's `defmulti num/add` and must be listed; got {names:?}"
        );
    }
}
