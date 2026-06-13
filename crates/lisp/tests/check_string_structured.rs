//! `check-string-structured` — the source-string counterpart of
//! `check-file-structured`, backing live editor-buffer diagnostics (myedit's
//! `:diagnostics` mode service). It type-checks a *string* (an unsaved buffer)
//! and returns `{:line :col :message}` maps, or `()` when the source doesn't
//! parse — so a mid-edit, unbalanced buffer never errors the diagnostics loop.

use brood::Interp;

/// Evaluate `src` in a fresh, prelude-loaded image and return its printed result.
fn out(src: &str) -> String {
    let mut interp = Interp::new();
    let v = interp.eval_str(src).expect("eval");
    interp.print(v)
}

#[test]
fn warns_with_position_and_message_on_a_bad_form() {
    // `(map cons …)` calls the 2-ary `cons` with one arg — the callback-arity
    // warning (ADR-078), carrying a 1-based :line/:col and a string :message.
    let bad = r#"(check-string-structured "(map cons (list 1 2 3))")"#;
    assert_eq!(out(&format!("(count {bad})")), "1");
    assert_eq!(out(&format!("(get (first {bad}) :line)")), "1");
    assert_eq!(
        out(&format!("(string? (get (first {bad}) :message))")),
        "true"
    );
}

#[test]
fn empty_on_clean_unparsable_or_empty_source() {
    assert_eq!(
        out(r#"(empty? (check-string-structured "(def x 1)"))"#),
        "true"
    );
    // incomplete input (mid-edit) — no diagnostics rather than an error
    assert_eq!(out(r#"(empty? (check-string-structured "("))"#), "true");
    assert_eq!(out(r#"(empty? (check-string-structured ""))"#), "true");
}
