# Known issues

KI-9 is a one-off arity sighting judged a transient inconsistent-build artifact, not
present in committed code; KI-10 no longer reproduces, incidentally fixed — both kept as
records, not open bugs. **Open: KI-13 (type checker) and KI-16 (the LSP still matches the retired
`defprotocol`/`defimpl`).**
This file is the condensed record — what each was, how it was fixed, and the regression
test that guards it — so a recurrence is recognizable. For the narrative discovery
writeup of the scheduler race, see
[claude-demo-findings.md](claude-demo-findings.md); deeper rationale is in the cited
ADRs / topic docs.

---

## KI-15 — `impl` silently misregisters a **bare** record id · **FIXED 2026-07-27**

**Fixed:** `impl` and `:sealed` now share one helper, `ability--id-kw`, which qualifies a
bare symbol against `(current-ns)` (`circle` → `:<ns>/circle`), keeps a keyword id
untouched (`:int`, `:default`), and never double-qualifies an already-`/`-qualified
symbol. So `(impl S circle …)` registers under the same `:<ns>/circle` a value presents.
Regression tests: `tests/ability_test.blsp` "impl id qualification (KI-15)" (a bare id
dispatches) and `sealed_ability_bare_impl_id_qualifies_ki15` (a bare impl satisfies a
sealed member). Original report:


`ability`'s two macros disagree about whether a bare record symbol gets namespace-
qualified, and the mismatch fails *silently* at registration:

```lisp
(defmodule geometry (:use ability))
(defrecord* circle (r))
(defability Shape :sealed [circle rect] (area [self] :-> float))  ; :sealed QUALIFIES → :geometry/circle
(impl Shape circle (area [c] …))                                  ; impl does NOT     → :circle
(area (circle 2))
;; error: ability Shape/area: no impl for :geometry/circle — have (:circle)
```

`defability--sealed-vec`'s member mapping qualifies a bare symbol against
`(current-ns)`; `impl`'s `id-kw` is a plain `(keyword (name key))`. Since
`identity-of` produces the *qualified* `:module/name` keyword, the bare form
registers under a key no value ever presents — so the impl never matches, the
sealed-exhaustiveness check reports the member as unimplemented, and the failure
only surfaces at the first call.

**Workaround:** always write the id qualified — `(impl Shape geometry/circle …)`,
which is what `tests/ability_test.blsp` and the documentation do.

**Fix direction:** qualify a bare symbol in `impl` the same way `:sealed` does (a
symbol with no `/` gets `(current-ns)` prepended), so both macros agree. A keyword
id (`:int`, `:default`) must keep passing through untouched, and an already-
qualified symbol must not be double-qualified. Alternatively make a bare,
unqualifiable symbol an expansion-time *error* rather than a silent
misregistration — but qualifying is the ergonomic answer and matches `:sealed`.
Either way, add a test that a bare id and a qualified id reach the same impl.

**Found by:** writing the ADR-168 documentation and following the `impl` docstring
verbatim on a first attempt (docs run, 2026-07-27).

---

## KI-16 — the LSP still matches the retired `defprotocol`/`defimpl` · **OPEN, found 2026-07-27**

ADR-168 removed `defprotocol`/`defimpl`, but the language server was not migrated
with them:

- `definition.rs` (`enclosing`/interface goto) and `module_ref.rs` match
  `"defbehaviour" | "defprotocol"`. The `defprotocol` arm is dead code — harmless,
  but it will never fire again.
- `completion.rs` offers an interface's ops inside a `(defimpl …)` form
  (`enclosing_defimpl`). Since `defimpl` no longer exists, **op completion inside
  an `(impl …)` form is simply missing** — the one user-visible loss.
- `completion.rs`'s test seeds `*protocols*` directly and is named
  `offers_protocol_ops_inside_defimpl`.

Nothing is broken for `defbehaviour` (goto + hover over `(:implements …)` still
work); abilities are never claimed in a module header, so they were never part of
that path. The gap is purely the `impl` completion affordance.

**Fix direction:** rename `enclosing_defimpl` → `enclosing_impl` and match `impl`,
reading ops from `ability/*abilities*` (via `introspect::protocol_ops`' ability
sibling) as well as `*protocols*`; drop the `defprotocol` arms from
`definition.rs`/`module_ref.rs`. Update the test name and seed.

**Found by:** the documentation validation pass for ADR-168 (docs run, 2026-07-27).

---

## KI-14 — the RUNTIME collector re-walked a deep process's whole root stack at every safepoint · **found 2026-07-27, fixed 2026-07-27**

