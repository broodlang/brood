# Known issues

KI-9 is a one-off arity sighting judged a transient inconsistent-build artifact, not
present in committed code; KI-10 no longer reproduces, incidentally fixed — both kept as
records, not open bugs. **KI-17** (the checker reachability gap) is now **FIXED** (ADR-189).
**KI-18** (effect duplication on a deopt) and **KI-19** (call-head evaluation order) are
both now **FIXED**, as is **KI-20** (a fast link ran the callee against the caller's IC
block — a cold cache, never a wrong answer), as is **KI-21** (`nest run --for` /
`--watch` emitted a pre-ADR-150 `~p` pin and failed on every file). **No open issues.**
This file is the condensed record — what each was, how it was fixed, and the regression
test that guards it — so a recurrence is recognizable. For the narrative discovery
writeup of the scheduler race, see
[claude-demo-findings.md](claude-demo-findings.md); deeper rationale is in the cited
ADRs / topic docs.

## Index — all resolved (⌘F the `KI-N` to jump)

| # | What | Status |
|---|---|---|
| KI-22 | concurrent registration loses ~40% of registrations (15 registries) | ⬜ **open**, root cause found 2026-08-01 |
| KI-21 | `nest run --for` / `--watch` generated a legacy `~p` pin — failed on any file | ✅ fixed 2026-07-30 |
| KI-20 | a JIT fast link ran the callee against the *caller's* IC block (cold cache) | ✅ fixed 2026-07-30 |
| KI-19 | VM resolved a call's free-global head *after* its arguments | ✅ fixed 2026-07-30 |
| KI-18 | a JIT deopt could re-run a `table-put` (effect duplication) | ✅ fixed 2026-07-30 |
| KI-17 | `nest check` validated qualified names against its load set, not per-file reachability | ✅ fixed 2026-07-30 |
| KI-16 | the LSP still matched the retired `defprotocol`/`defimpl` | ✅ fixed 2026-07-27 |
| KI-15 | `impl` silently misregistered a **bare** record id | ✅ fixed 2026-07-27 |
| KI-14 | RUNTIME collector re-walked a deep process's whole root stack every safepoint | ✅ fixed 2026-07-27 |
| KI-13 | cross-module return-type inference blew up exponentially in branch count (checker hang) | ✅ fixed 2026-07-27 |
| KI-12 | a frozen prelude global's inner handle resolved to the wrong object | ✅ fixed 2026-07-26 |
| KI-11 | JIT tail-chain recursion escaped the native-depth cap | ✅ fixed 2026-07-26 |
| KI-10 | `receive` compile cliff at the 13th arm | ☑️ no longer reproduces (2026-07-25) |
| KI-9 | arity error from a closure shipped in a `spawn` body | ☑️ transient build artifact; not in committed code |
| KI-8 | RUNTIME form-position table stranded by compaction | ✅ fixed 2026-07-03 |
| KI-7 | declared `(sig …)` type-expressions corrupted by RUNTIME compaction | ✅ fixed 2026-07-03 |
| KI-6 | `%isolate` snapshot/restore not RUNTIME-compaction-safe | ✅ fixed 2026-07-03 |
| KI-5 | `nest test` OOMs — shared RUNTIME region accumulates every test file's code | ✅ fixed 2026-07-03 |
| KI-4 | bitset stored as a non-UTF-8 `Value::Str` corrupts the GC on promote | ✅ fixed 2026-06-15 |
| KI-3 | RUNTIME compaction strands live VM / tree-walker constants | ✅ fixed 2026-06-01 |
| KI-2 | `nest test` flaky / hangs when parallel tests share heavy global lookups | ✅ fixed 2026-05-29 |
| KI-1 | multi-thread scheduler race: green processes can't resolve globals | ✅ fixed 2026-05-29 |

**No open issues.** Every KI above is fixed, incidentally fixed, or a non-reproducing
transient — each kept as a record with its regression test, so a recurrence is recognizable.

---

## KI-22 — concurrent registration silently loses registrations · **open, root cause found 2026-08-01**

**Symptom.** Intermittent failures of the whole in-language suite, ~1 run in 5. Seen so far:
`ability_test`'s "open extension" (`(esize [1 2 3])` returns `-1`, the `:default` impl, right
after its `:vector` impl was registered on the line above) and `modules_test`'s "provide records
a feature idempotently" (its own feature missing from `*features*` on the next line). Because
nextest retries, `make test` still prints `929 passed` with only a `FLAKY` marker — and
`make test | tail` discards the failing attempt, which is printed *above* the summary.

