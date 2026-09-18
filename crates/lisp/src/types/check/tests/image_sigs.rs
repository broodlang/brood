//! The stdlib image's signature footer (ADR-370), by construction: every type it carries is
//! the declaration the loaded module gives, read the way the checker reads it, and it
//! carries a type for every declaration the checker would read as-is — so a check that
//! never loads the module knows exactly what a check that did would know. The verdict-level
//! gate is `nest::image_sigs_differential`; this is the construction-level one, the shape
//! ADR-280 gave the image's bindings.
//!
//! Uses whatever image the cache holds for this binary's std, building one when there is
//! none (the source boot's `stdimage/build`, in a process holding only the prelude): either
//! way an image built from THIS std is installed, and both directions are compared against
//! the modules loaded into this same heap.

use crate::core::value::{self, Value};
use crate::eval::derive;
use crate::types::check::{self, sigs, std_index};

fn interp_with_image() -> crate::Interp {
    let mut interp = crate::Interp::new();
    let installed = interp
        .eval_str("(or (%std-image-installed) (%std-image-install) (do (stdimage/build) (%std-image-install)))")
        .map(|v| interp.print(v))
        .expect("install the stdlib image");
    assert_ne!(
        installed, "nil",
        "no stdlib image could be installed even after building one — with no footer to read \
         this gate would pass vacuously"
    );
    interp
}

fn module_of(sym: value::Symbol) -> String {
    let name = value::symbol_name(sym);
    let slash = name.rfind('/').expect("a footer name is qualified");
    name[..slash].to_string()
}

#[test]
fn every_type_the_image_carries_is_the_loaded_declaration_as_the_checker_reads_it() {
    let mut interp = interp_with_image();
    let names = derive::image_sig_typed_names(&interp.heap);
    assert!(
        !names.is_empty(),
        "the footer carries no typed name at all — the writer wrote none, or the index was not read"
    );
    let mut modules: Vec<String> = names.iter().map(|&s| module_of(s)).collect();
    modules.sort();
    modules.dedup();
    for module in &modules {
        interp
            .eval_str(&format!("(require-one '{module})"))
            .unwrap_or_else(|e| panic!("load {module}: {e:?}"));
    }
    let heap = &interp.heap;
    for &sym in &names {
        let name = value::symbol_name(sym);
        let carried = sigs::image_heap_sig(heap, sym)
            .unwrap_or_else(|| panic!("{name}: the carried text does not parse to an arrow"));
        let declared = sigs::declared_heap_sig(heap, sym).unwrap_or_else(|| {
            panic!("{name}: carried a type, but the loaded module declares none")
        });
        assert_eq!(
            carried, declared,
            "{name}: the carried type is not the loaded declaration"
        );
        assert!(
            !carried.ret.is_any() && !carried.ret.is_unrefined_collection(),
            "{name}: a return of `{}` is re-typed from the body at a call site and may not ride",
            carried.ret
        );
        assert!(
            !heap.is_private(sym),
            "{name}: a private name's cross-module call is a warning only the loaded module can raise"
        );
        // What a call site reads, with the module loaded, is what it read from the footer alone.
        assert_eq!(
            sigs::sig_of(heap, sym),
            Some(carried),
            "{name}: sig_of disagrees with the footer"
        );
        assert!(
            matches!(
                heap.env_get(heap.global(), sym),
                Some(Value::Fn(_) | Value::Native(_))
            ),
            "{name}: carried a type for something that is not a function"
        );
    }
}

#[test]
fn every_authoritative_public_declaration_of_an_imaged_module_rides_in_the_footer() {
    let mut interp = interp_with_image();
    interp
        .eval_str("(doseq (m (reflect/builtin-modules)) (try (require-one m) (catch _ nil)))")
        .expect("load every baked-in module");
    let heap = &interp.heap;
    let mut expected = 0usize;
    let mut missing: Vec<String> = Vec::new();
    for (sym, entry) in heap.declared_sigs_everywhere() {
        // Only an imaged function has a footer entry at all; a project's own or a
        // dependency's declaration is out of scope here, and so is a value-position `sig`.
        if derive::image_sig_arity(heap, sym).is_none() || heap.is_private(sym) {
            continue;
        }
        if !matches!(
            heap.env_get(heap.global(), sym),
            Some(Value::Fn(_) | Value::Native(_))
        ) {
            continue;
        }
        let Some(form) = crate::builtins::modules::sig_type_of(heap, Some(entry)) else {
            continue;
        };
        if std_index::image_carried_sig(heap, form).is_none() {
            continue;
        }
        expected += 1;
        if derive::image_sig_text(heap, sym).is_none() {
            missing.push(value::symbol_name(sym));
        }
    }
    assert!(
        expected > 0,
        "no authoritative public declaration found in any imaged module"
    );
    missing.sort();
    assert!(
        missing.is_empty(),
        "{} authoritative declaration(s) the footer does not carry — a check naming them loads \
         the module for a type it could have read: {missing:?}",
        missing.len()
    );
}

#[test]
fn the_footer_is_the_source_scan_byte_for_byte() {
    // The footer caches `std_signature_index`; a process with no image computes it. The two
    // paths must agree exactly, else the checker's knowledge depends on a cache file.
    let mut interp = interp_with_image();
    let from_image = derive::image_sig_entries(&interp.heap);
    assert!(!from_image.is_empty(), "the index is empty");
    let scanned: Vec<(String, String, u32, u32)> = check::std_signature_index(&mut interp.heap)
        .into_iter()
        .map(|e| (e.name, e.text, e.min, e.max))
        .collect();
    assert_eq!(
        from_image.len(),
        scanned.len(),
        "the footer and a fresh scan of the embedded sources index different names"
    );
    for (a, b) in from_image.iter().zip(scanned.iter()) {
        assert_eq!(a, b, "footer entry differs from the source scan");
    }
}

#[test]
fn every_indexed_arity_is_the_loaded_closures_arity() {
    let mut interp = interp_with_image();
    interp
        .eval_str("(doseq (m (reflect/builtin-modules)) (try (require-one m) (catch _ nil)))")
        .expect("load every baked-in module");
    let heap = &interp.heap;
    let mut checked = 0usize;
    for (name, _, min, max) in derive::image_sig_entries(heap) {
        let sym = value::intern(&name);
        let Some(Value::Fn(cid)) = heap.env_get(heap.global(), sym) else {
            continue; // a native, or a name the module did not bind as a closure
        };
        let c = heap.closure(cid);
        let live_min = c.arms.iter().map(|a| a.min_arity()).min().unwrap_or(0) as u32;
        let live_max = c
            .arms
            .iter()
            .try_fold(0usize, |acc, a| a.max_arity().map(|m| acc.max(m)))
            .map_or(std_index::NO_MAX, |m| m as u32);
        assert_eq!(
            (min, max),
            (live_min, live_max),
            "{name}: the index says arity ({min}, {max}), the loaded closure ({live_min}, {live_max})"
        );
        checked += 1;
    }
    assert!(
        checked > 1000,
        "only {checked} closures compared — the index or the load is short"
    );
}
