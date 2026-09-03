//! Every declared single-argument producer of a `failure` must be narrowable through.
//!
//! `guards::DETERMINISTIC_UNARY` is the allow-list a type guard narrows through: it is what
//! makes `(if (failure? (parse s)) d (parse s))` — the spelling that does not bind the value
//! — type the second occurrence as the non-failure half. ADR-316 reports a failure arm
//! nothing guards, so a parser MISSING from that list does not merely lose precision: the
//! arm survives the guard, the inferred return keeps it, and the lint reports every call
//! site of a function that provably cannot fail. That is the false positive v0.25.0 shipped,
//! for the one parser (`string/->number`) the list did not yet exist to hold.
//!
//! The drift is silent by construction — a new `(sig f (x -> (or T failure)))` lands, nobody
//! touches the Rust list, and nothing fails. So the guard is mechanical, in both directions:
//! take the list's own entries and the std sigs' own text, and require them to agree.

/// The allow-list, read out of its source rather than re-listed here — a hand-copied list
/// would drift from the one it is meant to police.
fn allow_list() -> Vec<String> {
    let src = include_str!("../src/types/check/guards.rs");
    let start = src
        .find("pub(super) const DETERMINISTIC_UNARY: &[&str] = &[")
        .expect("DETERMINISTIC_UNARY is declared");
    let body = &src[start..];
    let end = body.find("];").expect("the list is closed");
    body[..end]
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim();
            let inner = line.strip_prefix('"')?;
            Some(inner[..inner.find('"')?].to_string())
        })
        .collect()
}

/// Every `(sig name (one-param -> … failure …))` declared across `std/`, qualified with its
/// file's module name. Text, not the image: this must see a signature the moment it is
/// written, including in a module nothing has loaded.
fn declared_unary_failure_producers() -> Vec<String> {
    let mut out = Vec::new();
    for (module, src) in [
        ("encoding", include_str!("../../../std/encoding.blsp")),
        ("url", include_str!("../../../std/url.blsp")),
        ("datetime", include_str!("../../../std/datetime.blsp")),
        ("string", include_str!("../../../std/string.blsp")),
    ] {
        for line in src.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("(sig ") else {
                continue;
            };
            if !rest.contains("failure") {
                continue;
            }
            let Some((name, sig)) = rest.split_once(' ') else {
                continue;
            };
            // One parameter: exactly one token to the left of the arrow.
            let Some((params, _)) = sig.split_once("->") else {
                continue;
            };
            let params = params.trim().trim_start_matches('(').trim();
            if params.split_whitespace().count() != 1 || params.contains('(') {
                continue;
            }
            out.push(format!("{module}/{name}"));
        }
    }
    out
}

#[test]
fn every_declared_unary_failure_producer_is_narrowable_through() {
    let allowed = allow_list();
    assert!(
        allowed.len() >= 8,
        "the list parser found almost nothing — it has drifted from the source's shape: {allowed:?}"
    );
    let missing: Vec<_> = declared_unary_failure_producers()
        .into_iter()
        .filter(|n| !allowed.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "these declare a `T | failure` return over one argument but are not in \
         guards::DETERMINISTIC_UNARY, so a guard cannot narrow through them and ADR-316 will \
         report their callers: {missing:?}"
    );
}

#[test]
fn every_allow_list_entry_still_exists() {
    let mut interp = brood::Interp::new();
    let stale: Vec<_> = allow_list()
        .into_iter()
        .filter(|name| {
            // Load the owning module first — these are std modules, resolved on demand.
            if let Some((module, _)) = name.split_once('/') {
                let _ = interp.eval_str(&format!("(require-one '{module})"));
            }
            interp
                .eval_str(&format!("(bound? '{name})"))
                .map(|v| !matches!(v, brood::core::value::Value::Bool(true)))
                .unwrap_or(true)
        })
        .collect();
    assert!(
        stale.is_empty(),
        "guards::DETERMINISTIC_UNARY names functions that no longer exist — a rename left \
         them behind, and a guard silently stopped narrowing through them: {stale:?}"
    );
}
