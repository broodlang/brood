//! A name in the checker's curated table asks the transitive scan (ADR-340) for no load:
//! `sig_of` reads the table ahead of the stdlib image's footer and of inference, so whether
//! the name's module is loaded changes nothing the checker says about a call to it. The scan
//! loaded it anyway until 2026-09-20 — 38 std modules' bodies name `seq/filter`, 26 name
//! `io/puts`, and every check that loaded any of them materialised `seq` and `io` for a type
//! it already had.
//!
//! Its own process, not the in-crate suite: once an image is installed, which std modules
//! are loaded is process-wide state (the install shares the prelude region), so a test that
//! must observe a module NOT loading cannot hold that precondition after other tests ran.
//! The construction-level counterpart — every curated name reads the same loaded or not —
//! is `check/tests/image_sigs.rs::a_curated_name_reads_the_same_loaded_or_not`.

use brood::types::check;
use brood::Interp;

fn loaded(interp: &mut Interp, module: &str) -> bool {
    let value = interp
        .eval_str(&format!("(contains? *features* \"{module}\")"))
        .expect("read *features*");
    interp.print(value) == "true"
}

#[test]
fn a_curated_name_asks_for_no_load() {
    assert!(
        std::env::var_os("BROOD_IMAGE_TRACE").is_none(),
        "this test observes the untraced path; unset BROOD_IMAGE_TRACE"
    );
    const PROBES: [(&str, &str); 4] = [
        ("io/puts", "io"),
        ("seq/filter", "seq"),
        ("math/nan?", "math"),
        ("reflect/read-string", "reflect"),
    ];
    let mut interp = Interp::new();
    let installed = interp
        .eval_str("(or (%std-image-installed) (%std-image-install) (do (stdimage/build) (%std-image-install)))")
        .map(|v| interp.print(v))
        .expect("install the stdlib image");
    assert_ne!(
        installed, "nil",
        "no stdlib image could be installed even after building one"
    );
    for (name, _) in PROBES {
        assert!(
            check::is_curated(name),
            "{name} is no longer curated — the probe needs another curated, unbound name"
        );
    }
    let dir = std::env::temp_dir().join(format!("curated-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("curated-probe.blsp");
    std::fs::write(
        &file,
        "(defmodule curated-probe)\n\
         (defn say (x) (io/puts x))\n\
         (defn evens (xs) (seq/filter xs (fn (x) (math/nan? x))))\n\
         (defn read (s) (reflect/read-string s))\n",
    )
    .expect("write the fixture");
    interp
        .eval_str(&format!("(reflect/load {:?})", file.display().to_string()))
        .expect("load the fixture");
    assert!(loaded(&mut interp, "curated-probe"));
    let observable: Vec<&str> = PROBES
        .iter()
        .map(|(_, module)| *module)
        .filter(|module| !loaded(&mut interp, module))
        .collect();
    assert!(
        !observable.is_empty(),
        "every probe module is already loaded in a fresh process — the scan's decision cannot be observed"
    );
    check::materialise_referenced_modules(&mut interp.heap);
    let _ = std::fs::remove_dir_all(&dir);
    for module in observable {
        assert!(
            !loaded(&mut interp, module),
            "the transitive scan loaded `{module}` for a curated name"
        );
    }
}
