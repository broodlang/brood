# Frame representation — JIT-managed call frames (scope)

> **Status: SCOPE / design (2026-06-17).** Not implemented. The structural lever the
> incremental work converged on: every remaining global JIT win is blocked on one missing
> capability — **JIT'd native code managing the `roots` frame itself**, without an FFI per
> call. Background: `docs/jit-tier2.md` (the operand model + Brood→Brood ABI),
> `docs/jit-optimizing-tier.md` (§6a: the per-call cost is intrinsic dispatch, not FFI;
> §6c: the inliner regression), `docs/vm-perf-and-jit-runway.md` §4.E/§6.2 (the ABI).

## 1. Why this, and why now

Measured, this session (machine `whklat`, vs Elixir):

| lever tried | result | why |
|---|---|---|
| self-inliner (dispatch) | net-negative | helps fib (tiers), wrecks spawn 6.5× / bintree 4.8× (inflates the *shared* VM body) |
| GC `Vec<u32>` forward tables (alloc) | neutral | forwarding bookkeeping is ~rounding error; mimalloc + interp-dominance |
| inline small vectors (alloc) | neutral | bigger slab copy offsets the saved `malloc` (mimalloc makes small allocs cheap) |

The allocation levers are a dead end on this suite, and the dispatch lever (inlining)
can't ship because the VM runs the inflated body. **Both dead ends trace to the same
missing capability**, and so does the next tier of JIT coverage:

- **The inliner needs per-engine frame sizing.** `nslots` is fixed per-arm and used by
  `push_frame` for *both* engines, so an inlined (bigger) JIT body forces the bytecode VM
  to pay a bigger frame (and, today, run the bigger body). If the *native* arm could grow
  its own frame on entry, the VM would keep the small original arm — fib's win with no
  spawn/bintree regression.
- **fib/spawn are ~40% per-call protocol** (`jit-optimizing-tier.md §1`), and §6a proved
  that cost is **intrinsic** — `truncate_roots(stage_base+argc)` + `extend_roots_to_nil(
  stage_base+nslots)` per call (the callee-frame nil-fill) plus arg staging — *not* the FFI
  boundary (collapsing the FFI was a measured non-win). The only way to remove it is to let
  the JIT lay out the callee frame itself.
- **Extending JIT coverage** to the structure-walking / HOF shapes that currently bail
  (bintree/nqueens/reduce run 100% interpreted) needs the JIT to manage wider, deeper
  frames without trampolining to Rust per call.

One capability unlocks all three. That makes it the highest-leverage investment, per the
project's "global wins over local effort" steer.

## 2. The current representation (grounded in code)

- **`Heap::roots: Vec<Value>`** (`core/heap.rs:895`) is the unified **operand stack + call
  frame slots**. An activation of an arm occupies `roots[base .. base+nslots]`; slot 0 is
  also the return slot (return-via-roots, `jit-tier2.md §2`). `env_roots: Vec<EnvId>`
  (`:901`) is the parallel stack for env handles.
- **The moving GC relocates `roots` in place** (`arena_flip`, `:2989`): it rewrites the
  `Value` entries to their forwarded handles but **never reallocates the `Vec`**. So a raw
  `roots_base_ptr()` (`:4131`) stays valid *across a collection* — the property tier-1 JIT
  leans on to sidestep stack maps (`vm-perf-and-jit-runway.md §6.2`). It does **not** stay
  valid across a `push` that grows the `Vec` past capacity (a realloc moves the buffer).
