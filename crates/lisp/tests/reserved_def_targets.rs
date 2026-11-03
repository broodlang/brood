//! Where a `def` stops (ADR-166, design pass item 14), for the two targets the gate missed.
//!
//! An embedded std module's names were sealed only once the module loaded, so in a process
//! that had not loaded `string`, `(def string/join …)` was accepted. And a special form's
//! name was not reserved at all: `(def if 1)` bound a global no call could reach. Each case
//! runs in a fresh `Interp`, because the first half needs a module nothing has loaded yet.

use brood::Interp;

fn eval(src: &str) -> Result<String, String> {
    let mut interp = Interp::new();
    interp
        .eval_str(src)
        .map(|v| interp.print(v))
        .map_err(|e| e.message.clone())
}

#[test]
fn a_def_under_an_unloaded_std_module_is_refused() {
    // Not vacuous: `string` is not loaded in a fresh runtime, which is the case that slipped.
    assert_eq!(eval("(bound? 'string/join)").as_deref(), Ok("false"));
    let refused = eval("(reflect/eval-string \"(def string/join (fn (& _) :hijacked))\")");
    let message = refused.expect_err("a def under an unloaded std module was accepted");
    assert!(message.contains("reserved name"), "{message}");
    // …and the module's own function is the one that answers afterwards.
    assert_eq!(
        eval(
            "(try (reflect/eval-string \"(def string/join 1)\") (catch _ nil)) \
             (string/join [\"a\" \"b\"] \",\")"
        )
        .as_deref(),
        Ok("\"a,b\"")
    );
}

#[test]
fn a_def_under_your_own_namespace_loads_nothing_and_defines() {
    assert_eq!(
        eval("(def myns/answer 42) myns/answer").as_deref(),
        Ok("42")
    );
}

#[test]
fn a_special_form_name_cannot_be_defined() {
    for form in ["if", "let", "fn", "quote", "do"] {
        let message = eval(&format!("(reflect/eval-string \"(def {form} 1)\")"))
            .expect_err("a def of a special form's name was accepted");
        assert!(message.contains("special form"), "{form}: {message}");
    }
}