**Symptom.** `make test` could not go green: `brood::suite brood_suite_passes` was
SIGKILLed at its 600 s nextest cap, deterministically. One-line repro, which hung
indefinitely (observed >10 min):

```sh
nest test --only 'test:every n_ document'
```

**What made it confusing.** The same work outside the test framework was fast and correct:
the identical scan over the identical 318 corpus files returned `[0 188 nil]` in seconds,
each of the 188 `n_` documents parsed fine one-per-process, and the two deep-nesting
documents parsed cleanly inside a spawned green process. It needed the framework context —
and, it turned out, nothing about the framework except **how much code it had loaded**.

**Root cause.** The ADR-091 RUNTIME two-generation collector's cooperative drain report
(`Heap::report_gen_liveness` → `runtime_gen_referenced_private`) probes in two phases:
Phase 1 over the process's private roots and live arms, Phase 2 over its whole LOCAL heap.
Phase 2 already had a stale-dirty throttle (`P2_REVALIDATE_STRIDE`) added for exactly this
class of problem. Phase 1 was deliberately unthrottled, on the premise that it is the
*cheap* probe.

That premise has an unbounded term: `roots` is the VM operand/env stack, so **Phase 1's
cost is O(recursion depth)**. Parsing a 100 000-level JSON document makes it enormous, and
instrumenting the hung run showed the scale — inside a **single** drain epoch:

```
drains: 1   phase1 walks: 78409
[rt-dbg] phase1 roots=1727686 env=2 dyn=0 arms=66448
```

78 409 walks over a 1.7-million-entry root stack. Three compounding faults:

1. **The Phase-2 stale-dirty short-circuit sat *between* the phases.** A process dirty via
   Phase 2 therefore paid a full Phase-1 walk on every safepoint and then discarded the
   result — the verdict is `true` regardless of what Phase 1 found. Pure waste, invisible
   while Phase 1 really was cheap. This was the dominant cost.
2. **Phase 1 had no throttle of its own,** so a process dirty *via Phase 1* (deep inside
   draining-generation code) re-walked O(depth) every reporting safepoint.
3. **`live_vm_arms` was walked per active frame, not per distinct arm.** It is a per-frame
   stack, so a recursive function appears once per frame — 66 448 entries that are a
   handful of distinct `Arc`s, each arm's whole IR tree re-walked every time.

Why loaded-code volume was the trigger: the drain only arms once the shared RUNTIME region
crosses `BROOD_RT_GC_FLOOR`. Few files loaded → no drain → no probe → 520 ms. Whole suite
loaded → drain armed → the above. `BROOD_RT_GC_FLOOR=100000000` (collector off) took the
hung repro to 2.2 s, which is what confirmed the subsystem.

**Fix.** Hoist the stale-dirty short-circuit above Phase 1 (changes no verdict — it only
skips work whose result was already discarded); give Phase 1 its own `P1_REVALIDATE_STRIDE`
throttle, gated on `P1_LARGE_SEED` so a shallow process keeps reporting on its very next
safepoint (the promptness the drain-completion tests rely on); and dedup `live_vm_arms` by
`Arc` identity. Soundness is the same argument Phase 2 already made: a stale-dirty verdict
only *delays* drain completion, it can never fabricate a clean ack.

**Result.** The filed repro: hang → **9.8 s**. Whole in-language suite including
conformance: **3591 tests, all passing, 88 s**.

**Guarded by** `tests/jit_deep_recursion_test.blsp` (deep recursion under the collector)
and the existing `crates/lisp/tests/runtime_collector.rs` drain-completion tests, which pin
the promptness the seed-size gate preserves.

**Found alongside** two genuine stack-overflow aborts on the same corpus, both fixed and
both distinct from the hang (each aborts rather than hangs, and neither is the reason
`make test` was red):

- **A JIT'd arm could run the native stack into its guard page.** The pre-existing guards
  (`jit_native_depth` + the `stacker` headroom probe) sit on the *dispatch* paths, so they
  only bound recursion that goes through a fast link; recursion via `brood_rt_call_slow`
  re-enters Rust every level while the depth counter stays near zero. Fixed with a
  three-instruction prologue guard in every lowered arm, checking the frame's address
  against a `Heap::jit_stack_limit` stamped from the live remaining stack at each native
  entry. On a trip it sets `jit_force_vm` and deopts, so the subtree drains through the
  VM's bounded heap frames and raises the clean, catchable `MAX_BC_FRAMES` error. (Deopt
  alone livelocks: the VM re-runs the arm, the callee re-tiers, the prologue trips again.)
