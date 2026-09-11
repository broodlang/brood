//! Primitive builtins: the irreducible kernel implemented in Rust. Each takes
//! already-evaluated args, the call-site environment, and `&mut Heap`.
//!
//! Anything that can be written in Brood lives in `std/` instead. `%`-prefixed names
//! are low-level primitives not meant to be called directly. The annotated list is in
//! `docs/primitives.md`.
//!
//! One file per domain, and each domain file owns its registrations as well as its
//! implementations: its `register` lists every name, arity, signature, arglist and
//! docstring it contributes, so a primitive is a one-file edit. This file is the
//! [`Primitives`] registrar, the shared `expect!` macro, and the roll-call.

use crate::core::heap::Heap;
use crate::core::value::{self, Arity, EnvId, NativeFn, NativeFnPtr, Value};
use crate::error::{LispError, LispResult};
use crate::eval::apply;
use crate::types::Sig;

/// Require a value of a particular shape, or raise a self-identifying type error
/// attributed to `who` (the primitive that needed it). One macro behind every
/// `expect_*` helper in the domain files — the alternative was six hand-written
/// `match v { Value::X(id) => Ok(id), _ => Err(wrong_type(…, "kind", v)) }` copies
/// that drifted on the error helper used (`expect_node_name` chose `type_err` over
/// `wrong_type` and lost the offending value from its message). Declared before the
/// `mod` lines below because a `macro_rules!` is textually scoped.
macro_rules! expect {
    ($heap:expr, $who:expr, $v:expr, $expected:literal, $($pat:pat => $extract:expr),+ $(,)?) => {
        match $v {
            $($pat => Ok($extract),)+
            __other => Err(LispError::wrong_type($heap, $who, $expected, __other)),
        }
    };
}

// One file per primitive domain. Each owns its implementations AND its registrations —
// a `register(&mut Primitives)` listing every name, arity, signature, arglist and
// docstring it contributes — so a primitive is a one-file edit and `register` below is
// only the roll-call. `pub(crate)` where another layer reads a helper directly.
mod build_info;
mod bytes;
mod clipboard;
mod compress;
mod crypto;
mod diagnostics;
mod dynamic;
mod errors;
mod evaluation;
mod filesystem;
mod io;
// `pub(crate)` for `eval::unbound_error`'s KI-120 diagnostic, which asks whether a missing
// qualified name belongs to a baked-in module that `*features*` records as loaded.
pub(crate) mod modules;
mod nodes;
pub(crate) mod numeric;
mod offload;
mod os;
mod pkg;
mod processes;
mod rope;
mod selfhost_macros;
mod sequences;
pub(crate) mod signature_types;
mod sockets;
mod source;
mod string;
mod subprocesses;
mod syntax_scan;
mod table;
mod terminal;
mod tooling;
mod treesit;
#[cfg(feature = "wasm")]
mod wasm;

// The boot cache (`lib.rs`) keys its expanded-prelude file on the build id.
pub(crate) use build_info::build_id_string;

pub use io::{arm_mcp_progress, begin_stdout_capture, disarm_mcp_progress, take_captured_stdout};
// The checker specializes `(string/->number "1")` to the literal `1`, and may only do so by
// deciding parseability exactly as the runtime does — so it calls the runtime's own
// classifier rather than growing a second one that could drift (`types::check::infer`).
pub(crate) use numeric::{classify_numeric_text, NumericText};
pub use os::set_script_args;
pub use terminal::{restore_raw, restore_terminal, restore_terminal_on_exit};
pub use tooling::{DOC_FORMS, SPECIAL_FORMS};

pub fn realize_seqview(heap: &mut Heap, env: EnvId, sv: Value) -> LispResult {
    let f = heap
        .env_get(heap.global(), value::intern("%seqview-realize"))
        .ok_or_else(|| LispError::runtime("%seqview-realize is not defined".to_string()))?;
    apply(heap, f, &[sv], env)
}

/// The registrar each domain's `register` fills: one `def` per primitive, into `root`.
pub(crate) struct Primitives<'a> {
    heap: &'a mut Heap,
    root: EnvId,
}

impl Primitives<'_> {
    /// Define primitive `name` in the root env: `arity` is what the evaluator checks
    /// before the call, `sig` what the advisory checker reads, `params` and `doc` what
    /// `(doc name)` and the LSP show.
    pub(crate) fn def(
        &mut self,
        name: &str,
        arity: Arity,
        sig: Sig,
        params: &'static [&'static str],
        doc: &'static str,
        func: NativeFnPtr,
    ) {
        let v = self.heap.alloc_native(NativeFn {
            name: name.to_string(),
            arity,
            sig,
            func,
            params,
            doc,
        });
        self.heap.env_define(self.root, value::intern(name), v);
    }
}

