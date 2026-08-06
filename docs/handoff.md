# Handoff — what to do next, and the traps

**Replaced each session; this is the *current* picture, not history.** The narrative and the
measurements live in [`devlog.md`](devlog.md); decisions in [`decisions.md`](decisions.md); the
option book in [`runtime-frontier.md`](runtime-frontier.md); bugs in
[`known-issues.md`](known-issues.md). Read this to pick the work back up cold.

**As of 2026-08-06**, brood with ADR-213 (char→byte index), ADR-214 (form-start safepoints) and
ADR-215 (AST-keyed shared compiled code) on
top of the ADR-211/212 registry + package-signing work. Nothing half-finished — the 2026-08-06
receive-matcher session ends with a **clean tree**: its findings are docs (§1 item 1, §4, §6) plus
one new committed probe, and the code it wrote was measured, refuted and reverted. Rust
suite **960/960** (nextest), in-language **4410/4410** — also green under
`BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1` — `nest check` clean, `nest format --check` clean, rustfmt and
clippy clean, both default and `--no-default-features` builds warning-clean, metamorphic
differential clean across 4 engine configs. The 2026-08-05 batch (KI-29, KI-30, the
short-vs-long measurement rule, and two rounds of `spawn-live` refutations) **is pushed**;
`main` and `origin/main` agree. **Flake baseline** (2026-08-05): the in-language
suite is clean over 3 iterations × 3 seeds in one image, and `live_migration` is 16/16 under
self-contention. **No open issues; one watch item, not in the runtime** — KI-28, a single
unexplained `nodedown` flake seen once in a full run and not since (0/40 solo, 33/33 × 3 under
nextest, absent from the next 956/956 run); its diagnostic is armed, so a recurrence will explain
itself — and now that KI-29 is fixed, a recurrence can no longer be blamed on a stray node.
**KI-30 was fixed 2026-08-05**: seven `temp-dir` prefixes were never passed to `purge-stale-temp`,
which had left 4484 of 4622 `/tmp` directories (168 MB) across months of green runs. The mechanism
already existed and was documented — only seven lines were missing — and my filed fix direction (a
`with-temp-dir` helper) was the wrong shape. `tests/temp_purge_coverage_test.blsp` now scans each
file's source and fails on an unpurged prefix; the suite is flat at 128 directories over three
consecutive runs.
**KI-29 was fixed 2026-08-05**: the node/observe tests orphaned `brood` children (one found alive 9
days later, ~35% of a core across three strays) because a *killed* test binary runs no destructors.
`spawn_brood` now returns a `BroodChild` guard carrying two independent nets — `Drop` for a panicking
test, `PR_SET_PDEATHSIG(SIGKILL)` for a SIGKILLed binary — each verified by sabotage. Its filed
fix direction (a process group) was measured to be the *wrong* lever and not taken; see its entry.
KI-27 was fixed 2026-08-05: the three node-test harnesses
picked their ports out of the kernel's *ephemeral* range (32768–60999), so an unrelated process's
client socket could take the port a test node was about to bind. They now share
`crates/cli/tests/support/mod.rs` and allocate from a pid-sliced band below the ephemeral floor.
KI-25 (five suites failing when
re-run inside one image, which blocked `--repeat-until-failure` across the suite) was fixed
2026-08-04: four suites marked `:isolated` for the `%isolate` rollback, and `pid_identity_test`
now takes the one-shot `node-start` only when `(node-name)` is `:nonode`. The whole suite
re-runs clean in one image (4390/4390 twice), so `--repeat-until-failure` is usable for flake
hunting again.

---

## 1. START HERE — `spawn-live`: measure the candidate before you build it

**This section has now named the wrong lever three sessions running, and each time the mechanism
it named was the one *nearest the symptom* rather than the one carrying the cost.** Per-process
inline caches, then park/resume, then an identity-keyed IC, then "reach native code" — four
candidates, four refutations, each disposed of by one measurement that cost far less than the
implementation would have. §4 has them all. **So: before implementing anything below, measure
that its premise still holds.** Everything here is an argument, not a fact, until re-measured;
the facts are the ladder table and §4.

**What has actually been fixed on this row.** `fold` coerced with `seq` and walked with
`first`/`rest`, and `(rest v)` on a vector *materialises a list of the tail* — 15 cons cells and
~48 one-arg primitive dispatches to sum a 16-element payload. `fold` now indexes a vector
directly (`fold--vec`). Per unit: allocations **27.7 → 15.0**, one-arg dispatches **48 → 1.1**,
the shape **32.7 → 28.6 µs** CPU, the published row **11.44 → 10.40 CPU·s** (−9%). And ADR-215
fixed per-process recompilation (compiles 100 154 → 163 per 100k processes).

**Where the row now stands.** The ladder is now committed as
`scripts/fuzz/stress/spawn_live_ladder.blsp` (it was `sl_one.blsp`, uncommitted, so these
figures could not be re-derived). Run **one rung per process** and read CPU, not wall:

| rung | µs/unit (CPU, N=100k) | adds |
|---|---|---|
| `spawn` — spawn + exit | 6.9 | — |
| `send` — + an unread message | 7.1 | +0.2 |
| `nopark` — + a `receive` that never suspends | 15.8 | **+8.7** |
| `park-batched` — + every unit suspends (100 coexist) | 17.6 | +1.8 |
| `park` — + all N coexist | 22.2 | **+4.6** |
| `payload` — + the 16-cell copy and fold | 31.5 | **+9.3** |

The three big steps are the **receive machinery** (+8.7), the **payload copy and fold**
(+9.3), and **coexistence** (+4.6). Suspending is not one of them (item 4 below). The
earlier version of this table read 8.6 / 12.2 / 17.7 / 28.6 for the comparable rungs;
absolutes drift ~10% between invocations, so read the steps, not the levels.

So the next candidates, in the order the numbers support them:

