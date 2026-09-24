//! KI-171: the transitive scan (ADR-340) loads what a module's bodies reference, with no
//! trace armed. ADR-370's first shape of `materialise_referenced_modules` wrote
//! `trace && wanted.insert(module)`, so the load set was only ever filled when
//! `BROOD_IMAGE_TRACE` was set — and the one test of the scan (`tests/lazy_load_test.blsp`
//! § ADR-370) ran its child WITH the trace, observing the loads through it. In every ordinary
//! process the scan was a no-op for two days.
//!
//! Its own process, not the in-crate suite, like `curated_names_load_nothing.rs` and for the
//! same reason: which std modules are loaded is process-wide state once an image is
//! installed, so a test that must see a module NOT loaded before the scan cannot hold that
//! precondition after other tests ran beside it. In the crate suite it passed under nextest
//! (a process per test) and failed whenever the plain harness ran it among other threads —
//! the sanitizer job does, and went red on it (2026-09-24).
//!
//! The edge is planted: a fixture module whose body names `table/get`, whose result is `any`
//! by nature — a fresh copy of whatever was stored — so no declaration rides for it
//! (`image_carried_sig` declines a `-> any`) and no curated entry stands in for it. `std`
//! edges were tried first and each went away as coverage improved (`json` → `reflect` for
//! the curated `reflect/read-string`), which is the point of planting one.

use brood::types::check;
use brood::Interp;

fn loaded(interp: &mut Interp, module: &str) -> bool {
    let value = interp
        .eval_str(&format!("(contains? *features* \"{module}\")"))
        .expect("read *features*");
    interp.print(value) == "true"
}

#[test]
fn transitive_scan_loads_without_the_trace() {
    assert!(
        std::env::var_os("BROOD_IMAGE_TRACE").is_none(),
        "this test observes the untraced path; unset BROOD_IMAGE_TRACE"
    );
    // Build the image, when the cache holds none for this std, in a THROWAWAY runtime:
    // `stdimage/build` loads every module into the runtime that runs it.
    {
        let mut builder = Interp::new();
        builder
            .eval_str("(or (%std-image-installed) (%std-image-install) (stdimage/build))")
            .expect("build the stdlib image");
    }
    let mut interp = Interp::new();
    let installed = interp
        .eval_str("(or (%std-image-installed) (%std-image-install))")
        .map(|v| interp.print(v))
        .expect("install the stdlib image");
    assert_ne!(
        installed, "nil",
        "no stdlib image could be installed even after building one"
    );
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
    assert!(
        loaded(&mut interp, "ki171-probe"),
        "loading the fixture file did not register its module as a feature"
    );
    assert!(
        !loaded(&mut interp, "table"),
        "table is already loaded in a fresh process — the probe edge is gone, pick another"
    );
    check::materialise_referenced_modules(&mut interp.heap);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        loaded(&mut interp, "table"),
        "the transitive scan did not load `table` for the fixture's `table/get`"
    );
}
