//! Bytecode interpreter inner loop (extracted from mod.rs).
use super::*;

/// Back edges between two hand-backs of a hot self-tail loop to the driver's tier check.
#[cfg(feature = "jit")]
pub(crate) const BACKEDGE_TIER_INTERVAL: u32 = 256;

/// Tag an error with `pos` if it doesn't already carry one (innermost wins),
/// matching the `Node` interpreter's `or_pos` discipline.
#[inline]
pub(crate) fn tag_pos(e: LispError, pos: Option<Pos>) -> LispError {
    match pos {
        Some(p) => e.or_pos(p),
        None => e,
    }
}

/// The call at `pos` dispatched `callee` and raised `e`: name the call's local head, if it
/// has one (`eval::name_uncallable_local`), as the tree-walker does from its call form. Asked
/// at the dispatch, where the failing value is known to be this call's callee.
#[cold]
fn name_vm_local_callee(
    heap: &Heap,
    e: LispError,
    arm: &CompiledArm,
    callee: Value,
    pos: Option<Pos>,
) -> LispError {
    let head = match pos {
        Some(p) => arm.local_call_head(p),
        None => Some(crate::eval::POSITIONLESS_CALL),
    };
    crate::eval::name_uncallable_local(heap, e, callee, head)
}

/// The call position of a staged head read at global-read `site`, or `None` for a read that
/// is not one (see `CompiledArm::staged_head_pos`). Cold: an unbound operator's error path.
#[cold]
fn staged_head_pos(arm: &CompiledArm, site: u32) -> Option<Pos> {
    arm.staged_head_pos
        .iter()
        .find(|(s, _)| *s == site)
        .map(|(_, p)| *p)
}

/// What a global read's miss resolved to when [`lazy_load_call`] took it.
enum LazyMiss {
    /// The driver pushes `%lazy-global`'s frame; its value lands where the read's would.
    Call(ChunkExit),
    /// The read's value: `%lazy-global` ran to it here (it was not a VM arm), or the miss
    /// loaded the module itself (no other process held it).
    Value(Value),
}

/// The miss of a frame that can park ([`derive::parkable_miss`]), with `env` rooted across
/// it — a load runs code and can collect. `Elsewhere` carries the module and the re-read env.
fn parkable_miss_rooted(
    heap: &mut Heap,
    sym: Symbol,
    arm: &CompiledArm,
    env: EnvId,
) -> Result<Result<Value, (Symbol, EnvId)>, LispError> {
    let env_base = heap.env_roots_len();
    let env_root = heap.root_env(env);
    let missed = crate::eval::derive::parkable_miss(heap, env, sym, Some(arm));
    let env = heap.read_root_env(env_root);
    heap.truncate_env_roots(env_base);
    Ok(match missed? {
        crate::eval::derive::ParkableMiss::Bound(v) => Ok(v),
        crate::eval::derive::ParkableMiss::Elsewhere(module) => Err((module, env)),
    })
}

/// The module a first-use miss should load or wait for as ordinary frames, with the env to
/// call in — or the read's value when the miss settled it here. A module not yet provided
/// (`module_to_load_off_native`) goes straight to the prelude: its load, and every wait in it
/// (`%require-await`'s `receive`, for the claim or for a dependency another process holds),
/// then runs in frames that park; under the native lookup they blocked their OS workers — 27
/// when 200 processes made their first call into one module together, 139 for 40 loaders of
/// modules whose header used one held elsewhere. Every other miss takes
/// [`derive::parkable_miss`], which still hands off a claim another process wins meanwhile.
fn lazy_target(
    heap: &mut Heap,
    sym: Symbol,
    arm: &CompiledArm,
    env: EnvId,
) -> Result<Result<Value, (Symbol, EnvId)>, LispError> {
    match crate::eval::derive::module_to_load_off_native(heap, sym) {
        Some(module) => {
            crate::eval::derive::claim_race_fault(sym);
            Ok(Err((module, env)))
        }
        None => parkable_miss_rooted(heap, sym, arm, env),
    }
}

/// A first-use miss on a module to load or wait for ([`lazy_target`]): make it an ordinary
/// call to `%lazy-global` instead of running it under this native lookup. Only the top-level
/// capture driver can park, so a nested run (`capture` false) keeps the synchronous path
/// (`None`). A miss settled here is its value (`Value`). The arm is marked stale up front on
/// a hand-off: the load changes what it was compiled against (ADR-366).
#[cold]
fn lazy_load_call(
    heap: &mut Heap,
    sym: Symbol,
    arm: &CompiledArm,
    capture: bool,
    env: EnvId,
) -> Result<Option<LazyMiss>, LispError> {
    if !capture {
        return Ok(None);
    }
    let (module, env) = match lazy_target(heap, sym, arm, env)? {
        Ok(v) => return Ok(Some(LazyMiss::Value(v))),
        Err(target) => target,
    };
    let Some(callee) = heap.env_get(heap.global(), value::intern("%lazy-global")) else {
        return Ok(None);
    };
    crate::eval::derive::mark_arm_stale_for_miss(heap, arm, sym);
    let argv: SmallVec<[Value; 4]> =
        SmallVec::from_slice(&[Value::symbol(module), Value::symbol(sym)]);
    Ok(Some(match dispatch(heap, callee, argv, true, env)? {
        Step::Tail {
            compiled,
            args,
            genv,
            bases,
        } => LazyMiss::Call(ChunkExit::Call {
            arm: compiled,
            args,
            genv,
            bases,
        }),
        Step::Done(v) => LazyMiss::Value(v),
    }))
}

/// What a call head's miss resolved to when [`lazy_call`] took it.
enum LazyCall {
    /// Leave `exec_chunk` with this: `%lazy-call`'s frame (a `Call`, or a `Tail` for a call
    /// in tail position), or the finished value of a tail call (`Done`).
    Exit(ChunkExit),
    /// `%lazy-call` ran to a value here for a non-tail call: push it and carry on.
    Value(Value),
    /// No other process held the module: the miss loaded it here, and this is the head's
    /// value, for the call to proceed with as usual.
    Head(Value),
}

/// [`lazy_load_call`] for a call HEAD whose lookup missed: the arguments already sit on the
/// operand stack above `drop_base`, so the whole call goes to `%lazy-call`, which loads the
/// module and applies the function — a tail call the VM unfolds, so the callee runs as an
/// ordinary frame too. A non-tail call drops its arguments here, as `Inst::Call`'s own
/// `Call` exit does; a tail call leaves them for the driver to drop.
#[cold]
fn lazy_call(
    heap: &mut Heap,
    sym: Symbol,
    arm: &CompiledArm,
    capture: bool,
    env: EnvId,
    drop_base: usize,
    tail: bool,
) -> Result<Option<LazyCall>, LispError> {
    if !capture {
        return Ok(None);
    }
    let (module, env) = match lazy_target(heap, sym, arm, env)? {
        Ok(v) => return Ok(Some(LazyCall::Head(v))),
        Err(target) => target,
    };
    let Some(callee) = heap.env_get(heap.global(), value::intern("%lazy-call")) else {
        return Ok(None);
    };
    crate::eval::derive::mark_arm_stale_for_miss(heap, arm, sym);
    let mut argv: SmallVec<[Value; 4]> = SmallVec::new();
    argv.push(Value::symbol(module));
    argv.push(Value::symbol(sym));
    for k in drop_base..heap.roots_len() {
        argv.push(heap.root_at(k));
    }
    let step = dispatch(heap, callee, argv, true, env)?;
    Ok(Some(match (step, tail) {
        (
            Step::Tail {
                compiled,
                args,
                genv,
                bases,
            },
            true,
        ) => LazyCall::Exit(ChunkExit::Tail {
            arm: compiled,
            args,
            genv,
            bases,
        }),
        (
            Step::Tail {
                compiled,
                args,
                genv,
                bases,
            },
            false,
        ) => {
            heap.truncate_roots(drop_base);
            LazyCall::Exit(ChunkExit::Call {
                arm: compiled,
                args,
                genv,
                bases,
            })
        }
        (Step::Done(v), true) => LazyCall::Exit(ChunkExit::Done(v)),
        (Step::Done(v), false) => {
            heap.truncate_roots(drop_base);
            LazyCall::Value(v)
        }
    }))
}

