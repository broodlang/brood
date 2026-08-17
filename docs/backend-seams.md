# Backend seams — swapping the JIT, swapping the engine, and reading performance

> **Note on the background docs cited below.** `jit-tier2.md`, `jit-stage1.md`,
> `value-repr.md` and `frame-representation.md` were trimmed in `fdce5400` once the work
> they planned had shipped. Their conclusions are folded into this file, the ADR that
> cites them, or the source; the full text is recoverable from git history
> (`git show fdce5400^:docs/<name>.md`).


> **Status: ALL FIVE ITEMS LANDED (2026-08-11).** This began as the session roadmap and is now
> also the record of what was built. Where implementation contradicted the plan, the section
> says so rather than being quietly rewritten — items 2, 5 and the benchmarking doc each got
> a claim wrong, and the corrections are the most useful part of the file.
>
> Nothing here changes generated code or runtime behaviour: it changes *where the decisions
> live* and *how legible the machine is*. Items 1–2 are recorded as **ADR-221**; the narrative
> and the gate results are in `docs/devlog.md`.

## 0. Why structure and not compute, today

The compute frontier's cheap end is mined out. `ROADMAP.md` (§VM & JIT, 2026-07-24) records
the track as "at rest — its frontier is effectively mined out," and every lever still named
is a multi-session piece with a real invariant or risk attached:

| lever | row it moves | why it is not a today's-work item |
|---|---|---|
| X-register call convention | `bintree`, `fib` | a call-protocol redesign, not a knob (`brood-benchmarks/FRONTIER.md`) |
| unboxed floats across call boundaries | `mandelbrot`, `nbody` | "a much larger piece of work" (ROADMAP, 2026-08-04) |
| narrower cell representation | `bintree`, `nqueens` | spends a core invariant; brushes the NaN-box line `types.md` rejects |
| M2 shared IC tables | `pingpong`, per-process floor | "highest value and highest risk; needs a lock-free design plus TSAN/loom" (`runtime-frontier.md`) |

Starting one of those today means *starting* it. Meanwhile the swappability work is unusually
cheap **right now**, because the layering is already almost correct — most of it is making an
implicit contract explicit and compile-checked, not restructuring. And it is not a detour from
performance: the X-register redesign at the top of that table is precisely a rewrite of the
call protocol that item 1 pins down and item 2 hoists the decisions out of. Doing 1+2 first
makes the expensive item cheaper and safer, and it does so today.

## 1. The starting position — what was already a seam, and what only looked like one

This section is the state the work *began* from, kept as written; it is what the plan reasoned
about, and the "after" is in §2–§3. (Post-split the count is 10 files / 138 references, the
increase being `cranelift.rs` and `backend.rs` — the same code, now named for what it is.)

**Cranelift was already confined.** It was named in 8 files: 7 backend files carrying all 128
references, plus one comment in `ir.rs`.

```
crates/lisp/src/eval/compile/jit_lower.rs           44 refs   2522 lines
crates/lisp/src/eval/compile/jit_lower/i64.rs       39         945
crates/lisp/src/eval/compile/jit_lower/emit.rs      33        1166
crates/lisp/src/jit/mod.rs                           6        1227
crates/lisp/src/eval/compile/jit_lower/{call,prim,control}.rs
                                                     2 each    549 / 916 / 185
crates/lisp/src/eval/compile/jit_lower/prepass.rs    0          108
crates/lisp/src/eval/compile/ir.rs                   1 (a comment)
```

So the backend is ~6.4 kLOC under `jit_lower*` plus the Cranelift-module owner at the head of
`jit/mod.rs`; `ir.rs` — `Node` / `Inst` / `Chunk` / `CompiledArm` / `PrimOp*` — is
Cranelift-free. **The IR is the seam, and it already holds.**

**The production invocation surface is two places.** `jit_runtime.rs` reaches the backend from
`jit_compile_now` (the synchronous path) and from the `JIT_COMPILER` background thread — three
calls in total, since the background one picks between the plain and inlined lowering — each
locking `crate::jit::GLOBAL_JIT`.
`crates/lisp/src/eval/compile/tests.rs` adds ~10 direct calls. That is the whole coupling.

**What is *not* a seam, and is the actual risk.** The contract a second backend must satisfy
exists only as prose across `docs/jit-tier2.md` / `docs/jit-optimizing-tier.md`, enforced by
tests alone. Six obligations:

