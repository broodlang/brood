# The call convention — scoped design (DRAFT, not yet an ADR)

Status: **draft, 2026-09-16**. Written to scope the work before any code; the numbers are
from this tree (`48e71d2d`, v0.29.2) unless dated otherwise. When a rung lands, the decision
it embodies gets an ADR in `docs/decisions.md` and this file records what was measured.

## 1. Why this, now

Three of the four widest gaps to the BEAM are one mechanism. Per-row compute vs Elixir,
0.29.0 column (`brood-benchmarks` `2b6116d`):

| row | × Elixir | what the profile says |
|---|---|---|
| `nqueens` | 8.0 | 108k activations of gate-refused arms interpreted (`solve`, `range`, `reduce`); dispatch is 33% of the row |
| `errors-deep` | 7.5 | fifty native frames unwound one status return at a time; two closures + three driver entries per `try` |
| `bintree` | 5.8 | ~77 ns per node over four non-tail calls; `jit_run_fast_link` + `brood_rt_fast_frame` were 20% + 13% before §7.5 |
| `pipeline` | 5.0 | same class — HOF step closures crossing the boundary per element |

The per-call costs, counted (`docs/compute-frontier.md` §7.12, re-measured here):

| call | instructions | note |
|---|---|---|
| native → native, inline blob (§7.5 rung 3) | **~218** | was 640 on 2026-09-12; the blob, the callee's stack-limit check + tick init, `read_out` |
| VM → VM (`BROOD_TIER=1`) | **~1 400** (2 938 per loop iteration incl. the body, this tree) | `exec_chunk` 61%, `vm_run_bc` 15%, `push_frame` 7%, `push_root` 4%, thread-local scheduler lookups ~6%, call-IC probe 3% |
| BEAM (BeamAsm), for scale | ~10–20 | a `call` plus the reduction count; unwinding to a catch is O(1) |

Two conclusions the record already supports and this design accepts as premises:

- **Admission is closed** (§7.12): compiling a call-mediated boxed arm to native loses to the
  interpreter *because the boundary costs more than dispatch*. So the convention is not a
  tuning knob on the JIT — it decides whether the JIT can ever cover the hot core of json /
  regex / nqueens at all. Until the boundary is cheap, the interpreter's call is what those
  rows pay, 108k times a run.
