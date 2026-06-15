# JIT tier-2 — the hybrid operand model (handles in the JIT)

> **Status (2026-06-09): landed — `let`-bindings, return-via-roots, the hybrid operand
> model, and `cons`/`car`/`cdr`.** The JIT now fires on real list code, not just
> arithmetic.
>
> **Update (2026-06-15): Brood→Brood calls (`Inst::Call`) are DONE** — §6's "remaining"
> work shipped. `brood_rt_call_slow` is implemented (no longer `unimplemented!`) and
> `jit_dispatch_call` does full **native-to-native call linking** through the call-site IC
> (epoch-guarded), so a recursive callee like `fib` runs entirely native (verified:
> `jit_native` + ~30M `jit_link_done`, zero deopt). **The current frontier is per-call
> dispatch *overhead*, not "calls bail":** profiling `fib` shows ~69% of time in
> `jit_dispatch_call` itself (the IC probe + per-call Arc clone + several Acquire atomic
> loads + the `roots`-Vec frame setup/teardown), only ~14% in the compiled arm. BeamAsm
> resolves a local call to a compile-time label + a register frame; Brood re-validates the
> target and rebuilds a heap-`Vec` frame every call. Leaning that protocol (drop the
> per-call Arc clone, `resize` the nil-fill, fewer atomics) and — the bigger lever — *true
> inlining* (splice the callee, no call protocol at all) is the next work. See §6.

> This doc is the pickup reference; it assumes `docs/jit-stage1.md` (the tier-1 int JIT) as background.

Everything here lives behind `--features jit`. The codegen is `jit_lower_arm` in
`crates/lisp/src/eval/compile.rs`; the runtime callbacks are in
`crates/lisp/src/jit/mod.rs`; the tiering driver is `jit_tier` + the `vm_run_bc`
hooks (also `compile.rs`).

## 1. The problem tier-2 solves

Tier-1 kept the whole operand stack in unboxed `i64` **registers**, so any arm that
touched a **heap handle** (a `Pair`, the result of a call, …) bailed — `(Local xs)`
eagerly tag-checked `Int` and deopted on a list. Real Brood is full of handles and
calls, so tier-1 effectively only fired on self-contained arithmetic loops.

Tier-2 lets an operand-stack entry hold a handle, **in `roots` (a frame slot) or in
registers transiently**, so list/handle/call code JITs.

## 2. The operand model (`Op`, in `jit_lower_arm`)

The operand stack is `Vec<Op>`:

```
enum Op {
    Int(ir::Value),                       // an unboxed i64 in a register (fast path)
    Slot(usize),                          // a Value in roots[base+k]; type unknown, read lazily
    Handle(ir::Value, ir::Value, ir::Value), // a fresh Value as its 3 words, in registers (transient)
}
```

- **`Local(k)` pushes `Slot(k)`** (lazy — no eager load).
- A consumer that needs an **`i64`** (arithmetic, a branch condition, a join block-arg)
  calls `as_int`, which tag-checks `Int` and deopts otherwise (`Slot` → checked load;
  `Handle` → checked payload extract).
- A consumer that moves a **whole `Value`** (a `SetLocal` binder, a self-call arg, the
  return) copies/stores all of it via `read_words`/`store_words`/`store_op`/`copy_value`,
  so a handle round-trips untouched.