1. **input** — a `CompiledArm` plus `slot_tags: &[u8]` (the tier-time slot type profile).
2. **output** — `extern "C" fn(heap: *mut Heap, base: i64) -> i64`, reading frame slots from
   `roots[base..]`, boxing its result into `roots[base]`.
3. **outcome codes** — `0` Done, `3` error, `1`/`2`/`4` deopt / preempt / tail.
4. **the `brood_rt_*` table** (`jit/mod.rs`) as the *only* legal heap or GC interaction.
5. **roots-only value discipline** — no `Value` in a register across a safepoint; unboxed
   `i64`/`f64` only, within a safepoint-free segment (`jit/mod.rs` module docs).
6. **epoch guard + sentinels** — `jit_code` is null (untried) / `BAILED` / `QUEUED` / a real
   8-aligned pointer; plus the deopt journal and resume-arm protocol (ADR-210).

**And the engine selector is a `bool`, not an abstraction.** `vm_enabled()` and
`set_forced_engine(Option<bool>)` in `eval/compile/mod.rs`: `Some(true)` VM, `Some(false)`
tree-walker, `None` defers to `BROOD_VM`. That is *fine* as far as it goes — and the reason
engine swapping is tractable here is not the selector but the **differential harness** around
it: both engines live in every binary, `make test-both` and a dedicated CI job
(`.github/workflows/ci.yml`) run the whole suite through each, and `tests/differential.rs`
fuzzes them against one another. The abstraction is worth little; the conformance suite is
worth a great deal. Items 1–3 are all shaped by that observation.

## 2. Item 1 — `trait JitBackend`, `CraneliftBackend` as the sole impl — ✅ done

**Goal.** Make the six obligations above a compile-checked target instead of prose, so a second
backend has something to implement and something to be tested against.

**Shape.**

**As built** (the plan first sketched a single `lower(&LoweringPlan)`; see §3 for why the plan
struct did not survive contact with the code):

```rust
// crates/lisp/src/jit/backend.rs — backend-independent
pub(crate) trait JitBackend {
    fn lower_arm(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8>;
    fn lower_inlined_arm(&mut self, arm: &CompiledArm, slot_tags: &[u8]) -> Option<*const u8>;
}
```

Two methods, not one, because the inlined variant is the *deferred second body* two-stage tiering
installs over the small one — a different arm shape with its own epoch check, not a flag on a
shared request. And no `name()`: the sketch had one "for traces", nothing called it, and per
ADR-011 an unused knob is a tax. Whoever adds a second backend adds what that backend needs.

**File moves.** `jit/mod.rs` currently holds two unrelated things: the Cranelift `JITModule`
owner (`Jit`, roughly its first 280 lines) and the `brood_rt_*` callback table (the remaining
~945 lines, which is backend-independent ABI). Split accordingly:

