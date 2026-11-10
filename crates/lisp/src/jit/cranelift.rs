//! The Cranelift backend — today's sole implementation of [`JitBackend`].
//!
//! This file owns the *executable memory*: a process-lifetime [`JITModule`] that every
//! compiled arm's code lives in, plus the symbol table that resolves `brood_rt_*` to the
//! Rust functions in [`super::rt`].
//!
//! **The lowering itself is not here.** It lives in `eval/compile/jit_lower*` — the bulk of
//! the backend by volume — because it reads `eval::compile`'s *private* IR internals
//! (`Node`, `Inst`, `Chunk`, `CompiledArm`) through the `use super::*` convention its sibling
//! compile modules share, and so has to be a child of `compile` rather than of `jit`. So the
//! seam runs like this:
//!
//! | module | role |
//! |---|---|
//! | `jit/backend.rs` | the contract — what any backend must satisfy |
//! | `jit/rt.rs` | the ABI — the callback table, backend-independent |
//! | `jit/cranelift.rs` | *this file* — the Cranelift module + the `JitBackend` impl |
//! | `eval/compile/jit_lower*` | the Cranelift lowering, inside `compile` for IR access |
//!
//! A second backend adds a file beside this one and an `impl JitBackend`, with its own
//! lowering wherever it needs to live. What it does *not* re-derive: `rt.rs` (the ABI) and
//! the decisions about whether and how an arm should lower, which are backend-independent.

use super::backend::JitBackend;
use super::rt::*;
use crate::core::value::Symbol;
use crate::eval::compile::{
    CompiledArm, arm_i64_eligible, arm_i64_too_deep, i64_mark_too_deep, jit_lower_arm,
    jit_lower_arm_hot, jit_lower_inlined_arm,
};

use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::default_libcall_names;

/// The Cranelift JIT backend (ADR-101, Layer 1). Owns a Cranelift [`JITModule`] — the
/// executable memory + symbol table that every compiled arm's code lives in.
///
/// [`CraneliftBackend::new`] stands up the module for the host ISA and registers the
/// [runtime-callback table](super::rt) by name, so emitted code resolves `brood_rt_*` to
/// those Rust functions; [`CraneliftBackend::module`] is the handle the lowering in
/// `eval/compile/jit_lower*` declares and defines functions through.
///
/// One instance per process, behind the `Mutex` in [`super::GLOBAL_JIT`]: the module owns
/// code that must outlive every installed fn-pointer, and compilation mutates it.
pub struct CraneliftBackend {
    module: JITModule,
}