Return is **return-via-roots**: each exit stores its result into `roots[base]` (`exit_done`)
and jumps to a param-less `Done` block that returns 0 — so the result can be a handle (it
can't be an `i64` block param).

## 3. A `Value` is **24 bytes** — copy all of it

`size_of::<Value>() == 24` (three i64 words at offsets 0 / 8 / 16). The `Pid { node, id }`
variant uses the third word (`id` at offset 16), so **every whole-`Value` copy must move
all three words** — a tag+payload-only copy silently corrupts a `Pid`. `STRIDE =
size_of::<Value>()`; loops copy `0..STRIDE` step 8. Pinned by
`value_layout_is_stable_for_the_jit` (asserts `size == 24`, `TAG_INT == 2`, `TAG_PAIR ==
9`). Note `TAG_PAIR` is **9**, not `Tag::Pair`'s 7 — `Value` has an extra `BigInt` after
`Int` and a `Rope` before `Pair` (`jit_layout` in `core/value.rs`).

## 4. The handle ABI — by value, out-pointer

A `Value` is 24 bytes, so it can't be a C register-pair return. The handle ops use an
**out-pointer**: the JIT allocates one scratch `Value`-sized stack slot per arm
(`out_slot`), passes its address, the callback writes the result `Value` there, and the
JIT reads the three words back into an `Op::Handle` (`call_handle`).

Callbacks (`crates/lisp/src/jit/mod.rs`, registered in `Jit::new`):

| symbol | signature | notes |
|---|---|---|
| `brood_rt_cons(heap, out, c0,c1,c2, d0,d1,d2)` | writes the pair to `*out` | operands by 3-word value |
| `brood_rt_car(heap, out, w0,w1,w2)` | writes car to `*out` | JIT tag-checks `Pair` first |
| `brood_rt_cdr(heap, out, w0,w1,w2)` | writes cdr to `*out` | JIT tag-checks `Pair` first |
| `brood_rt_gc_safepoint(heap)` | collect if due | back-edge of cons-allocating arms |

`words_to_val(w0,w1,w2)` transmutes `[i64;3] → Value` — sound because the words came out of
a real `Value` (a slot, an `Int` box `[TAG_INT, payload, 0]`, or a previous handle result).

## 5. The GC discipline (the load-bearing invariants)

A moving collector relocates handles; the rules that keep the JIT correct:

1. **A handle in a register is only ever transient.** `cons`/`car`/`cdr` produce an
   `Op::Handle` (three registers) that is **produced and consumed within one block** — stored
   to a slot by a self-call arg / binder / return, or tag-checked back to an int. It never
   crosses the loop back-edge live. Bail if one would cross a join boundary as a handle (it
   goes through `as_int` there → deopt for a real handle).
2. **The only safepoint is the loop back-edge.** A cons-allocating arm calls
   `brood_rt_gc_safepoint` there — *after* args are stored to slots, so the operand stack is
   empty and no handle is live in a register across the collection. `car`/`rest` don't
   allocate, so non-cons arms have no safepoint at all.
3. **`alloc_pair` only grows the nursery; it never collects.** So a reconstructed operand
   `Value` can't go stale mid-`cons`.
4. **`collect` relocates roots *in place*** (it never reallocates the `roots` `Vec`) — and
   the handle ops write to the out-slot, not `roots` — so **`roots_base` (fetched once at
   entry) stays valid for the whole arm.** No re-fetch needed.
5. Loop-carried state lives in **frame slots** (in `roots`, GC-visible), written by
   `SelfCall`. `SelfCall` reads every arg's `Value` into registers *before* storing, so
   `(f b a)` can't alias slots being overwritten.

`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` is the gate that proves all this: a cons loop
allocates a pair per iteration, so the verifier walks the whole live graph at every
collection.

## 6. Brood→Brood calls — SHIPPED (the payoff)

> **DONE (2026-06-15).** Everything below is implemented: `Inst::Call` lowers, `brood_rt_call_slow`
> is real (not `unimplemented!`), and `jit_dispatch_call` native-to-native links through the
> call-site IC. The remaining work is *overhead*, not *coverage* — see the top-of-file 2026-06-15
> note (profiling: ~69% of `fib` is in `jit_dispatch_call`'s per-call protocol). The plan-text
> below is kept for historical context.

A body that calls a *helper* (`(f (g x))`, `(map h xs)` open-coded, etc.) is the common
real-code shape and currently **bails** (any `Inst::Call` is out of subset). Plan:

- Lower `Inst::Call { argc, tail, .. }`: stage the callee + args into `roots`, then call
  **`brood_rt_call_slow`** (the existing stub — finish it) to dispatch through the
  interpreter, returning the result as an `Op::Handle` (it's an arbitrary `Value`).
  This is a safepoint (the callee runs arbitrary Brood, allocates, may GC), so **all live
  operands must be in `roots` or slots across it** — same discipline as the back-edge.
- The callee may **error / preempt / suspend**. The JIT'd arm must surface those like the
  VM: `brood_rt_call_slow` returns a status (ok / error / yield) and the JIT deopts (returns
  the existing 1) or propagates. Decide whether a non-tail call result feeds back into the
  arm (yes) vs a tail call (reuse the frame, like `SelfCall`).
- Reuse: `Op::Handle` for the result, the out-pointer/word ABI for passing the result back,
  `as_int`/`store_op` for downstream use. The hard part is the **call protocol** (how args
  are staged, how the result/status comes back, deopt across the call) — `brood_rt_call_slow`
  is stubbed (`unimplemented!`) and the protocol is "finalized in Stage 1" per its doc.
- Tag-check / IC: a native call site doesn't yet have the epoch-guarded IC (Stage 3); the
  arm-granularity epoch guard in `jit_tier` still protects against `def` (a redefinition
  re-tiers the whole arm).

Start small: a **non-tail call to a known global helper** in an otherwise-arithmetic body
(e.g. `(defn use (x) (+ (helper x) 1))` warmed), differential vs the VM, then add tail
calls, error/preempt propagation, and multi-arg.

## 7. Verification protocol (every increment)

- `cargo test -p brood --features jit --lib jit` — the unit tests (lowering).
- `cargo test -p brood --features jit --test jit` — the e2e differential suite
  (`tests/jit.rs`); add a warmed JIT≡VM case for each new shape. **Also run it under
  `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`** — non-negotiable for anything that allocates or
  holds a handle across a safepoint.
- `cargo test -p brood --features jit --test differential` — the VM≡tree-walker corpus.
- Confirm an arm actually *tiers* (not silently bails): temporarily trace the bg compiler's
  install site in `compile.rs` (the `match lowered { Some => …, None => … }`) behind an env
  var, run a warmed program, check `COMPILED`.
- Keep the default (no-`jit`) build clean.

## 8. Key files & symbols

- `crates/lisp/src/eval/compile.rs` — `jit_lower_arm` (codegen: `Op`, `read_words`,
  `store_words`, `as_int`, `store_op`, `exit_done`, `call_handle`, the per-inst handlers),
  `jit_tier` (tiering + the hot-reload epoch guard), `chunk_ops_all_native`, the `vm_run_bc`
  hooks, `tests` (unit) + the `jit_speedup_vs_vm` bench.
- `crates/lisp/src/jit/mod.rs` — the `brood_rt_*` callbacks + `Jit::new` registration.
- `crates/lisp/src/core/value.rs` — `jit_layout` (`PAYLOAD_OFFSET`, `TAG_INT`, `TAG_PAIR`) +
  `value_layout_is_stable_for_the_jit`.
- `crates/lisp/tests/jit.rs` — the e2e JIT≡VM suite.
- Background: `docs/jit-stage1.md` (tier-1), `docs/vm-perf-and-jit-runway.md` (ADR-101 ABI),
  `docs/devlog.md` (2026-06-09 entries).
