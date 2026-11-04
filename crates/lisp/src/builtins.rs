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
pub(crate) mod args;
mod build_info;
mod bytes;
mod clipboard;
mod compress;
pub(crate) mod contracts;
mod crypto;
mod diagnostics;
mod dynamic;
mod pubkey;
pub(crate) use dynamic::{binding_enter, push_bindings};
pub(crate) use errors::handler_args;
mod errors;
mod evaluation;
mod filesystem;
mod fuzzy;
mod gui_fonts;
mod io;
mod json;
mod line_diff;
mod records;
mod regex_native;
mod rope_regex;
mod text_search;
// `pub(crate)` for `eval::unbound_error`'s KI-120 diagnostic, which asks whether a missing
// qualified name belongs to a baked-in module that `*features*` records as loaded.
mod editor_native;
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
mod tab;
mod table;
mod terminal;
mod tooling;
mod treesit;
#[cfg(feature = "wasm")]
mod wasm;

// The boot cache (`lib.rs`) keys its expanded-prelude file on the build id.
pub use build_info::compiled_features;
pub(crate) use build_info::{build_id_string, stdlib_id_string};

pub(crate) use io::warn_line;
pub use io::{begin_stdout_capture, take_captured_stdout};
pub(crate) use processes::interrupt_ceiling;
// The checker specializes `(string/->number "1")` to the literal `1`, and may only do so by
// deciding parseability exactly as the runtime does — so it calls the runtime's own
// classifier rather than growing a second one that could drift (`types::check::infer`).
pub(crate) use modules::{
    every_provided_feature_is_embedded, is_embedded_module, provided_features,
};
pub(crate) use numeric::{NumericText, classify_numeric_text};
pub use os::set_script_args;
#[cfg(feature = "jit")]
pub(crate) use sequences::{first_without_eval, is_empty_without_eval, rest_without_eval};
// The VM's `Prim1` fallback for a non-pair `first` / `rest` / `empty?` operand calls these
// directly once its guard has proved the head is the native (`exec_chunk`).
pub(crate) use sequences::{first, is_empty, rest};
pub use terminal::{restore_raw, restore_terminal, restore_terminal_on_exit};
pub(crate) use tooling::alias_expansion;
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
            is_apply: name == "apply",
            is_interruptible: name == "proc/interruptible",
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
    pubkey::register(&mut primitives);
    crate::boot::image::register(&mut primitives);
    compress::register(&mut primitives);
    pkg::register(&mut primitives);
    os::register(&mut primitives);
    selfhost_macros::register(&mut primitives);
    errors::register(&mut primitives);
    records::register(&mut primitives);
    dynamic::register(&mut primitives);
    contracts::register(&mut primitives);
    processes::register(&mut primitives);
    offload::register(&mut primitives);
    #[cfg(feature = "wasm")]
    wasm::register(&mut primitives);
    build_info::register(&mut primitives);
    nodes::register(&mut primitives);
    // Last: registration order feeds the intern table (see `syntax_scan::register`).
    editor_native::register(&mut primitives);
    // Appended after it, for the same order reason: `%fuzzy-top` is the newest primitive.
    fuzzy::register(&mut primitives);
    text_search::register(&mut primitives);
    regex_native::register(&mut primitives);
    rope_regex::register(&mut primitives);
    // Appended for the same order reason: the browser-tab display primitives.
    tab::register(&mut primitives);
    // And again: the native half of `json/encode`.
    json::register(&mut primitives);
    // And again: the GUI font natives of ADR-442 (`%gui-font-info`, `%gui-ligatures!`).
    gui_fonts::register(&mut primitives);
    // And again: the native line diff under `diff/line-hunks` (ADR-468).
    line_diff::register(&mut primitives);
    // And again: the structural queries over a held tree (ADR-467).
    treesit::register_tree_queries(&mut primitives);
    // And again: declaring a grammar without loading it (ADR-481).
    treesit::register_grammar_declarations(&mut primitives);
    // And again: colouring ranges of a text as an embedded document (ADR-489).
    treesit::register_injections(&mut primitives);
    // And again: handing a held tree to another process (ADR-495).
    treesit::register_tree_ownership(&mut primitives);
}

#[cfg(test)]
mod who_label_tests {
    //! Every error label a primitive passes as `who` must name something a user can call:
    //! a registered primitive, or a global (bound at boot or in a std module). A label left
    //! behind by a rename points the reader at a function that does not exist.
    //!
    //! The label-taking helpers are discovered, not listed: any `fn` under `src/` with a
    //! `who: &str` parameter, at that parameter's position. Its call sites under
    //! `src/builtins/` whose argument there is a string literal are the labels checked.