- **`gc_runtime::flush_rt_value` ⇄ `flush_rt_pair` recursed unguarded.** The cdr spine was
  already iterative, but *car* nesting recursed one native frame per level, so promoting a
  deeply nested value into the RUNTIME region aborted the thread. The LOCAL twin
  (`gc::flush_value`) already had the `stacker::maybe_grow` guard; the RUNTIME one was
  simply missed.

**Adjacent finding, still open:** `nest observe`/attach tests leak a `brood` child — one was
found alive 2h22m after its run (`/tmp/brood-observe-<pid>/target.blsp`, ~2.7% CPU).
Independent of this bug, but a long session accumulates stray processes.

## KI-13 — cross-module return-type inference blows up exponentially in branch count · **OPEN, found 2026-07-26**

**Symptom.** `nest check` never finishes. No diagnostic, no progress output, one core
pegged. The LSP is the same code path, so an editor hovering the call site hangs with it.

**Trigger.** A recursive function with several `cond`/`if` branches that build *nested
list structure*, **called from another module**. The same call inside the defining file
checks instantly — this is specifically the cross-module path, where the checker infers an
undeclared callee's signature (`sig_of` → `infer_sig` → `expr_ty` over the whole body).

Minimal reproduction — this is the `deriv` benchmark from the Gabriel corpus
(`tests/support/gabriel/deriv.blsp`), which is how it was found:

```lisp
;; a.blsp
(defmodule zzm)
(defn d (a)
  (cond
    (not (pair? a)) (if (= a 'x) 1 0)
    (= (first a) '+) (cons '+ (map d (rest a)))
    (= (first a) '-) (cons '- (map d (rest a)))
    (= (first a) '*) (list '* a (cons '+ (map (fn (b) (list '/ (d b) b)) (rest a))))
    (= (first a) '/) (list '- (list '/ (d (second a)) (third a))
                             (list '/ (second a) (list '* (third a) (third a) (d (third a)))))
    else (error "no")))

;; b.blsp — checking THIS file is what hangs
(defmodule zz-probe (:use zzm))
(def r (d '(+ x 1)))
```

**Measured scaling** (release `nest check`, one recursive branch added at a time):

| recursive branches | `nest check` |
|---|---|
| 2 | instant (105 ms, the floor) |
| 3 | 105 ms |
| 4 | **8.7 s** |
| 5 | did not finish (killed at 900 s) |

So each additional branch multiplies the work by roughly an order of magnitude —
consistent with the inferred `Ty` growing multiplicatively as `expr_ty` unions branch
results whose element types are themselves nested unions, with the lattice operations
(`union`/`is_subtype`) then superlinear in that size. Not diagnosed further than the
scaling; nobody has profiled it yet.

**Not a hang in the sense of a loop.** `InferGuard` (`types/check/sigs.rs`) already breaks
recursion *cycles* correctly, and `expr_ty` has a depth cap. What is missing is a bound on
the *size* of an inferred type.

**Workaround, and why the corpus port uses it.** A **declared** signature is consulted
before body inference (`declared_heap_sig`), so it short-circuits the blowup entirely:
adding `(sig deriv (any -> any))` takes the case above from >900 s to 105 ms. That is what
`tests/support/gabriel/deriv.blsp` does, with a comment saying so — the sig is honest and
declaring a public API's type is right anyway (ADR-153), but it is load-bearing there and
must not be removed as decoration.

**Likely fix.** Cap the size of an inferred type and widen past the cap. Widening an
over-approximation is always sound, so this cannot introduce a false positive; it can only
lose precision on exactly the pathological shapes. The existing `MAX_INFER_DEPTH` is the
precedent for the shape of the fix.

**Severity.** `nest check` is a CI gate and the checker also backs the LSP, so a hang is
worse than a wrong warning — and the trigger is ordinary code, not something exotic. No
regression test yet; the reproduction above should become one with the fix.

---

## KI-12 — a frozen prelude global's inner handle resolved to the wrong object · **found + fixed 2026-07-26**

**Symptom.** The prelude's only non-trivial global value is corrupt in every build:

```lisp
(println (pr-str *load-path*))
;; expected: (".")
;; actual:   ("A list of the given arguments.")   ← `list`'s DOCSTRING
```

The *list* is right (one element, `rest` is nil) — its **car** is a different object
entirely. Which object depends on heap layout, which is what makes it a handle bug
rather than a logic bug:

| build / flag | `*load-path*` |
|---|---|
| debug, default | `("A list of the given arguments.")` (a docstring) |
| debug, `BROOD_GC_VERIFY=1` | `("ret")` |
| prelude written `'(".")` instead of `(list ".")` | `(xs)` — a **symbol** |
| **`BROOD_VM=0`** (tree-walker) | `(".")` — **correct** |