**Root cause: a lost update.** Every load-time registry is one global holding a whole map or
list, written as `(def *X* (assoc *X* …))` — a read-modify-write. Two processes registering at
once each read the old value and each write their own successor; the later write silently drops
the earlier one. Reproduce with `scripts/fuzz/stress/registry_race.blsp`: N processes, one
**private** ability each (so nothing is a legitimate precedence contest), measured
**24/50, 88/200, 218/500 lost — about 40%**. It is not a dispatch-cache bug; the cache was never
involved.

**Fifteen registries share the shape**, so multimethod registration has the identical bug to
ability registration: `*record-ids*`, `*features*`, `*module-docs*`, `*deprecation-seen*`,
`*abilities*`, `*ability-owner*`, `*op-ability*`, `*ability-derives*`, `*sealed*`,
`*ability-requires*`, `*multi-algebra*`, `*methods*`, `*method-from*`, `*impls*`, `*impl-from*`.

**Why it matters past the test suite.** `impl` is hot-reloadable by design, so a registration
dropped by a concurrent one leaves an op dispatching to `:default` — silently and permanently.

### Two fixes tried and REVERTED — read before attempting a third

1. **Optimistic retry** (write, re-read, retry if our key vanished). Cut the loss from ~44% to
   ~20% but cannot close the window — every retry has the same read-write gap. A partial fix for
   a wrong-answer bug is worse than none: it just makes the flake rarer and harder to find.
2. **A ticket lock on `table-incr`** (the one atomic read-modify-write the language has). This
   *did* reach `LOST=0` at 50/200/500/1000 and took the suite from failing on run 8 of 10 to
   10/10 clean under `nest test` — and then made `make test` **worse**: the 120-process
   regression test took **157 s** of burnt CPU and still lost one, because the wait was a bounded
   busy-spin that heavy load blows straight through, after which the waiter proceeds
   unsynchronised. Adding `sleep` between checks fixed the CPU burn but exposed the next flaw:
   a waiter that times out never bumps `:served`, which desynchronises the ticket sequence
   permanently, so every later registration pays the full timeout (20 s constant, regardless of
   N). Reverted.

**What a real fix probably looks like.** A **registrar process** — registration becomes a
synchronous call to one process that performs the write single-threaded. No spinning (a blocking
receive parks the caller and costs a worker nothing), no timeout, no desync, and one writer means
zero lost updates by construction. It is the "a process holding state in its loop" option
`CLAUDE.md` names for mutable state. The open question is **bootstrap**: registrations happen
during prelude load, so the registrar has to exist first, and spawning it lazily is itself a
race. Note the registries cannot simply become `Table`s — dispatch reads `*impls*` on every call
and a table deep-clones values in and out, which would put a closure copy on the hot path.

**Reproduce:** `N=500 brood scripts/fuzz/stress/registry_race.blsp` (fast, deterministic — do
not use the suite flake, which is slow and 1-in-5). For the suite itself, `make test` is the
harsher environment; ten clean `nest test` runs proved nothing.

## KI-20 — a JIT fast link ran the callee against the **caller's** IC block · **fixed 2026-07-30**

`jit_run_fast_link` set `jit_call_env`, `jit_dbg_fn`, `jit_native_depth` and the stack limit
before entering the callee's native code — but not `ic_bases`, which the cloning native-link
path in `jit_dispatch_call` *does* set. `FastLink` (the IR-readable fast-path mirror) carried
no callee block, unlike `CallIcEntry`. So callee B ran with arm A's `cur_ic_base`/`cur_gic_base`,
and B's `vm_call_ic_put` / `vm_global_ic_put` / `vm_fast_link_publish_native` wrote into A's
slots, and vice versa.

**Never a wrong answer.** Every read path re-validates `(sym, argc, epoch)`, so a crossed
entry simply misses. The cost was that both arms ran with a permanently cold cache; it also
made `dbg_site_loc` and `[jit-staged-stale]` report the wrong source position.