/// A call to `proc/interruptible` the driver can run as an interrupt REGION (ADR-491): its
/// ceiling and the thunk's arm, captured env and IC block — when `callee` is that builtin,
/// with two arguments, and the thunk is a closure with a VM arm for no arguments. Anything
/// else (`None`) is an ordinary native call, which runs the thunk nested and reports a bad
/// argument itself. A bad ceiling is raised here, as the builtin would.
///
/// `(apply proc/interruptible args)` is the same call (ADR-494): `dispatch` would unfold it
/// and run the builtin nested, so it is unfolded here first. Only when the head IS that
/// builtin — any other `apply` costs one flag read — and its list is not a lazy seq-view
/// (realising one runs Brood; `dispatch` does that, and the thunk runs nested as before).
fn interruptible_region(
    heap: &mut Heap,
    callee: Value,
    argv: &[Value],
) -> Result<Option<(Option<u64>, Arc<ArmHandle>, EnvId, (u32, u32))>, LispError> {
    let ValueRef::Native(id) = callee.unpack() else {
        return Ok(None);
    };
    let spread: SmallVec<[Value; 4]>;
    let argv = if heap.native(id).is_apply && argv.len() >= 2 {
        let ValueRef::Native(head) = argv[0].unpack() else {
            return Ok(None);
        };
        let list = argv[argv.len() - 1];
        if !heap.native(head).is_interruptible || matches!(list.unpack(), ValueRef::SeqView(_)) {
            return Ok(None);
        }
        // Not a sequence: `dispatch` raises the error `apply` would.
        let Ok(items) = heap.seq_items(list) else {
            return Ok(None);
        };
        spread = argv[1..argv.len() - 1]
            .iter()
            .copied()
            .chain(items)
            .collect();
        &spread[..]
    } else if heap.native(id).is_interruptible {
        argv
    } else {
        return Ok(None);
    };
    if argv.len() != 2 {
        return Ok(None);
    }
    let ValueRef::Fn(thunk) = argv[1].unpack() else {
        return Ok(None);
    };
    let Some(arm) = compiled_arm_for(heap, thunk, 0) else {
        return Ok(None);
    };
    let ceiling = crate::builtins::interrupt_ceiling(heap, argv[0])?;
    let genv = heap.closure(thunk).env.unwrap_or_else(|| heap.global());
    let bases = heap.vm_arm_block(&arm);
    Ok(Some((ceiling, arm, genv, bases)))
}

