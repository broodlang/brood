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
        let diags = warnings("(def r (map cons (list 1 2 3)))");
        let hit = diags
            .iter()
            .find(|d| d.message.contains("callback") && d.message.contains("cons"))
            .expect("expected a callback-arity warning");
        assert_eq!(hit.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(hit.source.as_deref(), Some("brood"));
    }

    #[test]
    fn a_correct_arity_callback_produces_no_callback_warning() {
        let diags = warnings("(def r (map inc (list 1 2 3)))");
        assert!(
            diags.iter().all(|d| !d.message.contains("callback")),
            "a correct-arity callback must not warn: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
