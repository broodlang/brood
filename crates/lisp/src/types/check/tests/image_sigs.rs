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
    // With no image on disk (`BROOD_NO_STDIMAGE=1` skips the nextest setup script that
    // builds one — CI's tree-walker job) it is built here, in a THROWAWAY interpreter:
    // `stdimage/build` loads every std module from source, and the interpreter it ran in
    // would hand the tests a process where everything is already loaded — a transitive
    // load could then never be observed.
    let install = "(or (%std-image-installed) (%std-image-install))";
    let mut interp = crate::Interp::new();
    if interp
        .eval_str(install)
        .map(|v| interp.print(v))
        .expect("install the stdlib image")
        == "nil"
    {
        crate::Interp::new()
            .eval_str("(stdimage/build)")
            .expect("build the stdlib image");
        interp = crate::Interp::new();
    }
    let installed = interp
        .eval_str(install)
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
        // A variable-bearing declaration is read through its own reader, and the footer's
        // must be the loaded module's — its flat return is `any` by construction (`?A`
        // parses to `any`), which is why it is checked apart.
        match sigs::declared_heap_sig_with_vars(heap, sym) {
            Some(declared_vars) => {
                let carried_vars = sigs::image_heap_sig_with_vars(heap, sym).unwrap_or_else(|| {
                    panic!("{name}: the declaration carries type variables and the footer's reading has none")
                });
                assert_eq!(
                    format!("{carried_vars:?}"),
                    format!("{declared_vars:?}"),
                    "{name}: the carried type-variable reading is not the loaded declaration"
                );
            }
            None => assert!(
                !carried.ret.is_any() && !carried.ret.is_unrefined_collection(),
                "{name}: a return of `{}` is re-typed from the body at a call site and may not ride",
                carried.ret
            ),
        }
        assert!(
            !heap.is_private(sym),
            "{name}: a private name's cross-module call is a warning only the loaded module can raise"
        );
        // What a call site reads, with the module loaded, is what it read from the footer
        // alone — except for an extremum, whose `sig_of` is the registry-derived operator
        // domain (ADR-299) ahead of every declaration, loaded or carried alike.
        if !matches!(name.as_str(), "math/max" | "math/min") {
            assert_eq!(
                sigs::sig_of(heap, sym),
                Some(carried),
                "{name}: sig_of disagrees with the footer"
            );
        }
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

#[test]
fn a_declaration_whose_only_marker_is_optional_rides() {
    // `&optional` is a parameter-list MARKER of the arrow grammar (`annot::arrow_of`), the
    // same as `&` — not a name whose meaning depends on a loaded module. It was missing from
    // `std_index::every_symbol_is_a_type_word`, so every declaration using one was read as
    // naming a record or alias and declined: `string/pad-left`, `string/fields`,
    // `markdown/->html`, `reflect/type-aliases` and `string/number->` all stayed loads for a
    // reason the grammar does not have.
    //
    // Asserted on the SOURCE SCAN first, because that is what the writer stores and it holds
    // whatever the cache contains — a footer-only assertion passes vacuously against an image
    // built before the fix (which is how this test first went green under sabotage). The
    // footer assertion follows it: that is what a call site actually reads.
    const NAMES: [&str; 4] = [
        "string/pad-left",
        "string/pad-right",
        "string/fields",
        "string/number->",
    ];
    let mut interp = interp_with_image();
    let scanned = check::std_signature_index(&mut interp.heap);
    for name in NAMES {
        let entry = scanned
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name}: not indexed at all"));
        assert!(
            entry.text.contains("&optional"),
            "{name}: the scan carries `{}`, so an `&optional` declaration declined",
            entry.text
        );
    }
    let heap = &interp.heap;
    for name in NAMES {
        let sym = value::intern(name);
        let text = derive::image_sig_text(heap, sym)
            .unwrap_or_else(|| panic!("{name}: the scan carries a type and the footer does not"));
        assert!(
            text.contains("&optional"),
            "{name}: the footer carries `{text}`, which is not the declaration under test"
        );
        assert!(
            sigs::image_heap_sig(heap, sym).is_some(),
            "{name}: the carried text does not parse back to an arrow"
        );
    }
}