/// The module the contract shim templates live in (ADR-385) — the one module whose code
/// positions the error-position rule in [`attach_vm_trace`] never reports (KI-184).
pub(crate) const CONTRACT_MODULE: &str = "contract";

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
    arm_arc: &Arc<ArmHandle>,
    ip: &mut usize,
    base: usize,
    genv: EnvRoot,
    capture: bool,
    // The global epoch this FRAME's `SelfCall` hot-reload guard last validated at, owned by
    // `vm_run_bc` and stored in `BcFrame`. It must outlive this call: the frame re-enters
    // here after every non-tail call in its body, and a snapshot re-read on entry hid any
    // `def` made before that call — `(do (def-self …) (loop (list 1)))` looped on the old
    // body forever because `(list 1)` is a VM call (KI-190).
    entry_epoch: &mut u64,
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
    let arm: &CompiledArm = arm_arc;
    let chunk = arm.chunk.as_ref().expect("exec_chunk: arm has no chunk");
    // The hot-reload guard for `Inst::SelfCall` (see there) compares against `entry_epoch`.
    // A `def` bumps the epoch, so an unchanged value means no rebinding can have happened
    // since the frame last validated, and the loop takes its zero-lookup fast path.
    //
    // A stale arm (ADR-366) takes the sentinel instead, so its FIRST `SelfCall` takes the
    // slow path and hands the loop to the recompiled body: the load that staled it happened
    // inside this frame (`(apply mod/f nil)` misses, loads, is then CALLED), and it may have
    // run before this frame last validated. The test is against the heap's hot per-process
    // hint (the uid the marking wrote), not the arm's flag, which sits on a cold line:
    // reading that here cost `pipeline` 4% at the VM ceiling.
    if heap.stale_arm_hint() == arm.uid {
        *entry_epoch = u64::MAX;
    }
    while *ip < chunk.code.len() {
        let inst = &chunk.code[*ip];
        *ip += 1;
        // Instruction-level trace for debugging (BROOD_VM_TRACE=1).
        // Gate on a process-global OnceLock so we check once, not per-instruction.
        #[cfg(debug_assertions)]
        {
            static VM_TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            if crate::diagnostics::debug_flags::is_armed(&VM_TRACE, "BROOD_VM_TRACE") {
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
                // A miss may LOAD the name's module (ADR-335) — arbitrary eval, a collection;
                // the operand stack is on `roots`, so only `env` (re-read inside) is at stake.
                let v = match heap.env_get(env, *s) {
                    Some(v) => v,
                    None => match lazy_load_call(heap, *s, arm, capture, env)? {
                        Some(LazyMiss::Call(exit)) => return Ok(exit),
                        Some(LazyMiss::Value(v)) => v,
                        None => crate::eval::derive::global_miss_in_arm(heap, env, *s, arm)?,
                    },
                };
                heap.push_root(v);
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
                                if !heap.is_dynamic(*sym) {
                                    heap.vm_global_ic_put(*site, *sym, epoch, v);
                                }
                                v
                            }
                            // Autoload (ADR-335); not cached — the load bumped the epoch.
                            None => match lazy_load_call(heap, *sym, arm, capture, env)
                                .map_err(|e| tag_pos(e, staged_head_pos(arm, *site)))?
                            {
                                Some(LazyMiss::Call(exit)) => return Ok(exit),
                                Some(LazyMiss::Value(v)) => v,
                                None => {
                                    crate::eval::derive::global_miss_in_arm(heap, env, *sym, arm)
                                        .map_err(|e| tag_pos(e, staged_head_pos(arm, *site)))?
                                }
                            },
                        }
                    }
                } else {
                    match heap.env_get(env, *sym) {
                        Some(v) => v,
                        None => match lazy_load_call(heap, *sym, arm, capture, env)
                            .map_err(|e| tag_pos(e, staged_head_pos(arm, *site)))?
                        {
                            Some(LazyMiss::Call(exit)) => return Ok(exit),
                            Some(LazyMiss::Value(v)) => v,
                            None => crate::eval::derive::global_miss_in_arm(heap, env, *sym, arm)
                                .map_err(|e| tag_pos(e, staged_head_pos(arm, *site)))?,
                        },
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
                    crate::diagnostics::coverage::record(file, *line);
                }
            }
            // Branch coverage (ADR-148 tier 2) — as RecordLine, present only in a
            // coverage-armed chunk. Records which edge of a branch was taken.
            Inst::RecordBranch(line, col, taken) => {
                if let Some(file) = arm_arc.src_file.as_deref() {
                    crate::diagnostics::coverage::record_branch(file, *line, *col, *taken);
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
                // Build straight from the elements on the operand stack (inline for up
                // to two, no temporary `Vec` either way), then drop them. Allocation
                // never collects, so they stay valid while they are read.
                let n = heap.roots_len();
                let start = n - nelem;
                let v = heap.alloc_vector_from_roots(start, *nelem);
                heap.truncate_roots(start);
                heap.push_root(v);
            }
            Inst::MakeMap(npairs) => {
                let n = heap.roots_len();
                let start = n - 2 * npairs;
                // The pairs are copied off the operand stack (into an inline buffer for a
                // small literal — no `malloc`) because the build mutates the heap that
                // holds the stack.
                let pairs: SmallVec<[(Value, Value); 8]> = (0..*npairs)
                    .map(|i| (heap.root_at(start + 2 * i), heap.root_at(start + 2 * i + 1)))
                    .collect();
                heap.truncate_roots(start);
                let v = heap.map_from_pair_iter(pairs);
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
                        // A type predicate is `(%eq (type-of x) :kw)` — total, like `type-of`.
                        (PrimOp1::TypeIs(kw), _) => {
                            let kw = *kw;
                            crate::perf_bump!(prim1_inline);
                            let result =
                                Value::boolean(crate::core::value::tag(sa).keyword() == kw);
                            heap.truncate_roots(n - 1);
                            heap.push_root(result);
                            continue;
                        }
                        // Only the driver holds the frame stack: hand it the question,
                        // `ret` still on top of the operands (KI-216).
                        (PrimOp1::ContractTail, _) => return Ok(ChunkExit::ContractProbe),
                        (PrimOp1::VectorLen, ValueRef::Vector(id)) => {
                            crate::perf_bump!(prim1_inline);
                            let result = Value::Int(heap.vector(id).len() as i64);
                            heap.truncate_roots(n - 1);
                            heap.push_root(result);
                            continue;
                        }
                        // `count` of a size the heap holds; every other shape dispatches
                        // the real `count` below.
                        (PrimOp1::Count, _) if let Some(len) = heap.count_inline(sa) => {
                            crate::perf_bump!(prim1_inline);
                            heap.truncate_roots(n - 1);
                            heap.push_root(Value::Int(len));
                            continue;
                        }
                        // `first` / `rest` / `empty?` on anything else (a vector, a set, a
                        // map, a record, a seq-view, a type error): the guard proved the head
                        // is that native, so call its body directly — what `env_get` +
                        // `dispatch` (passthrough loop, `apply` test, `eval::apply`,
                        // `call_native`'s arity check on a one-argument call) would reach.
                        // The JIT's `unary_fallback` calls the same functions. The operand
                        // stays rooted at `n - 1` across a collection inside it. A record
                        // (ADR-542) dispatches below instead, where the builtin can hand its
                        // `->seq` back to be called here (ADR-505).
                        (PrimOp1::First | PrimOp1::Rest | PrimOp1::IsEmpty, _)
                            if !matches!(sa.unpack(), ValueRef::Record { .. }) =>
                        {
                            crate::perf_bump!(prim1_fallback);
                            let cur_env = heap.read_root_env(genv);
                            let builtin = match op {
                                PrimOp1::First => crate::builtins::first,
                                PrimOp1::Rest => crate::builtins::rest,
                                _ => crate::builtins::is_empty,
                            };
                            let result = builtin(&[sa], cur_env, heap);
                            heap.truncate_roots(n - 1);
                            match result {
                                Ok(v) => heap.push_root(v),
                                Err(e) => return Err(tag_pos(e, pos)),
                            }
                            continue;
                        }
                        _ => {}
                    }
                }
                crate::perf_bump!(prim1_fallback);
                let cur_env = heap.read_root_env(genv);
                let callee = match heap.env_get(cur_env, *head) {
                    Some(c) => c,
                    // Autoload (ADR-335): a load collects; the operand is on `roots` and
                    // `cur_env` is re-read below.
                    None => crate::eval::derive::global_miss_in_arm(heap, cur_env, *head, arm)
                        .map_err(|e| tag_pos(e, pos))?,
                };
                let cur_env = heap.read_root_env(genv);
                let sa = heap.root_at(n - 1);
                let argv: SmallVec<[Value; 4]> = SmallVec::from_slice(&[sa]);
                // A record operand (ADR-542) is where Brood runs under the call: its `->seq`
                // or `->count`, called from the builtin or from the prelude's `count`.
                // Dispatched as a call, a closure comes back un-run and gets a frame of its
                // own, so a `receive` inside parks (ADR-505). Every other operand keeps the
                // nested run.
                if matches!(sa.unpack(), ValueRef::Record { .. }) {
                    let step = dispatch(heap, callee, argv, true, cur_env);
                    heap.truncate_roots(n - 1);
                    match step.map_err(|e| tag_pos(e, pos))? {
                        Step::Done(v) => heap.push_root(v),
                        Step::Tail {
                            compiled,
                            args,
                            genv,
                            bases,
                        } => {
                            return Ok(ChunkExit::Call {
                                arm: compiled,
                                args,
                                genv,
                                bases,
                            });
                        }
                    }
                    continue;
                }
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
                match prim2_inline_exec(heap, *op, *map, false, *head, guard, x, y)
                    .map_err(|e| tag_pos(e, *pos))?
                {
                    Some(v) => {
                        heap.truncate_roots(n - 2);
                        heap.push_root(v);
                    }
                    // A record operand: the call is made as one (ADR-505).
                    None if prim2_may_redirect(sa, sb) => {
                        match prim2_dispatch_step(heap, *head, n - 2, *pos, genv)? {
                            Step::Done(v) => heap.push_root(v),
                            Step::Tail {
                                compiled,
                                args,
                                genv,
                                bases,
                            } => {
                                return Ok(ChunkExit::Call {
                                    arm: compiled,
                                    args,
                                    genv,
                                    bases,
                                });
                            }
                        }
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
                    match op {
                        PrimOp3::TablePut => {
                            if let ValueRef::Table(tid) = sa.unpack() {
                                // Same key guard as the native — a closure/NaN key raises
                                // the identical error; a non-Table first operand defers
                                // below.
                                crate::core::table::check_key("table/put", sb)
                                    .map_err(|e| tag_pos(e, *pos))?;
                                crate::perf_bump!(prim2_inline);
                                done = Some(
                                    crate::core::table::put(heap, tid, sb, sc)
                                        .map_err(|e| tag_pos(e, *pos))?,
                                );
                            }
                        }
                        // `(get m k default)` on a map (ADR-368): `Heap::map_get3_inline`
                        // is the rule; a record's miss and every non-map receiver
                        // defer to the real `get` below.
                        PrimOp3::MapGet3 => {
                            if let Some(v) = heap.map_get3_inline(sa, sb, sc) {
                                crate::perf_bump!(prim2_inline);
                                done = Some(v);
                            }
                        }
                        // `(nth v i default)` on a vector: the element, or the default out
                        // of range; every other shape defers to the real `nth` below.
                        PrimOp3::VectorRefOr => {
                            if let Some(v) = heap.nth3_inline(sa, sb, sc) {
                                crate::perf_bump!(prim2_inline);
                                done = Some(v);
                            }
                        }
                        // `(assoc m k v)` on a map (ADR-368): the kernel path-copy directly;
                        // a vector (or anything else) defers to the wrapper's own branches.
                        // The operands stay rooted at n-3..n across the allocation.
                        PrimOp3::MapAssoc => {
                            if let Some(v) = heap.map_assoc_inline(sa, sb, sc) {
                                crate::perf_bump!(prim2_inline);
                                done = Some(v);
                            }
                        }
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
                let v = match prim2_inline_exec(heap, *op, *map, false, *head, guard, x, y)
                    .map_err(|e| tag_pos(e, *pos))?
                {
                    Some(v) => v,
                    None => {
                        let save = heap.roots_len();
                        heap.push_root(sa);
                        heap.push_root(sb);
                        // A record operand: the call is made as one (ADR-505).
                        if prim2_may_redirect(sa, sb) {
                            match prim2_dispatch_step(heap, *head, save, *pos, genv)? {
                                Step::Done(v) => v,
                                Step::Tail {
                                    compiled,
                                    args,
                                    genv,
                                    bases,
                                } => {
                                    return Ok(ChunkExit::Call {
                                        arm: compiled,
                                        args,
                                        genv,
                                        bases,
                                    });
                                }
                            }
                        } else {
                            prim2_dispatch_rooted(heap, *head, save, *pos, genv)?
                        }
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
                let v = match prim2_inline_exec(heap, *op, *map, *swapped, *head, guard, x, y)
                    .map_err(|e| tag_pos(e, *pos))?
                {
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
                        // A record operand: the call is made as one (ADR-505).
                        if prim2_may_redirect(sa, sb) {
                            match prim2_dispatch_step(heap, *head, save, *pos, genv)? {
                                Step::Done(v) => v,
                                Step::Tail {
                                    compiled,
                                    args,
                                    genv,
                                    bases,
                                } => {
                                    return Ok(ChunkExit::Call {
                                        arm: compiled,
                                        args,
                                        genv,
                                        bases,
                                    });
                                }
                            }
                        } else {
                            prim2_dispatch_rooted(heap, *head, save, *pos, genv)?
                        }
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
                staged,
            } => {
                let pos = *pos;
                let argc = *argc;
                let staged = *staged;
                let n = heap.roots_len();
                let mut cur_env = heap.read_root_env(genv);
                // The top `argc` operands are always the args. A **free-global** head
                // (`head = Some`) is NOT staged — no preceding `Global` inst pushed it — so
                // the operands are just `[args]` (`drop_base = n - argc`) and the callee is
                // resolved here: the call-site IC gives `(callee, arm)` on a hit with no
                // `env_get`, else `env_get` resolves it and fills the IC. A **computed**
                // head (`head = None`) is staged below the args (`callee` at `n - argc - 1`,
                // `drop_base = n - argc - 1`) and takes no IC. This unifies callee resolution
                // into the call IC — the head no longer has its own `Global`/`env_get`.
                // The args, copied off the operand stack — built BELOW, after the in-place
                // call path (which never needs them) has declined.
                let mut argv: SmallVec<[Value; 4]>;
                let mut fast: Option<(Arc<ArmHandle>, EnvId, (u32, u32))> = None;
                // Set by either IC path when `vm_call_ic_self` proved this tail call targets
                // THIS arm in THIS env — `fast` then stays `None` (no clone). Asked only when
                // the inline self-tail reset below can take the call (no optional or rest
                // parameters): a hit it could not take would leave `fast` empty and send the
                // call through `dispatch` instead of straight to the arm.
                let mut self_tail = false;
                let self_tail_probe = *tail && arm.noptional == 0 && arm.rest_slot.is_none();
                // KI-19: a *staged* head was resolved before the args (it sits below them),
                // so the callee is that value — not a fresh resolution, which would observe
                // a `def` an argument performed. The call-site IC is still consulted for the
                // resolved **arm**, validated by identity against the staged callee: on the
                // overwhelmingly common path nothing rebound, the IC's cached callee is the
                // same value, and this costs one `Value` compare over the elided head.
                let (callee, drop_base) = if staged {
                    let drop_base = n - argc - 1;
                    let v = heap.root_at(drop_base);
                    if let Some(sym) = head
                        && *site != NO_SITE
                        && heap.is_global(cur_env)
                    {
                        let epoch = heap.global_epoch();
                        // A self-tail call first asks the non-cloning question the unstaged
                        // path asks (`vm_call_ic_self`): is the IC's arm THIS arm in THIS
                        // env? Its cached callee must still be the staged value itself —
                        // the same identity test as below — and then the inline self-tail
                        // reset takes the call with `fast` left `None`: no `Arc` increment
                        // and decrement per loop iteration for a handle it would drop.
                        let self_hit = if self_tail_probe {
                            heap.vm_call_ic_self(*site, *sym, argc as u32, epoch, arm, cur_env)
                        } else {
                            None
                        };
                        if let Some(cached) = self_hit
                            && matches!(
                                (cached.unpack(), v.unpack()),
                                (ValueRef::Fn(a), ValueRef::Fn(b)) if a == b
                            )
                        {
                            crate::perf_bump!(call_ic_hit);
                            self_tail = true;
                        } else if let Some((cached, payload)) =
                            heap.vm_call_ic_probe(*site, *sym, argc as u32, epoch)
                        {
                            // Identity, not equality: the IC's cached arm is only valid
                            // for the very closure it was resolved from. A non-closure
                            // callee is never arm-cached anyway, so it simply misses.
                            let same = matches!(
                                (cached.unpack(), v.unpack()),
                                (ValueRef::Fn(a), ValueRef::Fn(b)) if a == b
                            );
                            if same {
                                crate::perf_bump!(call_ic_hit);
                                fast = payload;
                            }
                        }
                    }
                    if fast.is_none() && !self_tail {
                        crate::perf_bump!(call_ic_miss);
                        fast = match v.unpack() {
                            ValueRef::Fn(id)
                                if crate::eval::passthrough_arm(heap, id, argc).is_none() =>
                            {
                                compiled_arm_for(heap, id, argc).map(|arm| {
                                    let cenv =
                                        heap.closure(id).env.unwrap_or_else(|| heap.global());
                                    let block = heap.vm_arm_block(&arm);
                                    (arm, cenv, block)
                                })
                            }
                            _ => None,
                        };
                    }
                    (v, drop_base)
                } else if let Some(sym) = head {
                    let drop_base = n - argc;
                    if *site != NO_SITE && heap.is_global(cur_env) {
                        let epoch = heap.global_epoch();
                        let self_hit = if self_tail_probe {
                            heap.vm_call_ic_self(*site, *sym, argc as u32, epoch, arm, cur_env)
                        } else {
                            None
                        };
                        if let Some(v) = self_hit {
                            // Self-tail IC hit, answered without cloning the arm handle —
                            // the inline self-tail reset below takes it.
                            crate::perf_bump!(call_ic_hit);
                            self_tail = true;
                            (v, drop_base)
                        } else if let Some((v, payload)) =
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
                                    // Another process is loading the module: the whole
                                    // call goes to `%lazy-call`, where the wait parks.
                                    match lazy_call(
                                        heap, *sym, arm, capture, cur_env, drop_base, *tail,
                                    )
                                    .map_err(|e| tag_pos(e, pos))?
                                    {
                                        Some(LazyCall::Exit(exit)) => return Ok(exit),
                                        Some(LazyCall::Value(v)) => {
                                            heap.push_root(v);
                                            continue;
                                        }
                                        // Loaded here: the load collects, so re-read the env
                                        // (the args sit on `roots` above `drop_base`).
                                        Some(LazyCall::Head(v)) => {
                                            cur_env = heap.read_root_env(genv);
                                            v
                                        }
                                        None => {
                                            // Autoload (ADR-335). The load collects: re-read
                                            // the env and the args, which sit on `roots`
                                            // above `drop_base`.
                                            let v = crate::eval::derive::global_miss_in_arm(
                                                heap, cur_env, *sym, arm,
                                            )
                                            .map_err(|e| tag_pos(e, pos))?;
                                            cur_env = heap.read_root_env(genv);
                                            v
                                        }
                                    }
                                }
                            };
                            // Cache the resolved callee + (for a non-passthrough VM closure)
                            // its arm.
                            let arm = call_site_arm(heap, v, argc);
                            fast = arm
                                .as_ref()
                                .map(|(a, cenv)| (a.clone(), *cenv, heap.vm_arm_block(a)));
                            heap.vm_call_ic_install(*site, *sym, argc as u32, epoch, v, arm);
                            (v, drop_base)
                        }
                    } else {
                        // No IC (a local/dynamic binding shadows the head, or no site):
                        // resolve live each call.
                        let v = match heap.env_get(cur_env, *sym) {
                            Some(v) => v,
                            None => {
                                match lazy_call(heap, *sym, arm, capture, cur_env, drop_base, *tail)
                                    .map_err(|e| tag_pos(e, pos))?
                                {
                                    Some(LazyCall::Exit(exit)) => return Ok(exit),
                                    Some(LazyCall::Value(v)) => {
                                        heap.push_root(v);
                                        continue;
                                    }
                                    Some(LazyCall::Head(v)) => {
                                        cur_env = heap.read_root_env(genv);
                                        v
                                    }
                                    None => {
                                        // Autoload (ADR-335); see the IC-miss arm above.
                                        let v = crate::eval::derive::global_miss_in_arm(
                                            heap, cur_env, *sym, arm,
                                        )
                                        .map_err(|e| tag_pos(e, pos))?;
                                        cur_env = heap.read_root_env(genv);
                                        v
                                    }
                                }
                            }
                        };
                        (v, drop_base)
                    }
                } else {
                    (heap.root_at(n - argc - 1), n - argc - 1)
                };
                // In-place non-tail call (`ChunkExit::CallInPlace`): a VM callee whose frame
                // is exactly its arguments takes them where they lie. Not for a callee with
                // native code installed — the VM→native direct call below owns that — and a
                // staged head's callee value (below the args) is dropped by shifting the args
                // down one slot, so the frame starts at `drop_base` either way.
                //
                // Whether the callee takes that direct call is asked ONCE, here: the in-place
                // path (which must decline it) and the direct call below read one answer.
                #[cfg(feature = "jit")]
                let direct_native = !*tail
                    && fast
                        .as_ref()
                        .is_some_and(|(farm, _, _)| vm_direct_eligible(heap, farm, argc));
                #[cfg(not(feature = "jit"))]
                let direct_native = false;
                if !*tail
                    && !direct_native
                    && let Some((ref farm, _, _)) = fast
                    && farm.nrequired == argc
                    && farm.noptional == 0
                    && farm.rest_slot.is_none()
                {
                    if drop_base + argc < n {
                        for k in 0..argc {
                            let v = heap.root_at(drop_base + 1 + k);
                            heap.set_root_at(drop_base + k, v);
                        }
                        heap.truncate_roots(drop_base + argc);
                    }
                    // Move the IC's handle out rather than clone it (no refcount).
                    let (arm, genv, bases) = fast.take().expect("matched Some above");
                    return Ok(ChunkExit::CallInPlace {
                        arm,
                        argc,
                        genv,
                        bases,
                    });
                }
                // VM→native DIRECT call (`docs/compute-frontier.md` §7.12). On the
                // call-heavy rows the interpreter's dominant "call" is not a VM-level apply
                // but an entry into a callee that is ALREADY native — a gate-refused arm's
                // loop calling a native helper non-tail (`json` 193k of these against 42k
                // `vm_apply`s per run; 650k on `supervisor`). Until now every one exited
                // `exec_chunk` to the driver (frame save, `push_frame`, the loop-top
                // safepoints, the tier check, re-entry) only to run code that is native.
                // Here the call site lays the callee's frame out itself and enters the
                // SAME `jit_tier_in_frame` the driver would, so the semantics are the frame
                // path's by construction; only the round trip is gone. A value comes back
                // here; a staged tail call is dispatched below exactly as `jit_dispatch_tail`
                // would (at frame level — so a `receive` down that chain suspends cleanly);
                // a deopt, preempt or declined activation is handed to the driver in its
                // in-place frame (`ChunkExit::CallResume`). Nothing runs nested: the first
                // cut of this used the native→native fast link, whose outcome-4 tail chain
                // is nested, and put 19 820 dirty worker parks on the `supervisor` row.
                //
                // Gated on installed native code, so the interpreted case pays one relaxed
                // load. Both head shapes qualify: a staged head's callee value sits at
                // `drop_base`, and the frame is laid out FROM `drop_base` either way, which
                // is where the result must land when the callee returns.
                #[cfg(feature = "jit")]
                if direct_native {
                    // Every outcome below returns or continues, so the IC's handle is moved
                    // out rather than cloned (two atomic refcount writes a call).
                    let (arm, cenv, bases) = fast.take().expect("matched Some above");
                    {
                        // The args already lie where the frame starts, but for a staged
                        // head's callee value below them: shift them down over it, in
                        // place (forward-safe — each write is below its read).
                        let n = heap.roots_len();
                        if drop_base + argc < n {
                            for k in 0..argc {
                                let v = heap.root_at(drop_base + 1 + k);
                                heap.set_root_at(drop_base + k, v);
                            }
                            heap.truncate_roots(drop_base + argc);
                        }
                        let (r, frame_nslots) = vm_call_native_direct(
                            heap, &arm, argc, drop_base, cenv, bases, capture,
                        );
                        match r {
                            DirectNative::Done(v) => {
                                // `roots` is back at `drop_base`; the result replaces
                                // the call. `v` is unrooted until pushed — nothing may
                                // allocate before this (see `crate::jit::JitArmFn`).
                                heap.push_root(v);
                                if !crate::process::macro_block_active() && heap.gc_due() {
                                    heap.collect(&mut [], &mut []);
                                }
                                continue;
                            }
                            DirectNative::Error => {
                                let e = jit_take_error(heap)
                                    .expect("native callee reported an error without parking one");
                                // A hard kill unwinds untouched to the top-level driver
                                // (as the `dispatch` arm below does). The native arm
                                // recorded its own trace frame (KI-117); the driver's
                                // `Err` arm names this caller and the pending frames.
                                if e.is_kill_signal() {
                                    return Err(e);
                                }
                                return Err(tag_pos(e, pos));
                            }
                            DirectNative::Tail { top } => {
                                // `[callee, arg0..]` staged above the callee's frame:
                                // `jit_dispatch_tail`'s dispatch, done here. A VM
                                // closure comes back UN-RUN as `Step::Tail` and becomes
                                // this call's frame push (the frame the native callee
                                // would have reused — same depth, same base); a native
                                // or tree-walked callee ran to `Done`, as it would there.
                                let local = heap.take_jit_tail_local_head();
                                let n2 = heap.roots_len();
                                let tail_callee = heap.root_at(top);
                                let mut tail_argv: SmallVec<[Value; 4]> =
                                    SmallVec::with_capacity(n2 - top - 1);
                                for k in (top + 1)..n2 {
                                    tail_argv.push(heap.root_at(k));
                                }
                                let step =
                                    match dispatch(heap, tail_callee, tail_argv, true, cur_env) {
                                        Ok(s) => s,
                                        Err(e) if e.is_control() => {
                                            // Only a kill can arrive here: the tail callee
                                            // of a native arm is never `%receive` (the
                                            // receive fence keeps that chunk off the JIT),
                                            // so there is no suspend to rewind for.
                                            debug_assert!(e.is_kill_signal());
                                            return Err(e);
                                        }
                                        Err(e) => {
                                            let e = crate::eval::name_uncallable_local(
                                                heap,
                                                e,
                                                tail_callee,
                                                local,
                                            );
                                            return Err(tag_pos(e, pos));
                                        }
                                    };
                                heap.truncate_roots(drop_base);
                                match step {
                                    Step::Tail {
                                        compiled,
                                        args,
                                        genv,
                                        bases,
                                    } => {
                                        return Ok(ChunkExit::Call {
                                            arm: compiled,
                                            args,
                                            genv,
                                            bases,
                                        });
                                    }
                                    Step::Done(v) => {
                                        heap.push_root(v);
                                        if !crate::process::macro_block_active() && heap.gc_due() {
                                            heap.collect(&mut [], &mut []);
                                        }
                                        continue;
                                    }
                                }
                            }
                            DirectNative::Unwound => {
                                // The parked signal has done its job: the frames it
                                // passed are recorded (ADR-401).
                                let _ = jit_take_error(heap);
                                return Ok(ChunkExit::Unwound { keep_current: true });
                            }
                            DirectNative::Handoff { outcome } => {
                                return Ok(ChunkExit::CallResume {
                                    arm,
                                    base: drop_base,
                                    nslots: frame_nslots,
                                    genv: cenv,
                                    bases,
                                    outcome,
                                });
                            }
                        }
                    }
                }
                // Inline fast-path: IC hit for the exact same arm, same captured env, no
                // optional/rest params, and GC is not yet due. Covers the common
                // `(defn f (x) … (f …))` self-tail pattern (which uses `Inst::Call` via
                // a global, unlike `letrec` self-recursion which emits `Inst::SelfCall`).
                // This is the main speedup for loop/collatz/fib/reduce.
                //
                // We check the inline condition here — using borrows only — before the
                // `match fast` below consumes `fast`. If the check passes we reset the frame
                // straight from the operand stack and `continue` the inner loop without
                // ever returning to `vm_run_bc`. If it doesn't, we fall through to the
                // normal dispatch path, which copies the args off the stack below.
                //
                // GC guard: we skip the inline if GC is due so the outer loop can collect;
                // nothing between the operand reads and the frame writes allocates.
                if *tail {
                    let is_self = self_tail
                        || matches!(fast, Some((ref compiled, cenv, _))
                            if std::ptr::eq(Arc::as_ptr(compiled.arc()), arm) && cur_env == cenv);
                    {
                        if is_self
                            && arm.noptional == 0
                            && arm.rest_slot.is_none()
                            && !heap.gc_due()
                        {
                            crate::perf_bump!(self_tail);
                            reset_frame_from_operands(heap, arm, base, argc);
                            *ip = 0;
                            if let Some(exit) = loop_safepoint(heap, capture)? {
                                return Ok(exit);
                            }
                            continue;
                        }
                    }
                }
                let n = heap.roots_len();
                argv = SmallVec::with_capacity(argc);
                for k in 0..argc {
                    argv.push(heap.root_at(n - argc + k));
                }
                // `proc/interruptible` over a VM thunk opens a region in this driver rather
                // than running the thunk nested under the builtin (ADR-491).
                if fast.is_none()
                    && let Some((ceiling, arm, genv, bases)) =
                        interruptible_region(heap, callee, &argv).map_err(|e| tag_pos(e, pos))?
                {
                    heap.truncate_roots(drop_base);
                    return Ok(ChunkExit::Interruptible {
                        ceiling,
                        arm,
                        genv,
                        bases,
                        tail: *tail,
                    });
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
                            // An arm that receives at its head (ADR-410) parks at ip 0
                            // instead, its operands dropped: everything before the `%receive`
                            // is a load, so the re-run is the same. A resume at ip 0 is an
                            // ENTRY — it counts toward the tier threshold and re-tiers once
                            // the arm is native — so a loop that parks on every message (a
                            // lightly loaded server) still gets hot. Parked mid-arm, its only
                            // count would be one back edge per wake, and it never would.
                            if arm.receive_at_head {
                                *ip = 0;
                                heap.truncate_roots(base + arm.nslots);
                            }
                            let deadline = match &e.control {
                                Some(crate::error::Control::Suspend { deadline }) => *deadline,
                                Some(crate::error::Control::Kill) | None => None,
                                // Only a call whose callee was entered unwindable can see
                                // one, and a call through `dispatch` never enters one so.
                                Some(crate::error::Control::PreemptUnwind) => unreachable!(
                                    "a native preempt-unwind reached a nested call (ADR-401)"
                                ),
                                // `dispatch` takes every redirect it offered (ADR-505).
                                Some(crate::error::Control::Redirect) => {
                                    unreachable!("a builtin's redirect escaped `dispatch`")
                                }
                            };
                            return Ok(ChunkExit::Suspend { deadline });
                        }
                        Err(e) => {
                            let e = name_vm_local_callee(heap, e, arm, callee, pos);
                            return Err(tag_pos(e, pos));
                        }
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
                // stack), then the frame is reset straight from the operand stack
                // (relocated values used). No collection fires after the args leave it.
                if !crate::process::macro_block_active() && heap.gc_due() {
                    heap.collect(&mut [], &mut []);
                }
                // Hot-reload guard (ADR-013). `SelfCall` is the zero-lookup loop back-edge:
                // it resets the frame and jumps to ip 0 without consulting the global at
                // all, so without this guard a `def` of the very function a loop is running
                // would never be observed — a long-lived `(defn serve (state) … (serve next))`
                // could never be reloaded. (The sibling `Inst::Call` tail path is covered by
                // its epoch-guarded IC.)
                //
                // Cost on the hot path is one relaxed u64 load and a compare per iteration —
                // the epoch only moves on a `def`. On a bump we re-resolve once and, if the
                // name still names this arm, re-arm and keep looping.
                let ep = heap.global_epoch();
                if ep != *entry_epoch {
                    // A fresh frame (`SELF_UNCHECKED_EPOCH`) checks once too: it may have
                    // been entered through an alias after `dbg_name` was rebound. The memo
                    // answers without a lookup when this arm was proven at this epoch.
                    let fresh = *entry_epoch == super::SELF_UNCHECKED_EPOCH;
                    *entry_epoch = ep;
                    let stamp = super::self_binding_stamp(heap, ep);
                    let proven = fresh
                        && arm
                            .self_bound_stamp
                            .load(std::sync::atomic::Ordering::Relaxed)
                            == stamp;
                    if let (false, Some(name)) = (proven, arm.dbg_name) {
                        // The name as this module's own code reads it: a contracted
                        // function's public name holds its shim (ADR-383), which is never
                        // "us" — asking it made every contracted loop leave through its own
                        // contract check, once per iteration.
                        let name = crate::builtins::contracts::self_call_binding(heap, name);
                        let env = heap.read_root_env(genv);
                        // ADR-366: the epoch may also have moved because a miss IN THIS LOOP
                        // loaded a module, which marked this arm stale and dropped its cache
                        // entry at the next lookup — so the read below finds nothing (or a
                        // fresh arm with another uid) and the transition hands the loop to the
                        // recompiled body. The frame entered with the sentinel epoch so this
                        // path runs at the first back-edge after the load (see entry).
                        let still_us = match heap.env_get(env, name) {
                            Some(Value::Fn(id)) => super::cached_arm_for(heap, id, *argc)
                                .is_some_and(|other| other.uid == arm.uid),
                            // Unbound, or bound to something that isn't a closure: the name
                            // no longer denotes this loop, so stop looping on the old body.
                            _ => false,
                        };
                        if !still_us {
                            // The self-call is in TAIL position, so the rebound callee's
                            // value IS this frame's result. Hand it to the DRIVER as a tail
                            // call (this frame is reused, as for any `Inst::Call` in tail
                            // position) — never applied nested here: a nested apply would
                            // run the rest of the loop's life under a native frame, where a
                            // `receive` cannot be state-captured (it parks its worker dirty)
                            // and a native preempt has no driver to yield to. A VM closure comes back
                            // un-run as `Step::Tail`; a native or tree-walked callee ran to
                            // `Done`, exactly as a tail `Inst::Call` resolves.
                            let callee = heap
                                .env_get(env, name)
                                .ok_or_else(|| crate::eval::unbound_error(heap, name))?;
                            // The args stay on the operand stack across `dispatch`, as for
                            // `Inst::Call`: a native callee can collect, and `argv` is a
                            // copy the collector does not see. The driver drops the
                            // leftover operands for a `Tail`/`Done`, as it does there.
                            let n = heap.roots_len();
                            let argv: SmallVec<[Value; 4]> =
                                (0..*argc).map(|k| heap.root_at(n - argc + k)).collect();
                            return Ok(match dispatch(heap, callee, argv, true, env) {
                                Ok(Step::Tail {
                                    compiled,
                                    args,
                                    genv,
                                    bases,
                                }) => ChunkExit::Tail {
                                    arm: compiled,
                                    args,
                                    genv,
                                    bases,
                                },
                                Ok(Step::Done(v)) => ChunkExit::Done(v),
                                // A closure applied with `tail = true` is never RUN here, so
                                // no suspend can arrive; a kill unwinds untouched.
                                Err(e) => return Err(e),
                            });
                        }
                        arm.self_bound_stamp
                            .store(stamp, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                // Reset frame in place (same as the old outer-loop SelfTail handler).
                reset_frame_from_operands(heap, arm, base, *argc);
                *ip = 0;
                // Frame already reset; on a `Preempt` the driver captures it as-is.
                if let Some(exit) = loop_safepoint(heap, capture)? {
                    return Ok(exit);
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
                            // A boundary exit is worth more than one call toward the tier
                            // threshold (`BACKEDGE_TIER_WEIGHT`): `jit_tier` adds the last
                            // unit itself on re-entry.
                            if code.is_null() {
                                arm.jit_calls.fetch_add(
                                    BACKEDGE_TIER_WEIGHT - 1,
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
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
                module,
            } => {
                // The capture values are on
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
                let closure = crate::eval::make_closure_cached(heap, fn_rest.load(), env, *module)?;
                // Direct `letrec` self-recursion: bind the binder name to the closure
                // in its own captured env (the env↔closure cycle the tracing GC owns).
                if let Some(name) = self_name {
                    heap.env_define(env, *name, closure);
                }
                heap.push_root(closure);
            }
            // Inline `try` (ADR-400): the body runs right here as ordinary bytecode, so
            // the driver's preemption, `receive` capture and native calls all reach it.
            // `TryEnter` only registers where to unwind to; the catch itself is the
            // driver's (`vm_run_bc`), which owns the frame stack an error unwinds.
            Inst::TryEnter { handler, bind_slot } => {
                heap.vm_handlers.push(crate::core::heap::VmHandler {
                    kind: crate::core::heap::HandlerKind::Try,
                    base,
                    sp: heap.roots_len(),
                    env_len: heap.env_roots_len(),
                    arms_len: heap.live_arm_len(),
                    ip: *handler,
                    bind: base + *bind_slot,
                    dyn_len: heap.dynamics_len(),
                    caller: 0,
                });
            }
            Inst::TryExit => {
                let closed = heap.vm_handlers.pop();
                debug_assert!(
                    closed.is_some_and(
                        |h| h.base == base && h.kind == crate::core::heap::HandlerKind::Try
                    ),
                    "TryExit closed a handler this frame did not open (ADR-400): {closed:?} at base {base}"
                );
            }
            // Inline `binding` (ADR-402): push the names for the body that follows. An error
            // that skips `BindExit` is restored by the driver, which truncates the dynamic
            // stack to what the catching `try` (or its own entry) recorded.
            Inst::BindEnter { names } => {
                let n = heap.roots_len();
                let v = heap.root_at(n - 1);
                heap.truncate_roots(n - 1);
                let items = heap.seq_items(v)?;
                crate::builtins::push_bindings(heap, names, &items)?;
            }
            Inst::BindExit(count) => {
                let len = heap.dynamics_len();
                heap.truncate_dynamics(len - count);
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
/// at the original call site. Nested drivers (a native's
/// `vm_apply` callback) each append their own frames as the error crosses them,
/// so the chain accumulates across native boundaries. Only ever runs on the
/// error path; capped at [`crate::error::MAX_TRACE_FRAMES`] keeping the
/// innermost frames (the end that shows a recursion cycle).
pub(crate) fn attach_vm_trace(e: &mut LispError, cur_arm: &CompiledArm, frames: &[BcFrame]) {
    use crate::error::TraceFrame;
    if e.is_control() || e.trace_full() {
        return;
    }
    let contract_module = value::intern(CONTRACT_MODULE);
    // The running arm's entry, called from the innermost pending caller; then the
    // pending callers themselves — frame k was called from frame k-1, and the
    // driver's outermost frame (k = 0) was entered from a native boundary or the
    // top level, which the next driver out (if any) accounts for. A contract frame
    // is machinery and never named (see [`is_contract_arm`]).
    if !is_contract_arm(cur_arm, contract_module) {
        let (file, pos) = entry_site(frames, frames.len(), contract_module);
        e.push_trace(TraceFrame {
            name: cur_arm.fn_name.map(value::symbol_name_ref),
            file,
            pos,
        });
    }
    // An error raised in code that carries no positions — the prelude's, and so every
    // contract shim's (ADR-381) — would otherwise escape UNTAGGED and take the position of
    // whatever frame caught it (the `try`'s line, not the call that failed). Give it the
    // innermost call site that HAS a position — the user form whose call led into the
    // positionless code — which is what the `:trace` already records. `or_pos`: a position
    // a prim already tagged is never moved.
    // A contract shim's own code is never the answer (KI-184). The shim templates live in
    // `std/contract.blsp` (ADR-385), a positioned module. The ORIGINAL is thin, so
    // its primitive runs INLINE in the shim's frame and its error is tagged with the shim's
    // `(%contract-orig …)` instruction — line 135 of a file the user never wrote; an original
    // that is not thin comes back through the same call, tagged by it. A shim is generated
    // code: an error whose position the shim's own chunk owns (the running arm is a shim and
    // the position is one of its instructions', or a pending shim frame's call site equals it)
    // is treated as untagged, and the site the user is told is the innermost positioned call
    // OUTSIDE any shim frame — the call that entered it. A shim frame is known by its arm's
    // authoring MODULE (`CompiledArm::module`, ADR-383): this runs after the driver unwound
    // its roots, so a frame's env may not be read here; the arm's `src_file` and `fn_name`
    // are the ORIGINAL's, carried over by `contract_bind`; the module is the shim's own.
    // Ownership rather than the file:
    // a primitive's error carries a position only, and a real error from a deeper user
    // frame surfaces with that frame as the running arm, never through here.
    let chunk_owns = |arm: &CompiledArm, p: Pos| {
        arm.chunk
            .as_ref()
            .is_some_and(|c| c.code.iter().any(|i| i.call_pos() == Some(p)))
    };
    let tagged_by_shim = e.pos.is_some_and(|ep| {
        (is_contract_arm(cur_arm, contract_module) && chunk_owns(cur_arm, ep))
            || frames
                .iter()
                .rev()
                .filter(|f| is_contract_arm(&f.arm, contract_module))
                .any(|f| call_site(f).1 == Some(ep))
    });
    if e.pos.is_none() || tagged_by_shim {
        let inner = frames
            .iter()
            .rev()
            .filter(|f| !is_contract_arm(&f.arm, contract_module))
            .map(call_site)
            .find(|(_, p)| p.is_some());
        if let Some((f, Some(p))) = inner {
            e.pos = Some(p);
            if e.file.is_none() || tagged_by_shim {
                e.file = f.map(|s| s.to_string());
            }
        } else if tagged_by_shim {
            // No positioned call outside the machinery in THIS driver — the contracted
            // function was entered from a native boundary or the top level, which a driver
            // further out (or the file runner's enclosing form) accounts for. Leave the
            // error POSITIONLESS for them rather than keep the shim's own site: a kept
            // position is never moved again (`or_pos`), and the runner would report
            // `std/contract.blsp`'s line under the USER's file name (it fills the file, not
            // the position).
            e.pos = None;
            e.file = None;
        }
    } else if e.file.is_none() && e.pos.is_some_and(|ep| chunk_owns(cur_arm, ep)) {
        // The running arm's own instruction tagged the error (`tag_pos` sets a position
        // only): it happened in THIS arm's source file. Left unset, the file runner would
        // report a `std/math.blsp` line under the script's name, unlike the tree-walker
        // (`or_form_pos`), which names the right file.
        e.file = cur_arm.src_file.as_deref().map(str::to_string);
    }
    attach_vm_trace_callers(e, frames);
}

/// A contract frame — a shim or the checks it calls, all authored by `std/contract.blsp`
/// (ADR-385) — is MACHINERY: never named in a trace, never the position an error is
/// reported at, and never the call site a frame is said to have been entered from. The
/// shim's compiled body is shared by every closure its template builds (ADR-175/215, keyed
/// on the template's body form), so the name its arm recorded was whichever contracted
/// function first ran through it (`at string/join` for a user's `add2`); the
/// function the user called is the ORIGINAL's frame just inside, which names itself.
#[inline]
fn is_contract_arm(arm: &CompiledArm, contract_module: Symbol) -> bool {
    arm.module == Some(contract_module)
}

/// The call site a pending frame's saved `ip` returns to: `code[ip - 1]` is the
/// `Inst::Call` that pushed its callee, and its recorded `pos` (plus the arm's `src_file`)
/// locate the call.
pub(crate) fn call_site(f: &BcFrame) -> (Option<std::sync::Arc<str>>, Option<crate::error::Pos>) {
    let pos = f
        .arm
        .chunk
        .as_ref()
        .and_then(|c| f.ip.checked_sub(1).and_then(|i| c.code.get(i)))
        .and_then(|inst| inst.call_pos());
    (f.arm.src_file.clone(), pos)
}

/// The call site that entered `frames[k]` (`k == frames.len()` for the running arm): the
/// nearest pending caller below it that is not contract machinery. A frame a shim called
/// — the ORIGINAL — was entered, as far as its author is concerned, where the shim was.
fn entry_site(
    frames: &[BcFrame],
    k: usize,
    contract_module: Symbol,
) -> (Option<std::sync::Arc<str>>, Option<crate::error::Pos>) {
    frames[..k]
        .iter()
        .rev()
        .find(|f| !is_contract_arm(&f.arm, contract_module))
        .map(call_site)
        .unwrap_or((None, None))
}

/// The pending-callers half of [`attach_vm_trace`], without the running arm's own entry.
/// For an arm that ran NATIVELY at the driver's loop top (`jit_tier_in_frame` outcome 3):
/// its error block already recorded it through `brood_rt_trace_push` (KI-117), so
/// attaching `cur_arm` here again would name the same frame twice.
pub(crate) fn attach_vm_trace_callers(e: &mut LispError, frames: &[BcFrame]) {
    use crate::error::TraceFrame;
    if e.is_control() || e.trace_full() {
        return;
    }
    let contract_module = value::intern(CONTRACT_MODULE);
    for k in (0..frames.len()).rev() {
        if e.trace_full() {
            break;
        }
        if is_contract_arm(&frames[k].arm, contract_module) {
            continue;
        }
        let (file, pos) = entry_site(frames, k, contract_module);
        e.push_trace(TraceFrame {
            name: frames[k].arm.fn_name.map(value::symbol_name_ref),
            file,
            pos,
        });
    }
}

/// The safepoint of an inline self-tail loop iteration — the ONE copy shared by the
/// `SelfCall` back-edge and the IC self-tail path (the driver's loop top in `vm_run_bc` is the
/// frame-boundary twin, with the same split). Keep it one copy: hand-kept copies drift, and a
/// loop that skips the RUNTIME drain report (a nested run never leaving `exec_chunk`) pins
/// the old generation for every process.
///
/// Per iteration: the process heap limit (a heap field) and the deadline (a `Cell` read).
/// Once per quantum, at the reduction rollover: the soft memory limit (it sums 64 allocator
/// shards when armed), the mailbox bound, an interrupt (ADR-491) and a pending hard kill (each
/// a thread-local `RefCell` borrow plus an atomic), and the drain report. A kill, an interrupt
/// or an overflow is honoured within one quantum — the latency the capture-mode preempt has anyway.
#[inline(always)]
fn loop_safepoint(heap: &mut Heap, capture: bool) -> Result<Option<ChunkExit>, LispError> {
    if let Some(live) = heap.take_proc_limit_hit() {
        let limit = heap.proc_mem_limit().unwrap_or(0);
        return Err(crate::eval::proc_memory_limit_error(live, limit));
    }
    if let crate::process::Safepoint::Quantum { yield_now } =
        crate::process::driver_safepoint(capture)
    {
        quantum_checks(heap)?;
        if crate::process::capture_hard_kill_pending() {
            // Capture mode retires the process at the driver; a nested run cannot return an
            // outcome across its native frame, so it unwinds with the untrappable signal and
            // the top-level driver converts it to `VmOutcome::Killed`.
            return if capture {
                Ok(Some(ChunkExit::Killed))
            } else {
                Err(crate::error::LispError::kill_signal())
            };
        }
        if yield_now {
            return Ok(Some(ChunkExit::Preempt));
        }
    }
    Ok(None)
}

/// The once-per-quantum half of [`loop_safepoint`] (and of the driver's loop top): the checks
/// too costly to make per iteration, none of which needs finer latency.
#[inline(never)]
pub(super) fn quantum_checks(heap: &Heap) -> Result<(), LispError> {
    crate::core::alloc::refresh_container_limits();
    match crate::core::alloc::soft_limit_state() {
        crate::core::alloc::SoftLimit::Clear => {}
        crate::core::alloc::SoftLimit::Near => crate::process::near_soft_limit(heap),
        crate::core::alloc::SoftLimit::Over(used) => {
            if let Some(error) = crate::process::soft_limit_crossed(heap, used) {
                return Err(error);
            }
        }
    }
    if let Some((len, limit)) = crate::process::take_current_mailbox_overflow() {
        return Err(crate::eval::proc_mailbox_limit_error(len, limit));
    }
    // An interrupt (ADR-491) is honoured here, once per quantum like the mailbox bound: a
    // relaxed load when nothing is armed. Not past a pending hard kill, which both callers
    // check next: the kill is untrappable, and an interrupt raised first would run the
    // interrupted code's handlers — the window's close and whatever follows it — before it.
    if !crate::process::capture_hard_kill_pending() {
        crate::eval::check_interrupt()?;
    }
    if heap.drain_active() {
        crate::process::report_drain_liveness(heap);
    }
    Ok(())
}

/// Reset a frame in place for a self-tail iteration: params take the new args, the
/// body's `let`/`letrec` slots go back to nil, and the **capture slots stay**.
///
/// The capture slots (`#3 lexical addressing`, right after the params — both self-tail
/// paths require no optionals and no rest, so they begin at `nrequired`) hold the
/// closure's enclosing lexicals, filled once by `push_frame` from the captured env. A
/// self-tail iteration re-enters the same arm under the same env, so those values are
/// still exactly right — and nil-ing them is not a reset, it is a loss: the next read of
/// a captured name in the loop body sees `nil` (e.g. a `defn` inside a `let` that sends
/// to a captured `me`).
///
/// The new args are read straight off the operand stack — the call's last `argc` roots —
/// rather than through a copied-out `SmallVec`. The copy is forward-safe: the operands sit
/// above the frame (`roots_len() - argc >= base + nslots`), so every source slot is above
/// every destination slot. Only the slots the args do not overwrite are nil-ed, and
/// `roots` is truncated once, to the frame top. Nothing here allocates, so no collection
/// can move a value between the read and the write.
#[inline]
fn reset_frame_from_operands(heap: &mut Heap, arm: &CompiledArm, base: usize, argc: usize) {
    let first_operand = heap.roots_len() - argc;
    let frame_top = base + arm.nslots;
    debug_assert!(
        first_operand >= frame_top,
        "self-tail operands overlap the frame they reset"
    );
    let written = argc.min(arm.nrequired);
    for k in 0..written {
        let v = heap.root_at(first_operand + k);
        heap.set_root_at(base + k, v);
    }
    // No captures to keep — nearly every loop: the rest of the frame is nil, which is one
    // bulk fill, not a test per slot.
    if arm.capture_names.is_empty() {
        heap.truncate_roots(base + written);
        heap.extend_roots_to_nil(frame_top);
        return;
    }
    heap.truncate_roots(frame_top);
    let capture_lo = arm.nrequired;
    let capture_hi = capture_lo + arm.capture_names.len();
    for i in written..arm.nslots {
        if i < capture_lo || i >= capture_hi {
            heap.set_root_at(base + i, Value::nil());
        }
    }
}
