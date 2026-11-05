use super::*;

// Bump a movable handle's index by `by`; leave atoms alone. Stands in for the
// `runtime_collect` flush that relocates a handle into the compacted region.
fn bump(v: Value, by: usize) -> Value {
    match v.unpack() {
        ValueRef::Str(id) => Value::str_(StrId::runtime(id.index() + by)),
        ValueRef::Pair(id) => Value::pair(PairId::runtime(id.index() + by)),
        _ => v,
    }
}

// `Value` has no `PartialEq` (Brood equality is a structural function), so compare
// a handle const by kind + index.
fn str_idx(v: Value) -> usize {
    match v.unpack() {
        ValueRef::Str(id) => id.index(),
        other => panic!("expected a Str, got {:?}", std::mem::discriminant(&other)),
    }
}
fn pair_idx(v: Value) -> usize {
    match v.unpack() {
        ValueRef::Pair(id) => id.index(),
        other => panic!("expected a Pair, got {:?}", std::mem::discriminant(&other)),
    }
}

/// Regression: a swapped `(op Const Local)` `Prim2SlotInt` must keep inlining after an
/// epoch bump. The fusion stores an *inverted* arg-map (so the inline operand pick is
/// correct); `prim2_inline_exec` revalidates against the head's *natural* map, so the
/// `swapped` call site must un-invert it; comparing the inverted map never matches
/// `resolve_prim`'s natural map, so every such prim would silently fall to the slow path
/// forever after the first `def` bumped the epoch.
#[test]
fn swapped_prim2slotint_reinlines_after_epoch_bump() {
    let mut interp = crate::Interp::new();
    let heap = &mut interp.heap;
    let minus = value::intern("-"); // natural map [0,1]; `(- 24 x)` fuses to [1,0] swapped
    // A stale guard (≠ current epoch) forces the revalidation path the bug lived on.
    let guard = AtomicU64::new(heap.global_epoch().wrapping_add(1));
    // Operands as the caller picks them for map=[1,0]: x = const 24, y = local 5.
    let out = prim2_inline_exec(
        heap,
        PrimOp::Sub,
        [1, 0],
        true,
        minus,
        &guard,
        Value::int(24),
        Value::int(5),
    )
    .expect("no arithmetic error");
    match out {
        Some(v) => match v.unpack() {
            ValueRef::Int(n) => assert_eq!(n, 19, "(- 24 5) must inline to 19"),
            _ => panic!("expected Int(19), got tag {:?}", value::tag(v)),
        },
        None => panic!("swapped Prim2SlotInt slow-pathed after an epoch bump (the bug)"),
    }
    // The guard was refreshed to the live epoch, so subsequent calls take the fast path.
    assert_eq!(guard.load(Ordering::Relaxed), heap.global_epoch());
}

#[test]
fn const_handle_round_trips() {
    // A heap-handle const decodes back to the same handle, and `rewrite` moves it.
    let cv = ConstVal::new(Value::str_(StrId::runtime(5)));
    assert!(
        matches!(cv, ConstVal::Handle { .. }),
        "a Str must encode as a Handle"
    );
    assert_eq!(str_idx(cv.load()), 5);
    cv.rewrite(&mut |v| bump(v, 100));
    assert_eq!(str_idx(cv.load()), 105, "rewrite must relocate the handle");

    // An atom stays inline and is never touched by a rewrite.
    let atom = ConstVal::new(Value::int(42));
    assert!(
        matches!(atom, ConstVal::Atom(_)),
        "an Int must encode as an Atom"
    );
    atom.rewrite(&mut |_| panic!("an atom const must not be passed to the flush"));
    assert!(matches!(atom.load().unpack(), ValueRef::Int(42)));
}

#[test]
fn rewrite_arm_handles_rewrites_every_embedded_handle() {
    // The regression guard: `runtime_collect` calls this on each LIVE arm, so it
    // must reach every movable handle a node tree embeds — a `Const` literal, a
    // `MakeClosure` `fn_rest` — through all the structural node variants, while leaving
    // atoms/symbols/indices alone. (An `&optional` default is part of the body now, so the
    // body's walk is what reaches it.)
    let body = Node::Do(Box::new([
        Node::Const(ConstVal::new(Value::str_(StrId::runtime(1)))),
        Node::If(
            Box::new(Node::Const(ConstVal::new(Value::int(7)))), // atom — untouched
            Box::new(Node::Const(ConstVal::new(Value::pair(PairId::runtime(2))))),
            Box::new(Node::MakeClosure {
                fn_rest: ConstVal::new(Value::pair(PairId::runtime(3))),
                captures: Box::new([]),
                self_name: None,
                module: None,
            }),
        ),
    ]));
    let arm = CompiledArm {
        body,
        has_runtime_handles: true,
        ..CompiledArm::default()
    };

    rewrite_arm_handles(&arm, &mut |v| bump(v, 100));

    // Destructure the (known) tree and assert each handle moved, the atom didn't.
    let Node::Do(top) = &arm.body else {
        panic!("body")
    };
    assert_eq!(str_idx(load_const(&top[0])), 101);
    let Node::If(cond, then, els) = &top[1] else {
        panic!("if")
    };
    assert!(
        matches!(load_const(cond).unpack(), ValueRef::Int(7)),
        "atom const must be untouched"
    );
    assert_eq!(pair_idx(load_const(then)), 102);
    let Node::MakeClosure { fn_rest, .. } = &**els else {
        panic!("makeclosure")
    };
    assert_eq!(pair_idx(fn_rest.load()), 103);
}

fn load_const(node: &Node) -> Value {
    match node {
        Node::Const(cv) => cv.load(),
        other => panic!("expected a Const, got {:?}", std::mem::discriminant(other)),
    }
}

// ===================== state-capture (ADR-100 §8) =====================

thread_local! {
    /// Drives the suspend-once test native: 0 → suspend, ≥1 → return the value.
    static SUSPEND_GATE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// A stand-in for the `%receive` native: the **first** call raises a
/// `Control::Suspend` (as a clean `receive` on an empty mailbox would); the
/// **second** returns a value (as it would once a message arrived). Lets the
/// capture→resume round-trip be tested in isolation
/// from the mailbox/scheduler plumbing — the machinery under test is the driver's
/// capture + replay, identical for any native that suspends mid-call.
fn suspend_once_native(_args: &[Value], _env: EnvId, _heap: &mut Heap) -> LispResult {
    let n = SUSPEND_GATE.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    if n == 0 {
        Err(LispError::suspend(None))
    } else {
        Ok(Value::int(42))
    }
}

#[test]
fn vm_run_bc_captures_and_resumes_a_suspend() {
    use crate::core::value::{Arity, NativeFn};
    use crate::types::Sig;

    SUSPEND_GATE.with(|c| c.set(0));
    let mut heap = Heap::new();

    // The suspend-once native, held in the arm's one frame slot (slot 0). A 0-arg
    // `Inst::Call` against it is the suspending point — the shape a `(receive …)`
    // lowers to (`%receive` is the callee, here `slot 0`).
    let native = heap.alloc_native(NativeFn {
        name: "%test-suspend-once".to_string(),
        arity: Arity::exact(0),
        func: suspend_once_native,
        params: &[],
        doc: "",
        sig: Sig::any(),
        is_apply: false,
        is_interruptible: false,
    });

    // Body `(slot0)`: push the native from slot 0, then a non-tail 0-ary call.
    let chunk = Chunk {
        code: vec![
            Inst::Local(0),
            Inst::Call {
                argc: 0,
                tail: false,
                pos: None,
                site: NO_SITE,
                head: None,
                staged: false,
            },
        ],
    };
    let arm = Arc::new(CompiledArm {
        nrequired: 1, // slot 0 = the callee, passed as the sole arg
        nslots: 1,
        body: Node::Const(ConstVal::new(Value::nil())), // unused at runtime (chunk drives)
        chunk: Some(chunk),
        ..CompiledArm::default()
    });

    // First run: the native suspends, so the driver captures the continuation
    // WITHOUT unwinding (the operand stack — the pushed callee — survives on the
    // heap for the resume).
    let roots_before = heap.roots_len();
    let outcome = vm_run_bc(
        &mut heap,
        ArmHandle::new(arm.clone()),
        &[native],
        EnvId::GLOBAL,
        None,
        true,
    )
    .expect("first run errored");
    let suspended = match outcome {
        VmOutcome::Suspended(s) => s.into_vm(),
        _ => panic!("expected a captured suspend"),
    };
    assert!(
        heap.roots_len() > roots_before,
        "the captured continuation's frame slots + operands must stay rooted"
    );

    // Resume: replay from the rewound `%receive` call; the native now returns 42.
    let resumed = vm_run_bc(
        &mut heap,
        ArmHandle::new(arm),
        &[native],
        EnvId::GLOBAL,
        Some(suspended),
        true,
    )
    .expect("resume errored");
    match resumed {
        VmOutcome::Done(v) => match v.unpack() {
            ValueRef::Int(n) => assert_eq!(n, 42, "resumed to the wrong value"),
            other => panic!("resumed to a non-int: {:?}", value::tag(other)),
        },
        other => panic!(
            "expected Done(42), got {}",
            match other {
                VmOutcome::Suspended(_) => "Suspended (the gate didn't advance)",
                VmOutcome::Preempted(_) => "Preempted",
                VmOutcome::Killed => "Killed",
                VmOutcome::Done(_) => unreachable!(),
            }
        ),
    }
    // The driver retired its only frame on `Done`, unwinding the operand stack
    // back to where the first run started.
    assert_eq!(
        heap.roots_len(),
        roots_before,
        "a completed resume must tear its frame stack back down to entry"
    );
}

/// The tier-0 escape audit's positive control (ADR-416). Since stage 4 no program reaches the
/// VM under `BROOD_TIER=0`, so the CLI guard (`crates/cli/tests/tier0_escape_audit.rs`)
/// asserts an EMPTY audit — which is also what a broken audit writes. Here bytecode is started
/// directly under a tree-walk ceiling, and the audit must name a placed site for it.
#[test]
fn the_tier0_audit_records_bytecode_started_under_a_tree_walk_ceiling() {
    let path = std::env::temp_dir().join(format!("brood-tier0-audit-unit-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    vm_run_bc::TIER0_AUDIT_OVERRIDE
        .with(|p| *p.borrow_mut() = Some(path.to_string_lossy().into_owned()));
    set_forced_ceiling(Some(Tier::TreeWalk));

    let mut heap = Heap::new();
    let arm = Arc::new(CompiledArm {
        nslots: 0,
        body: Node::Const(ConstVal::new(Value::int(7))),
        chunk: Some(Chunk {
            code: vec![Inst::Const(ConstVal::new(Value::int(7)))],
        }),
        ..CompiledArm::default()
    });
    let outcome = vm_run_bc(
        &mut heap,
        ArmHandle::new(arm),
        &[],
        EnvId::GLOBAL,
        None,
        true,
    );

    set_forced_ceiling(None);
    vm_run_bc::TIER0_AUDIT_OVERRIDE.with(|p| *p.borrow_mut() = None);
    let lines = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    assert!(
        matches!(outcome, Ok(VmOutcome::Done(_))),
        "the arm did not run"
    );
    assert!(
        lines.lines().any(|l| l.starts_with("top\t")
            && l.contains("the_tier0_audit_records_bytecode_started_under_a_tree_walk_ceiling")),
        "the audit did not record the bytecode this test started at tier 0:\n{lines}"
    );
}

/// JIT Stage-1 Step A: lower a straight-line int arm `(+ x 1)` to native code and
/// run it against a real heap frame — read the arg from `roots[base]`, compute in
/// registers, box the result back, and match the VM's answer.
/// Run a lowered arm the way the runtime does: hand it an `out` slot and read the Done
/// result from there, not from `roots[base]`. Since the ABI is reached by `transmute`, a
/// test that called it with the old arity would be silent UB rather than a compile error —
/// which is the reason [`crate::jit::JitArmFn`] is a named type and this is a helper.
///
/// It enters as a top-level driver does — `unwind` set, so a spent budget preempts
/// (ADR-401). [`run_arm_nested`] enters as everything else does.
#[cfg(feature = "jit")]
fn run_arm(f: crate::jit::JitArmFn, heap: &mut Heap, base: usize) -> (i64, Value) {
    run_arm_with(f, heap, base, 1)
}

/// [`run_arm`] entered the way a builtin calling back or a nested evaluation enters native
/// code: `unwind` clear, because a frame nothing can capture sits between it and the driver.
#[cfg(feature = "jit")]
fn run_arm_nested(f: crate::jit::JitArmFn, heap: &mut Heap, base: usize) -> (i64, Value) {
    run_arm_with(f, heap, base, 0)
}

#[cfg(feature = "jit")]
fn run_arm_with(f: crate::jit::JitArmFn, heap: &mut Heap, base: usize, unwind: u8) -> (i64, Value) {
    let mut out = Value::Nil;
    let mut ctx = crate::jit::JitCallCtx::from_heap(heap);
    ctx.unwind = unwind;
    let outcome = f(
        heap as *mut Heap,
        base as i64,
        &mut out as *mut Value,
        &mut ctx,
    );
    (outcome, out)
}

/// The `%receive` fence (see `chunk_in_jit_subset`): a chunk that calls `%receive` must
/// never enter the JIT subset, because a park inside the native boundary surfaces as an
/// uncatchable empty `runtime error:` that kills the receiver. With `MakeClosure` in the
/// subset, the fence is load-bearing on its own. Deterministic where the runtime repro
/// (`local_send_race` on a 2-worker pool) is contention-shaped: this asserts the predicate
/// itself, on a real compiled receive-bearing arm — and, non-vacuously, that the SAME arm
/// with the receive removed IS in the subset (so a predicate that rejects everything cannot
/// fake a pass).
#[cfg(feature = "jit")]
#[test]
fn receive_bearing_chunks_stay_out_of_the_jit_subset() {
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(def rcv (fn (want acc) (if (= want 0) acc (receive (m (rcv (- want 1) (+ acc m)))))))")
        .expect("define rcv");
    let f = interp.eval_str("rcv").expect("read rcv back");
    let heap = &mut interp.heap;
    let id = match f.unpack() {
        crate::core::value::ValueRef::Fn(id) => id,
        other => panic!(
            "expected a closure, got {:?}",
            crate::core::value::tag(other)
        ),
    };
    let arm = compiled_arm_for(heap, id, 2).expect("rcv/2 compiles");
    let chunk = arm.chunk.as_ref().expect("rcv has a chunk");
    assert!(
        !crate::eval::compile::jit_plan::chunk_in_jit_subset(&chunk.code),
        "a %receive-bearing chunk entered the JIT subset — a native park kills the process"
    );

    // Non-vacuity: the same shape minus the receive is in the subset.
    interp
        .eval_str("(def norcv (fn (want acc) (if (= want 0) acc (norcv (- want 1) (+ acc 1)))))")
        .expect("define norcv");
    let g = interp.eval_str("norcv").expect("read norcv back");
    let heap = &mut interp.heap;
    let gid = match g.unpack() {
        crate::core::value::ValueRef::Fn(id) => id,
        other => panic!(
            "expected a closure, got {:?}",
            crate::core::value::tag(other)
        ),
    };
    let garm = compiled_arm_for(heap, gid, 2).expect("norcv/2 compiles");
    let gchunk = garm.chunk.as_ref().expect("norcv has a chunk");
    assert!(
        crate::eval::compile::jit_plan::chunk_in_jit_subset(&gchunk.code),
        "the receive-free control arm should be in the subset — the fence test would be vacuous"
    );
}

/// ADR-494: a chunk that reads `proc/interruptible` as a VALUE — `(apply proc/interruptible
/// …)`, which the VM unfolds into an interrupt region — stays out of the JIT subset, as one
/// that calls it does: from native code the builtin would run the thunk nested, where a
/// `receive` blocks the worker. Non-vacuous: the same shape applying `vector` is in it.
#[cfg(feature = "jit")]
#[test]
fn a_chunk_reading_proc_interruptible_stays_out_of_the_jit_subset() {
    let mut interp = crate::Interp::new();
    let mut in_subset = |source: &str, name: &str| {
        interp.eval_str(source).expect("define the arm");
        let f = interp.eval_str(name).expect("read the arm back");
        let crate::core::value::ValueRef::Fn(id) = f.unpack() else {
            panic!("{name} is not a closure");
        };
        let arm = compiled_arm_for(&interp.heap, id, 2).expect("the arm compiles");
        let chunk = arm.chunk.as_ref().expect("the arm has a chunk");
        crate::eval::compile::jit_plan::chunk_in_jit_subset(&chunk.code)
    };
    assert!(
        !in_subset(
            "(def applies-window (fn (ceiling thunk) (apply proc/interruptible (list ceiling thunk))))",
            "applies-window"
        ),
        "a chunk applying proc/interruptible entered the JIT subset"
    );
    assert!(
        in_subset(
            "(def applies-vector (fn (ceiling thunk) (apply vector (list ceiling thunk))))",
            "applies-vector"
        ),
        "the control arm should be in the subset — the fence test would be vacuous"
    );
}
/// ADR-410: which arms receive AT THEIR HEAD — may go native, parking by a re-run from ip 0.
/// Only a `%receive` behind nothing but loads qualifies; one behind a branch or a call does
/// not (re-running those from ip 0 is not the same), and neither does a second `%receive`.
#[test]
fn receive_at_head_admits_only_a_leading_receive() {
    let mut interp = crate::Interp::new();
    let arm_of = |interp: &mut crate::Interp, src: &str, name: &str, argc: usize| {
        interp.eval_str(src).expect("define");
        let f = interp.eval_str(name).expect("read back");
        let crate::core::value::ValueRef::Fn(id) = f.unpack() else {
            panic!("{name} is not a closure")
        };
        compiled_arm_for(&interp.heap, id, argc).expect("compiles")
    };
    let head = arm_of(
        &mut interp,
        "(def lp (fn (acc) (receive ([:add n] (lp (+ acc n))) (:stop acc))))",
        "lp",
        1,
    );
    assert!(
        head.receive_at_head,
        "a loop that receives first is a receive-at-head arm"
    );
    assert!(
        crate::eval::compile::jit_plan::chunk_in_jit_subset(&head.chunk.as_ref().unwrap().code),
        "a receive-at-head chunk passes the %receive fence"
    );
    for (src, name, argc) in [
        // Behind a branch.
        (
            "(def br (fn (want acc) (if (= want 0) acc (receive (m (br (- want 1) (+ acc m)))))))",
            "br",
            2,
        ),
        // Behind a call with an effect.
        (
            "(def ef (fn (pid) (send pid :ready) (receive (m (ef pid)))))",
            "ef",
            1,
        ),
        // Two receives.
        (
            "(def two (fn () (receive (a (receive (b (two)))))))",
            "two",
            0,
        ),
    ] {
        let arm = arm_of(&mut interp, src, name, argc);
        assert!(
            !arm.receive_at_head,
            "{name} must not be a receive-at-head arm"
        );
        assert!(
            !crate::eval::compile::jit_plan::chunk_in_jit_subset(&arm.chunk.as_ref().unwrap().code),
            "{name} must stay behind the %receive fence"
        );
    }
}

#[test]
#[cfg(feature = "jit")]
fn jit_lowers_and_runs_a_straight_line_int_arm() {
    let mut heap = Heap::new();
    // Body `(+ x 1)`: [Local(0), Const(1), Prim2 Add].
    let chunk = Chunk {
        code: vec![
            Inst::Local(0),
            Inst::Const(ConstVal::new(Value::int(1))),
            Inst::Prim2 {
                op: PrimOp::Add,
                map: [0, 1],
                head: value::intern("+"),
                guard: AtomicU64::new(0),
                pos: None,
            },
        ],
    };
    let arm = CompiledArm {
        nrequired: 1,
        nslots: 1,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        ..CompiledArm::default()
    };

    let mut jit = crate::jit::CraneliftBackend::new();
    let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("straight-line int arm should JIT");
    let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(ptr) };

    // Frame: x = 41 at roots[base].
    let base = heap.roots_len();
    heap.push_root(Value::int(41));
    let (outcome, ret) = run_arm(f, &mut heap, base);
    assert_eq!(
        outcome,
        crate::jit::outcome::DONE,
        "Done (no deopt — arg is an Int)"
    );
    match ret.unpack() {
        ValueRef::Int(n) => assert_eq!(n, 42, "JIT-compiled (+ x 1) on x=41"),
        other => panic!("expected Int(42), got tag {:?}", value::tag(other)),
    }
}

/// JIT Stage-1 Step B: control flow + comparisons. Lower `(if (< x 0) (- 0 x) x)`
/// (an `abs`) — JumpIfFalse/Jump → CFG blocks, `<` → an `icmp` branch, the two arms
/// merging at a Done block param — and check both arms against the math.
#[cfg(feature = "jit")]
#[test]
fn jit_lowers_and_runs_an_if_with_comparison() {
    let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
        op,
        map: [0, 1],
        head: value::intern(head),
        guard: AtomicU64::new(0),
        pos: None,
    };
    // (if (< x 0) (- 0 x) x), x = slot 0.
    let chunk = Chunk {
        code: vec![
            Inst::Local(0),                            // 0: x
            Inst::Const(ConstVal::new(Value::int(0))), // 1: 0
            prim2(PrimOp::Lt, "<"),                    // 2: x < 0
            Inst::JumpIfFalse(8),                      // 3: false → else (ip 8)
            Inst::Const(ConstVal::new(Value::int(0))), // 4: then: 0
            Inst::Local(0),                            // 5: x
            prim2(PrimOp::Sub, "-"),                   // 6: 0 - x
            Inst::Jump(9),                             // 7: → done (ip 9 = len)
            Inst::Local(0),                            // 8: else: x
        ],
    };
    let arm = CompiledArm {
        nrequired: 1,
        nslots: 1,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        ..CompiledArm::default()
    };

    let mut jit = crate::jit::CraneliftBackend::new();
    let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("if/cmp arm should JIT");
    let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(ptr) };

    for (x, want) in [(-5i64, 5i64), (3, 3), (0, 0)] {
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(x));
        let (outcome, ret) = run_arm(f, &mut heap, base);
        assert_eq!(outcome, crate::jit::outcome::DONE, "Done for x={x}");
        match ret.unpack() {
            ValueRef::Int(n) => assert_eq!(n, want, "abs({x})"),
            other => panic!(
                "x={x}: expected Int({want}), got tag {:?}",
                value::tag(other)
            ),
        }
    }
}