- **Register arguments are not the lever** (§7.5 rung 4's groundwork): the hot cross-arm
  callees take handles, a handle must be frame-rooted across the callee's safepoints, so
  "no roots staging" moves stores from caller to callee prologue and saves nothing. What
  costs is the *ceremony* around the call, not where the arguments sit.

## 2. What a native call does today, and why each piece exists

Read from `jit_runtime/link.rs::jit_run_fast_link` and the inline blob in
`jit_lower/call.rs` (§7.5). Each row: the obligation, what it protects, and what the
proposal does with it.

| # | per call today | protects | proposal |
|---|---|---|---|
| 1 | `truncate_roots(stage_base+argc)` + nil-fill `[len, frame_end)` (`nslots − argc` stores) | GC sees only valid `Value`s in the callee frame | keep the nil-fill but move it to the **callee prologue**, sized by what the callee writes before its first safepoint (static per arm; often zero) |
| 2 | root `callee_env` in `env_roots`, save/restore `jit_call_env` | global lookups from native resolve in the callee's env; the env must survive GC | the guarded case is already GLOBAL-only (no rooting, constant env). Pass the env as a **native argument** in a register; the callee keeps it in a stack slot. No heap field traffic |
| 3 | save/restore `jit_dbg_fn` | diagnostics name the running arm | replace with the arm pointer the code already embeds as a constant; the diagnostics read `arm.dbg_name`. **Zero per-call cost** |
| 4 | save/restore `jit_native_depth` | KI-11: bound native recursion so a tail-delegator cycle cannot overflow the OS stack | pass `depth+1` as an argument; the callee compares once in its prologue. The stack-limit *stamp* stays where it is (outermost entry) |
| 5 | save/restore `jit_force_vm` | a deopt inside the callee must not leak the flag to the caller | the flag is set only on a cold outcome; restore it **on the cold path** (outcome ≠ 0), not on every call |
| 6 | `set_ic_bases(callee)` / restore (two `Cell` writes each way) | KI-20: per-process IC blocks under shared code — the callee must write its own block | pass the two bases as arguments; the callee addresses its ICs from its stack slot. The caller's bases are never touched, so nothing to restore |
| 7 | `native_gateway_seq += 1`, save/restore `cur_native_gateway`, the latch compare after | the suspend-host latch: a `receive` that parked under THIS activation latches the arm | the token can be the callee's **frame base** (unique among live native activations) — no counter, no restore; the latch compare stays (one load + compare on the hot path is the floor) |
| 8 | result through `out` (3 stores + 3 loads) | a 24-byte `Value` cannot return in `rax` | return `(w0 \| outcome<<8, w1)` in two registers — every scalar and every handle is two words; the third word is written through `out` only when the tag needs it (strings' aux) and the caller reloads it only on that tag |
| 9 | callee prologue: stack-limit check, tick init | OS stack safety; the reduction budget | keep the limit compare (one compare against the stamped line). Tick init: one store |
| 10 | on error: `brood_rt_trace_push` per level (a callback, an `Arc` clone) | `:trace` for the crash reporter | push the **arm pointer** into a fixed 32-slot buffer inline (a load, a store, a bump) and materialise names at the catch/report (§4) |

Everything in rows 2–7 is heap-field traffic that exists because the context lives in
`Heap` fields for the *Rust* callers' benefit (`jit_tier_in_frame`, `hof_apply_native`,
the scheduler resume). The native→native path can carry that context in registers between
two native frames and hand it back to the heap only at a Rust boundary (gateway entry/exit
and the cold outcomes). The target, counted from the table: **argc stores + a call +
prologue (limit compare, tick, nil-fill) + epilogue + 2-register return ≈ 30–50
instructions**, i.e. 4–7× under today's 218 and within 3× of the BEAM.

## 3. Part A — the native→native convention

**A1. Context in arguments.** `JitArmFn` grows from `(heap, base, out)` to
`(heap, base, out, env, ic_base, gic_base, depth)`. Cranelift's SysV convention keeps six
integer args in registers. The callee stores the four context words into its own stack
slots in the prologue (four stores; today they cost four heap stores + four heap restores
in the caller). Rust callers (`jit_tier_in_frame` and friends) read them from the heap
fields once and pass them; they remain the only writers of those fields.

**A2. The trampoline is gone for the guarded case.** With the context in arguments, the
inline blob (§7.5 rung 3) becomes: `nslots` guard, argc stores (already in place — §2j),
`call_indirect`, outcome test, 2-register result. The cold outcomes (deopt/preempt/tail/
error/fallthrough) still funnel through `brood_rt_xcall_cold`, which now also restores
`jit_force_vm` and truncates env roots — the work moves from every call to the cold path.

**A3. The callee prologue owns the frame shape.** Nil-fill in the callee, bounded by a
tier-time "first-safepoint write set": a slot the body definitely assigns before any
safepoint need not be nil'd. `fib`-shaped arms nil nothing; `bintree`'s `check-node`
(a vector param, two calls) nils its temporaries — today's blob nil-fills all of them in
the caller.

**A4. Results in registers.** `(w0 | outcome << 8, w1)`; `w0`'s tag lives in the low byte
and the bits above are padding today (`eq_dispatch` masks `0xff` for exactly this reason),
so the outcome rides free. The `out` pointer stays in the ABI for the third word and for
the Rust callers, which keep the memory protocol.

**A5. What does not change.** Arguments stay in the frame (`roots[base..base+argc]`) —
they are the callee's roots. The epoch guard and the flat-table identity check stay. The
hot re-lowering stage stays (rung 3). Every cold outcome keeps today's semantics.

## 4. Part C — unwinding

Longjmp-style O(1) unwind to the catch frame is **rejected for now**: the native chain is
interrupted by Rust frames with destructors (`GcBlockGuard`, the gateway's saves) at every
Rust boundary, and only a pure native→native run could be skipped — which the measurement
says is not where the time is (`errors-deep` with trace building disabled entirely reads
121 vs 128 ms; devlog 2026-09-16). What A does for a throw: each level returns outcome 3 (a
compare and a `ret`, ~5 instructions) and pushes its arm pointer into the error's fixed
32-slot buffer inline (row 10) — O(frames) at ~10 instructions a frame instead of a
callback and an `Arc` clone. `TraceFrame` becomes a 16-byte POD (`Symbol` for the name, an
interned file id, `Pos`); `CompiledArm` gains a `src_file_id`. The VM producers
(`attach_vm_trace_callers`) write the same POD. `to_value_map` and the crash reporter
materialise strings from ids. `LispError` stays heap-independent (§devlog: the lazy message
is blocked on exactly that), which the id representation respects — ids need no heap.

The per-`try` fixed cost — two closure allocations (`(fn () body)`, `(fn (e) …)`) and three
driver entries — is a separate item: `%try` as a **VM-level frame marker** (a `Try` opcode
that records the catch target and the roots/env lengths, with the body inline in the chunk)
removes both closures and two of the three entries. It is bytecode-compiler work, not
convention work; listed here because `errors-deep` needs both halves to move.

## 5. Part B — the VM's own call (~1 400 instructions)

Attributed on the 3M-iteration two-arm loop at `BROOD_TIER=1` (this tree): `exec_chunk`
61% (the interpreter proper — the body plus the `Call`/return instructions), `vm_run_bc`
15% (frame save/restore, the loop-top checks), `push_frame` 7%, `push_root` 4%,
**thread-local scheduler lookups ~6%** (`CURRENT`, `DEADLINE`, `REDUCTIONS` `with`
closures — three TLS probes per call), call-IC probe 3%, `extend_roots_to_nil` 1%.

Levers, cheapest first, each a one-session measurement:

- **B1. Hoist the TLS probes into the driver.** The scheduler context does not change
  within a quantum; read it once at `vm_run_bc` entry (or per quantum) into locals the
  call path reads. ~6% of the loop for a mechanical change.
- **B2. Frame push without the round trip.** §7.13 removed the VM→native round trip; the
  VM→VM `Call` still exits `exec_chunk` to the driver, saves the frame, pushes, re-enters.
  A direct push in `exec_chunk` (the callee's chunk run by the same loop instance, the
  driver informed by a lighter `ChunkExit`) is the same shape as §7.13's direct call.
- **B3. The IC probe** at 3% is already an IC; the re-validation (`sym`/`argc`/`epoch`)
  could be one compare on a packed word.

Together these are a 20–30% cut of the VM call, not a 5× — which is the point: **the VM
call cannot reach the native call's floor**, so Part B is a stopgap for the refused arms
while A makes admission win. Re-run `BROOD_XADMIT=1` (§7.12) after A lands: the
admission verdict was taken at 640 and re-taken at 218 instructions per call; at ~40 it may
flip, and then the interpreter's call stops being the number these rows pay.

## 6. Invariants every rung must keep (the sabotage list)

- Arguments and temporaries live in `roots[base..]` and are nil before the first safepoint
  that can see them — `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` over `tests/jit_*` and the
  fuzz differential are the gates; a register-carried handle across a safepoint is the
  KI-49 class and the verifier names the store site.
- The suspend-host latch fires for a park under the callee (`crates/cli/tests/vm_direct_call.rs`
  pins dirty parks at 0 for the `supervisor` shape; it must keep pinning them with the token
  changed to the frame base).
- KI-11: native recursion is bounded with the depth in a register — `jit_deep_recursion_test`
  and the three-function tail-delegator cycle.
- KI-20: the callee writes its own IC block — the `nbody` IC hit/miss counters
  (`BROOD_PERF_STATS`) are the gate (+24.5% and 304k misses is what forgetting it looks like).
- Hot reload: the epoch guard still invalidates a call whose callee was redefined
  (`jit_new_def_epoch_test`, `jit_self_rebind_test`).
- Effect-once: a deopt after the call must not repeat the call (`jit_effect_once_test`).
- The tier audit stays green (`make tier-audit`) and no row moves more than its floor the
  wrong way (`make ab --floor --all`, plus the two counted loops via `perf stat`).

## 7. The increment ladder, with its gates

| rung | change | gate |
|---|---|---|
| A0 | `JitArmFn` takes the context as arguments; Rust callers pass what they read; native code still writes nothing new | flat everywhere (`perf stat` on the call loop identical ±1%); full suite; fuzz |
| A1 | the inline blob stops saving/restoring rows 2–6; the cold path restores `jit_force_vm`/env roots | the call loop's instruction count (target ≤ 120 from 218); `bintree`, `pfib`, `nqueens`, `pipeline` under `ab --floor` |
| A2 | gateway token = frame base; latch compare unchanged | `vm_direct_call.rs` dirty-park count 0; call loop ≤ 100 |
| A3 | 2-register result + tag-gated third word | call loop ≤ 70; `sort`/`json`/`strings` (string results) flat; fuzz `strings` oracle |
| A4 | callee-prologue nil-fill by first-safepoint write set | call loop ≤ 50; GC stress + verify over every `jit_*` file, the KI-49 tuple matchers in particular |
| C1 | POD `TraceFrame` + inline arm-pointer push | `errors-deep` (expect ≤ 6%); `try_catch_test` trace assertions unchanged; crash-report tests |
| B1–B3 | the VM-call levers | the tier-1 loop (2 938/iteration) and `json`/`regex`/`nqueens` at default tier |
| — | re-run `BROOD_XADMIT=1` | if admission now wins on json/nqueens, that is the row-level payoff of A |

Each rung lands on its own, green, with its numbers in `docs/compute-frontier.md` §7 and
its decision as an ADR; a rung that reads noise on the counted loop is reverted (the
repo's standing rule).

## 8. Open questions (decide at A0)

1. **ABI width.** Seven integer arguments exceed SysV's six registers by one; either pack
   `ic_base`/`gic_base` into one word (both are `u32`) or pass `depth` through the frame.
   Packing is free; decide by reading `set_ic_bases`' consumers.
2. **Who owns `jit_call_env` for the Rust callers** once native code stops writing it: the
   gateway entry writes it for the outermost frame; every deeper frame's env is in its
   register/slot. Any Rust callback that reads `heap.jit_call_env` from *inside* a nested
   native frame (global-miss resolution, `%lookup-miss`) would read the OUTERMOST env —
   enumerate those readers first (`grep jit_call_env crates/lisp/src/jit/rt.rs`); the
   guarded case is GLOBAL-only, so today they agree by construction, and the answer may be
   "pass the env to the callback too".
3. **The third result word.** Which tags use `w2` — if only string aux data, A3's tag gate
   is one compare; if handles ever carry it, A3 needs a per-kind table.
4. **`%try` as a frame marker** (Part C's second half) is bytecode-compiler work with its own
   semantics questions (a `finally` under a suspend); it is not in this ladder.
