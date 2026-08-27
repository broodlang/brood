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
//! the advisory checker recognises (`not`/`spawn`/`case`/`system/module-doc`) so they
//! aren't mistaken for unbound symbols. They live here so the checker's
//! recognition lists read uniformly (`kw::*` throughout, no interleaved bare
//! strings) and a spelling still lives in exactly one place.

pub const QUOTE: &str = "quote";
pub const QUASIQUOTE: &str = "quasiquote";
pub const IF: &str = "if";
pub const DO: &str = "do";
pub const DEF: &str = "def";
pub const DEFN: &str = "defn";
// The module-private variants (ADR-146): macros over `def`, but the pre-expansion
// scanners (`scan_def_names`, `def_form_name`, `SCAN_DEF_HEADS`) must recognise them
// as def heads so a forward reference to a private qualifies and its def-site keys.
pub const DEF_PRIVATE: &str = "def-";
pub const DEFN_PRIVATE: &str = "defn-";
pub const DEFMACRO: &str = "defmacro";
pub const DEFDYN: &str = "defdyn";
pub const DEFRECORD: &str = "defrecord";
pub const DEFABILITY: &str = "defability";
pub const IMPL: &str = "impl";
pub const DEFMODULE: &str = "defmodule";
pub const FN: &str = "fn";
// Retired as a spelling (ADR-162) — `fn` is the only one. The constant survives
// because the unbound-symbol hint still recognises `lambda` and names `fn`.
pub const LAMBDA: &str = "lambda";
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
// Table (ETS) primitive names. Shared across five sites — the `def` registration
// (builtins/mod.rs), the IR PrimOp fast-path (compile/ir.rs), the linmap rewrite's
// emitted call heads (compile/inline.rs, eval/macros.rs), and the checker's effect
// list (types/check/guard_effects.rs) — so renaming one is a single edit the compiler
// then enforces at every site. A bare string here once desynced silently (a rename hit
// the registration but not the linmap emitter → `%table-snapshot` unbound at runtime).
pub const TABLE_NEW: &str = "%table";
pub const TABLE_PUT: &str = "%table-put";
pub const TABLE_GET: &str = "%table-get";
pub const TABLE_HAS: &str = "%table-has?";
pub const TABLE_DELETE: &str = "%table-delete";
pub const TABLE_INCR: &str = "%table-incr";
pub const TABLE_COUNT: &str = "%table-count";
pub const TABLE_SNAPSHOT: &str = "%table-snapshot";
pub const TABLE_DROP: &str = "%table-drop";
// The immutable-map ops the linmap rewrite recognizes on the SOURCE side (compile/inline.rs
// + eval/macros.rs) and rewrites into the Table ops above. Shared so a map-op rename stays
// in step with the rewrite that pattern-matches it.
pub const MAP_GET: &str = "%map-get";
pub const MAP_COUNT: &str = "%map-count";
pub const MAP_INT_ADD: &str = "%map-int-add";
pub const MAP_DISSOC: &str = "%map-dissoc";

// The ability / multimethod registry (ADR-241 §3, ADR-240's constant rule). These are
// prelude functions the `defability`/`impl`/`defmulti`/`defrecord` expansions EMIT, and the
// checker recognises them by name to track which impls exist. So the name is shared between
// the prelude that emits it and the Rust that reads it — exactly the split that desyncs
// silently. `%`-prefixed because nobody calls them by hand and the reference hides `%` names.
pub const REGISTER_ABILITY: &str = "%register-ability";
pub const REGISTER_ABILITY_REQUIRES: &str = "%register-ability-requires";
pub const REGISTER_IMPL: &str = "%register-impl";
pub const REGISTER_METHOD: &str = "%register-method";
pub const REGISTER_MULTI: &str = "%register-multi";
pub const REGISTER_SEALED: &str = "%register-sealed";
pub const MULTI_RESOLVE: &str = "%multi-resolve";
pub const DERIVE_INTO: &str = "%derive-into";
pub const IMPL_FOR: &str = "%impl-for";
/// `(%scope)` / `(%locals)` — the debugger locals intrinsic (ADR-174 path B). The VM
/// compiles a call to either into a map of every in-scope local `{name → value}` read
/// straight from the compile-time lexical-scope table; the tree-walker falls back to the
/// same-named builtin (which reads the env-frame chain). `dev-tools` only.
pub const SCOPE_PRIM: &str = "%scope";
pub const LOCALS_PRIM: &str = "%locals";
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
// guard head, like `%eq`); `spawn` is a primitive; `system/module-doc` is the
// module-docstring marker form; `case` is the literal-dispatch macro (it was long
// a deliberately-absent construct routed to a foreign-construct hint — it landed
// 2026-07-26, and the checker already modelled its flat `test result` shape).
pub const NOT: &str = "not";
pub const SPAWN: &str = "spawn";
pub const CASE: &str = "case";
pub const MODULE_DOC: &str = "system/module-doc";
pub const COMMENT: &str = "comment";

// Core macros (defined in std/prelude.blsp, *not* evaluator special forms) that
// nonetheless read as keywords, so they're highlighted as such everywhere from one
// source — they sit in `builtins::SPECIAL_FORMS` (ADR-092); `spawn` (above) joins
// them. Not in the evaluator's `SPECIAL_SPELLINGS` — they're ordinary macros.
pub const SPAWN_LINK: &str = "spawn-link";
pub const ERROR: &str = "error";
pub const WITH_OUT_STR: &str = "with-out-str";
pub const WITH_ERR_STR: &str = "with-err-str";
// (`remote-spawn`/`remote-spawn-sync` moved to `node/spawn`/`node/spawn-sync`, and
//  `bench` to `dev/bench` — a namespaced name is not a keyword, so they left this list.)

// Reader markers inside a quasiquote template — recognised by the reader, the
// quasiquote walker (`eval::macros`), and the checker (`hygiene`/`guards`).
pub const UNQUOTE: &str = "unquote";
pub const UNQUOTE_SPLICING: &str = "unquote-splicing";

// The **pin** marker: `^expr` in a *pattern* means "the current value of `expr`",
// as opposed to a bare symbol, which binds. Written `~expr` until the syntax was
// finalised — but a pin *is* `(unquote expr)`, so inside a macro's `` ` `` template
// the quasiquote walker consumed it first and a pinned pattern could not be
// emitted by a macro at all (the request/reply `receive ([:reply ^tag v] …)` idiom
// is exactly what you want to wrap). `^` (Elixir's spelling) frees `~` for
// quasiquote alone. `%`-prefixed so a user's own `pin` function can't collide with
// the marker the pattern compiler looks for.
pub const PIN: &str = "%pin";

// Parameter-list markers — the `&optional`/`&rest` (and bare `&`) separators a
// `fn`/`defn` param list uses, recognised by the macro lowering, the scope
// walker, introspection, and the checker.
pub const AMP: &str = "&";
pub const AMP_OPTIONAL: &str = "&optional";
pub const AMP_REST: &str = "&rest";
