# JIT optimizing tier — killing the per-call protocol (scope)

> **Status: SCOPE / design (2026-06-16).** Not yet implemented. This is the plan for the
> next big JIT lever after the tier-2 template JIT (`docs/jit-tier2.md`) and the two 2026-06-16
> call-path wins (`e672cee` no-clone fast-link, `eebfbd3` shared native code across processes).
> Background assumed: `docs/jit-tier2.md` (the hybrid operand model + the Brood→Brood call ABI).

## 1. The problem this solves

After native-to-native linking + the no-clone fast-link + shared-arm code, the two hottest
benchmarks (`fib`, `spawn`) are **bound by the per-call dispatch protocol**, not the compiled
body. Re-profiled `spawn` (0.17s): `jit_dispatch_call` 29.6%, `brood_jit_arm_0` (the actual fib
body) 16%, `brood_rt_push` 5.5%, `brood_rt_call_slow` 4.8% — i.e. **~40% is the protocol**. `fib`
is the same shape. The protocol is shared by every Brood→Brood call, so cutting it pays off across
`fib`/`pfib`/`spawn`/`nqueens`/any call-heavy code at once.

### What a single non-tail call costs today (the `Inst::Call` lowering, `compile.rs:~5732`)

1. **Stage operands** `[callee?, arg0..argc-1]` onto `roots` — **one `brood_rt_push` FFI call per
   operand** (`compile.rs:5834`; the callback is `jit/mod.rs:410`, a `push_root`).
2. **`brood_rt_call_slow`** (FFI, `jit/mod.rs:477`) → **`jit_dispatch_call`** (`compile.rs:~6560`):
   the IC probe (`vm_call_ic_fast_link`), `truncate_roots` + `extend_roots_to_nil` to lay out the
   callee frame, env save/restore, `jit_native_depth` bump, `transmute` + call the callee's native
   `fn(*mut Heap, base)`, then outcome handling (0=ok/3=err/1,2,4=deopt-preempt-tail).
3. **`brood_rt_roots_base`** (FFI) to re-fetch the (possibly relocated) frame base afterward
   (`compile.rs:5860`).

So every call crosses the FFI boundary **2 + argc times** and runs a chunk of Rust dispatch — even
on the fast (no-clone, epoch-current, native-linked) path. The compiled callee body is often
*smaller* than this wrapper (fib's body is 16% vs the protocol's 40%).

## 2. Two distinct techniques (often conflated as "inlining")

**A. Cranelift-emitted inline-cache + direct native call (BeamAsm-style).** Keep the *call*, but
emit the whole dispatch **inside the JIT'd arm in Cranelift IR** instead of trampolining out to
Rust: load the call site's cached `(callee_code_ptr, epoch)`, guard it (`epoch == global_epoch` &&
ptr matches the cached callee), lay the args into the callee frame, and `call_indirect` the callee's
native entry directly. On a guard miss, branch to the existing slow path (`brood_rt_call_slow`). This
**removes the `brood_rt_push`/`call_slow`/`roots_base` FFI boundaries and the Rust-side per-call
dispatch** for the hot (monomorphic, epoch-current) case — which is ~100% of `fib`/`spawn` calls.
It does **not** remove the call itself (the callee still runs as its own frame), so it works for
**recursive** callees with no unrolling. Medium effort, broad payoff, lower risk (no body splicing).

**B. True inlining (splicing).** For a call to a statically-known callee, copy the callee's lowered
body into the caller's arm, allocating the callee's slots in the caller's frame. **Removes the call
entirely** — no frame setup, no dispatch, no protocol. Clean and total for **non-recursive / leaf**
callees (`(defn sq (x) (* x x))` in a hot loop). For a **recursive** callee (`fib`, nqueens `check`)
it becomes **bounded unrolling**: inline N levels, then emit a real call (technique A) for depth > N
— so you pay the protocol every N levels instead of every level (an N× reduction, not elimination).
Higher effort; recursion-unroll + slot accounting + inlining heuristics are the cost.

**Recommendation: do A first, then B.** A is the foundation (a direct native call site is what B's
"depth > N" fallback emits anyway), delivers the bulk of the `fib`/`spawn` win on its own (kills the
FFI boundaries + Rust dispatch for the recursive hot path), and is far lower risk. B then layers on
top for leaf/helper calls and bounded recursion.

## 3. Why the hard parts are already mostly handled here

