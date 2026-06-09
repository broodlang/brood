# Plan — JIT Stage 1: first compiled arm (ADR-101)

> **Status: landed (2026-06-08).** Stage 1 is implemented behind `--features jit` (off by
> default). See §7 for what shipped and the verification results. Stage 0 plumbing is in
> commits `15ba15c`/`0cf1a9a`. Stage 1 is the first real codegen: compile a hot
> RUNTIME-region arm to native code and deopt to the VM on anything it can't handle —
> beating the interpreter ~27× on a dispatch-bound int loop.

See also: ADR-101 (architecture + calling convention), `value-repr.md` (the kept-enum
decision + the slot-size-is-neutral measurement this plan leans on), `vm-perf-and-jit-runway.md`
§6, `src/jit/mod.rs` (the Stage-0 callbacks), `src/eval/compile.rs` (the bytecode the
JIT lowers from).

## 1. Goal & success criterion

Compile **one** hot arm — a chunked, RUNTIME-region 0/1-arg arithmetic loop (`loop`'s
`loop--acc`, `fib`) — to native code via Cranelift, install it, and run it through the
platform trampoline, with **VM deopt** for everything outside the compiled subset.

**Success = the compiled arm is correct (differential-identical to the VM) and faster on
`loop`/`fib`.** Correctness first: a JIT bug under a moving GC is the worst class of bug,
so the `differential` corpus + the §6 KI-1 bar + `BROOD_GC_STRESS`/`BROOD_GC_VERIFY` gate
every step.

## 2. The linchpin: a JIT-readable `Value` layout

To inline arithmetic (the whole point — the profile is dispatch-bound, and inlining the
`+`/`<`/`-` is what removes the per-op dispatch), JIT'd code must read a `Value` out of a
`Heap::roots` slot, **check its tag, and extract the `i64`** — i.e. it must know `Value`'s
byte layout. Today `Value` is a `#[derive]`d enum with the **default (unspecified) Rust
layout** — discriminant position, niche use, and payload offsets are not guaranteed, so
the JIT can't read it.

**Decision for Stage 1: give `Value` a stable `#[repr(C, u8)]` layout** — a C tagged
union: a `u8` discriminant at offset 0, the payload at a fixed (8-aligned) offset. Then
JIT'd code knows "Int = discriminant N, payload `i64` at offset 8" and can emit the
tag-check + load directly.

- This is normally a hard call (it can grow `Value` and kills niche optimization for
  `Option<Value>`). **But the measurement in `value-repr.md` already settled the cost
  side:** operand-slot size is perf-neutral on compute, so a few extra bytes don't
  matter. So we pay layout-stability with size we've shown is free.
- It does **not** change any `match value { … }` syntax — only the in-memory layout.
- **Tasks:** add `#[repr(C, u8)]` (or `#[repr(u8)]` if it already yields a stable
  layout — verify with a layout test), add a `const`-checked `size_of`/offset test so
  the JIT's hardcoded offsets can't silently drift, audit for any code assuming 16 bytes
  or transmuting `Value`, and re-run the full suite + §6 bar (no behavior change expected).
- **Fallback if `repr(C,u8)` proves disruptive:** route tag/extract through callbacks
  (`brood_rt_value_tag`, `brood_rt_value_as_int`) for the first cut — correct but slower
  (a call per operand), then inline once the layout is stable. Lead with the repr.

## 3. Staging (each step keeps the suite green)

- **1a — Codegen pipeline smoke test (revised — no asm).** *Realization (2026-06-08):*
  the pinned-register trampoline is **not needed for tier-1 correctness**. The runtime
  callbacks already take `heap: *mut Heap` as their first arg, so JIT'd code receives
  `heap` as a normal `extern "C"` argument and threads it through — no register pinning,
  no hand-written assembly. (The pinned reg / trampoline of ADR-101 §6.2 is a perf
  optimization, deferred to Stage 1.5/2.) So 1a becomes: compile a trivial
  `extern "C" fn(heap: *mut Heap) -> i64 { 42 }` through Cranelift, finalize it, call the
  resulting fn pointer, assert it returns 42 — validating the whole codegen pipeline
  (build, `JITModule` define/finalize, fn-pointer call) with zero asm.