    use crate::Interp;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    fn rust_files(directory: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read source dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(path);
            }
        }
    }

    /// Split `text` (the inside of a parenthesised list) at top-level commas, skipping over
    /// nested brackets and string / char literals.
    fn split_top_level(text: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut depth = 0i32;
        let mut chars = text.chars().peekable();
        while let Some(character) = chars.next() {
            match character {
                '"' => {
                    current.push(character);
                    while let Some(inner) = chars.next() {
                        current.push(inner);
                        if inner == '\\' {
                            if let Some(escaped) = chars.next() {
                                current.push(escaped);
                            }
                        } else if inner == '"' {
                            break;
                        }
                    }
                }
                '(' | '[' | '{' | '<' => {
                    depth += 1;
                    current.push(character);
                }
                ')' | ']' | '}' => {
                    depth -= 1;
                    current.push(character);
                }
                // `->` is not a closing angle bracket.
                '>' if !current.ends_with('-') => {
                    depth -= 1;
                    current.push(character);
                }
                ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
                _ => current.push(character),
            }
        }
        if !current.trim().is_empty() {
            parts.push(current);
        }
        parts
            .into_iter()
            .map(|part| part.trim().to_string())
            .collect()
    }

    /// The text between the `(` at `open` and its matching `)`, or `None` if unbalanced.
    fn parenthesised(source: &str, open: usize) -> Option<&str> {
        let bytes = source.as_bytes();
        let mut depth = 0usize;
        let mut index = open;
        while index < bytes.len() {
            match bytes[index] {
                b'"' => {
                    index += 1;
                    while index < bytes.len() && bytes[index] != b'"' {
                        if bytes[index] == b'\\' {
                            index += 1;
                        }
                        index += 1;
                    }
                }
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&source[open + 1..index]);
                    }
                }
                _ => {}
            }
            index += 1;
        }
        None
    }

    fn is_identifier_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'_'
    }

    /// Every free function under `src/` with a `who: &str` parameter, by name, with the
    /// position(s) of that parameter.
    fn label_taking_functions(files: &[(PathBuf, String)]) -> BTreeMap<String, BTreeSet<usize>> {
        let mut functions: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
        for (_, source) in files {
            for (start, _) in source.match_indices("fn ") {
                if start > 0 && is_identifier_byte(source.as_bytes()[start - 1]) {
                    continue;
                }
                let rest = &source[start + 3..];
                let name: String = rest
                    .bytes()
                    .take_while(|&byte| is_identifier_byte(byte))
                    .map(char::from)
                    .collect();
                if name.is_empty() {
                    continue;
                }
                let Some(open) = rest[name.len()..].find('(') else {
                    continue;
                };
                let open = start + 3 + name.len() + open;
                let Some(parameters) = parenthesised(source, open) else {
                    continue;
                };
                let parameters = split_top_level(parameters);
                if parameters
                    .iter()
                    .any(|parameter| parameter.contains("self"))
                {
                    continue;
                }
                if let Some(position) = parameters
                    .iter()
                    .position(|parameter| parameter.replace(' ', "") == "who:&str")
                {
                    functions.entry(name).or_default().insert(position);
                }
            }
        }
        functions
    }

    /// `(file:line, label)` for every literal label passed to a label-taking function.
    fn literal_labels(
        files: &[(PathBuf, String)],
        functions: &BTreeMap<String, BTreeSet<usize>>,
    ) -> Vec<(String, String)> {
        let mut labels = Vec::new();
        for (path, source) in files {
            for (name, positions) in functions {
                let call = format!("{name}(");
                for (start, _) in source.match_indices(&call) {
                    if start > 0 && is_identifier_byte(source.as_bytes()[start - 1]) {
                        continue;
                    }
                    // Skip the definition itself.
                    if source[..start].ends_with("fn ") {
                        continue;
                    }
                    let Some(arguments) = parenthesised(source, start + name.len()) else {
                        continue;
                    };
                    let arguments = split_top_level(arguments);
                    for &position in positions {
                        let Some(argument) = arguments.get(position) else {
                            continue;
                        };
                        if argument.len() >= 2
                            && argument.starts_with('"')
                            && argument.ends_with('"')
                        {
                            let line = source[..start].lines().count();
                            let file = path.file_name().unwrap().to_string_lossy();
                            labels.push((
                                format!("{file}:{line}"),
                                argument[1..argument.len() - 1].to_string(),
                            ));
                        }
                    }
                }
            }
        }
        labels
    }

    /// `(file:line, label)` for every error message written with its label inline:
    /// `LispError::runtime("name: …")` or `LispError::runtime(format!("name: …", …))`, and
    /// the same for `type_err`. The label is the text before the first `": "` when it is
    /// one word — a prose message (`"index out of range: …"`) carries no label.
    fn message_prefix_labels(files: &[(PathBuf, String)]) -> Vec<(String, String)> {
        let mut labels = Vec::new();
        for (path, source) in files {
            for constructor in ["LispError::runtime(", "LispError::type_err("] {
                for (start, _) in source.match_indices(constructor) {
                    let open = start + constructor.len() - 1;
                    let Some(argument) = parenthesised(source, open) else {
                        continue;
                    };
                    let argument = argument.trim_start();
                    let argument = argument
                        .strip_prefix("format!(")
                        .map(str::trim_start)
                        .unwrap_or(argument);
                    let Some(text) = argument.strip_prefix('"') else {
                        continue;
                    };
                    let Some((label, _)) = text.split_once(": ") else {
                        continue;
                    };
                    if label.is_empty() || label.contains([' ', '"', '{', '\\']) {
                        continue;
                    }
                    let line = source[..start].lines().count();
                    let file = path.file_name().unwrap().to_string_lossy();
                    labels.push((format!("{file}:{line}"), label.to_string()));
                }
            }
        }
        labels
    }

    #[test]
    fn builtins_error_message_prefixes_name_callable_functions() {
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let labels = message_prefix_labels(&sources(&source_root.join("builtins")));
        assert!(
            labels.len() > 50,
            "message-prefix extraction found only {} labels — the scanner has broken",
            labels.len()
        );
        let mut interp = Interp::new();
        let stale: Vec<String> = labels
            .iter()
            .filter(|(_, label)| !callable(&mut interp, label))
            .map(|(site, label)| format!("{site}: {label:?}"))
            .collect();
        assert!(
            stale.is_empty(),
            "error messages whose `name:` prefix names no primitive or global — use the name \
             the user calls:\n  {}",
            stale.join("\n  ")
        );
    }

    fn sources(directory: &Path) -> Vec<(PathBuf, String)> {
        let mut paths = Vec::new();
        rust_files(directory, &mut paths);
        paths.sort();
        paths
            .into_iter()
            .map(|path| {
                let text = std::fs::read_to_string(&path).expect("read source");
                (path, text)
            })
            .collect()
    }

    /// Whether `label` names something a user can call. A qualified name is evaluated, which
    /// loads its std module on first reference.
    fn callable(interp: &mut Interp, label: &str) -> bool {
        // A label may name the option it is about after the function: `os/cmd :cwd`.
        let label = label.split(' ').next().unwrap_or_default();
        let not_a_symbol = label.is_empty()
            || label.starts_with([':', '"', '\''])
            || label.starts_with(|character: char| character.is_ascii_digit())
            || label.contains(|character: char| {
                character.is_whitespace() || "()[]{}".contains(character)
            });
        if not_a_symbol {
            return false;
        }
        crate::core::value::intern_existing(label)
            .is_some_and(|symbol| interp.heap.env_get(interp.heap.global(), symbol).is_some())
            || interp.eval_str(label).is_ok()
    }

    #[test]
    fn builtins_error_labels_name_callable_functions() {
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let all_files = sources(&source_root);
        let functions = label_taking_functions(&all_files);
        assert!(
            functions.contains_key("expect_int") && functions.contains_key("expect_string"),
            "label-taking function discovery found nothing: {functions:?}"
        );
        let builtin_files = sources(&source_root.join("builtins"));
        let labels = literal_labels(&builtin_files, &functions);
        assert!(
            labels.len() > 100,
            "label extraction found only {} labels — the scanner has broken",
            labels.len()
        );
        let mut interp = Interp::new();
        let stale: Vec<String> = labels
            .iter()
            .filter(|(_, label)| !callable(&mut interp, label))
            .map(|(site, label)| format!("{site}: {label:?}"))
            .collect();
        assert!(
            stale.is_empty(),
            "error labels that name no primitive or global — use the name the user calls:\n  {}",
            stale.join("\n  ")
        );
    }
}

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