So the car slot holds whatever value happens to live where the handle points, and
the kind bits change with it — i.e. the stored `Value` is wrong, not merely
mis-read. Correct under the tree-walker, wrong under the VM.

**Not a regression.** A `brood` binary from 2026-07-25 18:49 (predating that day's
syntax work — it still rejects `^x` pins) shows the same class of failure, with
`cond`'s docstring in the slot instead. The 2026-07-26 prelude edits (alias trims,
`sig` adoption) only changed the *layout*, hence which wrong object appears.

**Why it hid for so long.** The prelude has essentially one global holding a heap
value, and nothing reads it on the happy path: `require` finds every std module via
`%builtin-module`, and a project run replaces the path wholesale
(`project-setup` → `set-load-path!`). So filesystem module lookup from the
*default* path has been broken without a single test noticing — except
`brood-lsp`'s `completion::tests::completes_module_names_in_require_and_use`,
which puts a temp dir on the default path and asks for completions. That test is
the canary and is **currently failing**.

**Root cause.** `to_prelude` (`crates/lisp/src/core/heap.rs`) re-tagged a handle by
keeping its slab **index** and changing its **region** bits. That is valid only for a
LOCAL handle, because the builder's slabs *become* the prelude region — and it was
applied unconditionally. Instrumenting the freeze showed the offending global was:

```
[freeze] *load-path*: Pair region=0 idx=55990 car=Str region=2 idx=60
```

a LOCAL pair (region 0) whose car was a **RUNTIME** string (region 2). The VM interns
its constant-pool literals into the shared RUNTIME region so compiled code can be
shared between processes, so `(def *load-path* (list "."))` — evaluated by the VM
during the prelude build — bound a pair holding a RUNTIME `"."`. Re-tagging turned
that into PRELUDE `Str@60`: a different slab, an unrelated string. The tree-walker
was correct because it passes the LOCAL string straight from the read form.

**Fix.** Two halves, both in `heap.rs`:

1. `Heap::localize_for_freeze` — before anything is re-tagged, deep-copy any part of
   a global's reachable graph that lives outside LOCAL into the builder's own slabs
   (a forwarding table collapses shared structure and terminates cycles). Reachable
   prelude state is then all-LOCAL by construction, which is what the re-tag assumes.
2. `to_prelude` re-tags **LOCAL only** and returns anything else untouched. The slab
   sweep visits every cell including unreachable boot garbage, which may legitimately
   hold RUNTIME handles; flipping those is what did the damage. A `debug_assert` on
   the root bindings catches a future `localize` gap at freeze time instead of
   silently corrupting a global.

Cost: one walk of the reachable global graph per *source* boot (freeze 5.0 → 8.7 ms,
source-boot peak 3.7 → 10.6 MB, both once per build — the cached boot is unchanged at
~49 ms). All three boot paths now agree: `*load-path*` is `(".")` under the source
boot, the cached boot, and `BROOD_VM=0`.

**What it had been costing.** Filesystem module lookup from the *default* load path
never worked — `require` found std modules via `%builtin-module` and a project run
replaced the path wholesale, so nothing noticed. `brood-lsp`'s
`completion::tests::completes_module_names_in_require_and_use` was the only test that
read the default path; it passes again.

## KI-11 — JIT tail-chain recursion escaped the native-depth cap · **found 2026-07-25, fixed 2026-07-26**

**Symptom (before the fix).** Deep non-tail recursion on the **JIT** path overflowed the
*native* stack and killed the process — `thread '<unknown>' has overflowed its stack /
fatal runtime error: stack overflow, aborting`. An abort, not a panic, so the crash-dump
hook (`install_crash_dump`) never fired and there was no `.brood_crash_dump` to read.

The other two engines were correct on the identical input, which is what identified it as
a JIT bug rather than a missing guard in general:

| engine | 20,000-deep nested JSON |
|---|---|
| default (JIT) | **process aborted** — native stack overflow |
| `BROOD_NO_JIT=1` (bytecode VM) | parsed fine — frames are heap-backed |
| `BROOD_VM=0` (tree-walker) | clean, catchable `recursion too deep: used 12585200 bytes of stack, over the 12582912-byte budget` |

Threshold was between 10,000 and 20,000 nesting levels. `gdb --batch -ex run -ex bt` put
the recursion in `jit_runtime::jit_run_fast_link` ← `brood_rt_fast_frame`.

**Impact (before the fix).** Any Brood service parsing untrusted nested input was
killable with a few kilobytes — `std/json` the obvious one, but this was a property of
the JIT call path, not of the parser. Unrecoverable: `try`/`catch` could not see it, and
a supervisor could not restart from it because the whole OS process died, not the green
process.

