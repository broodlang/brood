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
/// `swapped` call site must un-invert it. Before the fix it compared the inverted map,
/// which never matched `resolve_prim`'s natural map — so every such prim silently fell
/// to the slow path forever after the first `def` bumped the epoch.
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
    // `MakeClosure` `fn_rest`, an `&optional` default — through all the structural
    // node variants, while leaving atoms/symbols/indices alone.
    let body = Node::Do(Box::new([
        Node::Const(ConstVal::new(Value::str_(StrId::runtime(1)))),
        Node::If(
            Box::new(Node::Const(ConstVal::new(Value::int(7)))), // atom — untouched
            Box::new(Node::Const(ConstVal::new(Value::pair(PairId::runtime(2))))),
            Box::new(Node::MakeClosure {
                fn_rest: ConstVal::new(Value::pair(PairId::runtime(3))),
                captures: Box::new([]),
                self_name: None,
            }),
        ),
    ]));
    let arm = CompiledArm {
        nrequired: 0,
        noptional: 1,
        optional_defaults: Box::new([Some(Node::Const(ConstVal::new(Value::str_(
            StrId::runtime(4),
        ))))]),
        rest_slot: None,
        nslots: 0,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body,
        chunk: None,
        has_runtime_handles: true,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
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
    let Some(def) = &arm.optional_defaults[0] else {
        panic!("optional default")
    };
    assert_eq!(str_idx(load_const(def)), 104);
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 1,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())), // unused at runtime (chunk drives)
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
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
        VmOutcome::Suspended(s) => s,
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

/// JIT Stage-1 Step A: lower a straight-line int arm `(+ x 1)` to native code and
/// run it against a real heap frame — read the arg from `roots[base]`, compute in
/// registers, box the result back, and match the VM's answer.
#[cfg(feature = "jit")]
#[test]
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 1,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
    };

    let mut jit = crate::jit::CraneliftBackend::new();
    let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("straight-line int arm should JIT");
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

    // Frame: x = 41 at roots[base].
    let base = heap.roots_len();
    heap.push_root(Value::int(41));
    let outcome = f(&mut heap as *mut Heap, base as i64);
    assert_eq!(outcome, 0, "Done (no deopt — arg is an Int)");
    match heap.root_at(base).unpack() {
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 1,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
    };

    let mut jit = crate::jit::CraneliftBackend::new();
    let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("if/cmp arm should JIT");
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

    for (x, want) in [(-5i64, 5i64), (3, 3), (0, 0)] {
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(x));
        assert_eq!(f(&mut heap as *mut Heap, base as i64), 0, "Done for x={x}");
        match heap.root_at(base).unpack() {
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 2,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
    };

    let mut jit = crate::jit::CraneliftBackend::new();
    let ptr = jit_lower_arm(&mut jit, &arm, &[]).expect("self-recursive int loop should JIT");
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe { std::mem::transmute(ptr) };

    // Prime the reduction budget so these short loops run to completion (the
    // back-edge `brood_rt_tick` would otherwise yield at REDUCTIONS == 0).
    crate::process::yield_now();
    // sumto(n,0) = n+(n-1)+…+1; sumto(1,0)→sumto(0,1)→1; sumto(0,0)→0.
    for (n, want) in [(5i64, 15i64), (100, 5050), (1, 1), (0, 0)] {
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(n)); // i = slot 0
        heap.push_root(Value::int(0)); // acc = slot 1
        assert_eq!(f(&mut heap as *mut Heap, base as i64), 0, "Done for n={n}");
        match heap.root_at(base).unpack() {
            ValueRef::Int(r) => assert_eq!(r, want, "sumto({n}, 0)"),
            other => panic!(
                "n={n}: expected Int({want}), got tag {:?}",
                value::tag(other)
            ),
        }
    }

    // Preemption: a loop longer than the reduction budget yields at a back-edge —
    // the JIT'd arm returns 2 (preempt), with the frame slots left mid-computation
    // in `roots` for the driver to resume on the VM. `brood_rt_tick` only preempts in
    // a capture-mode green process, so simulate one (set/clear `capture_run`).
    crate::process::set_capture_run(true);
    crate::process::yield_now(); // budget = REDUCTION_BUDGET
    let mut heap = Heap::new();
    let base = heap.roots_len();
    heap.push_root(Value::int(1_000_000)); // far more iterations than the budget
    heap.push_root(Value::int(0));
    let outcome = f(&mut heap as *mut Heap, base as i64);
    crate::process::set_capture_run(false); // restore (cargo test shares threads)
    assert_eq!(
        outcome, 2,
        "a loop exceeding the budget must preempt (return 2) in a green process"
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 2,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
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
                site: NO_SITE,
                head: Some(fb), // free-global head, elided from the stack
                staged: false,
            }, // 14
        ],
    };
    let elided_arm = CompiledArm {
        nrequired: 2,
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 2,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(elided),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
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
    //
    // (History: this case used to be a non-self-recursive delegator whose `is_none`
    // came not from the gate but from a malformed hand-written join — a `Jump` into
    // the middle of the else block with mismatched stack depths — that failed the
    // Cranelift verifier. The 2026-07-19 type-mixed-join fix routes a disagreeing
    // edge to deopt, producing *valid* IR, so that accidental bail disappeared and
    // the test now exercises the real gate.)
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 2,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(thin),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
    };
    assert!(
        jit_lower_arm(&mut jit, &thin_arm, &[]).is_none(),
        "a thin tail-call arm (2 work ops) must be gated out (stays on the VM)"
    );
}

