use super::*;

/// The advisory type-check diagnostics the LSP would publish for `src` — the
/// exact path `publish` takes, minus the wire send. `Interp::new()` loads the
/// prelude, so prelude names (`cons`, `inc`, `map`, …) resolve.
fn warnings(src: &str) -> Vec<Diagnostic> {
    let mut interp = brood::Interp::new();
    let a = analyze(src);
    typecheck_diagnostics(&mut interp, src, &a.cst, &a.line_index)
}

#[test]
fn surfaces_the_callback_arity_warning_as_a_brood_warning() {
    // The Step-5+ arrow check (ADR-078) must reach the editor: `map` calls
    // its callback with one arg, but `cons` takes two.
    let diags = warnings("(def r (map (list 1 2 3) cons))");
    let hit = diags
        .iter()
        .find(|d| d.message.contains("callback") && d.message.contains("cons"))
        .expect("expected a callback-arity warning");
    assert_eq!(hit.severity, Some(DiagnosticSeverity::WARNING));
    assert_eq!(hit.source.as_deref(), Some("brood"));
}

#[test]
fn a_correct_arity_callback_produces_no_callback_warning() {
    let diags = warnings("(def r (map (list 1 2 3) inc))");
    assert!(
        diags.iter().all(|d| !d.message.contains("callback")),
        "a correct-arity callback must not warn: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn a_deprecated_call_is_tagged_deprecated_not_only_warned() {
    // ADR-283 makes a deprecation a checker warning at the CALL SITE, and the prelude
    // deprecates `not=` in favour of `not`. `nest check` prints it and `nest doc` strikes the
    // heading through; the editor knew nothing, because a plain WARNING is a yellow squiggle
    // and nothing more. The tag is what makes a client render the name struck through.
    //
    // This pins the coupling in `typecheck_diagnostics`: `check_file` returns
    // `(Option<Pos>, String)` with no category channel, so the LSP recognises a deprecation by
    // the checker's own wording (`types/check/walk.rs::stability_msg`). Reword either side and
    // this fails, which is the point — the alternative is the tag silently never appearing.
    let diags = warnings("(defn f (a b) (not= a b))");
    let hit = diags
        .iter()
        .find(|d| d.message.contains("deprecated"))
        .expect("expected a deprecation warning for `not=`");
    assert_eq!(
        hit.tags.as_deref(),
        Some(&[DiagnosticTag::DEPRECATED][..]),
        "a deprecation must carry DiagnosticTag::DEPRECATED, not only WARNING severity: {:?}",
        hit
    );
    // Advisory, never gating — a deprecation that fails the build is a removal with extra steps.
    assert_eq!(hit.severity, Some(DiagnosticSeverity::WARNING));
    // …and the replacement `:use` names is in the message, which is what makes it actionable.
    assert!(hit.message.contains("not"), "message: {}", hit.message);
}

#[test]
fn an_ordinary_warning_carries_no_deprecated_tag() {
    // The negative half: the tag must come from the deprecation fact, not from every warning.
    let diags = warnings("(def r (map (list 1 2 3) cons))");
    let hit = diags
        .iter()
        .find(|d| d.message.contains("callback"))
        .expect("expected a callback-arity warning");
    assert_eq!(hit.tags, None, "only a deprecation is tagged: {hit:?}");
}
