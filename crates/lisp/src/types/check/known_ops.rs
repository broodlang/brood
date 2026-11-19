//! The named functions the checker knows something about beyond their signatures — one
//! definition per set, read by the rules in `infer.rs`, `guards.rs` and `walk/calls.rs`.
//!
//! A rename that leaves a name behind here does not break a rule loudly: the rule just
//! stops firing, and the checker quietly knows less. So every set is a plain `&[&str]`
//! that `tests/checker_known_names_exist.rs` reads out of this file and resolves in a
//! fresh image.

use crate::core::value::{self, Symbol};

/// Is `head` one of `names`?
pub(super) fn is_one_of(head: Symbol, names: &[&str]) -> bool {
    names.iter().any(|name| value::symbol_is(head, name))
}

/// The extremum operators — the ones that answer with one of their operands rather than
/// a value computed from them.
pub(super) const EXTREMUM: &[&str] = &["math/max", "math/min", "%max", "%min"];

/// The **additive** ring operators: shifting a value by whole numbers cannot change
/// whether it has a denominator, which multiplication can (`(* 2 1/2)` is `1`).
pub(super) const ADDITIVE: &[&str] = &["+", "-", "inc", "dec"];

/// The ring operators, with the binary kernel primitives they are written over: the
/// prelude's `inc` is `(%add n 1)`, so a call re-typed through its body under an int
/// argument reads `%add`, which must not answer the primitive's flat `number`.
pub(super) const RING: &[&str] = &["+", "-", "*", "%add", "%sub", "%mul", "inc", "dec"];

/// Integer-closed beyond the ring (`math/` since ADR-227; the bare integer divisions are
/// gone).
pub(super) const INT_CLOSED_BEYOND_RING: &[&str] =
    &["math/abs", "math/quot", "math/rem", "math/mod"];

/// The integer-closed ops a declared *return* type is checked against
/// (`walk/calls::is_int_closed_op`). Narrower than [`RING`] + [`INT_CLOSED_BEYOND_RING`],
/// and that is the checker's current behaviour, not a decision this file makes: the integer
/// divisions are out on purpose (a `(math/mod x 100)` interval is read by overlap in plain
/// mode and by inclusion only under strict — pinned by
/// `declarations::an_over_approximated_return_mismatch_is_strict_only`). Widening it to
/// `inc`/`dec`/`%add` changes what the checker reports, so it is a change to make with the
/// expectation files, not here.
pub(super) const RETURN_INT_CLOSED: &[&str] = &["+", "-", "*", "math/abs"];

/// The output sinks the function-as-value lint guards: passing a bare zero-arg function
/// to one stringifies the *function* (`#<fn …>`) instead of calling it.
pub(super) const OUTPUT_SINKS: &[&str] = &["io/puts", "io/write", "str", "string/format"];

/// The writers that join their arguments with one space — the doubled-space lint's heads.
/// `str` concatenates and `io/inspect` quotes its strings, so neither belongs here.
pub(super) const SPACE_JOINING_SINKS: &[&str] = &["io/puts", "io/write"];

/// The unary primitives a guard may narrow **through** — a call to one of these is a
/// `PathKey::Call` step, so `(if (failure? (string/->number s)) d (string/->number s))`
/// types the second occurrence as `number`, not `number | failure`.
///
/// The soundness argument is the one `path_types` already rests on: Brood data is
/// immutable, so an argument cannot change between two evaluations, and each of these is a
/// function of its argument alone — the two occurrences denote one value. Every entry here
/// is a *parser*, which is not a coincidence: these are the functions that answer
/// `T | failure` (ADR-310), so they are the ones people guard and re-evaluate.
///
/// **An allow-list, deliberately.** The opposite polarity — "narrow through anything not
/// known to be effectful" — would silently admit `os/env`, `now`, `random` and every
/// primitive added later, each of which can answer differently the second time; that is
/// the direction that makes the bound stop being an upper bound. An unlisted head simply
/// does not narrow, which costs precision and nothing else.
pub(super) const DETERMINISTIC_UNARY: &[&str] = &[
    "string/->number",
    "encoding/hex-decode",
    "encoding/hex-decode-bytes",
    "encoding/base64-decode",
    "encoding/base64-url-decode",
    "encoding/base64-decode-bytes",
    "encoding/base64-url-decode-bytes",
    "datetime/parse-date",
    "datetime/parse-time",
    "datetime/parse-iso8601",
    "url/percent-decode",
    "url/query-decode",
];