**Repro** (the minimal shape — a three-function cycle with a tail-call delegator,
entered from a non-tail position; see the root cause for why a plain `(defn deep (n) (+ 1
(deep (- n 1))))` runs to 200,000 without tripping it):

```lisp
(defn tv (n) (if (= n 0) [0 n] (ta (- n 1))))   ; enters the cycle
(defn ta (n) (tacc n))                           ; the tail-call delegator
(defn tacc (n) (let ([v j] (tv n)) [(+ v 1) j])) ; non-tail: result destructured
(tv 20000)                                       ; aborted pre-fix; fine now
```

**Found by** the JSONTestSuite corpus (`n_structure_100000_opening_arrays.json`,
`n_structure_open_array_object.json`) — see ROADMAP "External conformance corpora".

**Root cause.** Not a missing guard — a guard that stopped applying. The cap
(`JIT_NATIVE_DEPTH_LIMIT`, 1500) and its counter (`Heap::jit_native_depth`) already
existed and *were* checked by both dispatch entry points. But `jit_run_fast_link`
restored the counter to the caller's level as soon as the native callee returned, and
*then* handled the outcome — and three outcome arms re-enter the evaluator while that
frame is still on the native stack: the outcome-4 tail-chain follow-through
(`apply_value`) and the two deopt/preempt re-runs (`vm_resume_deopt` / `vm_apply`). So a
chain of tail-calling delegators oscillated between `depth` and `depth+1` forever while
the native stack grew without bound. The cap never tripped because the depth never
climbed.

That is why no existing depth test caught it: the trigger needs a **cycle** of functions
in which at least one link is a plain tail-call delegator (so its native returns outcome
4) and the cycle is entered from a non-tail position. Plain deep self-recursion — even
with a 20-local frame, even mutual, even building a 20,000-deep nested value — all stay
under the cap and run to 200,000 fine. `std/json`'s
`json--value` → `json--array` → `json--array--acc` → `json--value` is exactly the shape,
with `json--array` as the delegator.

**Fix, part 1 — make the counter count.** `jit_native_reenter` re-raises the depth for the
duration of each re-entrant call, so the cap sees the true native nesting and chains past
1500 levels drain on the VM's heap-backed frames as intended. The hot outcome-0 return path
is untouched, and the cap is only reached by depths that previously crashed.

**Fix, part 2 — make the cap a measurement.** With part 1 in place the release build was
fine but the *debug* build still aborted, because `JIT_NATIVE_DEPTH_LIMIT` is a frame
**count**, and a count is only ever right for one frame size: 1500 levels is a few MB of
the 16 MB worker stack in release and several times that in debug. Same root flaw as the
bug itself, one level up. `jit_native_headroom_ok` now probes
`stacker::remaining_stack()` and refuses a new native link below a 512 KB margin; the
count cap stays as the cheap first test, and the probe is skipped below 64 levels so the
hot shallow path (`fib`, `primes`) pays nothing beyond the existing integer compare.
Measured flat: primes-to-200k 57→58 ms, fib-30 5→6 ms (3-sample, within noise).

This also covers a case neither the count nor this bug report anticipated: a host embedding
Brood on a smaller thread stack. Returning `false` is never a correctness problem — the
caller falls through to the VM, which is where deep recursion belongs.

**Regression test.** `tests/jit_tail_chain_depth_test.blsp` — the minimal three-function
cycle at 30,000 levels, the deeply-nested-JSON rejection, and the plain-self-recursion
shapes that always worked (so a future depth change has to keep them working). Verified by
A/B against a pre-fix binary: pre-fix aborts at 20,000, post-fix runs to 50,000 — in
**both** profiles, debug included, which is what part 2 bought.

The two JSONTestSuite documents that found this are still skipped in the sweep, but on
cost grounds rather than correctness: rejecting a 100,000-level document now means actually
draining it on the VM, which is ~400 ms standalone and >120 s inside the full parallel
suite. That >250× gap is steeper than contention explains and is logged in ROADMAP as a
possible GC-depth pathology worth its own investigation; the property is covered by a
synthetic 5,000-level case.

---

## KI-10 — `receive` compile cliff at the 13th arm · **no longer reproduces 2026-07-25**