impl CraneliftBackend {
    /// Stand up the Cranelift JIT module for the host ISA, with the runtime-callback
    /// table registered as absolute symbols (so emitted code resolves `brood_rt_*` to
    /// these Rust functions). No code is compiled here.
    #[allow(clippy::new_without_default)] // construction can fail on an unsupported host
    pub fn new() -> Self {
        // `opt_level=speed` turns on Cranelift's GVN + alias-aware redundant-load
        // elimination, which matters here: a hot loop body re-reads the same frame slot
        // several times (`(< i 1)`, `(- i 1)`, `(+ acc i)` all tag-check + load slot `i`),
        // and the default `opt_level=none` keeps every one of those loads + tag-checks.
        // The extra compile cost is paid on the background compiler thread, off the hot
        // path; the optimizations are semantics-preserving, so the GC discipline is
        // unaffected. Falls back to default flags if the host rejects the setting.
        // The fallback closure yields cranelift's own (large) error in its Err arm,
        // which trips `result_large_err`; it's an external type we don't control and
        // the value is discarded (`|_|`), so the size is irrelevant here.
        // **The CLIF verifier is off in release and on under `debug_assertions`**, where it
        // is the miscompile net (KI-64's class). It cost 3–4.5% of a short row's compile
        // thread. On cranelift-codegen 0.133.1 turning it off made `remove_constant_phis`
        // hit an internal assert on `json`'s CLIF, and the tiering layer answered the caught
        // panic by switching the JIT off for the whole process (compute-frontier §7.2) — so
        // after any Cranelift bump, re-check: the fuzz differential against a RELEASE binary
        // (`BROOD=target/release-fast/brood scripts/fuzz/run.sh …` — the armed build keeps the
        // verifier on and cannot see this) and every benchmark row's stderr grepped for
        // CODEGEN-PANICKED. Re-verified clean on 0.136.3 (2026-10-15: eleven generators,
        // 3400 programs, against `release-fast`; the benchmark-row half belongs on the
        // benchmark machine and is still to run there).
        let verifier = if cfg!(debug_assertions) {
            "true"
        } else {
            "false"
        };
        #[allow(clippy::result_large_err)]
        let mut builder = JITBuilder::with_flags(
            &[("opt_level", "speed"), ("enable_verifier", verifier)],
            default_libcall_names(),
        )
        .or_else(|_| JITBuilder::new(default_libcall_names()))
        .expect("Cranelift JITBuilder for the host ISA");
        let callbacks = callback_table();
        for &(name, address) in &callbacks {
            builder.symbol(name, address);
        }
        // Print the callback addresses once, so an offline disassembly of a
        // BROOD_DUMP_CODE'd arm can resolve each `movabs/call` target to a name.
        #[cfg(debug_assertions)]
        if std::env::var_os("BROOD_DUMP_CODE").is_some() {
            for &(name, address) in &callbacks {
                eprintln!("[rt-addr] {name} = {:#x}", address as usize);
            }
        }
        CraneliftBackend {
            module: JITModule::new(builder),
        }
    }

    /// The Cranelift module to declare + define compiled arms through.
    pub fn module(&mut self) -> &mut JITModule {
        &mut self.module
    }
}

/// `(name, address)` for each listed `brood_rt_*` callback, the name being the function's.
macro_rules! callbacks {
    ($($callback:ident),* $(,)?) => {
        vec![$((stringify!($callback), $callback as *const u8)),*]
    };
}

/// Every runtime callback JIT'd code can call, by the symbol name the lowering declares it
/// under. The one list: registration and the `BROOD_DUMP_CODE` address dump both read it,
/// and `every_declared_callback_is_registered_and_used` checks it against the lowering's
/// declarations (`jit_lower/callbacks.rs`) in both directions.
fn callback_table() -> Vec<(&'static str, *const u8)> {
    #[allow(unused_mut)]
    let mut table = callbacks![
        brood_rt_tick_n,
        brood_rt_gc_safepoint,
        brood_rt_cons,
        brood_rt_vec2_room,
        brood_rt_make_vector_n,
        brood_rt_car,
        brood_rt_cdr,
        brood_rt_push_room,
        brood_rt_in_call_journal,
        brood_rt_call_native_fl,
        brood_rt_global,
        brood_rt_global_probe,
        brood_rt_global_ic,
        brood_rt_call_slow,
        brood_rt_note_deopt,
        brood_rt_trace_push,
        brood_rt_fastlink_base,
        brood_rt_fast_frame,
        brood_rt_xcall_latch,
        brood_rt_xcall_cold,
        brood_rt_vector_ref,
        brood_rt_vector_len,
        brood_rt_count,
        brood_rt_table_has,
        brood_rt_table_get2,
        brood_rt_table_put,
        brood_rt_vector_base,
        brood_rt_table_dense_base,
        brood_rt_map_get,
        brood_rt_map_get3,
        brood_rt_vector_ref3,
        brood_rt_make_map_n,
        brood_rt_map_assoc,
        brood_rt_equal,
        brood_rt_first,
        brood_rt_rest,
        brood_rt_is_empty,
        brood_rt_i64_overflow_ptr,
        brood_rt_global_epoch_ptr,
        brood_rt_i64_throw,
        brood_rt_pair_nursery_base,
        brood_rt_stack_recheck,
        brood_rt_pair_old_base,
        brood_rt_vec_nursery_base,
        brood_rt_vec_old_base,
        brood_rt_const_load,
    ];
    #[cfg(debug_assertions)]
    table.extend(callbacks![
        brood_rt_dbg_set_staging,
        brood_rt_dbg_check_slot
    ]);
    table
}