**The fix (the one the earlier reverted attempt spelled out).** The callee's block bases now
**ride in the `FastLink` slot** — `_pad` became `callee_ic_base`, and a `callee_gic_base` was
added (the struct grows one 8-byte slot). `vm_call_ic_fast_link` stamps them from the entry's
already-resolved `CallIcEntry::callee_bases` (no `vm_arm_block` call on the publish path, no
lookup on the memoised hot path), the IR loads them alongside `code`/`nslots`/`env` (two extra
`u32` loads from the same cache line) and passes them as two more args to `brood_rt_fast_frame`,
and `jit_run_fast_link` does `set_ic_bases(callee_bases)` around the native call and restores —
exactly as the cloning path already did. So the runtime callback **never re-reads the table**:
the whole install is two `Cell` writes off values it was handed. A native flat cell carries
`(0, 0)` (a builtin runs no IC-using arm).

**Why the first attempt regressed and this one doesn't.** The reverted version read the bases
inside `jit_dispatch_fast_frame` via a `RefCell` borrow + a bounds-checked index **per call** —
on the very path whose purpose is to skip the IC probe, and dominated by self-recursion where
the install is a no-op — costing `bintree` +5.5% for no gain. Riding the bases in the slot adds
no lookup: a pinned best-of-9 A/B (`fib`, `bintree`) measured **+0.0%** against a +0.0%
base-vs-base floor.

**Guard.** A debug cross-check in `jit_dispatch_fast_frame` asserts the IR-passed bases equal
what the authoritative IC resolves (`b == callee_bases`), so a future mirror desync trips in
debug across the whole suite. The correctness net is the full differential + jit suites (the
change can only make a callee read its *own* caches, never produce a wrong value).

---

## KI-19 — the VM resolved a call's free-global head **after** its arguments · **FIXED 2026-07-30**

**Fixed.** The tree-walker evaluates the operator first; the VM elided a free-global head
and resolved it through the call-site IC *after* the arguments ran, so an argument that
rebound the head made the engines disagree — `(f (bump))` gave `:new` on the VM and `:old`
on the tree-walker.

`Inst::Call` now carries a `staged` flag. The compiler stages the head (as a `GlobalIc`,
resolved at IC speed) ahead of the args for exactly the calls that can be affected: a
**rebindable** global head with at least one argument that can run user code. `head` and
`site` stay populated, so the call-site IC still caches the resolved **arm** — validated by
closure identity against the staged callee, one compare on the common path. The JIT treats
a staged head as a computed callee (`head: None`, `site: NO_SITE`) so it calls the staged
value rather than re-resolving.

Three things had to be got right, and each was learned by measuring a wrong version:

* **Don't demote to the plain computed-callee path.** What the elided head really buys is
  the IC's cached *arm*, not the callee lookup: dropping it cost `json` 168 → 1159 ms (6×).
* **`head: None` with a live `site` aborts the JIT.** `emit` decides elided-vs-staged from
  the callee node while `jit_lower` decides it from `(head, site)`; a mismatch made the JIT
  resolve a head that wasn't there — `json`/`bintree`/`nqueens`/`wordcount` all died in
  `brood_rt_call_slow` → `unbound_error` → non-unwinding panic.
* **Exempt reserved names.** `def` refuses a name the language ships (ADR-166), so its
  resolution cannot change mid-call and the elided head stays correct. Staging every call
  regressed `regex` 31%, `wordcount` 11% and `sieve` 9% — almost entirely `first`/`rest`/
  `str`-class calls that were never rebindable.

Rows flat on an idle machine: `json` −3.6%, `bintree` −2.0%, `fib` +0.4%, `nbody` +0.1%,
and the three largest movers at baseline on a solo re-run (`regex` 95.7 ms, `wordcount`
50.6, `sieve` 49.7). `Inst::Call`'s stale doc comment claiming eval order was unchanged is
gone with the change that made it false.

Regression test: `tests/vm_call_head_order_test.blsp`.

---

## KI-18 — a deopt could re-run a `table-put` · **FIXED 2026-07-30**

**Fixed.** Four distinct paths let a JIT deopt re-execute an effect; the first three were
closed earlier the same day (no journal for a call-free effectful arm, a multi-arity
self-call exemption that ignored argc, and the leaf inliner splicing an effectful callee
into an engine that cannot journal). Those made the corruption unbounded and proportional
to the workload — 200 000 iterations put 402 047 times.

The fourth was the residual recorded here, and it is now fixed too: **`jit_run_fast_link`
treated an IC re-probe miss as "give up and let the caller redo the call"**. By then the
callee's native code had already run, so the caller's `brood_rt_call_slow` executed it a
second time — effect included. It showed up as a bounded over-count of exactly 16 (the
`DEOPT_BAIL_CONSECUTIVE` threshold, after which the arm bails and the duplication stops),
independent of iteration count. The IC probe is only an *optimisation* for locating the
arm, so a miss must not change behaviour: it now resolves the arm by name
(`env_get` → `compiled_arm_for`) and takes the same checkpoint-resume path as a hit.