1. **The receive machinery (+8.7 µs/unit) — the largest step. Its "caching" premise was
   measured and REFUTED 2026-08-06; what the measurement found instead is that the matcher
   never reaches the native fast frame.** Still the recommended thread, and still the widest
   reach of anything on this list — every message-passing row pays it (`latency`, `pingpong`,
   `supervisor`), not just HOF loops — but the mechanism is not the one this item named.

   **The premise was wrong.** `receive` is a *macro*: `match-build-from` lowers the clause set
   at macro-expansion time into a literal `(fn (msg) …)` whose body is a fully inlined
   `vector?`/`vector-length`/`vector-ref`/`%eq` if-tree. Dump it —
   `(println (macroexpand '(receive ([:go v] 1))))` — and there is no clause compiler left to
   run. `match-compile-clause` executes **once per site, at load**, never per message; the ~20
   `match-*`/`receive-*` arms seen tiering during the ladder are that expansion work, not
   per-message work. So there is no ADR-215-shaped cache to add here. Compiles are already flat
   (`BROOD_TRACE_COMPILE`: 169 vs 179 across rungs at N=2000), and `nopark` adds **no**
   per-unit promotion over `send` (`BROOD_TRACE_PROMOTE`).

   **What the cost actually is.** `hof_resolve` succeeds on **100%** of receives and
   `hof_apply_step` is entered on 100% — but `hof_apply_native` declines every time, so every
   matcher call pays the `vm_apply` → `vm_run_bc` trampoline. The matcher arm lowers to native,
   then **deopts on every activation**; where `deopt_watch` is set, sixteen in a row make
   `jit_deopt_feedback` mark it `BAILED` for the rest of the program
   (`DEOPT_BAIL_CONSECUTIVE = 16`). Same failure shape as the float-global bug in `CLAUDE.md`.
   It is *by design* as a self-heal — closure arms are deliberately exempt from the static
   call-mediated profitability gate so deopt feedback can judge them at runtime — but here the
   arm is being judged for something it should pass.

   **WHICH patterns pay it, bisected 2026-08-06** (`scripts/fuzz/stress/recv_matcher.blsp`,
   one long-lived process so tiering is not a variable; N=300k, perf-stats build):

   | `PAT` | pattern | compares a literal? | ns/iter | `jit_deopt` | `jit_link_done` |
   |---|---|---|---|---|---|
   | `any` | `m` | no | 1266 | 0 | 281 043 |
   | `bind` | `[a v]` | no | **1286** | 0 | 281 812 |
   | `vec` | `[:go v]` | yes (keyword) | **1776** | 281 978 | **0** |
   | `intlit` | `[1 v]` | yes (int) | 1763 | 281 467 | **0** |

   **It is comparing a literal element that costs the native frame** — and `bind` is the honest
   control, because it does the *same* vector work as `vec` (same `vector?`, same
   `vector-length`, two `vector-ref`s) and merely binds the head instead of comparing it. So
   the gap is **−28% of the whole receive loop**, not an artefact of work not done. (The
   earlier catch-all comparison put `ns_match_run` at 534 vs 112 ns and `ns_receive` at 935 vs
   485; treat those as the same story measured against a weaker control.)

   That also rules out three things at once: it is **not** the matcher's calls (`bind` calls
   `vector?` and `vector-length` and stays native), **not** the vector machinery, and **not**
   keyword-specific (an int literal deopts identically). None of `BROOD_NO_LEAF_INLINE`,
   `BROOD_NO_INLINE`, `BROOD_NO_PARTIAL_LEAF` or `BROOD_LINMAP=0` move it either — it is in the
   base lowering. `BROOD_DEOPT_TRACE=1` names the deopting arm as the matcher `<closure>`
   itself (`watch=false resume_ip=-1`).

   **Reach: this is the entire tagged-tuple idiom** — `[:go v]`, `[:reply ^r v]`, every
   supervisor and `gen` protocol message. Bind-only patterns are the rare case; essentially
   every real `receive` is on the slow side of this line.

   **The open question, and it is now a narrow one: which guard in the lowering of
   `(if (%eq el <literal>) … nil)` branches to the deopt block?** Read the matcher's CLIF
   (`BROOD_JIT_DUMP_IR=1`; the deopt block is the one returning `1`, and `block16`'s
   unconditional jump into it is only the stack-guard prologue — ignore it). Worth knowing
   before you start: `eq_dispatch` (`jit_lower/emit.rs`) looks *correct* on paper for both
   operand shapes — int×int compares payloads, and either side Sym/Keyword compares interned
   ids — so the guard is probably **not** `eq_dispatch`, and the `if`/branch structure around
   it is the next place to look. Do NOT navigate by the deopt's `resume_ip`: it names the
   nearest checkpoint, not the failing guard (§6).
2. **The payload step (+9.3 µs/unit) — and the copy is NOT the cost.** `ns_msg_in` *fell* (216 →
   180 ns/unit) when the message grew from `[:go]` to a 16-element vector, so the deep copy this
   row was built to measure costs ~0.2 µs of a 31 µs row. Of the +12.5 µs the rung adds under
   perf-stats, only ~1.3 µs (10%) lands in any runtime timer; the rest is Brood-level `fold`
   work, which corroborates the older finding that hand-rolling the sum as an indexed loop
   recovers ~10.7 µs.

   Per element, warm: **hand loop 10 ns · `fold %add` 78 · `fold +` 163 · `fold myadd` 231**. Even
   the best HOF case is ~7× an inlined op. The wrapper's ~85 ns goes to `passthrough_arm` (closure
   deref + `select_arm` + a `SmallVec` **clone** of the arg map), two thread-local ticks in
   `passthrough_redirect_ok`, a fresh argv `SmallVec`, then `call_native`'s checks — five small
   costs, none dominant. **Measured and reverted:** memoising the redirect target on the arm is
   worth **2%** (167 → 163 ns); don't re-try it.

   **The only lever left for this shape is not calling per element** — an identity-guarded
   speculative inline of the HOF's step closure. Groundwork is further along than FRONTIER's
   "true call inlining" bullet suggests: ADR-210 already splices *statically known* leaf callees
   with a deopt checkpoint, and the missing piece is a guard on the step closure's identity. This
   is the one candidate in the neighbourhood that changes the *shape* rather than a constant —
   and note that the JIT gives this shape nothing today (a lowered `loop-computed` measures 274
   ns/call with the JIT and 271 without), so inlining is the whole prize.
3. **Coexistence (+4.6 µs/unit) — the cost of a live *idle* process.** 22.2 µs/unit with 100k
   alive vs 17.6 with 100, same parking either way. This is the ~4.27 KB floor plus the GC/cache
   pressure of 100k live heaps. §4 records that attacking the floor by boxing `Heap` in `Process`
   is the wrong trade (`spawn` +3.2%, `spawn-live` +6.4%) and that the direction is cutting the
   *number* of allocations per process, measuring the `spawn`/`spawn-live` pair alongside the
   floor from the start.
