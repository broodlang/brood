//! The canonical spellings of Brood's special forms and core macros — one
//! `const` per keyword, so the spelling lives in exactly one place.
//!
//! Several layers independently recognise these heads: the evaluator's dispatch
//! table (`eval::SPECIAL_SPELLINGS`), the checker's walk (`types::check::walk`),
//! and the introspection list the LSP/highlighter consume
//! (`builtins::SPECIAL_FORMS`). Before this module each re-typed the bare string
//! `"if"`/`"quote"`/`"fn"`/…, so a rename meant hunting magic strings across the
//! kernel. Now they all reference `kw::*` and a typo is a compile error.
//!
//! These are *spellings only* — each consumer still owns its own enum / view
//! (the evaluator's `SpecialForm`, the checker's `SpecialHead`); this module
//! deliberately holds no behaviour. Conventionally imported as
//! `use crate::core::keywords as kw;` so call sites read `kw::IF`.
//!
//! Beyond the special forms and core macros, this module also holds a few
//! *syntax-significant* head spellings that aren't themselves special forms —
//! the `%try`/`%eq` primitives the macro pass emits as a contract, and the heads
//! the advisory checker recognises (`not`/`spawn`/`case`/`module-doc`) so they
//! aren't mistaken for unbound symbols. They live here so the checker's
//! recognition lists read uniformly (`kw::*` throughout, no interleaved bare
//! strings) and a spelling still lives in exactly one place.

pub const QUOTE: &str = "quote";
pub const QUASIQUOTE: &str = "quasiquote";
pub const IF: &str = "if";
pub const DO: &str = "do";
pub const DEF: &str = "def";
pub const DEFN: &str = "defn";
pub const DEFMACRO: &str = "defmacro";
pub const DEFDYN: &str = "defdyn";
pub const DEFMODULE: &str = "defmodule";
pub const FN: &str = "fn";
pub const LET: &str = "let";
pub const LETREC: &str = "letrec";
pub const WHEN: &str = "when";
pub const UNLESS: &str = "unless";
pub const COND: &str = "cond";
pub const AND: &str = "and";
pub const OR: &str = "or";
pub const MATCH: &str = "match";
pub const MATCH_STAR: &str = "match*";
pub const TRY: &str = "try";
pub const CATCH: &str = "catch";
pub const THROW: &str = "throw";
pub const TRY_PRIM: &str = "%try";
pub const EQ_PRIM: &str = "%eq";
pub const ERROR_OF: &str = "error-of";
pub const ASSERT_ERROR: &str = "assert-error";
pub const RECEIVE: &str = "receive";
pub const BINDING: &str = "binding";
pub const DOLIST: &str = "dolist";
pub const DOSEQ: &str = "doseq";
pub const DOTIMES: &str = "dotimes";
pub const FOR: &str = "for";
pub const THREAD_FIRST: &str = "->";
pub const THREAD_LAST: &str = "->>";

// Heads that aren't special forms but are recognised by syntax-aware passes —
// chiefly the advisory checker's `is_syntactic_keyword` list, so they read
// uniformly alongside the special forms above. `not` is a boolean fn (also a
// guard head, like `%eq`); `spawn` is a primitive; `module-doc` is the
// module-docstring marker form; `case` is a construct Brood deliberately *lacks*
// — kept here so the checker routes it to the foreign-construct hint instead of
// flagging it as unbound (the runtime hint lives in `eval/mod.rs`).
pub const NOT: &str = "not";
pub const SPAWN: &str = "spawn";
pub const CASE: &str = "case";
pub const MODULE_DOC: &str = "module-doc";

// Core macros (defined in std/prelude.blsp, *not* evaluator special forms) that
// nonetheless read as keywords, so they're highlighted as such everywhere from one
// source — they sit in `builtins::SPECIAL_FORMS` (ADR-092); `spawn` (above) joins
// them. Not in the evaluator's `SPECIAL_SPELLINGS` — they're ordinary macros.
pub const SPAWN_LINK: &str = "spawn-link";
pub const REMOTE_SPAWN: &str = "remote-spawn";
pub const REMOTE_SPAWN_SYNC: &str = "remote-spawn-sync";
pub const ERROR: &str = "error";
pub const WITH_OUT_STR: &str = "with-out-str";
pub const BENCH: &str = "bench";

// Reader markers inside a quasiquote template — recognised by the reader, the
// quasiquote walker (`eval::macros`), and the checker (`hygiene`/`guards`).
pub const UNQUOTE: &str = "unquote";
pub const UNQUOTE_SPLICING: &str = "unquote-splicing";

// Parameter-list markers — the `&optional`/`&rest` (and bare `&`) separators a
// `fn`/`defn` param list uses, recognised by the macro lowering, the scope
// walker, introspection, and the checker.
pub const AMP: &str = "&";
pub const AMP_OPTIONAL: &str = "&optional";
pub const AMP_REST: &str = "&rest";
