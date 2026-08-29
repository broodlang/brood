//! Every name the checker curates a signature for must actually exist.
//!
//! `types/check/sigs.rs` hand-writes signatures for names `infer_sig` cannot walk. The
//! checker treats a curated name as KNOWN — so if a rename moves the function and the
//! entry is left behind, the checker stops reporting that name as unbound. Not "reports
//! the wrong type": stops reporting it at all. `(max 1 2)` then checks clean in ordinary
//! code, is not inside a `try`, has no warning of any kind, and raises at run time.
//!
//! That is exactly how hive shipped a broken `clamp-limit` — `(max 1 (min 100 …))` against
//! a brood where both had become `math/max`/`math/min`. `nest check` said nothing, the
//! suite said nothing, and the only thing that caught it was a docstring example being
//! executed by a test written for an unrelated reason.
//!
//! A stale entry is silent by construction, so the guard has to be mechanical: take the
//! table's own keys and require each to resolve in a fresh image. Three of sixty-six were
//! stale when this was written (`min`, `max`, `member?`).

use brood::Interp;

/// Names the checker curates, read out of the source of truth rather than re-listed here —
/// a hand-copied list would drift from the table it is meant to police.
fn curated_names() -> Vec<String> {
    let src = include_str!("../src/types/check/sigs.rs");
    let mut names = Vec::new();

    // `put("name", …)` — the ordinary form.
    for rest in src.split("put(").skip(1) {
        let rest = rest.trim_start();
        if let Some(inner) = rest.strip_prefix('"') {
            if let Some(end) = inner.find('"') {
                names.push(inner[..end].to_string());
            }
        }
    }
    // `for n in ["a", "b"] { put(n, …) }` — the grouped form.
    for rest in src.split("for n in [").skip(1) {
        if let Some(end) = rest.find(']') {
            for token in rest[..end].split(',') {
                let token = token.trim().trim_matches('"');
                if !token.is_empty() {
                    names.push(token.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

#[test]
fn every_curated_signature_names_something_that_exists() {
    let names = curated_names();
    assert!(
        names.len() > 40,
        "only {} curated names parsed — the reader has drifted from sigs.rs's shape, \
         which would make this guard silently vacuous",
        names.len()
    );

    let mut interp = Interp::new();
    // The curated names span modules (`math/…`, `string/…`), and a bare image holds only
    // the prelude, so load everything before asking.
    let _ = interp
        .eval_str("(doseq (m (reflect/builtin-modules)) (try (require-one m) (catch _ nil)))");

    let mut stale = Vec::new();
    for name in &names {
        let query = format!("(bound? '{name})");
        match interp.eval_str(&query) {
            Ok(value) => {
                if interp.print(value).trim() != "true" {
                    stale.push(name.clone());
                }
            }
            Err(_) => stale.push(name.clone()),
        }
    }

    assert!(
        stale.is_empty(),
        "these names are curated in types/check/sigs.rs but no longer exist: {stale:?}\n\
         A curated name is treated as KNOWN by the checker, so each of these is a name it \
         will never report as unbound — silently accepting code that raises at run time. \
         Move the entry to the new name, or delete it."
    );
}