4. ~~**Park/resume (5.5 µs/unit)**~~ — **measured 2026-08-05: suspend/resume costs ~0, and
   this item was a confound.** The rung that produced 5.5 µs changes **two** things against
   the one below it, which its own wording admits ("+ every unit held alive, *so* each
   parks"): every unit suspends, *and* all N are alive simultaneously. Separating them with
   `scripts/fuzz/stress/spawn_live_ladder.blsp`'s `park-batched` mode — units still park, but
   only `BATCH` coexist, verified by `BROOD_L1_STATS` showing one parked-receiver hit per
   unit — at `BATCH=1000`, five interleaved runs each, best CPU ms per run:

   | | runs | median |
   |---|---|---|
   | `nopark` (never suspends) | 1589 · 1710 · 1599 · 1589 · 1609 | 1599 |
   | `park-batched` (every unit suspends) | 1560 · 1560 · 1560 · 1540 · 1589 | **1560** |

   Parking is **2.5% cheaper**, not 5.5 µs dearer — consistent with ADR-178, whose local-send
   fast path fires *only* on a parked receiver, so suspending puts the wake on the fast path.
   The whole delta is **coexistence**: `park` (100k alive) 22.2 µs/unit vs `park-batched`
   (100 alive) 17.6 vs `nopark` 15.8.

   **So the lever is the cost of a live idle process, not the parking mechanism** — the
   ~4.27 KB floor and the GC/cache pressure of 100k live heaps. §4 already records that
   attacking the floor by boxing `Heap` in `Process` is the wrong trade, and that the
   direction is cutting the *number* of allocations per process.

   **Two traps this measurement walked into, both worth keeping.** The batch curve is
   **U-shaped**, not monotonic (BATCH 10 → 19.9 µs, 100 → 17.7, 1000 → 15.7, 10000 → 17.7,
   all → 22.5): a small batch serialises on the parent (spawn K, release K, collect K, ×N/K)
   and reads as a *high* per-unit cost that has nothing to do with coexistence, so picking
   the endpoints of that curve would have "confirmed" either story. And the ladder must be
   run **one rung per process**: in a single process the later rungs inherit the earlier
   ones' JIT tiering, which put `payload` *below* `park` and `send` below `spawn` — a
   monotonicity violation that is the signal the run is contaminated.
5. **Per-process inline caches (~2 µs)** — real but small, and now correctly sized. If you do it,
   the site-id work is already done; what is left is the race design for a shared block. Note the
   identity-IC result in §4 before assuming an IC buys anything here: on the VM, a cached callee
   measured *slower* than resolving one.

Measure with **CPU time over a fixed unit count, binaries interleaved** (<2% spread). The 20.6%
"noise floor" this row was once credited with is an artefact of measuring *wall* on a 3.3-core
workload. `BROOD_PERF_STATS=1` on a `--features perf-stats` build gives `ns_*` timing shares
(`ns_quantum` nests the rest) and per-unit counters; `BROOD_TRACE_COMPILE=1` names every compile.

## 2. Then: rope-native structural motion, the editor half of the `sexp` story

ADR-214 made a *sequence* of structural motions linear when the caller holds one text value —
the tooling/LSP shape. The **editor** shape is untouched and cannot be fixed by that cache:
`sexp/forward` and friends call `(buffer-text buf)`, which is `rope->string`, so every motion
allocates a fresh O(n) string before any scanning happens, and the safepoint table can never
hit. A keystroke-driven motion is therefore O(buffer) no matter how fast the scan is.

The options, in the order I would try them:

- **Motions over the rope.** `std/tool/sexp.blsp` is written against `(text point) -> point`
  with `text` a string; `parse-source-positioned` and `scan-form-start` both take strings. A
  rope-native path needs at least a rope-shaped form-start scan (ropey exposes chunk iteration,
  so the same safepoint idea applies) and a decision about whether the CST walk slices a rope
  or a string.
- **Or: let the caller hold the string.** A command loop that keeps one `text` value across a
  run of motions gets ADR-214's cache for free. Cheap, and it only helps a *sequence* of
  motions between edits — worth checking against how the downstream editor actually drives it
  before building anything in the kernel.

Measure `(buffer-text buf)` first: if the stringify dominates a single motion at realistic
buffer sizes, that number decides which of the two above is worth the work.

## 3. Then: the `expect_string` copy seam, by body cost

`expect_string` returns an **owned** `String` at ~105 remaining call sites — one copy of the
argument per call. Eight were converted on 2026-08-04, and the measurement gives the rule:

- the copy is worth removing where the call's own work is **O(1)** and the argument is a whole
  buffer — `(grapheme-at txt 0)` on 212 KB measured **−74%**;
- it is **noise** where the body is per-char — `grapheme-count` / `string->codepoints` over the
  whole string measured −0.6%, inside the drift floor, because UAX #29 segmentation dwarfs a
  memcpy.

So triage by what the body costs. Two sites **cannot** take the borrow at all: `string-split`
and `scan-tokens` allocate per piece *while* scanning, and `string-split` would gain nothing
anyway (its copies are the parts, not the input). `scan-tokens` would need a two-pass rewrite
(collect ranges, then allocate) — worth it only if the fontifier shows up in a measurement.

## 4. Closed — do NOT re-attempt these

Each was measured to a conclusion. Re-deriving them costs a session each.

- **Three explanations for the receive matcher's deopt** — all measured and **killed
  2026-08-06**, in the order they were tried. Each cost a build; re-deriving them costs the
  session. The symptom under test throughout: the matcher arm deopts 16× and is marked
  `BAILED`, so every receive pays the `vm_apply` trampoline (§1 item 1).

  1. **"It is the non-tail calls (`vector?` / `vector-length`) — make them prims."** No. The
     deopt's `resume_ip` sat immediately after a call, which reads as causal. It is not: the
     `resume_ip` names the nearest *checkpoint*, and checkpoints sit after calls. Implemented
     `PrimOp1::VectorLen` (IR + both VM exec paths + a Cranelift `inline_vec_len` mirroring
     `inline_vec_ref`) — the chunk's `Call` duly became `Prim1`, and the deopt simply **moved
     to the other call**. Removing that one too (the matcher generator emitting
     `(%eq (type-of t) :vector)` instead of `(vector? t)`) left a **call-free** matcher that
     deopts on **every** activation with no checkpoint at all (`resume_ip = -1`) and no longer
     self-heals — measurably **worse** (1936 vs 1627 ns/iter, same build), because losing the
     `BAILED` latch means paying a failed native entry per call forever. **Reverted; nothing
     of it is in-tree.** If a later thread wants the `VectorLen` prim for its own sake, it is
     ~120 lines and mirrors `inline_vec_ref` (`jit_lower/emit.rs`) exactly — but it must be
     justified on its own measurement, because on this row it bought nothing.
  2. **"The message vector is not LOCAL, so the inline vector ops deopt on the region check."**
     No — `inline_vec_ref`/`inline_vec_len` do deopt on a non-LOCAL handle, but instrumenting
     the deopt to print the operand's region gave **`region=0` (LOCAL) on every one** of 42 121
     deopts.
  3. **"The clauses are re-parsed/re-compiled per execution"** — the original §1 premise. No;
     see §1. `receive` is a macro and the matcher is an inlined if-tree.

  Two things worth keeping from that hunt. **`jit_link_done` is the counter that answers "did
  the HOF fast frame engage?"** — 0 against ~N calls is the whole diagnosis, and no other
  counter shows it (`jit_native` stays high because the *caller* is native). And
  **`BROOD_NO_HOF_JIT=1` measuring flat is not evidence the native path is worthless** — here
  it measured flat because the path was never taken at all.

- **"Make a short-lived process reach native code"** — measured and **declined 2026-08-05**,
  before being built. The premise came from `spawn-live` gaining *nothing* from the JIT
  (`BROOD_NO_JIT=1`: payload rung 4310 → 4280 ms, park rung 2050 → 2050) while
  `BROOD_JIT_DUMP_IR` shows 171 arms lowering — which reads as "the native code is compiled
  and the short-lived units never get to it", i.e. an ADR-215-shaped hole (share the *tier
  decision* the way ADR-215 shared the *code*).

  Both halves are wrong. **The units' own arms do lower** — `fold` and `fold-vec` each appear
  in the dump (twice, the two-stage dual body), as do `receive`, the `match-*` family, and
  `<closure>`. And **native is not faster for the shape that dominates**: `hof_call.blsp`'s
  `loop-computed` lowers (confirmed in the dump) and still measures 274 ns/call with the JIT
  and 271 without. A HOF-call-dominated loop pays for the *call*, which re-enters the runtime
  either way, so there is no native win to reach for.

  **What it converges on instead:** the only lever left for this shape is not calling per
  element — the identity-guarded speculative inline of the step closure that §1 named all
  along. Everything else in the neighbourhood has now been measured and declined: the IC
  below, park/resume, and this.