/// The whole seam, in one place: the tiering glue in `eval/compile/jit_runtime.rs` reaches
/// codegen only through the three lowering methods, each a straight delegation to the
/// Cranelift lowering. Nothing decides anything here — the decisions live above
/// (`jit_lower_arm`'s pre-bail + profitability gate today; see `docs/backend-seams.md` §3).
impl JitBackend for CraneliftBackend {
    fn lower_arm(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8> {
        jit_lower_arm(self, arm, slot_tags)
    }

    fn lower_arm_hot(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8> {
        jit_lower_arm_hot(self, arm, slot_tags)
    }

    fn lower_inlined_arm(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8> {
        jit_lower_inlined_arm(self, arm, slot_tags)
    }

    /// No, once this function has been demoted off the unboxed-scalar register worker: the
    /// published pointer is (or may be) that worker's wrapper, and adopting it walks straight
    /// back into the depth bail the demotion exists to escape.
    fn may_adopt_shared_code(arm: &CompiledArm) -> bool {
        !arm_i64_too_deep(arm)
    }

    /// Yes for a scalar-register arm: its small native *is* the register worker, which already
    /// recurses to full depth unboxed, so the boxed depth-2 upgrade would only swap in inferior
    /// code. (`arm_i64_eligible` already accounts for a prior depth bail.)
    fn declines_inline_upgrade(arm: &CompiledArm) -> bool {
        arm_i64_eligible(arm)
    }

    /// Switch this function permanently to the boxed path, which drains deep recursion via
    /// `jit_native_depth`/`jit_force_vm`.
    fn note_depth_bail(name: Symbol) {
        i64_mark_too_deep(name);
    }
}

#[cfg(test)]
mod callback_table_tests {
    /// The lowering declares each callback it calls by name (`jit_lower/callbacks.rs`,
    /// `jit_lower/i64.rs`) and this backend registers them (`callback_table`). A declared
    /// name with no registration fails at link time on the first arm that uses it; a
    /// registered one nothing declares is dead code whose doc still describes a protocol
    /// (six were, among them the abandoned dispatch-speculation guard and a staging push the
    /// "bug #2" comments still named). Every lowering file is read, so a new one is covered.
    #[test]
    fn every_declared_callback_is_registered_and_used() {
        let lowering =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/eval/compile/jit_lower");
        let mut sources = String::new();
        for entry in std::fs::read_dir(&lowering).expect("read jit_lower/") {
            let path = entry.expect("a jit_lower entry").path();
            if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push_str(&std::fs::read_to_string(&path).expect("read a lowering file"));
            }
        }
        // The `brood_rt_dbg_*` pair is declared and registered under `debug_assertions` only,
        // but this scan reads source text, which has no cfg: in a build without them (the
        // nightly ASAN job, `cargo test --release`) the names are in the text and not in
        // the table, and this test failed three nights running before it said so.
        let declared: std::collections::BTreeSet<&str> = sources
            .split('"')
            .filter(|piece| piece.starts_with("brood_rt_"))
            .filter(|piece| cfg!(debug_assertions) || !piece.starts_with("brood_rt_dbg_"))
            .collect();
        let registered: std::collections::BTreeSet<&str> = super::callback_table()
            .iter()
            .map(|&(name, _)| name)
            .collect();
        assert!(
            declared.len() > 30,
            "found only {} declarations",
            declared.len()
        );
        let unregistered: Vec<_> = declared.difference(&registered).collect();
        let undeclared: Vec<_> = registered.difference(&declared).collect();
        assert!(
            unregistered.is_empty() && undeclared.is_empty(),
            "declared but not registered: {unregistered:?}; registered but never declared: \
             {undeclared:?}"
        );
    }
}
