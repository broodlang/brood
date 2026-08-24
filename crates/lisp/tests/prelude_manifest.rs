//! That every `std/prelude/*.blsp` file actually reaches the `PRELUDE` const.
//!
//! The prelude used to be one 5779-line file; it is now nine files concatenated in a
//! hand-written `concat!(include_str!(…), …)` list in `lib.rs`. Evaluation order is
//! load-bearing (macros before use, forward refs), so that list has to stay hand-written
//! — it cannot be derived from a directory listing.
//!
//! The failure mode that creates is silent. Add `std/prelude/foo.blsp`, forget the
//! `include_str!` line, and nothing complains: the workspace builds, the boot cache is
//! rebuilt happily, and the file's `defn`s simply do not exist. The first symptom is an
//! unbound name somewhere far from the omission.
//!
//! So this asserts inclusion against the const itself rather than grepping `lib.rs`:
//! whatever spelling the list uses, each file's bytes must appear in `PRELUDE`, and
//! `PRELUDE` must be exactly the nine files' bytes with nothing else in it.
//!
//! Order is deliberately *not* asserted — it is a design decision recorded in the
//! `concat!` list, not something recoverable from filenames (the numeric prefixes were
//! dropped on purpose).

use std::path::PathBuf;

fn prelude_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../std/prelude")
}

/// Every `.blsp` file in `std/prelude/`, as (filename, contents).
fn prelude_files() -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = std::fs::read_dir(prelude_dir())
        .expect("read std/prelude")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "blsp"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (
                name,
                std::fs::read_to_string(&p).expect("read prelude file"),
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn every_prelude_file_is_included_in_the_concat() {
    let files = prelude_files();
    assert!(
        !files.is_empty(),
        "no .blsp files found in std/prelude — the split is gone, or the path is wrong"
    );

    for (name, body) in &files {
        assert!(
            brood::PRELUDE.contains(body.as_str()),
            "std/prelude/{name} is not in the PRELUDE const — add its include_str! line to \
             the concat! in crates/lisp/src/lib.rs (at the right position: evaluation \
             order is load-bearing)"
        );
    }
}

/// Files OUTSIDE `std/prelude/` that are deliberately part of `PRELUDE`. Each one is a
/// judgement that something is *core* rather than a library, so it is listed here by hand:
/// adding one has to be a decision, not a drift.
///
/// `std/protocol.blsp` joined on 2026-08-21 — behaviour contracts (`defbehaviour`,
/// `register-protocol`, `ops`, `*protocols*`) are core, so the prelude carries them.
///
/// `std/proc/gen.blsp` joined on 2026-08-21 (`7cb796f0`) — the gen_server framework is
/// core and bare. It was added to `lib.rs`'s `concat!` without being listed here, so this
/// test had been **red on `main` ever since**; recorded as KI-54 along with the two other
/// failures that commit caused. That is exactly the drift the hand-maintained list exists
/// to catch, and it caught it — nothing was watching the result.
const EXTRA_PRELUDE_FILES: &[&str] = &["std/protocol.blsp", "std/proc/gen.blsp"];

/// The complement: `PRELUDE` is the split files plus [`EXTRA_PRELUDE_FILES`] and nothing
/// more. Catches a stale `include_str!` of a file that has since been renamed away or
/// absorbed, which the containment check above cannot see.
#[test]
fn prelude_const_is_exactly_the_split_files() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut total: usize = prelude_files().iter().map(|(_, body)| body.len()).sum();
    for extra in EXTRA_PRELUDE_FILES {
        let body = std::fs::read_to_string(root.join(extra)).unwrap_or_else(|e| {
            panic!("EXTRA_PRELUDE_FILES names {extra}, which will not read: {e}")
        });
        assert!(
            brood::PRELUDE.contains(body.as_str()),
            "{extra} is listed in EXTRA_PRELUDE_FILES but is not in PRELUDE — either add its \
             include_str! line to the concat! in lib.rs, or drop it from that list"
        );
        total += body.len();
    }
    assert_eq!(
        brood::PRELUDE.len(),
        total,
        "PRELUDE is {} bytes but std/prelude/*.blsp + EXTRA_PRELUDE_FILES total {total} — \
         the concat! list in lib.rs includes something twice, includes a file listed in \
         neither, or is missing one. A deliberate non-prelude/ addition goes in \
         EXTRA_PRELUDE_FILES with a note saying why it is core.",
        brood::PRELUDE.len()
    );
}
