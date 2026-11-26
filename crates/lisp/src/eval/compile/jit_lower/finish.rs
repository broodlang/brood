//! The shared exit blocks every lowered arm ends with, the `BROOD_JIT_DUMP_IR` dump, and
//! the define/finalize step that turns the built function into a code pointer. jit-only.
#![cfg(feature = "jit")]
use super::*;
use cranelift_codegen::Context;
use cranelift_codegen::ir::{Block, FuncRef, InstBuilder, types};
use cranelift_frontend::FunctionBuilder;
use cranelift_jit::JITModule;
use cranelift_module::{FuncId, Module};

/// The arm's exit blocks — its return codes to whoever entered it.
pub(super) struct ExitBlocks {
    /// 0: the result was already stored through the caller's `out` pointer.
    pub(super) done: Block,
    /// 1: a guard failed (the block param is the `DeoptReason` code); the caller runs the
    /// arm on the VM.
    pub(super) deopt: Block,
    /// 2: the reduction budget was spent at a back edge. The frame slots hold the next
    /// iteration's args, so the driver resumes the arm on the VM.
    pub(super) preempt: Block,
    /// 3: a call / global read raised. The error is parked (`JIT_PENDING_ERROR`); the
    /// driver propagates it without re-running the arm (which would repeat the call).
    pub(super) error: Block,
    /// 4: a tail call staged its callee + args on `roots` above the frame; the driver
    /// dispatches it with `tail = true` and reuses this frame.
    pub(super) tailcall: Block,
}

/// Fill the exit blocks. `note_deopt` records the reason; `trace_push` adds this arm to
/// the parked error's `:trace` as the error leaves it (KI-117) — the arm pointer is a
/// constant, because installed code pins its `CompiledArm` for the process lifetime.
pub(super) fn emit_exit_blocks(
    b: &mut FunctionBuilder,
    exits: &ExitBlocks,
    arm: &CompiledArm,
    heap: cranelift_codegen::ir::Value,
    note_deopt: FuncRef,
    trace_push: FuncRef,
) {
    use crate::jit::outcome;
    b.switch_to_block(exits.done);
    let done = b.ins().iconst(types::I64, outcome::DONE);
    b.ins().return_(&[done]);
    b.switch_to_block(exits.deopt);
    let parked_reason = b.block_params(exits.deopt)[0];
    b.ins().call(note_deopt, &[heap, parked_reason]);
    let deopt = b.ins().iconst(types::I64, outcome::DEOPT);
    b.ins().return_(&[deopt]);
    b.switch_to_block(exits.preempt);
    let preempt = b.ins().iconst(types::I64, outcome::PREEMPT);
    b.ins().return_(&[preempt]);
    b.switch_to_block(exits.error);
    let arm_pointer = b.ins().iconst(types::I64, arm as *const CompiledArm as i64);
    b.ins().call(trace_push, &[heap, arm_pointer]);
    let error = b.ins().iconst(types::I64, outcome::ERROR);
    b.ins().return_(&[error]);
    b.switch_to_block(exits.tailcall);
    let tail = b.ins().iconst(types::I64, outcome::TAIL);
    b.ins().return_(&[tail]);
}

/// `BROOD_JIT_DUMP_IR=1`: each fully-lowered arm's bytecode fingerprint (opcode names —
/// `Inst` has no `Debug`) and call sites, then its Cranelift CLIF, to stderr — the tool for
/// reading a JIT miscompile against the intended semantics.
pub(super) fn dump_ir(context: &Context, code: &[Inst], arm: &CompiledArm, checkpoint_slot: u32) {
    if !crate::diagnostics::debug_flags::jit_dump_ir() {
        return;
    }
    let operations: Vec<&str> = code.iter().map(inst_opcode_name).collect();
    eprintln!(
        "[jit-ir] ===== arm: {} ({}) ckpt_slot: {} insts: {} =====",
        code.len(),
        arm.dbg_label(),
        checkpoint_slot,
        operations.join(" ")
    );
    for inst in code.iter() {
        if let Inst::Call {
            site,
            head,
            argc,
            tail,
            ..
        } = inst
        {
            let head_name = match head {
                Some(h) => crate::core::value::symbol_name(*h),
                None => "<computed>".to_string(),
            };
            eprintln!("[jit-ir]   Call site={site} head={head_name} argc={argc} tail={tail}");
        }
    }
    eprintln!("{}", context.func.display());
}

