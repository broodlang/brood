//! The signature index of the baked-in standard library (ADR-370): for every function a
//! std module's SOURCE defines, its qualified name, its arity, and — where the declaration
//! is one the checker reads as-is — the declaration's own text.
//!
//! This is a fact about std's source, not about any artifact: the stdlib image's footer
//! CACHES the output of [`std_signature_index`], and a process booted without an image
//! computes the same thing from the sources baked into the binary (`derive::ensure_std_index`).
//! Both paths run this one scanner, so what the checker knows about a std callee cannot
//! depend on whether `~/.cache/brood` happens to hold an image — the artifact matrix
//! (`cli/tests/artifact_matrix.rs`) read a prelude function's inferred signature as
//! `(any) -> nil` with an image and `nil` without one before this was true, because the
//! footer let `infer_sig` type a callee in an unloaded module.
//!
//! It reads source forms, not loaded closures, on purpose: it has to run where nothing but
//! the prelude is loaded. Arity follows the evaluator's parameter grammar
//! (`eval::parse_params`: required, `&optional`, `&`/`&rest`), and the construction gate
//! (`check/tests/image_sigs.rs`) holds every entry against the live closure once its module
//! is loaded.

use std::collections::{HashMap, HashSet};

use super::annot;
use crate::core::heap::Heap;
use crate::core::keywords as kw;
use crate::core::value::{self, Value};
use crate::types::Sig;

/// One indexed function. `max == NO_MAX` means a rest parameter; an empty `text` means the
/// checker reads this function's type from its loaded body (undeclared, private, or a
/// declaration the call-site typing does not take as-is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigEntry {
    pub name: String,
    pub text: String,
    pub min: u32,
    pub max: u32,
}

/// The `max` of an entry with a rest parameter.
pub const NO_MAX: u32 = u32::MAX;