- **Frame setup today** (`push_frame :3158`, and the JIT's `jit_dispatch_call :7238`): a
  call stages `[callee, arg0..argc-1]` at `roots[stage_base..]` (the JIT pushes each via
  the `brood_rt_push` FFI, `jit/mod.rs:410`), then `truncate_roots(stage_base+argc)` +
  `extend_roots_to_nil(stage_base+nslots)` nil-fills the callee's locals, sets `base =
  stage_base`, and calls the native `extern "C" fn(*mut Heap, base)`. After the call the JIT
  re-fetches `brood_rt_roots_base` (the push may have realloc'd).
- **Why the JIT can't do this itself today:** `push_root`/`extend_roots_to_nil`/
  `truncate_roots` mutate the `Vec`'s **length** (Rust-managed). The JIT can *read* slots
  through `roots_base_ptr` but cannot grow/shrink the `Vec` from Cranelift IR — the length
  is not at a JIT-known, layout-stable location. That encapsulation is deliberate (length-
  safety for the GC, which scans `roots[..len]`), and it is exactly what forces every frame
  operation through an FFI.

## 3. The capability to build

**Let JIT'd native code grow and lay out the call frame directly**, with a slow-path
helper only when it must reallocate. Concretely the native arm should, in IR:

1. On a non-tail call: ensure `roots` has room for `argc(+1)` operands; write the callee +
   arg `Value`s straight into `roots[len..]`; bump `len`. (Removes per-arg `brood_rt_push`.)
2. Lay out the callee frame: nil-fill `roots[stage_base+argc .. stage_base+nslots]` and set
   `len = stage_base+nslots`, in IR. (Removes the `truncate`/`extend` intrinsic cost.)
3. On entry, a native arm grows `roots` to `base+jit_nslots` itself — so the **VM keeps the
   small `nslots`** and only the native path pays the bigger inlined frame (per-engine frame
   sizing — the inliner unblock).
4. The only FFI left on the hot path is a **`reserve` slow-path** when `len+need > cap`
   (rare; geometric growth amortizes it), which reallocates and returns the new base.

### 3.1 The key design decision — `Vec` layout vs a JIT-ABI stack

Manipulating `len`/`cap` in IR means the JIT must know their memory offsets. `Vec`'s layout
(`{ptr, cap, len}`) is **not a stable Rust ABI guarantee**. Two options:

- **(A) Rely on `Vec`'s de-facto layout**, pinned by a `value_layout_is_stable`-style assert
  + a build-time check. Lowest churn; fragile to a stdlib change (caught by the assert).
- **(B) Replace `roots`/`env_roots` with a purpose-built `RootStack`** — a `{ptr, len, cap}`
  struct with a *guaranteed* `#[repr(C)]` layout the JIT codegens against, and the same
  push/extend/truncate/relocate-in-place API the VM + GC use today. More upfront work
  (touches every `roots` user + the GC `arena_flip`), but it makes the JIT/runtime ABI
  explicit and stable — the right foundation for a load-bearing structural change.

**Recommendation: (B).** A custom stack with a pinned layout is the honest substrate for
"the JIT owns the frame"; (A) buys speed of prototyping at the cost of a latent footgun in
the single most safety-critical path (moving-GC frame management). Prototype the IR against
(A) to de-risk the codegen, then land (B) as the shipped representation.

## 4. Invariants any implementation must preserve (non-negotiable)

- **GC sees a consistent `roots[..len]`.** Every slot in `[0,len)` is a valid `Value` (nil
  or a live handle) at every safepoint. A native arm that bumps `len` must nil-fill first
  (no uninitialized slot ever visible to a collection). This is why `extend_roots_to_nil`
  exists; the IR version must keep the same discipline.
- **Relocation-in-place stays true.** The GC must continue to rewrite `roots` without
  reallocating (so a native arm's cached base survives a collection). A `reserve` (realloc)
  may only happen at a JIT-controlled point that re-fetches the base — never mid-collection.
- **Proper tail calls stay O(1) stack** (`tail_calls_do_not_overflow`). A tail call reuses
  the frame (`SelfTail`); the IR frame-reuse must not grow `roots` per hop.
- **Reduction-counted preemption** (`ADR-027`) and **state-capture suspend** (`ADR-100`):
  a native arm's frame is heap data (`roots`), so a captured/preempted process's frame must
  remain valid for resume on another worker. The JIT-managed `len`/`base` must be part of
  the captured state, consistent with `vm_run_bc`'s `Suspended`/`Preempted`.
- **Hot reload / RUNTIME compaction:** the per-arm epoch guard (`compile_epoch` vs
  `global_epoch`) still invalidates a native arm on any `def`; nothing here changes that.

## 5. Phasing (each phase ships + gates green independently)

- **Phase 0 — `RootStack` substrate (decision 3.1B).** Replace `roots`/`env_roots` with the
  layout-pinned stack, same API, same in-place relocation; VM + GC unchanged behaviourally.
  Pure refactor, no perf change expected (gate: full suite + GC stress/verify, no regression;
  bench A/B flat). De-risks everything after.
- **Phase 1 — JIT writes call operands inline.** Replace per-arg `brood_rt_push` with direct
  `roots[len..]` stores + `len` bump in IR (reserve slow-path on cap miss). Removes the arg-
  staging FFI. Expected: part of the fib/spawn protocol win.
- **Phase 2 — JIT lays out the callee frame inline.** `truncate`/`extend`-to-nil in IR.
  Removes the intrinsic frame-setup cost — the bulk of the §1 protocol ~40%.
- **Phase 3 — per-engine frame sizing.** A native arm grows to `jit_nslots` on entry; the VM
  keeps `nslots`. Re-enable the self-inliner **default-on** (it stores the inlined body for
  the JIT only; the VM runs the original) — fib's ~1.7× with **no** spawn/bintree regression
  (the whole reason it was shelved, `BROOD_JIT_INLINE`, devlog 2026-06-17).
- **Phase 4 — widen JIT coverage.** With the JIT owning frame growth, admit the shapes that
  bail today (deeper handle-spill, structure-walking + multi-call, HOF-closure call sites)
  so bintree/nqueens/reduce can run native. Measure each against the benefit gate — some are
  still allocation-mixed (`allocation-elimination.md §1`), so confirm, don't assume.

## 6. Validation (every phase, per `jit-tier2.md §7` + CLAUDE.md)

HIGH risk — moving-GC frame codegen. Every increment: `--features jit --test jit` (the
JIT≡VM differential, add a warmed case per new frame shape) **under `BROOD_GC_STRESS=1
BROOD_GC_VERIFY=1`**; `--test differential` (VM≡tree-walker); `--lib jit`; full `nest test`
through the JIT under GC stress; a hot-reload-across-call/spawn check; and the §6-style
KI-1 scheduler-race bar (this touches the captured-process frame). Lowering stays
deterministic (no `Date`/`rand`). Bench A/B (`BROOD_NO_INLINE`-style flag per phase) — and
per the project rule, **a phase that doesn't move its target benchmark is investigated or
reverted, not shipped** (the lesson from this session's neutral allocation levers).

## 7. Key files & symbols

- `crates/lisp/src/core/heap.rs` — `roots`/`env_roots` (`:895`), `push_root`/
  `extend_roots_to_nil`/`truncate_roots`/`roots_base_ptr` (`:4107+`), `arena_flip` (`:2989`,
  the in-place relocation Phase 0 must preserve), `push_frame` (`:3158`).
- `crates/lisp/src/eval/compile.rs` — `jit_dispatch_call` (`:7238`, the protocol Phases 1–2
  internalize), `jit_lower_arm` (the `Inst::Call` lowering), the shelved self-inliner
  (`self_inline_arm`/`shift_slots`, gated `BROOD_JIT_INLINE`) for Phase 3.
- `crates/lisp/src/jit/mod.rs` — `brood_rt_push`/`brood_rt_roots_base`/`brood_rt_call_slow`
  (`:396+`, the FFI Phases 1–2 remove from the hot path), `Jit::new` symbol registration.
- Background: `docs/jit-optimizing-tier.md`, `docs/jit-tier2.md`, `docs/vm-perf-and-jit-runway.md`
  (ADR-101 ABI), `docs/decisions.md`.