/// Define the built function in the module, finalize it, and return its entry point.
///
/// A define failure is a LOWERING BUG surfacing as a bail (the verifier rejected IR the
/// lowering built), so under the bail trace it prints what Cranelift said.
pub(super) fn define_and_finalize(
    module: &mut JITModule,
    id: FuncId,
    context: &mut Context,
    arm: &CompiledArm,
    code: &[Inst],
    inlined: bool,
) -> Option<*const u8> {
    let defined = module.define_function(id, context);
    if let Err(e) = &defined
        && crate::diagnostics::debug_flags::jit_bail_trace()
    {
        eprintln!("[jit-bail] cranelift-define-function: {e:?}");
    }
    defined.ok().or_bail("cranelift-define-function")?;
    let size = context
        .compiled_code()
        .map_or(0, |compiled| compiled.code_buffer().len());
    #[cfg(debug_assertions)]
    let dump = code_dump_request(context, arm, code);
    #[cfg(not(debug_assertions))]
    let _ = code;
    module.clear_context(context);
    module
        .finalize_definitions()
        .ok()
        .or_bail("cranelift-finalize")?;
    let entry = module.get_finalized_function(id);
    note_perf_map(entry, size, arm, inlined);
    #[cfg(debug_assertions)]
    if let Some((name, length)) = dump {
        // SAFETY: `entry` is a finalized function of `length` bytes in r-x JIT memory.
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(entry, length) };
        eprintln!(
            "[dump-code] arm='{name}' inlined={inlined} entry={:#x} len={length} hex={}",
            entry as usize,
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
    }
    Some(entry)
}

/// `BROOD_PERF_MAP=1`: name the native arm at `entry` (`size` bytes) for `perf`, one
/// `address size name` line appended to `/tmp/perf-<pid>.map` — the file `perf report` reads to
/// name JIT code. Opened per arm (arms install rarely), so no process-wide handle is kept. A
/// failed write is ignored: it is a diagnostic, never a reason to refuse native code.
pub(super) fn note_perf_map(entry: *const u8, size: usize, arm: &CompiledArm, inlined: bool) {
    if size == 0 || !crate::diagnostics::debug_flags::perf_map() {
        return;
    }
    use std::io::Write;
    let name = match (arm.dbg_name.or(arm.fn_name), arm.src_file.as_deref()) {
        (Some(name), _) => crate::core::value::symbol_name(name),
        (None, Some(file)) => format!("<fn in {file}>"),
        (None, None) => "<fn>".to_string(),
    };
    let suffix = if inlined { " (inlined)" } else { "" };
    let path = format!("/tmp/perf-{}.map", std::process::id());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{:x} {:x} brood:{name}{suffix}", entry as usize, size);
    }
}

/// Debug builds: `BROOD_DUMP_CODE=<substring>[,insts:<n>…]` dumps the finalized machine code
/// (hex) of an arm whose name contains the substring (or whose bytecode has `n`
/// instructions, to catch anonymous arms), for offline disassembly — gdb cannot read JIT
/// pages at a crash pc. Captures the name and code length before the context is cleared;
/// the bytes are read from the finalized entry, so call targets are relocated.
#[cfg(debug_assertions)]
fn code_dump_request(
    context: &Context,
    arm: &CompiledArm,
    code: &[Inst],
) -> Option<(String, usize)> {
    let want = std::env::var("BROOD_DUMP_CODE")
        .ok()
        .filter(|w| !w.is_empty())?;
    let name = arm
        .dbg_name
        .map(crate::core::value::symbol_name)
        .unwrap_or_else(|| format!("<anon:{}insts>", code.len()));
    let matched = want.split(',').any(|w| {
        if let Some(n) = w.strip_prefix("insts:") {
            n.parse::<usize>().ok() == Some(code.len())
        } else {
            !w.is_empty() && name.contains(w)
        }
    });
    if !matched {
        return None;
    }
    context
        .compiled_code()
        .map(|compiled| (name, compiled.code_buffer().len()))
}