Found by elimination, after instrumenting every re-run path: the deopt paths all resumed
correctly at their journal, `[fl-fallthrough]` was the one that fired 16 times, and
counting entries to *each arm separately* showed the caller was re-entered too — which is
what pointed above `f` rather than inside it.

Also hardened while here: the checkpoint resume now covers **preempt** (outcome 2), not
just deopt. A preempt normally lands on a back edge where the journal is 0, so this is a
no-op there; but if one ever landed after a completed call or a `table-put`, the ip-0
re-run would have repeated it.

Regression tests: `tests/jit_effect_once_test.blsp`, four shapes including the multi-arity
one, each asserting the effect count equals the iteration count exactly.

---

## KI-17 — `nest check` validated qualified names against ITS load set, not per-file reachability · **FIXED 2026-07-30**

**Fixed (ADR-189).** `check-file` now takes a per-file **reachability set** — the
file's *transitive* require-closure — and flags a **user-written** qualified
reference `mod/name` whose `mod` is outside it (bound in the image only by load-order
luck). `std/tool/project.blsp` builds the module→direct-requires graph once
(`%module-direct-requires`, one native parse per file), closes it transitively per
file, and threads each file's set through the fresh / cached / structured check paths
(carried as data in the parallel chunks). Soundness — the checker's cardinal rule —
held zero false positives across the whole `std/` + `tests/` sweep: the transitive
closure clears legitimately-transitive references (a test that `(:use editor/treesit)`
naming `face/…`, since treesit requires `editor/face`), the guard restricts the lint
to references the user *literally wrote* (never a macro-injected one), and a genuine
circular case (`coverage` naming `project/…`, which can't `(require 'project)` without
a cycle) is resolved by a *lazy* runtime `(require 'project)` in the one function that
uses it. Regression tests: `ki17_flags_a_qualified_reference_to_an_unrequired_module`
(Rust) and the end-to-end `nest check` on a project with a bad reference. Suppress a
deliberate exception with `(check-allow :unrequired form…)`. Original report:

**Symptom.** A qualified call to a module the running program never loads —
`(path/basename f)` in a file whose module neither `(:use path)` nor
`(require 'path)` — passed `nest check` clean, then raised
`unbound symbol: path/basename` at runtime. Hit twice in one day downstream
(myedit: `path/basename`, then a cross-module form value with the same shape).

**Cause.** `check-project` loads the whole project image (sources + test files)
before checking; any file's `require` binds the qualified name image-wide, so the
checker resolved it for EVERY file — including files whose own load graph never
pulls that module in. The check answered "bound in the check image", not "bound when
this module is reachable".

**Workaround (pre-fix).** Discipline: every qualified `mod/…` call needs a
`(require 'mod)` in that file (a bare require avoids `(:use …)` import shadowing,
e.g. std path's `join` vs the prelude's) — which the lint now enforces.

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

## KI-16 — the LSP still matches the retired `defprotocol`/`defimpl` · **FIXED 2026-07-27**

**Fixed:** `introspect::protocol_ops` now scans both `*abilities*` and `*protocols*`, so
`impl` op-completion works and `:implements` hovers still resolve; `completion.rs`'s
`enclosing_impl` matches `(impl …)` (was `(defimpl …)`); `definition.rs`/`module_ref.rs`
match `"defbehaviour" | "defability"` (was `| "defprotocol"`). Regression:
`offers_ability_ops_inside_impl`. Original report:


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

**Residual — the canary's timeouts were too tight for the grown suite (2026-07-29).** The
hang is fixed, but the guard tests began hard-killing at the framework's 120 s per-test
ceiling under the *grown* suite (3591 → 3800+ tests). Not a regression — a false timeout: the
guard-page fix now makes deep recursion DEOPT to the VM and raise the catchable
`MAX_BC_FRAMES` error (→ the parse returns `:rejected`), so the worker is *slow*, never dead.
With the drain armed (the whole suite's loaded code arms it — the very condition under test)
one parse costs ~36 s of CPU (measured at `BROOD_RT_GC_FLOOR=1`), and `:serial` serialises
only *within* the group, so the warm passes + spawned parse compete with the rest of the
suite for cores; the whole-test wall-clock overran 120 s (the suite still *completed* — no
guard-page abort, the real KI-14 symptom). Fixed by tagging the group `:slow` (the
`*test-slow-timeout-ms*` = 900 s batch budget the conformance corpora already use for work
that is long, not stuck) and widening the in-test `after` liveness nets to 850 s (a stuck
worker is still caught, cleanly, under the ceiling). The warm passes were kept — they are
load-bearing (the spawned parse must hit the WARM native path, the condition the canary
guards). The *underlying* residual — a deep-recursing process still pays a
throttled-but-O(depth) Phase-1 drain-report cost — is bounded and left for a separate,
ADR-gated collector-perf pass if it ever bites outside this canary.

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

## KI-13 — cross-module return-type inference blows up exponentially in branch count · **FIXED 2026-07-27**

**Fixed — it had TWO independent causes, both now addressed.**

*Cause 1 — type size.* Inferring an undeclared recursive value-builder's return unions
branch results into a `Ty` with a deep, `Arc`-shared refinement DAG; `==`/`Hash`/
`is_subtype` (recursive, no sharing dedup) walk it as a tree → superlinear per op.
`Ty::bounded` now widens any `Ty` whose refinement tree exceeds `MAX_TY_NODES` (64) to its
flat tag set — a sound over-approximation — applied at `union`, `intersect`, and every
structural constructor (`seq_of`/`map_of`/`record_of`/`tuple_of`), so no `Ty` ever gets
large. Fixed the moderate case (4-branch `deriv`: 25 s → 0.3 s). Regression:
`inferred_type_size_is_bounded_ki13`.

*Cause 2 — walk count.* With size bounded, the full 5-branch `deriv` was still dominated by
exponential **re-inference**: `sig_of`/`infer_sig` had no result cache, so a callee's body
was re-walked on every request (~400k walks), and because the cycle guard is slipped by a
self-call stored as a bare name vs the qualified name a caller resolved, the re-walks
compounded. `infer_sig` now **memoizes** completed inferences per check pass (`SIG_MEMO`,
cleared per `check_file`), capping it at one walk per distinct name. Full 5-branch `deriv`:
>900 s → 1.1 s.

Both are sound (widening / a deterministic memo for a read-only pass): 255 checker tests
plus the full app ecosystem (hatch 750, brood-chat 102, hatch-demo 89, …) unchanged. The
`(sig deriv (any -> any))` in the corpus is no longer load-bearing (kept as API hygiene).
Original report:


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

## KI-21 — `nest run --for` / `--watch` emit a pre-ADR-150 `~p` pin

**Status:** ✅ **fixed** 2026-07-30 (`~p` → `^p`). Found while smoke-testing a
TUI/GUI app end to end.

**Symptom.** Any `nest run --for <duration>` or `nest run --watch <path>` fails
immediately, whatever the file:

```
$ printf '(println "hi")\n' > /tmp/t.blsp
$ nest run /tmp/t.blsp --for 1s
error: match: `~p` is not a pattern — a pin is written `^p` (`~` belongs to
quasiquote alone). To match the literal 2-element list (unquote x), quote the
head: ('unquote x).
    at error / match-compile-velems ×3 / match-compile-vector /
       match-build-clause / receive
```

Plain `nest run <file>` is unaffected — only the wrapped path.

**Cause.** `crates/nest/src/main.rs:1203` builds the wrapper source as a string:

```rust
"(let (p (%spawn (fn () {}))) \
      (monitor p) \
      (receive ([:down _ ~p reason] (println \"[exit]\" reason)) {}))",
```

`~p` was the pin syntax before **ADR-150** moved pins to `^p` and made `~`
quasiquote-only. The generated `receive` therefore fails to compile at runtime.
Because the wrapper is only emitted when `timed.is_some()` or `--watch` is set,
nothing in the normal `nest run` path exercises it — and since the code is a Rust
string literal, neither `nest check` nor the Brood test suite can see it.

**Fix.** `~p` → `^p` in that format string. Verified: `nest run --for 800ms` over a
trivial file now exits 0 and prints `[stopped after 800ms]`.

**Guard — still owed.** A test that actually *runs* `nest run --for 200ms` and asserts
a zero exit. The general lesson is the durable part: **Brood source generated from Rust
string literals is invisible to `nest check` and to the in-language suite**, so every
such snippet needs an execution test rather than a reading. Worth grepping for the
others.