/// Register every primitive into `root` — the roll-call of the domain files, each of
/// which lists its own. Order is deliberate only in that it is stable: registration
/// interns each name, and small-map key order is downstream of intern ids, so a
/// reorder here reshuffles symbol-keyed map iteration image-wide.
pub fn register(heap: &mut Heap, root: EnvId) {
    let mut primitives = Primitives { heap, root };
    numeric::register(&mut primitives);
    sequences::register(&mut primitives);
    string::register(&mut primitives);
    rope::register(&mut primitives);
    modules::register(&mut primitives);
    bytes::register(&mut primitives);
    io::register(&mut primitives);
    filesystem::register(&mut primitives);
    sockets::register(&mut primitives);
    table::register(&mut primitives);
    subprocesses::register(&mut primitives);
    terminal::register(&mut primitives);
    evaluation::register(&mut primitives);
    source::register(&mut primitives);
    syntax_scan::register(&mut primitives);
    clipboard::register(&mut primitives);
    treesit::register(&mut primitives);
    tooling::register(&mut primitives);
    diagnostics::register(&mut primitives);
    crypto::register(&mut primitives);
    crate::boot::image::register(&mut primitives);
    compress::register(&mut primitives);
    pkg::register(&mut primitives);
    os::register(&mut primitives);
    selfhost_macros::register(&mut primitives);
    errors::register(&mut primitives);
    dynamic::register(&mut primitives);
    processes::register(&mut primitives);
    offload::register(&mut primitives);
    #[cfg(feature = "wasm")]
    wasm::register(&mut primitives);
    build_info::register(&mut primitives);
    nodes::register(&mut primitives);
}

// (The doc comment and `#[rustfmt::skip]` that used to sit here belonged to the
// `PRIMITIVE_DOCS` table, which the v0.10.0 namespacing removed — a primitive's
// docstring now rides on its `NativeFn` (see `builtins/tooling.rs`). The attribute was
// left behind attached to nothing, which clippy reports as "empty lines after outer
// attribute". The test module below is unrelated and still live.)
#[cfg(test)]
mod primitive_docs_tests {
    use super::*;
    use crate::core::heap::Heap;

    /// Every primitive's registered arity, rendered the way `docs/primitives.md` writes it:
    /// `"2"`, `"1–2"` (en dash, as the table uses), `"1+"`, `"any"`.
    fn registered_arities() -> std::collections::HashMap<String, String> {
        let mut heap = Heap::new();
        let root = heap.new_env(None);
        register(&mut heap, root);
        let mut out = std::collections::HashMap::new();
        for sym in heap.env_chain_names(root) {
            if let Some(Value::Native(id)) = heap.env_get(root, sym) {
                let a = heap.native(id).arity;
                let rendered = match (a.min, a.max) {
                    (0, None) => "any".to_string(),
                    (min, None) => format!("{min}+"),
                    (min, Some(max)) if min == max => format!("{min}"),
                    (min, Some(max)) => format!("{min}\u{2013}{max}"),
                };
                out.insert(value::symbol_name(sym), rendered);
            }
        }
        out
    }

    /// The Arity column of `docs/primitives.md`, by primitive name. The table is prose we
    /// maintain by hand; this reads it back so the test below can compare.
    fn documented_arities() -> Vec<(String, String)> {
        let doc = include_str!("../../../docs/primitives.md");
        let mut out = Vec::new();
        for line in doc.lines() {
            // `| … | `name` | 1–2 | purpose… |` — the primitive is the first backticked
            // cell, the arity the cell after it.
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            for w in cells.windows(2) {
                let (a, b) = (w[0], w[1]);
                if a.starts_with('`') && a.ends_with('`') && a.len() > 2 {
                    let name = a.trim_matches('`');
                    let arity = b.replace('-', "\u{2013}");
                    let looks_like_arity = !arity.is_empty()
                        && arity.chars().next().is_some_and(|c| c.is_ascii_digit())
                        || arity == "any";
                    if looks_like_arity && !name.contains(' ') {
                        out.push((name.to_string(), arity));
                    }
                }
            }
        }
        out
    }

    /// The doc's own claim is that this column is machine-enforced — it says so at the top
    /// of the file. It was not: the *runtime* checks each `Arity`, but nothing compared the
    /// table against it, and 14 rows had drifted (mostly documenting a range's max as if it
    /// were exact, so `%proc-spawn` read as "3" when 2 is legal). A reader trusts this table
    /// precisely because it looks generated.
    #[test]
    fn documented_arities_match_the_registered_ones() {
        let registered = registered_arities();
        let mismatched: Vec<String> = documented_arities()
            .into_iter()
            .filter_map(|(name, doc_arity)| {
                registered.get(&name).and_then(|real| {
                    (*real != doc_arity)
                        .then(|| format!("{name}: doc says {doc_arity}, registered {real}"))
                })
            })
            .collect();
        assert!(
            mismatched.is_empty(),
            "docs/primitives.md arity column has drifted from the registrations:\n  {}",
            mismatched.join("\n  ")
        );
    }
}