- **An identity-keyed call-site IC for a computed (local) callee** — measured and
  **declined 2026-08-05**, before being built. §1 recommended it because `compile_node`
  allocates an IC id only for a free-global head, so a HOF's step call re-resolves per
  element. The premise is real and the *cost* is not: with `scripts/fuzz/stress/hof_call.blsp`
  (3M calls, callee reached as a global vs as a parameter, same callee, **same arity**, a
  non-inlinable body so the comparison is a call and not a splice) —

  | | JIT on | JIT off |
  |---|---|---|
  | global head (IC + fast-link) | 242 · 227 · 245 ns | 247 · 246 · 276 ns |
  | computed head (no IC) | 263 · 248 · 267 ns | **237 · 239 · 266 ns** |

  On the VM the computed callee is *faster* — three runs each way — because the global path
  pays an IC probe and validation while the computed path just reads a slot. So the thing
  the IC would cache (`passthrough_arm` probe + `compiled_arm_for`) costs about nothing to
  recompute, and the ~21 ns (8%) gap that does appear under the JIT is the **native
  fast-link**, not the cache; capturing that needs an identity-keyed `FastLink` slot, which
  is KI-20 territory, for 8%.

  **Checked short AND long, because a tiered runtime has two steady states** (the rule is
  now in `CLAUDE.md`). Sweeping the call count over four orders of magnitude, gap =
  computed − global:

  | N | 10k | 100k | 1M | 10M |
  |---|---|---|---|---|
  | warm gap | +18 ns | +23 | +19 | +23 |
  | cold gap (single pass, no warm-up) | **−63 ns** | +17 | +18 | +22 |

  The warm gap is flat at 7–8% from 10k to 10M, so the verdict is not an artefact of a
  half-tiered arm — and `BROOD_JIT_DUMP_IR` counts the **same 28 arms lowered** at 10k and
  1M with zero deopts, i.e. tiering is already complete by ten thousand calls. The cold
  column makes the case *against* the IC stronger, not weaker: on a short run the **global**
  arm is 63 ns/call worse, because it is the one paying IC install and tiering cost. An IC
  would be worth ~0 for long-lived work and negative for the short-lived kind.

  **The trap that nearly sold it, worth keeping:** the first version of that benchmark used
  `(defn step (acc x) (%add acc x))` as the callee and measured the global head at **1
  ns/call against the computed head's 160** — an apparent 160× that reads as a screaming
  case for the IC. That shape is a *passthrough to a `%`-native*, which `resolve_prim`
  (`compile/mod.rs:668`) inlines to a `Prim2` at the call site, so the row was measuring a
  deleted call. A callee is only measuring a *call* if it cannot be inlined — and a row
  reporting ~1 ns/call is reporting that its work is gone, which is why the committed
  version prints total ms and the accumulator beside every figure.

- **`spawn-live`'s per-process recompilation** — **fixed** (ADR-215): the compiled-code cache
  was keyed by the closure *handle*, and a no-capture closure is promoted afresh per creation
  (ADR-194), so every `spawn` thunk and `receive` matcher missed and every process recompiled —
  100 154 compiles per 100 000 units at 8.1 µs. Keyed by AST now. Do not re-attempt the
  *mechanism* (ADR-175 shipped it correctly); the bug was the key.
- **Nine scheduler/messaging switches as an explanation for `spawn-live`** — all measured
  neutral on it (`NO_HANDOFF`, `NO_STEAL_WAKE`, `SPAWN_RR`, `SPAWN_SPILL`, `NO_RECV_MARK`,
  `NO_JIT`, `MIMALLOC_PURGE_DELAY=0`, `NO_SHARE_FN`). The row is not a scheduling problem.
- **`sexp motions`, the last quadratic in the sweep** — **fixed** (ADR-214): the form-start scan
  resumes from a safepoint table cached against the string value, and runs over bytes instead of
  a per-call `Vec<char>`. Ratio 7.97 → 3.88, 18570 → 6146 ms at 12800 forms, linear on four
  rising bases. Do not re-attempt it as a *constant-factor* fix: two of those landed first and
  left the shape untouched. What is left is the **editor** path (§2), which this cannot reach.
- **`sse--frames`** — **fixed**: one `string-split` instead of a `substring` of the rest per
  event, 5671 → 15 ms at 25600 events, with its own sweep row.
- **`editor/lineedit`'s per-keystroke geometry** — measured and **declined**: 2.5 ns/char, so
  28 µs at 10 K chars and 2452 µs at 1 M. Under a frame even on a 1 MB pasted line.
