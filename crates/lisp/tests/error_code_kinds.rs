//! An error keeps its code, and a code keeps its `:kind` (road-to-1.0 §B7, ADR-673).
//!
//! `docs/error-codes.md` promised only that a code is never reused. A program branches on
//! both fields — `(catch {:code "E0042"} …)`, `(catch {:kind :runtime} …)` — so a code whose
//! kind moved would break it as surely as a reused one. `error_codes::KINDS` declares each
//! code's kind; this file holds it to three things:
//!
//! 1. `SHIPPED` — the rows as released. APPEND ONLY: a row is never edited or removed, so a
//!    code's kind cannot change and a code cannot disappear without this test going red.
//! 2. `docs/error-codes.md`'s table says exactly what `KINDS` says, and every `pub const`
//!    code in `error_codes` has a `KINDS` row.
//! 3. The codes a program can raise on demand raise with their declared kind.

use brood::Interp;
use brood::error::error_codes;

/// Every code as shipped, with its kind. Append a row for a new code; never edit one.
const SHIPPED: &[(&str, &str)] = &[
    ("E0001", "parse"),
    ("E0002", "parse"),
    ("E0010", "unbound"),
    ("E0020", "arity"),
    ("E0030", "type"),
    ("E0040", "runtime"),
    ("E0041", "runtime"),
    ("E0042", "runtime"),
    ("E0043", "runtime"),
    ("E0044", "runtime"),
    ("E0045", "runtime"),
    ("E0046", "runtime"),
    ("E0047", "interrupted"),
    ("E0048", "runtime"),
    ("E0050", "runtime"),
    ("E0051", "runtime"),
    ("E0060", "runtime"),
    ("E0070", "runtime"),
    ("E0080", "contract"),
    ("E0090", "error"),
    ("E0091", "assert"),
    ("E0092", "match"),
    ("E0099", "runtime"),
];

fn declared(code: &str) -> Option<&'static str> {
    error_codes::KINDS
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, kind)| *kind)
}

#[test]
fn a_shipped_code_keeps_its_kind_and_is_never_removed() {
    for (code, kind) in SHIPPED {
        assert_eq!(
            declared(code),
            Some(*kind),
            "{code} shipped as :{kind}; a code's kind never changes and a code is never removed"
        );
    }
    let mut codes: Vec<&str> = error_codes::KINDS.iter().map(|(code, _)| *code).collect();
    codes.sort_unstable();
    let count = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), count, "a code has one row in KINDS");
}

#[test]
fn every_code_constant_has_a_kind_and_the_docs_table_agrees() {
    // Every `pub const NAME: &str = "E…"` in the module, read from its source.
    let source = include_str!("../src/error.rs");
    let module = &source[source.find("pub mod error_codes").expect("the module")..];
    let constants: Vec<&str> = module
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("pub const ")?;
            let value = rest.split("= \"").nth(1)?.strip_suffix("\";")?;
            value.starts_with('E').then_some(value)
        })
        .collect();
    assert!(constants.len() >= SHIPPED.len(), "{constants:?}");
    for code in &constants {
        assert!(
            declared(code).is_some(),
            "{code} has no row in error_codes::KINDS"
        );
    }

    // The docs table: `| `E0001` | `:parse` | …`.
    let docs = include_str!("../../../docs/error-codes.md");
    let mut documented: Vec<(String, String)> = docs
        .lines()
        // a code row (`E0042`), not the numbering scheme's ranges (`E004x`)
        .filter(|line| {
            line.strip_prefix("| `E")
                .is_some_and(|rest| rest.len() > 5 && rest[..4].bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            (
                cells[1].trim_matches('`').to_string(),
                cells[2]
                    .trim_matches('`')
                    .trim_start_matches(':')
                    .to_string(),
            )
        })
        .collect();
    documented.sort();
    let mut declared_rows: Vec<(String, String)> = error_codes::KINDS
        .iter()
        .map(|(code, kind)| (code.to_string(), kind.to_string()))
        .collect();
    declared_rows.sort();
    assert_eq!(
        documented, declared_rows,
        "docs/error-codes.md's table must list exactly the codes and kinds error_codes::KINDS declares"
    );
}

#[test]
fn a_raised_code_carries_its_declared_kind() {
    let triggers = [
        "(no-such-function-anywhere)",
        "((fn (x) x))",
        "(first 5)",
        "(/ 1 0)",
        "(string/substring \"hi\" 0 99)",
        "(error \"boom\")",
        "(assert (> 0 1))",
        "(match 3 (1 :one))",
    ];
    for trigger in triggers {
        let mut interp = Interp::new();
        let caught = interp
            .eval_str(&format!(
                "(try {trigger} (catch e (str (get e :code) \" \" (get e :kind))))"
            ))
            .unwrap_or_else(|error| panic!("{trigger}: {error:?}"));
        let printed = interp.print(caught);
        let printed = printed.trim_matches('"');
        let (code, kind) = printed
            .split_once(' ')
            .unwrap_or_else(|| panic!("{trigger} caught {printed}"));
        assert_eq!(
            Some(kind.trim_start_matches(':')),
            declared(code),
            "{trigger} raised {code} as {kind}"
        );
    }
}