#[test]
fn transitive_scan_loads_without_the_trace() {
    // KI-171. ADR-370's first shape of `materialise_referenced_modules` wrote
    // `trace && wanted.insert(module)`, so the load set was only ever filled when
    // `BROOD_IMAGE_TRACE` was set — and the one test of the scan (`tests/lazy_load_test.blsp`
    // § ADR-370) ran its child WITH the trace, observing the loads through it. In every
    // ordinary process the transitive scan (ADR-340) was a no-op for two days.
    //
    // This calls the scan itself, with no trace, on an edge nothing can remove: a fixture
    // module whose body names `table/get`, whose result is `any` by nature — a fresh copy
    // of whatever was stored — so no declaration rides for it (`image_carried_sig`
    // declines a `-> any`) and no curated entry stands in for it. `std` edges were tried
    // first and each went away as coverage improved (`json` → `reflect` for the curated
    // `reflect/read-string`), which is the point of planting one. The test is only as good
    // as the environment it runs in: with `BROOD_IMAGE_TRACE` set it would have passed
    // against the bug, so it refuses to run traced.
    assert!(
        std::env::var_os("BROOD_IMAGE_TRACE").is_none(),
        "this test observes the untraced path; unset BROOD_IMAGE_TRACE"
    );
    let mut interp = interp_with_image();
    let dir = std::env::temp_dir().join(format!("ki171-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("ki171-probe.blsp");
    std::fs::write(
        &file,
        "(defmodule ki171-probe)\n(defn read-it (t k) (table/get t k))\n",
    )
    .expect("write the fixture");
    interp
        .eval_str(&format!("(reflect/load {:?})", file.display().to_string()))
        .expect("load the fixture");
    let loaded = |interp: &mut crate::Interp, m: &str| -> bool {
        let v = interp
            .eval_str(&format!("(contains? *features* \"{m}\")"))
            .expect("read *features*");
        interp.print(v) == "true"
    };
    assert!(
        loaded(&mut interp, "ki171-probe"),
        "loading the fixture file did not register its module as a feature"
    );
    assert!(
        !loaded(&mut interp, "table"),
        "table is already loaded — the probe edge is gone, pick another"
    );
    check::materialise_referenced_modules(&mut interp.heap);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        loaded(&mut interp, "table"),
        "the transitive scan did not load `table` for the fixture's `table/get`"
    );
}

#[test]
fn a_defseq_definition_is_indexed() {
    // `seq/filter`, `seq/reject` and `seq/keep` are `(%defseq name (params…) doc step)`
    // forms — a macro over `defn` — and the scanner read past them: no entry, so no arity
    // for a check that never loads `seq`, and a whole-module load for the name 38 std
    // modules' bodies reach for. The construction gate beside this
    // (`every_indexed_arity_is_the_loaded_closures_arity`) holds the arity to the closure.
    let mut interp = interp_with_image();
    let scanned = check::std_signature_index(&mut interp.heap);
    for (name, min, max) in [
        ("seq/filter", 2, 2),
        ("seq/reject", 2, 2),
        ("seq/keep", 2, 2),
    ] {
        let entry = scanned
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("{name}: not indexed — the scanner did not descend %defseq"));
        assert_eq!((entry.min, entry.max), (min, max), "{name}: arity");
    }
}