- **Char→byte conversion for non-ASCII strings** — **fixed** (ADR-213): a sparse char→byte index on
  the string slot, so a char index costs a lookup plus a walk bounded by 32 chars in either
  encoding regime. `inc-scan` 16.85× → linear; `sexp motions` lost its 9.80× → 5.42× encoding
  penalty. Do not re-apply the *char-count cache* idea expecting more: its mechanism is the ASCII
  test itself, which is why it never reached the slow path.
- **Forcing `mandelbrot`'s `row-sum` onto the native path.** Refused by the call-mediated
  profitability gate in `jit_lower_arm`; exempting it makes `mandelbrot` **+0.7%** and `matmul`
  **+5.1%** (0.3% floors). The arm *does* lower under the exemption — it simply is not faster. The
  real `mandelbrot` lever is removing the boxing (unboxed floats across call boundaries), a much
  larger piece of work.
- **Boxing the `Heap` inside `Process`** (memory-floor experiment (a)). `Process` 1304 → 112 bytes
  and the floor fell 4273 → 4124 B/proc (−3.5%), but it adds a second allocation per spawn:
  `spawn` **+3.2%**, `spawn-live` **+6.4%**. Wrong trade; reverted.
- **The allocator size-class histogram** (experiment (b)) — retired by (a)'s shape: `Process` shrank
  1192 bytes of struct and the floor moved 149, so size-class rounding is not the dominant term. The
  floor is spread thin across ~25–30 blocks. If anyone wants that row, the direction is cutting the
  *number* of allocations per process, not their sizes — and (a) shows that pulls against `spawn`,
  so measure the `spawn`/`spawn-live` pair alongside the floor from the start.
- A memory leak (chased, does not exist); endurance (16/16 soak, 12.7 M iterations); thread 6's
  throughput decay (fixed, ADR-208); the RUNTIME reclamation threshold (dissolved); per-message cost
  as an explanation for `latency` (it was spawn placement — send+receive is 1.1 µs).

## 5. What shipped recently, and how to turn each piece off

Every mechanism has an off-switch, because each is an optimisation whose fallback is the old
behaviour. **If something misbehaves, bisect with these before bisecting commits** — and for a
mechanism with a switch, the switch on ONE binary is the attribution (§6).