| new file | contents |
|---|---|
| `jit/mod.rs` | sentinels (`BAILED`/`QUEUED`), the backend selection, re-exports |
| `jit/rt.rs` | the `brood_rt_*` table — obligation 4, backend-independent |
| `jit/backend.rs` | the `JitBackend` trait + the obligations as doc-comments |
| `jit/cranelift.rs` | `CraneliftBackend` (today's `Jit`) — the `JITModule` owner |

`eval/compile/jit_lower*` becomes the `impl JitBackend for CraneliftBackend` body. Per the
greenfield rule in `CLAUDE.md`, rename outright and update callers — no alias kept.

**Backend selection, and why it is perf-neutral by construction.**

```rust
#[cfg(feature = "jit")]
pub(crate) type ActiveBackend = cranelift::CraneliftBackend;
pub(crate) static GLOBAL_JIT: LazyLock<Mutex<ActiveBackend>> = …;
```

A `#[cfg]`-selected type alias keeps dispatch static and monomorphic — no vtable anywhere.
The stronger argument is structural: **the hot path never touches the backend at all.** A
backend's entire output is a `*const u8`; everything after that crosses the `extern "C"` ABI
that already exists. There is no execution path on which a trait could cost anything, because
`lower` runs once per arm, on the background compiler thread, behind a `Mutex`. (A `dyn
JitBackend` would also be free for the same reason. The alias is chosen because it is simpler,
not because `dyn` would be slow.)

**The one hole, found by review and since closed.** `jit_runtime.rs` — the backend-*independent*
tiering glue — reached around this trait **four times** into `jit_lower/i64.rs`, the Cranelift
backend's unboxed-scalar submodule, to ask whether an arm was on the register worker. A second
backend would have found those calls meaningless. They are now three **tiering advisories** on
the trait:

| advisory | the question tiering is really asking |
|---|---|
| `may_adopt_shared_code(arm)` | may this arm adopt native code a *peer process* published? |
| `declines_inline_upgrade(arm)` | is your small native already better than the boxed depth-2 upgrade? |
| `note_depth_bail(name)` | outcome 5: you ran out of native stack — stop using that strategy |

All three are **associated functions, not `&self`** — deliberately. Tiering consults the first two
per activation, and `&self` would mean taking the `GLOBAL_JIT` lock there; that lock is
uncontended today precisely because only the background compiler takes it. The cost of that
choice is that `JitBackend` is no longer object-safe, which is recorded in its docs along with
what to do instead if runtime backend selection is ever wanted.

Each has a default (`true` / `false` / no-op), so a backend with no special strategies implements
nothing. Guarded by `tiering_advisories_route_to_the_predicate_they_name`, which exists because
the two Cranelift predicates are easy to confuse (`arm_i64_too_deep` = demoted? vs
`arm_i64_eligible` = takes the worker?) and **either swap compiles and passes every other test in
the tree** — the shared-code path would just quietly stop adopting, or a demoted function would
keep taking an upgrade it must not have. Verified by sabotage in both directions.

**A factual error in the contract, fixed at the same time.** Obligation 3 listed outcomes 0–4 and
omitted **5**, the depth bail — the very outcome `note_depth_bail` exists to service. A backend
written against the contract as first published would not have known to return it.

**Non-goal today: a second backend.** No LLVM, no Copy-and-Patch, no self-hosted assembler.
The deliverable is the seam and the conformance suite, not a user of them.

## 3. Item 2 — hoist the decisions above the backend — ✅ done

**This is where the value is.** The backend-independent decisions currently live inside
`jit_lower.rs`, interleaved with Cranelift builder calls. Those decisions are the repo's
expensive institutional memory, and each one has a measurement session behind it:

- the **call-mediated profitability gate** (inline in `jit_lower_arm`) exists because tiering
  the boxed-call shape *regressed* `nbody` 15–20%;
- exempting `mandelbrot`'s `row-sum` from that gate was measured 2026-08-04 and is **not** a
  win (+0.7% `mandelbrot`, +5.1% `matmul` against 0.3% floors) — a negative result a second
  backend would otherwise re-derive from scratch;
- float-global unboxing (ADR-related, `BROOD_NO_FLOAT_GLOBAL`) exists because an arm whose
  floats arrive from a `def`'d constant silently deopted on every activation until it bailed.

A second backend re-implementing `lower` while re-deriving *these* is the real hazard of a
swap — not the codegen, which is mechanical by comparison.

**What moves into a new `eval/compile/jit_plan.rs`** (all currently in `jit_lower.rs`):

| moves | what it decides |
|---|---|
| `chunk_in_jit_subset` | is this chunk lowerable at all — the bail rule |
| the profitability gate (inline in `jit_lower_arm`) | *should* a lowerable arm be lowered |
| `jit_spill_reserve`, `jit_ckpt_depth` | frame layout + deopt-checkpoint placement |
| `non_tail_call_count`, `inst_may_allocate`, `inst_allocates_hot` | the gate's inputs |
| `invariant_param_slots`, `invariant_global_vecs` | loop-invariant hoisting analysis |
| `collect_self_call_args` | self-tail-loop shape |
| `unbox_float_global`, `jit_i64_enabled` | unboxing eligibility |
| `inst_opcode_name` | the opcode fingerprint `BROOD_JIT_DUMP_IR` prints |
| the strategy choice | i64-register arm vs general lowering vs inlined upgrade |
| the opt-out flags | `BROOD_NO_{LEAF_INLINE,PARTIAL_LEAF,FLOAT_GLOBAL,JIT_COMPUTED,I64}` |

**Product — and a correction to this plan, from building it.** There is **no `LoweringPlan`
struct**. The design above assumed the general entry point threads many decisions into codegen;
reading it showed the opposite. `jit_lower_arm` makes exactly two choices before delegating —
is the scalar-register path enabled, and does the general lowering pass the profitability gate
— and *the order between them is load-bearing*. So the API is a bare gate:

```rust
pub fn plan_general_lowering(arm: &CompiledArm, slot_tags: &[u8]) -> Result<(), BailReason>
```

Bundling both into one value is worse than not having a struct: it invites a caller to consult
the gate first, which is wrong (see the trap below). Per ADR-011, a shape with no reader is a
shape that misleads — the struct is deferred until a second decision actually needs to travel.

**The module is two-tiered**, which the first `--no-default-features` build made obvious:

| tier | contents | gated? |
|---|---|---|
| frame layout the VM needs either way | `jit_spill_reserve`, `jit_ckpt_depth`, `non_tail_call_count`, `chunk_in_jit_subset` | **no** |
| `jit_plan::codegen` — what emitted code may assume | subset details, LICM analysis, alloc predicates, the profitability gate, `BailReason`, the dump/flag reads | one `#[cfg(feature = "jit")]` on the module |

Gating the module once beats eleven per-item attributes, and the import path becomes the
documentation: `jit_plan::codegen::…` at a use site says "this needs a backend to mean
anything".

**What the hoist actually removed.** `jit_spill_reserve` and `jit_ckpt_depth` were each defined
**twice** — a real version in the jit-gated `jit_lower`, and a zero/`None` stub in
`compile/mod.rs` — *and* `jit_lower` carried its own `#[cfg(not(feature = "jit"))]` copies,
which could never compile at all, since the module they sit in only exists when the feature is
on. Both decisions are frame layout, which the VM needs whether or not a backend exists, so one
ungated definition is not merely tidier — it is the correct shape. Four definitions became two.

**`BailReason` is reportable, and that closed a real blind spot.** A refusal used to be a bare
`None`, observable only as *absence* from `BROOD_JIT_DUMP_IR` — which reads identically for an
arm that was never hot, never tried, or lowered through the scalar-register path. That last case
was not hypothetical: **`jit_lower/i64.rs` emitted no `[jit-ir]` line at all**, so `fib` and
`pfib`, the arms it wins biggest on, were invisible to the one tool CLAUDE.md points at for "did
this arm ever lower?". Both are fixed: the scalar path now reports
(`scalar-register: i64|f64`), and `BROOD_JIT_BAIL_TRACE=1` names refusals.

**The trap this surfaced, recorded because it nearly landed.** The first version of the hoist
consulted the gate *before* trying the scalar path. The gate's predicate — a named `defn`, ≥1
non-tail call, no inline vector op, no self-tail loop — describes `fib`/`pfib` exactly. So they
would have silently stopped lowering: still *correct*, because they run on the VM, which means
the JIT≡VM differential stays green, `make test` stays green, and the lowering witness stayed
blind because the scalar path emitted nothing. **Only a benchmark would have moved.** That is
why the i64 dump fix belongs here rather than in item 4, and why the ordering now carries a
comment in both `jit_lower_arm` and `plan_general_lowering` explaining what breaks if it is
swapped.

## 4. Item 3 — `enum Engine` for the selector — ✅ done

- `bool` → `enum Engine { TreeWalker, Bytecode }`; `vm_enabled()` → `active_engine()`;
  `set_forced_engine(Option<bool>)` → `set_forced_engine(Option<Engine>)`. `BROOD_VM=0` still
  means `TreeWalker`.
- **`Engine::ALL` and `Engine::short()`** are what actually generalize the harnesses.
  `benches/eval.rs` built its grid from a *local* `Eng { Vm, Tw }` enum and
  `tests/{differential,gabriel_engines}.rs` each hardcoded the pair; all three now iterate
  `Engine::ALL`, so a new engine gets benchmark rows and inherits the whole differential and
  the Gabriel corpus without either file being edited. `scripts/bench_ratio.py` was
  two-engine by construction (`(Vm|Tw)` regex, exactly one pairing) and now reports every
  engine against the reference as its own column.
- **Seven `vm_enabled()` sites collapsed to one decision.** Three were the identical
  `if vm_enabled() { compile::run } else { eval::eval }`, now `run_on_active_engine`. Two
  legitimately differ and keep their own dispatch (a top-level `def` tags the error with its
  RHS position; `vm_run_bc`'s carve-out is an inverse check on the whole form). `apply_engine`
  and the `%range-reduce`/`%vector-reduce` callback routing became exhaustive `match`es, so a
  third engine cannot silently collapse to "not the VM" and tree-walk every element.

**The honest limit, stated in the enum's own docs.** The selector was never what coupled an
engine. `ir.rs` is shared by both engines, the JIT, *and* the deopt/journal protocol, so
"swap the VM" means replacing `exec_chunk.rs` + `vm_run_bc.rs` (~2 kLOC) while keeping that
IR — a register VM, threaded code, or computed-goto dispatch all fit; a different IR does not.

**A wrong claim found and corrected while doing this.** `docs/benchmarking.md` said size `N`
appears as *neighbouring* `(Vm, N)` / `(Tw, N)` rows. Divan sorts rows by label when printing,
so it never did; what makes the ratio load-robust is that both are measured **in one process**,
and `bench_ratio.py` pairs by `(bench, size)` regardless of print order. The doc now says that.

## 5. Item 4 — one-command perf triage — ✅ done

- **`make perf-brood`** — the counter-armed build, with `release-brood`'s exact flags plus
  `brood/perf-stats`. A target rather than a documented command line precisely so the flags
  cannot drift from the build it is compared against. (Arming counters in the *default* build
  stays unbuilt: relaxed atomics on the hot path need measuring first, and
  `docs/benchmarking.md` is explicit that the counting tool must stay separate from the timing
  tool.)
- **`std/tool/perf.blsp`** (a DEV module) — `(perf/report)`, `(perf/summary)`,
  `(perf/measure thunk)`, carrying §2's reading rules so they need not be recalled.
- **`brood --debug-flags`** — the `BROOD_*` catalogue (`crates/lisp/src/debug_flags.rs`),
  grouped by attribution / JIT / optimizer opt-outs / GC / scheduler / engine, with a
  dependency's flags (`MIMALLOC_PURGE_DELAY`) marked `[not brood's]`. A curated performance
  subset on purpose; CLAUDE.md's table stays the long form because it carries the measurement
  history behind each default.
- **`(vm-stats-reset)`** — a new builtin, because the counters had no way to be zeroed from
  Brood at all (`perf::reset` existed and had no caller).

**Building this found three ways the obvious version of it lies.** Each is now a property of
the module rather than a footnote, and each was caught by running the thing, not by reading it:

1. **The counters are cumulative from process start, so a short program's report is mostly
   *boot*.** The same list-building program read an **84% defer rate on a cold boot cache and
   0.8% on a warm one** — boot is macro-expansion-heavy and expansion defers to the tree-walker.
   Without a reset that is indistinguishable from a property of the program. Hence
   `(perf/measure thunk)`, which zeroes first and reports on the region.
2. **Allocation cannot be normalised by activations.** Once an arm tiers to native, its
   iterations stop being counted while its allocations keep being counted (through
   `brood_rt_cons`): a 200k-iteration loop measured `:alloc` 200017 against `:vm-apply` 197 — a
   ratio of 1025 that says nothing. So `:allocations` is reported as a **count**, and
   **`:alloc-bound` is deliberately absent from the verdict** even though §2 names it as one of
   the three. The honest answer is to read allocations against a second run, or sweep
   `BROOD_GC_FLOOR`. A tool that guessed would label every JIT'd tail loop alloc-bound.
3. **A rate needs a minimum sample count.** A region whose hot loop runs natively leaves a
   handful of IC probes behind; 0 hits out of 12 is a 0% hit rate that meant `:dispatch-bound`
   for a perfectly healthy native loop. `verdict` now refuses to judge a rate resting on fewer
   than `min-samples` (1000) and says which counts were short, so "cannot tell" is a reportable
   answer rather than a wrong label.

**And the drift guard was vacuous until sabotage caught it.** The test asserting every
catalogued flag exists in the source scanned `crates/`, which *contains* `debug_flags.rs` — so
every name found itself and the assertion always passed. Renaming a flag still passed. It now
skips its own file, and the sabotage was re-run to confirm the test fails when it should. That
check also immediately surfaced a real category: `MIMALLOC_PURGE_DELAY` is the allocator's flag,
not brood's, and is exempt by an explicit `ours: false` rather than by being quietly dropped.

Tests: `tests/perf_test.blsp`, 16 cases, green on **both** a counter-armed and an ordinary build
— written as implications precisely so the no-counters path (the one that must never report
zeros as measurements) is the case pinned hardest.

## 6. Item 5 — a perf verdict that is defensible — ✅ done, with one part deliberately not built

`scripts/ab-bench.sh` gains two things:

- **`--json <path>`** — the sweep as machine-readable rows
  (`{row, base_ms, new_ms, delta_pct, floor_pct, verdict}`), so a tool or an LLM reads results
  instead of re-parsing the human table.
- **`--floor`** — each row's own **noise floor**, by running the baseline binary a second time
  between the two sides and taking the base-vs-base spread. The `verdict` column then calls a
  regression only when the delta clears `max(5%, 2 × floor)`. This is CLAUDE.md's own
  prescription, and it exists because a +5.3% "confirmed" regression (2026-07-29) was a
  baseline that had wandered ~10% across the day; the same change read +0.9% against a +0.5%
  floor — neutral.

**What was NOT built, and why.** The plan called for a *committed baseline keyed by release
tag*. That would be a false reference: absolute ms drift 10–20% between runs on this box and
do not compare across machines at all (`docs/benchmarking.md` §1, and the `pingpong` episode
above). A stored number invites exactly the comparison the repo already knows is invalid. The
comparison has to be against a binary built here, now, from the same target — which is what
the baseline worktree exists for. `--floor` is the part of "a lookup, not a project" that
actually holds; the stored-baseline half is left as an open question rather than shipped
misleading. Still deliberately not a blocking CI gate: per-row drift would make a hard
threshold a flake generator.

## 7. Gates — what was actually run

**Per increment, for items 1 and 2**, because moving-GC JIT codegen is the one place where
"it only moved code" is the claim that needs checking rather than asserting:

```bash
cargo test --features jit --test jit                        # JIT≡VM differential — 40/40
BROOD_GC_STRESS=1 BROOD_GC_VERIFY=1 \
  cargo test --features jit --test jit                      # again under tripwire + verifier — 40/40
make test                                                   # 974/974 (item 1), 974/974 (item 2)
cargo check --workspace --no-default-features                # the jit-seam honesty check
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
scripts/jit-lower-witness.sh                                # the lowering witness (below)
```

**Across items 3–5:** `make test` **974/974** (item 3) and **976/976** (items 4–5; the two new
tests are `debug_flags`'s drift guards); `make test-both` **974 + 974 = 1948/1948**;
`cargo test --test differential --test gabriel_engines --test jit`; `tests/perf_test.blsp`
**16/16 on both a counter-armed and an ordinary build**; the `BROOD_VM` truthiness table verified
behaviourally (`0`/`false`/`off`/`no`/empty → tree-walker, everything else and unset → bytecode,
observed via whether any arm tiers at all).

**Deliberately NOT run this session, and still owed before a release:** `scripts/fuzz/run.sh`
(the generators × engine-configs differential) and the GC_STRESS sweep over the seven
concurrency binaries — both last green on `afe4bcff`, and both listed in `docs/handoff.md` as
outstanding. `make suite` was not run standalone; it runs inside `make test`, which passed.

### The lowering witness

For items 1–2 specifically, **`scripts/jit-lower-witness.sh`** must produce the same set before
and after. It runs 13 benchmark rows under `BROOD_JIT_DUMP_IR=1` and prints the sorted,
de-duplicated set of arm fingerprints — `(name, ckpt/strategy, opcode sequence)`.

The *count* is not usable: installation is asynchronous, so a marginal arm may or may not land
before a run ends (measured ±2 on a 78-lowering sweep). The *set* is deterministic — verified
byte-identical across three passes before any change was made. Item 1's diff was empty; item 2's
was **0 removed, 2 added**, the two additions being `fib` and `pfib` becoming visible for the
first time. Item 3 diffed empty again.

This is the only gate that sees "an arm quietly stopped lowering". Every other gate checks the
*answer*, and the VM's answer is also correct.

### Independently verified after the fact

- Every hoisted function body compared against the original: **9 of 11 byte-identical, the other
  two identical modulo rustfmt reflow** (the extra `mod codegen` indent rewrapped a match arm).
- Every collapsed engine call site read against its original; `run_on_active_engine` is exactly
  the `if`/`else` it replaced, and the two sites that differ kept their own dispatch.
- The drift guard in `debug_flags.rs` **verified by sabotage** — and found vacuous on the first
  attempt, because it scanned `crates/`, which contains the catalogue itself.

**Explicit non-goals.** No second JIT backend. No third engine. No change to generated code,
tiering thresholds, bail rules, or the profitability gate.

## 8. What this unblocks

The X-register call convention — the `bintree`/`fib` lever, and the one item on the frontier
that is a redesign rather than a knob — is a rewrite of the per-call protocol described in
`jit-optimizing-tier.md` §1. That protocol is obligations 2, 3, and 5 of the contract item 1
makes explicit, and the decisions about *when* a call is worth lowering are what item 2
hoists. Today's work is the interface that redesign gets to be checked against.