- **1b — Tiering hook.** A per-arm call counter (reuse the call-site IC's epoch/`CompiledArm`
  machinery); on crossing a threshold, hand the arm to the JIT. Compiled code installed
  **atomically** (an `AtomicPtr` fn-pointer slot on the arm, read on entry); until set,
  the VM runs it. Lock-free, late-binding-safe (a `def` epoch bump invalidates).
- **1c — IR generation for a minimal subset.** Lower the arm's bytecode `Chunk` (not the
  `Node` tree — ADR-101) to Cranelift IR for the dispatch-bound vocabulary only:
  `Const`, `Local`/`SetLocal` (read/write a roots slot at a fixed frame offset),
  `Prim2`-int (`+`/`-`/`<`/`=` with inline tag-check on both operands), `If`/jumps,
  `SelfTail` (loop back-edge), `Done`/return. **Anything else → bail the whole compile**
  (the arm stays on the VM) — same conservative "compile or defer" gate the bytecode
  lowering already uses. A tag-check that fails at runtime → `brood_rt_call_slow` deopt.
- **1d — Safepoints, preemption, epoch guard, deopt.** Per the calling convention
  (ADR-101 §6.2): GC-visible `Value`s stay in `Heap::roots` between callbacks (no stack
  maps); unboxed `i64` ride in registers within a segment; loop back-edges call
  `brood_rt_tick` (preempt) and allocation points call `brood_rt_gc_safepoint`; an
  IC/global read compiles the `cmp [epoch]; jne slow` guard against `brood_rt_global_epoch`;
  any unhandled shape calls `brood_rt_call_slow` to fall back to the interpreter.
- **1e — Verify.** Differential corpus (JIT vs VM vs tree-walker must agree),
  `loop`/`fib`/`pfib` correctness + speedup, the §6 KI-1 bar and
  `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` on a JIT + GC-heavy workload (a compiled arm that
  allocates across a forced collection — the use-after-GC gauntlet for the
  values-in-roots invariant).

## 4. Risks

- **Layout drift** — the JIT hardcodes `Value` offsets; the `const` offset test (§2) is
  the guard. Treat a layout change as an ABI break.
- **Moving GC × native code** — the no-stack-map discipline (values in roots, safepoints
  only at callbacks) is the mitigation; `GC_VERIFY` on a JIT+GC workload is the test.
- **Cranelift API churn / build** — pinned at 0.132; the trampoline asm is the only
  hand-written machine code and is arch-`#[cfg]`-gated with… (no pure-Rust fallback for
  the trampoline itself — `--features jit` simply requires a supported arch; document it).
- **Deopt completeness** — every un-lowered shape must have a working `call_slow` path;
  the differential corpus is what proves no shape is mis-handled.

## 5. Out of scope for Stage 1 (later stages)

Inlining `cons`/`car`/`cdr` + float arithmetic (Stage 2), native-code ICs beyond the
epoch guard (Stage 3), RUNTIME-compaction survival of compiled code (Stage 4, ADR-091 —
needed before a long-running server JITs under hot reload), and the optional computed-goto
interpreter dispatch (Layer 2). Stage 1 is one arm, one arithmetic subset, correct + faster.

## 6. Decision record

When Stage 1 lands, record the `Value`-layout change (§2) as an ADR note (it's the one
language-observable-ish decision — a fixed in-memory layout) and tick the roadmap JIT
Stage-1 entry.

## 7. Stage 1 — landed (2026-06-08)

All of §3 (1a–1e) is implemented behind `--features jit` and off by default (zero cost
when absent). What shipped:

- **Layout (§2):** `Value` is `#[repr(C, u8)]`; `value_layout_is_stable_for_the_jit`
  pins `PAYLOAD_OFFSET = 8` / `TAG_INT = 2` as a compile-time guard against drift.
- **Codegen (1c):** `jit_lower_arm` lowers the int subset — `Const(Int)`, `Local`,
  `Prim2{Add,Sub,Mul,Lt,Le,Eq}`, `JumpIfFalse`/`Jump`, `SelfCall` (loop back-edge) — to
  Cranelift IR over block leaders + a depth worklist with block params. Anything outside
  the subset bails the whole compile (the arm stays on the VM). `brif` is emitted only on
  the `I8` result of a comparison prim (Int `0` is truthy in Brood, so a raw payload can't
  drive the branch).
