//! Line-coverage recording — the second tier of ADR-148.
//!
//! `nest test --cover` reports FUNCTION coverage and needs no kernel support at all
//! (`std/tool/coverage.blsp` rebinds each project function through a counting shim).
//! This module backs `--cover-lines`: which source LINES actually executed.
//!
//! # Where the hook is, and why not somewhere cheaper
//!
//! The recording point is a bytecode instruction, [`Inst::RecordLine`], emitted at
//! COMPILE time and only when [`enabled`] is true. So an ordinary run's bytecode is
//! byte-for-byte what it always was — the interpreter never even sees the opcode —
//! and there is no per-instruction runtime check to pay for.
//!
//! A cheaper seam was tried first and does not work, which is worth recording so it
//! isn't retried: hook the tree-walking evaluator, where `Heap::form_pos` already
//! carries a form's file AND line. It records top-level forms correctly and *nothing
//! inside a function body*, because a compiled closure's body executes in
//! `exec_chunk` (the sole VM executor since ADR-100 Stage 5), never in `eval`. The
//! bytecode path is where the code actually runs, so that is where coverage has to
//! live.
//!
//! The instruction carries the LINE only. The file comes from the executing arm's
//! `CompiledArm::src_file`, which `exec_chunk` already holds — so nothing has to be
//! threaded through the hot recursive executor.
//!
//! # Recording
//!
//! Hits are `(file, line)` pairs in one process-wide set behind a mutex. Green
//! processes are multiplexed across OS threads, so the set must be shared rather than
//! thread-local: a line executed by any process counts. A lock per recorded line is
//! tolerable only because this mode is explicitly opt-in and explicitly a diagnostic
//! run, never a timing one.
//!
//! # Why there are two sets
//!
//! A percentage needs a denominator drawn from the SAME population as the numerator.
//! Counting "every line holding a form" against "lines that recorded a hit" compares
//! different things and systematically under-reports: a `defmodule` header, a docstring
//! and a `defn`'s own line are all forms, none is an instrumented node, so a fully
//! exercised file reports a fraction of itself. (Measured on a fixture whose every
//! function ran: 14%.)
//!
//! So the denominator is not inferred from source text at all — [`note_instrumented`]
//! records what the compiler actually instrumented, at compile time. Arms compile on
//! first CALL, so that alone is not enough either: the reporting side forces every
//! project function to compile before the suite runs
//! (`eval::compile::precompile` / `%coverage-precompile`), which is what puts a
//! never-called function into the denominator and nowhere else.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

/// `file -> set of executed lines`. Ordered containers so a report comes out stable
/// without sorting at the boundary.
type Hits = BTreeMap<String, BTreeSet<u32>>;

fn hits() -> &'static Mutex<Hits> {
    static HITS: OnceLock<Mutex<Hits>> = OnceLock::new();
    HITS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// `file -> set of lines the compiler instrumented` — the denominator.
fn instrumented_lines() -> &'static Mutex<Hits> {
    static LINES: OnceLock<Mutex<Hits>> = OnceLock::new();
    LINES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Is line recording armed? Read once and cached, so this is a relaxed load at
/// compile time and nothing at all at run time.
///
/// NB the cache means the flag must be set before anything builds an `Interp` — the
/// prelude is compiled during construction, and a chunk compiled without the flag has
/// no `RecordLine` in it. `nest` sets it in `main`, before `Cli` dispatch.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("BROOD_COVERAGE").is_ok_and(|v| v != "0" && !v.is_empty()))
}

/// Record that `line` of `file` executed.
pub fn record(file: &str, line: u32) {
    if let Ok(mut map) = hits().lock() {
        map.entry(file.to_string()).or_default().insert(line);
    }
    // A poisoned lock means another thread panicked mid-insert. Coverage is a
    // diagnostic: losing a hit beats propagating that panic into the measured program.
}

/// Register the lines of `file` that an arm's bytecode was instrumented for — every
/// line that COULD record a hit. Called once per arm compile, from `compile_arm`.
pub fn note_instrumented(file: &str, lines: impl IntoIterator<Item = u32>) {
    if let Ok(mut map) = instrumented_lines().lock() {
        let entry = map.entry(file.to_string()).or_default();
        entry.extend(lines);
    }
}

/// Every instrumented `(file, lines)` pair — the denominator, mirroring [`snapshot`].
pub fn instrumented() -> Vec<(String, Vec<u32>)> {
    match instrumented_lines().lock() {
        Ok(map) => map
            .iter()
            .map(|(f, lines)| (f.clone(), lines.iter().copied().collect()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Every recorded `(file, lines)` pair, for the reporting side.
pub fn snapshot() -> Vec<(String, Vec<u32>)> {
    match hits().lock() {
        Ok(map) => map
            .iter()
            .map(|(f, lines)| (f.clone(), lines.iter().copied().collect()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Drop the recorded HITS, so a long-lived image can measure more than once without
/// runs bleeding together. Deliberately leaves the instrumented set alone: that is a
/// property of the compiled code, not of a measurement, and re-deriving it would need
/// every arm recompiled.
pub fn reset() {
    if let Ok(mut map) = hits().lock() {
        map.clear();
    }
}