Adding a 13th arm to a hot `receive` loop degrades it badly: on `buffer--serve`
(std/editor/buffer.blsp), a single TRIVIAL extra arm (`([:sync-probe] (recur …))`)
took the buffer suite from 4.9 s / 139 MB peak to 8.0 s / 248 MB — +65% wall,
+80% peak — and pushed the full parallel suite over the 1 GB test soft limit.
Bisected 2026-07-11 while adding a `[:sync …]` arm for the resync primitive;
12 arms are fine, 13 fall off the cliff, so the dispatch likely drops from an
indexed strategy to an allocating linear one at that width. Worked around at the time by
merging `buffer--serve`'s two `[:edit …]` arms (buffer-edit always sends the
3-element form) to stay at 12.

**Re-measured 2026-07-25 on `b0b4fd1`: gone.** Adding the same trivial
`([:sync-probe] …)` arm to `buffer--serve` and rebuilding costs nothing, and neither
does adding eight:

| serve arms | buffer suite (`brood --test tests/buffer_test.blsp`) | full suite (`nest test`) |
| --- | --- | --- |
| 12 (committed) | 3.33 s / 55 MB | 22.49 s / 610 MB |
| 13 | 3.35 s / 54 MB | — |
| 20 | 3.36 s / 55 MB | 22.79 s / 636 MB |

Measured on a `cargo build --release` binary with the module **baked in** (the
configuration the cliff was originally seen in — a hot-loaded copy was checked too and
is equally flat). Against the original +65% wall / +80% peak for a single arm, +1.3%
wall / +4% peak for *eight* is flat.

**The mechanism was never identified, so this is an incidental fix**, from some part of
the VM/JIT/pattern work between 2026-07-11 and now — not a change aimed at it. The
arm-count budget is therefore lifted (the note in `buffer--serve` is updated), but if a
width-dependent cliff ever reappears it needs bisecting from scratch: the width alone
was never the trigger. A 13-arm receive of *uniform* trivial arms never showed it
either, which is why the original report is specifically about `buffer--serve`'s arm
shapes.

## KI-9 — arity error from a closure shipped in a `spawn` body · **likely transient; not present in committed code**

Surfaced once while parallelising `nest check`: a driver spawned
`(spawn (send me (list :chunk (self) (chunk-fn chunk))))` where `chunk-fn` was a
*closure passed in as an argument* (captured in the spawned body's env); ~1 worker in
3 died with a bogus `arity error: fn: expected 0 arguments, got 1` at the
`(chunk-fn chunk)` call, silently skipping its files.

**Most likely a transient inconsistent-build artifact, NOT a standing kernel bug.**
The sighting happened while a *concurrent session* was mid-edit on the Rust tree
(uncommitted type-checker changes to `check.rs` et al., committed later as
`be0f8cc`); building from a half-applied edit can yield a binary with a real-but-
transient fault. The decisive evidence is the **frequency collapse**: it died ~66 %
of runs then, and **0 of 50+** now under identical repro conditions (a minimal
captured-closure spawn; the *exact* old shipped-closure `pfold` + `check-file`
chunk-fn at 24 and ~470 workers; the full multi-level `pfold-files→groups→spawn`
capture; all under `BROOD_GC_STRESS=1` + `BROOD_GC_VERIFY=1` on a debug-assertions
build — all clean, no tripwire/verifier hit; deterministic arity-through-`spawn`+
`promote` passes). A stable race doesn't go 66 %→0 %; a changed binary does. A code
audit of the promote path (`heap.rs::promote_closure`/`promote_env`) found no defect
— arms/arity copied faithfully, reserve-then-fill with cycle-breaking, lock-free
concurrent append, and RUNTIME compaction can't fire mid-fan-out (uniquely-owned-Arc
gate). **No fix applied** — deliberately not blind-patching the moving-GC/shared-
RUNTIME path to chase a fault not present in the committed code. If it ever recurs on
a *clean committed build*, reopen with that repro.

