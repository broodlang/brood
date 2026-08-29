//! **The kernel's dispatch identity must equal the language's.**
//!
//! `%identity-of` (`std/prelude/tools.blsp`) is what ability dispatch keys on, and it is
//! written in Brood. `Heap::dispatch_identity` is the kernel's copy, added so the compiler's
//! devirtualization and — under speculative dispatch (docs/dispatch-speculation.md) — native
//! guard code can ask the same question without each re-deriving the answer. Before it, the
//! compiler re-derived it by hand and said so in a comment: *"mirrors `identity-of`"*. A
//! comment is not a mechanism.
//!
//! Why this matters more than it looks: a speculation guard compares the identity it computes
//! against the one it expects, and then calls an impl directly. If the guard's notion of
//! identity ever diverges from the dispatcher's, the guard passes and **the wrong impl runs**
//! — no crash, no error, a different answer. So the two definitions agreeing is the whole
//! safety property that phase rests on, and it is asserted here rather than assumed.
//!
//! The subtle cases are the ones a re-derivation gets wrong, and each is covered below: a
//! plain map is `:map`, but a map carrying a truthy `:__id__` is that record; a map whose
//! `:__id__` is `nil` or `false` is a plain map again (matching `record?`); and every
//! non-map answers with its `type-of` kind.

use brood::Interp;

/// Evaluate `expr`, then ask BOTH definitions for its dispatch identity: the language's, by
/// calling `%identity-of` on the same expression, and the kernel's, by handing the evaluated
/// value to `Heap::dispatch_identity`. Returns `(language, kernel)` as printed strings.
fn both(interp: &mut Interp, expr: &str) -> (String, String) {
    let language = interp
        .eval_str(&format!("(%identity-of {expr})"))
        .map(|v| interp.print(v))
        .unwrap_or_else(|e| panic!("evaluating (%identity-of {expr}): {e:?}"));
    let value = interp
        .eval_str(expr)
        .unwrap_or_else(|e| panic!("evaluating {expr}: {e:?}"));
    let kernel = interp.print(interp.heap.dispatch_identity(value));
    (language, kernel)
}

#[test]
fn the_kernel_and_the_language_agree_on_every_dispatch_identity() {
    let mut interp = Interp::new();
    // A record, so the nominal-identity path is exercised against a real `defrecord`.
    interp
        .eval_str("(defrecord ident-probe (n))")
        .expect("define a record");

    let cases = [
        // Non-maps answer with their `type-of` kind.
        "1",
        "2.5",
        "\"s\"",
        ":kw",
        "true",
        "nil",
        "[1 2]",
        "(list 1 2)",
        "(fn (x) x)",
        // A plain map is `:map` — NOT a record.
        "{}",
        "{:a 1}",
        // A real record answers with its nominal id.
        "(ident-probe 3)",
        // The three cases a re-derivation gets wrong. A hand-written `:__id__` decides
        // identity if truthy; a falsy one leaves the value a plain map, which is exactly
        // what `record?` documents and what a naive `map_get(:__id__)` would miss.
        "{:__id__ :hand/written}",
        "{:__id__ nil :a 1}",
        "{:__id__ false :a 1}",
    ];

    let mut disagreements = Vec::new();
    for expr in cases {
        let (language, kernel) = both(&mut interp, expr);
        if language != kernel {
            disagreements.push(format!(
                "  {expr}\n    %identity-of      → {language}\n    dispatch_identity → {kernel}"
            ));
        }
    }
    assert!(
        disagreements.is_empty(),
        "the kernel's dispatch identity disagrees with the language's on {} case(s). A guard \
         built on the kernel's answer would pass and call the impl the dispatcher would NOT \
         have chosen:\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}
