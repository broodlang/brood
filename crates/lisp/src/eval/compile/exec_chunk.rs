//! Bytecode interpreter inner loop (extracted from mod.rs).
use super::*;

/// Tag an error with `pos` if it doesn't already carry one (innermost wins),
/// matching the `Node` interpreter's `or_pos` discipline.
#[inline]
pub(crate) fn tag_pos(e: LispError, pos: Option<Pos>) -> LispError {
    match pos {
        Some(p) => e.or_pos(p),
        None => e,
    }
}

/// Run a [`Chunk`] frame from `*ip`, returning a [`ChunkExit`] to the driver
/// ([`vm_run_bc`]). `*ip` is **resumed and updated in place**, so after a non-tail
/// `Call` returns `ChunkExit::Call`, the driver re-enters here at the instruction
/// after the call once the callee frame completes. The operand stack (`Heap::roots`
/// above `base + nslots`) carries intermediate values; frame slots live at `base..`;
/// `genv` is the captured-env root. On error, returns `Err` without unwinding the
/// operand stack — the driver unwinds every frame's roots back to entry.
///
/// Stage 4: a **non-tail** `Call` to a chunked VM arm returns `ChunkExit::Call` so
/// the driver **pushes a frame** instead of recursing natively; a non-tail call to a
/// native or tree-walked arm is run here (via `dispatch`) and its value pushed. A
/// **tail** `Call`/`SelfCall` returns `Tail`/`SelfTail` so the driver reuses the
/// frame (TCO). A single pass to the next call/return is bounded by the chunk length.
pub(crate) fn exec_chunk(
    heap: &mut Heap,
    arm_arc: &Arc<CompiledArm>,
    ip: &mut usize,
    base: usize,
    genv: EnvRoot,
    capture: bool,
    // Back-edge tiering counter (jit only): persisted across exec_chunk re-entries for
    // the same frame so non-tail Brood calls (which exit and re-enter exec_chunk) don't
    // reset the SelfCall iteration count. Each SelfCall increments this; every 256th
    // iteration triggers a JIT tier check in the outer loop. Owned by vm_run_bc and
    // stored in BcFrame so it survives frame save/restore.
    #[cfg(feature = "jit")] back_edges: &mut u32,
) -> Result<ChunkExit, LispError> {
    // Deref the Arc ONCE — the dispatch loop reads `arm.` fields per instruction
    // (`nslots`/`nrequired` on every SelfCall), and going through the Arc each time
    // cost the most interpreter-bound row (json) ~10%. `arm_arc` itself is only for
    // the spinning-loop sync compile, which needs the Arc for the keepalive.
    let arm: &CompiledArm = arm_arc.as_ref();
    let chunk = arm.chunk.as_ref().expect("exec_chunk: arm has no chunk");
    while *ip < chunk.code.len() {
        let inst = &chunk.code[*ip];
        *ip += 1;
        // Instruction-level trace for debugging (BROOD_VM_TRACE=1).
        // Gate on a process-global OnceLock so we check once, not per-instruction.
        #[cfg(debug_assertions)]
        {
            static VM_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if *VM_TRACE.get_or_init(|| {
                std::env::var("BROOD_VM_TRACE").is_ok_and(|v| v != "0" && !v.is_empty())
            }) {
                eprintln!("[vm-trace ip={}] {}", *ip - 1, inst.trace_name());
            }
        }
        match inst {
            Inst::Const(cv) => {
                let v = cv.load();
                heap.push_root(v);
            }
            Inst::Local(i) => {
                let v = heap.root_at(base + i);
                heap.push_root(v);
            }
            Inst::Global(s) => {
                let env = heap.read_root_env(genv);
                match heap.env_get(env, *s) {
                    Some(v) => heap.push_root(v),
                    None => return Err(crate::eval::unbound_error(heap, *s)),
                }
            }
            Inst::GlobalIc { sym, site } => {
                let env = heap.read_root_env(genv);
                let v = if heap.is_global(env) {
                    let epoch = heap.global_epoch();
                    if let Some(v) = heap.vm_global_ic_probe(*site, *sym, epoch) {
                        crate::perf_bump!(global_ic_hit);
                        v
                    } else {
                        crate::perf_bump!(global_ic_miss);
                        match heap.env_get(env, *sym) {
                            Some(v) => {
                                if !value::is_dynamic(*sym) {
                                    heap.vm_global_ic_put(*site, *sym, epoch, v);
                                }
                                v
                            }
                            None => return Err(crate::eval::unbound_error(heap, *sym)),
                        }
                    }
                } else {
                    match heap.env_get(env, *sym) {
                        Some(v) => v,
                        None => return Err(crate::eval::unbound_error(heap, *sym)),
                    }
                };
                heap.push_root(v);
            }
            // Line coverage (ADR-148 tier 2). Only present when coverage was armed at
            // COMPILE time, so an ordinary chunk never reaches this arm. The line comes
            // from the instruction and the file from the arm being executed, which is
            // why nothing had to be threaded through the executor.
            Inst::RecordLine(line) => {
                if let Some(file) = arm_arc.src_file.as_deref() {
                    crate::coverage::record(file, *line);
                }
            }
            Inst::Pop => {
                let n = heap.roots_len();
                heap.truncate_roots(n - 1);
            }
            Inst::SetLocal(slot) => {
                let n = heap.roots_len();
                let v = heap.root_at(n - 1);
                heap.truncate_roots(n - 1);
                heap.set_root_at(base + slot, v);
            }
            Inst::Jump(t) => *ip = *t,
            Inst::JumpIfFalse(t) => {
                let n = heap.roots_len();
                let c = heap.root_at(n - 1);
                heap.truncate_roots(n - 1);
                if !crate::eval::truthy(c) {
                    *ip = *t;
                }
            }
            Inst::MakeVector(nelem) => {
                // Same discipline as `exec_value`'s `Node::Vector`: read the elements
                // (already on the operand stack), truncate, then build.
                let n = heap.roots_len();
                let start = n - nelem;
                let mut vals = Vec::with_capacity(*nelem);
                for k in 0..*nelem {
                    vals.push(heap.root_at(start + k));
                }
                heap.truncate_roots(start);
                let v = heap.alloc_vector(vals);
                heap.push_root(v);
            }
            Inst::MakeMap(npairs) => {
                let n = heap.roots_len();
                let start = n - 2 * npairs;
                let mut pairs = Vec::with_capacity(*npairs);
                for i in 0..*npairs {
                    pairs.push((heap.root_at(start + 2 * i), heap.root_at(start + 2 * i + 1)));
                }
                heap.truncate_roots(start);
                let v = heap.map_from_pairs(pairs);
                heap.push_root(v);
            }
            Inst::Prim1 {
                op,
                head,
                guard,
                pos,
            } => {
                let pos = *pos;
                let n = heap.roots_len();
                let sa = heap.root_at(n - 1);
                let cur = heap.global_epoch();
                let inlinable = if guard.load(Ordering::Relaxed) == cur {
                    true
                } else {
                    match resolve_prim1(heap, *head) {
                        Some(op2) if op2 == *op => {
                            guard.store(cur, Ordering::Relaxed);
                            true
                        }
                        _ => false,
                    }
                };
                if inlinable {
                    match (op, sa.unpack()) {
                        (PrimOp1::First, ValueRef::Pair(p)) => {
                            crate::perf_bump!(prim1_inline);
                            let v = heap.pair(p).0;
                            heap.truncate_roots(n - 1);
                            heap.push_root(v);
                            continue;
                        }
                        (PrimOp1::Rest, ValueRef::Pair(p)) => {
                            crate::perf_bump!(prim1_inline);
                            let v = heap.pair(p).1;
                            heap.truncate_roots(n - 1);
                            heap.push_root(v);
                            continue;
                        }
                        (PrimOp1::First | PrimOp1::Rest, ValueRef::Nil) => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::nil());
                            continue;
                        }
                        (PrimOp1::IsNil, v) => {
                            crate::perf_bump!(prim1_inline);
                            let result = Value::boolean(matches!(v, ValueRef::Nil));
                            heap.truncate_roots(n - 1);
                            heap.push_root(result);
                            continue;
                        }
                        (PrimOp1::IsPair, v) => {
                            crate::perf_bump!(prim1_inline);
                            let result = Value::boolean(matches!(
                                v,
                                ValueRef::Pair(_) | ValueRef::Range(_) | ValueRef::SeqView(_)
                            ));
                            heap.truncate_roots(n - 1);
                            heap.push_root(result);
                            continue;
                        }
                        (PrimOp1::IsEmpty, ValueRef::Nil) => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::boolean(true));
                            continue;
                        }
                        (PrimOp1::IsEmpty, ValueRef::Pair(_) | ValueRef::Range(_)) => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::boolean(false));
                            continue;
                        }
                        // `sqrt`, x > 0 only: `f64::sqrt` is IEEE correctly-rounded —
                        // identical to the wrapper's `%f64-sqrt`. Zero/negative/NaN/
                        // BigInt fall through to dispatch the real wrapper (its error
                        // and 0.0 cases, bit-identical).
                        (PrimOp1::Sqrt, ValueRef::Float(f)) if f > 0.0 => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::Float(f.sqrt()));
                            continue;
                        }
                        (PrimOp1::Sqrt, ValueRef::Int(i)) if i > 0 => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::Float((i as f64).sqrt()));
                            continue;
                        }
                        // `type-of` is total: tag → cached keyword, every operand shape.
                        (PrimOp1::TypeOf, _) => {
                            crate::perf_bump!(prim1_inline);
                            let result = Value::keyword(crate::core::value::tag(sa).keyword());
                            heap.truncate_roots(n - 1);
                            heap.push_root(result);
                            continue;
                        }
                        _ => {}
                    }
                }
                crate::perf_bump!(prim1_fallback);
                let cur_env = heap.read_root_env(genv);
                let callee = match heap.env_get(cur_env, *head) {
                    Some(c) => c,
                    None => return Err(tag_pos(crate::eval::unbound_error(heap, *head), pos)),
                };
                let sa = heap.root_at(n - 1);
                let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa]);
                let result =
                    dispatch(heap, callee, argv, false, cur_env).and_then(|s| force(heap, s));
                heap.truncate_roots(n - 1);
                match result {
                    Ok(v) => heap.push_root(v),
                    Err(e) => return Err(tag_pos(e, pos)),
                }
            }
            Inst::Prim2 {
                op,
                map,
                head,
                guard,
                pos,
            } => {
                let n = heap.roots_len();
                let sa = heap.root_at(n - 2);
                let sb = heap.root_at(n - 1);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                match prim2_inline_exec(heap, *op, *map, false, *head, guard, x, y)? {
                    Some(v) => {
                        heap.truncate_roots(n - 2);
                        heap.push_root(v);
                    }
                    None => {
                        // Operands already rooted at n-2 and n-1.
                        let v = prim2_dispatch_rooted(heap, *head, n - 2, *pos, genv)?;
                        heap.push_root(v);
                    }
                }
            }
            Inst::Prim3 {
                op,
                head,
                guard,
                pos,
            } => {
                let n = heap.roots_len();
                let sa = heap.root_at(n - 3); // table
                let sb = heap.root_at(n - 2); // key
                let sc = heap.root_at(n - 1); // value
                                              // Epoch guard, same discipline as Prim2: inline only while `head` still
                                              // resolves to the primitive; a `def` bump forces one re-validate.
                let cur = heap.global_epoch();
                let inlinable = guard.load(Ordering::Relaxed) == cur || {
                    match resolve_prim3(heap, *head) {
                        Some(op2) if op2 == *op => {
                            guard.store(cur, Ordering::Relaxed);
                            true
                        }
                        _ => false,
                    }
                };
                let mut done = None;
                if inlinable {
                    if let ValueRef::Table(tid) = sa.unpack() {
                        // Same key guard as the native — a closure/NaN key raises the
                        // identical error; a non-Table first operand defers below.
                        crate::core::table::check_key("table-put", sb)?;
                        crate::perf_bump!(prim2_inline);
                        done = Some(crate::core::table::put(heap, tid, sb, sc)?);
                    }
                }
                match done {
                    Some(v) => {
                        heap.truncate_roots(n - 3);
                        heap.push_root(v);
                    }
                    None => {
                        // Operands already rooted at n-3..n; dispatch the surface head.
                        let v = prim3_dispatch_rooted(heap, *head, n - 3, *pos, genv)?;
                        heap.push_root(v);
                    }
                }
            }
            Inst::Prim2SlotSlot {
                op,
                map,
                slot_a,
                slot_b,
                head,
                guard,
                pos,
            } => {
                let sa = heap.root_at(base + slot_a);
                let sb = heap.root_at(base + slot_b);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                let v = match prim2_inline_exec(heap, *op, *map, false, *head, guard, x, y)? {
                    Some(v) => v,
                    None => {
                        let save = heap.roots_len();
                        heap.push_root(sa);
                        heap.push_root(sb);
                        prim2_dispatch_rooted(heap, *head, save, *pos, genv)?
                    }
                };
                heap.push_root(v);
            }
            Inst::Prim2SlotInt {
                op,
                map,
                slot_a,
                int_b,
                swapped,
                head,
                guard,
                pos,
            } => {
                let sa = heap.root_at(base + slot_a);
                let sb = Value::int(*int_b);
                let x = [sa, sb][map[0] as usize];
                let y = [sa, sb][map[1] as usize];
                let v = match prim2_inline_exec(heap, *op, *map, *swapped, *head, guard, x, y)? {
                    Some(v) => v,
                    None => {
                        // Dispatch to the user `head` in the ORIGINAL call order. For the
                        // `(op Const Local)` fusion (`swapped`) that's `[const, local]` =
                        // `[sb, sa]`; otherwise `[sa, sb]`. (The inline path above used the
                        // map; this slow path must reconstruct the source order — a
                        // mismatch silently mis-ordered non-commutative ops, e.g.
                        // `(/ 24 x)` ran as `(/ x 24)`.)
                        let save = heap.roots_len();
                        let (first, second) = if *swapped { (sb, sa) } else { (sa, sb) };
                        heap.push_root(first);
                        heap.push_root(second);
                        prim2_dispatch_rooted(heap, *head, save, *pos, genv)?
                    }
                };
                heap.push_root(v);
            }
            Inst::Call {
                argc,
                tail,
                pos,
                site,
                head,
            } => {
                let pos = *pos;
                let argc = *argc;
                let n = heap.roots_len();
                let cur_env = heap.read_root_env(genv);
                // The top `argc` operands are always the args. A **free-global** head
                // (`head = Some`) is NOT staged — no preceding `Global` inst pushed it — so
                // the operands are just `[args]` (`drop_base = n - argc`) and the callee is
                // resolved here: the call-site IC gives `(callee, arm)` on a hit with no
                // `env_get`, else `env_get` resolves it and fills the IC. A **computed**
                // head (`head = None`) is staged below the args (`callee` at `n - argc - 1`,
                // `drop_base = n - argc - 1`) and takes no IC. This unifies callee resolution
                // into the call IC — the head no longer has its own `Global`/`env_get`.
                let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(argc);
                for k in 0..argc {
                    argv.push(heap.root_at(n - argc + k));
                }
                let mut fast: Option<(Arc<CompiledArm>, EnvId, (u32, u32))> = None;
                let (callee, drop_base) = if let Some(sym) = head {
                    let drop_base = n - argc;
                    if *site != NO_SITE && heap.is_global(cur_env) {
                        let epoch = heap.global_epoch();
                        if let Some((v, payload)) =
                            heap.vm_call_ic_probe(*site, *sym, argc as u32, epoch)
                        {
                            crate::perf_bump!(call_ic_hit);
                            fast = payload;
                            (v, drop_base)
                        } else {
                            crate::perf_bump!(call_ic_miss);
                            let v = match heap.env_get(cur_env, *sym) {
                                Some(v) => v,
                                None => {
                                    return Err(tag_pos(
                                        crate::eval::unbound_error(heap, *sym),
                                        pos,
                                    ))
                                }
                            };
                            // Cache the resolved callee + (for a non-passthrough VM closure)
                            // its arm. A dynamic var is never cached (it can shadow per call).
                            let arm = match v.unpack() {
                                ValueRef::Fn(id)
                                    if crate::eval::passthrough_arm(heap, id, argc).is_none() =>
                                {
                                    compiled_arm_for(heap, id, argc).map(|arm| {
                                        let cenv =
                                            heap.closure(id).env.unwrap_or_else(|| heap.global());
                                        (arm, cenv)
                                    })
                                }
                                _ => None,
                            };
                            fast = arm
                                .as_ref()
                                .map(|(a, cenv)| (a.clone(), *cenv, heap.vm_arm_block(a)));
                            if !value::is_dynamic(*sym) {
                                heap.vm_call_ic_put(
                                    *site,
                                    crate::core::heap::CallIcEntry {
                                        sym: *sym,
                                        argc: argc as u32,
                                        epoch,
                                        callee: v,
                                        arm,
                                        // Overwritten inside `vm_call_ic_put` (it
                                        // resolves the callee's block itself).
                                        callee_bases: (0, 0),
                                        fast: std::cell::Cell::new(None),
                                    },
                                );
                            }
                            (v, drop_base)
                        }
                    } else {
                        // No IC (a local/dynamic binding shadows the head, or no site):
                        // resolve live each call.
                        let v = match heap.env_get(cur_env, *sym) {
                            Some(v) => v,
                            None => {
                                return Err(tag_pos(crate::eval::unbound_error(heap, *sym), pos))
                            }
                        };
                        (v, drop_base)
                    }
                } else {
                    (heap.root_at(n - argc - 1), n - argc - 1)
                };
                // Inline fast-path: IC hit for the exact same arm, same captured env, no
                // optional/rest params, and GC is not yet due. Covers the common
                // `(defn f (x) … (f …))` self-tail pattern (which uses `Inst::Call` via
                // a global, unlike `letrec` self-recursion which emits `Inst::SelfCall`).
                // This is the main speedup for loop/collatz/fib/reduce.
                //
                // We check the inline condition here — using borrows only — before the
                // `match fast` below consumes `argv` and `fast`. If the check passes we
                // reset the frame and `continue` the inner loop without ever returning to
                // `vm_run_bc`. If it doesn't, we fall through to the normal dispatch path.
                //
                // GC guard: `argv` was read from roots just above, with no allocation in
                // between, so the values are still valid. We skip the inline if GC is due
                // so the outer loop can collect (and can't have stale off-heap SmallVec).
                if *tail {
                    if let Some((ref compiled, cenv, _)) = fast {
                        if std::ptr::eq(compiled.as_ref(), arm)
                            && arm.noptional == 0
                            && arm.rest_slot.is_none()
                            && cur_env == cenv
                            && !heap.gc_due()
                        {
                            crate::perf_bump!(self_tail);
                            heap.truncate_roots(base + arm.nslots);
                            for i in 0..arm.nslots {
                                heap.set_root_at(base + i, Value::nil());
                            }
                            for i in 0..arm.nrequired {
                                heap.set_root_at(base + i, argv[i]);
                            }
                            *ip = 0;
                            if let Some(used) = crate::core::alloc::soft_limit_hit() {
                                return Err(crate::eval::memory_limit_error(used));
                            }
                            if let Some(live) = heap.take_proc_limit_hit() {
                                let limit = heap.proc_mem_limit().unwrap_or(0);
                                return Err(crate::eval::proc_memory_limit_error(live, limit));
                            }
                            if capture {
                                if crate::process::capture_hard_kill_pending() {
                                    return Ok(ChunkExit::Killed);
                                }
                                if crate::process::tick_capture() {
                                    return Ok(ChunkExit::Preempt);
                                }
                            } else {
                                crate::process::tick();
                            }
                            if crate::process::deadline_exceeded() {
                                return Err(crate::eval::deadline_error());
                            }
                            continue;
                        }
                    }
                }
                // IC hit with a VM arm → skip `dispatch` entirely; else resolve with
                // `tail = true` so a VM-closure callee comes back as `Step::Tail` (the
                // resolved arm + args + env, **un-run**) and a native / tree-walked
                // callee comes back executed as `Step::Done(value)`.
                let step = match fast {
                    Some((arm, cenv, bases)) => Step::Tail {
                        compiled: arm,
                        args: argv,
                        genv: cenv,
                        bases,
                    },
                    None => match dispatch(heap, callee, argv, true, cur_env) {
                        Ok(s) => s,
                        Err(e) if e.is_control() => {
                            // A `Control::Kill` (an `(exit …)` interrupted a native-nested
                            // `receive` that then unwound past this call) is re-raised
                            // untouched — don't rewind, don't capture here. `vm_run_bc`'s
                            // error handler converts it to `VmOutcome::Killed` at the
                            // top-level driver (a nested run keeps unwinding).
                            if e.is_kill_signal() {
                                return Err(e);
                            }
                            // State-capture suspend (ADR-100 §8): a clean `receive`
                            // raised `Control::Suspend` through the `%receive` native.
                            // Rewind `ip` to re-run THIS call on resume (re-scan the
                            // mailbox); the callee + args are still on the operand stack
                            // (the `Err` path never truncated them), so the re-run reads
                            // them back. Hand the driver a `Suspend` to capture the
                            // continuation. Default-off builds never produce the signal.
                            *ip -= 1;
                            let deadline = match &e.control {
                                Some(crate::error::Control::Suspend { deadline }) => *deadline,
                                Some(crate::error::Control::Kill) | None => None,
                            };
                            return Ok(ChunkExit::Suspend { deadline });
                        }
                        Err(e) => return Err(tag_pos(e, pos)),
                    },
                };
                if *tail {
                    // Tail position: hand the call to the driver, which reuses this
                    // frame (TCO). Leftover operands are dropped by the driver
                    // (truncate to `base`).
                    return Ok(match step {
                        Step::Tail {
                            compiled,
                            args,
                            genv,
                            bases,
                        } => ChunkExit::Tail {
                            arm: compiled,
                            args,
                            genv,
                            bases,
                        },
                        Step::Done(v) => ChunkExit::Done(v),
                    });
                }
                match step {
                    // Non-tail call to a chunked VM arm: drop the operands (`[args]`, plus a
                    // computed callee) and hand the driver a frame to **push**.
                    Step::Tail {
                        compiled,
                        args,
                        genv,
                        bases,
                    } => {
                        heap.truncate_roots(drop_base);
                        return Ok(ChunkExit::Call {
                            arm: compiled,
                            args,
                            genv,
                            bases,
                        });
                    }
                    // Native / tree-walked callee already ran: push its value and continue.
                    Step::Done(v) => {
                        heap.truncate_roots(drop_base);
                        heap.push_root(v);
                        // GC safepoint: mirror the frequency the BcFrame path gets
                        // from vm_run_bc's outer loop. All live data is on heap.roots
                        // here (frame + result just pushed), so collection is safe.
                        if !crate::process::macro_block_active() && heap.gc_due() {
                            heap.collect(&mut [], &mut []);
                        }
                    }
                }
            }
            Inst::SelfCall { argc } => {
                crate::perf_bump!(self_tail);
                // Direct `letrec` self-tail-call: inline the frame reset and all
                // safepoints so we stay inside this `while` loop instead of
                // round-tripping through `vm_run_bc` on every iteration. Critical for
                // tight tail-recursive loops (loop/collatz/fib): eliminates one Rust
                // call-return and a `SmallVec` construction per iteration.
                //
                // Safety ordering: GC runs first (args still rooted on the operand
                // stack), then args are read (relocated values used), then the frame is
                // reset. No collection fires after the args leave the root stack.
                if !crate::process::macro_block_active() && heap.gc_due() {
                    heap.collect(&mut [], &mut []);
                }
                let n = heap.roots_len();
                let mut argv: SmallVec<[Value; 4]> = SmallVec::with_capacity(*argc);
                for k in 0..*argc {
                    argv.push(heap.root_at(n - argc + k));
                }
                // Reset frame in place (same as the old outer-loop SelfTail handler).
                heap.truncate_roots(base + arm.nslots);
                for i in 0..arm.nslots {
                    heap.set_root_at(base + i, Value::nil());
                }
                for i in 0..arm.nrequired {
                    heap.set_root_at(base + i, argv[i]);
                }
                *ip = 0;
                if let Some(used) = crate::core::alloc::soft_limit_hit() {
                    return Err(crate::eval::memory_limit_error(used));
                }
                if let Some(live) = heap.take_proc_limit_hit() {
                    let limit = heap.proc_mem_limit().unwrap_or(0);
                    return Err(crate::eval::proc_memory_limit_error(live, limit));
                }
                if capture {
                    if crate::process::capture_hard_kill_pending() {
                        return Ok(ChunkExit::Killed);
                    }
                    if crate::process::tick_capture() {
                        // Frame already reset; driver captures the continuation as-is.
                        return Ok(ChunkExit::Preempt);
                    }
                } else {
                    crate::process::tick();
                }
                if crate::process::deadline_exceeded() {
                    return Err(crate::eval::deadline_error());
                }
                // Back-edge tiering: periodically hand a hot self-tail loop to the driver
                // so it can tier. The frame is already reset (ip=0, args in slots), so the
                // driver re-enters this same arm at ip 0. We exit only when there's a
                // reason to: native code is installed (run it — it loops internally), or
                // the arm is still untried (drive `jit_tier`'s counter toward the
                // threshold). While QUEUED (compile in flight) or BAILED we stay inline —
                // no round-trips — just an atomic load every `BACKEDGE_TIER_INTERVAL`.
                #[cfg(feature = "jit")]
                {
                    const BACKEDGE_TIER_INTERVAL: u32 = 256;
                    let edges = back_edges.wrapping_add(1);
                    *back_edges = edges;
                    if edges.is_multiple_of(BACKEDGE_TIER_INTERVAL) {
                        let code = arm.jit_code.load(std::sync::atomic::Ordering::Acquire);
                        let installed = !code.is_null()
                            && code != crate::jit::BAILED
                            && code != crate::jit::QUEUED;
                        // A loop stuck QUEUED also exits (every 8th boundary) so the
                        // driver can compile it synchronously — see the back-edge
                        // check above `jit_tier` in `vm_run_bc`. Kept as a bare
                        // condition: calling into the compiler from inside this
                        // dispatch loop wrecked its codegen (+45% branch misses,
                        // json −10%).
                        if installed
                            || code.is_null()
                            || edges.is_multiple_of(JIT_QUEUED_SYNC_EDGES)
                        {
                            return Ok(ChunkExit::SelfTail);
                        }
                    }
                }
                // Stay in the inner dispatch loop — no function-call round-trip.
                continue;
            }
            Inst::MakeClosure {
                fn_rest,
                names,
                self_name,
            } => {
                // Mirrors `exec_value`'s `Node::MakeClosure`. The capture values are on
                // the operand stack (pushed by preceding leaf insts — safepoint-free,
                // and alloc here never collects mid-pass), so building the env and the
                // closure is collection-free; `env` stays valid until `make_closure`
                // consumes it. With no captures *and* no self-name the closure is
                // global-capturing; a self-name needs a frame to late-bind into.
                let ncap = names.len();
                let n = heap.roots_len();
                let env = if ncap == 0 && self_name.is_none() {
                    heap.global()
                } else {
                    let frame = heap.new_env(Some(heap.global()));
                    for i in 0..ncap {
                        let v = heap.root_at(n - ncap + i);
                        heap.env_define(frame, names[i], v);
                    }
                    frame
                };
                heap.truncate_roots(n - ncap); // drop the capture values
                let closure = crate::eval::make_closure_cached(heap, fn_rest.load(), env)?;
                // Direct `letrec` self-recursion: bind the binder name to the closure
                // in its own captured env (the env↔closure cycle the tracing GC owns).
                if let Some(name) = self_name {
                    heap.env_define(env, *name, closure);
                }
                heap.push_root(closure);
            }
            Inst::TryCatch {
                body,
                bind_slot,
                handler,
            } => {
                // SAFETY: NodePtrs reference nodes owned by arm.body (same CompiledArm),
                // which outlives exec_chunk via the Arc<CompiledArm> held by vm_run_bc.
                let body_node = unsafe { body.0.as_ref() };
                let handler_node = unsafe { handler.0.as_ref() };
                // `exec_value` runs the body through the tree-walker, which can't be
                // captured across the native frame boundary. Gate off `capture_top_level`
                // so a `receive` inside the body or handler blocks the worker (the §7.4
                // dirty-scheduler carve-out) rather than attempting a state-capture that
                // can't cross native frames — the same guard `dispatch` applies when it
                // defers a closure to the tree-walker.
                let _guard = CaptureTopGuard(crate::process::set_capture_top_level(false));
                match exec_value(heap, body_node, base, genv) {
                    Ok(v) => heap.push_root(v),
                    Err(e) if e.is_control() => return Err(e),
                    Err(e) => {
                        let caught = match e.payload {
                            Some(v) => v,
                            None => e.to_value_map(heap),
                        };
                        heap.set_root_at(base + bind_slot, caught);
                        let hv = exec_value(heap, handler_node, base, genv)?;
                        heap.push_root(hv);
                    }
                }
            }
        }
    }
    // The body's value is the lone operand left above the frame.
    let n = heap.roots_len();
    Ok(ChunkExit::Done(heap.root_at(n - 1)))
}