/// JIT Stage-1 Step C: the self-recursive **loop**. Lower
/// `(if (< i 1) acc (sumto (- i 1) (+ acc i)))` — `SelfCall` boxes the new args into
/// the frame slots and branches the loop header; the frame slots in `roots` carry the
/// loop state. A native int loop, no per-iteration dispatch. (No `tick` yet — tested
/// in isolation, not wired into the scheduler.)
#[cfg(feature = "jit")]
#[test]
fn jit_lowers_and_runs_a_self_recursive_int_loop() {
    let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
        op,
        map: [0, 1],
        head: value::intern(head),
        guard: AtomicU64::new(0),
        pos: None,
    };
    // (defn sumto (i acc) (if (< i 1) acc (sumto (- i 1) (+ acc i)))) — i=slot0, acc=slot1.
    let chunk = Chunk {
        code: vec![
            Inst::Local(0),                            // 0: i
            Inst::Const(ConstVal::new(Value::int(1))), // 1: 1
            prim2(PrimOp::Lt, "<"),                    // 2: i < 1
            Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
            Inst::Local(1),                            // 4: then: acc
            Inst::Jump(13),                            // 5: → done (len)
            Inst::Local(0),                            // 6: else: i
            Inst::Const(ConstVal::new(Value::int(1))), // 7: 1
            prim2(PrimOp::Sub, "-"),                   // 8: (- i 1)  = arg0
            Inst::Local(1),                            // 9: acc
            Inst::Local(0),                            // 10: i
            prim2(PrimOp::Add, "+"),                   // 11: (+ acc i) = arg1
            Inst::SelfCall { argc: 2 },                // 12: (sumto arg0 arg1)
        ],
    };
    let arm = CompiledArm {
        nrequired: 2,
        nslots: 2,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        ..CompiledArm::default()
    };

    let mut jit = crate::jit::CraneliftBackend::new();
    let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("self-recursive int loop should JIT");
    let f: crate::jit::JitArmFn = unsafe { std::mem::transmute(ptr) };

    // Prime the reduction budget so these short loops run to completion (the
    // back-edge `brood_rt_tick_n` would otherwise yield at REDUCTIONS == 0).
    crate::process::yield_now();
    // sumto(n,0) = n+(n-1)+…+1; sumto(1,0)→sumto(0,1)→1; sumto(0,0)→0.
    for (n, want) in [(5i64, 15i64), (100, 5050), (1, 1), (0, 0)] {
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(n)); // i = slot 0
        heap.push_root(Value::int(0)); // acc = slot 1
        let (outcome, ret) = run_arm(f, &mut heap, base);
        assert_eq!(outcome, crate::jit::outcome::DONE, "Done for n={n}");
        match ret.unpack() {
            ValueRef::Int(r) => assert_eq!(r, want, "sumto({n}, 0)"),
            other => panic!(
                "n={n}: expected Int({want}), got tag {:?}",
                value::tag(other)
            ),
        }
    }

    // Preemption: a loop longer than the reduction budget yields at a back-edge —
    // the JIT'd arm returns 2 (preempt), with the frame slots left mid-computation
    // in `roots` for the driver to resume on the VM. `brood_rt_tick_n` only preempts in
    // a capture-mode green process, so simulate one (set/clear `capture_run`), with a
    // process waiting for the worker — nothing else is worth unwinding for.
    crate::process::set_capture_run(true);
    crate::process::test_work_waiting(Some(true));
    crate::process::yield_now(); // budget = REDUCTION_BUDGET
    let mut heap = Heap::new();
    let base = heap.roots_len();
    heap.push_root(Value::int(1_000_000)); // far more iterations than the budget
    heap.push_root(Value::int(0));
    let (outcome, _) = run_arm(f, &mut heap, base);
    crate::process::test_work_waiting(Some(false));
    // The same unwindable loop with no process waiting: a preempt would re-queue it and pick
    // it straight back up, so it runs on through every spent budget and finishes.
    crate::process::yield_now();
    let mut heap = Heap::new();
    let base = heap.roots_len();
    heap.push_root(Value::int(1_000_000));
    heap.push_root(Value::int(0));
    let (alone_outcome, alone_ret) = run_arm(f, &mut heap, base);
    // The same loop entered where nothing can capture it (ADR-401): a preempt could only
    // deopt into an interpreter that cannot capture either, so it keeps running native on a
    // fresh quantum and finishes — sum 0..1_000_000.
    crate::process::yield_now();
    let mut heap = Heap::new();
    let base = heap.roots_len();
    heap.push_root(Value::int(1_000_000));
    heap.push_root(Value::int(0));
    let (nested_outcome, nested_ret) = run_arm_nested(f, &mut heap, base);
    crate::process::set_capture_run(false); // restore (cargo test shares threads)
    crate::process::test_work_waiting(None);
    assert_eq!(
        outcome,
        crate::jit::outcome::PREEMPT,
        "a loop exceeding the budget must preempt in a green process"
    );
    assert_eq!(
        alone_outcome,
        crate::jit::outcome::DONE,
        "with no process waiting, a spent budget must not unwind (nothing would run instead)"
    );
    assert!(
        matches!(alone_ret.unpack(), ValueRef::Int(500_000_500_000)),
        "the unpreempted run's result"
    );
    assert_eq!(
        nested_outcome,
        crate::jit::outcome::DONE,
        "an activation that cannot unwind must run on, not deopt into a nested interpreter"
    );
    assert!(
        matches!(nested_ret.unpack(), ValueRef::Int(500_000_500_000)),
        "the nested run's result"
    );
}