#[test]
fn a_type_variable_declaration_rides() {
    // `math/max` declares `(& ?A -> ?A)`. Its flat reading has an `any` return (`?A` parses
    // to `any`), so the writer declined it and the checker loaded `math` for it — behind 47
    // of 100 corpus checks (2026-09-20). A variable-bearing declaration is resolved per call
    // from the arguments, which is the reading a call site takes FIRST for a declared name;
    // the footer now carries it and `image_heap_sig_with_vars` reads it back.
    //
    // The VERDICT is not asserted here on purpose: an extremum has a by-name rule
    // (`is_extremum`) that answers `(math/max 1 2)` from its operands with no declaration
    // at all, so a verdict on it passed with the footer fallback removed (sabotage,
    // 2026-09-20) — and std declares no other variable-bearing signature. The call-site
    // reader is gated in its own process by `tests/image_sig_type_variables.rs`, which
    // plants a footer entry no rule knows.
    let interp = interp_with_image();
    let sym = value::intern("math/max");
    assert!(
        derive::image_sig_text(&interp.heap, sym).is_some(),
        "the footer carries no type for math/max"
    );
    assert!(
        sigs::image_heap_sig_with_vars(&interp.heap, sym).is_some(),
        "the footer's text does not read as a type-variable declaration"
    );
}

#[test]
fn a_curated_name_reads_the_same_loaded_or_not() {
    // The curated skip (a curated name asks the transitive scan for no load) is sound only
    // if `sig_of` answers the same for the name whether or not its module is in the heap.
    // The hole it could hide: a name that is curated AND declared with a declaration the
    // footer declines (`-> any`, an overload) — loaded, `sig_of` reads the declaration;
    // unloaded it reads the curated entry, and the skip means nothing ever loads the module
    // to close the gap. The tree differential says no such name is EXERCISED; this says
    // none EXISTS: for every curated name that is an imaged function, `sig_of` before its
    // module loads equals `sig_of` after.
    let mut interp = interp_with_image();
    let mut names: Vec<(String, value::Symbol)> = sigs::curated_names()
        .into_iter()
        .filter(|&sym| derive::image_sig_arity(&interp.heap, sym).is_some())
        .map(|sym| (value::symbol_name(sym), sym))
        .collect();
    names.sort();
    assert!(
        names.len() >= 10,
        "only {} curated names are imaged functions — the probe is not looking at std",
        names.len()
    );
    let before: Vec<Option<crate::types::Sig>> = names
        .iter()
        .map(|&(_, sym)| sigs::sig_of(&interp.heap, sym))
        .collect();
    let mut modules: Vec<String> = names.iter().map(|(_, sym)| module_of(*sym)).collect();
    modules.sort();
    modules.dedup();
    for module in &modules {
        interp
            .eval_str(&format!("(require-one '{module})"))
            .unwrap_or_else(|e| panic!("load {module}: {e:?}"));
    }
    for ((name, sym), before) in names.iter().zip(before) {
        let after = sigs::sig_of(&interp.heap, *sym);
        assert_eq!(
            before, after,
            "{name}: sig_of answers differently with its module loaded — the curated skip \
             would hide a declaration the footer does not carry"
        );
        // A call site reads an OVERLOAD (`declared_heap_overload`) and a type-variable
        // declaration (`declared_heap_sig_with_vars`) ahead of `sig_of`, and neither goes
        // through `as_arrow`, so `sig_of` agreeing proves nothing about them: the first
        // shape of this gate passed with `math/pow` — a four-arm overload — curated
        // (sabotage, 2026-09-20). Loaded, those readers must find nothing the footer does
        // not also carry.
        assert!(
            sigs::declared_heap_overload(&interp.heap, *sym).is_none(),
            "{name}: curated AND declared as an overload — loaded, a call resolves per arm; \
             unloaded, it reads the curated entry, and the skip never loads the module"
        );
        assert!(
            sigs::declared_heap_sig_with_vars(&interp.heap, *sym).is_none()
                || sigs::image_heap_sig_with_vars(&interp.heap, *sym).is_some(),
            "{name}: curated AND declared with type variables the footer does not carry"
        );
    }
}