/// The bytecode driver (ADR-100 Stage 4): run a chunked arm and the **entire chain of
/// chunked calls it makes** on one explicit frame stack, with no native recursion per
/// Brood call. A non-tail call to a chunked arm pushes a frame; a tail call reuses the
/// current frame (TCO); a self-tail-call resets it in place; `Done` pops. Calls to
/// natives / tree-walked arms run inline via `dispatch` (leaves w.r.t. this stack).
/// Every frame's slots live on `Heap::roots` and its env on `Heap::env_roots`, so one
/// safepoint at the loop top relocates the whole stack in place; each frame registers
/// its arm in `live_vm_arms` (hot-reload compaction rewrites every in-flight chunk).
///
/// This is what makes a paused process's continuation **relocatable heap data** — the
/// prerequisite for migrating a running process (concurrency-v2.md §7). `resume` drives
/// state capture (§8, the engine that replaced corosensei): `None` starts `arm0` fresh;
/// `Some(s)` replays a previously [`VmOutcome::Suspended`] continuation from the
/// `%receive` call it parked at, re-entering the loop with `s`'s frame stack (and the
/// operand stack it left on the heap) intact — on **any** worker, no coroutine. A clean
/// `receive` suspend returns `Ok(VmOutcome::Suspended(..))` *without unwinding* (the roots
/// must survive for the resume). The driver runs directly on the worker thread; the
/// continuation lives entirely in `s`, no native stack involved.
/// Append this driver's live frames to `e`'s call trace, innermost first — the
/// raise-path walk behind `:trace` on caught error maps. Each entry pairs a
/// frame's function name with the **call site that entered it**: a pending
/// caller's saved `ip` is the return address, so `code[ip - 1]` is the
/// `Inst::Call` that pushed the callee and its recorded `pos` (plus the caller
/// arm's `src_file`) locate the call. Tail calls collapse naturally — a tail
/// callee reuses its caller's frame, so the entry shows the final callee's name
/// at the original call site (the BEAM behaviour). Nested drivers (a native's
/// `vm_apply` callback) each append their own frames as the error crosses them,
/// so the chain accumulates across native boundaries. Only ever runs on the
/// error path; capped at [`crate::error::MAX_TRACE_FRAMES`] keeping the
/// innermost frames (the end that shows a recursion cycle).
pub(crate) fn attach_vm_trace(e: &mut LispError, cur_arm: &CompiledArm, frames: &[BcFrame]) {
    use crate::error::TraceFrame;
    if e.is_control() || e.trace_full() {
        return;
    }
    fn call_site(f: &BcFrame) -> (Option<String>, Option<crate::error::Pos>) {
        let pos = f
            .arm
            .chunk
            .as_ref()
            .and_then(|c| f.ip.checked_sub(1).and_then(|i| c.code.get(i)))
            .and_then(|inst| inst.call_pos());
        (f.arm.src_file.as_deref().map(str::to_string), pos)
    }
    // The running arm's entry, called from the innermost pending caller; then the
    // pending callers themselves — frame k was called from frame k-1, and the
    // driver's outermost frame (k = 0) was entered from a native boundary or the
    // top level, which the next driver out (if any) accounts for.
    let (file, pos) = frames.last().map(call_site).unwrap_or((None, None));
    e.push_trace(TraceFrame {
        name: cur_arm.fn_name.map(value::symbol_name_ref),
        file,
        pos,
    });
    for k in (0..frames.len()).rev() {
        if e.trace_full() {
            break;
        }
        let (file, pos) = match k {
            0 => (None, None),
            _ => call_site(&frames[k - 1]),
        };
        e.push_trace(TraceFrame {
            name: frames[k].arm.fn_name.map(value::symbol_name_ref),
            file,
            pos,
        });
    }
}