/// An arm *ending* in a **tail call with a staged (computed) callee**
/// (`Inst::Call { tail: true, head: None }`) must lower (return `Some`), not bail —
/// the jit-tier2 §6.2 payoff. The body is deliberately past the body-weight gate
/// (4 work ops: `=`, `-`, `*`, `*`), since a thinner tail-call arm is gated out.
/// We can't run it in isolation (outcome 4 needs the driver to dispatch the staged
/// callee), so this asserts the *lowering* succeeds; `tests/jit.rs` proves the result.
///
/// Also pins the deliberate counter-case: a **free-global** tail call
/// (`head: Some`, the head elided from the operand stack) *bails*. The tail path
/// (`jit_dispatch_tail`, outcome 4) reads a *staged* callee, which an elided head
/// doesn't leave behind — so such arms (the common mutual-recursion shape) stay on
/// the correct VM path rather than lower into a stale-callee read.
#[cfg(feature = "jit")]
#[test]
fn jit_lowers_an_arm_ending_in_a_tail_call() {
    let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
        op,
        map: [0, 1],
        head: value::intern(head),
        guard: AtomicU64::new(0),
        pos: None,
    };
    // (defn fa (n acc) (if (= n 0) acc (fb (- n 1) (* (* acc acc) acc)))) — n=slot0, acc=slot1.
    let fb = value::intern("fb");
    let chunk = Chunk {
        code: vec![
            Inst::Local(0),                            // 0: n
            Inst::Const(ConstVal::new(Value::int(0))), // 1: 0
            prim2(PrimOp::Eq, "="),                    // 2: n == 0    (work 1)
            Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
            Inst::Local(1),                            // 4: then: acc
            Inst::Jump(16),                            // 5: → done (len)
            Inst::Global(fb),                          // 6: else: callee `fb`
            Inst::Local(0),                            // 7: n
            Inst::Const(ConstVal::new(Value::int(1))), // 8: 1
            prim2(PrimOp::Sub, "-"),                   // 9: (- n 1) = arg0   (work 2)
            Inst::Local(1),                            // 10: acc
            Inst::Local(1),                            // 11: acc
            prim2(PrimOp::Mul, "*"),                   // 12: (* acc acc)     (work 3)
            Inst::Local(1),                            // 13: acc
            prim2(PrimOp::Mul, "*"),                   // 14: (* … acc) = arg1 (work 4)
            Inst::Call {
                argc: 2,
                tail: true,
                pos: None,
                site: NO_SITE,
                // Computed callee: `fb` is staged on the operand stack (the `Global(fb)`
                // at ip 6 above), so `head` is `None`. This is the shape that lowers — the
                // staged callee is exactly what `jit_dispatch_tail` reads back. (Not the
                // KI-19 `staged` flag, which is for a *free-global* head resolved ahead of
                // its args — here there is no head symbol at all.)
                head: None,
                staged: false,
            }, // 15
        ],
    };
    let arm = CompiledArm {
        nrequired: 2,
        nslots: 2,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        ..CompiledArm::default()
    };
    let mut jit = crate::jit::CraneliftBackend::new();
    assert!(
        jit_lower_arm(&mut jit, &arm, &[]).is_some(),
        "an arm ending in a computed-callee tail call (past the body-weight gate) must lower"
    );

    // The *same* 4-work-op arm whose tail call is a **free-global** head (`head:
    // Some(fb)`, elided shape) now lowers successfully: the JIT stages the callee
    // via `globic_ref` before the args (the free-global tail call fix, c99f539).
    let elided = Chunk {
        code: vec![
            Inst::Local(0),                            // 0: n
            Inst::Const(ConstVal::new(Value::int(0))), // 1: 0
            prim2(PrimOp::Eq, "="),                    // 2: n == 0    (work 1)
            Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
            Inst::Local(1),                            // 4: then: acc
            Inst::Jump(15),                            // 5: → done (len)
            Inst::Local(0),                            // 6: else: n (no staged callee — elided)
            Inst::Const(ConstVal::new(Value::int(1))), // 7: 1
            prim2(PrimOp::Sub, "-"),                   // 8: (- n 1) = arg0   (work 2)
            Inst::Local(1),                            // 9: acc
            Inst::Local(1),                            // 10: acc
            prim2(PrimOp::Mul, "*"),                   // 11: (* acc acc)     (work 3)
            Inst::Local(1),                            // 12: acc
            prim2(PrimOp::Mul, "*"),                   // 13: (* … acc) = arg1 (work 4)
            Inst::Call {
                argc: 2,
                tail: true,
                pos: None,
                // A named head always carries its call site (`emit.rs` asserts it).
                site: 0,
                head: Some(fb), // free-global head, elided from the stack
                staged: false,
            }, // 14
        ],
    };
    let elided_arm = CompiledArm {
        nrequired: 2,
        nslots: 2,
        // The elided call carries site 0, so the arm reserves it.
        nsites: 1,
        ngsites: super::ir::gsite_reservation(0, 1),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(elided),
        ..CompiledArm::default()
    };
    assert!(
        jit_lower_arm(&mut jit, &elided_arm, &[]).is_some(),
        "an elided free-global tail call must lower (callee staged via globic_ref, c99f539)"
    );

    // ...and a *thin* SELF-recursive arm with a tail call (2 work ops: `=`, `-`) is
    // gated out (§6.2 `TAIL_CALL_MIN_WORK`) — stays on the VM, where the per-hop
    // native↔driver round-trip would otherwise cost more than it saves. The gate
    // applies only to self-recursive arms (a pure thin delegator lowers fine since
    // outcome-4 follow-through); this chunk is `(defn f (n) (if (= n 0) (f 9)
    // (fb (- n 1))))` — a SelfCall loop whose exit is a thin tail call.
    let thin = Chunk {
        code: vec![
            Inst::Local(0),                            // 0: n
            Inst::Const(ConstVal::new(Value::int(0))), // 1: 0
            prim2(PrimOp::Eq, "="),                    // 2: (= n 0)   (work 1)
            Inst::JumpIfFalse(6),                      // 3: false → else (ip 6)
            Inst::Const(ConstVal::new(Value::int(9))), // 4: then: 9
            Inst::SelfCall { argc: 1 },                // 5: (f 9) — the self loop
            Inst::Global(fb),                          // 6: else: callee `fb`
            Inst::Local(0),                            // 7: n
            Inst::Const(ConstVal::new(Value::int(1))), // 8: 1
            prim2(PrimOp::Sub, "-"),                   // 9: (- n 1)   (work 2)
            Inst::Call {
                argc: 1,
                tail: true,
                pos: None,
                site: NO_SITE,
                head: Some(fb),
                staged: false,
            }, // 10
        ],
    };
    let thin_arm = CompiledArm {
        nrequired: 2,
        nslots: 2,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(thin),
        ..CompiledArm::default()
    };
    assert!(
        jit_lower_arm(&mut jit, &thin_arm, &[]).is_none(),
        "a thin tail-call arm (2 work ops) must be gated out (stays on the VM)"
    );
}

/// JIT Stage-1.5: the **fused** `Prim2Slot*` variants — which `emit_node` actually
/// produces for real loop bodies (`(- i 1)`, `(+ acc i)`, `(< i 1)`) — lower and run.
/// Without them the JIT would never fire on real compiled code. Also pins two
/// correctness rules:
/// `map` (the `>`/swapped-operand case) and overflow → deopt (so the JIT matches the
/// VM's BigInt promotion instead of silently wrapping).
#[cfg(feature = "jit")]
#[test]
fn jit_lowers_fused_prims_map_and_overflow() {
    // All uses here are the `(op Local Const)` form, so `swapped: false`.
    let slot_int =
        |op: PrimOp, map: [u8; 2], slot_a: usize, int_b: i64, head: &str| Inst::Prim2SlotInt {
            op,
            map,
            slot_a,
            int_b,
            swapped: false,
            head: value::intern(head),
            guard: AtomicU64::new(0),
            pos: None,
        };
    let slot_slot = |op: PrimOp, slot_a: usize, slot_b: usize, head: &str| Inst::Prim2SlotSlot {
        op,
        map: [0, 1],
        slot_a,
        slot_b,
        head: value::intern(head),
        guard: AtomicU64::new(0),
        pos: None,
    };
    let mk_arm = |chunk: Chunk, nreq: usize, nslots: usize| CompiledArm {
        nrequired: nreq,
        nslots,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        ..CompiledArm::default()
    };
    let mut jit = crate::jit::CraneliftBackend::new();

    // (a) sumto with the REAL fused shape: `(< i 1)`/`(- i 1)` → Prim2SlotInt,
    // `(+ acc i)` → Prim2SlotSlot. i = slot0, acc = slot1.
    let sumto = mk_arm(
        Chunk {
            code: vec![
                slot_int(PrimOp::Lt, [0, 1], 0, 1, "<"),  // 0: (< i 1)
                Inst::JumpIfFalse(4),                     // 1: false → else
                Inst::Local(1),                           // 2: then: acc
                Inst::Jump(7),                            // 3: → done
                slot_int(PrimOp::Sub, [0, 1], 0, 1, "-"), // 4: (- i 1) = arg0
                slot_slot(PrimOp::Add, 1, 0, "+"),        // 5: (+ acc i) = arg1
                Inst::SelfCall { argc: 2 },               // 6: (sumto arg0 arg1)
            ],
        },
        2,
        2,
    );
    let f: crate::jit::JitArmFn = unsafe {
        std::mem::transmute(jit_lower_arm(&mut jit, &sumto, &[]).expect("fused sumto JITs"))
    };
    crate::process::yield_now(); // prime the reduction budget so the loop completes
    for (n, want) in [(5i64, 15i64), (100, 5050), (1, 1), (0, 0)] {
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(n));
        heap.push_root(Value::int(0));
        let (outcome, ret) = run_arm(f, &mut heap, base);
        assert_eq!(outcome, crate::jit::outcome::DONE, "Done for sumto({n})");
        match ret.unpack() {
            ValueRef::Int(r) => assert_eq!(r, want, "fused sumto({n}, 0)"),
            other => panic!("expected Int, got tag {:?}", value::tag(other)),
        }
    }

    // (b) `map` — `>` lowers to `%lt` with `map = [1, 0]` (operands swapped). The JIT
    // must apply it: `(if (> x 5) 100 200)` is 100 for x=10 and 200 for x=3. Ignoring
    // `map` would compute `x < 5` and flip both answers.
    let gt = mk_arm(
        Chunk {
            code: vec![
                slot_int(PrimOp::Lt, [1, 0], 0, 5, ">"), // 0: (> x 5)  [swapped]
                Inst::JumpIfFalse(4),                    // 1
                Inst::Const(ConstVal::new(Value::int(100))), // 2: then
                Inst::Jump(5),                           // 3
                Inst::Const(ConstVal::new(Value::int(200))), // 4: else
            ],
        },
        1,
        1,
    );
    let g: crate::jit::JitArmFn =
        unsafe { std::mem::transmute(jit_lower_arm(&mut jit, &gt, &[]).expect("(> x 5) JITs")) };
    for (x, want) in [(10i64, 100i64), (3, 200)] {
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(x));
        let (outcome, ret) = run_arm(g, &mut heap, base);
        assert_eq!(outcome, crate::jit::outcome::DONE, "Done for (> {x} 5)");
        match ret.unpack() {
            ValueRef::Int(r) => {
                assert_eq!(r, want, "(if (> {x} 5) 100 200) — map must be applied")
            }
            other => panic!("expected Int, got tag {:?}", value::tag(other)),
        }
    }

    // (c) overflow → deopt. `(* x x)` for a huge x overflows i64; the VM defers such
    // an op to the native, which promotes to a BigInt, so the JIT must deopt (return
    // 1) rather than store a wrapped i64. A non-overflowing x runs to Done (0).
    let sq = mk_arm(
        Chunk {
            code: vec![slot_slot(PrimOp::Mul, 0, 0, "*")],
        },
        1,
        1,
    );
    let s: crate::jit::JitArmFn =
        unsafe { std::mem::transmute(jit_lower_arm(&mut jit, &sq, &[]).expect("(* x x) JITs")) };
    let mut heap = Heap::new();
    let base = heap.roots_len();
    heap.push_root(Value::int(3));
    let (outcome, ret) = run_arm(s, &mut heap, base);
    assert_eq!(outcome, crate::jit::outcome::DONE, "(* 3 3) is in range");
    assert!(matches!(ret.unpack(), ValueRef::Int(9)), "(* 3 3) = 9");
    let mut heap = Heap::new();
    let base = heap.roots_len();
    heap.push_root(Value::int(4_000_000_000)); // 4e9 * 4e9 = 1.6e19 > i64::MAX
    assert_eq!(
        run_arm(s, &mut heap, base).0,
        1,
        "an overflowing (* x x) must deopt to the VM (BigInt), not wrap"
    );
}