- **Tiering (1b):** each `CompiledArm` carries `jit_calls: AtomicU32` + `jit_code:
  AtomicPtr<u8>`. On the 8th call a `null → QUEUED` CAS elects one thread to hand the arm
  to the **background compiler** (see below); the finalized pointer is then installed
  atomically and every subsequent entry runs native. `BAILED` (1) marks an out-of-subset
  arm so it's tried once; `QUEUED` (2) marks a compile in flight (run the VM meanwhile).
- **Background compilation:** arms are lowered on a single dedicated `brood-jit` thread —
  the sole holder of the `GLOBAL_JIT` mutex, so it's otherwise uncontended. **Worker
  threads never compile**, they enqueue and keep running the VM. This is load-bearing for
  the scheduler: Cranelift codegen is CPU-bound work of non-trivial duration, and doing it
  inline on a worker (holding the lock) starves the pool during a compile burst — a process
  on a tight timer (`(after ms …)`, monitor `:down`) could then miss its deadline. Proven
  with an amplified 50ms-per-compile delay: synchronous compile failed the suite reliably
  (at 326s, on whichever concurrent timing-sensitive test coincided with the burst);
  background compile passes the *same* stress 2/2 at ~55s (the delay no longer touches the
  workers).
- **Safepoints/preempt/deopt (1d):** loop back-edges call `brood_rt_tick` (preempt only in
  a capture-mode green process — gated on `in_capture_run`, matching the VM loop-top);
  deopt returns code 1, preempt code 2, normal completion 0. Values live in `Heap::roots`
  between callbacks (no stack maps).
- **VM hooks:** `vm_run_bc` runs a tiered arm both on fresh process start and at the
  `ChunkExit::Call` site (so a hot Brood→Brood callee runs native), falling through to the
  VM on deopt/preempt with the frame stack intact.

**Verification (all with `--features jit`):** differential JIT≡VM 2/2; lib unit 258/258
(+6 JIT tests); the full in-language suite 2039/2039; the §6 KI-1 bar —
`concurrency_race` 10/10 under `BROOD_GC_STRESS=1`, built `RUSTFLAGS="-C
debug-assertions=on" --release`. Demonstrated **~27× speedup** on a `sumto` int loop
(`jit_speedup_vs_vm`, `#[ignore]` bench).

**Speedup** — `jit_speedup_vs_vm` measures **~65×** on `sumto(100000,0)` (VM ~18s vs JIT
~0.28s over 300 reps): the native loop replaces the whole bytecode dispatch/IC/env-hop
chain with a register-resident integer loop.

## 8. Follow-up — fire on real code + correctness (2026-06-09)

Stage 1 as landed only lowered the **unfused** `Prim2`, but `emit_node` fuses the actual
loop-body shapes (`(- i 1)` → `Prim2SlotInt`, `(+ acc i)` → `Prim2SlotSlot`), so the JIT
never fired on a real compiled int loop. The follow-up (see `devlog.md` 2026-06-09) lowers
both fused variants and closes four correctness gaps the wider coverage exposed:

- **`map`** is now applied (the unfused path ignored it, so `(> a b)` computed `a < b`).
- **Overflow** deopts via `sadd/ssub/smul_overflow` instead of wrapping, matching the VM's
  BigInt promotion.
- **Hot-reload**: an epoch guard in `jit_tier` (`CompiledArm::compile_epoch`) invalidates +
  re-tiers a JIT'd arm when a `def` rebinds an inlined operator — the Stage-3 idea at arm
  granularity (a per-activation Rust check; the *native* epoch-guarded IC is still Stage 3).
- **Integer division family** (`rem`/`quot`/`%div`) added with div-by-zero / `i64::MIN/-1` /
  non-exact-`%div` deopt guards (Cranelift `sdiv`/`srem` trap on those edges).

Coverage: `tests/jit.rs` (13 end-to-end JIT≡VM cases) + the fused/map/overflow lowering unit
test, all green under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`. Still deferred to Stage 2/3/4:
`cons`/`car`/`cdr` + float arithmetic as native IR, the native-code IC, and RUNTIME-compaction
survival.