/// Scan every baked-in std module's source for its functions and declarations. Sorted by
/// name and deduplicated (a name defined twice keeps its first definition), so two runs —
/// the image builder's and a source boot's — produce the same bytes.
pub(crate) fn std_signature_index(heap: &mut Heap) -> Vec<SigEntry> {
    let mut out: Vec<SigEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (key, source) in crate::builtins::modules::embedded_modules() {
        for entry in module_signature_index(heap, key, source) {
            if seen.insert(entry.name.clone()) {
                out.push(entry);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The index of ONE baked-in module's `source`, registered under `key` — what a process
/// with no image scans on the first question about that module's names, so a source-boot
/// check pays for the modules it meets and not for all of std (the whole scan is ~376M
/// instructions on a debug binary, the source boot itself ~280M).
pub(crate) fn module_signature_index(heap: &mut Heap, key: &str, source: &str) -> Vec<SigEntry> {
    let Ok(forms) = crate::syntax::reader::read_all(heap, source) else {
        return Vec::new();
    };
    let mut fns: Vec<(String, (u32, u32), bool)> = Vec::new();
    let mut sigs: HashMap<String, Value> = HashMap::new();
    for form in forms {
        collect(heap, key, form, &mut fns, &mut sigs);
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(fns.len());
    for (name, (min, max), private) in fns {
        if !seen.insert(name.clone()) {
            continue;
        }
        let text = if private {
            String::new()
        } else {
            sigs.get(&name)
                .copied()
                .filter(|&form| image_carried_sig(heap, form).is_some())
                .map(|form| crate::syntax::printer::print(heap, form))
                .unwrap_or_default()
        };
        out.push(SigEntry {
            name,
            text,
            min,
            max,
        });
    }
    out
}

/// The signature an image may carry for a declaration `form`, or `None`: the checker must
/// read it AS-IS at a call site — a single arrow (an overload resolves per arm from the
/// heap) whose return is neither `any` nor a bare collection (either is "no declaration" to
/// the call-site typing, which re-types the LOADED body under the call's argument types) —
/// and it must mean the same thing in every process, so every symbol in it is a word of the
/// type grammar itself: a `deftype` alias, a record or an ability name resolves only where
/// its module is loaded, and a form naming one stays a load.
pub(crate) fn image_carried_sig(heap: &Heap, form: Value) -> Option<Sig> {
    if !every_symbol_is_a_type_word(heap, form) {
        return None;
    }
    let sig = annot::without_tables(|| annot::parse_type(heap, form))?
        .as_arrow()
        .cloned()?;
    (!sig.ret.is_any() && !sig.ret.is_unrefined_collection()).then_some(sig)
}

/// Whether every symbol in `form` is a word of the type grammar itself, so that reading it
/// cannot depend on which modules a process has loaded. `&` and `&optional` are parameter-list
/// MARKERS of the arrow grammar (`annot::arrow_of`), not names to resolve — `&optional` was
/// missing here, so 14 declarations spelled entirely in the grammar (`string/pad-left`,
/// `string/fields`, `reflect/type-aliases`, …) declined for a reason the grammar does not have.
fn every_symbol_is_a_type_word(heap: &Heap, form: Value) -> bool {
    let mut work = vec![form];
    while let Some(v) = work.pop() {
        match v {
            Value::Sym(s) => {
                let name = value::symbol_name(s);
                let word = name.starts_with('?')
                    || matches!(name.as_str(), "->" | "&" | "&optional" | "_")
                    || annot::is_type_word(&name);
                if !word {
                    return false;
                }
            }
            Value::Pair(_) => match list_items(heap, v) {
                Some(items) => work.extend(items),
                None => return false,
            },
            Value::Vector(id) => work.extend(heap.vector(id).iter().copied()),
            _ => {}
        }
    }
    true
}

fn list_items(heap: &Heap, v: Value) -> Option<Vec<Value>> {
    match v {
        Value::Nil => Some(Vec::new()),
        Value::Pair(_) => heap.seq_items(v).ok(),
        _ => None,
    }
}

/// Walk one top-level form of module `key`: descend a `defmodule`, `check-allow` or `do`
/// wrapper; record a `defn`/`defn-` (or a `def` of a `fn`) with its arity; record a `sig`.
fn collect(
    heap: &Heap,
    key: &str,
    form: Value,
    fns: &mut Vec<(String, (u32, u32), bool)>,
    sigs: &mut HashMap<String, Value>,
) {
    let Some(items) = list_items(heap, form) else {
        return;
    };
    let Some(Value::Sym(head)) = items.first() else {
        return;
    };
    let head = value::symbol_name(*head);
    let qualify = |name: Value| -> Option<String> {
        let Value::Sym(s) = name else {
            return None;
        };
        let text = value::symbol_name(s);
        Some(if text.contains('/') {
            text
        } else {
            format!("{key}/{text}")
        })
    };
    match head.as_str() {
        "defmodule" | "check-allow" => {
            for &inner in items.iter().skip(2) {
                collect(heap, key, inner, fns, sigs);
            }
        }
        "do" => {
            for &inner in items.iter().skip(1) {
                collect(heap, key, inner, fns, sigs);
            }
        }
        "defn" | "defn-" => {
            let Some(name) = items.get(1).and_then(|&n| qualify(n)) else {
                return;
            };
            if let Some(arity) = fn_arity(heap, &items[2..]) {
                fns.push((name, arity, head == "defn-"));
            }
        }
        "def" => {
            let (Some(name), Some(&rhs)) = (items.get(1).and_then(|&n| qualify(n)), items.get(2))
            else {
                return;
            };
            let Some(fn_items) = list_items(heap, rhs) else {
                return;
            };
            if !matches!(fn_items.first(), Some(Value::Sym(s)) if value::symbol_is(*s, "fn")) {
                return;
            }
            if let Some(arity) = fn_arity(heap, &fn_items[1..]) {
                fns.push((name, arity, false));
            }
        }
        "sig" => {
            if let (Some(name), Some(&form)) =
                (items.get(1).and_then(|&n| qualify(n)), items.get(2))
            {
                sigs.insert(name, form);
            }
        }
        _ => {}
    }
}

/// The arity of a `defn`/`fn` tail — everything after the name: an optional docstring,
/// then either one parameter list or several `((params…) body…)` clauses. Across clauses:
/// the smallest minimum, the largest maximum, unbounded if any has a rest.
fn fn_arity(heap: &Heap, tail: &[Value]) -> Option<(u32, u32)> {
    let tail = match tail.first() {
        Some(Value::Str(_)) => &tail[1..],
        _ => tail,
    };
    let first = *tail.first()?;
    let first_items = list_items(heap, first)?;
    let is_clause = matches!(first_items.first(), Some(Value::Pair(_) | Value::Nil));
    if !is_clause {
        return Some(count_params(heap, &first_items));
    }
    let mut min = u32::MAX;
    let mut max = 0u32;
    let mut any = false;
    for &clause in tail {
        let Some(clause_items) = list_items(heap, clause) else {
            continue;
        };
        let Some(&params) = clause_items.first() else {
            continue;
        };
        let Some(params) = list_items(heap, params) else {
            continue;
        };
        let (lo, hi) = count_params(heap, &params);
        min = min.min(lo);
        max = if hi == NO_MAX || max == NO_MAX {
            NO_MAX
        } else {
            max.max(hi)
        };
        any = true;
    }
    any.then_some((min, max))
}

/// `(a b &optional (c 1) & rest)` → `(2, NO_MAX)`; the evaluator's grammar
/// (`eval::parse_params`), counted rather than bound.
fn count_params(_heap: &Heap, params: &[Value]) -> (u32, u32) {
    let mut required = 0u32;
    let mut optional = 0u32;
    let mut in_optional = false;
    for &p in params {
        if let Value::Sym(s) = p {
            if value::symbol_is(s, kw::AMP_OPTIONAL) {
                in_optional = true;
                continue;
            }
            if value::symbol_is(s, kw::AMP) || value::symbol_is(s, kw::AMP_REST) {
                return (required, NO_MAX);
            }
        }
        if in_optional {
            optional += 1;
        } else {
            required += 1;
        }
    }
    (required, required + optional)
}

#[cfg(test)]
mod triage {
    use super::*;

    /// **A triage tool, not a gate** — run it by name:
    ///
    /// ```text
    /// cargo test -p brood --lib untyped_names_that_force_a_module_load -- --ignored --nocapture
    /// ```
    ///
    /// It prints, for each std module, the names some OTHER std module references and the
    /// index carries no type for. Declare all of one module's names and the transitive scan
    /// (`check::materialise_referenced_modules`) stops loading that module at all — which is
    /// how `path` and `string` left the trace on 2026-09-18.
    ///
    /// `BROOD_IMAGE_TRACE=1` answers the same question one name per run, because a module
    /// once loaded hides every later name in it; this reads the sources and answers it whole.
    /// Read the measurement in `known-issues.md` (KI-150) before spending on the list: on an
    /// IMAGED run the loads it removes are below the instruction-count floor.
    #[test]
    #[ignore = "triage tool: prints a ranked list, asserts nothing"]
    fn untyped_names_that_force_a_module_load() {
        let mut interp = crate::Interp::new();
        let index = std_signature_index(&mut interp.heap);
        let typed: HashSet<&str> = index
            .iter()
            .filter(|e| !e.text.is_empty())
            .map(|e| e.name.as_str())
            .collect();
        let indexed: HashSet<&str> = index.iter().map(|e| e.name.as_str()).collect();
        let mut referenced_by: HashMap<String, HashSet<&'static str>> = HashMap::new();
        for (key, source) in crate::builtins::modules::embedded_modules() {
            let Ok(forms) = crate::syntax::reader::read_all(&mut interp.heap, source) else {
                continue;
            };
            let mut names = Vec::new();
            for form in forms {
                every_symbol(&interp.heap, form, &mut names);
            }
            for name in names {
                let Some((module, _)) = name.rsplit_once('/') else {
                    continue;
                };
                // A self-reference needs no other module; `%`-prefixed names are primitives.
                if module == key || name.starts_with('%') || module.is_empty() {
                    continue;
                }
                referenced_by.entry(name).or_default().insert(key);
            }
        }
        let mut per_module: HashMap<&str, Vec<(usize, &str)>> = HashMap::new();
        for (name, modules) in &referenced_by {
            if !indexed.contains(name.as_str()) || typed.contains(name.as_str()) {
                continue; // a native, a project name, or one the index already types
            }
            let module = name.rsplit_once('/').map(|(m, _)| m).unwrap();
            per_module
                .entry(module)
                .or_default()
                .push((modules.len(), name.as_str()));
        }
        let mut modules: Vec<(&str, Vec<(usize, &str)>)> = per_module.into_iter().collect();
        modules.sort_by_key(|(m, names)| (std::cmp::Reverse(names.len()), *m));
        println!("\n=== names that force a std module to load, by module ===");
        for (module, mut names) in modules {
            names.sort_by_key(|(n, name)| (std::cmp::Reverse(*n), *name));
            let rendered: Vec<String> = names
                .iter()
                .map(|(n, name)| format!("{name}({n})"))
                .collect();
            println!("  {module}: {}  {}", names.len(), rendered.join(" "));
        }
    }

    /// Every symbol in `form`, in no particular order.
    fn every_symbol(heap: &Heap, form: Value, out: &mut Vec<String>) {
        let mut work = vec![form];
        while let Some(v) = work.pop() {
            match v {
                Value::Sym(s) => out.push(value::symbol_name(s)),
                Value::Pair(_) => {
                    if let Some(items) = list_items(heap, v) {
                        work.extend(items)
                    }
                }
                Value::Vector(id) => work.extend(heap.vector(id).iter().copied()),
                _ => {}
            }
        }
    }
}