/// JIT Stage-1 1b: tiering. An arm invoked past the hotness threshold is compiled
/// once and thereafter runs as native code (`jit_tier` returns `Some(0)` with the
/// result in `roots[base]`); below the threshold it returns `None` (run on the VM).
/// An arm out of the JIT subset is marked BAILED and always returns `None`.
#[cfg(feature = "jit")]
#[test]
fn jit_tier_compiles_a_hot_arm_then_runs_native() {
    // These exercise native tiering directly, so they need ceiling 2 — say so rather than rely
    // on the default, or the tree-walker half of `make test-both` fails them.
    set_forced_ceiling(Some(Tier::Native));

    let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
        op,
        map: [0, 1],
        head: value::intern(head),
        guard: AtomicU64::new(0),
        pos: None,
    };
    // sumto(i acc) = (if (< i 1) acc (sumto (- i 1) (+ acc i))).
    let mk_arm = |chunk: Chunk, nreq: usize, nslots: usize| CompiledArm {
        nrequired: nreq,
        nslots,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        ..CompiledArm::default()
    };
    let sumto = Arc::new(mk_arm(
        Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(1))),
                prim2(PrimOp::Lt, "<"),
                Inst::JumpIfFalse(6),
                Inst::Local(1),
                Inst::Jump(13),
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(1))),
                prim2(PrimOp::Sub, "-"),
                Inst::Local(1),
                Inst::Local(0),
                prim2(PrimOp::Add, "+"),
                Inst::SelfCall { argc: 2 },
            ],
        },
        2,
        2,
    ));

    // A prelude-loaded heap, so `jit_tier`'s operator-validation (`+`/`-`/`<` must
    // still resolve to their natives — the hot-reload guard) sees the live globals; a
    // bare `Heap::new()` has no global env. One heap, reused across poll iterations
    // (truncate the frame each time), keeps the epoch stable so the arm stays tiered.
    let mut interp = crate::Interp::new();
    // Compilation is async (the background compiler thread), so a hot arm returns
    // None until the native pointer is installed. Poll past the threshold, giving the
    // compiler time to land the code, and assert it eventually runs native.
    crate::process::yield_now(); // prime the reduction budget (short loops)
    let mut ran_native = 0;
    // A generous WALL-CLOCK bound, not an iteration deadline: a short budget starves under
    // load, especially in libtest's single shared process, which queues every other test's
    // compiles ahead of this one (KI-74). A compiler that never lands the code still fails,
    // just on a bound a loaded box can meet (KI-43).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        crate::process::yield_now(); // keep the budget topped up across calls
        let base = interp.heap.roots_len();
        interp.heap.push_root(Value::int(5)); // i
        interp.heap.push_root(Value::int(0)); // acc
        let mut ret = Value::Nil;
        let outcome = jit_tier_in_frame(
            &sumto,
            &mut interp.heap,
            base,
            EnvRoot::Stable(EnvId::GLOBAL),
            sumto.nslots,
            &mut ret as *mut Value,
            false,
        );
        match outcome {
            None => {
                interp.heap.truncate_roots(base);
                std::thread::sleep(std::time::Duration::from_millis(2)); // not hot / compile in flight
            }
            Some(0) => {
                ran_native += 1;
                match ret.unpack() {
                    ValueRef::Int(r) => assert_eq!(r, 15, "JIT'd sumto(5,0)"),
                    other => panic!("expected Int(15), got tag {:?}", value::tag(other)),
                }
                interp.heap.truncate_roots(base);
                if ran_native >= 3 {
                    break;
                }
            }
            Some(o) => panic!("unexpected JIT outcome {o}"),
        }
    }
    assert!(ran_native > 0, "the hot arm should tier up to native code");

    // An out-of-subset arm is marked BAILED and never runs native. A coverage-instrumented
    // arm (`RecordLine`) is the one shape the lowering refuses BY DESIGN and always will:
    // native code would run without the recording.
    let bailing = Arc::new(mk_arm(
        Chunk {
            code: vec![Inst::RecordLine(1)],
        },
        0,
        1,
    ));
    // Generous poll cap: under plain `cargo test` (one process, every test
    // sharing the single background compiler thread) the queue ahead of this
    // arm can take seconds. nextest (the canonical runner) isolates per process.
    for _ in 0..5000 {
        let base = interp.heap.roots_len();
        interp.heap.push_root(Value::int(0));
        assert_eq!(
            jit_tier_in_frame(
                &bailing,
                &mut interp.heap,
                base,
                EnvRoot::Stable(EnvId::GLOBAL),
                bailing.nslots,
                &mut Value::Nil as *mut Value,
                false
            ),
            None,
            "out-of-subset arm bails"
        );
        interp.heap.truncate_roots(base);
        if bailing.jit_code.load(std::sync::atomic::Ordering::Acquire) == crate::jit::BAILED {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(
        bailing.jit_code.load(std::sync::atomic::Ordering::Acquire),
        crate::jit::BAILED,
        "out-of-subset arm must settle to BAILED"
    );
}

/// JIT Stage-1 end-to-end: `vm_run_bc`'s hot-path hook runs a tiered arm as native
/// code and returns the same result the interpreter would. Warm the arm past the
/// threshold so it compiles, then invoke it through `vm_run_bc` (fresh start) and
/// check the `Done` value.
#[cfg(feature = "jit")]
#[test]
fn vm_run_bc_runs_a_tiered_arm_via_the_hook() {
    // These exercise native tiering directly, so they need ceiling 2 — say so rather than rely
    // on the default, or the tree-walker half of `make test-both` fails them.
    set_forced_ceiling(Some(Tier::Native));

    let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
        op,
        map: [0, 1],
        head: value::intern(head),
        guard: AtomicU64::new(0),
        pos: None,
    };
    let chunk = Chunk {
        code: vec![
            Inst::Local(0),
            Inst::Const(ConstVal::new(Value::int(1))),
            prim2(PrimOp::Lt, "<"),
            Inst::JumpIfFalse(6),
            Inst::Local(1),
            Inst::Jump(13),
            Inst::Local(0),
            Inst::Const(ConstVal::new(Value::int(1))),
            prim2(PrimOp::Sub, "-"),
            Inst::Local(1),
            Inst::Local(0),
            prim2(PrimOp::Add, "+"),
            Inst::SelfCall { argc: 2 },
        ],
    };
    let arm = Arc::new(CompiledArm {
        nrequired: 2,
        nslots: 2,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        ..CompiledArm::default()
    });

    // Warm it past the threshold so jit_tier hands it to the background compiler;
    // poll until the native pointer is installed (compilation is async now). A
    // prelude-loaded heap, so the operator-validation in `jit_tier` resolves `+`/`-`/`<`.
    use std::sync::atomic::Ordering::Acquire;
    let mut interp = crate::Interp::new();
    crate::process::yield_now();
    let mut tiered = false;
    // Wall-clock bound, not an iteration deadline — same KI-74 hazard as the tiering test
    // above (see its comment).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        crate::process::yield_now();
        let base = interp.heap.roots_len();
        interp.heap.push_root(Value::int(5));
        interp.heap.push_root(Value::int(0));
        let _ = jit_tier_in_frame(
            &arm,
            &mut interp.heap,
            base,
            EnvRoot::Stable(EnvId::GLOBAL),
            arm.nslots,
            &mut Value::Nil as *mut Value,
            false,
        );
        interp.heap.truncate_roots(base);
        let code = arm.jit_code.load(Acquire);
        if !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED {
            tiered = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(tiered, "the arm should have tiered up to native code");

    // Now run it through vm_run_bc — its fresh-start hook should call the native code.
    crate::process::yield_now();
    let outcome = vm_run_bc(
        &mut interp.heap,
        ArmHandle::new(arm),
        &[Value::int(5), Value::int(0)],
        EnvId::GLOBAL,
        None,
        true,
    )
    .expect("vm_run_bc errored");
    match outcome {
        VmOutcome::Done(v) => match v.unpack() {
            ValueRef::Int(n) => assert_eq!(n, 15, "tiered sumto(5,0) via the hook"),
            other => panic!("Done non-int: tag {:?}", value::tag(other)),
        },
        _ => panic!("expected Done(15) from the JIT hook"),
    }
}

/// JIT Stage-1.5: the actual speedup — JIT'd `sumto(N,0)` vs the interpreter, same
/// arm, run through `vm_run_bc`. The VM baseline forces BAILED so its hook stays on
/// the interpreter; the JIT arm is warmed so the hook runs native. Benchmark, not a
/// pass/fail test — run with `--ignored --nocapture`.
#[cfg(feature = "jit")]
#[test]
#[ignore = "benchmark — cargo test -p brood --features jit --lib jit_speedup -- --ignored --nocapture"]
fn jit_speedup_vs_vm() {
    use web_time::Instant;
    let prim2 = |op: PrimOp, head: &str| Inst::Prim2 {
        op,
        map: [0, 1],
        head: value::intern(head),
        guard: AtomicU64::new(0),
        pos: None,
    };
    let mk = || CompiledArm {
        nrequired: 2,
        nslots: 2,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(Chunk {
            code: vec![
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(1))),
                prim2(PrimOp::Lt, "<"),
                Inst::JumpIfFalse(6),
                Inst::Local(1),
                Inst::Jump(13),
                Inst::Local(0),
                Inst::Const(ConstVal::new(Value::int(1))),
                prim2(PrimOp::Sub, "-"),
                Inst::Local(1),
                Inst::Local(0),
                prim2(PrimOp::Add, "+"),
                Inst::SelfCall { argc: 2 },
            ],
        }),
        ..CompiledArm::default()
    };
    let n = 100_000i64; // iterations per sumto call
    let reps = 300;
    // A prelude-loaded heap, reused across reps (vm_run_bc unwinds to entry on Done, so
    // roots stay clean): needed so the JIT tiering hook's operator-validation resolves
    // `+`/`-`/`<`, and so the per-rep cost is the loop, not a prelude load.
    let mut interp = crate::Interp::new();
    let run = |h: &mut Heap, arm: &Arc<CompiledArm>| -> i64 {
        match vm_run_bc(
            h,
            ArmHandle::new(arm.clone()),
            &[Value::int(n), Value::int(0)],
            EnvId::GLOBAL,
            None,
            true,
        )
        .expect("run")
        {
            VmOutcome::Done(v) => match v.unpack() {
                ValueRef::Int(r) => r,
                _ => panic!("bad outcome"),
            },
            _ => panic!("bad outcome"),
        }
    };

    // VM baseline: BAILED forces the hook to stay on the interpreter.
    let vm_arm = Arc::new(mk());
    vm_arm
        .jit_code
        .store(crate::jit::BAILED, std::sync::atomic::Ordering::Release);
    let r0 = run(&mut interp.heap, &vm_arm); // warm caches / verify
    let t = Instant::now();
    for _ in 0..reps {
        assert_eq!(run(&mut interp.heap, &vm_arm), r0);
    }
    let vm = t.elapsed();

    // JIT: warm the arm so the background compiler installs native code, then the
    // hook runs it. Poll until tiered (compilation is async).
    use std::sync::atomic::Ordering::Acquire;
    let jit_arm = Arc::new(mk());
    crate::process::yield_now();
    for _ in 0..1000 {
        let b = interp.heap.roots_len();
        interp.heap.push_root(Value::int(5));
        interp.heap.push_root(Value::int(0));
        let _ = jit_tier_in_frame(
            &jit_arm,
            &mut interp.heap,
            b,
            EnvRoot::Stable(EnvId::GLOBAL),
            jit_arm.nslots,
            &mut Value::Nil as *mut Value,
            false,
        );
        interp.heap.truncate_roots(b);
        let c = jit_arm.jit_code.load(Acquire);
        if !c.is_null() && c != crate::jit::BAILED && c != crate::jit::QUEUED {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let r1 = run(&mut interp.heap, &jit_arm);
    assert_eq!(r1, r0, "JIT must match the VM");
    let t = Instant::now();
    for _ in 0..reps {
        assert_eq!(run(&mut interp.heap, &jit_arm), r1);
    }
    let jit = t.elapsed();

    eprintln!(
        "sumto({n},0) x{reps}: VM {vm:?}  JIT {jit:?}  speedup {:.1}x",
        vm.as_secs_f64() / jit.as_secs_f64().max(1e-9)
    );
}

// ===== KI-26: the fast-link deopt shape check must be flag-free =====

/// A minimal arm carrying just the two frame sizes the shape check reads.
#[cfg(feature = "jit")]
fn ki26_arm(nslots: usize, inline_nslots: usize) -> CompiledArm {
    CompiledArm {
        nrequired: 1,
        nslots,
        uid: 0,
        body: Node::Const(ConstVal::new(Value::int(0))),
        inline_nslots,
        ..CompiledArm::default()
    }
}

#[test]
#[cfg(feature = "jit")]
fn ki26_frame_shape_check_is_independent_of_inline_installed() {
    use std::sync::atomic::Ordering::Release;
    // Small frame 6, leaf-spliced frame 9 — the layout pair a journalled leaf derivation
    // produces (the spliced one is strictly larger; see `leaf_inline_probe`).
    let arm = ki26_arm(6, 9);

    // Either of the arm's own frame sizes is resumable, in BOTH flag states. This is the
    // property the flag form lacked: with the frame built to the small 6 and the flag flipped
    // to true by another process's inline swap, `frame_size_for_new_entry()` returns 9, the old guard
    // declined, and the caller's fallthrough re-ran the arm from ip 0 — repeating whatever
    // effect the native had already journaled.
    for installed in [false, true] {
        arm.inline_installed.store(installed, Release);
        assert!(
            jit_frame_shape_matches(&arm, 6),
            "the small frame must stay resumable with inline_installed={installed}"
        );
        assert!(
            jit_frame_shape_matches(&arm, 9),
            "the spliced frame must stay resumable with inline_installed={installed}"
        );
        // The flag form disagrees with the shape form in exactly one of these states, which
        // is the bug: assert the divergence explicitly so nobody "simplifies" it back.
        if installed {
            assert_ne!(
                arm.frame_size_for_new_entry(),
                6,
                "with the flag set, frame_size_for_new_entry() no longer matches a small frame — the \
                 flag form would decline a resumable frame here"
            );
        }
    }

    // A genuinely foreign arm (the IC re-resolved the site) must still be refused, in both
    // flag states — that is the out-of-bounds protection the check exists for.
    for installed in [false, true] {
        arm.inline_installed.store(installed, Release);
        for foreign in [0, 5, 7, 8, 10, 100] {
            assert!(
                !jit_frame_shape_matches(&arm, foreign),
                "a frame of {foreign} slots belongs to no layout of this arm \
                 (inline_installed={installed})"
            );
        }
    }
}

#[test]
#[cfg(feature = "jit")]
fn ki26_shape_check_admits_everything_the_flag_form_did() {
    use std::sync::atomic::Ordering::Release;
    // The new form must be a strict SUPERSET of the old one — never refusing a frame the
    // flag form accepted — or the fix would trade one silent wrong-resume for another.
    // Includes the degenerate case where the two layouts coincide (an unjournalled leaf
    // derivation, whose `inline_nslots` is floored to the small frame).
    for (n, inl) in [(6usize, 9usize), (4, 4), (1, 32), (12, 19)] {
        let arm = ki26_arm(n, inl);
        for installed in [false, true] {
            arm.inline_installed.store(installed, Release);
            for frame in 0..40usize {
                if arm.frame_size_for_new_entry() == frame {
                    assert!(
                        jit_frame_shape_matches(&arm, frame),
                        "flag form accepted frame {frame} for ({n},{inl}) \
                         installed={installed} but the shape form refused it"
                    );
                }
            }
        }
    }
}

/// The three `JitBackend` tiering advisories must route to the predicate they name.
///
/// This is the guard for ADR-221's one remaining hole: `jit_runtime.rs` used to call straight
/// into the Cranelift backend's unboxed-scalar submodule, and routing those calls through the
/// trait means a delegation could now be wired to the *wrong* one of two similar predicates —
/// `arm_i64_too_deep` (has this fn been demoted?) versus `arm_i64_eligible` (does it take the
/// register worker?). Either swap compiles and passes every other test in the tree: the shared
/// -code path would just quietly stop adopting peers' code, or a demoted function would keep
/// taking an inline upgrade it must not have. Only the *pair* of assertions separates them.
#[cfg(feature = "jit")]
#[test]
fn tiering_advisories_route_to_the_predicate_they_name() {
    use crate::jit::{ActiveBackend, JitBackend};

    // A name of its own: `note_depth_bail` writes a process-global, monotonic set, so a shared
    // name would leak into (or from) another test in this binary.
    let name = value::intern("advisory-probe-fn-adr220");
    let chunk = Chunk {
        code: vec![
            Inst::Local(0),
            Inst::Const(ConstVal::new(Value::int(1))),
            Inst::Prim2 {
                op: PrimOp::Add,
                map: [0, 1],
                head: value::intern("+"),
                guard: AtomicU64::new(0),
                pos: None,
            },
        ],
    };
    let arm = CompiledArm {
        nrequired: 1,
        nslots: 1,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        dbg_name: Some(name),
        ..CompiledArm::default()
    };

    // Nothing demoted yet, so a peer's published code is adoptable...
    assert!(
        ActiveBackend::may_adopt_shared_code(&arm),
        "a fresh arm must be free to adopt shared code"
    );
    // ...and this arm is straight-line, not recursive, so it never takes the register worker and
    // the inline upgrade is never declined for it.
    assert!(
        !ActiveBackend::declines_inline_upgrade(&arm),
        "a non-recursive arm is not scalar-register eligible, so nothing to decline"
    );

    // Outcome 5: the worker ran out of native stack. Adoption must stop — otherwise the next
    // activation reinstalls the very wrapper the demotion exists to escape.
    ActiveBackend::note_depth_bail(name);
    assert!(
        !ActiveBackend::may_adopt_shared_code(&arm),
        "after a depth bail this fn must not adopt shared (possibly register-worker) code"
    );
    // And the *other* advisory must be unmoved by it — wiring it to the depth-bail set instead
    // of to eligibility is exactly the mistake this test exists to catch.
    assert!(
        !ActiveBackend::declines_inline_upgrade(&arm),
        "declines_inline_upgrade keys on scalar eligibility, not on the depth-bail set"
    );
}

/// The tier ladder's ORDER is load-bearing and derived, so nothing else guards it.
///
/// Every consumer compares (`tier_ceiling() >= Tier::Bytecode`, `< Tier::Native`), and those
/// comparisons come from `#[derive(PartialOrd, Ord)]`, i.e. from the order the variants are
/// *declared* in. Reorder the declaration and every comparison silently inverts: a ceiling of
/// `TreeWalk` would start admitting native code and `BROOD_TIER=0` would stop meaning anything.
/// Nothing would fail to compile.
///
/// The invitation to do exactly that is right there in the same `impl`: `Tier::ALL` is written
/// highest-first (`[Native, Bytecode, TreeWalk]`, the order harnesses should present), which reads
/// as though the enum is "backwards" and wants tidying to match. It does not.
#[test]
fn tier_order_is_treewalk_lowest_and_native_highest() {
    use super::Tier;
    assert!(
        Tier::TreeWalk < Tier::Bytecode,
        "tier 0 must be below tier 1"
    );
    assert!(Tier::Bytecode < Tier::Native, "tier 1 must be below tier 2");
    assert!(
        Tier::TreeWalk < Tier::Native,
        "and the ladder must be transitive"
    );

    // The comparisons the call sites actually make, spelled out so a reordering fails HERE with
    // a message rather than as a mystery in the scheduler or the JIT.
    assert!(
        Tier::Native >= Tier::Bytecode && Tier::Bytecode >= Tier::Bytecode,
        "ceilings 2 and 1 must both admit the bytecode VM (`run_top_form`, `apply_engine`)"
    );
    // Spelled as the negation of the call site's own `>=` rather than as `<`: the point is to
    // mirror the comparison the code actually makes, so a reordering fails against the real
    // expression. Clippy would rather see `<`, which is equivalent and loses that.
    #[allow(clippy::nonminimal_bool)]
    {
        assert!(
            !(Tier::TreeWalk >= Tier::Bytecode),
            "ceiling 0 must NOT admit the bytecode VM"
        );
    }
    assert!(
        Tier::TreeWalk < Tier::Native && Tier::Bytecode < Tier::Native,
        "ceilings 0 and 1 must both refuse native tiering (`jit_tier`)"
    );

    // `ALL` is a presentation order, deliberately the reverse of the ladder. Assert both facts
    // so neither can be "fixed" into the other.
    assert_eq!(
        Tier::ALL,
        &[Tier::Native, Tier::Bytecode, Tier::TreeWalk],
        "ALL is highest-first for harness presentation; the enum stays lowest-first for Ord"
    );
    let mut sorted = Tier::ALL.to_vec();
    sorted.sort();
    assert_eq!(sorted, vec![Tier::TreeWalk, Tier::Bytecode, Tier::Native]);
}

/// KI-40 guard: cloning the per-call [`ArmHandle`] must NOT touch the shared
/// [`CompiledArm`]'s refcount.
///
/// This is the invariant the whole fix rests on, and nothing else in the tree observes
/// it. ADR-175 Phase B publishes a compiled arm to `Runtime::shared_closures`, so every
/// green process's inline cache points at ONE allocation; the VM then clones that handle
/// up to three times per call (the IC probe, the `BcFrame`, `live_arm_push`). If those
/// clones land on the *shared* `Arc`, N worker threads RMW one cache line per call and
/// parallel code runs several times slower — while still computing the right answer, so `make test`, the
/// differential and the lowering witness all stay green and **only a benchmark moves**
/// (the same blind spot ADR-221 hit). Hence an assertion on the refcount itself.
#[test]
fn arm_handle_clone_does_not_touch_the_shared_arm_refcount() {
    let shared: Arc<CompiledArm> = Arc::new(CompiledArm {
        nrequired: 1,
        nslots: 1,
        ..CompiledArm::default()
    });

    // One process installs the arm into its IC: exactly one shared-refcount bump, ever.
    let handle = ArmHandle::new(shared.clone());
    let shared_after_install = Arc::strong_count(&shared);
    assert_eq!(
        shared_after_install, 2,
        "installing a handle should take exactly one reference to the shared arm"
    );

    // Now simulate the hot path: many per-call clones of the handle.
    let clones: Vec<Arc<ArmHandle>> = (0..1000).map(|_| handle.clone()).collect();

    assert_eq!(
        Arc::strong_count(&shared),
        shared_after_install,
        "per-call handle clones must not touch the SHARED arm's refcount — that \
         contended cache line is KI-40; this is the assertion that catches a revert \
         to cloning `Arc<CompiledArm>` on the call path"
    );
    assert_eq!(
        Arc::strong_count(&handle),
        1001,
        "the clones should land on the process-local handle instead"
    );

    // ...and that the handle still resolves to the same arm it wraps.
    assert!(std::ptr::eq(
        Arc::as_ptr(handle.arc()),
        Arc::as_ptr(&shared)
    ));
    drop(clones);
    assert_eq!(Arc::strong_count(&handle), 1);
}

/// The sibling of the guard above: **resolving** an arm must not allocate a handle per call.
///
/// A computed head takes no inline cache (see the call arm in `exec_chunk`), so `dispatch`,
/// `exec_chunk` and the JIT's non-elided resolve all reach `compiled_arm_for` on *every*
/// call — per element of a transducer chain, per message handled, per callback invoked.
/// While that returned a bare `Arc<CompiledArm>` for each caller to wrap, every such call
/// paid an `Arc::new` **and** a clone of the shared arm's `Arc` — an atomic RMW on the one
/// cross-process refcount cache line KI-40 is about. `compiled_arm_for` now hands back the
/// handle memoized in the `vm_cache` entry instead.
///
/// This needs its own assertion for the same reason KI-40 did: the VM keeps computing the
/// right answer either way, so `make test`, `make test-both` and the lowering witness all
/// stay green and **only a benchmark moves** (`pipeline` +5.7%, bisected to `98e97308`).
#[test]
fn resolving_a_closure_arm_twice_reuses_one_memoized_handle() {
    let mut interp = crate::Interp::new();
    interp.eval_str("(def f (fn (x) (+ x 1)))").expect("define");
    let f = interp.eval_str("f").expect("read f back");
    let id = match f.unpack() {
        ValueRef::Fn(id) => id,
        _ => panic!("`f` should be a closure"),
    };
    let heap = &interp.heap;

    let first = compiled_arm_for(heap, id, 1).expect("f/1 compiles");
    let second = compiled_arm_for(heap, id, 1).expect("f/1 compiles");
    assert!(
        Arc::ptr_eq(&first, &second),
        "a second resolution must reuse the memoized handle, not allocate another one"
    );

    // The hot path: many resolutions, every result held so nothing can be recycled into
    // the same address and fake a pass.
    let shared_before = Arc::strong_count(first.arc());
    let repeats: Vec<Arc<ArmHandle>> = (0..1000)
        .map(|_| compiled_arm_for(heap, id, 1).expect("f/1 compiles"))
        .collect();
    assert!(
        repeats.iter().all(|h| Arc::ptr_eq(h, &first)),
        "every resolution of the same (closure, argc) should be the one memoized handle"
    );
    assert_eq!(
        Arc::strong_count(first.arc()),
        shared_before,
        "resolving a computed-head callee must not touch the SHARED arm's refcount — that \
         contended cache line is KI-40, and this is the assertion that catches a revert to \
         deriving the handle per call"
    );
}

/// **The gate the KI-58 class was missing.** Every call-site inline is keyed on what a head
/// *resolves to*, and nothing asserted that it still resolves — so a rename retires an inline
/// silently: the program stays correct, the checker stays clean, and the only symptom is a
/// benchmark row nobody runs per commit. It has happened three times (KI-42 `sqrt` moving,
/// KI-44 the `sqrt` inline dying with it, KI-58 `table-put` behind a `table/` wrapper).
///
/// So: one table of the spellings a *program actually writes*, asserted to still reach their
/// primitive. A future namespacing wave fails here instead of costing 11x on a row.
///
/// This deliberately checks the user-facing names (`+`, `nth`, `table/get`), not the `%`-prefixed
/// natives — the natives are what the resolver bottoms out at, and testing those would pass
/// while every real call site had stopped inlining, which is precisely the failure being guarded.
#[test]
fn every_inlinable_head_still_reaches_its_primitive() {
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(require-one 'table)")
        .expect("load std/table");

    // 2-ary: the arithmetic/compare heads a program writes, plus the table reads. `+`/`-`/`*`
    // are variadic Brood `defn`s, so these also pin that `resolve_prim` still follows a
    // thin wrapper to the native underneath.
    for (head, want) in [
        ("+", PrimOp::Add),
        ("-", PrimOp::Sub),
        ("*", PrimOp::Mul),
        ("cons", PrimOp::Cons),
        // `%max`/`%min`, not `max`/`min`: those are `math/max`/`math/min`, and the KERNEL
        // halves carry the `%` prefix so the prelude can do arithmetic without loading a
        // module. `resolve_prim` keys on the call-site
        // name, so this is the name a hot loop actually writes.
        ("%max", PrimOp::Max),
        ("%min", PrimOp::Min),
        ("%rem", PrimOp::Rem),
        ("bit/and", PrimOp::BitAnd),
        ("bit/or", PrimOp::BitOr),
        ("bit/xor", PrimOp::BitXor),
        ("nth", PrimOp::VectorRef),
        // The native itself, by its CURRENT name: a stale entry makes every element read
        // the pattern compiler emits a full native call, hidden by `nth`'s inline above.
        ("%vector-ref", PrimOp::VectorRef),
        ("get", PrimOp::MapGet),
        ("table/get", PrimOp::TableGet),
        ("table/has?", PrimOp::TableHas),
    ] {
        assert_eq!(
            resolve_prim(&interp.heap, value::intern(head)).map(|(op, _)| op),
            Some(want),
            "`{head}` no longer inlines to {want:?} — a rename or rewording retired the \
             call-site inline; the program is still correct, which is why only this test fails"
        );
    }

    // 1-ary. `resolve_prim1` accepts only a DIRECT native binding (plus the structural `sqrt`
    // probe), so any of these gaining a wrapper would silently stop inlining — the KI-58 shape.
    for (head, want) in [
        ("first", PrimOp1::First),
        ("rest", PrimOp1::Rest),
        ("nil?", PrimOp1::IsNil),
        ("pair?", PrimOp1::IsPair),
        ("empty?", PrimOp1::IsEmpty),
        ("type-of", PrimOp1::TypeOf),
        ("%vector-length", PrimOp1::VectorLen),
        // The type predicates are prelude WRAPPERS over `type-of`, recognised by shape
        // (`type_predicate_tag`), so a rewording of one is a silent de-inline — exactly the
        // KI-58 class, and these are what every `match`/`receive` clause tests first.
        ("vector?", PrimOp1::TypeIs(value::intern("vector"))),
        ("map?", PrimOp1::TypeIs(value::intern("map"))),
        ("string?", PrimOp1::TypeIs(value::intern("string"))),
        ("int?", PrimOp1::TypeIs(value::intern("int"))),
        ("keyword?", PrimOp1::TypeIs(value::intern("keyword"))),
        ("set?", PrimOp1::TypeIs(value::intern("set"))),
        ("pid?", PrimOp1::TypeIs(value::intern("pid"))),
    ] {
        assert_eq!(
            resolve_prim1(&interp.heap, value::intern(head)),
            Some(want),
            "`{head}` no longer inlines to {want:?} — see the note above"
        );
    }

    // 3-ary.
    assert_eq!(
        resolve_prim3(&interp.heap, value::intern("table/put")),
        Some(PrimOp3::TablePut)
    );
    // And the one that is name-gated rather than binding-gated (KI-44).
    interp.eval_str("(require-one 'math)").expect("load math");
    assert_eq!(
        resolve_prim1(&interp.heap, value::intern("math/sqrt")),
        Some(PrimOp1::Sqrt)
    );
}

/// The type-predicate inline is earned by SHAPE, never by name: a user function that merely
/// ends in `?` — a guarded predicate, a two-way test, a predicate over a different function —
/// compiles as an ordinary call. Every one of these would be a wrong answer if inlined as a
/// tag compare, so the probe must refuse them all. And a predicate the user REBINDS to a
/// different shape must stop inlining at once (the epoch guard re-runs the probe).
#[test]
fn type_predicate_inline_is_structural_not_nominal() {
    let mut interp = crate::Interp::new();
    interp
        .eval_str(
            "(do
               (defn even-ish? (x) (= (math/rem x 2) 0))
               (defn wide? (x) (or (int? x) (float? x)))
               (defn kind? (x) (%eq (type-of x) :int))
               (defn two? (x y) (%eq (type-of x) y))
               (defn guarded? (x) (if (nil? x) false (%eq (type-of x) :map))))",
        )
        .expect("defs");
    // `fn?` is the prelude's own two-way test — `(or (%eq (type-of x) :fn) (%eq (type-of x)
    // :native))` — and must refuse for the same reason `wide?` does.
    for head in ["even-ish?", "wide?", "two?", "guarded?", "fn?"] {
        assert_eq!(
            resolve_prim1(&interp.heap, value::intern(head)),
            None,
            "`{head}` is not a type predicate and must not inline as one"
        );
    }
    // The canonical shape under a user's own name DOES qualify — the shape is the contract.
    assert_eq!(
        resolve_prim1(&interp.heap, value::intern("kind?")),
        Some(PrimOp1::TypeIs(value::intern("int")))
    );
    // …and a rebind away from the shape drops it.
    interp
        .eval_str("(defn kind? (x) (or (int? x) (float? x)))")
        .expect("rebind");
    assert_eq!(resolve_prim1(&interp.heap, value::intern("kind?")), None);
}

/// The 3-arg `table-put` call-site inline survives the namespacing that turned it into a
/// `std/table.blsp` wrapper: if `resolve_prim3` stops following the wrapper nothing errors —
/// the call just becomes an ordinary `Call` in the hot arm (an order of magnitude on
/// `sieve`).
///
/// The second half is the miscompile guard, and it is the reason the identity map is
/// *required* rather than applied: `Node::Prim3` carries no argument permutation (unlike
/// `Node::Prim2`'s `map`), so a wrapper that reorders its parameters must decline. Inlining
/// one would silently store the value under the wrong key.
/// The two map ops (ADR-368) inline by HEAD — `get` at three arguments and `assoc` — only
/// while the head is the PRELUDE closure, exactly as `resolve_prim`'s `get`/`nth` do. If
/// either stops resolving here, nothing errors: every record read-with-default and every
/// record update is a full call again, which is the KI-58 shape this pin exists for.
#[test]
fn map_prim3_call_sites_inline_the_prelude_get_and_assoc() {
    let interp = crate::Interp::new();
    assert_eq!(
        resolve_prim3(&interp.heap, value::intern("get")),
        Some(PrimOp3::MapGet3),
        "`get` at three arguments no longer inlines to PrimOp3::MapGet3"
    );
    assert_eq!(
        resolve_prim3(&interp.heap, value::intern("assoc")),
        Some(PrimOp3::MapAssoc),
        "`assoc` no longer inlines to PrimOp3::MapAssoc"
    );
}

#[test]
fn table_put_call_site_inline_recognizes_the_namespaced_wrapper() {
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(require-one 'table)")
        .expect("load std/table");
    assert_eq!(
        resolve_prim3(&interp.heap, value::intern("table/put")),
        Some(PrimOp3::TablePut),
        "the canonical table/put wrapper must inline to PrimOp3::TablePut — if this fails,          std/table's put was reworded out of the recognized shape and sieve is ~11x slower"
    );
    // The underlying native still resolves directly, as it always did.
    assert_eq!(
        resolve_prim3(&interp.heap, value::intern("%table-put")),
        Some(PrimOp3::TablePut)
    );
    // A 3-arg wrapper that forwards to something else entirely must not inline.
    interp
        .eval_str("(defmodule ut) (defn ut/put (t k v) (list t k v))")
        .expect("define a user ut/put");
    assert!(
        matches!(
            interp
                .eval_str("ut/put")
                .expect("read ut/put back")
                .unpack(),
            ValueRef::Fn(_)
        ),
        "ut/put must be a bound closure, or the None below passes for the wrong reason"
    );
    assert_eq!(resolve_prim3(&interp.heap, value::intern("ut/put")), None);
    // The miscompile guard: a wrapper that PERMUTES its parameters before forwarding must
    // decline, because `Node::Prim3` would compile the arguments in the caller's order and
    // store the value under the wrong key.
    interp
        .eval_str("(defmodule up) (defn up/put (t k v) (%table-put t v k))")
        .expect("define a permuting wrapper");
    assert_eq!(
        resolve_prim3(&interp.heap, value::intern("up/put")),
        None,
        "a wrapper that swaps k and v must not inline — Prim3 has no permutation to apply"
    );
}

/// KI-44: the `sqrt` call-site inline survives its move out of the prelude (ADR-227). The head
/// is now the qualified `math/sqrt`, a RUNTIME closure, so `resolve_prim1` identifies the
/// canonical wrapper STRUCTURALLY rather than by the old sealed-PRELUDE identity. This guards
/// both halves at once: the canonical wrapper still resolves to `PrimOp1::Sqrt` (so the
/// `nbody` win is not silently lost the next time `std/math`'s `sqrt` is reworded), and a user's
/// OWN `…/sqrt` — which the `/sqrt` name-gate also matches — does NOT inline unless it is
/// genuinely the `%f64-sqrt` wrapper. The latter is the real miscompile guard: the x>0 shortcut
/// returns `f64::sqrt(x)`, so inlining a `foo/sqrt` that computes something else would be wrong.
/// (`math/sqrt` itself is a reserved name and cannot be rebound — E0030 — so it stays canonical.)
#[test]
fn sqrt_call_site_inline_recognizes_the_moved_math_wrapper() {
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(require-one 'math)")
        .expect("load std/math");
    assert_eq!(
        resolve_prim1(&interp.heap, value::intern("math/sqrt")),
        Some(PrimOp1::Sqrt),
        "the canonical math/sqrt wrapper must inline to PrimOp1::Sqrt — if this fails, \
         std/math's sqrt was reworded out of the recognized shape and the inline (KI-44) is dead"
    );
    // A bare `sqrt` moved out of the prelude, so it is unbound and must not inline.
    assert_eq!(resolve_prim1(&interp.heap, value::intern("sqrt")), None);
    // A user's own `…/sqrt` that is NOT the `%f64-sqrt` wrapper must NOT inline — otherwise the
    // x>0 shortcut would return `f64::sqrt(x)` where this function returns `n*n`.
    interp
        .eval_str("(defmodule usersq) (defn usersq/sqrt (n) (* n n))")
        .expect("define a user usersq/sqrt");
    // Guard against the None below passing for the wrong reason (an unbound name): it must be
    // bound to a real closure, and *still* decline to inline.
    assert!(
        matches!(
            interp
                .eval_str("usersq/sqrt")
                .expect("read usersq/sqrt back")
                .unpack(),
            ValueRef::Fn(_)
        ),
        "usersq/sqrt must be a bound closure"
    );
    assert_eq!(
        resolve_prim1(&interp.heap, value::intern("usersq/sqrt")),
        None,
        "a user `usersq/sqrt` computing n*n must not inline as sqrt (it isn't the %f64-sqrt wrapper)"
    );
}

/// KI-48: the caller sizes the frame, then `jit_tier` re-loads the code
/// pointer — and a peer can swap the inlined body in between (`CompiledArm` is shared across
/// a runtime's processes since ADR-215). Running the inlined native against the small frame
/// is a raw write past the frame top.
///
/// The guard is `jit_tier_in_frame`: the caller states the size the frame was BUILT to, and
/// the native entry is declined when the installed code wants a bigger one. This test stages
/// exactly that state — `jit_code == inline_code`, `inline_nslots > nslots` — and asserts
/// `None` (interpret this activation).
///
/// The "native" here is a real function returning outcome 0, so a regression fails as a clean
/// `Some(0)` rather than by jumping into garbage.
#[cfg(feature = "jit")]
#[test]
fn jit_tier_declines_the_inlined_body_when_the_frame_was_built_small() {
    extern "C" fn fake_native(_heap: *mut Heap, _base: i64) -> i64 {
        0 // Done, with the result already in roots[base]
    }
    let mut interp = crate::Interp::new();
    let heap = &mut interp.heap;
    let code = fake_native as *mut u8;
    let chunk = Chunk {
        code: vec![Inst::Local(0)],
    };
    let arm = Arc::new(CompiledArm {
        nrequired: 1,
        nslots: 2,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        // The state a peer's inline swap leaves behind: the inlined pointer installed…
        jit_code: std::sync::atomic::AtomicPtr::new(code),
        compile_epoch: std::sync::atomic::AtomicU64::new(heap.global_epoch()),
        shared_published: std::sync::atomic::AtomicBool::new(true),
        inline_name: Some(value::intern("peer-inlined")),
        dbg_name: Some(value::intern("peer-inlined")),
        inline_stride: 2,
        // …wanting a frame six slots bigger than the one the caller built.
        inline_nslots: 8,
        inline_code: std::sync::atomic::AtomicPtr::new(code),
        inline_queued: std::sync::atomic::AtomicBool::new(true),
        inline_installed: std::sync::atomic::AtomicBool::new(true),
        ..CompiledArm::default()
    });

    let base = heap.roots_len();
    heap.push_root(Value::int(1));
    heap.push_root(Value::nil()); // frame built to the SMALL nslots = 2
    let env = heap.root_env(heap.global());

    assert_eq!(
        jit_tier_in_frame(
            &arm,
            heap,
            base,
            env,
            arm.nslots,
            &mut Value::Nil as *mut Value,
            false
        ),
        None,
        "a frame built to `nslots` must not run an `inline_nslots` native — it raw-writes \
         past the frame top (KI-48 family). Interpret this activation instead."
    );
    // The same arm entered with a frame that IS big enough still runs natively — the guard
    // must decline on size, not on the mere presence of an inlined body.
    //
    // Gated on the tier ceiling (ADR-222). Under the `differential (tree-walker)` CI job
    // (`BROOD_TIER=0`) — and under `BROOD_TIER=1` —
    // `jit_tier_in_frame` correctly refuses to run *any* native, so this half would fail for
    // a reason that has nothing to do with what it tests. The half above still runs at every
    // ceiling: declining is the safe answer, so it is the assertion that must hold always.
    if crate::eval::compile::tier_ceiling() == crate::eval::compile::Tier::Native {
        heap.extend_roots_to_nil(base + arm.inline_nslots);
        assert_eq!(
            jit_tier_in_frame(
                &arm,
                heap,
                base,
                env,
                arm.inline_nslots,
                &mut Value::Nil as *mut Value,
                false
            ),
            Some(0),
            "an `inline_nslots` frame is the layout the inlined native wants; it must run"
        );
    }
    heap.truncate_roots(base);
}

/// The LOCAL vector-base hoist (`brood_rt_vector_base` → inline `ptr + idx*STRIDE` reads) must
/// be OFF in any arm that can allocate. Nothing refreshes that pointer, and a small vector's
/// elements live inline in the `local.vectors` slab slot — a push that reallocates the slab
/// moves them, and every later read then reads freed memory. Garbage `Value` words with
/// valid-looking tags, invisible to the per-deref tripwire (the read bypasses `Heap`).
///
/// The gate must cover every op `inst_may_allocate` lists, the table ops included (the same
/// omission produced a use-after-free on the pair path). This pins the predicate itself, which the end-to-end shape cannot: a stale
/// read returns *freed-but-intact* memory far more often than it returns garbage, so a
/// behavioural test passes with the bug live (verified: `tests/jit_vector_hoist_alloc_test.blsp`
/// passes either way).
///
/// Sabotage-verified: reverting `vector_base_hoist_safe` to the old
/// `Call`/`MakeVector`/`Cons` list fails the `table-get` and `table-put` cases.
#[cfg(feature = "jit")]
#[test]
fn the_vector_base_hoist_is_off_for_any_allocating_arm() {
    use super::jit_plan::codegen::vector_base_hoist_safe;
    let vref = || Inst::Prim2SlotSlot {
        op: PrimOp::VectorRef,
        map: [0, 1],
        slot_a: 0,
        slot_b: 1,
        head: value::intern("vector-ref"),
        guard: AtomicU64::new(0),
        pos: None,
    };
    let prim2 = |op: PrimOp, name: &str| Inst::Prim2 {
        op,
        map: [0, 1],
        head: value::intern(name),
        guard: AtomicU64::new(0),
        pos: None,
    };

    // The lever itself: a fused vector read (plus arithmetic) still hoists.
    assert!(
        vector_base_hoist_safe(&[vref(), prim2(PrimOp::Add, "+")]),
        "a pure indexed read loop is exactly what the hoist exists for"
    );

    // `table-get` reconstructs a compound stored value into the caller's heap — it allocates.
    assert!(
        !vector_base_hoist_safe(&[vref(), prim2(PrimOp::TableGet, "table-get")]),
        "a table-get can reallocate the vector slab under the hoisted base"
    );
    assert!(
        !vector_base_hoist_safe(&[vref(), prim2(PrimOp::TableHas, "table-has?")]),
        "table-has? takes the same hashed reconstruction path"
    );
    // `table-put` (a `Prim3`), and the two builders `hoist_safe` never covered either.
    assert!(
        !vector_base_hoist_safe(&[
            vref(),
            Inst::Prim3 {
                op: super::ir::PrimOp3::TablePut,
                head: value::intern("table-put"),
                guard: AtomicU64::new(0),
                pos: None,
            }
        ]),
        "a table-put allocates (it deep-copies key and value)"
    );
    assert!(
        !vector_base_hoist_safe(&[vref(), Inst::MakeMap(2)]),
        "building a map allocates"
    );

    // The pre-existing exclusions must still hold.
    assert!(
        !vector_base_hoist_safe(&[vref(), Inst::MakeVector(2)]),
        "building a vector allocates"
    );
    assert!(
        !vector_base_hoist_safe(&[vref(), prim2(PrimOp::Cons, "cons")]),
        "cons allocates"
    );
    assert!(
        !vector_base_hoist_safe(&[
            vref(),
            Inst::Call {
                argc: 1,
                tail: false,
                pos: None,
                site: NO_SITE,
                head: None,
                staged: false,
            }
        ]),
        "a non-tail call is a GC safepoint (and can `def` → RUNTIME compaction)"
    );
}

/// A `letrec`-bound loop's tail self-call is a `SelfCall`, not a call through the
/// captured closure. `#3 lexical addressing` binds every capture to a frame slot — and the
/// self-name IS a capture (the frame binds it to the closure) — so the self-call rule's
/// "not shadowed by a local" check must admit the self-name's own capture slot, or every
/// letrec loop runs as a slow call through the closure. A `let` that rebinds the name
/// inside the body still declines.
#[cfg(feature = "jit")]
#[test]
fn a_letrec_loop_is_a_self_call_and_a_shadowed_name_is_not() {
    // Under `BROOD_TIER=0` (CI's tree-walker job) there is no VM arm to inspect: force the
    // ceiling, as the tiering test below does.
    set_forced_ceiling(Some(Tier::Native));
    let mut interp = crate::Interp::new();
    let chunk_of = |interp: &mut crate::Interp, src: &str| -> Vec<String> {
        let f = interp.eval_str(src).expect("build the closure");
        let id = match f.unpack() {
            crate::core::value::ValueRef::Fn(id) => id,
            other => panic!("not a closure: {other:?}"),
        };
        let handle = super::closure::compiled_arm_for(&interp.heap, id, 2).expect("a VM arm");
        let arm = handle.arc().clone();
        arm.chunk
            .as_ref()
            .expect("a flat chunk")
            .code
            .iter()
            .map(|i| super::jit_plan::codegen::inst_opcode_name(i).to_string())
            .collect()
    };
    // The plain loop, with a captured `n`: the back-edge is a SelfCall.
    let plain = chunk_of(
        &mut interp,
        "(let (n 10) (letrec (lp (fn (i acc) (if (>= i n) acc (lp (+ i 1) (+ acc i))))) lp))",
    );
    assert!(
        plain.iter().any(|i| i.starts_with("SelfCall")),
        "no SelfCall in the letrec loop: {plain:?}"
    );
    assert!(
        !plain.iter().any(|i| i.starts_with("Call")),
        "the letrec loop still calls through its capture: {plain:?}"
    );
    // A `let` inside the body rebinding the loop's own name: that call is the shadow's.
    let shadowed = chunk_of(
        &mut interp,
        "(letrec (lp (fn (i acc) (if (>= i 3) acc (let (lp (fn (a b) :inner)) (lp (+ i 1) acc))))) lp)",
    );
    assert!(
        !shadowed.iter().any(|i| i.starts_with("SelfCall")),
        "a shadowed self-name became a SelfCall: {shadowed:?}"
    );
    // …and it means what it says at runtime.
    let v = interp
        .eval_str("(letrec (lp (fn (i acc) (if (>= i 3) acc (let (lp (fn (a b) :inner)) (lp (+ i 1) acc))))) (lp 0 0))")
        .expect("run");
    assert!(
        matches!(v.unpack(), crate::core::value::ValueRef::Keyword(k) if crate::core::value::symbol_is(k, "inner")),
        "the shadowing let must win: {v:?}"
    );
    // The loop runs to its answer with the captured bound.
    let v = interp
        .eval_str("(let (n 100000) (letrec (lp (fn (i acc) (if (>= i n) acc (lp (+ i 1) (+ acc i))))) (lp 0 0)))")
        .expect("run");
    assert!(
        matches!(v.unpack(), crate::core::value::ValueRef::Int(4_999_950_000)),
        "{v:?}"
    );
}

/// ADR-333: a self-tail loop is elected for tiering within its FIRST activation, whatever
/// `TIER_THRESHOLD` says. The call threshold is 128, but a loop reaches the compiler through
/// the back-edge boundary exits (every 256 iterations), each worth `BACKEDGE_TIER_WEIGHT`
/// calls — so eight boundaries, 2048 iterations, elect it. This runs the loop through the
/// real entry point (`eval_str` → the VM → `exec_chunk`'s boundary → `jit_tier_in_frame`)
/// for 3000 iterations and asserts the arm left the untried state. Sabotage-verified: with
/// the weight at 1 the same loop has counted 11 of 128 and `jit_code` is still null.
#[test]
#[cfg(feature = "jit")]
fn a_self_tail_loop_is_elected_for_tiering_within_one_activation() {
    use std::sync::atomic::Ordering::Acquire;
    set_forced_ceiling(Some(Tier::Native));
    let mut interp = crate::Interp::new();
    interp
        .eval_str("(defn tier-loop (i acc) (if (< i 1) acc (tier-loop (- i 1) (+ acc i))))")
        .expect("define the loop");
    let f = interp.eval_str("tier-loop").expect("read the loop back");
    let id = match f.unpack() {
        crate::core::value::ValueRef::Fn(id) => id,
        other => panic!("tier-loop is not a closure: {other:?}"),
    };
    let handle = super::closure::compiled_arm_for(&interp.heap, id, 2).expect("a VM arm");
    let arm = handle.arc().clone();
    assert!(
        arm.jit_code.load(Acquire).is_null() && arm.jit_calls.load(Acquire) == 0,
        "the arm must start untried"
    );
    let got = interp
        .eval_str("(tier-loop 3000 0)")
        .expect("run the loop once");
    match got.unpack() {
        crate::core::value::ValueRef::Int(r) => assert_eq!(r, 4_501_500),
        other => panic!("tier-loop returned {other:?}"),
    }
    let calls = arm.jit_calls.load(Acquire);
    let code = arm.jit_code.load(Acquire);
    assert!(
        !code.is_null() || calls >= super::jit_runtime::TIER_THRESHOLD,
        "one 3000-iteration activation must elect the loop for tiering (11 boundary exits × \
         BACKEDGE_TIER_WEIGHT ≥ TIER_THRESHOLD), but jit_calls={calls} and jit_code is null"
    );
}

/// A native loop's PREEMPT must capture the loop's own frame at ip 0, not its callee's.
///
/// Handed to the interpreter "until its loop-top notices the spent budget", a preempted
/// native loop's first safepoint is inside the callee's entry, so the capture lands on the
/// callee at ip 0 and the resume returns into the loop MID-BODY, interpreting up to 256
/// iterations before its back-edge re-tiers it (see `vm_run_bc`'s outcome-2 arm). This drives the real capture-mode driver through
/// the real entry point with a 300-reduction budget and asserts that once the loop is native
/// every capture is the loop's frame at ip 0. Sabotage-verified: with the outcome-2 capture
/// removed, the captures land on `pre-g`.
#[cfg(feature = "jit")]
#[test]
fn a_native_preempt_captures_the_loop_frame_not_its_callee() {
    use std::sync::atomic::Ordering::Acquire;
    set_forced_ceiling(Some(Tier::Native));
    let mut interp = crate::Interp::new();
    // The callee is too big to leaf-splice (so the loop keeps a real call) and trivially
    // lowerable; the loop is a self-tail accumulator with that one non-tail call.
    interp
        .eval_str(
            "(defn pre-g (x) (cond (< x 0) (- 0 x) (< x 10) (+ x 1) (< x 100) (+ x 2) \
             (< x 1000) (+ x 3) (< x 10000) (+ x 4) (< x 100000) (+ x 5) \
             (< x 1000000) (+ x 6) else (+ x 7)))",
        )
        .expect("define pre-g");
    interp
        .eval_str("(defn pre-f (i acc) (if (< i 1) acc (pre-f (- i 1) (pre-g (- acc 7)))))")
        .expect("define pre-f");
    let f_id = match interp.eval_str("pre-f").expect("read pre-f back").unpack() {
        crate::core::value::ValueRef::Fn(id) => id,
        other => panic!("pre-f is not a closure: {other:?}"),
    };
    let f_arm = super::closure::compiled_arm_for(&interp.heap, f_id, 2)
        .expect("a VM arm for pre-f")
        .arc()
        .clone();
    let installed = |arm: &CompiledArm| {
        let c = arm.jit_code.load(Acquire);
        !c.is_null() && c != crate::jit::BAILED && c != crate::jit::QUEUED
    };
    // Stand in for the scheduler: a capture-mode run with a small budget, refilled on
    // every resume exactly as `run_one` does, and a process waiting for the worker (with
    // none, a spent budget does not unwind — ADR-401).
    crate::process::set_capture_run(true);
    crate::process::test_work_waiting(Some(true));
    crate::process::set_reduction_budget_for_test(300);
    let heap = &mut interp.heap;
    let mut outcome = vm_run_bc(
        heap,
        ArmHandle::new(f_arm.clone()),
        &[Value::int(300_000), Value::int(0)],
        EnvId::GLOBAL,
        None,
        true,
    )
    .expect("run pre-f");
    let mut captures_after_native = 0u32;
    let mut misplaced: Vec<(String, usize)> = Vec::new();
    loop {
        match outcome {
            VmOutcome::Done(v) => {
                assert!(
                    matches!(v.unpack(), crate::core::value::ValueRef::Int(_)),
                    "pre-f must finish with an int"
                );
                break;
            }
            VmOutcome::Preempted(s) => {
                let s = s.into_vm();
                if installed(&f_arm) {
                    captures_after_native += 1;
                    let cur = s.cur.arm.arc();
                    if cur.uid != f_arm.uid || s.cur.ip != 0 {
                        misplaced.push((
                            cur.dbg_name
                                .map(crate::core::value::symbol_name_ref)
                                .unwrap_or("<closure>")
                                .to_string(),
                            s.cur.ip,
                        ));
                    }
                }
                crate::process::set_reduction_budget_for_test(300);
                let arm = s.cur.arm.clone();
                outcome =
                    vm_run_bc(heap, arm, &[], EnvId::GLOBAL, Some(s), true).expect("resume pre-f");
            }
            VmOutcome::Suspended(_) => panic!("pre-f suspended — it has no receive"),
            VmOutcome::Killed => panic!("pre-f was killed"),
        }
    }
    crate::process::test_work_waiting(None);
    crate::process::set_capture_run(false);
    assert!(
        captures_after_native >= 10,
        "vacuous: pre-f never went native during the run ({captures_after_native} captures after install)"
    );
    assert!(
        misplaced.is_empty(),
        "{} of {captures_after_native} preempts of the native loop captured a frame other than \
         pre-f at ip 0 (first few: {:?}) — the resume then re-enters the loop mid-body and \
         interprets it to the next back-edge boundary",
        misplaced.len(),
        &misplaced[..misplaced.len().min(4)]
    );
}

/// Native code resolves the head of a tail call by name through the global-read cache, and
/// it used the CALL site's number as the slot. Call sites and global reads are numbered
/// separately, so that slot was another read's (here `limit`'s, and the two evicted each
/// other) or lay past the arm's block, where the cache drops every write and each call
/// walked the environment — `json`'s `string-acc` 100 000 times a run, `primes` 6% of its
/// instructions. Nothing else could see it: the cache checks the symbol, so every answer
/// stayed right. The arm now reserves a slot per call site after its reads
/// (`gsite_reservation`), and both entries must be in the arm's own block side by side.
#[cfg(feature = "jit")]
#[test]
fn a_native_tail_call_by_name_caches_its_head_in_its_own_slot() {
    use std::sync::atomic::Ordering::Acquire;
    // Below the native tier (`BROOD_TIER=0|1`, the tree-walker CI job) nothing goes native,
    // so there is no native tail call to observe.
    if crate::eval::compile::tier_ceiling() != crate::eval::compile::Tier::Native {
        return;
    }
    let mut interp = crate::Interp::new();
    interp
        .eval_str(
            "(def limit 7)
             (defn hop (n) (if (= n 0) limit (hop-on (- n 1))))
             (defn hop-on (n) (hop n))
             (defn warm (k) (if (< k 1) :ok (do (hop 1) (warm (- k 1)))))",
        )
        .expect("define");
    let id = match interp.eval_str("hop").expect("read hop").unpack() {
        ValueRef::Fn(id) => id,
        _ => panic!("`hop` should be a closure"),
    };
    let arm = compiled_arm_for(&interp.heap, id, 1).expect("hop/1 compiles");
    let arm = arm.arc().clone();
    wait_for_tier(
        || {
            let code = arm.jit_code.load(Acquire);
            !code.is_null() && code != crate::jit::BAILED && code != crate::jit::QUEUED
        },
        || arm.jit_code.load(Acquire) == crate::jit::QUEUED,
        || {
            interp.eval_str("(warm 200)").expect("warm");
        },
        || "`hop` should have tiered up to native code".to_string(),
    );
    let chunk = arm.chunk.as_ref().expect("hop has a chunk");
    let limit = value::intern("limit");
    let read_site = chunk
        .code
        .iter()
        .find_map(|i| match i {
            Inst::GlobalIc { sym, site } if *sym == limit => Some(*site),
            _ => None,
        })
        .expect("`limit` is a cached global read");
    let call_site = chunk
        .code
        .iter()
        .find_map(|i| match i {
            Inst::Call {
                tail: true,
                head: Some(_),
                site,
                ..
            } => Some(*site),
            _ => None,
        })
        .expect("`(hop-on …)` is a tail call by name");
    // Native `hop(1)` caches `hop-on` as it tail-calls it; `hop(0)` then caches `limit`.
    interp.eval_str("(hop 1)").expect("run hop natively");
    let (_, gbase) = interp.heap.vm_arm_block(&arm);
    let head_slot = arm.tail_head_gsite(call_site);
    assert!(
        head_slot < arm.ngsites,
        "the head's slot is inside the arm's block"
    );
    assert_ne!(
        head_slot, read_site,
        "the head and `limit` have slots of their own"
    );
    assert_eq!(
        interp.heap.dbg_global_ic_sym(gbase + head_slot),
        Some(value::intern("hop-on")),
        "the native tail call cached its head in the slot reserved for it"
    );
    assert_eq!(
        interp.heap.dbg_global_ic_sym(gbase + read_site),
        Some(limit),
        "the head did not evict `limit`'s entry"
    );
}

/// A leaf derivation is stamped with the epoch it was derived at, and the compiler thread
/// refuses a stale one for good. One `def` of anything between an arm's compile and its
/// tier-up therefore kept the arm on its small native for the rest of the program — `json`'s
/// `skip-ws`, `digits-run` and `json-object` whenever a module loaded mid-run. The tier-up
/// now checks what the derivation reads (its spliced callees and primitives) and moves the
/// stamp forward when nothing it reads changed, so the inlined body still installs.
/// An arm entered from the interpreter SETTLES on its installed pointer once the tier-up has
/// nothing left to do for it, so later entries take `jit_tier_in_frame`'s fast path. This
/// build has debug assertions, which keep the xcall re-lowering off, so `g` settles on its
/// first body here; the re-lowered case is `stress/jit_settle_test.blsp`, run on a release
/// build by `make check-stress`.
#[cfg(feature = "jit")]
#[test]
fn an_interpreted_callers_native_callees_settle_on_their_code() {
    use std::sync::atomic::Ordering::Acquire;
    if crate::eval::compile::tier_ceiling() != crate::eval::compile::Tier::Native {
        return;
    }
    let mut interp = crate::Interp::new();
    // `drive` stays interpreted (the `binding` keeps it off the JIT), so its calls to `g` and
    // `leaf` are the interpreter-to-native entries that settle. `g` is a native loop with a
    // non-tail named call to a non-leaf `h`: the shape a release build re-lowers.
    interp
        .eval_str(
            "(defn h (x) (if (< x 2) x (+ (h (- x 1)) 1)))
             (defn g (n acc) (if (= n 0) acc (g (- n 1) (+ acc (h (bit/and n 3))))))
             (defn leaf (x) (+ x 1))
             (defdyn *k* 1)
             (defn drive (i acc)
               (if (>= i 2000) acc (drive (+ i 1) (binding (*k* 1) (+ acc (g 3 0) (leaf i))))))",
        )
        .expect("define");
    let arm_of = |interp: &mut crate::Interp, name: &str, argc: usize| {
        let id = match interp.eval_str(name).expect("read").unpack() {
            ValueRef::Fn(id) => id,
            _ => panic!("`{name}` should be a closure"),
        };
        compiled_arm_for(&interp.heap, id, argc)
            .expect("compiles")
            .arc()
            .clone()
    };
    interp.eval_str("(drive 0 0)").expect("first run");
    let g = arm_of(&mut interp, "g", 2);
    let leaf = arm_of(&mut interp, "leaf", 1);
    let settled = |arm: &CompiledArm| {
        let code = arm.jit_code.load(Acquire);
        !code.is_null()
            && code != crate::jit::BAILED
            && code != crate::jit::QUEUED
            && arm.settled_code.load(Acquire) == code
    };
    let queued = |arm: &CompiledArm| {
        arm.jit_code.load(Acquire) == crate::jit::QUEUED || arm.inline_queued.load(Acquire)
    };
    wait_for_tier(
        || settled(&g) && settled(&leaf),
        || queued(&g) || queued(&leaf),
        || {
            interp.eval_str("(drive 0 0)").expect("warm");
        },
        || {
            format!(
                "never settled: g={} (inline_installed={}, inline_queued={}) leaf={}",
                settled(&g),
                g.inline_installed.load(Acquire),
                g.inline_queued.load(Acquire),
                settled(&leaf)
            )
        },
    );
    assert_eq!(
        interp
            .eval_str("(drive 0 0)")
            .expect("run settled")
            .as_int(),
        // `(g 3 0)` is h(3) + h(2) + h(1) = 6; `(leaf i)` is i + 1.
        Some(2000 * 6 + (1..=2000).sum::<i64>()),
    );
}

#[cfg(feature = "jit")]
#[test]
fn an_unrelated_def_does_not_cost_a_leaf_derivation_its_inlined_body() {
    use std::sync::atomic::Ordering::Acquire;
    if crate::eval::compile::tier_ceiling() != crate::eval::compile::Tier::Native {
        return;
    }
    let mut interp = crate::Interp::new();
    interp
        .eval_str(
            "(def unrelated 0)
             (defn leaf-sq (x) (* x x))
             (defn sum-sq (n acc) (if (= n 0) acc (sum-sq (- n 1) (+ acc (leaf-sq n)))))
             (defn warm (k) (if (< k 1) :ok (do (sum-sq 20 0) (warm (- k 1)))))",
        )
        .expect("define");
    // Compile `sum-sq` (deriving its leaf splice of `leaf-sq`) with one call, THEN move the
    // epoch by rebinding a global it does not read (a new name does not move it).
    assert_eq!(
        interp
            .eval_str("(sum-sq 3 0)")
            .expect("first call")
            .as_int(),
        Some(14)
    );
    let id = match interp.eval_str("sum-sq").expect("read sum-sq").unpack() {
        ValueRef::Fn(id) => id,
        _ => panic!("`sum-sq` should be a closure"),
    };
    let arm = compiled_arm_for(&interp.heap, id, 2).expect("sum-sq/2 compiles");
    let arm = arm.arc().clone();
    let leaf = arm.leaf.as_ref().expect(
        "`sum-sq` should carry a leaf derivation splicing `leaf-sq`, or this test checks nothing",
    );
    let derived_at = leaf.epoch.load(Acquire);
    interp.eval_str("(def unrelated 1)").expect("def");
    assert_ne!(
        interp.heap.global_epoch(),
        derived_at,
        "the def moved the epoch"
    );
    wait_for_tier(
        || arm.inline_installed.load(Acquire),
        || arm.inline_queued.load(Acquire) || arm.jit_code.load(Acquire) == crate::jit::QUEUED,
        || {
            interp.eval_str("(warm 200)").expect("warm");
        },
        || "`sum-sq`'s inlined body never installed after an unrelated def".to_string(),
    );
    assert_eq!(
        interp
            .eval_str("(sum-sq 20 0)")
            .expect("run inlined")
            .as_int(),
        Some(2870)
    );
    // The other direction: rebinding the callee the derivation spliced must not let the
    // tier-up re-stamp it — the old body would keep running. Warm past a re-tier and the
    // new definition must be what runs.
    interp
        .eval_str("(def leaf-sq (fn (x) (* x 10)))")
        .expect("rebind the spliced callee");
    for _ in 0..50 {
        interp.eval_str("(warm 200)").expect("warm");
        assert_eq!(
            interp.eval_str("(sum-sq 20 0)").expect("run").as_int(),
            Some(2100),
            "a re-tier after rebinding `leaf-sq` ran the body spliced before the rebind"
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// Warm until `done`, failing after a minute of waiting with no compile in flight. The
/// background compiler serves its whole process, and `cargo test` — the nightly ASAN job —
/// runs every lib test as a thread of ONE process: there, under ASAN, the queue backed up
/// past the flat 60 s these waits had, and three tests failed with their arm still queued
/// (`inline_queued=true`), nothing wrong but the wait. Time spent `queued` does not spend the
/// budget; ten minutes is the outer cap, so a compile that never finishes still fails.
fn wait_for_tier(
    mut done: impl FnMut() -> bool,
    mut queued: impl FnMut() -> bool,
    mut warm: impl FnMut(),
    what: impl Fn() -> String,
) {
    let start = std::time::Instant::now();
    let mut idle = std::time::Duration::ZERO;
    let mut last = start;
    while !done() {
        let now = std::time::Instant::now();
        if !queued() {
            idle += now - last;
        }
        last = now;
        assert!(
            idle < std::time::Duration::from_secs(60)
                && now - start < std::time::Duration::from_secs(600),
            "{}",
            what()
        );
        warm();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// A JIT'd arm standing in for one that ran, for the suspend-host latch tests: its native
/// installed and published to the shared cache under `key`, the way a peer finds it.
#[cfg(feature = "jit")]
fn published_native_arm(heap: &Heap, key: (u64, u16)) -> Arc<CompiledArm> {
    extern "C" fn fake_native(_heap: *mut Heap, _base: i64) -> i64 {
        0
    }
    let code = fake_native as *mut u8;
    let arm = Arc::new(CompiledArm {
        nrequired: 2,
        nslots: 2,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(Chunk {
            code: vec![Inst::Local(0)],
        }),
        jit_code: std::sync::atomic::AtomicPtr::new(code),
        compile_epoch: std::sync::atomic::AtomicU64::new(heap.global_epoch()),
        share_key: Some(key),
        shared_published: std::sync::atomic::AtomicBool::new(true),
        dbg_name: Some(value::intern("hosts-a-receive")),
        ..CompiledArm::default()
    });
    heap.jit_shared_publish(key, code, heap.global_epoch());
    arm
}

/// KI-309: a suspend-host latch is final. One exit can both block its worker — the latch —
/// and leave on the sixteenth entry-param deopt, whose polymorphic re-lowering resets the
/// arm to untried. The deopt feedback runs after the latch in the same `native_feedback`,
/// and its reset put the arm back on the native code the latch had just taken it off: the
/// next walk blocked its worker again (`interrupt_one_worker`, 1 run in 200).
#[cfg(feature = "jit")]
#[test]
fn a_suspend_latch_survives_a_relowering_due_in_the_same_exit() {
    use std::sync::atomic::Ordering::Acquire;
    let mut interp = crate::Interp::new();
    let heap = &mut interp.heap;
    let arm = published_native_arm(heap, (0x0309_0001, 2));
    arm.jit_entry_deopts.store(
        ENTRY_DEOPT_RELOWER - 1,
        std::sync::atomic::Ordering::Relaxed,
    );
    let gateway = 7;
    heap.blocked_under_gateway = gateway;
    heap.note_jit_deopt_reason(
        crate::eval::compile::deopt_reason::DeoptReason::EntryParamTag.code_with(0) as u32,
    );
    let exit = NativeExit {
        outcome: crate::jit::outcome::DEOPT,
        gateway,
        env: heap.global(),
    };
    let base = heap.roots_len();
    native_feedback(heap, &arm, base, arm.nslots, &exit);
    assert_eq!(
        arm.jit_entry_deopts
            .load(std::sync::atomic::Ordering::Relaxed),
        ENTRY_DEOPT_RELOWER,
        "the exit was not the sixteenth deopt, so it tested nothing"
    );
    assert_eq!(
        arm.jit_code.load(Acquire),
        crate::jit::BAILED,
        "the re-lowering put a suspend-latched arm back to untried"
    );
}

/// KI-309: the latch takes back the native code the arm published. Another copy of the same
/// closure's arm, untried or queued, adopts whatever the shared cache holds, so a latch that
/// left it there let that copy run the code and block its worker in turn.
#[cfg(feature = "jit")]
#[test]
fn a_suspend_latch_takes_back_the_code_it_published() {
    let mut interp = crate::Interp::new();
    let heap = &mut interp.heap;
    let key = (0x0309_0002, 2);
    let arm = published_native_arm(heap, key);
    assert!(heap.jit_shared_lookup(key).is_some());
    let gateway = 9;
    heap.blocked_under_gateway = gateway;
    let exit = NativeExit {
        outcome: 0,
        gateway,
        env: heap.global(),
    };
    let base = heap.roots_len();
    native_feedback(heap, &arm, base, arm.nslots, &exit);
    assert_eq!(
        arm.jit_code.load(std::sync::atomic::Ordering::Acquire),
        crate::jit::BAILED
    );
    assert!(
        heap.jit_shared_lookup(key).is_none(),
        "a copy of the arm could still adopt the latched code"
    );
}

/// KI-313: the latch is the function's, not one copy's. A function can have two
/// `CompiledArm`s at once — a recompile after a module load staled the first, a peer
/// compiling the same closure — and in the Nightly's conditions (the suite's stdlib image
/// in the cache) the first process latched its copy while a second copy, already native,
/// blocked the next process's worker (`interrupt_one_worker`'s native-walk case, 85% of
/// runs once primed). The latch reaches every copy admitted toward native code, and refuses
/// any copy that comes after it.
#[cfg(feature = "jit")]
#[test]
fn a_suspend_latch_reaches_every_copy_of_the_function() {
    use std::sync::atomic::Ordering::Acquire;
    let mut interp = crate::Interp::new();
    let heap = &mut interp.heap;
    let key = (0x0313_0003, 2);
    let blocked = published_native_arm(heap, key);
    let sibling = published_native_arm(heap, key);
    assert!(heap.jit_suspend_admit(&blocked));
    assert!(heap.jit_suspend_admit(&sibling));
    let gateway = 13;
    heap.blocked_under_gateway = gateway;
    let exit = NativeExit {
        outcome: 0,
        gateway,
        env: heap.global(),
    };
    let base = heap.roots_len();
    native_feedback(heap, &blocked, base, blocked.nslots, &exit);
    assert_eq!(blocked.jit_code.load(Acquire), crate::jit::BAILED);
    assert_eq!(
        sibling.jit_code.load(Acquire),
        crate::jit::BAILED,
        "another copy of the latched function stayed native"
    );
    let later = published_native_arm(heap, key);
    assert!(
        !heap.jit_suspend_admit(&later),
        "a copy compiled after the latch was admitted toward native code"
    );
    assert_eq!(later.jit_code.load(Acquire), crate::jit::BAILED);
}

/// The admitted-copies map forgets dead functions. Every re-evaluated def (eval-buffer, a
/// hot reload) admits its arm under a NEW key and never touches the old one again, so the
/// old entry — whose `Weak` holds the dropped arm's allocation — stayed for the runtime's
/// life: one per redefinition. An admission now sweeps the map when it has doubled, so it
/// stays within twice its live keys plus the floor.
#[cfg(feature = "jit")]
#[test]
fn suspend_admission_sweeps_the_copies_of_dead_functions() {
    use crate::core::heap::SUSPEND_SWEEP_FLOOR;
    let mut interp = crate::Interp::new();
    let heap = &mut interp.heap;
    let kept = published_native_arm(heap, (0x0609_0000, 1));
    assert!(heap.jit_suspend_admit(&kept));
    // a thousand redefinitions, each admitted once and then dropped
    for generation in 0..1000u64 {
        let arm = published_native_arm(heap, (0x0609_1000 + generation, 1));
        assert!(heap.jit_suspend_admit(&arm));
    }
    let keys = heap.jit_suspend_admitted_keys();
    assert!(
        keys <= SUSPEND_SWEEP_FLOOR + 1,
        "{keys} keys admitted for one live function: the dead ones were never swept"
    );
    // the live copy survived every sweep, and a later latch still reaches it
    let blocked = published_native_arm(heap, (0x0609_0000, 1));
    assert!(heap.jit_suspend_admit(&blocked));
    heap.jit_suspend_latch_function((0x0609_0000, 1));
    assert_eq!(
        kept.jit_code.load(std::sync::atomic::Ordering::Acquire),
        crate::jit::BAILED,
        "the sweep dropped a live copy, which the latch no longer reaches"
    );
}

/// Found chasing KI-313, and not its cause: one park latches every native arm under it. A
/// native walk calling a native reducer that opens a window blocks with both on the stack;
/// the latch named only the innermost, so the walk stayed native and blocked its worker again on the next call
/// (`interrupt_one_worker`'s native-walk case, intermittently: only when the reducer had
/// tiered before the first block). The reducer's exit hands the block to the walk.
#[cfg(feature = "jit")]
#[test]
fn a_suspend_latch_reaches_the_native_activation_enclosing_it() {
    use std::sync::atomic::Ordering::Acquire;
    let mut interp = crate::Interp::new();
    let heap = &mut interp.heap;
    let walk = published_native_arm(heap, (0x0313_0001, 2));
    let reducer = published_native_arm(heap, (0x0313_0002, 2));
    let (walk_gateway, reducer_gateway) = (11, 12);
    // The receive blocked under the reducer; its gateway has exited and restored the walk's.
    heap.blocked_under_gateway = reducer_gateway;
    heap.cur_native_gateway = walk_gateway;
    let base = heap.roots_len();
    let reducer_exit = NativeExit {
        outcome: 0,
        gateway: reducer_gateway,
        env: heap.global(),
    };
    native_feedback(heap, &reducer, base, reducer.nslots, &reducer_exit);
    assert_eq!(reducer.jit_code.load(Acquire), crate::jit::BAILED);
    heap.cur_native_gateway = 0;
    let walk_exit = NativeExit {
        outcome: 0,
        gateway: walk_gateway,
        env: heap.global(),
    };
    native_feedback(heap, &walk, base, walk.nslots, &walk_exit);
    assert_eq!(
        walk.jit_code.load(Acquire),
        crate::jit::BAILED,
        "the walk hosted the blocked receive and stayed native"
    );
    assert_eq!(
        heap.blocked_under_gateway, 0,
        "the outermost exit leaves no blame behind"
    );
}

/// KI-309: a latch is final only if nothing stores over it. Every write to an arm's
/// `jit_code` goes through `CompiledArm::replace_code` (a compare-and-swap from what the
/// writer read) or `clear_code_unless_latched`, except the latch itself storing `BAILED`;
/// the background compile's plain store over a latch was the one that put a suspend-latched
/// arm back on native code. The race needs a compile to finish while another process
/// latches, so this holds the line structurally: a new plain store anywhere in the crate
/// fails here.
#[test]
fn nothing_stores_over_an_arms_code_but_the_latch() {
    fn rust_files(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read the source tree") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&source_root, &mut files);
    let mut offenders = Vec::new();
    let this_file = source_root.join("eval/compile/tests.rs"); // its own patterns
    for file in files.iter().filter(|file| **file != this_file) {
        let text = std::fs::read_to_string(file).expect("read a source file");
        // Without whitespace, so a store rustfmt split over lines is still one token run.
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        for pattern in ["jit_code.store(", "jit_code.swap("] {
            for (at, _) in compact.match_indices(pattern) {
                let after = &compact[at + pattern.len()..];
                if pattern == "jit_code.store(" && after.starts_with("crate::jit::BAILED") {
                    continue; // the latch
                }
                offenders.push(format!(
                    "{}: {}",
                    file.strip_prefix(&source_root).unwrap().display(),
                    &compact[at..(at + 60).min(compact.len())]
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a plain store to jit_code can undo a latch — use replace_code or \
         clear_code_unless_latched:\n{}",
        offenders.join("\n")
    );
}