**Best-practice (independent of this):** prefer shipping **data** to a spawned worker
and resolving the operation through a **global** (as the test runner and
`std/tool/project.blsp`'s `project--pfold-run` do) over capturing a closure value in a
`spawn` body — it avoids the heavier per-spawn `promote` deep-copy of a captured-env
closure regardless.

## KI-8 — RUNTIME form-position table (`positions`) stranded by compaction · **fixed 2026-07-03**

The bug-KI-3 class again, in a side table the KI-3 fix didn't cover. `RuntimeCode::positions`
(source positions of RUNTIME list forms) is keyed by the pair's RUNTIME **slab index**, with a doc
comment asserting "a RUNTIME pair never moves" — a pre-ADR-091 premise the compactor invalidated. A
compaction relocated the pairs but not the keys, so `(form-pos …)` / source-location returned a
stranger's position (or none) afterward. Diagnostics-only. **Fix:** `runtime_collect_with` remaps
the keys through the same `fwd.pairs` forwarding after the evacuation walks, dropping entries whose
pair didn't survive (mirroring the LOCAL `form_pos` remap). **Guarded by:** the `declared_sigs`
regression test exercises the same rewrite pass; the LOCAL analog `form_pos` is remapped in `collect`.

## KI-7 — declared `(sig …)` type-expressions corrupted by RUNTIME compaction · **fixed 2026-07-03**

The bug-KI-3 class in another off-graph holder. `RuntimeCode::declared_sigs` stores each `(sig name
type)` form's type-expression as a **promoted RUNTIME `Value`**, held in a `SymbolMap<Value>` beside
`globals` but NOT walked by `runtime_collect_with`. A compaction relocated the type-expr out from
under the stored handle, so the checker's `sig_of` later read a garbage form (confirmed: `(int ->
int)` read back as `(i 1)` after churn + compact). Silent wrong-data (no tripwire — the handle is a
valid RUNTIME index, just the wrong cell). **Fix:** `runtime_collect_with` now `flush_rt_value`s
`declared_sigs` alongside `globals`. **Guarded by:** `tests/runtime_collector.rs::declared_sigs_survive_a_runtime_compaction`.

## KI-6 — `%isolate` snapshot/restore not RUNTIME-compaction-safe · **fixed 2026-07-03**

A sibling of KI-2 (that fix handled orphan-process reaping; this one the compaction-relocation race).
`%isolate` snapshots the global table — an off-graph `SymbolMap<Value>` of raw RUNTIME handles — runs
a thunk, then restores it. A RUNTIME compaction *during* the thunk (its `def`s crossing
`BROOD_RT_GC_FLOOR`, trivially met in a large image) relocated those handles; the stale snapshot then
reinstalled handles aliasing *other* closures → an unrelated pre-isolate global silently misdispatched
(`foo` → a 1-arg `z-*` defined inside the rolled-back isolate). Latent for every `:isolated` test.
**Fix:** a re-entrant `Heap::rt_collect_block` counter (a `Cell<u32>`) suppresses RUNTIME compaction
while a globals snapshot is outstanding — `snapshot_globals` increments it and `restore_globals`
decrements it, so the invariant holds *structurally* (every caller of the protocol is covered, not
just `%isolate`). Checked at the `runtime_collect_with` choke point, so BOTH the auto safepoint path
(via `rt_gc_due`) and a manual `(runtime-collect)` are covered — an explicit collect inside an
`%isolate` is a no-op, not a corruption. The isolate's `def`s become garbage at restore and are
reclaimed by the next safepoint. **Guarded by:**
`tests/runtime_collector.rs::{isolate_is_safe_against_a_runtime_compaction_inside_the_thunk,
manual_runtime_collect_inside_isolate_is_a_noop}`.

## KI-5 — `nest test` OOMs: shared RUNTIME region accumulates every test file's code · **fixed 2026-07-03**

`run-project-tests` loaded every test file into one long-lived driver image before running any, so
each file's top-level `def`s promoted their compiled closures/chunks into the shared `RuntimeCode`
region + global table — globally rooted, live, unbounded, unreclaimable (only same-name redefinition
frees the old version). A 725-test suite crossed the 1 GB soft cap → `memory limit exceeded` on
whichever workers were allocating (brood-edit: 9 spurious failures, all passing in isolation).
**Fix:** `test/run-tests-scoped` (+ `-structured`) runs the suite file-by-file, each file inside its
own `%isolate` (reset → load one file → drain → rollback), so the file's `def`s roll back and the next
safepoint reclaims the promoted code — bounding memory to ~one file (relies on the KI-6 fix so the
mid-run rollbacks are compaction-safe). `BROOD_TEST_NO_SCOPE` reverts to the legacy load-all path.
brood-edit: OOM → 725/725 at 199 MB. **Guarded by:**
`tests/runtime_collector.rs::per_isolate_scoping_bounds_runtime_region_growth`.

## KI-4 — bitset stored as a non-UTF-8 `Value::Str` corrupts the GC on promote · **fixed 2026-06-15**

A bitset was a blob-backed `Value::Str` holding raw, non-UTF-8 bytes, but
`Value::Str`/`SharedBlob` carry a valid-UTF-8 invariant; promoting a closure that
captured one (`spawn`/`def`) read the bytes through the UTF-8 string accessor →
panic (armed) or UB/`flush_oob`/SIGSEGV (release). Surfaced ~1-in-3 in the
brood-life `--fair` demo. **Fix:** bitsets are a distinct `Value::Bitset` kind with
their own raw-byte slab (LOCAL `Vec` + RUNTIME `boxcar`), byte-clean accessor /
`promote_in` / equality / `Message::Bitset`, mirroring the `bigint` leaf slab — a
bitset can no longer reach a string accessor. **Guarded by:** the spawn-promote-a-
bitset path under `BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1`.

> **Superseded 2026-06-28:** the whole `bitset` feature (the `Value::Bitset` kind +
> 13 `bitset-*` prims) was removed — it had no in-repo or external consumer left.
> The KI-4 fix is moot now that the type is gone; kept here as the historical record.

## KI-3 — RUNTIME compaction strands live VM / tree-walker constants · **fixed 2026-06-01**

Once the ADR-076 RUNTIME compactor made promoted code-region handles movable, two
sites held them as immovable: the tree-walker elided the operand-stack slot for a
RUNTIME root (so `runtime_collect` never rewrote it), and the VM held promoted
handles inline in `Node::Const`/`MakeClosure.fn_rest` (off the GC root graph). A
compaction at a nested safepoint left them dangling → `flush_oob` or a constant
read back as a different value. **Fix:** `needs_root_slot` (LOCAL **or** RUNTIME)
gives a RUNTIME handle an operand slot; the VM carries movable consts as
`ConstVal::Handle` and registers its live arm in `Heap::live_vm_arms`, which
`runtime_collect` rewrites in place. **Guarded by:** `compile::tests::{const_handle_round_trips,
rewrite_arm_handles_rewrites_every_embedded_handle}` and
`tests/runtime_collector.rs::auto_safepoint_collect_bounds_runtime_region`.

## KI-1 — multi-thread scheduler race: green processes can't resolve globals · **fixed 2026-05-29**

Spawning green processes that touched globals crashed workers with bogus `unbound
symbol` errors (a data race on shared global/scope state via the kernel
supervisor's RESUME_SLOT machinery, worsened by free-list slot reuse). **Fix
(in series):** strip the kernel supervisor (ADR-039, reverted → ADR-044); switch
to a bump-only allocator (slots never recycle, so a stale handle can't observe a
wrong-type value); per-worker pinned queues. **Durable invariant:** no recycled
slots / no stale handles across a safepoint. (The per-worker *pinning* stopgap was
later superseded by ADR-100's heap-captured continuations, which make cross-thread
migration safe and routine.) **Guarded by:**
`tests/concurrency_race.rs::fanout_with_concurrent_global_rebind_matches_serial`
(the fan-out-matches-serial bar) and the self-diagnosing `flush_oob`/`flush_bound!`
OOB check.

## KI-2 — `nest test` flaky / hangs when parallel tests share heavy global lookups · **fixed 2026-05-29**

Two bugs: (1) the KI-1 lookup race could kill a worker; (2) the runner didn't reap
a dead worker, so the run hung in `receive` forever. A 2026-06-07 recurrence under
maximal load was root-caused **not** to a core race but to test isolation:
`%isolate` (test-only) wholesale-restored the globals table, so a test that left an
orphan process running saw the orphan's next lookup die `unbound`. **Fix:** the
runner `monitor`s every worker and accounts for each exactly once (death → a failing
result, not a hang); `%isolate` reaps the processes its thunk spawned (via the
green-friendly `scheduler::yield_now`, never a thread sleep) **before** restoring
globals. Production never wholesale-restores globals, so the language itself was
never implicated. **Guarded by:** `tests/runner_failfast_test.blsp`.

## Platform gaps — GUI display seam · **all resolved 2026-05-31 (ADR-079)**

The GUI frontend had one font size for everything. Resolved: a `Face` carries an
integer `:scale` (per-op/region larger text in a scale×scale cell block — also
covers per-pane font); `gui-font!` takes an optional window id for per-window fonts;
`std/editor/pane.blsp` (ADR-077/078) provides pane layout + clip-rects. Per-pixel
`:height` sizing stays deferred (would break the uniform grid).

## Minor (all fixed)

- **Type-checker noise around `(require 'proc/hatch)`** — `check_file` pre-evaluates
  top-level `(require …)` so the required module's macros resolve.
- **`nest format` collapsed multi-line forms** — fixed (`5b19787`); respects author
  newlines. Still normalizes intra-line multi-space alignment (a standard trade-off).
- **Plain-release segfault on tail-recursive workers** — fixed by per-worker pinned
  queues, then made moot by ADR-100 (heap-captured continuations).
- **`cargo test --test suite` debug segfault** — coroutine stack overflow, not a
  memory bug; `WORKER_STACK_BYTES` raised (pages mmap'd lazily, ~0 cost until needed).