| Change | Off-switch | Worth |
|---|---|---|
| Compiled code keyed by the closure's AST, shared per runtime (ADR-215) | `BROOD_NO_SHARED_ARMS=1` | `spawn-live` wall −12.5%, CPU −25%, RSS −14%; bytecode compiles 100 154 → 163 per 100k processes |
| Form-start safepoint table on the string value (ADR-214) | — (a cache that changes no answer; gated by equality with the pre-table scan at every position) | `sexp motions` 7.97× → 3.88×, 3.0× at 12800 forms |
| Sparse char→byte index on the string slot (ADR-213) | — (a cache that changes no result; its gate is that its answers equal the walk's) | multi-byte char indexing **96×** on a micro (60.2 s → 0.62 s); `inc-scan` 16.85× → linear; `sexp motions` 9.80× → 5.42×; ASCII flat |
| Partial leaf splicing (ADR-210) | `BROOD_NO_PARTIAL_LEAF=1` | 2.4× on a lowering caller with a leaf beside a residual call; every published row flat |
| Shared closure crosses a **serialised** send by handle (ADR-208) | `BROOD_NO_SHARE_FN_MSG=1` | `rt_closures` 143,752 → 66 constant; RSS 213 vs 502 MB |
| Idle peer told at once that a peer queued a child | `BROOD_NO_STEAL_WAKE=1` | `latency` p50 27 → 19 µs, p99 124 → 78 µs |
| Owner keeps first refusal on a fresh child | `BROOD_STEAL_GRACE_NS=<n>` (0 disables) | protects `supervisor`; the cliff is at 2.5 µs on this machine and the default takes a 2× margin deliberately |
| Closure sends share already-shared code, **parked** path (ADR-194) | `BROOD_NO_SHARE_FN=1` | retained closure 436 B → 48 B |
| Spawn placement spills off a backlogged worker | `BROOD_SPAWN_SPILL=999999` / `BROOD_SPAWN_RR=1` | `latency` p50 5×, p99 2.9× |
| Receive-mark (ADR-195) | `BROOD_NO_RECV_MARK=1` | backlogged reply O(backlog) → O(1), 653 → 4 µs at 32k |
| Fast-link deopt shape check is flag-free (KI-26) | — (correctness) | a peer's stale link no longer re-runs a journaled effect from ip 0 |
| Registry updates are atomic (KI-22/23, ADR-202) | — (correctness) | ~40% of concurrent registrations were being lost |

**`std/` quadratics fixed** (each with a `scale_sweep.blsp` row so it cannot come back):
`template/render` 318→24 ms · `last-index-of` 540→1 ms · `strip-ansi` 1583→109 ms ·
`stream-lines` 303→39 ms · `format-source` 3593→1988 ms · **`sexp` motions 12061→6037 ms (2.0×)** ·
**`markdown-spans` multi-byte 1287→559 ms (2.3×)** · **`inc-scan` multi-byte 118→3 ms (ADR-213)** ·
**`sexp motions` 18570→6146 ms, quadratic→linear (ADR-214)** · **`sse frames-1c` 5671→15 ms**.

**Every row in the sweep is now linear in both encoding regimes.** Its job from here is
regression detection, which means running it **both** ways (`UTF8=1`) and checking the ratio
*trend* across bases, not one triple.

## 6. Traps — every one of these cost real time

**Measurement**

- **For a mechanism with an off-switch, the switch IS the attribution; a two-binary delta is only a
  hint.** One binary, one invocation, `MECHANISM=off` vs on. That disposed of a `nqueens` −5% in
  seconds (the switch is worth 0.3% there, so the mechanism cannot be worth 5%). Reach for it
  *before* building a fixed-baseline harness.
- **Hand-measuring against a `make ab` baseline requires `target/release-fast/brood`, not
  `target/release/brood`.** `make ab` builds both sides with `make release-brood` (profile
  `release-fast`); comparing its baseline against a `cargo build --release` binary compares two
  profiles and yields confident nonsense — it gave me `nqueens` −4.3% and `startup` −5.6%, both
  fictional. Footgun #1 in `ab-bench.sh`'s own header.
- **An optimisation whose mechanism is a fast-path test cannot clear the slow path, and a corpus
  that only exercises the fast path will report that it did.** The char-count cache was exactly
  that shape; so is anything gated on `is_ascii`. Sweep **both** encoding regimes.
- **A ratio near 4× that RISES across bases is not linear.** `format-source` read 3.80/4.12/4.64 and
  was cleared as linear; pushing the base gave 4.46 then 6.40. Only a *falling* ratio (warm-up)
  clears a row. Check the trend across triples, not one triple.
- **A row's "noise floor" can be an artefact of the metric.** `spawn-live` was credited with a
  20.6% floor and treated as unmeasurable for weeks; that was **wall** time on a 3.3-core
  workload. Measuring **CPU** time over a fixed unit count, with the two binaries interleaved,
  gives a <2% spread on the same row — enough to resolve a 12% change. Before accepting that a
  row cannot be measured, try measuring something else about it.
- **A cache that cannot be observed missing looks like a cache that works.** ADR-175's shared
  compiled-code cache shipped with the right mechanism and the wrong key; nothing in the suite
  or the benchmarks could tell, because the only symptom was slowness. The counter that catches
  it — compiles should be ~one per arm per *run*, never per *process* — is now `n_compile`, and
  `BROOD_TRACE_COMPILE=1` names the offender. Ask of any cache: what would I measure to see it
  missing?
- **Before believing a small two-binary delta, measure a row that CANNOT be affected by the
  change.** ADR-213's ASCII micro read `char-at` +2.1% and `last-index-of` +2.4% against 0.0%
  floors, and I spent two builds reshaping hot paths to chase it — the reshapes made it *worse*
  (+5.5%). `wordcount` calls none of the changed code and read **+2.2%** on the same binary pair:
  ~2% of whole-binary codegen/layout drift, and every string row was inside it. A size argument
  works too, and faster: `last-index-of`'s delta scaled with a byte scan costing ~90 µs per call,
  and no per-call change of a few instructions accounts for 2.4 µs.
- **Establish the noise floor first — and the floor measured *inside* one invocation does not bound
  the drift *across* invocations.** `nqueens` read −5.0% against a 0.2% base-vs-base floor while the
  same binary measured 104.6 and 107.6 ms in two best-of-15 runs.
- **A short row needs a mean, not a best-of.** `startup` is ~17 ms and `make ab` reports whole
  milliseconds, so it reads ±6% from quantisation alone; a 40-run mean gives +0.4%/0.4% floor.
- **Discard the first run after a fresh build.** Cold boot cache reports a ~44 MB base instead of
  ~24 MB in `process_floor.blsp` — the same size as the effect being measured.
- **Measure the slope, not the ratio.** RSS/N folds the runtime's ~24 MB base into a per-process
  figure; that is how the memory floor was once recorded as 5.9 KB when it is ~4.27 KB.
- **A harness can measure itself.** `process_floor.blsp` retaining the spawned pids put N cons cells
  of per-process cost into the very slope it measured (4470 vs 4271 B/proc).
- **Load contaminates everything.** `ring` "regressed" 12%, `supervisor` 8%, a sweep row 3.7% — all
  gone on a quiet machine. Wait for load < 0.5.
- **Never difference time-boxed runs** — RSS tracks iterations, so the comparison measures the
  iteration count.
- **A full disk looks like a toolchain crash** (`ld terminated with signal 7`). Check `df` first.
  `make ab-clean` is not automatic — ~1.1 GB per baseline worktree.
- **`RSS is not a proxy for live bytes`** here — but check before blaming the allocator:
  `MIMALLOC_PURGE_DELAY=0` moved the per-process floor by 2%.

**Testing**

- **"Cannot reproduce it locally" is evidence, not an obstacle — read what the passing configs rule
  out.** KI-27 passed 7/7 solo, 16/16 as concurrent copies of its own binary, and 3/3 as its whole
  binary under 12 CPU hogs, and failed only in a full `make test`. That set of results *is* the
  diagnosis: the cause has to be something only a full, heterogeneous suite supplies. It was other
  processes churning TCP connections, because the harness drew its ports from the kernel's
  **ephemeral range** (32768–60999) — so an unrelated client socket could be handed the port a
  test node was about to bind. Never allocate a test server's port with `bind(":0")`-and-drop.
- **A `Mutex` between tests does nothing under `cargo-nextest`.** The dist harness had a `PORTS`
  mutex around bind→spawn, which reads like the race was handled. nextest gives each test its own
  **process**, and `make test` uses nextest — so in the only configuration that fails, that
  mitigation does not exist. Any cross-test coordination has to be OS-level (the port band, a file
  lock), not a `static` in the test binary.
- **Copy-pasted harness helpers hide a bug in the copies you didn't look at.** `free_port` lived in
  three test files; fixing KI-27 in `distribution.rs` left the identical latent flake in
  `serve_attach.rs` and `observe_attach.rs`. They share `crates/cli/tests/support/mod.rs` now.
- **Build the concurrent reproducer before arguing about a flake rate, and compare failure *modes*
  not counts.** `live_migration deep_receive_…` failed inside a full `nextest` run and never in
  isolation (0/65). Two different failures were conflated: HEAD fails it on a *liveness* assert,
  while the change under test failed with an **out-of-bounds `root_at`** — a real GC bug. 16
  concurrent copies of that one test separated them in seconds: **8/16 vs 0/16 at HEAD**, where the
  full suite gave a 1-in-8 murmur that six baseline runs had failed to contradict.
- **A test-level `:isolated` inside a `describe` used to be silently dropped** —
  `register-test!` discarded the flag while collecting, so the marker did nothing and the suite
  reported `0 isolated`. Fixed 2026-08-05 (it rides in the meta; `emit-describe!` gives such a
  test its own isolated unit), but the lesson generalises: **a marker that is ignored rather than
  rejected is worse than an unsupported one**, because you believe the test is protected. Check
  the `(N isolated)` count in the summary when you rely on it.
- **A green test proves nothing until you run it with the mechanism off.** For any mechanism with an
  off-switch, run the test with the switch off before committing it. For a mechanism with **no**
  switch (a pure cache, like ADR-213's index), the equivalent is **sabotage**: break it by one
  character and confirm every new test fails. If they don't, they are not gates.
- **Verify a detector before trusting it — but "make it fire" need not mean "reproduce it end to
  end".** KI-26's runtime detector could only fire by winning a race, and never did across the
  suite, `pfib`, and a 24-process purpose-built race. The hazard was a *predicate*, so extracting
  the predicate and table-testing it was both possible and stronger: it covers both flag states and
  every nearby frame size, which no amount of hammering would.
- **Before adding a nextest retry, ask what else that test guards.** `live_migration`'s liveness
  assert flakes under load, but its *other* assertion catches intermittent continuation corruption —
  the very bug it caught in ADR-210. A retry would have absorbed that as FLAKY. Fixed by raising the
  burst budget instead (8/60 → 0/60, free on a normal run).
- **A derivation firing is not an optimisation landing.** `BROOD_INLINE_DBG` reported a
  partially-spliced derivation for `row-sum`; `BROOD_JIT_DUMP_IR` showed it never lowered. Leaf
  inlining is JIT-only and the VM always runs the small body, so a bailed arm gets nothing. A bailed
  arm never reaches the `[jit-ir]` dump, so *absence* there is the signal.
- **`std/*.blsp` is embedded at build time.** Rebuild `brood` **and** `nest` after touching `std/`,
  or you will debug yesterday's bytes. Same class as `-p brood` vs `--bin brood`.
- **The conformance tests need `nest test`, not `--test`** — they `(:use corpus)`, which only
  resolves through the project's module path. `brood --test tests/conformance_utf8_test.blsp` fails
  with "cannot find module 'corpus'", which is a harness error, not a failure.
- **`pkill -f <pattern>` matches your own shell** — it killed my own command twice in one session.
  Use `pgrep -f "[h]arness.py"` and kill by PID.
- **Process death reports go to stdout** — `2>/dev/null` will not filter them.

**Diagnosis**

- **"Restrict the scope" is not automatically simpler than "synchronise".** I had written up
  LOCAL-slots-only as the smallest sound way to populate ADR-213's index, because the shared regions
  race; a `OnceLock` turned out to be *smaller* (no region split in the accessor) and broader. When
  the cached value is a pure function of immutable data, a race between builders is benign — and
  this kernel has immutability everywhere (ADR-026), so reach for that argument before narrowing a
  feature's reach.
- **When a fix underdelivers against a mechanism you were confident about, that gap is evidence
  about where the cost actually is.** The `sexp` allocation fix I predicted was worth −18%; being
  disappointed by it sent me back and found the real one (two O(point) passes where one suffices,
  −39% more). I would otherwise have written up a true and incomplete story.
- **Threads get named after the mechanism nearest the symptom, not the cause.** Two of four were
  misnamed — and §1 was misnamed *again* on 2026-08-05: "cold inline caches" was the nearest
  plausible mechanism to "a fresh process is slow", so it became the item, and a ladder delta was
  read as confirming it. One measurement (a per-call warm-up curve) disproved it. Re-derive a
  thread's premise before implementing against it, and prefer a curve to a counter.
- **A deopt's `resume_ip` names the nearest CHECKPOINT, not the failing guard.** Checkpoints sit
  after calls, so a call-mediated arm's deopt always *looks* like it happened at the call — and
  removing the call moves the reported site to the next call, which reads as confirmation. It
  is not: with every call removed the arm still deopted, now with no checkpoint at all. To find
  a guard, read the CLIF (`BROOD_JIT_DUMP_IR`) for the branch into the deopt block; the ip is a
  hint about *where execution resumed*, never about what failed.
- **An off-switch measuring flat can mean the path was never taken.** `BROOD_NO_HOF_JIT=1` moved
  the receive micro 0.0% — which reads as "the native fast frame is worthless here" and is
  wrong. `jit_link_done = 0` showed the frame had never engaged on a single call. Before
  concluding a mechanism is worth nothing, check that it *ran*: for a mechanism with a success
  counter, the counter beats the A/B.
- **A probe that doesn't exercise the path reports confidently about nothing.** An A/B of two
  closure shapes through `fold` was built to test the HOF-step deopt — but `fold` is *Brood*
  (`fold--vec`), not a Rust HOF driver, so `hof_resolve` was never called and both arms'
  tallies were other arms' work. The tell was there: the `resolve:OK`/`step:enter` counters read
  **zero**. Check that your instrument fires before reading its output.
- **A counter is not a timing.** IC misses ran at 58% of call sites, which was true and cost ~2 µs
  of a 33 µs row. A high miss *rate* is unavoidable on a process that makes five calls; only the
  `ns_*` timers and a per-call curve sized it.
- **A comment asserting a cost is not evidence** — and a docstring can describe a bound's *intent*
  rather than its achievement. `sexp/narrow` says motions cost "~three forms, not the whole buffer";
  true of the CST work it wraps, false of finding the window, which was the whole cost.
- **Read the existing argument before inventing one.** ADR-194's comment named exactly why sharing is
  sound on the parked path, which identified what the serialised path lacked.
- **A benchmark port drifts silently when language semantics change under it.** `mandelbrot` looked
  like a 3.5× regression bisected to exact rationals; identical source measures 201 vs 200 ms —
  `(/ px n)` had simply stopped being a float divide. When a numeric primitive's semantics change,
  grep the benchmark ports.

## 7. Semantics worth knowing (documented, not bugs)

- **Char indexing costs O(1) in both encoding regimes** (ADR-213). A char index *is* a byte offset
  for pure-ASCII text; off that path the string slot's sparse char→byte index makes the conversion a
  lookup plus a walk bounded by 32 chars. So `substring` is O(result), and a `char-at` loop or an
  `index-of` scan with a rising `from` is linear on any text. The code-point-vector rewrites this
  class of bug once forced (`url`, `csv`, `ansi`) are no longer required — they are left alone.
- **A cache keyed to a string VALUE goes on the slot's `StrAux` cell, not in a map keyed by
  `StrId`.** A handle is unique only within a GC epoch; the cell travels with the bytes. The heap
  owns the cell as `dyn Any` and never interprets it, so a higher layer can cache its own table
  (ADR-214's lexer safepoints) without the core depending on it.
- **`(buffer-text buf)` is `rope->string` — a fresh O(n) string per call.** Anything that calls it
  per keystroke is O(buffer) per keystroke before it does any work of its own. This is why the
  editor gets nothing from ADR-214 (§2).
- **Hot reload does not reach a self-recursive loop.** A tail self-call compiles to
  `Node::SelfCall`, which re-runs the arm without resolving the callee. Redefining any *other* global
  the loop calls does reach it. Erlang's local-vs-remote rule; see `live-editing.md`.
- **A closure that captures no locals is already shared code**; one that captures a local is copied
  on send. That is why supervisor `:start` thunks should avoid captures — ADR-194/208.
- **`/` is exact.** `(/ 3 4)` is the rational `3/4` (ADR-196); `(/ 4 2)` is `2`. Use `quot` for an
  integer count, and convert to float *before* dividing in a float pipeline.
- **`->float` is a function call, not a cast** (~85 ns).
- **Leaf inlining is JIT-only** — the VM always runs the small body, so an arm whose native bails
  gets nothing from any splice.
- **Duplicate supervisor `:id`s** resolve to the later-started child.

## 8. Tools

All in `scripts/fuzz/stress/`, each with a usage header worth reading first.

- **`scale_sweep.blsp`** — a `std/` op at N and 4N, ratio printed (linear ~4×, quadratic ~16×).
  **`UTF8=1` re-runs every row in the multi-byte regime.** Its header records which rows are
  cleared, which were cleared *wrongly*, and why. **Every row is linear today**, so a
  superlinear reading is a regression, not a known gap.
- **`leaf_splice.blsp`** — partial leaf splicing's benchmark (ADR-210); ~220 ms vs ~520 ms with
  `BROOD_NO_PARTIAL_LEAF=1`. Its header carries the derivation-vs-lowering trap.
- **`spawn_live_ladder.blsp`** — decomposes the worst published row into rungs
  (`spawn`/`send`/`nopark`/`park`/`park-batched`/`payload`). **One rung per process** (a shared
  process leaks JIT tiering into later rungs and breaks monotonicity — which is the tell) and
  **CPU, not wall**. `park-batched` with `BATCH` separates suspending from coexisting, the
  confound that made item 2 look like a 5.5 µs lever. Verify any rung's parking claim with
  `BROOD_L1_STATS=1`: the fast path fires only on a parked receiver, so it counts parks
  directly.
- **`recv_matcher.blsp`** — what one `receive` costs, in a single long-lived process (so
  tiering is not a variable, unlike the ladder). Four modes, and the pairing is the point:
  `PAT=vec` (`[:go v]`) and `PAT=intlit` (`[1 v]`) compare a literal element and never reach
  the native fast frame; `PAT=bind` (`[a v]`) and `PAT=any` (`m`) do not compare one and stay
  native. **`bind` is the control to quote** — same vector work as `vec`, differing only in
  the compare, so the −28% between them is a real gap rather than work not done.
  `jit_link_done` — 0 vs ~N — is the counter that says which side of the fast frame you are
  on; no other counter shows it (`jit_native` stays high because the *caller* is native).
- **`hof_call.blsp`** — per-call cost of a HOF step function, global vs computed head. Its
  header carries the trap that makes this measurement easy to get 160× wrong: a callee of the
  form `(defn f (a b) (%prim a b))` is a passthrough that `resolve_prim` **inlines**, so such
  a row measures a deleted call, not a call. Prints total ms and the accumulator beside every
  figure so a vanished loop is visible.
- **`process_floor.blsp`** — the per-process idle floor; ~4.27 KB, flat across N. Read the slope,
  never `rss/n`; discard the first run after a fresh build.
- **`soak_selfcheck.blsp`** — sustained load with an invariant checked every iteration. **Always pair
  it with a control** reverting the mechanism under test.
- **`decay_isolate.blsp`** — throughput per fixed-size window plus RSS and `:runtime-closures`; run
  modes sequentially.
- **`receive_backlog.blsp`** · **`net_framed_scale.blsp`** — the receive-mark and framed-read
  benchmarks, each carrying its own controls.
- **`tests/registry_test.blsp`** / **`tests/shared_closure_msg_test.blsp`** — each carries a control
  that fails with its mechanism off, which is the only version worth having.
- **`tests/collection_identities_test.blsp`** — seeded-random laws over maps/vectors/strings,
  including the multi-byte char-index laws (each op against the code-point vector). The place to add
  a property that every engine would agree on, which the engine-differential is therefore blind to.
- **`BROOD_PERF_STATS=1` on a `--features perf-stats` build** — counts *and* (new) `ns_*`
  timing shares: spawn / deliver / message copy each way / receive / matcher resolve /
  teardown / one scheduler quantum (which nests the rest). This is what attributes a
  *process-shaped* cost; the counters alone could not. Pair with `BROOD_TRACE_COMPILE=1`.
- `scripts/fuzz/run.sh <generator>` — differential across 4 engine configs (tree-walker, VM-no-JIT,
  VM+JIT, GC-stress+verify). `make ab BASE=<ref>` for brood-vs-brood rows; `bench/harness.py` in
  brood-benchmarks for the published cross-language numbers.

## 9. Where we stand against the field

From the published run (`brood-benchmarks/results/`, **2026-08-05** — the first since the
ADR-213/214/215 work):

- **`latency`** (open-loop, ranked by p99) — Elixir 56 µs, **Brood 68 µs**, .NET 719, Python 469,
  Node 470. 2nd of five, **6.9× ahead of third**, p50 14 µs against Elixir's 8. The tail moved
  this run for a reason worth remembering: p99.9 658 → **461 µs** and max 6.0 → **1.6 ms** came
  from ADR-215, i.e. from removing compilation off the *arrival* path. **An open-loop tail is
  where one-off per-process setup shows up**; a throughput row amortises it away.
- **`spawn-live`** — still the worst row, but it moved for the first time provably: 2.56 → **2.13
  s**, 8.40 → **6.24 CPU·s**, 1.75 → **1.58 GB**. Now **2.9× slower and 1.75× heavier** than the
  BEAM (was 3.4× / 1.9×). §1 has what's next.
- **`supervisor`** — Brood 878 ms vs Elixir 449 ms, unchanged.
- **Compute aggregate** — 2.9× the fastest, 3rd of seven, ahead of Elixir.
- **Base RSS 22.4 MB** — 3rd-lightest of seven, and **up from 18.6 MB at the start of
  2026-08-04**. ~2 MB arrived with the batch that merged upstream's package-signing crates; ~1 MB
  is in the ADR-215 commit and is **not** the shared compiled-code cache (its off-switch accounts
  for ~40 kB) and is **not diagnosed**. Published with that caveat stated. If you pick this up:
  binary size is unchanged (37.479 → 37.481 MB), the boot cache is a 170 KB source file, and the
  `startup` row measures a warm best-of-9 — so the usual suspects are already ruled out.

**Publishing procedure** (from `brood-benchmarks/CLAUDE.md`, and it matters): install the **lean**
build first — `make install INSTALL_FEATURES='$(RUN_FEATURES)'` — run `python3 bench/harness.py`
at its defaults on a quiet machine with no concurrent builds, then update by hand in this order:
`bench/chart.py`, `BENCHMARKS.md`, `README.md`, `FRONTIER.md` (only if a gap materially moved).
The harness fails itself on a checksum mismatch or a compute-floor clamp, so a clean exit means
something. One trap of its own: `pgrep -f "bench/harness.py"` is useless as a wait condition —
stale waiter loops from earlier sessions match that pattern (and match themselves). Wait on the
PID.
