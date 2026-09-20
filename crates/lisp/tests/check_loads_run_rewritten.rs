//! KI-172: a module the CHECKER loads is the runtime's module. The checker holds
//! `NoSourceRewrites` across its compile pass so the file under check is read as the author
//! wrote it — but that pass is also what infers and performs the file's `require`s
//! (ADR-227), and the transitive scan (ADR-340) loads more from inside the same check. Those
//! loads expanded under the checker's flag, so every std module a `brood file.blsp`
//! pre-flight brought in ran WITHOUT the optimiser's source rewrites for the rest of the
//! program: `seq/frequencies` over 750k keys 860 ms against 343 ms with the check skipped,
//! the same as `BROOD_LINMAP=0`; and a stdlib image written by such a process carried the
//! unrewritten bodies to every later run. The loader holds `SourceRewritesOn` now.
//!
//! Its own process: whether `seq` is loaded is process-wide state once anything installed
//! an image, and the point is that THIS check is what loads it. `BROOD_NO_STDIMAGE=1` so
//! the load expands source (an imaged module is materialised, not expanded — the image
//! side of the same defect is `image_writer_differential`).

use brood::core::value::{self, Value};
use brood::Interp;

/// Does any form under `form` name `symbol`? The tally rewrite's marker is
/// `%table-from-map`, which no std source spells.
fn mentions(interp: &Interp, form: Value, symbol: &str) -> bool {
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        match v {
            Value::Sym(s) if value::symbol_is(s, symbol) => return true,
            Value::Pair(_) => {
                if let Ok(items) = interp.heap.seq_items(v) {
                    work.extend(items);
                }
            }
            Value::Vector(id) => work.extend(interp.heap.vector(id).iter().copied()),
            _ => {}
        }
    }
    false
}

#[test]
fn a_module_the_check_loads_is_expanded_with_the_rewrites() {
    // SAFETY-free: `set_var` before any thread reads it is the documented pattern in this
    // suite; the runtime reads it at boot.
    std::env::set_var("BROOD_NO_STDIMAGE", "1");
    let mut interp = Interp::new();
    let loaded = |interp: &mut Interp| -> bool {
        let v = interp
            .eval_str("(contains? *features* \"seq\")")
            .expect("read *features*");
        interp.print(v) == "true"
    };
    assert!(
        !loaded(&mut interp),
        "seq is loaded before the check — the probe cannot attribute the load"
    );
    // A file whose own reference drains `seq` in during the check.
    let forms =
        brood::syntax::reader::read_all(&mut interp.heap, "(defn tally (xs) (seq/frequencies xs))")
            .expect("parse");
    let _ = brood::types::check::check_file(&mut interp.heap, &forms);
    assert!(
        loaded(&mut interp),
        "the check did not load seq — nothing to observe"
    );
    let sym = value::intern("seq/frequencies");
    let Some(Value::Fn(cid)) = interp.heap.env_get(interp.heap.global(), sym) else {
        panic!("seq/frequencies is not a closure after the load");
    };
    let bodies: Vec<Value> = interp
        .heap
        .closure(cid)
        .arms
        .iter()
        .flat_map(|arm| arm.body.iter().copied())
        .collect();
    assert!(
        bodies.iter().any(|&b| mentions(&interp, b, "%table-from-map")),
        "seq/frequencies was loaded by the check WITHOUT the tally rewrite (no `%table-from-map` in \
         its body): the program that runs after this pre-flight pays the unrewritten fold"
    );
}