- **Hot-reload / deopt safety is FREE.** `global_epoch` is `runtime.version`, bumped on **any** `def`
  and on RUNTIME compaction (`heap.rs`). The arm's `compile_epoch == global_epoch` guard (`jit_tier`)
  already invalidates a JIT'd arm on any def — *including a redefinition of an inlined/inline-called
  callee or its operators*. So an arm that inlines `g` (B) or hard-links `g`'s code (A) is
  automatically re-tiered when `g`, `+`, `<`, … are redefined. **No new invalidation machinery** —
  the existing per-arm epoch guard covers callee changes because the epoch is global. (The IC guard
  for A must still re-check the callee *identity* per call in case the *call site* now resolves to a
  different function at the same epoch — same `(sym, argc, epoch)` validation `vm_call_ic_probe`
  already does.)
- **GC discipline is the existing per-arm discipline** (`docs/jit-tier2.md §5`). A spliced callee
  body (B) or an inline-dispatched call (A) is still one arm: handles live across the call/safepoint
  must be in `roots`/slots, not registers — exactly the handle-spill the `Inst::Call` lowering does
  today (`compile.rs:5806`). For B the callee's slots extend the caller's `nslots` (GC-visible,
  nil-init'd by `push_frame` like any frame slot). `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` is the gate.
- **The native code is position-independent + shareable** (proven by `eebfbd3`): it embeds only
  immediates + epoch-guarded globals and lives in the process-lifetime `GLOBAL_JIT` module. So a
  `call_indirect` to a callee's code ptr (A) is sound across processes/threads, and the shared
  `jit_code_cache` already maps `(closure_id, argc) → (code, epoch)` — **A can read the callee's
  code ptr straight from there** (or from the call-site IC).

## 4. The genuinely new work

- **Compile-time callee resolution.** B (and A's "bake the callee ptr at compile time" variant) needs
  to resolve a call's `head` symbol → callee closure → its `CompiledArm` **in the background
  compiler thread**. Today `JIT_COMPILER` receives `(arm, slot_tags)` only; it would also need the
  runtime `Arc` (to read `runtime.globals` + the shared `jit_code_cache`). Plumbing change, bounded.
  *A's pure-IC variant avoids this* — it reads the cached ptr at **run time** from the IC the
  interpreter already populates, so no compile-time resolution is needed for the first increment.
- **Emitting the frame setup in Cranelift (A).** Replicate `jit_dispatch_call`'s `truncate_roots` +
  `extend_roots_to_nil` + arg placement as IR. The `roots` `Vec` is `{ptr,len,cap}`; the JIT already
  reads `roots_base`. Writing slots inline means reading `roots.ptr`/`len`, bounds/capacity check,
  store the arg words, bump `len` — with a slow-path call to a `reserve` helper only when capacity is
  exceeded (rare). This is the fiddliest IR; a conservative first cut can still call **one** helper
  (`brood_rt_setup_frame`) that does the truncate/extend, keeping only the `call_indirect` inline —
  that alone removes `call_slow` + the Rust dispatch + per-arg `push`.
- **Recursion-unroll bound + inlining heuristics (B).** A depth/size budget: inline callees with
  body-size ≤ K, recursion unroll depth ≤ N (tune like `TAIL_CALL_MIN_WORK` / `jit_spill_reserve`
  were). Must be deterministic (same arm → same lowering, for the differential + journal-resume).
- **Polymorphic call sites (B/A phase 3).** HOF closures (`reduce`/`map` over a Brood fn — the
  nqueens/sort path) have a *computed* head: the IC is monomorphic-in-practice but not static. Handle
  with a guarded speculative inline/direct-call + deopt to the slow path on a guard miss. Defer.

## 5. Phasing (each phase ships + commits independently, gates green)

- **Phase 0 — A, run-time-IC variant.** Emit in Cranelift: load the call-site IC's cached
  `(code, epoch)`; guard `epoch == global_epoch` + callee-identity; `call_indirect` direct; miss →
  existing `brood_rt_call_slow`. Keep the frame setup in one helper at first. Removes the FFI
  boundaries + Rust dispatch for the hot path. **Expected: the bulk of the `fib`/`spawn` protocol
  win.** No compile-time callee resolution, no splicing — lowest risk.
- **Phase 1 — A, inline frame setup.** Move `truncate`/`extend`/arg-store into IR (slow-path
  `reserve` helper only on capacity miss). Removes `brood_rt_push` + `roots_base`.
- **Phase 2 — B, leaf inlining.** Splice non-recursive callees with body-size ≤ K (needs compile-time
  callee resolution via the runtime `Arc`). Helps helper-call-heavy code; foundation for unrolling.
- **Phase 3 — B, bounded recursive unrolling.** Inline a recursive callee to depth N, falling back to
  a Phase-0 direct call at the leaf. Helps `fib`/`nqueens`.
- **Phase 4 — speculative/polymorphic** inline for HOF-closure call sites (guard + deopt). Helps
  `reduce`/`map`/`sort`/nqueens. Highest risk; defer until 0–3 are banked.

## 6. Risk + validation (non-negotiable, per increment)

HIGH risk — this is moving-GC JIT codegen. Per `docs/jit-tier2.md §7` + CLAUDE.md, **every increment**:
`cargo test --features jit --test jit` (the JIT≡VM differential — add a warmed case per new call
shape) **under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`**; `--test differential` (VM≡tree-walker);
`--lib jit` (lowering units); the full in-language suite through the JIT (`nest test` with the `jit`
feature — both engines run via ADR-076 so the differential catches re-divergence); `make test` for
the default build; and a hot-reload-across-call/spawn check (a `def` between warmed batches must take
effect — `tests/jit_shared_spawn_test.blsp` is the template). Lowering must be **deterministic** (no
`Date`/`rand`; same arm → same IR) so the differential + workflow-resume stay sound. Commit only when
all gates are green; never ship a half-built lowering (pre-bail like `chunk_in_jit_subset` does).

## 6a. Phase 0 (FFI-collapse variant) — TRIED, measured NON-WIN (2026-06-16)

The first Phase-0 cut collapsed the **3 FFI calls per non-tail call** (per-arg `brood_rt_push` +
`brood_rt_call_slow` + `brood_rt_roots_base`) into **one** combined `brood_rt_call`: the JIT'd arm
stages operands into a stack buffer, the helper pushes them + dispatches + returns the refreshed
roots base. Built, correct (link stats unchanged: 3.71M links, 0 deopt; full correctness held), but
a clean interleaved A/B showed **fib regressed ~0.59→~0.72s and spawn was flat**. Reverted (patch:
`/tmp/phase0-combined-call.patch`). **Why it lost:** the dominant calls are **argc=1** (fib/spawn),
so it removed only ~2 cheap FFI calls while *adding* a stack-buffer round-trip (3 stores + the
helper's 3 loads) + an 8-arg call — and it still does the same `push_root` work (just moved into the
helper). **Lesson: the per-call protocol cost is NOT the FFI boundaries** (those are cheap); it's the
*intrinsic dispatch* — `jit_dispatch_call`'s IC validation + the `roots` frame setup
(`truncate_roots` + `extend_roots_to_nil` per call, the latter Nil-fills locals for GC safety and
can't be skipped). FFI-collapse doesn't touch any of that.

**Revised recommendation:** skip the FFI-collapse Phase 0 entirely. The only levers that remove the
*intrinsic* dispatch are: (i) emitting the IC guard + frame setup **inline in Cranelift** — but that
needs the JIT to grow `roots` (update `Vec::len`) without an FFI, which the current design
deliberately routes through `push_root` (the encapsulation is load-bearing for GC length-safety);
fragile, and (ii) **true inlining / bounded recursive unrolling (technique B)** — which removes the
frame setup + dispatch entirely for inlined levels (the inlined body runs in the caller's
already-set-up frame). For the recursive hot benchmarks (fib/spawn), **B is the only real win**;
go straight to it. The two shipped 2026-06-16 wins (no-clone fast-link, shared native code) already
cut the cheap parts; what remains is intrinsic and needs B.

## 6c. Phase 3 (source-level self-inlining) — TRIED, measured REGRESSION (2026-06-16)

Implemented the depth-1 self-inliner designed in §6b (the `copy_shift` slot-shift + `LetBind`
wrap, gated to top-level no-capture recursive defns). It was **correct** (fib = 832040 on both
engines, JIT≡VM + VM≡tree-walker differentials green) but a **severe regression**: fib 0.56 →
1.82s (3×), collatz 0.57 → 2.92s (5×). Cause (confirmed by perf-stats): the inlined arm is
bigger + has more non-tail calls, so it **bails the JIT subset entirely** (`jit_native=0`,
`jit_link_done=0`) → runs interpreted → far slower than the native non-inlined arm. Reverted
(patch: `/tmp/phase3-inliner.patch`).

**Lesson:** source-level inlining is counter-productive *as long as the bigger arm falls out of
the JIT*. The protocol it removes is worth less than the native execution it loses. So Phase 3
was **blocked on the JIT lowering handling larger multi-call inlined bodies**.

**UPDATE (2026-06-16) — the dominant blocker is fixed (Phase A landed).** Re-diagnosed: the inlined
arm did *not* fall out of the JIT **subset** (`chunk_in_jit_subset` gates on opcode *kind*, not
count — an inlined `fib` is all in-subset). It bailed on the **handle-spill reserve**: it was a
hardcoded **one slot**, and a depth-1 inlined body keeps >1 call result live across a safepoint, so
it tripped `spill_next >= reserve → None` and ran interpreted. `jit_spill_reserve` now reserves
`producers − 1` slots (liveness-driven, sound; see devlog 2026-06-16), proven by `BROOD_JIT_DUMP_IR`
to lower a 3-spill arm that bailed before. **`fib` itself is unchanged** (reserve still 1). The
benefit gate (`2call+walks-structure`) is a *separate*, still-standing gate, but it never fired on
`fib`/inlined-`fib` (no structure walk) — it's the bintree/nqueens concern, not the inlining one.

**So Phase 3 is now unblocked for the `fib`/recursive case** — retry the §6b self-inliner *fresh*
against the uncapped spill (the regression cause is removed). Keep the conservative gates (no
`SelfCall`/`MakeClosure`, body-size bound). Still do not inline a *structure-walking* body until
the benefit gate is revisited (that's the lever-2/allocation interaction, `allocation-elimination.md`).

**DONE (2026-06-17) — Phase B / Phase 3 shipped, ~1.7× on fib.** The §6b self-inliner landed on top
of Phase A: `shift_slots` + `inline_self_calls` + `self_inline_arm` in `compile.rs`, gated exactly as
designed (top-level no-capture recursive `defn`, no `SelfCall`/`MakeClosure`, fixed arity,
`SELF_INLINE_MAX_BODY = 64`). fib(35) 0.53 → 0.31 s (~1.7×, ~4.4× → ~2.6× of Elixir); the inlined arm
lowers to native (4 leaf calls, 3-handle spill — Phase A was the prerequisite). `BROOD_NO_INLINE=1`
disables for A/B. **One miscompile was caught + fixed before shipping** (the §6b silent-wrong-result
risk): `shift_slots` must **demote spliced `Call`s to `tail: false`** — a body's tail-position helper
call (e.g. `pow`'s `else (pow--acc …)`) spliced into operand position would otherwise return from the
whole frame and drop the wrapping expression (`(pow 2 -2)` → `4` not `0.25`; failed 32 stdlib tests
that the symmetric fib/collatz differential missed). See devlog 2026-06-17. **Next inlining levers
(deferred):** depth-N unrolling (the leaf still pays the protocol); polymorphic/HOF-closure call
sites (§Phase 4); and inlining structure-walking bodies once lever-2 allocation work lands.

**UPDATE (2026-06-17): the inliner ships default-OFF, and the unblock is the frame-rep change.**
Full-suite benchmarking showed the inliner is net-negative globally (spawn 6.5× / bintree 4.8×,
because it inflates the *shared* VM body — non-tiering arms run the bigger body interpreted). It is
shelved opt-in (`BROOD_JIT_INLINE=1`). Re-enabling it default-on (the JIT-only inlined body, VM keeps
the original) needs **per-engine frame sizing**, which is the same capability that removes the §1
per-call protocol cost — both are scoped together in **`docs/frame-representation.md`** (the chosen
structural lever after the incremental allocation levers measured neutral, devlog 2026-06-17).

## 6b. Phase 3 — recursive self-inlining (the fib lever), designed 2026-06-16 (see 6c: regressed)

Confirmed the right lever for `fib`/recursive benchmarks: fib is **63% call protocol, 26%
body**, and the protocol is dominated by per-call frame setup the dispatch-cell can't remove
(the roots-`Vec` frame-rep blocker, §6a). **Inlining sidesteps that** — the inlined body runs
in the *caller's* already-set-up frame, no new frame per level. Depth-1 self-inline collapses
~2 levels per protocol entry (~2.6–3× fewer protocol calls → fib protocol ~63%→~22%, toward
Elixir parity); depth-2 more.

**The transform (Node-level, in `compile_arm` after `compile_body`, before `compile_chunk`).**
A non-tail self-recursive call is `Node::Call { callee: GlobalIc{sym}|Global(sym), args, tail:
false, .. }` with `sym == defn_name` and `args.len() == nrequired`. Replace each with an
inlined block:
```
LetBind { binds: [(M*i + k, args[k]) for k in 0..nrequired],
          body: shift_slots(&body, M*i) }
```
where `M = scope.max` (the original arm's slot count) and `i` is the inline-block index
(1,2,… per self-call site). `shift_slots` deep-copies the body adding `M*i` to every slot
reference; the copy's own self-calls **stay as `Node::Call` (leaf protocol calls)** — that's
the depth-1 bound. `nslots = M*(1+#blocks) + jit_spill_reserve(new chunk)`.

**Why it's clean (the subtleties, resolved):**
- **No alpha-renaming** — slots are numeric, so inlining is a numeric shift + a `LetBind`
  wrapper. Args are bound in the *outer* scope (they reference outer slots) and written to the
  shifted param slots; the shifted body reads the shifted slots. No capture hazard.
- **Call sites are shareable** — the copied `Call`/`GlobalIc` nodes keep their `site` ids; all
  copies call the same function, so they hit the same (correct) IC entry. No site re-alloc.
- **`pos` shareable** — diagnostics only.

**The work + risk:** `shift_slots` is a **manual deep-copy of 14 `Node` variants** (`Node` is
NOT `Clone` — `Prim2`/`Prim1`/`ConstVal` carry `AtomicU64`s to reconstruct with their current
value). Gate conservatively: only arms with `defn_name`, **no `SelfCall`** (its frame reuse is
incompatible with shifting) and **no `MakeClosure`** (separate arm), a body-size bound (avoid
2^D blowup), and ≥1 qualifying self-call. ~200 lines, **miscompile-sensitive** (a missed slot
shift = silent wrong result). Validation net: the JIT≡VM + VM≡tree-walker differential corpora
run fib + the 2156-test suite through both engines, so a common-path miscompile fails the
gate; pair with GC_STRESS + a hot-reload check. Implement fresh (not at the tail of a long
session) — the design above is mechanical-but-careful, not exploratory.

## 7. Concrete first increment (Phase 0)

A `(defn use (x) (+ (helper x) 1))` with `helper` warmed, plus the `fib` two-call shape. Emit the
IC-guarded direct `call_indirect` for the non-tail `Inst::Call` when the site is a free-global head
(`site != NO_SITE`) and the IC is populated + epoch-current; else fall through to today's
`brood_rt_call_slow`. Differential vs the VM (warmed), under GC stress, then measure `fib`/`spawn`:
the target is `jit_dispatch_call` + `brood_rt_call_slow` dropping out of the profile for the hot path.

## 8. Key files & symbols

- `crates/lisp/src/eval/compile.rs` — `jit_lower_arm` (the `Inst::Call` handler, `~5732`),
  `jit_dispatch_call` (`~6560`, the dispatch to replace/inline), `vm_call_ic_fast_link` /
  `vm_call_ic_probe` (the IC, `core/heap.rs`), `jit_tier`, `chunk_in_jit_subset`, `CompiledArm`
  (`share_key`, `jit_code`, `compile_epoch`).
- `crates/lisp/src/jit/mod.rs` — `brood_rt_push`/`brood_rt_call_slow`/`brood_rt_roots_base` (the FFI
  callbacks Phase 0/1 remove from the hot path), `Jit::new` (symbol registration).
- `crates/lisp/src/core/heap.rs` — `RuntimeCode::jit_code_cache` (shared callee code ptrs),
  `global_epoch` (= `runtime.version`, the free deopt guard), `roots` layout (frame setup).
- Background: `docs/jit-tier2.md`, `docs/jit-stage1.md`, `docs/decisions.md` (ADR-101).

---

## Technique A — increment 1: SHIPPED 2026-06-18 (behind `BROOD_JIT_ICALL`)

**Status: done and gated.** Implemented exactly as the spec below, opt-in via `BROOD_JIT_ICALL`
(default-on once it bakes). The flat `#[repr(C)]` side table is `Heap::vm_fast_links`
(`FastLink { epoch, code:u64, nslots, _pad, env }` — `code` as a `u64`, not `*const u8`, to keep the
table `Send`/`Sync`); base+len via `Heap::vm_fast_links_base` (borrow-free `RefCell::as_ptr`); the two
callbacks are `brood_rt_fastlink_base` / `brood_rt_fast_frame`. The hot-path body is shared with
`jit_dispatch_call` through the extracted `jit_run_fast_link` helper (no desync); `jit_dispatch_fast_frame`
debug-asserts the flat-table read equals the authoritative `vm_call_ic_fast_link` on every hit.
**Result: fib(35) ≈20% faster** (0.38s→0.31s, clean `--release`). Gate green: `tests/jit.rs` 28/28
JIT≡VM (ICALL on+off, `-C debug-assertions=on`), `differential` both ways, `nest test` 2161/2161 both
ways, GC-stress+verify correct. See devlog 2026-06-18 (the Technique A increment-1 entry).

**Next = increment 2** (the open item below): move the frame setup + `call_indirect` fully into IR
(pre-reserve `roots` capacity so the nil-fill is branchless, no realloc), removing `brood_rt_fast_frame`
— the last FFI crossing on the hot path.

The original spec, as implemented:

## Technique A — increment 1 implementation spec (2026-06-18, code-grounded)

Begun by reading the real code (`jit_dispatch_call` compile.rs:7540, the `Inst::Call` lowering
~6700, `vm_call_ic_fast_link` + `CallIcEntry` heap.rs). Confirmed frontier with fresh `--bin`
numbers: `jit_dispatch_call` = **40.9% of fib(35)**. The cost is the Rust dispatch itself (IC probe +
frame setup + env/depth bookkeeping + the native call + outcome), not the FFI arg-staging (fib stages
1 arg). So the win requires emitting the **fast-link in IR** with a direct `call_indirect`.

**Critical constraint found:** the IC (`vm_call_ics: RefCell<Vec<Option<CallIcEntry>>>`, with a
`fast: Cell<Option<(usize,usize,EnvId)>>` memo) is **not safely readable from Cranelift IR** — RefCell
borrow flag, `Vec`/`Option` niche, `Cell`. So increment 1 must add an **IR-readable flat side table**.

**Increment 1 (the first testable slice — all-or-nothing for the fast-link path):**
1. **Flat fast-link table** on `Heap`: `Vec<FastLink>` indexed by call-site id, `#[repr(C)]`
   `struct FastLink { epoch: u64, code: *const u8, nslots: u32, _pad: u32, env: u64 }`. Populate it in
   `vm_call_ic_fast_link`'s memoise step (mirror of `e.fast`); zero/invalidate it everywhere
   `vm_call_ics` is cleared (`runtime_collect`) and on `vm_call_ic_put` (epoch bump). A debug-assert
   cross-checks it against the `Cell` memo so a desync is caught in the gate.
2. **`brood_rt_fastlink_base(heap) -> *const FastLink`** callback (+ register), so IR can load the
   table base once at arm entry (like `brood_rt_roots_base`).
3. **Codegen** in the non-tail elided `Inst::Call` arm (replacing the `callslow_ref` call on the hot
   path): load `slot = base + site*sizeof(FastLink)`; load `epoch`; `brif epoch == global_epoch` (the
   JIT already loads `global_epoch` for the hot-reload guard) → hit block / miss block.
   - **miss block:** the existing `brood_rt_call_slow` path (unchanged) — covers cold, redefined,
     polymorphic, over-depth.
   - **hit block:** frame setup is the open question — `extend_roots_to_nil(stage_base+nslots)` may
     realloc `roots`. Decision for increment 1: keep frame setup + env-save/depth-bump + outcome as
     **one lean FFI** `brood_rt_fast_frame(heap, stage_base, argc, nslots, env, code) -> status` that
     does exactly the fast-link body (no IC probe — already done in IR) and `call_indirect`s `code`
     internally; the IR reads the result from `out_slot` and re-fetches `roots_base`. This removes the
     **IC probe + RefCell borrow** from the Rust hot path (the measured cost) while deferring the
     in-IR frame management (roots realloc) to increment 2. Re-profile: target the 40.9% dropping.
   - increment 2: move the frame setup + `call_indirect` fully into IR (pre-reserve `roots` capacity
     so the nil-fill is branchless, no realloc), removing the last FFI crossing.

**Gating (non-negotiable, per jit-tier2.md §7):** `tests/jit.rs` JIT≡VM (add a warmed fib + a
2-call-recursion case) under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`; `differential`; `nest test`
2161; then A/B fib vs the current `--bin brood`. Behind `BROOD_JIT_ICALL` until green, then default-on.

**Risk note:** this is dispatch-critical codegen (a desync or bad guard = silent wrong answer). It is
the single riskiest change in the runtime and must be implemented incrementally with the full gate
run per step — a focused effort, not a tail-of-session burst.