/// JIT Stage-1.5: the **fused** `Prim2Slot*` variants — which `emit_node` actually
/// produces for real loop bodies (`(- i 1)`, `(+ acc i)`, `(< i 1)`) — lower and run.
/// Before this, the JIT bailed on every fused prim, so it never fired on real
/// compiled code. Also pins the two correctness fixes that came with the coverage:
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
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
    let f: extern "C" fn(*mut Heap, i64) -> i64 = unsafe {
        std::mem::transmute(jit_lower_arm(&mut jit, &sumto, &[]).expect("fused sumto JITs"))
    };
    crate::process::yield_now(); // prime the reduction budget so the loop completes
    for (n, want) in [(5i64, 15i64), (100, 5050), (1, 1), (0, 0)] {
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(n));
        heap.push_root(Value::int(0));
        assert_eq!(
            f(&mut heap as *mut Heap, base as i64),
            0,
            "Done for sumto({n})"
        );
        match heap.root_at(base).unpack() {
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
    let g: extern "C" fn(*mut Heap, i64) -> i64 =
        unsafe { std::mem::transmute(jit_lower_arm(&mut jit, &gt, &[]).expect("(> x 5) JITs")) };
    for (x, want) in [(10i64, 100i64), (3, 200)] {
        let mut heap = Heap::new();
        let base = heap.roots_len();
        heap.push_root(Value::int(x));
        assert_eq!(
            g(&mut heap as *mut Heap, base as i64),
            0,
            "Done for (> {x} 5)"
        );
        match heap.root_at(base).unpack() {
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
    let s: extern "C" fn(*mut Heap, i64) -> i64 =
        unsafe { std::mem::transmute(jit_lower_arm(&mut jit, &sq, &[]).expect("(* x x) JITs")) };
    let mut heap = Heap::new();
    let base = heap.roots_len();
    heap.push_root(Value::int(3));
    assert_eq!(
        s(&mut heap as *mut Heap, base as i64),
        0,
        "(* 3 3) is in range"
    );
    assert!(
        matches!(heap.root_at(base).unpack(), ValueRef::Int(9)),
        "(* 3 3) = 9"
    );
    let mut heap = Heap::new();
    let base = heap.roots_len();
    heap.push_root(Value::int(4_000_000_000)); // 4e9 * 4e9 = 1.6e19 > i64::MAX
    assert_eq!(
        s(&mut heap as *mut Heap, base as i64),
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
    // on the default. Before ADR-222 they did not have to: `jit_tier` read its own
    // `BROOD_NO_JIT` and knew nothing about the engine selector, so under `BROOD_VM=0` the
    // selector said tree-walker while this test still got native code. That incoherence is
    // exactly what the ceiling removes — and removing it is what made these two fail in the
    // tree-walker half of `make test-both` until they were pinned.
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots,
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
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
    // Compilation is async now (the background compiler thread), so a hot arm returns
    // None until the native pointer is installed. Poll past the threshold, giving the
    // compiler time to land the code, and assert it eventually runs native.
    crate::process::yield_now(); // prime the reduction budget (short loops)
    let mut ran_native = 0;
    for _ in 0..400 {
        crate::process::yield_now(); // keep the budget topped up across calls
        let base = interp.heap.roots_len();
        interp.heap.push_root(Value::int(5)); // i
        interp.heap.push_root(Value::int(0)); // acc
        let outcome = jit_tier_in_frame(
            &sumto,
            &mut interp.heap,
            base,
            EnvRoot::Stable(EnvId::GLOBAL),
            sumto.nslots,
        );
        match outcome {
            None => {
                interp.heap.truncate_roots(base);
                std::thread::sleep(std::time::Duration::from_millis(2)); // not hot / compile in flight
            }
            Some(0) => {
                ran_native += 1;
                match interp.heap.root_at(base).unpack() {
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

    // An out-of-subset arm is marked BAILED and never runs native. `MakeMap` has no
    // JIT lowering path (there's no map-build codegen), so a map-building arm is
    // always out of subset. (Scalar `Const`s — `Int`/`Nil`/`Float`/`Bool` — and a
    // bare `Global` now *are* in subset, so neither is the bail example any more.)
    let bailing = Arc::new(mk_arm(
        Chunk {
            code: vec![Inst::MakeMap(0)],
        },
        0,
        1,
    ));
    // Generous poll cap: under plain `cargo test` (one process, every test
    // sharing the single background compiler thread) the queue ahead of this
    // arm can take seconds — 400×2ms flaked there. nextest (the canonical
    // runner) isolates per process and never sees it.
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
    // on the default. Before ADR-222 they did not have to: `jit_tier` read its own
    // `BROOD_NO_JIT` and knew nothing about the engine selector, so under `BROOD_VM=0` the
    // selector said tree-walker while this test still got native code. That incoherence is
    // exactly what the ceiling removes — and removing it is what made these two fail in the
    // tree-walker half of `make test-both` until they were pinned.
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 2,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
    });

    // Warm it past the threshold so jit_tier hands it to the background compiler;
    // poll until the native pointer is installed (compilation is async now). A
    // prelude-loaded heap, so the operator-validation in `jit_tier` resolves `+`/`-`/`<`.
    use std::sync::atomic::Ordering::Acquire;
    let mut interp = crate::Interp::new();
    crate::process::yield_now();
    let mut tiered = false;
    for _ in 0..400 {
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 2,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
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
        has_runtime_handles: false,
        jit_code: AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: None,
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots,
        nsites: 0,
        ngsites: 0,
        uid: 0,
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::int(0))),
        chunk: None,
        has_runtime_handles: false,
        jit_code: AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        dbg_name: None,
        inline_name: None,
        inline_stride: 0,
        inline_nslots,
        inline_code: AtomicPtr::new(std::ptr::null_mut()),
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        leaf: None,
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 1,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        jit_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(0),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(false),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        #[cfg(feature = "jit")]
        inline_name: None,
        dbg_name: Some(name),
        #[cfg(feature = "jit")]
        inline_stride: 0,
        #[cfg(feature = "jit")]
        inline_nslots: 0,
        #[cfg(feature = "jit")]
        inline_code: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
        #[cfg(feature = "jit")]
        inline_queued: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        inline_installed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "jit")]
        leaf: None,
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
    assert!(
        !(Tier::TreeWalk >= Tier::Bytecode),
        "ceiling 0 must NOT admit the bytecode VM"
    );
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
/// `pfib` runs 3.2x slower — while still computing the right answer, so `make test`, the
/// differential and the lowering witness all stay green and **only a benchmark moves**
/// (the same blind spot ADR-221 hit). Hence an assertion on the refcount itself.
#[test]
fn arm_handle_clone_does_not_touch_the_shared_arm_refcount() {
    // Reuse the existing arm builder rather than a second 30-field literal that would
    // drift out of step with `CompiledArm`.
    let shared: Arc<CompiledArm> = Arc::new(ki26_arm(1, 0));

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

/// KI-44: the `sqrt` call-site inline survives its move out of the prelude (ADR-227). The head
/// is now the qualified `math/sqrt`, a RUNTIME closure, so `resolve_prim1` identifies the
/// canonical wrapper STRUCTURALLY rather than by the old sealed-PRELUDE identity. This guards
/// both halves at once: the canonical wrapper still resolves to `PrimOp1::Sqrt` (so the ~1.8×
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

/// KI-48, fourth appearance: the caller sizes the frame, then `jit_tier` re-loads the code
/// pointer — and a peer can swap the inlined body in between (`CompiledArm` is shared across
/// a runtime's processes since ADR-215). Running the inlined native against the small frame
/// is a raw write past the frame top (measured overshoot: 12 slots on `fold`).
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
        noptional: 0,
        optional_defaults: Box::new([]),
        rest_slot: None,
        nslots: 2,
        nsites: 0,
        ngsites: 0,
        uid: super::ir::next_arm_uid(),
        site_pos: Box::new([]),
        body: Node::Const(ConstVal::new(Value::nil())),
        chunk: Some(chunk),
        has_runtime_handles: false,
        // The state a peer's inline swap leaves behind: the inlined pointer installed…
        jit_code: std::sync::atomic::AtomicPtr::new(code),
        jit_calls: std::sync::atomic::AtomicU32::new(0),
        deopt_watch: false,
        jit_deopts: std::sync::atomic::AtomicU32::new(0),
        float_globals: std::sync::OnceLock::new(),
        self_global_ok: std::sync::atomic::AtomicBool::new(false),
        ckpt_slot: u32::MAX,
        compile_epoch: std::sync::atomic::AtomicU64::new(heap.global_epoch()),
        share_key: None,
        shared_published: std::sync::atomic::AtomicBool::new(true),
        fn_name: None,
        src_file: None,
        capture_names: Box::new([]),
        inline_name: Some(value::intern("peer-inlined")),
        dbg_name: Some(value::intern("peer-inlined")),
        inline_stride: 2,
        // …wanting a frame six slots bigger than the one the caller built.
        inline_nslots: 8,
        inline_code: std::sync::atomic::AtomicPtr::new(code),
        inline_queued: std::sync::atomic::AtomicBool::new(true),
        inline_installed: std::sync::atomic::AtomicBool::new(true),
        leaf: None,
    });

    let base = heap.roots_len();
    heap.push_root(Value::int(1));
    heap.push_root(Value::nil()); // frame built to the SMALL nslots = 2
    let env = heap.root_env(heap.global());

    assert_eq!(
        jit_tier_in_frame(&arm, heap, base, env, arm.nslots),
        None,
        "a frame built to `nslots` must not run an `inline_nslots` native — it raw-writes \
         past the frame top (KI-48 family). Interpret this activation instead."
    );
    // The same arm entered with a frame that IS big enough still runs natively — the guard
    // must decline on size, not on the mere presence of an inlined body.
    //
    // Gated on the tier ceiling (ADR-222). Under the `differential (tree-walker)` CI job
    // (`BROOD_VM=0`, i.e. ceiling `TreeWalk`) — and under `BROOD_TIER=1`/`BROOD_NO_JIT` —
    // `jit_tier_in_frame` correctly refuses to run *any* native, so this half would fail for
    // a reason that has nothing to do with what it tests. The half above still runs at every
    // ceiling: declining is the safe answer, so it is the assertion that must hold always.
    if crate::eval::compile::tier_ceiling() == crate::eval::compile::Tier::Native {
        heap.extend_roots_to_nil(base + arm.inline_nslots);
        assert_eq!(
            jit_tier_in_frame(&arm, heap, base, env, arm.inline_nslots),
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
/// The gate used to name only `Call`/`MakeVector`/`Cons`, leaving out the table ops that
/// `inst_may_allocate` lists — the same omission that already produced one use-after-free on
/// the pair path. This pins the predicate itself, which the end-to-end shape cannot: a stale
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
